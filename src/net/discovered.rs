//! Connecting to the entry points interface discovery finds, the way
//! Python Reticulum does, with *Discovered entry points* (a setting) for
//! how many.
//!
//! rsReticulum can connect to them itself (`autoconnect_discovered_interfaces`
//! in its config), but it drops Bootstrap only interfaces as soon as it
//! starts connecting to that many, before any is up; never brings them
//! back; keeps one that never comes up for good, so it's never replaced;
//! and after a restart connects to the ones it remembers at once, dropping
//! the Bootstrap only ones again whether those are reachable or not. A new
//! user with a Bootstrap only entry point can be left with no connection.
//!
//! So rettui does it, with Reticulum's own auto-connect left at 0
//! (discovery still finds them): it connects to up to that many found
//! entry points, drops one down for [`DETACH_AFTER`] and tries another,
//! drops the Bootstrap only interfaces once that many are up, and brings
//! them back when none is. Decided here; [`super`] does it.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rns_transport::discovery::{DiscoveredInterface, DiscoveryStatus};

/// How long a found entry point may be down before it's dropped (Python's
/// `InterfaceDiscovery.DETACH_THRESHOLD`, 12 s). A new one has that long
/// to connect.
pub const DETACH_AFTER: Duration = Duration::from_secs(12);
/// How long one dropped isn't tried again. Python tries it again at once;
/// the Bootstrap only interfaces are back meanwhile, so no hurry.
pub const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// A Bootstrap only interface in the config: what bringing it back takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bootstrap {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub network_name: Option<String>,
    pub passphrase: Option<String>,
    /// As set; the config's default is 8 bytes.
    pub ifac_size: Option<usize>,
}

/// An entry point discovery found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// As it's announced.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub network_name: Option<String>,
    pub passphrase: Option<String>,
}

impl Candidate {
    fn endpoint(&self) -> String {
        endpoint(&self.host, self.port)
    }

    /// The interface's name, as rsReticulum names the ones it connects to.
    pub fn interface_name(&self) -> String {
        format!("Discovered/{}", self.name.replace('/', "_"))
    }
}

fn endpoint(host: &str, port: u16) -> String {
    format!("{}:{port}", host.to_ascii_lowercase())
}

/// An interface as it is now.
#[derive(Debug, Clone, Copy)]
pub struct Seen<'a> {
    pub id: u64,
    pub name: &'a str,
    pub online: bool,
}

/// What to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Connect(Candidate),
    /// A found one down too long (or one too many): drop it.
    Drop {
        id: u64,
        name: String,
    },
    /// Enough found ones are up: drop the Bootstrap only interfaces.
    DropBootstrap(Vec<(u64, String)>),
    /// None is: bring them back.
    RestoreBootstrap(Vec<Bootstrap>),
}

struct Connection {
    /// Until it's connected.
    id: Option<u64>,
    since_down: Option<Instant>,
}

pub struct Manager {
    /// How many found ones to keep; 0 drops them all.
    target: usize,
    bootstrap: Vec<Bootstrap>,
    bootstrap_dropped: bool,
    /// By endpoint (`host:port`).
    connections: HashMap<String, Connection>,
    /// When each was last dropped.
    dropped: HashMap<String, Instant>,
    /// Endpoints the config connects to already.
    configured: HashSet<String>,
}

impl Manager {
    /// `configured`: the hosts and ports the config's interfaces connect
    /// to, which aren't connected to twice.
    pub fn new(target: usize, bootstrap: Vec<Bootstrap>, configured: &[(String, u16)]) -> Self {
        Self {
            target,
            bootstrap,
            bootstrap_dropped: false,
            connections: HashMap::new(),
            dropped: HashMap::new(),
            configured: configured.iter().map(|(host, port)| endpoint(host, *port)).collect(),
        }
    }

    pub fn set_target(&mut self, target: usize) {
        self.target = target;
    }

    /// Whether a look at what discovery found is due (there's room for
    /// one more).
    pub fn wants_candidates(&self) -> bool {
        self.connections.len() < self.target
    }

