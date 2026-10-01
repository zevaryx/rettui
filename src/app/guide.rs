//! The getting-started guide: shown the first time rettui starts, and from
//! the Status tab (`g`) or the web UI's Status page after that.
//!
//! Reticulum's default config only reaches the local network, so a new user
//! usually hears no one. The guide sets up what's needed to reach others,
//! following Reticulum's manual ("Getting Started Fast"): a community entry
//! point (RMAP World), optionally interface discovery (more entry points,
//! found over time, with RMAP World then used only to bootstrap), and
//! optionally an automatic propagation node; and links to where to learn
//! more. Nothing changes until the user applies it.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::{App, PromptKind};
use crate::reticulum as rns;

/// The community entry point offered: RMAP World's transport node.
pub const ENTRY_NAME: &str = "RMAP World";
pub const ENTRY_HOST: &str = "rmap.world";
pub const ENTRY_PORT: u16 = 4242;
/// Discovered entry points connected to at once, when discovery is on.
const DISCOVERED: u32 = 2;

/// Where to learn more: title, address, and what's there when the title
/// doesn't say.
pub const LINKS: &[(&str, &str, Option<&str>)] = &[
    ("Reticulum: getting started", "https://reticulum.network/manual/gettingstartedfast.html", None),
    ("Understanding Reticulum", "https://reticulum.network/manual/understanding.html", None),
    ("Connecting to others", "https://reticulum.network/manual/gettingstartedfast.html#bootstrapping-connectivity", None),
    ("Entry points on a map: RMAP World", "https://rmap.world", None),
    ("A directory of entry points", "https://directory.rns.recipes", None),
    ("Using a LoRa radio (RNode)", "https://github.com/zevaryx/rettui/wiki/RNode-Radios", Some(RNODE_NOTE)),
    ("Using rettui", "https://github.com/zevaryx/rettui/wiki", None),
];
/// LoRa radios: rettui doesn't flash them, the page says what does.
pub const RNODE_NOTE: &str = "LoRa radios reach others with no internet. Flashing one with RNode or microReticulum firmware (rnodeconf, or a web flasher), and adding it here.";

/// What each choice does, for both UIs.
pub const INTRO: &str = "Reticulum reaches others without servers: on your local network, by radio, or over the internet through entry points people run. Until you add one, rettui only reaches your local network.";
pub const NAME_HELP: &str = "The name sent in your announces, so others see who you are.";
pub const CONNECT_HELP: &str = "RMAP World runs a public transport node: your messages, pages and announces reach the wider network through it. Its operator sees your IP address, and anyone watching your connection can tell you use Reticulum.";
pub const DISCOVER_HELP: &str = "Connects to up to 2 entry points others announce, as Reticulum's manual recommends, and uses RMAP World only until they connect. You'll connect to hosts you didn't choose.";
pub const AUTO_PROPAGATION_HELP: &str = "A propagation node keeps messages for you while you're offline. Anyone can run one: the one picked (the nearest that answers fastest) sees who your messages are for and when you collect them, and could lose them; it can't read them.";
pub const APPLY_HELP: &str = "Saves your choices. If the Reticulum config changes, Reticulum restarts to connect.";
pub const LATER_HELP: &str = "Closes the guide; it's in the Status tab (g) whenever you want it.";

/// What the guide shows: what's set up, and how connected rettui is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideView {
    pub name: String,
    /// The Reticulum config already reaches the entry point, or finds
    /// entry points itself.
    pub has_entry_point: bool,
    pub has_discovery: bool,
    /// Another program runs the shared instance: its config has the
    /// interfaces, not this one.
    pub external: bool,
    pub auto_propagation: bool,
    pub interfaces_online: usize,
    /// Peers and nodes heard so far.
    pub heard: usize,
}

/// What the user chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideChoices {
    pub name: String,
    pub connect: bool,
    pub discover: bool,
    pub auto_propagation: bool,
}

/// The guide open in the terminal UI: the row picked, and the choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guide {
    pub row: usize,
    pub choices: GuideChoices,
}

/// The guide's rows in the terminal UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideRow {
    Name,
    Connect,
    Discover,
    AutoPropagation,
    Link(usize),
    Apply,
    Later,
}

impl Guide {
    pub fn rows(view: &GuideView) -> Vec<GuideRow> {
        let mut rows = vec![GuideRow::Name];
        if !view.external {
            rows.extend([GuideRow::Connect, GuideRow::Discover]);
        }
        rows.push(GuideRow::AutoPropagation);
        rows.extend((0..LINKS.len()).map(GuideRow::Link));
        rows.extend([GuideRow::Apply, GuideRow::Later]);
        rows
    }
}

