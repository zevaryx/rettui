//! Announcing again when an interface comes online, as Sideband does. An
//! announce only reaches those connected when it goes out: one made as
//! rettui starts, before a slow connection (an entry point over the
//! internet) is up, reaches no one through it, and the next comes hours
//! later. That's so for interfaces interface discovery connects to, and for
//! ones that drop what's sent while they're down; rsReticulum's TCP client
//! keeps it and sends it on connecting, so for one of those that was down,
//! this repeats the announce. An interface that comes back soon after an
//! announce went out on it (one going up and down) doesn't bring another,
//! since public gateways hold back destinations that announce too often.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How long after an announce went out on an interface its coming back
/// online doesn't bring another.
const QUIET: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Default)]
pub struct Reannounce {
    /// The interfaces online at the last look.
    online: HashSet<String>,
    /// When an announce last went out on each.
    announced: HashMap<String, Instant>,
}

impl Reannounce {
    /// An announce went out on the interfaces `online` at `now`.
    pub fn announced(&mut self, online: impl IntoIterator<Item = String>, now: Instant) {
        for name in online {
            self.announced.insert(name, now);
        }
    }

    /// The interfaces `online` now: of those, the ones that came online
    /// since the last look and haven't had an announce lately, which call
    /// for one.
    pub fn look(&mut self, online: HashSet<String>, now: Instant) -> Vec<String> {
        let mut up: Vec<String> = online
            .difference(&self.online)
            .filter(|name| self.announced.get(*name).is_none_or(|at| now.duration_since(*at) >= QUIET))
            .cloned()
            .collect();
        up.sort();
        self.online = online;
        up
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn an_interface_coming_online_calls_for_an_announce_now_and_then() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut reannounce = Reannounce::default();
        // Before the announce at start: the Auto interface is up.
        assert_eq!(reannounce.look(set(&["Auto"]), at(0)), ["Auto"]);
        reannounce.announced(["Auto".to_string()], at(3));
        // The entry point connects after it.
        assert_eq!(reannounce.look(set(&["Auto", "RMAP World"]), at(5)), ["RMAP World"]);
        reannounce.announced(["Auto".to_string(), "RMAP World".to_string()], at(5));
        assert!(reannounce.look(set(&["Auto", "RMAP World"]), at(10)).is_empty(), "still up");
        // Going down and up again soon after: no announce.
        assert!(reannounce.look(set(&["Auto"]), at(60)).is_empty());
        assert!(reannounce.look(set(&["Auto", "RMAP World"]), at(120)).is_empty());
        // Later on, an announce again.
        assert!(reannounce.look(set(&["Auto"]), at(1000)).is_empty());
        assert_eq!(reannounce.look(set(&["Auto", "RMAP World"]), at(5 + 30 * 60)), ["RMAP World"]);
        // One that was up when an announce went out, and came back later.
        let mut reannounce = Reannounce::default();
        reannounce.announced(["Ratspeak".to_string()], at(3));
        assert!(reannounce.look(set(&["Ratspeak"]), at(5)).is_empty(), "the announce went out on it");
    }
}
