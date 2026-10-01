//! Contacts: your own name for someone, notes about them, and the card in
//! the Messages tab (`c`) that shows them with what can be done.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::{App, PromptKind};
use crate::net::{NetCommand, parse_hash};
use crate::store::{MAX_ALIAS, MAX_NOTES, Trust};

/// What a contact card's buttons do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    Rename,
    Notes,
    Copy,
    Trust,
    Untrust,
    /// Leave an unknown sender as they are (no more asking).
    LeaveAsIs,
    Block,
    Unblock,
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
            CardAction::Trust => ("Trust", "t"),
            CardAction::Untrust => ("Stop trusting", "t"),
            CardAction::LeaveAsIs => ("Leave as is", "l"),
            CardAction::Block => ("Block", "b"),
            CardAction::Unblock => ("Unblock", "b"),
            CardAction::DeleteConversation => ("Delete conversation", "X"),
            CardAction::Close => ("Close", "Esc"),
        }
    }
}

/// What a contact's trust says, for the card and the web UI.
pub fn trust_label(trust: Trust, known: bool) -> &'static str {
    match trust {
        Trust::Trusted => "trusted: no stamp asked, given tickets",
        Trust::Blocked => "blocked",
        Trust::Untrusted => "not trusted (left as is)",
        Trust::Unknown if known => "a contact (you've written to them)",
        Trust::Unknown => "unknown sender",
    }
}

impl App {
    /// Start a conversation with someone typed or scanned: an address, or
    /// an `lxma://` link, whose public key is remembered so you can write
    /// before hearing them announce. Their conversation's key.
    pub fn add_contact(&mut self, text: &str) -> Result<String, String> {
        let (address, key) = crate::lxmf::parse_contact(text)?;
        if self.lxmf_hash == Some(address) {
            return Err("That's your own address".into());
        }
        if let Some(public_key) = key {
            self.send(NetCommand::Remember { to: address, public_key });
        }
        let key = hex::encode(address);
        self.store.conversations.entry(key.clone()).or_default();
        self.store_dirty = true;
        Ok(key)
    }

    /// Your address with your public key, for others to add you
    /// (`lxma://`, as Columba shares contacts).
    pub fn identity_link(&self) -> Option<String> {
        Some(crate::lxmf::identity_link(self.lxmf_hash?, self.public_key.as_ref()?))
    }

    /// Show your address as a QR code (`c` in the Status tab).
    pub(super) fn show_address_qr(&mut self) {
        match self.identity_link() {
            Some(link) => self.paper_view = Some(super::PaperView::address(link)),
            None => self.warn("Reticulum is still starting"),
        }
    }

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

    /// Whether someone is a contact: trusted, left as is, or written to.
    /// Anyone else is an unknown sender.
    pub fn is_known(&self, key: &str) -> bool {
        matches!(self.store.contact(key).trust, Trust::Trusted | Trust::Untrusted)
            || self.store.conversations.get(key).is_some_and(|c| c.messages.iter().any(|m| !m.incoming))
    }

    /// The card's buttons for someone, by how far they're trusted.
    pub fn card_actions(&self, key: &str) -> Vec<CardAction> {
        let trust = self.store.contact(key).trust;
        let mut actions = vec![CardAction::Rename, CardAction::Notes, CardAction::Copy];
        match trust {
            Trust::Blocked => actions.push(CardAction::Unblock),
            Trust::Trusted => actions.extend([CardAction::Untrust, CardAction::Block]),
            Trust::Unknown if !self.is_known(key) => actions.extend([CardAction::Trust, CardAction::LeaveAsIs, CardAction::Block]),
            _ => actions.extend([CardAction::Trust, CardAction::Block]),
        }
        if self.store.conversations.contains_key(key) {
            actions.push(CardAction::DeleteConversation);
        }
        actions.push(CardAction::Close);
        actions
    }

    /// Trust someone, stop trusting them, or leave them as they are (not
    /// blocking: see [`App::block_contact`]).
    pub fn set_trust(&mut self, key: &str, trust: Trust) -> Result<(), String> {
        parse_hash(key).ok_or("An LXMF address is 32 hex characters")?;
        match (self.store.contact(key).trust, trust) {
            (_, Trust::Blocked) => return self.block_contact(key),
            (Trust::Blocked, _) => self.unblock_contact(key)?,
            _ => {}
        }
        self.store.update_contact(key, |c| c.trust = trust);
        self.store_dirty = true;
        self.update_policy(false);
        Ok(())
    }

    /// Block someone: their messages are dropped, the conversation with them
    /// is deleted, and their identity is blocked in Reticulum (as NomadNet
    /// does).
    pub fn block_contact(&mut self, key: &str) -> Result<(), String> {
        let to = parse_hash(key).ok_or("An LXMF address is 32 hex characters")?;
        self.store.update_contact(key, |c| c.trust = Trust::Blocked);
        self.delete_conversation(key);
        self.send(NetCommand::Blackhole { to, block: true, quiet: false });
        self.store_dirty = true;
        self.update_policy(false);
        Ok(())
    }

