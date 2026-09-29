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
    pub announce_at_start: bool,
    /// Minutes between automatic announces; 0 disables them.
    pub announce_interval_mins: u64,
    /// Reticulum config directory; `None` uses the standard location
    /// (and joins a running shared instance such as rnsd or NomadNet).
    pub rns_config: Option<String>,
    /// Page opened by the browser's "home" key, e.g. `<hash>:/page/index.mu`.
    pub home: Option<String>,
    /// Outbound propagation node (hex destination hash).
    pub propagation_node: Option<String>,
    /// Minutes between automatic propagation node syncs; 0 disables them.
    pub sync_interval_mins: u64,
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display_name: "rettui user".to_string(),
            announce_at_start: true,
            announce_interval_mins: 360,
            rns_config: None,
            home: None,
            propagation_node: None,
            sync_interval_mins: 30,
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
        }
    }
}

#[derive(Clone)]
pub struct Paths {
    pub settings: PathBuf,
    pub identity: PathBuf,
    pub store: PathBuf,
    pub log: PathBuf,
    pub downloads: PathBuf,
    pub known_identities: PathBuf,
    pub cache: PathBuf,
    pub rrc_history: PathBuf,
    /// Messages older than the newest each conversation keeps.
    pub archive: PathBuf,
    /// Files attached to messages sent from the web UI.
    pub uploads: PathBuf,
    /// Secret for the web UI's login link.
    pub web_token: PathBuf,
    /// Default folder for a hosted node's pages and files.
    pub node: PathBuf,
}

impl Paths {
    pub fn new(base: Option<PathBuf>) -> Result<Self> {
        let base = match base {
            Some(base) => base,
            None => directories::ProjectDirs::from("", "", "rettui")
                .context("could not determine a data directory")?
                .data_dir()
                .to_path_buf(),
        };
        fs::create_dir_all(&base).with_context(|| format!("creating {}", base.display()))?;
        Ok(Self {
            settings: base.join("settings.json"),
            identity: base.join("identity"),
            store: base.join("store.json.gz"),
            log: base.join("rettui.log"),
            downloads: base.join("downloads"),
            known_identities: base.join("known_identities.json"),
            cache: base.join("cache"),
            rrc_history: base.join("rrc"),
            archive: base.join("archive"),
            uploads: base.join("uploads"),
            web_token: base.join("web_token"),
            node: base.join("node"),
        })
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            let settings = Self::default();
            settings.save(path)?;
            return Ok(settings);
        }
        let text = fs::read_to_string(path)?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
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
            "rns_config" | "node_dir" => WebAccess::TerminalOnly,
            "node_executable_pages" => WebAccess::TurnOffOnly,
            _ => WebAccess::Change,
        }
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
        key: "announce_at_start",
        label: "Announce at start",
        help: "Announce your LXMF address when rettui starts",
        kind: FieldKind::Toggle,
        effect: Effect::NextStart,
    },
    Field {
        key: "announce_interval_mins",
        label: "Announce every (min)",
        help: "Minutes between automatic announces; 0 turns them off",
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
        key: "sync_interval_mins",
        label: "Sync every (min)",
        help: "Minutes between propagation node syncs; 0 turns them off",
        kind: FieldKind::Number,
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
const MAX_HOURS: u64 = 8_760;
/// Most messages a conversation may be set to keep.
const MAX_MESSAGES_KEPT: u64 = 1_000_000;
/// Largest storage limit, in megabytes (a petabyte, so bytes can't overflow).
const MAX_STORAGE_MB: u64 = 1_000_000_000;

pub fn field(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new()
            .map(|d| d.home_dir().join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

fn number(value: &str, max: u64) -> Result<u64, String> {
    let n: u64 = value
        .trim()
        .parse()
        .map_err(|_| format!("{:?} is not a whole number", value.trim()))?;
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
            "announce_at_start" => self.announce_at_start.to_string(),
            "announce_interval_mins" => self.announce_interval_mins.to_string(),
            "rns_config" => self.rns_config.clone().unwrap_or_default(),
            "home" => self.home.clone().unwrap_or_default(),
            "propagation_node" => self.propagation_node.clone().unwrap_or_default(),
            "sync_interval_mins" => self.sync_interval_mins.to_string(),
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
            "node_enabled" => self.node_enabled = toggle(value).map_err(fail)?,
            "node_executable_pages" => self.node_executable_pages = toggle(value).map_err(fail)?,
            "wrap_lines" => self.wrap_lines = toggle(value).map_err(fail)?,
            "show_joins" => self.show_joins = toggle(value).map_err(fail)?,
            "notify_messages" => self.notify_messages = toggle(value).map_err(fail)?,
            "notify_rrc" => self.notify_rrc = toggle(value).map_err(fail)?,
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
            "announce_interval_mins" => self.announce_interval_mins = number(value, MAX_MINUTES).map_err(fail)?,
            "sync_interval_mins" => self.sync_interval_mins = number(value, MAX_MINUTES).map_err(fail)?,
            "messages_kept" => self.messages_kept = number(value, MAX_MESSAGES_KEPT).map_err(fail)?,
            "message_storage_mb" => self.message_storage_mb = number(value, MAX_STORAGE_MB).map_err(fail)?,
            "cache_hours" => self.cache_hours = number(value, MAX_HOURS).map_err(fail)?,
            "propagation_node" => {
                self.propagation_node = match optional(value) {
                    None => None,
                    Some(text) => Some(hex::encode(
                        crate::net::parse_hash(&text).ok_or_else(|| fail("an address is 32 hex characters".into()))?,
                    )),
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
    let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
    [
        PathBuf::from("/etc/reticulum"),
        home.join(".config/reticulum"),
        home.join(".reticulum"),
    ]
    .into_iter()
    .find(|dir| dir.join("config").is_file())
    .map(|dir| dir.to_string_lossy().into_owned())
}

/// Load the identity, creating one on first run.
pub fn load_identity(path: &Path) -> Result<Identity> {
    if path.exists() {
        return Identity::from_file(path)
            .map_err(|e| anyhow::anyhow!("loading identity {}: {e}", path.display()));
    }
    let identity = Identity::new();
    identity
        .to_file(path)
        .map_err(|e| anyhow::anyhow!("saving identity {}: {e}", path.display()))?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let error = s.set_field("cache_hours", "lots").unwrap_err();
        assert!(error.starts_with("Cache pages for (h):"), "{error}");
        assert!(s.set_field("nonsense", "1").is_err());
    }
}
