//! Persistent peers and conversations.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Result;
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

#[derive(Debug, Default, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    /// `<node hash>:/page/path.mu`
    pub url: String,
}

/// An RRC hub as saved in `store.json`.
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
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut store: Self = serde_json::from_str(&std::fs::read_to_string(path)?)?;
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

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, serde_json::to_string(self)?.as_bytes())
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
