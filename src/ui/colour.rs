//! Chat formatting in the channel input: the bar of buttons shown while
//! text is selected, and the colour picker.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{accent, block, dim};
use crate::app::App;
use crate::app::colour::{COLUMNS, ColourHit, SWATCHES};
use crate::app::format::{self, Action};
use crate::nomad::micron::parse_color;

/// A swatch's width in columns.
const CELL: u16 = 5;
/// The picker's height: its border, two rows of swatches, the colour line
/// and a line for what's wrong with it.
const HEIGHT: u16 = 6;

/// The bar of formatting buttons, on the row above the input's box, while
/// text is selected in it.
pub(super) fn draw_format_bar(frame: &mut Frame, app: &mut App, input: Rect, bounds: Rect) {
    app.regions.format_bar.clear();
    if !app.channels.typing || app.channels.input.selection().is_none() || input.y <= bounds.y || app.colour.is_some() {
        return;
    }
    let row = Rect::new(input.x + 1, input.y - 1, input.width.saturating_sub(2), 1).intersection(bounds);
    let buttons: Vec<(Action, String, String)> =
        format::CHAT_ACTIONS.iter().map(|a| (*a, format!(" {} ", a.short()), format!("Alt+{} ", a.key().to_ascii_uppercase()))).collect();
    // With the keys shown if there is room for them, else just the buttons.
    let full: usize = buttons.iter().map(|(_, label, key)| label.width() + key.width()).sum();
    let keys = full <= row.width as usize;
    let mut spans = Vec::new();
    let mut x = row.x;
    for (action, label, key) in buttons {
        let width = (label.width() + if keys { key.width() } else { 0 }) as u16;
        if x + width > row.right() {
            break;
        }
        spans.push(Span::styled(label, Style::default().fg(accent()).bold()));
        if keys {
            spans.push(Span::styled(key, Style::default().fg(dim())));
        }
        app.regions.format_bar.push((Rect::new(x, row.y, width, 1), action));
        x += width;
    }
    let bar = Rect { width: x - row.x, ..row };
    frame.render_widget(Clear, bar);
    frame.render_widget(Paragraph::new(Line::from(spans)), bar);
}

/// A swatch's text colour: dark on a light one, light on a dark one.
fn contrast(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) if u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 > 140_000 => Color::Black,
        _ => Color::White,
    }
}

/// The colour picker, above the input (`input` is its text area, inside
/// the box), inside `bounds`.
pub(super) fn draw_colour(frame: &mut Frame, app: &mut App, input: Rect, bounds: Rect) {
    app.regions.colour_popup = Rect::default();
    app.regions.colour_hits.clear();
    let Some(picker) = &app.colour else { return };
    let width = (COLUMNS as u16 * CELL + 2).min(bounds.width);
    // Over the formatting bar's row too: the bar is hidden meanwhile.
    let bottom = input.y.saturating_sub(1);
    let x = input.x.saturating_sub(1).min(bounds.right().saturating_sub(width)).max(bounds.x);
    let area = Rect::new(x, bottom.saturating_sub(HEIGHT), width, HEIGHT).intersection(bounds);
    if area.height < HEIGHT || area.width < COLUMNS as u16 * CELL + 2 {
        return;
    }
    let title = if picker.background { "Background colour" } else { "Text colour" };
    let popup = block(title, true).title_bottom(Line::styled(" Enter apply · Esc cancel ", Style::default().fg(dim())));
    let inner = popup.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(popup, area);
    app.regions.colour_popup = area;

    // The swatches, and a mark on the chosen one (unless a colour is typed).
    let typed = picker.hex.text().trim();
    let marked = typed.is_empty();
    for (i, code) in SWATCHES.iter().enumerate() {
        let color = parse_color(code).unwrap_or(Color::Reset);
        let cell = Rect::new(inner.x + (i % COLUMNS) as u16 * CELL, inner.y + (i / COLUMNS) as u16, CELL, 1);
        let text = if marked && i == picker.pick { "  ✓  " } else { "     " };
        frame.render_widget(Paragraph::new(text).style(Style::default().fg(contrast(color)).bg(color).bold()), cell);
        app.regions.colour_hits.push((cell, ColourHit::Swatch(i)));
    }

    // The typed colour, with what it looks like.
    let label = "Colour #";
    let line = Rect { y: inner.y + 2, height: 1, ..inner };
    let shown = typed.trim_start_matches('#');
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(label, Style::default().fg(dim())), Span::raw(shown.to_string())])),
        line,
    );
    let hex_cursor = picker.hex.cursor_column().saturating_sub(usize::from(picker.hex.text().starts_with('#')));
    frame.set_cursor_position(Position::new((inner.x + label.width() as u16 + hex_cursor as u16).min(inner.right() - 1), line.y));
    let preview = if marked { format::color(SWATCHES[picker.pick]) } else { format::color(typed) };
    if let Ok(code) = preview
        && let Some(color) = parse_color(&code)
    {
        let cell = Rect::new(inner.right() - 6, line.y, 6, 1);
        frame.render_widget(Paragraph::new("      ").style(Style::default().bg(color)), cell);
    }
    let note = match &picker.error {
        Some(error) => Line::styled(error.clone(), Style::default().fg(Color::Red)),
        None => Line::styled("3 or 6 digits: f80, 00aaff", Style::default().fg(dim())),
    };
    frame.render_widget(Paragraph::new(note), Rect { y: inner.y + 3, height: 1, ..inner });
}
