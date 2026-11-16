//! Tools tab: the tests of the network in one place.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{accent, block, dim, human_bytes, selected_bg};
use crate::app::App;
use crate::app::paths::{lookup_label, probe_state_label};
use crate::app::reach::Reach;
use crate::app::tools::{LISTEN_NOTE, Tool, reach_label};

pub(super) fn draw_tools(frame: &mut Frame, app: &mut App, area: Rect) {
    let history = app.tools.history.len().clamp(1, 8) as u16;
    let [tools_area, recent_area, interfaces_area] =
        Layout::vertical([Constraint::Length(Tool::ALL.len() as u16 + 2), Constraint::Length(history + 2), Constraint::Min(4)]).areas(area);
    draw_menu(frame, app, tools_area);
    draw_recent(frame, app, recent_area);
    draw_interfaces(frame, app, interfaces_area);
    if app.speed.open {
        super::network::draw_speed(frame, app, area);
    }
    if app.whois.open {
        super::network::draw_whois(frame, app, area);
    }
}

fn draw_menu(frame: &mut Frame, app: &mut App, area: Rect) {
    let outer = block("Tools", true);
    let inner = outer.inner(area);
    app.regions.tools = inner;
    let lines: Vec<Line> = Tool::ALL
        .iter()
        .enumerate()
        .map(|(i, tool)| {
            let (name, key, help) = tool.label();
            let row = if i == app.tools.selected { Style::default().bg(selected_bg()) } else { Style::default() };
            Line::from(vec![
                Span::styled(format!(" {key} "), Style::default().fg(accent())),
                Span::raw(format!("{name:<18}")),
                Span::styled(help, Style::default().fg(dim())),
            ])
            .style(row)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(outer), area);
}

/// The addresses tried lately, with how the path and the probe went.
fn draw_recent(frame: &mut Frame, app: &App, area: Rect) {
    let now = chrono::Utc::now().timestamp();
    let mut lines = Vec::new();
    for key in &app.tools.history {
        let mut spans =
            vec![Span::styled(format!(" {} ", app.store.display_name(key)), Style::default().fg(accent())), Span::raw(key.clone())];
        if let Some(lookup) = app.path_lookups.get(key) {
            spans.push(Span::styled(format!("  path: {}", lookup_label(lookup, now)), Style::default().fg(dim())));
        }
        if let Some(probe) = app.probes.get(key) {
            spans.push(Span::styled(format!("  probe: {}", probe_state_label(probe)), Style::default().fg(dim())));
        }
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::styled(" Nothing tried yet: P finds a path, T probes (Enter on a tool does too)", Style::default().fg(dim())));
    }
    frame.render_widget(Paragraph::new(lines).block(block("Tried lately (y copies the newest)", false)), area);
}

/// Each interface: up or down, its traffic, and how reaching its entry
/// point went.
fn draw_interfaces(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    for row in app.interface_rows() {
        let (mark, colour) = if row.info.online { ("●", Color::Green) } else { ("○", Color::Red) };
        let mut spans = vec![
            Span::styled(format!(" {mark} "), Style::default().fg(colour)),
            Span::raw(row.info.name.clone()),
            Span::styled(format!("  ↓{} ↑{}", human_bytes(row.info.rx_bytes), human_bytes(row.info.tx_bytes)), Style::default().fg(dim())),
        ];
        if let Some(entry) = &row.entry {
            let place =
                if entry.listens { format!("  listens on port {}", entry.port) } else { format!("  {}:{}", entry.host, entry.port) };
            spans.push(Span::styled(place, Style::default().fg(dim())));
            if !entry.enabled {
                spans.push(Span::styled("  switched off", Style::default().fg(dim())));
            }
            if entry.ifac {
                spans.push(Span::styled("  IFAC", Style::default().fg(dim())));
            }
        }
        lines.push(Line::from(spans));
        if let Some(check) = &row.check {
            let colour = match check {
                Reach::Up(_) => Color::Green,
                Reach::Down(_) => Color::Red,
                Reach::Checking => dim(),
            };
            lines.push(Line::styled(format!("     {}", reach_label(*check)), Style::default().fg(colour)));
            if row.entry.as_ref().is_some_and(|e| e.listens) {
                lines.push(Line::styled(format!("     {}", LISTEN_NOTE), Style::default().fg(dim())));
            }
        } else if !row.info.online && row.entry.is_some() {
            lines.push(Line::styled("     c tries connecting to it", Style::default().fg(dim())));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(" No interface yet: Reticulum is starting, or the config has none", Style::default().fg(dim())));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(block("Interfaces (c checks the entry points)", false)),
        area,
    );
}
