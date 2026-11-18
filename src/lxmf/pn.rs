//! Hosting an LXMF propagation node, on rsLXMF's [`PropagationNode`] (its
//! message store, stamp checks, and answers to `/get` and `/offer`).
//!
//! The node keeps messages for people who aren't online, as Python LXMF's
//! propagation nodes do:
//!
//! - **Taking messages:** clients send them as a Resource or, when small, a
//!   packet over a Link: `[timebase, [message]]`, each message encrypted
//!   for its recipient and followed by a propagation stamp of at least the
//!   announced cost (less its flexibility). Messages for this client are
//!   delivered here rather than stored.
//! - **Giving them out:** `/get` lists, sends and deletes the messages for
//!   the client that identified on the Link.
//! - **Peers:** nodes that peer with this one (`/offer`, with a peering key
//!   proving their work) send it the messages it doesn't have, and it sends
//!   them its own (see [`super::peering`]), as Python's `lxmd` does.
//! - It announces itself (`lxmf.propagation`) with its name, stamp cost and
//!   limits, and culls old messages (after LXMF's 30 days) to stay within its
//!   storage limit.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lxmf_core::constants::{
    LXMF_OVERHEAD, MESSAGE_EXPIRY, PEERING_COST, PROPAGATION_COST_FLEX, PROPAGATION_COST_MIN, PeerError, SYNC_LIMIT,
};
use lxmf_core::handlers::{PropagationNodeAnnounceData, get_propagation_node_app_data};
use lxmf_core::propagation_node::{
    OfferRequestContext, PropagationAdmission, PropagationNode, PropagationNodeConfig, ValidatedOpaquePropagationBlob,
};
use lxmf_core::stamper::validate_peering_key;
use rmpv::Value;
use rns_identity::destination::Destination;
use rns_runtime::prelude::*;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::peering::{HeardNode, PeeringConfig, PeeringStats, Peers};
use super::{LXMF_ASPECT, PROPAGATION_ASPECT, encode};
use crate::config::{Paths, Settings};
use crate::net::Hash;

/// How the node runs, from the settings.
#[derive(Debug, Clone, PartialEq)]
pub struct PnConfig {
    /// The name it announces.
    pub name: String,
    /// The propagation stamp cost asked of senders.
    pub stamp_cost: f64,
    /// Most bytes of messages kept.
    pub storage_bytes: u64,
    /// Largest transfer a client may send, in kilobytes.
    pub transfer_kb: u64,
    /// Where messages are kept.
    pub dir: PathBuf,
    /// Time between announces (`None`: only when it starts).
    pub announce_interval: Option<Duration>,
    /// Which other nodes it sends messages on to.
    pub peering: PeeringConfig,
}

impl PnConfig {
    /// The node the settings ask for, if any.
    pub fn from_settings(settings: &Settings, paths: &Paths) -> Option<Self> {
        settings.pn_enabled.then(|| Self {
            name: settings.pn_name.clone().unwrap_or_else(|| settings.display_name.clone()),
            stamp_cost: settings.pn_stamp_cost.clamp(PROPAGATION_COST_MIN as u64, 254) as f64,
            storage_bytes: settings.pn_storage_mb.max(1).saturating_mul(1_000_000),
            transfer_kb: settings.pn_transfer_kb.clamp(1, SYNC_LIMIT as u64),
            dir: paths.propagation.clone(),
            announce_interval: (settings.announce_interval_mins > 0).then(|| Duration::from_secs(settings.announce_interval_mins * 60)),
            peering: PeeringConfig {
                autopeer: settings.pn_autopeer,
                max_depth: settings.pn_autopeer_depth.min(u64::from(u8::MAX)) as u8,
                max_peers: usize::try_from(settings.pn_max_peers).unwrap_or(usize::MAX),
                static_peers: settings.static_peers(),
            },
        })
    }
}

/// What the node holds and has done, for the UIs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PnStats {
    /// Messages held, and their size in bytes.
    pub messages: usize,
    pub bytes: usize,
    /// Taken in to keep, delivered here (they were for this client), and
    /// given out to their recipients.
    pub received: u64,
    pub delivered_here: u64,
    pub served: u64,
    /// Turned away: without a valid stamp, over the size or storage limit.
    pub rejected: u64,
    /// Peers connected now (that proved their peering key).
    pub peers: usize,
    /// Nodes it sends messages on to, and how many it has sent them.
    pub peered: usize,
    pub sent_to_peers: u64,
}

#[derive(Default)]
struct Counters {
    received: AtomicU64,
    delivered_here: AtomicU64,
    served: AtomicU64,
    rejected: AtomicU64,
}

/// Messages with too small a stamp in one transfer, after which the rest
/// of it is refused unchecked: checking each costs a workblock (several
/// milliseconds), an honest peer sends only messages whose stamps pass, and
/// a transfer may hold tens of thousands.
const MAX_BAD_STAMPS: usize = 16;
/// Messages with too small a stamp one Link may send over all its
/// transfers, after which it is closed and what it still has queued is
/// refused unchecked. A client may send any number of one-message
/// transfers, each a workblock to check: without this one Link could keep
/// the node busy for good.
const MAX_BAD_STAMPS_PER_LINK: usize = 16;
/// Bad stamps from Links nothing was stored from, within this long, after
/// which such a Link is closed at its first bad stamp: a flood over many
/// fresh Links then costs one check per Link, not 16.
const UNTRUSTED_BAD_LIMIT: usize = 64;
const UNTRUSTED_BAD_WINDOW: Duration = Duration::from_secs(10);
/// Transfers waiting to be checked, and their size in all (see [`Intake`]).
/// There is no smaller limit for one Link: the library proves a packet as
/// it comes, so an honest client's burst refused here would be lost.
const MAX_QUEUED: usize = 4096;
const MAX_QUEUED_BYTES: usize = 4 * SYNC_LIMIT * 1000;
/// A propagated message's stamp, after the message.
const STAMP_SIZE: usize = 32;

type LinkId = [u8; 16];

/// What checking one transfer found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tally {
    /// Messages newly stored or delivered here (not copies of ones held).
    fresh: usize,
    /// Messages whose stamps were checked and found short, or that weren't
    /// messages at all.
    bad: usize,
}

/// Transfers waiting to be checked, taken in turn from each Link that sent
/// some (not in the order they came), so one Link's backlog can't keep
/// others' messages waiting. Links a message was stored from go first, then
/// new ones, then those that sent bad stamps: a flood over many Links
/// doesn't hold up a client once one of its messages has passed. It is
/// bounded; when it is full, room is made at the cost of the Link that
/// least deserves it (see [`Intake::standing`]). The transfers dropped then
/// were already proven to their senders by the library (as Python's node
/// proves them before looking at them), so they are lost without a word;
/// that happens only while 4,096 transfers or 40 MB wait, which takes a
/// flood, and Links with bad stamps lose theirs first.
#[derive(Debug, Default)]
struct Intake {
    queues: HashMap<LinkId, VecDeque<Vec<u8>>>,
    /// Links with something queued, the next to be served first.
    turns: VecDeque<LinkId>,
    queued: usize,
    bytes: usize,
    /// Bad stamps each Link has sent so far.
    bad: HashMap<LinkId, usize>,
    /// Links a message was newly stored from.
    good: HashSet<LinkId>,
    /// Links that closed with transfers still queued or being checked:
    /// those are checked (they were proven to the sender), then the Link is
    /// forgotten.
    closing: HashSet<LinkId>,
    /// The Link whose transfer is being checked.
    checking: Option<LinkId>,
    /// When the last bad stamps from Links nothing was stored from came, at
    /// most [`UNTRUSTED_BAD_LIMIT`] of them.
    untrusted_bad: VecDeque<std::time::Instant>,
}

