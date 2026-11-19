//! Chat formatting in the channel input: the keys for it (`Alt-B` bold,
//! `Alt-I` italic, `Alt-U` underline, `Alt-F` text colour, `Alt-G` background
//! colour, `Alt-N` normal), the colour picker those last two open, and the
//! bar of buttons shown while text is selected.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::format::{self, Action};
use super::{App, Tab};
use crate::term::input::TextInput;

/// The picker's swatches, as Micron's three-digit colours. The web UI's
/// picker has the same ones in the same order.
pub const SWATCHES: [&str; 14] = ["f00", "f80", "ff0", "8f0", "0f0", "0fc", "0af", "00f", "80f", "f0f", "fff", "aaa", "555", "000"];
/// Swatches in a row of the picker's grid.
pub const COLUMNS: usize = 7;

/// The colour picker: a grid of swatches and a line to type a colour in.
#[derive(Debug)]
pub struct ColourPicker {
    /// Whether it picks the background (else the text colour).
    pub background: bool,
    /// The chosen swatch.
    pub pick: usize,
    /// A colour typed instead: 3 or 6 hex digits, `#` optional.
    pub hex: TextInput,
    /// Why what was typed can't be used.
    pub error: Option<String>,
}

impl ColourPicker {
    pub fn action(&self) -> Action {
        if self.background { Action::Background } else { Action::Foreground }
    }
}

/// What a click on the picker or the formatting bar lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourHit {
    Swatch(usize),
}

impl App {
    /// Whether chat formatting applies: a room's input is being written in.
    fn formatting_chat(&self) -> bool {
        self.tab == Tab::Channels && self.channels.typing
    }

    /// The Alt-key shortcuts of chat formatting; true when `key` was one.
    pub(crate) fn chat_format_key(&mut self, key: KeyEvent) -> bool {
        let KeyCode::Char(c) = key.code else { return false };
        if !key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match Action::from_key(c).filter(|a| format::CHAT_ACTIONS.contains(a)) {
            Some(action) => {
                self.chat_format(action);
                true
            }
            None => false,
        }
    }

    /// Do a formatting action on the channel input: the colours ask first.
    pub fn chat_format(&mut self, action: Action) {
        if !self.formatting_chat() {
            return;
        }
        match action {
            Action::Foreground | Action::Background => {
                let background = action == Action::Background;
                self.colour = Some(ColourPicker { background, pick: 0, hex: TextInput::default(), error: None });
            }
            _ => {
                if let Err(e) = format::apply_chat(&mut self.channels.input, action, "") {
                    self.warn(e);
                }
            }
        }
    }

    /// Keys while the picker is open: arrows choose a swatch, typing fills
    /// the colour line, `Enter` applies and `Esc` cancels.
    pub(crate) fn colour_key(&mut self, key: KeyEvent) {
        let Some(picker) = &mut self.colour else { return };
        let last = SWATCHES.len() - 1;
        match key.code {
            KeyCode::Esc => self.colour = None,
            KeyCode::Enter => {
                let typed = picker.hex.text().trim().to_string();
                let code = if typed.is_empty() { SWATCHES[picker.pick].to_string() } else { typed };
                self.apply_colour(&code);
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End => {
                picker.pick = match key.code {
                    KeyCode::Left => picker.pick.saturating_sub(1),
                    KeyCode::Right => (picker.pick + 1).min(last),
                    KeyCode::Up => picker.pick.saturating_sub(COLUMNS),
                    KeyCode::Down => (picker.pick + COLUMNS).min(last),
                    KeyCode::Home => 0,
                    _ => last,
                };
                // Choosing a swatch sets aside what was typed.
                picker.hex = TextInput::default();
                picker.error = None;
            }
            // Alt keys are shortcuts, not text.
            KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::ALT) => {}
            _ => {
                if picker.hex.handle(key) {
                    picker.error = None;
                }
            }
        }
    }

    /// A click on the picker.
    pub(crate) fn click_colour(&mut self, hit: ColourHit) {
        match hit {
            ColourHit::Swatch(i) => {
                if let Some(code) = SWATCHES.get(i) {
                    self.apply_colour(code);
                }
            }
        }
    }

    /// Put the colour (3 or 6 hex digits) on the selection; a colour that
    /// isn't one is said, with the picker left open.
    fn apply_colour(&mut self, code: &str) {
        let Some(picker) = &self.colour else { return };
        let action = picker.action();
        match format::apply_chat(&mut self.channels.input, action, code) {
            Ok(()) => self.colour = None,
            Err(e) => {
                if let Some(picker) = &mut self.colour {
                    picker.error = Some(e);
                }
            }
        }
    }

    /// A click on a button of the formatting bar.
    pub(crate) fn click_format_bar(&mut self, action: Action) {
        self.chat_format(action);
    }
}
