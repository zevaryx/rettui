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

/// Public keys and announce data heard from LXMF and propagation node
/// announces, kept across runs so messages can be encrypted, stamped and
/// verified without first hearing the peer again.
#[derive(Default, Serialize, Deserialize)]
struct KnownEntry {
    key: String,
    app_data: Option<String>,
}

#[derive(Default)]
pub struct KnownIdentities {
    path: PathBuf,
    entries: HashMap<String, KnownEntry>,
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
        }))
    }

    pub(super) fn remember(&mut self, destination: Hash, key: &[u8; 64], app_data: Option<&[u8]>) {
        let entry = KnownEntry {
            key: hex::encode(key),
            app_data: app_data.map(hex::encode),
        };
        let slot = self.entries.entry(hex::encode(destination)).or_default();
        if slot.key == entry.key && slot.app_data == entry.app_data {
            return;
        }
        *slot = entry;
        if let Ok(text) = serde_json::to_string(&self.entries)
            && let Err(e) = crate::config::write_atomic(&self.path, text.as_bytes())
        {
            tracing::warn!("could not save known identities: {e}");
        }
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
    runtime
        .await_path(destination, PATH_TIMEOUT)
        .await
        .map_err(|e| format!("No path to destination: {e}"))
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

pub fn link_options(label: &str, identify: bool) -> LinkConnectOptions {
    LinkConnectOptions {
        path_timeout: PATH_TIMEOUT,
        client_label: label.to_string(),
        identify,
        ..LinkConnectOptions::default()
    }
}
