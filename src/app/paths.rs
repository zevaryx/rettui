//! Paths to destinations, found or forgotten on request, as `rnpath` does:
//! for when one can't be reached until it announces again (finding asks
//! for a path again, as [`crate::net::find_path`] does), or a path known
//! has gone stale. And probes, as `rnprobe` sends.

use crate::net::{Hash, NetCommand, PathInfo, Probe, parse_hash};

use super::App;

/// How finding a path to a destination is going.
#[derive(Debug, Clone, PartialEq)]
pub enum PathLookup {
    /// Being looked for; how it's going, once there's word.
    Waiting(Option<String>),
    Done(Result<PathInfo, String>),
}

/// How probing a destination is going.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeState {
    /// Waiting for an answer; how finding a path goes, once there's word.
    Waiting(Option<String>),
    Done(Result<Probe, String>),
}

/// A probe's answer in words: "answered in 182 ms, 2 hops away (LXMF
/// address)"; for one reached with a Link, "a Link opened in …".
pub fn probe_label(probe: &Probe) -> String {
    let hops = match probe.hops {
        Some(0 | 1) => ", heard directly".to_string(),
        Some(n) => format!(", {n} hops away"),
        None => String::new(),
    };
    let kind = probe.kind.map(|kind| format!(" ({kind})")).unwrap_or_default();
    let how = if probe.by_link { "a Link opened" } else { "answered" };
    format!("{how} in {} ms{hops}{kind}", probe.rtt.as_millis())
}

/// How probing went, or is going, in words.
pub fn probe_state_label(state: &ProbeState) -> String {
    match state {
        ProbeState::Waiting(None) => "waiting for an answer…".into(),
        ProbeState::Waiting(Some(how)) => format!("looking for a path: {how}…"),
        ProbeState::Done(Ok(probe)) => probe_label(probe),
        ProbeState::Done(Err(e)) => e.clone(),
    }
}

/// How far a path goes: "heard directly" (one hop: over an interface of
/// this one's), or "3 hops".
pub fn hops_label(hops: u8) -> String {
    match hops {
        0 | 1 => "heard directly".to_string(),
        n => format!("{n} hops"),
    }
}

/// How long a path is kept for, unless heard again: "40 min", "5 h", "6 days".
pub fn kept_label(expires: i64, now: i64) -> String {
    let left = (expires - now).max(0);
    match left {
        0..3600 => format!("{} min", left / 60),
        3600..172_800 => format!("{} h", left / 3600),
        _ => format!("{} days", left / 86_400),
    }
}

/// A path in words: "2 hops via <0a1b2c3d…> on RMAP World, kept for 6 days".
pub fn path_label(info: &PathInfo, now: i64) -> String {
    let via = info.via.map(|via| format!(" via <{}…>", &hex::encode(via)[..8])).unwrap_or_default();
    format!("{}{via} on {}, kept for {}", hops_label(info.hops), info.interface, kept_label(info.expires, now))
}

/// How finding a path went, or is going, in words.
pub fn lookup_label(lookup: &PathLookup, now: i64) -> String {
    match lookup {
        PathLookup::Waiting(None) => "looking for a path…".into(),
        PathLookup::Waiting(Some(how)) => format!("looking for a path: {how}…"),
        PathLookup::Done(Ok(info)) => path_label(info, now),
        PathLookup::Done(Err(e)) => e.clone(),
    }
}

impl App {
    /// Find the path to `key` (any destination's address), asking for one
    /// if it isn't known. One at a time each; [`App::path_lookups`] has how
    /// it went.
    pub fn find_path_to(&mut self, key: &str) -> Result<(), String> {
        let hash = parse_hash(key).ok_or("An address is 32 hex characters")?;
        let key = hex::encode(hash);
        if matches!(self.path_lookups.get(&key), Some(PathLookup::Waiting(_))) {
            return Ok(());
        }
        self.path_lookups.insert(key.clone(), PathLookup::Waiting(None));
        self.send(NetCommand::FindPath(hash));
        self.confirm(format!("Finding a path to {}…", self.store.display_name(&key)));
        Ok(())
    }

    /// Forget the path to `key`, so the next use asks for a fresh one.
    pub fn forget_path(&mut self, key: &str) -> Result<(), String> {
        let hash = parse_hash(key).ok_or("An address is 32 hex characters")?;
        self.path_lookups.remove(&hex::encode(hash));
        self.send(NetCommand::ForgetPath(hash));
        Ok(())
    }

    /// Probe `key` (any destination's address), as `rnprobe` does. One at a
    /// time each; [`App::probes`] has how it went.
    pub fn probe(&mut self, key: &str) -> Result<(), String> {
        let hash = parse_hash(key).ok_or("An address is 32 hex characters")?;
        let key = hex::encode(hash);
        if matches!(self.probes.get(&key), Some(ProbeState::Waiting(_))) {
            return Ok(());
        }
        self.probes.insert(key.clone(), ProbeState::Waiting(None));
        self.send(NetCommand::Probe(hash));
        self.confirm(format!("Probing {}…", self.store.display_name(&key)));
        Ok(())
    }

