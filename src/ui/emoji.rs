//! The emoji picker and the `:name` list, just above the input they put
//! emoji into.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{block, dim, picked_style};
use crate::app::App;
use crate::app::emoji::EmojiHit;
use crate::emoji;

/// A grid cell: the emoji (two columns) with one either side, so the
/// chosen one's highlight shows around it.
const CELL: u16 = 4;
/// A tab: the group's emoji and a column before it.
const TAB: u16 = 3;
/// The grid's most columns and rows.
const COLUMNS: u16 = 9;
const ROWS: u16 = 6;
/// The recently used emoji's tab.
const RECENT_ICON: &str = "🕘";

/// Whichever is open: the picker, or the `:name` list. `input` is the
/// input's text area (scrolled `scroll` columns); both stay in `bounds`.
pub(super) fn draw_emoji(frame: &mut Frame, app: &mut App, input: Rect, scroll: usize, bounds: Rect) {
    app.regions.emoji_popup = Rect::default();
    app.regions.emoji_hits.clear();
    if app.emoji.is_some() {
        draw_picker(frame, app, input, scroll, bounds);
    } else {
        draw_shortcodes(frame, app, input, scroll, bounds);
    }
}

/// Where a popup `width` by `height` goes: on the input's box (a row above
/// its text), from `column` of the text, inside `bounds`.
fn above(input: Rect, column: usize, width: u16, height: u16, bounds: Rect) -> Rect {
    let bottom = input.y.saturating_sub(1);
    let x = (input.x + column as u16).saturating_sub(1).min(bounds.right().saturating_sub(width)).max(bounds.x);
    Rect::new(x, bottom.saturating_sub(height), width, height).intersection(bounds)
}

fn draw_picker(frame: &mut Frame, app: &mut App, input: Rect, scroll: usize, bounds: Rect) {
    let choices = app.emoji_choices();
    let recent_empty = emoji::recent(&app.store.recent_emoji).is_empty();
    let Some(target) = app.emoji.as_ref().map(|p| p.target) else { return };
    let cursor = app.emoji_input(target).cursor_column().saturating_sub(scroll);
    let Some(picker) = &mut app.emoji else { return };
    let width = (COLUMNS * CELL + 2).min(bounds.width);
    let columns = width.saturating_sub(2) / CELL;
    // The search, the tabs and the chosen emoji's name, around the grid.
    let room = input.y.saturating_sub(1).saturating_sub(bounds.y);
    let rows = ROWS.min(room.saturating_sub(5));
    if columns == 0 || rows == 0 {
        return;
    }
    let area = above(input, cursor, width, rows + 5, bounds);
    picker.columns = columns as usize;
    picker.rows = rows as usize;
    picker.pick = picker.pick.min(choices.len().saturating_sub(1));
    picker.follow();

    let searching = !picker.search.text().trim().is_empty();
    let title = match (searching, picker.tab) {
        (true, _) => format!("Emoji · {} found", choices.len()),
        (false, 0) => "Emoji · used lately".to_string(),
        (false, tab) => format!("Emoji · {}", emoji::groups()[tab - 1].name),
    };
    let hint =
        if target == crate::app::emoji::EmojiTarget::Reaction { " Enter react · Esc close " } else { " Enter insert · Esc close " };
    let popup = block(&title, true).title_bottom(Line::styled(hint, Style::default().fg(dim())));
    let inner = popup.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(popup, area);
    app.regions.emoji_popup = area;

    // The search, with the cursor at its end.
    let search = if picker.search.text().is_empty() {
        Line::from(vec![
            Span::styled("Search ", Style::default().fg(dim())),
            Span::styled("a name, or Tab for groups", Style::default().fg(dim()).italic()),
        ])
    } else {
        Line::from(vec![Span::styled("Search ", Style::default().fg(dim())), Span::raw(picker.search.text().to_string())])
    };
    frame.render_widget(Paragraph::new(search), Rect { height: 1, ..inner });
    let at = inner.x + 7 + picker.search.cursor_column() as u16;
    frame.set_cursor_position(Position::new(at.min(inner.right().saturating_sub(1)), inner.y));

    // The tabs.
    let icons = std::iter::once(RECENT_ICON).chain(emoji::groups().iter().map(|g| g.icon));
    let mut tabs = Vec::new();
    for (tab, icon) in icons.enumerate() {
        let x = inner.x + tab as u16 * TAB;
        if x + TAB > inner.right() {
            break;
        }
        let style = if !searching && tab == picker.tab { picked_style() } else { Style::default() };
        tabs.push(Span::styled(format!(" {icon}"), style));
        app.regions.emoji_hits.push((Rect::new(x, inner.y + 1, TAB, 1), EmojiHit::Tab(tab)));
    }
    frame.render_widget(Paragraph::new(Line::from(tabs)), Rect { y: inner.y + 1, height: 1, ..inner });

    // The grid, from its top row on screen.
    let grid = Rect { y: inner.y + 2, height: rows, ..inner };
    let mut lines = Vec::new();
    for row in 0..picker.rows {
        let first = (picker.top + row) * picker.columns;
        let mut spans = Vec::new();
        for (column, chosen) in choices.iter().skip(first).take(picker.columns).enumerate() {
            let index = first + column;
            let style = if index == picker.pick { picked_style() } else { Style::default() };
            spans.push(Span::styled(format!(" {} ", chosen.as_str()), style));
            let rect = Rect::new(grid.x + column as u16 * CELL, grid.y + row as u16, CELL, 1);
            app.regions.emoji_hits.push((rect, EmojiHit::Pick(index)));
        }
        lines.push(Line::from(spans));
    }
    if choices.is_empty() {
        let why = match (searching, picker.tab) {
            (true, _) => "No emoji with that name",
            (false, 0) if recent_empty => "None yet: the ones you pick show here",
            _ => "",
        };
        lines = vec![Line::styled(why, Style::default().fg(dim()))];
    }
    frame.render_widget(Paragraph::new(lines), grid);

    // What the chosen one is called (its shortcode first: names run long).
    let name = choices.get(picker.pick).map(|chosen| {
        let code = emoji::shortcode_for(chosen, picker.search.text()).map(|c| format!(":{c}:  ")).unwrap_or_default();
        Line::from(vec![
            Span::raw(format!("{} {code}", chosen.as_str())),
            Span::styled(chosen.name().to_string(), Style::default().fg(dim())),
        ])
    });
    let name_row = Rect { y: grid.bottom(), height: 1, ..inner };
    frame.render_widget(Paragraph::new(name.unwrap_or_default()), name_row);
}

