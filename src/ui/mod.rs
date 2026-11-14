//! Rendering. Drawing also records screen regions in `app.regions` so mouse
//! events can be routed to what was actually on screen.
//!
//! [`chrome`] draws the sidebar, footer and prompt around the current tab;
//! each tab has its own module, mirroring `app/`.

mod archive;
mod browser;
mod channels;
mod chrome;
mod contact;
mod editor;
mod emoji;
mod face;
mod forward;
mod games;
mod guide;
mod hub;
mod keys;
mod linked;
mod map;
mod messages;
mod network;
mod node;
mod repos;
mod reticulum;
mod search;
mod status;
#[cfg(test)]
pub(crate) mod tui_tests;

use chrono::Local;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Tab, match_mask};
use browser::draw_browser;
use channels::draw_channels;
use chrome::{draw_footer, draw_prompt, draw_sidebar, sidebar_width};
use messages::draw_messages;
use network::draw_network;
use node::draw_node;
use reticulum::draw_reticulum;
use status::draw_status;

/// The TUI's colours (the `tui_theme` setting): for dark terminals, light
/// ones, or basic 16-colour ones.
pub const THEMES: &[&str] = &["dark", "light", "basic"];

thread_local! {
    /// The theme being drawn in. Set from the settings at the start of each
    /// frame, so it always matches them, on whichever thread draws.
    static THEME: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

/// Draw in the theme called `name` from now on.
pub fn set_theme(name: &str) {
    let index = THEMES.iter().position(|t| *t == name).unwrap_or(0);
    THEME.with(|theme| theme.set(index as u8));
}

fn theme() -> u8 {
    THEME.with(std::cell::Cell::get)
}

const LIGHT: u8 = 1;
const BASIC: u8 = 2;

/// What stands out: titles, the focused box, your name.
fn accent() -> Color {
    match theme() {
        LIGHT => Color::Blue,
        _ => Color::Cyan,
    }
}

/// Text on an `accent()` background (a chosen tab, the picked row).
fn on_accent() -> Color {
    match theme() {
        LIGHT => Color::White,
        _ => Color::Black,
    }
}

/// `text` cut to `width` columns, with an ellipsis if it was longer.
pub(super) fn clipped(text: &str, width: usize) -> String {
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

/// Rows of a label-and-value box, each cut to `width` columns with an
/// ellipsis (in the style of the text it cuts) where it's longer, so a
/// value cut short says so.
fn fit_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            if line.width() <= width {
                return line;
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut used = 0;
            for span in line.spans {
                let room = width.saturating_sub(used);
                if span.width() < room {
                    used += span.width();
                    spans.push(span);
                    continue;
                }
                // `clipped` keeps a column for its ellipsis.
                spans.push(Span::styled(clipped(&span.content, room), span.style));
                break;
            }
            Line::from(spans).style(line.style)
        })
        .collect()
}

/// What stays in the background: hints, times, addresses. A fixed grey
/// rather than the terminal's "bright black", which some colour schemes
/// (Solarized among them) make the same as the background.
fn dim() -> Color {
    match theme() {
        LIGHT => Color::Rgb(0x6a, 0x6a, 0x72),
        BASIC => Color::DarkGray,
        _ => Color::Rgb(0x8a, 0x8a, 0x96),
    }
}

/// Behind the selected row.
fn selected_bg() -> Color {
    match theme() {
        LIGHT => Color::Rgb(0xd8, 0xe0, 0xf0),
        BASIC => Color::Blue,
        _ => Color::Rgb(0x2a, 0x2a, 0x3a),
    }
}

/// A key's label in the footer and in lists of keys.
fn keycap_style() -> Style {
    match theme() {
        LIGHT => Style::new().fg(Color::Black).bg(Color::Rgb(0xd4, 0xd4, 0xdc)),
        BASIC => Style::new().fg(Color::Black).bg(Color::Gray),
        _ => Style::new().fg(Color::Rgb(0xe8, 0xe8, 0xee)).bg(Color::Rgb(0x48, 0x48, 0x56)),
    }
}

/// The chosen row of a popup (a user's menu, the member picker): solid, so
/// it shows on any terminal theme (`selected_bg()` can be close to its
/// background).
fn picked_style() -> Style {
    Style::new().fg(on_accent()).bg(accent()).add_modifier(Modifier::BOLD)
}

