//! Reaching another user from a hub: a private notice through the hub
//! (`/msg`, a "whisper"), or an LXMF message to the delivery address of
//! their identity, which RRC already tells us.
//!
//! The lookups are shared by the TUI and the web UI; the user menu and the
//! member picker are the TUI's.

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::widgets::ListState;
use rns_identity::destination::Destination;

use super::{ChatLine, LineKind, whisper_key};
use crate::app::App;
use crate::lxmf::{DeliveryMode, LXMF_ASPECT};
use crate::net::{Hash, PeerKind};
use crate::rrc::t;

/// What rettui knows about a user seen on a hub.
#[derive(Debug, Clone)]
pub struct UserInfo {
    pub identity: Vec<u8>,
    pub name: String,
    /// Their LXMF address (from their identity).
    pub lxmf: Hash,
    /// They have announced that address (so a message can find them), or
    /// there is already a conversation with them.
    pub lxmf_known: bool,
    /// The hub passes private notices.
    pub whisper: bool,
}

/// Something to do for a user, from the user menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAction {
    Mention,
    Whisper,
    Lxmf,
    CopyLxmf,
    CopyIdentity,
}

impl UserAction {
    pub const ALL: [UserAction; 5] =
        [UserAction::Mention, UserAction::Whisper, UserAction::Lxmf, UserAction::CopyLxmf, UserAction::CopyIdentity];
}

/// The TUI's popup for a user.
pub struct UserMenu {
    pub user: UserInfo,
    pub list: ListState,
}

/// The TUI's popup listing a room's members, to pick one.
pub struct MemberPicker {
    pub room: String,
    /// (name, identity)
    pub members: Vec<(String, Vec<u8>)>,
    pub list: ListState,
}

/// The LXMF delivery address of an identity.
pub fn lxmf_address(identity: &[u8]) -> Option<Hash> {
    let identity: Hash = identity.try_into().ok()?;
    Some(Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&identity)))
}

impl App {
    pub fn rrc_user(&self, index: usize, identity: &[u8]) -> Option<UserInfo> {
        let hub = self.channels.hubs.get(index)?;
        let lxmf = lxmf_address(identity)?;
        let key = hex::encode(lxmf);
        let announced = self.store.peers.get(&key).is_some_and(|p| p.kind == PeerKind::Lxmf);
        Some(UserInfo {
            identity: identity.to_vec(),
            name: hub.display_name(identity),
            lxmf,
            lxmf_known: announced || self.store.conversations.contains_key(&key),
            whisper: hub.direct_notices,
        })
    }

