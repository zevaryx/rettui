//! The message search (`/` in Messages) and the channel search (`/` in
//! Channels): a box over the tab with what's typed, and what's found,
//! newest first.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{accent, block, dim, highlighted, selected_bg};
use crate::app::App;
use crate::app::network::search_terms;
use crate::app::search::snippet_of;

/// Widest a name is shown in a result.
const NAME_WIDTH: usize = 18;

/// When a message was sent: the time today, the day this year, or the date.
fn when(timestamp: f64) -> String {
    crate::clock::short(timestamp)
}

/// Cut `text` to `width` columns, with "…" when cut.
fn fit(text: &str, width: usize) -> String {
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

/// A result as listed: who (or where), when, who said it, and the texts to
/// show a part of.
struct Row<'a> {
    name: String,
    time: String,
    who: &'a str,
    texts: Vec<&'a str>,
}

/// What a search box shows: its title, the query and where its cursor is,
/// what to say before anything's typed, and the Tab hint (if any).
struct Box<'a> {
    title: String,
    query: String,
    cursor: usize,
    selected: usize,
    empty: &'a str,
    scope_hint: &'a str,
}

/// A search over the tab: the box, what's typed after a `/`, how many are
/// found, and the results, the selected one marked.
fn draw_search(frame: &mut Frame, area: Rect, regions: &mut crate::app::Regions, shown: Box, rows: Vec<Row>, max: usize) {
    let width = area.width.saturating_sub(4).min(110);
    let height = area.height.saturating_sub(4).min(26);
    if width < 20 || height < 6 {
        return;
    }
    let rect = Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height };
    let outer = block(&shown.title, true);
    let inner = outer.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(outer, rect);
    regions.message_search_box = rect;

    let field = Rect { x: inner.x + 2, width: inner.width.saturating_sub(2), ..inner };
    frame.render_widget(Paragraph::new(Span::styled("/", Style::default().fg(accent()).bold())), inner);
    let offset = shown.cursor.saturating_sub(field.width.saturating_sub(1) as usize);
    frame.render_widget(Paragraph::new(shown.query.clone()).scroll((0, offset as u16)), Rect { height: 1, ..field });
    frame.set_cursor_position(Position::new(field.x + (shown.cursor - offset) as u16, field.y));

    let terms = search_terms(&shown.query);
    let status = if terms.is_empty() {
        shown.empty.to_string()
    } else if rows.is_empty() {
        "Nothing found".to_string()
    } else if rows.len() >= max {
        format!("The newest {} found", rows.len())
    } else {
        format!("{} found", rows.len())
    };
    let status_row = Rect { y: inner.y + 1, height: 1, ..inner };
    let status_line =
        Line::from(vec![Span::styled(status, Style::default().fg(dim())), Span::styled(shown.scope_hint, Style::default().fg(dim()))]);
    frame.render_widget(Paragraph::new(status_line), status_row);

    let list = Rect { y: inner.y + 3, height: inner.height.saturating_sub(3), ..inner };
    regions.message_search = list;
    let visible = list.height as usize;
    let selected = shown.selected.min(rows.len().saturating_sub(1));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    regions.message_search_first = first;
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(first).take(visible) {
        let name = fit(&row.name, NAME_WIDTH);
        let used = NAME_WIDTH + 2 + 13 + row.who.width();
        let text = snippet_of(&row.texts, &shown.query, (list.width as usize).saturating_sub(used));
        let base = if i == selected { Style::default().bg(selected_bg()) } else { Style::default() };
        let mut spans = vec![
            Span::styled(format!("{name:<NAME_WIDTH$}  "), base.add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:<11}  ", row.time), base.fg(dim())),
            Span::styled(row.who, base.fg(dim())),
        ];
        spans.extend(highlighted(&text, usize::MAX, &terms, base));
        let line_width: usize = spans.iter().map(|s| s.content.width()).sum();
        if i == selected && line_width < list.width as usize {
            spans.push(Span::styled(" ".repeat(list.width as usize - line_width), base));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), list);
}

pub(super) fn draw_message_search(frame: &mut Frame, app: &mut App) {
    let Some(search) = &app.message_search else { return };
    let title = match (search.here, app.active_conversation.as_deref()) {
        (true, Some(key)) => format!("Search messages · {}", app.store.display_name(key)),
        _ => "Search messages · all conversations".to_string(),
    };
    let shown = Box {
        title,
        query: search.input.text().to_string(),
        cursor: search.input.cursor_column(),
        selected: search.selected,
        empty: "Type to find messages: every word, in any order, in their text, title or files",
        scope_hint: if app.active_conversation.is_some() { "   Tab: this conversation / all" } else { "" },
    };
    let hits = app.message_search_hits();
    let rows: Vec<Row> = hits
        .iter()
        .filter_map(|hit| {
            let message = app.store.conversations.get(&hit.key)?.messages.get(hit.index)?;
            let mut texts = vec![message.content.as_str(), message.title.as_str()];
            texts.extend(message.attachments.iter().map(|a| a.name.as_str()));
            texts.extend(message.notes.iter().map(String::as_str));
            Some(Row {
                name: app.store.display_name(&hit.key),
                time: when(hit.timestamp),
                who: if hit.incoming { "" } else { "You: " },
                texts,
            })
        })
        .collect();
    let area = frame.area();
    draw_search(frame, area, &mut app.regions, shown, rows, crate::app::search::MAX_HITS);
}