    /// What to do, given the interfaces now and the entry points found
    /// (best first).
    pub fn tick(&mut self, now: Instant, interfaces: &[Seen], candidates: &[Candidate]) -> Vec<Step> {
        let mut steps = Vec::new();
        let mut up = Vec::new();
        let mut gone = Vec::new();
        for (key, connection) in &mut self.connections {
            let Some(id) = connection.id else { continue };
            match interfaces.iter().find(|i| i.id == id) {
                // Taken away by something else.
                None => gone.push(key.clone()),
                Some(seen) if seen.online => {
                    connection.since_down = None;
                    up.push(key.clone());
                }
                Some(seen) => {
                    let since = *connection.since_down.get_or_insert(now);
                    if now.duration_since(since) >= DETACH_AFTER {
                        steps.push(Step::Drop { id, name: seen.name.to_string() });
                        self.dropped.insert(key.clone(), now);
                        gone.push(key.clone());
                    }
                }
            }
        }
        for key in &gone {
            self.connections.remove(key);
        }
        // One too many (the number was lowered): the ones down go first.
        let mut extra = self.connections.len().saturating_sub(self.target);
        let mut keys: Vec<String> = self.connections.keys().cloned().collect();
        keys.sort_by_key(|key| (up.contains(key), key.clone()));
        for key in keys {
            if extra == 0 {
                break;
            }
            let Some(id) = self.connections[&key].id else { continue };
            let name = interfaces.iter().find(|i| i.id == id).map_or_else(|| key.clone(), |i| i.name.to_string());
            steps.push(Step::Drop { id, name });
            self.connections.remove(&key);
            up.retain(|k| *k != key);
            extra -= 1;
        }

        let up = up.len();
        if self.target > 0 && up >= self.target && !self.bootstrap_dropped {
            let ids: Vec<(u64, String)> =
                interfaces.iter().filter(|i| self.bootstrap.iter().any(|b| b.name == i.name)).map(|i| (i.id, i.name.to_string())).collect();
            if !ids.is_empty() {
                steps.push(Step::DropBootstrap(ids));
            }
            self.bootstrap_dropped = true;
        } else if up == 0 && self.bootstrap_dropped {
            // Any still there (it never went) isn't added twice.
            let back: Vec<Bootstrap> = self.bootstrap.iter().filter(|b| !interfaces.iter().any(|i| i.name == b.name)).cloned().collect();
            if !back.is_empty() {
                steps.push(Step::RestoreBootstrap(back));
            }
            self.bootstrap_dropped = false;
        }

        let room = self.target.saturating_sub(self.connections.len());
        let fresh = |c: &&Candidate| {
            let key = c.endpoint();
            !self.connections.contains_key(&key)
                && !self.configured.contains(&key)
                && self.dropped.get(&key).is_none_or(|at| now.duration_since(*at) >= RETRY_AFTER)
        };
        let mut picked = Vec::new();
        for candidate in candidates.iter().filter(fresh) {
            if picked.len() == room {
                break;
            }
            if !picked.iter().any(|c: &Candidate| c.endpoint() == candidate.endpoint()) {
                picked.push(candidate.clone());
            }
        }
        for candidate in picked {
            self.connections.insert(candidate.endpoint(), Connection { id: None, since_down: None });
            steps.push(Step::Connect(candidate));
        }
        steps
    }

    /// How connecting to `candidate` went: its interface, or why not.
    pub fn connected(&mut self, candidate: &Candidate, result: Result<u64, String>, now: Instant) {
        let key = candidate.endpoint();
        match result {
            Ok(id) => {
                if let Some(connection) = self.connections.get_mut(&key) {
                    connection.id = Some(id);
                }
            }
            Err(_) => {
                self.connections.remove(&key);
                self.dropped.insert(key, now);
            }
        }
    }
}

/// The entry points found that may be connected to, best first (as
/// rsReticulum lists them): ones run by transport nodes, heard from in
/// the last 3 days, reachable over TCP at an address this device can try.
pub fn candidates(found: &[DiscoveredInterface]) -> Vec<Candidate> {
    found
        .iter()
        .filter(|record| record.status != Some(DiscoveryStatus::Stale))
        .filter_map(|record| {
            let info = &record.info;
            if !info.transport_enabled || !matches!(info.interface_type.as_str(), "BackboneInterface" | "TCPServerInterface") {
                return None;
            }
            let host = info.reachable_on.clone()?;
            (reachable(&host)).then_some(Candidate {
                name: info.name.clone(),
                host,
                port: info.port?,
                network_name: info.ifac_netname.clone(),
                passphrase: info.ifac_netkey.clone(),
            })
        })
        .collect()
}