impl App {
    /// The Reticulum config's text (or the default one, if there's none).
    fn guide_config(&self) -> String {
        rns::load(&self.rns_path()).map(|(text, _)| text).unwrap_or_default()
    }

    pub fn guide_view(&self) -> GuideView {
        let config = self.guide_config();
        GuideView {
            name: self.settings.display_name.clone(),
            has_entry_point: rns::has_interface_to(&config, ENTRY_HOST),
            has_discovery: rns::discovery_on(&config),
            external: self.uses_external_shared_instance(),
            auto_propagation: self.settings.auto_propagation_node,
            interfaces_online: self.interfaces.iter().filter(|i| i.online).count(),
            heard: self.store.peers.len(),
        }
    }

    /// The choices to start from: what's set up already, else connecting
    /// through the entry point.
    pub fn guide_defaults(&self) -> GuideChoices {
        let view = self.guide_view();
        GuideChoices { name: view.name, connect: true, discover: view.has_discovery, auto_propagation: view.auto_propagation }
    }

    /// Apply the choices: the name and settings now, the Reticulum config
    /// (then restarting Reticulum to use it) if it changes. `restricted`:
    /// from the web UI. What was done, to show.
    pub fn apply_guide(&mut self, choices: &GuideChoices, restricted: bool) -> Result<Vec<String>, String> {
        let mut done = Vec::new();
        let auto = if choices.auto_propagation { "true" } else { "false" };
        let mut changes = vec![("auto_propagation_node", auto)];
        let name = choices.name.trim();
        if !name.is_empty() {
            changes.push(("display_name", name));
        }
        done.extend(self.update_settings(&changes)?);
        let view = self.guide_view();
        if view.external && (choices.connect || choices.discover) {
            done.push(format!("{}: add entry points in that program's Reticulum config", rns::EXTERNAL_NOTE));
        } else {
            let add = choices.connect && !view.has_entry_point;
            let discover = choices.discover && !view.has_discovery;
            if add || discover {
                // With discovery, the entry point only bootstraps it.
                let warnings = self.rns_edit(restricted, |text| {
                    let text = if add {
                        rns::add_entry_point(text, ENTRY_NAME, ENTRY_HOST, ENTRY_PORT, choices.discover)?
                    } else {
                        text.to_string()
                    };
                    Ok(if discover { rns::enable_discovery(&text, DISCOVERED) } else { text })
                })?;
                done.extend(warnings);
                if add {
                    done.push(format!("Added {ENTRY_NAME} ({ENTRY_HOST}:{ENTRY_PORT}) to your Reticulum config"));
                }
                if discover {
                    done.push("Turned on interface discovery".into());
                }
                done.push("Restarting Reticulum to connect".into());
                self.request_rns_restart();
            }
        }
        self.finish_guide();
        Ok(done)
    }

    /// The guide was seen: it isn't shown at start again.
    pub fn finish_guide(&mut self) {
        self.guide = None;
        if self.settings.welcomed {
            return;
        }
        self.settings.welcomed = true;
        match self.saved_settings() {
            Ok(mut saved) => {
                saved.welcomed = true;
                if let Err(e) = saved.save(&self.paths.settings) {
                    self.log(format!("Could not save settings: {e:#}"));
                }
                self.settings_file = saved;
            }
            Err(e) => self.log(e),
        }
    }

    // ---- Terminal UI --------------------------------------------------------

    pub fn open_guide(&mut self) {
        self.guide = Some(Guide { row: 0, choices: self.guide_defaults() });
    }

    pub(super) fn guide_key(&mut self, key: KeyEvent) {
        let rows = Guide::rows(&self.guide_view());
        let Some(guide) = &mut self.guide else { return };
        match key.code {
            KeyCode::Esc => self.finish_guide(),
            KeyCode::Up | KeyCode::Char('k') => guide.row = guide.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => guide.row = (guide.row + 1).min(rows.len() - 1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(&row) = rows.get(guide.row) {
                    self.guide_activate(row);
                }
            }
            KeyCode::Char('y') => {
                if let Some(GuideRow::Link(i)) = rows.get(guide.row) {
                    self.copy(LINKS[*i].1, "the link");
                }
            }
            _ => {}
        }
    }

