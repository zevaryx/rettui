//! Who may see what's on the node hosted here (`w` in the Node tab): a
//! page's, picture's or file's `.allowed` list beside it, of the identities
//! that may have it, as NomadNet keeps it. The node reads the list for each
//! request, so a change applies at once; a visitor not on it, or not
//! identified, is refused.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::App;
use crate::net::{Hash, parse_hash};
use crate::nomad::pages::{Allowed, Root};
use crate::store::Trust;

/// Most identities a list may have.
pub const MAX_LISTED: usize = 1000;

/// Someone who may be let see it.
#[derive(Debug, Clone, PartialEq)]
pub struct Viewer {
    /// Their identity's hash, as the list has it.
    pub identity: Hash,
    pub name: String,
    /// Their LXMF address, if they're someone rettui knows.
    pub key: Option<String>,
    pub on: bool,
}

/// The TUI's list of who may see something.
#[derive(Debug, Clone)]
pub struct AllowedView {
    pub root: Root,
    pub path: String,
    /// No list: anyone may.
    pub everyone: bool,
    pub people: Vec<Viewer>,
    /// Row 0 is "everyone"; then the people.
    pub list: ListState,
}

impl App {
    /// The identity behind an LXMF address, once its key has been heard
    /// (only the key that is the address's own).
    pub(super) fn identity_of(&self, key: &str) -> Option<Hash> {
        let address = parse_hash(key)?;
        let public_key = self.known.as_ref()?.lock().ok()?.public_key_of(address)?;
        let identity = rns_identity::identity::Identity::from_public_key(&public_key).ok()?;
        let own = rns_identity::destination::Destination::hash_from_name_and_identity(crate::lxmf::LXMF_ASPECT, Some(&identity.hash));
        (own == address).then_some(identity.hash)
    }

    /// Who may see `path`, and who could be let: your contacts and those
    /// you've talked with (whose keys are known), and anyone else on the
    /// list, by identity.
    pub fn node_allowed(&self, root: Root, path: &str) -> Result<(Allowed, Vec<Viewer>), String> {
        let allowed = self.node_store()?.allowed(root, path)?;
        let listed: &[Hash] = match &allowed {
            Allowed::Only(hashes) => hashes,
            _ => &[],
        };
        let mut keys: Vec<&String> = self.store.contacts.keys().chain(self.store.conversations.keys()).collect();
        keys.sort();
        keys.dedup();
        let mut people: Vec<Viewer> = keys
            .into_iter()
            .filter(|key| self.store.contact(key).trust != Trust::Blocked)
            .filter_map(|key| {
                let identity = self.identity_of(key)?;
                Some(Viewer { identity, name: self.store.display_name(key), key: Some(key.clone()), on: listed.contains(&identity) })
            })
            .collect();
        for identity in listed {
            if !people.iter().any(|p| p.identity == *identity) {
                people.push(Viewer { identity: *identity, name: format!("<{}>", &hex::encode(identity)[..12]), key: None, on: true });
            }
        }
        people.sort_by(|a, b| b.on.cmp(&a.on).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok((allowed, people))
    }

    /// Let anyone see `path` (`None`), or only these identities.
    pub fn set_node_allowed(&mut self, root: Root, path: &str, only: Option<Vec<Hash>>) -> Result<(), String> {
        if only.as_ref().is_some_and(|only| only.len() > MAX_LISTED) {
            return Err(format!("A list has at most {MAX_LISTED} people"));
        }
        self.node_store()?.set_allowed(root, path, only.as_deref())?;
        self.log(match &only {
            None => format!("Anyone may see {path} now"),
            Some(only) if only.is_empty() => format!("No one may see {path} now (you still can, in the Browser)"),
            Some(only) => format!("Only {} {} may see {path} now", only.len(), if only.len() == 1 { "person" } else { "people" }),
        });
        Ok(())
    }

    // ---- TUI ---------------------------------------------------------------

    pub(super) fn open_allowed(&mut self) {
        let Some((root, path, false)) = self.selected_item() else { return };
        match self.node_allowed(root, &path) {
            Ok((Allowed::Script, _)) => self.warn(format!("{path}'s list of who may see it is a program; change it on the host")),
            Ok((allowed, people)) => {
                let everyone = allowed == Allowed::Everyone;
                self.node.allowed = Some(AllowedView { root, path, everyone, people, list: ListState::default().with_selected(Some(0)) });
            }
            Err(e) => self.warn(e),
        }
    }

    fn toggle_allowed_row(&mut self, row: usize) {
        let Some(view) = self.node.allowed.as_mut() else { return };
        match row.checked_sub(1) {
            None => view.everyone = !view.everyone,
            Some(i) => {
                if let Some(person) = view.people.get_mut(i) {
                    person.on = !person.on;
                    // Picking someone keeps it to those picked.
                    view.everyone = false;
                }
            }
        }
    }

    pub(super) fn allowed_key(&mut self, key: KeyEvent) {
        let Some(view) = self.node.allowed.as_mut() else { return };
        let rows = view.people.len() + 1;
        let at = view.list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc => self.node.allowed = None,
            KeyCode::Down | KeyCode::Char('j') => view.list.select(Some((at + 1).min(rows - 1))),
            KeyCode::Up | KeyCode::Char('k') => view.list.select(Some(at.saturating_sub(1))),
            KeyCode::Home => view.list.select(Some(0)),
            KeyCode::End => view.list.select(Some(rows - 1)),
            KeyCode::Char(' ') => self.toggle_allowed_row(at),
            KeyCode::Enter => {
                let Some(view) = self.node.allowed.take() else { return };
                let only = (!view.everyone).then(|| view.people.iter().filter(|p| p.on).map(|p| p.identity).collect());
                match self.set_node_allowed(view.root, &view.path, only) {
                    Ok(()) => {
                        let said = self.log.back().cloned().unwrap_or_default();
                        self.confirm(said);
                    }
                    Err(e) => self.fail(e),
                }
            }
            _ => {}
        }
    }

    pub(super) fn click_allowed(&mut self, at: Position) {
        let area = self.regions.node_allowed;
        let Some(view) = self.node.allowed.as_mut() else { return };
        if !area.contains(at) {
            self.node.allowed = None;
            return;
        }
        let row = view.list.offset() + (at.y - area.y) as usize;
        if row <= view.people.len() {
            view.list.select(Some(row));
            self.toggle_allowed_row(row);
        }
    }
}
