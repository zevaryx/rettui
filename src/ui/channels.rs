//! Channels tab: hubs and rooms, chat history, members and the input line.

use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::{ACCENT, DIM, SELECTED_BG, block, wrap};
use crate::app::App;
use crate::app::channels::users::UserAction;
use crate::app::channels::{ChatLine, HubStatus, LineKind, Row};

/// A stable colour per identity, so nicks are easy to tell apart.
fn nick_color(src: Option<&str>) -> Color {
    const PALETTE: [Color; 10] = [
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::Rgb(0xff, 0xa5, 0x5a),
        Color::Rgb(0xb4, 0x8e, 0xff),
        Color::Rgb(0x7f, 0xdb, 0xca),
        Color::Rgb(0xff, 0x8f, 0xb3),
    ];
    let seed = src
        .and_then(|s| u8::from_str_radix(s.get(..2)?, 16).ok())
        .unwrap_or(0);
    PALETTE[seed as usize % PALETTE.len()]
}

/// Split a piece of text (starting at byte `offset` of the message) into
/// spans, with the parts inside `highlights` in `mark`.
fn marked(piece: &str, offset: usize, highlights: &[(usize, usize)], style: Style, mark: Style) -> Vec<Span<'static>> {
    let end = offset + piece.len();
    let mut spans = Vec::new();
    let mut at = offset;
    for &(start, stop) in highlights {
        let (start, stop) = (start.max(at), stop.min(end));
        if start >= stop || !piece.is_char_boundary(start - offset) || !piece.is_char_boundary(stop - offset) {
            continue;
        }
        if start > at {
            spans.push(Span::styled(piece[at - offset..start - offset].to_string(), style));
        }
        spans.push(Span::styled(piece[start - offset..stop - offset].to_string(), mark));
        at = stop;
    }
    if at < end {
        spans.push(Span::styled(piece[at - offset..].to_string(), style));
    }
    spans
}

/// One chat line, wrapped to `width` with continuation lines indented.
/// Returns the rows and the columns of the sender's name on the first row
/// (for clicking it).
fn chat_lines(
    line: &ChatLine,
    hub: &crate::app::channels::Hub,
    width: usize,
    own_nick: &str,
) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    let time = Local
        .timestamp_millis_opt(line.ts as i64)
        .single()
        .map(|t| t.format("%H:%M:%S ").to_string())
        .unwrap_or_default();
    let name = line.nick.clone().unwrap_or_else(|| {
        line.src
            .as_deref()
            .and_then(|s| hex::decode(s).ok())
            .map(|h| hub.name_of(&h))
            .unwrap_or_default()
    });
    let color = if line.own { ACCENT } else { nick_color(line.src.as_deref()) };
    let (prefix, text_style): (Vec<Span<'static>>, Style) = match line.kind {
        LineKind::Msg => (
            vec![Span::styled(format!("<{name}> "), Style::default().fg(color).bold())],
            Style::default(),
        ),
        LineKind::Action => (
            vec![Span::styled(format!("* {name} "), Style::default().fg(color).italic())],
            Style::default().italic(),
        ),
        LineKind::Private if line.own => (
            vec![Span::styled(format!("» to {name}: "), Style::default().fg(Color::LightMagenta).bold())],
            Style::default().fg(Color::LightMagenta),
        ),
        LineKind::Private => (
            vec![Span::styled(format!("» {name} (private): "), Style::default().fg(Color::LightMagenta).bold())],
            Style::default().fg(Color::LightMagenta),
        ),
        LineKind::Notice => (
            vec![Span::styled("-hub- ", Style::default().fg(Color::LightCyan))],
            Style::default().fg(Color::Gray),
        ),
        LineKind::Error => (
            vec![Span::styled("! ", Style::default().fg(Color::LightRed).bold())],
            Style::default().fg(Color::LightRed),
        ),
        LineKind::System => (vec![Span::styled("— ", Style::default().fg(DIM))], Style::default().fg(DIM)),
    };
    let mut text_style = text_style;
    if line.pending.is_some() {
        text_style = text_style.fg(DIM);
    }
    // Only the mention itself stands out, not the whole message.
    let mark = Style::default().fg(Color::Black).bg(Color::Yellow).bold();
    let highlights = line.highlights(own_nick);
    let prefix_width = prefix.iter().map(Span::width).sum::<usize>();
    let lead = time.width() + prefix_width;
    // Other people's names can be clicked (for the user menu).
    let name_cols = (line.src.is_some() && !line.own && matches!(line.kind, LineKind::Msg | LineKind::Action | LineKind::Private))
        .then(|| (time.width(), time.width() + prefix_width));
    // In a narrow view, continuation lines start near the left edge instead
    // of under the text, so the message isn't squeezed into a sliver.
    let narrow = width.saturating_sub(lead) < 24;
    let indent = if narrow { 2 } else { lead };
    let body_width = width.saturating_sub(lead).max(8);
    let rest_width = width.saturating_sub(indent).max(8);
    let mut out = Vec::new();
    let mut first = true;
    let mut paragraph_start = 0;
    for raw in line.text.split('\n') {
        let paragraph = raw.strip_suffix('\r').unwrap_or(raw);
        // Where each wrapped piece is in the message, to place highlights.
        let mut cursor = 0;
        // The first row is narrower than the rest when they're indented less.
        let pieces = match wrap(paragraph, if first { body_width } else { rest_width }).split_first() {
            Some((head, _)) if first && narrow => {
                let rest = paragraph[head.len()..].trim_start();
                std::iter::once(head.clone()).chain(if rest.is_empty() { Vec::new() } else { wrap(rest, rest_width) }).collect()
            }
            _ => wrap(paragraph, if first { body_width } else { rest_width }),
        };
        for piece in pieces {
            let at = paragraph[cursor..].find(piece.as_str()).map_or(cursor, |i| cursor + i);
            cursor = at + piece.len();
            let mut spans = if first {
                let mut head = vec![Span::styled(time.clone(), Style::default().fg(DIM))];
                head.extend(prefix.iter().cloned());
                head
            } else {
                vec![Span::raw(" ".repeat(indent))]
            };
            first = false;
            spans.extend(marked(&piece, paragraph_start + at, &highlights, text_style, mark));
            out.push(Line::from(spans));
        }
        paragraph_start += raw.len() + 1;
    }
    if line.pending.is_some()
        && let Some(last) = out.last_mut()
    {
        last.spans.push(Span::styled(" …", Style::default().fg(DIM)));
    }
    (out, name_cols)
}

