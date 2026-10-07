//! Searching channel (RRC) history: what was said in every hub's rooms and
//! whisper conversations, as rettui keeps it, or in the one open. Every
//! word must be found, in the line or the name of who said it, as the
//! message search has it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Channels, LineKind, Target};
use crate::app::network::{matches_text, search_terms};
use crate::app::search::MessageSearch;
use crate::app::{App, Tab};
use crate::net::Hash;

/// Most results listed: the newest.
pub const MAX_HITS: usize = 200;

/// A line found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelHit {
    pub hub: Hash,
    /// Its room's buffer: a room's name, a whisper conversation's key, or
    /// "" for the hub's own page.
    pub room: String,
    /// Where it is in that buffer.
    pub index: usize,
    /// When it arrived (Unix milliseconds).
    pub ts: u64,
}

/// Lines matching `query`, newest first, at most [`MAX_HITS`]: in the
/// buffer `only`, or in all of them. What people said (not joins, leaves
/// or errors).
pub fn search(channels: &Channels, query: &str, only: Option<(Hash, &str)>) -> Vec<ChannelHit> {
    let terms = search_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<ChannelHit> = Vec::new();
    for hub in &channels.hubs {
        for (room, lines) in &hub.buffers {
            if only.is_some_and(|(h, r)| h != hub.hash || r != room) {
                continue;
            }
            for (index, line) in lines.iter().enumerate() {
                let said = matches!(line.kind, LineKind::Msg | LineKind::Action | LineKind::Private | LineKind::Notice);
                let nick = line.nick.as_deref().unwrap_or_default();
                if said && matches_text(&terms, &[line.text.as_str(), nick]) {
                    hits.push(ChannelHit { hub: hub.hash, room: room.clone(), index, ts: line.ts });
                }
            }
        }
    }
    hits.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| a.room.cmp(&b.room)));
    hits.truncate(MAX_HITS);
    hits
}

impl App {
    /// Open the channel search (`/` in Channels).
    pub(crate) fn open_channel_search(&mut self) {
        self.channel_search = Some(MessageSearch::default());
        self.channels.typing = false;
    }

    /// What the channel search finds now.
    pub fn channel_search_hits(&self) -> Vec<ChannelHit> {
        let Some(open) = &self.channel_search else { return Vec::new() };
        let here = self.channels.active().map(|(i, room)| (self.channels.hubs[i].hash, room));
        let only = open.here.then_some(here).flatten();
        search(&self.channels, open.input.text(), only.as_ref().map(|(h, r)| (*h, r.as_str())))
    }

    pub(crate) fn channel_search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.channel_search_hits().len();
        let open = self.channels.active().is_some();
        let Some(search) = self.channel_search.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.channel_search = None,
            KeyCode::Enter => {
                let selected = search.selected;
                if let Some(hit) = self.channel_search_hits().get(selected).cloned() {
                    self.open_channel_hit(&hit);
                }
            }
            KeyCode::Up => search.selected = search.selected.saturating_sub(1),
            KeyCode::Down => search.selected = (search.selected + 1).min(count.saturating_sub(1)),
            KeyCode::PageUp => search.selected = search.selected.saturating_sub(10),
            KeyCode::PageDown => search.selected = (search.selected + 10).min(count.saturating_sub(1)),
            KeyCode::Tab if open => {
                search.here = !search.here;
                search.selected = 0;
            }
            KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
            _ => {
                if search.input.handle(key) {
                    search.selected = 0;
                }
            }
        }
    }

    pub(crate) fn paste_channel_search(&mut self, text: &str) {
        if let Some(search) = self.channel_search.as_mut() {
            search.input.insert_str(&text.replace(['\n', '\r'], " "));
            search.selected = 0;
        }
    }

    /// Open a hit's room, scrolled to the line, marked.
    pub(crate) fn open_channel_hit(&mut self, hit: &ChannelHit) {
        self.channel_search = None;
        self.tab = Tab::Channels;
        let room = (!hit.room.is_empty()).then(|| hit.room.clone());
        let target = Target { hub: hit.hub, room };
        if let Some(row) = self.channels.rows().iter().position(|row| match row {
            super::Row::Hub(i) => self.channels.hubs[*i].hash == hit.hub && hit.room.is_empty(),
            super::Row::Room(i, r) | super::Row::Whisper(i, r) => self.channels.hubs[*i].hash == hit.hub && *r == hit.room,
        }) {
            self.channels.list.select(Some(row));
        }
        self.channels.selected = Some(target);
        self.channels.sync_draft();
        self.channels.found = Some((hit.hub, hit.room.clone(), hit.index));
        self.channels.jump_to = Some(hit.index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::channels::{ChatLine, Hub};

    fn said(nick: &str, text: &str, ts: u64) -> ChatLine {
        ChatLine { nick: Some(nick.into()), ts, ..ChatLine::presence(text.into()) }
    }

    fn channels() -> Channels {
        let mut channels = Channels::default();
        let mut hub = Hub::new([1; 16], "rrc".into(), "Hilltop".into());
        let line = |nick: &str, text: &str, ts| ChatLine { kind: LineKind::Msg, presence: false, ..said(nick, text, ts) };
        hub.buffers.insert("general".into(), vec![line("ann", "the repeater is up", 10), line("bob", "nice, which antenna?", 20)]);
        hub.buffers.insert("radio".into(), vec![line("cat", "Antenna on the roof now", 30)]);
        // Who joined and left isn't searched.
        hub.buffers.get_mut("general").unwrap().push(said("dan", "dan joined", 40));
        channels.hubs.push(hub);
        channels
    }

    #[test]
    fn every_word_in_the_line_or_its_name_newest_first() {
        let channels = channels();
        let found = search(&channels, "antenna", None);
        assert_eq!(found.iter().map(|h| (h.room.as_str(), h.index)).collect::<Vec<_>>(), [("radio", 0), ("general", 1)]);
        // A name counts, with the rest of the words anywhere.
        assert_eq!(search(&channels, "ann repeater", None).len(), 1);
        assert!(search(&channels, "joined", None).is_empty());
        assert!(search(&channels, "   ", None).is_empty());
        // In one room only.
        let only = search(&channels, "antenna", Some(([1; 16], "general")));
        assert_eq!(only.iter().map(|h| h.ts).collect::<Vec<_>>(), [20]);
    }

    #[test]
    fn slash_finds_and_enter_opens_the_room_at_the_line() {
        let dir = std::env::temp_dir().join(format!("rettui-channel-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        app.channels = channels();
        app.tab = Tab::Channels;
        let press = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        press(&mut app, KeyCode::Char('/'));
        assert!(app.channel_search.is_some());
        for c in "antenna".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.channel_search_hits().len(), 2);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(app.channel_search.is_none());
        assert_eq!(app.channels.selected, Some(Target { hub: [1; 16], room: Some("general".into()) }));
        assert_eq!((app.channels.found.clone(), app.channels.jump_to), (Some(([1; 16], "general".into(), 1)), Some(1)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
