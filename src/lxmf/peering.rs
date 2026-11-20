//! Peering out: the propagation node hosted here sends the messages it
//! holds on to other propagation nodes, as Python's `lxmd` does, so they
//! spread rather than wait here for their recipient.
//!
//! - **Who:** nodes heard announcing within *Auto-peer depth* hops, up to
//!   *Most peers* of them (`autopeer`), and the *Static peers* named in the
//!   settings, whatever their distance. A node that stops serving, or moves
//!   out of range, is no longer peered with (static peers stay).
//! - **How:** every so often, one peer with messages it hasn't had is
//!   synced: a Link to it (identified), an `/offer` of what it hasn't had
//!   with a peering key (proof of work over both identities, at the cost it
//!   announces), and the messages it wants as one Resource
//!   (`[timebase, [message, ...]]`). What it has or took isn't offered
//!   again.
//!
//! rsLXMF keeps each peer's state (`LxmPeer`, saved as `<hash>.peer` beside
//! the messages) and picks what to offer; this runs the Links.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lxmf_core::constants::{MAX_PEERING_COST, OFFER_REQUEST_PATH, PeerError};
use lxmf_core::handlers::parse_pn_announce_data;
use lxmf_core::peer::{LxmPeer, OutboundOfferPolicy};
use lxmf_core::propagation_node::{PropagationNode, read_planned_messages};
use lxmf_core::stamper::StampControl;
use lxmf_core::types::PropagationTransientId;
use rmpv::Value;
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use super::encode;
use crate::net::Hash;

/// How often a peer is picked to sync with (as often as `lxmd`).
const SYNC_EVERY: Duration = Duration::from_secs(24);
/// The longest a whole sync may take (a path, a Link, an offer and the
/// transfer), so a peer that stalls can't hold up the others for long.
const SYNC_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// The longest an offer, or the transfer after it, is waited for.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(600);

/// Who to peer with, from the settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeeringConfig {
    /// Peer with nodes heard announcing, within `max_depth` hops.
    pub autopeer: bool,
    pub max_depth: u8,
    /// Most nodes peered with (static ones count too).
    pub max_peers: usize,
    /// Nodes always peered with.
    pub static_peers: Vec<Hash>,
}

/// The most nodes whose distance is remembered.
const MAX_HEARD: usize = 4096;
/// The most peers listed one by one in [`PeeringStats::rows`].
pub const ROWS_MAX: usize = 256;
/// The longest peer name kept, in bytes (it comes from the network).
const NAME_MAX: usize = 256;

/// A propagation node's announce, as peering needs it.
#[derive(Debug, Clone)]
pub struct HeardNode {
    pub hash: Hash,
    pub identity: Option<[u8; 16]>,
    pub hops: u8,
    /// An answer to a path request, not an announce of its own: not
    /// peered with on its strength (as `lxmd`).
    pub path_response: bool,
    pub app_data: Vec<u8>,
}

/// What became of one heard node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Peered,
    Updated,
    Unpeered,
    Unchanged,
}

/// The nodes peered with, and their identities (for peering keys).
#[derive(Debug, Default)]
pub struct Peers {
    pub peers: HashMap<Hash, LxmPeer>,
    identities: HashMap<Hash, [u8; 16]>,
    config: PeeringConfig,
    /// This node's own address: never its own peer.
    own: Hash,
    /// How far away each was when last heard announcing.
    hops: HashMap<Hash, u8>,
    /// Why each peer's last sync failed (not saved: it is for the stats page).
    failures: HashMap<Hash, Failure>,
}

impl Peers {
    /// The peers saved (`saved`), kept as the settings now say: autopeered
    /// ones only while auto-peering is on, static ones as listed.
    pub fn new(config: PeeringConfig, own: Hash, saved: Vec<LxmPeer>) -> Self {
        let mut peers: HashMap<Hash, LxmPeer> = saved
            .into_iter()
            .filter(|p| p.destination_hash != own)
            .map(|mut p| {
                p.is_static = config.static_peers.contains(&p.destination_hash);
                p.state = lxmf_core::constants::PeerState::Idle;
                (p.destination_hash, p)
            })
            .filter(|(_, p)| p.is_static || (config.autopeer && p.autopeered))
            .collect();
        for hash in &config.static_peers {
            if *hash != own {
                peers.entry(*hash).or_insert_with(|| {
                    let mut peer = LxmPeer::new(*hash);
                    peer.is_static = true;
                    peer
                });
            }
        }
        Self { peers, identities: HashMap::new(), config, own, hops: HashMap::new(), failures: HashMap::new() }
    }

    /// An announce heard from a propagation node, taken as `lxmd` takes it.
    pub fn heard(&mut self, node: &HeardNode) -> Change {
        if node.hash == self.own {
            return Change::Unchanged;
        }
        let Some(data) = parse_pn_announce_data(&node.app_data) else { return Change::Unchanged };
        if let Some(identity) = node.identity {
            self.identities.insert(node.hash, identity);
        }
        if self.hops.len() < MAX_HEARD || self.hops.contains_key(&node.hash) {
            self.hops.insert(node.hash, node.hops);
        }
        let is_static = self.config.static_peers.contains(&node.hash);
        let peered = self.peers.contains_key(&node.hash);
        if data.peering_cost > MAX_PEERING_COST {
            // More work than any peering key here is worth.
            return if peered && !is_static { self.unpeer(node.hash) } else { Change::Unchanged };
        }
        let in_range = usize::from(node.hops) <= usize::from(self.config.max_depth);
        if is_static {
            // A path's answer only fills in what isn't known yet.
            if node.path_response && self.peers.get(&node.hash).is_some_and(|p| p.stamp_costs_known()) {
                return Change::Unchanged;
            }
        } else if !self.config.autopeer || node.path_response {
            return Change::Unchanged;
        } else if !data.node_state || !in_range {
            return if peered { self.unpeer(node.hash) } else { Change::Unchanged };
        }
        // What it says it takes in one sync is what's read into memory to
        // send it: no more than this node would take itself.
        let most = lxmf_core::constants::SYNC_LIMIT as f64;
        let limits = (Some(data.transfer_limit.as_f64().min(most)), Some(data.sync_limit.as_f64().min(most)));
        let costs = (Some(data.stamp_cost), Some(data.stamp_flex), Some(data.peering_cost));
        let metadata = Some(data.metadata.clone());
        let full = self.peers.len() >= self.config.max_peers;
        match self.peers.get_mut(&node.hash) {
            Some(peer) => {
                let updated =
                    peer.refresh_from_announce_with_metadata(data.timebase as f64, limits.0, limits.1, costs.0, costs.1, costs.2, metadata);
                if updated { Change::Updated } else { Change::Unchanged }
            }
            None if full => Change::Unchanged,
            None => {
                let mut peer = LxmPeer::from_announce_with_metadata(
                    node.hash,
                    data.timebase as f64,
                    limits.0,
                    limits.1,
                    costs.0,
                    costs.1,
                    costs.2,
                    metadata,
                );
                peer.is_static = is_static;
                peer.autopeered = !is_static;
                self.peers.insert(node.hash, peer);
                Change::Peered
            }
        }
    }

