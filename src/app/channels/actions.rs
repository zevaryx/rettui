//! Channels list actions: adding and removing hubs, opening `rrc://` links,
//! selection, and the tab's keys and clicks.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::{Hub, HubStatus, Row, Target, whisper_peer};
use crate::app::{App, Prompt, PromptKind, Tab};
use crate::net::Hash;
use crate::rrc;
use crate::store::NotifyLevel;
use crate::term::input::TextInput;

impl App {
    /// Add a hub (or select it if known), optionally joining a room.
    pub fn add_hub(&mut self, hash: Hash, aspect: &str, room: Option<String>) {
        let index = match self.channels.hubs.iter().position(|h| h.hash == hash && h.aspect == aspect) {
            Some(index) => index,
            None => {
                let name = self
                    .store
                    .peers
                    .get(&hex::encode(hash))
                    .and_then(|p| p.name.clone())
                    .unwrap_or_else(|| format!("<{}>", &hex::encode(hash)[..12]));
                self.channels.hubs.push(Hub::new(hash, aspect.to_string(), name));
                self.save_hubs();
                self.channels.hubs.len() - 1
            }
        };
        self.tab = Tab::Channels;
        self.channels.selected = Some(crate::app::channels::Target {
            hub: hash,
            room: room.clone(),
        });
        match room {
            Some(room) if !self.channels.hubs[index].rooms.contains(&room) => {
                self.join(index, &room, None, false);
            }
            _ => self.connect_hub(index),
        }
        self.mark_channel_read();
    }

    /// Open an `rrc://` link: known hubs open directly; new ones ask first,
    /// because connecting reveals your identity to the hub.
    pub fn open_rrc_link(&mut self, link: &str) {
        let Some((hash, aspect, room)) = rrc::parse_link(link) else {
            self.log(format!("Not an RRC link: {link}"));
            return;
        };
        if self.channels.hubs.iter().any(|h| h.hash == hash && h.aspect == aspect) {
            self.add_hub(hash, &aspect, room);
            return;
        }
        let room_text = room.as_ref().map(|r| format!(" #{r}")).unwrap_or_default();
        self.prompt = Some(Prompt {
            kind: PromptKind::ConfirmHub { hash, aspect, room },
            title: format!(
                "Open RRC hub {}{room_text}? It will learn your identity. Type y",
                &hex::encode(hash)[..12]
            ),
            input: TextInput::default(),
        });
    }

    pub fn remove_hub(&mut self, hash: Hash) {
        let Some(index) = self.channels.hub_index(hash) else { return };
        self.disconnect_hub(index);
        let hub = self.channels.hubs.remove(index);
        self.saver.remove(hub.history_path(&self.paths.rrc_history));
        self.channels.selected = self.channels.hubs.first().map(|h| Target { hub: h.hash, room: None });
        self.save_hubs();
        self.notify(format!("Removed hub {}", hub.name));
    }

    /// `x` in the list: leave and forget a room, or remove a hub (asks).
    pub fn remove_selected_channel(&mut self) {
        let Some((index, room)) = self.channels.active() else { return };
        if room.is_empty() {
            let hub = &self.channels.hubs[index];
            self.prompt = Some(Prompt {
                kind: PromptKind::ConfirmRemoveHub(hub.hash),
                title: format!("Remove hub {} and its history? Type y", hub.name),
                input: TextInput::default(),
            });
            return;
        }
        self.forget_room(index, &room);
        let hash = self.channels.hubs[index].hash;
        self.channels.selected = Some(Target { hub: hash, room: None });
    }

    /// Leave a room (if joined) and delete its messages.
    pub fn forget_room(&mut self, index: usize, room: &str) {
        if self.channels.hubs[index].rooms.contains(room) {
            self.part(index, room);
        }
        let hub = self.hub_mut(index);
        hub.buffers.remove(room);
        hub.unread.remove(room);
        hub.mentions.remove(room);
        hub.history_dirty = true;
        self.save_hubs();
    }

    pub fn toggle_selected_connection(&mut self) {
        if let Some((index, _)) = self.channels.active() {
            self.toggle_connection(index);
        }
    }

