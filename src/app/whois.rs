//! Your addresses, and whose another is (`I` in the Network tab), as
//! NomadNet 1.4's Identity utility shows them: the identity behind an
//! address (asked of the network if it hasn't been heard), what it is, and
//! that identity's other addresses.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::{App, PromptKind};
use crate::net::{Hash, Identified, NetCommand, parse_hash};

#[derive(Debug, Default)]
pub struct Whois {
    /// Shown in the Network tab.
    pub open: bool,
    /// The address looked up last, and what was found (`None` while it's
    /// being asked).
    pub looked_up: Option<(Hash, Option<Result<Identified, String>>)>,
    pub list: ListState,
}

/// A row of it: what, and the address (to copy).
#[derive(Debug, serde::Serialize)]
pub struct WhoisRow {
    pub label: String,
    pub address: Option<String>,
    /// A heading, not an address.
    pub heading: bool,
}

impl App {
    /// This client's own addresses, by kind.
    pub fn own_addresses(&self) -> Vec<(&'static str, Hash)> {
        let mut mine = vec![("identity", self.identity_hash)];
        if let Some(lxmf) = self.lxmf_hash {
            mine.push(("LXMF address", lxmf));
        }
        mine.push(("NomadNet node", self.node.hash));
        mine.push(("propagation node", self.pn.hash));
        if let Some((hub, _)) = self.hub.address {
            mine.push(("RRC hub", hub));
        }
        mine.push(("speed test", self.speed_address()));
        mine
    }

    /// Find whose `address` is.
    pub fn whois(&mut self, address: &str) -> Result<(), String> {
        let text = address.trim().trim_start_matches('<').trim_end_matches('>');
        let to = parse_hash(text).ok_or("An address is 32 hex characters")?;
        self.whois.looked_up = Some((to, None));
        self.send(NetCommand::Identify(to));
        Ok(())
    }

    pub(super) fn on_identified(&mut self, to: Hash, result: Result<Identified, String>) {
        if let Some((asked, found)) = &mut self.whois.looked_up
            && *asked == to
        {
            *found = Some(result);
        }
    }

    /// What it shows: your addresses, then what was looked up.
    pub fn whois_rows(&self) -> Vec<WhoisRow> {
        let heading = |label: &str| WhoisRow { label: label.to_string(), address: None, heading: true };
        let row = |label: &str, hash: Hash| WhoisRow { label: label.to_string(), address: Some(hex::encode(hash)), heading: false };
        let mut rows = vec![heading("Yours")];
        rows.extend(self.own_addresses().into_iter().map(|(kind, hash)| row(kind, hash)));
        if let Some((to, found)) = &self.whois.looked_up {
            let name = self.store.display_name(&hex::encode(to));
            rows.push(heading(&format!("Looked up: {name}")));
            match found {
                None => rows.push(WhoisRow { label: "asking the network…".into(), address: None, heading: false }),
                Some(Err(e)) => rows.push(WhoisRow { label: e.clone(), address: None, heading: false }),
                Some(Ok(found)) => {
                    rows.push(row(&format!("identity ({})", found.kind.unwrap_or("a kind rettui doesn't know")), found.identity));
                    rows.extend(found.others.iter().map(|(kind, hash)| row(&format!("its {kind}"), *hash)));
                }
            }
        }
        rows
    }

    // ---- TUI ---------------------------------------------------------------

    /// Open it, looking up `address` (the selected one) if there is one.
    pub(super) fn open_whois(&mut self, address: Option<String>) {
        self.whois.open = true;
        self.whois.list = ListState::default().with_selected(Some(1));
        self.open_prompt(PromptKind::Whois, "Whose address? (empty: just show yours)", &address.unwrap_or_default());
    }

    pub(super) fn submit_whois_prompt(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Err(e) = self.whois(text) {
            self.warn(e);
        }
    }

    pub(super) fn whois_key(&mut self, key: KeyEvent) {
        let rows = self.whois_rows();
        let at = self.whois.list.selected().unwrap_or(0);
        let last = rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('I') => self.whois.open = false,
            KeyCode::Down | KeyCode::Char('j') => self.whois.list.select(Some((at + 1).min(last))),
            KeyCode::Up | KeyCode::Char('k') => self.whois.list.select(Some(at.saturating_sub(1))),
            KeyCode::Char('e') | KeyCode::Char('/') => self.open_prompt(PromptKind::Whois, "Whose address?", ""),
            KeyCode::Char('y') | KeyCode::Enter => {
                if let Some(row) = rows.get(at)
                    && let Some(address) = &row.address
                {
                    self.copy(address, &row.label);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::net::remote::destinations_of;

    #[test]
    fn an_identitys_addresses_are_as_reticulum_works_them_out() {
        let identity = rns_identity::identity::Identity::new();
        let lxmf = rns_identity::destination::Destination::hash_from_name_and_identity(crate::lxmf::LXMF_ASPECT, Some(&identity.hash));
        let all = destinations_of(identity.hash);
        assert!(all.contains(&("LXMF address", lxmf)), "{all:?}");
        assert_eq!(all.len(), 7);
        assert!(all.iter().any(|(kind, hash)| *kind == "speed test" && *hash == crate::net::speedtest::address(identity.hash)));
    }
}
