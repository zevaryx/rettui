//! The Tools tab: the tests of the network in one place. Finding and
//! probing a path, forgetting one, whose an address is, a speed test and
//! rncp files (all also in the Network tab), and a check of the interfaces
//! and of whether each entry point in the Reticulum config can be reached.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};

use super::reach::{self, Reach};
use super::{App, PromptKind};
use crate::net::InterfaceInfo;
use crate::net::checks::{self, EntryPoint};

/// How many addresses are remembered, newest first.
const HISTORY: usize = 8;

/// A test in the Tools tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    FindPath,
    Probe,
    ForgetPath,
    Identities,
    SpeedTest,
    SendFile,
    FetchFile,
    CheckInterfaces,
}

impl Tool {
    pub const ALL: [Tool; 8] = [
        Tool::FindPath,
        Tool::Probe,
        Tool::ForgetPath,
        Tool::Identities,
        Tool::SpeedTest,
        Tool::SendFile,
        Tool::FetchFile,
        Tool::CheckInterfaces,
    ];

    /// Its name, its key and what it does.
    pub fn label(self) -> (&'static str, char, &'static str) {
        match self {
            Tool::FindPath => ("Find a path", 'P', "Where an address is reached through (asks for one if none is known), as rnpath does"),
            Tool::Probe => ("Probe", 'T', "Times an answer from an address, as rnprobe does"),
            Tool::ForgetPath => ("Forget a path", 'D', "Drops a path gone stale: the next use asks for a fresh one"),
            Tool::Identities => ("Identities", 'I', "Your addresses, and whose another address is"),
            Tool::SpeedTest => ("Speed test", 'v', "How fast data goes between you and someone else, with rettui or NomadNet"),
            Tool::SendFile => ("Send a file", 'F', "To someone running rncp -l"),
            Tool::FetchFile => ("Fetch a file", 'O', "From someone running rncp -l -F"),
            Tool::CheckInterfaces => ("Check interfaces", 'c', "Whether each interface is up, and each entry point can be reached"),
        }
    }

    /// The tool with this key.
    pub fn with_key(key: char) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.label().1 == key)
    }
}

/// How a try at an entry point is put in words: "up · 120 ms", or
/// "down · refused" with what to do about it.
pub fn reach_label(reach: Reach) -> String {
    match reach {
        Reach::Down(why) => format!("{} ({})", reach.label(), down_hint(why)),
        _ => reach.label(),
    }
}

/// What a server interface's check shows and doesn't: that it accepts
/// connections here, not whether others can reach it from outside.
pub const LISTEN_NOTE: &str = "Tried from this computer: it shows rettui is listening, not whether a router or firewall lets others in.";

pub fn down_hint(why: &str) -> &'static str {
    match why {
        "refused" => "nothing accepted the connection: the host may be down, or the port is wrong",
        "no answer" => "the host may be down, or a firewall is in the way",
        "unreachable" => "this computer has no route there: check its internet connection",
        _ => "check the host name and this computer's internet connection",
    }
}

/// What the Tools tab remembers of its own.
#[derive(Debug, Default)]
pub struct ToolsState {
    pub selected: usize,
    /// The addresses last tried, newest first.
    pub history: Vec<String>,
}

/// An interface as the check shows it.
#[derive(Debug, Clone)]
pub struct InterfaceRow {
    pub info: InterfaceInfo,
    /// The config entry that connects to a host, if this is one.
    pub entry: Option<EntryPoint>,
    pub check: Option<Reach>,
}

impl App {
    /// Remember an address the tools were used on.
    pub(super) fn note_tool_address(&mut self, key: &str) {
        self.tools.history.retain(|k| k != key);
        self.tools.history.insert(0, key.to_string());
        self.tools.history.truncate(HISTORY);
    }

    /// Each interface running, with the config entry it connects through
    /// and how checking that went. Entry points not running (off, or not
    /// up yet) follow, so a check of them shows too.
    pub fn interface_rows(&self) -> Vec<InterfaceRow> {
        let entries = checks::entry_points(&self.rns.text);
        let mut left = entries.clone();
        let mut rows: Vec<InterfaceRow> = self
            .interfaces
            .iter()
            .map(|info| {
                let at = left.iter().position(|e| info.name.contains(&e.name));
                let entry = at.map(|at| left.remove(at));
                let check = entry.as_ref().and_then(|e| self.entry_check(&e.name));
                InterfaceRow { info: info.clone(), entry, check }
            })
            .collect();
        for entry in left {
            let check = self.entry_check(&entry.name);
            let info = InterfaceInfo { name: entry.name.clone(), online: false, ..InterfaceInfo::default() };
            rows.push(InterfaceRow { info, entry: Some(entry), check });
        }
        rows
    }

    /// How the last try at the entry point of this name went.
    pub fn entry_check(&self, name: &str) -> Option<Reach> {
        self.entry_checks.lock().ok()?.get(name).copied()
    }

