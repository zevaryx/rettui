//! The getting-started guide: shown the first time rettui starts, and from
//! the Status tab (`g`) or the web UI's Status page after that.
//!
//! Reticulum's default config only reaches the local network, so a new user
//! usually hears no one. The guide sets up what's needed to reach others,
//! following Reticulum's manual ("Getting Started Fast"): interface
//! discovery (entry points others announce, found over time), public entry
//! points when the config has nothing else to hear them through,
//! optionally an automatic propagation node; and links to where to learn
//! more. Nothing changes until the user applies it.
//!
//! Discovery hears of entry points through a connection the config already
//! has, so a fresh install (the Auto interface only, which reaches the
//! local network) gets the few entry points that answer fastest ticked; a
//! config with interfaces of its own doesn't, unless asked. Each entry
//! point is tried as the guide opens (see [`super::reach`]): one that
//! refuses or doesn't answer can't be ticked.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::reach::Reach;
use super::{App, PromptKind};
use crate::net::Hash;
use crate::reticulum as rns;

/// An entry point the guide offers: a public transport node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryPoint {
    /// Where it's listed.
    pub region: &'static str,
    /// Also the interface's name in the Reticulum config.
    pub name: &'static str,
    pub host: &'static str,
    pub port: u16,
}

/// The entry points offered: the ones Colorado Mesh's mesh-client
/// recommends (its default hubs), in its order and groups. Several, so
/// one being down doesn't leave a new user unconnected. Not its RNS Dublin
/// Mainnet (`dublin.connect.reticulum.network:4965`): that name no longer
/// exists, like its Amsterdam twin, which mesh-client counts as shut down.
pub const ENTRY_POINTS: &[EntryPoint] = &[
    EntryPoint { region: "Primary & global backbone", name: "RNS Between The Borders", host: "reticulum.betweentheborders.com", port: 4242 },
    EntryPoint { region: "Primary & global backbone", name: "RMAP World", host: "rmap.world", port: 4242 },
    EntryPoint { region: "Primary & global backbone", name: "RNS Simply Equipped", host: "rns.simplyequipped.com", port: 4242 },
    EntryPoint { region: "Primary & global backbone", name: "RNS Beleth", host: "rns.beleth.net", port: 4242 },
    EntryPoint { region: "North America", name: "MichMesh", host: "rns.michmesh.net", port: 7822 },
    EntryPoint { region: "Specialty", name: "Ratspeak & Colorado Mesh", host: "rns.ratspeak.org", port: 4242 },
];
/// Discovered entry points connected to at once, when discovery is on.
const DISCOVERED: u32 = 2;
/// How long the entry points get to connect before the guide says one
/// hasn't (a connection that fails says so within seconds; one that
/// times out, in about half a minute).
const CONNECT_WAIT: Duration = Duration::from_secs(45);

/// Where to learn more: title, address, and what's there when the title
/// doesn't say.
pub const LINKS: &[(&str, &str, Option<&str>)] = &[
    ("Reticulum: getting started", "https://reticulum.network/manual/gettingstartedfast.html", None),
    ("Understanding Reticulum", "https://reticulum.network/manual/understanding.html", None),
    ("Entry points on a map: RMAP World", "https://rmap.world", None),
    ("A directory of entry points", "https://directory.rns.recipes", None),
    ("Using a LoRa radio (RNode)", "https://github.com/zevaryx/rettui/wiki/RNode-Radios", Some(RNODE_NOTE)),
    ("Words you'll meet", "https://github.com/zevaryx/rettui/wiki/Glossary", Some(GLOSSARY_NOTE)),
    ("Using rettui", "https://github.com/zevaryx/rettui/wiki", None),
];
/// The glossary: what it is.
pub const GLOSSARY_NOTE: &str = "Identity, announce, hops, entry point, propagation node and the rest, a line or two each, with where Reticulum's manual explains them.";
/// LoRa radios: rettui doesn't flash them, the page says what does.
pub const RNODE_NOTE: &str = "LoRa radios reach others with no internet. Flashing one with RNode or microReticulum firmware (rnodeconf, or a web flasher), and adding it here.";

