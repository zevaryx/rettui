//! Reading a conversation's archived messages in the terminal UI (`H` in
//! Messages): the older ones moved out of the store (see
//! `store::read_archive`), shown over the tab, only to read.

use crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::store::Message;

/// What's open: whose archive, its messages (oldest first), and how far up
/// it's scrolled, in rows from the newest.
pub struct ArchiveReader {
    pub key: String,
    pub messages: Vec<Message>,
    pub scroll: usize,
    /// Rows a page holds, as last drawn (for PgUp/PgDn).
    pub page: usize,
}

impl App {
    /// Open the archive of the open conversation (read now: it's only
    /// asked for now and then).
    pub(super) fn open_archive(&mut self) {
        let Some(key) = self.active_conversation.clone() else { return };
        match crate::store::read_archive(&self.paths.archive, &key) {
            Ok(messages) if messages.is_empty() => {
                self.confirm(format!("Nothing of {}'s is in the archive", self.store.display_name(&key)));
            }
            Ok(messages) => self.archive_reader = Some(ArchiveReader { key, messages, scroll: 0, page: 10 }),
            Err(e) => self.warn(format!("Couldn't read the archive: {e}")),
        }
    }

    pub(super) fn archive_key(&mut self, key: KeyEvent) {
        let Some(reader) = self.archive_reader.as_mut() else { return };
        let page = reader.page.max(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('H') => self.archive_reader = None,
            KeyCode::Up | KeyCode::Char('k') => reader.scroll += 1,
            KeyCode::Down | KeyCode::Char('j') => reader.scroll = reader.scroll.saturating_sub(1),
            KeyCode::PageUp => reader.scroll += page,
            KeyCode::PageDown => reader.scroll = reader.scroll.saturating_sub(page),
            // The oldest (as far up as it goes, once drawn), or the newest.
            KeyCode::Home => reader.scroll = usize::MAX,
            KeyCode::End => reader.scroll = 0,
            _ => {}
        }
    }

    /// The mouse wheel over it scrolls it.
    pub(super) fn scroll_archive(&mut self, up: bool) {
        if let Some(reader) = self.archive_reader.as_mut() {
            reader.scroll = if up { reader.scroll.saturating_add(3) } else { reader.scroll.saturating_sub(3) };
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::store::{Archived, Conversation, Message, Store, archive_month, encode_archive};

    #[test]
    fn h_opens_the_archive_and_esc_closes_it() {
        let dir = std::env::temp_dir().join(format!("rettui-archive-reader-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = "ab".repeat(16);
        let mut store = Store::default();
        store.conversations.insert(key.clone(), Conversation { archived: 2, ..Conversation::default() });
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), store);
        app.tab = crate::app::Tab::Messages;
        app.open_newest_conversation();
        let press = |app: &mut crate::app::App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        // Nothing there yet: says so.
        press(&mut app, KeyCode::Char('H'));
        assert!(app.archive_reader.is_none());
        let old = |id: &str, timestamp: f64| Archived {
            conversation: key.clone(),
            message: Message { id: id.into(), timestamp, content: format!("old {id}"), ..Message::default() },
        };
        std::fs::create_dir_all(&app.paths.archive).unwrap();
        let month = archive_month(&app.paths.archive, 1_700_000_000.0);
        std::fs::write(&month, encode_archive(&[old("b", 1_700_000_100.0), old("a", 1_700_000_000.0)]).unwrap()).unwrap();
        press(&mut app, KeyCode::Char('H'));
        let reader = app.archive_reader.as_ref().unwrap();
        assert_eq!(reader.messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        // Keys go to it: `q` closes it rather than quitting.
        press(&mut app, KeyCode::Up);
        assert_eq!(app.archive_reader.as_ref().unwrap().scroll, 1);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.archive_reader.is_none() && !app.should_quit);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
