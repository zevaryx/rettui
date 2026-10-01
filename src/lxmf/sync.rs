//! Propagation node sync: list, download and delete waiting messages.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lxmf_core::constants::MESSAGE_GET_PATH;
use rmpv::Value;
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use super::{bytes_of, encode};
use super::inbound::deliver_inbound;
use super::policy::SharedPolicy;
use crate::net::{Hash, Known, NetEvent, ensure_path, link_options};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Per-transfer limit sent to propagation nodes when downloading (KB).
const SYNC_LIMIT_KB: f64 = 1000.0;

/// Runs a propagation node sync in the background; at most one at a time.
pub struct Syncer {
    runtime: ReticulumHandle,
    known: Known,
    policy: SharedPolicy,
    identity: Identity,
    lxmf_hash: Hash,
    /// The LXMF address's ratchets, to decrypt with.
    ring: PathBuf,
    ev: mpsc::UnboundedSender<NetEvent>,
    running: Arc<AtomicBool>,
}

impl Syncer {
    pub fn new(
        runtime: ReticulumHandle,
        known: Known,
        policy: SharedPolicy,
        identity: Identity,
        lxmf_hash: Hash,
        ring: PathBuf,
        ev: mpsc::UnboundedSender<NetEvent>,
    ) -> Self {
        Self {
            runtime,
            known,
            policy,
            identity,
            lxmf_hash,
            ring,
            ev,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self, node: Option<Hash>) {
        let Some(node) = node else {
            let _ = self.ev.send(NetEvent::Synced(Err(
                "No propagation node selected (pick one in the Network tab)".to_string(),
            )));
            return;
        };
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.ev.send(NetEvent::SyncStarted);
        let (runtime, known, policy, identity, ev, running, lxmf_hash, ring) = (
            self.runtime.clone(),
            self.known.clone(),
            self.policy.clone(),
            self.identity.clone(),
            self.ev.clone(),
            self.running.clone(),
            self.lxmf_hash,
            self.ring.clone(),
        );
        tokio::spawn(async move {
            let result = match sync(&runtime, &identity, lxmf_hash, &ring, node).await {
                Ok(messages) => {
                    let count = messages.len();
                    // Parse concurrently: verifying an unknown sender can wait
                    // on a path lookup.
                    futures_util::future::join_all(
                        messages.iter().map(|data| deliver_inbound(&runtime, &known, &policy, data, &ev)),
                    )
                    .await;
                    Ok(count)
                }
                Err(e) => Err(e),
            };
            running.store(false, Ordering::SeqCst);
            let _ = ev.send(NetEvent::Synced(result));
        });
    }
}

fn peer_error(code: u64) -> String {
    let reason = match code {
        0xF0 => "node could not identify us",
        0xF1 => "access denied",
        0xF3 => "invalid key",
        0xF4 => "invalid data",
        0xF5 => "invalid stamp",
        0xF6 => "throttled, try again later",
        0xFD => "not found",
        0xFE => "timed out",
        _ => "unknown error",
    };
    format!("Propagation node error {code:#x}: {reason}")
}

async fn get(handle: &LinkSessionHandle, request: Value) -> Result<Value, String> {
    let response = handle
        .request(MESSAGE_GET_PATH, &encode(&request), Some(REQUEST_TIMEOUT))
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    let value = rmpv::decode::read_value(&mut response.data.as_slice())
        .unwrap_or(Value::Binary(response.data));
    match value {
        Value::Integer(code) => Err(peer_error(code.as_u64().unwrap_or(0))),
        other => Ok(other),
    }
}

/// Download (and remove from the node) every message waiting for us.
/// Returns complete LXMF messages ready for [`parse_inbound`].
pub async fn sync(
    runtime: &ReticulumHandle,
    identity: &Identity,
    lxmf_hash: Hash,
    ring: &Path,
    node: Hash,
) -> Result<Vec<Vec<u8>>, String> {
    ensure_path(runtime, node).await?;
    // The node finds our messages from the identity we present on the Link.
    let LinkSession { handle, .. } = runtime
        .connect_link(node, identity.clone(), link_options("rettui.sync", true))
        .await
        .map_err(|e| format!("Link to propagation node failed: {e}"))?;
    let result = sync_on(&handle, identity, lxmf_hash, ring).await;
    handle.close().await;
    result
}

async fn sync_on(
    handle: &LinkSessionHandle,
    identity: &Identity,
    lxmf_hash: Hash,
    ring: &Path,
) -> Result<Vec<Vec<u8>>, String> {
    let Value::Array(available) = get(handle, Value::Array(vec![Value::Nil, Value::Nil])).await?
    else {
        return Err("Unexpected message list from propagation node".to_string());
    };
    if available.is_empty() {
        return Ok(Vec::new());
    }
    let wants = Value::Array(available);
    let Value::Array(blobs) = get(
        handle,
        Value::Array(vec![wants, Value::Array(Vec::new()), Value::F64(SYNC_LIMIT_KB)]),
    )
    .await?
    else {
        return Err("Unexpected message data from propagation node".to_string());
    };

    let mut messages = Vec::new();
    let mut received = Vec::new();
    for blob in blobs.iter().filter_map(bytes_of) {
        received.push(Value::Binary(rns_crypto::sha::full_hash(&blob).to_vec()));
        if blob.len() <= 16 || blob[..16] != lxmf_hash {
            continue;
        }
        match super::ratchets::decrypt(identity, ring, &blob[16..]) {
            Ok(plaintext) => {
                let mut full = lxmf_hash.to_vec();
                full.extend_from_slice(&plaintext);
                messages.push(full);
            }
            Err(e) => tracing::warn!("could not decrypt propagated message: {e}"),
        }
    }
    // Tell the node which messages we now hold so it can drop them.
    if !received.is_empty()
        && let Err(e) = get(handle, Value::Array(vec![Value::Nil, Value::Array(received)])).await
    {
        tracing::warn!("propagation node purge failed: {e}");
    }
    Ok(messages)
}
