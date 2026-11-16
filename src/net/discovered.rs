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
//! entry points and drops one down for [`DETACH_AFTER`] and tries another.
//! The interfaces in the config are never dropped for the ones found,
//! unless *Disconnect Bootstrap only interfaces* (a setting, off) is on: then
//! those the user marked Bootstrap only are dropped once that many found
//! ones are up, and brought back when none is, as Python Reticulum does.
//! Off, what is found adds to the user's entry points and doesn't replace
//! them: anyone can announce an entry point. Decided here; [`super`] does it.
//!
//! What is found is only trusted as far as what it names: a name is
//! resolved here, every address it gives is looked at, and the one
//! connected to is the one looked at ([`resolve_checked`]).

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use rns_transport::discovery::{DiscoveredInterface, DiscoveryStatus};
use tokio::sync::mpsc;

/// How long a found entry point may be down before it's dropped (Python's
/// `InterfaceDiscovery.DETACH_THRESHOLD`, 12 s). A new one has that long
/// to connect.
pub const DETACH_AFTER: Duration = Duration::from_secs(12);
/// How long one dropped isn't tried again. Python tries it again at once;
/// the interfaces in the config are there meanwhile, so no hurry.
pub const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);
/// How long a name may take to resolve before it isn't connected to.
const RESOLVE_WITHIN: Duration = Duration::from_secs(10);

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
    /// The Bootstrap only interfaces to drop once enough found ones are up:
    /// none unless the user asked for it.
    bootstrap: Vec<Bootstrap>,
    bootstrap_dropped: bool,
    /// By endpoint (`host:port`).
    connections: HashMap<String, Connection>,
    /// When each was last dropped.
    dropped: HashMap<String, Instant>,
    /// Endpoints the config connects to already.
    configured: HashSet<String>,
    /// How connecting went, from the tasks that resolve names and connect
    /// (a name may take its time to resolve, which nothing else waits for).
    outcomes: mpsc::UnboundedReceiver<(Candidate, Result<u64, String>)>,
    outcome_sender: mpsc::UnboundedSender<(Candidate, Result<u64, String>)>,
}

