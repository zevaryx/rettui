//! Reading, checking and editing the Reticulum config file (the one in the
//! Reticulum config directory, not rettui's own settings).
//!
//! Edits go through [`doc::Doc`] so the file keeps its comments and layout,
//! and every edit is checked with rns-runtime's own parser before it is
//! saved. Reticulum reads the file when it starts, so saved changes apply
//! after a restart (of rettui, or of whatever runs the shared instance).

pub mod doc;
pub mod schema;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rns_runtime::config::{Config, ConfigSection, ConfigValue};
use rns_runtime::interface_factory::{InterfaceFactoryError, synthesize_interface};
use rns_runtime::reticulum::ReticulumConfig;

use doc::{Doc, format_list, format_value};
use schema::{Group, Kind, Opt};

pub const RESTART_NOTE: &str = "Reticulum reads this file when it starts: restart Reticulum to apply changes";

/// Shown when rettui uses another program's shared instance.
pub const EXTERNAL_NOTE: &str =
    "rettui uses another program's shared instance (such as rnsd): its interfaces change when that program restarts";

/// The config file for a Reticulum config directory setting.
pub fn config_path(rns_config: Option<&str>) -> PathBuf {
    let dir = rns_config.map(|d| crate::config::expand_home(d).to_string_lossy().into_owned());
    rns_runtime::platform::resolve_config_dir(dir.as_deref()).join("config")
}

/// The file's text, or rsReticulum's default config when there is none yet
/// (saving then creates it).
pub fn load(path: &Path) -> Result<(String, bool), String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok((text, true)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Config::default_config().to_string(), false)),
        Err(e) => Err(format!("Could not read {}: {e}", path.display())),
    }
}

/// Write the file, keeping the previous version as `config.backup`.
pub fn save(path: &Path, text: &str) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("Could not save {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    if path.is_file() {
        std::fs::copy(path, path.with_file_name("config.backup")).map_err(fail)?;
    }
    let temp = path.with_file_name(".config.rettui-tmp");
    std::fs::write(&temp, text).map_err(fail)?;
    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temp, metadata.permissions());
    }
    std::fs::rename(&temp, path).map_err(fail)
}

/// Problems found in a config: an error stops Reticulum from starting (and
/// the file from being saved); warnings are interfaces that will fail.
#[derive(Debug, Default, Clone)]
pub struct Check {
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

pub fn check(text: &str) -> (Option<Config>, Check) {
    let config = match Config::parse(text) {
        Ok(config) => config,
        Err(e) => {
            return (
                None,
                Check {
                    error: Some(e.to_string()),
                    warnings: Vec::new(),
                },
            );
        }
    };
    let mut result = Check::default();
    if let Err(e) = ReticulumConfig::try_from_config(&config) {
        result.error = Some(e.to_string());
    }
    for (name, section) in config.subsections("interfaces") {
        match synthesize_interface(name, section) {
            Ok(_) => {}
            // Switched off in the file.
            Err(InterfaceFactoryError::Disabled(n)) if n == name => {}
            Err(InterfaceFactoryError::Disabled(why)) => result.warnings.push(format!("{name}: not supported here ({why})")),
            Err(e) => result.warnings.push(format!("{name}: {e}")),
        }
    }
    result.warnings.extend(bootstrap_warnings(&config));
    (Some(config), result)
}

/// How many entry points interface discovery connects to by itself (0
/// when it's off or doesn't connect).
pub fn discovery_autoconnect(config: &Config) -> u64 {
    let Some(reticulum) = config.section("reticulum") else { return 0 };
    if reticulum.get_bool("discover_interfaces") != Some(true) {
        return 0;
    }
    reticulum
        .get_uint("autoconnect_discovered_interfaces")
        .or_else(|| reticulum.get_uint("discover_interfaces_autoconnect"))
        .unwrap_or(0)
}

/// Interfaces marked Bootstrap only, while discovery connects by itself:
/// rsReticulum drops them as soon as it starts connecting to that many
/// discovered entry points, before any of them is up, and doesn't bring
/// them back if they never come up.
pub fn bootstrap_warnings(config: &Config) -> Vec<String> {
    let connects = discovery_autoconnect(config);
    if connects == 0 {
        return Vec::new();
    }
    config
        .subsections("interfaces")
        .into_iter()
        .filter(|(_, section)| {
            let on = section.get_bool("enabled").or_else(|| section.get_bool("interface_enabled")).unwrap_or(true);
            on && section.get_bool("bootstrap_only") == Some(true)
        })
        .map(|(name, _)| {
            format!(
                "{name}: Bootstrap only: dropped as soon as Reticulum starts connecting to {connects} discovered entry point{}, \
                 before any is up, and not brought back if they never come up (rsReticulum, for now)",
                if connects == 1 { "" } else { "s" }
            )
        })
        .collect()
}

/// A part of the file the editors show as one page of options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    Reticulum,
    Logging,
    Interface(String),
}

