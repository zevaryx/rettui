//! Where to forward a message (`f` on a picked one): a box over the tab with
//! what's typed and the conversations it matches.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{accent, dim, selected_bg, block};
use crate::app::App;

pub(super) fn draw_forward(frame: &mut Frame, app: &mut App) {
    let Some(picker) = &app.forward else { return };
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(72);
    let height = area.height.saturating_sub(4).min(18);
    if width < 24 || height < 6 {
        return;
    }
    let rect = Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height };
    let outer = block("Forward to", true);
    let inner = outer.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(outer, rect);
    app.regions.forward_box = rect;

    let typed = picker.input.text().to_string();
    let cursor = picker.input.cursor_column();
    let selected = picker.selected;
    let field = Rect { x: inner.x + 2, width: inner.width.saturating_sub(2), height: 1, ..inner };
    frame.render_widget(Paragraph::new(Span::styled("›", Style::default().fg(accent()).bold())), inner);
    let offset = cursor.saturating_sub(field.width.saturating_sub(1) as usize);
    frame.render_widget(Paragraph::new(typed.clone()).scroll((0, offset as u16)), field);
    frame.set_cursor_position(Position::new(field.x + (cursor - offset) as u16, field.y));

    let targets = app.forward_targets();
    let hint = if targets.is_empty() {
        "No conversation matches: type an LXMF address (32 hex characters)"
    } else {
        "Type a name or address · Enter forwards its text and files"
    };
    frame.render_widget(Paragraph::new(Span::styled(hint, Style::default().fg(dim()))), Rect { y: inner.y + 1, height: 1, ..inner });

    let list = Rect { y: inner.y + 3, height: inner.height.saturating_sub(3), ..inner };
    app.regions.forward_list = list;
    let selected = selected.min(targets.len().saturating_sub(1));
    let first = selected.saturating_sub((list.height as usize).saturating_sub(1));
    let lines: Vec<Line> = targets
        .iter()
        .enumerate()
        .skip(first)
        .take(list.height as usize)
        .map(|(i, key)| {
            let base = if i == selected { Style::default().bg(selected_bg()) } else { Style::default() };
            let name = app.store.display_name(key);
            let new = if app.store.conversations.contains_key(key) { "" } else { "  (new conversation)" };
            let mut spans = vec![
                Span::styled(name, base.add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}{new}", &key[..12]), base.fg(dim())),
            ];
            let used: usize = spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
            if i == selected && used < list.width as usize {
                spans.push(Span::styled(" ".repeat(list.width as usize - used), base));
            }
            Line::from(spans)
        })
        .collect();
    app.regions.forward_first = first;
    frame.render_widget(Paragraph::new(lines), list);
}