    /// A user of a hub by nick (any case, `@` optional) or identity prefix.
    pub(super) fn find_rrc_user(&self, index: usize, who: &str) -> Result<Vec<u8>, &'static str> {
        let hub = &self.channels.hubs[index];
        let who = who.trim_start_matches('@').to_lowercase();
        let known: BTreeSet<&Vec<u8>> = hub.members.values().flatten().chain(hub.nicks.keys()).collect();
        let matches: Vec<&Vec<u8>> = known
            .into_iter()
            .filter(|h| hub.nicks.get(*h).is_some_and(|n| n.to_lowercase() == who) || (who.len() >= 6 && hex::encode(h).starts_with(&who)))
            .collect();
        match matches.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err("No such user here (try /who)"),
            _ => Err("Several users match; use a longer name or hash"),
        }
    }

    /// Send a private notice (a whisper) to a user through the hub. It goes
    /// in the whisper conversation with them.
    pub fn rrc_whisper(&mut self, index: usize, target: &[u8], text: &str) -> Result<(), String> {
        let hub = self.channels.hubs.get(index).ok_or("No such hub")?;
        if !hub.direct_notices {
            return Err("This hub does not support private notices".into());
        }
        let text = text.trim();
        if text.is_empty() {
            return Err("Nothing to send".into());
        }
        let env = self.envelope(index, t::NOTICE).text(text).dst(target);
        self.send_env(index, &env);
        let mut line = ChatLine::new(LineKind::Private, text);
        line.nick = self.effective_nick(index);
        line.own = true;
        self.record(index, &whisper_key(target), line);
        Ok(())
    }

    /// Open (creating if needed) the whisper conversation with a user and
    /// show it. Returns its buffer key.
    pub fn open_whisper(&mut self, index: usize, target: &[u8]) -> String {
        let key = whisper_key(target);
        let hub = self.hub_mut(index);
        hub.buffers.entry(key.clone()).or_default();
        let hash = hub.hash;
        self.channels.selected = Some(super::Target { hub: hash, room: Some(key.clone()) });
        self.channels.scroll = 0;
        self.mark_channel_read();
        key
    }

    /// Send an LXMF message to a hub user's address, noting it in the room.
    pub fn rrc_lxmf(&mut self, index: usize, room: &str, target: &[u8], text: &str) -> Result<String, String> {
        let user = self.rrc_user(index, target).ok_or("Not a user identity")?;
        let key = hex::encode(user.lxmf);
        self.send_message(key.clone(), text.to_string(), Vec::new(), DeliveryMode::Auto, None).map_err(|(e, ..)| e)?;
        let note = format!("LXMF message sent to {} ({key}); replies arrive in Messages", user.name);
        self.record(index, room, ChatLine::new(LineKind::System, note));
        Ok(key)
    }

    /// `/dm <nick> [text]`: message a user over LXMF, or open the
    /// conversation with them when there is no text.
    pub(super) fn dm_command(&mut self, index: usize, room: &str, arg: &str) {
        let (who, text) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
        if who.is_empty() {
            return self.record(index, room, ChatLine::new(LineKind::Error, "Usage: /dm <nick> [text]"));
        }
        let target = match self.find_rrc_user(index, who) {
            Ok(target) => target,
            Err(e) => return self.record(index, room, ChatLine::new(LineKind::Error, e)),
        };
        if text.trim().is_empty() {
            if let Some(user) = self.rrc_user(index, &target) {
                self.open_conversation(hex::encode(user.lxmf));
            }
        } else if let Err(e) = self.rrc_lxmf(index, room, &target, text) {
            self.record(index, room, ChatLine::new(LineKind::Error, e));
        }
    }

    // ---- TUI ---------------------------------------------------------------

    pub(crate) fn open_user_menu(&mut self, target: &[u8]) {
        let Some((hub, _)) = self.channels.active() else { return };
        if target == self.identity_hash {
            return;
        }
        if let Some(user) = self.rrc_user(hub, target) {
            self.channels.picker = None;
            self.channels.menu = Some(UserMenu { user, list: ListState::default().with_selected(Some(0)) });
        }
    }

    /// `m`: pick one of the room's members.
    pub(crate) fn open_member_picker(&mut self) {
        let Some((hub, room)) = self.channels.active() else { return };
        let own = self.identity_hash.to_vec();
        let members: Vec<(String, Vec<u8>)> = self.channels.hubs[hub].members_of(&room).into_iter().filter(|(_, id)| *id != own).collect();
        if members.is_empty() {
            return self.warn("Nobody else is here (join a room, or try /who)");
        }
        self.channels.picker = Some(MemberPicker { room, members, list: ListState::default().with_selected(Some(0)) });
    }

    pub(crate) fn member_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = &mut self.channels.picker else { return };
        let (len, i) = (picker.members.len(), picker.list.selected().unwrap_or(0));
        match key.code {
            KeyCode::Esc => self.channels.picker = None,
            KeyCode::Down | KeyCode::Char('j') => picker.list.select(Some((i + 1).min(len - 1))),
            KeyCode::Up | KeyCode::Char('k') => picker.list.select(Some(i.saturating_sub(1))),
            KeyCode::Enter => {
                let target = picker.members[i].1.clone();
                self.open_user_menu(&target);
            }
            _ => {}
        }
    }

    pub(crate) fn user_menu_key(&mut self, key: KeyEvent) {
        let Some(menu) = &mut self.channels.menu else { return };
        let i = menu.list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc => self.channels.menu = None,
            KeyCode::Down | KeyCode::Char('j') => menu.list.select(Some((i + 1).min(UserAction::ALL.len() - 1))),
            KeyCode::Up | KeyCode::Char('k') => menu.list.select(Some(i.saturating_sub(1))),
            KeyCode::Char('@') => self.user_action(UserAction::Mention),
            KeyCode::Char('w') => self.user_action(UserAction::Whisper),
            KeyCode::Char('l') => self.user_action(UserAction::Lxmf),
            KeyCode::Enter => self.user_action(UserAction::ALL[i]),
            _ => {}
        }
    }

    pub(crate) fn click_user_menu(&mut self, at: Position) {
        let area = self.regions.channel_popup;
        if !area.contains(at) {
            self.channels.menu = None;
            self.channels.picker = None;
            return;
        }
        let row = (at.y - area.y) as usize;
        if let Some(picker) = &mut self.channels.picker {
            let index = picker.list.offset() + row;
            if index < picker.members.len() {
                let target = picker.members[index].1.clone();
                self.open_user_menu(&target);
            }
        } else if let Some(action) = UserAction::ALL.get(row) {
            self.user_action(*action);
        }
    }

    fn user_action(&mut self, action: UserAction) {
        let Some(menu) = self.channels.menu.take() else { return };
        let user = menu.user;
        match action {
            UserAction::Mention if !self.in_room() => {
                self.warn("Mentions are for rooms");
                self.channels.menu = Some(UserMenu { user, list: menu.list });
            }
            UserAction::Mention => self.insert_mention(&user.name),
            UserAction::Whisper if !user.whisper => {
                self.warn("This hub does not pass private notices");
                self.channels.menu = Some(UserMenu { user, list: menu.list });
            }
            UserAction::Whisper => {
                // Their whisper conversation, ready to write in.
                if let Some((index, _)) = self.channels.active() {
                    self.open_whisper(index, &user.identity);
                    self.channels.typing = true;
                }
            }
            UserAction::Lxmf => {
                if !user.lxmf_known {
                    self.log(format!("No LXMF announce seen from {} yet; the message will wait until a path to them is found", user.name));
                }
                self.open_conversation(hex::encode(user.lxmf));
            }
            UserAction::CopyLxmf => self.copy(&hex::encode(user.lxmf), &format!("{}'s LXMF address", user.name)),
            UserAction::CopyIdentity => self.copy(&hex::encode(&user.identity), &format!("{}'s identity hash", user.name)),
        }
    }
}

