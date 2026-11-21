//! Which settings remote management may change (`MANAGE_FIELDS`), a
//! smaller list than the web UI's: names, limits and intervals of the
//! services the node already runs, and turning those services off. Only the
//! terminal turns a service on, and nothing here decides what runs on the
//! computer, where files are, or who may connect.

use sha2::{Digest, Sha256};

use super::audit::Change;
use crate::config::{self, FieldKind, Settings};

/// What remote management may do with a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Change,
    /// A toggle it may turn off, but not on.
    TurnOffOnly,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Change => "change",
            Access::TurnOffOnly => "turn_off_only",
        }
    }
}

/// The settings remote management may change, in the order `/state` lists
/// them. Before adding one, read the test at the end of this file.
pub const MANAGE_FIELDS: &[(&str, Access)] = &[
    ("display_name", Access::Change),
    ("announce_interval_mins", Access::Change),
    ("pn_enabled", Access::TurnOffOnly),
    ("pn_name", Access::Change),
    ("pn_stamp_cost", Access::Change),
    ("pn_storage_mb", Access::Change),
    ("pn_transfer_kb", Access::Change),
    ("pn_autopeer", Access::Change),
    ("pn_autopeer_depth", Access::Change),
    ("pn_max_peers", Access::Change),
    ("pn_static_peers", Access::Change),
    ("node_enabled", Access::TurnOffOnly),
    ("node_name", Access::Change),
    ("node_announce_interval_mins", Access::Change),
    ("hub_enabled", Access::TurnOffOnly),
    ("hub_name", Access::Change),
    ("hub_greeting", Access::Change),
    ("hub_announce_interval_mins", Access::Change),
    ("git_host_enabled", Access::TurnOffOnly),
    ("git_host_pages", Access::TurnOffOnly),
    ("git_host_name", Access::Change),
    ("log_level", Access::Change),
];

/// What remote management may do with `key`, if anything.
pub fn access(key: &str) -> Option<Access> {
    MANAGE_FIELDS.iter().find(|(k, _)| *k == key).map(|(_, access)| *access)
}

/// How `/state` names a kind of setting.
pub fn kind_name(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Text | FieldKind::Color => "text",
        FieldKind::Optional => "optional",
        FieldKind::Number => "number",
        FieldKind::Toggle => "toggle",
        FieldKind::Choice(_) => "choice",
    }
}

/// The settings revision: the first 16 bytes of the SHA-256 of the managed
/// settings as `key=value` lines, in hex. A change made anywhere (here, the
/// terminal, the web UI, by hand) gives another one.
pub fn revision(settings: &Settings) -> String {
    let mut hash = Sha256::new();
    for (key, _) in MANAGE_FIELDS {
        hash.update(format!("{key}={}\n", settings.field_value(key)).as_bytes());
    }
    hex::encode(&hash.finalize()[..16])
}

/// The least `pn_storage_mb` a remote change may set: a smaller store can
/// make the propagation node drop messages it holds, which a rollback
/// doesn't bring back.
pub const MIN_REMOTE_STORAGE_MB: u64 = 32;
/// The most detailed `log_level` a remote change may set: `trace` can
/// record peer addresses and message details.
pub const MAX_REMOTE_LOG_LEVEL: &str = "debug";

/// Why a value the setting itself takes is still refused remotely (only
/// the terminal may set it), if it is.
fn remote_limit(key: &str, storage_reference: u64, after: &Settings) -> Option<String> {
    match key {
        "log_level" if after.log_level == "trace" => Some(format!(
            "Log level can be set to {MAX_REMOTE_LOG_LEVEL} at most remotely: trace can record peer addresses and message details, so set it in the terminal"
        )),
        "pn_max_peers" if after.pn_max_peers == 0 => Some(
            "Propagation peers can't be set to 0 remotely: the node would stop passing messages on to other propagation nodes, so set it in the terminal"
                .to_string(),
        ),
        "pn_storage_mb" if after.pn_storage_mb < MIN_REMOTE_STORAGE_MB => Some(format!(
            "Propagation storage can be set to {MIN_REMOTE_STORAGE_MB} MB at least remotely: less can make the node drop messages it holds, so set it in the terminal"
        )),
        // A big cut can drop much of what is stored, which a rollback doesn't bring back. Held to the
        // most it was in the last 24 hours, so a few small cuts in a row can't add up to a big one.
        "pn_storage_mb" if after.pn_storage_mb.saturating_mul(2) < storage_reference => Some(format!(
            "Propagation storage can be cut to half of the most it was in the last 24 hours at most remotely (from {storage_reference} MB to {} MB): a bigger cut can make the node drop messages it holds, so make it in the terminal",
            storage_reference.div_ceil(2)
        )),
        _ => None,
    }
}

