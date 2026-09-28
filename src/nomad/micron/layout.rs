//! Laying a parsed [`Page`] out for the terminal: wrapping, alignment,
//! form fields, image rows, and which cells each link or field occupies.

use std::collections::HashMap;

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{FieldKind, Interactive, MLine, Page};
use crate::term::images::{Graphics, Picture, Placement};

pub struct Layout {
    pub lines: Vec<Line<'static>>,
    /// Visual row where each interactive item starts.
    pub item_rows: Vec<usize>,
    /// Clickable cells: (row, first column, end column, item).
    pub hits: Vec<(usize, usize, usize, usize)>,
    /// Terminal graphics to draw over reserved rows, keyed by image URL.
    pub placements: Vec<Placement<String>>,
}

/// Tallest an inline page image may be, in rows.
const MAX_IMAGE_ROWS: usize = 40;

/// The interactive item drawn at a layout cell, if any.
pub fn hit_at(hits: &[(usize, usize, usize, usize)], row: usize, col: usize) -> Option<usize> {
    hits.iter()
        .find(|(r, x0, x1, _)| *r == row && (*x0..*x1).contains(&col))
        .map(|(.., item)| *item)
}

/// Where a row of `used` columns starts inside `width` for an alignment.
fn align_offset(align: Alignment, width: usize, used: usize) -> usize {
    match align {
        Alignment::Left => 0,
        Alignment::Center => width.saturating_sub(used) / 2,
        Alignment::Right => width.saturating_sub(used),
    }
}