/// While typing `:name`: the emoji it could be, under the `:`.
fn draw_shortcodes(frame: &mut Frame, app: &mut App, input: Rect, scroll: usize, bounds: Rect) {
    let Some((at, found)) = app.shortcode_matches() else { return };
    let Some(target) = app.emoji_target() else { return };
    let text = app.emoji_input(target).text();
    let typed = text[at + 1..app.emoji_input(target).before_cursor().len()].to_string();
    let column = text[..at].width().saturating_sub(scroll);
    let pick = app.shortcode.pick.min(found.len() - 1);
    let rows: Vec<(String, String)> = found
        .iter()
        .map(|e| {
            let label = emoji::shortcode_for(e, &typed).map(|c| format!(":{c}:")).unwrap_or_else(|| e.name().to_string());
            (e.as_str().to_string(), label)
        })
        .collect();
    let width = (rows.iter().map(|(_, label)| label.width() + 5).max().unwrap_or(0).max(18) as u16 + 2).min(bounds.width);
    let area = above(input, column, width, rows.len() as u16 + 2, bounds);
    if area.height < 3 {
        return;
    }
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, (emoji, label))| {
            if i == pick {
                ListItem::new(Line::styled(format!("›{emoji} {label} "), picked_style()))
            } else {
                ListItem::new(Line::from(vec![Span::raw(format!(" {emoji} ")), Span::styled(label.clone(), Style::default().fg(dim()))]))
            }
        })
        .collect();
    let list = block("Emoji", true).title_bottom(Line::styled(" Tab pick ", Style::default().fg(dim())));
    let inner = list.inner(area);
    for row in 0..rows.len().min(inner.height as usize) {
        app.regions.emoji_hits.push((Rect::new(inner.x, inner.y + row as u16, inner.width, 1), EmojiHit::Pick(row)));
    }
    app.regions.emoji_popup = area;
    frame.render_widget(Clear, area);
    frame.render_widget(List::new(items).block(list), area);
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::{Tab, test_app};
    use crate::config::Settings;
    use crate::store::{Conversation, Store};

    fn writing(name: &str) -> (App, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("rettui-emoji-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = "ab".repeat(16);
        let mut store = Store::default();
        store.conversations.insert(key.clone(), Conversation::default());
        let mut app = test_app(&dir, Settings::default(), store);
        app.tab = Tab::Messages;
        app.active_conversation = Some(key);
        app.composing = true;
        (app, dir)
    }

    fn draw(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| super::super::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    fn click(app: &mut App, hit: EmojiHit) {
        let (rect, _) = *app.regions.emoji_hits.iter().find(|(_, h)| *h == hit).unwrap();
        let mouse = |kind| MouseEvent { kind, column: rect.x + 1, row: rect.y, modifiers: KeyModifiers::NONE };
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left)));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left)));
    }

    #[test]
    fn the_picker_shows_above_the_message_box_and_takes_clicks() {
        let (mut app, dir) = writing("picker");
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("Emoji · Smileys & emotion") && screen.contains("😀"), "{screen}");
        assert!(screen.contains(":grinning:  grinning face"), "{screen}");
        // A tab, then an emoji in it.
        click(&mut app, EmojiHit::Tab(4));
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("Emoji · Food & drink"), "{screen}");
        let second = app.emoji_choices()[1].as_str();
        click(&mut app, EmojiHit::Pick(1));
        assert!(app.emoji.is_none());
        assert_eq!(app.compose.text(), second);
        // At 80×24 it still fits.
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        let screen = draw(&mut app, 80, 24);
        assert!(screen.contains("used lately") && screen.contains(second), "{screen}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_typed_after_a_colon_list_their_emoji() {
        let (mut app, dir) = writing("list");
        for c in "here :drag".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains(":dragon:") && screen.contains(":dragon_face:"), "{screen}");
        click(&mut app, EmojiHit::Pick(0));
        assert_eq!(app.compose.text(), "here 🐉");
        let screen = draw(&mut app, 100, 30);
        assert!(!screen.contains(":dragon_face:"), "{screen}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