/// Most names the `@` list shows at once.
pub const MENTION_ROWS: usize = 8;

/// The `@` being typed (its byte in the input) and who it could be, as
/// (name, identity).
pub type MentionMatches = (usize, Vec<(String, Vec<u8>)>);

impl App {
    /// While typing in a room: the `@name` being typed (the byte of its `@`
    /// in the input) and who it could be, best matches first. `None` when
    /// no name is being typed, nobody matches, or Esc closed the list for
    /// this `@`.
    pub fn mention_matches(&self) -> Option<MentionMatches> {
        // The emoji picker or `:name` list is in the way.
        if !self.channels.typing || self.emoji.is_some() || self.shortcode_matches().is_some() {
            return None;
        }
        let (index, room) = self.channels.active()?;
        if room.is_empty() {
            return None;
        }
        let (at, partial) = crate::rrc::mention_prefix(self.channels.input.before_cursor())?;
        if self.channels.mention_dismissed == Some(at) {
            return None;
        }
        let own = self.identity_hash.to_vec();
        let people: Vec<(String, Vec<u8>)> =
            self.channels.hubs[index].mentionable(&room).into_iter().filter(|(_, id)| *id != own).collect();
        let matches: Vec<(String, Vec<u8>)> =
            crate::rrc::complete_names(&people, partial).into_iter().take(MENTION_ROWS).cloned().collect();
        (!matches.is_empty()).then_some((at, matches))
    }

