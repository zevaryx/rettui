//! Changes to the Reticulum config asked for from afar (`/reticulum`): a
//! short list of keys ([`RULES`]), each a single in-place edit through
//! [`Doc`], so every other byte of the file stays as it was. Anything that
//! could be read two ways is refused rather than guessed at: a section or a
//! key given twice or spelled in another case, both names of a key, a value
//! over several lines, a value that never ends, a file too big to read whole.
//! An interface that runs a program, or has no `type`, can't be changed at
//! all, as the redacted view shows only its `type` and `enabled`.
//!
//! The file is read three ways (the editor's [`Doc`], the redacted view and
//! Reticulum's own parser), and a change is made only where all three agree:
//! every line that starts with `[` must be a header the editor and the view
//! read alike, and once the change is made, Reticulum must read the new file
//! as the old one with only the keys asked for changed.
//!
//! Turning things on (an interface, transport) opens the node to others, so
//! it is refused unless the node allows it (`--manage-allow-enable`).
//!
//! Nothing Reticulum or the file says reaches the answer: an error is
//! logged here and answered with a fixed sentence.
//!
//! No I/O but the log: the owner loop reads the file, plans the change here
//! and saves it.

use rns_runtime::config::{Config, ConfigSection, ConfigValue};
use rns_runtime::interface_factory::{InterfaceFactoryError, synthesize_interface};

use super::audit::Change;
use crate::reticulum::doc::{Doc, unquote};
use crate::reticulum::redact::{self, View};
use crate::reticulum::{RUNS_PROGRAMS, bootstrap_warnings, enable_warnings, history, listener_warning, reachable_runs, validate};

/// The most changes in one request.
pub const MAX_RNS_CHANGES: usize = 16;
/// The most notes in an answer, and the longest each.
const MAX_NOTES: usize = 16;
const MAX_NOTE: usize = 300;
/// The most interfaces listed as editable.
const MAX_EDITABLE_INTERFACES: usize = 32;

/// One change asked for: a section (`["reticulum"]` or `["interfaces",
/// name]`), a key and its new value as sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsChange {
    pub path: Vec<String>,
    pub key: String,
    pub value: String,
}

/// A change worked out: the new text, what changes (for the answer and the
/// audit log), the interfaces it turns off, what it turns on (a sentence
/// each, for the log and the audit log), and the notes for the answer (what
/// it turns on, and rettui's own warnings about the config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub after: String,
    pub applied: Vec<Change>,
    pub disabled: Vec<String>,
    pub turned_on: Vec<String>,
    pub notes: Vec<String>,
}

/// What a change Reticulum can't load is answered with (the reason, which
/// may quote the file, goes to the node's log).
pub const LOAD_FAILED: &str = "Reticulum couldn't load the config with this change (see the node's log)";
/// What turning something on is refused with, unless the node allows it.
pub const ENABLE_OFF: &str = "Turning interfaces or transport on from afar is off on this node: do it in the terminal, \
                              or start the node with --manage-allow-enable to allow it";

/// Why a change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RnsRefusal {
    /// The key isn't on the list, or the interface runs a program or has no type.
    NotAllowed {
        path: Vec<String>,
        key: String,
        message: String,
    },
    /// The value isn't one the key takes, or the config wouldn't load with it.
    Invalid {
        path: Vec<String>,
        key: String,
        message: String,
    },
    UnknownSection {
        path: Vec<String>,
    },
    /// The file could be read two ways where the change goes.
    Ambiguous {
        path: Vec<String>,
        key: Option<String>,
        message: String,
    },
    /// The config changed since it was read; its revision now.
    Stale {
        revision: String,
    },
    NoChange,
}

/// Where a key is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Reticulum,
    Interface,
}

/// The values a key takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `yes` or `no`.
    Switch,
    /// An interface mode.
    Mode,
    /// A percentage of the bitrate, over 0 and at most 100.
    AnnounceCap,
    /// Bits a second.
    Bitrate,
}

/// A key that can be changed, with the other name it may have in the file
/// (edited where found).
#[derive(Debug)]
struct Rule {
    place: Place,
    key: &'static str,
    aliases: &'static [&'static str],
    kind: Kind,
}

const RULES: &[Rule] = &[
    Rule { place: Place::Reticulum, key: "enable_transport", aliases: &[], kind: Kind::Switch },
    Rule { place: Place::Interface, key: "enabled", aliases: &["interface_enabled"], kind: Kind::Switch },
    Rule { place: Place::Interface, key: "mode", aliases: &["interface_mode"], kind: Kind::Mode },
    Rule { place: Place::Interface, key: "announce_cap", aliases: &[], kind: Kind::AnnounceCap },
    Rule { place: Place::Interface, key: "bitrate", aliases: &[], kind: Kind::Bitrate },
    Rule { place: Place::Interface, key: "ingress_control", aliases: &[], kind: Kind::Switch },
];

const MODES: &[&str] = &["full", "gateway", "access_point", "roaming", "boundary"];

fn place_of(path: &[&str]) -> Option<Place> {
    match path {
        ["reticulum"] => Some(Place::Reticulum),
        ["interfaces", _] => Some(Place::Interface),
        _ => None,
    }
}

/// Whether `name` can be an interface's name in a request: 1 to 64 bytes,
/// no spaces around it, no control or invisible characters, and nothing that
/// would end or quote a section header (`[ ] # " '`).
pub fn interface_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name == name.trim()
        && !name.chars().any(|c| c.is_control() || crate::stats::is_invisible(c) || matches!(c, '[' | ']' | '#' | '"' | '\'' | '\n'))
}