    fn unpeer(&mut self, hash: Hash) -> Change {
        self.peers.remove(&hash);
        self.hops.remove(&hash);
        self.failures.remove(&hash);
        Change::Unpeered
    }

    /// Note who a node is, from a key heard before (a static peer not yet
    /// heard this time).
    pub fn know(&mut self, hash: Hash, identity: [u8; 16]) {
        self.identities.entry(hash).or_insert(identity);
    }

    /// What is known of each peer, and totals over them. `live` says whether
    /// a message is still stored. `unhandled` adds up, over the peers, the
    /// stored messages each is still to be offered (one waiting for 20 peers
    /// counts 20); `unhandled_distinct` counts each such message once.
    pub fn summary(&self, live: impl Fn(&PropagationTransientId) -> bool) -> PeeringStats {
        let secs = |at: f64| (at.is_finite() && at > 0.0).then_some(at as u64);
        let mut stats = PeeringStats { peered: self.peers.len(), ..PeeringStats::default() };
        let mut rows = Vec::new();
        let mut distinct: HashSet<&PropagationTransientId> = HashSet::new();
        for peer in self.peers.values() {
            let mut waiting = 0u64;
            for id in peer.unhandled_messages.iter().filter(|id| live(id)) {
                waiting += 1;
                distinct.insert(id);
            }
            stats.alive += usize::from(peer.alive);
            stats.statics += usize::from(peer.is_static);
            stats.offered = stats.offered.saturating_add(peer.offered);
            stats.unhandled = stats.unhandled.saturating_add(waiting);
            let name = peer.metadata.as_ref().and_then(|m| m.get(&lxmf_core::constants::PN_META_NAME)).map(|bytes| {
                let end = bytes.len().min(NAME_MAX);
                String::from_utf8_lossy(&bytes[..end]).into_owned()
            });
            rows.push(PeerRow {
                hash: peer.destination_hash,
                name,
                is_static: peer.is_static,
                alive: peer.alive,
                state: state_name(peer.state),
                hops: self.hops.get(&peer.destination_hash).copied(),
                last_heard: secs(peer.last_heard),
                last_sync: secs(peer.last_sync),
                offered: peer.offered,
                unhandled: waiting,
                sync_failures: peer.unreachable_count,
                last_attempt: secs(peer.last_sync_attempt),
                next_attempt: secs(peer.next_sync_attempt),
                last_failure: self.failures.get(&peer.destination_hash).map(|why| why.as_str()),
                peering_cost: if peer.peering_cost.is_finite() && peer.peering_cost > 0.0 { peer.peering_cost as u64 } else { 0 },
            });
        }
        stats.unhandled_distinct = distinct.len() as u64;
        rows.sort_unstable_by_key(|row| row.hash);
        rows.truncate(ROWS_MAX);
        stats.rows = rows;
        stats
    }

    /// Note how a sync with `hash` went: why it failed, or (`None`) that it
    /// didn't.
    pub fn synced(&mut self, hash: Hash, failure: Option<Failure>) {
        match failure {
            Some(why) => self.failures.insert(hash, why),
            None => self.failures.remove(&hash),
        };
    }

    pub fn identity_of(&self, hash: &Hash) -> Option<[u8; 16]> {
        self.identities.get(hash).copied()
    }

    /// The peer to sync with next, if one has messages it hasn't had and
    /// is ready: its costs and identity known, its backoff over.
    /// `unhandled` gives the messages a peer hasn't had, from the ones it has
    /// and the least stamp cost it takes. Messages below that cost are left
    /// out: the peer will refuse them, so they are not waiting. This is worked
    /// out afresh each time (from the store and the peer's announced cost),
    /// so a peer that lowers its cost is offered them again, and a message
    /// that leaves the store is forgotten, with nothing more to keep or save.
    pub fn next(&mut self, unhandled: impl Fn(&HashSet<PropagationTransientId>, f64) -> HashSet<PropagationTransientId>) -> Option<Hash> {
        let now = now();
        let ready: Vec<Hash> = self
            .peers
            .values()
            .filter(|p| p.stamp_costs_known() && self.identities.contains_key(&p.destination_hash) && p.next_sync_attempt <= now)
            .map(|p| p.destination_hash)
            .collect();
        for hash in &ready {
            if let Some(peer) = self.peers.get_mut(hash) {
                peer.unhandled_messages = unhandled(&peer.handled_messages, peer.minimum_accepted_stamp_cost());
            }
        }
        // The one tried longest ago goes first (one never tried before any
        // other), so every peer is synced in turn. rsLXMF's own pick chooses
        // among the fastest peers, and rettui doesn't measure speeds, so it
        // would always choose the same two.
        ready
            .iter()
            .filter_map(|h| self.peers.get(h))
            .filter(|p| p.state == lxmf_core::constants::PeerState::Idle && !p.unhandled_messages.is_empty())
            .min_by(|a, b| a.last_sync_attempt.total_cmp(&b.last_sync_attempt).then(a.destination_hash.cmp(&b.destination_hash)))
            .map(|p| p.destination_hash)
    }
}

/// The longest a peer is left alone after failed syncs. Each attempt adds
/// 12 minutes to its backoff, and only a success or a newer announce resets it,
/// so a peer that was down for a while would otherwise wait hours after it is back.
const BACKOFF_MAX: f64 = 3600.0;

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// A sync with `peer` failed at `now`: it is tried again later, but not
/// later than [`BACKOFF_MAX`] from now.
fn failed(peer: &mut LxmPeer, now: f64) {
    peer.sync_failed();
    peer.sync_backoff = peer.sync_backoff.min(BACKOFF_MAX);
    peer.next_sync_attempt = peer.next_sync_attempt.min(now + BACKOFF_MAX);
}