    /// Keys for the `@` list while it is open (Up and Down choose, Tab or
    /// Enter picks, Esc closes it); true when the key was one of those.
    pub(crate) fn mention_key(&mut self, key: KeyEvent) -> bool {
        let Some((at, matches)) = self.mention_matches() else { return false };
        let count = matches.len();
        let pick = self.channels.mention_pick.min(count - 1);
        match key.code {
            KeyCode::Down => self.channels.mention_pick = (pick + 1) % count,
            KeyCode::Up => self.channels.mention_pick = (pick + count - 1) % count,
            KeyCode::Tab | KeyCode::Enter => self.pick_mention(at, &matches[pick].0),
            KeyCode::Esc => self.channels.mention_dismissed = Some(at),
            _ => return false,
        }
        true
    }

    /// After typing: a new `@` gets its list back.
    pub(crate) fn mention_typed(&mut self) {
        self.channels.mention_pick = 0;
        let at = crate::rrc::mention_prefix(self.channels.input.before_cursor()).map(|(at, _)| at);
        if at != self.channels.mention_dismissed {
            self.channels.mention_dismissed = None;
        }
    }

    /// Whether a room is open (not the hub's page or a whisper conversation).
    pub fn in_room(&self) -> bool {
        self.channels.active().is_some_and(|(_, room)| !room.is_empty() && super::whisper_peer(&room).is_none())
    }

    /// Add `@name ` to what is being written, at the cursor, and carry on
    /// writing.
    pub fn insert_mention(&mut self, name: &str) {
        let input = &mut self.channels.input;
        let before = input.before_cursor();
        let space = if before.is_empty() || before.ends_with(char::is_whitespace) { "" } else { " " };
        let after_has_space = input.text()[before.len()..].starts_with(' ');
        input.insert_str(&format!("{space}@{name}{}", if after_has_space { "" } else { " " }));
        self.channels.mention_dismissed = None;
        self.channels.typing = true;
    }

    /// Put `@name ` in place of what was typed after the `@` at byte `at`.
    pub fn pick_mention(&mut self, at: usize, name: &str) {
        let input = &mut self.channels.input;
        let cursor = input.before_cursor().len();
        // One space after the name, even if one follows already.
        let end = if input.text()[cursor..].starts_with(' ') { cursor + 1 } else { cursor };
        input.replace(at, end, &format!("@{name} "));
        self.channels.mention_pick = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_start_with_an_emoji_can_be_mentioned() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let dir = std::env::temp_dir().join(format!("rettui-emoji-mention-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        let hub = [0xcd; 16];
        app.add_hub(hub, "rrc.hub", None);
        let (alex, bob) = (vec![0xa1; 16], vec![0xb2; 16]);
        let room = app.channels.hubs[0].members.entry("general".into()).or_default();
        room.extend([alex.clone(), bob.clone()]);
        app.channels.hubs[0].nicks.extend([(alex, "🆎 Alex".to_string()), (bob, "Bob".to_string())]);
        app.channels.hubs[0].rooms.insert("general".into());
        app.channels.selected = Some(crate::app::channels::Target { hub, room: Some("general".into()) });
        app.tab = crate::app::Tab::Channels;
        app.channels.typing = true;
        app.on_key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE));
        let listed = |app: &App| app.mention_matches().map(|(_, people)| people.into_iter().map(|(name, _)| name).collect::<Vec<_>>());
        assert_eq!(listed(&app), Some(vec!["Bob".to_string(), "🆎 Alex".to_string()]));
        // An emoji keyboard's 🆎 (with its variation selector) still finds it.
        app.on_paste("🆎\u{fe0f}");
        assert_eq!(listed(&app), Some(vec!["🆎 Alex".to_string()]));
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.channels.input.text(), "@🆎 Alex ");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lxmf_addresses_come_from_identities() {
        let identity = [7u8; 16];
        let address = lxmf_address(&identity).unwrap();
        assert_eq!(address, Destination::hash_from_name_and_identity("lxmf.delivery", Some(&identity)));
        assert_ne!(address, identity);
        assert!(lxmf_address(&[1, 2, 3]).is_none());
    }
}
