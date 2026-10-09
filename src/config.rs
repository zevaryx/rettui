//! Settings, on-disk locations and the local identity.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rns_identity::identity::Identity;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Name sent in LXMF announces.
    pub display_name: String,
    /// Your icon (a Material Design Icon's name), sent with your messages
    /// for Sideband, Columba and MeshChat to show; none when unset.
    pub icon: Option<String>,
    /// Its colour and the colour behind it, as `#rrggbb`.
    pub icon_color: String,
    pub icon_background: String,
    pub announce_at_start: bool,
    /// How the LXMF address is announced on its own: one of
    /// [`ANNOUNCE_SCHEDULES`] (see [`Settings::announce_schedule`]).
    pub announce_schedule: String,
    /// With the fixed schedule: minutes between announces.
    pub announce_interval_mins: u64,
    /// With the random one: the shortest and longest time between them.
    pub announce_random_min_mins: u64,
    pub announce_random_max_mins: u64,
    /// Reticulum config directory; `None` uses the standard location
    /// (and joins a running shared instance such as rnsd or NomadNet).
    pub rns_config: Option<String>,
    /// Page opened by the browser's "home" key, e.g. `<hash>:/page/index.mu`.
    pub home: Option<String>,
    /// Outbound propagation node (hex destination hash).
    pub propagation_node: Option<String>,
    /// Minutes between automatic propagation node syncs; 0 disables them.
    pub sync_interval_mins: u64,
    /// Pick the propagation node automatically: the nearest that answers
    /// fastest (see `net::autopn`). Off unless turned on.
    pub auto_propagation_node: bool,
    /// Newest messages each conversation keeps in the store; older ones go
    /// to the archive. 0 keeps them all in the store.
    pub messages_kept: u64,
    /// Megabytes the store and the archive may use together; the oldest
    /// archived months are deleted to stay within it. 0 is no limit.
    pub message_storage_mb: u64,
    /// How long NomadNet pages and images stay cached (pages can ask for
    /// less with `#!c=`).
    pub cache_hours: u64,
    /// Host a NomadNet node with the pages in `node_dir`.
    pub node_enabled: bool,
    /// Name the node announces; the display name when unset.
    pub node_name: Option<String>,
    /// Minutes between node announces; 0 announces only at start.
    pub node_announce_interval_mins: u64,
    /// Folder with the node's `pages/` and `files/`; `node` in the data
    /// directory when unset.
    pub node_dir: Option<String>,
    /// Run executable pages as scripts (NomadNet dynamic pages).
    pub node_executable_pages: bool,
    /// Wrap long lines in the text editors instead of scrolling sideways.
    pub wrap_lines: bool,
    /// Show people joining and leaving RRC rooms in the chat.
    pub show_joins: bool,
    /// Notifications for new LXMF messages (each conversation can be muted).
    pub notify_messages: bool,
    /// Notifications from RRC (each hub and room has its own level).
    pub notify_rrc: bool,
    /// No notifications between these times of day (`22:00-07:00`); none
    /// when unset (see `app::notify::QuietHours`).
    pub quiet_hours: Option<String>,
    /// Trusted contacts' messages notify during quiet hours anyway.
    pub quiet_hours_trusted: bool,
    /// What becomes of messages from senders who aren't contacts (not
    /// trusted, nor left as they are, nor ever written to): one of
    /// [`UNKNOWN_SENDERS`]. Settings files from before it said
    /// `ignore_unknown_senders`, which is read as `ignore`.
    pub unknown_senders: String,
    /// Send failed messages (text ones) again when their recipient
    /// announces, as MeshChat does.
    pub resend_on_announce: bool,
    /// Answer Sideband's ping, echo and signal report commands: one of
    /// [`ANSWER_COMMANDS`] (whose commands are answered).
    pub answer_commands: String,
    /// Where this station is, as `latitude, longitude`: shared from the
    /// terminal, and sent to answer location requests if they're answered.
    pub location: Option<String>,
    /// Answer location requests (Sideband's telemetry requests) with
    /// [`Settings::location`]: one of [`ANSWER_COMMANDS`].
    pub location_requests: String,
    /// Where the web UI's map gets its tiles (`{z}`, `{x}` and `{y}` in
    /// it), fetched and kept by rettui; none draws the map without them.
    pub map_tiles: Option<String>,
    /// An MBTiles file the web UI's map takes its tiles from first (any it
    /// hasn't come from [`Settings::map_tiles`]).
    pub map_tiles_file: Option<String>,
    /// Proof-of-work stamp cost asked of senders who aren't contacts
    /// (announced); 0 asks none.
    pub stamp_cost: u64,
    /// Largest message taken, in kilobytes; 0 takes any size.
    pub max_message_kb: u64,
    /// The getting-started guide was seen (it's shown at the first start).
    /// Settings files from before it count as seen.
    pub welcomed: bool,
    /// A copy of the identity was saved (`b` in the Status tab, or the web
    /// UI was told one was): the first steps stop asking for one.
    pub identity_backed_up: bool,
    /// Show the first steps in the Status tab (until they're taken, or
    /// hidden). New installs only: settings files from before them don't.
    pub show_first_steps: bool,
    /// Mark the messages you write as Markdown (LXMF's renderer field), so
    /// clients that format it show them formatted.
    pub markdown_messages: bool,
    /// How big pictures you send may be: one of
    /// [`crate::app::shrink::PICTURE_SIZES`] (smaller ones are made first).
    pub picture_size: String,
    /// How much goes to `rettui.log`: one of [`crate::logging::LEVELS`].
    pub log_level: String,
    /// Ask GitHub once a day whether there's a newer release (see
    /// `crate::update`). Off unless turned on (the getting-started guide
    /// recommends it).
    pub update_check: bool,
    /// How times and dates read: one of [`crate::clock::CLOCKS`], and of
    /// [`crate::clock::DATE_STYLES`].
    pub clock: String,
    pub date_style: String,
    /// The terminal UI's colours: one of [`crate::ui::THEMES`].
    pub tui_theme: String,
    /// Host an LXMF propagation node (messages kept in `propagation/`).
    pub pn_enabled: bool,
    /// Name the propagation node announces; the display name when unset.
    pub pn_name: Option<String>,
    /// Propagation stamp cost asked of senders (LXMF's least is 13).
    pub pn_stamp_cost: u64,
    /// Megabytes of messages the propagation node keeps.
    pub pn_storage_mb: u64,
    /// Largest message a client may send it, in kilobytes.
    pub pn_transfer_kb: u64,
    /// Host an RRC hub (rsRRCD's, in `rrc-hub/`).
    pub hub_enabled: bool,
    /// Name the hub announces and welcomes people with; the display name
    /// when unset.
    pub hub_name: Option<String>,
    /// Sent to everyone who connects (`\n` starts a new line).
    pub hub_greeting: Option<String>,
    pub hub_announce_interval_mins: u64,
    /// Anyone may make a room by joining it; else only you can.
    pub hub_open_rooms: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display_name: "rettui user".to_string(),
            icon: None,
            // Dark on rettui's own colour, as its buttons are.
            icon_color: "#0b0b10".into(),
            icon_background: "#4fd6e0".into(),
            announce_at_start: true,
            announce_schedule: "random".into(),
            announce_interval_mins: 360,
            announce_random_min_mins: ANNOUNCE_MINS.0,
            announce_random_max_mins: ANNOUNCE_MINS.1,
            rns_config: None,
            home: None,
            propagation_node: None,
            sync_interval_mins: 120,
            auto_propagation_node: false,
            messages_kept: 1000,
            message_storage_mb: 0,
            cache_hours: 24,
            node_enabled: false,
            node_name: None,
            node_announce_interval_mins: 360,
            node_dir: None,
            node_executable_pages: false,
            show_joins: true,
            wrap_lines: false,
            notify_messages: true,
            notify_rrc: true,
            quiet_hours: None,
            quiet_hours_trusted: true,
            unknown_senders: "show".into(),
            resend_on_announce: true,
            answer_commands: "off".into(),
            location: None,
            location_requests: "off".into(),
            map_tiles: Some(OSM_TILES.into()),
            map_tiles_file: None,
            stamp_cost: 0,
            // LXMF's own default delivery limit (NomadNet's is 500).
            max_message_kb: 1000,
            welcomed: true,
            identity_backed_up: false,
            show_first_steps: false,
            markdown_messages: true,
            picture_size: "medium".into(),
            log_level: "warn".into(),
            update_check: false,
            clock: "24-hour".into(),
            tui_theme: "dark".into(),
            date_style: "month-day".into(),
            pn_enabled: false,
            pn_name: None,
            // lxmd's defaults.
            pn_stamp_cost: u64::from(lxmf_core::constants::PROPAGATION_COST),
            pn_storage_mb: 500,
            pn_transfer_kb: lxmf_core::constants::PROPAGATION_LIMIT as u64,
            hub_enabled: false,
            hub_name: None,
            hub_greeting: None,
            hub_announce_interval_mins: 360,
            hub_open_rooms: true,
        }
    }
}

