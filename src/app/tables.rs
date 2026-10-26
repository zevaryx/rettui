//! Reticulum's tables (`t` in the Network tab): every path known, and how
//! often each destination announces, as `rnpath -t` and `rnpath -r` list
//! them. Read when shown, and again every few seconds while they are (in
//! the terminal, or asked for by a browser).

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::{App, now};
use crate::net::{AnnounceRate, NetCommand, PathInfo, Tables};

/// Seconds between readings while they're shown.
pub const READ_EVERY: f64 = 5.0;
/// Seconds a browser's asking keeps them read.
const WANTED_FOR: f64 = 15.0;
/// Seconds after which an answer that hasn't come is given up on.
const GIVE_UP: f64 = 30.0;

/// Which table is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Table {
    #[default]
    Paths,
    Rates,
}

impl Table {
    pub fn key(self) -> &'static str {
        match self {
            Table::Paths => "paths",
            Table::Rates => "rates",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Table::Paths, Table::Rates].into_iter().find(|t| t.key() == text)
    }

    pub fn label(self) -> &'static str {
        match self {
            Table::Paths => "Path table",
            Table::Rates => "Announce rates",
        }
    }

    fn other(self) -> Self {
        match self {
            Table::Paths => Table::Rates,
            Table::Rates => Table::Paths,
        }
    }
}

#[derive(Debug, Default)]
pub struct TablesView {
    /// Shown in the Network tab, in place of the list.
    pub open: bool,
    pub which: Table,
    pub list: ListState,
    /// The last reading, or why it couldn't be read.
    pub read: Option<Result<Tables, String>>,
    /// When they were last asked for (Unix seconds), and whether that's
    /// still to be answered.
    asked: f64,
    waiting: bool,
    /// Kept read until then, for a browser showing them.
    wanted_until: f64,
}

/// A path, with the name of where it goes.
pub struct PathRow<'a> {
    pub key: String,
    pub name: Option<&'a str>,
    pub info: &'a PathInfo,
}

/// A destination's announce rate, with its name.
pub struct RateRow<'a> {
    pub key: String,
    pub name: Option<&'a str>,
    pub rate: &'a AnnounceRate,
}

/// How long ago, in words: "40 s", "3 min", "5 h", "6 days".
fn span(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        3600..172_800 => format!("{} h", secs / 3600),
        _ => format!("{} days", secs / 86_400),
    }
}

/// An announce rate in words, what matters most first: "held back for
/// 12 min · 2 times too often · 4.2 an hour · announced 3 min ago".
pub fn rate_label(rate: &AnnounceRate, now: f64) -> String {
    let mut parts = Vec::new();
    if rate.blocked_until > now {
        parts.push(format!("held back for {}", span(rate.blocked_until - now)));
    }
    if rate.violations > 0 {
        parts.push(format!("{} time{} too often", rate.violations, if rate.violations == 1 { "" } else { "s" }));
    }
    if let Some(per_hour) = rate.per_hour {
        parts.push(format!("{per_hour:.1} an hour"));
    }
    parts.push(format!("announced {} ago", span(now - rate.last)));
    parts.join(" · ")
}

impl App {
    /// Read the tables now, unless a reading is on its way.
    fn read_tables(&mut self) {
        let now = now();
        let view = &mut self.tables;
        if view.waiting && now - view.asked < GIVE_UP {
            return;
        }
        view.asked = now;
        view.waiting = true;
        self.send(NetCommand::ReadTables);
    }

    /// A browser shows them: keep them read for a while.
    pub fn tables_wanted(&mut self) {
        self.tables.wanted_until = now() + WANTED_FOR;
        if now() - self.tables.asked >= READ_EVERY {
            self.read_tables();
        }
    }

    pub(super) fn tables_tick(&mut self) {
        let view = &self.tables;
        if (view.open || now() < view.wanted_until) && now() - view.asked >= READ_EVERY {
            self.read_tables();
        }
    }

    pub(super) fn on_tables(&mut self, result: Result<Tables, String>) {
        self.tables.waiting = false;
        self.tables.read = Some(result);
    }

    /// Names for addresses: a contact's, an announced one, or the newest
    /// heard in the announce viewer (which hears every kind).
    fn heard_names(&self) -> HashMap<String, &str> {
        let mut names: HashMap<String, &str> = HashMap::new();
        for entry in &self.announce_log.entries {
            if let Some(name) = entry.heard.name.as_deref() {
                names.insert(entry.address(), name);
            }
        }
        names
    }

    fn table_name<'a>(&'a self, key: &str, heard: &HashMap<String, &'a str>) -> Option<&'a str> {
        self.store
            .contacts
            .get(key)
            .and_then(|c| c.alias.as_deref())
            .or_else(|| self.store.announced_name(key))
            .or_else(|| heard.get(key).copied())
    }

