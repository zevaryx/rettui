//! Status tab: identity, network state, settings, interfaces and the log.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use super::{ACCENT, DIM, SELECTED_BG, block, human_bytes, wrap};
use crate::app::{App, NetState, SyncState};
use crate::config::{Effect, FIELDS, FieldKind};

/// `settings.json`, one row per setting, with help for the selected one.
fn draw_settings(frame: &mut Frame, app: &mut App, area: Rect) {
    let path = app.paths.settings.display().to_string();
    let settings_block = block("Settings", true).title_bottom(Line::styled(format!(" {path} "), Style::default().fg(DIM)));
    let inner = settings_block.inner(area);
    frame.render_widget(settings_block, area);
    let [list_area, help_area] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    app.regions.settings = list_area;
    let items: Vec<ListItem> = FIELDS
        .iter()
        .map(|field| {
            let value = app.settings_file.field_value(field.key);
            let shown = match field.kind {
                FieldKind::Toggle => Span::raw(if value == "true" { "on" } else { "off" }),
                FieldKind::Optional if value.is_empty() => Span::styled("(none)", Style::default().fg(DIM)),
                FieldKind::Number if value == "0" => Span::styled("0 (off)", Style::default().fg(DIM)),
                _ => Span::raw(value),
            };
            let mut spans = vec![Span::styled(format!(" {:<22}", field.label), Style::default().fg(DIM)), shown];
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
    // The settings list scrolls, so it may give way on short terminals.
    let [info, settings, rest] = Layout::vertical([
        Constraint::Length(10),
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
    let propagation = match &app.settings.propagation_node {
        Some(hash) => format!("{}  {hash}", app.store.display_name(hash)),
        None => "none (pick one in the Network tab)".to_string(),
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
        Line::from(vec![label("LXMF address"), Span::styled(address, Style::default().fg(ACCENT))]),
        Line::from(vec![label("Network"), Span::raw(network)]),
        Line::from(vec![label("Propagation node"), Span::raw(propagation)]),
        Line::from(vec![label("Last sync"), Span::raw(sync)]),
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
    frame.render_widget(
        Paragraph::new(iface_lines).block(block("Interfaces", false)),
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
    use super::*;

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
