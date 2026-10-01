//! Reticulum tab: editing the Reticulum config file, option by option or as
//! text.
//!
//! The `rns_*` operations are shared by the TUI and the web UI. Each one
//! reads the file fresh, applies the change, checks the result with
//! rns-runtime's parser and saves it (see [`crate::reticulum`]). The web UI
//! passes `restricted`, which refuses new or changed pipe interface
//! commands, since those run programs.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::{App, PromptKind};
use crate::reticulum::schema::{self, Kind};
use crate::reticulum::{self as rns, Check, OptionView, Section};
use crate::term::textarea::TextArea;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsFocus {
    Sections,
    Options,
}

/// A row of the options list: a group heading or an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsRow {
    Group(&'static str),
    Option(usize),
}

/// What a pick from the list overlay does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickFor {
    /// The type of a new interface with this name.
    NewInterface(String),
    /// A choice option of a section.
    Option(Section, String),
}

pub struct Picker {
    pub title: String,
    /// (value, label)
    pub choices: Vec<(String, String)>,
    pub list: ListState,
    pub purpose: PickFor,
}

/// The config file open as text.
pub struct RnsText {
    pub area: TextArea,
    saved: String,
}

impl RnsText {
    pub fn dirty(&self) -> bool {
        self.area.text() != self.saved
    }
}

pub struct RnsState {
    pub path: PathBuf,
    /// The file as last read (rsReticulum's default when it does not exist).
    pub text: String,
    pub exists: bool,
    /// Why the file could not be read.
    pub load_error: Option<String>,
    pub check: Check,
    pub sections: Vec<Section>,
    pub section_list: ListState,
    pub options: Vec<OptionView>,
    pub rows: Vec<RnsRow>,
    pub option_list: ListState,
    pub focus: RnsFocus,
    pub picker: Option<Picker>,
    pub editor: Option<RnsText>,
}

impl Default for RnsState {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            text: String::new(),
            exists: false,
            load_error: None,
            check: Check::default(),
            sections: Vec::new(),
            section_list: ListState::default().with_selected(Some(0)),
            options: Vec::new(),
            rows: Vec::new(),
            option_list: ListState::default(),
            focus: RnsFocus::Sections,
            picker: None,
            editor: None,
        }
    }
}

impl RnsState {
    pub fn section(&self) -> Option<&Section> {
        self.sections.get(self.section_list.selected()?)
    }

    pub fn selected_option(&self) -> Option<&OptionView> {
        match self.rows.get(self.option_list.selected()?)? {
            RnsRow::Option(i) => self.options.get(*i),
            RnsRow::Group(_) => None,
        }
    }
}

impl App {
    /// The Reticulum config file in use.
    pub fn rns_path(&self) -> PathBuf {
        rns::config_path(self.settings.rns_config.as_deref())
    }

    /// Re-read the file and rebuild the lists (keeping the selection).
    pub fn rns_reload(&mut self) {
        let state = &mut self.rns;
        state.path = rns::config_path(self.settings.rns_config.as_deref());
        match rns::load(&state.path) {
            Ok((text, exists)) => {
                state.text = text;
                state.exists = exists;
                state.load_error = None;
            }
            Err(e) => {
                state.text.clear();
                state.load_error = Some(e);
            }
        }
        let selected = state.section().cloned();
        state.sections = rns::sections(&state.text);
        let index = selected.and_then(|s| state.sections.iter().position(|x| *x == s)).unwrap_or(0);
        state.section_list.select(Some(index.min(state.sections.len() - 1)));
        self.rns_rebuild_options();
    }

    fn rns_rebuild_options(&mut self) {
        let state = &mut self.rns;
        let (config, check) = rns::check(&state.text);
        state.check = check;
        let selected_key = state.selected_option().map(|o| o.key.clone());
        state.options = state.section().map_or_else(Vec::new, |s| rns::options(config.as_ref(), s));
        state.rows.clear();
        let mut group = "";
        for (i, option) in state.options.iter().enumerate() {
            if option.group != group {
                group = option.group;
                state.rows.push(RnsRow::Group(group));
            }
            state.rows.push(RnsRow::Option(i));
        }
        let row = selected_key
            .and_then(|key| {
                state.rows.iter().position(|r| matches!(r, RnsRow::Option(i) if state.options[*i].key == key))
            })
            .or_else(|| state.rows.iter().position(|r| matches!(r, RnsRow::Option(_))));
        state.option_list.select(row);
    }

