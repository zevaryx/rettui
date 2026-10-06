//! Rendering. Drawing also records screen regions in `app.regions` so mouse
//! events can be routed to what was actually on screen.
//!
//! [`chrome`] draws the sidebar, footer and prompt around the current tab;
//! each tab has its own module, mirroring `app/`.

mod browser;
mod channels;
mod chrome;
mod contact;
mod guide;
mod keys;
mod editor;
mod emoji;
mod messages;
mod network;
mod node;
mod reticulum;
mod status;

use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Tab};
use browser::draw_browser;
use channels::draw_channels;
use chrome::{draw_footer, draw_prompt, draw_sidebar, sidebar_width};
use messages::draw_messages;
use network::draw_network;
use node::draw_node;
use reticulum::draw_reticulum;
use status::draw_status;

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const SELECTED_BG: Color = Color::Rgb(0x2a, 0x2a, 0x3a);
/// The chosen row of a popup (a user's menu, the member picker): solid, so
/// it shows on any terminal theme (`SELECTED_BG` can be close to its
/// background).
const PICKED: Style = Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD);

/// Width for a side pane (a list beside the main view): `preferred` when
/// there is room, else about a third of `total`, and never below 16.
fn side_width(total: u16, preferred: u16) -> u16 {
    preferred.min((total / 3).max(16)).min(total)
}

fn block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { DIM }))
        .title(Span::styled(format!(" {title} "), Style::default().bold()))
}

fn time_label(ts: f64) -> String {
    let Some(time) = Local.timestamp_opt(ts as i64, 0).single() else {
        return String::new();
    };
    if time.date_naive() == Local::now().date_naive() {
        time.format("%H:%M").to_string()
    } else {
        time.format("%b %d %H:%M").to_string()
    }
}

fn ago_secs(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

fn ago(unix: i64) -> String {
    let secs = (Local::now().timestamp() - unix).max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(sidebar_width(frame.area().width)), Constraint::Min(20)]).areas(frame.area());
    let [body, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(main);

    draw_sidebar(frame, app, sidebar);
    match app.tab {
        Tab::Messages => draw_messages(frame, app, body),
        Tab::Channels => draw_channels(frame, app, body),
        Tab::Network => draw_network(frame, app, body),
        Tab::Browser => draw_browser(frame, app, body),
        Tab::Node => draw_node(frame, app, body),
        Tab::Status => draw_status(frame, app, body),
        Tab::Reticulum => draw_reticulum(frame, app, body),
    }
    draw_footer(frame, app, footer);
    if app.contact_card.is_some() && app.tab == Tab::Messages {
        contact::draw_contact_card(frame, app);
    }
    if app.paper_view.is_some() {
        messages::draw_paper(frame, app);
    }
    if app.guide.is_some() {
        guide::draw_guide(frame, app);
    }
    if app.prompt.is_some() {
        draw_prompt(frame, app);
    }
    if app.keys_help {
        keys::draw_keys(frame, app);
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let needed = if line.is_empty() { word.width() } else { line.width() + 1 + word.width() };
            if needed > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            // Break words longer than the whole line.
            for ch in word.chars() {
                if line.width() + ch.to_string().width() > width {
                    out.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        out.push(line);
    }
    out
}