impl Intake {
    /// Queue a transfer from `link`. False if it was refused: the Link is
    /// over its budget of bad stamps, or the queue is full of transfers from
    /// Links no worse than this one. `shed` counts transfers of other Links
    /// dropped to make room.
    fn push(&mut self, link: LinkId, data: Vec<u8>, shed: &mut usize) -> bool {
        if self.over_budget(&link) {
            return false;
        }
        while self.queued > 0 && (self.queued >= MAX_QUEUED || self.bytes + data.len() > MAX_QUEUED_BYTES) {
            if !self.shed_for(&link) {
                return false;
            }
            *shed += 1;
        }
        self.queued += 1;
        self.bytes += data.len();
        let queue = self.queues.entry(link).or_default();
        if queue.is_empty() {
            self.turns.push_back(link);
        }
        queue.push_back(data);
        true
    }

    /// How readily a Link's transfers are dropped when the queue is full,
    /// the greatest first: one that sent bad stamps, then one nothing was
    /// stored from yet, then the one with most waiting.
    fn standing(&self, link: &LinkId) -> (bool, bool, usize) {
        let waiting = self.queues.get(link).map_or(0, VecDeque::len);
        (self.bad.get(link).is_some_and(|bad| *bad > 0), !self.good.contains(link), waiting)
    }

    /// Drop the oldest transfer of the Link that least deserves its place,
    /// if it deserves it less than `link` does. False if there's none.
    fn shed_for(&mut self, link: &LinkId) -> bool {
        let own = self.standing(link);
        let victim = self
            .queues
            .iter()
            .filter(|(other, queue)| *other != link && !queue.is_empty())
            .map(|(other, _)| (self.standing(other), *other))
            .filter(|(standing, _)| *standing > own)
            .max()
            .map(|(_, other)| other);
        let Some(victim) = victim else { return false };
        if let Some(data) = self.queues.get_mut(&victim).and_then(VecDeque::pop_front) {
            self.queued -= 1;
            self.bytes -= data.len();
        }
        if self.queues.get(&victim).is_some_and(VecDeque::is_empty) {
            self.queues.remove(&victim);
            self.turns.retain(|other| *other != victim);
            self.forget_if_closed(&victim);
        }
        true
    }

    /// Which Links' turns come first: 0 for one a message was stored from
    /// (and no bad stamp), 1 for one not known yet, 2 for one that sent bad
    /// stamps.
    fn rank(&self, link: &LinkId) -> u8 {
        match (self.bad.get(link).is_some_and(|bad| *bad > 0), self.good.contains(link)) {
            (true, _) => 2,
            (false, true) => 0,
            (false, false) => 1,
        }
    }

    /// The next transfer to check: the oldest of the Link whose turn it is,
    /// among those ranked first.
    fn next(&mut self) -> Option<(LinkId, Vec<u8>)> {
        loop {
            let best = self.turns.iter().map(|link| self.rank(link)).min()?;
            let at = self.turns.iter().position(|link| self.rank(link) == best)?;
            let link = self.turns.remove(at)?;
            let Some(data) = self.queues.get_mut(&link).and_then(VecDeque::pop_front) else {
                self.queues.remove(&link);
                continue;
            };
            self.queued -= 1;
            self.bytes -= data.len();
            if self.queues.get(&link).is_some_and(|queue| !queue.is_empty()) {
                self.turns.push_back(link);
            } else {
                self.queues.remove(&link);
            }
            self.checking = Some(link);
            return Some((link, data));
        }
    }

    /// A transfer from `link` was checked, at `now`. True if what it found
    /// put the Link over its budget: what it has queued is dropped (counted
    /// in `dropped`), and the caller closes it (unless it has closed).
    fn judged(&mut self, link: LinkId, tally: Tally, now: std::time::Instant, dropped: &mut usize) -> bool {
        self.checking = None;
        let closing = self.closing.contains(&link);
        if tally.fresh > 0 && !closing {
            self.good.insert(link);
        }
        let mut over = false;
        if tally.bad > 0 && !self.over_budget(&link) {
            let trusted = self.good.contains(&link);
            // Judged by the bad stamps before these.
            let budget = if trusted || !self.under_pressure(now) { MAX_BAD_STAMPS_PER_LINK } else { 1 };
            if !trusted {
                self.note_untrusted_bad(tally.bad, now);
            }
            let count = self.bad.entry(link).or_default();
            *count = count.saturating_add(tally.bad);
            if *count >= budget {
                // Over budget from here on, whichever budget it had.
                *count = (*count).max(MAX_BAD_STAMPS_PER_LINK);
                if let Some(queue) = self.queues.remove(&link) {
                    *dropped += queue.len();
                    self.queued -= queue.len();
                    self.bytes -= queue.iter().map(Vec::len).sum::<usize>();
                }
                self.turns.retain(|other| *other != link);
                over = true;
            }
        }
        if closing && !self.queues.contains_key(&link) {
            self.forget_if_closed(&link);
        }
        over && !closing
    }

    fn over_budget(&self, link: &LinkId) -> bool {
        self.bad.get(link).is_some_and(|bad| *bad >= MAX_BAD_STAMPS_PER_LINK)
    }

    fn note_untrusted_bad(&mut self, bad: usize, now: std::time::Instant) {
        for _ in 0..bad.min(UNTRUSTED_BAD_LIMIT) {
            self.untrusted_bad.push_back(now);
        }
        while self.untrusted_bad.len() > UNTRUSTED_BAD_LIMIT {
            self.untrusted_bad.pop_front();
        }
    }

    /// Whether Links nothing was stored from have sent
    /// [`UNTRUSTED_BAD_LIMIT`] bad stamps in the last
    /// [`UNTRUSTED_BAD_WINDOW`].
    fn under_pressure(&self, now: std::time::Instant) -> bool {
        self.untrusted_bad.len() >= UNTRUSTED_BAD_LIMIT
            && self.untrusted_bad.front().is_some_and(|first| now.saturating_duration_since(*first) <= UNTRUSTED_BAD_WINDOW)
    }

    /// The Link closed. What it had queued is still checked (the sender was
    /// told it arrived), unless it goes over its budget; what is known of it
    /// goes once nothing of it is left.
    fn closed(&mut self, link: &LinkId) {
        self.closing.insert(*link);
        if !self.queues.contains_key(link) {
            self.forget_if_closed(link);
        }
    }

    /// Forget a closed Link, unless its transfer is being checked (then
    /// once it has been).
    fn forget_if_closed(&mut self, link: &LinkId) {
        if self.checking == Some(*link) {
            return;
        }
        if self.closing.remove(link) {
            self.bad.remove(link);
            self.good.remove(link);
        }
    }
}

/// Check a transfer from `link` off the event task, and send what was found
/// on `judged`; if the check panicked, as if nothing was found, so the event
/// task never waits on it for good.
fn check_in_background<F>(link: LinkId, judged: mpsc::UnboundedSender<(LinkId, Tally)>, check: F)
where
    F: FnOnce() -> Tally + Send + 'static,
{
    let task = tokio::task::spawn_blocking(check);
    tokio::spawn(async move {
        let tally = task.await.unwrap_or_else(|e| {
            tracing::warn!("checking a propagation transfer failed: {e}");
            Tally::default()
        });
        let _ = judged.send((link, tally));
    });
}

/// What taking in messages needs, shared by the Link handlers and this
/// client's own sends to its node.
struct Ingest {
    node: Arc<Mutex<PropagationNode>>,
    counters: Counters,
    /// The smallest stamp value taken.
    min_stamp: f64,
    /// This client's LXMF address and identity: messages for it are
    /// delivered here.
    lxmf_hash: Hash,
    identity: Identity,
    /// Its ratchets, to decrypt with.
    ring: PathBuf,
    /// Complete messages for this client, to read as received.
    deliver: mpsc::UnboundedSender<Vec<u8>>,
}

impl Ingest {
    /// Take in a transfer of propagated messages (`[timebase, [message,
    /// ...]]`); `several` only from a peer. How many were kept or delivered.
    fn take(&self, data: &[u8], several: bool) -> Result<usize, String> {
        self.take_tallied(data, several, &mut Tally::default())
    }

    /// A transfer from a Link, checked: what it held, for the Link's budget
    /// and standing (see [`Intake::judged`]). One that isn't a transfer, or
    /// is several messages from a client, counts as a bad stamp.
    fn judge(&self, data: &[u8], several: bool) -> Tally {
        let mut tally = Tally::default();
        if let Err(e) = self.take_tallied(data, several, &mut tally) {
            tracing::debug!("propagation transfer not taken: {e}");
            tally.bad += 1;
        }
        tally
    }

