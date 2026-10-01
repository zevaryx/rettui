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
//!   proving their work) send it the messages it doesn't have. It doesn't
//!   peer with other nodes itself, as Python's `lxmd` with `autopeer` off:
//!   what's sent through it waits here until its recipient collects it.
//! - It announces itself (`lxmf.propagation`) with its name, stamp cost and
//!   limits, and culls old messages (after LXMF's 30 days) to stay within its
//!   storage limit.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lxmf_core::constants::{
    MESSAGE_EXPIRY, PEERING_COST, PROPAGATION_COST_FLEX, PROPAGATION_COST_MIN, PeerError, SYNC_LIMIT,
};
use lxmf_core::handlers::{PropagationNodeAnnounceData, get_propagation_node_app_data};
use lxmf_core::propagation_node::{OfferRequestContext, PropagationNode, PropagationNodeConfig};
use lxmf_core::stamper::validate_pn_stamp;
use rmpv::Value;
use rns_identity::destination::Destination;
use rns_runtime::prelude::*;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::{LXMF_ASPECT, PROPAGATION_ASPECT, encode};
use crate::config::{Paths, Settings};
use crate::net::Hash;

/// How the node runs, from the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PnConfig {
    /// The name it announces.
    pub name: String,
    /// The propagation stamp cost asked of senders.
    pub stamp_cost: u8,
    /// Most bytes of messages kept.
    pub storage_bytes: u64,
    /// Largest transfer a client may send, in kilobytes.
    pub transfer_kb: u64,
    /// Where messages are kept.
    pub dir: PathBuf,
    /// Time between announces (`None`: only when it starts).
    pub announce_interval: Option<Duration>,
}

