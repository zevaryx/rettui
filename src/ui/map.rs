//! The map of where everyone was (`M` in Messages), over the tab: the
//! world's coastlines with each place on them, and the places listed, the
//! newest first, with how long ago and how far from this station.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Map, MapResolution};
use ratatui::widgets::{Clear, Paragraph};

use super::{accent, ago, block, dim, selected_bg};
use crate::app::App;
use crate::app::map::{MapView, bounds};

/// The list beside the map, when there's room for both side by side.
const LIST_WIDTH: u16 = 34;
/// Degrees of latitude in view from which the coastlines are drawn.
const COASTLINES: f64 = 10.0;

pub(super) fn draw_map(frame: &mut Frame, app: &mut App) {
    let places = app.locations();
    let Some(view) = app.map.clone() else { return };
    let area = frame.area();
    let width = area.width.saturating_sub(2);
    let height = area.height.saturating_sub(2);
    if width < 30 || height < 10 || places.is_empty() {
        return;
    }
    let rect = Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height };
    let selected = view.selected.min(places.len() - 1);
    let side = width >= 70;
    let [map_area, list_area] = if side {
        Layout::horizontal([Constraint::Min(20), Constraint::Length(LIST_WIDTH)]).areas(rect)
    } else {
        let rows = (places.len() as u16 * 2 + 2).min(height / 3);
        Layout::vertical([Constraint::Min(6), Constraint::Length(rows)]).areas(rect)
    };

    // The map, as wide on the ground as it's drawn (inside its border).
    let inner_w = map_area.width.saturating_sub(2);
    let inner_h = map_area.height.saturating_sub(2);
    let fit = bounds(&places, &MapView { span: None, ..view.clone() }, inner_w, inner_h);
    if let Some(map) = app.map.as_mut() {
        map.fitted = fit.1[1] - fit.1[0];
    }
    let (lons, lats) = bounds(&places, &view, inner_w, inner_h);
    let middle = (lats[0] + lats[1]) / 2.0;
    let across = (lons[1] - lons[0]) * middle.to_radians().cos().abs() * 111.32;
    let across = if across < 10.0 { format!("{:.1} km across", across) } else { format!("{across:.0} km across") };
    let title = format!("Map · {} · {across}", places[selected].name);
    let hints = if view.span.is_some() { " +/- zoom · 0 all " } else { " + zoom in " };
    let outer = block(&title, true).title_bottom(Line::styled(hints, Style::default().fg(dim())));
    let canvas = Canvas::default()
        .block(outer)
        .marker(Marker::Braille)
        .x_bounds(lons)
        .y_bounds(lats)
        .paint(|ctx| {
            // The coastlines are points a few kilometres apart at best:
            // closer in, they'd only be specks.
            if lats[1] - lats[0] >= COASTLINES {
                ctx.draw(&Map { color: dim(), resolution: MapResolution::High });
                ctx.layer();
            }
            // The one picked last, so it's on top.
            let order = (0..places.len()).filter(|&i| i != selected).chain([selected]);
            for i in order {
                let place = &places[i];
                let (mark, colour) = match place.key {
                    None => ('◆', Color::Yellow),
                    Some(_) if i == selected => ('●', accent()),
                    Some(_) => ('●', Color::LightMagenta),
                };
                let mut style = Style::default().fg(colour);
                if i == selected {
                    style = style.add_modifier(Modifier::BOLD);
                }
                let name: String = place.name.chars().take(24).collect();
                ctx.print(place.location.longitude, place.location.latitude, Line::styled(format!("{mark} {name}"), style));
            }
        });
    frame.render_widget(Clear, rect);
    frame.render_widget(canvas, map_area);

    // The places: name, then when and how far.
    let here = app.settings.own_location();
    let list_block = block("Places", false)
        .title_bottom(Line::styled(" ↑↓ · Enter open · o map · Esc ", Style::default().fg(dim())));
    let list_inner = list_block.inner(list_area);
    let rows = usize::from(list_inner.height / 2).max(1);
    let first = selected.saturating_sub(rows - 1);
    let mut lines = Vec::new();
    for (i, place) in places.iter().enumerate().skip(first).take(rows) {
        let picked = i == selected;
        let base = if picked { Style::default().bg(selected_bg()) } else { Style::default() };
        let mark = if place.key.is_none() { "◆ " } else { "● " };
        lines.push(Line::from(vec![Span::styled(mark, base.fg(accent())), Span::styled(place.name.clone(), base.add_modifier(Modifier::BOLD))]));
        let mut about = match place.at {
            Some(at) => format!("  {} ago", ago(at as i64)),
            None => "  this station (settings)".to_string(),
        };
        if let (Some(here), Some(_)) = (here, &place.key) {
            about.push_str(&format!(" · {}", here.away(&place.location)));
        }
        lines.push(Line::styled(about, base.fg(dim())));
    }
    frame.render_widget(Paragraph::new(lines).block(list_block), list_area);
    app.regions.map = rect;
    app.regions.map_list = list_inner;
    app.regions.map_first = first;
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::lxmf::Location;
    use crate::store::{Conversation, Message, MessageState, Store};

    fn screen(app: &mut crate::app::App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn places_on_the_map_and_in_the_list() {
        let dir = std::env::temp_dir().join(format!("rettui-ui-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = "ab".repeat(16);
        let mut store = Store::default();
        let shared = Message {
            id: "in-1".into(),
            incoming: true,
            timestamp: crate::app::now() - 300.0,
            state: MessageState::Received { verified: true },
            location: Some(Location::at(48.8566, 2.3522)),
            ..Message::default()
        };
        store.conversations.insert(key.clone(), Conversation { messages: vec![shared], ..Conversation::default() });
        let settings = crate::config::Settings { location: Some("51.5074, -0.1278".into()), ..crate::config::Settings::default() };
        let mut app = crate::app::test_app(&dir, settings, store);
        app.tab = crate::app::Tab::Messages;
        app.store.update_contact(&key, |c| c.alias = Some("Alice".into()));
        app.on_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
        let shown = screen(&mut app, 100, 30);
        // Both on the map, and listed: when, and how far from here.
        assert!(shown.contains("◆ This station") && shown.contains("● Alice"), "{shown}");
        assert!(shown.contains("5m ago · 344 km SE"), "{shown}");
        assert!(shown.contains("km across"), "{shown}");
        // Narrow: the list goes under the map.
        let narrow = screen(&mut app, 50, 30);
        assert!(narrow.contains("Places") && narrow.contains("Alice"), "{narrow}");
        // A click on a place in the list picks it.
        let list = app.regions.map_list;
        app.on_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: list.x + 1,
            row: list.y + 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.map.as_ref().unwrap().selected, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