impl Section {
    pub fn path(&self) -> Vec<&str> {
        match self {
            Section::Reticulum => vec!["reticulum"],
            Section::Logging => vec!["logging"],
            Section::Interface(name) => vec!["interfaces", name.as_str()],
        }
    }

    pub fn title(&self) -> String {
        match self {
            Section::Reticulum => "Reticulum".into(),
            Section::Logging => "Logging".into(),
            Section::Interface(name) => name.clone(),
        }
    }

    /// `reticulum`, `logging` or `interface:<name>` (for the web API).
    pub fn id(&self) -> String {
        match self {
            Section::Reticulum => "reticulum".into(),
            Section::Logging => "logging".into(),
            Section::Interface(name) => format!("interface:{name}"),
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "reticulum" => Some(Section::Reticulum),
            "logging" => Some(Section::Logging),
            _ => id.strip_prefix("interface:").map(|n| Section::Interface(n.to_string())),
        }
    }
}

/// The sections of a config, interfaces in file order.
pub fn sections(text: &str) -> Vec<Section> {
    let mut sections = vec![Section::Reticulum, Section::Logging];
    sections.extend(Doc::new(text).subsections("interfaces").into_iter().map(Section::Interface));
    sections
}

fn section_of<'a>(config: &'a Config, section: &Section) -> Option<&'a ConfigSection> {
    match section {
        Section::Interface(name) => config.subsection("interfaces", name),
        _ => config.section(section.path()[0]),
    }
}

/// One option as the editors show it.
#[derive(Debug, Clone)]
pub struct OptionView {
    pub group: &'static str,
    pub key: String,
    pub aliases: &'static [&'static str],
    pub label: String,
    pub kind: Kind,
    pub default: &'static str,
    pub help: &'static str,
    /// The value in the file, lists joined with commas; `None` when unset.
    pub value: Option<String>,
}

impl OptionView {
    /// The value to show, with secrets masked.
    pub fn shown(&self) -> Option<String> {
        self.value.as_ref().map(|v| if self.kind == Kind::Secret && !v.is_empty() { "•".repeat(8) } else { v.clone() })
    }
}

const TYPE_OPT: Opt = Opt {
    key: "type",
    aliases: &[],
    label: "Type",
    kind: Kind::Text,
    default: "",
    help: "Kind of interface; changing it keeps the other keys, so check them afterwards",
};

fn value_text(value: &ConfigValue) -> String {
    match value {
        ConfigValue::Scalar(s) => s.clone(),
        ConfigValue::List(items) => items.join(", "),
    }
}

/// The groups of options for a section of this config.
fn groups(config: Option<&Config>, section: &Section) -> Vec<(&'static str, &'static [Opt])> {
    let group = |g: &'static Group| (g.title, g.opts);
    match section {
        Section::Reticulum => schema::RETICULUM.iter().map(group).collect(),
        Section::Logging => schema::LOGGING.iter().map(group).collect(),
        Section::Interface(_) => {
            let kind = config.and_then(|c| section_of(c, section)).and_then(|s| s.get("type")).and_then(schema::interface_type);
            let mut groups = vec![("Interface", std::slice::from_ref(&TYPE_OPT)), group(&schema::INTERFACE_BASIC)];
            if let Some(kind) = kind {
                groups.push((kind.label, kind.opts));
            }
            groups.extend(schema::INTERFACE_ADVANCED.iter().map(group));
            groups
        }
    }
}

