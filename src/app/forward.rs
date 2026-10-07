//! Forwarding a message to another conversation (its text and files, as a
//! new message), and exporting a conversation as text.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;
use super::files::unique_path;
use super::network::{matches_text, search_terms};
use crate::net::parse_hash;
use crate::store::{Message, MessageState};
use crate::term::input::TextInput;

/// The terminal UI's "forward to" list (`f` on a picked message).
pub struct ForwardPicker {
    /// The message: its conversation and id.
    pub from: String,
    pub id: String,
    pub input: TextInput,
    pub selected: usize,
}

impl App {
    /// Send the message `id` of conversation `from` on to `to`, as a new
    /// message: its text and copies of its files (so each message keeps its
    /// own), by how messages to them go. Its local id.
    pub fn forward_message(&mut self, from: &str, id: &str, to: &str) -> Result<u64, String> {
        let to = hex::encode(parse_hash(to).ok_or("An LXMF address is 32 hex characters")?);
        let message = self
            .store
            .conversations
            .get(from)
            .and_then(|c| c.messages.iter().find(|m| m.id == id))
            .cloned()
            .ok_or("That message isn't here any more")?;
        if message.content.trim().is_empty() && message.attachments.is_empty() {
            return Err("There's nothing in that message to forward (only its text and files are)".into());
        }
        let mut files: Vec<PathBuf> = Vec::new();
        let copied = |files: &[PathBuf]| {
            for file in files {
                let _ = std::fs::remove_file(file);
            }
        };
        for attachment in &message.attachments {
            let copy = unique_path(&self.paths.uploads, &attachment.name);
            let result = std::fs::create_dir_all(&self.paths.uploads).and_then(|()| std::fs::copy(&attachment.path, &copy));
            if let Err(e) = result {
                copied(&files);
                return Err(format!("Couldn't copy {}: {e}", attachment.name));
            }
            files.push(copy);
        }
        let mode = self.delivery_for(&to);
        self.send_message(to, message.content, files, mode, None).map_err(|(e, _, files)| {
            copied(&files);
            e
        })
    }

    /// Open the "forward to" list for the open conversation's message at
    /// `index`.
    pub(super) fn open_forward(&mut self, index: usize) {
        let Some(from) = self.active_conversation.clone() else { return };
        let Some(id) = self.store.conversations.get(&from).and_then(|c| c.messages.get(index)).map(|m| m.id.clone()) else {
            return;
        };
        self.forward = Some(ForwardPicker { from, id, input: TextInput::default(), selected: 0 });
    }

    /// Where it can go: conversations whose name or address holds every
    /// word typed (all of them, but message requests, when nothing is), and
    /// an address typed in full that has none yet.
    pub fn forward_targets(&self) -> Vec<String> {
        let Some(picker) = &self.forward else { return Vec::new() };
        let typed = picker.input.text().trim();
        let terms = search_terms(typed);
        let mut targets: Vec<String> = self
            .conversation_order()
            .into_iter()
            .filter(|key| !self.is_request(key))
            .filter(|key| terms.is_empty() || matches_text(&terms, &[self.store.display_name(key).as_str(), key.as_str()]))
            .collect();
        if let Some(hash) = parse_hash(typed.trim_start_matches("lxmf@")) {
            let key = hex::encode(hash);
            if !targets.contains(&key) {
                targets.insert(0, key);
            }
        }
        targets
    }

    pub(super) fn forward_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.forward_targets().len();
        let Some(picker) = self.forward.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.forward = None,
            KeyCode::Enter => {
                let selected = picker.selected;
                if let Some(to) = self.forward_targets().get(selected).cloned() {
                    self.forward_to(&to);
                }
            }
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => picker.selected = (picker.selected + 1).min(count.saturating_sub(1)),
            KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
            _ => {
                if picker.input.handle(key) {
                    picker.selected = 0;
                }
            }
        }
    }

    pub(super) fn paste_forward(&mut self, text: &str) {
        if let Some(picker) = self.forward.as_mut() {
            picker.input.insert_str(&text.replace(['\n', '\r'], " "));
            picker.selected = 0;
        }
    }

    /// Forward the picked one to `to`, and say so.
    pub(super) fn forward_to(&mut self, to: &str) {
        let Some(picker) = self.forward.take() else { return };
        match self.forward_message(&picker.from, &picker.id, to) {
            Ok(_) => {
                self.picked = None;
                self.confirm(format!("Forwarded to {}", self.store.display_name(to)));
            }
            Err(e) => self.warn(e),
        }
    }

    /// Write the open conversation, archived messages and all, to a text
    /// file in the downloads folder's `exports/`; where it went.
    pub fn export_conversation(&mut self, key: &str) -> Result<PathBuf, String> {
        let archived = crate::store::read_archive(&self.paths.archive, key).map_err(|e| format!("Couldn't read the archive: {e}"))?;
        let kept = self.store.conversations.get(key).map(|c| c.messages.clone()).unwrap_or_default();
        let name = self.store.display_name(key);
        let text = transcript(&name, key, archived.iter().chain(&kept));
        let dir = self.paths.downloads.join("exports");
        let stamp = chrono::Local::now().format("%Y-%m-%d");
        let file: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let path = unique_path(&dir, &format!("{}-{stamp}.txt", file.trim_matches('_').chars().take(40).collect::<String>()));
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
        Ok(path)
    }
}

