//! Network tab: peers, NomadNet nodes and propagation nodes heard.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::App;
use crate::net::autopn::Pick;
use crate::net::{Hash, NetCommand, PeerKind, parse_hash};
use crate::store::{Peer, Trust};
use crate::term::input::TextInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetFilter {
    All,
    Peers,
    Nodes,
    Propagation,
    /// Contacts you've blocked, heard or not.
    Blocked,
}

fn hops_label(hops: u8) -> String {
    format!("{hops} hop{}", if hops == 1 { "" } else { "s" })
}

/// A blocked contact not heard announcing (blocking stops their announces).
pub static UNHEARD: Peer = Peer { kind: PeerKind::Lxmf, name: None, hops: 0, last_seen: 0 };

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
        search_terms(self.input.text())
    }
}

/// Case-folded search words (see [`NetSearch::terms`]); the web UI's
/// Network search uses the same rules.
pub fn search_terms(query: &str) -> Vec<Vec<char>> {
    query
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

/// Whether a peer's name or address contains every search word.
pub fn matches(terms: &[Vec<char>], hash: &str, peer: &Peer) -> bool {
    if terms.is_empty() {
        return true;
    }
    let name = fold(peer.name.as_deref().unwrap_or_default());
    let hash = fold(hash);
    terms
        .iter()
        .all(|term| !find_all(&name, term).is_empty() || !find_all(&hash, term).is_empty())
}

impl App {
    pub fn network_rows(&self) -> Vec<(&String, &Peer)> {
        let terms = self.net_search.terms();
        if self.net_filter == NetFilter::Blocked {
            let blocked = self.store.contacts.iter().filter(|(_, c)| c.trust == Trust::Blocked);
            let mut rows: Vec<_> =
                blocked.map(|(hash, _)| (hash, self.store.peers.get(hash).unwrap_or(&UNHEARD))).filter(|(hash, p)| matches(&terms, hash, p)).collect();
            rows.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen).then_with(|| a.0.cmp(b.0)));
            return rows;
        }
        let mut rows: Vec<_> = self
            .store
            .peers
            .iter()
            .filter(|(_, p)| match self.net_filter {
                NetFilter::All => true,
                NetFilter::Peers => p.kind == PeerKind::Lxmf,
                NetFilter::Nodes => p.kind == PeerKind::Nomad,
                NetFilter::Propagation => p.kind == PeerKind::Propagation,
                NetFilter::Blocked => false,
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
        // Picking one by hand stops picking them automatically.
        let auto = self.settings.auto_propagation_node;
        let changes: &[(&str, &str)] = if auto {
            &[("propagation_node", &key), ("auto_propagation_node", "false")]
        } else {
            &[("propagation_node", &key)]
        };
        if let Err(e) = self.update_settings(changes) {
            return self.fail(e);
        }
        let name = self.store.display_name(&key);
        let off = if auto { " (picking one automatically is off now)" } else { "" };
        self.notify(format!("Propagation node set to {name}{off}"));
    }

    /// The propagation nodes heard: address, hops, when last heard.
    fn heard_propagation_nodes(&self) -> Vec<(Hash, u8, i64)> {
        self.store
            .peers
            .iter()
            .filter(|(_, peer)| peer.kind == PeerKind::Propagation)
            .filter_map(|(key, peer)| Some((parse_hash(key)?, peer.hops, peer.last_seen)))
            .collect()
    }

    /// Start (with the nodes heard before) or stop picking the propagation
    /// node automatically, as set.
    pub(super) fn apply_auto_propagation(&mut self) {
        let enabled = self.settings.auto_propagation_node;
        let known = if enabled { self.heard_propagation_nodes() } else { Vec::new() };
        self.auto_pick = None;
        self.auto_error = None;
        self.send(NetCommand::AutoPropagation { enabled, known });
    }

    /// A propagation node picked automatically: used, kept in the settings
    /// (for the next start), and said in the log when it's another.
    pub(super) fn on_propagation_picked(&mut self, result: Result<Pick, String>) {
        if !self.settings.auto_propagation_node {
            return;
        }
        let pick = match result {
            Ok(pick) => pick,
            Err(e) => {
                // Said once, not every ten minutes.
                if self.auto_error.as_ref() != Some(&e) {
                    self.log(format!("Couldn't pick a propagation node: {e}; trying again in 10 minutes"));
                    self.auto_error = Some(e);
                }
                return;
            }
        };
        self.auto_error = None;
        let key = hex::encode(pick.node);
        if self.settings.propagation_node.as_deref() != Some(key.as_str()) {
            self.settings.propagation_node = Some(key.clone());
            match self.saved_settings() {
                Ok(mut saved) => {
                    saved.propagation_node = Some(key.clone());
                    if let Err(e) = saved.save(&self.paths.settings) {
                        self.log(format!("Could not save settings: {e:#}"));
                    }
                    self.settings_file = saved;
                }
                Err(e) => self.log(e),
            }
        }
        if pick.changed {
            self.log(format!(
                "Picked {} as propagation node automatically: {}, answered in {} ms ({} of {} nearest answered)",
                self.store.display_name(&key),
                hops_label(pick.hops),
                pick.rtt.as_millis(),
                pick.answered,
                pick.probed,
            ));
        }
        self.auto_pick = Some(pick);
    }

    /// How the propagation node in use was picked, if automatically, for
    /// the Status tab: `picked automatically (2 hops, 40 ms)`.
    pub fn auto_pick_label(&self) -> Option<String> {
        if !self.settings.auto_propagation_node {
            return None;
        }
        Some(match &self.auto_pick {
            Some(pick) => format!("picked automatically ({}, {} ms)", hops_label(pick.hops), pick.rtt.as_millis()),
            None if self.settings.propagation_node.is_some() => "picked automatically (checking it)".into(),
            None => "picking one automatically…".into(),
        })
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
        // One pass over the (possibly thousands of) rows for both.
        let (count, selected) = {
            let rows = self.network_rows();
            let selected = self.peers.selected().and_then(|i| rows.get(i)).map(|(k, p)| ((*k).clone(), p.kind));
            (rows.len(), selected)
        };
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
                    NetFilter::Propagation => NetFilter::Blocked,
                    NetFilter::Blocked => NetFilter::All,
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
            // Block an LXMF peer, or unblock one.
            KeyCode::Char('b') => {
                if let Some((key, PeerKind::Lxmf)) = selected {
                    if self.store.contact(&key).trust == Trust::Blocked {
                        match self.unblock_contact(&key) {
                            Ok(()) => self.notify(format!("Unblocked {}", self.store.display_name(&key))),
                            Err(e) => self.warn(e),
                        }
                    } else {
                        self.ask_block(key);
                    }
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
    fn picking_the_propagation_node_automatically() {
        use std::time::Duration;
        use crate::config::Settings;
        use crate::net::NetEvent;
        use crate::net::autopn::Pick;
        use crate::store::Store;
        let dir = std::env::temp_dir().join(format!("rettui-autopn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (near, far) = ("aa".repeat(16), "bb".repeat(16));
        let mut store = Store::default();
        store.peers.insert(near.clone(), Peer { kind: PeerKind::Propagation, name: Some("Near".into()), hops: 2, last_seen: 100 });
        store.peers.insert(far.clone(), Peer { kind: PeerKind::Propagation, name: None, hops: 5, last_seen: 90 });
        store.peers.insert("cc".repeat(16), peer(Some("a page node")));
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, Settings::default(), store);
        let commands = |net: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>| std::iter::from_fn(|| net.try_recv().ok()).collect::<Vec<_>>();
        // Turning it on warns, and starts from the nodes heard before.
        let notes = app.update_settings(&[("auto_propagation_node", "true")]).unwrap();
        assert!(notes.iter().any(|n| n.contains("could lose them")), "{notes:?}");
        let started = commands(&mut net).into_iter().find_map(|c| match c {
            NetCommand::AutoPropagation { enabled: true, mut known } => {
                known.sort();
                Some(known)
            }
            _ => None,
        });
        assert_eq!(started, Some(vec![([0xaa; 16], 2, 100), ([0xbb; 16], 5, 90)]));
        assert_eq!(app.auto_pick_label().as_deref(), Some("picking one automatically…"));
        // A pick is used and kept for the next start.
        let pick = Pick { node: [0xaa; 16], hops: 2, rtt: Duration::from_millis(40), changed: true, probed: 2, answered: 2 };
        app.on_net(NetEvent::PropagationPicked(Ok(pick)));
        assert_eq!(app.saved_settings().unwrap().propagation_node.as_deref(), Some(near.as_str()));
        assert!(app.log.iter().any(|l| l.contains("Picked Near as propagation node automatically: 2 hops, answered in 40 ms")));
        assert_eq!(app.auto_pick_label().as_deref(), Some("picked automatically (2 hops, 40 ms)"));
        // Failing to sync with it asks for a check; why none was picked is
        // said once.
        app.on_net(NetEvent::Synced(Err("Link failed".into())));
        assert!(commands(&mut net).iter().any(|c| matches!(c, NetCommand::RecheckPropagation)));
        for _ in 0..3 {
            app.on_net(NetEvent::PropagationPicked(Err("none of the 2 nearest answered".into())));
        }
        assert_eq!(app.log.iter().filter(|l| l.contains("Couldn't pick a propagation node")).count(), 1);
        // Setting one in the settings editor (either UI) turns it off.
        let notes = app.update_settings(&[("propagation_node", &far)]).unwrap();
        assert!(notes.iter().any(|n| n.contains("automatically is off now")), "{notes:?}");
        let saved = app.saved_settings().unwrap();
        assert_eq!((saved.auto_propagation_node, saved.propagation_node.as_deref()), (false, Some(far.as_str())));
        assert!(commands(&mut net).iter().any(|c| matches!(c, NetCommand::AutoPropagation { enabled: false, .. })));
        // Unless the same change turns it on.
        app.update_settings(&[("propagation_node", &near), ("auto_propagation_node", "true")]).unwrap();
        assert!(app.saved_settings().unwrap().auto_propagation_node);
        // Picking one by hand turns it off.
        app.set_propagation_node(&far);
        let saved = app.saved_settings().unwrap();
        assert_eq!((saved.auto_propagation_node, saved.propagation_node.as_deref()), (false, Some(far.as_str())));
        assert!(commands(&mut net).iter().any(|c| matches!(c, NetCommand::AutoPropagation { enabled: false, .. })));
        assert_eq!(app.auto_pick_label(), None);
        std::fs::remove_dir_all(&dir).unwrap();
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
