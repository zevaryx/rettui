//! Multi-line text editor state (the page and config editors): cursor
//! movement, selection, editing, paste, undo/redo, and optional soft
//! wrapping. Drawing
//! is left to the UI, which calls [`TextArea::scroll_to_cursor`] (or
//! [`TextArea::scroll_wrapped`]) with the visible size first.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Undo steps kept.
const MAX_UNDO: usize = 200;
const TAB: &str = "    ";

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Other,
}

#[derive(Clone)]
struct Snapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

pub struct TextArea {
    lines: Vec<String>,
    /// Cursor line and position in chars.
    row: usize,
    col: usize,
    /// The other end of the selection, when there is one (the cursor is
    /// the end that moves).
    anchor: Option<(usize, usize)>,
    /// First visible line (or wrapped row, when wrapping) and first
    /// visible display column (always 0 when wrapping).
    pub top: usize,
    pub left: usize,
    /// Wrap width in columns at the last draw, when wrapping.
    wrap: Option<usize>,
    /// Visible lines at the last draw (for Page Up/Down).
    height: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<EditKind>,
    /// The text's highlighting at the last draw, and the text it was for:
    /// the editor reuses it while the text is unchanged (cursor moves,
    /// ticks), instead of highlighting the whole text on every frame.
    pub highlight_cache: Option<(String, Highlighted)>,
}

fn byte_at(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}

/// Styled pieces of each line of a text.
pub type Highlighted = Vec<Vec<(ratatui::style::Style, String)>>;

/// Display width of the first `col` chars (a grapheme counts once started).
pub fn display_width(line: &str, col: usize) -> usize {
    graphemes(line).iter().take_while(|g| g.0 < col).map(|g| g.2).sum()
}

/// A line's characters as drawn: each grapheme (an emoji with its modifiers
/// or joined parts is one) as its first char index, its length in chars and
/// its display width. Terminals draw a grapheme as one unit, so widths are
/// measured per grapheme; control characters are shown as one-column
/// symbols.
pub fn graphemes(line: &str) -> Vec<(usize, usize, usize)> {
    if line.is_ascii() {
        // Each ASCII char is its own grapheme (most lines; much quicker).
        return line.chars().enumerate().map(|(i, c)| (i, 1, c.width().unwrap_or(1))).collect();
    }
    let mut at = 0;
    line.graphemes(true)
        .map(|g| {
            let chars = g.chars().count();
            let width = if g.chars().any(char::is_control) {
                g.chars().map(|c| c.width().unwrap_or(1)).sum()
            } else {
                g.width()
            };
            let entry = (at, chars, width);
            at += chars;
            entry
        })
        .collect()
}

impl TextArea {
    pub fn new(text: &str) -> Self {
        let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            lines,
            row: 0,
            col: 0,
            anchor: None,
            top: 0,
            left: 0,
            height: 1,
            wrap: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            highlight_cache: None,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Cursor as (line, char column).
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines[row].chars().count()
    }