/// Not an address Python leaves alone: this device itself, anywhere, a
/// Yggdrasil address (unless Yggdrasil runs here), a Tor one.
fn reachable(host: &str) -> bool {
    if matches!(host, "127.0.0.1" | "0.0.0.0" | "::1" | "::") || host.ends_with(".onion") || host.is_empty() {
        return false;
    }
    match host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) => !matches!(v6.octets()[0], 0x02 | 0x03),
        _ => true,
    }
}

/// The config's Bootstrap only interfaces that can be brought back (TCP
/// clients), the others marked so (left alone), and the hosts and ports
/// its interfaces connect to.
pub fn from_config(config: &rns_runtime::config::Config) -> (Vec<Bootstrap>, Vec<String>, Vec<(String, u16)>) {
    let mut bootstrap = Vec::new();
    let mut others = Vec::new();
    let mut configured = Vec::new();
    for (name, section) in config.subsections("interfaces") {
        let on = section.get_bool("enabled").or_else(|| section.get_bool("interface_enabled")).unwrap_or(true);
        let kind = section.get("type").unwrap_or_default();
        let target = section.get("target_host").zip(section.get_uint("target_port").and_then(|p| u16::try_from(p).ok()));
        if let Some((host, port)) = target {
            configured.push((host.to_string(), port));
        }
        if !on || section.get_bool("bootstrap_only") != Some(true) {
            continue;
        }
        match target {
            Some((host, port)) if kind == "TCPClientInterface" => bootstrap.push(Bootstrap {
                name: name.to_string(),
                host: host.to_string(),
                port,
                network_name: section.get("networkname").or_else(|| section.get("network_name")).map(str::to_string),
                passphrase: section.get("passphrase").or_else(|| section.get("pass_phrase")).map(str::to_string),
                ifac_size: section.get_uint("ifac_size").map(|size| size as usize),
            }),
            _ => others.push(name.to_string()),
        }
    }
    (bootstrap, others, configured)
}

/// rettui's own Reticulum, set up to find entry points and not connect to
/// them itself: the manager, with `target` for how many. Otherwise none,
/// with why if a number is set.
pub fn start(
    runtime: &rns_runtime::prelude::ReticulumHandle,
    rns_config: Option<&str>,
    target: u32,
    log: &dyn Fn(String),
) -> Option<Manager> {
    let config = &runtime.config;
    let set = |why: &str| {
        if target > 0 {
            log(format!("Discovered entry points is {target}, but {why}"));
        }
    };
    if runtime.instance_mode == rns_runtime::reticulum::InstanceMode::Client {
        set("another program runs Reticulum here (the shared instance): finding entry points is up to its config");
        return None;
    }
    if !config.discover_interfaces {
        set("interface discovery is off in the Reticulum config (discover_interfaces)");
        return None;
    }
    if config.autoconnect_discovered_interfaces > 0 {
        set(&format!(
            "the Reticulum config connects to entry points it finds itself (autoconnect_discovered_interfaces = {}): rettui leaves that to it",
            config.autoconnect_discovered_interfaces
        ));
        return None;
    }
    let text = crate::reticulum::load(&crate::reticulum::config_path(rns_config)).ok()?.0;
    let parsed = rns_runtime::config::Config::parse(&text).ok()?;
    let (bootstrap, others, configured) = from_config(&parsed);
    if target > 0 {
        for name in others {
            log(format!("{name} is Bootstrap only, but only a TCP client interface can be brought back if it goes: it stays connected"));
        }
    }
    Some(Manager::new(target as usize, bootstrap, &configured))
}

