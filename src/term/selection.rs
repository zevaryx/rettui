//! Terminal-style text selection over laid-out rows (display columns).

use unicode_width::UnicodeWidthChar;

/// A selection from `anchor` (where the drag started) to `head` (where the
/// pointer is), as (row, display column). Both ends are inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (usize, usize),
    pub head: (usize, usize),
}

impl Selection {
    pub fn at(row: usize, col: usize) -> Self {
        Self {
            anchor: (row, col),
            head: (row, col),
        }
    }

    /// Start and end in reading order.
    pub fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Whether the cell at (row, col) is selected.
    pub fn contains(&self, row: usize, col: usize) -> bool {
        let ((r0, c0), (r1, c1)) = self.ordered();
        if row < r0 || row > r1 {
            return false;
        }
        let from = if row == r0 { c0 } else { 0 };
        let to = if row == r1 { c1 } else { usize::MAX };
        (from..=to).contains(&col)
    }

    /// The selected text: whole rows between the ends, trailing blanks
    /// trimmed, rows joined by newlines.
    pub fn extract(&self, rows: &[String]) -> String {
        let ((r0, c0), (r1, c1)) = self.ordered();
        let mut out = Vec::new();
        for row in r0..=r1.min(rows.len().saturating_sub(1)) {
            let Some(line) = rows.get(row) else { break };
            let from = if row == r0 { c0 } else { 0 };
            let to = if row == r1 { c1 } else { usize::MAX };
            let mut col = 0;
            let mut piece = String::new();
            for ch in line.chars() {
                let width = ch.width().unwrap_or(0);
                // A wide character counts when any of its cells is selected.
                if col + width.max(1) > from && col <= to {
                    piece.push(ch);
                }
                col += width;
            }
            out.push(piece.trim_end().to_string());
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<String> {
        vec!["hello world".into(), "  second line   ".into(), "全角 text".into()]
    }

    #[test]
    fn extracts_within_one_row() {
        let sel = Selection { anchor: (0, 6), head: (0, 10) };
        assert_eq!(sel.extract(&rows()), "world");
        // Dragging backwards selects the same text.
        let back = Selection { anchor: (0, 10), head: (0, 6) };
        assert_eq!(back.extract(&rows()), "world");
    }

    #[test]
    fn extracts_across_rows_and_trims() {
        let sel = Selection { anchor: (0, 6), head: (1, 30) };
        assert_eq!(sel.extract(&rows()), "world\n  second line");
        assert!(sel.contains(1, 0) && sel.contains(0, 6) && !sel.contains(0, 5));
    }

    #[test]
    fn wide_characters_use_display_columns() {
        // "全" occupies columns 0-1, "角" 2-3.
        let sel = Selection { anchor: (2, 1), head: (2, 2) };
        assert_eq!(sel.extract(&rows()), "全角");
        let sel = Selection { anchor: (2, 5), head: (2, 8) };
        assert_eq!(sel.extract(&rows()), "text");
    }
}