impl PnConfig {
    /// The node the settings ask for, if any.
    pub fn from_settings(settings: &Settings, paths: &Paths) -> Option<Self> {
        settings.pn_enabled.then(|| Self {
            name: settings.pn_name.clone().unwrap_or_else(|| settings.display_name.clone()),
            stamp_cost: settings.pn_stamp_cost.clamp(u64::from(PROPAGATION_COST_MIN), 254) as u8,
            storage_bytes: settings.pn_storage_mb.max(1).saturating_mul(1_000_000),
            transfer_kb: settings.pn_transfer_kb.clamp(1, SYNC_LIMIT as u64),
            dir: paths.propagation.clone(),
            announce_interval: (settings.announce_interval_mins > 0)
                .then(|| Duration::from_secs(settings.announce_interval_mins * 60)),
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
}

#[derive(Default)]
struct Counters {
    received: AtomicU64,
    delivered_here: AtomicU64,
    served: AtomicU64,
    rejected: AtomicU64,
}

/// What taking in messages needs, shared by the Link handlers and this
/// client's own sends to its node.
struct Ingest {
    node: Arc<Mutex<PropagationNode>>,
    counters: Counters,
    /// The smallest stamp value taken.
    min_stamp: u8,
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
        let mut taken = 0;
        for message in messages {
            let Value::Binary(data) = message else { continue };
            // The stamp proves the sender's work; without it, it's not taken.
            let Some((transient_id, lxmf_data, value, stamp)) = validate_pn_stamp(&data, self.min_stamp) else {
                self.counters.rejected.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            if lxmf_data.len() > 16 && lxmf_data[..16] == self.lxmf_hash {
                match super::ratchets::decrypt(&self.identity, &self.ring, &lxmf_data[16..]) {
                    Ok(plaintext) => {
                        let mut full = self.lxmf_hash.to_vec();
                        full.extend_from_slice(&plaintext);
                        let _ = self.deliver.send(full);
                        self.counters.delivered_here.fetch_add(1, Ordering::Relaxed);
                        taken += 1;
                    }
                    Err(e) => tracing::warn!("could not decrypt a propagated message for us: {e}"),
                }
                continue;
            }
            // Reserved under the lock, written without it, then committed.
            let plan = self.node.lock().unwrap().plan_accept_stamped_propagated_blob(&lxmf_data, &stamp, value.min(255) as u8);
            let Some(plan) = plan else {
                // Held already (a duplicate is fine), or over a limit.
                if self.node.lock().unwrap().contains(&transient_id) {
                    taken += 1;
                } else {
                    self.counters.rejected.fetch_add(1, Ordering::Relaxed);
                }
                continue;
            };
            let size = plan.size();
            match plan.persist() {
                Ok(persisted) => {
                    if self.node.lock().unwrap().commit_store_write(persisted) {
                        self.counters.received.fetch_add(1, Ordering::Relaxed);
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
    pub fn stamp_cost(&self) -> u8 {
        self.ingest.min_stamp + PROPAGATION_COST_FLEX
    }
}

/// A running node: dropping it stops it, [`HostedPn::stop`] waits until
/// it has (before it's started again, so the old destination's removal
/// can't follow the new one's registration).
pub struct HostedPn {
    pub hash: Hash,
    ingest: Arc<Ingest>,
    peers: Arc<Mutex<HashSet<[u8; 16]>>>,
    /// Dropped to stop the event task, which then closes the destination.
    stop: Option<oneshot::Sender<()>>,
    events: Option<tokio::task::JoinHandle<()>>,
    announcer: tokio::task::JoinHandle<()>,
}

impl Drop for HostedPn {
    fn drop(&mut self) {
        self.announcer.abort();
    }
}

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
    data.set_name(&config.name);
    get_propagation_node_app_data(&data)
}

/// A peer error code, as `/get` and `/offer` answer it.
fn error_reply(error: PeerError) -> RequestOutcome {
    RequestOutcome::Reply(encode(&Value::from(error as u8)))
}

impl HostedPn {
    /// Start the node on the running Reticulum instance, with this client's
    /// identity (its LXMF address is `lxmf_hash`, its ratchets in `ring`).
    /// Messages for this client go to `deliver`.
    pub async fn start(
        runtime: &ReticulumHandle,
        identity: &Identity,
        lxmf_hash: Hash,
        ring: &Path,
        config: &PnConfig,
        deliver: mpsc::UnboundedSender<Vec<u8>>,
    ) -> Result<Self, String> {
        let hash = Destination::hash_from_name_and_identity(PROPAGATION_ASPECT, Some(&identity.hash));
        let min_stamp = config.stamp_cost.saturating_sub(PROPAGATION_COST_FLEX);
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
            .map_err(|e| format!("Could not open the message store {}: {e}", config.dir.display()))?;
        let node = Arc::new(Mutex::new(node));
        let ingest = Arc::new(Ingest { node: node.clone(), counters: Counters::default(), min_stamp, lxmf_hash, identity: identity.clone(), ring: ring.to_path_buf(), deliver });
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
        let (get_node, get_ingest) = (node.clone(), ingest.clone());
        destination
            .handle
            .register_request_handler(lxmf_core::constants::MESSAGE_GET_PATH, AllowPolicy::AllowAll, Vec::new(), false, move |request| {
                let Some(remote) = request.remote_identity else { return error_reply(PeerError::NoIdentity) };
                let client = Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&remote.hash));
                let action = get_node.lock().unwrap().handle_get_request(&request.data, &client);
                let (reply, served) = action.into_response_with_served_count();
                get_ingest.counters.served.fetch_add(served, Ordering::Relaxed);
                RequestOutcome::Reply(reply)
            })
            .await
            .map_err(|e| format!("Could not start the propagation node: {e}"))?;
        // `/offer`: a peer lists what it has, proving its peering key; a Link
        // that did may send several messages at once.
        let (offer_node, offer_peers, local_identity) = (node.clone(), peers.clone(), identity.hash);
        destination
            .handle
            .register_request_handler(lxmf_core::constants::OFFER_REQUEST_PATH, AllowPolicy::AllowAll, Vec::new(), false, move |request| {
                let remote = request.remote_identity.as_ref().map(|identity| identity.hash);
                let peer_hash = remote.map_or([0; 16], |hash| Destination::hash_from_name_and_identity(PROPAGATION_ASPECT, Some(&hash)));
                let context = OfferRequestContext {
                    peer_hash,
                    identity_known: remote.is_some(),
                    is_throttled: false,
                    access_allowed: true,
                    local_identity_hash: Some(&local_identity),
                    remote_identity_hash: remote.as_ref(),
                };
                let reply = offer_node.lock().unwrap().handle_offer_request(&request.data, context);
                // Wanting some, all or none: the key was good.
                let accepted = matches!(rmpv::decode::read_value(&mut reply.as_slice()), Ok(Value::Boolean(_) | Value::Array(_)));
                if accepted {
                    offer_peers.lock().unwrap().insert(request.link_id);
                }
                RequestOutcome::Reply(reply)
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
            loop {
                let (data, link) = tokio::select! {
                    _ = &mut stopped => break,
                    Some(completion) = events.resource_completions.recv() => (completion.data, completion.link_id),
                    Some((data, link)) = events.link_packets.recv() => (data, link),
                    Some(link) = events.links_closed.recv() => {
                        event_peers.lock().unwrap().remove(&link);
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
                let several = event_peers.lock().unwrap().contains(&link);
                let ingest = event_ingest.clone();
                let taken = tokio::task::spawn_blocking(move || ingest.take(&data, several)).await;
                if let Ok(Err(e)) = taken {
                    tracing::debug!("propagation transfer not taken: {e}");
                }
            }
            let _ = destination.close().await;
        });
        // Announce (after interfaces have had a moment), then every so often;
        // cull old messages now and then.
        let (handle, config, cull_node) = (announcer, config.clone(), node.clone());
        let announcer = tokio::spawn(async move {
            let announce = async || {
                let options = DestinationAnnounceOptions { app_data: Some(announce_data(&config)), ..DestinationAnnounceOptions::default() };
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
        Ok(Self { hash, ingest, peers, stop: Some(stop), events: Some(events), announcer })
    }

    /// Stop the node, and wait (a little) until its destination is gone.
    pub async fn stop(mut self) {
        self.announcer.abort();
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
        let counters = &self.ingest.counters;
        PnStats {
            messages,
            bytes,
            received: counters.received.load(Ordering::Relaxed),
            delivered_here: counters.delivered_here.load(Ordering::Relaxed),
            served: counters.served.load(Ordering::Relaxed),
            rejected: counters.rejected.load(Ordering::Relaxed),
            peers: self.peers.lock().unwrap().len(),
        }
    }

    /// For this client's own sends to its node.
    pub fn local(&self) -> LocalNode {
        LocalNode { hash: self.hash, ingest: self.ingest.clone() }
    }
}

#[cfg(test)]
mod tests {
    use lxmf_core::message_api::{DeliveryMethod, LxMessage, MessageError};

    use super::*;

    fn ingest(dir: &std::path::Path, stamp_cost: u8) -> (Ingest, mpsc::UnboundedReceiver<Vec<u8>>, Identity) {
        let identity = Identity::new();
        let lxmf_hash = Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&identity.hash));
        let config = PropagationNodeConfig { max_storage: 1_000_000, min_stamp_cost: stamp_cost - PROPAGATION_COST_FLEX, ..PropagationNodeConfig::default() };
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
    fn transfer(to: Hash, recipient: &Identity, cost: u8) -> Vec<u8> {
        let mut message = LxMessage::new(to, [3; 16], "", "kept for you", DeliveryMethod::Propagated);
        message.sign(&Identity::new().get_signing_key().unwrap()).unwrap();
        message
            .pack_propagated_encrypted_with_stamp(|plain| recipient.encrypt(plain, None).map_err(|e| MessageError::PackFailed(e.to_string())), cost)
            .unwrap()
            .0
    }

    #[test]
    fn messages_are_kept_for_others_and_delivered_here_for_us() {
        let dir = std::env::temp_dir().join(format!("rettui-pn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (ingest, mut delivered, ours) = ingest(&dir, 13);
        let local = LocalNode { hash: [9; 16], ingest: Arc::new(ingest) };
        assert_eq!(local.stamp_cost(), 13);
        // For someone else: kept (once, however often it's sent), and listed
        // for them.
        let bob = Identity::new();
        let bob_hash = Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&bob.hash));
        let for_bob = transfer(bob_hash, &bob, 13);
        local.submit(&for_bob).unwrap();
        local.submit(&for_bob).unwrap();
        assert_eq!(local.ingest.node.lock().unwrap().message_count(), 1);
        let list = local.ingest.node.lock().unwrap().handle_get_request(&encode(&Value::Array(vec![Value::Nil, Value::Nil])), &bob_hash);
        let lxmf_core::propagation_node::GetRequestAction::Respond(reply) = list else { panic!() };
        assert!(matches!(rmpv::decode::read_value(&mut reply.as_slice()), Ok(Value::Array(ids)) if ids.len() == 1));
        // For us: delivered, not kept.
        let us = local.ingest.lxmf_hash;
        local.submit(&transfer(us, &ours, 13)).unwrap();
        let full = delivered.try_recv().unwrap();
        assert_eq!(&full[..16], &us);
        assert_eq!(LxMessage::unpack(&full).unwrap().content, "kept for you");
        // Too little work: turned away. Several at once: only from a peer.
        assert!(local.submit(&transfer(bob_hash, &bob, 0)).is_err());
        let stats = &local.ingest.counters;
        assert_eq!((stats.received.load(Ordering::Relaxed), stats.delivered_here.load(Ordering::Relaxed)), (1, 1));
        assert!(stats.rejected.load(Ordering::Relaxed) >= 1);
        let one = rmpv::decode::read_value(&mut transfer(bob_hash, &bob, 13).as_slice()).unwrap();
        let Value::Array(parts) = one else { panic!() };
        let blob = parts[1].as_array().unwrap()[0].clone();
        let two = encode(&Value::Array(vec![parts[0].clone(), Value::Array(vec![blob.clone(), blob])]));
        assert!(local.ingest.take(&two, false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
