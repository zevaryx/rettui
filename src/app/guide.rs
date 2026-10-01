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

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::{App, PromptKind};
use crate::net::Hash;
use crate::reticulum as rns;

/// The community entry point offered: RMAP World's transport node.
pub const ENTRY_NAME: &str = "RMAP World";
pub const ENTRY_HOST: &str = "rmap.world";
pub const ENTRY_PORT: u16 = 4242;
/// Discovered entry points connected to at once, when discovery is on.
const DISCOVERED: u32 = 2;
/// How long the entry point gets to connect before the guide says it
/// hasn't (a connection that fails says so within seconds; one that
/// times out, in about half a minute).
const CONNECT_WAIT: Duration = Duration::from_secs(45);

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
pub const IDENTITY_HELP: &str = "Your identity is the key behind your address. Coming from Sideband, NomadNet or MeshChat? Use its identity file, and contacts reach you here at the address they know. Enter picks the file; it's used from the next start.";
/// In the web UI, which can't change the identity.
pub const IDENTITY_WEB_NOTE: &str = "Coming from Sideband, NomadNet or MeshChat? To keep your address, use that identity file from the terminal UI (i in its Status tab), or copy it over this file while rettui is stopped:";
pub const CONNECT_HELP: &str = "RMAP World runs a public transport node: your messages, pages and announces reach the wider network through it. Its operator sees your IP address, and anyone watching your connection can tell you use Reticulum.";
pub const DISCOVER_HELP: &str = "Connects to up to 2 entry points others announce, as Reticulum's manual recommends, and uses RMAP World only until they connect. You'll connect to hosts you didn't choose.";
pub const AUTO_PROPAGATION_HELP: &str = "A propagation node keeps messages for you while you're offline. Anyone can run one: the one picked (the nearest that answers fastest) sees who your messages are for and when you collect them, and could lose them; it can't read them.";
pub const APPLY_HELP: &str = "Saves your choices. If the Reticulum config changes, Reticulum restarts to connect.";
pub const LATER_HELP: &str = "Closes the guide; it's in the Status tab (g) whenever you want it.";

/// What the guide shows: what's set up, and how connected rettui is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideView {
    pub name: String,
    /// The LXMF address in use, and one to use from the next start.
    pub address: Option<Hash>,
    pub identity_pending: Option<Hash>,
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

/// A first step on the network, and whether it's taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstStep {
    pub label: &'static str,
    pub done: bool,
    /// How to take it, in the terminal UI and in the web UI.
    pub how: &'static str,
    pub how_web: &'static str,
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
    Identity,
    Connect,
    Discover,
    AutoPropagation,
    Link(usize),
    Apply,
    Later,
}

