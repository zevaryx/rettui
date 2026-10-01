//! Laying a parsed [`Page`] out for the terminal: wrapping, alignment,
//! form fields, image rows, and which cells each link or field occupies.

use std::collections::HashMap;

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{FieldKind, Interactive, MLine, MSpan, Page, Table};
use crate::term::images::{Graphics, Picture, Placement};

pub struct Layout {
    pub lines: Vec<Line<'static>>,
    /// Visual row where each interactive item starts.
    pub item_rows: Vec<usize>,
    /// Clickable cells: (row, first column, end column, item).
    pub hits: Vec<(usize, usize, usize, usize)>,
    /// Terminal graphics to draw over reserved rows, keyed by image URL.
    pub placements: Vec<Placement<String>>,
    /// The first row of each of the page's lines (`None` while folded
    /// away), for jumping to anchors.
    pub line_rows: Vec<Option<usize>>,
}

/// Table borders, and partials waiting to load, are drawn dim.
const BORDER: Style = Style::new().fg(Color::DarkGray);
/// A text field's background, where its text has none.
const FIELD_BG: Color = Color::Rgb(0x30, 0x30, 0x30);

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
/// Text to wrap: its pieces, each with its style and item.
type Piece = (String, Style, Option<usize>);

/// Word-wrap styled pieces into rows of at most `avail` columns. Breaks go
/// at spaces, which are dropped at the break; a word wider than a whole row
/// is split. Returns the rows and the row each piece starts on.
fn wrap(pieces: &[Piece], avail: usize) -> (Vec<Row>, Vec<usize>) {
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
                // On one row: the first line, and ↵ if there are more.
                let value = match field.value.split_once('\n') {
                    Some((first, _)) => format!("{first}↵"),
                    None => field.value.clone(),
                };
                let shown: String = if masked { "*".repeat(value.chars().count()) } else { value };
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

    /// The text an item's span shows: a field's from its state, a fold's
    /// mark, or the span's own.
    fn item_text(&self, span: &MSpan, item: usize) -> String {
        match self.items[item] {
            Interactive::Field(f) => self.field_text(f),
            Interactive::Fold(f) if span.text.is_empty() => format!("{} ", self.fold_mark(self.folds[f].open)),
            _ => span.text.clone(),
        }
    }

    /// Styled pieces of a run of spans, as drawn: fields from their state,
    /// the selection reversed, and the row's background under text without
    /// one of its own.
    fn pieces(&self, spans: &[MSpan], selected: Option<usize>, fill: Option<Style>) -> Vec<Piece> {
        let mut pieces = Vec::new();
        for span in spans {
            let mut style = span.style;
            let text = match span.item {
                Some(item) => {
                    if let Interactive::Field(f) = self.items[item] {
                        style = style.add_modifier(Modifier::UNDERLINED);
                        if matches!(self.fields[f].kind, FieldKind::Text { .. }) && style.bg.is_none() {
                            style = style.bg(FIELD_BG);
                        }
                    }
                    if selected == Some(item) {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    self.item_text(span, item)
                }
                None => span.text.clone(),
            };
            if style.bg.is_none() {
                style.bg = fill.and_then(|f| f.bg);
            }
            pieces.push((text, style, span.item));
        }
        pieces
    }

    pub fn layout(
        &self,
        width: usize,
        selected: Option<usize>,
        images: &HashMap<String, Picture>,
        graphics: Option<&Graphics>,
    ) -> Layout {
        let width = width.max(1);
        let mut out = Layout {
            lines: Vec::new(),
            item_rows: vec![0; self.items.len()],
            hits: Vec::new(),
            placements: Vec::new(),
            line_rows: vec![None; self.lines.len()],
        };
        // A folded section hides what's under it, up to the next heading
        // of its level or above (or a `<` line).
        let mut folded_at: Option<usize> = None;

        for (index, line) in self.lines.iter().enumerate() {
            match line {
                MLine::SectionEnd => {
                    folded_at = None;
                    continue;
                }
                MLine::Text { section: Some(section), .. } if folded_at.is_some_and(|level| section.level <= level) => {
                    folded_at = None;
                }
                _ if folded_at.is_some() => continue,
                _ => {}
            }
            out.line_rows[index] = Some(out.lines.len());
            match line {
                MLine::SectionEnd => {}
                MLine::Divider { indent, ch, style } => {
                    let indent = (*indent).min(width - 1);
                    let cw = ch.width().unwrap_or(1).max(1);
                    let count = (width - indent) / cw;
                    let rule: String = std::iter::repeat_n(*ch, count).collect();
                    // Its background spans the row, indent included.
                    let fill = style.bg.map_or_else(Style::default, |bg| Style::default().bg(bg));
                    out.lines.push(Line::from(vec![
                        Span::styled(" ".repeat(indent), fill),
                        Span::styled(rule, *style),
                        Span::styled(" ".repeat(width - indent - count * cw), fill),
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
                            out.placements.push(Placement {
                                row: out.lines.len(),
                                col: indent + align_offset(*align, width - indent, used),
                                size,
                                key: url.clone(),
                            });
                            for _ in 0..size.height {
                                out.lines.push(Line::raw(""));
                            }
                        }
                        None => out.lines.push(
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
                MLine::Partial { indent, .. } => {
                    out.lines.push(Line::from(vec![
                        Span::raw(" ".repeat((*indent).min(width - 1))),
                        Span::styled("loading…", BORDER.add_modifier(Modifier::ITALIC)),
                    ]));
                }
                MLine::FieldBlock { indent, item, style } => self.field_block(&mut out, width, *indent, *item, *style, selected),
                MLine::Table(table) => self.table(&mut out, width, table, selected),
                MLine::Text {
                    indent,
                    align,
                    fill,
                    spans,
                    section,
                } => {
                    if let Some(fold) = section.and_then(|s| s.fold)
                        && !self.folds[fold].open
                    {
                        folded_at = section.map(|s| s.level);
                    }
                    let indent = (*indent).min(width - 1);
                    let avail = width - indent;
                    // Styled pieces first, then wrap them as one run of text.
                    let pieces = self.pieces(spans, selected, *fill);
                    let (rows, starts) = wrap(&pieces, avail);
                    for ((.., item), start) in pieces.iter().zip(starts) {
                        if let Some(item) = item {
                            out.item_rows[*item] = out.lines.len() + start;
                        }
                    }
                    for mut row in rows {
                        let align = match fill {
                            Some(fill) => {
                                // Filled rows span the full width, indent
                                // included, so place the text by padding
                                // instead of by line alignment.
                                let row_width: usize = row.iter().map(|(s, _)| s.width()).sum();
                                let gap = avail.saturating_sub(row_width);
                                let before = align_offset(*align, avail, row_width);
                                row.insert(0, (Span::styled(" ".repeat(indent + before), *fill), None));
                                row.push((Span::styled(" ".repeat(gap - before), *fill), None));
                                Alignment::Left
                            }
                            None => {
                                if *align == Alignment::Left && indent > 0 {
                                    row.insert(0, (Span::raw(" ".repeat(indent)), None));
                                }
                                *align
                            }
                        };
                        let row_width: usize = row.iter().map(|(s, _)| s.width()).sum();
                        let mut x = align_offset(align, width, row_width);
                        for (span, item) in &row {
                            let w = span.width();
                            if let Some(item) = item {
                                out.hits.push((out.lines.len(), x, x + w, *item));
                            }
                            x += w;
                        }
                        let spans: Vec<Span<'static>> = row.into_iter().map(|(s, _)| s).collect();
                        out.lines.push(Line::from(spans).alignment(align));
                    }
                }
            }
        }
        out
    }

    /// A text field of several rows: its text's lines, wrapped to its
    /// width, on a background of its own.
    fn field_block(&self, out: &mut Layout, width: usize, indent: usize, item: usize, style: Style, selected: Option<usize>) {
        let Interactive::Field(f) = self.items[item] else { return };
        let field = &self.fields[f];
        let indent = indent.min(width - 1);
        let field_width = field.width.min(width - indent).max(1);
        let masked = matches!(field.kind, FieldKind::Text { masked: true });
        let mut lines: Vec<String> = Vec::new();
        for line in field.value.split('\n') {
            let line = if masked { "*".repeat(line.chars().count()) } else { line.to_string() };
            let pieces = [(line, Style::default(), None)];
            let (rows, _) = wrap(&pieces, field_width);
            lines.extend(rows.into_iter().map(|row| row.into_iter().map(|(s, _)| s.content.into_owned()).collect::<String>()));
        }
        if lines.len() > field.rows {
            lines.truncate(field.rows);
            if let Some(last) = lines.last_mut() {
                last.push('…');
            }
        }
        let mut style = style.add_modifier(Modifier::UNDERLINED);
        if style.bg.is_none() {
            style = style.bg(FIELD_BG);
        }
        if selected == Some(item) {
            style = style.add_modifier(Modifier::REVERSED);
        }
        out.item_rows[item] = out.lines.len();
        for row in 0..field.rows {
            let text = lines.get(row).cloned().unwrap_or_default();
            let pad = field_width.saturating_sub(text.width());
            out.hits.push((out.lines.len(), indent, indent + field_width, item));
            out.lines.push(Line::from(vec![Span::raw(" ".repeat(indent)), Span::styled(format!("{text}{}", " ".repeat(pad)), style)]));
        }
    }

    /// A table, with borders, its columns as wide as their widest cells
    /// where they fit (else the widest give way, and cells wrap).
    fn table(&self, out: &mut Layout, width: usize, table: &Table, selected: Option<usize>) {
        let indent = table.indent.min(width - 1);
        let avail = table.max_width.map_or(width - indent, |w| w.min(width - indent));
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0).max(table.columns.len());
        if columns == 0 {
            return;
        }
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let cells: Vec<Vec<Vec<Piece>>> = table
            .rows
            .iter()
            .enumerate()
            .map(|(r, row)| {
                (0..columns)
                    .map(|c| {
                        let spans = row.get(c).map(Vec::as_slice).unwrap_or_default();
                        let mut pieces = self.pieces(spans, selected, None);
                        if table.header && r == 0 {
                            pieces.iter_mut().for_each(|p| p.1 = bold.patch(p.1));
                        }
                        pieces
                    })
                    .collect()
            })
            .collect();
        // Room for text: less a border and a space either side of each.
        let room = avail.saturating_sub(3 * columns + 1);
        if room < columns {
            // Too narrow for a table: each cell on a row of its own.
            for row in &cells {
                for cell in row {
                    let (rows, _) = wrap(cell, width - indent);
                    for row in rows {
                        let mut spans = vec![Span::raw(" ".repeat(indent))];
                        spans.extend(row.into_iter().map(|(s, _)| s));
                        out.lines.push(Line::from(spans));
                    }
                }
            }
            return;
        }
        let natural = |c: usize| cells.iter().map(|row| row[c].iter().map(|p| p.0.width()).sum::<usize>()).max().unwrap_or(0).max(1);
        let mut widths: Vec<usize> = (0..columns).map(natural).collect();
        // The widest give way, a column at a time, until they fit.
        while widths.iter().sum::<usize>() > room {
            let widest = (0..columns).max_by_key(|&c| widths[c]).expect("there are columns");
            widths[widest] -= 1;
        }
        let total = widths.iter().sum::<usize>() + 3 * columns + 1;
        let left = indent + align_offset(table.align, width - indent, total);
        let rule = |l: &str, m: &str, r: &str| {
            let middle: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            Line::from(vec![Span::raw(" ".repeat(left)), Span::styled(format!("{l}{}{r}", middle.join(m)), BORDER)])
        };
        out.lines.push(rule("┌", "┬", "┐"));
        for (r, row) in cells.iter().enumerate() {
            let wrapped: Vec<(Vec<Row>, Vec<usize>)> = row.iter().zip(&widths).map(|(cell, w)| wrap(cell, *w)).collect();
            let height = wrapped.iter().map(|(rows, _)| rows.len()).max().unwrap_or(1);
            let first = out.lines.len();
            for ((_, starts), cell) in wrapped.iter().zip(row) {
                for ((.., item), start) in cell.iter().zip(starts) {
                    if let Some(item) = item {
                        out.item_rows[*item] = first + start;
                    }
                }
            }
            for line in 0..height {
                let mut spans = vec![Span::raw(" ".repeat(left)), Span::styled("│", BORDER)];
                let mut x = left + 1;
                for (c, (rows, _)) in wrapped.iter().enumerate() {
                    let content: Row = rows.get(line).cloned().unwrap_or_default();
                    let used: usize = content.iter().map(|(s, _)| s.width()).sum();
                    let align = table.columns.get(c).copied().unwrap_or(Alignment::Left);
                    let before = align_offset(align, widths[c], used);
                    spans.push(Span::raw(" ".repeat(1 + before)));
                    x += 1 + before;
                    for (span, item) in content {
                        let w = span.width();
                        if let Some(item) = item {
                            out.hits.push((out.lines.len(), x, x + w, item));
                        }
                        x += w;
                        spans.push(span);
                    }
                    let after = widths[c].saturating_sub(before + used) + 1;
                    spans.push(Span::raw(" ".repeat(after)));
                    spans.push(Span::styled("│", BORDER));
                    x += after + 1;
                }
                out.lines.push(Line::from(spans));
            }
            if table.header && r == 0 && cells.len() > 1 {
                out.lines.push(rule("├", "┼", "┤"));
            }
        }
        out.lines.push(rule("└", "┴", "┘"));
    }
}
