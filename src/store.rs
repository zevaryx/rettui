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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageState {
    Received {
        verified: bool,
    },
    #[default]
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
    /// A voice message: its codec (`Opus`, `Codec2 1200`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

impl StoredAttachment {
    /// A voice message that plays elsewhere: an Ogg file (Opus), or a WAV
    /// file (Codec2, decoded).
    pub fn playable(&self) -> bool {
        self.voice.is_some() && self.path.extension().is_some_and(|e| e == "ogg" || e == "wav")
    }

    /// For a voice message recorded here, the Codec2 frames it went as,
    /// kept beside the WAV file played (to send the same again).
    pub fn frames_path(&self) -> Option<PathBuf> {
        self.voice.as_ref()?;
        Some(self.path.with_extension("codec2")).filter(|p| p.is_file())
    }

    /// The files it has: the one shown, and the frames of a recording.
    pub fn files(&self) -> Vec<PathBuf> {
        std::iter::once(self.path.clone()).chain(self.frames_path()).collect()
    }
}

/// A reaction to a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    /// What it is: an emoji, usually.
    pub emoji: String,
    /// Theirs, or yours.
    pub incoming: bool,
    /// Unix seconds.
    pub timestamp: f64,
    /// A received one's LXMF hash (hex), or `local-N` for yours.
    pub id: String,
    /// Yours: sending, sent or failed.
    pub state: MessageState,
}

/// A reaction to a message that isn't here (yet): kept in case it arrives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrayReaction {
    /// The LXMF hash (hex) of the message it reacts to.
    pub to: String,
    #[serde(flatten)]
    pub reaction: Reaction,
}

/// The most reactions kept waiting for their messages, a conversation.
const STRAY_REACTIONS: usize = 100;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// A sent message's LXMF hash (hex), once it's sent or written: what
    /// replies to it name. A received message's is its `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The message this one answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<ReplyTo>,
    /// Reactions to it, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<Reaction>,
    /// Where they were, if it brought a location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<crate::lxmf::Location>,
    /// What else it brought, in words: a command asked for, a location
    /// share that stopped, something rettui can't show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// How its text is written, if not plain (shown formatted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<crate::markdown::TextFormat>,
    /// Your location, shared live: the newest update, replaced by the next.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub live: bool,
    /// How a received message was heard (RSSI, SNR, link quality), when it
    /// came in one packet straight from the sender over a radio that says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<crate::net::remote::Signal>,
}

impl Message {
    /// What its text says, without Markdown's markup (for previews).
    pub fn text(&self) -> String {
        match self.format {
            Some(crate::markdown::TextFormat::Markdown) => crate::markdown::plain(&self.content),
            _ => self.content.clone(),
        }
    }

    /// The LXMF hash (hex) replies to this message name it by, if it has
    /// one (a message being sent, or that failed, has none yet).
    pub fn lxmf_hash(&self) -> Option<&str> {
        if self.incoming { (self.id.len() == 64).then_some(self.id.as_str()) } else { self.hash.as_deref() }
    }

    /// The start of what it says, for a reply's quote and the
    /// conversation list: its first line of text, else its title, else
    /// what it brought (a voice message, a file, a location, a note).
    pub fn opening(&self) -> String {
        let first = |text: &str| text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string);
        first(&self.text())
            .or_else(|| first(&self.title))
            .or_else(|| {
                self.attachments.first().map(|a| match &a.voice {
                    Some(_) => "🎤 Voice message".to_string(),
                    None => format!("📎 {}", a.name),
                })
            })
            .or_else(|| self.location.map(|_| "📍 Location".to_string()))
            .or_else(|| self.notes.first().cloned())
            .unwrap_or_default()
    }

    /// Nothing but a location or notes (telemetry, a command): not worth a
    /// notification, and a newer one replaces it.
    pub fn is_quiet(&self) -> bool {
        self.incoming
            && self.content.trim().is_empty()
            && self.title.trim().is_empty()
            && self.attachments.is_empty()
            && self.reply.is_none()
    }

    /// Whether `other` is the same kind of quiet message (a newer location
    /// update, the same command again), which replaces this one.
    pub fn same_quiet_kind(&self, other: &Message) -> bool {
        self.is_quiet() && other.is_quiet() && self.location.is_some() == other.location.is_some() && self.notes == other.notes
    }
}

