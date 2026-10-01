//! The contact card over the Messages tab (`c`): who someone is, what you
//! keep about them, and buttons for what can be done.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{ACCENT, DIM, PICKED, ago, block, wrap};
use crate::app::App;
use crate::app::contacts::{CardAction, PingState, ping_label, trust_label};

/// The card's widest, in columns.
const WIDTH: u16 = 64;

/// A label and its value, on one row.
fn field(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{label:<10}"), Style::default().fg(DIM)), Span::raw(value.into())])
}

/// A button placed in rows: its row, first column, width and action.
type Placed = (usize, usize, usize, CardAction);

/// Rows of buttons, as wide as `width` allows, and where each one is.
fn button_rows(actions: &[CardAction], width: usize) -> (Vec<Line<'static>>, Vec<Placed>) {
    let (mut rows, mut placed) = (vec![Line::default()], Vec::new());
    let mut x = 0;
    for &action in actions {
        let (label, key) = action.label();
        let text = format!(" {label} ({key}) ");
        let w = text.width();
        if x > 0 && x + w > width {
            rows.push(Line::default());
            x = 0;
        }
        placed.push((rows.len() - 1, x, w, action));
        let row = rows.last_mut().expect("there is a row");
        row.spans.push(Span::styled(text, PICKED));
        row.spans.push(Span::raw(" "));
        x += w + 1;
    }
    (rows, placed)
}

pub(super) fn draw_contact_card(frame: &mut Frame, app: &mut App) {
    app.regions.card = Rect::default();
    app.regions.card_buttons.clear();
    let Some(key) = app.contact_card.clone() else { return };
    let area = frame.area();
    let width = WIDTH.min(area.width.saturating_sub(2));
    let inner_width = width.saturating_sub(4) as usize;
    let contact = app.store.contact(&key);
    let name = app.store.display_name(&key);

    let mut lines = vec![Line::styled(name, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))];
    if contact.alias.is_some() {
        let theirs = app.store.announced_name(&key).unwrap_or("nothing heard yet").to_string();
        lines.push(field("Announces", theirs));
    }
    lines.push(field("Address", key.clone()));
    match app.store.peers.get(&key) {
        Some(peer) => lines.push(field("Heard", format!("{} ago, {} hop{}", ago(peer.last_seen), peer.hops, if peer.hops == 1 { "" } else { "s" }))),
        None => lines.push(field("Heard", "not yet (no announce)")),
    }
    lines.push(field("Trust", trust_label(contact.trust, app.is_known(&key))));
    if let Some(mode) = contact.delivery {
        lines.push(field("Delivery", format!("{} (d changes it in the conversation)", mode.label())));
    }
    if let Some(ping) = app.pings.get(&key) {
        let when = match ping {
            PingState::Done { at, .. } => format!(" ({} ago)", ago(*at)),
            PingState::Waiting => String::new(),
        };
        lines.push(field("Ping", format!("{}{when}", ping_label(ping))));
    }
    if let Some(conversation) = app.store.conversations.get(&key) {
        let mut count = format!("{}", conversation.messages.len());
        if conversation.archived > 0 {
            count.push_str(&format!(" (and {} archived)", conversation.archived));
        }
        lines.push(field("Messages", count));
    }
    lines.push(Line::raw(""));
    if contact.notes.is_empty() {
        lines.push(Line::styled("No notes (e adds some)", Style::default().fg(DIM)));
    } else {
        lines.push(Line::styled("Notes", Style::default().fg(DIM)));
        for line in contact.notes.lines().flat_map(|l| wrap(l, inner_width)) {
            lines.push(Line::raw(line));
        }
    }
    lines.push(Line::raw(""));
    let actions = app.card_actions(&key);
    let (rows, buttons) = button_rows(&actions, inner_width);
    let first_button_row = lines.len();
    lines.extend(rows);

    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let card = block("Contact", true).padding(ratatui::widgets::Padding::horizontal(1));
    let inner = card.inner(rect);
    frame.render_widget(Paragraph::new(lines).block(card), rect);
    app.regions.card = rect;
    for (row, x, w, action) in buttons {
        let y = inner.y + (first_button_row + row) as u16;
        if y < inner.bottom() {
            let button = Rect::new(inner.x + x as u16, y, w as u16, 1).intersection(inner);
            app.regions.card_buttons.push((button, action));
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::contacts::CardAction;
    use crate::app::test_app;
    use crate::config::Settings;
    use crate::net::PeerKind;
    use crate::store::{Conversation, Peer, Store};

    #[test]
    fn the_card_shows_who_and_its_buttons_work() {
        let dir = std::env::temp_dir().join(format!("rettui-card-ui-{}", std::process::id()));
        let key = "ab".repeat(16);
        let mut store = Store::default();
        store.peers.insert(key.clone(), Peer { kind: PeerKind::Lxmf, name: Some("Alice".into()), hops: 3, last_seen: 0 });
        store.conversations.insert(key.clone(), Conversation::default());
        store.update_contact(&key, |c| {
            c.alias = Some("Ally".into());
            c.notes = "met at the swapfest".into();
        });
        let mut app = test_app(&dir, Settings::default(), store);
        app.open_newest_conversation();
        app.contact_card = Some(key.clone());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..30).map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect();
        assert!(screen.contains("Ally") && screen.contains("Announces Alice") && screen.contains("3 hops"), "{screen}");
        assert!(screen.contains("met at the swapfest") && screen.contains("Rename (r)"), "{screen}");
        assert!(screen.contains("unknown sender") && screen.contains("Leave as is (l)") && screen.contains("Block (b)"), "{screen}");
        // Ping (p): asked of the network, and shown when it answers.
        app.on_key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(app.pings.get(&key), Some(&crate::app::contacts::PingState::Waiting));
        let ping = crate::net::Ping { rtt: std::time::Duration::from_millis(420), hops: Some(3) };
        app.on_net(crate::net::NetEvent::Pinged { to: [0xab; 16], result: Ok(ping) });
        terminal.draw(|frame| crate::ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..30).map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect();
        assert!(screen.contains("Ping      answered in 420 ms, 3 hops away"), "{screen}");
        // Its Close button closes it.
        let (rect, _) = *app.regions.card_buttons.iter().find(|(_, a)| *a == CardAction::Close).unwrap();
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: rect.y, modifiers: KeyModifiers::NONE });
        assert!(app.contact_card.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