#[derive(Clone)]
pub struct Paths {
    /// The data directory itself.
    pub base: PathBuf,
    pub settings: PathBuf,
    pub identity: PathBuf,
    pub store: PathBuf,
    pub log: PathBuf,
    pub downloads: PathBuf,
    pub known_identities: PathBuf,
    /// Stamp tickets given to contacts and received from them.
    pub tickets: PathBuf,
    /// Ratchet keys for the LXMF address (see [`crate::lxmf::ratchets`]).
    pub ratchets: PathBuf,
    pub cache: PathBuf,
    pub rrc_history: PathBuf,
    /// Messages older than the newest each conversation keeps.
    pub archive: PathBuf,
    /// Files attached to messages sent from the web UI.
    pub uploads: PathBuf,
    /// Secret for the web UI's login link.
    pub web_token: PathBuf,
    /// rettui's own HTTPS certificates (`--https`).
    pub web_tls: PathBuf,
    /// The web UI's key for Web Push, and the browsers subscribed.
    pub web_push_key: PathBuf,
    pub web_push: PathBuf,
    /// Default folder for a hosted node's pages and files.
    pub node: PathBuf,
    /// Messages kept by the hosted propagation node.
    pub propagation: PathBuf,
    /// The hosted RRC hub's configuration and rooms (rsRRCD's files).
    pub rrc_hub: PathBuf,
    /// The hosted hub's identity (backed up with yours).
    pub rrc_hub_identity: PathBuf,
    /// Map tiles fetched for the web UI's map.
    pub map_tiles: PathBuf,
    /// What the last update check found (see `crate::update`).
    pub update_check: PathBuf,
}

impl Paths {
    pub fn new(base: Option<PathBuf>) -> Result<Self> {
        let base = match base {
            Some(base) => base,
            None => {
                directories::ProjectDirs::from("", "", "rettui").context("could not determine a data directory")?.data_dir().to_path_buf()
            }
        };
        fs::create_dir_all(&base).with_context(|| format!("creating {}", base.display()))?;
        Ok(Self {
            settings: base.join("settings.json"),
            identity: base.join("identity"),
            store: base.join("store.json.gz"),
            log: base.join("rettui.log"),
            downloads: base.join("downloads"),
            known_identities: base.join("known_identities.json"),
            tickets: base.join("tickets.json"),
            ratchets: base.join("ratchets"),
            cache: base.join("cache"),
            rrc_history: base.join("rrc"),
            archive: base.join("archive"),
            uploads: base.join("uploads"),
            web_token: base.join("web_token"),
            web_tls: base.join("web-tls"),
            web_push_key: base.join("web_push_key"),
            web_push: base.join("web_push.json"),
            node: base.join("node"),
            propagation: base.join("propagation"),
            rrc_hub: base.join("rrc-hub"),
            rrc_hub_identity: base.join("rrc-hub-identity"),
            map_tiles: base.join("map-tiles"),
            update_check: base.join("update-check.json"),
            base,
        })
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            // A new install: the getting-started guide is shown, and the
            // first steps.
            let settings = Self { welcomed: false, show_first_steps: true, ..Self::default() };
            settings.save(path)?;
            return Ok(settings);
        }
        let text = fs::read_to_string(path)?;
        let value: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let schedule_set = value.get("announce_schedule").is_some();
        // From before message requests: ignoring unknown senders was a
        // toggle.
        let ignored = value.get("unknown_senders").is_none()
            && value.get("ignore_unknown_senders").and_then(serde_json::Value::as_bool) == Some(true);
        let mut settings: Self = serde_json::from_value(value).with_context(|| format!("parsing {}", path.display()))?;
        // From before announcing on a random schedule: 0 minutes was off,
        // and an interval of one's own stays (6 hours, the default then,
        // gives way to the random schedule). Syncing every 30 minutes, the
        // default then too, gives way to every two hours.
        if !schedule_set {
            if settings.sync_interval_mins == 30 {
                settings.sync_interval_mins = 120;
            }
            match settings.announce_interval_mins {
                0 => {
                    settings.announce_schedule = "off".into();
                    settings.announce_interval_mins = ANNOUNCE_MINS.1;
                }
                360 => {}
                own => {
                    settings.announce_schedule = "fixed".into();
                    settings.announce_interval_mins = own.clamp(ANNOUNCE_MINS.0, ANNOUNCE_MINS.1);
                }
            }
        }
        if ignored {
            settings.unknown_senders = "ignore".into();
        }
        Ok(settings)
    }

    /// Where this station is, if that's set.
    pub fn own_location(&self) -> Option<crate::lxmf::Location> {
        self.location.as_deref().and_then(crate::lxmf::Location::parse)
    }

    /// Whether messages from unknown senders are kept aside as requests.
    pub fn requests(&self) -> bool {
        self.unknown_senders == "requests"
    }

    /// When the LXMF address is announced on its own, within
    /// [`ANNOUNCE_MINS`] whatever settings.json says.
    pub fn announce_schedule(&self) -> crate::net::AnnounceSchedule {
        use crate::net::AnnounceSchedule;
        let minutes = |m: u64| std::time::Duration::from_secs(m.clamp(ANNOUNCE_MINS.0, ANNOUNCE_MINS.1) * 60);
        match self.announce_schedule.as_str() {
            "off" => AnnounceSchedule::Off,
            "fixed" => AnnounceSchedule::Every(minutes(self.announce_interval_mins)),
            _ => {
                let (low, high) = (minutes(self.announce_random_min_mins), minutes(self.announce_random_max_mins));
                AnnounceSchedule::Between(low.min(high), low.max(high))
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())
    }
}

