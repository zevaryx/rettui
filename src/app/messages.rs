//! Messages tab: LXMF conversations, composing, and attachments.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::files::{path_from_input, unique_path};
use super::notify::{self, Notification, Target};
use super::{App, HistoryHit, MessageAction, PaperView, PromptKind, Tab, now};
use crate::lxmf::{self, DeliveryMode};
use crate::markdown::TextFormat;
use crate::net::{NetCommand, parse_hash};
use crate::store::{Archived, Message, MessageState, Reaction, ReplyTo, StoredAttachment, Trust};
use crate::term::images::{DecodeFor, Picture};
use crate::term::input::TextInput;

/// Many LXMF clients refuse direct transfers above ~1 MB by default.
const LARGE_MESSAGE_BYTES: u64 = 1_000_000;
/// How a paper message that couldn't be written fails: it isn't sent again
/// on its own, since that would send it over the network instead.
const PAPER_FAILED: &str = "Not written: ";
/// A failed message sent again when its recipient announced isn't sent
/// again so for this long.
const RESEND_GAP: Duration = Duration::from_secs(10 * 60);

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
            // A duplicate (e.g. direct and propagated copies).
            if message.paper {
                self.notify(format!("You have this message from {} already", self.store.display_name(&key)));
            }
            return;
        }
        // Read in by the user, who's looking at it.
        let paper = message.paper;
        if !paper {
            if self.store.contact(&key).trust == Trust::Blocked {
                return self.log(format!("Dropped a message from {}, who is blocked", self.store.display_name(&key)));
            }
            if self.settings.ignore_unknown_senders && !self.is_known(&key) {
                return self.log(format!(
                    "Ignored a message from {}, who isn't a contact (Ignore unknown senders is on)",
                    self.store.display_name(&key)
                ));
            }
        }
        let extras = &message.extras;
        if let Some(appearance) = &extras.appearance {
            self.keep_appearance(&key, appearance, message.timestamp);
        }
        let location = extras.telemetry.as_ref().and_then(|t| t.location);
        // Nothing to show but what the fields say.
        let bare = message.content.trim().is_empty()
            && message.title.trim().is_empty()
            && message.attachments.is_empty()
            && extras.audio.is_none()
            && location.is_none();
        let notes = extras.notes(bare);
        // Their icon alone isn't a message either (it's kept, above).
        if bare && notes.is_empty() && extras.reaction.is_none() && extras.appearance.is_some() && extras.telemetry.is_none() {
            return;
        }
        // A reaction goes on the message it's for: on its own, it's not a
        // message.
        if let Some(reaction) = extras.reaction.clone() {
            self.on_reaction(&key, id.clone(), &message, reaction);
            if bare && notes.is_empty() {
                if paper {
                    self.notify(format!("Read a reaction from {} on paper", self.store.display_name(&key)));
                }
                return;
            }
        }

        // Files, and a voice message, are saved as they arrive.
        let mut attachments = Vec::new();
        let audio = extras.audio.as_ref().map(|a| {
            let (name, data) = a.file();
            (name, data, false, Some(a.codec()))
        });
        let files: Vec<(String, &[u8], bool, Option<String>)> = message
            .attachments
            .iter()
            .map(|a| (a.name.clone(), a.data.as_slice(), a.image, None))
            .chain(audio)
            .collect();
        if !files.is_empty() {
            let dir = self.paths.downloads.join(&key[..key.len().min(12)]);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                self.log(format!("Could not create {}: {e}", dir.display()));
            }
            for (name, data, image, voice) in files {
                let path = unique_path(&dir, &name);
                match std::fs::write(&path, data) {
                    Ok(()) => attachments.push(StoredAttachment { name, path, size: data.len() as u64, image, voice }),
                    Err(e) => self.log(format!("Could not save {}: {e}", path.display())),
                }
            }
        }

        let stored = Message {
            id,
            incoming: true,
            title: message.title.clone(),
            content: message.content.clone(),
            timestamp: message.timestamp,
            state: MessageState::Received {
                verified: message.verified,
            },
            attachments,
            reply: message.reply.clone().map(|reply| ReplyTo { hash: hex::encode(reply.to), quote: reply.quote }),
            location,
            notes,
            format: extras.format.filter(|_| !message.content.trim().is_empty()),
            ..Message::default()
        };
        // On screen, in a window that has the focus: what arrives while the
        // user is away counts as unread.
        let is_active = paper
            || (self.focused && self.tab == Tab::Messages && self.active_conversation.as_deref() == Some(key.as_str()));
        // What the notification says: the text, else what came with it.
        let preview = if !message.content.trim().is_empty() {
            notify::body(&stored.text())
        } else if !message.title.trim().is_empty() {
            notify::body(&message.title)
        } else {
            match stored.attachments.as_slice() {
                [one] if one.voice.is_some() => "🎤 Voice message".to_string(),
                [one] => format!("📎 {}", one.name),
                [] => stored.opening(),
                many => format!("📎 {} files", many.len()),
            }
        };
        // A location update, or a command: shown, but not unread, and no
        // notification. A newer one replaces the one before it.
        let quiet = stored.is_quiet();
        let summary = stored.opening();
        let conversation = self.store.conversations.entry(key.clone()).or_default();
        let muted = conversation.muted;
        if quiet
            && conversation.messages.last().is_some_and(|last| last.same_quiet_kind(&stored) && last.timestamp <= stored.timestamp)
        {
            conversation.messages.pop();
        }
        conversation.messages.push(stored);
        // Messages downloaded later may be older than ones already shown.
        conversation
            .messages
            .sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
        conversation.adopt_strays();
        if !is_active && !quiet {
            conversation.unread += 1;
        }
        self.store_dirty = true;
        let name = self.store.display_name(&key);
        if paper {
            self.notify(format!("Read a paper message from {name}"));
            self.paper_read = Some(key);
            return;
        }
        self.keep_conversation_selection();
        if quiet {
            self.log(format!("{name}: {summary}"));
            return;
        }
        self.log(format!("Message from {name}"));
        if self.settings.notify_messages && !muted {
            let target = Target::Conversation { key };
            self.push_notification(is_active, Notification { title: name, body: preview, target });
        }
    }

    /// A reaction from them: on the message it's for, or kept until that
    /// message is here. Reactions to your messages get a notification.
    fn on_reaction(&mut self, key: &str, id: String, message: &crate::lxmf::InboundMessage, reaction: lxmf::Reaction) {
        let name = self.store.display_name(key);
        let Some(conversation) = self.store.conversations.get_mut(key) else {
            self.log(format!("Ignored a reaction from {name}, who you have no conversation with"));
            return;
        };
        let to = hex::encode(reaction.to);
        let stored = Reaction {
            emoji: reaction.emoji.clone(),
            incoming: true,
            timestamp: message.timestamp,
            id,
            state: MessageState::Received { verified: message.verified },
        };
        if !conversation.add_reaction(&to, stored) {
            return;
        }
        self.store_dirty = true;
        let yours = conversation.by_hash(&to).filter(|(_, m)| !m.incoming).map(|(_, m)| m.opening());
        let muted = conversation.muted;
        self.log(format!("{name} reacted {}", reaction.emoji));
        if let Some(opening) = yours
            && self.settings.notify_messages
            && !muted
            && !message.paper
        {
            let on_screen = self.focused && self.tab == Tab::Messages && self.active_conversation.as_deref() == Some(key);
            let body = notify::body(&format!("Reacted {} to “{opening}”", reaction.emoji));
            self.push_notification(on_screen, Notification { title: name, body, target: Target::Conversation { key: key.to_string() } });
        }
    }

    /// React to the message with id `message_id` in conversation `key`
    /// (sent like a message, with no text). Picking the same emoji again
    /// after it failed sends it again.
    pub fn send_reaction(&mut self, key: &str, message_id: &str, emoji: &str) -> Result<(), String> {
        let emoji = lxmf::fields::clean_reaction(emoji).ok_or("Nothing to react with")?;
        let to = parse_hash(key).ok_or("An LXMF address is 32 hex characters")?;
        let conversation = self.store.conversations.get_mut(key).ok_or("There's no such conversation")?;
        let message = conversation
            .messages
            .iter_mut()
            .find(|m| m.id == message_id)
            .ok_or("The message reacted to isn't in this conversation")?;
        let hash = message.lxmf_hash().ok_or("That message hasn't been sent, so there's nothing to react to yet")?.to_string();
        match message.reactions.iter().position(|r| !r.incoming && r.emoji == emoji) {
            Some(at) if matches!(message.reactions[at].state, MessageState::Failed(_)) => {
                message.reactions.remove(at);
            }
            Some(_) => return Err(format!("You reacted {emoji} to that already")),
            None => {}
        }
        let id = self.store.next_local_id;
        self.store.next_local_id += 1;
        let timestamp = now();
        message.reactions.push(Reaction {
            emoji: emoji.clone(),
            incoming: false,
            timestamp,
            id: format!("local-{id}"),
            state: MessageState::Sending,
        });
        self.store_dirty = true;
        self.remember_emoji(&emoji);
        let target = hex::decode(&hash).ok().and_then(|h| h.try_into().ok()).ok_or("That message's hash is not valid")?;
        let mode = match self.delivery_mode {
            DeliveryMode::Paper => DeliveryMode::Auto,
            mode => mode,
        };
        let reaction = lxmf::Reaction { to: target, emoji };
        let message = lxmf::Outgoing {
            reaction: Some(reaction),
            appearance: self.own_appearance(),
            ..lxmf::Outgoing::text(to, String::new(), Vec::new(), mode, timestamp)
        };
        self.send(NetCommand::SendMessage { id, message });
        Ok(())
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
        if self.active_conversation.as_ref() != Some(&key) {
            self.delivery_mode = self.delivery_for(&key);
        }
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

    /// Open the most recent conversation, as the terminal UI starts (the
    /// web UI doesn't: browsers mark what they show read).
    pub fn open_newest_conversation(&mut self) {
        if !self.store.conversations.is_empty() {
            self.conversations.select(Some(0));
            self.sync_active_conversation();
        }
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
            self.picked = None;
            // Their way of delivery (paper was for the one conversation).
            self.delivery_mode = self.active_conversation.as_deref().map_or(DeliveryMode::Auto, |key| self.delivery_for(key));
            // What's written (and attached) stays with the conversation it
            // was written in, rather than going to the one opened.
            let draft = (std::mem::take(&mut self.compose), std::mem::take(&mut self.attachments), self.reply.take());
            if let Some(previous) = previous
                && (!draft.0.text().is_empty() || !draft.1.is_empty() || draft.2.is_some())
            {
                self.drafts.insert(previous, draft);
            }
            if let Some((text, files, reply)) = self.active_conversation.as_ref().and_then(|key| self.drafts.remove(key)) {
                self.compose = text;
                self.attachments = files;
                self.reply = reply;
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
        let originals = std::mem::take(&mut self.attachments);
        let files = self.shrink_pictures(originals.clone());
        let reply = self.reply.take();
        if let Err((e, content, files)) = self.send_message(key.clone(), content, files, self.delivery_mode, reply.clone()) {
            // Keep what was written so it can be sent once the problem is
            // fixed, with the files picked rather than smaller copies.
            self.warn(e);
            for file in files.iter().filter(|f| !originals.contains(f)) {
                crate::store::remove_owned(file, std::slice::from_ref(&self.paths.uploads));
            }
            self.compose = TextInput::with_text(&content);
            self.attachments = originals;
            self.reply = reply;
            return;
        }
        // Paper is for one message; then it's their usual way again.
        if self.delivery_mode == DeliveryMode::Paper {
            self.delivery_mode = self.delivery_for(&key);
        }
        self.message_scroll = 0;
    }

    /// Our icon, as the settings have it (none unless one's set).
    pub fn own_appearance(&self) -> Option<lxmf::fields::Appearance> {
        let icon = self.settings.icon.clone()?;
        let colour = |text: &str, or: [u8; 3]| crate::icons::parse_colour(text).unwrap_or(or);
        Some(lxmf::fields::Appearance {
            icon,
            foreground: colour(&self.settings.icon_color, [0x0b, 0x0b, 0x10]),
            background: colour(&self.settings.icon_background, [0x4f, 0xd6, 0xe0]),
        })
    }

    /// Keep someone's icon, from a message written at `timestamp`, unless
    /// one from a newer message is kept already.
    fn keep_appearance(&mut self, key: &str, appearance: &lxmf::fields::Appearance, timestamp: f64) {
        let contact = self.store.contact(key);
        if contact.icon_at > timestamp || contact.icon.as_ref() == Some(appearance) && contact.icon_at == timestamp {
            return;
        }
        self.store.update_contact(key, |c| {
            c.icon = Some(appearance.clone());
            c.icon_at = timestamp;
        });
        self.store_dirty = true;
    }

    /// How messages to someone go: the mode kept for them, or auto.
    pub fn delivery_for(&self, key: &str) -> DeliveryMode {
        self.store.contact(key).delivery.unwrap_or(DeliveryMode::Auto)
    }

    /// Keep how messages to someone go. Paper isn't kept (it's for one
    /// message); auto is the default, so nothing is kept for it.
    pub fn set_delivery(&mut self, key: &str, mode: DeliveryMode) -> Result<(), String> {
        parse_hash(key).ok_or("An LXMF address is 32 hex characters")?;
        if mode == DeliveryMode::Paper {
            return Ok(());
        }
        let kept = (mode != DeliveryMode::Auto).then_some(mode);
        if self.store.contact(key).delivery != kept {
            self.store.update_contact(key, |c| c.delivery = kept);
            self.store_dirty = true;
        }
        Ok(())
    }

    /// The next delivery mode for the open conversation (`d`, or Ctrl-P
    /// while writing), kept for them unless it's paper.
    pub(super) fn cycle_delivery(&mut self) {
        self.delivery_mode = self.delivery_mode.next();
        let Some(key) = self.active_conversation.clone() else { return };
        let name = self.store.display_name(&key);
        match self.set_delivery(&key, self.delivery_mode) {
            Ok(()) if self.delivery_mode == DeliveryMode::Paper => self.confirm("Paper: the next message is written as a QR code"),
            Ok(()) => self.confirm(format!("Messages to {name}: {}", self.delivery_mode.label())),
            Err(e) => self.warn(e),
        }
    }

    /// Queue an LXMF message to `key` (a hex address), or write it as a
    /// paper message, as a reply to the message with id `reply` if given;
    /// its local id. On failure the text and files are handed back with the
    /// reason.
    pub fn send_message(
        &mut self,
        key: String,
        content: String,
        files: Vec<PathBuf>,
        mode: DeliveryMode,
        reply: Option<String>,
    ) -> Result<u64, (String, String, Vec<PathBuf>)> {
        let Some(to) = parse_hash(&key) else {
            return Err(("An LXMF address is 32 hex characters".into(), content, files));
        };
        if content.trim().is_empty() && files.is_empty() {
            return Err(("Nothing to send".into(), content, files));
        }
        if mode == DeliveryMode::Paper && !files.is_empty() {
            return Err(("A paper message carries text only".into(), content, files));
        }
        if mode == DeliveryMode::Propagated && self.propagation_node().is_none() {
            return Err(("Select a propagation node first (Network tab, p)".into(), content, files));
        }
        let key = hex::encode(to);
        let reply = match reply {
            None => None,
            Some(id) => match self.reply_for(&key, &id) {
                Ok(reply) => Some(reply),
                Err(e) => return Err((e, content, files)),
            },
        };
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
                voice: None,
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
        let timestamp = now();
        // Written in Markdown, and marked so (unless that's turned off).
        let format = (self.settings.markdown_messages && !content.trim().is_empty()).then_some(TextFormat::Markdown);
        self.store
            .conversations
            .entry(key)
            .or_default()
            .messages
            .push(Message {
                id: format!("local-{id}"),
                content: content.clone(),
                timestamp,
                state: MessageState::Sending,
                attachments,
                reply: reply.clone(),
                format,
                ..Message::default()
            });
        self.store_dirty = true;
        self.keep_conversation_selection();
        // Someone written to is a contact, spared the stamp.
        self.update_policy(false);
        let reply = reply.and_then(|reply| {
            let to = hex::decode(&reply.hash).ok()?.try_into().ok()?;
            Some(lxmf::Reply { to, quote: reply.quote })
        });
        if mode == DeliveryMode::Paper {
            self.send(NetCommand::WritePaper { id, paper: lxmf::paper::Paper { to, content, timestamp, reply, format } });
        } else {
            let appearance = self.own_appearance();
            let message = lxmf::Outgoing { reply, format, appearance, ..lxmf::Outgoing::text(to, content, files, mode, timestamp) };
            self.send(NetCommand::SendMessage { id, message });
        }
        Ok(id)
    }

    /// The message being replied to in the open conversation, and where it
    /// is.
    pub fn reply_target(&self) -> Option<(usize, &Message)> {
        let id = self.reply.as_deref()?;
        let conversation = self.store.conversations.get(self.active_conversation.as_deref()?)?;
        conversation.messages.iter().enumerate().find(|(_, m)| m.id == id)
    }

    /// The open conversation's messages that can be replied to (those with
    /// an LXMF hash), by index, oldest first.
    fn repliable(&self) -> Vec<usize> {
        let Some(conversation) = self.active_conversation.as_deref().and_then(|key| self.store.conversations.get(key)) else {
            return Vec::new();
        };
        conversation.messages.iter().enumerate().filter(|(_, m)| m.lxmf_hash().is_some()).map(|(i, _)| i).collect()
    }

    /// Reply to the message at `index` in the open conversation, bringing
    /// it into view, and write.
    fn reply_to_index(&mut self, index: usize) {
        let Some(message) = self.active_conversation.as_deref().and_then(|key| self.store.conversations.get(key)?.messages.get(index)) else {
            return;
        };
        if message.lxmf_hash().is_none() {
            return self.warn("That message hasn't been sent, so there's nothing to reply to yet");
        }
        self.reply = Some(message.id.clone());
        self.scroll_to = Some(index);
        self.composing = true;
    }

    /// Start a reply: to the newest message they sent, else the newest that
    /// can be answered.
    pub(super) fn start_reply(&mut self) {
        let repliable = self.repliable();
        let Some(conversation) = self.active_conversation.as_deref().and_then(|key| self.store.conversations.get(key)) else {
            return;
        };
        let theirs = repliable.iter().rev().find(|&&i| conversation.messages[i].incoming);
        match theirs.or(repliable.last()).copied() {
            Some(index) => self.reply_to_index(index),
            None => self.warn("Nothing to reply to here yet"),
        }
    }

    /// Reply to the message before (`older`) or after the one being replied to.
    pub(super) fn move_reply(&mut self, older: bool) {
        let repliable = self.repliable();
        let Some((current, _)) = self.reply_target() else { return self.start_reply() };
        let next = if older {
            repliable.iter().rev().find(|&&i| i < current)
        } else {
            repliable.iter().find(|&&i| i > current)
        };
        if let Some(&index) = next {
            self.reply_to_index(index);
        }
    }

    /// The picked message in the open conversation, and where it is.
    pub fn picked_message(&self) -> Option<(usize, &Message)> {
        let id = self.picked.as_deref()?;
        let conversation = self.store.conversations.get(self.active_conversation.as_deref()?)?;
        conversation.messages.iter().enumerate().find(|(_, m)| m.id == id)
    }

    /// Pick the message at `index` in the open conversation (or put it down
    /// if it's picked), bringing it into view.
    fn pick_index(&mut self, index: usize) {
        let Some(message) = self.active_conversation.as_deref().and_then(|key| self.store.conversations.get(key)?.messages.get(index)) else {
            return;
        };
        if self.picked.as_deref() == Some(message.id.as_str()) {
            self.picked = None;
            return;
        }
        self.picked = Some(message.id.clone());
        self.scroll_to = Some(index);
        self.composing = false;
    }

    /// Pick the newest message (`m`).
    pub(super) fn start_picking(&mut self) {
        let count = self.active_conversation.as_deref().and_then(|key| self.store.conversations.get(key)).map_or(0, |c| c.messages.len());
        match count {
            0 => self.warn("No messages here yet"),
            n => self.pick_index(n - 1),
        }
    }

    /// Pick the message before (`older`) or after the picked one.
    fn move_pick(&mut self, older: bool) {
        let Some((current, _)) = self.picked_message() else { return self.start_picking() };
        let count = self.active_conversation.as_deref().and_then(|key| self.store.conversations.get(key)).map_or(0, |c| c.messages.len());
        let next = if older { current.checked_sub(1) } else { Some(current + 1).filter(|&i| i < count) };
        if let Some(index) = next {
            self.pick_index(index);
        }
    }

    /// Do `action` with the message at `index` in the open conversation.
    pub(super) fn message_action(&mut self, index: usize, action: MessageAction) {
        let Some(message) = self.active_conversation.as_deref().and_then(|key| self.store.conversations.get(key)?.messages.get(index)) else {
            return;
        };
        let id = message.id.clone();
        match action {
            MessageAction::Reply => {
                self.picked = None;
                self.reply_to_index(index);
            }
            MessageAction::React => {
                if message.lxmf_hash().is_none() {
                    return self.warn("That message hasn't been sent, so there's nothing to react to yet");
                }
                self.open_reaction_picker(id);
            }
            MessageAction::Copy => {
                let text = message.content.clone();
                let text = if text.is_empty() { message.opening() } else { text };
                self.copy(&text, "the message");
            }
            MessageAction::Retry => {
                let Some(key) = self.active_conversation.clone() else { return };
                match self.retry_message(&key, &id, self.delivery_mode) {
                    Ok(()) => self.confirm("Sending it again"),
                    Err(e) => self.warn(e),
                }
            }
            MessageAction::Delete => {
                let Some(key) = self.active_conversation.clone() else { return };
                let what = if message.attachments.is_empty() { "this message" } else { "this message and its files" };
                self.open_prompt(PromptKind::ConfirmDeleteMessage { key, id }, &format!("Delete {what}? Type y"), "");
            }
        }
    }

    /// What can be done with a message (the buttons a picked one shows).
    pub fn message_actions(message: &Message) -> Vec<MessageAction> {
        let mut actions = Vec::new();
        if message.lxmf_hash().is_some() {
            actions.extend([MessageAction::Reply, MessageAction::React]);
        }
        actions.push(MessageAction::Copy);
        if !message.incoming && matches!(message.state, MessageState::Failed(_)) {
            actions.push(MessageAction::Retry);
        }
        actions.push(MessageAction::Delete);
        actions
    }

    /// Send again, as MeshChat does, the messages to `key` that failed,
    /// now that they announced (so they can be reached): text ones only (a
    /// file can be large), not paper ones (they'd go over the network
    /// instead), and not ones sent again on their own a moment ago, should
    /// they announce often. By how messages to them go.
    pub(super) fn resend_on_announce(&mut self, key: &str) {
        if !self.settings.resend_on_announce {
            return;
        }
        let Some(conversation) = self.store.conversations.get(key) else { return };
        let now = Instant::now();
        let failed: Vec<String> = conversation
            .messages
            .iter()
            .filter(|m| !m.incoming && m.attachments.is_empty() && m.id.starts_with("local-"))
            .filter(|m| matches!(&m.state, MessageState::Failed(why) if !why.starts_with(PAPER_FAILED)))
            .filter(|m| self.auto_resent.get(&m.id).is_none_or(|at| now.duration_since(*at) >= RESEND_GAP))
            .map(|m| m.id.clone())
            .collect();
        let mode = self.delivery_for(key);
        let mut sent = 0;
        for id in failed {
            self.auto_resent.insert(id.clone(), now);
            if self.retry_message(key, &id, mode).is_ok() {
                sent += 1;
            }
        }
        if sent > 0 {
            let name = self.store.display_name(key);
            self.log(format!("{name} announced: sending {sent} message(s) that failed again"));
        }
    }

    /// Send a message of yours that failed again, the same message (so a
    /// copy that did get there isn't shown twice), by `mode`.
    pub fn retry_message(&mut self, key: &str, id: &str, mode: DeliveryMode) -> Result<(), String> {
        let to = parse_hash(key).ok_or("An LXMF address is 32 hex characters")?;
        if mode == DeliveryMode::Propagated && self.propagation_node().is_none() {
            return Err("Select a propagation node first (Network tab, p)".into());
        }
        let number: u64 = id.strip_prefix("local-").and_then(|n| n.parse().ok()).ok_or("Only your own messages can be sent again")?;
        let message = self
            .store
            .conversations
            .get_mut(key)
            .and_then(|c| c.messages.iter_mut().find(|m| m.id == id))
            .ok_or("That message isn't in this conversation")?;
        if !matches!(message.state, MessageState::Failed(_)) {
            return Err("Only a message that failed can be sent again".into());
        }
        let files: Vec<PathBuf> = message.attachments.iter().map(|a| a.path.clone()).collect();
        if let Some(missing) = message.attachments.iter().find(|a| !a.path.is_file()) {
            return Err(format!("{} isn't there any more, so it can't be sent again", missing.name));
        }
        if mode == DeliveryMode::Paper && !files.is_empty() {
            return Err("A paper message carries text only".into());
        }
        message.state = MessageState::Sending;
        let (content, timestamp, format) = (message.content.clone(), message.timestamp, message.format);
        let reply = message.reply.clone().and_then(|reply| {
            let to = hex::decode(&reply.hash).ok()?.try_into().ok()?;
            Some(lxmf::Reply { to, quote: reply.quote })
        });
        self.store_dirty = true;
        if mode == DeliveryMode::Paper {
            self.send(NetCommand::WritePaper { id: number, paper: lxmf::paper::Paper { to, content, timestamp, reply, format } });
        } else {
            let appearance = self.own_appearance();
            let message = lxmf::Outgoing { reply, format, appearance, ..lxmf::Outgoing::text(to, content, files, mode, timestamp) };
            self.send(NetCommand::SendMessage { id: number, message });
        }
        Ok(())
    }

    /// Folders of files rettui saved (received, or uploaded from the web
    /// UI), which deleting a message deletes its files from. Files sent
    /// from elsewhere on this computer are left alone.
    fn owned_folders(&self) -> Vec<PathBuf> {
        vec![self.paths.downloads.clone(), self.paths.uploads.clone()]
    }

    /// Delete a message (and its files rettui saved).
    pub fn delete_message(&mut self, key: &str, id: &str) -> Result<(), String> {
        let owned = self.owned_folders();
        let conversation = self.store.conversations.get_mut(key).ok_or("There's no such conversation")?;
        let at = conversation.messages.iter().position(|m| m.id == id).ok_or("That message isn't in this conversation")?;
        let message = conversation.messages.remove(at);
        conversation.unread = conversation.unread.min(conversation.messages.len());
        for attachment in &message.attachments {
            crate::store::remove_owned(&attachment.path, &owned);
            self.pictures.remove(&attachment.path);
        }
        for id in [&mut self.picked, &mut self.reply, &mut self.reacting] {
            if id.as_deref() == Some(message.id.as_str()) {
                *id = None;
            }
        }
        self.store_dirty = true;
        Ok(())
    }

    /// Delete a conversation: its messages, those in the archive, and their
    /// files rettui saved. What you keep about the contact stays. False if
    /// there's no such conversation.
    pub fn delete_conversation(&mut self, key: &str) -> bool {
        let Some(conversation) = self.store.conversations.remove(key) else { return false };
        let owned = self.owned_folders();
        for attachment in conversation.messages.iter().flat_map(|m| &m.attachments) {
            crate::store::remove_owned(&attachment.path, &owned);
        }
        // Their folder of downloads, if that leaves it empty.
        let _ = std::fs::remove_dir(self.paths.downloads.join(&key[..key.len().min(12)]));
        self.drafts.remove(key);
        self.read(Target::Conversation { key: key.to_string() });
        if self.active_conversation.as_deref() == Some(key) {
            self.active_conversation = None;
            (self.picked, self.reply, self.reacting) = (None, None, None);
            self.compose = TextInput::default();
            self.attachments.clear();
            self.conversations.select(None);
            self.keep_conversation_selection();
        }
        if conversation.archived > 0 {
            let (archive, key) = (self.paths.archive.clone(), key.to_string());
            self.saver.run("delete the conversation's archived messages", move || {
                crate::store::remove_from_archive(&archive, &key, &owned)
            });
        }
        self.store_dirty = true;
        self.update_policy(false);
        true
    }

    /// Ask before deleting the open conversation (`X`).
    pub(super) fn ask_delete_conversation(&mut self, key: String) {
        let name = self.store.display_name(&key);
        let title = format!("Delete the conversation with {name}, and the files it brought? Type y");
        self.open_prompt(PromptKind::ConfirmDeleteConversation(key), &title, "");
    }

    /// Keys while a message is picked; true when the key was one of them.
    fn picked_key(&mut self, key: KeyEvent) -> bool {
        let Some((index, _)) = self.picked_message() else {
            self.picked = None;
            return false;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_pick(true),
            KeyCode::Down | KeyCode::Char('j') => self.move_pick(false),
            KeyCode::Enter | KeyCode::Char('r') => self.message_action(index, MessageAction::Reply),
            KeyCode::Char('e') => self.message_action(index, MessageAction::React),
            KeyCode::Char('y') => self.message_action(index, MessageAction::Copy),
            KeyCode::Char('o') => self.open_picked(index),
            KeyCode::Char('t') => self.message_action(index, MessageAction::Retry),
            KeyCode::Char('x') | KeyCode::Delete => self.message_action(index, MessageAction::Delete),
            KeyCode::Esc => self.picked = None,
            _ => return false,
        }
        true
    }

    /// Open what the picked message brought: its first file, else its
    /// location on a map.
    fn open_picked(&mut self, index: usize) {
        let Some(message) = self.active_conversation.as_deref().and_then(|key| self.store.conversations.get(key)?.messages.get(index)) else {
            return;
        };
        if let Some(attachment) = message.attachments.first() {
            let path = attachment.path.clone();
            self.open_file(&path);
        } else if let Some(location) = message.location {
            self.open_url(&location.map_url());
        } else {
            self.warn("Nothing to open in that message");
        }
    }

    /// The emoji picker, to react to the message with id `id`.
    fn open_reaction_picker(&mut self, id: String) {
        self.reacting = Some(id);
        self.open_emoji_picker_for(super::emoji::EmojiTarget::Reaction);
    }

    /// The emoji picked to react with: sent.
    pub(super) fn react_with(&mut self, emoji: &str) {
        let (Some(key), Some(id)) = (self.active_conversation.clone(), self.reacting.take()) else { return };
        match self.send_reaction(&key, &id, emoji) {
            Ok(()) => self.confirm(format!("Reacted {emoji}")),
            Err(e) => self.warn(e),
        }
    }

    /// What a reply to the message with id `id` in conversation `key`
    /// carries: its hash, and the start of its text to quote.
    fn reply_for(&self, key: &str, id: &str) -> Result<ReplyTo, String> {
        let message = self
            .store
            .conversations
            .get(key)
            .and_then(|c| c.messages.iter().find(|m| m.id == id))
            .ok_or("The message replied to isn't in this conversation")?;
        let hash = message.lxmf_hash().ok_or("That message hasn't been sent, so there's nothing to reply to yet")?;
        Ok(ReplyTo { hash: hash.to_string(), quote: lxmf::quote_of(&message.content) })
    }

    /// A paper message written (or not): it's kept with its link, and shown
    /// as a QR code in the terminal UI when its conversation is open.
    pub(super) fn on_paper(&mut self, id: u64, result: Result<(String, [u8; 32]), String>) {
        let local = format!("local-{id}");
        let Some(message) = self.store.find_message_mut(&local) else { return };
        match result {
            Ok((link, hash)) => {
                message.state = MessageState::Delivered;
                message.paper = Some(link.clone());
                message.hash = Some(hex::encode(hash));
                self.store_dirty = true;
                if self.tab == Tab::Messages {
                    self.paper_view = Some(PaperView::new(link));
                }
            }
            Err(e) => {
                message.state = MessageState::Failed(format!("{PAPER_FAILED}{e}"));
                self.store_dirty = true;
                self.fail(format!("Could not write the paper message: {e}"));
            }
        }
    }

    /// Read in a paper message (its `lxm://` link): checked for this client
    /// here, then decrypted and verified like any message received.
    pub fn read_paper(&mut self, link: &str) -> Result<(), String> {
        let link = link.trim();
        let (to, _) = lxmf_core::message_api::LxMessage::decode_paper_uri(link)
            .map_err(|_| "Not a paper message: those are lxm:// links (or their QR codes)")?;
        if self.lxmf_hash.is_some_and(|own| own != to) {
            return Err(format!("This paper message is for {}, not for you", hex::encode(to)));
        }
        self.send(NetCommand::ReadPaper(link.to_string()));
        Ok(())
    }

    /// Open the conversation a paper message was just read into (the
    /// terminal UI does; a browser knows it from its request).
    pub fn open_paper_read(&mut self) {
        if let Some(key) = self.paper_read.take() {
            self.open_conversation(key);
            self.composing = false;
        }
    }

    /// The newest paper message written in the open conversation (`P`).
    pub(super) fn show_newest_paper(&mut self) {
        let link = self
            .active_conversation
            .as_ref()
            .and_then(|key| self.store.conversations.get(key))
            .and_then(|c| c.messages.iter().rev().find_map(|m| m.paper.clone()));
        match link {
            Some(link) => self.paper_view = Some(PaperView::new(link)),
            None => self.warn("No paper message written in this conversation (d picks paper, then write one)"),
        }
    }

    /// Save the paper message shown as an SVG image, to print or pass on
    /// (`s` on its QR code).
    pub(super) fn save_paper_qr(&mut self) {
        let Some(view) = &self.paper_view else { return };
        let svg = match &view.qr {
            Ok(qr) => qr.svg(),
            Err(e) => return self.warn(e.clone()),
        };
        let name = match view.kind {
            super::QrKind::Paper => {
                let who = self.active_conversation.as_deref().map_or("", |key| &key[..key.len().min(12)]);
                format!("paper-message-{who}.svg")
            }
            super::QrKind::Address => "my-lxmf-address.svg".to_string(),
        };
        let result = std::fs::create_dir_all(&self.paths.downloads).and_then(|()| {
            let path = unique_path(&self.paths.downloads, &name);
            std::fs::write(&path, svg).map(|()| path)
        });
        match result {
            Ok(path) => self.notify(format!("Saved the QR code to {}", path.display())),
            Err(e) => self.fail(format!("Could not save the QR code: {e}")),
        }
    }

    /// Move each conversation's messages beyond the newest "Messages kept"
    /// to the archive, by the month they were sent (written by the saver,
    /// before the store: if rettui stops in between, a message is in both,
    /// never neither). How many messages, from how many conversations.
    pub(super) fn archive_overflow(&mut self) -> (usize, usize) {
        let keep = usize::try_from(self.settings.messages_kept).unwrap_or(usize::MAX);
        if keep == 0 {
            return (0, 0);
        }
        let mut months: BTreeMap<PathBuf, Vec<Archived>> = BTreeMap::new();
        let (mut messages, mut conversations) = (0, 0);
        for (key, conversation) in &mut self.store.conversations {
            let over = conversation.messages.len().saturating_sub(keep);
            if over == 0 {
                continue;
            }
            // Oldest first (conversations are kept in time order).
            for message in conversation.messages.drain(..over) {
                let path = crate::store::archive_month(&self.paths.archive, message.timestamp);
                months.entry(path).or_default().push(Archived { conversation: key.clone(), message });
            }
            conversation.archived += over;
            // Unread ones among them can't be shown any more.
            conversation.unread = conversation.unread.min(conversation.messages.len());
            messages += over;
            conversations += 1;
        }
        for (path, archived) in months {
            let dir = self.paths.archive.clone();
            self.saver.append(path, "the message archive", move || {
                std::fs::create_dir_all(&dir)?;
                crate::store::encode_archive(&archived)
            });
        }
        if messages > 0 {
            self.store_dirty = true;
        }
        (messages, conversations)
    }

    /// Delete the archive's oldest months while messages use more than
    /// "Message storage" allows (after what's queued to save). `warn`: say
    /// so when the store alone is over it.
    pub(super) fn keep_within_storage(&mut self, warn: bool) {
        let limit = self.settings.message_storage_mb;
        if limit == 0 {
            return;
        }
        let (store, archive) = (self.paths.store.clone(), self.paths.archive.clone());
        self.saver.run("keep messages within the storage limit", move || {
            let (mut notes, over) = crate::store::keep_within(&store, &archive, limit * 1_000_000)?;
            if over && warn {
                notes.push(format!(
                    "The messages kept use more than the {limit} MB allowed for messages: lower \"Messages kept\" to use less"
                ));
            }
            Ok(notes)
        });
    }

    /// Archive what's over the limit, save straight away and keep within
    /// the storage limit, saying so in the log (at start, and when either
    /// limit changes).
    pub(super) fn archive_overflow_now(&mut self) {
        let (messages, conversations) = self.archive_overflow();
        if messages > 0 {
            self.log(format!(
                "Archived {messages} older message(s) from {conversations} conversation(s) to {}",
                self.paths.archive.display()
            ));
            self.save_if_dirty();
        }
        self.keep_within_storage(true);
    }

    pub fn mark_conversation_read(&mut self, key: &str) {
        if let Some(conversation) = self.store.conversations.get_mut(key)
            && conversation.unread > 0
        {
            conversation.unread = 0;
            self.store_dirty = true;
            self.read(Target::Conversation { key: key.to_string() });
        }
    }

    pub(super) fn add_attachment(&mut self, text: &str) {
        let path = path_from_input(text);
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
        if self.picked.is_some() && self.picked_key(key) {
            return;
        }
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
            KeyCode::Char('r') if self.active_conversation.is_some() => self.start_reply(),
            KeyCode::Char('m') if self.active_conversation.is_some() => self.start_picking(),
            KeyCode::Char('/') => self.open_message_search(),
            KeyCode::Char('c') if self.active_conversation.is_some() => self.contact_card = self.active_conversation.clone(),
            KeyCode::Char('X') => {
                if let Some(key) = self.active_conversation.clone() {
                    self.ask_delete_conversation(key);
                }
            }
            KeyCode::Char('a') => self.open_attach_prompt(),
            KeyCode::Char('o') => self.open_latest_attachment(),
            KeyCode::Char('d') => self.cycle_delivery(),
            KeyCode::Char('p') => {
                self.open_prompt(PromptKind::ReadPaper, "Read a paper message (lxm:// link, or a picture of its QR code)", "");
            }
            KeyCode::Char('P') => self.show_newest_paper(),
            KeyCode::Char('N') => self.toggle_active_muted(),
            KeyCode::PageUp => self.message_scroll = self.message_scroll.saturating_add(self.history_page()),
            KeyCode::PageDown => self.message_scroll = self.message_scroll.saturating_sub(self.history_page()),
            KeyCode::Home => self.message_scroll = super::SCROLL_TOP,
            KeyCode::End => self.message_scroll = 0,
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
            let button = self.regions.history_buttons.iter().find(|(rect, _)| rect.contains(at)).map(|(_, hit)| hit.clone());
            match button.or_else(|| self.regions.history_rows.get(row).cloned().flatten()) {
                Some(HistoryHit::File(path)) => self.open_file(&path),
                Some(HistoryHit::Pick(index)) => self.pick_index(index),
                Some(HistoryHit::Original(index)) => self.scroll_to = Some(index),
                Some(HistoryHit::Link(url)) => self.open_url(&url),
                Some(HistoryHit::Action(index, action)) => self.message_action(index, action),
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::config::Settings;
    use crate::store::{Conversation, Store};

    fn key() -> String {
        "ab".repeat(16)
    }

    /// Message `n`, a month after message `n - 1`.
    fn message(n: usize, content: String) -> Message {
        Message {
            id: format!("m{n}"),
            incoming: true,
            content,
            timestamp: 1_700_000_000.0 + n as f64 * 31.0 * 86_400.0,
            state: MessageState::Received { verified: true },
            ..Message::default()
        }
    }

    #[test]
    fn each_conversation_keeps_its_delivery_mode_and_paper_is_for_one_message() {
        let dir = std::env::temp_dir().join(format!("rettui-delivery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (alice, bob) = (key(), "cd".repeat(16));
        let mut store = Store::default();
        store.conversations.insert(alice.clone(), Conversation { messages: vec![message(1, "hi".into())], ..Default::default() });
        store.conversations.insert(bob.clone(), Conversation { messages: vec![message(2, "yo".into())], ..Default::default() });
        let mut app = crate::app::test_app(&dir, Settings::default(), store);
        let open = |app: &mut App, key: &str| {
            let index = app.store.conversation_order().iter().position(|k| k == key).unwrap();
            app.select_conversation(index);
        };
        open(&mut app, &alice);
        assert_eq!(app.delivery_mode, DeliveryMode::Auto);
        app.messages_key(KeyEvent::new(KeyCode::Char('d'), crossterm::event::KeyModifiers::NONE));
        assert_eq!((app.delivery_mode, app.store.contact(&alice).delivery), (DeliveryMode::Direct, Some(DeliveryMode::Direct)));
        open(&mut app, &bob);
        assert_eq!(app.delivery_mode, DeliveryMode::Auto, "Bob's is his own");
        open(&mut app, &alice);
        assert_eq!(app.delivery_mode, DeliveryMode::Direct);
        // Direct → propagated → paper: paper isn't kept, and goes after one
        // message.
        app.cycle_delivery();
        app.cycle_delivery();
        assert_eq!((app.delivery_mode, app.store.contact(&alice).delivery), (DeliveryMode::Paper, Some(DeliveryMode::Propagated)));
        app.compose = TextInput::with_text("on paper");
        app.send_compose();
        assert_eq!(app.delivery_mode, DeliveryMode::Propagated);
        // Back to auto: nothing kept.
        app.set_delivery(&alice, DeliveryMode::Auto).unwrap();
        assert_eq!(app.store.contact(&alice).delivery, None);
        assert!(app.store.contact(&alice).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Text that barely compresses, so file sizes are predictable.
    fn noise(seed: usize, len: usize) -> String {
        let mut x = seed as u64 + 1;
        (0..len)
            .map(|_| {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                char::from(b'a' + ((x >> 59) as u8 % 26))
            })
            .collect()
    }

    fn store_of(messages: Vec<Message>) -> Store {
        let mut store = Store::default();
        let unread = messages.len();
        store.conversations.insert(key(), Conversation { messages, unread, ..Default::default() });
        store
    }

    fn app(dir: &Path, store: Store, kept: u64, storage_mb: u64) -> App {
        let settings = Settings { messages_kept: kept, message_storage_mb: storage_mb, ..Settings::default() };
        crate::app::test_app(dir, settings, store)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-archive-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The archive's messages (ids), oldest month first.
    fn archived_ids(dir: &Path) -> Vec<String> {
        let mut files: Vec<PathBuf> =
            std::fs::read_dir(dir).map(|e| e.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
        files.sort();
        files
            .iter()
            .flat_map(|path| {
                let mut text = String::new();
                flate2::read::MultiGzDecoder::new(std::fs::File::open(path).unwrap()).read_to_string(&mut text).unwrap();
                text.lines()
                    .map(|line| serde_json::from_str::<Archived>(line).unwrap())
                    .inspect(|a| assert_eq!(a.conversation, key()))
                    .map(|a| a.message.id)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn ids(from: usize, to: usize) -> Vec<String> {
        (from..to).map(|n| format!("m{n}")).collect()
    }

    #[test]
    fn older_messages_move_to_the_archive() {
        let dir = temp_dir("kept");
        let messages = (0..25).map(|n| message(n, format!("hello {n}"))).collect();
        let mut app = app(&dir, store_of(messages), 10, 0);
        // At start, the newest 10 stay.
        let conversation = &app.store.conversations[&key()];
        assert_eq!(conversation.messages.len(), 10);
        assert_eq!(conversation.messages[0].id, "m15");
        assert_eq!(conversation.archived, 15);
        assert!(app.log.iter().any(|l| l.contains("Archived 15 older message(s) from 1 conversation(s)")));
        // New ones push older ones out with the next save.
        let conversation = app.store.conversations.get_mut(&key()).unwrap();
        conversation.messages.extend((25..28).map(|n| message(n, format!("hello {n}"))));
        app.store_dirty = true;
        app.save_if_dirty();
        assert_eq!(app.store.conversations[&key()].archived, 18);
        // Keeping fewer archives more at once.
        app.update_settings(&[("messages_kept", "5")]).unwrap();
        let conversation = &app.store.conversations[&key()];
        assert_eq!((conversation.messages.len(), conversation.archived), (5, 23));
        let paths = app.paths.clone();
        app.finish_saves();
        // Every archived message, once, in order; the store has the rest.
        assert_eq!(archived_ids(&paths.archive), ids(0, 23));
        let saved = Store::load(&paths.store).unwrap();
        let conversation = &saved.conversations[&key()];
        assert_eq!(conversation.messages.iter().map(|m| m.id.clone()).collect::<Vec<_>>(), ids(23, 28));
        assert_eq!(conversation.archived, 23);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zero_keeps_every_message() {
        let dir = temp_dir("all");
        let messages = (0..25).map(|n| message(n, format!("hello {n}"))).collect();
        let mut app = app(&dir, store_of(messages), 0, 0);
        assert_eq!(app.store.conversations[&key()].messages.len(), 25);
        let paths = app.paths.clone();
        app.finish_saves();
        assert!(!paths.archive.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_storage_limit_deletes_the_oldest_archived_months() {
        let dir = temp_dir("limit");
        // 25 messages of 100 KB of text that barely compresses.
        let messages = (0..25).map(|n| message(n, noise(n, 100_000))).collect();
        let mut app = app(&dir, store_of(messages), 2, 1);
        let paths = app.paths.clone();
        app.finish_saves();
        app.on_tick();
        let size = |path: &Path| std::fs::metadata(path).map_or(0, |m| m.len());
        let archive: u64 = std::fs::read_dir(&paths.archive).unwrap().map(|e| size(&e.unwrap().path())).sum();
        assert!(size(&paths.store) + archive <= 1_000_000, "{} + {archive}: {:?}", size(&paths.store), app.log);
        // The newest archived months are the ones left.
        let left = archived_ids(&paths.archive);
        assert!(!left.is_empty() && left.len() < 23, "{left:?}");
        assert_eq!(left, ids(23 - left.len(), 23));
        assert!(app.log.iter().any(|l| l.contains("Deleted the archived messages of")));
        assert!(!app.log.iter().any(|l| l.contains("lower \"Messages kept\"")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_store_bigger_than_the_limit_is_reported() {
        let dir = temp_dir("over");
        let messages = (0..25).map(|n| message(n, noise(n, 100_000))).collect();
        // Keeping 20 of them is about 1.3 MB, over the 1 MB allowed.
        let mut app = app(&dir, store_of(messages), 20, 1);
        let paths = app.paths.clone();
        app.finish_saves();
        app.on_tick();
        assert!(archived_ids(&paths.archive).is_empty(), "every archived month went");
        assert!(app.log.iter().any(|l| l.contains("lower \"Messages kept\"")), "{:?}", app.log);
        assert_eq!(Store::load(&paths.store).unwrap().conversations[&key()].messages.len(), 20);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn announces_save_the_peers_not_the_store() {
        let dir = temp_dir("announce");
        let mut app = app(&dir, Store::default(), 1000, 0);
        let announce = crate::net::NetEvent::Announce { kind: crate::net::PeerKind::Lxmf, hash: [7; 16], name: Some("Bob".into()), hops: 2 };
        app.on_net(announce);
        assert!(app.peers_dirty && !app.store_dirty);
        let paths = app.paths.clone();
        app.finish_saves();
        assert!(!paths.store.exists(), "no conversation changed");
        let loaded = Store::load(&paths.store).unwrap();
        assert_eq!(loaded.display_name(&"07".repeat(16)), "Bob");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replies_name_and_quote_the_message_they_answer() {
        use crate::lxmf::{Delivered, Reply, Sent};
        use crate::net::NetEvent;
        let dir = temp_dir("replies");
        let mut app = app(&dir, Store::default(), 1000, 0);
        let incoming = |n: u8, content: &str, reply: Option<Reply>| crate::lxmf::InboundMessage {
            id: Some([n; 32]),
            source: [0xab; 16],
            content: content.into(),
            timestamp: 1_700_000_000.0 + f64::from(n),
            verified: true,
            reply,
            ..Default::default()
        };
        app.on_message(incoming(1, "Lunch tomorrow?\nAt noon", None));
        let theirs = hex::encode([1u8; 32]);
        // A reply to theirs carries its hash and the start of its text.
        let id = app.send_message(key(), "Yes!".into(), Vec::new(), DeliveryMode::Auto, Some(theirs.clone())).unwrap();
        let conversation = &app.store.conversations[&key()];
        let mine = conversation.messages.last().unwrap();
        let reply = mine.reply.clone().unwrap();
        assert_eq!(reply.hash, theirs);
        assert_eq!(reply.quote.as_deref(), Some("Lunch tomorrow?\nAt noon"));
        // It shows their message's first line.
        let quoted = conversation.quoted(&reply);
        assert_eq!((quoted.index, quoted.incoming, quoted.text.as_str()), (Some(0), Some(true), "Lunch tomorrow?"));
        // Mine can't be answered until it's sent; then it has a hash.
        let local = format!("local-{id}");
        let err = app.send_message(key(), "and".into(), Vec::new(), DeliveryMode::Auto, Some(local.clone())).unwrap_err();
        assert!(err.0.contains("hasn't been sent"), "{}", err.0);
        app.on_net(NetEvent::Delivery { id, result: Ok(Sent { delivered: Delivered::Direct, hash: [5; 32] }) });
        assert_eq!(app.store.find_message_mut(&local).unwrap().hash, Some(hex::encode([5u8; 32])));
        // Their reply to mine finds it; one to a message not here shows
        // what it quoted.
        app.on_message(incoming(2, "Great", Some(Reply { to: [5; 32], quote: Some("Yes!".into()) })));
        app.on_message(incoming(3, "Also", Some(Reply { to: [8; 32], quote: Some("Old news".into()) })));
        let conversation = &app.store.conversations[&key()];
        let quoted = |content: &str| {
            let message = conversation.messages.iter().find(|m| m.content == content).unwrap();
            let quoted = conversation.quoted(message.reply.as_ref().unwrap());
            (quoted.index.map(|i| conversation.messages[i].content.clone()), quoted.incoming, quoted.text)
        };
        assert_eq!(quoted("Great"), (Some("Yes!".to_string()), Some(false), "Yes!".to_string()));
        assert_eq!(quoted("Also"), (None, None, "Old news".to_string()));
        // Nor is the reply kept if what it answers isn't in this conversation.
        let err = app.send_message(key(), "x".into(), Vec::new(), DeliveryMode::Auto, Some("nope".into())).unwrap_err();
        assert!(err.0.contains("isn't in this conversation"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A message from them (0xab…), number `n`.
    fn from_them(n: u8, content: &str, extras: crate::lxmf::Extras) -> crate::lxmf::InboundMessage {
        crate::lxmf::InboundMessage {
            id: Some([n; 32]),
            source: [0xab; 16],
            content: content.into(),
            timestamp: 1_800_000_000.0 + f64::from(n),
            verified: true,
            extras,
            ..Default::default()
        }
    }

    #[test]
    fn their_icon_is_kept_and_ours_is_sent_once_set() {
        use crate::lxmf::Extras;
        use crate::lxmf::fields::Appearance;
        let dir = temp_dir("icons");
        let mut app = app(&dir, Store::default(), 1000, 0);
        let tower = Appearance { icon: "radio-tower".into(), foreground: [255; 3], background: [0, 0, 128] };
        let antenna = Appearance { icon: "antenna".into(), ..tower.clone() };
        let with = |n: u8, content: &str, icon: &Appearance| {
            from_them(n, content, Extras { appearance: Some(icon.clone()), ..Extras::default() })
        };
        app.on_message(with(2, "hello", &tower));
        assert_eq!(app.store.contact(&key()).icon.as_ref(), Some(&tower));
        // An older message's icon doesn't replace a newer one's.
        app.on_message(with(1, "from before", &antenna));
        assert_eq!(app.store.contact(&key()).icon.as_ref(), Some(&tower));
        // An icon alone is kept, but isn't a message.
        let count = app.store.conversations[&key()].messages.len();
        app.on_message(with(3, "", &antenna));
        assert_eq!(app.store.contact(&key()).icon.as_ref(), Some(&antenna));
        assert_eq!(app.store.conversations[&key()].messages.len(), count);
        // Ours: none until one's set.
        assert_eq!(app.own_appearance(), None);
        app.settings.icon = Some("account".into());
        let ours = app.own_appearance().unwrap();
        assert_eq!((ours.icon.as_str(), ours.foreground, ours.background), ("account", [0x0b, 0x0b, 0x10], [0x4f, 0xd6, 0xe0]));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reactions_go_on_their_message_both_ways() {
        use crate::lxmf::{Delivered, Extras, Reaction as Wire, Sent};
        use crate::net::NetEvent;
        let dir = temp_dir("reactions");
        let mut app = app(&dir, Store::default(), 1000, 0);
        app.set_focus(false);
        let id = app.send_message(key(), "Lunch?".into(), Vec::new(), DeliveryMode::Auto, None).unwrap();
        // Their reaction to mine arrives before my delivery proof does: it
        // waits, and goes on my message once it has its hash.
        let reaction = |n: u8, to: [u8; 32], emoji: &str| {
            from_them(n, "", Extras { reaction: Some(Wire { to, emoji: emoji.into() }), ..Extras::default() })
        };
        app.on_message(reaction(1, [5; 32], "👍"));
        let conversation = &app.store.conversations[&key()];
        assert_eq!(conversation.messages.len(), 1, "a reaction isn't a message");
        assert_eq!(conversation.stray_reactions.len(), 1);
        app.on_net(NetEvent::Delivery { id, result: Ok(Sent { delivered: Delivered::Direct, hash: [5; 32] }) });
        let conversation = &app.store.conversations[&key()];
        assert!(conversation.stray_reactions.is_empty());
        assert_eq!(conversation.messages[0].reactions[0].emoji, "👍");
        // Once it's there, another comes with a notification (it's mine),
        // and the same one twice is kept once.
        app.take_notifications();
        app.on_message(reaction(2, [5; 32], "❤️"));
        app.on_message(reaction(2, [5; 32], "❤️"));
        assert_eq!(app.store.conversations[&key()].messages[0].reactions.len(), 2);
        let notes = app.take_notifications();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].body, "Reacted ❤️ to “Lunch?”");
        assert_eq!(app.store.conversations[&key()].unread, 0, "reactions aren't unread messages");
        // Mine to theirs: sending, then sent; a failed one goes again.
        app.on_message(from_them(3, "Sure", Extras::default()));
        let theirs = hex::encode([3u8; 32]);
        app.send_reaction(&key(), &theirs, "🎉").unwrap();
        let mine = |app: &App| app.store.conversations[&key()].messages[1].reactions[0].clone();
        assert_eq!((mine(&app).state, mine(&app).incoming), (MessageState::Sending, false));
        assert!(app.send_reaction(&key(), &theirs, "🎉").unwrap_err().contains("already"));
        let sent = mine(&app).id.trim_start_matches("local-").parse().unwrap();
        app.on_net(NetEvent::Delivery { id: sent, result: Err("no path".into()) });
        assert_eq!(mine(&app).state, MessageState::Failed("no path".into()));
        app.send_reaction(&key(), &theirs, "🎉").unwrap();
        assert_eq!(app.store.conversations[&key()].messages[1].reactions.len(), 1);
        assert_eq!(mine(&app).state, MessageState::Sending);
        assert_eq!(app.store.recent_emoji[0], "🎉");
        // Nothing to react to before it's sent.
        let local = format!("local-{}", app.send_message(key(), "x".into(), Vec::new(), DeliveryMode::Auto, None).unwrap());
        assert!(app.send_reaction(&key(), &local, "👍").unwrap_err().contains("hasn't been sent"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn locations_and_commands_are_shown_quietly() {
        use crate::lxmf::fields::{Command, Telemetry};
        use crate::lxmf::{Extras, Location};
        let dir = temp_dir("quiet");
        let mut app = app(&dir, Store::default(), 1000, 0);
        app.set_focus(false);
        let at = |lat: f64| Location { latitude: lat, longitude: 1.0, altitude: None, speed: None, bearing: None, accuracy: None, updated: None };
        let update = |n: u8, lat: f64| {
            from_them(n, "", Extras { telemetry: Some(Telemetry { time: None, location: Some(at(lat)) }), ..Extras::default() })
        };
        app.on_message(update(1, 50.0));
        app.on_message(update(2, 51.0));
        let conversation = &app.store.conversations[&key()];
        // One line, the newest place: no unread, no notification.
        assert_eq!(conversation.messages.len(), 1);
        assert_eq!(conversation.messages[0].location.map(|l| l.latitude), Some(51.0));
        assert_eq!(conversation.messages[0].opening(), "📍 Location");
        assert_eq!(conversation.unread, 0);
        assert!(app.take_notifications().is_empty());
        // A telemetry request is said in words; a text message in between
        // stops the next update replacing the one before.
        app.on_message(from_them(3, "", Extras { commands: vec![Command::TelemetryRequest], ..Extras::default() }));
        app.on_message(from_them(4, "hello", Extras::default()));
        app.on_message(update(5, 52.0));
        let conversation = &app.store.conversations[&key()];
        assert_eq!(conversation.messages.len(), 4);
        assert!(conversation.messages[1].notes[0].starts_with("Asked for your location"));
        assert_eq!(conversation.unread, 1);
        assert_eq!(app.take_notifications().len(), 1);
        // An empty message, with a field rettui doesn't know.
        app.on_message(from_them(6, "", Extras { unknown: vec![0x77], ..Extras::default() }));
        let last = app.store.conversations[&key()].messages.last().unwrap().clone();
        assert_eq!(last.notes, ["Sent something rettui can't show (LXMF field 0x77)"]);
        assert!(app.take_notifications().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn voice_messages_are_saved_to_play() {
        use crate::lxmf::Extras;
        use crate::lxmf::fields::Audio;
        let dir = temp_dir("voice");
        let mut app = app(&dir, Store::default(), 1000, 0);
        app.set_focus(false);
        let voice = |n: u8, mode: u8, data: &[u8]| from_them(n, "", Extras { audio: Some(Audio::new(mode, data.to_vec()).decoded()), ..Extras::default() });
        app.on_message(voice(1, 0x10, b"OggS opus"));
        app.on_message(voice(2, 0x04, &[0; 60]));
        app.on_message(voice(3, 0x03, b"\x01\x02"));
        let conversation = &app.store.conversations[&key()];
        let attachment = |i: usize| &conversation.messages[i].attachments[0];
        let (opus, codec2, c700) = (attachment(0), attachment(1), attachment(2));
        assert_eq!(std::fs::read(&opus.path).unwrap(), b"OggS opus");
        assert!(opus.path.ends_with("voice-message.ogg") && opus.playable());
        assert_eq!(opus.voice.as_deref(), Some("Opus"));
        // Codec2 1200 is decoded to WAV (10 frames: 0.4 s); 700C can't be.
        assert!(codec2.playable() && codec2.path.ends_with("voice-message.wav"));
        assert_eq!((codec2.voice.as_deref(), codec2.size), (Some("Codec2 1200"), 44 + 3200 * 2));
        assert!(!c700.playable() && c700.path.to_string_lossy().ends_with(".codec2"));
        assert_eq!(conversation.messages[0].opening(), "🎤 Voice message");
        let notes = app.take_notifications();
        assert_eq!(notes.len(), 1, "all together, about the one conversation");
        assert_eq!(notes[0].body, "🎤 Voice message");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_messages_go_again_as_the_same_message() {
        use crate::net::NetEvent;
        let dir = temp_dir("retry");
        let mut app = app(&dir, Store::default(), 1000, 0);
        let id = app.send_message(key(), "hello".into(), Vec::new(), DeliveryMode::Auto, None).unwrap();
        let local = format!("local-{id}");
        let state = |app: &mut App| app.store.find_message_mut(&local).unwrap().state.clone();
        assert!(app.retry_message(&key(), &local, DeliveryMode::Auto).unwrap_err().contains("failed"));
        app.on_net(NetEvent::Delivery { id, result: Err("no path".into()) });
        let before = app.store.find_message_mut(&local).unwrap().timestamp;
        app.retry_message(&key(), &local, DeliveryMode::Direct).unwrap();
        assert_eq!(state(&mut app), MessageState::Sending);
        let message = app.store.find_message_mut(&local).unwrap();
        assert_eq!(message.timestamp, before, "the same message, so the same hash");
        assert_eq!(app.store.conversations[&key()].messages.len(), 1);
        // Theirs can't be sent again, nor can a file that's gone.
        app.on_message(from_them(1, "hi", crate::lxmf::Extras::default()));
        let theirs = hex::encode([1u8; 32]);
        assert!(app.retry_message(&key(), &theirs, DeliveryMode::Auto).is_err());
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("gone.txt");
        std::fs::write(&file, b"x").unwrap();
        let with_file = app.send_message(key(), String::new(), vec![file.clone()], DeliveryMode::Auto, None).unwrap();
        app.on_net(NetEvent::Delivery { id: with_file, result: Err("timed out".into()) });
        std::fs::remove_file(&file).unwrap();
        let err = app.retry_message(&key(), &format!("local-{with_file}"), DeliveryMode::Auto).unwrap_err();
        assert!(err.contains("gone.txt isn't there any more"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_messages_go_again_when_their_recipient_announces() {
        use crate::net::{NetEvent, PeerKind};
        let dir = temp_dir("resend");
        let mut app = app(&dir, Store::default(), 1000, 0);
        let fail = |app: &mut App, id: u64| app.on_net(NetEvent::Delivery { id, result: Err("no path".into()) });
        let state = |app: &mut App, id: u64| app.store.find_message_mut(&format!("local-{id}")).unwrap().state.clone();
        let announce = |app: &mut App| {
            app.on_net(NetEvent::Announce { kind: PeerKind::Lxmf, hash: parse_hash(&key()).unwrap(), name: Some("Bob".into()), hops: 1 })
        };
        // A text message, one with a file, and a paper one that couldn't be
        // written: all failed.
        let text = app.send_message(key(), "hello".into(), Vec::new(), DeliveryMode::Auto, None).unwrap();
        fail(&mut app, text);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("photo.png");
        std::fs::write(&file, b"png").unwrap();
        let with_file = app.send_message(key(), String::new(), vec![file], DeliveryMode::Auto, None).unwrap();
        fail(&mut app, with_file);
        let paper = app.send_message(key(), "on paper".into(), Vec::new(), DeliveryMode::Paper, None).unwrap();
        app.on_net(NetEvent::Paper { id: paper, result: Err("no key".into()) });
        assert_eq!(state(&mut app, paper), MessageState::Failed("Not written: no key".into()));
        // They announce: the text one goes again, the others don't.
        announce(&mut app);
        assert_eq!(state(&mut app, text), MessageState::Sending);
        assert!(matches!(state(&mut app, with_file), MessageState::Failed(_)));
        assert!(matches!(state(&mut app, paper), MessageState::Failed(_)));
        // Failing again, it isn't sent again for a while, however often
        // they announce.
        fail(&mut app, text);
        announce(&mut app);
        assert!(matches!(state(&mut app, text), MessageState::Failed(_)));
        app.auto_resent.values_mut().for_each(|at| *at -= RESEND_GAP);
        announce(&mut app);
        assert_eq!(state(&mut app, text), MessageState::Sending);
        // Not when turned off.
        fail(&mut app, text);
        app.auto_resent.clear();
        app.update_settings(&[("resend_on_announce", "false")]).unwrap();
        announce(&mut app);
        assert!(matches!(state(&mut app, text), MessageState::Failed(_)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_messages_and_conversations_takes_their_files() {
        use crate::lxmf::{Attachment, InboundMessage};
        let dir = temp_dir("delete");
        // 30 messages, 20 kept: 10 go to the archive.
        let messages = (0..30).map(|n| message(n, format!("old {n}"))).collect();
        let mut app = app(&dir, store_of(messages), 20, 0);
        let mut photo = from_them(1, "look", crate::lxmf::Extras::default());
        photo.attachments = vec![Attachment { name: "photo.png".into(), data: b"png".to_vec(), image: true }];
        photo.timestamp = 1_900_000_000.0;
        app.on_message(photo);
        let conversation = &app.store.conversations[&key()];
        let saved = conversation.messages.last().unwrap().attachments[0].path.clone();
        assert!(saved.exists());
        // A message of yours from elsewhere on this computer keeps its file.
        std::fs::create_dir_all(dir.join("mine")).unwrap();
        let own = dir.join("mine").join("notes.txt");
        std::fs::write(&own, b"x").unwrap();
        let id = app.send_message(key(), String::new(), vec![own.clone()], DeliveryMode::Auto, None).unwrap();
        app.delete_message(&key(), &format!("local-{id}")).unwrap();
        assert!(own.exists());
        // Theirs takes the file rettui saved.
        app.picked = Some(hex::encode([1u8; 32]));
        app.delete_message(&key(), &hex::encode([1u8; 32])).unwrap();
        assert!(!saved.exists() && app.picked.is_none());
        assert_eq!(app.store.conversations[&key()].messages.len(), 20);
        assert!(app.delete_message(&key(), "nope").is_err());
        // The conversation goes, from the archive too.
        let paths = app.paths.clone();
        assert_eq!(app.store.conversations[&key()].archived, 10);
        let other = InboundMessage { source: [0xcd; 16], id: Some([9; 32]), content: "stay".into(), ..Default::default() };
        app.on_message(other);
        assert!(app.delete_conversation(&key()));
        assert!(!app.delete_conversation(&key()));
        app.finish_saves();
        app.on_tick();
        assert!(archived_ids(&paths.archive).is_empty());
        assert!(app.log.iter().any(|l| l.contains("Deleted 10 archived message(s)")), "{:?}", app.log);
        let saved = Store::load(&paths.store).unwrap();
        assert!(!saved.conversations.contains_key(&key()) && saved.conversations.contains_key(&"cd".repeat(16)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn messages_arriving_while_away_are_unread_until_back() {
        let dir = temp_dir("focus");
        let mut app = app(&dir, store_of(vec![message(0, "hi".into())]), 1000, 0);
        // Nothing is read just by starting (the web UI starts like this).
        assert_eq!(app.store.conversations[&key()].unread, 1);
        // The terminal UI opens the conversation in the Messages tab.
        app.open_newest_conversation();
        assert_eq!(app.active_conversation.as_deref(), Some(key().as_str()));
        let arrive = |app: &mut App, n: u8| {
            app.on_message(crate::lxmf::InboundMessage {
                id: Some([n; 32]),
                source: [0xab; 16],
                content: format!("message {n}"),
                timestamp: 1_800_000_000.0 + f64::from(n),
                verified: true,
                ..Default::default()
            });
        };
        let unread = |app: &App| app.store.conversations[&key()].unread;
        arrive(&mut app, 1);
        assert_eq!(unread(&app), 0, "read: on screen, and looked at");
        app.set_focus(false);
        arrive(&mut app, 2);
        arrive(&mut app, 3);
        assert_eq!(unread(&app), 2, "on screen, but nobody there");
        app.set_focus(true);
        assert_eq!(unread(&app), 0, "read on coming back");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
