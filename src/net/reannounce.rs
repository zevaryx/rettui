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

/// How many interfaces not online an announce is remembered for, within
/// `QUIET`.
const KEPT_OFFLINE: usize = 256;

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
        self.forget(now);
    }

    /// How many interfaces an announce is remembered for.
    #[cfg(test)]
    fn remembered(&self) -> usize {
        self.announced.len()
    }

    /// An announce older than `QUIET` holds nothing back, so it's not kept.
    /// Of those not online, no more than `KEPT_OFFLINE` are kept (the newest
    /// first): interfaces that come and go under new names (AutoInterface
    /// peers, thousands of them in a flood) would otherwise add to this for
    /// as long as rettui runs. One of those forgotten early may be announced
    /// on again when it returns within `QUIET`.
    fn forget(&mut self, now: Instant) {
        self.announced.retain(|_, at| now.saturating_duration_since(*at) < QUIET);
        let offline = self.announced.keys().filter(|name| !self.online.contains(*name)).count();
        if offline > KEPT_OFFLINE {
            let mut gone: Vec<(Instant, String)> =
                self.announced.iter().filter(|(name, _)| !self.online.contains(*name)).map(|(name, at)| (*at, name.clone())).collect();
            gone.sort();
            gone.truncate(gone.len().saturating_sub(KEPT_OFFLINE));
            for (_, name) in gone {
                self.announced.remove(&name);
            }
        }
        if self.announced.capacity() > 4 * self.announced.len().max(16) {
            self.announced.shrink_to_fit();
        }
    }

    /// The interfaces `online` now: of those, the ones that came online
    /// since the last look and haven't had an announce lately, which call
    /// for one.
    pub fn look(&mut self, online: HashSet<String>, now: Instant) -> Vec<String> {
        let mut up: Vec<String> = online
            .difference(&self.online)
            .filter(|name| self.announced.get(*name).is_none_or(|at| now.saturating_duration_since(*at) >= QUIET))
            .cloned()
            .collect();
        up.sort();
        self.online = online;
        self.forget(now);
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

    #[test]
    fn announces_older_than_the_quiet_time_are_forgotten() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut reannounce = Reannounce::default();
        // Peers that each come and go under a name of their own.
        for n in 0..200u64 {
            let name = format!("peer {n}");
            reannounce.look(HashSet::from([name.clone()]), at(n));
            reannounce.announced([name], at(n));
        }
        assert_eq!(reannounce.remembered(), 200, "all within the quiet time");
        reannounce.look(HashSet::new(), at(100 + 30 * 60));
        assert_eq!(reannounce.remembered(), 99, "only those 101 to 199 s old are kept");
        reannounce.look(HashSet::new(), at(2000 + 30 * 60));
        assert_eq!(reannounce.remembered(), 0);
        // Forgetting changes nothing for the interface: it was due one anyway.
        assert_eq!(reannounce.look(set(&["peer 5"]), at(2001 + 30 * 60)), ["peer 5"]);
    }

    #[test]
    fn one_within_the_quiet_time_is_kept_and_holds_back_the_announce() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut reannounce = Reannounce::default();
        reannounce.announced(["A".to_string()], at(0));
        reannounce.look(HashSet::new(), at(10 * 60));
        assert_eq!(reannounce.remembered(), 1);
        assert!(reannounce.look(set(&["A"]), at(20 * 60)).is_empty());
    }

    #[test]
    fn only_so_many_offline_interfaces_are_remembered_but_online_ones_all_are() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut reannounce = Reannounce::default();
        let names: Vec<String> = (0..5000).map(|n| format!("peer {n}")).collect();
        // A flood: all online and announced on.
        reannounce.look(names.iter().cloned().collect(), at(0));
        reannounce.announced(names.iter().cloned(), at(1));
        assert_eq!(reannounce.remembered(), 5000, "online ones are never dropped");
        // All gone within the quiet time: the newest few are kept.
        reannounce.look(set(&["Auto"]), at(60));
        assert_eq!(reannounce.remembered(), KEPT_OFFLINE);
        assert!(reannounce.announced.capacity() < 1000, "the table gives its memory back");
        // One kept still holds back an announce; a forgotten one gets it.
        reannounce.announced(["Auto".to_string()], at(61));
        assert_eq!(reannounce.remembered(), KEPT_OFFLINE + 1);
        let up = reannounce.look(set(&["Auto", "peer 1"]), at(120));
        assert_eq!(up, ["peer 1"], "forgotten, so due one again");
    }

    #[test]
    fn online_interfaces_not_announced_on_yet_dont_count_as_offline() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut reannounce = Reannounce::default();
        // More than KEPT_OFFLINE offline ones announced on, and online ones with no announce yet.
        let offline: Vec<String> = (0..KEPT_OFFLINE + 44).map(|n| format!("old {n}")).collect();
        reannounce.look(offline.iter().cloned().collect(), at(0));
        reannounce.announced(offline.iter().cloned(), at(1));
        let online: HashSet<String> = (0..100).map(|n| format!("new {n}")).collect();
        reannounce.look(online, at(60));
        assert_eq!(reannounce.remembered(), KEPT_OFFLINE, "the offline ones over the limit are dropped");
    }
}