impl Guide {
    pub fn rows(view: &GuideView) -> Vec<GuideRow> {
        let mut rows = vec![GuideRow::Name, GuideRow::Identity];
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
            address: self.lxmf_hash,
            identity_pending: self.identity_pending,
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
                let mut added = None;
                let warnings = self.rns_edit(restricted, |text| {
                    let text = if add {
                        let (text, name) = rns::add_entry_point(text, ENTRY_NAME, ENTRY_HOST, ENTRY_PORT, choices.discover)?;
                        added = Some(name);
                        text
                    } else {
                        text.to_string()
                    };
                    Ok(if discover { rns::enable_discovery(&text, DISCOVERED) } else { text })
                })?;
                done.extend(warnings);
                if let Some(name) = added {
                    done.push(format!("Added {ENTRY_NAME} ({ENTRY_HOST}:{ENTRY_PORT}) to your Reticulum config"));
                    // Said once it connects, or if it doesn't.
                    self.connect_watch = Some((name, Instant::now()));
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
        if !self.settings.welcomed {
            self.save_flag(|settings| settings.welcomed = true);
        }
    }

    /// A copy of the identity was saved.
    pub fn identity_backed_up(&mut self) {
        if !self.settings.identity_backed_up {
            self.save_flag(|settings| settings.identity_backed_up = true);
        }
    }

    /// Set something in the settings the user doesn't edit, and save it
    /// (with the file's other values as they are).
    fn save_flag(&mut self, set: impl Fn(&mut crate::config::Settings)) {
        set(&mut self.settings);
        match self.saved_settings() {
            Ok(mut saved) => {
                set(&mut saved);
                if let Err(e) = saved.save(&self.paths.settings) {
                    self.log(format!("Could not save settings: {e:#}"));
                }
                self.settings_file = saved;
            }
            Err(e) => self.log(e),
        }
    }

    /// The first steps on the network, while some are left (none once all
    /// are taken).
    pub fn first_steps(&self) -> Vec<FirstStep> {
        let sent = self.store.conversations.values().any(|c| c.messages.iter().any(|m| !m.incoming));
        let steps = vec![
            FirstStep {
                label: "Hear from others",
                done: !self.store.peers.is_empty(),
                how: "g, then wait",
                how_web: "connect through an entry point (Getting started, above); announces then arrive over hours",
            },
            FirstStep { label: "Announce yourself", done: self.announced, how: "A", how_web: "Announce, above" },
            FirstStep {
                label: "Choose a propagation node",
                done: self.settings.propagation_node.is_some() || self.settings.auto_propagation_node,
                how: "p in the Network tab, or g",
                how_web: "in the Network section, or let Getting started pick",
            },
            FirstStep {
                label: "Back up your identity",
                done: self.settings.identity_backed_up,
                how: "b",
                how_web: "copy the identity file in the Data folder (below) somewhere safe, or use b in the terminal UI",
            },
            FirstStep { label: "Send a message", done: sent, how: "n in Messages", how_web: "+ New in Messages" },
        ];
        if steps.iter().all(|step| step.done) { Vec::new() } else { steps }
    }

    /// After the guide added the entry point: say when it connects, or, if
    /// it hasn't after a while, why (its latest trouble in the log, in
    /// plain words). Called as interfaces change, and on every tick.
    pub(super) fn watch_connection(&mut self) {
        let Some((name, since)) = &self.connect_watch else { return };
        if self.interfaces.iter().any(|i| &i.name == name && i.online) {
            let name = name.clone();
            self.connect_watch = None;
            self.notify(format!("Connected through {name}: peers and nodes appear in the Network tab as they announce"));
        } else if since.elapsed() >= CONNECT_WAIT {
            let name = name.clone();
            self.connect_watch = None;
            let about = format!("Interface {name}:");
            let why = self
                .log
                .iter()
                .rev()
                .find(|line| line.contains(&about))
                .and_then(|line| crate::net::iface_log::hint(line))
                .unwrap_or("The log in the Status tab says why");
            self.warn(format!("Couldn't reach {name} yet (it keeps trying). {why}"));
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
            GuideRow::Identity => self.ask_identity_file(),
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
        // Once Reticulum is back, it says when the entry point connects.
        let iface = |online| crate::net::InterfaceInfo { name: ENTRY_NAME.into(), online, rx_bytes: 0, tx_bytes: 0 };
        app.on_net(crate::net::NetEvent::Interfaces(vec![iface(false)]));
        assert!(app.connect_watch.is_some());
        app.on_net(crate::net::NetEvent::Interfaces(vec![iface(true)]));
        assert!(app.connect_watch.is_none());
        assert!(app.notice.as_ref().unwrap().text.starts_with("Connected through RMAP World"));
        // Or, if it doesn't, why.
        app.interfaces = vec![iface(false)];
        app.connect_watch = Some((ENTRY_NAME.into(), Instant::now() - CONNECT_WAIT));
        app.on_net(crate::net::NetEvent::Log("Interface RMAP World: TCP connect failed: Connection refused (os error 111)".into()));
        app.on_tick();
        let notice = app.notice.as_ref().unwrap();
        assert!(notice.text.starts_with("Couldn't reach RMAP World yet") && notice.text.contains("Nothing accepted"), "{}", notice.text);
        assert!(app.connect_watch.is_none());
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
    fn first_steps_until_all_are_taken() {
        use crate::net::PeerKind;
        use crate::store::{Conversation, Message, Peer};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("rettui-steps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        let done = |app: &App| app.first_steps().iter().map(|step| step.done).collect::<Vec<_>>();
        assert_eq!(done(&app), [false; 5]);
        let draw = |app: &mut App| {
            let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
            terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
            let buffer = terminal.backend().buffer();
            (0..30).map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect::<String>()
        };
        app.tab = crate::app::Tab::Status;
        let screen = draw(&mut app);
        assert!(screen.contains("○○○○○  next: Hear from others (g, then wait)"), "{screen}");
        // Each, as it's taken.
        app.store.peers.insert("ab".repeat(16), Peer { kind: PeerKind::Lxmf, name: None, hops: 2, last_seen: 0 });
        app.on_net(crate::net::NetEvent::Announced);
        app.settings.auto_propagation_node = true;
        assert_eq!(done(&app), [true, true, true, false, false]);
        assert!(draw(&mut app).contains("✓✓✓○○  next: Back up your identity (b)"));
        app.identity_backed_up();
        assert!(app.saved_settings().unwrap().identity_backed_up, "kept for next time");
        let sent = Message { id: "local-1".into(), incoming: false, ..Message::default() };
        app.store.conversations.insert("cd".repeat(16), Conversation { messages: vec![sent], ..Default::default() });
        // All taken: gone.
        assert!(app.first_steps().is_empty());
        assert!(!draw(&mut app).contains("First steps"));
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
        for _ in 0..4 {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Char(' '));
        assert!(app.guide.as_ref().unwrap().choices.auto_propagation);
        // Changing the name goes through a prompt and comes back.
        for _ in 0..4 {
            press(&mut app, KeyCode::Up);
        }
        press(&mut app, KeyCode::Enter);
        app.on_paste("Zev");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.guide.as_ref().unwrap().choices.name, "rettui userZev");
        // The identity row asks for a file; the guide stays.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.prompt.as_ref().map(|p| &p.kind), Some(PromptKind::ImportIdentity)));
        press(&mut app, KeyCode::Esc);
        assert!(app.prompt.is_none() && app.guide.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.guide.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