    /// Change the file: read it, apply `edit`, check and save. Returns the
    /// warnings about interfaces that will not start.
    pub(super) fn rns_edit(&mut self, restricted: bool, edit: impl FnOnce(&str) -> Result<String, String>) -> Result<Vec<String>, String> {
        let path = self.rns_path();
        let (before, _) = rns::load(&path)?;
        let after = edit(&before)?;
        if after == before && path.is_file() {
            return Ok(Vec::new());
        }
        let check = rns::validate(&before, &after, restricted)?;
        rns::save(&path, &after)?;
        self.log(format!("Saved {} (applies when Reticulum restarts)", path.display()));
        self.rns_reload();
        Ok(check.warnings)
    }

    pub fn rns_set_options(&mut self, section: &Section, changes: &[(&str, &str)], restricted: bool) -> Result<Vec<String>, String> {
        let section = section.clone();
        self.rns_edit(restricted, |text| rns::set_options(text, &section, changes))
    }

    pub fn rns_add_interface(&mut self, name: &str, kind: &str, restricted: bool) -> Result<Vec<String>, String> {
        self.rns_edit(restricted, |text| rns::add_interface(text, name, kind))
    }

    pub fn rns_rename_interface(&mut self, name: &str, to: &str, restricted: bool) -> Result<Vec<String>, String> {
        let warnings = self.rns_edit(restricted, |text| rns::rename_interface(text, name, to))?;
        self.rns_select(&Section::Interface(to.trim().to_string()));
        Ok(warnings)
    }

    pub fn rns_remove_interface(&mut self, name: &str, restricted: bool) -> Result<Vec<String>, String> {
        self.rns_edit(restricted, |text| rns::remove_interface(text, name))
    }

    /// Replace the whole file with edited text.
    pub fn rns_save_text(&mut self, text: &str, restricted: bool) -> Result<Vec<String>, String> {
        let text = text.to_string();
        self.rns_edit(restricted, move |_| Ok(text))
    }

    // ---- TUI ---------------------------------------------------------------

    fn rns_select(&mut self, section: &Section) {
        if let Some(i) = self.rns.sections.iter().position(|s| s == section) {
            self.rns.section_list.select(Some(i));
            self.rns_rebuild_options();
        }
    }

    /// Show the outcome of a TUI edit in the footer.
    fn rns_report(&mut self, result: Result<Vec<String>, String>, done: &str) {
        match result {
            Ok(warnings) if warnings.is_empty() => {
                self.confirm(format!("{done}; restart to apply"));
            }
            Ok(warnings) => self.warn(format!("{done}, but: {}", warnings.join("; "))),
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn rns_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('r') {
            return self.open_restart_prompt();
        }
        if self.rns.load_error.is_some() && key.code != KeyCode::Char('R') {
            return;
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.rns.focus = match self.rns.focus {
                    RnsFocus::Sections => RnsFocus::Options,
                    RnsFocus::Options => RnsFocus::Sections,
                };
            }
            KeyCode::Right | KeyCode::Char('l') if self.rns.focus == RnsFocus::Sections => self.rns.focus = RnsFocus::Options,
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc if self.rns.focus == RnsFocus::Options => {
                self.rns.focus = RnsFocus::Sections;
            }
            KeyCode::Down | KeyCode::Char('j') => self.rns_move(1),
            KeyCode::Up | KeyCode::Char('k') => self.rns_move(-1),
            KeyCode::PageDown => self.rns_move(10),
            KeyCode::PageUp => self.rns_move(-10),
            KeyCode::Char('t') => self.rns_open_text(),
            KeyCode::Char('R') => {
                self.rns_reload();
                self.confirm("Reloaded the Reticulum config");
            }
            KeyCode::Char('a') => self.open_prompt(PromptKind::NewInterface, "New interface name", ""),
            _ if self.rns.focus == RnsFocus::Options => self.rns_option_key(key),
            _ => self.rns_section_key(key),
        }
    }

    fn rns_move(&mut self, delta: isize) {
        match self.rns.focus {
            RnsFocus::Sections => {
                let len = self.rns.sections.len();
                let i = self.rns.section_list.selected().unwrap_or(0);
                self.rns.section_list.select(Some(i.saturating_add_signed(delta).min(len.saturating_sub(1))));
                self.rns_rebuild_options();
            }
            RnsFocus::Options => {
                let rows = &self.rns.rows;
                if rows.is_empty() {
                    return;
                }
                let mut i = self.rns.option_list.selected().unwrap_or(0);
                let step = delta.signum();
                for _ in 0..delta.unsigned_abs() {
                    // Step over group headings.
                    let mut next = i;
                    loop {
                        match next.checked_add_signed(step) {
                            Some(n) if n < rows.len() => next = n,
                            _ => break,
                        }
                        if matches!(rows[next], RnsRow::Option(_)) {
                            i = next;
                            break;
                        }
                    }
                }
                self.rns.option_list.select(Some(i));
            }
        }
    }