/// How a setting is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    /// Text that may be left empty (unset).
    Optional,
    Toggle,
    /// A whole number; 0 turns the feature off where noted.
    Number,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// A colour, as `#rrggbb`.
    Color,
}

/// When a change to a setting takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Now,
    NextStart,
}

/// One entry of `settings.json`, as the settings editors show it.
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: FieldKind,
    pub effect: Effect,
}

/// What the web UI may do with a setting. The web login must not be able to
/// run programs on this computer (as with pipe interface commands and page
/// scripts), so settings that decide what runs are changed in the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebAccess {
    Change,
    /// A toggle it may turn off, but not on.
    TurnOffOnly,
    TerminalOnly,
}

impl Field {
    pub fn web_access(&self) -> WebAccess {
        match self.key {
            // Another config directory could hold pipe interfaces, whose
            // commands Reticulum runs; another node folder could hold
            // executable pages, or share files that aren't meant to be.
            "rns_config" | "node_dir" | "map_tiles_file" => WebAccess::TerminalOnly,
            "node_executable_pages" => WebAccess::TurnOffOnly,
            _ => WebAccess::Change,
        }
    }
}

/// The groups the settings editors show settings in, one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Profile,
    Messages,
    Location,
    Browsing,
    Hosting,
    Display,
    Notifications,
    System,
}

impl Section {
    pub const ALL: [Section; 8] = [
        Section::Profile,
        Section::Messages,
        Section::Location,
        Section::Browsing,
        Section::Hosting,
        Section::Display,
        Section::Notifications,
        Section::System,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Profile => "Profile",
            Section::Messages => "Messages",
            Section::Location => "Location",
            Section::Browsing => "Browsing",
            Section::Hosting => "Hosting",
            Section::Display => "Display",
            Section::Notifications => "Notifications",
            Section::System => "System",
        }
    }

    /// For a narrow terminal.
    pub fn short(self) -> &'static str {
        match self {
            Section::Profile => "You",
            Section::Messages => "Msgs",
            Section::Location => "Map",
            Section::Browsing => "Browse",
            Section::Hosting => "Host",
            Section::Display => "Look",
            Section::Notifications => "Notify",
            Section::System => "System",
        }
    }

    /// As the web UI names it.
    pub fn id(self) -> &'static str {
        match self {
            Section::Profile => "profile",
            Section::Messages => "messages",
            Section::Location => "location",
            Section::Browsing => "browsing",
            Section::Hosting => "hosting",
            Section::Display => "display",
            Section::Notifications => "notifications",
            Section::System => "system",
        }
    }

    /// Its settings, in the order the editors show them.
    pub fn fields(self) -> impl Iterator<Item = &'static Field> {
        FIELDS.iter().filter(move |field| field.section() == self)
    }
}

/// Which settings each group has.
const SECTIONS: &[(Section, &[&str])] = &[
    (
        Section::Profile,
        &[
            "display_name",
            "icon",
            "icon_color",
            "icon_background",
            "announce_at_start",
            "announce_schedule",
            "announce_random_min_mins",
            "announce_random_max_mins",
            "announce_interval_mins",
        ],
    ),
    (
        Section::Messages,
        &[
            "propagation_node",
            "auto_propagation_node",
            "sync_interval_mins",
            "resend_on_announce",
            "unknown_senders",
            "answer_commands",
            "stamp_cost",
            "max_message_kb",
            "markdown_messages",
            "picture_size",
            "messages_kept",
            "message_storage_mb",
        ],
    ),
    (Section::Location, &["location", "location_requests", "map_tiles", "map_tiles_file"]),
    (Section::Browsing, &["home", "cache_hours"]),
    (
        Section::Hosting,
        &[
            "node_enabled",
            "node_name",
            "node_announce_interval_mins",
            "node_dir",
            "node_executable_pages",
            "pn_enabled",
            "pn_name",
            "pn_stamp_cost",
            "pn_storage_mb",
            "pn_transfer_kb",
            "hub_enabled",
            "hub_name",
            "hub_greeting",
            "hub_announce_interval_mins",
            "hub_open_rooms",
        ],
    ),
    (Section::Display, &["tui_theme", "clock", "date_style", "wrap_lines", "show_joins"]),
    (Section::Notifications, &["notify_messages", "notify_rrc", "quiet_hours", "quiet_hours_trusted"]),
    (Section::System, &["log_level", "update_check", "rns_config"]),
];

impl Field {
    /// The group it's shown in.
    pub fn section(&self) -> Section {
        SECTIONS.iter().find(|(_, keys)| keys.contains(&self.key)).map_or(Section::System, |(section, _)| *section)
    }
}

/// Where rettui lives (opened by clicking the version in either UI).
pub const PROJECT_URL: &str = "https://github.com/zevaryx/rettui";

