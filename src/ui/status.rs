//! Status tab: identity, network state, settings, interfaces and the log.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use super::{ACCENT, DIM, SELECTED_BG, block, human_bytes, wrap};
use crate::app::node::NodeStatus;
use crate::app::{App, NetState, SyncState};
use crate::config::{Effect, FIELDS, FieldKind};

/// The propagation node hosted here: how it's doing, and what it holds.
fn hosting(app: &App) -> Vec<Span<'static>> {
    match &app.pn.status {
        NodeStatus::Off => vec![Span::styled("no (Host a propagation node, below)", Style::default().fg(DIM))],
        NodeStatus::Starting => vec![Span::styled("◌ starting", Style::default().fg(Color::Yellow))],
        NodeStatus::Failed(e) => vec![Span::styled(format!("✗ {e}"), Style::default().fg(Color::Red))],
        NodeStatus::Running => {
            let mut spans = vec![
                Span::styled("● ", Style::default().fg(Color::Green)),
                Span::styled(hex::encode(app.pn.hash), Style::default().fg(ACCENT)),
            ];
            if let Some(s) = &app.pn.stats {
                let mut parts = vec![
                    format!("{} kept ({})", s.messages, human_bytes(s.bytes as u64)),
                    format!("{} in", s.received),
                    format!("{} collected", s.served),
                ];
                if s.delivered_here > 0 {
                    parts.push(format!("{} for you", s.delivered_here));
                }
                if s.rejected > 0 {
                    parts.push(format!("{} refused", s.rejected));
                }
                parts.push(format!("{} peer{}", s.peers, if s.peers == 1 { "" } else { "s" }));
                spans.push(Span::raw(format!("  {}", parts.join(" · "))));
            }
            spans
        }
    }
}

