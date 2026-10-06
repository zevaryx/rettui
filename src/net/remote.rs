//! Reaching remote destinations: paths, public keys (live or remembered
//! from earlier runs) and Link options.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rns_runtime::prelude::*;
use serde::{Deserialize, Serialize};

use super::Hash;

/// How long to wait for a path when a destination has never been heard.
pub const PATH_TIMEOUT: Duration = Duration::from_secs(30);
/// When path requests go out, in seconds from the first; the last is
/// waited on for [`PATH_TRY`]. One request often goes unanswered: it can be
/// lost on the way (the second covers that), or reach a node that hasn't
/// heard of the destination yet. Python Reticulum folds further requests
/// for a destination into one it's still working on, for 45 s
/// (`PATH_REQUEST_GATE_TIMEOUT`, cleared within 5 s after), so the third
/// goes after that: a node that heard the first then looks again.
const PATH_REQUESTS: [u64; 3] = [0, 15, 52];
/// How long one path request is waited on: rsReticulum's transport stops
/// waiting at 15 s, whatever the caller allows.
const PATH_TRY: Duration = Duration::from_secs(15);
/// Path requests sent before giving up on finding a path.
pub const PATH_TRIES: u32 = PATH_REQUESTS.len() as u32;

/// Told how finding a path (or a fresh one) is going, in words.
pub type Progress<'a> = &'a (dyn Fn(String) + Send + Sync);

/// Public keys and announce data heard from LXMF and propagation node
/// announces, kept across runs so messages can be encrypted, stamped and
/// verified without first hearing the peer again.
#[derive(Default, Clone, Serialize, Deserialize)]
struct KnownEntry {
    key: String,
    app_data: Option<String>,
}

#[derive(Default)]
pub struct KnownIdentities {
    path: PathBuf,
    entries: HashMap<String, KnownEntry>,
    /// Changed since the last save. Saving is batched (see
    /// [`KnownIdentities::save_in_background`]): rewriting the whole file for
    /// every announce slowed the network actor to a crawl during bursts, and
    /// announces were dropped.
    dirty: bool,
}

pub type Known = Arc<std::sync::Mutex<KnownIdentities>>;

impl KnownIdentities {
    pub fn load(path: &Path) -> Known {
        let entries = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Arc::new(std::sync::Mutex::new(Self {
            path: path.to_path_buf(),
            entries,
            dirty: false,
        }))
    }

    /// Write the file if anything changed, off the async threads.
    pub async fn save_in_background(known: &Known) {
        let snapshot = {
            let mut known = known.lock().unwrap();
            if !known.dirty {
                return;
            }
            known.dirty = false;
            (known.path.clone(), known.entries.clone())
        };
        let (path, entries) = snapshot;
        let saved = tokio::task::spawn_blocking(move || {
            let text = serde_json::to_string(&entries)?;
            crate::config::write_atomic(&path, text.as_bytes())
        })
        .await;
        match saved {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("could not save known identities: {e}"),
            Err(e) => tracing::warn!("could not save known identities: {e}"),
        }
    }

    /// The announce data kept for a destination, if any.
    pub fn app_data_of(&self, destination: Hash) -> Option<Vec<u8>> {
        self.entries.get(&hex::encode(destination))?.app_data.as_deref().and_then(|d| hex::decode(d).ok())
    }

    pub(super) fn remember(&mut self, destination: Hash, key: &[u8; 64], app_data: Option<&[u8]>) {
        let slot = self.entries.entry(hex::encode(destination)).or_default();
        let key = hex::encode(key);
        // Announces without app data (such as the automatic path responses
        // of Python shared-instance clients) keep what an earlier one said.
        let app_data = match app_data {
            Some(data) => Some(hex::encode(data)),
            None if slot.key == key => slot.app_data.clone(),
            None => None,
        };
        if slot.key == key && slot.app_data == app_data {
            return;
        }
        *slot = KnownEntry { key, app_data };
        self.dirty = true;
    }

    fn get(&self, destination: Hash) -> Option<Remote> {
        let entry = self.entries.get(&hex::encode(destination))?;
        let identity = Identity::from_public_key(&hex::decode(&entry.key).ok()?).ok()?;
        Some(Remote {
            identity,
            app_data: entry.app_data.as_deref().and_then(|d| hex::decode(d).ok()),
            ratchet: None,
        })
    }
}

/// What we know about a remote destination.
pub struct Remote {
    pub identity: Identity,
    pub app_data: Option<Vec<u8>>,
    pub ratchet: Option<[u8; 32]>,
}

impl From<RecalledDestination> for Remote {
    fn from(recalled: RecalledDestination) -> Self {
        Self {
            identity: recalled.identity,
            app_data: recalled.app_data,
            ratchet: recalled.ratchet,
        }
    }
}

/// Make sure this process has a path before opening a Link. A cached
/// identity alone is not enough: without a path the Link request is broadcast
/// without a transport id, and multi-hop transport nodes drop it.
///
/// `has_path` is not used here because shared-instance clients answer it from
/// the daemon's path table, while outbound routing uses the local one.
/// `await_path` checks the local table and requests a path when missing.
pub async fn ensure_path(runtime: &ReticulumHandle, destination: Hash) -> Result<(), String> {
    find_path(runtime, destination, &|_| {}).await
}