pub(super) fn draw_channels(frame: &mut Frame, app: &mut App, area: Rect) {
    let active = app.channels.active();
    let show_members = area.width >= 90 && active.as_ref().is_some_and(|(_, room)| !room.is_empty());
    let [list_area, chat_area, members_area] = Layout::horizontal([
        Constraint::Length(super::side_width(area.width, 30)),
        Constraint::Min(20),
        Constraint::Length(if show_members { 24 } else { 0 }),
    ])
    .areas(area);

    // Hub and room list.
    let list_block = block("Channels", !app.channels.typing);
    app.regions.channel_list = list_block.inner(list_area);
    let rows = app.channels.rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new("No hubs yet.\n\nPress n to add one by address or rrc:// link. NomadNet pages can also link to hubs.")
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(DIM))
                .block(list_block),
            list_area,
        );
    } else {
        let items: Vec<ListItem> = rows
            .iter()
            .map(|row| match row {
                Row::Hub(i) => {
                    let hub = &app.channels.hubs[*i];
                    let (dot, color) = match &hub.status {
                        HubStatus::Connected => ("●", Color::Green),
                        HubStatus::Connecting(_) => ("◌", Color::Yellow),
                        HubStatus::Failed(_) => ("●", Color::Red),
                        HubStatus::Disconnected => ("○", DIM),
                    };
                    let name = hub.hub_name.clone().unwrap_or_else(|| hub.name.clone());
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{dot} "), Style::default().fg(color)),
                        Span::styled(name, Style::default().bold()),
                    ]))
                }
                Row::Room(i, room) => {
                    let hub = &app.channels.hubs[*i];
                    let joined = hub.rooms.contains(room);
                    let style = if joined { Style::default() } else { Style::default().fg(DIM) };
                    let mut spans = vec![Span::styled(format!("  # {room}"), style)];
                    if let Some(count) = hub.unread.get(room).filter(|c| **c > 0) {
                        let bg = if hub.mentions.contains(room) { Color::LightRed } else { Color::Yellow };
                        spans.push(Span::raw(" "));
                        spans.push(Span::styled(format!(" {count} "), Style::default().fg(Color::Black).bg(bg).bold()));
                    }
                    ListItem::new(Line::from(spans))
                }
            })
            .collect();
        let selected = app.channel_row_index(&rows);
        app.channels.list.select(selected);
        let list = List::new(items)
            .block(list_block)
            .highlight_style(Style::default().bg(SELECTED_BG).bold());
        frame.render_stateful_widget(list, list_area, &mut app.channels.list);
    }

    let [history_area, input_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(chat_area);
    app.regions.channel_rooms.clear();
    let Some((index, room)) = active else {
        frame.render_widget(block("", false), chat_area);
        app.regions.channel_history = Rect::default();
        app.regions.channel_input = Rect::default();
        return;
    };
    let hub = &app.channels.hubs[index];
    let title = if room.is_empty() {
        let name = hub.hub_name.clone().unwrap_or_else(|| hub.name.clone());
        format!("{name}  {}", hex::encode(hub.hash))
    } else {
        match hub.topics.get(&room) {
            Some(topic) => format!("#{room} — {topic}"),
            None => format!("#{room}"),
        }
    };
    let history_block = block(&title, false).padding(ratatui::widgets::Padding::horizontal(1));
    let inner = history_block.inner(history_area);
    let width = inner.width as usize;

    // Lines, each optionally a clickable public room (hub view) or user.
    let mut lines: Vec<(Line, Link)> = Vec::new();
    if room.is_empty() {
        let label = |s: &str| Span::styled(format!("{s:<10}"), Style::default().fg(DIM));
        let status = match &hub.status {
            HubStatus::Connected => Span::styled("connected", Style::default().fg(Color::Green)),
            HubStatus::Connecting(step) => Span::styled(format!("connecting: {step}"), Style::default().fg(Color::Yellow)),
            HubStatus::Failed(e) => Span::styled(format!("failed: {e}"), Style::default().fg(Color::Red)),
            HubStatus::Disconnected => Span::styled("disconnected (c to connect)", Style::default().fg(DIM)),
        };
        let nick = hub.nick.clone().unwrap_or_else(|| format!("{} (display name)", app.settings.display_name));
        let auto = if hub.auto_connect { "on" } else { "off" };
        for line in [
            Line::from(vec![label("Status"), status]),
            Line::from(vec![label("Address"), Span::raw(hex::encode(hub.hash)), Span::styled(format!("  {}", hub.aspect), Style::default().fg(DIM))]),
            Line::from(vec![label("Nick"), Span::raw(nick)]),
            Line::from(vec![label("Auto"), Span::raw(format!("reconnect {auto} (a to toggle)"))]),
            Line::from(vec![
                label("Limits"),
                Span::raw(format!("{} bytes per message, {} rooms", hub.limits.max_msg_bytes, hub.limits.max_rooms)),
            ]),
        ] {
            lines.push((line, Link::None));
        }
        if let Some(motd) = &hub.motd {
            lines.push((Line::raw(""), Link::None));
            for piece in motd.lines().flat_map(|l| wrap(l, width)) {
                lines.push((Line::styled(piece, Style::default().fg(Color::LightCyan)), Link::None));
            }
        }
        match &hub.available {
            Some(rooms) if !rooms.is_empty() => {
                lines.push((Line::raw(""), Link::None));
                lines.push((Line::styled("Public rooms (click or /join)", Style::default().bold()), Link::None));
                for (name, topic) in rooms {
                    let joined = hub.rooms.contains(name);
                    let mut spans = vec![Span::styled(
                        format!("  # {name}"),
                        Style::default().fg(if joined { Color::Green } else { ACCENT }).add_modifier(Modifier::UNDERLINED),
                    )];
                    if let Some(topic) = topic {
                        spans.push(Span::styled(format!("  {topic}"), Style::default().fg(DIM)));
                    }
                    lines.push((Line::from(spans), Link::Room(name.clone())));
                }
            }
            Some(_) => lines.push((Line::styled("No public rooms on this hub; /join <name> to create one", Style::default().fg(DIM)), Link::None)),
            None => {}
        }
        lines.push((Line::raw(""), Link::None));
    }
    let own_nick = hub.nick.clone().unwrap_or_else(|| app.settings.display_name.clone());
    for line in hub.buffers.get(&room).map(Vec::as_slice).unwrap_or_default() {
        let (rendered, name_cols) = chat_lines(line, hub, width, &own_nick);
        let user = line.src.as_deref().and_then(|s| hex::decode(s).ok());
        for (i, row) in rendered.into_iter().enumerate() {
            let link = match (i, name_cols, &user) {
                (0, Some((from, to)), Some(id)) => Link::User(id.clone(), from, to),
                _ => Link::None,
            };
            lines.push((row, link));
        }
    }
    let height = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    app.channels.scroll = app.channels.scroll.min(max_scroll);
    let top = max_scroll - app.channels.scroll;
    let mut visible = Vec::new();
    app.regions.channel_users.clear();
    for (row, (line, link)) in lines.into_iter().skip(top).take(height).enumerate() {
        let y = inner.y + row as u16;
        match link {
            Link::Room(name) => app.regions.channel_rooms.push((Rect::new(inner.x, y, inner.width, 1), name)),
            Link::User(id, from, to) => {
                let rect = Rect::new(inner.x + from as u16, y, (to - from) as u16, 1).intersection(inner);
                app.regions.channel_users.push((rect, id));
            }
            Link::None => {}
        }
        visible.push(line);
    }
    app.regions.channel_history = inner;
    frame.render_widget(Paragraph::new(visible).block(history_block), history_area);

    // Input.
    let hub = &app.channels.hubs[index];
    let nick = hub.nick.clone().unwrap_or_else(|| app.settings.display_name.clone());
    let target = if room.is_empty() { "commands".to_string() } else { format!("#{room}") };
    let input_title = if app.channels.typing {
        format!("{target} as {nick}")
    } else {
        format!("Press Enter to write · {target}")
    };
    let input_block = block(&input_title, app.channels.typing);
    let input_inner = input_block.inner(input_area);
    app.regions.channel_input = input_area;
    let cursor = app.channels.input.cursor_column();
    let offset = cursor.saturating_sub(input_inner.width.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(app.channels.input.text())
            .scroll((0, offset as u16))
            .block(input_block),
        input_area,
    );
    if app.channels.typing {
        frame.set_cursor_position(Position::new(input_inner.x + (cursor - offset) as u16, input_inner.y));
    }

    if show_members {
        let members = hub.members_of(&room);
        let own = app.identity_hash.to_vec();
        let members_title = format!("Members {}", members.len());
        let members_block = block(&members_title, false)
            .title_bottom(Line::styled(" m message ", Style::default().fg(DIM)));
        let members_inner = members_block.inner(members_area);
        let items: Vec<ListItem> = members
            .iter()
            .enumerate()
            .map(|(row, (name, id))| {
                let style = if *id == own { Style::default().fg(ACCENT).bold() } else { Style::default() };
                if *id != own && (row as u16) < members_inner.height {
                    let rect = Rect::new(members_inner.x, members_inner.y + row as u16, members_inner.width, 1);
                    app.regions.channel_users.push((rect, id.clone()));
                }
                ListItem::new(Span::styled(name.clone(), style))
            })
            .collect();
        frame.render_widget(List::new(items).block(members_block), members_area);
    }
    draw_popup(frame, app, area);
}