    pub fn toggle_connection(&mut self, index: usize) {
        if matches!(self.channels.hubs[index].status, HubStatus::Connected | HubStatus::Connecting(_)) {
            self.disconnect_hub(index);
        } else {
            self.connect_hub(index);
        }
    }

    pub fn toggle_auto_connect(&mut self) {
        if let Some((index, _)) = self.channels.active() {
            self.toggle_auto_connect_at(index);
        }
    }

    pub fn toggle_auto_connect_at(&mut self, index: usize) {
        let hub = self.hub_mut(index);
        hub.auto_connect = !hub.auto_connect;
        let text = if hub.auto_connect { "on" } else { "off" };
        let name = hub.name.clone();
        self.save_hubs();
        self.notify(format!("Auto-connect {text} for {name}"));
    }

    /// `rrc://` link to the selection, for sharing.
    pub fn channel_link(&self) -> Option<String> {
        let (index, room) = self.channels.active()?;
        let hub = &self.channels.hubs[index];
        let mut link = format!("rrc://{}", hex::encode(hub.hash));
        if hub.aspect != rrc::DEFAULT_ASPECT {
            link.push(':');
            link.push_str(&hub.aspect);
        }
        if !room.is_empty() {
            link.push('/');
            link.push_str(&room);
        }
        Some(link)
    }

    pub fn move_channel_selection(&mut self, delta: isize) {
        let rows = self.channels.rows();
        if rows.is_empty() {
            return;
        }
        let current = self.channel_row_index(&rows).unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(rows.len() - 1);
        self.channels.select_row(&rows[next]);
        self.mark_channel_read();
    }

    pub fn select_channel_row(&mut self, index: usize) {
        let rows = self.channels.rows();
        if let Some(row) = rows.get(index) {
            self.channels.select_row(row);
            self.mark_channel_read();
        }
    }

    pub fn channel_row_index(&self, rows: &[Row]) -> Option<usize> {
        let (index, room) = self.channels.active()?;
        rows.iter().position(|row| match row {
            Row::Hub(i) => *i == index && room.is_empty(),
            Row::Room(i, r) | Row::Whisper(i, r) => *i == index && *r == room,
        })
    }

    /// Join a room from the hub's public room list.
    pub fn join_listed_room(&mut self, room: &str) {
        let Some((index, _)) = self.channels.active() else { return };
        self.join_room(index, room);
        self.channels.selected = Some(Target {
            hub: self.channels.hubs[index].hash,
            room: Some(rrc::normalize_room(room)),
        });
        self.mark_channel_read();
    }

