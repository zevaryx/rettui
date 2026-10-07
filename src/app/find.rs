//! Finding text in the page the Browser shows (`f`, or Ctrl-F): its rows as
//! drawn are searched (case aside), every match is marked, and the current
//! one is scrolled to.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

use super::App;
use super::network::{find_all, fold};
use crate::term::input::TextInput;

/// What's being looked for, and which match is current.
#[derive(Default)]
pub struct PageFind {
    pub input: TextInput,
    pub current: usize,
}

/// A match: its row of the page, and its first and last-but-one column.
pub type Found = (usize, usize, usize);

/// Where `query` is found in `rows` (case aside), in order.
pub fn find_in(rows: &[String], query: &str) -> Vec<Found> {
    let needle = fold(query);
    if needle.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (row, text) in rows.iter().enumerate() {
        let chars: Vec<char> = text.chars().collect();
        for at in find_all(&fold(text), &needle) {
            let before: String = chars[..at].iter().collect();
            let hit: String = chars[at..at + needle.len()].iter().collect();
            let start = before.width();
            found.push((row, start, start + hit.width()));
        }
    }
    found
}

impl App {
    /// Where what's typed is found in the page shown.
    pub fn page_matches(&self) -> Vec<Found> {
        let Some(find) = &self.browser.find else { return Vec::new() };
        find_in(&self.regions.page_text, find.input.text())
    }

    pub(super) fn open_find(&mut self) {
        if self.browser.page.is_none() && !self.browser.view_source {
            return self.warn("No page to find in");
        }
        self.browser.focus = super::BrowserFocus::Page;
        if self.browser.find.is_none() {
            self.browser.find = Some(PageFind::default());
        }
    }

    /// Show a match: the first from the top of the view on (`step` 0),
    /// or the next or previous one, round the page.
    fn show_match(&mut self, step: isize) {
        let found = self.page_matches();
        let scroll = self.browser.scroll;
        let viewport = self.browser.viewport.max(1);
        let Some(find) = self.browser.find.as_mut() else { return };
        if found.is_empty() {
            find.current = 0;
            return;
        }
        find.current = match step {
            0 => found.iter().position(|(row, ..)| *row >= scroll).unwrap_or(0),
            _ => (find.current as isize + step).rem_euclid(found.len() as isize) as usize,
        };
        let row = found[find.current].0;
        // In view, a little below the top.
        if row < scroll || row >= scroll + viewport {
            self.browser.scroll = row.saturating_sub(viewport / 3);
        }
    }

    pub(super) fn find_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(find) = self.browser.find.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.browser.find = None,
            KeyCode::Enter | KeyCode::Down => self.show_match(1),
            KeyCode::Up => self.show_match(-1),
            KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
            _ => {
                if find.input.handle(key) {
                    self.show_match(0);
                }
            }
        }
    }

    pub(super) fn paste_find(&mut self, text: &str) {
        if let Some(find) = self.browser.find.as_mut() {
            find.input.insert_str(&text.replace(['\n', '\r'], " "));
            self.show_match(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_row_and_column_case_aside() {
        let rows = vec!["Welcome to the node".to_string(), "  NODE status: up, node two".to_string(), "日本 node".to_string()];
        assert_eq!(find_in(&rows, "node"), [(0, 15, 19), (1, 2, 6), (1, 19, 23), (2, 5, 9)]);
        assert!(find_in(&rows, "").is_empty());
        assert!(find_in(&rows, "absent").is_empty());
    }

    #[test]
    fn f_finds_and_scrolls_to_each_match() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let dir = std::env::temp_dir().join(format!("rettui-find-{}", std::process::id()));
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        app.tab = crate::app::Tab::Browser;
        app.browser.page = Some(crate::nomad::micron::parse("x"));
        app.browser.viewport = 10;
        app.regions.page_text = (0..100).map(|i| if i % 30 == 5 { format!("row {i}: the Tower") } else { format!("row {i}") }).collect();
        let press = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        press(&mut app, KeyCode::Char('f'));
        for c in "tower".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        // Rows 5, 35, 65, 95; the first one shown.
        assert_eq!(app.page_matches().iter().map(|m| m.0).collect::<Vec<_>>(), [5, 35, 65, 95]);
        assert_eq!(app.browser.find.as_ref().unwrap().current, 0);
        press(&mut app, KeyCode::Enter);
        assert_eq!((app.browser.find.as_ref().unwrap().current, app.browser.scroll), (1, 32));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        // Round to the last.
        assert_eq!((app.browser.find.as_ref().unwrap().current, app.browser.scroll), (3, 92));
        // Keys go to the box: `b` is typed, not "back".
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.browser.find.as_ref().unwrap().input.text(), "towerb");
        press(&mut app, KeyCode::Esc);
        assert!(app.browser.find.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