    pub(super) fn guide_activate(&mut self, row: GuideRow) {
        let Some(guide) = &mut self.guide else { return };
        let choices = &mut guide.choices;
        match row {
            GuideRow::Name => {
                let name = choices.name.clone();
                self.open_prompt(PromptKind::GuideName, "Your name, as others see it", &name);
            }
            GuideRow::Connect => choices.connect = !choices.connect,
            GuideRow::Discover => choices.discover = !choices.discover,
            GuideRow::AutoPropagation => choices.auto_propagation = !choices.auto_propagation,
            GuideRow::Link(i) => self.open_url(LINKS[i].1),
            GuideRow::Apply => {
                let choices = choices.clone();
                match self.apply_guide(&choices, false) {
                    Ok(done) if done.is_empty() => self.notify("All set"),
                    Ok(done) => self.notify(done.join("; ")),
                    Err(e) => self.fail(e),
                }
            }
            GuideRow::Later => {
                self.finish_guide();
                self.confirm("The guide is in the Status tab (g) whenever you want it");
            }
        }
    }

    pub(super) fn click_guide(&mut self, at: Position) {
        let Some(&(_, row)) = self.regions.guide_rows.iter().find(|(rect, _)| rect.contains(at)) else {
            // Outside the guide: nothing (it's closed with Esc or a button).
            return;
        };
        let rows = Guide::rows(&self.guide_view());
        if let (Some(guide), Some(index)) = (&mut self.guide, rows.iter().position(|r| *r == row)) {
            guide.row = index;
        }
        self.guide_activate(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use crate::store::Store;

    #[test]
    fn the_guide_connects_through_the_entry_point_and_is_shown_once() {
        let dir = std::env::temp_dir().join(format!("rettui-guide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rns_dir = dir.join("rns");
        let settings = Settings { welcomed: false, rns_config: Some(rns_dir.display().to_string()), ..Settings::default() };
        let mut app = crate::app::test_app(&dir, settings, Store::default());
        assert!(app.guide.is_some(), "shown the first time");
        let view = app.guide_view();
        assert!(!view.has_entry_point && !view.has_discovery && !view.external);
        assert_eq!(app.guide_defaults(), GuideChoices { name: "rettui user".into(), connect: true, discover: false, auto_propagation: false });
        // Applying: the name, the entry point in the Reticulum config, and a
        // restart to connect.
        let choices = GuideChoices { name: "Zev".into(), connect: true, discover: true, auto_propagation: false };
        let done = app.apply_guide(&choices, false).unwrap();
        assert!(done.iter().any(|d| d.contains("Added RMAP World")), "{done:?}");
        let config = std::fs::read_to_string(rns_dir.join("config")).unwrap();
        assert!(config.contains("target_host = rmap.world") && config.contains("bootstrap_only = Yes"), "{config}");
        assert!(rns::discovery_on(&config));
        assert!(app.take_rns_restart());
        let saved = app.saved_settings().unwrap();
        assert_eq!((saved.display_name.as_str(), saved.welcomed), ("Zev", true));
        assert!(app.guide.is_none());
        // Again: nothing more to add, and no restart.
        let view = app.guide_view();
        assert!(view.has_entry_point && view.has_discovery);
        let done = app.apply_guide(&choices, false).unwrap();
        assert!(!done.iter().any(|d| d.contains("Added")), "{done:?}");
        assert!(!app.take_rns_restart());
        // Existing settings files without the field don't show it again.
        let old: Settings = serde_json::from_str(r#"{"display_name": "Old hand"}"#).unwrap();
        assert!(old.welcomed);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_guide_in_the_terminal() {
        use crossterm::event::KeyModifiers;
        let dir = std::env::temp_dir().join(format!("rettui-guide-tui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings { rns_config: Some(dir.join("rns").display().to_string()), ..Settings::default() };
        let mut app = crate::app::test_app(&dir, settings, Store::default());
        assert!(app.guide.is_none(), "not shown to those who've seen it");
        app.tab = crate::app::Tab::Status;
        let press = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        press(&mut app, KeyCode::Char('g'));
        assert!(app.guide.is_some());
        // Down to the propagation node row, and on.
        for _ in 0..3 {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Char(' '));
        assert!(app.guide.as_ref().unwrap().choices.auto_propagation);
        // Changing the name goes through a prompt and comes back.
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        app.on_paste("Zev");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.guide.as_ref().unwrap().choices.name, "rettui userZev");
        press(&mut app, KeyCode::Esc);
        assert!(app.guide.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