/// The message a reply answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyTo {
    /// Its LXMF hash (hex).
    pub hash: String,
    /// The start of its text, as the reply carried it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

/// What a reply shows of the message it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quoted {
    /// Where that message is in the conversation, if it's there.
    pub index: Option<usize>,
    /// Whether they wrote it (`Some(false)`: you did), if that's known.
    pub incoming: Option<bool>,
    /// The start of it: from the message itself where it's here (so a
    /// quote can't put words in anyone's mouth), else what the reply quoted.
    pub text: String,
}

impl Conversation {
    /// Add a reaction to the message with LXMF hash `to`, or keep it until
    /// that message arrives. False if it's here already (the same one
    /// twice, or the same emoji from the same side).
    pub fn add_reaction(&mut self, to: &str, reaction: Reaction) -> bool {
        let mut known = self.messages.iter().flat_map(|m| &m.reactions).chain(self.stray_reactions.iter().map(|s| &s.reaction));
        if known.any(|r| r.id == reaction.id) {
            return false;
        }
        match self.messages.iter_mut().find(|m| m.lxmf_hash() == Some(to)) {
            Some(message) => {
                if message.reactions.iter().any(|r| r.incoming == reaction.incoming && r.emoji == reaction.emoji) {
                    return false;
                }
                message.reactions.push(reaction);
            }
            None => {
                self.stray_reactions.push(StrayReaction { to: to.to_string(), reaction });
                let over = self.stray_reactions.len().saturating_sub(STRAY_REACTIONS);
                self.stray_reactions.drain(..over);
            }
        }
        true
    }

    /// Put reactions that were waiting on the messages they're for, now
    /// that they're here (or have their hash).
    pub fn adopt_strays(&mut self) {
        for stray in std::mem::take(&mut self.stray_reactions) {
            if self.messages.iter().any(|m| m.lxmf_hash() == Some(stray.to.as_str())) {
                self.add_reaction(&stray.to.clone(), stray.reaction);
            } else {
                self.stray_reactions.push(stray);
            }
        }
    }

    /// The message with LXMF hash `hash`, and where it is.
    pub fn by_hash(&self, hash: &str) -> Option<(usize, &Message)> {
        self.messages.iter().enumerate().find(|(_, m)| m.lxmf_hash() == Some(hash))
    }

