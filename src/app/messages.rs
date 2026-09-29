//! Messages tab: LXMF conversations, composing, and attachments.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::files::{expand_home, unique_path};
use super::notify::{self, Notification, Target};
use super::{App, PromptKind, Tab, now};
use crate::lxmf::DeliveryMode;
use crate::net::{NetCommand, parse_hash};
use crate::store::{Message, MessageState, StoredAttachment};
use crate::term::images::{DecodeFor, Picture};
use crate::term::input::TextInput;

/// Many LXMF clients refuse direct transfers above ~1 MB by default.
const LARGE_MESSAGE_BYTES: u64 = 1_000_000;

impl App {
    /// An attachment image already loaded by [`App::picture`].
    pub fn picture_ref(&self, path: &Path) -> Option<&Picture> {
        self.pictures.get(path).and_then(Option::as_ref)
    }

    /// Decoded image for an attachment. The first call starts loading it in
    /// the background and returns `None`; it shows once decoded.
    pub fn picture(&mut self, path: &Path) -> Option<&Picture> {
        if !self.pictures.contains_key(path) && self.decoding.insert(path.to_path_buf()) {
            self.decode_later(DecodeFor::File(path.to_path_buf()), None);
        }
        self.pictures.get(path).and_then(Option::as_ref)
    }

    pub(super) fn on_message(&mut self, message: crate::lxmf::InboundMessage) {
        let key = hex::encode(message.source);
        let id = message
            .id
            .map(hex::encode)
            .unwrap_or_else(|| format!("in-{}", message.timestamp));
        if self
            .store
            .conversations
            .get(&key)
            .is_some_and(|c| c.messages.iter().any(|m| m.id == id))
        {
            return; // duplicate (e.g. direct and propagated copies)
        }

        let mut attachments = Vec::new();
        if !message.attachments.is_empty() {
            let dir = self.paths.downloads.join(&key[..key.len().min(12)]);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                self.log(format!("Could not create {}: {e}", dir.display()));
            }
            for attachment in &message.attachments {
                let path = unique_path(&dir, &attachment.name);
                match std::fs::write(&path, &attachment.data) {
                    Ok(()) => attachments.push(StoredAttachment {
                        name: attachment.name.clone(),
                        path,
                        size: attachment.data.len() as u64,
                        image: attachment.image,
                    }),
                    Err(e) => self.log(format!("Could not save {}: {e}", path.display())),
                }
            }
        }

