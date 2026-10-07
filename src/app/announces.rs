//! The announce viewer (`a` in the Network tab): every announce heard
//! while rettui runs, of any kind, oldest first, following the newest. The
//! Network list keeps one row per destination, of the kinds rettui uses;
//! this shows them as they arrive, each time, with what else announces.

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::{App, PromptKind, now};
use crate::net::PeerKind;
use crate::net::heard::{Heard, Kind};

/// How many are kept (the oldest go first).
pub const KEPT: usize = 1000;

/// Which kinds the viewer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeardFilter {
    #[default]
    All,
    Lxmf,
    Nomad,
    Propagation,
    /// Anything else: RRC hubs, calls, and what rettui doesn't know.
    Other,
}

impl HeardFilter {
    pub const ALL: [HeardFilter; 5] =
        [HeardFilter::All, HeardFilter::Lxmf, HeardFilter::Nomad, HeardFilter::Propagation, HeardFilter::Other];

    pub fn key(self) -> &'static str {
        match self {
            HeardFilter::All => "all",
            HeardFilter::Lxmf => "lxmf",
            HeardFilter::Nomad => "nomad",
            HeardFilter::Propagation => "propagation",
            HeardFilter::Other => "other",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.key() == text)
    }

    pub fn label(self) -> &'static str {
        match self {
            HeardFilter::All => "all",
            HeardFilter::Lxmf => "LXMF peers",
            HeardFilter::Nomad => "NomadNet nodes",
            HeardFilter::Propagation => "propagation nodes",
            HeardFilter::Other => "everything else",
        }
    }

    pub fn shows(self, kind: Kind) -> bool {
        match self {
            HeardFilter::All => true,
            HeardFilter::Lxmf => kind == Kind::Lxmf,
            HeardFilter::Nomad => kind == Kind::Nomad,
            HeardFilter::Propagation => kind == Kind::Propagation,
            HeardFilter::Other => matches!(kind, Kind::Rrc | Kind::Phone | Kind::Other),
        }
    }
}

/// One announce heard: when (Unix seconds), and a number the web UI asks
/// for those after.
#[derive(Debug, Clone)]
pub struct HeardAnnounce {
    pub seq: u64,
    pub at: f64,
    pub heard: Heard,
}

impl HeardAnnounce {
    pub fn address(&self) -> String {
        hex::encode(self.heard.hash)
    }
}

#[derive(Debug, Default)]
pub struct AnnounceLog {
    pub entries: VecDeque<HeardAnnounce>,
    /// The next one's number.
    next: u64,
    /// Shown in the Network tab, in place of the list.
    pub open: bool,
    pub filter: HeardFilter,
    pub list: ListState,
    /// Moved away from the newest: new ones don't move the selection.
    pub held: bool,
}

impl AnnounceLog {
    pub fn push(&mut self, heard: Heard, at: f64) -> Option<HeardAnnounce> {
        self.entries.push_back(HeardAnnounce { seq: self.next, at, heard });
        self.next += 1;
        (self.entries.len() > KEPT).then(|| self.entries.pop_front()).flatten()
    }

    /// Those after `seq` (all, with none).
    pub fn after(&self, seq: Option<u64>) -> impl Iterator<Item = &HeardAnnounce> {
        self.entries.iter().filter(move |entry| seq.is_none_or(|seq| entry.seq > seq))
    }

    /// How many were heard in the last minute.
    pub fn last_minute(&self) -> usize {
        let since = now() - 60.0;
        self.entries.iter().rev().take_while(|entry| entry.at >= since).count()
    }
}

impl App {
    pub(super) fn on_heard(&mut self, heard: Heard) {
        let dropped = self.announce_log.push(heard, now());
        // Held on an older one: it stays selected as the oldest go.
        if let Some(dropped) = dropped
            && self.announce_log.held
            && self.heard_shows(&dropped)
            && let Some(i) = self.announce_log.list.selected()
        {
            self.announce_log.list.select(Some(i.saturating_sub(1)));
        }
    }

    /// Whether the viewer, as filtered and searched, shows `entry`.
    pub fn heard_shows(&self, entry: &HeardAnnounce) -> bool {
        let address = entry.address();
        self.announce_log.filter.shows(entry.heard.kind)
            && super::network::matches_text(&self.net_search.terms(), &[entry.heard.name.as_deref().unwrap_or_default(), &address])
            && self.goes_via(&address, self.net_via.as_deref())
    }

    /// What the viewer shows, oldest first.
    pub fn heard_rows(&self) -> Vec<&HeardAnnounce> {
        let terms = self.net_search.terms();
        let filter = self.announce_log.filter;
        let via = self.net_via.as_deref();
        self.announce_log
            .entries
            .iter()
            .filter(|entry| {
                if !filter.shows(entry.heard.kind) {
                    return false;
                }
                let address = entry.address();
                super::network::matches_text(&terms, &[entry.heard.name.as_deref().unwrap_or_default(), &address])
                    && self.goes_via(&address, via)
            })
            .collect()
    }

    /// The row selected: the newest, unless held on another.
    pub fn heard_selected(&self, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
        match self.announce_log.list.selected() {
            Some(i) if self.announce_log.held => Some(i.min(count - 1)),
            _ => Some(count - 1),
        }
    }