    /// [`Self::take`], adding to `tally` the messages newly stored or
    /// delivered here, and those that weren't messages or whose stamps were
    /// checked and found short (each such check cost a workblock).
    fn take_tallied(&self, data: &[u8], several: bool, tally: &mut Tally) -> Result<usize, String> {
        let value = rmpv::decode::read_value(&mut &data[..]).map_err(|e| format!("not a propagation transfer: {e}"))?;
        let messages = match value {
            Value::Array(parts) if parts.len() >= 2 => match parts.into_iter().nth(1) {
                Some(Value::Array(messages)) => messages,
                _ => return Err("not a propagation transfer".into()),
            },
            _ => return Err("not a propagation transfer".into()),
        };
        if messages.len() > 1 && !several {
            self.counters.rejected.fetch_add(messages.len() as u64, Ordering::Relaxed);
            return Err("several messages at once from a client (only peers may)".into());
        }
        let (mut taken, mut bad) = (0, 0);
        let total = messages.len();
        for (index, message) in messages.into_iter().enumerate() {
            let Value::Binary(data) = message else {
                tally.bad += 1;
                continue;
            };
            if bad >= MAX_BAD_STAMPS {
                let left = (total - index) as u64;
                self.counters.rejected.fetch_add(left, Ordering::Relaxed);
                tracing::warn!(
                    "refused the last {left} messages of a propagation transfer: {MAX_BAD_STAMPS} before them had no valid stamp"
                );
                break;
            }
            // One held already is a duplicate, fine, and not worth checking
            // its stamp again (the transient id is the hash of all but the
            // stamp).
            if data.len() > STAMP_SIZE && self.node.lock().unwrap().contains(&rns_crypto::sha::full_hash(&data[..data.len() - STAMP_SIZE]))
            {
                taken += 1;
                continue;
            }
            // The stamp proves the sender's work; without it, it's not taken.
            // It is checked here, once, off the node's lock: the node then
            // reserves the message without checking it again.
            let blob = (data.len() > LXMF_OVERHEAD + STAMP_SIZE)
                .then(|| ValidatedOpaquePropagationBlob::validate(data.clone().into(), self.min_stamp))
                .flatten();
            let Some(blob) = blob else {
                self.counters.rejected.fetch_add(1, Ordering::Relaxed);
                bad += 1;
                tally.bad += 1;
                continue;
            };
            let transient_id = blob.transient_id();
            let lxmf_data = &data[..data.len() - STAMP_SIZE];
            if lxmf_data.len() > 16 && lxmf_data[..16] == self.lxmf_hash {
                match super::ratchets::decrypt(&self.identity, &self.ring, &lxmf_data[16..]) {
                    Ok(plaintext) => {
                        let mut full = self.lxmf_hash.to_vec();
                        full.extend_from_slice(&plaintext);
                        let _ = self.deliver.send(full);
                        self.counters.delivered_here.fetch_add(1, Ordering::Relaxed);
                        tally.fresh += 1;
                        taken += 1;
                    }
                    Err(e) => tracing::warn!("could not decrypt a propagated message for us: {e}"),
                }
                continue;
            }
            // Reserved under the lock, written without it, then committed.
            let admission = self.node.lock().unwrap().plan_opaque_admission(blob);
            let plan = match admission {
                PropagationAdmission::Reserved(plan) => *plan,
                // Held already, or being stored from another transfer: a
                // duplicate is fine.
                PropagationAdmission::CommittedDuplicate(_) | PropagationAdmission::Pending(_) | PropagationAdmission::AlreadyProcessed => {
                    taken += 1;
                    continue;
                }
                PropagationAdmission::Rejected => {
                    // Over a limit, or held without a way to check it.
                    if self.node.lock().unwrap().contains(&transient_id) {
                        taken += 1;
                    } else {
                        self.counters.rejected.fetch_add(1, Ordering::Relaxed);
                    }
                    continue;
                }
            };
            let size = plan.size();
            match plan.persist() {
                Ok(persisted) => {
                    if self.node.lock().unwrap().commit_store_write(persisted) {
                        self.counters.received.fetch_add(1, Ordering::Relaxed);
                        tally.fresh += 1;
                        taken += 1;
                    }
                }
                Err(e) => {
                    self.node.lock().unwrap().abort_store_write(&transient_id, size);
                    tracing::warn!("could not store a propagated message: {e}");
                }
            }
        }
        Ok(taken)
    }
}

/// This client's own node, for its own sends to it (a Link to a
/// destination it hosts itself isn't possible).
#[derive(Clone)]
pub struct LocalNode {
    pub hash: Hash,
    ingest: Arc<Ingest>,
    /// Propagation nodes' announces, for peering.
    heard: mpsc::UnboundedSender<HeardNode>,
}

impl std::fmt::Debug for LocalNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalNode").field("hash", &hex::encode(self.hash)).finish()
    }
}

impl LocalNode {
    /// Take in a propagation transfer as if a client had sent it.
    pub fn submit(&self, transfer: &[u8]) -> Result<(), String> {
        match self.ingest.take(transfer, false)? {
            0 => Err("Your propagation node didn't take the message (its stamp, or a limit)".into()),
            _ => Ok(()),
        }
    }

    /// The stamp cost it asks.
    pub fn stamp_cost(&self) -> f64 {
        self.ingest.min_stamp + PROPAGATION_COST_FLEX
    }

    /// A propagation node announced: perhaps one to peer with.
    pub fn heard(&self, node: HeardNode) {
        let _ = self.heard.send(node);
    }
}

/// A running node: dropping it stops it, [`HostedPn::stop`] waits until
/// it has (before it's started again, so the old destination's removal
/// can't follow the new one's registration).
pub struct HostedPn {
    pub hash: Hash,
    ingest: Arc<Ingest>,
    peers: Arc<Mutex<HashSet<[u8; 16]>>>,
    heard: mpsc::UnboundedSender<HeardNode>,
    peering: Arc<Mutex<PeeringStats>>,
    peering_task: tokio::task::JoinHandle<()>,
    /// Dropped to stop the event task, which then closes the destination.
    stop: Option<oneshot::Sender<()>>,
    events: Option<tokio::task::JoinHandle<()>>,
    announcer: tokio::task::JoinHandle<()>,
}

impl Drop for HostedPn {
    fn drop(&mut self) {
        self.announcer.abort();
        self.peering_task.abort();
    }
}

/// What the implementation is called, in its announce (as `lxmd` names
/// itself), beside the LXMF version it follows.
const PN_META_IMPL_NAME: u8 = 0xFE;
const LXMF_VERSION: &str = "1.2.0";

/// Its announce data: name, stamp cost and limits.
fn announce_data(config: &PnConfig) -> Vec<u8> {
    let mut data = PropagationNodeAnnounceData::new(
        true,
        config.transfer_kb,
        SYNC_LIMIT as u64,
        config.stamp_cost,
        PROPAGATION_COST_FLEX,
        PEERING_COST,
    );
    // Its name as Python LXMF sends it, cleaned as rettui cleans others'
    // (lxmf-core's `set_name` drops emoji).
    if let Some(name) = crate::names::clean(&config.name) {
        data.metadata.insert(lxmf_core::constants::PN_META_NAME, name.into_bytes());
    }
    data.metadata.insert(PN_META_IMPL_NAME, b"rettui".to_vec());
    data.metadata.insert(lxmf_core::constants::PN_META_VERSION, LXMF_VERSION.as_bytes().to_vec());
    get_propagation_node_app_data(&data)
}

/// A peer error code, as `/get` and `/offer` answer it.
fn error_reply(error: PeerError) -> RequestOutcome {
    RequestOutcome::ReplyValue(encode(&Value::from(error as u8)))
}