pub const FIELDS: &[Field] = &[
    Field {
        key: "display_name",
        label: "Display name",
        help: "Name sent in your LXMF announces (and your RRC nick unless a hub has its own)",
        kind: FieldKind::Text,
        effect: Effect::Now,
    },
    Field {
        key: "icon",
        label: "Icon",
        help: "Your icon, sent with your messages for Sideband, Columba and MeshChat to show beside your name: a Material Design Icon's name, such as account, radio-tower or antenna (find one at pictogrammers.com/library/mdi). Empty sends none",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field { key: "icon_color", label: "Icon colour", help: "Your icon's colour, as #rrggbb", kind: FieldKind::Color, effect: Effect::Now },
    Field {
        key: "icon_background",
        label: "Icon background",
        help: "The colour behind your icon, as #rrggbb",
        kind: FieldKind::Color,
        effect: Effect::Now,
    },
    Field {
        key: "announce_at_start",
        label: "Announce at start",
        help: "Announce your LXMF address when rettui starts",
        kind: FieldKind::Toggle,
        effect: Effect::NextStart,
    },
    Field {
        key: "announce_schedule",
        label: "Announce on its own",
        help: "random: at a random time between the two limits below, picked again each time (as Sideband does, so announces don't fall in step); fixed: every so many minutes; off: only at start and when you announce",
        kind: FieldKind::Choice(ANNOUNCE_SCHEDULES),
        effect: Effect::Now,
    },
    Field {
        key: "announce_random_min_mins",
        label: "Random: from (min)",
        help: "With random: the shortest time between announces, 60 to 360 minutes (1 to 6 hours)",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "announce_random_max_mins",
        label: "Random: to (min)",
        help: "With random: the longest time between announces, 60 to 360 minutes",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "announce_interval_mins",
        label: "Fixed: every (min)",
        help: "With fixed: minutes between announces, 60 to 360 (1 to 6 hours)",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "propagation_node",
        label: "Propagation node",
        help: "Outbound propagation node address (32 hex characters); empty for none",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "auto_propagation_node",
        label: "Pick propagation node automatically",
        help: "Warning: anyone can run a propagation node near you. The one picked (the nearest that answers fastest) sees who your propagated messages are for and when you collect yours, and could lose them; it can't read or change them. Pick a node you trust where you can",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "sync_interval_mins",
        label: "Sync every (min)",
        help: "Minutes between propagation node syncs (120, two hours, to start with); 0 turns them off",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "resend_on_announce",
        label: "Resend when they announce",
        help: "Send your messages that failed (text ones) again when their recipient announces, which shows they can be reached",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "unknown_senders",
        label: "Unknown senders",
        help: "Messages from anyone who isn't a contact (trusted, left as they are, or someone you've written to). show: like anyone's, marked as from someone unknown; requests: kept aside as message requests, last in the list, with no notifications, until you trust them, leave them as they are, reply, block them or delete it; ignore: dropped",
        kind: FieldKind::Choice(UNKNOWN_SENDERS),
        effect: Effect::Now,
    },
    Field {
        key: "answer_commands",
        label: "Answer commands",
        help: "Answer the ping, echo and signal report commands Sideband sends, as Sideband does: off; trusted (only from contacts you trust); or contacts (any contact). Answers go as messages, shown in the conversation, at most one a minute to each sender",
        kind: FieldKind::Choice(ANSWER_COMMANDS),
        effect: Effect::Now,
    },
    Field {
        key: "location",
        label: "Location",
        help: "Where this station is, as latitude, longitude (e.g. 51.5074, -0.1278): what L shares in the terminal, and the answer to location requests if they're answered. Set it for a station that stays put; the web UI can share where its device is instead. Empty for none",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "location_requests",
        label: "Location requests",
        help: "Answer the location requests (telemetry requests) Sideband sends with this station's Location: off; trusted (only from contacts you trust); or contacts (any contact). Answers are shown in the conversation, at most one a minute to each sender",
        kind: FieldKind::Choice(ANSWER_COMMANDS),
        effect: Effect::Now,
    },
    Field {
        key: "map_tiles",
        label: "Map tiles",
        help: "Where the web UI's map gets its pictures, with {z}, {x} and {y} for the zoom and the tile (OpenStreetMap's by default). rettui fetches them, so browsers need no internet of their own, and keeps them in map-tiles/ in the data directory to show again offline. Empty draws the map without them: just the places, on a grid",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "map_tiles_file",
        label: "Offline map",
        help: "An MBTiles file of map tiles (pictures: png, jpg or webp), as MOBAC, QGIS or TileMill make them, for the web UI's map with no internet at all. Its tiles come first; any it hasn't come from Map tiles, if that's set. It names a file on this computer, so it's set in the terminal UI or settings.json, not the web UI",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "stamp_cost",
        label: "Stamp cost",
        help: "Proof of work asked of senders who aren't contacts, announced with your address (8 to 16 is usual; trusted contacts get tickets instead). Messages without it are dropped. 0 asks none",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "max_message_kb",
        label: "Largest message (KB)",
        help: "Largest message taken; bigger ones are refused (NomadNet takes 500). 0 takes any size",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "markdown_messages",
        label: "Write in Markdown",
        help: "Mark the messages you write as Markdown (LXMF's renderer field), so Sideband, NomadNet and rettui show **bold**, *italic*, `code`, lists and links formatted",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "picture_size",
        label: "Send pictures at",
        help: "Pictures you send are made smaller first, as Sideband and MeshChat do, to fit: small (480 pixels on the longest side, for LoRa), medium (1024), large (2048, often too big for a propagation node), or original (as they are). GIFs go as they are. A picture made smaller leaves out its metadata, such as where a photo was taken",
        kind: FieldKind::Choice(crate::app::shrink::PICTURE_SIZES),
        effect: Effect::Now,
    },
    Field {
        key: "messages_kept",
        label: "Messages kept",
        help: "Newest messages each conversation keeps and shows; older ones move to the archive (archive/ in the data directory). 0 keeps all",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "message_storage_mb",
        label: "Message storage (MB)",
        help: "Most disk messages may use (the store and the archive together); the oldest archived months are deleted to stay within it. Attachments aren't counted. 0 is no limit",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "home",
        label: "Home page",
        help: "NomadNet page opened by the browser's home key, e.g. <hash>:/page/index.mu",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "cache_hours",
        label: "Cache pages for (h)",
        help: "Hours NomadNet pages and images stay cached; 0 turns the cache off",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "node_enabled",
        label: "Host a node",
        help: "Serve the pages in the node folder as a NomadNet node (edit them in the Node tab)",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "node_name",
        label: "Node name",
        help: "Name your node announces; empty uses your display name",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "node_announce_interval_mins",
        label: "Node announce (min)",
        help: "Minutes between node announces; 0 announces only when the node starts",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "node_dir",
        label: "Node folder",
        help: "Folder with the node's pages/ and files/; empty uses node/ in the data directory",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "node_executable_pages",
        label: "Run page scripts",
        help: "Run executable pages as programs (NomadNet dynamic pages); only with scripts you trust",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "pn_enabled",
        label: "Host a propagation node",
        help: "Keep messages for people who are offline until they collect them, as lxmd does (it takes messages from peers, but doesn't sync to other nodes itself)",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "pn_name",
        label: "Propagation node name",
        help: "Name your propagation node announces; empty uses your display name",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "pn_stamp_cost",
        label: "Propagation stamp cost",
        help: "Proof of work asked of each message sent through your propagation node (at least 13; 16 is usual)",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "pn_storage_mb",
        label: "Propagation storage (MB)",
        help: "Most disk the propagation node's messages may use (propagation/ in the data directory); the oldest go first",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "pn_transfer_kb",
        label: "Propagation message (KB)",
        help: "Largest message a client may send through your propagation node (256 is usual)",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "hub_enabled",
        label: "Host an RRC hub",
        help: "Run an RRC chat hub that people can join from NomadNet, MeshChatX, Ratspeak or rettui (rsRRCD's hub, on your Reticulum). It has an address of its own, not your LXMF one. Manage it in the Hub tab",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "hub_name",
        label: "Hub name",
        help: "Name your hub announces and welcomes people with; empty uses your display name",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "hub_greeting",
        label: "Hub greeting",
        help: "Sent to everyone who connects to your hub (\\n starts a new line); empty sends none",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "hub_announce_interval_mins",
        label: "Hub announce (min)",
        help: "Minutes between hub announces; 0 announces only when the hub starts",
        kind: FieldKind::Number,
        effect: Effect::Now,
    },
    Field {
        key: "hub_open_rooms",
        label: "Anyone makes rooms",
        help: "Anyone may make a room on your hub by joining it, as on most hubs; off, only you can, and others join the ones there are",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "tui_theme",
        label: "Terminal colours",
        help: "The terminal UI's colours: dark (for a dark terminal), light (for a light one), or basic (16 colours, for terminals without full colour). The web UI picks its own, in its Status page",
        kind: FieldKind::Choice(crate::ui::THEMES),
        effect: Effect::Now,
    },
    Field {
        key: "clock",
        label: "Clock",
        help: "Times as 24-hour (14:05) or 12-hour (2:05 PM), in both UIs",
        kind: FieldKind::Choice(crate::clock::CLOCKS),
        effect: Effect::Now,
    },
    Field {
        key: "date_style",
        label: "Dates",
        help: "Dates as month-day (Oct 07), day-month (07 Oct) or year-month-day (2025-10-07)",
        kind: FieldKind::Choice(crate::clock::DATE_STYLES),
        effect: Effect::Now,
    },
    Field {
        key: "wrap_lines",
        label: "Wrap editor lines",
        help: "Wrap long lines in the text editors (pages, Reticulum config) instead of scrolling sideways",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "show_joins",
        label: "Show joins and leaves",
        help: "Show people joining and leaving RRC rooms in the chat (also on each room page)",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "notify_messages",
        label: "Notify on messages",
        help: "Notifications for new LXMF messages you aren't looking at (each conversation can be muted)",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "notify_rrc",
        label: "Notify from RRC",
        help: "Notifications from RRC hubs: mentions and whispers unless a hub or room is set otherwise",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "quiet_hours",
        label: "Quiet hours",
        help: "No notifications between these times of day, this computer's time, e.g. 22:00-07:00 (past midnight is fine). Messages still arrive and count as unread; only the notifications wait. Empty for none",
        kind: FieldKind::Optional,
        effect: Effect::Now,
    },
    Field {
        key: "quiet_hours_trusted",
        label: "Trusted break quiet hours",
        help: "During quiet hours, messages from contacts you trust still notify",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "log_level",
        label: "Log level",
        help: "How much goes to rettui.log in the data directory: error, warn (the default), info, debug, or trace (a great deal, for tracking a problem down). RETTUI_LOG, if set when rettui starts, decides instead",
        kind: FieldKind::Choice(crate::logging::LEVELS),
        effect: Effect::Now,
    },
    Field {
        key: "update_check",
        label: "Check for updates",
        help: "Recommended: once a day, ask GitHub for the newest rettui release, and say so (by the version, and in Status) when it's newer than this one. It's one HTTPS request to api.github.com (through the proxy set in the environment, if any), saying it's rettui and which version; nothing is downloaded or installed. GitHub sees your IP address, so leave it off if you use Reticulum to stay off the internet. Off by default",
        kind: FieldKind::Toggle,
        effect: Effect::Now,
    },
    Field {
        key: "rns_config",
        label: "Reticulum config",
        help: "Reticulum config directory; empty uses the standard one (and joins a running rnsd)",
        kind: FieldKind::Optional,
        effect: Effect::NextStart,
    },
];

/// Longest display name accepted.
const MAX_DISPLAY_NAME: usize = 128;
/// Upper bounds for numbers (a year), so durations cannot overflow.
const MAX_MINUTES: u64 = 525_600;
/// How often the LXMF address may be announced on its own, in minutes:
/// not more than hourly (public gateways hold back destinations that
/// announce more), and at least every six hours, as NomadNet does.
pub const ANNOUNCE_MINS: (u64, u64) = (60, 360);
/// Whose commands are answered: no one's, trusted contacts', or any
/// contact's.
pub const ANSWER_COMMANDS: &[&str] = &["off", "trusted", "contacts"];
/// OpenStreetMap's own map tiles (their tile usage policy asks that apps
/// say who they are, keep tiles a while and credit OpenStreetMap, which
/// rettui does).
pub const OSM_TILES: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
/// What can become of messages from unknown senders.
pub const UNKNOWN_SENDERS: &[&str] = &["show", "requests", "ignore"];
/// The ways of announcing on its own.
pub const ANNOUNCE_SCHEDULES: &[&str] = &["random", "fixed", "off"];
const MAX_HOURS: u64 = 8_760;
/// Highest stamp cost (LXMF's), and largest message limit (a gigabyte).
const MAX_STAMP_COST: u64 = 254;
const MAX_MESSAGE_KB: u64 = 1_000_000;
/// Most messages a conversation may be set to keep.
const MAX_MESSAGES_KEPT: u64 = 1_000_000;
/// Largest storage limit, in megabytes (a petabyte, so bytes can't overflow).
const MAX_STORAGE_MB: u64 = 1_000_000_000;

pub fn field(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new().map(|d| d.home_dir().join(rest)).unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// A number from `low` to `high`.
fn between(value: &str, (low, high): (u64, u64)) -> Result<u64, String> {
    let n = number(value, high)?;
    if n < low {
        return Err(format!("at least {low}"));
    }
    Ok(n)
}

/// One of `choices`.
fn choice(value: &str, choices: &[&str]) -> Result<String, String> {
    let value = value.trim().to_lowercase();
    if choices.contains(&value.as_str()) { Ok(value) } else { Err(format!("one of {}", choices.join(", "))) }
}

fn number(value: &str, max: u64) -> Result<u64, String> {
    let n: u64 = value.trim().parse().map_err(|_| format!("{:?} is not a whole number", value.trim()))?;
    if n > max {
        return Err(format!("at most {max}"));
    }
    Ok(n)
}

fn toggle(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        other => Err(format!("{other:?} is not on or off")),
    }
}

fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

impl Settings {
    /// A setting as text, the way the editors show and edit it (toggles are
    /// `true` / `false`, unset optional values are empty).
    pub fn field_value(&self, key: &str) -> String {
        match key {
            "display_name" => self.display_name.clone(),
            "icon" => self.icon.clone().unwrap_or_default(),
            "icon_color" => self.icon_color.clone(),
            "icon_background" => self.icon_background.clone(),
            "announce_at_start" => self.announce_at_start.to_string(),
            "announce_schedule" => self.announce_schedule.clone(),
            "announce_interval_mins" => self.announce_interval_mins.to_string(),
            "announce_random_min_mins" => self.announce_random_min_mins.to_string(),
            "announce_random_max_mins" => self.announce_random_max_mins.to_string(),
            "rns_config" => self.rns_config.clone().unwrap_or_default(),
            "home" => self.home.clone().unwrap_or_default(),
            "propagation_node" => self.propagation_node.clone().unwrap_or_default(),
            "sync_interval_mins" => self.sync_interval_mins.to_string(),
            "auto_propagation_node" => self.auto_propagation_node.to_string(),
            "messages_kept" => self.messages_kept.to_string(),
            "message_storage_mb" => self.message_storage_mb.to_string(),
            "cache_hours" => self.cache_hours.to_string(),
            "node_enabled" => self.node_enabled.to_string(),
            "node_name" => self.node_name.clone().unwrap_or_default(),
            "node_announce_interval_mins" => self.node_announce_interval_mins.to_string(),
            "node_dir" => self.node_dir.clone().unwrap_or_default(),
            "node_executable_pages" => self.node_executable_pages.to_string(),
            "wrap_lines" => self.wrap_lines.to_string(),
            "show_joins" => self.show_joins.to_string(),
            "notify_messages" => self.notify_messages.to_string(),
            "notify_rrc" => self.notify_rrc.to_string(),
            "quiet_hours" => self.quiet_hours.clone().unwrap_or_default(),
            "quiet_hours_trusted" => self.quiet_hours_trusted.to_string(),
            "unknown_senders" => self.unknown_senders.clone(),
            "resend_on_announce" => self.resend_on_announce.to_string(),
            "answer_commands" => self.answer_commands.clone(),
            "location" => self.location.clone().unwrap_or_default(),
            "location_requests" => self.location_requests.clone(),
            "map_tiles" => self.map_tiles.clone().unwrap_or_default(),
            "map_tiles_file" => self.map_tiles_file.clone().unwrap_or_default(),
            "stamp_cost" => self.stamp_cost.to_string(),
            "max_message_kb" => self.max_message_kb.to_string(),
            "markdown_messages" => self.markdown_messages.to_string(),
            "picture_size" => self.picture_size.clone(),
            "log_level" => self.log_level.clone(),
            "update_check" => self.update_check.to_string(),
            "clock" => self.clock.clone(),
            "tui_theme" => self.tui_theme.clone(),
            "date_style" => self.date_style.clone(),
            "pn_enabled" => self.pn_enabled.to_string(),
            "pn_name" => self.pn_name.clone().unwrap_or_default(),
            "pn_stamp_cost" => self.pn_stamp_cost.to_string(),
            "pn_storage_mb" => self.pn_storage_mb.to_string(),
            "pn_transfer_kb" => self.pn_transfer_kb.to_string(),
            "hub_enabled" => self.hub_enabled.to_string(),
            "hub_name" => self.hub_name.clone().unwrap_or_default(),
            "hub_greeting" => self.hub_greeting.clone().unwrap_or_default(),
            "hub_announce_interval_mins" => self.hub_announce_interval_mins.to_string(),
            "hub_open_rooms" => self.hub_open_rooms.to_string(),
            _ => String::new(),
        }
    }

    /// Change one setting from text, checking it first.
    pub fn set_field(&mut self, key: &str, value: &str) -> Result<(), String> {
        let label = field(key).map_or(key, |f| f.label);
        let fail = |e: String| format!("{label}: {e}");
        match key {
            "display_name" => {
                let name = value.trim();
                if name.is_empty() {
                    return Err(fail("cannot be empty".into()));
                }
                if name.chars().count() > MAX_DISPLAY_NAME {
                    return Err(fail(format!("at most {MAX_DISPLAY_NAME} characters")));
                }
                self.display_name = name.to_string();
            }
            "announce_at_start" => self.announce_at_start = toggle(value).map_err(fail)?,
            "icon" => {
                self.icon = match optional(value).map(|name| name.to_lowercase()) {
                    Some(name) if crate::icons::glyph(&name).is_none() => {
                        return Err(fail(format!("there's no icon called {name:?} (find one at {})", crate::icons::LIBRARY_URL)));
                    }
                    name => name,
                };
            }
            "icon_color" | "icon_background" => {
                let colour = crate::icons::parse_colour(value).ok_or_else(|| fail("a colour is #rrggbb, as #4fd6e0".into()))?;
                let colour = crate::icons::hex_colour(colour);
                if key == "icon_color" {
                    self.icon_color = colour;
                } else {
                    self.icon_background = colour;
                }
            }
            "auto_propagation_node" => self.auto_propagation_node = toggle(value).map_err(fail)?,
            "node_enabled" => self.node_enabled = toggle(value).map_err(fail)?,
            "node_executable_pages" => self.node_executable_pages = toggle(value).map_err(fail)?,
            "wrap_lines" => self.wrap_lines = toggle(value).map_err(fail)?,
            "show_joins" => self.show_joins = toggle(value).map_err(fail)?,
            "notify_messages" => self.notify_messages = toggle(value).map_err(fail)?,
            "notify_rrc" => self.notify_rrc = toggle(value).map_err(fail)?,
            "quiet_hours" => {
                self.quiet_hours = match optional(value) {
                    None => None,
                    Some(text) => Some(
                        crate::app::notify::QuietHours::parse(&text)
                            .ok_or_else(|| fail("two times of day, as 22:00-07:00".into()))?
                            .label(),
                    ),
                };
            }
            "quiet_hours_trusted" => self.quiet_hours_trusted = toggle(value).map_err(fail)?,
            "unknown_senders" => self.unknown_senders = choice(value, UNKNOWN_SENDERS).map_err(fail)?,
            "resend_on_announce" => self.resend_on_announce = toggle(value).map_err(fail)?,
            "answer_commands" => self.answer_commands = choice(value, ANSWER_COMMANDS).map_err(fail)?,
            "location" => {
                self.location = match optional(value) {
                    None => None,
                    Some(text) => {
                        let at = crate::lxmf::Location::parse(&text)
                            .ok_or_else(|| fail("latitude, longitude in degrees, as 51.5074, -0.1278".into()))?;
                        Some(format!("{}, {}", at.latitude, at.longitude))
                    }
                };
            }
            "location_requests" => self.location_requests = choice(value, ANSWER_COMMANDS).map_err(fail)?,
            "map_tiles" => {
                if let Some(url) = optional(value) {
                    let web = url.starts_with("https://") || url.starts_with("http://");
                    if !web || !["{z}", "{x}", "{y}"].iter().all(|part| url.contains(part)) {
                        return Err(fail(
                            "a web address with {z}, {x} and {y} in it, as https://tile.openstreetmap.org/{z}/{x}/{y}.png".into(),
                        ));
                    }
                }
                self.map_tiles = optional(value);
            }
            "map_tiles_file" => {
                // The one open now, closed: another is chosen, or none.
                crate::web::mbtiles::forget();
                self.map_tiles_file = match optional(value) {
                    None => None,
                    Some(text) => {
                        let path = crate::app::files::path_from_input(&text);
                        crate::web::mbtiles::describe(&path).map_err(fail)?;
                        Some(path.display().to_string())
                    }
                };
            }
            "stamp_cost" => self.stamp_cost = number(value, MAX_STAMP_COST).map_err(fail)?,
            "max_message_kb" => self.max_message_kb = number(value, MAX_MESSAGE_KB).map_err(fail)?,
            "pn_enabled" => self.pn_enabled = toggle(value).map_err(fail)?,
            "hub_enabled" => self.hub_enabled = toggle(value).map_err(fail)?,
            "hub_open_rooms" => self.hub_open_rooms = toggle(value).map_err(fail)?,
            "hub_name" => {
                if optional(value).is_some_and(|n| n.chars().count() > MAX_DISPLAY_NAME) {
                    return Err(fail(format!("at most {MAX_DISPLAY_NAME} characters")));
                }
                self.hub_name = optional(value);
            }
            "hub_greeting" => self.hub_greeting = optional(value),
            "hub_announce_interval_mins" => {
                self.hub_announce_interval_mins = number(value, MAX_MINUTES).map_err(fail)?;
            }
            "markdown_messages" => self.markdown_messages = toggle(value).map_err(fail)?,
            "picture_size" => self.picture_size = choice(value, crate::app::shrink::PICTURE_SIZES).map_err(fail)?,
            "log_level" => self.log_level = choice(value, crate::logging::LEVELS).map_err(fail)?,
            "update_check" => self.update_check = toggle(value).map_err(fail)?,
            "clock" => self.clock = choice(value, crate::clock::CLOCKS).map_err(fail)?,
            "tui_theme" => self.tui_theme = choice(value, crate::ui::THEMES).map_err(fail)?,
            "date_style" => self.date_style = choice(value, crate::clock::DATE_STYLES).map_err(fail)?,
            "pn_name" => {
                if optional(value).is_some_and(|n| n.chars().count() > MAX_DISPLAY_NAME) {
                    return Err(fail(format!("at most {MAX_DISPLAY_NAME} characters")));
                }
                self.pn_name = optional(value);
            }
            "pn_stamp_cost" => {
                let cost = number(value, MAX_STAMP_COST).map_err(fail)?;
                let least = u64::from(lxmf_core::constants::PROPAGATION_COST_MIN);
                if cost < least {
                    return Err(fail(format!("at least {least} (LXMF's least)")));
                }
                self.pn_stamp_cost = cost;
            }
            "pn_storage_mb" => {
                let mb = number(value, MAX_STORAGE_MB).map_err(fail)?;
                if mb == 0 {
                    return Err(fail("at least 1".into()));
                }
                self.pn_storage_mb = mb;
            }
            "pn_transfer_kb" => {
                // Peers send at most a sync's worth at once.
                let kb = number(value, lxmf_core::constants::SYNC_LIMIT as u64).map_err(fail)?;
                if kb == 0 {
                    return Err(fail("at least 1".into()));
                }
                self.pn_transfer_kb = kb;
            }
            "node_announce_interval_mins" => {
                self.node_announce_interval_mins = number(value, MAX_MINUTES).map_err(fail)?;
            }
            "node_name" => {
                if optional(value).is_some_and(|n| n.len() > nomad_core::MAX_ANNOUNCE_NAME_BYTES) {
                    return Err(fail(format!("at most {} bytes", nomad_core::MAX_ANNOUNCE_NAME_BYTES)));
                }
                self.node_name = optional(value);
            }
            "node_dir" => {
                // Created when the node starts, so it only needs a parent.
                if let Some(text) = optional(value)
                    && !expand_home(&text).parent().is_some_and(Path::is_dir)
                {
                    return Err(fail(format!("no such parent directory for {text}")));
                }
                self.node_dir = optional(value);
            }
            "announce_schedule" => self.announce_schedule = choice(value, ANNOUNCE_SCHEDULES).map_err(fail)?,
            "announce_interval_mins" => self.announce_interval_mins = between(value, ANNOUNCE_MINS).map_err(fail)?,
            "announce_random_min_mins" => self.announce_random_min_mins = between(value, ANNOUNCE_MINS).map_err(fail)?,
            "announce_random_max_mins" => self.announce_random_max_mins = between(value, ANNOUNCE_MINS).map_err(fail)?,
            "sync_interval_mins" => self.sync_interval_mins = number(value, MAX_MINUTES).map_err(fail)?,
            "messages_kept" => self.messages_kept = number(value, MAX_MESSAGES_KEPT).map_err(fail)?,
            "message_storage_mb" => self.message_storage_mb = number(value, MAX_STORAGE_MB).map_err(fail)?,
            "cache_hours" => self.cache_hours = number(value, MAX_HOURS).map_err(fail)?,
            "propagation_node" => {
                self.propagation_node = match optional(value) {
                    None => None,
                    Some(text) => {
                        Some(hex::encode(crate::net::parse_hash(&text).ok_or_else(|| fail("an address is 32 hex characters".into()))?))
                    }
                };
            }
            "home" => {
                if let Some(text) = optional(value) {
                    let (node, path) = text.split_once(':').unwrap_or((&text, ""));
                    if crate::net::parse_hash(node).is_none() {
                        return Err(fail("starts with a node address (32 hex characters)".into()));
                    }
                    if !path.is_empty() && !path.starts_with('/') {
                        return Err(fail("the page path starts with /, e.g. <hash>:/page/index.mu".into()));
                    }
                }
                self.home = optional(value);
            }
            "rns_config" => {
                if let Some(text) = optional(value)
                    && !expand_home(&text).is_dir()
                {
                    return Err(fail(format!("no such directory: {text}")));
                }
                self.rns_config = optional(value);
            }
            _ => return Err(format!("unknown setting {key}")),
        }
        Ok(())
    }
}

/// The Reticulum config directory to use when none is configured.
///
/// rsReticulum's own default is `~/.rsReticulum`, but joining an existing
/// shared instance (rnsd, NomadNet, Sideband) requires that instance's config
/// directory, since the shared-instance RPC key derives from it. Prefer the
/// standard Python RNS locations when they exist.
pub fn default_rns_config() -> Option<String> {
    python_rns_dirs().into_iter().find(|dir| dir.join("config").is_file()).map(|dir| dir.to_string_lossy().into_owned())
}

/// Where Python Reticulum (and so NomadNet, Sideband and rnsd) keeps its
/// config, in the order it looks.
fn python_rns_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/etc/reticulum")];
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
        dirs.extend([home.join(".config/reticulum"), home.join(".reticulum")]);
    }
    dirs
}

