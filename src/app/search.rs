//! Searching messages: in the open conversation, or in all of them, by
//! their text, title, attachments' names and what rettui noted of them.
//! Every word must be found, as the Network search has it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::network::{match_mask, matches_text, search_terms};
use super::App;
use crate::store::{Message, Store};
use crate::term::input::TextInput;

/// Most results listed: the newest.
pub const MAX_HITS: usize = 200;

/// A message found.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Its conversation.
    pub key: String,
    pub id: String,
    /// Where it is in its conversation (0 the oldest kept).
    pub index: usize,
    /// How many newer ones follow it there.
    pub newer: usize,
    pub timestamp: f64,
    pub incoming: bool,
}

/// What of a message is searched.
fn searched(message: &Message) -> Vec<&str> {
    let mut texts = vec![message.content.as_str(), message.title.as_str()];
    texts.extend(message.attachments.iter().map(|a| a.name.as_str()));
    texts.extend(message.notes.iter().map(String::as_str));
    texts
}

/// Messages matching `query`, newest first, at most [`MAX_HITS`]: in the
/// conversation `only`, or in all of them.
pub fn search(store: &Store, query: &str, only: Option<&str>) -> Vec<Hit> {
    let terms = search_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<Hit> = store
        .conversations
        .iter()
        .filter(|(key, _)| only.is_none_or(|only| only == key.as_str()))
        .flat_map(|(key, conversation)| {
            let count = conversation.messages.len();
            conversation.messages.iter().enumerate().filter(|(_, m)| matches_text(&terms, &searched(m))).map(
                move |(index, m)| Hit {
                    key: key.clone(),
                    id: m.id.clone(),
                    index,
                    newer: count - 1 - index,
                    timestamp: m.timestamp,
                    incoming: m.incoming,
                },
            )
        })
        .collect();
    hits.sort_by(|a, b| b.timestamp.total_cmp(&a.timestamp).then_with(|| a.id.cmp(&b.id)));
    hits.truncate(MAX_HITS);
    hits
}

/// A line of a message to show with a hit: around the first match in its
/// text (or else its title, an attachment's name, a note), on one line, at
/// most `width` characters, cut with "…".
pub fn snippet(message: &Message, query: &str, width: usize) -> String {
    let terms = search_terms(query);
    let texts = searched(message);
    let text = texts
        .iter()
        .find(|t| match_mask(t, &terms).contains(&true))
        .or(texts.first())
        .copied()
        .unwrap_or_default();
    let flat: Vec<char> = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    let width = width.max(8);
    if flat.len() <= width {
        return flat.into_iter().collect();
    }
    let line: String = flat.iter().collect();
    let first = match_mask(&line, &terms).iter().position(|&m| m).unwrap_or(0);
    // A little before the match, for context.
    let start = first.saturating_sub(width / 4).min(flat.len() - width);
    let (lead, trail) = (start > 0, start + width < flat.len());
    let inner_start = start + usize::from(lead);
    let inner_end = (start + width - usize::from(trail)).max(inner_start);
    let mut out = String::new();
    if lead {
        out.push('…');
    }
    out.extend(&flat[inner_start..inner_end]);
    if trail {
        out.push('…');
    }
    out
}

/// The terminal UI's message search (`/` in Messages).
#[derive(Default)]
pub struct MessageSearch {
    pub input: TextInput,
    /// In the open conversation only (Tab switches).
    pub here: bool,
    pub selected: usize,
}

impl App {
    /// Open the message search: in all conversations, or with `here`, the
    /// open one.
    pub(super) fn open_message_search(&mut self) {
        self.message_search = Some(MessageSearch::default());
        self.composing = false;
    }

    /// What the message search finds now.
    pub fn message_search_hits(&self) -> Vec<Hit> {
        let Some(open) = &self.message_search else { return Vec::new() };
        let only = open.here.then_some(self.active_conversation.as_deref()).flatten();
        search(&self.store, open.input.text(), only)
    }