/// Every option of a section with its value, plus keys in the file that
/// rettui does not know (shown as plain text under "Other").
pub fn options(config: Option<&Config>, section: &Section) -> Vec<OptionView> {
    let values = config.and_then(|c| section_of(c, section));
    let lookup = |opt: &Opt| {
        let values = values?;
        std::iter::once(opt.key)
            .chain(opt.aliases.iter().copied())
            .find_map(|k| values.values.get(k))
            .map(value_text)
    };
    let mut known: BTreeSet<&str> = BTreeSet::new();
    let mut views = Vec::new();
    for (title, opts) in groups(config, section) {
        for opt in opts {
            known.insert(opt.key);
            known.extend(opt.aliases);
            views.push(OptionView {
                group: title,
                key: opt.key.to_string(),
                aliases: opt.aliases,
                label: opt.label.to_string(),
                kind: opt.kind,
                default: opt.default,
                help: opt.help,
                value: lookup(opt),
            });
        }
    }
    if let Some(values) = values {
        let mut other: Vec<(&String, &ConfigValue)> = values.values.iter().filter(|(k, _)| !known.contains(k.as_str())).collect();
        other.sort_by_key(|(k, _)| k.as_str());
        views.extend(other.into_iter().map(|(key, value)| OptionView {
            group: "Other",
            key: key.clone(),
            aliases: &[],
            label: key.clone(),
            kind: Kind::Text,
            default: "",
            help: "A key rettui has no description for; it is kept as written",
            value: Some(value_text(value)),
        }));
    }
    views
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Some(true),
        "no" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// `Yes`/`No`, or the style the existing value is written in.
fn bool_text(value: bool, existing: Option<&str>) -> String {
    let style = existing.map(|e| e.trim().trim_matches(['"', '\'']).to_string()).unwrap_or_default();
    let (yes, no) = match style.as_str() {
        "True" | "False" => ("True", "False"),
        "true" | "false" => ("true", "false"),
        "yes" | "no" => ("yes", "no"),
        "on" | "off" => ("on", "off"),
        "On" | "Off" => ("On", "Off"),
        "1" | "0" => ("1", "0"),
        _ => ("Yes", "No"),
    };
    (if value { yes } else { no }).to_string()
}

/// Check a typed value for an option and turn it into file text. An empty
/// value unsets the option.
fn encode(view: &OptionView, value: &str, existing: Option<&str>) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let label = &view.label;
    let text = match view.kind {
        Kind::Bool => {
            let b = parse_bool(value).ok_or_else(|| format!("{label}: {value:?} is not yes or no"))?;
            bool_text(b, existing)
        }
        Kind::Int => {
            value.parse::<i64>().map_err(|_| format!("{label}: {value:?} is not a whole number"))?;
            value.to_string()
        }
        Kind::Float => {
            let n: f64 = value.parse().map_err(|_| format!("{label}: {value:?} is not a number"))?;
            if !n.is_finite() {
                return Err(format!("{label}: {value:?} is not a number"));
            }
            value.to_string()
        }
        Kind::Choice(choices) => choices
            .iter()
            .find(|c| c.eq_ignore_ascii_case(value))
            .map(|c| c.to_string())
            .ok_or_else(|| format!("{label}: one of {}", choices.join(", ")))?,
        Kind::List => {
            let items: Vec<String> = value.split(',').map(|i| i.trim().to_string()).filter(|i| !i.is_empty()).collect();
            format_list(&items)
        }
        Kind::Text | Kind::Secret => format_value(value),
    };
    Ok(Some(text))
}

/// Commands of the pipe interfaces (programs Reticulum runs).
fn pipe_commands(config: Option<&Config>) -> BTreeSet<String> {
    let Some(config) = config else { return BTreeSet::new() };
    config
        .subsections("interfaces")
        .into_iter()
        .filter(|(_, s)| s.get("type") == Some("PipeInterface"))
        .filter_map(|(_, s)| s.get("command").map(str::to_string))
        .collect()
}

/// Check an edited config before saving: it must load, and with
/// `restricted` (the web UI) it may not add or change programs that
/// Reticulum runs (pipe interface commands).
pub fn validate(before: &str, after: &str, restricted: bool) -> Result<Check, String> {
    let (config, check) = check(after);
    if let Some(error) = &check.error {
        return Err(format!("Not saved: {error}"));
    }
    if restricted && !pipe_commands(config.as_ref()).is_subset(&pipe_commands(check_config(before).as_ref())) {
        return Err("Pipe interface commands run programs, so they can only be changed from the terminal".into());
    }
    Ok(check)
}

