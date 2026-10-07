//! A conversation's archived messages (`H` in Messages), over the tab:
//! oldest at the top, the newest at the bottom where it opens, only to read.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{accent, dim, block, wrap};
use crate::app::App;

pub(super) fn draw_archive(frame: &mut Frame, app: &mut App) {
    let Some(reader) = &app.archive_reader else { return };
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(110);
    let height = area.height.saturating_sub(2);
    if width < 20 || height < 5 {
        return;
    }
    let rect = Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height };
    let name = app.store.display_name(&reader.key);
    let count = reader.messages.len();
    let title = format!("Archive · {name} · {count} {}", if count == 1 { "message" } else { "messages" });
    let outer = block(&title, true)
        .title_bottom(Line::styled(" ↑↓ PgUp PgDn Home End scroll · Esc close ", Style::default().fg(dim())));
    let inner = outer.inner(rect);
    let columns = inner.width as usize;

    let dim = Style::default().fg(dim());
    let mut lines: Vec<Line> = Vec::new();
    for message in &reader.messages {
        let (who, colour) = if message.incoming { (name.as_str(), Color::LightMagenta) } else { ("You", accent()) };
        let when = crate::clock::full(message.timestamp);
        lines.push(Line::from(vec![
            Span::styled(who.to_string(), Style::default().fg(colour).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {when}"), dim),
        ]));
        if !message.title.trim().is_empty() {
            lines.extend(wrap(&message.title, columns).into_iter().map(|l| Line::styled(l, Style::default().add_modifier(Modifier::BOLD))));
        }
        if !message.content.trim().is_empty() {
            lines.extend(wrap(&message.text(), columns).into_iter().map(Line::raw));
        }
        for attachment in &message.attachments {
            let mark = if attachment.voice.is_some() { "🎤" } else { "📎" };
            let gone = if attachment.path.exists() { "" } else { " (file missing)" };
            lines.extend(wrap(&format!("{mark} {}{gone}", attachment.name), columns).into_iter().map(|l| Line::styled(l, dim)));
        }
        if let Some(location) = message.location {
            lines.push(Line::styled(format!("📍 {}", location.label()), dim));
        }
        for note in &message.notes {
            lines.extend(wrap(note, columns).into_iter().map(|l| Line::styled(l, dim)));
        }
        lines.push(Line::raw(""));
    }

    // Scrolled up from the newest, no further than the oldest.
    let rows = inner.height as usize;
    let most = lines.len().saturating_sub(rows);
    let scroll = reader.scroll.min(most);
    let first = most - scroll;
    let shown: Vec<Line> = lines.into_iter().skip(first).take(rows).collect();
    frame.render_widget(Clear, rect);
    frame.render_widget(outer, rect);
    frame.render_widget(Paragraph::new(shown), inner);
    app.regions.archive = rect;
    if let Some(reader) = app.archive_reader.as_mut() {
        reader.scroll = scroll;
        reader.page = rows.saturating_sub(1).max(1);
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::archive::ArchiveReader;
    use crate::store::{Conversation, Message, Store};

    fn screen(app: &mut crate::app::App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..16).map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn it_opens_at_the_newest_and_scrolls_back_to_the_oldest() {
        let dir = std::env::temp_dir().join(format!("rettui-ui-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = "ab".repeat(16);
        let mut store = Store::default();
        store.conversations.insert(key.clone(), Conversation::default());
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), store);
        app.tab = crate::app::Tab::Messages;
        let messages = (0..20)
            .map(|n| Message {
                id: format!("m{n}"),
                incoming: n % 2 == 0,
                content: format!("archived number {n}"),
                timestamp: 1_700_000_000.0 + f64::from(n) * 60.0,
                ..Message::default()
            })
            .collect();
        app.archive_reader = Some(ArchiveReader { key, messages, scroll: 0, page: 10 });
        let shown = screen(&mut app);
        assert!(shown.contains("Archive · <abababababab> · 20 messages") || shown.contains("· 20 messages"), "{shown}");
        assert!(shown.contains("archived number 19") && !shown.contains("archived number 0 "), "{shown}");
        app.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        let shown = screen(&mut app);
        assert!(shown.contains("archived number 0") && !shown.contains("archived number 19"), "{shown}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