/// Look again (every few seconds), and do what's called for.
pub async fn tick(
    runtime: &rns_runtime::prelude::ReticulumHandle,
    manager: &mut Manager,
    interfaces: &[rns_transport::messages::InterfaceStatRpcEntry],
    log: &dyn Fn(String),
) {
    use rns_runtime::reticulum::{
        RuntimeBackboneClientConfig, RuntimeInterfaceIfacConfig, spawn_backbone_client_runtime_with_ifac,
        spawn_tcp_client_runtime_with_ifac, teardown_interface,
    };
    let found = if manager.wants_candidates() { candidates(&runtime.discovered_interfaces().await) } else { Vec::new() };
    let seen: Vec<Seen> = interfaces.iter().map(|i| Seen { id: i.id, name: &i.name, online: i.online }).collect();
    let now = Instant::now();
    let ifac = |network_name: &Option<String>, passphrase: &Option<String>, size: Option<usize>| {
        (network_name.is_some() || passphrase.is_some()).then(|| RuntimeInterfaceIfacConfig {
            network_name: network_name.clone(),
            passphrase: passphrase.clone(),
            ifac_size: size,
        })
    };
    for step in manager.tick(now, &seen, &found) {
        match step {
            Step::Connect(candidate) => {
                let name = candidate.interface_name();
                log(format!(
                    "Connecting to {} ({}:{}), an entry point interface discovery found",
                    candidate.name, candidate.host, candidate.port
                ));
                let result = spawn_backbone_client_runtime_with_ifac(
                    runtime,
                    RuntimeBackboneClientConfig {
                        name: &name,
                        host: &candidate.host,
                        port: candidate.port,
                        prefer_ipv6: false,
                        connect_timeout: None,
                        max_reconnect_tries: None,
                        ifac: ifac(&candidate.network_name, &candidate.passphrase, None),
                    },
                )
                .await;
                if let Err(e) = &result {
                    log(format!("Couldn't connect to {}: {e}", candidate.name));
                }
                manager.connected(&candidate, result, now);
            }
            Step::Drop { id, name } => {
                log(format!("{name} isn't up: disconnected from it (another entry point found is tried)"));
                teardown_interface(runtime, id).await;
            }
            Step::DropBootstrap(ids) => {
                let names: Vec<&str> = ids.iter().map(|(_, name)| name.as_str()).collect();
                log(format!("Entry points found are up: disconnected from {} (Bootstrap only)", names.join(", ")));
                for (id, _) in &ids {
                    teardown_interface(runtime, *id).await;
                }
            }
            Step::RestoreBootstrap(entries) => {
                let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
                log(format!("No entry point found is up: connecting to {} again (Bootstrap only)", names.join(", ")));
                for entry in &entries {
                    // The config's default size, not the runtime's.
                    let ifac = ifac(&entry.network_name, &entry.passphrase, Some(entry.ifac_size.unwrap_or(8)));
                    if let Err(e) = spawn_tcp_client_runtime_with_ifac(runtime, &entry.name, &entry.host, entry.port, ifac).await {
                        log(format!("Couldn't connect to {} again: {e}", entry.name));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, port: u16) -> Bootstrap {
        Bootstrap { name: name.into(), host: "hub.example".into(), port, network_name: None, passphrase: None, ifac_size: None }
    }

    fn found(name: &str, host: &str) -> Candidate {
        Candidate { name: name.into(), host: host.into(), port: 4242, network_name: None, passphrase: None }
    }

    /// Connect what's asked, with ids from `next`.
    fn connect(manager: &mut Manager, steps: &[Step], next: &mut u64, now: Instant) -> Vec<String> {
        let mut names = Vec::new();
        for step in steps {
            if let Step::Connect(candidate) = step {
                *next += 1;
                manager.connected(candidate, Ok(*next), now);
                names.push(candidate.name.clone());
            }
        }
        names
    }

    #[test]
    fn bootstrap_goes_once_enough_are_up_and_comes_back_when_none_is() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut manager = Manager::new(2, vec![entry("Hub", 4242)], &[("hub.example".into(), 4242)]);
        let (a, b, c) = (found("A", "a.example"), found("B", "b.example"), found("C", "c.example"));
        let mut next = 10;
        // Nothing found yet: the hub stays.
        let hub = Seen { id: 1, name: "Hub", online: true };
        assert!(manager.tick(at(0), &[hub], &[]).is_empty());
        assert!(manager.wants_candidates());
        // Two found: connected to, best first. The hub itself, if it's
        // found too, isn't connected to twice.
        let steps = manager.tick(at(5), &[hub], &[found("Hub", "HUB.example"), a.clone(), b.clone(), c.clone()]);
        assert_eq!(connect(&mut manager, &steps, &mut next, at(5)), ["A", "B"]);
        assert!(!manager.wants_candidates());
        // Not up yet (connecting): the hub stays.
        let seen =
            |a: bool, b: bool| [hub, Seen { id: 11, name: "Discovered/A", online: a }, Seen { id: 12, name: "Discovered/B", online: b }];
        assert!(manager.tick(at(10), &seen(true, false), &[]).is_empty());
        // Both up: the hub goes.
        assert_eq!(manager.tick(at(15), &seen(true, true), &[]), [Step::DropBootstrap(vec![(1, "Hub".into())])]);
        let without_hub =
            |a: bool, b: bool| [Seen { id: 11, name: "Discovered/A", online: a }, Seen { id: 12, name: "Discovered/B", online: b }];
        assert!(manager.tick(at(20), &without_hub(true, true), &[]).is_empty());
        // Both go down: with none up the hub comes back at once (as in
        // Python); 12 s on they're dropped, and the next found is tried,
        // not those just dropped.
        assert_eq!(manager.tick(at(25), &without_hub(false, false), &[]), [Step::RestoreBootstrap(vec![entry("Hub", 4242)])]);
        assert!(manager.tick(at(30), &without_hub(false, false), &[]).is_empty(), "not brought back twice");
        let steps = manager.tick(at(37), &without_hub(false, false), &[a.clone(), b.clone(), c.clone()]);
        for step in [
            Step::Drop { id: 11, name: "Discovered/A".into() },
            Step::Drop { id: 12, name: "Discovered/B".into() },
            Step::Connect(c.clone()),
        ] {
            assert!(steps.contains(&step), "{step:?} in {steps:?}");
        }
        assert_eq!(steps.len(), 3, "{steps:?}");
        // Dropped ones are tried again later.
        let steps = manager.tick(at(37 + RETRY_AFTER.as_secs()), &[hub], std::slice::from_ref(&a));
        assert!(steps.contains(&Step::Connect(a)), "{steps:?}");
    }

    #[test]
    fn one_that_never_comes_up_is_replaced_and_the_bootstrap_stays() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut manager = Manager::new(1, vec![entry("Hub", 4242)], &[]);
        let (dead, live) = (found("Dead", "203.0.113.1"), found("Live", "live.example"));
        let hub = Seen { id: 1, name: "Hub", online: true };
        let mut next = 10;
        let steps = manager.tick(at(0), &[hub], &[dead.clone(), live.clone()]);
        assert_eq!(connect(&mut manager, &steps, &mut next, at(0)), ["Dead"]);
        let with = |online| [hub, Seen { id: 11, name: "Discovered/Dead", online }];
        assert!(manager.tick(at(5), &with(false), &[]).is_empty(), "the hub isn't dropped for one connecting");
        let steps = manager.tick(at(17), &with(false), &[dead.clone(), live.clone()]);
        assert_eq!(steps[0], Step::Drop { id: 11, name: "Discovered/Dead".into() });
        assert_eq!(connect(&mut manager, &steps, &mut next, at(17)), ["Live"]);
        // It comes up: then the hub goes.
        let up = [hub, Seen { id: 12, name: "Discovered/Live", online: true }];
        assert_eq!(manager.tick(at(22), &up, &[]), [Step::DropBootstrap(vec![(1, "Hub".into())])]);
        // A failed connect is like a drop.
        let mut manager = Manager::new(1, vec![], &[]);
        let steps = manager.tick(at(0), &[], std::slice::from_ref(&dead));
        let Step::Connect(candidate) = &steps[0] else { panic!("{steps:?}") };
        manager.connected(candidate, Err("refused".into()), at(0));
        assert!(manager.tick(at(5), &[], std::slice::from_ref(&dead)).is_empty());
    }

    #[test]
    fn lowering_the_number_drops_some_and_zero_brings_the_bootstrap_back() {
        let start = Instant::now();
        let mut manager = Manager::new(2, vec![entry("Hub", 1)], &[]);
        let mut next = 10;
        let steps = manager.tick(start, &[], &[found("A", "a.example"), found("B", "b.example")]);
        connect(&mut manager, &steps, &mut next, start);
        let seen = [Seen { id: 11, name: "Discovered/A", online: true }, Seen { id: 12, name: "Discovered/B", online: false }];
        manager.tick(start, &seen, &[]);
        manager.set_target(1);
        assert_eq!(manager.tick(start, &seen, &[]), [Step::Drop { id: 12, name: "Discovered/B".into() }], "the one down first");
        let seen = [Seen { id: 11, name: "Discovered/A", online: true }];
        assert!(manager.tick(start, &seen, &[]).is_empty());
        manager.set_target(0);
        assert_eq!(
            manager.tick(start, &seen, &[]),
            [Step::Drop { id: 11, name: "Discovered/A".into() }, Step::RestoreBootstrap(vec![entry("Hub", 1)])]
        );
    }

    #[test]
    fn what_may_be_connected_to_and_what_the_config_has() {
        assert!(reachable("rns.example.org") && reachable("203.0.113.5") && reachable("2001:db8::1"));
        for host in ["127.0.0.1", "0.0.0.0", "200:1234::1", "300:1::2", "abc.onion", ""] {
            assert!(!reachable(host), "{host}");
        }
        let text = "[reticulum]\n  discover_interfaces = Yes\n[interfaces]\n  [[Hub]]\n    type = TCPClientInterface\n    enabled = Yes\n    target_host = hub.example\n    target_port = 4242\n    bootstrap_only = Yes\n    network_name = mesh\n  [[Mine]]\n    type = TCPClientInterface\n    enabled = Yes\n    target_host = mine.example\n    target_port = 4243\n  [[Radio]]\n    type = RNodeInterface\n    enabled = No\n    bootstrap_only = Yes\n  [[Back]]\n    type = BackboneInterface\n    enabled = Yes\n    target_host = back.example\n    target_port = 4244\n    bootstrap_only = Yes\n";
        let config = rns_runtime::config::Config::parse(text).unwrap();
        let (bootstrap, others, configured) = from_config(&config);
        assert_eq!(bootstrap.len(), 1);
        assert_eq!((bootstrap[0].name.as_str(), bootstrap[0].port, bootstrap[0].network_name.as_deref()), ("Hub", 4242, Some("mesh")));
        assert_eq!(others, ["Back"], "a TCP client only can be brought back; one off is left out");
        assert_eq!(configured.len(), 3);
    }

    #[test]
    fn only_transport_nodes_reachable_over_tcp_are_tried() {
        use rns_transport::discovery::app_data::DiscoveryInfo;
        let record = |name: &str, kind: &str, transport: bool, host: Option<&str>, status: Option<DiscoveryStatus>| DiscoveredInterface {
            info: DiscoveryInfo {
                name: name.into(),
                interface_type: kind.into(),
                transport_enabled: transport,
                reachable_on: host.map(str::to_string),
                port: Some(4242),
                ifac_netname: Some("net".into()),
                ..DiscoveryInfo::default()
            },
            network_id: [0; 16],
            hops: 1,
            stamp_value: 0,
            stamp: Vec::new(),
            discovered: 0,
            last_heard: 0,
            heard_count: 1,
            status,
        };
        let found = [
            record("good", "BackboneInterface", true, Some("example.org"), None),
            record("tcp", "TCPServerInterface", true, Some("[2001:db8::1]"), None),
            record("stale", "BackboneInterface", true, Some("example.org"), Some(DiscoveryStatus::Stale)),
            record("no transport", "BackboneInterface", false, Some("example.org"), None),
            record("radio", "RNodeInterface", true, Some("example.org"), None),
            record("nowhere", "BackboneInterface", true, None, None),
            record("here", "BackboneInterface", true, Some("127.0.0.1"), None),
            record("tor", "BackboneInterface", true, Some("abc.onion"), None),
            record("yggdrasil", "BackboneInterface", true, Some("[200::1]"), None),
        ];
        let names: Vec<_> = candidates(&found).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["good", "tcp"]);
        assert_eq!(candidates(&found)[0].network_name.as_deref(), Some("net"));
    }
}