/// Find a path to `destination`: one known already, or one asked for, with
/// requests at [`PATH_REQUESTS`]; `progress` hears of each one after the
/// first. Between them, a path that comes anyway (with an announce) is
/// taken.
pub async fn find_path(runtime: &ReticulumHandle, destination: Hash, progress: Progress<'_>) -> Result<(), String> {
    let start = std::time::Instant::now();
    for (i, &at) in PATH_REQUESTS.iter().enumerate() {
        if i > 0 {
            while start.elapsed() < Duration::from_secs(at) {
                if runtime.has_path(destination).await.unwrap_or(false) {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            progress(format!("path request {} of {PATH_TRIES}", i + 1));
        }
        // Sends a request, unless a path is known.
        match runtime.await_path(destination, PATH_TRY).await {
            Ok(()) => return Ok(()),
            Err(rns_transport::await_path::AwaitPathError::TransportDown) => return Err("No path: Reticulum isn't running".into()),
            Err(rns_transport::await_path::AwaitPathError::Timeout) => {}
        }
    }
    Err(format!(
        "No path found: {PATH_TRIES} requests over a minute went unanswered. Nodes in reach may not have heard of it yet; its next announce brings one"
    ))
}

/// Open a Link to `destination` (a path to it found first). One that isn't
/// answered may have gone over a stale path (the destination moved, or a
/// node on the way went), or one answered from an out-of-date cache: that
/// path is dropped, a fresh one found, and the Link tried once more.
pub async fn connect(
    runtime: &ReticulumHandle,
    destination: Hash,
    identity: Identity,
    options: LinkConnectOptions,
    progress: Progress<'_>,
) -> Result<LinkSession, String> {
    find_path(runtime, destination, progress).await?;
    match runtime.connect_link(destination, identity.clone(), options.clone()).await {
        Err(LinkConnectError::Session(LinkSessionError::Timeout(_))) => {
            progress("no answer, finding a fresh path".into());
            drop_path(runtime, destination).await;
            find_path(runtime, destination, progress).await?;
            runtime.connect_link(destination, identity, options).await.map_err(|e| e.to_string())
        }
        result => result.map_err(|e| e.to_string()),
    }
}

/// Forget the path to `destination`, so the next use asks for a fresh one.
/// Whether there was one.
pub async fn drop_path(runtime: &ReticulumHandle, destination: Hash) -> bool {
    let had = runtime.has_path(destination).await.unwrap_or(false);
    runtime.query_control(rns_transport::messages::TransportQuery::DropPath { dest: destination }).await;
    had
}

/// Keys for a destination: the runtime's live announce cache, then keys
/// remembered from earlier runs, then a path request (whose response carries
/// the announce). Does not require a path when the key is already known.
pub async fn lookup(runtime: &ReticulumHandle, known: &Known, destination: Hash) -> Result<Remote, String> {
    if let Ok(Some(recalled)) = runtime.recall(destination).await {
        return Ok(recalled.into());
    }
    if let Some(remote) = known.lock().unwrap().get(destination) {
        return Ok(remote);
    }
    ensure_path(runtime, destination).await?;
    runtime
        .recall(destination)
        .await
        .map_err(|e| e.to_string())?
        .map(Remote::from)
        .ok_or_else(|| "Destination identity is unknown (no announce heard)".to_string())
}

/// How a peer answered a ping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ping {
    /// How long a Link to them took to set up: a round trip, and their
    /// work to accept it.
    pub rtt: std::time::Duration,
    /// How far away they are, if the path table says.
    pub hops: Option<u8>,
}

/// Ping an LXMF address: find a path, then time setting up a Link to it
/// (closed straight away). Any LXMF client answers; nothing is sent over it.
pub async fn ping(runtime: &ReticulumHandle, known: &Known, to: Hash) -> Result<Ping, String> {
    ensure_path(runtime, to).await?;
    // Their key, from earlier runs if the runtime hasn't heard it.
    lookup(runtime, known, to).await?;
    let hops = runtime.hops_to(to).await.ok().filter(|&h| h < rns_transport::constants::PATHFINDER_M);
    let started = std::time::Instant::now();
    let LinkSession { handle, .. } = runtime
        .connect_link(to, Identity::new(), link_options("rettui.ping", false))
        .await
        .map_err(|e| format!("No answer: {e}"))?;
    let rtt = started.elapsed();
    handle.close().await;
    Ok(Ping { rtt, hops })
}

pub fn link_options(label: &str, identify: bool) -> LinkConnectOptions {
    LinkConnectOptions {
        path_timeout: PATH_TIMEOUT,
        client_label: label.to_string(),
        identify,
        ..LinkConnectOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_in_memory_and_keeps_app_data_from_named_announces() {
        let mut known = KnownIdentities::default();
        let (dest, key, other_key) = ([1u8; 16], [2u8; 64], [3u8; 64]);
        known.remember(dest, &key, Some(b"named"));
        assert!(known.dirty);
        known.dirty = false;
        // The same announce again changes nothing.
        known.remember(dest, &key, Some(b"named"));
        assert!(!known.dirty);
        // A nameless announce (an automatic path response) keeps the data.
        known.remember(dest, &key, None);
        assert!(!known.dirty);
        assert_eq!(known.entries[&hex::encode(dest)].app_data, Some(hex::encode(b"named")));
        // New data replaces it; a different key starts over.
        known.remember(dest, &key, Some(b"renamed"));
        assert!(known.dirty);
        assert_eq!(known.entries[&hex::encode(dest)].app_data, Some(hex::encode(b"renamed")));
        known.remember(dest, &other_key, None);
        assert_eq!(known.entries[&hex::encode(dest)].app_data, None);
    }
}
