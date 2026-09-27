//! Network tab: peers, NomadNet nodes and propagation nodes heard.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::App;
use crate::net::{PeerKind, parse_hash};
use crate::store::Peer;
use crate::term::input::TextInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetFilter {
    All,
    Peers,
    Nodes,
    Propagation,
}

/// The Network tab's search box: finds peers and nodes by name or address.
#[derive(Default)]
pub struct NetSearch {
    pub input: TextInput,
    /// Keys go to the search box.
    pub typing: bool,
}

impl NetSearch {
    /// Case-folded search words. Every word must appear in the name or the
    /// address. Addresses may be pasted as `<hash>`, `lxmf@hash` or
    /// `hash:/page/index.mu`.
    pub fn terms(&self) -> Vec<Vec<char>> {
        self.input
            .text()
            .split_whitespace()
            .map(|word| {
                let word = word.trim_start_matches('<').trim_end_matches('>');
                let word = ["lxmf@", "lxmf://", "nomadnetwork://"]
                    .iter()
                    .find_map(|prefix| word.strip_prefix(prefix))
                    .unwrap_or(word);
                let word = match word.split_once(':') {
                    Some((hash, _)) if !hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit()) => hash,
                    _ => word,
                };
                fold(word)
            })
            .filter(|term| !term.is_empty())
            .collect()
    }
}