    fn rns_section_key(&mut self, key: KeyEvent) {
        let Some(Section::Interface(name)) = self.rns.section().cloned() else {
            if key.code == KeyCode::Enter {
                self.rns.focus = RnsFocus::Options;
            }
            return;
        };
        match key.code {
            KeyCode::Enter => self.rns.focus = RnsFocus::Options,
            KeyCode::Char(' ') => {
                let enabled = self.rns.options.iter().find(|o| o.key == "enabled");
                let on = enabled.and_then(|o| o.value.as_deref()).is_none_or(rns_truthy);
                let section = Section::Interface(name.clone());
                let result = self.rns_set_options(&section, &[("enabled", if on { "no" } else { "yes" })], false);
                self.rns_report(result, &format!("{name} {}", if on { "disabled" } else { "enabled" }));
            }
            KeyCode::Char('r') => {
                self.open_prompt(PromptKind::RenameInterface(name.clone()), &format!("Rename {name} to"), &name);
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                let title = format!("Delete interface {name}? Type y");
                self.open_prompt(PromptKind::ConfirmDeleteInterface(name), &title, "");
            }
            _ => {}
        }
    }

    fn rns_option_key(&mut self, key: KeyEvent) {
        let (Some(section), Some(option)) = (self.rns.section().cloned(), self.rns.selected_option().cloned()) else { return };
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('e') => self.rns_edit_option(section, option),
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace if option.value.is_some() => {
                if option.key == "type" {
                    return self.warn("An interface needs a type");
                }
                let result = self.rns_set_options(&section, &[(&option.key, "")], false);
                self.rns_report(result, &format!("{} back to its default", option.label));
            }
            _ => {}
        }
    }

    fn rns_edit_option(&mut self, section: Section, option: OptionView) {
        let current = option.value.clone().unwrap_or_default();
        match option.kind {
            Kind::Bool => {
                let on = option.value.as_deref().or(Some(option.default)).is_some_and(rns_truthy);
                let result = self.rns_set_options(&section, &[(&option.key, if on { "no" } else { "yes" })], false);
                self.rns_report(result, &format!("{} {}", option.label, if on { "off" } else { "on" }));
            }
            Kind::Choice(choices) => {
                let choices: Vec<(String, String)> = choices.iter().map(|c| (c.to_string(), c.to_string())).collect();
                self.rns_pick(&option.label, choices, &current, PickFor::Option(section, option.key.clone()));
            }
            _ if option.key == "type" => {
                let choices = schema::INTERFACE_TYPES.iter().map(|t| (t.name.to_string(), format!("{} · {}", t.label, t.name))).collect();
                self.rns_pick("Interface type", choices, &current, PickFor::Option(section, option.key.clone()));
            }
            _ => {
                let hint = match option.kind {
                    Kind::List => " (comma-separated; empty for default)",
                    _ => " (empty for default)",
                };
                let title = format!("{}{hint}", option.label);
                self.open_prompt(PromptKind::RnsOption(section, option.key.clone()), &title, &current);
            }
        }
    }

    fn rns_pick(&mut self, title: &str, choices: Vec<(String, String)>, current: &str, purpose: PickFor) {
        let index = choices.iter().position(|(v, _)| v.eq_ignore_ascii_case(current)).unwrap_or(0);
        self.rns.picker = Some(Picker {
            title: title.to_string(),
            choices,
            list: ListState::default().with_selected(Some(index)),
            purpose,
        });
    }

    pub(super) fn rns_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = &mut self.rns.picker else { return };
        let len = picker.choices.len();
        let i = picker.list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc => self.rns.picker = None,
            KeyCode::Down | KeyCode::Char('j') => picker.list.select(Some((i + 1).min(len - 1))),
            KeyCode::Up | KeyCode::Char('k') => picker.list.select(Some(i.saturating_sub(1))),
            KeyCode::Enter => {
                let picker = self.rns.picker.take().expect("picker is open");
                let (value, _) = picker.choices[i].clone();
                self.rns_picked(picker.purpose, &value);
            }
            _ => {}
        }
    }

    pub(super) fn click_rns_picker(&mut self, at: Position) {
        let area = self.regions.rns_picker;
        let Some(picker) = &mut self.rns.picker else { return };
        if !area.contains(at) {
            self.rns.picker = None;
            return;
        }
        let index = picker.list.offset() + (at.y - area.y) as usize;
        if index < picker.choices.len() {
            picker.list.select(Some(index));
            self.rns_picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        }
    }

    fn rns_picked(&mut self, purpose: PickFor, value: &str) {
        match purpose {
            PickFor::NewInterface(name) => {
                let result = self.rns_add_interface(&name, value, false);
                if result.is_ok() {
                    self.rns_select(&Section::Interface(name.trim().to_string()));
                    self.rns.focus = RnsFocus::Options;
                }
                self.rns_report(result, &format!("Added interface {}", name.trim()));
            }
            PickFor::Option(section, key) => {
                let result = self.rns_set_options(&section, &[(&key, value)], false);
                self.rns_report(result, &format!("Set {key} to {value}"));
            }
        }
    }

    /// Prompts opened from this tab.
    pub(super) fn submit_rns_prompt(&mut self, kind: PromptKind, text: &str) {
        match kind {
            PromptKind::NewInterface if !text.trim().is_empty() => {
                let choices = schema::INTERFACE_TYPES.iter().map(|t| (t.name.to_string(), t.label.to_string())).collect();
                let title = format!("Type of {}", text.trim());
                self.rns_pick(&title, choices, "AutoInterface", PickFor::NewInterface(text.to_string()));
            }
            PromptKind::RenameInterface(name) if !text.trim().is_empty() && text.trim() != name => {
                let result = self.rns_rename_interface(&name, text, false);
                self.rns_report(result, &format!("Renamed {name}"));
            }
            PromptKind::ConfirmDeleteInterface(name) if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") => {
                let result = self.rns_remove_interface(&name, false);
                self.rns_report(result, &format!("Deleted interface {name}"));
            }
            PromptKind::RnsOption(section, key) => {
                let result = self.rns_set_options(&section, &[(&key, text)], false);
                self.rns_report(result, &format!("Saved {key}"));
            }
            PromptKind::ConfirmDiscardRns if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") => {
                self.rns.editor = None;
            }
            _ => {}
        }
    }

    // ---- The file as text --------------------------------------------------

    fn rns_open_text(&mut self) {
        self.rns_reload();
        let text = self.rns.text.clone();
        self.rns.editor = Some(RnsText {
            area: TextArea::new(&text),
            saved: text,
        });
    }

    pub(super) fn rns_editor_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(editor) = &mut self.rns.editor else { return };
        match key.code {
            KeyCode::Esc if editor.dirty() => {
                self.open_prompt(PromptKind::ConfirmDiscardRns, "Discard your changes to the Reticulum config? Type y", "");
            }
            KeyCode::Esc => self.rns.editor = None,
            KeyCode::Char('s') if ctrl => {
                let text = editor.area.text();
                // The TUI runs as the local user, who may add pipe commands.
                match self.rns_save_text(&text, false) {
                    Ok(warnings) => {
                        if let Some(editor) = &mut self.rns.editor {
                            editor.saved = text;
                            editor.area.break_undo_group();
                        }
                        self.rns_report(Ok(warnings), "Saved the Reticulum config");
                    }
                    Err(e) => self.fail(e),
                }
            }
            KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
            _ => {
                editor.area.handle(key);
            }
        }
    }

    pub(super) fn rns_paste(&mut self, text: &str) {
        if let Some(editor) = &mut self.rns.editor {
            editor.area.insert_str(text);
        }
    }

    pub(super) fn click_rns(&mut self, at: Position, double: bool) {
        if let Some(editor) = &mut self.rns.editor {
            let area = self.regions.rns_editor;
            if area.contains(at) {
                let (row, col) = editor.area.position_at((at.y - area.y) as usize, (at.x - area.x) as usize);
                editor.area.set_cursor(row, col);
            }
            return;
        }
        let (sections, options) = (self.regions.rns_sections, self.regions.rns_options);
        if sections.contains(at) {
            self.rns.focus = RnsFocus::Sections;
            let index = self.rns.section_list.offset() + (at.y - sections.y) as usize;
            if index < self.rns.sections.len() {
                self.rns.section_list.select(Some(index));
                self.rns_rebuild_options();
            }
        } else if options.contains(at) {
            self.rns.focus = RnsFocus::Options;
            let index = self.rns.option_list.offset() + (at.y - options.y) as usize;
            if matches!(self.rns.rows.get(index), Some(RnsRow::Option(_))) {
                self.rns.option_list.select(Some(index));
                if double {
                    self.rns_option_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
            }
        }
    }

    pub(super) fn scroll_rns(&mut self, at: Position, delta: isize) {
        if let Some(editor) = &mut self.rns.editor {
            let (row, col) = editor.area.cursor();
            editor.area.set_cursor(row.saturating_add_signed(delta), col);
            return;
        }
        let focus = if self.regions.rns_sections.contains(at) { RnsFocus::Sections } else { RnsFocus::Options };
        let before = self.rns.focus;
        self.rns.focus = focus;
        self.rns_move(delta.signum());
        self.rns.focus = before;
    }
}

pub fn rns_truthy(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "on" | "1")
}
