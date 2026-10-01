//! Persistent peers and conversations.
//!
//! Saved as gzip-compressed JSON, about a fifth of the size of the plain
//! JSON (`store.json`) that earlier versions wrote. The first launch after
//! upgrading converts the old file (see [`Store::load`]).
//!
//! - `store.json.gz`: conversations, saved pages, RRC hubs and the rest.
//! - `peers.json.gz`: the peers heard. They change with every announce,
//!   several a second on a busy network, so they have a file of their own:
//!   saving them doesn't write every conversation again.
//! - `archive/<year>-<month>.jsonl.gz`: messages older than the newest ones
//!   each conversation keeps, by the month they were sent (see
//!   [`encode_archive`]). Added to, never rewritten; whole months are
//!   deleted, oldest first, to keep within the storage limit (see
//!   [`keep_within`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    /// A paper message written: its `lxm://` link (shown as a QR code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paper: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
    pub unread: usize,
    /// No notifications for new messages here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
    /// How many older messages have been moved to the archive.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub archived: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// Keyed by destination hash (hex). Saved in `peers.json.gz` (see
    /// [`encode_peers`]); only read here, from stores of earlier versions.
    #[serde(skip_serializing)]
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
    /// Emoji picked lately, newest first (for the pickers).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent_emoji: Vec<String>,
    /// What loading did to files of earlier versions, for the app's log.
    #[serde(skip)]
    pub migration_notes: Vec<String>,
    /// The store file still holds the peers (from before they had a file
    /// of their own): write it again without them.
    #[serde(skip)]
    pub rewrite_store: bool,
    /// The peers couldn't be written to their own file while loading: try
    /// again with the next save.
    #[serde(skip)]
    pub rewrite_peers: bool,
}

/// The first bytes of a gzip file.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// The peers' file, beside the store.
pub fn peers_path(store: &Path) -> PathBuf {
    store.with_file_name("peers.json.gz")
}

/// A message in the archive, with the conversation it's from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Archived {
    pub conversation: String,
    #[serde(flatten)]
    pub message: Message,
}

/// The archive's file for the month a message was sent (UTC), in `dir`.
pub fn archive_month(dir: &Path, timestamp: f64) -> PathBuf {
    let month = chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .map_or_else(|| "0000-00".to_string(), |t| t.format("%Y-%m").to_string());
    dir.join(format!("{month}{ARCHIVE_EXTENSION}"))
}

const ARCHIVE_EXTENSION: &str = ".jsonl.gz";

/// JSON, gzipped at the store's level.
fn gzip_json(value: &impl Serialize) -> Result<Vec<u8>> {
    // Serialising first and compressing in one go is several times
    // faster than streaming serde's many small writes through gzip.
    let json = serde_json::to_vec(value)?;
    gzip(&json)
}

fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut gzip = GzEncoder::new(Vec::with_capacity(bytes.len() / 4), STORE_COMPRESSION);
    gzip.write_all(bytes)?;
    Ok(gzip.finish()?)
}

/// Read gzip-compressed JSON, or the plain JSON of earlier versions (told
/// apart by the gzip header).
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path)?;
    if bytes.starts_with(&GZIP_MAGIC) {
        let mut json = Vec::new();
        GzDecoder::new(bytes.as_slice()).read_to_end(&mut json)?;
        Ok(serde_json::from_slice(&json)?)
    } else {
        Ok(serde_json::from_slice(&bytes)?)
    }
}

/// `peers.json.gz`'s contents.
pub fn encode_peers(peers: &HashMap<String, Peer>) -> Result<Vec<u8>> {
    gzip_json(peers)
}

/// Messages to add to the end of an archive file: one per line (JSON), as
/// a gzip member of its own. Members added one after another make one
/// gzip file (`zcat archive/*.jsonl.gz` reads them all), so archiving never
/// rewrites what is already there.
pub fn encode_archive(messages: &[Archived]) -> Result<Vec<u8>> {
    let mut lines = Vec::new();
    for message in messages {
        serde_json::to_writer(&mut lines, message)?;
        lines.push(b'\n');
    }
    gzip(&lines)
}

