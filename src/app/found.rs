//! Interfaces others make known (`e` in the Network tab, *Interfaces
//! found* in the web UI), as Sideband 2.x lists them: those interface
//! discovery finds, and those contacts send in their telemetry (Sideband's
//! Interfaces sensor). Each says how to reach it, and one can be added to
//! the Reticulum config. Those that say where they are go on the map.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::App;
use crate::net::found::FoundInterface;
use crate::store::Trust;

#[derive(Debug, Default)]
pub struct FoundView {
    /// Shown in the Network tab.
    pub open: bool,
    pub list: ListState,
}

/// An interface found, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub interface: FoundInterface,
    /// The contact who sent it in their telemetry, by address and name;
    /// none for one discovery found.
    pub from: Option<(String, String)>,
    /// When they sent it (Unix seconds).
    pub at: Option<f64>,
    /// Whether rettui is connected to it now (one discovery found, which
    /// *Discovered entry points* connected to).
    pub connected: bool,
}

impl Found {
    /// Where it came from, in words.
    pub fn source(&self) -> String {
        match &self.from {
            None => "interface discovery".into(),
            Some((_, name)) => format!("{name}'s telemetry"),
        }
    }
}

impl App {
    pub(super) fn on_discovered(&mut self, found: Vec<FoundInterface>) {
        self.discovered = found;
    }

    /// Every interface made known: those discovery found (newest first),
    /// then those contacts sent, from the newest telemetry of each that
    /// brought any. Blocked contacts' are left out, and one heard both
    /// ways is listed once.
    pub fn found_interfaces(&self) -> Vec<Found> {
        let mut found: Vec<Found> = self
            .discovered
            .iter()
            .map(|interface| {
                let name = format!("Discovered/{}", interface.name.replace('/', "_"));
                let connected = self.interfaces.iter().any(|i| i.online && i.name == name);
                Found { interface: interface.clone(), from: None, at: None, connected }
            })
            .collect();
        let mut sent: Vec<Found> = self
            .store
            .conversations
            .iter()
            .filter(|(key, _)| self.store.contact(key).trust != Trust::Blocked)
            .filter_map(|(key, conversation)| {
                let newest = conversation.messages.iter().rev().find(|m| m.incoming && !m.interfaces.is_empty())?;
                Some((key, newest))
            })
            .flat_map(|(key, message)| {
                let from = (key.clone(), self.store.display_name(key));
                message.interfaces.iter().map(move |interface| Found {
                    interface: interface.clone(),
                    from: Some(from.clone()),
                    at: Some(message.timestamp),
                    connected: false,
                })
            })
            .collect();
        sent.sort_by(|a, b| b.at.unwrap_or(0.0).total_cmp(&a.at.unwrap_or(0.0)));
        for one in sent {
            if !found.iter().any(|f| f.interface.id() == one.interface.id()) {
                found.push(one);
            }
        }
        found
    }

    /// Add the interface with this id to the Reticulum config, as a TCP
    /// client for an entry point, or an I2P or RNode interface (an RNode
    /// added off, as its port has to be set first). Says what was done.
    pub fn add_found_interface(&mut self, id: &str, restricted: bool) -> Result<String, String> {
        let found = self.found_interfaces().into_iter().find(|f| f.interface.id() == id).ok_or("That interface isn't listed any more")?;
        let crate::net::found::ConfigEntry { kind, keys, off } = found.interface.config()?;
        let path = self.rns_path();
        let text = crate::reticulum::load(&path)?.0;
        let base = found.interface.config_name();
        let taken = crate::reticulum::sections(&text);
        let free = |name: &str| !taken.iter().any(|s| matches!(s, crate::reticulum::Section::Interface(n) if n.eq_ignore_ascii_case(name)));
        let name = std::iter::once(base.clone())
            .chain((2..100).map(|n| format!("{base} {n}")))
            .find(|n| free(n))
            .ok_or("Too many with that name")?;
        let what = format!("Added interface {name} ({kind}) from {}", found.source());
        let warnings = self.rns_edit(restricted, &what, |text| crate::reticulum::add_found_interface(text, &name, kind, &keys, !off))?;
        for warning in warnings {
            self.log(warning);
        }
        Ok(if off {
            format!("Added {name} to the Reticulum config, off: set its port (and TX power) in the Reticulum tab, then turn it on")
        } else {
            format!("Added {name} to the Reticulum config: it connects when Reticulum restarts")
        })
    }

    // ---- TUI ---------------------------------------------------------------

    pub(super) fn toggle_found(&mut self) {
        self.found_view.open = !self.found_view.open;
        if self.found_view.open {
            self.found_view.list = ListState::default().with_selected(Some(0));
        }
    }

    /// Show the one with this id in the Network tab.
    pub(super) fn show_found(&mut self, id: &str) {
        let at = self.found_interfaces().iter().position(|f| f.interface.id() == id).unwrap_or(0);
        self.tab = super::Tab::Network;
        self.found_view.open = true;
        self.found_view.list = ListState::default().with_selected(Some(at));
    }

