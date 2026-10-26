//! Interface profiles: named sets of interfaces to have on, to switch
//! between (at home, out with a radio, and so on), as NomadNet 1.4 has.
//!
//! A profile is each interface's on or off as it was when the profile was
//! saved. Switching to one turns those interfaces on or off in the config
//! file (an edit like any other, kept in its history) and leaves the rest,
//! such as ones added since, as they are. Profiles are kept in the data
//! directory, shared by every identity, as the config file is.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::doc::Doc;

/// Most profiles kept.
pub const MAX_PROFILES: usize = 50;
/// Longest name a profile may have, in characters.
pub const MAX_NAME: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Each interface it names, and whether it's on in it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub interfaces: BTreeMap<String, bool>,
}

impl Profile {
    /// Whether the config `text` has these interfaces on and off as it says
    /// (those it names that the file no longer has aside).
    pub fn matches(&self, text: &str) -> bool {
        let now = interfaces(text);
        !now.is_empty() && self.interfaces.iter().all(|(name, on)| now.get(name).is_none_or(|is| is == on))
    }
}

/// Each interface in the config `text`, and whether it's on.
pub fn interfaces(text: &str) -> BTreeMap<String, bool> {
    let doc = Doc::new(text);
    doc.subsections("interfaces").into_iter().map(|name| (name.clone(), super::interface_on(&doc, &name))).collect()
}

/// The config `text` with the interfaces `profile` names turned on or off
/// as it has them, and what that changed (as "Hill on").
pub fn apply(text: &str, profile: &Profile) -> (String, Vec<String>) {
    let now = interfaces(text);
    let mut doc = Doc::new(text);
    let mut changed = Vec::new();
    for (name, on) in &profile.interfaces {
        if now.get(name).is_some_and(|is| is != on) {
            doc.set(&["interfaces", name], "enabled", &["interface_enabled"], Some(if *on { "Yes" } else { "No" }));
            changed.push(format!("{name} {}", if *on { "on" } else { "off" }));
        }
    }
    (doc.text(), changed)
}

/// The profiles file.
pub struct Profiles {
    path: PathBuf,
}

impl Profiles {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The profiles saved, by name. A file that can't be read has none.
    pub fn list(&self) -> Vec<Profile> {
        let Ok(bytes) = std::fs::read(&self.path) else { return Vec::new() };
        let mut profiles: Vec<Profile> = serde_json::from_slice(&bytes).unwrap_or_default();
        profiles.sort_by_key(|p| p.name.to_lowercase());
        profiles
    }

    pub fn get(&self, name: &str) -> Option<Profile> {
        self.list().into_iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Save the interfaces in the config `text`, on and off as they are now,
    /// as profile `name` (replacing one of that name).
    pub fn save(&self, name: &str, text: &str) -> Result<Profile, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A profile needs a name".into());
        }
        if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
            return Err(format!("A profile's name is at most {MAX_NAME} characters, on one line"));
        }
        let interfaces = interfaces(text);
        if interfaces.is_empty() {
            return Err("The Reticulum config has no interfaces to keep in a profile".into());
        }
        let mut profiles = self.list();
        profiles.retain(|p| !p.name.eq_ignore_ascii_case(name));
        if profiles.len() >= MAX_PROFILES {
            return Err(format!("There are already {MAX_PROFILES} profiles: delete one first"));
        }
        let profile = Profile { name: name.to_string(), interfaces };
        profiles.push(profile.clone());
        self.write(&profiles)?;
        Ok(profile)
    }

    pub fn delete(&self, name: &str) -> Result<(), String> {
        let mut profiles = self.list();
        let before = profiles.len();
        profiles.retain(|p| !p.name.eq_ignore_ascii_case(name.trim()));
        if profiles.len() == before {
            return Err(format!("There's no profile named {}", name.trim()));
        }
        self.write(&profiles)
    }

    fn write(&self, profiles: &[Profile]) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(profiles).map_err(|e| e.to_string())?;
        crate::config::write_atomic(&self.path, &bytes).map_err(|e| format!("Could not save the profiles: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const CONFIG: &str = "[reticulum]\n  share_instance = Yes\n\n[interfaces]\n  [[Default Interface]]\n    type = AutoInterface\n    enabled = Yes\n  [[Hill]]\n    type = TCPClientInterface\n    interface_enabled = No\n    target_host = hill.example\n    target_port = 4242\n  [[Radio]]\n    type = RNodeInterface\n";

    #[test]
    fn a_profile_keeps_interfaces_on_and_off_and_puts_them_back() {
        let dir = dir("keep");
        let profiles = Profiles::new(dir.join("interface-profiles.json"));
        let home = profiles.save(" Home ", CONFIG).unwrap();
        assert_eq!(home.name, "Home");
        assert_eq!(home.interfaces, BTreeMap::from([("Default Interface".into(), true), ("Hill".into(), false), ("Radio".into(), true)]));
        assert!(home.matches(CONFIG));

        let (away, changed) = apply(
            CONFIG,
            &Profile {
                name: "Away".into(),
                interfaces: BTreeMap::from([("Default Interface".into(), false), ("Hill".into(), true), ("Gone".into(), true)]),
            },
        );
        assert_eq!(changed, ["Default Interface off", "Hill on"]);
        assert!(away.contains("    enabled = No\n") && away.contains("    interface_enabled = Yes\n"), "{away}");
        assert!(!home.matches(&away));
        let (back, changed) = apply(&away, &home);
        assert_eq!(changed.len(), 2);
        assert!(home.matches(&back));
        assert_eq!(apply(&back, &home).1, Vec::<String>::new());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn profiles_are_named_listed_and_deleted() {
        let dir = dir("names");
        let profiles = Profiles::new(dir.join("interface-profiles.json"));
        assert!(profiles.list().is_empty());
        assert!(profiles.save("  ", CONFIG).is_err());
        assert!(profiles.save("a\nb", CONFIG).is_err());
        assert!(profiles.save(&"x".repeat(MAX_NAME + 1), CONFIG).is_err());
        assert!(profiles.save("Empty", "[reticulum]\n").is_err());
        profiles.save("walk", CONFIG).unwrap();
        profiles.save("Home", CONFIG).unwrap();
        profiles.save("WALK", CONFIG).unwrap();
        let names: Vec<String> = profiles.list().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Home", "WALK"]);
        assert!(profiles.get("home").is_some());
        profiles.delete("home").unwrap();
        assert!(profiles.delete("home").is_err());
        assert_eq!(profiles.list().len(), 1);
        std::fs::write(dir.join("interface-profiles.json"), "not json").unwrap();
        assert!(profiles.list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