/// `yes`, `true`, `on` (any case) as true; `no`, `false`, `off` as false.
fn switch_word(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" => Some(true),
        "no" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// Why `value` isn't one `kind` takes, if it isn't.
fn check_value(kind: Kind, value: &str) -> Result<(), String> {
    let digits = |text: &str, max: usize| !text.is_empty() && text.len() <= max && text.bytes().all(|b| b.is_ascii_digit());
    match kind {
        Kind::Switch if value == "yes" || value == "no" => Ok(()),
        Kind::Switch => Err("This takes yes or no".into()),
        Kind::Mode if MODES.contains(&value) => Ok(()),
        Kind::Mode => Err(format!("A mode is one of {}", MODES.join(", "))),
        Kind::AnnounceCap => {
            let (whole, fraction) = value.split_once('.').map_or((value, None), |(w, f)| (w, Some(f)));
            let whole_ok = whole == "0" || whole == "100" || (digits(whole, 2) && !whole.starts_with('0'));
            let fraction_ok = fraction.is_none_or(|f| digits(f, 2));
            match value.parse::<f64>() {
                Ok(cap) if whole_ok && fraction_ok && cap > 0.0 && cap <= 100.0 => Ok(()),
                _ => Err("The announce cap is a percentage over 0 and at most 100, with at most two decimals (such as 2 or 2.5)".into()),
            }
        }
        Kind::Bitrate => match value.parse::<u64>() {
            Ok(rate) if digits(value, 11) && !value.starts_with('0') && (5..=10_000_000_000).contains(&rate) => Ok(()),
            _ => Err("The bitrate is a whole number of bits a second, from 5 to 10000000000".into()),
        },
    }
}

/// Whether the value in the file (unquoted) already is `value`.
fn same(kind: Kind, old: &str, value: &str) -> bool {
    match kind {
        Kind::Switch => switch_word(old).is_some_and(|on| on == (value == "yes")),
        Kind::Mode => old.eq_ignore_ascii_case(value),
        Kind::AnnounceCap => old.parse::<f64>().ok().zip(value.parse::<f64>().ok()).is_some_and(|(a, b)| a == b),
        Kind::Bitrate => old.parse::<u64>().ok().zip(value.parse::<u64>().ok()).is_some_and(|(a, b)| a == b),
    }
}

/// How `value` is written: a switch in the style of the value it replaces
/// (yes/no, true/false or on/off, in lower, Title or UPPER case), else
/// `Yes`/`No`; anything else as sent.
fn written(kind: Kind, old: Option<&str>, value: &str) -> String {
    if kind != Kind::Switch {
        return value.to_string();
    }
    let on = value == "yes";
    let Some(old) = old.filter(|old| switch_word(old).is_some()) else { return if on { "Yes" } else { "No" }.to_string() };
    let lower = old.to_ascii_lowercase();
    let word = match (lower.as_str(), on) {
        ("true" | "false", true) => "true",
        ("true" | "false", false) => "false",
        ("on" | "off", true) => "on",
        ("on" | "off", false) => "off",
        (_, true) => "yes",
        (_, false) => "no",
    };
    if old == lower {
        word.to_string()
    } else if old == old.to_ascii_uppercase() {
        word.to_ascii_uppercase()
    } else {
        let mut title = word.to_string();
        title[..1].make_ascii_uppercase();
        title
    }
}

/// The file's line endings: `Some(false)` for `\n`, `Some(true)` for `\r\n`
/// throughout, `None` for a mix (or a lone `\r`), which an edit would change.
fn line_endings(text: &str) -> Option<bool> {
    if !text.contains('\r') {
        return Some(false);
    }
    let bytes = text.as_bytes();
    let crlf = bytes.iter().enumerate().all(|(i, &b)| match b {
        b'\r' => bytes.get(i + 1) == Some(&b'\n'),
        b'\n' => i > 0 && bytes[i - 1] == b'\r',
        _ => true,
    });
    crlf.then_some(true)
}

const TOO_BIG: &str = "The Reticulum config is too big to be read whole here, so it can only be changed in the terminal";
const UNREADABLE: &str = "Reticulum can't read the config as it is now (see the node's log), so it can only be changed in the terminal";
const READ_TWO_WAYS: &str = "The change would read differently to Reticulum than asked for, so it can only be made in the terminal";

/// Characters Python's `str.splitlines` (and so ConfigObj, which Python
/// Reticulum reads the file with) takes for line breaks, which rsReticulum
/// and the editor don't: a file with one reads differently to each.
const LINE_BREAKS: &[char] = &['\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}'];

/// Whether every line that starts with `[` is a section header that the
/// editor and the redacted view read alike (`[[Shell # pipe]]`, `[[A]] x`,
/// `[x] = 1` and `[]` are not).
fn headers_agree(doc: &Doc) -> bool {
    doc.bracket_lines().into_iter().all(|(line, header)| match (header, redact::header_of(line)) {
        (Some((depth, name)), Some((seen_depth, seen))) => depth == seen_depth && name == seen,
        _ => false,
    })
}

/// Why the file as a whole can't be edited here, if it can't.
fn whole_file_problem(text: &str, view: &View, doc: &Doc) -> Option<&'static str> {
    // An open value makes the view cut too, so it is said first.
    if doc.has_open_value() {
        Some("The Reticulum config has a triple-quoted value that never ends, so it can only be changed in the terminal")
    } else if view.truncated {
        Some(TOO_BIG)
    } else if line_endings(text).is_none() {
        Some("The Reticulum config mixes line endings, so it can only be changed in the terminal")
    } else if text.contains(LINE_BREAKS) {
        Some(
            "The Reticulum config has characters that Python Reticulum reads as line breaks (and rsReticulum doesn't), so it can only be changed in the terminal",
        )
    } else if text.contains('\u{fffd}') {
        Some("The Reticulum config isn't plain UTF-8 text, so it can only be changed in the terminal")
    } else if !headers_agree(doc) {
        Some("The Reticulum config has a line starting with [ that isn't a plain section header, so it can only be changed in the terminal")
    } else {
        None
    }
}

/// `text` for the node's log, with control and invisible characters written
/// out, so the file can't split a line or colour the terminal.
fn plain(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() || crate::stats::is_invisible(c) {
            shown.extend(c.escape_unicode());
        } else {
            shown.push(c);
        }
    }
    shown
}

/// The config as Reticulum reads it, or `None` (logged) if it can't.
fn parse(text: &str, which: &str) -> Option<Config> {
    match Config::parse(text) {
        Ok(config) => Some(config),
        Err(e) => {
            tracing::warn!("Remote management: Reticulum can't read the config {which}: {}", plain(&e.to_string()));
            None
        }
    }
}

/// The section at `path` as Reticulum reads it.
fn parsed<'a>(config: &'a Config, path: &[&str]) -> Option<&'a ConfigSection> {
    match path {
        ["reticulum"] => config.section("reticulum"),
        ["interfaces", name] => config.subsection("interfaces", name),
        _ => None,
    }
}

fn parsed_mut<'a>(config: &'a mut Config, path: &[&str]) -> Option<&'a mut ConfigSection> {
    match path {
        ["reticulum"] => config.sections.get_mut("reticulum"),
        ["interfaces", name] => config.sections.get_mut("interfaces")?.subsections.get_mut(*name),
        _ => None,
    }
}

/// Whether an interface, as Reticulum reads it, may run a program or has no
/// type: `command`, `location_cmd`, a pipe type, a `reachable_on` that isn't
/// an address, in any case. The same rule as the redacted view's, on the
/// parser's reading, so a file the two read differently is caught.
fn runs_program(section: &ConfigSection) -> bool {
    let typed = section
        .values
        .iter()
        .any(|(key, value)| key.eq_ignore_ascii_case("type") && matches!(value, ConfigValue::Scalar(kind) if !kind.trim().is_empty()));
    !typed
        || section.values.iter().any(|(key, value)| {
            let key = key.to_ascii_lowercase();
            matches!(key.as_str(), "command" | "location_cmd")
                || (key == "type"
                    && match value {
                        ConfigValue::Scalar(kind) => kind.to_ascii_lowercase().contains("pipe"),
                        ConfigValue::List(_) => true,
                    })
                || (redact::REACHABLE.contains(&key.as_str()) && reachable_runs(value))
        })
}

/// Whether two sections hold the same keys, values and subsections.
fn same_section(a: &ConfigSection, b: &ConfigSection) -> bool {
    a.values == b.values
        && a.subsections.len() == b.subsections.len()
        && a.subsections.iter().all(|(name, section)| b.subsections.get(name).is_some_and(|other| same_section(section, other)))
}

fn same_config(a: &Config, b: &Config) -> bool {
    a.sections.len() == b.sections.len()
        && a.sections.iter().all(|(name, section)| b.sections.get(name).is_some_and(|other| same_section(section, other)))
}