/// Why an `/offer` isn't given to the node, if it isn't: it is from no one
/// who identified or from someone blocked, or its peering key (the first
/// 32 bytes of `[key, [id, ...]]`, written as MessagePack writes them) isn't
/// the work `cost` asks over both identities. rsLXMF notes every offer it
/// reads, for 3 minutes, in a table nothing empties; with only offers that
/// proved their work given to it, adding an entry costs that work.
fn offer_refusal(data: &[u8], remote: Option<&Hash>, blocked: bool, local: &Hash, cost: f64) -> Option<PeerError> {
    let Some(remote) = remote else { return Some(PeerError::NoIdentity) };
    if blocked {
        return Some(PeerError::NoAccess);
    }
    let [0x92, 0xc4, 0x20, rest @ ..] = data else { return Some(PeerError::InvalidData) };
    let Some(key) = rest.get(..32).and_then(|key| <[u8; 32]>::try_from(key).ok()) else { return Some(PeerError::InvalidData) };
    let mut peering_id = [0u8; 32];
    peering_id[..16].copy_from_slice(local);
    peering_id[16..].copy_from_slice(remote);
    (!validate_peering_key(&peering_id, &key, cost)).then_some(PeerError::InvalidKey)
}

impl HostedPn {
    /// Start the node on the running Reticulum instance, with this client's
    /// identity (its LXMF address is `lxmf_hash`, its ratchets in `ring`).
    /// Messages for this client go to `deliver`; `known` has the keys heard
    /// before (static peers' among them).
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        runtime: &ReticulumHandle,
        identity: &Identity,
        lxmf_hash: Hash,
        ring: &Path,
        config: &PnConfig,
        known: &crate::net::Known,
        deliver: mpsc::UnboundedSender<Vec<u8>>,
        blocked: crate::store::Blocked,
    ) -> Result<Self, String> {
        let hash = Destination::hash_from_name_and_identity(PROPAGATION_ASPECT, Some(&identity.hash));
        let min_stamp = (config.stamp_cost - PROPAGATION_COST_FLEX).max(0.0);
        let node_config = PropagationNodeConfig {
            max_storage: usize::try_from(config.storage_bytes).unwrap_or(usize::MAX),
            max_message_age: MESSAGE_EXPIRY,
            min_stamp_cost: min_stamp,
            peering_cost: PEERING_COST,
            max_message_size: usize::try_from(config.transfer_kb * 1000).unwrap_or(usize::MAX),
            max_offer_size: SYNC_LIMIT * 1000,
        };
        let dir = config.dir.clone();
        let node = tokio::task::spawn_blocking(move || PropagationNode::with_storage(node_config, hash, dir))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| {
                if lxmf_core::propagation_store_v2::is_migration_required(&e) {
                    format!(
                        "The message store {} is from before rettui 2.0 and has to be converted first: stop rettui and run `rettui migrate-propagation` (the old messages are kept apart, not served). See https://github.com/rettui-dev/rettui/wiki/Hosting-a-Node#upgrading-the-propagation-node",
                        config.dir.display()
                    )
                } else {
                    format!("Could not open the message store {}: {e}", config.dir.display())
                }
            })?;
        let node = Arc::new(Mutex::new(node));
        let ingest = Arc::new(Ingest {
            node: node.clone(),
            counters: Counters::default(),
            min_stamp,
            lxmf_hash,
            identity: identity.clone(),
            ring: ring.to_path_buf(),
            deliver,
        });
        let peers: Arc<Mutex<HashSet<[u8; 16]>>> = Arc::default();

        // Clients may send up to the transfer limit; peers up to a sync's.
        let size_gate = {
            let (peers, client_limit) = (peers.clone(), config.transfer_kb as usize * 1000);
            ResourceAcceptPolicy::new(move |link, advertisement| {
                let limit = if peers.lock().unwrap().contains(&link) { SYNC_LIMIT * 1000 } else { client_limit };
                advertisement.data_size <= limit
            })
        };
        let mut destination = runtime
            .register_destination(
                identity.clone(),
                PROPAGATION_ASPECT,
                DestinationRuntimeOptions {
                    proof_strategy: ProofStrategy::ProveAll,
                    resource_strategy: ResourceStrategy::AcceptApp,
                    resource_accept: Some(size_gate),
                    default_app_data: Some(announce_data(config)),
                    ..DestinationRuntimeOptions::default()
                },
            )
            .await
            .map_err(|e| format!("Could not register the propagation node: {e}"))?;

        // `/get`: the messages for whoever identified on the Link.
        let (get_node, get_ingest, get_blocked) = (node.clone(), ingest.clone(), blocked.clone());
        destination
            .handle
            .register_request_handler(lxmf_core::constants::MESSAGE_GET_PATH, AllowPolicy::AllowAll, Vec::new(), false, move |request| {
                let Some(remote) = request.remote_identity else { return error_reply(PeerError::NoIdentity) };
                if get_blocked.has_identity(&remote.hash) {
                    return error_reply(PeerError::NoAccess);
                }
                let client = Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&remote.hash));
                let action = get_node.lock().unwrap().handle_get_request(&request.data, &client);
                let (reply, served) = action.into_response_with_served_count();
                get_ingest.counters.served.fetch_add(served, Ordering::Relaxed);
                RequestOutcome::ReplyValue(reply)
            })
            .await
            .map_err(|e| format!("Could not start the propagation node: {e}"))?;
        // `/offer`: a peer lists what it has, proving its peering key; a Link
        // that did may send several messages at once.
        let (offer_node, offer_peers, local_identity, offer_blocked) = (node.clone(), peers.clone(), identity.hash, blocked.clone());
        destination
            .handle
            .register_request_handler(lxmf_core::constants::OFFER_REQUEST_PATH, AllowPolicy::AllowAll, Vec::new(), false, move |request| {
                let remote = request.remote_identity.as_ref().map(|identity| identity.hash);
                let blocked = remote.is_some_and(|hash| offer_blocked.has_identity(&hash));
                if let Some(error) = offer_refusal(&request.data, remote.as_ref(), blocked, &local_identity, PEERING_COST) {
                    return error_reply(error);
                }
                let peer_hash = remote.map_or([0; 16], |hash| Destination::hash_from_name_and_identity(PROPAGATION_ASPECT, Some(&hash)));
                let context = OfferRequestContext {
                    peer_hash,
                    identity_known: remote.is_some(),
                    is_throttled: false,
                    access_allowed: !blocked,
                    local_identity_hash: Some(&local_identity),
                    remote_identity_hash: remote.as_ref(),
                };
                let reply = offer_node.lock().unwrap().handle_offer_request(&request.data, context);
                // Wanting some, all or none: the key was good.
                let accepted = matches!(rmpv::decode::read_value(&mut reply.as_slice()), Ok(Value::Boolean(_) | Value::Array(_)));
                if accepted {
                    offer_peers.lock().unwrap().insert(request.link_id);
                }
                RequestOutcome::ReplyValue(reply)
            })
            .await
            .map_err(|e| format!("Could not start the propagation node: {e}"))?;

        let announcer = destination.handle.clone();
        // Transfers in (Resources, or small ones as packets), and Links
        // closing.
        let (event_ingest, event_peers) = (ingest.clone(), peers.clone());
        let (stop, mut stopped) = oneshot::channel::<()>();
        let events = tokio::spawn(async move {
            let events = &mut destination.events;
            // Transfers are queued as they come and checked one at a time,
            // off this task, taking turns between Links (see `Intake`).
            let mut intake = Intake::default();
            let (judged_tx, mut judged_rx) = mpsc::unbounded_channel::<(LinkId, Tally)>();
            let mut busy = false;
            loop {
                if !busy && let Some((link, data)) = intake.next() {
                    busy = true;
                    let several = event_peers.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&link);
                    let ingest = event_ingest.clone();
                    check_in_background(link, judged_tx.clone(), move || ingest.judge(&data, several));
                }
                let (data, link) = tokio::select! {
                    _ = &mut stopped => break,
                    Some((link, tally)) = judged_rx.recv(), if busy => {
                        busy = false;
                        let mut dropped = 0;
                        let over = intake.judged(link, tally, std::time::Instant::now(), &mut dropped);
                        event_ingest.counters.rejected.fetch_add(dropped as u64, Ordering::Relaxed);
                        if over {
                            tracing::debug!("closed a Link to the propagation node: too many messages on it had no valid stamp");
                            let _ = destination.handle.close_link(link, CloseReason::DestinationClosed, true).await;
                            // The library says so on `links_closed` too; this
                            // doesn't depend on it.
                            intake.closed(&link);
                        }
                        continue;
                    }
                    Some(completion) = events.resource_completions.recv() => (completion.data, completion.link_id),
                    Some((data, link)) = events.link_packets.recv() => (data, link),
                    Some(link) = events.links_closed.recv() => {
                        event_peers.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&link);
                        intake.closed(&link);
                        continue;
                    }
                    // Nothing else is used; taken so nothing waits on it.
                    Some(_) = events.packets.recv() => continue,
                    Some(_) = events.links_established.recv() => continue,
                    Some(_) = events.links_identified.recv() => continue,
                    Some(_) = events.link_packet_proofs.recv() => continue,
                    Some(_) = events.resource_proofs.recv() => continue,
                    Some(_) = events.resource_events.recv() => continue,
                    Some(_) = events.channel_messages.recv() => continue,
                    else => break,
                };
                let mut shed = 0;
                let refused = !intake.push(link, data, &mut shed);
                let turned_away = shed + usize::from(refused);
                if turned_away > 0 {
                    event_ingest.counters.rejected.fetch_add(turned_away as u64, Ordering::Relaxed);
                    tracing::debug!("{turned_away} propagation transfer(s) refused unchecked: too many waiting, or bad stamps before");
                }
            }
            let _ = destination.close().await;
        });
        // Announce (after interfaces have had a moment), then every so often;
        // cull old messages now and then.
        let (handle, announcing, cull_node) = (announcer, config.clone(), node.clone());
        let announcer = tokio::spawn(async move {
            let config = announcing;
            let announce = async || {
                let options =
                    DestinationAnnounceOptions { app_data: Some(announce_data(&config)), ..DestinationAnnounceOptions::default() };
                if let Err(e) = handle.announce(options).await {
                    tracing::warn!("propagation node announce failed: {e}");
                }
            };
            tokio::time::sleep(Duration::from_secs(3)).await;
            announce().await;
            // A timer's first tick is at once; the announce was just made.
            let every = config.announce_interval.unwrap_or(Duration::from_secs(86_400));
            let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
            let mut cull = tokio::time::interval(Duration::from_secs(10 * 60));
            loop {
                tokio::select! {
                    _ = timer.tick(), if config.announce_interval.is_some() => announce().await,
                    _ = cull.tick() => {
                        let node = cull_node.clone();
                        let _ = tokio::task::spawn_blocking(move || node.lock().unwrap().tick()).await;
                    }
                }
            }
        });
        // Peering out, with the peers saved last time.
        let saved = {
            let node = node.clone();
            tokio::task::spawn_blocking(move || node.lock().unwrap().load_peers()).await.unwrap_or_default()
        };
        let mut peering_peers = Peers::new(config.peering.clone(), hash, saved);
        for peer in &config.peering.static_peers {
            let key = known.lock().unwrap().public_key_of(*peer);
            if let Some(identity) = key.and_then(|key| Identity::from_public_key(&key).ok()) {
                peering_peers.know(*peer, identity.hash);
            }
        }
        let static_heard: Vec<HeardNode> = config
            .peering
            .static_peers
            .iter()
            .filter_map(|peer| {
                let app_data = known.lock().unwrap().app_data_of(*peer)?;
                Some(HeardNode { hash: *peer, identity: None, hops: 0, path_response: true, app_data })
            })
            .collect();
        for heard in &static_heard {
            peering_peers.heard(heard);
        }
        let (heard, heard_rx) = mpsc::unbounded_channel();
        let peering = Arc::new(Mutex::new(PeeringStats::default()));
        let peering_task =
            tokio::spawn(super::peering::run(runtime.clone(), identity.clone(), node.clone(), peering_peers, heard_rx, peering.clone()));
        Ok(Self { hash, ingest, peers, heard, peering, peering_task, stop: Some(stop), events: Some(events), announcer })
    }

    /// Stop the node, and wait (a little) until its destination is gone.
    pub async fn stop(mut self) {
        self.announcer.abort();
        self.peering_task.abort();
        drop(self.stop.take());
        if let Some(events) = self.events.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), events).await;
        }
    }

    /// What it holds and has done.
    pub fn stats(&self) -> PnStats {
        let (messages, bytes) = {
            let node = self.ingest.node.lock().unwrap();
            (node.message_count(), node.total_size())
        };
        let (peered, sent_to_peers) = {
            let peering = self.peering.lock().unwrap();
            (peering.peered, peering.sent)
        };
        let counters = &self.ingest.counters;
        PnStats {
            messages,
            bytes,
            received: counters.received.load(Ordering::Relaxed),
            delivered_here: counters.delivered_here.load(Ordering::Relaxed),
            served: counters.served.load(Ordering::Relaxed),
            rejected: counters.rejected.load(Ordering::Relaxed),
            peers: self.peers.lock().unwrap().len(),
            peered,
            sent_to_peers,
        }
    }

    /// For this client's own sends to its node.
    pub fn local(&self) -> LocalNode {
        LocalNode { hash: self.hash, ingest: self.ingest.clone(), heard: self.heard.clone() }
    }
}

