//! Persistent peers and conversations.
//!
//! Saved as gzip-compressed JSON (`store.json.gz`), about a fifth of the
//! size of the plain JSON (`store.json`) that earlier versions wrote. The
//! first launch after upgrading converts the old file (see [`Store::load`]).

use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};

use crate::config::write_atomic;
use crate::net::PeerKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub kind: PeerKind,
    pub name: Option<String>,
    pub hops: u8,
    /// Unix seconds.
    pub last_seen: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageState {
    Received { verified: bool },
    Sending,
    Delivered,
    /// Accepted by a propagation node for the recipient to collect.
    Propagated,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAttachment {
    pub name: String,
    /// Where the file lives locally (received files are saved on arrival).
    pub path: PathBuf,
    pub size: u64,
    pub image: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// LXMF message id (hex) for inbound messages, `local-N` for outbound.
    pub id: String,
    pub incoming: bool,
    pub title: String,
    pub content: String,
    /// Unix seconds.
    pub timestamp: f64,
    pub state: MessageState,
    #[serde(default)]
    pub attachments: Vec<StoredAttachment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
    pub unread: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// Keyed by destination hash (hex).
    pub peers: HashMap<String, Peer>,
    /// Keyed by the remote LXMF destination hash (hex).
    pub conversations: HashMap<String, Conversation>,
    pub next_local_id: u64,
    /// NomadNet nodes (hex) we identify ourselves to when browsing.
    pub identified_nodes: BTreeSet<String>,
    /// Saved NomadNet pages, in the order they were saved.
    pub saved: Vec<Bookmark>,
    /// RRC hubs and the rooms joined on them.
    pub rrc_hubs: Vec<HubConfig>,
    /// What loading did to an old-format file, for the app's log.
    #[serde(skip)]
    pub migration_note: Option<String>,
}

/// The first bytes of a gzip file.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Where earlier versions kept the store, as plain JSON.
fn legacy_path(path: &Path) -> PathBuf {
    path.with_file_name("store.json")
}

fn kilobytes(bytes: u64) -> String {
    format!("{:.1} KB", bytes as f64 / 1000.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    /// `<node hash>:/page/path.mu`
    pub url: String,
}

/// An RRC hub as saved in the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubConfig {
    pub hash: String,
    pub aspect: String,
    pub name: String,
    pub rooms: Vec<String>,
    /// Nick for this hub; the display name is used when unset.
    pub nick: Option<String>,
    pub auto_connect: bool,
}

impl Store {
    /// Load the store from `path` (`store.json.gz`). On the first launch
    /// after upgrading, a plain `store.json` beside it is converted: the new
    /// file is written, read back and compared, and only then is the old one
    /// removed. If any step fails, the old file stays and is used.
    pub fn load(path: &Path) -> Result<Self> {
        let legacy = legacy_path(path);
        let mut store = if !path.exists() && legacy.exists() {
            match Self::migrate(&legacy, path) {
                Ok(store) => store,
                Err(e) => {
                    let mut store = Self::read(&legacy)?;
                    store.migration_note = Some(format!("Could not convert store.json to store.json.gz, still using it: {e:#}"));
                    tracing::warn!("store conversion failed: {e:#}");
                    store
                }
            }
        } else if path.exists() {
            Self::read(path)?
        } else {
            return Ok(Self::default());
        };
        // Anything still "sending" was interrupted by the last shutdown.
        for conversation in store.conversations.values_mut() {
            for message in &mut conversation.messages {
                if message.state == MessageState::Sending {
                    message.state = MessageState::Failed("interrupted".into());
                }
            }
        }
        Ok(store)
    }

    /// Read a store file in either format: gzip-compressed JSON, or the
    /// plain JSON of earlier versions (told apart by the gzip header).
    fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        if bytes.starts_with(&GZIP_MAGIC) {
            let mut json = Vec::new();
            GzDecoder::new(bytes.as_slice()).read_to_end(&mut json)?;
            Ok(serde_json::from_slice(&json)?)
        } else {
            Ok(serde_json::from_slice(&bytes)?)
        }
    }

    /// The file's contents: compressed JSON.
    pub fn encode(&self) -> Result<Vec<u8>> {
        // Serialising first and compressing in one go is several times
        // faster than streaming serde's many small writes through gzip.
        let json = serde_json::to_vec(self)?;
        let mut gzip = GzEncoder::new(Vec::with_capacity(json.len() / 4), Compression::default());
        gzip.write_all(&json)?;
        Ok(gzip.finish()?)
    }

    /// Convert an old plain-JSON store to the compressed format.
    fn migrate(legacy: &Path, path: &Path) -> Result<Self> {
        let store = Self::read(legacy)?;
        store.save(path)?;
        let written = Self::read(path);
        let same = written.as_ref().is_ok_and(|w| serde_json::to_value(w).ok() == serde_json::to_value(&store).ok());
        if !same {
            let _ = std::fs::remove_file(path);
            bail!("the converted store did not read back the same");
        }
        let before = std::fs::metadata(legacy).map_or(0, |m| m.len());
        let after = std::fs::metadata(path).map_or(0, |m| m.len());
        std::fs::remove_file(legacy)?;
        let mut store = store;
        store.migration_note = Some(format!(
            "Converted store.json to the compressed store.json.gz ({} → {})",
            kilobytes(before),
            kilobytes(after)
        ));
        Ok(store)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, &self.encode()?)
    }

    pub fn display_name(&self, hash: &str) -> String {
        match self.peers.get(hash).and_then(|p| p.name.as_deref()) {
            Some(name) => name.to_string(),
            None => format!("<{}>", &hash[..hash.len().min(12)]),
        }
    }

    /// Conversation keys, most recent activity first.
    pub fn conversation_order(&self) -> Vec<String> {
        let mut keys: Vec<(&String, f64)> = self
            .conversations
            .iter()
            .map(|(k, c)| (k, c.messages.last().map_or(0.0, |m| m.timestamp)))
            .collect();
        keys.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        keys.into_iter().map(|(k, _)| k.clone()).collect()
    }

    pub fn find_message_mut(&mut self, id: &str) -> Option<&mut Message> {
        self.conversations
            .values_mut()
            .flat_map(|c| c.messages.iter_mut())
            .find(|m| m.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Store {
        let mut store = Store::default();
        store.peers.insert(
            "ab".repeat(16),
            Peer {
                kind: PeerKind::Lxmf,
                name: Some("Alice".into()),
                hops: 2,
                last_seen: 1_790_000_000,
            },
        );
        let message = Message {
            id: "local-1".into(),
            incoming: false,
            title: String::new(),
            content: "hello ".repeat(50),
            timestamp: 1_790_000_000.5,
            state: MessageState::Delivered,
            attachments: Vec::new(),
        };
        store.conversations.insert("ab".repeat(16), Conversation { messages: vec![message], unread: 1 });
        store.next_local_id = 2;
        store
    }

    fn same(a: &Store, b: &Store) -> bool {
        serde_json::to_value(a).unwrap() == serde_json::to_value(b).unwrap()
    }

    #[test]
    fn saves_compressed_and_loads_back() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("store.json.gz");
        let store = sample();
        store.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(&GZIP_MAGIC));
        assert!(bytes.len() < serde_json::to_vec(&store).unwrap().len() / 2, "compressed well");
        assert!(same(&Store::load(&path).unwrap(), &store));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn converts_the_old_plain_json_once() {
        let dir = temp_dir("migrate");
        let (path, legacy) = (dir.join("store.json.gz"), dir.join("store.json"));
        // Written the way earlier versions did, with a message caught mid-send.
        let mut old = sample();
        old.conversations.values_mut().next().unwrap().messages[0].state = MessageState::Sending;
        std::fs::write(&legacy, serde_json::to_string(&old).unwrap()).unwrap();
        let store = Store::load(&path).unwrap();
        assert!(!legacy.exists(), "the old file is removed after converting");
        assert!(std::fs::read(&path).unwrap().starts_with(&GZIP_MAGIC));
        assert!(store.migration_note.as_deref().unwrap().starts_with("Converted store.json"));
        assert_eq!(store.peers.len(), 1);
        let message = &store.conversations.values().next().unwrap().messages[0];
        assert_eq!(message.state, MessageState::Failed("interrupted".into()));
        // The next launch reads the new file, with no note.
        let again = Store::load(&path).unwrap();
        assert!(again.migration_note.is_none());
        assert_eq!(again.peers.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_conversion_keeps_the_old_file() {
        let dir = temp_dir("fail");
        let (path, legacy) = (dir.join("store.json.gz"), dir.join("store.json"));
        std::fs::write(&legacy, serde_json::to_string(&sample()).unwrap()).unwrap();
        // Block the temporary file the new store is written through.
        std::fs::create_dir(path.with_extension("tmp")).unwrap();
        let store = Store::load(&path).unwrap();
        assert!(legacy.exists(), "the old file stays");
        assert!(!path.exists());
        assert_eq!(store.peers.len(), 1, "and its contents are used");
        assert!(store.migration_note.as_deref().unwrap().starts_with("Could not convert"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_new_file_wins_when_both_exist() {
        let dir = temp_dir("both");
        let (path, legacy) = (dir.join("store.json.gz"), dir.join("store.json"));
        sample().save(&path).unwrap();
        std::fs::write(&legacy, serde_json::to_string(&Store::default()).unwrap()).unwrap();
        let store = Store::load(&path).unwrap();
        assert_eq!(store.peers.len(), 1);
        assert!(legacy.exists(), "left alone");
        assert!(store.migration_note.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