/// Why a key in a section is ambiguous, if it is: set twice, under both of
/// its names, in another case, or over several lines.
fn key_problem(doc: &Doc, path: &[&str], rule: &Rule) -> Option<&'static str> {
    let names: Vec<&str> = std::iter::once(rule.key).chain(rule.aliases.iter().copied()).collect();
    match doc.key_lines(path, &names).as_slice() {
        [] => None,
        [one] if one.multiline => Some("Its value is written over several lines, so it can only be changed in the terminal"),
        [one] if !names.contains(&one.key.as_str()) => Some("It is written in another case, so it can only be changed in the terminal"),
        [_] => None,
        _ => Some("It is set more than once (or under both of its names), so it can only be changed in the terminal"),
    }
}

/// Whether the section at `path` (in `view`) can be changed from afar: it is
/// there once, spelled only one way, isn't restricted, and none of the keys
/// that can be changed in it is ambiguous.
fn section_editable(doc: &Doc, config: &Config, section: &redact::Section, place: Place) -> bool {
    let path: Vec<&str> = section.path.iter().map(String::as_str).collect();
    doc.section_count(&path) == (1, 1)
        && !section.restricted
        && parsed(config, &path).is_some_and(|found| place == Place::Reticulum || !runs_program(found))
        && RULES.iter().filter(|rule| rule.place == place).all(|rule| key_problem(doc, &path, rule).is_none())
}

/// The sections of `text` (whose redacted view is `view`) that can be
/// changed from afar: `["reticulum"]` first, then at most 32 interfaces.
/// None if the file as a whole can't be (see [`plan`]).
pub fn editable(text: &str, view: &View) -> Vec<Vec<String>> {
    let doc = Doc::new(text);
    if whole_file_problem(text, view, &doc).is_some() {
        return Vec::new();
    }
    let Ok(config) = Config::parse(text) else { return Vec::new() };
    let reticulum =
        view.sections.iter().filter(|s| s.path == ["reticulum"]).filter(|s| section_editable(&doc, &config, s, Place::Reticulum));
    let interfaces = view
        .sections
        .iter()
        .filter(|s| matches!(s.path.as_slice(), [first, name] if first == "interfaces" && interface_name_ok(name)))
        .filter(|s| section_editable(&doc, &config, s, Place::Interface))
        .take(MAX_EDITABLE_INTERFACES);
    reticulum.chain(interfaces).map(|s| s.path.clone()).collect()
}

/// The refusal of `changes` to a file of which only the start, `start`, could
/// be read (it is over the 1 MiB read): stale if `revision` isn't the one
/// `/state` gives for it, else ambiguous, as it can't be saved whole.
pub fn too_big(start: &str, revision: &str, changes: &[RnsChange]) -> RnsRefusal {
    let current = redact::view_of(start, true).revision();
    if current != revision {
        return RnsRefusal::Stale { revision: current };
    }
    let path = changes.first().map(|c| c.path.clone()).unwrap_or_default();
    RnsRefusal::Ambiguous { path, key: None, message: TOO_BIG.into() }
}

/// What turning on the interface `name` (as Reticulum reads it after the
/// change) opens, in sentences that quote only values of the shape the view
/// shows.
fn opens(name: &str, section: &ConfigSection) -> Vec<String> {
    let mut said = Vec::new();
    let shown = |key: &str| section.get(key).and_then(|value| redact::shown_value(key, value));
    if let Some(port) = shown("listen_port") {
        let at = match shown("listen_ip") {
            Some(ip) if ip.contains(':') => format!("[{ip}]:{port}"),
            Some(ip) => format!("{ip}:{port}"),
            None => format!("port {port}"),
        };
        said.push(format!("{name}: This opens {at} to anyone who can reach it"));
    }
    if section.get_bool("discoverable") == Some(true) {
        said.push(format!("{name}: This interface announces its address"));
    }
    if said.is_empty() {
        said.push(format!("{name}: This interface is turned on"));
    }
    said
}

/// The notes for the answer: rettui's own warnings about the config after the
/// change, and for each interface it changes that Reticulum may not start, a
/// fixed sentence (why goes to the node's log, as it may quote the file).
fn warnings(after: &Config, applied: &[Change]) -> Vec<String> {
    let mut notes: Vec<String> = enable_warnings(after);
    notes.extend(bootstrap_warnings(after));
    notes.extend(listener_warning(after));
    let mut seen: Vec<&str> = Vec::new();
    for change in applied {
        let [_, name] = change.path.as_slice() else { continue };
        if seen.contains(&name.as_str()) {
            continue;
        }
        seen.push(name);
        let Some(section) = after.subsection("interfaces", name) else { continue };
        match synthesize_interface(name, section) {
            Ok(_) => {}
            // Switched off in the file.
            Err(InterfaceFactoryError::Disabled(n)) if n == *name => {}
            Err(e) => {
                tracing::warn!("Remote management: {}: this interface may not start: {}", plain(name), plain(&e.to_string()));
                notes.push(format!("{name}: this interface may not start (see the node's log)"));
            }
        }
    }
    notes
}

