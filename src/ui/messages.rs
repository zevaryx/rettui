//! Messages tab: conversation list, history with image previews, compose.

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};

use unicode_width::UnicodeWidthStr;

use super::{accent, block, dim, human_bytes, picked_style, selected_bg, time_label, wrap};
use crate::app::{App, HistoryHit, QrKind};
use crate::lxmf::DeliveryMode;
use crate::markdown::{self, TextFormat};
use crate::store::{Message, MessageState, Reaction};
use crate::term::images::{Placement, draw_placements};

/// Widest inline image preview in the message history.
const MAX_PREVIEW_COLS: usize = 48;
const MAX_PREVIEW_ROWS: usize = 12;

/// A history row, with what a click on it does.
type HistoryRow = (Line<'static>, Option<HistoryHit>);
/// A button on a history row: its row, first column, width, and what it does.
type HistoryButton = (usize, usize, usize, HistoryHit);
/// A message's rows (or a note's: no message), and image placements and
/// buttons from its first row.
type HistoryGroup = (Option<usize>, Vec<HistoryRow>, Vec<Placement<PathBuf>>, Vec<HistoryButton>);

/// A message's reactions, each emoji with who reacted with it (you, or
/// `name`), in the order they came, in rows of at most `width` columns.
fn reaction_rows(reactions: &[Reaction], name: &str, width: usize) -> Vec<Line<'static>> {
    let mut emoji: Vec<(&str, Vec<String>)> = Vec::new();
    for reaction in reactions {
        let who = if !reaction.incoming {
            match &reaction.state {
                MessageState::Sending => "You (sending…)".to_string(),
                MessageState::Failed(_) => "You (failed)".to_string(),
                _ => "You".to_string(),
            }
        } else {
            name.to_string()
        };
        match emoji.iter_mut().find(|(e, _)| *e == reaction.emoji) {
            Some((_, people)) => people.push(who),
            None => emoji.push((&reaction.emoji, vec![who])),
        }
    }
    let mut rows: Vec<Line<'static>> = vec![Line::default()];
    let mut used = 0;
    for (emoji, people) in emoji {
        let (chip, who) = (format!("{emoji} "), people.join(", "));
        let needed = chip.width() + who.width();
        if used > 0 && used + 2 + needed > width {
            rows.push(Line::default());
            used = 0;
        }
        let row = rows.last_mut().expect("there is a row");
        if used > 0 {
            row.spans.push(Span::raw("  "));
            used += 2;
        }
        row.spans.push(Span::styled(chip, Style::default().bg(selected_bg())));
        row.spans.push(Span::styled(who, Style::default().fg(dim())));
        used += needed;
    }
    rows
}

/// `text` cut to `width` columns, with an ellipsis if it was longer.
fn clipped(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        if out.width() + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) + 1 > width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

