//! How wide each tab's side list is (terminal UI): its own width, dragged
//! by its border, or changed with `<` and `>` (`=` goes back to the
//! default), kept in the settings (`pane_widths`).

use ratatui::layout::{Position, Rect};

use super::{App, Tab};
use crate::config::Settings;

/// The narrowest a side list gets, and the room it leaves beside it.
pub const MIN_SIDE: u16 = 16;
pub const MIN_MAIN: u16 = 20;
/// Columns a key press moves the border by.
const STEP: u16 = 2;

/// Where the shown tab's side list is: the area it shares with the pane
/// beside it, its width, and the width it has by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SideSplit {
    pub area: Rect,
    pub width: u16,
    pub default: u16,
}

impl SideSplit {
    /// Whether `at` is on the border between the list and the pane beside
    /// it (the list's right edge, or the pane's left).
    pub fn on_border(&self, at: Position) -> bool {
        let edge = self.area.x + self.width;
        (self.area.y..self.area.bottom()).contains(&at.y) && (edge.saturating_sub(1)..=edge).contains(&at.x)
    }

    /// The widest the list may be here.
    fn widest(&self) -> u16 {
        widest(self.area.width)
    }
}

fn widest(total: u16) -> u16 {
    total.saturating_sub(MIN_MAIN).max(MIN_SIDE).min(total)
}

/// The settings' name for a tab's width.
fn key(tab: Tab) -> String {
    tab.title().to_lowercase()
}

/// Where a tab's side list is in `area`: as wide as the user left it, or
/// as `preferred` (at most a third of it).
pub fn side_split(settings: &Settings, tab: Tab, area: Rect, preferred: u16) -> SideSplit {
    let total = area.width;
    let default = preferred.min((total / 3).max(MIN_SIDE)).min(total);
    let width = match settings.pane_widths.get(&key(tab)) {
        Some(&chosen) => chosen.clamp(MIN_SIDE.min(total), widest(total)),
        None => default,
    };
    SideSplit { area, width, default }
}

impl App {
    /// A press on the side list's border starts resizing it; a double
    /// click there puts it back as it was by default. False if it isn't
    /// on the border.
    pub(super) fn press_side_border(&mut self, at: Position, double: bool) -> bool {
        let Some(split) = self.regions.side_split.filter(|s| s.on_border(at)) else { return false };
        if double {
            self.set_side_width(None);
        } else {
            self.resizing_side = Some(split);
        }
        true
    }

    /// Dragging the border to column `x`.
    pub(super) fn drag_side_border(&mut self, x: u16) {
        let Some(split) = self.resizing_side else { return };
        let width = (x + 1).saturating_sub(split.area.x).clamp(MIN_SIDE, split.widest());
        self.settings.pane_widths.insert(key(self.tab), width);
    }

    /// Let go of the border: the width is kept.
    pub(super) fn release_side_border(&mut self) {
        if self.resizing_side.take().is_some() {
            let width = self.settings.pane_widths.get(&key(self.tab)).copied();
            self.set_side_width(width);
        }
    }

    /// `<` and `>`: the side list narrower or wider (than it is now, drawn
    /// or not).
    pub(super) fn nudge_side(&mut self, wider: bool) {
        let Some(split) = self.regions.side_split else { return };
        let now = side_split(&self.settings, self.tab, split.area, split.default).width;
        let width = if wider { now + STEP } else { now.saturating_sub(STEP) };
        self.set_side_width(Some(width.clamp(MIN_SIDE, split.widest())));
    }

    /// `=`: the side list back to its default width.
    pub(super) fn reset_side(&mut self) {
        if self.regions.side_split.is_some() {
            self.set_side_width(None);
        }
    }

    /// Set the shown tab's side list's width (`None`: the default), and
    /// save it.
    fn set_side_width(&mut self, width: Option<u16>) {
        let tab = key(self.tab);
        self.save_flag(|settings| match width {
            Some(width) => {
                settings.pane_widths.insert(tab.clone(), width);
            }
            None => {
                settings.pane_widths.remove(&tab);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::store::Store;

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
    }

    #[test]
    fn each_tab_keeps_its_own_width_dragged_or_by_keys() {
        let dir = std::env::temp_dir().join(format!("rettui-panes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, _net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        let mut draw = |app: &mut App| {
            terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
            app.regions.side_split.unwrap()
        };
        let before = draw(&mut app);
        assert_eq!(before.width, 30);
        // Dragged by its border, 10 columns to the right.
        let edge = before.area.x + before.width - 1;
        let row = before.area.y + 5;
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), edge, row));
        app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), edge + 10, row));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), edge + 10, row));
        assert_eq!(draw(&mut app).width, 40);
        // Saved, for next time.
        assert_eq!(Settings::load(&app.paths.settings).unwrap().pane_widths.get("messages"), Some(&40));
        // Never so wide the pane beside it hasn't room (pressed on the
        // pane's side of the border, a row down: not a double click).
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), before.area.x + 40, row + 1));
        app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 500, row + 1));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 500, row + 1));
        assert_eq!(draw(&mut app).width, before.area.width - MIN_MAIN);
        // A double click on it: as it was by default.
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), before.area.x + before.area.width - MIN_MAIN, row + 3));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), before.area.x + before.area.width - MIN_MAIN, row + 3));
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), before.area.x + before.area.width - MIN_MAIN, row + 3));
        assert_eq!(draw(&mut app).width, 30);
        app.on_key(KeyEvent::from(KeyCode::Char('>')));
        app.on_key(KeyEvent::from(KeyCode::Char('>')));
        assert_eq!(draw(&mut app).width, 34);
        // `<` narrower, `=` back to the default.
        app.on_key(KeyEvent::from(KeyCode::Char('<')));
        assert_eq!(draw(&mut app).width, 34 - STEP);
        app.on_key(KeyEvent::from(KeyCode::Char('=')));
        assert_eq!(draw(&mut app).width, 30);
        assert!(Settings::load(&app.paths.settings).unwrap().pane_widths.is_empty());
        // Another tab's is its own.
        app.on_key(KeyEvent::from(KeyCode::Char('>')));
        app.switch_tab(Tab::Games);
        assert_eq!(draw(&mut app).width, 34);
        app.switch_tab(Tab::Messages);
        assert_eq!(draw(&mut app).width, 32);
        // A tab without a side list has nothing to resize.
        app.switch_tab(Tab::Status);
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        assert!(app.regions.side_split.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
