//! Receiving files sent with `rncp`: a listener at this identity's rncp
//! address (the one the Network tab and `rncp -a` know), taking files from
//! the identities allowed and saving them in a folder.
//!
//! The listener is rsReticulum's. It never writes over a file already there
//! (a second file of the same name gets `.1`, `.2` and so on after it). It
//! saves a file from a sender who never said who they were, and a name ending
//! in `..` can put the file beside the folder, so rettui deletes both and
//! counts them as refused.

use std::collections::HashMap;
use std::path::PathBuf;

use rns_identity::identity::Identity;
use rns_runtime::prelude::*;
use rns_runtime::rncp::{DEFAULT_RNCP_APP_NAME, RncpEvent, RncpListenerConfig, RncpListenerHandle, spawn_rncp_listener};
use tokio::sync::mpsc;

use super::{Hash, NetEvent};
use crate::config::{Paths, Settings, expand_home};

/// Links remembered to say who a file came from, at most (one a file).
const MAX_LINKS: usize = 256;

/// Whether a file the listener has saved must go: it came from a Link that
/// never identified (the listener takes those, whatever the list says), or
/// it landed outside `dir` (a name ending in `..` makes it).
fn must_discard(saved: &std::path::Path, dir: &std::path::Path, from: Option<Hash>) -> bool {
    if from.is_none_or(|from| from == [0; 16]) {
        return true;
    }
    match (saved.parent().and_then(|p| p.canonicalize().ok()), dir.canonicalize().ok()) {
        (Some(parent), Some(dir)) => parent != dir,
        _ => true,
    }
}

/// What the listener runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RncpListenConfig {
    pub dir: PathBuf,
    /// Identity hashes that may send; nobody else.
    pub allowed: Vec<Hash>,
}

impl RncpListenConfig {
    /// `None` when listening is off.
    pub fn from_settings(settings: &Settings, paths: &Paths) -> Option<Self> {
        settings.rncp_listen.then(|| Self {
            dir: settings.rncp_dir.as_deref().map_or_else(|| paths.rncp_received.clone(), expand_home),
            allowed: settings.rncp_allowed_list(),
        })
    }
}

/// What the listener reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RncpListenEvent {
    Started {
        hash: Hash,
    },
    Stopped,
    Failed(String),
    /// A file arrived and was saved.
    Received {
        name: String,
        bytes: usize,
        path: PathBuf,
        /// The sender's identity, if they said.
        from: Option<Hash>,
    },
    /// Someone not on the list tried to send.
    Denied {
        from: Hash,
    },
    /// A file arrived but could not be saved.
    WriteFailed {
        name: String,
        reason: String,
    },
}

/// A running listener.
pub struct Listening {
    pub hash: Hash,
    handle: RncpListenerHandle,
    forward: tokio::task::JoinHandle<()>,
}

impl Listening {
    pub async fn stop(self) {
        self.forward.abort();
        self.handle.shutdown().await;
    }
}

/// Start listening at the identity's rncp address.
pub async fn start(
    runtime: &ReticulumHandle,
    identity: &Identity,
    config: &RncpListenConfig,
    ev: mpsc::UnboundedSender<NetEvent>,
) -> Result<Listening, String> {
    let (events_tx, mut events_rx) = mpsc::channel::<RncpEvent>(64);
    let handle = spawn_rncp_listener(
        runtime.transport_tx.clone(),
        RncpListenerConfig {
            identity: identity.clone(),
            app_name: DEFAULT_RNCP_APP_NAME.to_string(),
            save_dir: config.dir.clone(),
            allow_all: false,
            allowed: config.allowed.clone(),
            overwrite: false,
            allow_fetch: false,
            fetch_jail: None,
            fetch_auto_compress: true,
        },
        events_tx,
    )
    .await
    .map_err(|e| format!("Could not listen for files: {e}"))?;
    let hash = handle.destination_hash();
    let dir = config.dir.clone();
    let forward = tokio::spawn(async move {
        // Who each link said it was, to name the sender of a file.
        let mut who: HashMap<[u8; 16], Hash> = HashMap::new();
        while let Some(event) = events_rx.recv().await {
            let say = match event {
                RncpEvent::SenderIdentified { link_id, identity_hash } => {
                    if who.len() >= MAX_LINKS {
                        who.clear();
                    }
                    who.insert(link_id, identity_hash);
                    None
                }
                RncpEvent::SenderDenied { identity_hash, .. } => Some(RncpListenEvent::Denied { from: identity_hash }),
                RncpEvent::Completed { link_id, file_name, saved_path, bytes } => {
                    let from = who.remove(&link_id).filter(|from| *from != [0; 16]);
                    if must_discard(&saved_path, &dir, from) {
                        // Never overwrites, so what is at this path is what it just wrote.
                        let _ = std::fs::remove_file(&saved_path);
                        Some(RncpListenEvent::Denied { from: from.unwrap_or([0; 16]) })
                    } else {
                        Some(RncpListenEvent::Received { name: file_name, bytes, path: saved_path, from })
                    }
                }
                RncpEvent::WriteFailed { file_name, reason, .. } => Some(RncpListenEvent::WriteFailed { name: file_name, reason }),
                _ => None,
            };
            if let Some(event) = say {
                let _ = ev.send(NetEvent::RncpListen(event));
            }
        }
    });
    Ok(Listening { hash, handle, forward })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_from_strangers_or_outside_the_folder_are_discarded() {
        let dir = std::env::temp_dir().join(format!("rettui-rncp-discard-{}", std::process::id()));
        let inside = dir.join("got");
        std::fs::create_dir_all(&inside).unwrap();
        let file = inside.join("a.txt");
        assert!(!must_discard(&file, &inside, Some([1; 16])));
        // Never said who they were.
        assert!(must_discard(&file, &inside, None));
        assert!(must_discard(&file, &inside, Some([0; 16])));
        // A name like `../..` puts the file beside the folder.
        assert!(must_discard(&dir.join("...1"), &inside, Some([1; 16])));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn it_listens_only_when_asked_and_takes_only_who_is_listed() {
        let dir = std::env::temp_dir().join(format!("rettui-rncp-listen-{}", std::process::id()));
        let paths = Paths::new(Some(dir.clone())).unwrap();
        let mut settings = Settings::default();
        assert_eq!(RncpListenConfig::from_settings(&settings, &paths), None);
        settings.rncp_listen = true;
        let config = RncpListenConfig::from_settings(&settings, &paths).unwrap();
        assert_eq!((config.dir.clone(), config.allowed.clone()), (paths.rncp_received.clone(), vec![]));
        settings.set_field("rncp_allowed", &format!("{}, {}", "ab".repeat(16), "cd".repeat(16))).unwrap();
        settings.set_field("rncp_dir", &dir.display().to_string()).unwrap();
        let config = RncpListenConfig::from_settings(&settings, &paths).unwrap();
        assert_eq!((config.dir, config.allowed), (dir.clone(), vec![[0xab; 16], [0xcd; 16]]));
        // A list that isn't hashes is refused, and repeats count once.
        assert!(settings.set_field("rncp_allowed", "not-a-hash").is_err());
        settings.set_field("rncp_allowed", &format!("{0} {0}", "ab".repeat(16))).unwrap();
        assert_eq!(settings.field_value("rncp_allowed"), "ab".repeat(16));
        let _ = std::fs::remove_dir_all(dir);
    }
}
