//! Hub tab: the RRC hub hosted here. Its status and what the panel's
//! commands came to on the left; who's connected, the rooms and the bans on
//! the right, with what's known of the one picked.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::{accent, ago, block, dim, human_bytes, selected_bg};
use crate::app::App;
use crate::app::hub::HubPane;
use crate::app::node::NodeStatus;

pub(super) fn draw_hub(frame: &mut Frame, app: &mut App, area: Rect) {
    // Wide enough for the 32-character hub address.
    let [left, right] = Layout::horizontal([Constraint::Length(super::side_width(area.width, 43)), Constraint::Min(20)]).areas(area);
    let [status_area, replies_area] = Layout::vertical([Constraint::Length(10), Constraint::Min(3)]).areas(left);
    let [list_area, details_area] = Layout::vertical([Constraint::Min(5), Constraint::Length(9)]).areas(right);
    draw_status(frame, app, status_area);
    draw_replies(frame, app, replies_area);
    draw_list(frame, app, list_area);
    draw_details(frame, app, details_area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let label = |s: &str| Span::styled(format!("{s:<9}"), Style::default().fg(dim()));
    let hub = &app.hub;
    let state = match &hub.status {
        NodeStatus::Running => Span::styled("● hosting", Style::default().fg(Color::Green)),
        NodeStatus::Starting => Span::styled("◌ starting", Style::default().fg(Color::Yellow)),
        NodeStatus::Off => Span::styled("○ off (e starts it)", Style::default().fg(dim())),
        NodeStatus::Failed(e) => Span::styled(format!("✗ {e}"), Style::default().fg(Color::Red)),
    };
    let running = hub.running();
    let address = match hub.address {
        Some((hash, _)) => Span::styled(hex::encode(hash), Style::default().fg(if running { accent() } else { dim() })),
        None => Span::styled("made when it first starts", Style::default().fg(dim())),
    };
    let name = hub
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| app.settings.hub_name.clone().unwrap_or_else(|| app.settings.display_name.clone()));
    let snapshot = &hub.snapshot;
    let people = match snapshot.unidentified {
        0 => snapshot.people.len().to_string(),
        n => format!("{} (and {n} saying who they are)", snapshot.people.len()),
    };
    let registered = snapshot.rooms.iter().filter(|r| r.registered).count();
    let rooms = format!("{} ({registered} registered)", snapshot.rooms.len());
    let stats = &snapshot.stats;
    let lines = vec![
        Line::from(vec![label("Hub"), state]),
        Line::from(vec![label("Address"), address]),
        Line::from(vec![label("Name"), Span::raw(name)]),
        Line::from(vec![label("People"), Span::raw(people)]),
        Line::from(vec![label("Rooms"), Span::raw(rooms)]),
        Line::from(vec![label("Messages"), Span::raw(format!("{} · {} joins", stats.messages, stats.joins))]),
        Line::from(vec![label("Traffic"), Span::raw(format!("↓ {} · ↑ {}", human_bytes(stats.bytes_in), human_bytes(stats.bytes_out)))]),
        Line::from(vec![label("Folder"), Span::styled(app.paths.rrc_hub.display().to_string(), Style::default().fg(dim()))]),
    ];
    frame.render_widget(Paragraph::new(lines).block(block("Hosting", false)), area);
}

