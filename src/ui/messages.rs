//! Messages tab: conversation list, history with image previews, compose.

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};

use super::{ACCENT, DIM, SELECTED_BG, block, human_bytes, time_label, wrap};
use crate::app::App;
use crate::store::{Message, MessageState};
use crate::term::images::{ImageRows, Placement, draw_placements};

/// Widest inline image preview in the message history.
const MAX_PREVIEW_COLS: usize = 48;
const MAX_PREVIEW_ROWS: usize = 12;

/// A history row, with the attachment it belongs to (for clicks).
type HistoryRow = (Line<'static>, Option<PathBuf>);

pub(super) fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let [list_area, chat_area] =
        Layout::horizontal([Constraint::Length(super::side_width(area.width, 30)), Constraint::Min(20)]).areas(area);

    let order = app.store.conversation_order();
    let list_block = block("Conversations", !app.composing);
    app.regions.conversations = list_block.inner(list_area);
    if order.is_empty() {
        let hint = Paragraph::new(
            "No conversations yet.\n\nPress n to message an LXMF address, or pick a peer in the Network tab.",
        )
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(DIM))
        .block(list_block);
        frame.render_widget(hint, list_area);
    } else {
        let items: Vec<ListItem> = order
            .iter()
            .map(|key| {
                let conversation = &app.store.conversations[key];
                let mut spans = vec![Span::raw(app.store.display_name(key))];
                if conversation.unread > 0 {
                    spans.push(Span::styled(
                        format!(" {}", conversation.unread),
                        Style::default().fg(Color::Black).bg(Color::Yellow).bold(),
                    ));
                }
                let preview = conversation
                    .messages
                    .last()
                    .map(|m| match m.content.lines().next() {
                        Some(line) if !line.is_empty() => line.to_string(),
                        _ if !m.attachments.is_empty() => format!("📎 {}", m.attachments[0].name),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                ListItem::new(vec![
                    Line::from(spans),
                    Line::styled(format!("  {preview}"), Style::default().fg(DIM)),
                ])
            })
            .collect();
        let list = List::new(items)
            .block(list_block)
            .highlight_style(Style::default().bg(SELECTED_BG).bold())
            .highlight_symbol("▌");
        frame.render_stateful_widget(list, list_area, &mut app.conversations);
    }

    let [history_area, compose_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(chat_area);

    let Some(key) = app.active_conversation.clone() else {
        frame.render_widget(block("", false), chat_area);
        app.regions.history = Rect::default();
        app.regions.compose = Rect::default();
        return;
    };
    let name = app.store.display_name(&key);
    let title = format!("{name}  {key}");
    let history_block = block(&title, false).padding(ratatui::widgets::Padding::horizontal(1));
    let history_inner = history_block.inner(history_area);
    let inner_width = history_inner.width as usize;
    let count = app.store.conversations.get(&key).map_or(0, |c| c.messages.len());
    let height = history_inner.height as usize;
    // Only the messages that can be on screen: build from the newest back
    // until the view (and however far it is scrolled up) is full. Each
    // message's rows, with image placements relative to its first row.
    let needed = app.message_scroll + height;
    let graphics = app.graphics.clone();
    let mut groups: Vec<(Vec<HistoryRow>, Vec<Placement<PathBuf>>)> = Vec::new();
    let mut built = 0;
    for index in (0..count).rev() {
        if built >= needed {
            break;
        }
        let message: Message = app.store.conversations[&key].messages[index].clone();
        // Each history line carries the attachment it belongs to (for clicks).
        let mut lines: Vec<HistoryRow> = Vec::new();
        let mut placements: Vec<Placement<PathBuf>> = Vec::new();
        let (who, color) = if message.incoming {
            (name.as_str(), Color::LightMagenta)
        } else {
            ("You", ACCENT)
        };
        let status = match &message.state {
            MessageState::Received { verified: true } => Span::raw(""),
            MessageState::Received { verified: false } => {
                Span::styled(" unverified", Style::default().fg(Color::Yellow))
            }
            MessageState::Sending => Span::styled(" sending…", Style::default().fg(DIM)),
            MessageState::Delivered => Span::styled(" ✓", Style::default().fg(Color::Green)),
            MessageState::Propagated => {
                Span::styled(" ✓ via propagation node", Style::default().fg(Color::Green))
            }
            MessageState::Failed(e) => {
                Span::styled(format!(" failed: {e}"), Style::default().fg(Color::Red))
            }
        };
        lines.push((
            Line::from(vec![
                Span::styled(who.to_string(), Style::default().fg(color).bold()),
                Span::styled(format!("  {}", time_label(message.timestamp)), Style::default().fg(DIM)),
                status,
            ]),
            None,
        ));
        if !message.title.is_empty() {
            lines.push((Line::styled(message.title.clone(), Style::default().bold()), None));
        }
        if !message.content.is_empty() {
            for line in wrap(&message.content, inner_width) {
                lines.push((Line::raw(line), None));
            }
        }
        for attachment in &message.attachments {
            let path = Some(attachment.path.clone());
            if attachment.image
                && let Some(picture) = app.picture(&attachment.path)
            {
                let max_cols = inner_width.min(MAX_PREVIEW_COLS);
                match picture.rows(graphics.as_ref(), max_cols, MAX_PREVIEW_ROWS) {
                    ImageRows::Graphic(size) => {
                        placements.push(Placement {
                            row: lines.len(),
                            col: 0,
                            size,
                            key: attachment.path.clone(),
                        });
                        for _ in 0..size.height {
                            lines.push((Line::raw(""), path.clone()));
                        }
                    }
                    ImageRows::Text(rows) => {
                        for row in rows {
                            lines.push((row, path.clone()));
                        }
                    }
                }
            }
            lines.push((
                Line::from(vec![
                    Span::styled("📎 ", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        attachment.name.clone(),
                        Style::default().add_modifier(Modifier::UNDERLINED),
                    ),
                    Span::styled(format!("  {}", human_bytes(attachment.size)), Style::default().fg(DIM)),
                ]),
                path,
            ));
        }
        lines.push((Line::raw(""), None));
        built += lines.len();
        groups.push((lines, placements));
    }
    // Oldest first, with placements at their rows in the whole list.
    let mut lines: Vec<HistoryRow> = Vec::with_capacity(built);
    let mut placements: Vec<Placement<PathBuf>> = Vec::new();
    for (group, group_placements) in groups.into_iter().rev() {
        let base = lines.len();
        placements.extend(group_placements.into_iter().map(|p| Placement { row: base + p.row, ..p }));
        lines.extend(group);
    }
    let max_scroll = lines.len().saturating_sub(height);
    app.message_scroll = app.message_scroll.min(max_scroll);
    let top = max_scroll - app.message_scroll;
    let (visible, rows): (Vec<Line>, Vec<Option<PathBuf>>) =
        lines.into_iter().skip(top).take(height).unzip();
    app.regions.history = history_inner;
    app.regions.history_rows = rows;
    frame.render_widget(Paragraph::new(visible).block(history_block), history_area);
    draw_placements(&placements, top, history_inner, frame.buffer_mut(), |path| {
        app.picture_ref(path)
    });

    let mut compose_title = if app.composing {
        format!("Write · {}", app.delivery_mode.label())
    } else {
        format!("Press Enter to write · {}", app.delivery_mode.label())
    };
    if !app.attachments.is_empty() {
        let names: Vec<String> = app
            .attachments
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        compose_title.push_str(&format!(" · 📎 {}", names.join(", ")));
    }
    let compose_block = block(&compose_title, app.composing);
    let inner = compose_block.inner(compose_area);
    app.regions.compose = compose_area;
    // Keep the cursor visible for long input.
    let cursor = app.compose.cursor_column();
    let offset = cursor.saturating_sub(inner.width.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(app.compose.text())
            .scroll((0, offset as u16))
            .block(compose_block),
        compose_area,
    );
    if app.composing {
        frame.set_cursor_position(Position::new(inner.x + (cursor - offset) as u16, inner.y));
    }
}