/// What each choice does, for both UIs.
pub const INTRO: &str = "Reticulum reaches others without servers: on your local network, by radio, or over the internet through entry points people run. Until you add one, rettui only reaches your local network.";
pub const NAME_HELP: &str = "The name sent in your announces, so others see who you are.";
pub const IDENTITY_HELP: &str = "Your identity is the key behind your address. Coming from Sideband, NomadNet or MeshChat? Use its identity file, and contacts reach you here at the address they know. Enter picks the file; it's used from the next start.";
/// In the web UI, which can't change the identity.
pub const IDENTITY_WEB_NOTE: &str = "Coming from Sideband, NomadNet or MeshChat? To keep your address, stop this web UI first (two rettuis on one data folder overwrite each other's files), then use that identity file from the terminal UI (i in its Status tab), or copy it over this one:";
pub const CONNECT_HELP: &str = "A public transport node: your traffic reaches the wider network through it, and its operator sees your IP address. When your config reaches only your local network, the 3 that answer fastest are ticked: discovery needs a connection to hear of others.";

/// How many entry points a fresh install is given to start with: the
/// ones that answer fastest.
pub const FASTEST: usize = 3;
/// None of the entry points answered.
pub const NONE_ANSWERED: &str = "None of the entry points answered: is this device online? A firewall may block them, too.";

/// One that didn't answer when tried, and so can't be ticked.
pub fn down_help(why: &str) -> String {
    format!("It didn't answer just now ({why}), so it can't be chosen; it's tried again when the guide opens later.")
}
/// The config is Python Reticulum's.
pub const SHARED_NOTE: &str = "This Reticulum config is shared with NomadNet, Sideband and rnsd too.";
pub const DISCOVER_HELP: &str = "Connects to up to 2 entry points others announce, as Reticulum's manual recommends; you'll connect to hosts you didn't choose. It hears of them through a connection you have, so it needs one to start.";
pub const AUTO_PROPAGATION_HELP: &str = "A propagation node keeps messages for you while you're offline, for you to collect. Ticked, rettui picks one: the nearest that answers fastest.";

/// Shown under picking a propagation node automatically while it's ticked
/// (it is to start with, the first time the guide opens).
pub const AUTO_PROPAGATION_WARNING: &str = "Warning: anyone can run a propagation node near you. The one picked sees who your messages are for and when you collect them, and could lose them; it can't read them. Where you can, pick one you trust in the Network tab.";
pub const APPLY_HELP: &str = "Saves your choices. If the Reticulum config changes, Reticulum restarts to connect.";
pub const LATER_HELP: &str = "Closes the guide; it's in the Status tab (g) whenever you want it.";

/// What the guide shows: what's set up, and how connected rettui is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideView {
    pub name: String,
    /// The LXMF address in use, and one to use from the next start.
    pub address: Option<Hash>,
    pub identity_pending: Option<Hash>,
    /// Which of [`ENTRY_POINTS`] the Reticulum config already reaches;
    /// and whether it finds entry points itself.
    pub has_entry_point: Vec<bool>,
    pub has_discovery: bool,
    /// The config has interfaces of its own besides the Auto one (which
    /// reaches only the local network).
    pub has_own_interfaces: bool,
    /// The config is Python Reticulum's, which NomadNet, Sideband and rnsd
    /// use too: changes to it are theirs.
    pub shared_config: bool,
    /// Another program runs the shared instance: its config has the
    /// interfaces, not this one.
    pub external: bool,
    pub auto_propagation: bool,
    /// Whether each of [`ENTRY_POINTS`] answered.
    pub reach: Vec<Reach>,
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
    /// For each of [`ENTRY_POINTS`]: connect through it.
    pub connect: Vec<bool>,
    pub discover: bool,
    pub auto_propagation: bool,
}

/// The guide open in the terminal UI: the row picked, the choices, and
/// the first of its lines shown (when they don't all fit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guide {
    pub row: usize,
    pub choices: GuideChoices,
    pub scroll: usize,
    /// An entry point was ticked or unticked: the ticks are the user's.
    /// Until then they follow [`picked`] as the tries come in.
    pub by_hand: bool,
}

/// The entry points to tick to start with. For a config with nothing
/// besides the Auto interface (a fresh install), the [`FASTEST`] that
/// answered quickest; while they're being tried, those still being tried
/// make up the number, in the list's order, so the ones ticked are always
/// the ones Apply adds. None for a config with interfaces of its own.
pub fn picked(view: &GuideView) -> Vec<bool> {
    let mut picked = vec![false; view.reach.len()];
    if view.has_own_interfaces {
        return picked;
    }
    let mut ranked: Vec<(usize, Option<Duration>)> = view
        .reach
        .iter()
        .enumerate()
        .filter_map(|(i, reach)| match *reach {
            Reach::Up(took) => Some((i, Some(took))),
            Reach::Checking => Some((i, None)),
            Reach::Down(_) => None,
        })
        .collect();
    ranked.sort_by_key(|&(i, took)| (took.is_none(), took, i));
    for (i, _) in ranked.into_iter().take(FASTEST) {
        picked[i] = true;
    }
    picked
}