/// `settings.json`, one row per setting, with help for the selected one.
fn draw_settings(frame: &mut Frame, app: &mut App, area: Rect) {
    let path = app.paths.settings.display().to_string();
    let settings_block = block("Settings", true).title_bottom(Line::styled(format!(" {path} "), Style::default().fg(DIM)));
    let inner = settings_block.inner(area);
    frame.render_widget(settings_block, area);
    let [list_area, help_area] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    app.regions.settings = list_area;
    // Values line up two spaces after the longest label.
    let width = FIELDS.iter().map(|f| f.label.chars().count()).max().unwrap_or(0) + 2;
    let items: Vec<ListItem> = FIELDS
        .iter()
        .map(|field| {
            let value = app.settings_file.field_value(field.key);
            let shown = match field.kind {
                FieldKind::Toggle => Span::raw(if value == "true" { "on" } else { "off" }),
                FieldKind::Optional if value.is_empty() => Span::styled("(none)", Style::default().fg(DIM)),
                FieldKind::Number if value == "0" => Span::styled("0 (off)", Style::default().fg(DIM)),
                _ => Span::raw(value.clone()),
            };
            let mut spans = vec![Span::styled(format!(" {:<width$}", field.label), Style::default().fg(DIM))];
            // A colour shows as itself, before its code.
            if let (FieldKind::Color, Some([r, g, b])) = (field.kind, crate::icons::parse_colour(&value)) {
                spans.push(Span::styled("██ ", Style::default().fg(ratatui::style::Color::Rgb(r, g, b))));
            }
            spans.push(shown);
            if field.effect == Effect::NextStart {
                spans.push(Span::styled("  · next start", Style::default().fg(DIM)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items).highlight_style(Style::default().bg(SELECTED_BG).bold());
    frame.render_stateful_widget(list, list_area, &mut app.settings_list);
    let help = app
        .settings_list
        .selected()
        .and_then(|i| FIELDS.get(i))
        .map_or("", |f| f.help);
    frame.render_widget(Paragraph::new(Span::styled(format!(" {help}"), Style::default().fg(DIM).italic())), help_area);
}

pub(super) fn draw_status(frame: &mut Frame, app: &mut App, area: Rect) {
    let settings_height = FIELDS.len() as u16 + 3;
    let steps = app.first_steps();
    // The settings list scrolls, so it may give way on short terminals.
    let [info, settings, rest] = Layout::vertical([
        Constraint::Length(if steps.is_empty() { 11 } else { 12 }),
        Constraint::Max(settings_height),
        Constraint::Min(5),
    ])
    .areas(area);
    draw_settings(frame, app, settings);
    let [ifaces, log] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(rest);

    let address = app
        .lxmf_hash
        .map(hex::encode)
        .unwrap_or_else(|| "(starting)".to_string());
    let network = match &app.net_state {
        NetState::Starting => "starting".to_string(),
        NetState::Online => "online".to_string(),
        NetState::Failed(e) => format!("failed: {e}"),
    };
    let auto = app.auto_pick_label();
    let propagation = match (&app.settings.propagation_node, auto) {
        (Some(hash), Some(auto)) => format!("{}  {hash} · {auto}", app.store.display_name(hash)),
        (Some(hash), None) => format!("{}  {hash}", app.store.display_name(hash)),
        (None, Some(auto)) => auto,
        (None, None) => "none (pick one in the Network tab)".to_string(),
    };
    let sync = match &app.sync {
        SyncState::Idle => "never".to_string(),
        SyncState::Running(started) => format!("running ({}s)", started.elapsed().as_secs()),
        SyncState::Done(at, Ok(n)) => format!("{} · {n} new", at.format("%H:%M")),
        SyncState::Done(at, Err(e)) => format!("{} · failed: {e}", at.format("%H:%M")),
    };
    let label = |s: &str| Span::styled(format!("{s:<18}"), Style::default().fg(DIM));
    let lines = vec![
        Line::from(vec![label("Display name"), Span::raw(app.settings.display_name.clone()).bold()]),
        Line::from(vec![
            label("LXMF address"),
            Span::styled(address, Style::default().fg(ACCENT)),
            match app.identity_pending {
                Some(next) => Span::styled(format!("  → {} from the next start", hex::encode(next)), Style::default().fg(Color::Yellow)),
                None => Span::raw(""),
            },
        ]),
        Line::from(vec![label("Network"), Span::raw(network)]),
        Line::from(vec![label("Propagation node"), Span::raw(propagation)]),
        Line::from(vec![label("Last sync"), Span::raw(sync)]),
        Line::from([vec![label("Hosting messages")], hosting(app)].concat()),
        Line::from(vec![
            label("RNS config"),
            Span::raw(app.settings.rns_config.clone().unwrap_or_else(|| "rsReticulum default".into())),
        ]),
        Line::from(vec![
            label("Data"),
            Span::raw(app.paths.store.parent().map(|p| p.display().to_string()).unwrap_or_default()),
        ]),
        Line::from(vec![label("Known"), Span::raw(format!("{} destinations", app.store.peers.len()))]),
    ];
    let mut lines = lines;
    // Until they're all taken: each step's mark, and the next to take.
    if let Some(next) = steps.iter().find(|step| !step.done) {
        let mut spans = vec![label("First steps")];
        spans.extend(steps.iter().map(|step| match step.done {
            true => Span::styled("✓", Style::default().fg(Color::Green)),
            false => Span::styled("○", Style::default().fg(DIM)),
        }));
        spans.push(Span::raw(format!("  next: {} ({})", next.label, next.how)));
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines).block(block("Identity", false)), info);

    let iface_lines: Vec<Line> = app
        .interfaces
        .iter()
        .map(|i| {
            let (dot, color) = if i.online { ("●", Color::Green) } else { ("○", Color::Red) };
            Line::from(vec![
                Span::styled(format!("{dot} "), Style::default().fg(color)),
                Span::raw(i.name.clone()),
                Span::styled(
                    format!("  ↓{} ↑{}", human_bytes(i.rx_bytes), human_bytes(i.tx_bytes)),
                    Style::default().fg(DIM),
                ),
            ])
        })
        .collect();
    let iface_lines = if iface_lines.is_empty() {
        let none = match app.net_state {
            NetState::Online if app.uses_external_shared_instance() => {
                "None here: the program running the shared instance (such as rnsd) has them."
            }
            NetState::Online => "None: add one in the Reticulum tab to reach other peers.",
            _ => "Reticulum is starting…",
        };
        vec![Line::styled(none, Style::default().fg(DIM))]
    } else {
        iface_lines
    };
    frame.render_widget(
        Paragraph::new(iface_lines).wrap(ratatui::widgets::Wrap { trim: true }).block(block("Interfaces", false)),
        ifaces,
    );

    let rows = log_rows(app.log.iter(), log.width.saturating_sub(2) as usize, log.height.saturating_sub(2) as usize);
    frame.render_widget(Paragraph::new(rows).block(block("Log", false)), log);
}

/// The newest log lines that fit, wrapped under their times (long ones,
/// such as an interface's error, take a few rows without pushing the
/// newest off the bottom).
fn log_rows<'a>(log: impl DoubleEndedIterator<Item = &'a String>, width: usize, height: usize) -> Vec<Line<'static>> {
    // Lines start with the time: "HH:MM:SS  ".
    const TIME: usize = 10;
    let mut rows = Vec::new();
    for entry in log.rev() {
        if rows.len() >= height {
            break;
        }
        let pieces = match entry.get(..TIME).zip(entry.get(TIME..)) {
            Some((time, text)) if width > TIME * 2 => {
                let mut pieces = wrap(text, width - TIME).into_iter();
                let first = format!("{time}{}", pieces.next().unwrap_or_default());
                std::iter::once(first).chain(pieces.map(|p| format!("{:TIME$}{p}", ""))).collect()
            }
            _ => wrap(entry, width),
        };
        rows.extend(pieces.into_iter().rev().map(Line::raw));
    }
    rows.truncate(height);
    rows.reverse();
    rows
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::config::Settings;
    use crate::lxmf::pn::PnStats;
    use crate::net::{NetCommand, NetEvent, PnEvent};
    use crate::store::Store;

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..40).map(|y| (0..140).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn the_propagation_node_follows_the_settings_and_shows_what_it_holds() {
        let dir = std::env::temp_dir().join(format!("rettui-pn-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings { pn_enabled: true, pn_name: Some("Hilltop".into()), ..Settings::default() };
        let (mut app, mut commands) = crate::app::test_app_with_net(&dir, settings, Store::default());
        app.tab = crate::app::Tab::Status;
        assert!(screen(&mut app).contains("Hosting messages  ◌ starting"));
        app.on_net(NetEvent::Pn(PnEvent::Started { hash: [7; 16] }));
        app.on_net(NetEvent::Pn(PnEvent::Stats(PnStats { messages: 3, bytes: 2400, received: 4, served: 1, peers: 1, ..PnStats::default() })));
        let shown = screen(&mut app);
        assert!(shown.contains(&hex::encode([7; 16])) && shown.contains("3 kept (2.4 KB) · 4 in · 1 collected"), "{shown}");
        assert!(shown.contains("1 peer") && !shown.contains("1 peers"), "{shown}");
        // It's listed by name, to be picked as your propagation node.
        assert_eq!(app.store.peers[&hex::encode([7; 16])].name.as_deref(), Some("Hilltop"));
        // Settings it doesn't run with leave it be; its own restart it.
        while commands.try_recv().is_ok() {}
        app.update_settings(&[("wrap_lines", "true")]).unwrap();
        assert!(!std::iter::from_fn(|| commands.try_recv().ok()).any(|c| matches!(c, NetCommand::Propagation(_))));
        app.update_settings(&[("pn_storage_mb", "100")]).unwrap();
        let config = std::iter::from_fn(|| commands.try_recv().ok()).find_map(|c| match c {
            NetCommand::Propagation(config) => config,
            _ => None,
        });
        assert_eq!(config.map(|c| (c.name, c.storage_bytes)), Some(("Hilltop".to_string(), 100_000_000)));
        assert!(screen(&mut app).contains("◌ starting"));
        app.update_settings(&[("pn_enabled", "false")]).unwrap();
        assert!(std::iter::from_fn(|| commands.try_recv().ok()).any(|c| matches!(c, NetCommand::Propagation(None))));
        app.on_net(NetEvent::Pn(PnEvent::Stopped));
        assert!(screen(&mut app).contains("Hosting messages  no"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn long_log_lines_wrap_under_their_time_and_keep_the_newest_in_view() {
        let log: Vec<String> = [
            "12:00:00  Old line that should scroll away",
            "12:00:01  Interface Dead Link: TCP connect failed: Connection refused (os error 111)",
            "12:00:02  Newest",
        ]
        .map(String::from)
        .to_vec();
        let rows: Vec<String> = log_rows(log.iter(), 40, 4).iter().map(|l| l.to_string()).collect();
        assert_eq!(rows, [
            "12:00:01  Interface Dead Link: TCP",
            "          connect failed: Connection",
            "          refused (os error 111)",
            "12:00:02  Newest",
        ]);
    }
}