fn check_config(text: &str) -> Option<Config> {
    Config::parse(text).ok()
}

/// Apply option changes (`key`, typed value; empty unsets) to a section.
pub fn set_options(text: &str, section: &Section, changes: &[(&str, &str)]) -> Result<String, String> {
    let config = check_config(text);
    let views = options(config.as_ref(), section);
    let mut doc = Doc::new(text);
    let path = section.path();
    if let Section::Interface(name) = section
        && !doc.has_section(&path)
    {
        return Err(format!("There is no interface named {name}"));
    }
    for (key, value) in changes {
        let view = views.iter().find(|v| v.key == *key).cloned().unwrap_or_else(|| OptionView {
            group: "Other",
            key: key.to_string(),
            aliases: &[],
            label: key.to_string(),
            kind: Kind::Text,
            default: "",
            help: "",
            value: None,
        });
        if view.key == "type" && value.trim().is_empty() {
            return Err("An interface needs a type".into());
        }
        let present = doc.present_key(&path, &std::iter::once(view.key.as_str()).chain(view.aliases.iter().copied()).collect::<Vec<_>>());
        let existing = present.as_deref().and_then(|k| doc.raw_value(&path, k));
        let encoded = encode(&view, value, existing.as_deref())?;
        doc.set(&path, &view.key, view.aliases, encoded.as_deref());
    }
    Ok(doc.text())
}

pub fn add_interface(text: &str, name: &str, kind: &str) -> Result<String, String> {
    let kind = schema::interface_type(kind).ok_or_else(|| format!("Unknown interface type {kind}"))?;
    let mut doc = Doc::new(text);
    let mut keys = vec![("type", kind.name), ("enabled", "Yes")];
    keys.extend(kind.starter.iter().copied());
    doc.add_subsection("interfaces", name.trim(), &keys)?;
    Ok(doc.text())
}

pub fn rename_interface(text: &str, name: &str, new_name: &str) -> Result<String, String> {
    let mut doc = Doc::new(text);
    doc.rename(&["interfaces", name], new_name.trim())?;
    Ok(doc.text())
}

pub fn remove_interface(text: &str, name: &str) -> Result<String, String> {
    let mut doc = Doc::new(text);
    doc.remove(&["interfaces", name])?;
    Ok(doc.text())
}

/// Whether an interface is on: rsReticulum starts it unless `enabled` (or
/// `interface_enabled`) says no.
fn interface_on(doc: &Doc, name: &str) -> bool {
    ["enabled", "interface_enabled"]
        .iter()
        .find_map(|key| doc.raw_value(&["interfaces", name], key))
        .is_none_or(|value| crate::app::reticulum::rns_truthy(value.trim_matches(['"', '\''])))
}

/// Whether an interface that's on connects to `host` (a TCP client's
/// `target_host`).
pub fn has_interface_to(text: &str, host: &str) -> bool {
    let doc = Doc::new(text);
    doc.subsections("interfaces").iter().any(|name| {
        interface_on(&doc, name)
            && doc
                .raw_value(&["interfaces", name], "target_host")
                .is_some_and(|value| value.trim_matches(['"', '\'']).eq_ignore_ascii_case(host))
    })
}

/// Whether an interface that's on is one of the user's own, other than the
/// Auto interface (which reaches only the local network): an entry point,
/// a radio, and so on.
pub fn has_own_interfaces(text: &str) -> bool {
    let doc = Doc::new(text);
    doc.subsections("interfaces").iter().any(|name| {
        interface_on(&doc, name)
            && doc
                .raw_value(&["interfaces", name], "type")
                .is_some_and(|kind| kind.trim_matches(['"', '\'', ' ']) != "AutoInterface")
    })
}

/// Whether interface discovery (finding entry points others announce) is
/// on, and connects to any.
pub fn discovery_on(text: &str) -> bool {
    let doc = Doc::new(text);
    let on = |key: &str| doc.raw_value(&["reticulum"], key);
    let finds = on("discover_interfaces").is_some_and(|v| crate::app::reticulum::rns_truthy(&v));
    let connects = on("autoconnect_discovered_interfaces")
        .or_else(|| on("discover_interfaces_autoconnect"))
        .and_then(|v| v.trim().parse::<u32>().ok())
        .is_some_and(|n| n > 0);
    finds && connects
}