/// What a history row links to.
enum Link {
    None,
    Room(String),
    /// A user, with the columns of their name.
    User(Vec<u8>, usize, usize),
}

/// The member picker or the user menu, over the chat.
fn draw_popup(frame: &mut Frame, app: &mut App, area: Rect) {
    app.regions.channel_popup = Rect::default();
    let (title, items, list, bottom): (String, Vec<ListItem>, &mut ratatui::widgets::ListState, &str) =
        if let Some(menu) = &mut app.channels.menu {
            let user = &menu.user;
            let lxmf = hex::encode(user.lxmf);
            let dim = Style::default().fg(DIM);
            let items = UserAction::ALL
                .iter()
                .map(|action| {
                    let line = match action {
                        UserAction::Whisper if user.whisper => Line::from(vec![Span::raw(" Whisper through the hub  "), Span::styled("/msg", dim)]),
                        UserAction::Whisper => Line::styled(" Whisper (this hub does not pass them)", dim),
                        UserAction::Lxmf if user.lxmf_known => Line::from(vec![Span::raw(" LXMF message  "), Span::styled(lxmf.clone(), dim)]),
                        UserAction::Lxmf => Line::from(vec![Span::raw(" LXMF message  "), Span::styled("(no announce seen yet)", dim)]),
                        UserAction::CopyLxmf => Line::raw(" Copy LXMF address"),
                        UserAction::CopyIdentity => Line::raw(" Copy identity hash"),
                    };
                    ListItem::new(line)
                })
                .collect();
            (user.name.clone(), items, &mut menu.list, " w whisper · l LXMF · Esc close ")
        } else if let Some(picker) = &mut app.channels.picker {
            let items = picker.members.iter().map(|(name, _)| ListItem::new(format!(" {name}"))).collect();
            let title = if picker.room.is_empty() { "Message a user".to_string() } else { format!("Message someone in #{}", picker.room) };
            (title, items, &mut picker.list, " Enter pick · Esc close ")
        } else {
            return;
        };
    let width = 60.min(area.width);
    let height = (items.len() as u16 + 2).min(area.height.saturating_sub(2)).max(3);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + area.height.saturating_sub(height) / 3,
        width,
        height,
    };
    frame.render_widget(ratatui::widgets::Clear, rect);
    let popup_block = block(&title, true).title_bottom(Line::styled(bottom, Style::default().fg(DIM)));
    app.regions.channel_popup = popup_block.inner(rect);
    let widget = List::new(items).block(popup_block).highlight_style(Style::default().bg(SELECTED_BG).bold());
    frame.render_stateful_widget(widget, rect, list);
}