    /// The paths known that `terms` find (by name, address, the transport
    /// node they go through or their interface), nearest first.
    pub fn path_rows(&self, terms: &[Vec<char>]) -> Vec<PathRow<'_>> {
        let Some(Ok(tables)) = &self.tables.read else { return Vec::new() };
        let heard = self.heard_names();
        let mut rows: Vec<PathRow> = tables
            .paths
            .iter()
            .map(|(hash, info)| {
                let key = hex::encode(hash);
                let name = self.table_name(&key, &heard);
                PathRow { key, name, info }
            })
            .filter(|row| {
                let via = row.info.via.map(hex::encode).unwrap_or_default();
                super::network::matches_text(terms, &[row.name.unwrap_or_default(), &row.key, &via, &row.info.interface])
            })
            .collect();
        rows.sort_by(|a, b| {
            a.info.hops.cmp(&b.info.hops).then_with(|| a.name.is_none().cmp(&b.name.is_none())).then_with(|| a.key.cmp(&b.key))
        });
        rows
    }

    /// The announce rates that `terms` find, by name or address, those
    /// held back first, then the most often.
    pub fn rate_rows(&self, terms: &[Vec<char>]) -> Vec<RateRow<'_>> {
        let Some(Ok(tables)) = &self.tables.read else { return Vec::new() };
        let heard = self.heard_names();
        let now = now();
        let mut rows: Vec<RateRow> = tables
            .rates
            .iter()
            .map(|rate| {
                let key = hex::encode(rate.hash);
                let name = self.table_name(&key, &heard);
                RateRow { key, name, rate }
            })
            .filter(|row| super::network::matches_text(terms, &[row.name.unwrap_or_default(), &row.key]))
            .collect();
        rows.sort_by(|a, b| {
            (b.rate.blocked_until > now)
                .cmp(&(a.rate.blocked_until > now))
                .then_with(|| b.rate.per_hour.unwrap_or(0.0).total_cmp(&a.rate.per_hour.unwrap_or(0.0)))
                .then_with(|| a.key.cmp(&b.key))
        });
        rows
    }

    /// How many rows the table shown has, and the address of the one
    /// selected.
    fn table_selection(&self) -> (usize, Option<String>) {
        let terms = self.net_search.terms();
        let at = self.tables.list.selected();
        match self.tables.which {
            Table::Paths => {
                let rows = self.path_rows(&terms);
                (rows.len(), at.and_then(|i| rows.get(i)).map(|r| r.key.clone()))
            }
            Table::Rates => {
                let rows = self.rate_rows(&terms);
                (rows.len(), at.and_then(|i| rows.get(i)).map(|r| r.key.clone()))
            }
        }
    }

    pub(super) fn toggle_tables(&mut self) {
        self.tables.open = !self.tables.open;
        self.tables.list = ListState::default().with_selected(Some(0));
        if self.tables.open {
            self.announce_log.open = false;
            self.read_tables();
        }
    }

    /// Keys while the tables are shown.
    pub(super) fn tables_key(&mut self, key: KeyEvent) {
        let (count, selected) = self.table_selection();
        let at = self.tables.list.selected().unwrap_or(0);
        let last = count.saturating_sub(1);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.tables.list.select(Some((at + 1).min(last))),
            KeyCode::Up | KeyCode::Char('k') => self.tables.list.select(Some(at.saturating_sub(1))),
            KeyCode::PageDown => self.tables.list.select(Some((at + 10).min(last))),
            KeyCode::PageUp => self.tables.list.select(Some(at.saturating_sub(10))),
            KeyCode::Home | KeyCode::Char('g') => self.tables.list.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => self.tables.list.select(Some(last)),
            KeyCode::Char('f') => {
                self.tables.which = self.tables.which.other();
                self.tables.list.select(Some(0));
            }
            KeyCode::Char('t') => self.toggle_tables(),
            KeyCode::Esc if self.net_search.input.text().is_empty() => self.toggle_tables(),
            KeyCode::Esc => self.clear_network_search(),
            KeyCode::Char('/') => self.net_search.typing = true,
            KeyCode::Char('R') => {
                self.read_tables();
                self.confirm("Reading the tables again");
            }
            // As in the Network list.
            KeyCode::Char(c @ ('y' | 'P' | 'T' | 'D' | 'F')) => self.address_action(c, selected),
            _ => {}
        }
    }

    pub(super) fn click_tables(&mut self, at: Position) {
        let area = self.regions.peers;
        if !area.contains(at) {
            return;
        }
        self.net_search.typing = false;
        let index = self.tables.list.offset() + (at.y - area.y) as usize;
        if index < self.table_selection().0 {
            self.tables.list.select(Some(index));
        }
    }
}