    pub(crate) fn channels_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_channel_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_channel_selection(-1),
            KeyCode::Enter | KeyCode::Char('i') if self.channels.active().is_some() => {
                self.channels.typing = true;
            }
            KeyCode::Char('n') => self.open_prompt(
                PromptKind::AddHub,
                "Add RRC hub (address, or rrc://address/room)",
                "",
            ),
            KeyCode::Char('m') => self.open_member_picker(),
            KeyCode::Char('c') => self.toggle_selected_connection(),
            KeyCode::Char('a') => self.toggle_auto_connect(),
            KeyCode::Char('J') => self.toggle_show_joins(),
            KeyCode::Char('N') => self.cycle_notify_level(),
            KeyCode::Char('x') | KeyCode::Delete => self.remove_selected_channel(),
            KeyCode::Char('y') => {
                if let Some(link) = self.channel_link() {
                    self.copy(&link, "link");
                }
            }
            KeyCode::PageUp => self.channels.scroll = self.channels.scroll.saturating_add(self.channel_page()),
            KeyCode::PageDown => self.channels.scroll = self.channels.scroll.saturating_sub(self.channel_page()),
            KeyCode::Home => self.channels.scroll = crate::app::SCROLL_TOP,
            KeyCode::End => self.channels.scroll = 0,
            _ => {}
        }
    }

    /// `J` (or the web UI's room toggle): show or hide people joining and
    /// leaving rooms. It's a setting ("Show joins and leaves").
    pub fn toggle_show_joins(&mut self) {
        let show = !self.settings.show_joins;
        match self.update_settings(&[("show_joins", if show { "true" } else { "false" })]) {
            Ok(_) => self.notify(if show { "Showing people joining and leaving" } else { "Hiding people joining and leaving" }),
            Err(e) => self.fail(e),
        }
    }

    /// A hub's notifications (`room` empty), or a room's or whisper
    /// conversation's. `None` puts it back: mentions and whispers for a
    /// hub, the hub's level for a room.
    pub fn set_notify_level(&mut self, index: usize, room: &str, level: Option<NotifyLevel>) {
        let hub = self.hub_mut(index);
        if room.is_empty() {
            hub.notify = level.filter(|l| *l != NotifyLevel::Mentions);
        } else {
            match level {
                Some(level) => hub.room_notify.insert(room.to_string(), level),
                None => hub.room_notify.remove(room),
            };
        }
        self.save_hubs();
    }

    /// `N`: the next notification level for the hub, room or whisper
    /// conversation on screen. A room goes through the levels other than
    /// its hub's, then back to the hub's.
    fn cycle_notify_level(&mut self) {
        let Some((index, room)) = self.channels.active() else { return };
        let hub = &self.channels.hubs[index];
        let whisper = whisper_peer(&room).is_some();
        // Whispers notify at either level but off.
        let order: &[NotifyLevel] =
            if whisper { &[NotifyLevel::Mentions, NotifyLevel::Off] } else { &[NotifyLevel::Mentions, NotifyLevel::All, NotifyLevel::Off] };
        let same = |a: NotifyLevel, b: NotifyLevel| a == b || (whisper && a != NotifyLevel::Off && b != NotifyLevel::Off);
        let hub_level = hub.notify.unwrap_or(NotifyLevel::Mentions);
        let next = if room.is_empty() {
            let at = order.iter().position(|l| *l == hub_level).unwrap_or(0);
            Some(order[(at + 1) % order.len()])
        } else {
            let choices: Vec<Option<NotifyLevel>> =
                std::iter::once(None).chain(order.iter().copied().filter(|l| !same(*l, hub_level)).map(Some)).collect();
            let current = hub.room_notify.get(&room).copied().filter(|l| !same(*l, hub_level));
            let at = choices.iter().position(|c| *c == current).unwrap_or(0);
            choices[(at + 1) % choices.len()]
        };
        self.set_notify_level(index, &room, next);
        let hub = &self.channels.hubs[index];
        let level = hub.notify_level(&room);
        let what = if room.is_empty() {
            hub.hub_name.clone().unwrap_or_else(|| hub.name.clone())
        } else if whisper {
            format!("@{}", hub.whisper_name(&room))
        } else {
            format!("#{room}")
        };
        let label = if whisper && level != NotifyLevel::Off { "on" } else { level.label() };
        let inherited = if !room.is_empty() && next.is_none() { " (as the hub)" } else { "" };
        self.notify(format!("Notifications in {what}: {label}{inherited}"));
    }

    pub(crate) fn click_channels(&mut self, at: Position) {
        if self.channels.menu.is_some() || self.channels.picker.is_some() {
            return self.click_user_menu(at);
        }
        let mention = self.regions.channel_mentions.iter().find(|(rect, _)| rect.contains(at)).map(|(_, name)| name.clone());
        if let Some(name) = mention {
            if let Some((start, _)) = self.mention_matches() {
                self.pick_mention(start, &name);
            }
            return;
        }
        let user = self.regions.channel_users.iter().find(|(rect, _)| rect.contains(at)).map(|(_, id)| id.clone());
        if let Some(identity) = user {
            return self.open_user_menu(&identity);
        }
        let list = self.regions.channel_list;
        if list.contains(at) {
            self.channels.typing = false;
            let index = self.channels.list.offset() + (at.y - list.y) as usize;
            self.select_channel_row(index);
        } else if self.regions.channel_input.contains(at) && self.channels.active().is_some() {
            self.channels.typing = true;
        } else if let Some((_, room)) = self
            .regions
            .channel_rooms
            .iter()
            .find(|(rect, _)| rect.contains(at))
            .cloned()
        {
            self.join_listed_room(&room);
        }
    }
}