/// Add a TCP client interface to an entry point (named `name`, or with a
/// number after it if that's taken). The new text, and the name used.
///
/// Not `bootstrap_only`, whatever discovery does: rsReticulum drops those
/// as soon as it starts connecting to as many discovered interfaces as
/// it's set to, before any of them is up, and never brings them back.
pub fn add_entry_point(text: &str, name: &str, host: &str, port: u16) -> Result<(String, String), String> {
    let mut doc = Doc::new(text);
    let taken = doc.subsections("interfaces");
    let name = std::iter::once(name.to_string())
        .chain((2..).map(|n| format!("{name} {n}")))
        .find(|candidate| !taken.contains(candidate))
        .expect("some name is free");
    let port = port.to_string();
    let keys = [("type", "TCPClientInterface"), ("enabled", "Yes"), ("target_host", host), ("target_port", port.as_str())];
    doc.add_subsection("interfaces", &name, &keys)?;
    Ok((doc.text(), name))
}

/// Turn interface discovery on, connecting to up to `connect` of the
/// entry points found (unless it already connects to some).
pub fn enable_discovery(text: &str, connect: u32) -> String {
    let mut doc = Doc::new(text);
    doc.set(&["reticulum"], "discover_interfaces", &[], Some("Yes"));
    let current = doc
        .present_key(&["reticulum"], &["autoconnect_discovered_interfaces", "discover_interfaces_autoconnect"])
        .and_then(|key| doc.raw_value(&["reticulum"], &key))
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if current == 0 {
        doc.set(&["reticulum"], "autoconnect_discovered_interfaces", &["discover_interfaces_autoconnect"], Some(&connect.to_string()));
    }
    doc.text()
}

#[cfg(test)]
mod tests {
    #[test]
    fn entry_points_and_discovery_for_the_guide() {
        let base = rns_runtime::config::Config::default_config();
        assert!(!super::has_interface_to(base, "rmap.world") && !super::discovery_on(base));
        assert!(!super::has_own_interfaces(base), "only the Auto interface");
        let (added, name) = super::add_entry_point(base, "RMAP World", "rmap.world", 4242).unwrap();
        assert_eq!(name, "RMAP World");
        let (config, check) = super::check(&added);
        assert!(check.error.is_none(), "{:?}", check.error);
        let interface = config.unwrap();
        let section = interface.subsection("interfaces", "RMAP World").unwrap();
        assert_eq!(section.get("target_host"), Some("rmap.world"));
        assert_eq!(section.get_uint("target_port"), Some(4242));
        assert_eq!(section.get_bool("bootstrap_only"), None);
        assert!(super::has_interface_to(&added, "RMAP.world") && super::has_own_interfaces(&added));
        // Turned off, it doesn't count.
        let off = super::set_options(&added, &super::Section::Interface("RMAP World".into()), &[("enabled", "No")]).unwrap();
        assert!(!super::has_interface_to(&off, "rmap.world") && !super::has_own_interfaces(&off), "{off}");
        // A second one gets a name of its own; the default interface stays.
        let (twice, name) = super::add_entry_point(&added, "RMAP World", "rmap.world", 4242).unwrap();
        assert!(twice.contains("[[RMAP World 2]]") && twice.contains("[[Default Interface]]"));
        assert_eq!(name, "RMAP World 2");
        // Bootstrap only, with discovery connecting by itself: warned of.
        let bootstrapped = super::set_options(&added, &super::Section::Interface("RMAP World".into()), &[("bootstrap_only", "Yes")]).unwrap();
        assert!(super::check(&bootstrapped).1.warnings.is_empty(), "no warning without discovery");
        let warned = super::check(&super::enable_discovery(&bootstrapped, 2)).1.warnings;
        assert!(warned.iter().any(|w| w.starts_with("RMAP World: Bootstrap only: dropped as soon as") && w.contains("2 discovered entry points")), "{warned:?}");
        let discovering = super::enable_discovery(&added, 2);
        assert!(super::discovery_on(&discovering) && super::check(&discovering).1.error.is_none());
        let config = super::check(&discovering).0.unwrap();
        let reticulum = config.section("reticulum").unwrap();
        assert_eq!((reticulum.get_bool("discover_interfaces"), reticulum.get_uint("autoconnect_discovered_interfaces")), (Some(true), Some(2)));
        // A number already set is kept.
        let set = super::enable_discovery(&discovering.replace("autoconnect_discovered_interfaces = 2", "autoconnect_discovered_interfaces = 5"), 2);
        assert!(set.contains("autoconnect_discovered_interfaces = 5"));
    }