/// Keep the store and its archive within `limit` bytes together, by
/// deleting the archive's oldest months. Returns what it did, for the log,
/// and whether the store alone is over the limit (nothing left to delete).
pub fn keep_within(store: &Path, archive: &Path, limit: u64) -> Result<(Vec<String>, bool)> {
    let size = |path: &Path| std::fs::metadata(path).map_or(0, |m| m.len());
    let mut months: Vec<(PathBuf, u64)> = match std::fs::read_dir(archive) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(ARCHIVE_EXTENSION)))
            .map(|path| {
                let bytes = size(&path);
                (path, bytes)
            })
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    // Named by year and month, so by name is oldest first.
    months.sort();
    let mut total = size(store) + months.iter().map(|(_, bytes)| bytes).sum::<u64>();
    let mut notes = Vec::new();
    for (path, bytes) in months {
        if total <= limit {
            break;
        }
        std::fs::remove_file(&path)?;
        total -= bytes;
        let name = path.file_name().map(|n| n.to_string_lossy().replace(ARCHIVE_EXTENSION, "")).unwrap_or_default();
        notes.push(format!(
            "Deleted the archived messages of {name} ({}) to keep messages within the storage limit",
            kilobytes(bytes)
        ));
    }
    Ok((notes, total > limit))
}

/// gzip level for the store, which is written again whenever it changes.
/// Measured on a 17.6 MB store (60,000 messages), level 3 takes a third of
/// the time of the default (6) for a file about 7% larger; level 1 is
/// quicker still but 35% larger.
const STORE_COMPRESSION: Compression = Compression::new(3);

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
    /// Notifications for the hub's rooms and whispers; mentions (and
    /// whispers) when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<NotifyLevel>,
    /// Rooms and whisper conversations whose notifications differ from the
    /// hub's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub room_notify: BTreeMap<String, NotifyLevel>,
}

/// What in an RRC room (or a hub's rooms) gets a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyLevel {
    /// Every message.
    All,
    /// Messages that mention you, and whispers.
    Mentions,
    Off,
}

impl NotifyLevel {
    pub fn key(self) -> &'static str {
        match self {
            NotifyLevel::All => "all",
            NotifyLevel::Mentions => "mentions",
            NotifyLevel::Off => "off",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NotifyLevel::All => "all messages",
            NotifyLevel::Mentions => "mentions and whispers",
            NotifyLevel::Off => "off",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [NotifyLevel::All, NotifyLevel::Mentions, NotifyLevel::Off].into_iter().find(|level| level.key() == text)
    }
}