/// Why a sync with a peer failed, in a few fixed words. Nothing a peer sent
/// goes into these: they are published on the stats page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// No Link to it could be made.
    NoLink,
    /// The Link came up but the offer got no answer.
    NoAnswer,
    /// It said it couldn't read the offer.
    RefusedInvalidData,
    /// It refused the peering key.
    BadKey,
    /// It doesn't let us peer, or couldn't tell who we are.
    NoAccess,
    /// It's busy.
    Busy,
    /// Something took too long.
    Timeout,
    /// It sent an answer, or an error code, not understood.
    BadAnswer,
    /// Making the key, or sending the messages, failed here or on the way.
    Io,
}

impl Failure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoLink => "no_link",
            Self::NoAnswer => "no_answer",
            Self::RefusedInvalidData => "refused_invalid_data",
            Self::BadKey => "bad_key",
            Self::NoAccess => "no_access",
            Self::Busy => "busy",
            Self::Timeout => "timeout",
            Self::BadAnswer => "bad_answer",
            Self::Io => "io",
        }
    }
}

/// What an `/offer` was answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offered {
    /// These are wanted (of those offered); the rest it has.
    Wants(Vec<PropagationTransientId>),
    /// Its peering key was refused: make a new one.
    BadKey,
    /// The sync failed, and why (the text is for the log, not the stats).
    Refused(Failure, String),
}

impl Offered {
    /// Why this failed, if it did.
    pub fn failure(&self) -> Option<Failure> {
        match self {
            Self::Wants(_) => None,
            Self::BadKey => Some(Failure::BadKey),
            Self::Refused(why, _) => Some(*why),
        }
    }
}

/// How an offer or a `/get` is sent: as the list Python's LXMF reads
/// (`[peering key, [transient id, ...]]`), not as bytes holding that list,
/// which it can't read.
pub fn offer_request_value(key: &[u8], ids: &[PropagationTransientId]) -> Value {
    Value::Array(vec![Value::Binary(key.to_vec()), Value::Array(ids.iter().map(|id| Value::Binary(id.to_vec())).collect())])
}

/// A failed request, as a [`Failure`].
fn request_failure(error: &LinkSessionError) -> Failure {
    match error {
        LinkSessionError::Timeout(_) => Failure::Timeout,
        _ => Failure::NoAnswer,
    }
}

/// Read a peer's answer to an offer of `offered`: `true` (all), `false`
/// (none), a list of those it wants (others aren't taken), or an error
/// code.
pub fn read_offer_response(data: &[u8], offered: &[PropagationTransientId]) -> Offered {
    match rmpv::decode::read_value(&mut &data[..]) {
        Ok(Value::Boolean(true)) => Offered::Wants(offered.to_vec()),
        Ok(Value::Boolean(false)) => Offered::Wants(Vec::new()),
        Ok(Value::Array(ids)) => {
            let asked: HashSet<PropagationTransientId> = ids.iter().filter_map(super::bytes_of).filter_map(|b| b.try_into().ok()).collect();
            Offered::Wants(offered.iter().filter(|id| asked.contains(*id)).copied().collect())
        }
        Ok(Value::Integer(code)) if code.as_u64() == Some(PeerError::InvalidKey as u64) => Offered::BadKey,
        Ok(Value::Integer(code)) => match code.as_u64().and_then(|c| u8::try_from(c).ok()) {
            Some(0xF0) => Offered::Refused(Failure::NoAccess, "it couldn't tell who we are".into()),
            Some(0xF1) => Offered::Refused(Failure::NoAccess, "it doesn't let us peer with it".into()),
            Some(0xF4) => Offered::Refused(Failure::RefusedInvalidData, "it couldn't read the offer".into()),
            Some(0xF6) => Offered::Refused(Failure::Busy, "it's busy: trying again later".into()),
            _ => Offered::Refused(Failure::BadAnswer, format!("error {code}")),
        },
        _ => Offered::Refused(Failure::BadAnswer, "an answer it couldn't read".into()),
    }
}

/// How a sync went: how many messages were offered and sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Synced {
    pub offered: usize,
    pub sent: usize,
}

/// `policy` with what a peer takes in one sync cut to what this node takes
/// itself, as [`Peers::heard`] does for an announce: what the peer wants is
/// read into memory to be sent, and a peer saved earlier, or announcing
/// no limit (which is none), would have everything read.
fn within_what_is_taken(policy: &mut OutboundOfferPolicy) {
    let most = lxmf_core::constants::SYNC_LIMIT as f64;
    for limit in [&mut policy.propagation_transfer_limit, &mut policy.propagation_sync_limit] {
        *limit = Some(limit.filter(|kilobytes| kilobytes.is_finite()).map_or(most, |kilobytes| kilobytes.min(most)));
    }
}

/// Sync with one peer, as its `policy` says: offer what it hasn't had,
/// then send what it wants. What it has, or took, is recorded as handled.
pub async fn sync_peer(
    runtime: &ReticulumHandle,
    identity: &Identity,
    node: &Arc<Mutex<PropagationNode>>,
    mut policy: OutboundOfferPolicy,
) -> Result<Synced, Offered> {
    within_what_is_taken(&mut policy);
    let offer = {
        let (node, policy) = (node.clone(), policy.clone());
        tokio::task::spawn_blocking(move || node.lock().unwrap().prepare_sync_offer_with_policy(&policy))
            .await
            .map_err(|e| Offered::Refused(Failure::Io, e.to_string()))?
    };
    let offered: Vec<PropagationTransientId> = offer.transient_ids.iter().filter_map(|id| id.as_slice().try_into().ok()).collect();
    if offered.is_empty() {
        node.lock().unwrap().complete_sync(&policy.peer_hash);
        return Ok(Synced::default());
    }
    let quiet = |_: String| {};
    let LinkSession { handle, .. } =
        crate::net::connect(runtime, policy.peer_hash, identity.clone(), crate::net::link_options("rettui.peer", true), &quiet)
            .await
            .map_err(|e| Offered::Refused(Failure::NoLink, format!("no Link to it: {e}")))?;
    let result = offer_on(&handle, node, &policy, &offer.peering_key, &offered).await;
    handle.close().await;
    node.lock().unwrap().complete_sync(&policy.peer_hash);
    result
}

