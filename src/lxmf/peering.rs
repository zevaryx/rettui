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
use lxmf_core::peer::{LxmPeer, OutboundOfferPolicy, select_sync_peer};
use lxmf_core::propagation_node::{PropagationNode, read_planned_messages};
use lxmf_core::types::PropagationTransientId;
use rmpv::Value;
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use super::encode;
use crate::net::Hash;

/// How often a peer is picked to sync with (as often as `lxmd`).
const SYNC_EVERY: Duration = Duration::from_secs(24);
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
        Self { peers, identities: HashMap::new(), config, own }
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
        let limits = (Some(data.transfer_limit as f64), Some(data.sync_limit as f64));
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
        Change::Unpeered
    }

    /// Note who a node is, from a key heard before (a static peer not yet
    /// heard this time).
    pub fn know(&mut self, hash: Hash, identity: [u8; 16]) {
        self.identities.entry(hash).or_insert(identity);
    }

    pub fn identity_of(&self, hash: &Hash) -> Option<[u8; 16]> {
        self.identities.get(hash).copied()
    }

    /// The peer to sync with next, if one has messages it hasn't had and
    /// is ready: its costs and identity known, its backoff over.
    /// `unhandled` counts a peer's messages it hasn't had.
    pub fn next(&mut self, unhandled: impl Fn(&HashSet<PropagationTransientId>) -> usize) -> Option<Hash> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        let ready: Vec<Hash> = self
            .peers
            .values()
            .filter(|p| p.stamp_costs_known() && self.identities.contains_key(&p.destination_hash) && p.next_sync_attempt <= now)
            .map(|p| p.destination_hash)
            .collect();
        for hash in &ready {
            if let Some(peer) = self.peers.get_mut(hash) {
                let count = unhandled(&peer.handled_messages);
                peer.set_unhandled_count(u32::try_from(count).unwrap_or(u32::MAX));
            }
        }
        let candidates: Vec<&LxmPeer> = ready.iter().filter_map(|h| self.peers.get(h)).collect();
        select_sync_peer(&candidates).map(|i| candidates[i].destination_hash)
    }
}

/// What an `/offer` was answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offered {
    /// These are wanted (of those offered); the rest it has.
    Wants(Vec<PropagationTransientId>),
    /// Its peering key was refused: make a new one.
    BadKey,
    Refused(String),
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
        Ok(Value::Integer(code)) => Offered::Refused(match code.as_u64().and_then(|c| u8::try_from(c).ok()) {
            Some(0xF0) => "it couldn't tell who we are".into(),
            Some(0xF1) => "it doesn't let us peer with it".into(),
            Some(0xF4) => "it couldn't read the offer".into(),
            Some(0xF6) => "it's busy: trying again later".into(),
            _ => format!("error {code}"),
        }),
        _ => Offered::Refused("an answer it couldn't read".into()),
    }
}

/// How a sync went: how many messages were offered and sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Synced {
    pub offered: usize,
    pub sent: usize,
}