impl Store {
    /// Load the store from `path` (`store.json.gz`). On the first launch
    /// after upgrading, a plain `store.json` beside it is converted: the new
    /// file is written, read back and compared, and only then is the old one
    /// removed. If any step fails, the old file stays and is used.
    pub fn load(path: &Path) -> Result<Self> {
        let legacy = legacy_path(path);
        // Whether store.json.gz itself still holds the peers.
        let mut holds_peers = false;
        let mut store = if !path.exists() && legacy.exists() {
            match Self::migrate(&legacy, path) {
                Ok(store) => store,
                Err(e) => {
                    let mut store: Self = read_json(&legacy)?;
                    store
                        .migration_notes
                        .push(format!("Could not convert store.json to store.json.gz, still using it: {e:#}"));
                    tracing::warn!("store conversion failed: {e:#}");
                    store
                }
            }
        } else if path.exists() {
            let store: Self = read_json(path)?;
            holds_peers = !store.peers.is_empty();
            store
        } else {
            Self::default()
        };
        store.load_peers(&peers_path(path), holds_peers);
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

    /// The peers, from their own file. A store of an earlier version holds
    /// them itself: they are written to their file now, before the store is
    /// written again without them (`holds_peers`), so they are never only
    /// in memory.
    fn load_peers(&mut self, path: &Path, holds_peers: bool) {
        if path.exists() {
            match read_json(path) {
                Ok(peers) => {
                    // Any still in the store are older than the file's.
                    self.rewrite_store = holds_peers;
                    self.peers = peers;
                }
                Err(e) => {
                    tracing::warn!("could not read {}: {e:#}", path.display());
                    self.migration_notes.push(format!(
                        "Could not read peers.json.gz ({e:#}); peers are listed again as they announce"
                    ));
                }
            }
        } else if !self.peers.is_empty() {
            match encode_peers(&self.peers).and_then(|bytes| write_atomic(path, &bytes)) {
                Ok(()) => {
                    self.rewrite_store = holds_peers;
                    self.migration_notes
                        .push(format!("Moved {} peers from store.json.gz to peers.json.gz", self.peers.len()));
                }
                Err(e) => {
                    self.rewrite_peers = true;
                    self.migration_notes.push(format!("Could not write peers.json.gz: {e:#}"));
                }
            }
        }
    }

    /// The file's contents: compressed JSON (without the peers).
    pub fn encode(&self) -> Result<Vec<u8>> {
        gzip_json(self)
    }

    /// A copy to save in the background: everything the store file holds,
    /// which leaves out the peers (see [`encode_peers`]).
    pub fn snapshot(&self) -> Self {
        Self {
            peers: HashMap::new(),
            conversations: self.conversations.clone(),
            next_local_id: self.next_local_id,
            identified_nodes: self.identified_nodes.clone(),
            saved: self.saved.clone(),
            rrc_hubs: self.rrc_hubs.clone(),
            recent_emoji: self.recent_emoji.clone(),
            migration_notes: Vec::new(),
            rewrite_store: false,
            rewrite_peers: false,
        }
    }

    /// Convert an old plain-JSON store to the compressed format, its peers
    /// to their own file.
    fn migrate(legacy: &Path, path: &Path) -> Result<Self> {
        let mut store: Self = read_json(legacy)?;
        store.save(path)?;
        let written: Result<Self> = read_json(path);
        let same = written.as_ref().is_ok_and(|w| serde_json::to_value(w).ok() == serde_json::to_value(&store).ok());
        if !same {
            let _ = std::fs::remove_file(path);
            bail!("the converted store did not read back the same");
        }
        // The old file goes only once its peers are in theirs.
        let peers = peers_path(path);
        if !store.peers.is_empty()
            && !peers.exists()
            && let Err(e) = encode_peers(&store.peers).and_then(|bytes| write_atomic(&peers, &bytes))
        {
            let _ = std::fs::remove_file(path);
            bail!("could not write peers.json.gz: {e:#}");
        }
        let before = std::fs::metadata(legacy).map_or(0, |m| m.len());
        let after = std::fs::metadata(path).map_or(0, |m| m.len());
        std::fs::remove_file(legacy)?;
        store.migration_notes.push(format!(
            "Converted store.json to the compressed store.json.gz ({} → {})",
            kilobytes(before),
            kilobytes(after)
        ));
        Ok(store)
    }

    /// Write the store file (not the peers).
    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, &self.encode()?)
    }

    pub fn display_name(&self, hash: &str) -> String {
        match self.peers.get(hash).and_then(|p| p.name.as_deref()) {
            Some(name) => name.to_string(),
            // By characters: `hash` may come from a request, not only as hex.
            None => format!("<{}>", hash.chars().take(12).collect::<String>()),
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

    #[test]
    fn hubs_and_conversations_saved_before_notifications_load_with_the_defaults() {
        let old = r#"{"hash":"ab","aspect":"rrc.hub","name":"Hub","rooms":["general"],"nick":null,"auto_connect":true}"#;
        let mut hub: HubConfig = serde_json::from_str(old).unwrap();
        assert_eq!((hub.notify, hub.room_notify.len()), (None, 0));
        assert_eq!(serde_json::to_string(&hub).unwrap(), old, "nothing new is written until something is set");
        hub.notify = Some(NotifyLevel::All);
        hub.room_notify.insert("general".into(), NotifyLevel::Off);
        let saved = serde_json::to_string(&hub).unwrap();
        assert!(saved.contains(r#""notify":"all""#) && saved.contains(r#""room_notify":{"general":"off"}"#), "{saved}");
        let conversation: Conversation = serde_json::from_str(r#"{"messages":[],"unread":2}"#).unwrap();
        assert!(!conversation.muted);
        assert_eq!(NotifyLevel::parse("mentions"), Some(NotifyLevel::Mentions));
        assert_eq!(NotifyLevel::parse("loud"), None);
    }

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
            paper: None,
        };
        store.conversations.insert("ab".repeat(16), Conversation { messages: vec![message], unread: 1, ..Default::default() });
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
        std::fs::write(&legacy, legacy_json(&old)).unwrap();
        let store = Store::load(&path).unwrap();
        assert!(!legacy.exists(), "the old file is removed after converting");
        assert!(std::fs::read(&path).unwrap().starts_with(&GZIP_MAGIC));
        assert!(peers_path(&path).exists(), "the peers go to their own file");
        assert_eq!(store.migration_notes.len(), 1);
        assert!(store.migration_notes[0].starts_with("Converted store.json"));
        assert!(!store.rewrite_store && !store.rewrite_peers);
        assert_eq!(store.peers.len(), 1);
        let message = &store.conversations.values().next().unwrap().messages[0];
        assert_eq!(message.state, MessageState::Failed("interrupted".into()));
        // The next launch reads the new files, with no note.
        let again = Store::load(&path).unwrap();
        assert!(again.migration_notes.is_empty());
        assert_eq!(again.peers.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_conversion_keeps_the_old_file() {
        let dir = temp_dir("fail");
        let (path, legacy) = (dir.join("store.json.gz"), dir.join("store.json"));
        std::fs::write(&legacy, legacy_json(&sample())).unwrap();
        // Block the temporary file the new store is written through.
        std::fs::create_dir(path.with_extension("tmp")).unwrap();
        let store = Store::load(&path).unwrap();
        assert!(legacy.exists(), "the old file stays");
        assert!(!path.exists());
        assert_eq!(store.peers.len(), 1, "and its contents are used");
        assert!(store.migration_notes[0].starts_with("Could not convert"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_names_are_cut_by_character() {
        let store = sample();
        assert_eq!(store.display_name(&"ab".repeat(16)), "Alice");
        assert_eq!(store.display_name(&"cd".repeat(16)), "<cdcdcdcdcdcd>");
        assert_eq!(store.display_name("abc"), "<abc>");
        // Not a character boundary at byte 12: this used to panic.
        assert_eq!(store.display_name("aéééééé"), "<aéééééé>");
        assert_eq!(store.display_name(&"é".repeat(20)), format!("<{}>", "é".repeat(12)));
    }

    #[test]
    fn the_new_file_wins_when_both_exist() {
        let dir = temp_dir("both");
        let (path, legacy) = (dir.join("store.json.gz"), dir.join("store.json"));
        sample().save(&path).unwrap();
        std::fs::write(&legacy, serde_json::to_string(&Store::default()).unwrap()).unwrap();
        let store = Store::load(&path).unwrap();
        assert_eq!(store.conversations.len(), 1);
        assert!(legacy.exists(), "left alone");
        assert!(store.migration_notes.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
    /// A store's JSON as earlier versions wrote it, peers included.
    fn legacy_json(store: &Store) -> Vec<u8> {
        let mut value = serde_json::to_value(store).unwrap();
        value["peers"] = serde_json::to_value(&store.peers).unwrap();
        serde_json::to_vec(&value).unwrap()
    }

    /// A store as the previous version wrote it: gzipped, peers included.
    fn write_with_peers(store: &Store, path: &Path) {
        std::fs::write(path, gzip(&legacy_json(store)).unwrap()).unwrap();
    }

    fn unzip(path: &Path) -> String {
        let mut text = String::new();
        flate2::read::MultiGzDecoder::new(std::fs::File::open(path).unwrap()).read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn peers_are_saved_apart_from_the_store() {
        let dir = temp_dir("peers");
        let path = dir.join("store.json.gz");
        let store = sample();
        store.save(&path).unwrap();
        assert!(!unzip(&path).contains("Alice"), "the store file leaves the peers out");
        assert!(store.snapshot().peers.is_empty());
        write_atomic(&peers_path(&path), &encode_peers(&store.peers).unwrap()).unwrap();
        let loaded = Store::load(&path).unwrap();
        assert_eq!(loaded.peers.len(), 1);
        assert_eq!(loaded.display_name(&"ab".repeat(16)), "Alice");
        assert!(!loaded.rewrite_store && !loaded.rewrite_peers && loaded.migration_notes.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn peers_move_out_of_a_store_from_the_previous_version() {
        let dir = temp_dir("peers-move");
        let path = dir.join("store.json.gz");
        write_with_peers(&sample(), &path);
        let store = Store::load(&path).unwrap();
        assert_eq!(store.peers.len(), 1);
        assert!(peers_path(&path).exists(), "written while loading, before the store drops them");
        assert!(store.rewrite_store, "the store is written again without them");
        assert!(store.migration_notes[0].starts_with("Moved 1 peer"), "{:?}", store.migration_notes);
        // Stopped before the store was written again: the peers' file wins,
        // and the store is still due to be rewritten.
        let mut newer = sample();
        newer.peers.values_mut().for_each(|p| p.name = Some("Alice B".into()));
        write_atomic(&peers_path(&path), &encode_peers(&newer.peers).unwrap()).unwrap();
        let again = Store::load(&path).unwrap();
        assert_eq!(again.display_name(&"ab".repeat(16)), "Alice B");
        assert!(again.rewrite_store && again.migration_notes.is_empty());
        // Once rewritten, nothing is left to do.
        again.save(&path).unwrap();
        let last = Store::load(&path).unwrap();
        assert!(!last.rewrite_store && last.peers.len() == 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn archives_are_added_to_not_rewritten() {
        let dir = temp_dir("archive");
        let message = |id: &str| Archived {
            conversation: "ab".repeat(16),
            message: Message { id: id.into(), ..sample().conversations.into_values().next().unwrap().messages[0].clone() },
        };
        let path = archive_month(&dir, 1_790_000_000.5);
        assert!(path.ends_with("2026-09.jsonl.gz"), "{}", path.display());
        for batch in [vec![message("1"), message("2")], vec![message("3")]] {
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path).unwrap();
            file.write_all(&encode_archive(&batch).unwrap()).unwrap();
        }
        let archived: Vec<Archived> = unzip(&path).lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        let ids: Vec<&str> = archived.iter().map(|a| a.message.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3"]);
        assert!(archived.iter().all(|a| a.conversation == "ab".repeat(16)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_oldest_months_go_first_to_keep_within_the_limit() {
        let dir = temp_dir("limit");
        let (store, archive) = (dir.join("store.json.gz"), dir.join("archive"));
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(&store, vec![0; 1000]).unwrap();
        for month in ["2025-11", "2025-12", "2026-01"] {
            std::fs::write(archive.join(format!("{month}.jsonl.gz")), vec![0; 500]).unwrap();
        }
        std::fs::write(archive.join("notes.txt"), vec![0; 5000]).unwrap();
        // 2,500 bytes of messages: within 3,000, nothing goes.
        assert_eq!(keep_within(&store, &archive, 3000).unwrap(), (Vec::new(), false));
        // Within 2,000: the oldest month goes (other files are not counted).
        let (notes, over) = keep_within(&store, &archive, 2000).unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("2025-11") && !over, "{notes:?}");
        assert!(!archive.join("2025-11.jsonl.gz").exists() && archive.join("2025-12.jsonl.gz").exists());
        // Smaller than the store itself: every month goes, and it says so.
        let (notes, over) = keep_within(&store, &archive, 900).unwrap();
        assert_eq!(notes.len(), 2);
        assert!(over);
        assert!(archive.join("notes.txt").exists());
        // No archive yet is fine.
        assert_eq!(keep_within(&store, &dir.join("none"), 5000).unwrap(), (Vec::new(), false));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
