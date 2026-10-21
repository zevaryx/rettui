//! Several identities, one used at a time. Each is an LXMF address of its
//! own, with its own name, conversations, contacts and settings (the rest
//! of a data directory), so switching is being someone else, as Columba's
//! and MeshChat's identities are.
//!
//! The first identity is the data directory's own, as it always was; each
//! one added has a folder in `identities/` (named by the start of its LXMF
//! address) that's a data directory of its own. `identities/active` says
//! which is in use (none, or one that's gone: the first). What isn't about
//! who you are is the data directory's, whichever is in use: the log, the
//! web UI's login, certificates and browsers, update checks and caches.

use std::path::{Path, PathBuf};

use rns_identity::identity::Identity;

use crate::config::{Paths, Settings};
use crate::net::Hash;

/// The file saying which identity is in use.
const ACTIVE: &str = "active";

/// Most identities kept (the first included).
pub const MOST: usize = 20;

/// An identity kept here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Its folder's name in `identities/`; empty for the first.
    pub id: String,
    /// Its data directory.
    pub dir: PathBuf,
    /// The name it announces.
    pub name: String,
    pub lxmf: Hash,
    /// Its `lxma://` link: the address with its key, to share.
    pub link: String,
    pub active: bool,
}

/// Where the identities after the first are kept.
pub fn folder(root: &Path) -> PathBuf {
    root.join("identities")
}

/// The data directory of the identity in use, in the data directory
/// `root`.
pub fn active_dir(root: &Path) -> PathBuf {
    let id = std::fs::read_to_string(folder(root).join(ACTIVE)).unwrap_or_default();
    let id = id.trim();
    let dir = folder(root).join(id);
    if valid_id(id) && dir.join("identity").is_file() { dir } else { root.to_path_buf() }
}

/// An identity's folder name: hex, so never a path.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The identities kept in the data directory `root`: the first, then the
/// rest by name.
pub fn list(root: &Path) -> Vec<Profile> {
    let active = active_dir(root);
    let mut rest: Vec<Profile> = std::fs::read_dir(folder(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            valid_id(&id).then(|| profile(id, entry.path(), &active))?
        })
        .collect();
    rest.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    profile(String::new(), root.to_path_buf(), &active).into_iter().chain(rest).collect()
}

/// The identity in the data directory `dir`, if it has one.
fn profile(id: String, dir: PathBuf, active: &Path) -> Option<Profile> {
    let identity = Identity::from_file(&dir.join("identity")).ok()?;
    let lxmf = crate::app::identity::lxmf_address(&identity);
    let name = Settings::load(&dir.join("settings.json")).map(|s| s.display_name).unwrap_or_default();
    Some(Profile { link: crate::lxmf::identity_link(lxmf, &identity.get_public_key()), active: dir == active, id, dir, name, lxmf })
}

/// One of the identities, by id.
pub fn find(root: &Path, id: &str) -> Result<Profile, String> {
    list(root).into_iter().find(|p| p.id == id).ok_or_else(|| "That identity isn't here any more".to_string())
}

/// Keep another identity: `identity` (one brought from elsewhere), or a
/// new one. It starts with the settings of the one in use (`paths` and
/// `settings`), but its own `name`, and nothing hosted or shared of the
/// other's: no node, propagation node or hub, location or icon.
pub fn add(paths: &Paths, settings: &Settings, name: &str, identity: Option<Identity>) -> Result<Profile, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give it a name, the one it announces".into());
    }
    if name.chars().count() > crate::config::MAX_DISPLAY_NAME {
        return Err(format!("A name is at most {} characters", crate::config::MAX_DISPLAY_NAME));
    }
    let profiles = list(&paths.root);
    if profiles.len() >= MOST {
        return Err(format!("At most {MOST} identities are kept"));
    }
    let identity = identity.unwrap_or_default();
    let lxmf = crate::app::identity::lxmf_address(&identity);
    if let Some(same) = profiles.iter().find(|p| p.lxmf == lxmf) {
        return Err(format!("That identity is here already, as {}", same.name));
    }
    let id = hex::encode(&lxmf[..8]);
    let dir = folder(&paths.root).join(&id);
    let fail = |e: String| format!("Couldn't keep the identity: {e}");
    std::fs::create_dir_all(&dir).map_err(|e| fail(e.to_string()))?;
    let made = (|| {
        identity.to_file(&dir.join("identity")).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("identity"), std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        }
        let own = Settings {
            display_name: name.to_string(),
            icon: None,
            location: None,
            node_enabled: false,
            node_name: None,
            node_dir: None,
            pn_enabled: false,
            pn_name: None,
            hub_enabled: false,
            hub_name: None,
            hub_greeting: None,
            identity_backed_up: false,
            show_first_steps: false,
            welcomed: true,
            ..settings.clone()
        };
        own.save(&dir.join("settings.json")).map_err(|e| e.to_string())?;
        // Others' keys, heard already: so it can write to them at once.
        let _ = std::fs::copy(&paths.known_identities, dir.join("known_identities.json"));
        Ok::<(), String>(())
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(fail(e));
    }
    find(&paths.root, &id)
}