#[cfg(test)]
mod tests {
    use lxmf_core::message_api::{DeliveryMethod, LxMessage, LxMessageDraft, MessageError};
    use lxmf_core::propagation_send::{AdvertisedNodePolicy, EncryptionBinding, PreparedNode, PropagationEnvelope};
    use lxmf_core::stamper::validate_pn_stamp;

    use std::time::Instant;

    use super::*;

    #[test]
    fn its_name_goes_out_with_emoji() {
        let config = |name: &str| PnConfig {
            name: name.into(),
            stamp_cost: 16.0,
            storage_bytes: 1,
            transfer_kb: 256,
            dir: PathBuf::new(),
            announce_interval: None,
            peering: PeeringConfig::default(),
        };
        let name_of = |name: &str| {
            let data = lxmf_core::handlers::parse_pn_announce_data(&announce_data(&config(name))).unwrap();
            data.metadata.get(&lxmf_core::constants::PN_META_NAME).map(|n| String::from_utf8(n.clone()).unwrap())
        };
        assert_eq!(name_of("📮 Post\u{202E}").as_deref(), Some("📮 Post"));
        assert_eq!(name_of(" "), None);
    }

    fn ingest(dir: &std::path::Path, stamp_cost: f64) -> (Ingest, mpsc::UnboundedReceiver<Vec<u8>>, Identity) {
        let identity = Identity::new();
        let lxmf_hash = Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&identity.hash));
        let config = PropagationNodeConfig {
            max_storage: 1_000_000,
            min_stamp_cost: stamp_cost - PROPAGATION_COST_FLEX,
            ..PropagationNodeConfig::default()
        };
        let node = PropagationNode::with_storage(config, [9; 16], dir.to_path_buf()).unwrap();
        let (deliver, delivered) = mpsc::unbounded_channel();
        let ingest = Ingest {
            node: Arc::new(Mutex::new(node)),
            counters: Counters::default(),
            min_stamp: stamp_cost - PROPAGATION_COST_FLEX,
            lxmf_hash,
            identity: identity.clone(),
            ring: dir.join("no.ratchets"),
            deliver,
        };
        (ingest, delivered, identity)
    }

    /// A message to `to` (encrypted for `recipient`), packed for a
    /// propagation node with a stamp of `cost`, as a client sends it.
    fn transfer(to: Hash, recipient: &Identity, cost: f64) -> Vec<u8> {
        stamped(to, recipient, cost).0
    }

    /// [`transfer`], and what its stamp is worth (at least `cost`; a stamp
    /// of no work is worth whatever its hash happens to be).
    fn stamped(to: Hash, recipient: &Identity, cost: f64) -> (Vec<u8>, f64) {
        let message = LxMessageDraft::new(to, [3; 16], "", "kept for you", DeliveryMethod::Propagated).unwrap();
        let message = message.sign(&Identity::new().get_signing_key().unwrap()).unwrap().finalize(None).unwrap();
        let mut envelope =
            PropagationEnvelope::new(message, EncryptionBinding::for_recipient(&recipient.get_public_key(), None), cost, |plain| {
                recipient.encrypt(plain, None).map_err(|e| MessageError::PackFailed(e.to_string()))
            })
            .unwrap();
        let policy = AdvertisedNodePolicy { stamp_cost: cost, transfer_limit_kb: f64::MAX, recorded_at: 0. };
        let packed = envelope.pack_for(PreparedNode { node: [9; 16], policy }, 1_800_000_000.).unwrap();
        (packed, envelope.stamp_value())
    }

    #[test]
    fn messages_are_kept_for_others_and_delivered_here_for_us() {
        let dir = std::env::temp_dir().join(format!("rettui-pn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (ingest, mut delivered, ours) = ingest(&dir, 13.0);
        let local = LocalNode { hash: [9; 16], ingest: Arc::new(ingest), heard: mpsc::unbounded_channel().0 };
        assert_eq!(local.stamp_cost(), 13.0);
        // For someone else: kept (once, however often it's sent), and listed
        // for them.
        let bob = Identity::new();
        let bob_hash = Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&bob.hash));
        let for_bob = transfer(bob_hash, &bob, 13.0);
        local.submit(&for_bob).unwrap();
        local.submit(&for_bob).unwrap();
        assert_eq!(local.ingest.node.lock().unwrap().message_count(), 1);
        let list = local.ingest.node.lock().unwrap().handle_get_request(&encode(&Value::Array(vec![Value::Nil, Value::Nil])), &bob_hash);
        let lxmf_core::propagation_node::GetRequestAction::Respond(reply) = list else { panic!() };
        assert!(matches!(rmpv::decode::read_value(&mut reply.as_slice()), Ok(Value::Array(ids)) if ids.len() == 1));
        // For us: delivered, not kept.
        let us = local.ingest.lxmf_hash;
        local.submit(&transfer(us, &ours, 13.0)).unwrap();
        let full = delivered.try_recv().unwrap();
        assert_eq!(&full[..16], &us);
        assert_eq!(LxMessage::unpack(&full).unwrap().content_text(), "kept for you");
        // Too little work: turned away. (A stamp of no work has enough by
        // chance about one time in a thousand: one that hasn't.) Several at
        // once: only from a peer.
        let min = local.ingest.min_stamp;
        let (short, _) = std::iter::repeat_with(|| stamped(bob_hash, &bob, 0.0)).find(|(_, value)| *value < min).unwrap();
        assert!(local.submit(&short).is_err());
        let stats = &local.ingest.counters;
        assert_eq!((stats.received.load(Ordering::Relaxed), stats.delivered_here.load(Ordering::Relaxed)), (1, 1));
        assert!(stats.rejected.load(Ordering::Relaxed) >= 1);
        let one = rmpv::decode::read_value(&mut transfer(bob_hash, &bob, 13.0).as_slice()).unwrap();
        let Value::Array(parts) = one else { panic!() };
        let blob = parts[1].as_array().unwrap()[0].clone();
        let two = encode(&Value::Array(vec![parts[0].clone(), Value::Array(vec![blob.clone(), blob])]));
        assert!(local.ingest.take(&two, false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_isnt_a_transfer_and_messages_for_us_that_dont_open() {
        let dir = std::env::temp_dir().join(format!("rettui-pn-odd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (ingest, mut delivered, _) = ingest(&dir, 13.0);
        // Not msgpack, not a list, a list without messages.
        assert!(ingest.take(&[0xc1], false).is_err());
        assert!(ingest.take(&encode(&Value::from(1)), false).is_err());
        assert!(ingest.take(&encode(&Value::Array(vec![Value::Nil, Value::from(2)])), false).is_err());
        // Several at once from a peer: each taken.
        let bob = Identity::new();
        let bob_hash = Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&bob.hash));
        let parts = |transfer: Vec<u8>| match rmpv::decode::read_value(&mut transfer.as_slice()).unwrap() {
            Value::Array(parts) => parts,
            _ => panic!(),
        };
        let (first, second) = (parts(transfer(bob_hash, &bob, 13.0)), parts(transfer(bob_hash, &bob, 13.0)));
        let both = encode(&Value::Array(vec![first[0].clone(), Value::Array(vec![first[1][0].clone(), second[1][0].clone()])]));
        assert_eq!(ingest.take(&both, true), Ok(2));
        // For us, but encrypted for someone else: not delivered.
        let wrong = transfer(ingest.lxmf_hash, &Identity::new(), 13.0);
        let _ = ingest.take(&wrong, false);
        assert!(delivered.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_offer_reaches_the_node_only_with_a_peering_key_that_is_work() {
        use lxmf_core::constants::STAMP_WORKBLOCK_EXPAND_ROUNDS_PEERING;
        let (local, remote, cost) = ([1; 16], [2; 16], 8.0);
        let mut id = [0u8; 32];
        id[..16].copy_from_slice(&local);
        id[16..].copy_from_slice(&remote);
        let Some((key, _)) = lxmf_core::stamper::generate_stamp(
            &id,
            cost,
            STAMP_WORKBLOCK_EXPAND_ROUNDS_PEERING,
            &lxmf_core::stamper::StampControl::default(),
        )
        .unwrap() else {
            panic!("no key")
        };
        let offer = |key: &[u8]| encode(&Value::Array(vec![Value::Binary(key.to_vec()), Value::Array(vec![Value::Binary(vec![7; 32])])]));
        let refusal = |data: &[u8], remote: Option<&Hash>, blocked: bool| offer_refusal(data, remote, blocked, &local, cost);
        // Work over these two identities: through.
        assert_eq!(refusal(&offer(&key), Some(&remote), false), None);
        // Work that isn't (nothing, or someone else's): turned away, so
        // the node keeps no note of it.
        let zeros = offer(&[0; 32]);
        assert!(!lxmf_core::stamper::validate_peering_key(&id, &[0; 32], cost), "a key of nothing is not work");
        assert_eq!(refusal(&zeros, Some(&remote), false), Some(PeerError::InvalidKey));
        let other = [3; 16];
        assert!(refusal(&offer(&key), Some(&other), false).is_some(), "the key is for one identity");
        // Who it is, and a block.
        assert_eq!(refusal(&offer(&key), None, false), Some(PeerError::NoIdentity));
        assert_eq!(refusal(&offer(&key), Some(&remote), true), Some(PeerError::NoAccess));
        // Not an offer: the wrong shape, a key of another length, too short.
        assert_eq!(refusal(&[0xc1], Some(&remote), false), Some(PeerError::InvalidData));
        assert_eq!(refusal(&offer(&[1; 31]), Some(&remote), false), Some(PeerError::InvalidData));
        assert_eq!(refusal(&offer(&key)[..20], Some(&remote), false), Some(PeerError::InvalidData));
        assert_eq!(refusal(&[], Some(&remote), false), Some(PeerError::InvalidData));
    }

    #[test]
    fn a_peers_transfer_of_bad_stamps_or_copies_costs_little_work() {
        let dir = std::env::temp_dir().join(format!("rettui-pn-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (ingest, _delivered, _) = ingest(&dir, 13.0);
        let entry = |transfer: Vec<u8>| match rmpv::decode::read_value(&mut transfer.as_slice()) {
            Ok(Value::Array(parts)) => match &parts[1] {
                Value::Array(messages) => messages[0].clone(),
                _ => panic!(),
            },
            _ => panic!(),
        };
        let several = |entries: Vec<Value>| encode(&Value::Array(vec![Value::F64(1_800_000_000.), Value::Array(entries)]));
        let bob = Identity::new();
        let bob_hash = Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&bob.hash));
        let good = entry(transfer(bob_hash, &bob, 13.0));
        let bad = || {
            let Value::Binary(mut data) = entry(transfer(bob_hash, &bob, 13.0)) else { panic!() };
            let at = data.len() - 1;
            data[at] ^= 0xff;
            Value::Binary(data)
        };
        // A flipped stamp still worth the node's minimum by chance (one in
        // about a thousand) isn't a bad one.
        let min = ingest.min_stamp;
        let bads = |n: usize| (0..n).map(|_| bad()).filter(move |b| !matches!(b, Value::Binary(d) if validate_pn_stamp(d, min).is_some()));
        // Past MAX_BAD_STAMPS bad stamps, the rest isn't looked at.
        let mut entries: Vec<Value> = bads(MAX_BAD_STAMPS + 4).take(MAX_BAD_STAMPS).collect();
        entries.push(good.clone());
        assert_eq!(ingest.take(&several(entries), true), Ok(0));
        assert_eq!(ingest.node.lock().unwrap().message_count(), 0);
        let mut entries: Vec<Value> = bads(MAX_BAD_STAMPS + 4).take(MAX_BAD_STAMPS - 1).collect();
        entries.push(good.clone());
        assert_eq!(ingest.take(&several(entries), true), Ok(1));
        // Copies of one held already are taken without checking their stamps.
        let once = std::time::Instant::now();
        assert_eq!(ingest.take(&several(vec![bad()]), true), Ok(0));
        let one_check = once.elapsed();
        let copies = std::time::Instant::now();
        assert_eq!(ingest.take(&several(vec![good; 40]), true), Ok(40));
        assert!(copies.elapsed() < one_check * 10, "40 copies took {:?}, one check {one_check:?}", copies.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_that_floods_bad_stamps_is_cut_off_and_others_keep_their_turn() {
        let (flood, honest, third) = ([1; 16], [2; 16], [3; 16]);
        let mut intake = Intake::default();
        let (mut shed, mut dropped) = (0, 0);
        // One Link sends more than the whole queue holds: the rest is refused.
        let queued = (0..MAX_QUEUED + 10).filter(|n| intake.push(flood, n.to_le_bytes().to_vec(), &mut shed)).count();
        assert_eq!((queued, shed), (MAX_QUEUED, 0));
        // Another Link's transfer still gets in, in the flood's place, and is
        // checked next, not after the backlog.
        assert!(intake.push(honest, b"honest".to_vec(), &mut shed));
        assert_eq!((shed, intake.queued), (1, MAX_QUEUED));
        assert_eq!(intake.next(), Some((flood, 1usize.to_le_bytes().to_vec())));
        // Bad stamps add up over a Link's transfers: under the budget it
        // stays; at it, what it has queued goes and it is closed.
        assert!(!intake.judged(flood, Tally { fresh: 0, bad: MAX_BAD_STAMPS_PER_LINK - 1 }, Instant::now(), &mut dropped));
        assert_eq!(intake.next(), Some((honest, b"honest".to_vec())));
        assert!(!intake.judged(honest, Tally { fresh: 1, bad: 0 }, Instant::now(), &mut dropped));
        assert_eq!(intake.next().map(|(link, _)| link), Some(flood));
        assert!(intake.judged(flood, Tally { fresh: 0, bad: 1 }, Instant::now(), &mut dropped));
        assert_eq!(dropped, MAX_QUEUED - 3);
        assert!(!intake.push(flood, vec![1], &mut shed), "refused unchecked once over its budget");
        assert!(!intake.judged(flood, Tally { fresh: 0, bad: 5 }, Instant::now(), &mut dropped), "closed once, not again");
        assert_eq!(intake.next(), None);
        assert_eq!((intake.queued, intake.bytes), (0, 0));
        // Once it has closed, the Link is forgotten.
        intake.closed(&flood);
        intake.closed(&honest);
        assert!(intake.bad.is_empty() && intake.good.is_empty() && intake.closing.is_empty());
        assert!(intake.queues.is_empty() && intake.turns.is_empty());
        // Turns go round every Link with something queued.
        for link in [third, honest, third, honest] {
            assert!(intake.push(link, link.to_vec(), &mut shed));
        }
        let order: Vec<LinkId> = std::iter::from_fn(|| intake.next()).map(|(link, _)| link).collect();
        assert_eq!(order, vec![third, honest, third, honest]);
    }

    #[test]
    fn a_client_whose_message_passed_goes_before_a_flood_over_many_links() {
        let mut intake = Intake::default();
        let (mut shed, mut dropped) = (0, 0);
        let client = [0xcc; 16];
        // Many Links, each with a few bad stamps waiting; a client with 20.
        let flood: Vec<LinkId> = (0..200u128).map(u128::to_le_bytes).collect();
        for link in &flood {
            for _ in 0..4 {
                assert!(intake.push(*link, vec![0], &mut shed));
            }
        }
        for n in 0..20u8 {
            assert!(intake.push(client, vec![n], &mut shed));
        }
        // New Links take turns, the client among them...
        let mut turns = 0;
        loop {
            let (link, _) = intake.next().unwrap();
            turns += 1;
            if link == client {
                intake.judged(link, Tally { fresh: 1, bad: 0 }, Instant::now(), &mut dropped);
                break;
            }
            intake.judged(link, Tally { fresh: 0, bad: 1 }, Instant::now(), &mut dropped);
        }
        assert_eq!(turns, flood.len() + 1);
        // ...and once one of its messages passed, its other 19 go before the
        // Links that sent bad stamps.
        for n in 1..20u8 {
            assert_eq!(intake.next(), Some((client, vec![n])));
            intake.judged(client, Tally { fresh: 1, bad: 0 }, Instant::now(), &mut dropped);
        }
        assert_ne!(intake.next().map(|(link, _)| link), Some(client));
    }

    #[test]
    fn what_a_closed_link_sent_is_still_checked_then_it_is_forgotten() {
        let (client, other) = ([1; 16], [2; 16]);
        let mut intake = Intake::default();
        let (mut shed, mut dropped) = (0, 0);
        // A client sends three and goes: all three are still checked (the
        // library proved them to it as they came).
        for n in 0..3u8 {
            assert!(intake.push(client, vec![n], &mut shed));
        }
        intake.closed(&client);
        for n in 0..3u8 {
            let (link, data) = intake.next().unwrap();
            assert_eq!((link, data), (client, vec![n]));
            assert!(!intake.judged(link, Tally { fresh: 1, bad: 0 }, Instant::now(), &mut dropped));
        }
        assert_eq!(dropped, 0);
        assert!(intake.good.is_empty() && intake.bad.is_empty() && intake.closing.is_empty());
        // Closed while its last one is being checked: forgotten once it is.
        assert!(intake.push(other, vec![9], &mut shed));
        let (link, _) = intake.next().unwrap();
        intake.closed(&link);
        assert!(intake.closing.contains(&other));
        assert!(!intake.judged(link, Tally { fresh: 0, bad: 3 }, Instant::now(), &mut dropped));
        assert!(intake.bad.is_empty() && intake.closing.is_empty());
    }

    #[test]
    fn when_the_intake_is_full_links_that_sent_bad_stamps_lose_theirs_first() {
        let mut intake = Intake::default();
        let (mut shed, mut dropped) = (0, 0);
        const EACH: usize = 16;
        let links: Vec<LinkId> = (0..MAX_QUEUED / EACH).map(|n| (n as u128).to_le_bytes()).collect();
        for link in &links {
            for _ in 0..EACH {
                assert!(intake.push(*link, vec![0; 10], &mut shed));
            }
        }
        assert_eq!((intake.queued, shed), (MAX_QUEUED, 0));
        // An honest Link (one a message was taken from) gets in, at the cost
        // of the Link that sent bad stamps, though that one waits no longer.
        let (bad, honest) = (links[3], links[5]);
        intake.judged(bad, Tally { fresh: 0, bad: 1 }, Instant::now(), &mut dropped);
        intake.judged(honest, Tally { fresh: 1, bad: 0 }, Instant::now(), &mut dropped);
        assert!(intake.push(honest, vec![1], &mut shed));
        assert_eq!((shed, intake.queues[&bad].len(), intake.queued), (1, EACH - 1, MAX_QUEUED));
        // The Link with bad stamps can't make room for itself at others' cost.
        assert!(!intake.push(bad, vec![2], &mut shed));
        // With no bad stamps anywhere, a Link nothing was taken from yet gives
        // way before an honest one, even with fewer waiting.
        let mut mixed = Intake::default();
        let (unknown, known) = ([0xcc; 16], [0xdd; 16]);
        assert!(mixed.push(unknown, vec![0], &mut shed) && mixed.push(unknown, vec![0], &mut shed));
        mixed.judged(known, Tally { fresh: 1, bad: 0 }, Instant::now(), &mut dropped);
        while mixed.queued < MAX_QUEUED {
            assert!(mixed.push(known, vec![1], &mut shed));
        }
        assert!(mixed.push(known, vec![2], &mut shed));
        assert_eq!(mixed.queues[&unknown].len(), 1);
        // Among Links alike, the longest gives way, never one only as long as the sender's own.
        let mut even = Intake::default();
        let (a, b) = ([0xaa; 16], [0xbb; 16]);
        while even.queued < MAX_QUEUED {
            assert!(even.push(a, vec![0], &mut shed));
        }
        assert!(even.push(b, vec![1], &mut shed), "a newcomer gets in");
        assert_eq!((even.queues[&a].len(), even.queues[&b].len()), (MAX_QUEUED - 1, 1));
        // Bytes are bounded too: one too big for what's left makes room, and
        // one bigger than all of it is refused.
        let mut big = Intake::default();
        assert!(big.push([1; 16], vec![0; MAX_QUEUED_BYTES / 2], &mut shed));
        assert!(big.push([1; 16], vec![0; MAX_QUEUED_BYTES / 4], &mut shed));
        let before = shed;
        assert!(big.push([2; 16], vec![0; MAX_QUEUED_BYTES / 2], &mut shed));
        assert_eq!((shed - before, big.queued), (1, 2));
        assert!(big.bytes <= MAX_QUEUED_BYTES);
        let mut empty = Intake::default();
        assert!(empty.push([3; 16], vec![0; MAX_QUEUED_BYTES + 1], &mut shed), "one alone is always taken");
        assert!(!empty.push([3; 16], vec![0; 1], &mut shed), "nothing to make room with but its own");
    }

    #[test]
    fn bad_stamps_and_new_messages_are_counted_for_the_links_budget_and_standing() {
        let dir = std::env::temp_dir().join(format!("rettui-pn-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (ingest, _delivered, _) = ingest(&dir, 13.0);
        let bob = Identity::new();
        let bob_hash = Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&bob.hash));
        let min = ingest.min_stamp;
        let (short, _) = std::iter::repeat_with(|| stamped(bob_hash, &bob, 0.0)).find(|(_, value)| *value < min).unwrap();
        assert_eq!(ingest.judge(&short, false), Tally { fresh: 0, bad: 1 });
        // A new message is fresh; the same again is a copy, taken but not
        // fresh, so replaying one message earns a Link nothing.
        let good = transfer(bob_hash, &bob, 13.0);
        assert_eq!(ingest.judge(&good, false), Tally { fresh: 1, bad: 0 });
        assert_eq!(ingest.judge(&good, false), Tally { fresh: 0, bad: 0 });
        assert_eq!(ingest.take(&good, false), Ok(1));
        // Not a transfer, several from a client, or an entry that isn't a
        // message: each counts against the budget.
        assert_eq!(ingest.judge(&[0xc1], false), Tally { fresh: 0, bad: 1 });
        let entries = |entries: Vec<Value>| encode(&Value::Array(vec![Value::F64(1_800_000_000.), Value::Array(entries)]));
        assert_eq!(ingest.judge(&entries(vec![Value::Nil, Value::Nil]), false), Tally { fresh: 0, bad: 1 });
        assert_eq!(ingest.judge(&entries(vec![Value::from(7)]), true), Tally { fresh: 0, bad: 1 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_earns_no_standing_and_a_closed_link_is_still_held_to_its_budget() {
        let (link, other) = ([1; 16], [2; 16]);
        let mut intake = Intake::default();
        let (mut shed, mut dropped) = (0, 0);
        // Only a newly stored message makes a Link one that goes first.
        intake.judged(link, Tally { fresh: 0, bad: 0 }, Instant::now(), &mut dropped);
        assert_eq!(intake.rank(&link), 1);
        intake.judged(link, Tally { fresh: 1, bad: 0 }, Instant::now(), &mut dropped);
        assert_eq!(intake.rank(&link), 0);
        // A Link queues many and closes: its bad stamps still count, and over
        // the budget the rest goes unchecked (no second close).
        for n in 0..100u8 {
            assert!(intake.push(other, vec![n], &mut shed));
        }
        intake.closed(&other);
        let mut checked = 0;
        while let Some((from, _)) = intake.next() {
            assert_eq!(from, other);
            checked += 1;
            assert!(!intake.judged(from, Tally { fresh: 0, bad: 1 }, Instant::now(), &mut dropped), "it has closed already");
        }
        assert_eq!((checked, dropped), (MAX_BAD_STAMPS_PER_LINK, 100 - MAX_BAD_STAMPS_PER_LINK));
        assert!(!intake.bad.contains_key(&other) && intake.closing.is_empty(), "forgotten once nothing is left");
        // Shedding a closed Link's last waiting transfer while another of its
        // transfers is being checked doesn't forget it before that is judged.
        let mut full = Intake::default();
        let (closing, newcomer) = ([3; 16], [4; 16]);
        assert!(full.push(closing, vec![0], &mut shed));
        while full.queued < MAX_QUEUED {
            assert!(full.push(closing, vec![1], &mut shed));
        }
        assert_eq!(full.next().map(|(from, _)| from), Some(closing));
        full.judged(closing, Tally { fresh: 0, bad: 1 }, Instant::now(), &mut dropped);
        assert_eq!(full.next().map(|(from, _)| from), Some(closing));
        full.closed(&closing);
        while full.queues.contains_key(&closing) {
            assert!(full.push(newcomer, vec![2], &mut shed));
        }
        assert!(full.closing.contains(&closing) && full.bad.contains_key(&closing), "still being checked");
        full.judged(closing, Tally { fresh: 0, bad: 1 }, Instant::now(), &mut dropped);
        assert!(!full.closing.contains(&closing) && !full.bad.contains_key(&closing));
    }

    #[test]
    fn a_flood_over_fresh_links_gets_one_check_per_link() {
        let mut intake = Intake::default();
        let (mut shed, mut dropped) = (0, 0);
        let start = Instant::now();
        let trusted = [0xaa; 16];
        intake.judged(trusted, Tally { fresh: 1, bad: 0 }, start, &mut dropped);
        // Until Links nothing was stored from have sent UNTRUSTED_BAD_LIMIT
        // bad stamps, each has the whole budget.
        let fresh = |n: u128| (n + 1000).to_le_bytes();
        for n in 0..(UNTRUSTED_BAD_LIMIT / 4) as u128 {
            assert!(!intake.judged(fresh(n), Tally { fresh: 0, bad: 4 }, start, &mut dropped));
        }
        assert!(intake.under_pressure(start));
        // Then a new Link is closed at its first bad stamp, its queue dropped...
        let next = fresh(9999);
        for _ in 0..10 {
            assert!(intake.push(next, vec![0], &mut shed));
        }
        intake.next();
        assert!(intake.judged(next, Tally { fresh: 0, bad: 1 }, start, &mut dropped));
        assert_eq!(dropped, 9);
        assert!(!intake.push(next, vec![0], &mut shed));
        // ...while one a message was stored from keeps its 16.
        assert!(!intake.judged(trusted, Tally { fresh: 0, bad: MAX_BAD_STAMPS_PER_LINK - 1 }, start, &mut dropped));
        // Once the window has passed, a new Link has the whole budget again.
        let later = start + UNTRUSTED_BAD_WINDOW + Duration::from_secs(1);
        assert!(!intake.under_pressure(later));
        assert!(!intake.judged(fresh(5000), Tally { fresh: 0, bad: 1 }, later, &mut dropped));
        assert!(intake.untrusted_bad.len() <= UNTRUSTED_BAD_LIMIT);
    }

    #[tokio::test]
    async fn a_check_that_panics_still_frees_the_intake() {
        let (judged, mut results) = mpsc::unbounded_channel();
        check_in_background([5; 16], judged.clone(), || panic!("injected: checking failed"));
        assert_eq!(results.recv().await, Some(([5; 16], Tally::default())));
        // The next check goes ahead as usual.
        check_in_background([6; 16], judged, || Tally { fresh: 1, bad: 0 });
        assert_eq!(results.recv().await, Some(([6; 16], Tally { fresh: 1, bad: 0 })));
    }
}
