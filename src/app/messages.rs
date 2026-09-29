//! Messages tab: LXMF conversations, composing, and attachments.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::files::{expand_home, unique_path};
use super::notify::{self, Notification, Target};
use super::{App, PromptKind, Tab, now};
use crate::lxmf::DeliveryMode;
use crate::net::{NetCommand, parse_hash};
use crate::store::{Archived, Message, MessageState, StoredAttachment};
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

        // On screen, in a window that has the focus: what arrives while the
        // user is away counts as unread.
        let is_active = self.focused
            && self.tab == Tab::Messages
            && self.active_conversation.as_deref() == Some(key.as_str());
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
            if let Some(Some(path)) = self.regions.history_rows.get(row).cloned() {
                self.open_file(&path);
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
            title: String::new(),
            content,
            timestamp: 1_700_000_000.0 + n as f64 * 31.0 * 86_400.0,
            state: MessageState::Received { verified: true },
            attachments: Vec::new(),
        }
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
                title: String::new(),
                content: format!("message {n}"),
                timestamp: 1_800_000_000.0 + f64::from(n),
                verified: true,
                attachments: Vec::new(),
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