    /// What `reply` shows of the message it answers.
    pub fn quoted(&self, reply: &ReplyTo) -> Quoted {
        match self.by_hash(&reply.hash) {
            Some((index, message)) => Quoted { index: Some(index), incoming: Some(message.incoming), text: message.opening() },
            None => Quoted {
                index: None,
                incoming: None,
                text: reply
                    .quote
                    .as_deref()
                    .and_then(|q| q.lines().map(str::trim).find(|l| !l.is_empty()))
                    .unwrap_or("a message that isn't here")
                    .to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
    pub unread: usize,
    /// No notifications for new messages here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
    /// Kept at the top of the list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// How many older messages have been moved to the archive.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub archived: usize,
    /// Reactions to messages that aren't here (yet).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stray_reactions: Vec<StrayReaction>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// How far you trust someone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trust {
    /// Nothing decided.
    #[default]
    Unknown,
    /// Looked at and left as is: their messages are taken, but they aren't
    /// trusted.
    Untrusted,
    /// Spared the stamp cost, and given tickets.
    Trusted,
    /// Their messages are dropped, and their identity is blocked in
    /// Reticulum.
    Blocked,
}

impl Trust {
    pub fn key(self) -> &'static str {
        match self {
            Trust::Unknown => "unknown",
            Trust::Untrusted => "untrusted",
            Trust::Trusted => "trusted",
            Trust::Blocked => "blocked",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Trust::Unknown, Trust::Untrusted, Trust::Trusted, Trust::Blocked].into_iter().find(|t| t.key() == text)
    }
}

/// What you keep about someone you message: your own name for them, notes,
/// and how far you trust them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Contact {
    /// Your name for them, shown instead of the one they announce.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(skip_serializing_if = "is_unknown")]
    pub trust: Trust,
    /// How messages to them go, if not auto (never paper: that's for one
    /// message).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<crate::lxmf::DeliveryMode>,
    /// Their icon, from their newest message that had one, and when that
    /// was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<crate::lxmf::fields::Appearance>,
    #[serde(skip_serializing_if = "is_never")]
    pub icon_at: f64,
}

fn is_never(n: &f64) -> bool {
    *n == 0.0
}

fn is_unknown(trust: &Trust) -> bool {
    *trust == Trust::Unknown
}

impl Contact {
    /// Nothing kept: no need for an entry.
    pub fn is_empty(&self) -> bool {
        *self == Contact::default()
    }
}

/// Longest name of your own for a contact, and longest notes, in characters.
pub const MAX_ALIAS: usize = 128;
pub const MAX_NOTES: usize = 10_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// Keyed by destination hash (hex). Saved in `peers.json.gz` (see
    /// [`encode_peers`]); only read here, from stores of earlier versions.
    #[serde(skip_serializing)]
    pub peers: HashMap<String, Peer>,
    /// Keyed by the remote LXMF destination hash (hex).
    pub conversations: HashMap<String, Conversation>,
    /// What you keep about people, by LXMF address (hex).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub contacts: BTreeMap<String, Contact>,
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
    let month =
        chrono::DateTime::from_timestamp(timestamp as i64, 0).map_or_else(|| "0000-00".to_string(), |t| t.format("%Y-%m").to_string());
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

/// Take a conversation's messages out of the archive (it was deleted):
/// months that held some are written again without them, or deleted if
/// that leaves nothing. Their files in `owned` folders (rettui's downloads
/// and uploads) are deleted too. Returns what it did, for the log.
pub fn remove_from_archive(dir: &Path, conversation: &str, owned: &[PathBuf]) -> Result<Vec<String>> {
    let months: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(ARCHIVE_EXTENSION)))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut removed = 0;
    for path in months {
        let mut text = String::new();
        flate2::read::MultiGzDecoder::new(std::fs::File::open(&path)?).read_to_string(&mut text)?;
        let (mut kept, mut gone) = (Vec::new(), 0);
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Archived>(line) {
                Ok(archived) if archived.conversation == conversation => {
                    for file in archived.message.attachments.iter().flat_map(StoredAttachment::files) {
                        remove_owned(&file, owned);
                    }
                    gone += 1;
                }
                // Lines it can't read are kept as they are.
                _ => kept.push(line),
            }
        }
        if gone == 0 {
            continue;
        }
        removed += gone;
        if kept.is_empty() {
            std::fs::remove_file(&path)?;
        } else {
            write_atomic(&path, &gzip(format!("{}\n", kept.join("\n")).as_bytes())?)?;
        }
    }
    Ok(if removed > 0 { vec![format!("Deleted {removed} archived message(s) of the deleted conversation")] } else { Vec::new() })
}

/// A conversation's messages in the archive, oldest first (lines it can't
/// read are left out). Reads every month, so it's for when they're asked
/// for, away from the screen where that may take a while.
pub fn read_archive(dir: &Path, conversation: &str) -> Result<Vec<Message>> {
    let months: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(ARCHIVE_EXTENSION)))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut messages = Vec::new();
    for path in months {
        let mut text = String::new();
        flate2::read::MultiGzDecoder::new(std::fs::File::open(&path)?).read_to_string(&mut text)?;
        // Only lines that name it are read through.
        for line in text.lines().filter(|l| l.contains(conversation)) {
            if let Ok(archived) = serde_json::from_str::<Archived>(line)
                && archived.conversation == conversation
            {
                messages.push(archived.message);
            }
        }
    }
    messages.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
    Ok(messages)
}