    /// The selected range as (start, end), start first; `None` when
    /// nothing is selected.
    pub fn selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.anchor?;
        let cursor = (self.row, self.col);
        match anchor.cmp(&cursor) {
            std::cmp::Ordering::Less => Some((anchor, cursor)),
            std::cmp::Ordering::Greater => Some((cursor, anchor)),
            std::cmp::Ordering::Equal => None,
        }
    }

    pub fn selected_text(&self) -> String {
        let Some(((r0, c0), (r1, c1))) = self.selection() else { return String::new() };
        if r0 == r1 {
            return self.lines[r0].chars().skip(c0).take(c1 - c0).collect();
        }
        let mut out: String = self.lines[r0].chars().skip(c0).collect();
        for line in &self.lines[r0 + 1..r1] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.extend(self.lines[r1].chars().take(c1));
        out
    }

    /// Select from `from` to `to` (the cursor goes to `to`); both clamped.
    pub fn select(&mut self, from: (usize, usize), to: (usize, usize)) {
        self.set_cursor(from.0, from.1);
        let anchor = (self.row, self.col);
        self.set_cursor(to.0, to.1);
        self.anchor = Some(anchor);
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Drag the cursor to a place, selecting from where it was (for mouse
    /// drags: the press placed the cursor).
    pub fn extend_to(&mut self, row: usize, col: usize) {
        if self.anchor.is_none() {
            self.anchor = Some((self.row, self.col));
        }
        self.set_cursor(row, col);
    }

    /// Remove the selected text (no undo step of its own).
    fn remove_selection(&mut self) -> bool {
        let Some(((r0, c0), (r1, c1))) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let tail: String = self.lines[r1].chars().skip(c1).collect();
        let head: String = self.lines[r0].chars().take(c0).collect();
        self.lines.splice(r0..=r1, [format!("{head}{tail}")]);
        self.row = r0;
        self.col = c0;
        self.anchor = None;
        true
    }

    /// Replace the selection (or insert at the cursor) with `text`, as one
    /// undo step; the cursor ends after it.
    pub fn replace_selection(&mut self, text: &str) {
        self.checkpoint(EditKind::Other);
        self.remove_selection();
        self.insert_text(text);
        self.last_edit = Some(EditKind::Other);
    }

    /// Replace lines `first..=last` with `lines`, as one undo step. The
    /// selection is dropped and the cursor kept on its line where it can be.
    pub fn replace_lines(&mut self, first: usize, last: usize, lines: Vec<String>) {
        self.checkpoint(EditKind::Other);
        let last = last.min(self.lines.len() - 1);
        self.lines.splice(first..=last, lines);
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.anchor = None;
        self.set_cursor(self.row, self.col);
        self.last_edit = Some(EditKind::Other);
    }

    /// Place the cursor (clamped), e.g. from a mouse click.
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.row = row.min(self.lines.len() - 1);
        self.col = col.min(self.line_len(self.row));
        self.last_edit = None;
    }

    /// Char column under a display column of a line (for clicks).
    pub fn col_at(&self, row: usize, display_col: usize) -> usize {
        let Some(line) = self.lines.get(row) else { return 0 };
        let mut width = 0;
        for (start, _, w) in graphemes(line) {
            width += w;
            if width > display_col {
                return start;
            }
        }
        line.chars().count()
    }

    /// Keep the cursor inside a `height` x `width` view.
    pub fn scroll_to_cursor(&mut self, height: usize, width: usize) {
        self.wrap = None;
        self.height = height.max(1);
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + self.height {
            self.top = self.row + 1 - self.height;
        }
        let x = display_width(&self.lines[self.row], self.col);
        let width = width.max(2);
        if x < self.left {
            self.left = x.saturating_sub(width / 4);
        } else if x >= self.left + width {
            self.left = x + 1 - width + width / 4;
        }
    }

    /// Keep the cursor inside a `height` x `width` view with long lines
    /// wrapped. One column is kept free so the cursor fits after a full row.
    pub fn scroll_wrapped(&mut self, height: usize, width: usize) {
        let wrap = width.saturating_sub(1).max(1);
        self.wrap = Some(wrap);
        self.height = height.max(1);
        self.left = 0;
        let (row, _) = self.visual_cursor(wrap);
        if row < self.top {
            self.top = row;
        } else if row >= self.top + self.height {
            self.top = row + 1 - self.height;
        }
    }

    /// Where each line breaks when wrapped to `width` columns: rows of
    /// (line, first char, end char). Lines break after a space when one
    /// fits, else mid-word; an empty line is one row.
    pub fn wrapped_rows(&self, width: usize) -> Vec<(usize, usize, usize)> {
        let width = width.max(1);
        let mut rows = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            // Rows start and end at char indexes, between graphemes.
            let cells = graphemes(line);
            let chars: Vec<char> = line.chars().collect();
            let total = chars.len();
            let (mut start, mut used) = (0, 0);
            // Char index just after the last space in the current row.
            let mut after_space = None;
            let mut i = 0;
            while i < cells.len() {
                let (at, len, w) = cells[i];
                if used + w > width && at > start {
                    let end = after_space.filter(|&e| e > start).unwrap_or(at);
                    rows.push((index, start, end));
                    start = end;
                    used = cells.iter().filter(|c| c.0 >= start && c.0 < at).map(|c| c.2).sum();
                    after_space = None;
                    continue;
                }
                used += w;
                if len == 1 && chars[at] == ' ' {
                    after_space = Some(at + 1);
                }
                i += 1;
            }
            rows.push((index, start, total));
        }
        rows
    }

    /// Rows the text takes in the view as last drawn (lines, or wrapped
    /// rows when wrapping); `top` counts in the same rows.
    pub fn row_count(&self) -> usize {
        self.wrap.map_or(self.lines.len(), |width| self.wrapped_rows(width).len())
    }

    /// The cursor's wrapped row and display column.
    fn visual_cursor(&self, width: usize) -> (usize, usize) {
        let rows = self.wrapped_rows(width);
        let index = rows
            .iter()
            .enumerate()
            .find(|(i, (line, start, end))| {
                let last = rows.get(i + 1).is_none_or(|next| next.0 != *line);
                *line == self.row && self.col >= *start && (self.col < *end || (last && self.col == *end))
            })
            .map_or(0, |(i, _)| i);
        let (_, start, _) = rows[index];
        let line = &self.lines[self.row];
        let x = display_width(line, self.col) - display_width(line, start);
        (index, x)
    }

    /// Move the cursor by wrapped rows, keeping its column where it can.
    fn move_visual(&mut self, width: usize, delta: isize) {
        let (row, x) = self.visual_cursor(width);
        let rows = self.wrapped_rows(width);
        let target = row.saturating_add_signed(delta).min(rows.len() - 1);
        let (line, start, end) = rows[target];
        let text = &self.lines[line];
        let base = display_width(text, start);
        let mut col = self.col_at(line, base + x).clamp(start, end);
        // Stay on this row rather than jumping to the start of the next.
        let last = rows.get(target + 1).is_none_or(|next| next.0 != line);
        if col == end && !last && end > start {
            col = end - 1;
        }
        self.row = line;
        self.col = col;
        self.last_edit = None;
    }

    /// The line and char under a cell of the view (`y` rows down, `x`
    /// columns across), e.g. for a mouse click.
    pub fn position_at(&self, y: usize, x: usize) -> (usize, usize) {
        match self.wrap {
            None => {
                let row = (self.top + y).min(self.lines.len() - 1);
                (row, self.col_at(row, self.left + x))
            }
            Some(width) => {
                let rows = self.wrapped_rows(width);
                let (line, start, end) = rows[(self.top + y).min(rows.len() - 1)];
                let base = display_width(&self.lines[line], start);
                (line, self.col_at(line, base + x).clamp(start, end))
            }
        }
    }

    /// End the current run of typing, so the next edit is its own undo
    /// step (e.g. after saving: undo then returns to the saved text).
    pub fn break_undo_group(&mut self) {
        self.last_edit = None;
    }

    /// Record the state before an edit. Runs of typing make one undo step.
    fn checkpoint(&mut self, kind: EditKind) {
        if kind == EditKind::Typing && self.last_edit == Some(EditKind::Typing) {
            return;
        }
        self.undo.push(Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
        });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_edit = Some(kind);
    }

    fn restore(&mut self, from_undo: bool) -> bool {
        let (source, target) = if from_undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(snapshot) = source.pop() else { return false };
        target.push(Snapshot {
            lines: std::mem::replace(&mut self.lines, snapshot.lines),
            row: self.row,
            col: self.col,
        });
        self.row = snapshot.row;
        self.col = snapshot.col;
        self.anchor = None;
        self.last_edit = None;
        true
    }

    fn insert_char(&mut self, c: char) {
        let at = byte_at(&self.lines[self.row], self.col);
        self.lines[self.row].insert(at, c);
        self.col += 1;
    }

    fn newline(&mut self) {
        let at = byte_at(&self.lines[self.row], self.col);
        let rest = self.lines[self.row].split_off(at);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    /// Insert pasted text at the cursor, over any selection (line breaks
    /// kept, tabs expanded, other control characters dropped).
    pub fn insert_str(&mut self, text: &str) {
        self.replace_selection(text);
    }

    fn insert_text(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\t', TAB);
        for c in text.chars() {
            match c {
                '\n' | '\r' => self.newline(),
                c if c.is_control() => {}
                c => self.insert_char(c),
            }
        }
    }

    /// Handle a key. Returns true when it was an editor key.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = self.lines.len() - 1;
        // Moving with Shift selects; moving without it drops the selection.
        let moving = matches!(
            key.code,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End
        );
        if moving {
            if !key.modifiers.contains(KeyModifiers::SHIFT) {
                self.anchor = None;
            } else if self.anchor.is_none() {
                self.anchor = Some((self.row, self.col));
            }
        }
        // Typing, Enter and deleting replace a selection.
        let editing = matches!(key.code, KeyCode::Char(_) | KeyCode::Tab | KeyCode::Enter) && !ctrl
            || matches!(key.code, KeyCode::Backspace | KeyCode::Delete);
        if editing && self.selection().is_some() {
            self.checkpoint(EditKind::Other);
            self.remove_selection();
            if matches!(key.code, KeyCode::Backspace | KeyCode::Delete) {
                return true;
            }
            self.last_edit = Some(EditKind::Typing);
        }
        match key.code {
            KeyCode::Char('a') if ctrl => {
                let end = self.line_len(last);
                self.select((0, 0), (last, end));
            }
            KeyCode::Char('z') if ctrl => {
                self.restore(true);
            }
            KeyCode::Char('y') if ctrl => {
                self.restore(false);
            }
            KeyCode::Char(c) if !ctrl => {
                self.checkpoint(EditKind::Typing);
                self.insert_char(c);
            }
            KeyCode::Tab => {
                self.checkpoint(EditKind::Typing);
                for c in TAB.chars() {
                    self.insert_char(c);
                }
            }
            KeyCode::Enter => {
                self.checkpoint(EditKind::Other);
                self.newline();
            }
            KeyCode::Backspace => {
                if self.col > 0 {
                    self.checkpoint(EditKind::Typing);
                    self.col -= 1;
                    let at = byte_at(&self.lines[self.row], self.col);
                    self.lines[self.row].remove(at);
                } else if self.row > 0 {
                    self.checkpoint(EditKind::Other);
                    let line = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = self.line_len(self.row);
                    self.lines[self.row].push_str(&line);
                }
            }
            KeyCode::Delete => {
                if self.col < self.line_len(self.row) {
                    self.checkpoint(EditKind::Typing);
                    let at = byte_at(&self.lines[self.row], self.col);
                    self.lines[self.row].remove(at);
                } else if self.row < last {
                    self.checkpoint(EditKind::Other);
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&next);
                }
            }
            KeyCode::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.line_len(self.row);
                }
                self.last_edit = None;
            }
            KeyCode::Right => {
                if self.col < self.line_len(self.row) {
                    self.col += 1;
                } else if self.row < last {
                    self.row += 1;
                    self.col = 0;
                }
                self.last_edit = None;
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown if self.wrap.is_some() => {
                let step = if matches!(key.code, KeyCode::Up | KeyCode::Down) { 1 } else { self.height as isize };
                let delta = if matches!(key.code, KeyCode::Up | KeyCode::PageUp) { -step } else { step };
                self.move_visual(self.wrap.unwrap_or(1), delta);
            }
            KeyCode::Up => self.set_cursor(self.row.saturating_sub(1), self.col),
            KeyCode::Down => self.set_cursor(self.row + 1, self.col),
            KeyCode::PageUp => self.set_cursor(self.row.saturating_sub(self.height), self.col),
            KeyCode::PageDown => self.set_cursor(self.row + self.height, self.col),
            KeyCode::Home if ctrl => self.set_cursor(0, 0),
            KeyCode::End if ctrl => self.set_cursor(last, usize::MAX),
            KeyCode::Home => self.set_cursor(self.row, 0),
            KeyCode::End => self.set_cursor(self.row, usize::MAX),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(area: &mut TextArea, text: &str) {
        for c in text.chars() {
            area.handle(key(if c == '\n' { KeyCode::Enter } else { KeyCode::Char(c) }));
        }
    }

    #[test]
    fn editing_and_moving() {
        let mut area = TextArea::new(">Title\nbody\n");
        assert_eq!(area.lines().len(), 3);
        area.handle(key(KeyCode::Down));
        area.handle(key(KeyCode::End));
        typed(&mut area, " text\nnew line");
        assert_eq!(area.text(), ">Title\nbody text\nnew line\n");
        // Backspace at the start of a line joins it to the previous one.
        area.set_cursor(2, 0);
        area.handle(key(KeyCode::Backspace));
        assert_eq!(area.text(), ">Title\nbody textnew line\n");
        assert_eq!(area.cursor(), (1, 9));
        // Delete at the end of a line joins the next one.
        area.handle(key(KeyCode::End));
        area.handle(key(KeyCode::Delete));
        assert_eq!(area.text(), ">Title\nbody textnew line");
        // Left at the start wraps to the previous line's end.
        area.set_cursor(1, 0);
        area.handle(key(KeyCode::Left));
        assert_eq!(area.cursor(), (0, 6));
    }

    #[test]
    fn multibyte_text_and_paste() {
        let mut area = TextArea::new("héllo");
        area.set_cursor(0, 2);
        area.insert_str("ü\tX\r\nnext\u{1b}line");
        assert_eq!(area.text(), "héü    X\nnextlinello");
        assert_eq!(area.cursor(), (1, 8));
        assert_eq!(display_width("日本", 2), 4);
        assert_eq!(TextArea::new("日本語").col_at(0, 3), 1);
    }

    #[test]
    fn undo_groups_typing_and_redo_restores() {
        let mut area = TextArea::new("");
        typed(&mut area, "hello");
        area.handle(key(KeyCode::Enter));
        typed(&mut area, "world");
        area.handle(ctrl('z'));
        assert_eq!(area.text(), "hello\n");
        area.handle(ctrl('z'));
        assert_eq!(area.text(), "hello");
        area.handle(ctrl('z'));
        assert_eq!(area.text(), "");
        area.handle(ctrl('y'));
        area.handle(ctrl('y'));
        assert_eq!(area.text(), "hello\n");
        // A new edit clears what could be redone.
        typed(&mut area, "!");
        assert!(!area.restore(false));
        // After a save, new typing undoes back to the saved text.
        let mut area = TextArea::new("");
        typed(&mut area, "saved");
        area.break_undo_group();
        typed(&mut area, " more");
        area.handle(ctrl('z'));
        assert_eq!(area.text(), "saved");
    }

    #[test]
    fn wrapping_breaks_at_spaces_and_moves_by_rows() {
        let mut area = TextArea::new("one two three four\nabcdefghijkl\n");
        // Width 9 (one column kept for the cursor): rows break after spaces.
        let rows: Vec<String> = area
            .wrapped_rows(8)
            .iter()
            .map(|&(l, s, e)| area.lines()[l].chars().skip(s).take(e - s).collect())
            .collect();
        assert_eq!(rows, ["one two ", "three ", "four", "abcdefgh", "ijkl", ""]);
        area.scroll_wrapped(3, 9);
        // Down moves one wrapped row, keeping the column.
        area.set_cursor(0, 1);
        area.handle(key(KeyCode::Down));
        assert_eq!(area.cursor(), (0, 9));
        area.handle(key(KeyCode::Down));
        area.handle(key(KeyCode::Down));
        assert_eq!(area.cursor(), (1, 1));
        area.scroll_wrapped(3, 9);
        assert_eq!(area.top, 1, "the view follows wrapped rows");
        // Clicks map through the wrapped rows.
        assert_eq!(area.position_at(1, 2), (0, 16));
        // Onto a shorter row, the cursor stops at its end, not the next row's start.
        area.set_cursor(0, 7);
        area.handle(key(KeyCode::Down));
        assert_eq!(area.cursor(), (0, 13));
    }

    #[test]
    fn view_follows_the_cursor() {
        let mut area = TextArea::new(&"line\n".repeat(50));
        area.set_cursor(30, 0);
        area.scroll_to_cursor(10, 20);
        assert_eq!(area.top, 21);
        area.set_cursor(0, 0);
        area.scroll_to_cursor(10, 20);
        assert_eq!(area.top, 0);
        let mut wide = TextArea::new(&"x".repeat(100));
        wide.set_cursor(0, 90);
        wide.scroll_to_cursor(5, 40);
        assert!(wide.left <= 90 && 90 < wide.left + 40);
    }

    #[test]
    fn widths_follow_drawn_graphemes() {
        // a ❤️ b: the heart and its selector are two chars drawn two wide.
        let line = "a❤\u{fe0f}b";
        assert_eq!(display_width(line, 1), 1);
        assert_eq!(display_width(line, 3), 3);
        assert_eq!(display_width(line, 4), 4);
        let area = TextArea::new(line);
        assert_eq!(area.col_at(0, 0), 0);
        assert_eq!(area.col_at(0, 1), 1);
        assert_eq!(area.col_at(0, 2), 1);
        assert_eq!(area.col_at(0, 3), 3);
        // Soft wrap breaks between graphemes, by drawn width.
        let area = TextArea::new("👍🏽👍🏽👍🏽");
        assert_eq!(area.wrapped_rows(4), vec![(0, 0, 4), (0, 4, 6)]);
        // Tabs and control characters are one column, as they are shown.
        assert_eq!(display_width("\tx\u{1}", 3), 3);
    }

    #[test]
    fn selecting_and_replacing() {
        let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);
        let mut area = TextArea::new("hello world\nsecond");
        area.set_cursor(0, 6);
        for _ in 0..5 {
            area.handle(shift(KeyCode::Right));
        }
        assert_eq!(area.selected_text(), "world");
        // Typing replaces it, as one undo step with the typing.
        typed(&mut area, "there");
        assert_eq!(area.text(), "hello there\nsecond");
        assert_eq!(area.selection(), None);
        area.handle(ctrl('z'));
        assert_eq!(area.text(), "hello world\nsecond");
        // Across lines, and Backspace removes it.
        area.set_cursor(0, 5);
        area.handle(shift(KeyCode::Down));
        assert_eq!(area.selected_text(), " world\nsecon");
        area.handle(key(KeyCode::Backspace));
        assert_eq!(area.text(), "hellod");
        // Moving without Shift drops the selection.
        area.handle(shift(KeyCode::Home));
        assert!(area.selection().is_some());
        area.handle(key(KeyCode::End));
        assert_eq!(area.selection(), None);
        // Ctrl-A selects everything; a paste replaces it.
        area.handle(ctrl('a'));
        assert_eq!(area.selected_text(), "hellod");
        area.insert_str("new\ntext");
        assert_eq!(area.text(), "new\ntext");
        // A drag from the cursor.
        area.set_cursor(0, 0);
        area.extend_to(1, 2);
        assert_eq!(area.selected_text(), "new\nte");
        // Uppercase letters (typed with Shift) still type.
        area.handle(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT));
        assert_eq!(area.text(), "Xxt");
    }
}