    pub(super) fn found_key(&mut self, key: KeyEvent) {
        let rows = self.found_interfaces();
        let at = self.found_view.list.selected().unwrap_or(0).min(rows.len().saturating_sub(1));
        let last = rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('e') | KeyCode::Char('q') => self.found_view.open = false,
            KeyCode::Down | KeyCode::Char('j') => self.found_view.list.select(Some((at + 1).min(last))),
            KeyCode::Up | KeyCode::Char('k') => self.found_view.list.select(Some(at.saturating_sub(1))),
            KeyCode::Home => self.found_view.list.select(Some(0)),
            KeyCode::End => self.found_view.list.select(Some(last)),
            KeyCode::Char('y') => {
                if let Some(reach) = rows.get(at).and_then(|f| f.interface.reach()) {
                    self.copy(&reach, "where to reach it");
                }
            }
            KeyCode::Char('m') => {
                if let Some(found) = rows.get(at) {
                    if found.interface.location().is_none() {
                        return self.warn("It doesn't say where it is");
                    }
                    let id = found.interface.id();
                    self.found_view.open = false;
                    self.tab = super::Tab::Messages;
                    self.open_map_at(|p| p.interface.as_deref() == Some(id.as_str()));
                }
            }
            KeyCode::Char('a') | KeyCode::Enter => {
                if let Some(found) = rows.get(at) {
                    match self.add_found_interface(&found.interface.id(), false) {
                        Ok(done) => self.confirm(done),
                        Err(e) => self.warn(e),
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Conversation, Message, MessageState, Store};

    fn entry_point(name: &str, host: &str) -> FoundInterface {
        FoundInterface {
            name: name.into(),
            kind: "BackboneInterface".into(),
            transport: true,
            transport_id: Some("ab".repeat(16)),
            reachable_on: Some(host.into()),
            port: Some(4242),
            latitude: Some(51.5),
            longitude: Some(-0.12),
            last_heard: Some(100),
            heard_count: Some(3),
            ..FoundInterface::default()
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-found-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app_with(dir: &std::path::Path, store: Store) -> App {
        let rns = dir.join("rns");
        std::fs::create_dir_all(&rns).unwrap();
        std::fs::write(rns.join("config"), "[reticulum]\n  share_instance = Yes\n\n[interfaces]\n  [[Hill]]\n    type = AutoInterface\n")
            .unwrap();
        let settings = crate::config::Settings { rns_config: Some(rns.display().to_string()), ..crate::config::Settings::default() };
        crate::app::test_app(dir, settings, store)
    }

    #[test]
    fn found_by_discovery_and_sent_by_contacts_are_listed_once_each() {
        let dir = scratch("listed");
        let alice = "aa".repeat(16);
        let blocked = "bb".repeat(16);
        let mut store = Store::default();
        let sent = |interfaces: Vec<FoundInterface>| Message {
            id: "m".into(),
            incoming: true,
            timestamp: 50.0,
            state: MessageState::Received { verified: true },
            interfaces,
            ..Message::default()
        };
        store.conversations.insert(
            alice.clone(),
            Conversation {
                messages: vec![sent(vec![entry_point("Hill", "hill.example"), entry_point("Vale", "vale.example")])],
                ..Conversation::default()
            },
        );
        store.conversations.insert(
            blocked.clone(),
            Conversation { messages: vec![sent(vec![entry_point("Spam", "spam.example")])], ..Conversation::default() },
        );
        store.contacts.entry(blocked).or_default().trust = Trust::Blocked;
        let mut app = app_with(&dir, store);
        app.on_discovered(vec![entry_point("Hill", "hill.example")]);
        let found = app.found_interfaces();
        let said: Vec<(String, String)> = found.iter().map(|f| (f.interface.name.clone(), f.source())).collect();
        let alices = format!("{}'s telemetry", app.store.display_name(&alice));
        assert_eq!(said, vec![("Hill".to_string(), "interface discovery".to_string()), ("Vale".to_string(), alices)]);
    }

    #[test]
    fn one_is_added_to_the_reticulum_config_with_a_free_name() {
        let dir = scratch("added");
        let mut app = app_with(&dir, Store::default());
        app.on_discovered(vec![entry_point("Hill", "hill.example"), entry_point("Home", "127.0.0.1")]);
        let hill = app.found_interfaces()[0].interface.id();
        let said = app.add_found_interface(&hill, true).unwrap();
        assert!(said.contains("Hill 2"), "{said}");
        let text = std::fs::read_to_string(app.rns_path()).unwrap();
        assert!(
            text.contains("[[Hill 2]]") && text.contains("target_host = hill.example") && text.contains("target_port = 4242"),
            "{text}"
        );
        assert!(text.contains("type = TCPClientInterface"));
        // It's in the config's history, saying where it came from.
        assert!(app.rns_versions().iter().any(|v| v.what.contains("from interface discovery")));
        // This device isn't connected to on a stranger's say-so.
        let home = app.found_interfaces()[1].interface.id();
        assert!(app.add_found_interface(&home, true).unwrap_err().contains("this device"));
    }
}