        let is_active =
            self.tab == Tab::Messages && self.active_conversation.as_deref() == Some(key.as_str());
        // What the notification says: the text, else what came with it.
        let preview = if !message.content.trim().is_empty() {
            notify::body(&message.content)
        } else if !message.title.trim().is_empty() {
            notify::body(&message.title)
        } else {
            match attachments.as_slice() {
                [] => "(empty message)".to_string(),
                [one] => format!("📎 {}", one.name),
                many => format!("📎 {} files", many.len()),
            }
        };
        let conversation = self.store.conversations.entry(key.clone()).or_default();
        let muted = conversation.muted;
        conversation.messages.push(Message {
            id,
            incoming: true,
            title: message.title,
            content: message.content,
            timestamp: message.timestamp,
            state: MessageState::Received {
                verified: message.verified,
            },
            attachments,
        });
        // Messages downloaded later may be older than ones already shown.
        conversation
            .messages
            .sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
        if !is_active {
            conversation.unread += 1;
        }
        self.store_dirty = true;
        let name = self.store.display_name(&key);
        self.log(format!("Message from {name}"));
        self.keep_conversation_selection();
        if self.settings.notify_messages && !muted {
            let target = Target::Conversation { key };
            self.push_notification(is_active, Notification { title: name, body: preview, target });
        }
    }

    /// Turn a conversation's notifications off or back on (`N`, or the web
    /// UI's bell). False if there is no such conversation.
    pub fn set_conversation_muted(&mut self, key: &str, muted: bool) -> bool {
        let Some(conversation) = self.store.conversations.get_mut(key) else { return false };
        conversation.muted = muted;
        self.store_dirty = true;
        true
    }

    fn toggle_active_muted(&mut self) {
        let Some(key) = self.active_conversation.clone() else { return };
        let muted = !self.store.conversations.get(&key).is_some_and(|c| c.muted);
        if self.set_conversation_muted(&key, muted) {
            let name = self.store.display_name(&key);
            self.notify(if muted {
                format!("No notifications from {name}")
            } else {
                format!("Notifications from {name} are on")
            });
        }
    }

    pub(super) fn open_conversation(&mut self, key: String) {
        self.store.conversations.entry(key.clone()).or_default();
        self.store_dirty = true;
        self.tab = Tab::Messages;
        self.active_conversation = Some(key);
        self.keep_conversation_selection();
        self.sync_active_conversation();
        self.composing = true;
    }

    /// Keep the list selection on the active conversation after reordering.
    fn keep_conversation_selection(&mut self) {
        let order = self.store.conversation_order();
        if let Some(active) = &self.active_conversation {
            self.conversations
                .select(order.iter().position(|k| k == active));
        } else if let Some(first) = order.first() {
            // First conversation ever: show it. Unread is cleared only once
            // the Messages tab is actually viewed.
            self.conversations.select(Some(0));
            self.active_conversation = Some(first.clone());
            if self.tab == Tab::Messages {
                self.sync_active_conversation();
            }
        }
    }

    pub(super) fn select_conversation(&mut self, index: usize) {
        self.conversations.select(Some(index));
        self.sync_active_conversation();
    }

    pub(super) fn sync_active_conversation(&mut self) {
        let order = self.store.conversation_order();
        let previous = self.active_conversation.clone();
        self.active_conversation = self.conversations.selected().and_then(|i| order.get(i).cloned());
        if let Some(key) = &self.active_conversation
            && let Some(conversation) = self.store.conversations.get_mut(key)
            && conversation.unread > 0
        {
            conversation.unread = 0;
            self.store_dirty = true;
        }
        if previous != self.active_conversation {
            self.message_scroll = 0;
            // What's written (and attached) stays with the conversation it
            // was written in, rather than going to the one opened.
            let draft = (std::mem::take(&mut self.compose), std::mem::take(&mut self.attachments));
            if let Some(previous) = previous
                && (!draft.0.text().is_empty() || !draft.1.is_empty())
            {
                self.drafts.insert(previous, draft);
            }
            if let Some((text, files)) = self.active_conversation.as_ref().and_then(|key| self.drafts.remove(key)) {
                self.compose = text;
                self.attachments = files;
            }
        }
    }

    pub(super) fn send_compose(&mut self) {
        let Some(key) = self.active_conversation.clone() else {
            return;
        };
        if self.compose.text().trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        let content = self.compose.take();
        let files = std::mem::take(&mut self.attachments);
        if let Err((e, content, files)) = self.send_message(key, content, files, self.delivery_mode) {
            // Keep what was written so it can be sent once the problem is fixed.
            self.warn(e);
            self.compose = TextInput::with_text(&content);
            self.attachments = files;
            return;
        }
        self.message_scroll = 0;
    }

    /// Queue an LXMF message to `key` (a hex address). On failure the text
    /// and files are handed back with the reason.
    pub fn send_message(
        &mut self,
        key: String,
        content: String,
        files: Vec<PathBuf>,
        mode: DeliveryMode,
    ) -> Result<(), (String, String, Vec<PathBuf>)> {
        let Some(to) = parse_hash(&key) else {
            return Err(("An LXMF address is 32 hex characters".into(), content, files));
        };
        if content.trim().is_empty() && files.is_empty() {
            return Err(("Nothing to send".into(), content, files));
        }
        if mode == DeliveryMode::Propagated && self.propagation_node().is_none() {
            return Err(("Select a propagation node first (Network tab, p)".into(), content, files));
        }
        let key = hex::encode(to);
        let attachments: Vec<StoredAttachment> = files
            .iter()
            .map(|path| StoredAttachment {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                image: crate::lxmf::is_image_path(path),
                path: path.clone(),
            })
            .collect();
        let total: u64 = attachments.iter().map(|a| a.size).sum();
        if total > LARGE_MESSAGE_BYTES {
            self.warn(format!(
                "Sending {total} bytes of attachments; many clients reject direct transfers over 1 MB"
            ));
        }
        let id = self.store.next_local_id;
        self.store.next_local_id += 1;
        self.store
            .conversations
            .entry(key)
            .or_default()
            .messages
            .push(Message {
                id: format!("local-{id}"),
                incoming: false,
                title: String::new(),
                content: content.clone(),
                timestamp: now(),
                state: MessageState::Sending,
                attachments,
            });
        self.store_dirty = true;
        self.keep_conversation_selection();
        self.send(NetCommand::SendMessage {
            id,
            to,
            content,
            attachments: files,
            mode,
        });
        Ok(())
    }

    pub fn mark_conversation_read(&mut self, key: &str) {
        if let Some(conversation) = self.store.conversations.get_mut(key)
            && conversation.unread > 0
        {
            conversation.unread = 0;
            self.store_dirty = true;
        }
    }

    pub(super) fn add_attachment(&mut self, text: &str) {
        let path = expand_home(text);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => {
                let path = path.canonicalize().unwrap_or(path);
                self.attachments.push(path);
            }
            Ok(_) => self.warn(format!("Not a file: {}", path.display())),
            Err(e) => self.warn(format!("Cannot attach {}: {e}", path.display())),
        }
        self.composing = true;
    }

    /// Open the newest attachment in the active conversation.
    fn open_latest_attachment(&mut self) {
        let latest = self
            .active_conversation
            .as_ref()
            .and_then(|key| self.store.conversations.get(key))
            .and_then(|c| c.messages.iter().rev().find_map(|m| m.attachments.last()))
            .map(|a| a.path.clone());
        match latest {
            Some(path) => self.open_file(&path),
            None => self.warn("No attachments in this conversation"),
        }
    }

    pub(super) fn open_attach_prompt(&mut self) {
        if self.active_conversation.is_some() {
            self.open_prompt(PromptKind::Attach, "Attach file (path)", "~/");
        }
    }

    pub(super) fn messages_key(&mut self, key: KeyEvent) {
        let count = self.store.conversations.len();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                let i = self.conversations.selected().map_or(0, |i| (i + 1).min(count - 1));
                self.select_conversation(i);
            }
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                let i = self.conversations.selected().map_or(0, |i| i.saturating_sub(1));
                self.select_conversation(i);
            }
            KeyCode::Enter | KeyCode::Char('i') if self.active_conversation.is_some() => {
                self.composing = true;
            }
            KeyCode::Char('a') => self.open_attach_prompt(),
            KeyCode::Char('o') => self.open_latest_attachment(),
            KeyCode::Char('d') => self.delivery_mode = self.delivery_mode.next(),
            KeyCode::Char('N') => self.toggle_active_muted(),
            KeyCode::PageUp => self.message_scroll += 5,
            KeyCode::PageDown => self.message_scroll = self.message_scroll.saturating_sub(5),
            KeyCode::Char('n') => self.open_prompt(PromptKind::NewConversation, "LXMF address", ""),
            KeyCode::Char('y') => {
                let address = self.active_conversation.clone().unwrap_or_default();
                self.copy(&address, "LXMF address");
            }
            _ => {}
        }
    }

    pub(super) fn click_messages(&mut self, at: Position) {
        let list = self.regions.conversations;
        if list.contains(at) {
            // Each conversation occupies two rows.
            let index = self.conversations.offset() + (at.y - list.y) as usize / 2;
            if index < self.store.conversations.len() {
                self.composing = false;
                self.select_conversation(index);
            }
        } else if self.regions.compose.contains(at) && self.active_conversation.is_some() {
            self.composing = true;
        } else if self.regions.history.contains(at) {
            let row = (at.y - self.regions.history.y) as usize;
            if let Some(Some(path)) = self.regions.history_rows.get(row).cloned() {
                self.open_file(&path);
            }
        }
    }
}