    /// Unblock someone: their messages are taken again (as from an unknown
    /// sender), and Reticulum lets their identity through.
    pub fn unblock_contact(&mut self, key: &str) -> Result<(), String> {
        let to = parse_hash(key).ok_or("An LXMF address is 32 hex characters")?;
        if self.store.contact(key).trust != Trust::Blocked {
            return Err("They aren't blocked".into());
        }
        self.store.update_contact(key, |c| c.trust = Trust::Unknown);
        self.send(NetCommand::Blackhole { to, block: false, quiet: false });
        self.store_dirty = true;
        Ok(())
    }

    /// Tell the network actor who's spared the stamp (contacts) and who is
    /// given tickets (trusted contacts), if that changed (or `always`).
    pub(super) fn update_policy(&mut self, always: bool) {
        let hashes = |keys: Vec<&String>| keys.into_iter().filter_map(|k| parse_hash(k)).collect::<Vec<_>>();
        let trusted = hashes(self.store.contacts.iter().filter(|(_, c)| c.trust == Trust::Trusted).map(|(k, _)| k).collect());
        let written = self.store.conversations.iter().filter(|(_, c)| c.messages.iter().any(|m| !m.incoming)).map(|(k, _)| k);
        let mut exempt = hashes(written.collect());
        exempt.sort_unstable();
        let contacts = (trusted, exempt);
        if always || self.policy_sent.as_ref() != Some(&contacts) {
            self.send(NetCommand::SetContacts { trusted: contacts.0.clone(), exempt: contacts.1.clone() });
            self.policy_sent = Some(contacts);
        }
    }

    /// Block again those blocked (after a restart, quietly): the transport
    /// keeps its own list, but one that couldn't be blocked then (their
    /// identity wasn't known) may be now.
    pub(super) fn reapply_blocks(&mut self) {
        let blocked: Vec<_> =
            self.store.contacts.iter().filter(|(_, c)| c.trust == Trust::Blocked).filter_map(|(k, _)| parse_hash(k)).collect();
        for to in blocked {
            self.send(NetCommand::Blackhole { to, block: true, quiet: true });
        }
    }

