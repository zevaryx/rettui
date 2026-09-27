//! Editing `settings.json` from the TUI and the web UI.
//!
//! Changes are made to the file on disk (so values that only apply to this
//! session, like `--rns-config`, are not written into it) and then applied
//! to the running client where they can be.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;

use super::{App, PromptKind};
use crate::config::{self, Effect, FieldKind, Settings};
use crate::net::{NetCommand, parse_hash};

fn minutes(n: u64) -> Option<Duration> {
    (n > 0).then(|| Duration::from_secs(n * 60))
}

impl App {
    /// The settings as saved in `settings.json`.
    pub fn saved_settings(&self) -> Result<Settings, String> {
        Settings::load(&self.paths.settings).map_err(|e| format!("Could not read settings: {e:#}"))
    }

    /// Change settings (`key`, text value) in `settings.json` and apply them.
    /// Nothing is saved unless every value is valid. Returns notes about
    /// changes that only apply after a restart.
    pub fn update_settings(&mut self, changes: &[(&str, &str)]) -> Result<Vec<String>, String> {
        let before = self.saved_settings()?;
        let mut after = before.clone();
        for (key, value) in changes {
            after.set_field(key, value)?;
        }
        let changed: Vec<&'static config::Field> = config::FIELDS
            .iter()
            .filter(|f| before.field_value(f.key) != after.field_value(f.key))
            .collect();
        if changed.is_empty() {
            return Ok(Vec::new());
        }
        after
            .save(&self.paths.settings)
            .map_err(|e| format!("Could not save settings: {e:#}"))?;
        self.settings_file = after.clone();

        let mut notes = Vec::new();
        let mut intervals = false;
        let mut node = false;
        for field in &changed {
            match field.key {
                "display_name" => {
                    self.settings.display_name = after.display_name.clone();
                    self.send(NetCommand::SetDisplayName(after.display_name.clone()));
                    self.send(NetCommand::Announce);
                }
                "propagation_node" => {
                    self.settings.propagation_node = after.propagation_node.clone();
                    let node = after.propagation_node.as_deref().and_then(parse_hash);
                    self.send(NetCommand::SetPropagationNode(node));
                }
                "announce_interval_mins" | "sync_interval_mins" => {
                    self.settings.announce_interval_mins = after.announce_interval_mins;
                    self.settings.sync_interval_mins = after.sync_interval_mins;
                    intervals = true;
                }
                "home" => self.settings.home = after.home.clone(),
                "cache_hours" => {
                    self.settings.cache_hours = after.cache_hours;
                    self.cache.set_default_ttl(Duration::from_secs(after.cache_hours * 3600));
                }
                "announce_at_start" => self.settings.announce_at_start = after.announce_at_start,
                "wrap_lines" => self.settings.wrap_lines = after.wrap_lines,
                "node_enabled" | "node_name" | "node_announce_interval_mins" | "node_dir" | "node_executable_pages" => {
                    self.settings.node_enabled = after.node_enabled;
                    self.settings.node_name = after.node_name.clone();
                    self.settings.node_announce_interval_mins = after.node_announce_interval_mins;
                    self.settings.node_dir = after.node_dir.clone();
                    self.settings.node_executable_pages = after.node_executable_pages;
                    node = true;
                }
                // The running Reticulum instance keeps its config.
                _ => {}
            }
            if field.effect == Effect::NextStart {
                notes.push(format!("{} applies the next time rettui starts", field.label));
            }
        }
        // The node announces the display name unless it has its own name.
        let renamed = changed.iter().any(|f| f.key == "display_name") && after.node_name.is_none();
        if node || (renamed && after.node_enabled) {
            self.apply_node_settings();
        }
        if intervals {
            self.send(NetCommand::SetIntervals {
                announce: minutes(after.announce_interval_mins),
                sync: minutes(after.sync_interval_mins),
            });
        }
        let names: Vec<&str> = changed.iter().map(|f| f.label).collect();
        self.log(format!("Settings saved: {}", names.join(", ")));
        Ok(notes)
    }

    /// Change one setting, confirming in the footer (the log already has
    /// what was saved; errors go to both).
    pub fn update_setting(&mut self, key: &str, value: &str) {
        let confirmation = match self.update_settings(&[(key, value)]) {
            Ok(notes) if notes.is_empty() => {
                format!("Saved {}", config::field(key).map_or(key, |f| f.label))
            }
            Ok(notes) => notes.join("; "),
            Err(e) => return self.fail(e),
        };
        self.confirm(confirmation);
    }

    // ---- Status tab: the settings list -------------------------------------

    pub(super) fn status_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => self.open_restart_prompt(),
            KeyCode::Down | KeyCode::Char('j') => self.move_setting(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_setting(-1),
            KeyCode::Enter | KeyCode::Char(' ') => self.edit_setting(),
            KeyCode::Char('e') => {
                let name = self.settings.display_name.clone();
                self.open_prompt(PromptKind::DisplayName, "Display name", &name);
            }
            KeyCode::Char('y') => {
                let address = self.lxmf_hash.map(hex::encode).unwrap_or_default();
                self.copy(&address, "your LXMF address");
            }
            _ => {}
        }
    }

    pub(super) fn move_setting(&mut self, delta: isize) {
        let current = self.settings_list.selected().unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(config::FIELDS.len() - 1);
        self.settings_list.select(Some(next));
    }

    /// Edit the selected setting: toggles flip, anything else opens a prompt.
    fn edit_setting(&mut self) {
        // Pick up changes made to the file by hand since it was read.
        if let Ok(saved) = self.saved_settings() {
            self.settings_file = saved;
        }
        let Some(field) = self.settings_list.selected().and_then(|i| config::FIELDS.get(i)) else {
            return;
        };
        let current = self.settings_file.field_value(field.key);
        match field.kind {
            FieldKind::Toggle => {
                let flipped = if current == "true" { "false" } else { "true" };
                self.update_setting(field.key, flipped);
            }
            FieldKind::Optional => {
                let title = format!("{} (empty for none)", field.label);
                self.open_prompt(PromptKind::EditSetting(field.key), &title, &current);
            }
            FieldKind::Text | FieldKind::Number => {
                self.open_prompt(PromptKind::EditSetting(field.key), field.label, &current);
            }
        }
    }

    pub(super) fn click_status(&mut self, at: Position, double: bool) {
        let area = self.regions.settings;
        if !area.contains(at) {
            return;
        }
        let index = self.settings_list.offset() + (at.y - area.y) as usize;
        if index < config::FIELDS.len() {
            self.settings_list.select(Some(index));
            if double {
                self.edit_setting();
            }
        }
    }
}