    pub(super) fn message_search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.message_search_hits().len();
        let Some(search) = self.message_search.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.message_search = None,
            KeyCode::Enter => {
                let hits = self.message_search_hits();
                let selected = self.message_search.as_ref().map_or(0, |s| s.selected);
                if let Some(hit) = hits.get(selected).cloned() {
                    self.open_hit(&hit);
                }
            }
            KeyCode::Up => search.selected = search.selected.saturating_sub(1),
            KeyCode::Down => search.selected = (search.selected + 1).min(count.saturating_sub(1)),
            KeyCode::PageUp => search.selected = search.selected.saturating_sub(10),
            KeyCode::PageDown => search.selected = (search.selected + 10).min(count.saturating_sub(1)),
            KeyCode::Tab if self.active_conversation.is_some() => {
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

    pub(super) fn paste_message_search(&mut self, text: &str) {
        if let Some(search) = self.message_search.as_mut() {
            search.input.insert_str(&text.replace(['\n', '\r'], " "));
            search.selected = 0;
        }
    }

    /// Open a hit's conversation, with the message picked (and shown).
    pub(super) fn open_hit(&mut self, hit: &Hit) {
        self.message_search = None;
        let Some(position) = self.conversation_order().iter().position(|k| *k == hit.key) else { return };
        self.select_conversation(position);
        let index = self.store.conversations.get(&hit.key).and_then(|c| c.messages.iter().position(|m| m.id == hit.id));
        if let Some(index) = index {
            self.picked = Some(hit.id.clone());
            self.scroll_to = Some(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Conversation, MessageState};

    fn message(id: &str, text: &str, timestamp: f64) -> Message {
        Message {
            id: id.into(),
            incoming: true,
            title: String::new(),
            content: text.into(),
            timestamp,
            state: MessageState::Delivered,
            ..Default::default()
        }
    }

    fn store() -> Store {
        let mut store = Store::default();
        let alice = Conversation {
            messages: vec![message("a1", "The repeater on the hill is up", 10.0), message("a2", "Bring the antenna", 30.0)],
            ..Default::default()
        };
        let bob = Conversation { messages: vec![message("b1", "Antenna tuned, the HILL site works", 20.0)], ..Default::default() };
        store.conversations.insert("aa".repeat(16), alice);
        store.conversations.insert("bb".repeat(16), bob);
        store
    }

    #[test]
    fn every_word_in_any_order_newest_first() {
        let store = store();
        let ids = |hits: Vec<Hit>| hits.into_iter().map(|h| h.id).collect::<Vec<_>>();
        assert_eq!(ids(search(&store, "antenna", None)), ["a2", "b1"]);
        assert_eq!(ids(search(&store, "hill THE", None)), ["b1", "a1"]);
        assert_eq!(ids(search(&store, "hill antenna", None)), ["b1"]);
        assert_eq!(ids(search(&store, "antenna", Some(&"aa".repeat(16)))), ["a2"]);
        assert!(search(&store, "  ", None).is_empty());
        let hit = &search(&store, "repeater", None)[0];
        assert_eq!((hit.index, hit.newer), (0, 1));
    }

    #[test]
    fn snippets_show_the_match_on_one_line() {
        let long = message("x", "A long line of text before\nthe word we want: antenna, and more text after it, quite a bit more", 0.0);
        let cut = snippet(&long, "antenna", 30);
        assert!(cut.starts_with('…') && cut.ends_with('…') && cut.contains("antenna"), "{cut}");
        assert_eq!(cut.chars().count(), 30, "{cut}");
        assert!(!cut.contains('\n'));
        assert_eq!(snippet(&message("y", "short one", 0.0), "short", 30), "short one");
        // Found in an attachment's name: that shows.
        let mut file = message("z", "see attached", 0.0);
        file.attachments.push(crate::store::StoredAttachment {
            name: "map-of-hill.png".into(),
            path: "map-of-hill.png".into(),
            size: 1,
            image: true,
            voice: None,
        });
        assert_eq!(snippet(&file, "hill", 30), "map-of-hill.png");
    }

    #[test]
    fn slash_finds_and_enter_opens_the_message() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let dir = std::env::temp_dir().join(format!("rettui-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), store());
        app.tab = crate::app::Tab::Messages;
        app.open_newest_conversation();
        let press = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        press(&mut app, KeyCode::Char('/'));
        assert!(app.message_search.is_some());
        for c in "hill".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        // Keys go to the box, not the tab: no quitting on `q`, say.
        assert_eq!(app.message_search_hits().iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["b1", "a1"]);
        // Tab: in the open conversation (alice's, the newest) only.
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.message_search_hits().iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["a1"]);
        press(&mut app, KeyCode::Tab);
        // The second: alice's first message, opened and picked.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(app.message_search.is_none());
        assert_eq!(app.active_conversation.as_deref(), Some("aa".repeat(16).as_str()));
        assert_eq!((app.picked.as_deref(), app.scroll_to), (Some("a1"), Some(0)));
        // Esc closes it.
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Esc);
        assert!(app.message_search.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