pub(super) fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let [list_area, chat_area] =
        Layout::horizontal([Constraint::Length(super::side_width(area.width, 30)), Constraint::Min(20)]).areas(area);

    let order = app.conversation_order();
    let list_block = block("Conversations", !app.composing);
    app.regions.conversations = list_block.inner(list_area);
    if order.is_empty() {
        let hint = Paragraph::new("No conversations yet.\n\nPress n to message an LXMF address, or pick a peer in the Network tab.")
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(dim()))
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
                    spans.push(Span::styled(" muted", Style::default().fg(dim())));
                }
                if conversation.pinned {
                    spans.push(Span::styled(" pinned", Style::default().fg(accent())));
                }
                if app.is_request(key) {
                    spans.push(Span::styled(" request", Style::default().fg(Color::Black).bg(Color::Yellow)));
                } else if !app.is_known(key) {
                    spans.push(Span::styled(" ?", Style::default().fg(Color::Yellow)));
                }
                let preview = conversation.messages.last().map(Message::opening).unwrap_or_default();
                ListItem::new(vec![Line::from(spans), Line::styled(format!("  {preview}"), Style::default().fg(dim()))])
            })
            .collect();
        let list = List::new(items).block(list_block).highlight_style(Style::default().bg(selected_bg()).bold()).highlight_symbol("▌");
        frame.render_stateful_widget(list, list_area, &mut app.conversations);
    }

    let [history_area, compose_area] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(chat_area);

    let Some(key) = app.active_conversation.clone() else {
        frame.render_widget(block("", false), chat_area);
        app.regions.history = Rect::default();
        app.regions.compose = Rect::default();
        return;
    };
    let name = app.store.display_name(&key);
    let title = format!("{name}  {key}");
    let mut history_block = block(&title, false).padding(ratatui::widgets::Padding::horizontal(1));
    if !app.is_known(&key) {
        history_block = history_block
            .title_bottom(Line::styled(" not one of your contacts · c to trust or block them ", Style::default().fg(Color::Yellow)));
    } else if app.store.conversations.get(&key).is_some_and(|c| c.muted) {
        history_block = history_block.title_bottom(Line::styled(" notifications off · N turns them on ", Style::default().fg(dim())));
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
    // The message being replied to, and one to bring into view (built even
    // if it's further back than what's on screen).
    let target = app.reply_target().map(|(index, _)| index);
    let picked = app.picked_message().map(|(index, _)| index);
    let scroll_to = app.scroll_to.take();
    let mut groups: Vec<HistoryGroup> = Vec::new();
    let mut built = 0;
    for index in (0..count).rev() {
        if built >= needed && scroll_to.is_none_or(|wanted| index < wanted) {
            break;
        }
        let message: Message = app.store.conversations[&key].messages[index].clone();
        let quoted = message.reply.as_ref().map(|reply| app.store.conversations[&key].quoted(reply));
        // Each history line carries the attachment it belongs to (for clicks).
        let mut lines: Vec<HistoryRow> = Vec::new();
        let mut placements: Vec<Placement<PathBuf>> = Vec::new();
        let mut buttons: Vec<HistoryButton> = Vec::new();
        let (who, color) = if message.incoming { (name.as_str(), Color::LightMagenta) } else { ("You", accent()) };
        let status = match &message.state {
            MessageState::Received { verified: true } => Span::raw(""),
            MessageState::Received { verified: false } => Span::styled(" unverified", Style::default().fg(Color::Yellow)),
            MessageState::Sending => Span::styled(" sending…", Style::default().fg(dim())),
            MessageState::Delivered if message.paper.is_some() => {
                Span::styled(" ✓ paper message · P shows its QR code", Style::default().fg(Color::Green))
            }
            MessageState::Delivered => Span::styled(" ✓", Style::default().fg(Color::Green)),
            MessageState::Propagated => Span::styled(" ✓ via propagation node", Style::default().fg(Color::Green)),
            MessageState::Failed(e) => Span::styled(format!(" failed: {e}"), Style::default().fg(Color::Red)),
        };
        let mut header = Line::from(vec![
            Span::styled(who.to_string(), Style::default().fg(color).bold()),
            Span::styled(format!("  {}", time_label(message.timestamp)), Style::default().fg(dim())),
            status,
        ]);
        if target == Some(index) {
            header.spans.push(Span::styled("  ↩ replying to this", Style::default().fg(accent())));
            header = header.style(Style::default().bg(selected_bg()));
        }
        if picked == Some(index) {
            header = header.style(Style::default().bg(selected_bg()));
        }
        lines.push((header, Some(HistoryHit::Pick(index))));
        // The picked message's buttons, under its name.
        if picked == Some(index) {
            let mut spans = Vec::new();
            let mut x = 0;
            for action in App::message_actions(&message) {
                let (label, key) = action.label();
                let text = format!(" {label} ({key}) ");
                let width = text.width();
                buttons.push((lines.len(), x, width, HistoryHit::Action(index, action)));
                spans.push(Span::styled(text, picked_style()));
                spans.push(Span::raw(" "));
                x += width + 1;
            }
            spans.push(Span::styled("↑↓ another · Esc done", Style::default().fg(dim())));
            lines.push((Line::from(spans), None));
        }
        // What it answers: a click shows that message.
        if let Some(quoted) = quoted {
            let author = match quoted.incoming {
                Some(true) => format!("{name}: "),
                Some(false) => "You: ".to_string(),
                None => String::new(),
            };
            let text = clipped(&format!("{author}{}", quoted.text), inner_width.saturating_sub(2));
            lines.push((
                Line::from(vec![
                    Span::styled("▎ ", Style::default().fg(accent())),
                    Span::styled(text, Style::default().fg(dim()).add_modifier(Modifier::ITALIC)),
                ]),
                quoted.index.map(HistoryHit::Original),
            ));
        }
        if !message.title.is_empty() {
            lines.push((Line::styled(message.title.clone(), Style::default().bold()), None));
        }
        if !message.content.is_empty() {
            match message.format {
                Some(TextFormat::Markdown) => {
                    lines.extend(markdown::lines(&message.content, inner_width).into_iter().map(|line| (line, None)));
                }
                Some(TextFormat::Micron) => {
                    let page = crate::nomad::micron::parse(&message.content);
                    let layout = page.layout(inner_width, None, &std::collections::HashMap::new(), None);
                    lines.extend(layout.lines.into_iter().map(|line| (line, None)));
                }
                None => {
                    for line in wrap(&message.content, inner_width) {
                        lines.push((Line::raw(line), None));
                    }
                }
            }
        }
        // A location: a click shows it on a map.
        if let Some(location) = message.location {
            let url = location.map_url();
            lines.push((
                Line::from(vec![
                    Span::raw("📍 "),
                    Span::styled(location.label(), Style::default().add_modifier(Modifier::UNDERLINED)),
                    Span::styled("  map ↗", Style::default().fg(dim())),
                ]),
                Some(HistoryHit::Link(url)),
            ));
        }
        for note in &message.notes {
            for line in wrap(note, inner_width) {
                lines.push((Line::styled(line, Style::default().fg(dim()).add_modifier(Modifier::ITALIC)), None));
            }
        }
        for attachment in &message.attachments {
            let path = Some(HistoryHit::File(attachment.path.clone()));
            if attachment.image
                && let Some(picture) = app.picture(&attachment.path)
            {
                let max_cols = inner_width.min(MAX_PREVIEW_COLS);
                if let Some(size) = picture.rows(graphics.as_ref(), max_cols, MAX_PREVIEW_ROWS) {
                    placements.push(Placement { row: lines.len(), col: 0, size, key: attachment.path.clone() });
                    for _ in 0..size.height {
                        lines.push((Line::raw(""), path.clone()));
                    }
                }
            }
            let line = match &attachment.voice {
                Some(codec) => {
                    let note = if attachment.playable() {
                        format!("  {codec} · {}", human_bytes(attachment.size))
                    } else {
                        format!("  {codec} (rettui can't play it) · {}", human_bytes(attachment.size))
                    };
                    Line::from(vec![
                        Span::styled("🎤 ", Style::default().fg(Color::Yellow)),
                        Span::styled("Voice message", Style::default().add_modifier(Modifier::UNDERLINED)),
                        Span::styled(note, Style::default().fg(dim())),
                    ])
                }
                None => Line::from(vec![
                    Span::styled("📎 ", Style::default().fg(Color::Yellow)),
                    Span::styled(attachment.name.clone(), Style::default().add_modifier(Modifier::UNDERLINED)),
                    Span::styled(format!("  {}", human_bytes(attachment.size)), Style::default().fg(dim())),
                ]),
            };
            lines.push((line, path));
        }
        if !message.reactions.is_empty() {
            lines.extend(reaction_rows(&message.reactions, &name, inner_width).into_iter().map(|row| (row, None)));
        }
        lines.push((Line::raw(""), None));
        built += lines.len();
        groups.push((Some(index), lines, placements, buttons));
    }
    // Scrolled back to the oldest message kept: say where older ones went.
    let archived = app.store.conversations.get(&key).map_or(0, |c| c.archived);
    if groups.len() == count && archived > 0 {
        let note = format!(
            "{archived} older {} moved to the archive ({}): H shows {}",
            if archived == 1 { "message was" } else { "messages were" },
            app.paths.archive.display(),
            if archived == 1 { "it" } else { "them" },
        );
        let mut rows: Vec<HistoryRow> =
            wrap(&note, inner_width).into_iter().map(|l| (Line::styled(l, Style::default().fg(dim())), None)).collect();
        rows.push((Line::raw(""), None));
        groups.push((None, rows, Vec::new(), Vec::new()));
    }
    // Oldest first, with placements at their rows in the whole list.
    let mut lines: Vec<HistoryRow> = Vec::with_capacity(built);
    let mut placements: Vec<Placement<PathBuf>> = Vec::new();
    let mut buttons: Vec<HistoryButton> = Vec::new();
    let mut wanted_row = None;
    for (index, group, group_placements, group_buttons) in groups.into_iter().rev() {
        let base = lines.len();
        if index.is_some() && index == scroll_to {
            wanted_row = Some(base);
        }
        placements.extend(group_placements.into_iter().map(|p| Placement { row: base + p.row, ..p }));
        buttons.extend(group_buttons.into_iter().map(|(row, x, width, hit)| (base + row, x, width, hit)));
        lines.extend(group);
    }
    let max_scroll = lines.len().saturating_sub(height);
    app.message_scroll = app.message_scroll.min(max_scroll);
    // Scroll so the message asked for starts on screen (a row of what's
    // before it showing too), if it isn't already.
    if let Some(row) = wanted_row {
        let top = max_scroll - app.message_scroll;
        if row < top || row >= top + height {
            app.message_scroll = max_scroll - row.saturating_sub(1).min(max_scroll);
        }
    }
    let top = max_scroll - app.message_scroll;
    let (visible, rows): (Vec<Line>, Vec<Option<HistoryHit>>) = lines.into_iter().skip(top).take(height).unzip();
    app.regions.history = history_inner;
    app.regions.history_rows = rows;
    app.regions.history_buttons = buttons
        .into_iter()
        .filter(|(row, ..)| (top..top + height).contains(row))
        .map(|(row, x, width, hit)| {
            let rect = Rect::new(history_inner.x + x as u16, history_inner.y + (row - top) as u16, width as u16, 1);
            (rect.intersection(history_inner), hit)
        })
        .collect();
    frame.render_widget(Paragraph::new(visible).block(history_block), history_area);
    draw_placements(&placements, top, history_inner, frame.buffer_mut(), |path| app.picture_ref(path));

    let mode = match app.delivery_mode {
        DeliveryMode::Paper => "paper (a QR code to pass on, not sent)",
        mode => mode.label(),
    };
    let mut compose_title = if app.composing { format!("Write · {mode}") } else { format!("Press Enter to write · {mode}") };
    if !app.attachments.is_empty() {
        let names: Vec<String> = app
            .attachments
            .iter()
            .filter_map(|p| {
                let name = p.file_name()?.to_string_lossy().into_owned();
                // Pictures go smaller, as the setting has it.
                let smaller = crate::app::shrink::shrinks(p, &app.settings.picture_size);
                Some(if smaller { format!("{name} (smaller)") } else { name })
            })
            .collect();
        compose_title.push_str(&format!(" · 📎 {}", names.join(", ")));
    }
    let mut compose_block = block(&compose_title, app.composing);
    if let Some((_, target)) = app.reply_target() {
        let author = if target.incoming { name.as_str() } else { "You" };
        let hint = if app.composing { " · ↑↓ another · Esc cancels " } else { " " };
        let room = (compose_area.width as usize).saturating_sub(hint.width() + 6);
        let text = clipped(&format!("↩ {author}: {}", target.opening()), room);
        compose_block = compose_block.title_bottom(Line::from(vec![
            Span::styled(format!(" {text}"), Style::default().fg(accent())),
            Span::styled(hint, Style::default().fg(dim())),
        ]));
    }
    let inner = compose_block.inner(compose_area);
    app.regions.compose = compose_area;
    // Keep the cursor visible for long input.
    let cursor = app.compose.cursor_column();
    let offset = cursor.saturating_sub(inner.width.saturating_sub(1) as usize);
    frame.render_widget(Paragraph::new(app.compose.text()).scroll((0, offset as u16)).block(compose_block), compose_area);
    if app.composing {
        frame.set_cursor_position(Position::new(inner.x + (cursor - offset) as u16, inner.y));
    }
    super::emoji::draw_emoji(frame, app, inner, offset, history_area);
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
    let (title, caption) = match view.kind {
        QrKind::Paper => ("Paper message", "Scan it into the recipient's app (Sideband, rettui…)"),
        QrKind::Address => ("Your address", "Scan it in Columba or rettui to add you as a contact"),
    };
    let paper_block = block(title, true).title_bottom(Line::styled(" y copy link · s save image · Esc close ", Style::default().fg(dim())));
    let inner = paper_block.inner(rect);
    frame.render_widget(paper_block, rect);
    match &view.qr {
        Ok(qr) if fits => {
            let caption_area = Rect { height: 1, ..inner };
            frame.render_widget(Paragraph::new(caption).style(Style::default().fg(dim())), caption_area);
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

    #[test]
    fn markdown_messages_show_formatted_and_sent_ones_are_marked() {
        use crate::markdown::TextFormat;
        use crate::store::{Conversation, Message, MessageState};
        let dir = std::env::temp_dir().join(format!("rettui-markdown-ui-{}", std::process::id()));
        let key = "ab".repeat(16);
        let message = Message {
            id: "m1".into(),
            incoming: true,
            content: "Meet at **noon**\n- bring `tea`\n- and cake".into(),
            state: MessageState::Received { verified: true },
            format: Some(TextFormat::Markdown),
            ..Message::default()
        };
        let mut store = Store::default();
        store.conversations.insert(key.clone(), Conversation { messages: vec![message], ..Default::default() });
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, Settings::default(), store);
        app.open_newest_conversation();
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("Meet at noon") && screen.contains("• bring tea") && !screen.contains("**"), "{screen}");
        // The list's preview leaves the markup out too.
        assert!(screen.contains("Meet at noon") && !screen.contains("`tea`"), "{screen}");
        // What you write goes marked as Markdown (unless that's off).
        app.send_message(key.clone(), "*hi*".into(), Vec::new(), crate::lxmf::DeliveryMode::Direct, None).unwrap();
        let sent = std::iter::from_fn(|| net.try_recv().ok()).find_map(|c| match c {
            crate::net::NetCommand::SendMessage { message, .. } => Some(message.format),
            _ => None,
        });
        assert_eq!(sent, Some(Some(TextFormat::Markdown)));
        app.settings.markdown_messages = false;
        app.send_message(key.clone(), "*hi*".into(), Vec::new(), crate::lxmf::DeliveryMode::Direct, None).unwrap();
        let sent = std::iter::from_fn(|| net.try_recv().ok()).find_map(|c| match c {
            crate::net::NetCommand::SendMessage { message, .. } => Some(message.format),
            _ => None,
        });
        assert_eq!(sent, Some(None));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replies_are_picked_shown_and_sent() {
        use crate::net::PeerKind;
        use crate::store::{Conversation, Message, MessageState, Peer};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let dir = std::env::temp_dir().join(format!("rettui-replies-ui-{}", std::process::id()));
        let key = "ab".repeat(16);
        let said = |n: u8, content: &str| Message {
            id: hex::encode([n; 32]),
            incoming: true,
            content: content.into(),
            timestamp: 1_700_000_000.0 + f64::from(n),
            state: MessageState::Received { verified: true },
            ..Message::default()
        };
        let mut store = Store::default();
        store.peers.insert(key.clone(), Peer { kind: PeerKind::Lxmf, name: Some("Alice".into()), hops: 1, last_seen: 0 });
        let messages = vec![said(1, "Lunch tomorrow?"), said(2, "Or Friday")];
        store.conversations.insert(key.clone(), Conversation { messages, ..Default::default() });
        let mut app = test_app(&dir, Settings::default(), store);
        app.open_newest_conversation();
        let press = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        // `r` replies to their newest; ↑ goes to the one before.
        press(&mut app, KeyCode::Char('r'));
        assert!(app.composing);
        assert_eq!(app.reply.as_deref(), Some(hex::encode([2u8; 32]).as_str()));
        press(&mut app, KeyCode::Up);
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("↩ replying to this") && screen.contains("↩ Alice: Lunch tomorrow?"), "{screen}");
        // Sent, it shows what it answers; Esc first gives up a reply.
        for c in "Yes".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.reply.is_none());
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("▎ Alice: Lunch tomorrow?"), "{screen}");
        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(app.reply.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.reply.is_none() && app.composing);
        // A click on a message's first row picks it, and its Reply button
        // replies to it; one on a quote brings back what it answers.
        draw(&mut app, 100, 30);
        let click_at = |app: &mut App, x: u16, y: u16| {
            app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
        };
        let click = |app: &mut App, hit: &crate::app::HistoryHit| {
            let row = app.regions.history_rows.iter().position(|h| h.as_ref() == Some(hit)).unwrap();
            click_at(app, app.regions.history.x, app.regions.history.y + row as u16);
        };
        click(&mut app, &crate::app::HistoryHit::Pick(1));
        assert_eq!(app.picked.as_deref(), Some(hex::encode([2u8; 32]).as_str()));
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("↩ Reply (r)") && screen.contains("React (e)"), "{screen}");
        let reply = crate::app::HistoryHit::Action(1, crate::app::MessageAction::Reply);
        let (rect, _) = app.regions.history_buttons.iter().find(|(_, hit)| *hit == reply).unwrap().clone();
        click_at(&mut app, rect.x + 1, rect.y);
        assert_eq!(app.reply.as_deref(), Some(hex::encode([2u8; 32]).as_str()));
        assert!(app.picked.is_none() && app.composing);
        app.composing = false;
        draw(&mut app, 100, 30);
        click(&mut app, &crate::app::HistoryHit::Original(0));
        assert_eq!(app.scroll_to, Some(0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reactions_locations_notes_and_voice_are_drawn() {
        use crate::lxmf::Location;
        use crate::store::{Conversation, Message, MessageState, Reaction, StoredAttachment};
        let dir = std::env::temp_dir().join(format!("rettui-extras-ui-{}", std::process::id()));
        let key = "ab".repeat(16);
        let reaction = |emoji: &str, incoming: bool, state: MessageState| Reaction {
            emoji: emoji.into(),
            incoming,
            timestamp: 0.0,
            id: emoji.into(),
            state,
        };
        let messages = vec![
            Message {
                id: hex::encode([1u8; 32]),
                incoming: true,
                content: "Here".into(),
                state: MessageState::Received { verified: true },
                location: Some(Location {
                    latitude: 51.5,
                    longitude: -0.12,
                    altitude: None,
                    speed: None,
                    bearing: None,
                    accuracy: Some(8.0),
                    updated: None,
                }),
                notes: vec!["Stopped sharing their location".into()],
                reactions: vec![
                    reaction("👍", true, MessageState::Received { verified: true }),
                    reaction("👍", false, MessageState::Delivered),
                    reaction("❤️", false, MessageState::Failed("x".into())),
                ],
                ..Message::default()
            },
            Message {
                id: hex::encode([2u8; 32]),
                incoming: true,
                state: MessageState::Received { verified: true },
                attachments: vec![StoredAttachment {
                    name: "voice-message-1200.codec2".into(),
                    path: dir.join("voice-message-1200.codec2"),
                    size: 1200,
                    image: false,
                    voice: Some("Codec2 1200".into()),
                }],
                ..Message::default()
            },
        ];
        let mut store = Store::default();
        store.conversations.insert(key.clone(), Conversation { messages, ..Default::default() });
        let mut app = test_app(&dir, Settings::default(), store);
        app.open_newest_conversation();
        let screen = draw(&mut app, 110, 30);
        assert!(screen.contains("51.50000, -0.12000 (±8 m)  map ↗"), "{screen}");
        assert!(screen.contains("Stopped sharing their location"));
        assert!(screen.contains("<abababababab>, You") && screen.contains("You (failed)"), "{screen}");
        // Too narrow for both: the second goes on a row of its own.
        let narrow = draw(&mut app, 64, 30);
        assert!(narrow.lines().any(|l| l.contains("You (failed)") && !l.contains("<abababababab>, You")), "{narrow}");
        assert!(screen.contains("Voice message  Codec2 1200 (rettui can't play it)"), "{screen}");
        assert!(screen.contains("Voice message   "), "the list shows what the newest brought");
        // A click on the location opens it on a map (a link hit).
        let row = app.regions.history_rows.iter().position(|h| matches!(h, Some(crate::app::HistoryHit::Link(_)))).unwrap();
        let Some(crate::app::HistoryHit::Link(url)) = app.regions.history_rows[row].clone() else { unreachable!() };
        assert!(url.starts_with("https://www.openstreetmap.org/?mlat=51.500000"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