/// Work out `changes` to the config `before`, read at `revision` (the
/// redacted view's): the new text, or why not. Each change is one line
/// changed or added, and nothing else in the file moves. With
/// `allow_enable`, a change may turn an interface or transport on.
pub fn plan(before: &str, revision: &str, changes: &[RnsChange], allow_enable: bool) -> Result<Planned, RnsRefusal> {
    let view = redact::redact(before);
    let current = view.revision();
    if current != revision {
        return Err(RnsRefusal::Stale { revision: current });
    }
    let first = changes.first().map(|c| c.path.clone()).unwrap_or_default();
    let whole = |message: &str| RnsRefusal::Ambiguous { path: first.clone(), key: None, message: message.to_string() };
    let mut doc = Doc::new(before);
    if let Some(problem) = whole_file_problem(before, &view, &doc) {
        return Err(whole(problem));
    }
    let Some(old) = parse(before, "as it is") else { return Err(whole(UNREADABLE)) };
    let crlf = line_endings(before) == Some(true);
    let mut applied = Vec::new();
    let mut disabled = Vec::new();
    let mut turned_on = Vec::new();
    // Each change as Reticulum should read it: the section, the key's name
    // in the file, and the value written.
    let mut expected: Vec<(Vec<&str>, String, String)> = Vec::new();
    for change in changes {
        let path: Vec<&str> = change.path.iter().map(String::as_str).collect();
        let refuse = |message: String| RnsRefusal::NotAllowed { path: change.path.clone(), key: change.key.clone(), message };
        let Some(place) = place_of(&path) else {
            return Err(refuse("Only [reticulum] and the interfaces' own sections can be changed from afar".into()));
        };
        let Some(rule) = RULES.iter().find(|rule| rule.place == place && rule.key == change.key) else {
            let keys: Vec<&str> = RULES.iter().filter(|rule| rule.place == place).map(|rule| rule.key).collect();
            return Err(refuse(format!("This key can't be changed from afar; here only {} can", keys.join(", "))));
        };
        check_value(rule.kind, &change.value).map_err(|message| RnsRefusal::Invalid {
            path: change.path.clone(),
            key: change.key.clone(),
            message,
        })?;
        match doc.section_count(&path) {
            (0, 0) => return Err(RnsRefusal::UnknownSection { path: change.path.clone() }),
            (1, 1) => {}
            _ => {
                let message =
                    "There is more than one section of this name (perhaps spelled another way), so it can only be changed in the terminal";
                return Err(RnsRefusal::Ambiguous { path: change.path.clone(), key: None, message: message.into() });
            }
        }
        let Some(section) = parsed(&old, &path) else {
            return Err(RnsRefusal::Ambiguous { path: change.path.clone(), key: None, message: READ_TWO_WAYS.into() });
        };
        if place == Place::Interface {
            let restricted = match view.sections.iter().find(|s| s.path == change.path) {
                None => return Err(RnsRefusal::UnknownSection { path: change.path.clone() }),
                Some(shown) => shown.restricted || runs_program(section),
            };
            if restricted {
                return Err(refuse("This interface runs a program or has no type, so it can only be changed in the terminal".into()));
            }
        }
        if let Some(problem) = key_problem(&doc, &path, rule) {
            return Err(RnsRefusal::Ambiguous { path: change.path.clone(), key: Some(change.key.clone()), message: problem.into() });
        }
        let names: Vec<&str> = std::iter::once(rule.key).chain(rule.aliases.iter().copied()).collect();
        let existing = doc.key_lines(&path, &names).into_iter().next();
        let was = existing.as_ref().map(|line| unquote(&line.raw));
        if was.as_deref().is_some_and(|was| same(rule.kind, was, &change.value)) {
            continue;
        }
        let turns_on = matches!(rule.key, "enabled" | "enable_transport") && change.value == "yes";
        if turns_on && !allow_enable {
            return Err(refuse(ENABLE_OFF.into()));
        }
        let text = written(rule.kind, was.as_deref(), &change.value);
        doc.set(&path, rule.key, rule.aliases, Some(&text));
        // The old value only if the view would show it.
        let from = match &existing {
            Some(line) if was.as_deref().is_some_and(|was| !was.is_empty()) => {
                redact::shown_value(&line.key, &line.raw).unwrap_or_else(|| "(hidden)".to_string())
            }
            _ => String::new(),
        };
        applied.push(Change { path: change.path.clone(), key: change.key.clone(), from, to: change.value.clone() });
        expected.push((path.clone(), existing.map_or_else(|| rule.key.to_string(), |line| line.key), text));
        if rule.key == "enabled"
            && change.value == "no"
            && let [_, name] = path.as_slice()
        {
            disabled.push(name.to_string());
        }
        if turns_on {
            turned_on.push(path.clone());
        }
    }
    let Some(last) = applied.last() else { return Err(RnsRefusal::NoChange) };
    let mut after = doc.text();
    if crlf {
        after = after.replace('\n', "\r\n");
    }
    // Each change is one line changed or added: anything more is a bug, or a
    // file read two ways, and isn't saved.
    let (added, removed) = history::counts(&history::diff(before, &after, 0));
    if added > applied.len() || removed > applied.len() {
        return Err(whole("The change would touch more of the file than asked for, so it can only be made in the terminal"));
    }
    validate(before, &after, true).map_err(|message| {
        let message = if message == RUNS_PROGRAMS {
            message
        } else {
            tracing::warn!("Remote management: a Reticulum config change wasn't saved: {}", plain(&message));
            LOAD_FAILED.to_string()
        };
        RnsRefusal::Invalid { path: last.path.clone(), key: last.key.clone(), message }
    })?;
    // Reticulum must read the new file as the old one with only the keys
    // asked for changed: the editor and the parser read it alike.
    let Some(new) = parse(&after, "with the change") else { return Err(whole(READ_TWO_WAYS)) };
    let mut wanted = old.clone();
    for (path, key, value) in &expected {
        let Some(section) = parsed_mut(&mut wanted, path) else { return Err(whole(READ_TWO_WAYS)) };
        section.values.insert(key.clone(), ConfigValue::Scalar(value.clone()));
    }
    if !same_config(&wanted, &new) {
        tracing::warn!("Remote management: a Reticulum config change would have read differently than asked for, so it wasn't saved");
        return Err(whole(READ_TWO_WAYS));
    }
    let mut said = Vec::new();
    for path in &turned_on {
        match path.as_slice() {
            ["reticulum"] => said.push("This node will relay traffic for others".to_string()),
            ["interfaces", name] => said.extend(new.subsection("interfaces", name).map(|section| opens(name, section)).unwrap_or_default()),
            _ => {}
        }
    }
    let notes = said
        .iter()
        .cloned()
        .chain(warnings(&new, &applied))
        .take(MAX_NOTES)
        .map(|note| plain(&note).chars().take(MAX_NOTE).collect())
        .collect();
    Ok(Planned { after, applied, disabled, turned_on: said, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reticulum::redact::redact;

    const CONFIG: &str = "\
# The node's config
[reticulum]
  enable_transport = False   # not a router yet
  share_instance = No
  instance_name = secret-instance-name

[logging]
  loglevel = 4

[interfaces]
  # The public entry point
  [[Hilltop TCP]]
    type = TCPServerInterface
    enabled = yes
    listen_ip = 0.0.0.0
    listen_port = 4242
    passphrase = hunter2-secret
    mode = gateway

  [[Radio]]
    type = RNodeInterface
    interface_enabled = On
    port = /dev/ttyUSB0
    bitrate = 1200
    announce_cap = 2.5
    target_host = user:secret@host

  [[Shell]]
    type = PipeInterface
    enabled = no
    command = /usr/bin/secret-script

  [[Wanderer]]
    type = TCPClientInterface
    enabled = no
    location_cmd = /usr/bin/where
    target_host = hub.example.org

  [[Bare]]
    enabled = yes
";

    fn change(path: &[&str], key: &str, value: &str) -> RnsChange {
        RnsChange { path: path.iter().map(|s| s.to_string()).collect(), key: key.into(), value: value.into() }
    }

    fn rev(text: &str) -> String {
        redact(text).revision()
    }

    fn planned(text: &str, changes: &[RnsChange]) -> Result<Planned, RnsRefusal> {
        plan(text, &rev(text), changes, true)
    }

    const HILL: &[&str] = &["interfaces", "Hilltop TCP"];
    const RADIO: &[&str] = &["interfaces", "Radio"];

    /// The lines that differ, (before, after), position by position.
    fn changed_lines(before: &str, after: &str) -> Vec<(String, String)> {
        let (a, b): (Vec<&str>, Vec<&str>) = (before.split('\n').collect(), after.split('\n').collect());
        assert_eq!(a.len(), b.len(), "the same number of lines");
        a.iter().zip(&b).filter(|(x, y)| x != y).map(|(x, y)| (x.to_string(), y.to_string())).collect()
    }

    #[test]
    fn each_key_is_changed_in_place_and_nothing_else_moves() {
        let cases: &[(&[&str], &str, &str, &str, &str)] = &[
            (
                &["reticulum"],
                "enable_transport",
                "yes",
                "  enable_transport = False   # not a router yet",
                "  enable_transport = True # not a router yet",
            ),
            (HILL, "enabled", "no", "    enabled = yes", "    enabled = no"),
            (HILL, "mode", "boundary", "    mode = gateway", "    mode = boundary"),
            (RADIO, "enabled", "no", "    interface_enabled = On", "    interface_enabled = Off"),
            (RADIO, "bitrate", "62500", "    bitrate = 1200", "    bitrate = 62500"),
            (RADIO, "announce_cap", "100", "    announce_cap = 2.5", "    announce_cap = 100"),
        ];
        for (path, key, value, from, to) in cases {
            let plan = planned(CONFIG, &[change(path, key, value)]).unwrap();
            assert_eq!(changed_lines(CONFIG, &plan.after), [(from.to_string(), to.to_string())], "{key}");
            assert_eq!(plan.applied.len(), 1);
            assert_eq!(plan.applied[0].to, *value);
            // Secrets, comments and other keys are byte for byte as they were.
            for kept in ["hunter2-secret", "secret-instance-name", "# The public entry point", "/usr/bin/secret-script"] {
                assert!(plan.after.contains(kept));
            }
            assert!(rns_runtime::config::Config::parse(&plan.after).is_ok());
        }
    }

    #[test]
    fn the_old_value_is_given_only_if_the_view_would_show_it() {
        let plan = planned(CONFIG, &[change(HILL, "enabled", "no"), change(RADIO, "mode", "roaming")]).unwrap();
        assert_eq!(
            plan.applied,
            [
                Change {
                    path: vec!["interfaces".into(), "Hilltop TCP".into()],
                    key: "enabled".into(),
                    from: "yes".into(),
                    to: "no".into()
                },
                Change { path: vec!["interfaces".into(), "Radio".into()], key: "mode".into(), from: "".into(), to: "roaming".into() },
            ]
        );
        assert_eq!(plan.disabled, ["Hilltop TCP"]);
        // A value set but not of the key's shape is hidden.
        let text = "[interfaces]\n[[A]]\ntype = TCPClientInterface\nbitrate = 'fast'\n";
        let plan = planned(text, &[change(&["interfaces", "A"], "bitrate", "1000")]).unwrap();
        assert_eq!(plan.applied[0].from, "(hidden)");
        // Set but empty counts as unset.
        let text = "[interfaces]\n[[A]]\ntype = TCPClientInterface\nmode =\n";
        assert_eq!(planned(text, &[change(&["interfaces", "A"], "mode", "full")]).unwrap().applied[0].from, "");
    }

    #[test]
    fn a_missing_key_is_added_to_its_section() {
        let plan = planned(CONFIG, &[change(HILL, "ingress_control", "no"), change(HILL, "announce_cap", "0.5")]).unwrap();
        assert!(
            plan.after.contains("    mode = gateway\n    ingress_control = No\n    announce_cap = 0.5\n\n  [[Radio]]"),
            "{}",
            plan.after
        );
        let (added, removed) = history::counts(&history::diff(CONFIG, &plan.after, 0));
        assert_eq!((added, removed), (2, 0));
        let parsed = rns_runtime::config::Config::parse(&plan.after).unwrap();
        assert_eq!(parsed.subsection("interfaces", "Hilltop TCP").unwrap().get("ingress_control"), Some("No"));
    }

    #[test]
    fn switches_keep_the_style_of_the_file() {
        for (old, new, expect) in [
            ("yes", "no", "no"),
            ("Yes", "no", "No"),
            ("YES", "no", "NO"),
            ("true", "no", "false"),
            ("False", "yes", "True"),
            ("FALSE", "yes", "TRUE"),
            ("on", "no", "off"),
            ("Off", "yes", "On"),
            ("1", "no", "No"),
            ("maybe", "yes", "Yes"),
        ] {
            let text = format!("[reticulum]\nenable_transport = {old}\n");
            let plan = planned(&text, &[change(&["reticulum"], "enable_transport", new)]).unwrap();
            assert_eq!(plan.after, format!("[reticulum]\nenable_transport = {expect}\n"), "{old} -> {new}");
        }
        // A quoted value loses its quotes.
        let plan = planned("[reticulum]\nenable_transport = 'no'\n", &[change(&["reticulum"], "enable_transport", "yes")]).unwrap();
        assert_eq!(plan.after, "[reticulum]\nenable_transport = yes\n");
    }

    #[test]
    fn the_same_value_is_no_change() {
        for (path, key, value) in [
            (&["reticulum"][..], "enable_transport", "no"),
            (HILL, "enabled", "yes"),
            (HILL, "mode", "gateway"),
            (RADIO, "enabled", "yes"),
            (RADIO, "bitrate", "1200"),
            (RADIO, "announce_cap", "2.50"),
        ] {
            assert_eq!(planned(CONFIG, &[change(path, key, value)]), Err(RnsRefusal::NoChange), "{key}");
        }
        let text = "[interfaces]\n[[A]]\ntype = X\nmode = Gateway\n";
        assert_eq!(planned(text, &[change(&["interfaces", "A"], "mode", "gateway")]), Err(RnsRefusal::NoChange));
        // One the same and one not: only the other is applied.
        let plan = planned(CONFIG, &[change(HILL, "enabled", "yes"), change(HILL, "mode", "full")]).unwrap();
        assert_eq!(plan.applied.len(), 1);
    }

    #[test]
    fn values_are_checked() {
        let bad: &[(&[&str], &str, &str)] = &[
            (&["reticulum"], "enable_transport", "Yes"),
            (&["reticulum"], "enable_transport", "true"),
            (HILL, "enabled", "1"),
            (HILL, "ingress_control", ""),
            (HILL, "mode", "Gateway"),
            (HILL, "mode", "ap"),
            (HILL, "announce_cap", "0"),
            (HILL, "announce_cap", "0.0"),
            (HILL, "announce_cap", "100.5"),
            (HILL, "announce_cap", "101"),
            (HILL, "announce_cap", "05"),
            (HILL, "announce_cap", "2.555"),
            (HILL, "announce_cap", ".5"),
            (HILL, "announce_cap", "5."),
            (HILL, "announce_cap", "-1"),
            (HILL, "announce_cap", "1e2"),
            (HILL, "announce_cap", "NaN"),
            (HILL, "bitrate", "4"),
            (HILL, "bitrate", "0"),
            (HILL, "bitrate", "010"),
            (HILL, "bitrate", "10000000001"),
            (HILL, "bitrate", "99999999999"),
            (HILL, "bitrate", "1.5"),
            (HILL, "bitrate", "+100"),
        ];
        for (path, key, value) in bad {
            let refused = planned(CONFIG, &[change(path, key, value)]);
            assert!(matches!(&refused, Err(RnsRefusal::Invalid { key: k, .. }) if k == key), "{key} = {value}: {refused:?}");
        }
        let good: &[(&str, &str)] = &[
            ("announce_cap", "0.01"),
            ("announce_cap", "0.5"),
            ("announce_cap", "7"),
            ("announce_cap", "99.99"),
            ("announce_cap", "100"),
            ("announce_cap", "100.0"),
            ("bitrate", "5"),
            ("bitrate", "10000000000"),
            ("mode", "full"),
            ("mode", "access_point"),
            ("mode", "roaming"),
        ];
        for (key, value) in good {
            assert!(planned(CONFIG, &[change(HILL, key, value)]).is_ok(), "{key} = {value}");
        }
    }

    #[test]
    fn keys_off_the_list_and_restricted_interfaces_are_not_allowed() {
        for (path, key) in [
            (&["reticulum"][..], "share_instance"),
            (&["reticulum"], "enabled"),
            (HILL, "enable_transport"),
            (HILL, "interface_enabled"),
            (HILL, "listen_port"),
            (HILL, "passphrase"),
            (HILL, "type"),
            (&["logging"], "loglevel"),
            (&["interfaces"], "enabled"),
        ] {
            let refused = planned(CONFIG, &[change(path, key, "yes")]);
            assert!(matches!(refused, Err(RnsRefusal::NotAllowed { .. })), "{path:?} {key}: {refused:?}");
        }
        // A pipe, a location command, no type: nothing at all, not even turning it on.
        for name in ["Shell", "Wanderer", "Bare"] {
            for (key, value) in [("enabled", "yes"), ("enabled", "no"), ("mode", "full")] {
                let refused = planned(CONFIG, &[change(&["interfaces", name], key, value)]);
                assert!(
                    matches!(&refused, Err(RnsRefusal::NotAllowed { message, .. }) if message.contains("runs a program")),
                    "{name} {key}: {refused:?}"
                );
            }
        }
    }

    #[test]
    fn unknown_sections_and_ambiguous_ones_are_refused() {
        assert_eq!(
            planned(CONFIG, &[change(&["interfaces", "Nowhere"], "enabled", "no")]),
            Err(RnsRefusal::UnknownSection { path: vec!["interfaces".into(), "Nowhere".into()] })
        );
        assert!(matches!(
            planned("[logging]\nloglevel = 4\n", &[change(&["reticulum"], "enable_transport", "yes")]),
            Err(RnsRefusal::UnknownSection { .. })
        ));
        let ambiguous = |text: &str, change: RnsChange| {
            let refused = planned(text, &[change]);
            assert!(matches!(refused, Err(RnsRefusal::Ambiguous { .. })), "{text}: {refused:?}");
        };
        let a = &["interfaces", "A"][..];
        // The same section twice, or in another case or spacing.
        ambiguous("[interfaces]\n[[A]]\ntype = X\n[[A]]\nmode = full\n", change(a, "mode", "gateway"));
        ambiguous("[interfaces]\n[[A]]\ntype = X\n[[a]]\ntype = X\n", change(a, "mode", "gateway"));
        ambiguous("[interfaces]\n[[Hill top]]\ntype = X\n[[Hilltop]]\ntype = X\n", change(&["interfaces", "Hilltop"], "mode", "full"));
        ambiguous("[reticulum]\nenable_transport = no\n[Reticulum]\n", change(&["reticulum"], "enable_transport", "yes"));
        ambiguous("[reticulum]\nenable_transport = no\n[reticulum]\n", change(&["reticulum"], "enable_transport", "yes"));
        // Only a case variant of the section: not the one asked for, but not unknown either.
        ambiguous("[interfaces]\n[[a]]\ntype = X\n", change(a, "mode", "gateway"));
        // The key twice, in another case, both names, or over several lines.
        ambiguous("[interfaces]\n[[A]]\ntype = X\nmode = full\nmode = full\n", change(a, "mode", "gateway"));
        ambiguous("[interfaces]\n[[A]]\ntype = X\nMode = full\n", change(a, "mode", "gateway"));
        ambiguous("[interfaces]\n[[A]]\ntype = X\nmode = full\ninterface_mode = full\n", change(a, "mode", "gateway"));
        ambiguous("[interfaces]\n[[A]]\ntype = X\nenabled = no\ninterface_enabled = no\n", change(a, "enabled", "yes"));
        ambiguous("[interfaces]\n[[A]]\ntype = X\nmode = \"\"\"full\n\"\"\"\n", change(a, "mode", "gateway"));
        ambiguous("[interfaces]\n[[A]]\ntype = X\nmode = '''full'''\n", change(a, "mode", "gateway"));
        // A value that never ends.
        ambiguous("[interfaces]\n[[A]]\ntype = X\nnotes = '''\nmode = full\n", change(a, "mode", "gateway"));
        // Mixed line endings, or bytes that weren't text.
        ambiguous("[interfaces]\r\n[[A]]\ntype = X\n", change(a, "mode", "gateway"));
        ambiguous("[interfaces]\n[[A]]\ntype = X\r\n\r", change(a, "mode", "gateway"));
        ambiguous("[interfaces]\n[[A]]\ntype = X\nname = \u{fffd}\n", change(a, "mode", "gateway"));
        // Another key over several lines doesn't matter.
        let plan = planned("[interfaces]\n[[A]]\ntype = X\nnotes = '''\nmode = x\n'''\n", &[change(a, "mode", "full")]).unwrap();
        assert_eq!(plan.after, "[interfaces]\n[[A]]\ntype = X\nnotes = '''\nmode = x\n'''\nmode = full\n");
    }

    #[test]
    fn windows_line_endings_are_kept() {
        let text = "[reticulum]\r\n  enable_transport = no\r\n[interfaces]\r\n  [[A]]\r\n    type = X\r\n";
        let plan =
            planned(text, &[change(&["reticulum"], "enable_transport", "yes"), change(&["interfaces", "A"], "mode", "full")]).unwrap();
        assert_eq!(plan.after, "[reticulum]\r\n  enable_transport = yes\r\n[interfaces]\r\n  [[A]]\r\n    type = X\r\n    mode = full\r\n");
    }

    #[test]
    fn a_truncated_view_or_a_stale_revision_is_refused() {
        let big = format!("{CONFIG}{}", (0..200).map(|i| format!("[s{i}]\n")).collect::<String>());
        let refused = planned(&big, &[change(HILL, "enabled", "no")]);
        assert!(matches!(&refused, Err(RnsRefusal::Ambiguous { message, .. }) if message.contains("too big")), "{refused:?}");
        // The start of a file that went on.
        let mut view = redact(CONFIG);
        view.truncated = true;
        let refused = too_big(CONFIG, &view.revision(), &[change(HILL, "enabled", "no")]);
        assert!(
            matches!(&refused, RnsRefusal::Ambiguous { message, path, .. } if message.contains("too big") && path == HILL),
            "{refused:?}"
        );
        // Read before a change: refused with the revision now.
        let stale = plan(CONFIG, &"0".repeat(32), &[change(HILL, "enabled", "no")], true);
        assert_eq!(stale, Err(RnsRefusal::Stale { revision: rev(CONFIG) }));
        // A capped file with the uncapped revision is stale, not ambiguous.
        assert_eq!(too_big(CONFIG, &rev(CONFIG), &[change(HILL, "enabled", "no")]), RnsRefusal::Stale { revision: view.revision() });
    }

    #[test]
    fn the_first_refusal_wins_and_nothing_is_half_applied() {
        let refused = planned(CONFIG, &[change(HILL, "enabled", "no"), change(HILL, "passphrase", "x")]);
        assert!(matches!(refused, Err(RnsRefusal::NotAllowed { key, .. }) if key == "passphrase"));
        let refused = planned(CONFIG, &[change(&["interfaces", "Gone"], "enabled", "no"), change(HILL, "mode", "nope")]);
        assert!(matches!(refused, Err(RnsRefusal::UnknownSection { .. })));
    }

    #[test]
    fn a_change_the_config_check_refuses_is_invalid() {
        // A config Reticulum can't load (a port that is a word) isn't saved, though the change itself is fine.
        let text = "[reticulum]\nenable_transport = no\nshared_instance_port = notaport\n";
        let refused = planned(text, &[change(&["reticulum"], "enable_transport", "yes")]);
        assert!(
            matches!(&refused, Err(RnsRefusal::Invalid { key, path, message }) if key == "enable_transport" && path == &["reticulum"] && message == LOAD_FAILED),
            "{refused:?}"
        );
        // Warnings about interfaces that won't start are notes.
        let text = "[interfaces]\n[[A]]\ntype = TCPClientInterface\nenabled = no\n";
        let plan = planned(text, &[change(&["interfaces", "A"], "enabled", "yes")]).unwrap();
        assert!(!plan.notes.is_empty() && plan.notes.iter().all(|n| n.chars().count() <= MAX_NOTE), "{:?}", plan.notes);
    }

    #[test]
    fn sixteen_changes_fit() {
        let mut text = String::from("[interfaces]\n");
        for i in 0..16 {
            text.push_str(&format!("[[I{i}]]\ntype = TCPClientInterface\nenabled = no\n"));
        }
        let changes: Vec<RnsChange> = (0..MAX_RNS_CHANGES).map(|i| change(&["interfaces", &format!("I{i}")], "mode", "full")).collect();
        let plan = planned(&text, &changes).unwrap();
        assert_eq!(plan.applied.len(), 16);
        assert_eq!(history::counts(&history::diff(&text, &plan.after, 0)), (16, 0));
    }

    #[test]
    fn interface_names_in_requests_are_plain() {
        for fine in ["A", "Hilltop TCP", "Ünïcode name", &"x".repeat(64), "RNode (LoRa) / 868"] {
            assert!(interface_name_ok(fine), "{fine}");
        }
        for bad in
            ["", " A", "A ", &"x".repeat(65), "a[b", "a]b", "a#b", "a\"b", "a'b", "a\nb", "a\tb", "a\u{1b}b", "a\u{200b}b", "a\u{202e}b"]
        {
            assert!(!interface_name_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_editable_sections_are_those_a_change_could_go_to() {
        let view = redact(CONFIG);
        let paths = editable(CONFIG, &view);
        let shown: Vec<String> = paths.iter().map(|p| p.join("/")).collect();
        assert_eq!(shown, ["reticulum", "interfaces/Hilltop TCP", "interfaces/Radio"]);
        // Not part of the revision.
        assert_eq!(redact(CONFIG).revision(), view.revision());
        assert_eq!(view.to_json_with(false, &paths)["editable"], serde_json::json!(paths));
        assert_eq!(view.to_json_with(false, &[])["editable"], serde_json::json!([]));
        // Ambiguous ones are left out; a file that can't be edited has none.
        let text = "[reticulum]\nenable_transport = no\nEnable_Transport = no\n[interfaces]\n[[A]]\ntype = X\n[[B]]\ntype = X\nmode = a\nMODE = b\n[[C]]\ntype = X\n[[c]]\ntype = Y\n";
        assert_eq!(editable(text, &redact(text)), [vec!["interfaces".to_string(), "A".into()]]);
        // A file Reticulum can't read has none.
        let twice = "[reticulum]\nenable_transport = no\nenable_transport = no\n[interfaces]\n[[A]]\ntype = X\n";
        assert!(editable(twice, &redact(twice)).is_empty());
        let open = "[reticulum]\n[interfaces]\n[[A]]\ntype = X\nnotes = '''\n";
        assert!(editable(open, &redact(open)).is_empty());
        let big = format!("{CONFIG}{}", (0..200).map(|i| format!("[s{i}]\n")).collect::<String>());
        assert!(editable(&big, &redact(&big)).is_empty());
        // At most 32 interfaces; names a request can't carry are left out.
        let mut many = String::from("[reticulum]\n[interfaces]\n[[a]b]]\ntype = X\n");
        for i in 0..40 {
            many.push_str(&format!("[[I{i:02}]]\ntype = X\n"));
        }
        let paths = editable(&many, &redact(&many));
        assert_eq!(paths.len(), 33);
        assert_eq!(paths[0], ["reticulum"]);
        assert!(paths.iter().all(|p| p.last().is_some_and(|n| n != "a]b")));
        // No [reticulum]: only interfaces.
        assert_eq!(editable("[interfaces]\n[[A]]\ntype = X\n", &redact("[interfaces]\n[[A]]\ntype = X\n")).len(), 1);
    }

    /// A config with an interface that is off: a TCP server on 0.0.0.0:4242
    /// that announces itself.
    const OFF: &str = "\
[reticulum]
  enable_transport = no

[interfaces]
  [[Hilltop TCP]]
    type = TCPServerInterface
    enabled = no
    listen_ip = 0.0.0.0
    listen_port = 4242
    discoverable = yes
    reachable_on = node.example.org
";

    #[test]
    fn turning_things_on_needs_the_node_to_allow_it() {
        let without = |changes: &[RnsChange]| plan(OFF, &rev(OFF), changes, false);
        for refused in [change(HILL, "enabled", "yes"), change(&["reticulum"], "enable_transport", "yes")] {
            let answer = without(std::slice::from_ref(&refused));
            assert!(
                matches!(&answer, Err(RnsRefusal::NotAllowed { message, key, .. }) if message == ENABLE_OFF && *key == refused.key),
                "{answer:?}"
            );
        }
        // Even with other changes alongside.
        assert!(matches!(without(&[change(HILL, "mode", "full"), change(HILL, "enabled", "yes")]), Err(RnsRefusal::NotAllowed { .. })));
        // The rest is allowed, and turning off too.
        for fine in [
            change(HILL, "mode", "boundary"),
            change(HILL, "announce_cap", "5"),
            change(HILL, "bitrate", "1000"),
            change(HILL, "ingress_control", "no"),
        ] {
            let plan = without(std::slice::from_ref(&fine)).unwrap();
            assert!(plan.turned_on.is_empty(), "{plan:?}");
        }
        let off = plan(CONFIG, &rev(CONFIG), &[change(HILL, "enabled", "no")], false).unwrap();
        assert_eq!(off.disabled, ["Hilltop TCP"]);
        // Already on is no change, not a refusal.
        assert_eq!(plan(CONFIG, &rev(CONFIG), &[change(HILL, "enabled", "yes")], false), Err(RnsRefusal::NoChange));
        // Allowed: the answer says what it opens.
        let plan = planned(OFF, &[change(HILL, "enabled", "yes"), change(&["reticulum"], "enable_transport", "yes")]).unwrap();
        let said = [
            "Hilltop TCP: This opens 0.0.0.0:4242 to anyone who can reach it",
            "Hilltop TCP: This interface announces its address",
            "This node will relay traffic for others",
        ];
        assert_eq!(plan.turned_on, said);
        assert_eq!(plan.notes[..3], said);
    }

    #[test]
    fn what_a_turn_on_opens_quotes_only_shaped_values() {
        let text = "[interfaces]\n[[S]]\ntype = TCPServerInterface\nenabled = no\nlisten_ip = secret-ip\nlisten_port = 4242\n\
                    [[V6]]\ntype = TCPServerInterface\nenabled = no\nlisten_ip = ::\nlisten_port = 4243\n\
                    [[R]]\ntype = TCPClientInterface\nenabled = no\ntarget_host = hub.example.org\ntarget_port = 4242\n";
        let on = |name: &str| change(&["interfaces", name], "enabled", "yes");
        let plan = planned(text, &[on("S"), on("V6"), on("R")]).unwrap();
        assert_eq!(
            plan.turned_on,
            [
                "S: This opens port 4242 to anyone who can reach it",
                "V6: This opens [::]:4243 to anyone who can reach it",
                "R: This interface is turned on"
            ]
        );
        assert!(!plan.notes.iter().any(|n| n.contains("secret-ip")));
    }

    #[test]
    fn nothing_reticulum_says_reaches_the_answer() {
        // A value Reticulum can't start the interface with, holding a secret.
        let text = "[interfaces]\n[[A]]\ntype = TCPClientInterface\nenabled = no\ntarget_host = hub.example.org\ntarget_port = hunter2-port-secret\n";
        let plan = planned(text, &[change(&["interfaces", "A"], "enabled", "yes")]).unwrap();
        assert!(plan.notes.contains(&"A: this interface may not start (see the node's log)".to_string()), "{:?}", plan.notes);
        assert!(!plan.notes.iter().chain(&plan.turned_on).any(|n| n.contains("hunter2")), "{:?}", plan.notes);
        // One the change leaves off says nothing.
        let plan = planned(text, &[change(&["interfaces", "A"], "mode", "full")]).unwrap();
        assert!(plan.notes.iter().all(|n| !n.contains("may not start")), "{:?}", plan.notes);
        // A config Reticulum can't load says only that.
        let text = "[reticulum]\nenable_transport = no\nshared_instance_port = hunter2-secret\n";
        let refused = format!("{:?}", planned(text, &[change(&["reticulum"], "enable_transport", "yes")]));
        assert!(refused.contains(LOAD_FAILED) && !refused.contains("hunter2"), "{refused}");
        // A file Reticulum can't read at all.
        let text = "[reticulum]\nenable_transport = no\n[interfaces]\n[[A]]\ntype = X\nk = 1\nk = hunter2\n";
        let refused = format!("{:?}", planned(text, &[change(&["reticulum"], "enable_transport", "yes")]));
        assert!(refused.contains("can't read the config") && !refused.contains("hunter2"), "{refused}");
    }

    #[test]
    fn lines_that_look_like_headers_but_arent_are_refused() {
        let base = "[reticulum]\nenable_transport = no\n[interfaces]\n[[A]]\ntype = TCPClientInterface\nenabled = no\n";
        for odd in [
            "[[Shell # pipe]]\ntype = PipeInterface\ncommand = /bin/x\n",
            "[[B]] x\n",
            "[[B]\n",
            "[]\n",
            "[x] = 1\n",
            "[[ 'B ' ]]\ntype = X\n",
            "[[\u{1b}]]\ntype = X\n",
        ] {
            let text = format!("{base}{odd}");
            for asked in [change(&["reticulum"], "enable_transport", "yes"), change(&["interfaces", "A"], "mode", "full")] {
                let refused = planned(&text, &[asked]);
                assert!(
                    matches!(&refused, Err(RnsRefusal::Ambiguous { message, .. }) if message.contains("section header")),
                    "{odd:?}: {refused:?}"
                );
            }
            assert!(editable(&text, &redact(&text)).is_empty(), "{odd:?}");
        }
        // A bracket inside a value over several lines, or in a comment, is fine.
        let text = format!("{base}notes = '''\n[[not a header\n'''\n# [[nor this\n");
        assert!(planned(&text, &[change(&["interfaces", "A"], "mode", "full")]).is_ok());
    }

    #[test]
    fn the_parser_must_agree_with_the_editor() {
        let parse = |text: &str| Config::parse(text).unwrap();
        let a = parse("top = 1\n[reticulum]\nk = 1\n[interfaces]\n[[A]]\ntype = X\n[[[inner]]]\nx = 1\n");
        assert!(same_config(&a, &a.clone()));
        for other in [
            "top = 2\n[reticulum]\nk = 1\n[interfaces]\n[[A]]\ntype = X\n[[[inner]]]\nx = 1\n",
            "top = 1\n[reticulum]\nk = 1\n[interfaces]\n[[A]]\ntype = X\n[[[inner]]]\nx = 2\n",
            "top = 1\n[reticulum]\nk = 1\n[interfaces]\n[[A]]\ntype = X\n",
            "top = 1\n[reticulum]\nk = 1\n[interfaces]\n[[A]]\ntype = X\n[[[inner]]]\nx = 1\n[[B]]\n",
            "top = 1\n[reticulum]\nk = 1\nj = 1\n[interfaces]\n[[A]]\ntype = X\n[[[inner]]]\nx = 1\n",
            "[reticulum]\nk = 1\n[interfaces]\n[[A]]\ntype = X\n[[[inner]]]\nx = 1\n",
        ] {
            assert!(!same_config(&a, &parse(other)), "{other}");
        }
        // The parser's reading of an interface that may run a program, in any case.
        for text in [
            "[interfaces]\n[[A]]\ntype = TCPClientInterface\nCOMMAND = x\n",
            "[interfaces]\n[[A]]\ntype = TCPClientInterface\nLocation_Cmd = x\n",
            "[interfaces]\n[[A]]\nTYPE = pipeinterface\n",
            "[interfaces]\n[[A]]\ntype = a, b\n",
            "[interfaces]\n[[A]]\ntype = \n",
            "[interfaces]\n[[A]]\nenabled = yes\n",
            "[interfaces]\n[[A]]\ntype = TCPServerInterface\nreachable_on = /bin/where\n",
            "[interfaces]\n[[A]]\ntype = TCPServerInterface\ndiscovery_reachable_on = ~/where\n",
        ] {
            assert!(runs_program(parse(text).subsection("interfaces", "A").unwrap()), "{text}");
        }
        assert!(!runs_program(parse(OFF).subsection("interfaces", "Hilltop TCP").unwrap()));
    }

    #[test]
    fn an_interface_whose_reachable_on_may_run_a_program_cant_be_changed() {
        for value in ["/usr/local/bin/where", "~/where.sh", "./where"] {
            let text = OFF.replace("node.example.org", value);
            for asked in [change(HILL, "enabled", "yes"), change(HILL, "mode", "full")] {
                let refused = planned(&text, &[asked]);
                assert!(
                    matches!(&refused, Err(RnsRefusal::NotAllowed { message, .. }) if message.contains("runs a program")),
                    "{value}: {refused:?}"
                );
            }
            assert_eq!(editable(&text, &redact(&text)), [vec!["reticulum".to_string()]]);
        }
    }

    #[test]
    fn characters_python_reads_as_line_breaks_are_refused() {
        let base = "[reticulum]\nenable_transport = no\n[interfaces]\n[[A]]\ntype = TCPClientInterface\nenabled = no\n";
        for c in ['\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}'] {
            // Python would read a header or a key here that rsReticulum doesn't.
            let text = format!("{base}notes = x{c}[[Shell]]{c}type = PipeInterface\n");
            let refused = planned(&text, &[change(&["interfaces", "A"], "mode", "full")]);
            assert!(
                matches!(&refused, Err(RnsRefusal::Ambiguous { message, .. }) if message.contains("line breaks")),
                "{c:?}: {refused:?}"
            );
            assert!(editable(&text, &redact(&text)).is_empty(), "{c:?}");
        }
        // A tab or a plain line break is fine.
        assert!(planned(&format!("{base}notes = a\tb\n"), &[change(&["interfaces", "A"], "mode", "full")]).is_ok());
    }

    #[test]
    fn a_value_that_never_ends_says_so() {
        let text = "[reticulum]\nenable_transport = no\nnotes = '''\nmore\n";
        let refused = planned(text, &[change(&["reticulum"], "enable_transport", "yes")]);
        assert!(
            matches!(&refused, Err(RnsRefusal::Ambiguous { message, .. }) if message.contains("never ends") && !message.contains("too big")),
            "{refused:?}"
        );
    }
}