/// Use the identity `id` from the next start (or switch).
pub fn choose(root: &Path, id: &str) -> Result<Profile, String> {
    let profile = find(root, id)?;
    std::fs::create_dir_all(folder(root)).map_err(|e| e.to_string())?;
    crate::config::write_atomic(&folder(root).join(ACTIVE), id.as_bytes()).map_err(|e| format!("Couldn't switch: {e}"))?;
    Ok(profile)
}

/// Rename the identity `id` (the name it announces); not the one in use,
/// whose name is a setting the app changes.
pub fn rename(root: &Path, id: &str, name: &str) -> Result<(), String> {
    let profile = find(root, id)?;
    let path = profile.dir.join("settings.json");
    let mut settings = Settings::load(&path).map_err(|e| e.to_string())?;
    settings.set_field("display_name", name).map_err(|e| e.to_string())?;
    settings.save(&path).map_err(|e| format!("Couldn't rename it: {e}"))
}

/// Delete the identity `id`, with all it holds: its key, conversations and
/// contacts. Not the first, nor the one in use.
pub fn delete(root: &Path, id: &str) -> Result<Profile, String> {
    let profile = find(root, id)?;
    if profile.id.is_empty() {
        return Err("The first identity stays (it's the data directory's own)".into());
    }
    if profile.active {
        return Err("That's the identity in use: switch to another first".into());
    }
    std::fs::remove_dir_all(&profile.dir).map_err(|e| format!("Couldn't delete it: {e}"))?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_kept_chosen_renamed_and_deleted() {
        let root = std::env::temp_dir().join(format!("rettui-identities-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::new(Some(root.clone())).unwrap();
        let first = crate::config::load_identity(&paths.identity).unwrap();
        let settings = Settings { display_name: "Amy".into(), location: Some("1, 2".into()), node_enabled: true, ..Settings::default() };
        settings.save(&paths.settings).unwrap();
        std::fs::write(&paths.known_identities, "{}").unwrap();
        let [one] = list(&root).try_into().unwrap();
        assert_eq!((one.id.as_str(), one.name.as_str(), one.active), ("", "Amy", true));
        assert_eq!(one.lxmf, crate::app::identity::lxmf_address(&first));
        assert!(one.link.starts_with("lxma://"));

        // A new one: its own name, the rest of the settings, nothing hosted
        // or shared.
        let work = add(&paths, &settings, " Amy at work ", None).unwrap();
        assert_eq!((work.name.as_str(), work.active, work.id.len()), ("Amy at work", false, 16));
        let theirs = Settings::load(&work.dir.join("settings.json")).unwrap();
        assert_eq!((theirs.location, theirs.node_enabled, theirs.clock), (None, false, settings.clock.clone()));
        assert!(work.dir.join("known_identities.json").is_file());
        assert_eq!(list(&root).len(), 2);
        // One brought along; not twice, and none nameless.
        let brought = Identity::new();
        let other = add(&paths, &settings, "Bee", Some(brought.clone())).unwrap();
        assert!(add(&paths, &settings, "Bee again", Some(brought)).unwrap_err().contains("here already, as Bee"));
        assert!(add(&paths, &settings, "   ", None).is_err());
        assert_eq!(list(&root).iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Amy", "Amy at work", "Bee"]);

        // Chosen: the data directory follows it, but not what's shared.
        choose(&root, &work.id).unwrap();
        assert_eq!(active_dir(&root), work.dir);
        let switched = Paths::new(Some(root.clone())).unwrap();
        assert_eq!((switched.base.as_path(), switched.root.as_path()), (work.dir.as_path(), root.as_path()));
        assert_eq!(switched.identity, work.dir.join("identity"));
        assert_eq!(switched.web_token, root.join("web_token"));
        assert!(list(&root).iter().find(|p| p.id == work.id).unwrap().active);
        assert!(delete(&root, &work.id).unwrap_err().contains("in use"));
        assert!(delete(&root, "").unwrap_err().contains("first identity stays"));
        // An odd or missing choice is the first.
        for odd in ["../..", "", "abcdef"] {
            std::fs::write(folder(&root).join(ACTIVE), odd).unwrap();
            assert_eq!(active_dir(&root), root);
        }
        assert!(choose(&root, "abcdef").is_err());

        rename(&root, &other.id, "Bea").unwrap();
        assert_eq!(find(&root, &other.id).unwrap().name, "Bea");
        assert_eq!(delete(&root, &other.id).unwrap().name, "Bea");
        assert!(!other.dir.exists() && find(&root, &other.id).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