    pub(super) fn toggle_announce_viewer(&mut self) {
        let log = &mut self.announce_log;
        log.open = !log.open;
        log.held = false;
        log.list = ListState::default();
        self.peers.select(Some(0));
    }

    /// Keys while the viewer is shown.
    pub(super) fn announces_key(&mut self, key: KeyEvent) {
        let (count, selected) = {
            let rows = self.heard_rows();
            let selected = self.heard_selected(rows.len()).and_then(|i| rows.get(i)).map(|entry| (entry.address(), entry.heard.kind));
            (rows.len(), selected)
        };
        let at = self.heard_selected(count);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                self.announce_log.held = true;
                self.announce_log.list.select(at.map(|i| i.saturating_sub(1)));
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                let next = at.map_or(0, |i| (i + 1).min(count - 1));
                // Back at the newest: following again.
                self.announce_log.held = next + 1 < count;
                self.announce_log.list.select(Some(next));
            }
            KeyCode::Home | KeyCode::Char('g') if count > 0 => {
                self.announce_log.held = true;
                self.announce_log.list.select(Some(0));
            }
            KeyCode::End | KeyCode::Char('G') => self.announce_log.held = false,
            KeyCode::Char('a') => self.toggle_announce_viewer(),
            KeyCode::Esc if self.net_search.input.text().is_empty() => self.toggle_announce_viewer(),
            KeyCode::Char('f') => {
                let filters = HeardFilter::ALL;
                let at = filters.iter().position(|f| *f == self.announce_log.filter).unwrap_or(0);
                self.announce_log.filter = filters[(at + 1) % filters.len()];
                self.announce_log.held = false;
            }
            KeyCode::Enter => match selected {
                Some((key, Kind::Lxmf)) => self.open_conversation(key),
                Some((key, Kind::Nomad)) => self.open_peer(key, PeerKind::Nomad),
                Some((key, Kind::Propagation)) => self.set_propagation_node(&key),
                Some((key, Kind::Rrc)) => self.open_prompt(PromptKind::AddHub, "Add an RRC hub (address or rrc:// link)", &key),
                Some(_) => self.warn("rettui can't open that kind: y copies its address, P finds a path to it, T probes it"),
                None => {}
            },
            // As in the Network list.
            KeyCode::Char(c @ ('y' | 'P' | 'T' | 'D')) => self.address_action(c, selected.map(|(key, _)| key)),
            KeyCode::Char('/') => self.net_search.typing = true,
            KeyCode::Esc => self.clear_network_search(),
            KeyCode::Char('i') => self.cycle_net_via(),
            _ => {}
        }
    }

    pub(super) fn click_announces(&mut self, at: Position, double: bool) {
        let area = self.regions.peers;
        if !area.contains(at) {
            return;
        }
        self.net_search.typing = false;
        let index = self.announce_log.list.offset() + (at.y - area.y) as usize;
        let count = self.heard_rows().len();
        if index < count {
            self.announce_log.held = index + 1 < count;
            self.announce_log.list.select(Some(index));
            if double {
                self.announces_key(KeyEvent::from(KeyCode::Enter));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn heard(byte: u8, kind: Kind, name: &str) -> Heard {
        Heard { hash: [byte; 16], kind, aspect: None, name: Some(name.into()), hops: byte % 4 }
    }

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(140, 20)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..20).map(|y| (0..140).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn every_announce_shown_as_heard_following_the_newest() {
        let dir = std::env::temp_dir().join(format!("rettui-announces-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        app.tab = crate::app::Tab::Network;
        press(&mut app, KeyCode::Char('a'));
        assert!(screen(&mut app).contains("No announces heard since rettui started"));
        app.on_heard(heard(1, Kind::Lxmf, "Alex"));
        app.on_heard(heard(2, Kind::Rrc, "Town square"));
        app.on_heard(heard(1, Kind::Lxmf, "Alex"));
        let shown = screen(&mut app);
        assert!(shown.contains("Announces · all · 3 · 3 in the last minute"), "{shown}");
        assert!(shown.contains("RRC") && shown.contains("Town square") && shown.matches("Alex").count() == 2, "{shown}");
        // Following the newest.
        assert_eq!(app.heard_selected(3), Some(2));
        // Held on an older one: new ones don't move it, and it stays put as
        // the oldest go.
        press(&mut app, KeyCode::Up);
        assert_eq!(app.heard_selected(3), Some(1));
        app.on_heard(heard(3, Kind::Other, "thing"));
        assert_eq!(app.heard_rows()[app.heard_selected(4).unwrap()].heard.kind, Kind::Rrc);
        assert!(screen(&mut app).contains("End follows the newest"));
        press(&mut app, KeyCode::End);
        assert_eq!(app.heard_selected(4), Some(3));
        // One kind only.
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.heard_rows().len(), 2);
        press(&mut app, KeyCode::Char('f'));
        press(&mut app, KeyCode::Char('f'));
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.announce_log.filter, HeardFilter::Other);
        assert_eq!(app.heard_rows().iter().map(|e| e.heard.kind).collect::<Vec<_>>(), [Kind::Rrc, Kind::Other]);
        // Only the newest are kept.
        for i in 0..KEPT {
            app.on_heard(heard((i % 200) as u8, Kind::Nomad, "node"));
        }
        assert_eq!(app.announce_log.entries.len(), KEPT);
        // Back to the list.
        press(&mut app, KeyCode::Char('a'));
        assert!(screen(&mut app).contains("Heard announces"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