    use super::*;

    const DEFAULT: &str = "[reticulum]\n  enable_transport = False\n  share_instance = Yes\n\n[logging]\n  loglevel = 4\n\n[interfaces]\n  [[Default Interface]]\n    type = AutoInterface\n    enabled = Yes\n";

    #[test]
    fn options_show_values_defaults_and_unknown_keys() {
        let text = format!("{DEFAULT}    custom_thing = 1\n    interface_mode = gateway\n");
        let (config, check) = check(&text);
        assert!(check.error.is_none() && check.warnings.is_empty(), "{check:?}");
        let section = Section::Interface("Default Interface".into());
        let views = options(config.as_ref(), &section);
        let get = |key: &str| views.iter().find(|v| v.key == key).unwrap();
        assert_eq!(get("type").value.as_deref(), Some("AutoInterface"));
        assert_eq!(get("mode").value.as_deref(), Some("gateway"), "read through its alias");
        assert_eq!(get("group_id").value, None);
        assert_eq!(get("group_id").default, "reticulum");
        assert_eq!(get("custom_thing").group, "Other");
        assert_eq!(sections(&text).len(), 3);
        assert_eq!(Section::from_id(&section.id()), Some(section));
    }

    #[test]
    fn values_are_checked_and_keep_their_style() {
        let text = set_options(DEFAULT, &Section::Reticulum, &[("enable_transport", "on"), ("instance_control_port", "4000")]).unwrap();
        assert!(text.contains("enable_transport = True\n"), "True/False style kept: {text}");
        assert!(text.contains("instance_control_port = 4000"));
        assert!(set_options(DEFAULT, &Section::Reticulum, &[("instance_control_port", "port")]).is_err());
        assert!(set_options(DEFAULT, &Section::Logging, &[("loglevel", "9")]).is_err());
        let text = set_options(&text, &Section::Reticulum, &[("enable_transport", "")]).unwrap();
        assert!(!text.contains("enable_transport"));
        // A bad value that parses but stops Reticulum is caught by validate.
        let bad = set_options(DEFAULT, &Section::Reticulum, &[("shared_instance_port", "70000")]).unwrap();
        assert!(validate(DEFAULT, &bad, false).is_err());
    }

    #[test]
    fn interfaces_are_added_checked_and_removed() {
        let text = add_interface(DEFAULT, "Hub", "TCPClientInterface").unwrap();
        let (_, check) = check(&text);
        assert!(check.warnings.is_empty(), "{:?}", check.warnings);
        let hub = Section::Interface("Hub".into());
        let text = set_options(&text, &hub, &[("target_host", "")]).unwrap();
        assert!(validate(DEFAULT, &text, false).unwrap().warnings[0].contains("target_host"));
        let text = rename_interface(&text, "Hub", "Remote").unwrap();
        assert!(set_options(&text, &hub, &[("bitrate", "1")]).is_err());
        let text = remove_interface(&text, "Remote").unwrap();
        assert_eq!(sections(&text).len(), 3);
    }

    #[test]
    fn the_web_cannot_add_programs() {
        let text = add_interface(DEFAULT, "Pipe", "PipeInterface").unwrap();
        assert!(validate(DEFAULT, &text, true).is_ok(), "an interface without a command runs nothing");
        let with_command = set_options(&text, &Section::Interface("Pipe".into()), &[("command", "/bin/evil")]).unwrap();
        assert!(validate(&text, &with_command, true).is_err());
        assert!(validate(&text, &with_command, false).is_ok());
        // Unchanged commands are fine, e.g. when editing another option.
        let other = set_options(&with_command, &Section::Logging, &[("loglevel", "5")]).unwrap();
        assert!(validate(&with_command, &other, true).is_ok());
        // Turning an interface with a stray command into a pipe counts too.
        let sneaky = format!("{DEFAULT}    command = /bin/evil\n");
        let switched = set_options(&sneaky, &Section::Interface("Default Interface".into()), &[("type", "PipeInterface")]).unwrap();
        assert!(validate(&sneaky, &switched, true).is_err());
    }
}