    /// Ask before blocking someone.
    pub(super) fn ask_block(&mut self, key: String) {
        let name = self.store.display_name(&key);
        let title = format!(
            "Block {name}? Their messages are dropped, the conversation is deleted, and their identity is blocked in Reticulum. Type y"
        );
        self.open_prompt(PromptKind::ConfirmBlock(key), &title, "");
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
            CardAction::Trust | CardAction::Untrust | CardAction::LeaveAsIs => {
                let trust = match action {
                    CardAction::Trust => Trust::Trusted,
                    CardAction::Untrust => Trust::Unknown,
                    _ => Trust::Untrusted,
                };
                match self.set_trust(&key, trust) {
                    Ok(()) => {
                        let name = self.store.display_name(&key);
                        self.confirm(match trust {
                            Trust::Trusted => format!("Trusting {name}"),
                            Trust::Untrusted => format!("Leaving {name} as is"),
                            _ => format!("Not trusting {name}"),
                        });
                    }
                    Err(e) => self.warn(e),
                }
            }
            CardAction::Block => self.ask_block(key),
            CardAction::Unblock => match self.unblock_contact(&key) {
                Ok(()) => self.notify(format!("Unblocked {}", self.store.display_name(&key))),
                Err(e) => self.warn(e),
            },
            CardAction::DeleteConversation => self.ask_delete_conversation(key),
            CardAction::Close => self.contact_card = None,
        }
    }

    /// Keys while a contact card is open: its buttons' keys.
    pub(super) fn contact_card_key(&mut self, key: KeyEvent) {
        let Some(card) = self.contact_card.clone() else { return };
        let pressed = match key.code {
            KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('q') => "Esc".to_string(),
            KeyCode::Char(c) => c.to_string(),
            _ => return,
        };
        if let Some(action) = self.card_actions(&card).into_iter().find(|a| a.label().1 == pressed) {
            self.card_action(action);
        }
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
    fn contacts_from_links_and_your_own_as_a_qr_code() {
        let dir = std::env::temp_dir().join(format!("rettui-contact-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut commands) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        let press = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        // Theirs: the key is remembered, and the conversation opens.
        let them = rns_runtime::prelude::Identity::new();
        let address = rns_identity::destination::Destination::hash_from_name_and_identity(crate::lxmf::LXMF_ASPECT, Some(&them.hash));
        let link = crate::lxmf::identity_link(address, &them.get_public_key());
        press(&mut app, KeyCode::Char('n'));
        app.on_paste(&link);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.active_conversation, Some(hex::encode(address)));
        let remembered = std::iter::from_fn(|| commands.try_recv().ok())
            .any(|c| matches!(c, NetCommand::Remember { to, public_key } if to == address && public_key == them.get_public_key()));
        assert!(remembered);
        assert!(app.add_contact("lxma://00").is_err());
        // Yours, once Reticulum is up: `c` in the Status tab.
        app.composing = false;
        app.tab = crate::app::Tab::Status;
        press(&mut app, KeyCode::Char('c'));
        assert!(app.paper_view.is_none(), "not before Reticulum is up");
        let ours = rns_runtime::prelude::Identity::new();
        app.on_net(crate::net::NetEvent::Started { lxmf_hash: [5; 16], public_key: ours.get_public_key() });
        press(&mut app, KeyCode::Char('c'));
        let view = app.paper_view.as_ref().unwrap();
        assert_eq!((view.kind, view.link.as_str()), (crate::app::QrKind::Address, app.identity_link().unwrap().as_str()));
        assert!(view.link.starts_with(&format!("lxma://{}:", hex::encode([5; 16]))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

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

    #[test]
    fn blocking_drops_them_deletes_the_conversation_and_tells_reticulum() {
        use crate::lxmf::InboundMessage;
        let dir = std::env::temp_dir().join(format!("rettui-block-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        let (alice, bob) = ("ab".repeat(16), "cd".repeat(16));
        let from = |source: u8, n: u8| InboundMessage { id: Some([n; 32]), source: [source; 16], content: format!("hi {n}"), ..Default::default() };
        app.on_message(from(0xab, 1));
        assert!(!app.is_known(&alice), "never written to");
        assert_eq!(app.card_actions(&alice)[3..6], [CardAction::Trust, CardAction::LeaveAsIs, CardAction::Block]);
        while net.try_recv().is_ok() {}
        app.block_contact(&alice).unwrap();
        assert!(!app.store.conversations.contains_key(&alice));
        let commands: Vec<NetCommand> = std::iter::from_fn(|| net.try_recv().ok()).collect();
        assert!(commands.iter().any(|c| matches!(c, NetCommand::Blackhole { to, block: true, .. } if *to == [0xab; 16])), "{commands:?}");
        app.on_message(from(0xab, 2));
        assert!(!app.store.conversations.contains_key(&alice), "their messages are dropped");
        assert!(app.log.iter().any(|l| l.contains("who is blocked")));
        // Unblocked, they're an unknown sender again.
        app.set_trust(&alice, Trust::Unknown).unwrap();
        assert!(std::iter::from_fn(|| net.try_recv().ok()).any(|c| matches!(c, NetCommand::Blackhole { block: false, .. })));
        app.on_message(from(0xab, 3));
        assert!(app.store.conversations.contains_key(&alice));
        // Ignoring unknown senders: Bob's goes, Alice once trusted gets in.
        app.update_settings(&[("ignore_unknown_senders", "true")]).unwrap();
        app.on_message(from(0xcd, 4));
        assert!(!app.store.conversations.contains_key(&bob));
        app.set_trust(&alice, Trust::Trusted).unwrap();
        app.on_message(from(0xab, 5));
        assert_eq!(app.store.conversations[&alice].messages.len(), 2);
        // Someone written to is a contact too.
        app.send_message(bob.clone(), "hello".into(), Vec::new(), crate::lxmf::DeliveryMode::Auto, None).unwrap();
        app.on_message(from(0xcd, 6));
        assert_eq!(app.store.conversations[&bob].messages.len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_network_is_told_who_is_spared_the_stamp() {
        let dir = std::env::temp_dir().join(format!("rettui-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        let contacts = |net: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>| {
            std::iter::from_fn(|| net.try_recv().ok())
                .filter_map(|c| match c {
                    NetCommand::SetContacts { trusted, exempt } => Some((trusted, exempt)),
                    _ => None,
                })
                .last()
        };
        app.set_trust(&"ab".repeat(16), Trust::Trusted).unwrap();
        assert_eq!(contacts(&mut net), Some((vec![[0xab; 16]], Vec::new())));
        app.send_message("cd".repeat(16), "hi".into(), Vec::new(), crate::lxmf::DeliveryMode::Auto, None).unwrap();
        assert_eq!(contacts(&mut net), Some((vec![[0xab; 16]], vec![[0xcd; 16]])));
        // Nothing changed: nothing said.
        app.send_message("cd".repeat(16), "again".into(), Vec::new(), crate::lxmf::DeliveryMode::Auto, None).unwrap();
        assert_eq!(contacts(&mut net), None);
        // The stamp cost and size limit go with the settings.
        app.update_settings(&[("stamp_cost", "12"), ("max_message_kb", "500")]).unwrap();
        let policy = std::iter::from_fn(|| net.try_recv().ok()).find_map(|c| match c {
            NetCommand::SetPolicy { stamp_cost, max_bytes } => Some((stamp_cost, max_bytes)),
            _ => None,
        });
        assert_eq!(policy, Some((Some(12), 500_000)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
