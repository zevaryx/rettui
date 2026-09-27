//! Single-line text input with a cursor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Default, Clone)]
pub struct TextInput {
    text: String,
    /// Cursor position in chars.
    cursor: usize,
}

impl TextInput {
    pub fn with_text(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.chars().count(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Insert pasted text at the cursor. Inputs are single-line, so line
    /// breaks become spaces and other control characters are dropped.
    pub fn insert_str(&mut self, text: &str) {
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

    /// Returns true when the key was consumed.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
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