    pub(super) fn on_path_progress(&mut self, to: Hash, text: String) {
        let key = hex::encode(to);
        let name = self.store.display_name(&key);
        if let Some(PathLookup::Waiting(how)) = self.path_lookups.get_mut(&key) {
            *how = Some(text.clone());
            self.confirm(format!("Finding a path to {name}: {text}…"));
        }
        if let Some(ProbeState::Waiting(how)) = self.probes.get_mut(&key) {
            *how = Some(text.clone());
            self.confirm(format!("Probing {name}: {text}…"));
        }
    }

    pub(super) fn on_probed(&mut self, to: Hash, result: Result<Probe, String>) {
        let key = hex::encode(to);
        let name = self.store.display_name(&key);
        match &result {
            Ok(probe) => self.notify(format!("Probe {name}: {}", probe_label(probe))),
            Err(e) => self.warn(format!("Probe {name}: {e}")),
        }
        self.probes.insert(key, ProbeState::Done(result));
    }

    pub(super) fn on_path(&mut self, to: Hash, result: Result<PathInfo, String>) {
        let key = hex::encode(to);
        let name = self.store.display_name(&key);
        match &result {
            Ok(info) => self.notify(format!("Path to {name}: {}", path_label(info, chrono::Utc::now().timestamp()))),
            Err(e) => self.warn(format!("Path to {name}: {e}")),
        }
        self.path_lookups.insert(key, PathLookup::Done(result));
    }

    pub(super) fn on_path_forgotten(&mut self, to: Hash, had: bool) {
        let name = self.store.display_name(&hex::encode(to));
        if had {
            self.notify(format!("Forgot the path to {name}: the next use asks for a fresh one"));
        } else {
            self.confirm(format!("No path to {name} was known"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use crate::net::NetEvent;
    use crate::store::Store;

    #[test]
    fn paths_found_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("rettui-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        let key = "ab".repeat(16);
        let to = parse_hash(&key).unwrap();
        assert!(app.find_path_to("nonsense").is_err());
        // Pasted as `<hash>` it's the same address.
        app.find_path_to(&format!("<{key}>")).unwrap();
        assert_eq!(app.path_lookups.get(&key), Some(&PathLookup::Waiting(None)));
        app.on_net(NetEvent::PathProgress { to, text: "path request 2 of 3".into() });
        assert_eq!(lookup_label(&app.path_lookups[&key], 0), "looking for a path: path request 2 of 3…");
        let now = chrono::Utc::now().timestamp();
        let info = PathInfo { hops: 3, via: Some([0x0a; 16]), interface: "RMAP World".into(), expires: now + 6 * 86_400 + 60 };
        app.on_net(NetEvent::Path { to, result: Ok(info.clone()) });
        assert_eq!(app.path_lookups[&key], PathLookup::Done(Ok(info.clone())));
        assert!(app.notice.as_ref().unwrap().text.ends_with("3 hops via <0a0a0a0a…> on RMAP World, kept for 6 days"), "{}", app.notice.as_ref().unwrap().text);
        let direct = PathInfo { hops: 1, via: None, interface: "Auto".into(), expires: now + 90 * 60 };
        assert_eq!(path_label(&direct, now), "heard directly on Auto, kept for 1 h");
        app.on_net(NetEvent::Path { to, result: Err("No path found".into()) });
        assert_eq!(lookup_label(&app.path_lookups[&key], now), "No path found");
        // Forgetting: said, and the lookup goes.
        app.forget_path(&key).unwrap();
        assert!(!app.path_lookups.contains_key(&key));
        app.on_net(NetEvent::PathForgotten { to, had: true });
        assert!(app.notice.as_ref().unwrap().text.contains("the next use asks for a fresh one"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probes_say_how_they_were_answered() {
        let dir = std::env::temp_dir().join(format!("rettui-probes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        let key = "cd".repeat(16);
        let to = parse_hash(&key).unwrap();
        app.probe(&key).unwrap();
        assert_eq!(app.probes.get(&key), Some(&ProbeState::Waiting(None)));
        app.on_net(NetEvent::PathProgress { to, text: "path request 3 of 3".into() });
        assert_eq!(probe_state_label(&app.probes[&key]), "looking for a path: path request 3 of 3…");
        let rtt = std::time::Duration::from_millis(182);
        let by_packet = Probe { rtt, hops: Some(2), kind: Some("LXMF address"), by_link: false };
        app.on_net(NetEvent::Probed { to, result: Ok(by_packet) });
        assert!(app.notice.as_ref().unwrap().text.ends_with("answered in 182 ms, 2 hops away (LXMF address)"));
        let by_link = Probe { rtt, hops: Some(1), kind: Some("NomadNet node"), by_link: true };
        assert_eq!(probe_label(&by_link), "a Link opened in 182 ms, heard directly (NomadNet node)");
        app.on_net(NetEvent::Probed { to, result: Err("No answer".into()) });
        assert_eq!(probe_state_label(&app.probes[&key]), "No answer");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
