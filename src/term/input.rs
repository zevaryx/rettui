//! Single-line text input with a cursor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Default, Clone)]
pub struct TextInput {
    text: String,
    /// Cursor position in chars.
    cursor: usize,
    /// The other end of the selection, in chars, when text is selected
    /// (only [`TextInput::handle_selecting`] starts one).
    anchor: Option<usize>,
}

impl TextInput {
    pub fn with_text(text: &str) -> Self {
        Self { text: text.to_string(), cursor: text.chars().count(), anchor: None }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.anchor = None;
        std::mem::take(&mut self.text)
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.text.char_indices().nth(chars).map_or(self.text.len(), |(i, _)| i)
    }

    /// Insert pasted text at the cursor (in place of the selection, if
    /// any). Inputs are single-line, so line breaks become spaces and other
    /// control characters are dropped.
    pub fn insert_str(&mut self, text: &str) {
        self.delete_selection();
        let clean: String = text
            .replace("\r\n", " ")
            .chars()
            .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        let at = self.byte_index(self.cursor);
        self.text.insert_str(at, &clean);
        self.cursor += clean.chars().count();
    }

    /// Display column of the cursor.
    pub fn cursor_column(&self) -> usize {
        self.text[..self.byte_index(self.cursor)].width()
    }

    /// The text before the cursor.
    pub fn before_cursor(&self) -> &str {
        &self.text[..self.byte_index(self.cursor)]
    }

    /// Replace the bytes `start..end` with `with`, leaving the cursor
    /// after it.
    pub fn replace(&mut self, start: usize, end: usize, with: &str) {
        self.text.replace_range(start..end, with);
        self.anchor = None;
        self.cursor = self.text[..start + with.len()].chars().count();
    }

    /// The cursor's place in chars.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selected range in chars (start first), when something is
    /// selected.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor.filter(|a| *a != self.cursor)?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    /// Put in new text, the cursor and the selection's other end (in
    /// chars, clamped to the text).
    pub fn set(&mut self, text: &str, cursor: usize, anchor: Option<usize>) {
        let len = text.chars().count();
        self.text = text.to_string();
        self.cursor = cursor.min(len);
        self.anchor = anchor.map(|a| a.min(len));
    }

    /// Remove the selected text, leaving the cursor where it was.
    fn delete_selection(&mut self) {
        if let Some((start, end)) = self.selection() {
            let (from, to) = (self.byte_index(start), self.byte_index(end));
            self.text.replace_range(from..to, "");
            self.cursor = start;
        }
        self.anchor = None;
    }

    /// Like [`TextInput::handle`], with a selection: `Shift` with Left,
    /// Right, Home or End selects, and typing, Backspace or Delete take
    /// the selected text's place.
    pub fn handle_selecting(&mut self, key: KeyEvent) -> bool {
        let moving = matches!(key.code, KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End);
        if moving && key.modifiers.contains(KeyModifiers::SHIFT) {
            let anchor = self.anchor.unwrap_or(self.cursor);
            self.handle(KeyEvent::new(key.code, KeyModifiers::NONE));
            self.anchor = Some(anchor);
            return true;
        }
        if self.selection().is_some() {
            match key.code {
                KeyCode::Backspace | KeyCode::Delete => {
                    self.delete_selection();
                    return true;
                }
                KeyCode::Char(_) if !key.modifiers.contains(KeyModifiers::CONTROL) => self.delete_selection(),
                _ => {}
            }
        }
        self.handle(key)
    }

    /// Returns true when the key was consumed. Any key ends a selection.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        self.anchor = None;
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let at = self.byte_index(self.cursor);
                self.text.drain(..at);
                self.cursor = 0;
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => self.cursor = 0,
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => self.cursor = len,
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let at = self.byte_index(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_index(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_index(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn shift_selects_and_typing_replaces() {
        let mut input = TextInput::with_text("hello world");
        input.handle_selecting(key(KeyCode::Home, KeyModifiers::NONE));
        for _ in 0..5 {
            input.handle_selecting(key(KeyCode::Right, KeyModifiers::SHIFT));
        }
        assert_eq!(input.selection(), Some((0, 5)));
        // Back to where it started: nothing selected.
        for _ in 0..5 {
            input.handle_selecting(key(KeyCode::Left, KeyModifiers::SHIFT));
        }
        assert_eq!(input.selection(), None);
        input.handle_selecting(key(KeyCode::End, KeyModifiers::SHIFT));
        assert_eq!(input.selection(), Some((0, 11)));
        // A plain move lets go; typing replaces what's selected.
        input.handle_selecting(key(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(input.selection(), None);
        input.set("hello world", 5, Some(0));
        input.handle_selecting(key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!((input.text(), input.cursor(), input.selection()), ("j world", 1, None));
        input.set("héllo", 4, Some(1));
        input.handle_selecting(key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!((input.text(), input.cursor()), ("ho", 1));
        input.set("abc", 1, Some(3));
        input.handle_selecting(key(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!((input.text(), input.cursor()), ("a", 1));
        // Pasting replaces it too; clamped places are safe.
        input.set("abc", 0, Some(2));
        input.insert_str("xy");
        assert_eq!(input.text(), "xyc");
        input.set("ab", 9, Some(9));
        assert_eq!((input.cursor(), input.selection()), (2, None));
        // Taking the text leaves nothing selected.
        input.set("ab", 2, Some(0));
        assert_eq!(input.take(), "ab");
        assert_eq!(input.selection(), None);
    }
}