/// A conversation as text: who it's with, then each message, oldest first,
/// with when it was written, who by, its text, and what came with it.
pub fn transcript<'a>(name: &str, key: &str, messages: impl Iterator<Item = &'a Message>) -> String {
    let when = |timestamp: f64| {
        chrono::DateTime::from_timestamp(timestamp as i64, 0)
            .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default()
    };
    let mut out = format!("Conversation with {name} ({key})\nExported by rettui on {}\n", when(chrono::Utc::now().timestamp() as f64));
    for message in messages {
        let who = if message.incoming { name } else { "You" };
        let state = match &message.state {
            MessageState::Failed(e) => format!("  (not sent: {e})"),
            MessageState::Sending => "  (not sent yet)".into(),
            MessageState::Received { verified: false } => "  (unverified)".into(),
            _ => String::new(),
        };
        out.push_str(&format!("\n{}  {who}{state}\n", when(message.timestamp)));
        if !message.title.trim().is_empty() {
            out.push_str(&format!("{}\n", message.title.trim()));
        }
        if !message.content.trim().is_empty() {
            out.push_str(&format!("{}\n", message.content.trim_end()));
        }
        for attachment in &message.attachments {
            out.push_str(&format!("  [file: {}]\n", attachment.name));
        }
        if let Some(location) = message.location {
            out.push_str(&format!("  [location: {} {}]\n", location.label(), location.map_url()));
        }
        for note in &message.notes {
            out.push_str(&format!("  [{note}]\n"));
        }
        if !message.reactions.is_empty() {
            let reactions: Vec<String> = message
                .reactions
                .iter()
                .map(|r| format!("{} ({})", r.emoji, if r.incoming { name } else { "You" }))
                .collect();
            out.push_str(&format!("  [reactions: {}]\n", reactions.join(", ")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::store::{Conversation, Message, MessageState, Store, StoredAttachment};

    fn press(app: &mut crate::app::App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn a_message_and_a_copy_of_its_file_go_on_to_someone_else() {
        let dir = std::env::temp_dir().join(format!("rettui-forward-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (alice, bob) = ("ab".repeat(16), "cd".repeat(16));
        let mut store = Store::default();
        std::fs::create_dir_all(dir.join("downloads")).unwrap();
        let photo = dir.join("downloads").join("map.png");
        std::fs::write(&photo, b"png").unwrap();
        let message = Message {
            id: "m1".into(),
            incoming: true,
            content: "The repeater is up".into(),
            timestamp: 1.0,
            state: MessageState::Received { verified: true },
            attachments: vec![StoredAttachment { name: "map.png".into(), path: photo.clone(), size: 3, image: true, voice: None }],
            ..Message::default()
        };
        store.conversations.insert(alice.clone(), Conversation { messages: vec![message], ..Conversation::default() });
        store.conversations.insert(bob.clone(), Conversation::default());
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), store);
        app.tab = crate::app::Tab::Messages;
        app.active_conversation = Some(alice.clone());
        // Pick it, `f`, type part of Bob's address, Enter.
        app.picked = Some("m1".into());
        press(&mut app, KeyCode::Char('f'));
        assert!(app.forward.is_some());
        assert!(!app.forward_targets().contains(&"ff".repeat(16)));
        for c in "cdcd".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.forward_targets(), std::slice::from_ref(&bob));
        press(&mut app, KeyCode::Enter);
        assert!(app.forward.is_none());
        let sent = app.store.conversations[&bob].messages.last().unwrap().clone();
        assert_eq!(sent.content, "The repeater is up");
        assert!(!sent.incoming);
        let copy = &sent.attachments[0].path;
        assert!(copy.starts_with(&app.paths.uploads) && copy != &photo && copy.exists());
        // Deleting the forwarded one leaves the first one's file.
        let id = sent.id.clone();
        app.delete_message(&bob, &id).unwrap();
        assert!(photo.exists() && !copy.exists());
        // An address typed in full can be forwarded to as well.
        app.picked = Some("m1".into());
        press(&mut app, KeyCode::Char('f'));
        for c in "ef".repeat(16).chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.forward_targets()[0], "ef".repeat(16));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conversation_exports_as_text_with_its_archive() {
        let dir = std::env::temp_dir().join(format!("rettui-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let alice = "ab".repeat(16);
        let mut store = Store::default();
        let kept = Message { id: "new".into(), content: "Kept here".into(), timestamp: 1_790_000_100.0, state: MessageState::Delivered, ..Message::default() };
        store.conversations.insert(alice.clone(), Conversation { messages: vec![kept], archived: 1, ..Conversation::default() });
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), store);
        let old = crate::store::Archived {
            conversation: alice.clone(),
            message: Message { id: "old".into(), incoming: true, content: "From the archive".into(), timestamp: 1_790_000_000.0, ..Message::default() },
        };
        std::fs::create_dir_all(&app.paths.archive).unwrap();
        std::fs::write(crate::store::archive_month(&app.paths.archive, 1_790_000_000.0), crate::store::encode_archive(&[old]).unwrap()).unwrap();
        let path = app.export_conversation(&alice).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("Conversation with <ababababab"), "{text}");
        let (archived, kept) = (text.find("From the archive").unwrap(), text.find("Kept here").unwrap());
        assert!(archived < kept, "{text}");
        assert!(text.contains("  You\nKept here"), "{text}");
        assert!(path.starts_with(app.paths.downloads.join("exports")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
