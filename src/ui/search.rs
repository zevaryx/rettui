//! The message search (`/` in Messages): a box over the tab with what's
//! typed, and the messages found, newest first.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{accent, dim, selected_bg, block, highlighted};
use crate::app::network::search_terms;
use crate::app::search::snippet;
use crate::app::App;

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

pub(super) fn draw_message_search(frame: &mut Frame, app: &mut App) {
    let Some(search) = &app.message_search else { return };
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(110);
    let height = area.height.saturating_sub(4).min(26);
    if width < 20 || height < 6 {
        return;
    }
    let rect = Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height };
    let scope = match (search.here, app.active_conversation.as_deref()) {
        (true, Some(key)) => format!("Search messages · {}", app.store.display_name(key)),
        _ => "Search messages · all conversations".to_string(),
    };
    let outer = block(&scope, true);
    let inner = outer.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(outer, rect);
    app.regions.message_search_box = rect;

    // What's typed, after a `/`.
    let query = search.input.text().to_string();
    let field = Rect { x: inner.x + 2, width: inner.width.saturating_sub(2), ..inner };
    frame.render_widget(Paragraph::new(Span::styled("/", Style::default().fg(accent()).bold())), inner);
    let cursor = search.input.cursor_column();
    let offset = cursor.saturating_sub(field.width.saturating_sub(1) as usize);
    frame.render_widget(Paragraph::new(query.clone()).scroll((0, offset as u16)), Rect { height: 1, ..field });
    frame.set_cursor_position(Position::new(field.x + (cursor - offset) as u16, field.y));

    let hits = app.message_search_hits();
    let terms = search_terms(&query);
    let status = if terms.is_empty() {
        "Type to find messages: every word, in any order, in their text, title or files".to_string()
    } else if hits.is_empty() {
        "Nothing found".to_string()
    } else if hits.len() >= crate::app::search::MAX_HITS {
        format!("The newest {} found", hits.len())
    } else {
        format!("{} found", hits.len())
    };
    let scope_hint = if app.active_conversation.is_some() { "   Tab: this conversation / all" } else { "" };
    let status_row = Rect { y: inner.y + 1, height: 1, ..inner };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(status, Style::default().fg(dim())),
            Span::styled(scope_hint, Style::default().fg(dim())),
        ])),
        status_row,
    );

    let list = Rect { y: inner.y + 3, height: inner.height.saturating_sub(3), ..inner };
    app.regions.message_search = list;
    let rows = list.height as usize;
    let selected = search.selected.min(hits.len().saturating_sub(1));
    let first = selected.saturating_sub(rows.saturating_sub(1));
    app.regions.message_search_first = first;
    let mut lines = Vec::new();
    for (i, hit) in hits.iter().enumerate().skip(first).take(rows) {
        let Some(message) = app.store.conversations.get(&hit.key).and_then(|c| c.messages.get(hit.index)) else { continue };
        let name = fit(&app.store.display_name(&hit.key), NAME_WIDTH);
        let time = when(hit.timestamp);
        let who = if hit.incoming { "" } else { "You: " };
        let used = NAME_WIDTH + 2 + 13 + who.len();
        let text = snippet(message, &query, (list.width as usize).saturating_sub(used));
        let base = if i == selected { Style::default().bg(selected_bg()) } else { Style::default() };
        let mut spans = vec![
            Span::styled(format!("{name:<NAME_WIDTH$}  "), base.add_modifier(Modifier::BOLD)),
            Span::styled(format!("{time:<11}  "), base.fg(dim())),
            Span::styled(who, base.fg(dim())),
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
        conversation.messages.push(Message { id: "m1".into(), content: text, timestamp: 1.0, state: MessageState::Delivered, ..Default::default() });
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
}