    /// Try connecting to every entry point of the config that's switched
    /// on (each on a thread of its own, as a name lookup can take seconds).
    /// Says how many.
    pub fn test_entry_points(&mut self) -> Result<usize, String> {
        let entries: Vec<_> = checks::entry_points(&self.rns.text).into_iter().filter(|e| e.enabled).collect();
        if entries.is_empty() {
            return Err(
                "No interface in the Reticulum config that connects out or listens is switched on: add one in the Reticulum tab".into()
            );
        }
        if let Ok(mut results) = self.entry_checks.lock() {
            results.clear();
            for entry in &entries {
                results.insert(entry.name.clone(), Reach::Checking);
            }
        }
        // Tests don't reach out to the internet (they set results instead).
        if !cfg!(test) {
            for entry in &entries {
                let (results, entry) = (Arc::clone(&self.entry_checks), entry.clone());
                let _ = std::thread::Builder::new().name("rettui-reach".into()).spawn(move || {
                    let reach = reach::probe(&entry.host, entry.port);
                    if let Ok(mut results) = results.lock() {
                        results.insert(entry.name, reach);
                    }
                });
            }
        }
        self.confirm(format!("Checking {} entry point{}…", entries.len(), if entries.len() == 1 { "" } else { "s" }));
        Ok(entries.len())
    }

    /// Run a tool; on an address, the one last tried (or `address`).
    pub(super) fn run_tool(&mut self, tool: Tool, address: Option<String>) {
        let current = address.or_else(|| self.tools.history.first().cloned());
        match tool {
            Tool::FindPath => self.open_prompt(PromptKind::FindPath, "Find a path to (address)", &current.unwrap_or_default()),
            Tool::Probe => self.open_prompt(PromptKind::ProbeAddress, "Probe (address)", &current.unwrap_or_default()),
            Tool::ForgetPath => self.open_prompt(PromptKind::ForgetAddress, "Forget the path to (address)", &current.unwrap_or_default()),
            Tool::Identities => self.open_whois(current),
            Tool::SpeedTest => self.toggle_speed(),
            Tool::SendFile => self.ask_rncp(current),
            Tool::FetchFile => self.ask_rncp_fetch(current),
            Tool::CheckInterfaces => {
                if let Err(e) = self.test_entry_points() {
                    self.warn(e);
                }
            }
        }
    }

    pub(super) fn tools_key(&mut self, key: KeyEvent) {
        if self.whois.open {
            return self.whois_key(key);
        }
        if self.speed.open {
            return self.speed_key(key);
        }
        let last = Tool::ALL.len() - 1;
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.tools.selected = (self.tools.selected + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => self.tools.selected = self.tools.selected.saturating_sub(1),
            KeyCode::Enter => self.run_tool(Tool::ALL[self.tools.selected.min(last)], None),
            KeyCode::Char('y') => {
                if let Some(address) = self.tools.history.first().cloned() {
                    self.copy(&address, "address");
                }
            }
            KeyCode::Char(c) => {
                if let Some(tool) = Tool::with_key(c) {
                    self.tools.selected = Tool::ALL.iter().position(|t| *t == tool).unwrap_or(0);
                    self.run_tool(tool, None);
                }
            }
            _ => {}
        }
    }

    /// A click in the Tools tab, on a tool's row.
    pub(super) fn click_tools(&mut self, at: ratatui::layout::Position) {
        let list = self.regions.tools;
        if !list.contains(at) {
            return;
        }
        let row = (at.y - list.y) as usize;
        if let Some(tool) = Tool::ALL.get(row) {
            self.tools.selected = row;
            self.run_tool(*tool, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use crate::store::Store;

    #[test]
    fn checking_entry_points_says_what_it_tried_and_how_it_went() {
        let dir = std::env::temp_dir().join(format!("rettui-tools-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        app.rns.text = "[interfaces]\n  [[Testnet]]\n    type = TCPClientInterface\n    enabled = yes\n    target_host = example.org\n    target_port = 4242\n    networkname = net\n  [[Off]]\n    type = TCPClientInterface\n    enabled = no\n    target_host = example.net\n    target_port = 1\n".into();
        assert_eq!(app.test_entry_points(), Ok(1));
        assert_eq!(app.entry_check("Testnet"), Some(Reach::Checking));
        assert_eq!(app.entry_check("Off"), None);
        // The entry point shows with its interface, and the off one after it.
        app.interfaces.push(InterfaceInfo { name: "TCPInterface[Testnet]".into(), online: true, ..InterfaceInfo::default() });
        app.entry_checks.lock().unwrap().insert("Testnet".into(), Reach::Up(std::time::Duration::from_millis(37)));
        let rows = app.interface_rows();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].entry.as_ref().is_some_and(|e| e.ifac) && rows[0].check == Some(Reach::Up(std::time::Duration::from_millis(37))));
        assert!(!rows[1].info.online && rows[1].check.is_none());
        assert_eq!(reach_label(Reach::Up(std::time::Duration::from_millis(37))), "up · 37 ms");
        assert!(reach_label(Reach::Down("refused")).starts_with("down · refused (nothing accepted the connection"));
        for why in ["no answer", "unreachable", "name not found"] {
            assert!(reach_label(Reach::Down(why)).contains(why));
        }
        // None switched on: it says so.
        app.rns.text = String::new();
        assert!(app.test_entry_points().unwrap_err().contains("switched on"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_addresses_tried_are_remembered_newest_first() {
        let dir = std::env::temp_dir().join(format!("rettui-tools-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        for n in 0..10u8 {
            app.find_path_to(&hex::encode([n; 16])).unwrap();
        }
        app.probe(&hex::encode([3u8; 16])).unwrap();
        assert_eq!(app.tools.history.len(), HISTORY);
        assert_eq!(app.tools.history[0], hex::encode([3u8; 16]));
        assert_eq!(app.tools.history.iter().filter(|k| **k == hex::encode([3u8; 16])).count(), 1);
        assert_eq!(Tool::with_key('T'), Some(Tool::Probe));
        assert_eq!(Tool::with_key('z'), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