/// The channel search (`/` in Channels): where each line was said (the
/// room, or whisper conversation, and the hub), when, and by whom.
pub(super) fn draw_channel_search(frame: &mut Frame, app: &mut App) {
    let Some(search) = &app.channel_search else { return };
    let channels = &app.channels;
    let open = channels.active().map(|(i, room)| place(&channels.hubs[i], &room));
    let title = match (search.here, &open) {
        (true, Some(place)) => format!("Search channels · {place}"),
        _ => "Search channels · every room".to_string(),
    };
    let shown = Box {
        title,
        query: search.input.text().to_string(),
        cursor: search.input.cursor_column(),
        selected: search.selected,
        empty: "Type to find what was said: every word, in any order, in the line or who said it",
        scope_hint: if open.is_some() { "   Tab: this room / all" } else { "" },
    };
    let hits = app.channel_search_hits();
    let names: Vec<(String, String)> = hits
        .iter()
        .filter_map(|hit| {
            let hub = channels.hubs.iter().find(|h| h.hash == hit.hub)?;
            let line = hub.buffers.get(&hit.room)?.get(hit.index)?;
            let who = if line.own { "You".to_string() } else { line.nick.clone().unwrap_or_default() };
            Some((place(hub, &hit.room), format!("{who}: ")))
        })
        .collect();
    let rows: Vec<Row> = hits
        .iter()
        .zip(&names)
        .filter_map(|(hit, (name, who))| {
            let hub = channels.hubs.iter().find(|h| h.hash == hit.hub)?;
            let line = hub.buffers.get(&hit.room)?.get(hit.index)?;
            Some(Row { name: name.clone(), time: when(hit.ts as f64 / 1000.0), who: who.as_str(), texts: vec![line.text.as_str()] })
        })
        .collect();
    let area = frame.area();
    draw_search(frame, area, &mut app.regions, shown, rows, crate::app::channels::search::MAX_HITS);
}

/// Where a line was said, as listed: `#room · hub`, `@name · hub` for a
/// whisper conversation, or the hub.
fn place(hub: &crate::app::channels::Hub, room: &str) -> String {
    match hub.whispers().into_iter().find(|(key, _)| key == room) {
        Some((_, name)) => format!("@{name} · {}", hub.name),
        None if room.is_empty() => hub.name.clone(),
        None => format!("#{room} · {}", hub.name),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::store::{Conversation, Message, MessageState, Store};

    #[test]
    fn results_show_who_when_and_where() {
        let dir = std::env::temp_dir().join(format!("rettui-ui-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::default();
        let mut conversation = Conversation::default();
        let text = "Meet at the repeater site on the hill, by the old water tower, at noon tomorrow".to_string();
        conversation.messages.push(Message {
            id: "m1".into(),
            content: text,
            timestamp: 1.0,
            state: MessageState::Delivered,
            ..Default::default()
        });
        store.conversations.insert("aa".repeat(16), conversation);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), store);
        app.tab = crate::app::Tab::Messages;
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in "tower".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let shown: String = (0..20).map(|y| (0..90).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect();
        assert!(shown.contains("Search messages · all conversations") && shown.contains("/ tower") && shown.contains("1 found"), "{shown}");
        assert!(shown.contains("You: ") && shown.contains("tower"), "{shown}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn channel_results_show_where_who_and_what() {
        use crate::app::channels::{ChatLine, Hub, LineKind};
        let dir = std::env::temp_dir().join(format!("rettui-ui-channel-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), Store::default());
        let mut hub = Hub::new([1; 16], "rrc.hub".into(), "Hilltop".into());
        let mut line = ChatLine::presence("the repeater on the hill is up again".into());
        (line.kind, line.presence, line.nick, line.ts) = (LineKind::Msg, false, Some("ann".into()), 1_790_000_000_000);
        hub.buffers.insert("general".into(), vec![line]);
        app.channels.hubs.push(hub);
        app.tab = crate::app::Tab::Channels;
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in "hill".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let shown: String = (0..20).map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect();
        assert!(shown.contains("Search channels · every room") && shown.contains("1 found"), "{shown}");
        assert!(shown.contains("#general · Hilltop") && shown.contains("ann: the repeater on the hill"), "{shown}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
