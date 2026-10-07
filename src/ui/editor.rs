//! Drawing a [`TextArea`]: line numbers, highlighted text cut to the view
//! (or wrapped to it), and the cursor. Used by the page editor and the
//! Reticulum config editor.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::dim;
use crate::nomad::micron::source::visible;
pub(super) use crate::term::textarea::Highlighted;
use crate::term::textarea::{TextArea, display_width};

/// Whether the char at `(line, col)` is inside the selection.
fn is_selected(selection: Option<((usize, usize), (usize, usize))>, line: usize, col: usize) -> bool {
    selection.is_some_and(|(start, end)| (line, col) >= start && (line, col) < end)
}

fn selected(style: Style) -> Style {
    style.add_modifier(Modifier::REVERSED)
}

/// The text and its highlighting: the last draw's while the text is the
/// same. Put back in `area.highlight_cache` once drawn.
fn take_highlighted(area: &mut TextArea, highlight: impl Fn(&str) -> Highlighted) -> (String, Highlighted) {
    let text = area.text();
    match area.highlight_cache.take() {
        Some((cached, styled)) if cached == text => (text, styled),
        _ => {
            let styled = highlight(&text);
            (text, styled)
        }
    }
}

/// Draw the editor into `inner` (inside its block) and return the text area
/// (for mouse clicks), or an empty rect when there is no room. With `wrap`,
/// long lines continue on the next rows instead of scrolling sideways.
pub(super) fn draw_text_editor(
    frame: &mut Frame,
    area: &mut TextArea,
    inner: Rect,
    focused: bool,
    wrap: bool,
    highlight: impl Fn(&str) -> Highlighted,
) -> Rect {
    let lines = area.lines().len();
    let gutter_width = lines.to_string().len() as u16 + 1;
    if inner.width <= gutter_width + 2 || inner.height == 0 {
        return Rect::default();
    }
    let [gutter, text_area] = Layout::horizontal([Constraint::Length(gutter_width), Constraint::Min(1)]).areas(inner);
    let (height, width) = (text_area.height as usize, text_area.width as usize);
    if wrap {
        return draw_wrapped(frame, area, gutter, text_area, focused, highlight);
    }
    area.scroll_to_cursor(height, width);
    let (top, left) = (area.top, area.left);

    let (text, styled) = take_highlighted(area, highlight);
    let selection = area.selection();
    let mut numbers = Vec::new();
    let mut rows = Vec::new();
    for (index, pieces) in styled.iter().enumerate().skip(top).take(height) {
        numbers.push(Line::styled(format!("{:>w$}", index + 1, w = gutter_width as usize - 1), Style::default().fg(dim())));
        // Styled graphemes (measured as the terminal draws them), cut to the
        // visible columns, with the selection shown reversed.
        let mut spans: Vec<Span> = Vec::new();
        let (mut column, mut char_index) = (0, 0);
        for (style, text) in pieces {
            let shown: String = text.chars().map(visible).collect();
            for g in shown.graphemes(true) {
                let w = g.width();
                if column >= left && column + w <= left + width {
                    let style = if is_selected(selection, index, char_index) { selected(*style) } else { *style };
                    match spans.last_mut() {
                        Some(span) if span.style == style => span.content.to_mut().push_str(g),
                        _ => spans.push(Span::styled(g.to_string(), style)),
                    }
                }
                column += w;
                char_index += g.chars().count();
            }
        }
        // A selected line break shows as one selected cell.
        if is_selected(selection, index, char_index) && column >= left && column < left + width {
            spans.push(Span::styled(" ", selected(Style::default())));
        }
        rows.push(Line::from(spans));
    }
    area.highlight_cache = Some((text, styled));
    frame.render_widget(Paragraph::new(numbers), gutter);
    frame.render_widget(Paragraph::new(rows), text_area);
    if focused {
        let (row, col) = area.cursor();
        let x = display_width(&area.lines()[row], col).saturating_sub(left);
        frame.set_cursor_position(Position::new(text_area.x + x as u16, text_area.y + (row - top) as u16));
    }
    text_area
}

fn draw_wrapped(
    frame: &mut Frame,
    area: &mut TextArea,
    gutter: Rect,
    text_area: Rect,
    focused: bool,
    highlight: impl Fn(&str) -> Highlighted,
) -> Rect {
    let (height, width) = (text_area.height as usize, text_area.width as usize);
    area.scroll_wrapped(height, width);
    let wrap = width.saturating_sub(1).max(1);
    let (text, styled) = take_highlighted(area, highlight);
    let selection = area.selection();
    // Each line's chars with their styles (the selection reversed), to cut
    // into wrapped rows.
    let cells = |line: usize| -> Vec<(char, Style)> {
        styled
            .get(line)
            .map(|pieces| {
                pieces
                    .iter()
                    .flat_map(|(style, text)| text.chars().map(move |c| (visible(c), *style)))
                    .enumerate()
                    .map(|(i, (c, style))| (c, if is_selected(selection, line, i) { selected(style) } else { style }))
                    .collect()
            })
            .unwrap_or_default()
    };
    let gutter_width = gutter.width as usize;
    let mut numbers = Vec::new();
    let mut rows = Vec::new();
    let mut cached: Option<(usize, Vec<(char, Style)>)> = None;
    let wrapped = area.wrapped_rows(wrap);
    for (index, &(line, start, end)) in wrapped.iter().enumerate().skip(area.top).take(height) {
        // The number only on a line's first row.
        let first = index == 0 || wrapped[index - 1].0 != line;
        let number = if first { format!("{:>w$}", line + 1, w = gutter_width - 1) } else { String::new() };
        numbers.push(Line::styled(number, Style::default().fg(dim())));
        if cached.as_ref().is_none_or(|(l, _)| *l != line) {
            cached = Some((line, cells(line)));
        }
        let chars = &cached.as_ref().expect("cached").1;
        let mut spans: Vec<Span> = Vec::new();
        for &(c, style) in chars.iter().take(end).skip(start) {
            match spans.last_mut() {
                Some(span) if span.style == style => span.content.to_mut().push(c),
                _ => spans.push(Span::styled(c.to_string(), style)),
            }
        }
        rows.push(Line::from(spans));
    }
    area.highlight_cache = Some((text, styled));
    frame.render_widget(Paragraph::new(numbers), gutter);
    frame.render_widget(Paragraph::new(rows), text_area);
    if focused {
        let (row, col) = area.cursor();
        if let Some(index) = wrapped.iter().enumerate().position(|(i, &(line, start, end))| {
            let last = wrapped.get(i + 1).is_none_or(|next| next.0 != line);
            line == row && col >= start && (col < end || (last && col == end))
        }) && index >= area.top
            && index < area.top + height
        {
            let (_, start, _) = wrapped[index];
            let text = &area.lines()[row];
            let x = display_width(text, col) - display_width(text, start);
            frame.set_cursor_position(Position::new(text_area.x + x as u16, text_area.y + (index - area.top) as u16));
        }
    }
    text_area
}