impl Manager {
    /// `bootstrap`: the Bootstrap only interfaces that are dropped for the
    /// ones found (none leaves them all alone); `configured`: the hosts and
    /// ports the config's interfaces connect to, which aren't connected to
    /// twice.
    pub fn new(target: usize, bootstrap: Vec<Bootstrap>, configured: &[(String, u16)]) -> Self {
        let (outcome_sender, outcomes) = mpsc::unbounded_channel();
        Self {
            outcomes,
            outcome_sender,
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

/// The entry points found that may be connected to: ones run by transport
/// nodes, heard from in the last 3 days, reachable over TCP at an address this
/// device can try. Best first: those known longest (so one that announces
/// now doesn't displace those found before it), nearest. Not by the work in
/// the stamp, which rsReticulum ranks by (and by when it was last heard):
/// the work that counts is the least that is asked, any more is for whoever
/// is willing to do it, and the stamp can be re-used for ever.
pub fn candidates(found: &[DiscoveredInterface]) -> Vec<Candidate> {
    let mut found: Vec<&DiscoveredInterface> = found.iter().filter(|record| record.status != Some(DiscoveryStatus::Stale)).collect();
    found.sort_by_key(|record| (std::cmp::Reverse(record.status.map_or(0, DiscoveryStatus::code)), record.discovered, record.hops));
    found
        .into_iter()
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
/// Yggdrasil address (unless Yggdrasil runs here), a Tor one. Read as an
/// address, not as text: anyone can announce an interface, and
/// `127.1.2.3`, `[::ffff:127.0.0.1]` or `LocalHost.` would otherwise point
/// rettui at what listens on this device. Other networks' addresses (a LAN)
/// stay: interfaces there are found this way too.
///
/// This looks at what is announced, so that nothing is resolved for what
/// can't be connected to anyway. A name can say what it likes about where it
/// points: that is for [`resolve_checked`], which all that is connected to
/// goes through.
pub(crate) fn reachable(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") || host.ends_with(".onion") {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => address_ok(ip),
        // Not an address, so a name: but not one that is only digits or
        // hexadecimal (`127.1`, `0x7f.1`, `2130706433`: addresses as some
        // resolvers read them, though no name ends so), nor with a colon or
        // a zone in it.
        Err(_) => !(host.contains([':', '%']) || host.rsplit('.').next().is_some_and(spelt_as_a_number)),
    }
}

/// A label of digits, or `0x` and hexadecimal digits.
fn spelt_as_a_number(label: &str) -> bool {
    label.chars().all(|c| c.is_ascii_digit()) || label.strip_prefix("0x").is_some_and(|hex| hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether an address may be connected to for an entry point: not this
/// device, nowhere, a Yggdrasil address, or one of those in IPv6 clothes.
fn address_ok(ip: IpAddr) -> bool {
    let here = |ip: IpAddr| ip.is_loopback() || ip.is_unspecified();
    match ip {
        IpAddr::V4(v4) => !here(ip) && v4.octets()[0] != 0,
        IpAddr::V6(v6) => {
            !here(ip)
                && !v6.to_ipv4_mapped().is_some_and(|v4| here(v4.into()) || v4.octets()[0] == 0)
                && !matches!(v6.octets()[0], 0x02 | 0x03)
        }
    }
}

/// The address to connect to for an entry point at `host`: the name
/// resolved here (an address is its own answer), every address it gives
/// looked at, and one of those, IPv4 first, returned. Connecting to that
/// address and not to the name leaves the name no second say: it may answer
/// differently when asked again, or to the one that asked first.
pub(crate) async fn resolve_checked(host: &str, port: u16) -> Result<IpAddr, String> {
    let host = host.trim_matches(['[', ']']);
    let found = tokio::time::timeout(RESOLVE_WITHIN, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| "its name didn't resolve in time".to_string())?
        .map_err(|e| format!("its name didn't resolve: {e}"))?;
    pick_address(&found.map(|address| address.ip()).collect::<Vec<_>>())
}

/// What [`resolve_checked`] does with the addresses a name gave.
fn pick_address(found: &[IpAddr]) -> Result<IpAddr, String> {
    if let Some(bad) = found.iter().find(|ip| !address_ok(**ip)) {
        return Err(format!("its name points to {bad}, which is this device or a network rettui doesn't connect to"));
    }
    found.iter().find(|ip| ip.is_ipv4()).or(found.first()).copied().ok_or_else(|| "its name gave no address".to_string())
}

/// The config's Bootstrap only interfaces that can be brought back (TCP
/// clients), the others marked so (left alone), and the hosts and ports
/// its interfaces connect to.
pub fn from_config(config: &rns_runtime::config::Config) -> (Vec<Bootstrap>, Vec<String>, Vec<(String, u16)>) {
    let mut bootstrap = Vec::new();
    let mut others = Vec::new();
    let mut configured = Vec::new();
    for (name, section) in config.subsections("interfaces") {
        let on = section.interface_enabled();
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
    drop_bootstrap: bool,
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
    if target > 0 && drop_bootstrap {
        for name in others {
            log(format!("{name} is Bootstrap only, but only a TCP client interface can be brought back if it goes: it stays connected"));
        }
    }
    Some(Manager::new(target as usize, if drop_bootstrap { bootstrap } else { Vec::new() }, &configured))
}

/// Connect to `candidate`, at the address its name resolves to when
/// [`resolve_checked`] has looked at them all. The interface's id, or why not.
async fn connect(
    runtime: &rns_runtime::prelude::ReticulumHandle,
    candidate: &Candidate,
    ifac: Option<rns_runtime::reticulum::RuntimeInterfaceIfacConfig>,
) -> Result<u64, String> {
    let address = resolve_checked(&candidate.host, candidate.port).await?;
    let name = candidate.interface_name();
    rns_runtime::reticulum::spawn_backbone_client_runtime_with_ifac(
        runtime,
        rns_runtime::reticulum::RuntimeBackboneClientConfig {
            name: &name,
            host: &address.to_string(),
            port: candidate.port,
            prefer_ipv6: false,
            connect_timeout: None,
            max_reconnect_tries: None,
            ifac,
        },
    )
    .await
    .map_err(|e| e.to_string())
}

/// Look again (every few seconds), and do what's called for.
pub async fn tick(
    runtime: &rns_runtime::prelude::ReticulumHandle,
    manager: &mut Manager,
    interfaces: &[rns_transport::messages::InterfaceStatRpcEntry],
    log: &dyn Fn(String),
) {
    use rns_runtime::reticulum::{RuntimeInterfaceIfacConfig, spawn_tcp_client_runtime_with_ifac, teardown_interface};
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
    while let Ok((candidate, result)) = manager.outcomes.try_recv() {
        if let Err(e) = &result {
            log(format!("Couldn't connect to {}: {e}", candidate.name));
        }
        manager.connected(&candidate, result, now);
    }
    for step in manager.tick(now, &seen, &found) {
        match step {
            Step::Connect(candidate) => {
                log(format!(
                    "Connecting to {} ({}:{}), an entry point interface discovery found",
                    candidate.name, candidate.host, candidate.port
                ));
                let ifac = ifac(&candidate.network_name, &candidate.passphrase, None);
                let (runtime, sender) = (runtime.clone(), manager.outcome_sender.clone());
                // Its name is resolved first, which may take a while: not
                // waited for here.
                tokio::spawn(async move {
                    let result = connect(&runtime, &candidate, ifac).await;
                    let _ = sender.send((candidate, result));
                });
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
    fn the_interfaces_in_the_config_stay_whatever_is_found_unless_asked_otherwise() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut manager = Manager::new(2, Vec::new(), &[("hub.example".into(), 4242)]);
        let (a, b, c) = (found("A", "a.example"), found("B", "b.example"), found("C", "c.example"));
        let mut next = 10;
        // Nothing found yet.
        let hub = Seen { id: 1, name: "Hub", online: true };
        assert!(manager.tick(at(0), &[hub], &[]).is_empty());
        assert!(manager.wants_candidates());
        // Two found: connected to, best first. The hub itself, if it's
        // found too, isn't connected to twice.
        let steps = manager.tick(at(5), &[hub], &[found("Hub", "HUB.example"), a.clone(), b.clone(), c.clone()]);
        assert_eq!(connect(&mut manager, &steps, &mut next, at(5)), ["A", "B"]);
        assert!(!manager.wants_candidates());
        // Both up: the hub stays, as it does when they go down again.
        let seen =
            |a: bool, b: bool| [hub, Seen { id: 11, name: "Discovered/A", online: a }, Seen { id: 12, name: "Discovered/B", online: b }];
        assert!(manager.tick(at(10), &seen(true, false), &[]).is_empty());
        assert!(manager.tick(at(15), &seen(true, true), &[]).is_empty(), "the interfaces the user has aren't dropped for those found");
        assert!(manager.tick(at(20), &seen(false, false), &[]).is_empty());
        // 12 s on they're dropped, and the next found is tried, not those
        // just dropped.
        let steps = manager.tick(at(33), &seen(false, false), &[a.clone(), b.clone(), c.clone()]);
        for step in [
            Step::Drop { id: 11, name: "Discovered/A".into() },
            Step::Drop { id: 12, name: "Discovered/B".into() },
            Step::Connect(c.clone()),
        ] {
            assert!(steps.contains(&step), "{step:?} in {steps:?}");
        }
        assert_eq!(steps.len(), 3, "{steps:?}");
        // Dropped ones are tried again later.
        let steps = manager.tick(at(33 + RETRY_AFTER.as_secs()), &[hub], std::slice::from_ref(&a));
        assert!(steps.contains(&Step::Connect(a)), "{steps:?}");
    }

    #[test]
    fn one_that_never_comes_up_is_replaced() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut manager = Manager::new(1, Vec::new(), &[]);
        let (dead, live) = (found("Dead", "203.0.113.1"), found("Live", "live.example"));
        let mut next = 10;
        let steps = manager.tick(at(0), &[], &[dead.clone(), live.clone()]);
        assert_eq!(connect(&mut manager, &steps, &mut next, at(0)), ["Dead"]);
        let with = |online| [Seen { id: 11, name: "Discovered/Dead", online }];
        assert!(manager.tick(at(5), &with(false), &[]).is_empty());
        let steps = manager.tick(at(17), &with(false), &[dead.clone(), live.clone()]);
        assert_eq!(steps[0], Step::Drop { id: 11, name: "Discovered/Dead".into() });
        assert_eq!(connect(&mut manager, &steps, &mut next, at(17)), ["Live"]);
        // It comes up: nothing more to do.
        let up = [Seen { id: 12, name: "Discovered/Live", online: true }];
        assert!(manager.tick(at(22), &up, &[]).is_empty());
        // A failed connect is like a drop.
        let mut manager = Manager::new(1, Vec::new(), &[]);
        let steps = manager.tick(at(0), &[], std::slice::from_ref(&dead));
        let Step::Connect(candidate) = &steps[0] else { panic!("{steps:?}") };
        manager.connected(candidate, Err("refused".into()), at(0));
        assert!(manager.tick(at(5), &[], std::slice::from_ref(&dead)).is_empty());
    }

    #[test]
    fn lowering_the_number_drops_some() {
        let start = Instant::now();
        let mut manager = Manager::new(2, Vec::new(), &[]);
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
        assert_eq!(manager.tick(start, &seen, &[]), [Step::Drop { id: 11, name: "Discovered/A".into() }]);
    }

    #[test]
    fn asked_to_the_bootstrap_only_interfaces_go_once_enough_are_up_and_come_back_when_none_is() {
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
        assert!(reachable("192.168.1.20") && reachable("[fd00::5]"), "another computer on the LAN");
        let here =
            ["127.0.0.1", "127.1.2.3", "0.0.0.0", "::1", "[::1]", "::", "::ffff:127.0.0.1", "[::ffff:0.0.0.0]", "localhost", "LocalHost."];
        // Names that point here are for the resolver to find out; addresses
        // spelt as the older resolvers read them aren't names.
        let numbers =
            ["127.1", "127.0.1", "0x7f.1", "0x7f.0.0.1", "0x7f000001", "2130706433", "017700000001", "0177.0.0.1", "0", "0.1.2.3"];
        for host in
            here.into_iter().chain(numbers).chain(["a.localhost", "200:1234::1", "300:1::2", "abc.onion", "ABC.ONION.", "", "::1%lo"])
        {
            assert!(!reachable(host), "{host}");
        }
        for name in ["localtest.me", "127.0.0.1.nip.io", "my-laptop.local", "a.b.c", "hub0.example.org", "0xff.example", "7.example"] {
            assert!(reachable(name), "{name} is a name");
        }
        let text = "[reticulum]\n  discover_interfaces = Yes\n[interfaces]\n  [[Hub]]\n    type = TCPClientInterface\n    enabled = Yes\n    target_host = hub.example\n    target_port = 4242\n    bootstrap_only = Yes\n    network_name = mesh\n  [[Unsaid]]\n    type = TCPClientInterface\n    target_host = unsaid.example\n    target_port = 4245\n    bootstrap_only = Yes\n  [[Mine]]\n    type = TCPClientInterface\n    enabled = Yes\n    target_host = mine.example\n    target_port = 4243\n  [[Radio]]\n    type = RNodeInterface\n    enabled = No\n    bootstrap_only = Yes\n  [[Back]]\n    type = BackboneInterface\n    enabled = Yes\n    target_host = back.example\n    target_port = 4244\n    bootstrap_only = Yes\n";
        let config = rns_runtime::config::Config::parse(text).unwrap();
        let (bootstrap, others, configured) = from_config(&config);
        assert_eq!(bootstrap.len(), 1);
        assert_eq!((bootstrap[0].name.as_str(), bootstrap[0].port, bootstrap[0].network_name.as_deref()), ("Hub", 4242, Some("mesh")));
        assert_eq!(others, ["Back"], "a TCP client only can be brought back; one off (or that doesn't say it's on) is left out");
        assert_eq!(configured.len(), 4);
    }

    #[test]
    fn the_address_a_name_gives_is_looked_at_and_the_one_looked_at_is_used() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        assert_eq!(pick_address(&[ip("203.0.113.9")]), Ok(ip("203.0.113.9")));
        assert_eq!(pick_address(&[ip("2001:db8::1"), ip("203.0.113.9")]), Ok(ip("203.0.113.9")), "IPv4 first");
        assert_eq!(pick_address(&[ip("2001:db8::1")]), Ok(ip("2001:db8::1")));
        // One of them is here: the name isn't used, whatever else it says.
        for bad in ["127.0.0.1", "127.8.8.8", "0.0.0.0", "::1", "::ffff:127.0.0.1", "200::1", "0.1.2.3"] {
            assert!(pick_address(&[ip("203.0.113.9"), ip(bad)]).is_err(), "{bad}");
            assert!(pick_address(&[ip(bad), ip("203.0.113.9")]).is_err(), "{bad}");
        }
        assert!(pick_address(&[]).is_err());
    }

    #[tokio::test]
    async fn names_are_resolved_and_checked_before_they_are_connected_to() {
        // An address is its own answer, and this device's is refused.
        assert_eq!(resolve_checked("203.0.113.9", 4242).await, Ok("203.0.113.9".parse().unwrap()));
        assert_eq!(resolve_checked("[2001:db8::1]", 4242).await, Ok("2001:db8::1".parse().unwrap()));
        // Names that the filter on the text lets through, and numbers
        // spelt as older resolvers read them, whichever this one does.
        for host in ["127.0.0.1", "::1", "localhost", "LocalHost.", "127.1", "0x7f.1", "2130706433", "0"] {
            let why = resolve_checked(host, 4242).await.unwrap_err();
            assert!(why.contains("points to") || why.contains("didn't resolve"), "{host}: {why}");
        }
    }

    #[test]
    fn found_entry_points_are_ranked_by_how_long_they_have_been_known_not_by_work() {
        use rns_transport::discovery::app_data::DiscoveryInfo;
        let record = |name: &str, host: &str, stamp_value: u8, discovered: u64, last_heard: u64, hops: u8| DiscoveredInterface {
            info: DiscoveryInfo {
                name: name.into(),
                interface_type: "BackboneInterface".into(),
                transport_enabled: true,
                reachable_on: Some(host.into()),
                port: Some(4242),
                ..DiscoveryInfo::default()
            },
            network_id: [0; 16],
            hops,
            stamp_value,
            stamp: Vec::new(),
            discovered,
            last_heard,
            heard_count: 1,
            status: Some(DiscoveryStatus::Available),
        };
        let found = [
            record("newcomer", "new.example", 40, 9_000, 9_999, 1),
            record("old, far", "far.example", 16, 1_000, 1_100, 6),
            record("old, near", "near.example", 16, 1_000, 1_000, 2),
            record("older", "older.example", 16, 500, 500, 9),
        ];
        let names: Vec<_> = candidates(&found).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["older", "old, near", "old, far", "newcomer"], "more work, or a newer announce, doesn't put one first");
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
