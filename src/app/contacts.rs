//! Contacts: your own name for someone, notes about them, and the card in
//! the Messages tab (`c`) that shows them with what can be done.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::{App, PromptKind};
use crate::store::{MAX_ALIAS, MAX_NOTES};

/// What a contact card's buttons do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    Rename,
    Notes,
    Copy,
    DeleteConversation,
    Close,
}

impl CardAction {
    /// The button's label, and its key.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            CardAction::Rename => ("Rename", "r"),
            CardAction::Notes => ("Notes", "e"),
            CardAction::Copy => ("Copy address", "y"),
            CardAction::DeleteConversation => ("Delete conversation", "X"),
            CardAction::Close => ("Close", "Esc"),
        }
    }
}

impl App {
    /// Your own name for someone (empty: the one they announce).
    pub fn set_contact_name(&mut self, key: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.chars().count() > MAX_ALIAS {
            return Err(format!("A name is at most {MAX_ALIAS} characters"));
        }
        let name: String = name.chars().filter(|c| !c.is_control()).collect();
        self.store.update_contact(key, |c| c.alias = (!name.is_empty()).then_some(name));
        self.store_dirty = true;
        Ok(())
    }

    /// Notes about someone (empty: none).
    pub fn set_contact_notes(&mut self, key: &str, notes: &str) -> Result<(), String> {
        let notes = notes.trim();
        if notes.chars().count() > MAX_NOTES {
            return Err(format!("Notes are at most {MAX_NOTES} characters"));
        }
        self.store.update_contact(key, |c| c.notes = notes.to_string());
        self.store_dirty = true;
        Ok(())
    }

    /// Do what a contact card's button does.
    pub(super) fn card_action(&mut self, action: CardAction) {
        let Some(key) = self.contact_card.clone() else { return };
        match action {
            CardAction::Rename => {
                let current = self.store.contact(&key).alias.unwrap_or_default();
                let theirs = self.store.announced_name(&key).map(|n| format!(" (they announce {n})")).unwrap_or_default();
                let title = format!("Your name for them{theirs}; empty uses theirs");
                self.open_prompt(PromptKind::ContactName(key), &title, &current);
            }
            CardAction::Notes => {
                // One line to edit: line breaks show as ↵.
                let notes = self.store.contact(&key).notes.replace('\n', " ↵ ");
                let title = format!("Notes about {}", self.store.display_name(&key));
                self.open_prompt(PromptKind::ContactNotes(key), &title, &notes);
            }
            CardAction::Copy => self.copy(&key, "LXMF address"),
            CardAction::DeleteConversation => self.ask_delete_conversation(key),
            CardAction::Close => self.contact_card = None,
        }
    }

    /// Keys while a contact card is open.
    pub(super) fn contact_card_key(&mut self, key: KeyEvent) {
        let action = match key.code {
            KeyCode::Char('r') => CardAction::Rename,
            KeyCode::Char('e') => CardAction::Notes,
            KeyCode::Char('y') => CardAction::Copy,
            KeyCode::Char('X') => CardAction::DeleteConversation,
            KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('q') => CardAction::Close,
            _ => return,
        };
        self.card_action(action);
    }

    /// A click while a contact card is open: on a button, or outside (which
    /// closes it).
    pub(super) fn click_contact_card(&mut self, at: Position) {
        if let Some(&(_, action)) = self.regions.card_buttons.iter().find(|(rect, _)| rect.contains(at)) {
            self.card_action(action);
        } else if !self.regions.card.contains(at) {
            self.contact_card = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;
    use crate::config::Settings;
    use crate::store::{Conversation, Message, MessageState, Store};

    #[test]
    fn names_and_notes_from_the_card() {
        let dir = std::env::temp_dir().join(format!("rettui-contacts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = "ab".repeat(16);
        let mut store = Store::default();
        let message = Message { id: "x".into(), incoming: true, content: "hi".into(), state: MessageState::Received { verified: true }, ..Message::default() };
        store.conversations.insert(key.clone(), Conversation { messages: vec![message], ..Default::default() });
        let mut app = crate::app::test_app(&dir, Settings::default(), store);
        app.open_newest_conversation();
        let press = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        let typed = |app: &mut App, text: &str| text.chars().for_each(|c| app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.contact_card.as_deref(), Some(key.as_str()));
        press(&mut app, KeyCode::Char('r'));
        typed(&mut app, "Ally");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.store.display_name(&key), "Ally");
        assert!(app.contact_card.is_some(), "the card stays open");
        // Notes keep their lines through the one-line editor.
        app.set_contact_notes(&key, "line one\nline two").unwrap();
        press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.prompt.as_ref().unwrap().input.text(), "line one ↵ line two");
        typed(&mut app, " ↵ three");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.store.contact(&key).notes, "line one\nline two\nthree");
        assert!(app.set_contact_name(&key, &"x".repeat(MAX_ALIAS + 1)).is_err());
        // An empty name goes back to theirs.
        app.set_contact_name(&key, "  ").unwrap();
        assert_eq!(app.store.display_name(&key), format!("<{}>", &key[..12]));
        press(&mut app, KeyCode::Esc);
        assert!(app.contact_card.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
