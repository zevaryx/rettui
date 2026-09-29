//! Messages tab: conversation list, history with image previews, compose.

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::buffer::Buffer;
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};

use super::{ACCENT, DIM, SELECTED_BG, block, human_bytes, time_label, wrap};
use crate::app::App;
use crate::lxmf::DeliveryMode;
use crate::store::{Message, MessageState};
use crate::term::images::{Placement, draw_placements};

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
                if conversation.muted {
                    spans.push(Span::styled(" muted", Style::default().fg(DIM)));
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
    let mut history_block = block(&title, false).padding(ratatui::widgets::Padding::horizontal(1));
    if app.store.conversations.get(&key).is_some_and(|c| c.muted) {
        history_block = history_block.title_bottom(Line::styled(" notifications off · N turns them on ", Style::default().fg(DIM)));
    }
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
            MessageState::Delivered if message.paper.is_some() => {
                Span::styled(" ✓ paper message · P shows its QR code", Style::default().fg(Color::Green))
            }
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
                if let Some(size) = picture.rows(graphics.as_ref(), max_cols, MAX_PREVIEW_ROWS) {
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
    // Scrolled back to the oldest message kept: say where older ones went.
    let archived = app.store.conversations.get(&key).map_or(0, |c| c.archived);
    if groups.len() == count && archived > 0 {
        let note = format!(
            "{archived} older {} moved to the archive ({})",
            if archived == 1 { "message was" } else { "messages were" },
            app.paths.archive.display()
        );
        let mut rows: Vec<HistoryRow> =
            wrap(&note, inner_width).into_iter().map(|l| (Line::styled(l, Style::default().fg(DIM)), None)).collect();
        rows.push((Line::raw(""), None));
        groups.push((rows, Vec::new()));
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

    let mode = match app.delivery_mode {
        DeliveryMode::Paper => "paper (a QR code to pass on, not sent)",
        mode => mode.label(),
    };
    let mut compose_title =
        if app.composing { format!("Write · {mode}") } else { format!("Press Enter to write · {mode}") };
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

/// Paper modules: pure black and white from the 256-colour cube, which
/// themes leave alone (unlike the 16 named colours), so it scans.
const QR_DARK: Color = Color::Indexed(16);
const QR_LIGHT: Color = Color::Indexed(231);

/// A paper message's QR code over the tab: two modules a cell (half
/// blocks), which makes them square.
pub(super) fn draw_paper(frame: &mut Frame, app: &App) {
    let Some(view) = &app.paper_view else { return };
    let area = frame.area();
    let (cols, rows) = match &view.qr {
        Ok(qr) => (qr.size() as u16, qr.size().div_ceil(2) as u16),
        Err(_) => (0, 0),
    };
    let fits = view.qr.is_ok() && cols + 2 <= area.width && rows + 3 <= area.height;
    let (width, height) = if fits { (cols + 2, rows + 3) } else { (area.width.saturating_sub(4).min(64), 7) };
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height.min(area.height)) / 2,
        width,
        height: height.min(area.height),
    };
    frame.render_widget(Clear, rect);
    let paper_block = block("Paper message", true)
        .title_bottom(Line::styled(" y copy link · s save image · Esc close ", Style::default().fg(DIM)));
    let inner = paper_block.inner(rect);
    frame.render_widget(paper_block, rect);
    match &view.qr {
        Ok(qr) if fits => {
            let caption = Rect { height: 1, ..inner };
            frame.render_widget(
                Paragraph::new("Scan it into the recipient's app (Sideband, rettui…)").style(Style::default().fg(DIM)),
                caption,
            );
            let code = Rect { x: inner.x, y: inner.y + 1, width: cols, height: rows };
            draw_qr(qr, code, frame.buffer_mut());
        }
        _ => {
            let why = match &view.qr {
                Ok(_) => format!(
                    "Its QR code needs {}×{} cells: make the window bigger (or the font smaller), \
                     or copy the link (y) or save the code as an image (s) to pass it on.",
                    cols + 2,
                    rows + 3
                ),
                Err(e) => format!("{e}: copy the link (y) to pass it on."),
            };
            frame.render_widget(Paragraph::new(why).wrap(Wrap { trim: true }), inner);
        }
    }
}

fn draw_qr(qr: &crate::lxmf::paper::Qr, area: Rect, buffer: &mut Buffer) {
    let color = |x: usize, y: usize| if qr.is_dark(x, y) { QR_DARK } else { QR_LIGHT };
    for row in 0..area.height {
        for col in 0..area.width {
            let (x, y) = (col as usize, row as usize * 2);
            if let Some(cell) = buffer.cell_mut(Position::new(area.x + col, area.y + row)) {
                // The lower half of the last row, if the size is odd, is quiet zone.
                cell.set_char('▀').set_fg(color(x, y)).set_bg(color(x, y + 1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::{App, PaperView, test_app};
    use crate::config::Settings;
    use crate::store::Store;

    fn draw(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| super::super::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn a_paper_message_shows_as_a_qr_code_that_fits_or_says_why() {
        let dir = std::env::temp_dir().join(format!("rettui-paper-ui-{}", std::process::id()));
        let mut app = test_app(&dir, Settings::default(), Store::default());
        app.paper_view = Some(PaperView::new(format!("lxm://{}", "a".repeat(300))));
        let screen = draw(&mut app, 120, 50);
        assert!(screen.contains("Paper message") && screen.contains('▀'), "{screen}");
        assert!(screen.contains("y copy link"));
        // Too small: no half a code, but what to do instead.
        let screen = draw(&mut app, 80, 24);
        assert!(!screen.contains('▀') && screen.contains("make the window bigger"), "{screen}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