/// The guide's rows in the terminal UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideRow {
    Name,
    Identity,
    /// One of [`ENTRY_POINTS`].
    Connect(usize),
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
            rows.extend((0..ENTRY_POINTS.len()).map(GuideRow::Connect));
            rows.push(GuideRow::Discover);
        }
        rows.push(GuideRow::AutoPropagation);
        rows.extend((0..LINKS.len()).map(GuideRow::Link));
        rows.extend([GuideRow::Apply, GuideRow::Later]);
        rows
    }

    /// The choices, with the entry points ticked as shown.
    pub fn shown(&self, view: &GuideView) -> GuideChoices {
        let mut choices = self.choices.clone();
        if !self.by_hand {
            choices.connect = picked(view);
        }
        choices
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
            has_entry_point: ENTRY_POINTS.iter().map(|entry| rns::has_interface_to(&config, entry.host)).collect(),
            has_own_interfaces: rns::has_own_interfaces(&config),
            shared_config: self.rns_path().parent().is_some_and(crate::config::is_shared_rns_dir),
            has_discovery: rns::discovery_on(&config),
            external: self.uses_external_shared_instance(),
            auto_propagation: self.settings.auto_propagation_node,
            reach: self.entry_reach.get(ENTRY_POINTS.len()),
            interfaces_online: self.interfaces.iter().filter(|i| i.online).count(),
            heard: self.store.peers.len(),
        }
    }

    /// The choices to start from: what's set up already; and for a config
    /// with nothing besides the Auto interface (a fresh install), discovery
    /// and the entry points that answer fastest to hear of others through
    /// (it needs a connection to start). A config with interfaces of its
    /// own (perhaps shared with other Reticulum programs) is changed only as
    /// asked. The first time it opens, picking a propagation node
    /// automatically too, unless one was picked by hand (with
    /// [`AUTO_PROPAGATION_WARNING`] under it); after that, it's as set.
    pub fn guide_defaults(&self) -> GuideChoices {
        let view = self.guide_view();
        let fresh = !view.has_own_interfaces;
        let connect = picked(&view);
        let auto_propagation = view.auto_propagation || (!self.settings.welcomed && self.settings.propagation_node.is_none());
        GuideChoices { name: view.name, connect, discover: fresh || view.has_discovery, auto_propagation }
    }

    /// Try the entry points, unless they were a moment ago (or another
    /// program's config has the interfaces).
    pub fn check_entry_points(&mut self) {
        if !self.uses_external_shared_instance() {
            let targets: Vec<(&'static str, u16)> = ENTRY_POINTS.iter().map(|entry| (entry.host, entry.port)).collect();
            self.entry_reach.check(&targets);
        }
    }

    /// Apply the choices: the name and settings now, the Reticulum config
    /// (then restarting Reticulum to use it) if it changes. An entry point
    /// that didn't answer isn't added. `restricted`: from the web UI. What
    /// was done, to show.
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
        let chosen = |i: usize| choices.connect.get(i).copied().unwrap_or(false) && !view.reach[i].is_down();
        if view.external && ((0..ENTRY_POINTS.len()).any(chosen) || choices.discover) {
            done.push(format!("{}: add entry points in that program's Reticulum config", rns::EXTERNAL_NOTE));
        } else {
            let add: Vec<&EntryPoint> =
                ENTRY_POINTS.iter().enumerate().filter(|&(i, _)| chosen(i) && !view.has_entry_point[i]).map(|(_, entry)| entry).collect();
            let discover = choices.discover && !view.has_discovery;
            if !add.is_empty() || discover {
                let mut added = Vec::new();
                let warnings = self.rns_edit(restricted, |text| {
                    let mut text = text.to_string();
                    for entry in &add {
                        let (with, name) = rns::add_entry_point(&text, entry.name, entry.host, entry.port)?;
                        added.push((name, **entry));
                        text = with;
                    }
                    Ok(if discover { rns::enable_discovery(&text, DISCOVERED) } else { text })
                })?;
                done.extend(warnings);
                for (_, entry) in &added {
                    done.push(format!("Added {} ({}:{}) to your Reticulum config", entry.name, entry.host, entry.port));
                }
                if !added.is_empty() {
                    // Said as each connects, or if one doesn't.
                    self.connect_watch = Some((added.into_iter().map(|(name, _)| name).collect(), Instant::now()));
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

    /// The first steps aren't shown any more (`x` in the Status tab, or
    /// Hide in the web UI).
    pub fn hide_first_steps(&mut self) {
        if self.settings.show_first_steps {
            self.save_flag(|settings| settings.show_first_steps = false);
        }
    }

    /// The first steps on the network, while some are left (none once all
    /// are taken, when they're hidden, or for installs from before them).
    pub fn first_steps(&self) -> Vec<FirstStep> {
        if !self.settings.show_first_steps {
            return Vec::new();
        }
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

    /// After the guide added entry points: say as each connects, or, for
    /// one that hasn't after a while, why (its latest trouble in the log,
    /// in plain words). Called as interfaces change, and on every tick.
    pub(super) fn watch_connection(&mut self) {
        let Some((names, since)) = &mut self.connect_watch else { return };
        let since = *since;
        let online: Vec<String> = names.iter().filter(|name| self.interfaces.iter().any(|i| &i.name == *name && i.online)).cloned().collect();
        names.retain(|name| !online.contains(name));
        let waiting = (since.elapsed() >= CONNECT_WAIT).then(|| std::mem::take(names));
        if names.is_empty() {
            self.connect_watch = None;
        }
        for name in online {
            self.notify(format!("Connected through {name}: peers and nodes appear in the Network tab as they announce"));
        }
        for name in waiting.unwrap_or_default() {
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
        self.check_entry_points();
        self.guide = Some(Guide { row: 0, choices: self.guide_defaults(), scroll: 0, by_hand: false });
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
        let view = self.guide_view();
        let down = match row {
            GuideRow::Connect(i) => view.reach[i].is_down(),
            _ => false,
        };
        let Some(guide) = &mut self.guide else { return };
        // Ticking or unticking an entry point keeps the ticks as they are.
        if matches!(row, GuideRow::Connect(_)) && !down && !guide.by_hand {
            guide.choices = guide.shown(&view);
            guide.by_hand = true;
        }
        let shown = guide.shown(&view);
        let choices = &mut guide.choices;
        match row {
            GuideRow::Name => {
                let name = choices.name.clone();
                self.open_prompt(PromptKind::GuideName, "Your name, as others see it", &name);
            }
            GuideRow::Identity => self.ask_identity_file(),
            GuideRow::Connect(i) => {
                if let Some(on) = choices.connect.get_mut(i).filter(|_| !down) {
                    *on = !*on;
                }
            }
            GuideRow::Discover => choices.discover = !choices.discover,
            GuideRow::AutoPropagation => choices.auto_propagation = !choices.auto_propagation,
            GuideRow::Link(i) => self.open_url(LINKS[i].1),
            GuideRow::Apply => {
                match self.apply_guide(&shown, false) {
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
    fn the_guide_connects_through_the_entry_points_and_is_shown_once() {
        let dir = std::env::temp_dir().join(format!("rettui-guide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rns_dir = dir.join("rns");
        let settings = Settings { welcomed: false, rns_config: Some(rns_dir.display().to_string()), ..Settings::default() };
        let mut app = crate::app::test_app(&dir, settings, Store::default());
        assert!(app.guide.is_some(), "shown the first time");
        let view = app.guide_view();
        assert!(view.has_entry_point.iter().all(|has| !has) && !view.has_discovery && !view.external);
        assert!(view.reach.iter().all(|reach| *reach == Reach::Checking), "tried as it opens");
        let all = vec![true; ENTRY_POINTS.len()];
        let ticked = |connect: &[bool]| connect.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect::<Vec<_>>();
        // A fresh install: discovery, and the 3 entry points that answer
        // fastest to hear of others through. While they're tried, the ones
        // still being tried in the list's order.
        let defaults = app.guide_defaults();
        assert_eq!((defaults.name.as_str(), defaults.discover, defaults.auto_propagation), ("rettui user", true, true));
        // Picking a propagation node automatically, unless one was picked
        // by hand.
        app.settings.propagation_node = Some("ab".repeat(16));
        assert!(!app.guide_defaults().auto_propagation);
        app.settings.propagation_node = None;
        let (borders, rmap, simply, beleth, mich, ratspeak) = (0, 1, 2, 3, 4, 5);
        assert_eq!(ENTRY_POINTS.len(), 6);
        assert_eq!((ENTRY_POINTS[rmap].name, ENTRY_POINTS[ratspeak].host), ("RMAP World", "rns.ratspeak.org"));
        assert_eq!(ticked(&defaults.connect), [borders, rmap, simply]);
        // Those that answered first, quickest first; one that didn't
        // answer isn't ticked, and isn't added even if asked for.
        let mut reach = vec![Reach::Checking; ENTRY_POINTS.len()];
        reach[borders] = Reach::Down("refused");
        reach[ratspeak] = Reach::Up(Duration::from_millis(90));
        app.entry_reach.set(reach.clone());
        assert_eq!(ticked(&app.guide_defaults().connect), [rmap, simply, ratspeak]);
        // All in: the 3 fastest.
        for (i, ms) in [(rmap, 120), (simply, 250), (beleth, 300), (mich, 600)] {
            reach[i] = Reach::Up(Duration::from_millis(ms));
        }
        app.entry_reach.set(reach.clone());
        assert_eq!(ticked(&app.guide_defaults().connect), [rmap, simply, ratspeak]);
        // In the terminal, the ticks follow the tries until one is ticked
        // or unticked; a row that didn't answer can't be.
        let mut reach = vec![Reach::Down("refused"); ENTRY_POINTS.len()];
        reach[rmap] = Reach::Checking;
        reach[ratspeak] = Reach::Checking;
        app.entry_reach.set(reach.clone());
        app.open_guide();
        let shown = |app: &App| ticked(&app.guide.as_ref().unwrap().shown(&app.guide_view()).connect);
        assert_eq!(shown(&app), [rmap, ratspeak]);
        app.guide_activate(GuideRow::Connect(borders));
        assert_eq!(shown(&app), [rmap, ratspeak], "not ticked");
        assert!(!app.guide.as_ref().unwrap().by_hand);
        reach[rmap] = Reach::Up(Duration::from_millis(120));
        reach[ratspeak] = Reach::Up(Duration::from_millis(90));
        reach[simply] = Reach::Up(Duration::from_millis(250));
        reach[beleth] = Reach::Up(Duration::from_millis(30));
        app.entry_reach.set(reach.clone());
        assert_eq!(shown(&app), [rmap, beleth, ratspeak], "as they answer");
        app.guide_activate(GuideRow::Connect(rmap));
        assert_eq!(shown(&app), [beleth, ratspeak]);
        reach[mich] = Reach::Up(Duration::from_millis(10));
        app.entry_reach.set(reach);
        assert_eq!(shown(&app), [beleth, ratspeak], "the user's from then on");
        let mut reach = vec![Reach::Down("refused"); ENTRY_POINTS.len()];
        reach[rmap] = Reach::Up(Duration::from_millis(120));
        reach[ratspeak] = Reach::Up(Duration::from_millis(90));
        app.entry_reach.set(reach);
        // Applying: the name, the entry points in the Reticulum config, and
        // a restart to connect.
        let choices = GuideChoices { name: "Zev".into(), connect: all, discover: true, auto_propagation: false };
        let done = app.apply_guide(&choices, false).unwrap();
        assert!(done.iter().any(|d| d.contains("Added RMAP World (rmap.world:4242)")), "{done:?}");
        assert!(done.iter().any(|d| d.contains("Added Ratspeak & Colorado Mesh (rns.ratspeak.org:4242)")), "{done:?}");
        assert_eq!(done.iter().filter(|d| d.starts_with("Added")).count(), 2, "{done:?}");
        let config = std::fs::read_to_string(rns_dir.join("config")).unwrap();
        assert!(config.contains("target_host = rmap.world") && config.contains("target_host = rns.ratspeak.org"), "{config}");
        assert!(config.contains("[[Ratspeak & Colorado Mesh]]"), "{config}");
        assert!(!config.contains(ENTRY_POINTS[borders].host) && !config.contains(ENTRY_POINTS[simply].host), "{config}");
        assert!(!config.contains("bootstrap_only"), "{config}");
        assert!(rns::discovery_on(&config));
        assert!(app.take_rns_restart());
        // Once Reticulum is back, it says as each connects; and, for one
        // that doesn't, why.
        let iface = |name: &str, online| crate::net::InterfaceInfo { name: name.into(), online, rx_bytes: 0, tx_bytes: 0, ..Default::default() };
        let ratspeak = ENTRY_POINTS[ratspeak].name;
        app.on_net(crate::net::NetEvent::Interfaces(vec![iface("RMAP World", false), iface(ratspeak, false)]));
        assert_eq!(app.connect_watch.as_ref().map(|(names, _)| names.len()), Some(2));
        app.on_net(crate::net::NetEvent::Interfaces(vec![iface("RMAP World", true), iface(ratspeak, false)]));
        assert!(app.notice.as_ref().unwrap().text.starts_with("Connected through RMAP World"));
        assert_eq!(app.connect_watch.as_ref().map(|(names, _)| names.clone()), Some(vec![ratspeak.to_string()]));
        app.connect_watch.as_mut().unwrap().1 = Instant::now() - CONNECT_WAIT;
        app.on_net(crate::net::NetEvent::Log(format!("Interface {ratspeak}: TCP connect failed: Connection refused (os error 111)")));
        app.on_tick();
        let notice = app.notice.as_ref().unwrap();
        assert!(notice.text.starts_with("Couldn't reach Ratspeak & Colorado Mesh yet") && notice.text.contains("Nothing accepted"), "{}", notice.text);
        assert!(app.connect_watch.is_none());
        let saved = app.saved_settings().unwrap();
        assert_eq!((saved.display_name.as_str(), saved.welcomed), ("Zev", true));
        assert!(app.guide.is_none());
        // Again: nothing more to add, and no restart.
        let view = app.guide_view();
        assert_eq!(view.has_entry_point.iter().filter(|&&has| has).count(), 2);
        assert!(view.has_discovery);
        let done = app.apply_guide(&choices, false).unwrap();
        assert!(!done.iter().any(|d| d.contains("Added")), "{done:?}");
        assert!(!app.take_rns_restart());
        // After the first time, picking a propagation node is as it's set.
        assert!(!app.guide_defaults().auto_propagation);
        // Existing settings files without the field don't show it again.
        let old: Settings = serde_json::from_str(r#"{"display_name": "Old hand"}"#).unwrap();
        assert!(old.welcomed);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_config_with_its_own_interfaces_is_changed_only_as_asked() {
        let dir = std::env::temp_dir().join(format!("rettui-guide-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rns_dir = dir.join("rns");
        std::fs::create_dir_all(&rns_dir).unwrap();
        let config = "[reticulum]\n  share_instance = Yes\n\n[interfaces]\n  [[Default Interface]]\n    type = AutoInterface\n    enabled = Yes\n  [[My hub]]\n    type = TCPClientInterface\n    enabled = Yes\n    target_host = hub.example\n    target_port = 4242\n  [[RMAP World]]\n    type = TCPClientInterface\n    enabled = No\n    target_host = rmap.world\n    target_port = 4242\n";
        std::fs::write(rns_dir.join("config"), config).unwrap();
        let settings = Settings { rns_config: Some(rns_dir.display().to_string()), ..Settings::default() };
        let app = crate::app::test_app(&dir, settings, Store::default());
        let view = app.guide_view();
        // RMAP World is there, but off: not counted as connecting through it.
        assert!(view.has_own_interfaces && view.has_entry_point.iter().all(|has| !has));
        let defaults = app.guide_defaults();
        assert!(defaults.connect.iter().all(|on| !on) && !defaults.discover, "{defaults:?}");
        assert!(!view.shared_config, "a config of rettui's own");
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
        // Installs from before the first steps don't see them.
        let app = crate::app::test_app(&dir, Settings::default(), Store::default());
        assert!(app.first_steps().is_empty());
        let old: Settings = serde_json::from_str(r#"{"display_name": "Old hand"}"#).unwrap();
        assert!(!old.show_first_steps);
        // A new install does.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fresh = Settings::load(&dir.join("fresh-settings.json")).unwrap();
        assert!(fresh.show_first_steps && !fresh.welcomed);
        let mut app = crate::app::test_app(&dir, Settings { show_first_steps: true, ..Settings::default() }, Store::default());
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
        // Or hidden before that (x in the Status tab), for good.
        app.store.conversations.clear();
        assert!(draw(&mut app).contains("First steps"));
        app.on_key(crossterm::event::KeyEvent::new(KeyCode::Char('x'), crossterm::event::KeyModifiers::NONE));
        assert!(app.first_steps().is_empty() && !app.saved_settings().unwrap().show_first_steps);
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
        // Down to the propagation node row (past both entry points), and on.
        let rows = 2 + ENTRY_POINTS.len() + 1;
        for _ in 0..rows {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Char(' '));
        assert!(app.guide.as_ref().unwrap().choices.auto_propagation);
        // Changing the name goes through a prompt and comes back.
        for _ in 0..rows {
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