/// Lowercase one char per char, so match positions line up with the text.
fn fold(text: &str) -> Vec<char> {
    text.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

fn find_all(haystack: &[char], needle: &[char]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    (0..=haystack.len() - needle.len())
        .filter(|&i| haystack[i..i + needle.len()] == *needle)
        .collect()
}

/// Which chars of `text` are part of a match, for highlighting.
pub fn match_mask(text: &str, terms: &[Vec<char>]) -> Vec<bool> {
    let folded = fold(text);
    let mut mask = vec![false; folded.len()];
    for term in terms {
        for start in find_all(&folded, term) {
            mask[start..start + term.len()].fill(true);
        }
    }
    mask
}

fn matches(terms: &[Vec<char>], hash: &str, peer: &Peer) -> bool {
    let name = fold(peer.name.as_deref().unwrap_or_default());
    let hash = fold(hash);
    terms
        .iter()
        .all(|term| !find_all(&name, term).is_empty() || !find_all(&hash, term).is_empty())
}

impl App {
    pub fn network_rows(&self) -> Vec<(&String, &Peer)> {
        let terms = self.net_search.terms();
        let mut rows: Vec<_> = self
            .store
            .peers
            .iter()
            .filter(|(_, p)| match self.net_filter {
                NetFilter::All => true,
                NetFilter::Peers => p.kind == PeerKind::Lxmf,
                NetFilter::Nodes => p.kind == PeerKind::Nomad,
                NetFilter::Propagation => p.kind == PeerKind::Propagation,
            })
            .filter(|(hash, p)| matches(&terms, hash, p))
            .collect();
        rows.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen).then_with(|| a.0.cmp(b.0)));
        rows
    }

    fn selected_peer(&self) -> Option<(String, PeerKind)> {
        let i = self.peers.selected()?;
        self.network_rows().get(i).map(|(k, p)| ((*k).clone(), p.kind))
    }

    fn open_peer(&mut self, key: String, kind: PeerKind) {
        match kind {
            PeerKind::Lxmf => self.open_conversation(key),
            PeerKind::Nomad => {
                if let Some(location) = self.resolve(&key) {
                    self.navigate(location);
                }
            }
            PeerKind::Propagation => self.set_propagation_node(&key),
        }
    }

    pub fn set_propagation_node(&mut self, key: &str) {
        let Some(hash) = parse_hash(key) else { return };
        let key = hex::encode(hash);
        if let Err(e) = self.update_settings(&[("propagation_node", &key)]) {
            return self.fail(e);
        }
        let name = self.store.display_name(&key);
        self.notify(format!("Propagation node set to {name}"));
    }

    /// Keys while the search box has focus.
    pub(super) fn network_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.clear_network_search(),
            KeyCode::Enter => self.net_search.typing = false,
            KeyCode::Up | KeyCode::Down => self.network_key(key),
            _ => {
                if self.net_search.input.handle(key) {
                    self.network_search_changed();
                }
            }
        }
    }

    pub(super) fn paste_network_search(&mut self, text: &str) {
        self.net_search.input.insert_str(text);
        self.network_search_changed();
    }

    fn clear_network_search(&mut self) {
        self.net_search.input.take();
        self.net_search.typing = false;
        self.network_search_changed();
    }

    /// Show the best match first whenever the query changes.
    fn network_search_changed(&mut self) {
        let any = !self.network_rows().is_empty();
        self.peers = ListState::default().with_selected(any.then_some(0));
    }

    pub(super) fn network_key(&mut self, key: KeyEvent) {
        let count = self.network_rows().len();
        let selected = self.selected_peer();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                let i = self.peers.selected().map_or(0, |i| (i + 1).min(count - 1));
                self.peers.select(Some(i));
            }
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                let i = self.peers.selected().map_or(0, |i| i.saturating_sub(1));
                self.peers.select(Some(i));
            }
            KeyCode::Char('f') => {
                self.net_filter = match self.net_filter {
                    NetFilter::All => NetFilter::Peers,
                    NetFilter::Peers => NetFilter::Nodes,
                    NetFilter::Nodes => NetFilter::Propagation,
                    NetFilter::Propagation => NetFilter::All,
                };
                self.peers.select(None);
            }
            KeyCode::Char('/') => self.net_search.typing = true,
            KeyCode::Esc if !self.net_search.input.text().is_empty() => self.clear_network_search(),
            KeyCode::Enter => {
                if let Some((key, kind)) = selected {
                    self.open_peer(key, kind);
                }
            }
            KeyCode::Char('m') => {
                if let Some((key, PeerKind::Lxmf)) = selected {
                    self.open_conversation(key);
                }
            }
            KeyCode::Char('p') => {
                if let Some((key, PeerKind::Propagation)) = selected {
                    self.set_propagation_node(&key);
                }
            }
            KeyCode::Char('y') => {
                if let Some((key, _)) = selected {
                    self.copy(&key, "address");
                }
            }
            _ => {}
        }
    }

    pub(super) fn click_network(&mut self, at: Position, double: bool) {
        if self.regions.net_search.contains(at) {
            self.net_search.typing = true;
            return;
        }
        let area = self.regions.peers;
        if area.contains(at) {
            self.net_search.typing = false;
            let index = self.peers.offset() + (at.y - area.y) as usize;
            if index < self.network_rows().len() {
                self.peers.select(Some(index));
                if double && let Some((key, kind)) = self.selected_peer() {
                    self.open_peer(key, kind);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(text: &str) -> Vec<Vec<char>> {
        NetSearch {
            input: TextInput::with_text(text),
            typing: false,
        }
        .terms()
    }

    fn peer(name: Option<&str>) -> Peer {
        Peer {
            kind: PeerKind::Nomad,
            name: name.map(str::to_string),
            hops: 1,
            last_seen: 0,
        }
    }

    #[test]
    fn finds_by_name_or_address() {
        let hash = "7832ec3f38b1dee89865ddc4bfbe8b51";
        let node = peer(Some("Colorado Mesh Blog"));
        assert!(matches(&search(""), hash, &node));
        assert!(matches(&search("mesh"), hash, &node));
        assert!(matches(&search("COLORADO blog"), hash, &node));
        assert!(matches(&search("7832EC"), hash, &node));
        assert!(matches(&search("mesh 8b51"), hash, &node));
        assert!(!matches(&search("mesh nope"), hash, &node));
        assert!(!matches(&search("mesh"), hash, &peer(None)));
        // Addresses pasted in the usual forms.
        for pasted in [format!("<{hash}>"), format!("lxmf@{hash}"), format!("{hash}:/page/index.mu")] {
            assert!(matches(&search(&pasted), hash, &node), "{pasted}");
        }
    }

    #[test]
    fn mask_marks_every_match() {
        let mask = match_mask("Ärger mesh MESH", &search("mesh är"));
        let marked: String = "Ärger mesh MESH"
            .chars()
            .zip(mask)
            .map(|(c, m)| if m { c } else { '.' })
            .collect();
        assert_eq!(marked, "Är....mesh.MESH");
    }
}