/// Delete `path` if it's in one of the `owned` folders (files rettui saved:
/// downloads and uploads), not a file of yours sent from elsewhere.
pub fn remove_owned(path: &Path, owned: &[PathBuf]) -> bool {
    owned.iter().any(|dir| path.starts_with(dir)) && std::fs::remove_file(path).is_ok()
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
        notes.push(format!("Deleted the archived messages of {name} ({}) to keep messages within the storage limit", kilobytes(bytes)));
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
    /// Keys of keyed rooms joined, to rejoin them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub room_keys: BTreeMap<String, String>,
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
                    store.migration_notes.push(format!("Could not convert store.json to store.json.gz, still using it: {e:#}"));
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
        // Names heard before they were cleaned (see [`crate::names`]).
        for peer in store.peers.values_mut() {
            peer.name = peer.name.take().and_then(|name| crate::names::clean(&name));
        }
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
                    self.migration_notes.push(format!("Could not read peers.json.gz ({e:#}); peers are listed again as they announce"));
                }
            }
        } else if !self.peers.is_empty() {
            match encode_peers(&self.peers).and_then(|bytes| write_atomic(path, &bytes)) {
                Ok(()) => {
                    self.rewrite_store = holds_peers;
                    self.migration_notes.push(format!("Moved {} peers from store.json.gz to peers.json.gz", self.peers.len()));
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
            contacts: self.contacts.clone(),
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

    /// Who someone is: your name for them, else the one they announce,
    /// else the start of their address.
    pub fn display_name(&self, hash: &str) -> String {
        match self.contacts.get(hash).and_then(|c| c.alias.as_deref()).or_else(|| self.announced_name(hash)) {
            Some(name) => name.to_string(),
            // By characters: `hash` may come from a request, not only as hex.
            None => format!("<{}>", hash.chars().take(12).collect::<String>()),
        }
    }

    /// The name someone announces, if they've been heard.
    pub fn announced_name(&self, hash: &str) -> Option<&str> {
        self.peers.get(hash).and_then(|p| p.name.as_deref())
    }

    /// What you keep about someone (nothing, if there's no entry).
    pub fn contact(&self, hash: &str) -> Contact {
        self.contacts.get(hash).cloned().unwrap_or_default()
    }

    /// Change what you keep about someone; an empty entry is dropped.
    pub fn update_contact(&mut self, hash: &str, change: impl FnOnce(&mut Contact)) {
        let mut contact = self.contact(hash);
        change(&mut contact);
        if contact.is_empty() {
            self.contacts.remove(hash);
        } else {
            self.contacts.insert(hash.to_string(), contact);
        }
    }

    /// Conversation keys, most recent activity first.
    pub fn conversation_order(&self) -> Vec<String> {
        let mut keys: Vec<(&String, f64)> =
            self.conversations.iter().map(|(k, c)| (k, c.messages.last().map_or(0.0, |m| m.timestamp))).collect();
        keys.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        keys.into_iter().map(|(k, _)| k.clone()).collect()
    }

    pub fn find_message_mut(&mut self, id: &str) -> Option<&mut Message> {
        self.conversations.values_mut().flat_map(|c| c.messages.iter_mut()).find(|m| m.id == id)
    }

    /// A reaction of yours, by its `local-N` id (on a message, or waiting
    /// for one).
    pub fn find_reaction_mut(&mut self, id: &str) -> Option<&mut Reaction> {
        self.conversations.values_mut().find_map(|c| {
            let on_messages = c.messages.iter_mut().flat_map(|m| m.reactions.iter_mut());
            let waiting = c.stray_reactions.iter_mut().map(|s| &mut s.reaction);
            on_messages.chain(waiting).find(|r| r.id == id)
        })
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
        store.peers.insert("ab".repeat(16), Peer { kind: PeerKind::Lxmf, name: Some("Alice".into()), hops: 2, last_seen: 1_790_000_000 });
        let message = Message {
            id: "local-1".into(),
            content: "hello ".repeat(50),
            timestamp: 1_790_000_000.5,
            state: MessageState::Delivered,
            ..Message::default()
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
    fn names_heard_before_are_cleaned_when_loaded() {
        let dir = temp_dir("clean-names");
        let path = dir.join("store.json.gz");
        let mut peers = sample().peers;
        peers.get_mut(&"ab".repeat(16)).unwrap().name = Some("Ali\u{202E}ce\u{200B}".into());
        Store::default().save(&path).unwrap();
        write_atomic(&peers_path(&path), &encode_peers(&peers).unwrap()).unwrap();
        assert_eq!(Store::load(&path).unwrap().display_name(&"ab".repeat(16)), "Alice");
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
    fn a_conversations_archive_is_read_oldest_first() {
        let dir = temp_dir("archive-read");
        let message = |conversation: &str, id: &str, timestamp: f64| Archived {
            conversation: conversation.into(),
            message: Message { id: id.into(), timestamp, content: format!("message {id}"), ..Message::default() },
        };
        let (alice, bob) = ("ab".repeat(16), "cd".repeat(16));
        // Two months, out of order, with someone else's between.
        let october = archive_month(&dir, 1_791_000_000.0);
        std::fs::write(
            &october,
            encode_archive(&[message(&alice, "late", 1_791_000_000.0), message(&bob, "bob", 1_791_000_001.0)]).unwrap(),
        )
        .unwrap();
        let september = archive_month(&dir, 1_790_000_000.0);
        let mut bytes = encode_archive(&[message(&alice, "early", 1_790_000_000.0)]).unwrap();
        bytes.extend(gzip(b"not json, but naming abababababababababababababababab\n").unwrap());
        std::fs::write(&september, bytes).unwrap();
        let read = read_archive(&dir, &alice).unwrap();
        assert_eq!(read.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["early", "late"]);
        assert_eq!(read_archive(&dir, &bob).unwrap().len(), 1);
        assert!(read_archive(&dir.join("none"), &alice).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_deleted_conversation_leaves_the_archive() {
        let dir = temp_dir("forget");
        let (archive, downloads) = (dir.join("archive"), dir.join("downloads"));
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&downloads).unwrap();
        let (theirs, mine) = (downloads.join("photo.png"), dir.join("elsewhere.png"));
        std::fs::write(&theirs, b"x").unwrap();
        std::fs::write(&mine, b"x").unwrap();
        let file = |path: &Path| StoredAttachment { name: "f".into(), path: path.to_path_buf(), size: 1, image: true, voice: None };
        let message = |conversation: &str, id: &str, attachments| Archived {
            conversation: conversation.into(),
            message: Message { id: id.into(), attachments, ..Message::default() },
        };
        let both = archive.join("2026-01.jsonl.gz");
        let only = archive.join("2026-02.jsonl.gz");
        let gone = "aa".repeat(16);
        let stays = "bb".repeat(16);
        std::fs::write(&both, encode_archive(&[message(&gone, "1", vec![file(&theirs)]), message(&stays, "2", vec![])]).unwrap()).unwrap();
        std::fs::write(&only, encode_archive(&[message(&gone, "3", vec![file(&mine)])]).unwrap()).unwrap();
        let notes = remove_from_archive(&archive, &gone, std::slice::from_ref(&downloads)).unwrap();
        assert_eq!(notes, ["Deleted 2 archived message(s) of the deleted conversation"]);
        let left: Vec<Archived> = unzip(&both).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(left.iter().map(|a| a.message.id.as_str()).collect::<Vec<_>>(), ["2"]);
        assert!(!only.exists(), "a month with nothing left goes");
        assert!(!theirs.exists(), "a file rettui saved goes");
        assert!(mine.exists(), "a file of yours stays");
        assert!(remove_from_archive(&dir.join("none"), &gone, &[]).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn contacts_rename_people_and_go_when_empty() {
        let mut store = sample();
        let alice = "ab".repeat(16);
        store.update_contact(&alice, |c| c.alias = Some("Ally".into()));
        assert_eq!(store.display_name(&alice), "Ally");
        assert_eq!(store.announced_name(&alice), Some("Alice"));
        store.update_contact(&alice, |c| c.alias = None);
        assert_eq!(store.display_name(&alice), "Alice");
        assert!(store.contacts.is_empty(), "nothing kept, no entry");
        store.update_contact(&alice, |c| c.notes = "met at the swapfest".into());
        let saved: Store = serde_json::from_slice(&serde_json::to_vec(&store.snapshot()).unwrap()).unwrap();
        assert_eq!(saved.contact(&alice).notes, "met at the swapfest");
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