/// Whether a Reticulum config directory is one Python Reticulum programs
/// use too: a change to it changes theirs.
pub fn is_shared_rns_dir(dir: &Path) -> bool {
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    python_rns_dirs().iter().any(|python| same(python, dir))
}

/// Load the identity, creating one on first run.
pub fn load_identity(path: &Path) -> Result<Identity> {
    if path.exists() {
        return Identity::from_file(path).map_err(|e| anyhow::anyhow!("loading identity {}: {e}", path.display()));
    }
    let identity = Identity::new();
    identity.to_file(path).map_err(|e| anyhow::anyhow!("saving identity {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(identity)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// [`write_atomic`] for secrets: only the owner can read the file, from the
/// moment it's created (on Unix).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&tmp)?.write_all(bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_is_in_one_group() {
        for field in FIELDS {
            let groups = SECTIONS.iter().filter(|(_, keys)| keys.contains(&field.key)).count();
            assert_eq!(groups, 1, "{} is in {groups} groups", field.key);
        }
        for (section, keys) in SECTIONS {
            for key in *keys {
                assert!(FIELDS.iter().any(|f| f.key == *key), "{section:?} names {key}, which isn't a setting");
            }
        }
        // Each group has some, and all of them are shown somewhere.
        assert!(Section::ALL.iter().all(|s| s.fields().next().is_some()));
        assert_eq!(Section::ALL.iter().map(|s| s.fields().count()).sum::<usize>(), FIELDS.len());
    }

    #[test]
    fn ignoring_unknown_senders_from_before_requests_is_kept() {
        let dir = std::env::temp_dir().join(format!("rettui-unknown-senders-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let read = |json: &str| {
            std::fs::write(&path, json).unwrap();
            Settings::load(&path).unwrap().unknown_senders
        };
        assert_eq!(read(r#"{"ignore_unknown_senders": true}"#), "ignore");
        assert_eq!(read(r#"{"ignore_unknown_senders": false}"#), "show");
        assert_eq!(read(r#"{"ignore_unknown_senders": true, "unknown_senders": "requests"}"#), "requests");
        assert_eq!(read("{}"), "show");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn icons_are_named_as_mdi_names_them_and_colours_are_colours() {
        let mut settings = Settings::default();
        settings.set_field("icon", " Radio-Tower ").unwrap();
        assert_eq!(settings.icon.as_deref(), Some("radio-tower"));
        let err = settings.set_field("icon", "radio-towers").unwrap_err();
        assert!(err.contains("no icon called") && err.contains("pictogrammers.com"), "{err}");
        settings.set_field("icon", "").unwrap();
        assert_eq!(settings.icon, None);
        settings.set_field("icon_color", "F80").unwrap();
        assert_eq!(settings.field_value("icon_color"), "#ff8800");
        assert!(settings.set_field("icon_background", "blue").is_err());
    }

    #[test]
    fn python_reticulum_configs_are_shared() {
        let home = directories::BaseDirs::new().unwrap().home_dir().to_path_buf();
        assert!(is_shared_rns_dir(&home.join(".reticulum")) && is_shared_rns_dir(Path::new("/etc/reticulum")));
        assert!(!is_shared_rns_dir(&std::env::temp_dir().join("rettui-own-rns")));
    }

    #[test]
    fn every_field_round_trips() {
        let settings = Settings::default();
        for field in FIELDS {
            let mut copy = settings.clone();
            let value = settings.field_value(field.key);
            copy.set_field(field.key, &value).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(copy.field_value(field.key), value, "{}", field.key);
        }
    }

    #[test]
    fn settings_are_checked() {
        let mut s = Settings::default();
        assert!(s.set_field("display_name", "  ").is_err());
        s.set_field("display_name", "  Zev  ").unwrap();
        assert_eq!(s.display_name, "Zev");
        assert!(s.set_field("announce_interval_mins", "-5").is_err());
        assert!(s.set_field("announce_interval_mins", "99999999999").is_err());
        s.set_field("sync_interval_mins", "0").unwrap();
        s.set_field("announce_at_start", "off").unwrap();
        assert!(!s.announce_at_start);
        assert!(s.set_field("propagation_node", "xyz").is_err());
        s.set_field("propagation_node", "<AA11BB22CC33DD44EE55FF6600778899>").unwrap();
        assert_eq!(s.propagation_node.as_deref(), Some("aa11bb22cc33dd44ee55ff6600778899"));
        s.set_field("propagation_node", "").unwrap();
        assert_eq!(s.propagation_node, None);
        assert!(s.set_field("home", "aa11bb22cc33dd44ee55ff6600778899:page").is_err());
        s.set_field("home", "aa11bb22cc33dd44ee55ff6600778899:/page/index.mu").unwrap();
        assert!(s.set_field("rns_config", "/definitely/not/here").is_err());
        s.set_field("rns_config", &std::env::temp_dir().to_string_lossy()).unwrap();
        // A propagation node asks at least LXMF's least stamp cost, and
        // keeps something.
        assert!(s.set_field("pn_stamp_cost", "12").is_err());
        s.set_field("pn_stamp_cost", "20").unwrap();
        assert!(s.set_field("pn_storage_mb", "0").is_err());
        assert!(s.set_field("pn_transfer_kb", "20000").is_err());
        s.set_field("pn_transfer_kb", "512").unwrap();
        assert_eq!((s.pn_stamp_cost, s.pn_transfer_kb), (20, 512));
        let error = s.set_field("cache_hours", "lots").unwrap_err();
        assert!(error.starts_with("Cache pages for (h):"), "{error}");
        assert!(s.set_field("nonsense", "1").is_err());
    }

    #[test]
    fn announcing_on_its_own_keeps_within_one_to_six_hours() {
        use crate::net::AnnounceSchedule;
        use std::time::Duration;
        let hours = |h: u64| Duration::from_secs(h * 3600);
        let mut s = Settings::default();
        // Random, between an hour and six, to start with.
        assert_eq!(s.announce_schedule(), AnnounceSchedule::Between(hours(1), hours(6)));
        for _ in 0..200 {
            let wait = s.announce_schedule().next().unwrap();
            assert!(wait >= hours(1) && wait <= hours(6), "{wait:?}");
        }
        // Fixed, or random within limits of one's own; none outside 1 to 6
        // hours.
        for (key, value) in [("announce_interval_mins", "59"), ("announce_random_min_mins", "361"), ("announce_schedule", "sometimes")] {
            assert!(s.set_field(key, value).is_err(), "{key} {value}");
        }
        s.set_field("announce_random_min_mins", "120").unwrap();
        s.set_field("announce_random_max_mins", "90").unwrap();
        assert_eq!(s.announce_schedule(), AnnounceSchedule::Between(Duration::from_secs(90 * 60), hours(2)), "either way round");
        s.set_field("announce_schedule", " Fixed ").unwrap();
        s.set_field("announce_interval_mins", "90").unwrap();
        assert_eq!(s.announce_schedule(), AnnounceSchedule::Every(Duration::from_secs(90 * 60)));
        assert_eq!(s.announce_schedule().next(), Some(Duration::from_secs(90 * 60)));
        // settings.json edited by hand: held to the limits.
        s.announce_interval_mins = 5;
        assert_eq!(s.announce_schedule(), AnnounceSchedule::Every(hours(1)));
        s.set_field("announce_schedule", "off").unwrap();
        assert_eq!((s.announce_schedule(), s.announce_schedule().next()), (AnnounceSchedule::Off, None));
    }

    #[test]
    fn settings_from_before_the_announce_schedule_keep_their_choice() {
        let dir = std::env::temp_dir().join(format!("rettui-schedule-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let load = |minutes: u64| {
            std::fs::write(&path, format!("{{\"display_name\": \"Zev\", \"announce_interval_mins\": {minutes}}}")).unwrap();
            let s = Settings::load(&path).unwrap();
            (s.announce_schedule, s.announce_interval_mins)
        };
        assert_eq!(load(0), ("off".into(), 360), "off stays off");
        assert_eq!(load(360), ("random".into(), 360), "the default then gives way");
        assert_eq!(load(120), ("fixed".into(), 120), "one's own interval stays");
        assert_eq!(load(30), ("fixed".into(), 60), "within the limits");
        // Syncing every 30 minutes, the default then, is every two hours
        // now; an interval of one's own stays.
        let sync = |minutes: u64| {
            std::fs::write(&path, format!("{{\"announce_interval_mins\": 360, \"sync_interval_mins\": {minutes}}}")).unwrap();
            Settings::load(&path).unwrap().sync_interval_mins
        };
        assert_eq!((sync(30), sync(45), sync(0)), (120, 45, 0));
        assert_eq!(Settings::default().sync_interval_mins, 120);
        // Once set, it's what was chosen.
        std::fs::write(&path, r#"{"announce_schedule": "random", "announce_interval_mins": 0}"#).unwrap();
        assert_eq!(Settings::load(&path).unwrap().announce_schedule, "random");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