/// Sync with one peer, as its `policy` says: offer what it hasn't had,
/// then send what it wants. What it has, or took, is recorded as handled.
pub async fn sync_peer(
    runtime: &ReticulumHandle,
    identity: &Identity,
    node: &Arc<Mutex<PropagationNode>>,
    policy: OutboundOfferPolicy,
) -> Result<Synced, Offered> {
    let refused = |e: String| Offered::Refused(e);
    let offer = {
        let (node, policy) = (node.clone(), policy.clone());
        tokio::task::spawn_blocking(move || node.lock().unwrap().prepare_sync_offer_with_policy(&policy))
            .await
            .map_err(|e| refused(e.to_string()))?
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
            .map_err(|e| refused(format!("no Link to it: {e}")))?;
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
    let ids = offered.iter().map(|id| Value::Binary(id.to_vec())).collect();
    let request = encode(&Value::Array(vec![Value::Binary(key.to_vec()), Value::Array(ids)]));
    let response = handle
        .request(OFFER_REQUEST_PATH, &request, Some(REQUEST_TIMEOUT))
        .await
        .map_err(|e| Offered::Refused(format!("no answer to the offer: {e}")))?;
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
        .map_err(|e| Offered::Refused(format!("couldn't send the messages: {e}")))?;
    match tokio::time::timeout(TRANSFER_TIMEOUT, transfer.concluded()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(Offered::Refused(format!("the transfer failed: {e}"))),
        Err(_) => return Err(Offered::Refused("the transfer timed out".into())),
    }
    let sent = sent_ids.len();
    handled(node, policy, sent_ids).await;
    Ok(Synced { offered: offered.len(), sent })
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeeringStats {
    pub peered: usize,
    pub sent: u64,
}

/// A finished sync: the peer, how it went, and the peering key made for it
/// with the cost it was made at.
type SyncDone = (Hash, Result<Synced, Offered>, Option<([u8; 32], u32)>);

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
    stats.lock().unwrap().peered = peers.peers.len();
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
                stats.lock().unwrap().peered = peers.peers.len();
            }
            Some(done) = async { match syncing.as_mut() { Some(task) => Some(task.await), None => None } } => {
                syncing = None;
                let Ok((hash, result, key)) = done else { continue };
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
                        peer.peering_key = None;
                        peer.sync_failed();
                    }
                    Err(why) => {
                        tracing::debug!("peer sync with {} failed: {why:?}", hex::encode(hash));
                        peer.sync_failed();
                    }
                }
                // What it has now was saved to its file as the sync went.
                let saved = node.lock().unwrap().load_peers().into_iter().find(|p| p.destination_hash == hash);
                if let Some(saved) = saved {
                    peer.handled_messages.extend(saved.handled_messages);
                }
                save(&node, peer);
            }
            _ = timer.tick(), if syncing.is_none() => {
                let next = {
                    let node = node.lock().unwrap();
                    peers.next(|handled| node.unhandled_message_count(handled))
                };
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
                            keyed.generate_peering_key(&their_identity, &ours);
                            keyed.peering_key
                        });
                        made.await.ok().flatten()
                    };
                    let Some((stamp, value)) = key else {
                        return (hash, Err(Offered::Refused("couldn't make a peering key".into())), None);
                    };
                    policy.peering_key = stamp.to_vec();
                    policy.peering_key_value = Some(value);
                    (hash, sync_peer(&runtime, &identity, &node, policy).await, Some((stamp, value)))
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

    fn announce(serving: bool, peering_cost: u8, timebase: i64) -> Vec<u8> {
        let mut data = PropagationNodeAnnounceData::new(serving, 256, 10240, 16, 3, peering_cost);
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
        assert_eq!(peers.heard(&node(0, 1, announce(true, 18, 1))), Change::Unchanged);
        assert_eq!(peers.heard(&node(1, 5, announce(true, 18, 1))), Change::Unchanged);
        assert_eq!(peers.heard(&node(1, 1, announce(false, 18, 1))), Change::Unchanged);
        // Nearby and serving: peered, with what it announces.
        assert_eq!(peers.heard(&node(1, 2, announce(true, 18, 1))), Change::Peered);
        let peer = &peers.peers[&[1; 16]];
        assert!(peer.autopeered && peer.stamp_costs_known() && peer.peering_cost == 18);
        assert_eq!(peers.identity_of(&[1; 16]), Some([101; 16]));
        // The same announce again changes nothing; a newer one updates it.
        assert_eq!(peers.heard(&node(1, 2, announce(true, 18, 1))), Change::Unchanged);
        assert_eq!(peers.heard(&node(1, 2, announce(true, 20, 2))), Change::Updated);
        // Full (the static peer counts): no more.
        assert_eq!(peers.heard(&node(2, 1, announce(true, 18, 1))), Change::Unchanged);
        // Moved away, or stopped serving: no longer peered.
        assert_eq!(peers.heard(&node(1, 6, announce(true, 18, 3))), Change::Unpeered);
        assert_eq!(peers.heard(&node(2, 1, announce(true, 18, 1))), Change::Peered);
        assert_eq!(peers.heard(&node(2, 1, announce(false, 18, 2))), Change::Unpeered);
        // A peering cost past what's worth working for: not peered.
        assert_eq!(peers.heard(&node(3, 1, announce(true, MAX_PEERING_COST + 1, 1))), Change::Unchanged);
        // A static peer, however far, and by a path's answer too.
        let mut far = node(9, 30, announce(true, 18, 1));
        far.path_response = true;
        assert_eq!(peers.heard(&far), Change::Updated);
        assert!(peers.peers[&[9; 16]].stamp_costs_known());
        // Not on a path's answer, for others.
        let mut answer = node(4, 1, announce(true, 18, 1));
        answer.path_response = true;
        assert_eq!(peers.heard(&answer), Change::Unchanged);
        // Not an announce at all.
        assert_eq!(peers.heard(&node(5, 1, vec![1, 2, 3])), Change::Unchanged);
    }

    #[test]
    fn auto_peering_off_keeps_static_peers_only() {
        let saved = vec![LxmPeer::from_announce([1; 16], 1.0, Some(256.0), Some(10240.0), Some(16), Some(3), Some(18))];
        let config = PeeringConfig { autopeer: false, ..config() };
        let mut peers = Peers::new(config, [0; 16], saved);
        assert_eq!(peers.peers.keys().collect::<Vec<_>>(), [&[9; 16]]);
        assert_eq!(peers.heard(&node(2, 1, announce(true, 18, 1))), Change::Unchanged);
        // On, the one saved is kept.
        let saved = vec![LxmPeer::from_announce([1; 16], 1.0, None, None, Some(16), Some(3), Some(18))];
        assert!(
            Peers::new(PeeringConfig { static_peers: Vec::new(), ..super::tests::config() }, [0; 16], saved).peers.contains_key(&[1; 16])
        );
    }

    #[test]
    fn the_next_to_sync_has_messages_it_hasnt_had() {
        let mut peers = Peers::new(PeeringConfig { static_peers: Vec::new(), ..config() }, [0; 16], Vec::new());
        assert_eq!(peers.next(|_| 3), None);
        peers.heard(&node(1, 1, announce(true, 18, 1)));
        assert_eq!(peers.next(|_| 0), None, "nothing to send");
        assert_eq!(peers.next(|_| 3), Some([1; 16]));
        // Not while its backoff lasts.
        peers.peers.get_mut(&[1; 16]).unwrap().begin_sync();
        peers.peers.get_mut(&[1; 16]).unwrap().state = lxmf_core::constants::PeerState::Idle;
        assert_eq!(peers.next(|_| 3), None);
        // Nor without knowing who it is.
        let mut unknown = node(2, 1, announce(true, 18, 1));
        unknown.identity = None;
        peers.heard(&unknown);
        assert_eq!(peers.next(|_| 3), None);
        peers.know([2; 16], [7; 16]);
        assert_eq!(peers.next(|_| 3), Some([2; 16]));
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
        assert!(matches!(answer(Value::from(PeerError::Throttled as u64)), Offered::Refused(why) if why.contains("busy")));
        assert!(matches!(answer(Value::from(0x99)), Offered::Refused(why) if why.contains("153")));
        assert!(matches!(read_offer_response(&[0xc1], &offered), Offered::Refused(_)));
    }
}