async fn offer_on(
    handle: &LinkSessionHandle,
    node: &Arc<Mutex<PropagationNode>>,
    policy: &OutboundOfferPolicy,
    key: &[u8],
    offered: &[PropagationTransientId],
) -> Result<Synced, Offered> {
    let request = offer_request_value(key, offered);
    tracing::debug!(
        "peer sync with {}: offering {} messages, request sent as {}",
        hex::encode(&policy.peer_hash[..4]),
        offered.len(),
        kind_of(&request)
    );
    let response = handle
        .request_with_options(OFFER_REQUEST_PATH, request, RequestOptions { timeout: Some(REQUEST_TIMEOUT), ..RequestOptions::default() })
        .await
        .map_err(|e| Offered::Refused(request_failure(&e), format!("no answer to the offer: {e}")))?;
    let wanted = match read_offer_response(&response.data, offered) {
        Offered::Wants(wanted) => wanted,
        other => return Err(other),
    };
    // What it didn't want, it has.
    let has: Vec<PropagationTransientId> = offered.iter().filter(|id| !wanted.contains(id)).copied().collect();
    handled(node, policy, has).await;
    if wanted.is_empty() {
        return Ok(Synced { offered: offered.len(), sent: 0 });
    }
    let plan = node.lock().unwrap().plan_message_reads(&wanted);
    let messages = tokio::task::spawn_blocking(move || read_planned_messages(&plan)).await.unwrap_or_default();
    let sent_ids: Vec<PropagationTransientId> = messages.iter().map(|(id, _)| *id).collect();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let blobs = messages.into_iter().map(|(_, data)| Value::Binary(data)).collect();
    let payload = encode(&Value::Array(vec![Value::from(now), Value::Array(blobs)]));
    let transfer = handle
        .send_resource_bytes(payload, ResourceOptions { auto_compress: true, metadata: None })
        .await
        .map_err(|e| Offered::Refused(Failure::Io, format!("couldn't send the messages: {e}")))?;
    match tokio::time::timeout(TRANSFER_TIMEOUT, transfer.concluded()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(Offered::Refused(Failure::Io, format!("the transfer failed: {e}"))),
        Err(_) => return Err(Offered::Refused(Failure::Timeout, "the transfer timed out".into())),
    }
    let sent = sent_ids.len();
    handled(node, policy, sent_ids).await;
    Ok(Synced { offered: offered.len(), sent })
}

/// What a request's data is, for the log.
fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Array(_) => "list",
        Value::Binary(_) => "bytes",
        _ => "other",
    }
}

/// Record messages a peer has (or took), so they aren't offered again.
async fn handled(node: &Arc<Mutex<PropagationNode>>, policy: &OutboundOfferPolicy, ids: Vec<PropagationTransientId>) {
    if ids.is_empty() {
        return;
    }
    let (node, policy) = (node.clone(), policy.clone());
    let saved = tokio::task::spawn_blocking(move || node.lock().unwrap().persist_peer_handled(&policy, &ids)).await;
    if let Ok(Err(e)) = saved {
        tracing::warn!("could not save what a peer has: {e}");
    }
}

/// What peering has done, for the UIs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeeringStats {
    pub peered: usize,
    pub sent: u64,
    /// Peers that answered when last tried, and those named in the settings.
    pub alive: usize,
    pub statics: usize,
    /// Messages offered to all peers, and waiting to be offered.
    pub offered: u64,
    /// Stored messages waiting to be offered, added up over the peers: one
    /// waiting for 20 peers counts 20.
    pub unhandled: u64,
    /// The same, each message counted once.
    pub unhandled_distinct: u64,
    /// The peers one by one (the first [`ROWS_MAX`] by address).
    pub rows: Vec<PeerRow>,
}

/// One peered node, as the stats page shows it to whoever has the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRow {
    pub hash: Hash,
    /// Its announced name, as sent (not yet cleaned).
    pub name: Option<String>,
    pub is_static: bool,
    pub alive: bool,
    pub state: &'static str,
    pub hops: Option<u8>,
    /// Unix seconds it was last heard announcing, and last synced with.
    pub last_heard: Option<u64>,
    pub last_sync: Option<u64>,
    pub offered: u64,
    pub unhandled: u64,
    /// Syncs that failed since it was last heard, and Unix seconds of the last
    /// attempt and of the earliest next one (none if never, or due now).
    pub sync_failures: u32,
    pub last_attempt: Option<u64>,
    pub next_attempt: Option<u64>,
    /// Why the last sync failed, as a short fixed word (none if it didn't).
    pub last_failure: Option<&'static str>,
    pub peering_cost: u64,
}

fn state_name(state: lxmf_core::constants::PeerState) -> &'static str {
    use lxmf_core::constants::PeerState;
    match state {
        PeerState::Idle => "idle",
        PeerState::LinkEstablishing => "link_establishing",
        PeerState::LinkReady => "link_ready",
        PeerState::RequestSent => "request_sent",
        PeerState::ResponseReceived => "response_received",
        PeerState::ResourceTransferring => "transferring",
    }
}

/// Publish what is known of the peers (keeping the count sent, which is kept here).
fn publish(stats: &Mutex<PeeringStats>, peers: &Peers, node: &Mutex<PropagationNode>) {
    let summary = {
        let node = node.lock().unwrap();
        peers.summary(|id| node.contains(id))
    };
    let mut stats = stats.lock().unwrap();
    let sent = stats.sent;
    *stats = PeeringStats { sent, ..summary };
}

/// A finished sync: the peer, how it went, and the peering key made for it
/// with the cost it was made at.
type SyncDone = (Hash, Result<Synced, Offered>, Option<([u8; 32], f64)>);