/// Width for a side pane (a list beside the main view): `preferred` when
/// there is room, else about a third of `total`, and never below 16.
/// The shown tab's side list's width in `area` (which it shares with the
/// pane beside it): as the user left it, or `preferred`. Kept for
/// resizing it (see `crate::app::panes`).
fn side_width(app: &mut App, area: ratatui::layout::Rect, preferred: u16) -> u16 {
    let split = crate::app::panes::side_split(&app.settings, app.tab, area, preferred);
    app.regions.side_split = Some(split);
    split.width
}

fn block(title: &str, focused: bool) -> Block<'_> {
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(if focused {
        accent()
    } else {
        dim()
    }));
    if title.is_empty() { block } else { block.title(Span::styled(format!(" {title} "), Style::default().bold())) }
}

/// Clear the screen under a popup, and a column and a row around it, so
/// nothing behind it shows at its edges; a popup that nearly reaches the
/// screen's edge is cleared to it, so no scraps of borders are left in the
/// gap. The footer stays: it has the popup's keys.
fn clear_popup(frame: &mut Frame, rect: ratatui::layout::Rect) {
    let screen = frame.area();
    let screen = ratatui::layout::Rect { height: screen.height.saturating_sub(1), ..screen };
    let mut around = rect.outer(ratatui::layout::Margin::new(1, 1)).intersection(screen);
    if around.is_empty() {
        return frame.render_widget(ratatui::widgets::Clear, rect);
    }
    const NEAR: u16 = 4;
    if around.x - screen.x < NEAR {
        around.width += around.x - screen.x;
        around.x = screen.x;
    }
    if around.y - screen.y < NEAR {
        around.height += around.y - screen.y;
        around.y = screen.y;
    }
    if screen.right() - around.right() < NEAR {
        around.width = screen.right() - around.x;
    }
    if screen.bottom() - around.bottom() < NEAR {
        around.height = screen.bottom() - around.y;
    }
    frame.render_widget(ratatui::widgets::Clear, around);
}

/// A pane with nothing in it yet: a box saying what goes there and how to
/// put something in it.
fn empty_pane(frame: &mut Frame, area: ratatui::layout::Rect, hint: &str) {
    let inner = block("", false).inner(area);
    frame.render_widget(block("", false), area);
    let text = ratatui::widgets::Paragraph::new(hint)
        .style(Style::default().fg(dim()))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .alignment(ratatui::layout::Alignment::Center);
    // A third of the way down, where the eye goes.
    let rows = wrap(hint, inner.width as usize).len() as u16;
    let top = inner.height.saturating_sub(rows) / 3;
    let area = ratatui::layout::Rect { y: inner.y + top, height: inner.height - top, ..inner };
    frame.render_widget(text, area);
}

fn time_label(ts: f64) -> String {
    crate::clock::when(ts)
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

/// `text` (at most `limit` chars) with search matches highlighted.
fn highlighted(text: &str, limit: usize, terms: &[Vec<char>], style: Style) -> Vec<Span<'static>> {
    let hit = Style::default().fg(Color::Black).bg(Color::Yellow);
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut on = false;
    for (c, matched) in text.chars().zip(match_mask(text, terms)).take(limit) {
        if matched != on && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), if on { hit } else { style }));
        }
        on = matched;
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if on { hit } else { style }));
    }
    spans
}

pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    set_theme(&app.settings.tui_theme);
    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(sidebar_width(frame.area().width)), Constraint::Min(20)]).areas(frame.area());
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(main);

    draw_sidebar(frame, app, sidebar);
    app.regions.side_split = None;
    match app.tab {
        Tab::Messages => draw_messages(frame, app, body),
        Tab::Games => games::draw_games(frame, app, body),
        Tab::Channels => draw_channels(frame, app, body),
        Tab::Network => draw_network(frame, app, body),
        Tab::Browser => draw_browser(frame, app, body),
        Tab::Node => draw_node(frame, app, body),
        Tab::Status => draw_status(frame, app, body),
        Tab::Reticulum => draw_reticulum(frame, app, body),
        Tab::Hub => hub::draw_hub(frame, app, body),
        Tab::Repos => repos::draw_repos(frame, app, body),
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
    if app.message_search.is_some() && app.tab == Tab::Messages {
        search::draw_message_search(frame, app);
    }
    if app.channel_search.is_some() && app.tab == Tab::Channels {
        search::draw_channel_search(frame, app);
    }
    if app.archive_reader.is_some() && app.tab == Tab::Messages {
        archive::draw_archive(frame, app);
    }
    if app.forward.is_some() && app.tab == Tab::Messages {
        forward::draw_forward(frame, app);
    }
    if app.map.is_some() && app.tab == Tab::Messages {
        map::draw_map(frame, app);
    }
    if app.tab == Tab::Games && app.game_menu.is_some() {
        games::draw_game_menu(frame, app);
    }
    if app.prompt.is_some() {
        draw_prompt(frame, app);
    }
    if app.keys_help {
        keys::draw_keys(frame, app);
    }
    adapt_colours(frame.buffer_mut());
}

/// Colours picked for a dark terminal, made to work in the other themes:
/// on a light one, pale text on the terminal's own background is darkened;
/// in the basic theme, full colours become the nearest of the 16 that every
/// terminal has. Text on a background of its own (badges, boards) already
/// stands out, and blocks (colour swatches, pictures) are colours rather
/// than text, so those keep their colour on a light terminal.
fn adapt_colours(buffer: &mut Buffer) {
    let theme = theme();
    if theme == 0 {
        return;
    }
    let selected = selected_bg();
    for cell in &mut buffer.content {
        if theme == LIGHT {
            let picture = cell.symbol().chars().next().is_some_and(|c| ('\u{2580}'..='\u{259f}').contains(&c));
            if (cell.bg == Color::Reset || cell.bg == selected) && !picture {
                cell.fg = for_light(cell.fg);
            }
        } else {
            (cell.fg, cell.bg) = (basic(cell.fg), basic(cell.bg));
        }
    }
}

/// A text colour that reads on white.
fn for_light(colour: Color) -> Color {
    match colour {
        Color::Yellow | Color::LightYellow => Color::Rgb(0x8a, 0x5c, 0x00),
        Color::Green | Color::LightGreen => Color::Rgb(0x1a, 0x7a, 0x34),
        Color::Cyan | Color::LightCyan => Color::Rgb(0x00, 0x6a, 0x78),
        Color::Magenta | Color::LightMagenta => Color::Rgb(0x9c, 0x1f, 0x98),
        Color::LightBlue => Color::Rgb(0x1f, 0x4f, 0xc8),
        Color::Red | Color::LightRed => Color::Rgb(0xb4, 0x1c, 0x26),
        Color::Gray | Color::White => Color::Rgb(0x40, 0x40, 0x48),
        // Pale colours (nicks, Micron pages made for dark terminals) darkened
        // until they read on white, keeping their hue.
        Color::Rgb(r, g, b) => {
            let light = (299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000;
            if light <= 110 {
                return colour;
            }
            let scale = |c: u8| (u32::from(c) * 110 / light) as u8;
            Color::Rgb(scale(r), scale(g), scale(b))
        }
        other => other,
    }
}

/// The 16 colours as most terminals draw them, in `Color::Indexed` order.
const BASIC_16: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

/// The same 16 by name: these are sent as the oldest colour codes, which
/// every terminal knows (`Color::Indexed` needs a 256-colour one).
const NAMED_16: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

/// The nearest of the 16 basic colours to a full or 256-palette one.
fn basic(colour: Color) -> Color {
    let (r, g, b) = match colour {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) if i < 16 => return NAMED_16[usize::from(i)],
        Color::Indexed(i) if i < 232 => {
            let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            let i = i - 16;
            (level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        Color::Indexed(i) => {
            let grey = 8 + (i - 232) * 10;
            (grey, grey, grey)
        }
        other => return other,
    };
    let distance = |&(pr, pg, pb): &(u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(r, pr) + d(g, pg) + d(b, pb)
    };
    let nearest = BASIC_16.iter().enumerate().min_by_key(|(_, c)| distance(c)).map_or(0, |(i, _)| i);
    NAMED_16[nearest]
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