/// What the panel's commands came to, newest at the bottom.
fn draw_replies(frame: &mut Frame, app: &App, area: Rect) {
    let replies_block = block("Replies", false);
    let inner = replies_block.inner(area);
    if app.hub.replies.is_empty() {
        let hint = "What the hub says to commands from here (: runs one, as in a room: /stats, /kline list...)";
        frame.render_widget(Paragraph::new(hint).style(Style::default().fg(dim())).wrap(Wrap { trim: true }).block(replies_block), area);
        return;
    }
    let width = inner.width.max(1) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for (text, error) in &app.hub.replies {
        let style = if *error { Style::default().fg(Color::Red) } else { Style::default() };
        for line in text.lines() {
            for part in super::wrap(line, width) {
                lines.push(Line::styled(part, style));
            }
        }
    }
    // The newest that fit.
    let skip = lines.len().saturating_sub(inner.height as usize);
    let lines: Vec<Line> = lines.into_iter().skip(skip).collect();
    frame.render_widget(Paragraph::new(lines).block(replies_block), area);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let running = app.hub.running();
    let list_block = block("Hub", running);
    let inner = list_block.inner(area);
    frame.render_widget(list_block, area);
    let [tabs_row, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);

    // The panes, with how many each has.
    let bans = app.hub.bans();
    let counts = [app.hub.snapshot.people.len(), app.hub.snapshot.rooms.len(), bans.len()];
    let mut spans = Vec::new();
    let mut tabs = Vec::new();
    let mut x = tabs_row.x;
    for (pane, count) in HubPane::ALL.into_iter().zip(counts) {
        let label = format!(" {} {count} ", pane.title());
        let width = label.width() as u16;
        tabs.push((Rect::new(x, tabs_row.y, width, 1), pane));
        x += width + 1;
        let style = if pane == app.hub.pane { Style::default().fg(Color::Black).bg(accent()).bold() } else { Style::default().fg(dim()) };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs_row);
    app.regions.hub_panes = tabs;
    app.regions.hub_list = list_area;

    let hub = &app.hub;
    let open_rooms = app.settings.hub_open_rooms;
    let (items, empty): (Vec<ListItem>, &str) = match hub.pane {
        HubPane::People => (
            hub.snapshot
                .people
                .iter()
                .map(|person| {
                    let name = person.nick.clone().unwrap_or_else(|| "(no nick)".into());
                    let mut spans = vec![Span::styled(name, Style::default().bold())];
                    if person.operator {
                        spans.push(Span::styled(" ★", Style::default().fg(Color::Yellow)));
                    }
                    spans.push(Span::styled(
                        format!("  <{}>", &person.identity[..12.min(person.identity.len())]),
                        Style::default().fg(dim()),
                    ));
                    if !person.rooms.is_empty() {
                        let rooms: Vec<String> = person.rooms.iter().map(|r| format!("#{r}")).collect();
                        spans.push(Span::raw(format!("  {}", rooms.join(" "))));
                    }
                    spans.push(Span::styled(format!("  {}", ago(person.since)), Style::default().fg(dim())));
                    ListItem::new(Line::from(spans))
                })
                .collect(),
            if running { "Nobody's connected yet. y copies the hub's link to share." } else { "The hub isn't running: e starts it." },
        ),
        HubPane::Rooms => (
            hub.snapshot
                .rooms
                .iter()
                .map(|room| {
                    let mut spans = vec![Span::styled(format!("#{}", room.name), Style::default().bold())];
                    if room.registered {
                        spans.push(Span::styled(" registered", Style::default().fg(Color::Green)));
                    }
                    if room.modes != "(none)" {
                        spans.push(Span::styled(format!(" {}", room.modes), Style::default().fg(Color::LightMagenta)));
                    }
                    spans.push(Span::styled(format!("  {} in it", room.members.len()), Style::default().fg(dim())));
                    if let Some(topic) = &room.topic {
                        spans.push(Span::raw(format!("  {topic}")));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect(),
            match (running, open_rooms) {
                (false, _) => "The hub isn't running: e starts it.",
                (true, true) => "No rooms yet: n makes one that stays, or anyone joining a room makes it.",
                (true, false) => "No rooms yet: n makes one (only you make rooms here).",
            },
        ),
        HubPane::Bans => (
            bans.iter()
                .map(|ban| {
                    let place = match &ban.room {
                        Some(room) => Span::raw(format!("#{room}")),
                        None => Span::styled("whole hub", Style::default().fg(Color::Red)),
                    };
                    let who = hub.name_of(&ban.identity);
                    ListItem::new(Line::from(vec![
                        place,
                        Span::raw(format!("  {who}")),
                        Span::styled(format!("  <{}>", &ban.identity[..12.min(ban.identity.len())]), Style::default().fg(dim())),
                    ]))
                })
                .collect(),
            "Nobody's banned. In People, b bans someone from a room, B from the whole hub.",
        ),
    };
    if items.is_empty() {
        frame.render_widget(Paragraph::new(empty).style(Style::default().fg(dim())).wrap(Wrap { trim: true }), list_area);
        return;
    }
    let list = List::new(items).highlight_style(Style::default().bg(selected_bg()).add_modifier(Modifier::BOLD));
    let state = app.hub.list();
    frame.render_stateful_widget(list, list_area, state);
}

/// What's known of the one picked, and what can be done with it.
fn draw_details(frame: &mut Frame, app: &App, area: Rect) {
    let hub = &app.hub;
    let label = |s: &str| Span::styled(format!("{s:<10}"), Style::default().fg(dim()));
    let names = |ids: &[String]| -> String {
        if ids.is_empty() { "–".into() } else { ids.iter().map(|id| hub.name_of(id)).collect::<Vec<_>>().join(", ") }
    };
    let (title, lines) = match hub.pane {
        HubPane::People => match hub.person() {
            Some(person) => (
                person.nick.clone().unwrap_or_else(|| "Someone".into()),
                vec![
                    Line::from(vec![label("Identity"), Span::styled(person.identity.clone(), Style::default().fg(accent()))]),
                    Line::from(vec![label("Rooms"), Span::raw(if person.rooms.is_empty() { "–".into() } else { person.rooms.join(", ") })]),
                    Line::from(vec![
                        label("Connected"),
                        Span::raw(format!(
                            "{} ago{}",
                            ago(person.since),
                            if person.links > 1 { format!(", {} times", person.links) } else { String::new() }
                        )),
                    ]),
                    Line::from(vec![label("Operator"), Span::raw(if person.operator { "of the hub" } else { "no" })]),
                    Line::raw(""),
                    Line::styled(
                        "K kick · b ban from a room · o operator · v voice · B ban from the hub · d disconnect",
                        Style::default().fg(dim()),
                    ),
                ],
            ),
            None => ("Someone".into(), Vec::new()),
        },
        HubPane::Rooms => match hub.room() {
            Some(room) => (
                format!("#{}", room.name),
                vec![
                    Line::from(vec![label("Topic"), Span::raw(room.topic.clone().unwrap_or_else(|| "–".into()))]),
                    Line::from(vec![
                        label("Modes"),
                        Span::raw(format!("{}{}", room.modes, if room.registered { " · registered" } else { "" })),
                    ]),
                    Line::from(vec![label("In it"), Span::raw(names(&room.members))]),
                    Line::from(vec![
                        label("Operators"),
                        Span::raw(names(&room.operators)),
                        Span::styled(
                            room.founder.as_ref().map(|f| format!(" (founded by {})", hub.name_of(f))).unwrap_or_default(),
                            Style::default().fg(dim()),
                        ),
                    ]),
                    Line::from(vec![label("Voiced"), Span::raw(names(&room.voiced))]),
                    Line::raw(""),
                    Line::styled(
                        format!("t topic · m modes · r {}", if room.registered { "unregister" } else { "register" }),
                        Style::default().fg(dim()),
                    ),
                ],
            ),
            None => ("Room".into(), Vec::new()),
        },
        HubPane::Bans => match hub.ban() {
            Some(ban) => (
                hub.name_of(&ban.identity),
                vec![
                    Line::from(vec![label("Identity"), Span::styled(ban.identity.clone(), Style::default().fg(accent()))]),
                    Line::from(vec![
                        label("From"),
                        Span::raw(ban.room.as_ref().map_or_else(|| "the whole hub".into(), |room| format!("#{room}"))),
                    ]),
                    Line::raw(""),
                    Line::styled("x lifts the ban", Style::default().fg(dim())),
                ],
            ),
            None => ("Ban".into(), Vec::new()),
        },
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block(&title, false)), area);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::config::Settings;
    use crate::net::NetEvent;
    use crate::rrc::host::{HubEvent, HubPerson, HubRoom, HubSnapshot};
    use crate::store::Store;

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..40).map(|y| (0..160).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn the_hub_shows_who_is_on_it_and_its_rooms() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-tab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings { hub_enabled: true, hub_name: Some("Hilltop".into()), ..Settings::default() };
        let mut app = crate::app::test_app(&dir, settings, Store::default());
        app.tab = crate::app::Tab::Hub;
        assert!(screen(&mut app).contains("◌ starting"));
        app.on_net(NetEvent::Hub(HubEvent::Started { hash: [7; 16], public_key: [1; 64] }));
        let amy = "a".repeat(32);
        app.on_net(NetEvent::Hub(HubEvent::State(HubSnapshot {
            people: vec![HubPerson {
                identity: amy.clone(),
                nick: Some("amy".into()),
                rooms: vec!["lobby".into()],
                since: chrono::Utc::now().timestamp() - 120,
                links: 1,
                operator: false,
            }],
            rooms: vec![HubRoom {
                name: "lobby".into(),
                registered: true,
                modes: "+nt".into(),
                topic: Some("Say hi".into()),
                members: vec![amy.clone()],
                founder: Some(amy.clone()),
                operators: vec![amy.clone()],
                voiced: Vec::new(),
                banned: Vec::new(),
            }],
            klines: vec!["b".repeat(32)],
            ..HubSnapshot::default()
        })));
        let shown = screen(&mut app);
        assert!(shown.contains("● hosting") && shown.contains(&hex::encode([7; 16])) && shown.contains("Hilltop"), "{shown}");
        assert!(shown.contains("People 1") && shown.contains("Rooms 1") && shown.contains("Bans 1"), "{shown}");
        assert!(shown.contains("amy  <aaaaaaaaaaaa>  #lobby  2m"), "{shown}");
        assert!(shown.contains(&amy) && shown.contains("K kick"), "{shown}");
        // Rooms, then bans.
        app.on_key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Tab));
        let shown = screen(&mut app);
        assert!(shown.contains("#lobby registered +nt  1 in it  Say hi"), "{shown}");
        assert!(shown.contains("(founded by amy)"), "{shown}");
        app.on_key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Tab));
        let shown = screen(&mut app);
        assert!(shown.contains("whole hub") && shown.contains("x lifts the ban"), "{shown}");
        // Stopped: nothing of it is left.
        app.on_net(NetEvent::Hub(HubEvent::Stopped));
        let shown = screen(&mut app);
        assert!(shown.contains("○ off (e starts it)") && shown.contains("Bans 0"), "{shown}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