/// Why a change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not on the list, or would turn a service on.
    NotAllowed { key: String, message: String },
    /// A value the setting doesn't take.
    Invalid { key: String, message: String },
    /// Every value is what it is already.
    NoChange,
}

/// Check `changes` against `before` (the settings as saved): every key is on
/// the list, every value is one its setting takes, no service is turned on,
/// and no value goes past what may be set remotely ([`MAX_REMOTE_LOG_LEVEL`],
/// [`MIN_REMOTE_STORAGE_MB`] and no less than half the storage there is, at
/// least one propagation peer). The managed
/// settings that would change, with their old and new values.
#[cfg(test)]
pub fn check(before: &Settings, changes: &[(&str, &str)]) -> Result<Vec<Change>, Refusal> {
    check_against(before, changes, before.pn_storage_mb)
}

/// [`check`], with `storage_reference` the most the propagation storage was
/// in the last 24 hours (at least what it is now), which a cut is held to.
pub fn check_against(before: &Settings, changes: &[(&str, &str)], storage_reference: u64) -> Result<Vec<Change>, Refusal> {
    let storage_reference = storage_reference.max(before.pn_storage_mb);
    let mut after = before.clone();
    for (key, value) in changes {
        if access(key).is_none() {
            let name: String = key.chars().take(64).collect();
            return Err(Refusal::NotAllowed {
                key: name.clone(),
                message: format!("{name} can't be changed remotely: change it in the terminal UI or settings.json"),
            });
        }
        after.set_field(key, value).map_err(|message| Refusal::Invalid { key: key.to_string(), message })?;
    }
    let mut applied = Vec::new();
    for (key, access) in MANAGE_FIELDS {
        let (from, to) = (before.field_value(key), after.field_value(key));
        if from == to {
            continue;
        }
        if *access == Access::TurnOffOnly && to != "false" {
            let label = config::field(key).map_or(*key, |f| f.label);
            return Err(Refusal::NotAllowed {
                key: key.to_string(),
                message: format!("{label} can only be turned on in the terminal: a service turned on remotely could expose data"),
            });
        }
        if let Some(message) = remote_limit(key, storage_reference, &after) {
            return Err(Refusal::NotAllowed { key: key.to_string(), message });
        }
        applied.push(Change { path: Vec::new(), key: key.to_string(), from, to });
    }
    if applied.is_empty() {
        return Err(Refusal::NoChange);
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Effect, WebAccess};

    /// Settings that must never be on the list, whatever the web UI may do with
    /// them: who may clone, where data is sent, which entry points are used,
    /// and which Reticulum config runs. Keys ending in [`DENY_ENDINGS`] are
    /// folders, search paths and commands.
    const DENY: &[&str] = &["git_host_access", "telemetry_mqtt", "discovered_entry_points", "disconnect_bootstrap_only", "rns_config"];
    const DENY_ENDINGS: &[&str] = &["_dir", "_path", "_command"];

    /// Each managed setting can be changed by whoever holds the website's
    /// login without reaching further than the setting looks: the web UI may
    /// change it, it applies at once (so `settings_that_apply_now_do` covers
    /// it, or `log_level` its own arm there), it isn't a path, command, URL
    /// or access rule, a turn-off-only one is a toggle, and its value is
    /// bounded.
    #[test]
    fn every_manage_field_is_safe_to_change_remotely() {
        let mut seen = std::collections::HashSet::new();
        for (key, access) in MANAGE_FIELDS {
            assert!(seen.insert(key), "{key} is listed twice");
            let field = config::field(key).unwrap_or_else(|| panic!("{key} isn't a setting"));
            assert_ne!(field.web_access(), WebAccess::TerminalOnly, "{key} decides what runs on the computer");
            assert_eq!(field.effect, Effect::Now, "{key} applies only at the next start");
            assert!(!DENY.contains(key), "{key} must not be managed remotely");
            assert!(!DENY_ENDINGS.iter().any(|end| key.ends_with(end)), "{key} is a folder, path or command");
            assert_ne!(field.kind, FieldKind::Color, "{key}: /state has no colour kind");
            if *access == Access::TurnOffOnly {
                assert_eq!(field.kind, FieldKind::Toggle, "{key} is turn-off-only but not a toggle");
            }
            // The web UI's own turn-off-only settings stay so here.
            if field.web_access() == WebAccess::TurnOffOnly {
                assert_eq!(*access, Access::TurnOffOnly, "{key}");
            }
            let mut settings = Settings::default();
            let long = "x".repeat(5000);
            if settings.set_field(key, &long).is_ok() {
                assert!(settings.field_value(key).len() <= 4096, "{key} keeps a value longer than 4 KiB");
            }
        }
    }

    #[test]
    fn the_revision_follows_the_managed_settings_only() {
        let settings = Settings::default();
        let first = revision(&settings);
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(revision(&settings.clone()), first);
        let mut renamed = settings.clone();
        renamed.set_field("pn_name", "Hilltop").unwrap();
        assert_ne!(revision(&renamed), first);
        // A setting not on the list doesn't change it.
        let mut other = settings.clone();
        other.set_field("messages_kept", "77").unwrap();
        assert_eq!(revision(&other), first);
    }

    #[test]
    fn changes_are_checked_against_the_list() {
        let mut before = Settings { pn_enabled: true, ..Settings::default() };
        before.set_field("pn_name", "Old").unwrap();
        let peers = before.field_value("pn_max_peers");
        assert_ne!(peers, "9");
        let applied = check(&before, &[("pn_name", "New"), ("pn_max_peers", "9"), ("pn_enabled", "off")]).unwrap();
        let keys: Vec<_> = applied.iter().map(|c| (c.key.as_str(), c.from.as_str(), c.to.as_str())).collect();
        // In the list's order, with the values as saved (a toggle as true or false).
        assert_eq!(keys, [("pn_enabled", "true", "false"), ("pn_name", "Old", "New"), ("pn_max_peers", peers.as_str(), "9")]);
        // The same value again is no change; unchanged ones beside a change are left out.
        assert_eq!(check(&before, &[("pn_name", "Old")]), Err(Refusal::NoChange));
        assert_eq!(check(&before, &[]), Err(Refusal::NoChange));
        assert_eq!(check(&before, &[("pn_name", "Old"), ("pn_max_peers", "9")]).unwrap().len(), 1);
        // Not on the list.
        for key in ["rns_config", "node_dir", "node_executable_pages", "git_host_access", "telemetry_mqtt", "no_such_key"] {
            match check(&before, &[(key, "x")]) {
                Err(Refusal::NotAllowed { key: refused, message }) => {
                    assert_eq!(refused, key);
                    assert!(message.contains("terminal"), "{message}");
                }
                other => panic!("{key}: {other:?}"),
            }
        }
        // A long unknown key is cut in the answer.
        let long = "k".repeat(500);
        let Err(Refusal::NotAllowed { key, .. }) = check(&before, &[(&long, "x")]) else { panic!() };
        assert_eq!(key.len(), 64);
        // A value the setting doesn't take.
        let Err(Refusal::Invalid { key, message }) = check(&before, &[("pn_name", "New"), ("pn_max_peers", "lots")]) else { panic!() };
        assert_eq!(key, "pn_max_peers");
        assert!(message.contains("whole number"), "{message}");
    }

    #[test]
    fn turn_off_only_settings_refuse_on() {
        let mut off = Settings::default();
        for (key, access) in MANAGE_FIELDS {
            if *access == Access::TurnOffOnly {
                off.set_field(key, "false").unwrap();
            }
        }
        for key in ["pn_enabled", "node_enabled", "hub_enabled", "git_host_enabled", "git_host_pages"] {
            for on in ["true", "on", "yes", "1"] {
                match check(&off, &[(key, on)]) {
                    Err(Refusal::NotAllowed { key: refused, message }) => {
                        assert_eq!(refused, key);
                        assert!(message.contains("only be turned on in the terminal"), "{message}");
                    }
                    other => panic!("{key}={on}: {other:?}"),
                }
            }
            // Off when off already is no change; on to off is fine.
            assert_eq!(check(&off, &[(key, "false")]), Err(Refusal::NoChange));
            let mut on = Settings::default();
            on.set_field(key, "true").unwrap();
            assert_eq!(check(&on, &[(key, "off")]).unwrap()[0].to, "false");
            // On when it is on already changes nothing either.
            assert_eq!(check(&on, &[(key, "true")]), Err(Refusal::NoChange));
        }
    }

    #[test]
    fn remote_changes_stop_at_debug_and_keep_the_propagation_node_whole() {
        let before = Settings::default();
        let refused = |changes: &[(&str, &str)]| match check(&before, changes) {
            Err(Refusal::NotAllowed { key, message }) => (key, message),
            other => panic!("{changes:?}: {other:?}"),
        };
        let (key, message) = refused(&[("log_level", "trace")]);
        assert_eq!(key, "log_level");
        assert!(message.contains("debug at most") && message.contains("terminal"), "{message}");
        let (key, message) = refused(&[("pn_name", "Fine"), ("pn_max_peers", "0")]);
        assert_eq!(key, "pn_max_peers");
        assert!(message.contains("can't be set to 0 remotely"), "{message}");
        for small in ["1", "31"] {
            let (key, message) = refused(&[("pn_storage_mb", small)]);
            assert_eq!(key, "pn_storage_mb");
            assert!(message.contains("32 MB at least"), "{message}");
        }
        // A cut to less than half the storage there is, too, however big.
        assert_eq!(before.pn_storage_mb, 500);
        let (key, message) = refused(&[("pn_storage_mb", "249")]);
        assert_eq!(key, "pn_storage_mb");
        assert!(message.contains("half") && message.contains("from 500 MB to 250 MB") && message.contains("terminal"), "{message}");
        assert_eq!(check(&before, &[("pn_storage_mb", "250")]).unwrap()[0].to, "250");
        assert_eq!(check(&before, &[("pn_storage_mb", "100000")]).unwrap()[0].to, "100000", "a raise is fine");
        let big = Settings { pn_storage_mb: 100_000, ..Settings::default() };
        assert!(matches!(check(&big, &[("pn_storage_mb", "500")]), Err(Refusal::NotAllowed { .. })));
        let odd = Settings { pn_storage_mb: 41, ..Settings::default() };
        assert!(check(&odd, &[("pn_storage_mb", "21")]).is_err(), "under 32 all the same");
        assert_eq!(check(&Settings { pn_storage_mb: 63, ..Settings::default() }, &[("pn_storage_mb", "32")]).unwrap()[0].to, "32");
        // Held to the most in the last 24 hours: small cuts in a row don't add up.
        let (first, after_it) =
            (Settings { pn_storage_mb: 1000, ..Settings::default() }, Settings { pn_storage_mb: 500, ..Settings::default() });
        assert_eq!(check_against(&first, &[("pn_storage_mb", "500")], 1000).unwrap()[0].to, "500");
        let Err(Refusal::NotAllowed { message, .. }) = check_against(&after_it, &[("pn_storage_mb", "250")], 1000) else { panic!() };
        assert!(message.contains("last 24 hours") && message.contains("from 1000 MB to 500 MB"), "{message}");
        assert_eq!(check_against(&after_it, &[("pn_storage_mb", "250")], 500).unwrap()[0].to, "250", "a day later");
        // Up to the limits is fine.
        assert_eq!(check(&before, &[("log_level", "debug")]).unwrap()[0].to, "debug");
        assert_eq!(check(&before, &[("pn_max_peers", "1")]).unwrap()[0].to, "1");
        let small = Settings { pn_storage_mb: 64, ..Settings::default() };
        assert_eq!(check(&small, &[("pn_storage_mb", "32")]).unwrap()[0].to, "32");
        // The terminal can still set them; a value set there already isn't
        // refused when something else changes remotely.
        let mut terminal = Settings::default();
        for (key, value) in [("log_level", "trace"), ("pn_max_peers", "0"), ("pn_storage_mb", "1")] {
            terminal.set_field(key, value).unwrap();
        }
        assert_eq!(check(&terminal, &[("pn_name", "Hilltop")]).unwrap().len(), 1);
        // Down from trace is fine.
        assert_eq!(check(&terminal, &[("log_level", "info")]).unwrap()[0].from, "trace");
    }

    #[test]
    fn kinds_and_access_have_their_wire_names() {
        assert_eq!(kind_name(FieldKind::Text), "text");
        assert_eq!(kind_name(FieldKind::Color), "text");
        assert_eq!(kind_name(FieldKind::Optional), "optional");
        assert_eq!(kind_name(FieldKind::Number), "number");
        assert_eq!(kind_name(FieldKind::Toggle), "toggle");
        assert_eq!(kind_name(FieldKind::Choice(&["a"])), "choice");
        assert_eq!((Access::Change.as_str(), Access::TurnOffOnly.as_str()), ("change", "turn_off_only"));
        assert_eq!(access("pn_name"), Some(Access::Change));
        assert_eq!(access("rns_config"), None);
    }
}