/// A row of wrapped text: styled spans and the item each belongs to.
type Row = Vec<(Span<'static>, Option<usize>)>;

/// Word-wrap styled pieces into rows of at most `avail` columns. Breaks go
/// at spaces, which are dropped at the break; a word wider than a whole row
/// is split. Returns the rows and the row each piece starts on.
fn wrap(pieces: &[(String, Style, Option<usize>)], avail: usize) -> (Vec<Row>, Vec<usize>) {
    // Each character as shown (a grapheme: an emoji with its modifiers or
    // joined parts is one, measured whole as the terminal draws it) with its
    // display width and piece, split into words and runs of spaces.
    let mut chars: Vec<(&str, usize, usize)> = Vec::new();
    let mut starts_at = Vec::with_capacity(pieces.len());
    for (index, (text, ..)) in pieces.iter().enumerate() {
        starts_at.push(chars.len());
        if text.is_ascii() {
            // Each ASCII char is its own grapheme (most text; much quicker).
            chars.extend(text.char_indices().map(|(i, c)| (&text[i..=i], c.width().unwrap_or(0), index)));
        } else {
            chars.extend(text.graphemes(true).map(|g| (g, g.width(), index)));
        }
    }
    let mut rows: Vec<Vec<(&str, usize)>> = vec![Vec::new()];
    // Row of each char (for where pieces start).
    let mut char_rows = vec![0; chars.len() + 1];
    let mut used = 0;
    let mut i = 0;
    while i < chars.len() {
        let space = chars[i].0 == " ";
        let mut end = i;
        while end < chars.len() && (chars[end].0 == " ") == space {
            end += 1;
        }
        let run_width: usize = chars[i..end].iter().map(|c| c.1).sum();
        if space {
            if used + run_width > avail && used > 0 {
                // Break here; the spaces go.
                rows.push(Vec::new());
                used = 0;
                char_rows[i..end].fill(rows.len() - 1);
            } else {
                // Leading spaces wider than the row are clipped.
                let fit = run_width.min(avail - used);
                char_rows[i..end].fill(rows.len() - 1);
                rows.last_mut().unwrap().extend(chars[i..i + fit].iter().map(|&(c, _, piece)| (c, piece)));
                used += fit;
            }
        } else {
            if used + run_width > avail && used > 0 {
                // Move the word down, without the spaces before it.
                let row = rows.last_mut().unwrap();
                while row.last().is_some_and(|(c, _)| *c == " ") {
                    row.pop();
                }
                rows.push(Vec::new());
                used = 0;
            }
            for (k, &(c, w, piece)) in chars[i..end].iter().enumerate() {
                if used + w > avail && used > 0 {
                    rows.push(Vec::new());
                    used = 0;
                }
                char_rows[i + k] = rows.len() - 1;
                rows.last_mut().unwrap().push((c, piece));
                used += w;
            }
        }
        i = end;
    }
    char_rows[chars.len()] = rows.len() - 1;
    let starts = starts_at.iter().map(|&at| char_rows[at]).collect();
    let rows = rows
        .into_iter()
        .map(|row| {
            let mut spans: Row = Vec::new();
            let mut text = String::new();
            let mut current: Option<usize> = None;
            for (c, piece) in row {
                if current.is_some_and(|p| p != piece) {
                    let (_, style, item) = &pieces[current.unwrap()];
                    spans.push((Span::styled(std::mem::take(&mut text), *style), *item));
                }
                current = Some(piece);
                text.push_str(c);
            }
            if let Some(p) = current {
                let (_, style, item) = &pieces[p];
                spans.push((Span::styled(text, *style), *item));
            }
            spans
        })
        .collect();
    (rows, starts)
}

impl Page {
    fn field_text(&self, index: usize) -> String {
        let field = &self.fields[index];
        match field.kind {
            FieldKind::Text { masked } => {
                let shown: String = if masked {
                    "*".repeat(field.value.chars().count())
                } else {
                    field.value.clone()
                };
                let pad = field.width.saturating_sub(shown.width());
                format!("{shown}{}", "_".repeat(pad))
            }
            FieldKind::Checkbox => {
                format!("[{}] {}", if field.checked { "x" } else { " " }, field.label)
            }
            FieldKind::Radio => {
                format!("({}) {}", if field.checked { "*" } else { " " }, field.label)
            }
        }
    }

    pub fn layout(
        &self,
        width: usize,
        selected: Option<usize>,
        images: &HashMap<String, Picture>,
        graphics: Option<&Graphics>,
    ) -> Layout {
        let width = width.max(1);
        let mut lines = Vec::new();
        let mut item_rows = vec![0; self.items.len()];
        let mut hits = Vec::new();
        let mut placements = Vec::new();

        for line in &self.lines {
            match line {
                MLine::Divider { indent, ch, style } => {
                    let indent = (*indent).min(width - 1);
                    let cw = ch.width().unwrap_or(1).max(1);
                    let rule: String = std::iter::repeat_n(*ch, (width - indent) / cw).collect();
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(indent)),
                        Span::styled(rule, *style),
                    ]));
                }
                MLine::Image {
                    indent,
                    align,
                    url,
                    alt,
                    width: wanted,
                } => {
                    let indent = (*indent).min(width - 1);
                    let avail = wanted.unwrap_or(width).min(width - indent);
                    match images.get(url).and_then(|p| p.rows(graphics, avail, MAX_IMAGE_ROWS)) {
                        Some(size) => {
                            let used = usize::from(size.width);
                            placements.push(Placement {
                                row: lines.len(),
                                col: indent + align_offset(*align, width - indent, used),
                                size,
                                key: url.clone(),
                            });
                            for _ in 0..size.height {
                                lines.push(Line::raw(""));
                            }
                        }
                        None => lines.push(
                            Line::from(vec![
                                Span::raw(" ".repeat(indent)),
                                Span::styled(
                                    format!("[image: {alt}]"),
                                    Style::default()
                                        .fg(Color::DarkGray)
                                        .add_modifier(Modifier::ITALIC),
                                ),
                            ])
                            .alignment(*align),
                        ),
                    }
                }
                MLine::Text {
                    indent,
                    align,
                    fill,
                    spans,
                } => {
                    let indent = (*indent).min(width - 1);
                    let avail = width - indent;
                    // Styled pieces first, then wrap them as one run of text.
                    let mut pieces: Vec<(String, Style, Option<usize>)> = Vec::new();
                    for span in spans {
                        let mut style = span.style;
                        let text = match span.item {
                            Some(item) => {
                                if let Interactive::Field(f) = self.items[item] {
                                    style = style.add_modifier(Modifier::UNDERLINED);
                                    if matches!(self.fields[f].kind, FieldKind::Text { .. })
                                        && style.bg.is_none()
                                    {
                                        style = style.bg(Color::Rgb(0x30, 0x30, 0x30));
                                    }
                                }
                                if selected == Some(item) {
                                    style = style.add_modifier(Modifier::REVERSED);
                                }
                                match self.items[item] {
                                    Interactive::Field(f) => self.field_text(f),
                                    Interactive::Link { .. } => span.text.clone(),
                                }
                            }
                            None => span.text.clone(),
                        };
                        pieces.push((text, style, span.item));
                    }
                    let (rows, starts) = wrap(&pieces, avail);
                    for ((.., item), start) in pieces.iter().zip(starts) {
                        if let Some(item) = item {
                            item_rows[*item] = lines.len() + start;
                        }
                    }
                    for mut row in rows {
                        if let Some(fill) = fill {
                            // Filled rows span the full width, so place the
                            // text by padding instead of by line alignment.
                            let row_width: usize = row.iter().map(|(s, _)| s.width()).sum();
                            let gap = avail.saturating_sub(row_width);
                            let before = align_offset(*align, avail, row_width);
                            row.insert(0, (Span::styled(" ".repeat(before), *fill), None));
                            row.push((Span::styled(" ".repeat(gap - before), *fill), None));
                        }
                        let align = if fill.is_some() { Alignment::Left } else { *align };
                        if align == Alignment::Left && indent > 0 {
                            row.insert(0, (Span::raw(" ".repeat(indent)), None));
                        }
                        let row_width: usize = row.iter().map(|(s, _)| s.width()).sum();
                        let mut x = align_offset(align, width, row_width);
                        for (span, item) in &row {
                            let w = span.width();
                            if let Some(item) = item {
                                hits.push((lines.len(), x, x + w, *item));
                            }
                            x += w;
                        }
                        let spans: Vec<Span<'static>> = row.into_iter().map(|(s, _)| s).collect();
                        lines.push(Line::from(spans).alignment(align));
                    }
                }
            }
        }
        Layout {
            lines,
            item_rows,
            hits,
            placements,
        }
    }
}