/// Run peering for the node in `node` until `heard` closes: take
/// announces, and sync with one peer at a time.
pub async fn run(
    runtime: ReticulumHandle,
    identity: Identity,
    node: Arc<Mutex<PropagationNode>>,
    mut peers: Peers,
    mut heard: mpsc::UnboundedReceiver<HeardNode>,
    stats: Arc<Mutex<PeeringStats>>,
) {
    let save = |node: &Arc<Mutex<PropagationNode>>, peer: &LxmPeer| {
        if let Err(e) = node.lock().unwrap().save_peer(peer) {
            tracing::warn!("could not save a peer: {e}");
        }
    };
    let delete = |node: &Arc<Mutex<PropagationNode>>, hash: &Hash| {
        let _ = node.lock().unwrap().delete_peer(hash);
    };
    for peer in peers.peers.values() {
        save(&node, peer);
    }
    publish(&stats, &peers, &node);
    let mut timer = tokio::time::interval(SYNC_EVERY);
    let mut syncing: Option<tokio::task::JoinHandle<SyncDone>> = None;
    loop {
        tokio::select! {
            got = heard.recv() => {
                let Some(heard) = got else { break };
                match peers.heard(&heard) {
                    Change::Peered | Change::Updated => {
                        if let Some(peer) = peers.peers.get(&heard.hash) {
                            save(&node, peer);
                        }
                    }
                    Change::Unpeered => delete(&node, &heard.hash),
                    Change::Unchanged => {}
                }
                publish(&stats, &peers, &node);
            }
            Some(done) = async { match syncing.as_mut() { Some(task) => Some(task.await), None => None } } => {
                syncing = None;
                let Ok((hash, result, key)) = done else { continue };
                if !peers.peers.contains_key(&hash) {
                    continue;
                }
                peers.synced(hash, result.as_ref().err().and_then(Offered::failure));
                let Some(peer) = peers.peers.get_mut(&hash) else { continue };
                if key.is_some() {
                    peer.peering_key = key;
                }
                match result {
                    Ok(synced) => {
                        peer.sync_complete();
                        peer.offered += synced.offered as u64;
                        peer.outgoing += synced.sent as u64;
                        stats.lock().unwrap().sent += synced.sent as u64;
                    }
                    Err(Offered::BadKey) => {
                        tracing::debug!("peer sync with {} failed: it refused the peering key", hex::encode(hash));
                        peer.peering_key = None;
                        failed(peer, now());
                    }
                    Err(why) => {
                        tracing::debug!("peer sync with {} failed: {why:?}", hex::encode(hash));
                        failed(peer, now());
                    }
                }
                // What it has now was saved to its file as the sync went.
                let saved = node.lock().unwrap().load_peers().into_iter().find(|p| p.destination_hash == hash);
                if let Some(saved) = saved {
                    peer.handled_messages.extend(saved.handled_messages);
                }
                save(&node, peer);
                publish(&stats, &peers, &node);
            }
            _ = timer.tick(), if syncing.is_none() => {
                let next = {
                    let node = node.lock().unwrap();
                    peers.next(|handled, least| {
                        node.create_offer([0; 16], Some(least)).into_iter().filter(|id| !handled.contains(id)).collect()
                    })
                };
                publish(&stats, &peers, &node);
                let Some(hash) = next else { continue };
                let Some(their_identity) = peers.identity_of(&hash) else { continue };
                let Some(peer) = peers.peers.get_mut(&hash) else { continue };
                peer.begin_sync();
                let mut keyed = LxmPeer::new(hash);
                keyed.peering_cost = peer.peering_cost;
                keyed.peering_key = peer.peering_key;
                let mut policy = peer.outbound_offer_policy();
                let (runtime, identity, node) = (runtime.clone(), identity.clone(), node.clone());
                syncing = Some(tokio::spawn(async move {
                    // The key is work: made once, off the async threads.
                    let ours = identity.hash;
                    let key = if keyed.peering_key_ready() {
                        keyed.peering_key
                    } else {
                        let made = tokio::task::spawn_blocking(move || {
                            keyed.generate_peering_key(&their_identity, &ours, &StampControl::default());
                            keyed.peering_key
                        });
                        made.await.ok().flatten()
                    };
                    let Some((stamp, value)) = key else {
                        return (hash, Err(Offered::Refused(Failure::Io, "couldn't make a peering key".into())), None);
                    };
                    policy.peering_key = stamp.to_vec();
                    policy.peering_key_value = Some(value);
                    let synced = match tokio::time::timeout(SYNC_TIMEOUT, sync_peer(&runtime, &identity, &node, policy)).await {
                        Ok(result) => result,
                        Err(_) => {
                            node.lock().unwrap().complete_sync(&hash);
                            Err(Offered::Refused(Failure::Timeout, "the sync took too long".into()))
                        }
                    };
                    (hash, synced, Some((stamp, value)))
                }));
            }
        }
    }
    if let Some(task) = syncing {
        task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lxmf_core::handlers::{PropagationNodeAnnounceData, get_propagation_node_app_data};

    fn some(n: u8) -> HashSet<PropagationTransientId> {
        (0..n).map(|i| [i; 32]).collect()
    }

    fn announce(serving: bool, peering_cost: f64, timebase: i64) -> Vec<u8> {
        let mut data = PropagationNodeAnnounceData::new(serving, 256, 10240, 16.0, 3.0, peering_cost);
        data.timebase = timebase;
        get_propagation_node_app_data(&data)
    }

    fn node(n: u8, hops: u8, app_data: Vec<u8>) -> HeardNode {
        HeardNode { hash: [n; 16], identity: Some([n + 100; 16]), hops, path_response: false, app_data }
    }

    fn config() -> PeeringConfig {
        PeeringConfig { autopeer: true, max_depth: 4, max_peers: 2, static_peers: vec![[9; 16]] }
    }

    #[test]
    fn peered_as_lxmd_peers() {
        let mut peers = Peers::new(config(), [0; 16], Vec::new());
        // A static peer is there from the start, waiting to be heard.
        assert!(peers.peers[&[9; 16]].is_static && !peers.peers[&[9; 16]].stamp_costs_known());
        // Itself, a node too far, or one not serving: not peered with.
        assert_eq!(peers.heard(&node(0, 1, announce(true, 18.0, 1))), Change::Unchanged);
        assert_eq!(peers.heard(&node(1, 5, announce(true, 18.0, 1))), Change::Unchanged);
        assert_eq!(peers.heard(&node(1, 1, announce(false, 18.0, 1))), Change::Unchanged);
        // Nearby and serving: peered, with what it announces.
        assert_eq!(peers.heard(&node(1, 2, announce(true, 18.0, 1))), Change::Peered);
        let peer = &peers.peers[&[1; 16]];
        assert!(peer.autopeered && peer.stamp_costs_known() && peer.peering_cost == 18.0);
        assert_eq!(peers.identity_of(&[1; 16]), Some([101; 16]));
        // The same announce again changes nothing; a newer one updates it.
        assert_eq!(peers.heard(&node(1, 2, announce(true, 18.0, 1))), Change::Unchanged);
        assert_eq!(peers.heard(&node(1, 2, announce(true, 20.0, 2))), Change::Updated);
        // Full (the static peer counts): no more.
        assert_eq!(peers.heard(&node(2, 1, announce(true, 18.0, 1))), Change::Unchanged);
        // Moved away, or stopped serving: no longer peered.
        assert_eq!(peers.heard(&node(1, 6, announce(true, 18.0, 3))), Change::Unpeered);
        assert_eq!(peers.heard(&node(2, 1, announce(true, 18.0, 1))), Change::Peered);
        assert_eq!(peers.heard(&node(2, 1, announce(false, 18.0, 2))), Change::Unpeered);
        // A peering cost past what's worth working for: not peered.
        assert_eq!(peers.heard(&node(3, 1, announce(true, MAX_PEERING_COST + 1.0, 1))), Change::Unchanged);
        // A static peer, however far, and by a path's answer too.
        let mut far = node(9, 30, announce(true, 18.0, 1));
        far.path_response = true;
        assert_eq!(peers.heard(&far), Change::Updated);
        assert!(peers.peers[&[9; 16]].stamp_costs_known());
        // Not on a path's answer, for others.
        let mut answer = node(4, 1, announce(true, 18.0, 1));
        answer.path_response = true;
        assert_eq!(peers.heard(&answer), Change::Unchanged);
        // Not an announce at all.
        assert_eq!(peers.heard(&node(5, 1, vec![1, 2, 3])), Change::Unchanged);
    }

    #[test]
    fn a_peer_is_sent_no_more_in_one_sync_than_is_taken_here() {
        let mut peers = Peers::new(config(), [0; 16], Vec::new());
        let mut data = PropagationNodeAnnounceData::new(true, u64::MAX / 2, u64::MAX / 2, 16.0, 3.0, 18.0);
        data.timebase = 1;
        assert_eq!(peers.heard(&node(1, 1, get_propagation_node_app_data(&data))), Change::Peered);
        let most = lxmf_core::constants::SYNC_LIMIT as f64;
        let peer = &peers.peers[&[1; 16]];
        assert_eq!((peer.propagation_transfer_limit, peer.propagation_sync_limit), (Some(most), Some(most)));
    }

    #[test]
    fn a_peer_saved_with_more_than_is_taken_here_is_cut_when_synced() {
        let most = lxmf_core::constants::SYNC_LIMIT as f64;
        let policy_of = |transfer: Option<f64>, sync: Option<f64>| {
            let mut peer = LxmPeer::from_announce([1; 16], 1.0, transfer, sync, Some(16.0), Some(3.0), Some(18.0));
            peer.propagation_sync_limit = sync;
            let mut policy = peer.outbound_offer_policy();
            within_what_is_taken(&mut policy);
            (policy.propagation_transfer_limit, policy.propagation_sync_limit)
        };
        assert_eq!(policy_of(Some(1e18), Some(1e18)), (Some(most), Some(most)));
        assert_eq!(policy_of(None, None), (Some(most), Some(most)), "no limit announced is no limit");
        assert_eq!(policy_of(Some(f64::NAN), Some(f64::INFINITY)), (Some(most), Some(most)));
        assert_eq!(policy_of(Some(256.0), Some(2048.0)), (Some(256.0), Some(2048.0)), "less is left as it is");
    }

    #[test]
    fn auto_peering_off_keeps_static_peers_only() {
        let saved = vec![LxmPeer::from_announce([1; 16], 1.0, Some(256.0), Some(10240.0), Some(16.0), Some(3.0), Some(18.0))];
        let config = PeeringConfig { autopeer: false, ..config() };
        let mut peers = Peers::new(config, [0; 16], saved);
        assert_eq!(peers.peers.keys().collect::<Vec<_>>(), [&[9; 16]]);
        assert_eq!(peers.heard(&node(2, 1, announce(true, 18.0, 1))), Change::Unchanged);
        // On, the one saved is kept.
        let saved = vec![LxmPeer::from_announce([1; 16], 1.0, None, None, Some(16.0), Some(3.0), Some(18.0))];
        assert!(
            Peers::new(PeeringConfig { static_peers: Vec::new(), ..super::tests::config() }, [0; 16], saved).peers.contains_key(&[1; 16])
        );
    }

    #[test]
    fn the_next_to_sync_has_messages_it_hasnt_had() {
        let mut peers = Peers::new(PeeringConfig { static_peers: Vec::new(), ..config() }, [0; 16], Vec::new());
        assert_eq!(peers.next(|_, _| some(3)), None);
        peers.heard(&node(1, 1, announce(true, 18.0, 1)));
        assert_eq!(peers.next(|_, _| some(0)), None, "nothing to send");
        assert_eq!(peers.next(|_, _| some(3)), Some([1; 16]));
        // Not while its backoff lasts.
        peers.peers.get_mut(&[1; 16]).unwrap().begin_sync();
        peers.peers.get_mut(&[1; 16]).unwrap().state = lxmf_core::constants::PeerState::Idle;
        assert_eq!(peers.next(|_, _| some(3)), None);
        // Nor without knowing who it is.
        let mut unknown = node(2, 1, announce(true, 18.0, 1));
        unknown.identity = None;
        peers.heard(&unknown);
        assert_eq!(peers.next(|_, _| some(3)), None);
        peers.know([2; 16], [7; 16]);
        assert_eq!(peers.next(|_, _| some(3)), Some([2; 16]));
    }

    /// A store of messages 0, 1 and 2, with stamp values 20, 20 and 5.
    fn stamped(handled: &HashSet<PropagationTransientId>, least: f64) -> HashSet<PropagationTransientId> {
        [([0; 32], 20.0), ([1; 32], 20.0), ([2; 32], 5.0)]
            .into_iter()
            .filter(|(id, value)| *value >= least && !handled.contains(id))
            .map(|(id, _)| id)
            .collect()
    }

    fn with_peer(cost: f64, timebase: i64) -> Peers {
        let mut peers = Peers::new(PeeringConfig { static_peers: Vec::new(), ..config() }, [0; 16], Vec::new());
        peers.heard(&node(1, 1, announce(true, cost, timebase)));
        peers
    }

    #[test]
    fn a_message_dropped_for_a_low_stamp_does_not_come_back_as_unhandled() {
        // The peer takes stamps from 13 up (announced 16, less 3): message 2 is below.
        let mut peers = with_peer(18.0, 1);
        assert_eq!(peers.next(stamped), Some([1; 16]));
        assert_eq!(peers.peers[&[1; 16]].unhandled_messages, some(2));
        // A sync: 0 and 1 are taken.
        let peer = peers.peers.get_mut(&[1; 16]).unwrap();
        peer.handled_messages.extend([[0; 32], [1; 32]]);
        peer.sync_complete();
        // The next tick rebuilds the set; the refused message stays out of it.
        assert_eq!(peers.next(stamped), None);
        assert_eq!(peers.summary(|_| true).unhandled, 0);
    }

    #[test]
    fn a_message_below_the_stamp_cost_is_offered_again_if_the_cost_falls() {
        let mut peers = with_peer(18.0, 1);
        peers.next(stamped);
        assert!(!peers.peers[&[1; 16]].unhandled_messages.contains(&[2; 32]));
        // The peer announces a lower cost (stamp 6, flexibility 3: takes 3 up).
        let mut data = PropagationNodeAnnounceData::new(true, 256, 10240, 6.0, 3.0, 18.0);
        data.timebase = 2;
        let heard = node(1, 1, get_propagation_node_app_data(&data));
        assert_eq!(peers.heard(&heard), Change::Updated);
        peers.next(stamped);
        assert_eq!(peers.peers[&[1; 16]].unhandled_messages, some(3));
        // And a higher cost again takes it out.
        data.timebase = 3;
        data.stamp_cost = 16.0;
        peers.heard(&node(1, 1, get_propagation_node_app_data(&data)));
        peers.next(stamped);
        assert_eq!(peers.peers[&[1; 16]].unhandled_messages, some(2));
    }

    #[test]
    fn a_message_that_leaves_the_store_is_no_longer_waiting() {
        let mut peers = with_peer(18.0, 1);
        peers.next(stamped);
        assert_eq!(peers.summary(|_| true).unhandled, 2);
        assert_eq!(peers.summary(|id| id != &[0; 32]).unhandled, 1, "message 0 was removed from the store");
        // And the next tick's set is built from what is stored.
        let gone = |handled: &HashSet<PropagationTransientId>, least: f64| {
            let mut ids = stamped(handled, least);
            ids.remove(&[0; 32]);
            ids
        };
        peers.next(gone);
        assert_eq!(peers.peers[&[1; 16]].unhandled_messages, HashSet::from([[1; 32]]));
    }

    #[test]
    fn a_peer_saved_before_this_change_loads() {
        // Nothing is saved for what a peer refuses, so an older peer file is read as it was.
        let mut saved = LxmPeer::from_announce([1; 16], 1.0, Some(256.0), Some(10240.0), Some(16.0), Some(3.0), Some(18.0));
        saved.autopeered = true;
        saved.handled_messages.insert([0; 32]);
        let bytes = saved.to_bytes_with_handled();
        let loaded = LxmPeer::from_bytes_with_handled(&bytes).expect("a saved peer file is read");
        let mut peers = Peers::new(config(), [0; 16], vec![loaded]);
        peers.know([1; 16], [5; 16]);
        assert_eq!(peers.next(stamped), Some([1; 16]));
        assert_eq!(peers.peers[&[1; 16]].unhandled_messages, HashSet::from([[1; 32]]));
    }

    #[test]
    fn distinct_messages_are_counted_once_however_many_peers_wait_for_them() {
        let mut peers = Peers::new(PeeringConfig { max_peers: 10, static_peers: Vec::new(), ..config() }, [0; 16], Vec::new());
        for n in 1..=3 {
            peers.heard(&node(n, 1, announce(true, 18.0, 1)));
        }
        // Peers 1 and 2 wait for messages 0 and 1; peer 3 for 1 and 2; 5 is not stored.
        let set = |ids: &[u8]| ids.iter().map(|i| [*i; 32]).collect::<HashSet<_>>();
        peers.peers.get_mut(&[1; 16]).unwrap().unhandled_messages = set(&[0, 1]);
        peers.peers.get_mut(&[2; 16]).unwrap().unhandled_messages = set(&[0, 1]);
        peers.peers.get_mut(&[3; 16]).unwrap().unhandled_messages = set(&[1, 2, 5]);
        let stats = peers.summary(|id| id[0] < 5);
        assert_eq!((stats.unhandled, stats.unhandled_distinct), (6, 3));
        assert_eq!(stats.rows.iter().map(|r| r.unhandled).collect::<Vec<_>>(), [2, 2, 2]);
        assert_eq!(peers.summary(|_| false).unhandled_distinct, 0);
        assert_eq!(Peers::default().summary(|_| true), PeeringStats::default());
    }

    #[test]
    fn every_peer_is_synced_in_turn_and_those_never_synced_go_first() {
        let mut peers = Peers::new(PeeringConfig { max_peers: 10, static_peers: Vec::new(), ..config() }, [0; 16], Vec::new());
        for n in 1..=6 {
            peers.heard(&node(n, 1, announce(true, 18.0, 1)));
        }
        // Peer 3 was synced a while ago; the rest never were.
        peers.peers.get_mut(&[3; 16]).unwrap().last_sync_attempt = 1_000.0;
        let mut order = Vec::new();
        for _ in 0..6 {
            let hash = peers.next(stamped).expect("a peer is waiting");
            let peer = peers.peers.get_mut(&hash).unwrap();
            peer.begin_sync();
            // Each is synced (taking what it was offered, but not everything).
            peer.sync_complete();
            order.push(hash[0]);
        }
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, [1, 2, 3, 4, 5, 6], "each once, none starved: {order:?}");
        assert_eq!(order[5], 3, "the one synced before comes last: {order:?}");
        // And round again: the first synced is first.
        assert_eq!(peers.next(stamped), Some([order[0]; 16]));
    }

    #[test]
    fn a_peer_still_syncing_or_in_backoff_does_not_hold_the_others_back() {
        let mut peers = Peers::new(PeeringConfig { max_peers: 10, static_peers: Vec::new(), ..config() }, [0; 16], Vec::new());
        for n in 1..=3 {
            peers.heard(&node(n, 1, announce(true, 18.0, 1)));
        }
        peers.peers.get_mut(&[1; 16]).unwrap().begin_sync(); // stuck establishing a Link
        let slow = peers.peers.get_mut(&[2; 16]).unwrap();
        slow.begin_sync();
        failed(slow, now());
        slow.state = lxmf_core::constants::PeerState::Idle; // failed, in backoff
        for _ in 0..5 {
            assert_eq!(peers.next(stamped), Some([3; 16]));
        }
    }

    #[test]
    fn failed_syncs_back_off_for_a_bounded_time_and_a_success_resets_it() {
        let mut peers = with_peer(18.0, 1);
        let peer = peers.peers.get_mut(&[1; 16]).unwrap();
        let start = now();
        for tries in 1..=20 {
            peer.begin_sync();
            failed(peer, now());
            assert_eq!(peer.unreachable_count, tries);
            assert!(peer.sync_backoff <= BACKOFF_MAX, "{}", peer.sync_backoff);
            assert!(peer.next_sync_attempt <= now() + BACKOFF_MAX + 1.0);
        }
        assert!(peer.next_sync_attempt > start + 60.0, "still waits");
        peer.begin_sync();
        peer.sync_complete();
        assert_eq!((peer.sync_backoff, peer.next_sync_attempt), (0.0, 0.0));
        // A peer failing does not hold back the others: they are chosen while its backoff lasts.
        peers.heard(&node(2, 1, announce(true, 18.0, 1)));
        let failing = peers.peers.get_mut(&[1; 16]).unwrap();
        failing.begin_sync();
        failed(failing, now());
        failing.state = lxmf_core::constants::PeerState::Idle;
        for _ in 0..10 {
            assert_eq!(peers.next(stamped), Some([2; 16]));
        }
    }

    #[test]
    fn answers_to_an_offer() {
        let offered = [[1; 32], [2; 32], [3; 32]];
        let answer = |value: Value| read_offer_response(&encode(&value), &offered);
        assert_eq!(answer(Value::Boolean(true)), Offered::Wants(offered.to_vec()));
        assert_eq!(answer(Value::Boolean(false)), Offered::Wants(Vec::new()));
        // Only what was offered, whatever it asks for.
        let some = Value::Array(vec![Value::Binary(vec![2; 32]), Value::Binary(vec![8; 32]), Value::Binary(vec![1; 3]), Value::from(4)]);
        assert_eq!(answer(some), Offered::Wants(vec![[2; 32]]));
        assert_eq!(answer(Value::from(PeerError::InvalidKey as u64)), Offered::BadKey);
        assert!(matches!(answer(Value::from(PeerError::Throttled as u64)), Offered::Refused(Failure::Busy, why) if why.contains("busy")));
        assert!(matches!(answer(Value::from(0x99)), Offered::Refused(Failure::BadAnswer, why) if why.contains("153")));
        assert!(matches!(read_offer_response(&[0xc1], &offered), Offered::Refused(Failure::BadAnswer, _)));
        assert_eq!(answer(Value::from(0xF4)).failure(), Some(Failure::RefusedInvalidData));
        assert_eq!(answer(Value::from(0xF0)).failure(), Some(Failure::NoAccess));
        assert_eq!(answer(Value::from(0xF1)).failure(), Some(Failure::NoAccess));
        assert_eq!(answer(Value::from(PeerError::InvalidKey as u64)).failure(), Some(Failure::BadKey));
        assert_eq!(answer(Value::Boolean(true)).failure(), None);
    }

    #[test]
    fn an_offer_is_sent_as_a_list_not_as_bytes() {
        let ids = [[1u8; 32], [2; 32]];
        let value = offer_request_value(&[9; 32], &ids);
        assert_eq!(kind_of(&value), "list");
        let Value::Array(parts) = &value else { panic!("not a list: {value:?}") };
        assert_eq!(parts[0], Value::Binary(vec![9; 32]));
        assert_eq!(parts[1], Value::Array(vec![Value::Binary(vec![1; 32]), Value::Binary(vec![2; 32])]));
        assert_eq!(parts.len(), 2);
        let empty = offer_request_value(&[], &[]);
        assert_eq!(empty, Value::Array(vec![Value::Binary(Vec::new()), Value::Array(Vec::new())]));
    }

    #[test]
    fn failures_are_noted_per_peer_and_cleared_by_a_success() {
        let mut peers = Peers::new(PeeringConfig { static_peers: vec![[7; 16]], ..PeeringConfig::default() }, [0; 16], Vec::new());
        assert_eq!(peers.summary(|_| true).rows[0].last_failure, None);
        peers.synced([7; 16], Some(Failure::RefusedInvalidData));
        assert_eq!(peers.summary(|_| true).rows[0].last_failure, Some("refused_invalid_data"));
        peers.synced([7; 16], Some(Failure::NoLink));
        assert_eq!(peers.summary(|_| true).rows[0].last_failure, Some("no_link"));
        peers.synced([7; 16], None);
        assert_eq!(peers.summary(|_| true).rows[0].last_failure, None);
        for why in [
            Failure::NoLink,
            Failure::NoAnswer,
            Failure::RefusedInvalidData,
            Failure::BadKey,
            Failure::NoAccess,
            Failure::Busy,
            Failure::Timeout,
            Failure::BadAnswer,
            Failure::Io,
        ] {
            assert!(!why.as_str().is_empty() && why.as_str().chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
        assert_eq!(request_failure(&LinkSessionError::Timeout("a response")), Failure::Timeout);
        assert_eq!(request_failure(&LinkSessionError::SessionClosed), Failure::NoAnswer);
    }

    #[test]
    fn the_summary_counts_the_peers_and_lists_them_by_address() {
        let mut peers =
            Peers::new(PeeringConfig { autopeer: true, max_depth: 4, max_peers: 10, static_peers: vec![[7; 16]] }, [0; 16], Vec::new());
        let mut data = PropagationNodeAnnounceData::new(true, 256, 10240, 16.0, 3.0, 5.0);
        data.metadata.insert(lxmf_core::constants::PN_META_NAME, "Far away".as_bytes().to_vec());
        data.timebase = 100;
        let heard =
            HeardNode { hash: [3; 16], identity: None, hops: 3, path_response: false, app_data: get_propagation_node_app_data(&data) };
        assert_eq!(peers.heard(&heard), Change::Peered);
        peers.peers.get_mut(&[3; 16]).unwrap().offered = 4;
        peers.peers.get_mut(&[3; 16]).unwrap().alive = true;
        peers.peers.get_mut(&[7; 16]).unwrap().offered = 1;
        for hash in [[3; 16], [7; 16]] {
            peers.peers.get_mut(&hash).unwrap().unhandled_messages = some(2);
        }
        let summary = peers.summary(|_| true);
        assert_eq!((summary.peered, summary.alive, summary.statics, summary.offered, summary.unhandled), (2, 1, 1, 5, 4));
        let rows = &summary.rows;
        assert_eq!(rows.iter().map(|r| r.hash).collect::<Vec<_>>(), [[3; 16], [7; 16]]);
        assert_eq!((rows[0].name.as_deref(), rows[0].hops, rows[0].is_static, rows[0].state), (Some("Far away"), Some(3), false, "idle"));
        assert_eq!((rows[1].name.as_deref(), rows[1].hops, rows[1].is_static, rows[1].last_sync), (None, None, true, None));
        // Unpeered, it is forgotten.
        let mut far = heard.clone();
        far.hops = 9;
        assert_eq!(peers.heard(&far), Change::Unpeered);
        assert_eq!(peers.summary(|_| false).rows.len(), 1);
        assert!(!peers.hops.contains_key(&[3; 16]));
    }
}
