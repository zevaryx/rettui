//! The self-test of a Reticulum config change: once Reticulum is back with
//! it, the node dials each interface it has to keep that takes clients (a
//! `TCPServerInterface`, or a `BackboneInterface` that listens) as a
//! Reticulum client would, with the interface's framing and IFAC, and asks
//! for a path to its own LXMF address. Reticulum answers a path request for a
//! destination of its own at once, with an announce of it on the interface
//! the request came in on (rsReticulum's transport hands the request to the
//! destination, which announces as a path response; transport mode doesn't
//! matter). The test passes when an announce for that address, signed by the
//! node's identity, comes back as a path response made after the probe
//! began: a port that only accepts connections, a Reticulum that drops what
//! comes in on it, or an old announce played back, doesn't pass.
//!
//! An interface that can't be dialled (a radio, a serial line, an
//! AutoInterface, a client, a listener bound to a device or given by a
//! host name rather than an IP address, one that sends nothing, a Backbone
//! listener that counts connections as flapping for over two minutes, or
//! one whose path request stamps cost too much to make) only has to be online, and whether anything was heard
//! on it since the restart is said, for information.
//!
//! The probes run in a thread of their own, so the owner loop never waits on
//! the network; the owner reads their results on its ticks.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rns_runtime::config::{Config, ConfigSection};
use rns_runtime::interface_factory::{self, InterfaceConfig, InterfacePostInit};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::owner::{MAX_PROBLEM, cut};
use super::unix_now;
use crate::net::InterfaceInfo;

/// How long one test request waits for its answer before another is sent.
pub const ATTEMPT: Duration = Duration::from_secs(10);
/// The most checks `/state` lists.
pub const MAX_CHECKS: usize = 32;
/// Path request stamps are made up to this cost (a few milliseconds); an
/// interface that wants more is only checked to be online.
const MAX_STAMP_COST: f64 = 16.0;
/// The longest a stamp is searched for, for one request.
const STAMP_WITHIN: Duration = Duration::from_secs(5);
/// A Backbone listener that counts a connection as flapping for longer than
/// this is only checked to be online: the probe would have to stay that long.
const MAX_HOLD: Duration = Duration::from_secs(120);
/// How far an answer's announce time may be before the probe began (clock
/// steps within a second, rounding).
const ANNOUNCE_SLACK: u64 = 2;
/// The most connections one probe opens: a Backbone listener blocks an
/// address that connects and leaves too often.
const MAX_CONNECTIONS: usize = 3;
/// The longest frame taken from the listener, and the most bytes read from it
/// in one probe: what comes back is checked, not trusted.
const MAX_FRAME: usize = 64 * 1024;
const MAX_READ: usize = 8 * 1024 * 1024;
/// The aspect path requests are sent to (`rnstransport.path.request`).
const PATH_REQUEST: &str = "rnstransport.path.request";

/// What is checked of an interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Dialled, and asked for a path to the node's own address.
    SelfTest,
    /// Only has to be online (it can't be dialled).
    Online,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::SelfTest => "self_test",
            Kind::Online => "online",
        }
    }
}

/// How a check stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    Waiting,
    /// The test request was answered.
    Passed,
    /// It wasn't, within the time.
    Failed,
    /// Online, and something was heard on it since the restart.
    Traffic,
    /// Online, but nothing heard on it yet.
    NoTraffic,
}

impl Seen {
    pub fn as_str(self) -> &'static str {
        match self {
            Seen::Waiting => "waiting",
            Seen::Passed => "passed",
            Seen::Failed => "failed",
            Seen::Traffic => "traffic",
            Seen::NoTraffic => "no_traffic",
        }
    }
}

/// One interface's check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub interface: String,
    pub kind: Kind,
    pub seen: Seen,
}

/// An interface's IFAC: its key and the length of its tags.
#[derive(Clone)]
struct Ifac {
    key: zeroize::Zeroizing<[u8; 64]>,
    size: usize,
}

/// How to reach an interface's listener as a client.
#[derive(Clone)]
pub struct Dial {
    /// The address it listens on, with the any-address as loopback.
    address: SocketAddr,
    kiss: bool,
    ifac: Option<Ifac>,
    /// The cost of the stamp a path request needs, when stamps are enforced.
    stamp_cost: Option<f64>,
    /// How long a connection stays open at least, so a Backbone listener
    /// doesn't count it as flapping and block loopback.
    hold: Duration,
}

impl std::fmt::Debug for Dial {
    // The IFAC key is a secret of the config: never shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dial")
            .field("address", &self.address)
            .field("kiss", &self.kiss)
            .field("ifac", &self.ifac.as_ref().map(|ifac| ifac.size))
            .field("stamp_cost", &self.stamp_cost)
            .field("hold", &self.hold)
            .finish()
    }
}

/// Where the probes go (another address, or fixed answers, in tests).
#[derive(Debug, Clone, Default)]
pub enum Prober {
    /// Dial each interface where the config says it listens.
    #[default]
    Dial,
    /// Answer at once: passed for the interfaces named `true`, failed for
    /// those named `false`; the rest never answer.
    #[cfg(test)]
    Fixed(HashMap<String, bool>),
    /// Dial these addresses for these interfaces instead, while a change
    /// is tested (after a rollback, where the config says).
    #[cfg(test)]
    Redirect(HashMap<String, SocketAddr>),
}

/// The checks of one config, and the probes once started.
#[derive(Debug, Default)]
pub struct Run {
    checks: Vec<Check>,
    dials: HashMap<usize, Dial>,
    results: Option<mpsc::Receiver<(usize, bool)>>,
    started: bool,
}

impl Run {
    /// The checks of the interfaces `required` in the config `text`.
    pub fn plan(text: &str, required: &[String]) -> Self {
        let config = Config::parse(text).ok();
        let mut run = Run::default();
        for (index, name) in required.iter().enumerate() {
            let dial = config.as_ref().and_then(|config| dial_of(config, name));
            let kind = if dial.is_some() { Kind::SelfTest } else { Kind::Online };
            if let Some(dial) = dial {
                run.dials.insert(index, dial);
            }
            run.checks.push(Check { interface: name.clone(), kind, seen: Seen::Waiting });
        }
        run
    }

    pub fn started(&self) -> bool {
        self.started
    }

    /// Start the probes: ask for a path to `destination` (the node's LXMF
    /// address) on each interface that can be dialled, for up to `within`,
    /// one request per `attempt`. Without the address (Reticulum hasn't said
    /// it yet) only a run with nothing to dial starts. `rollback` says the
    /// config is the old one put back.
    pub fn start(&mut self, destination: Option<[u8; 16]>, within: Duration, attempt: Duration, prober: &Prober, rollback: bool) {
        let destination = match destination {
            Some(destination) => destination,
            None if self.dials.is_empty() => [0; 16],
            None => return,
        };
        self.started = true;
        let dials: Vec<(usize, Dial)> = self.dials.iter().map(|(index, dial)| (*index, dial.clone())).collect();
        let dials = match prober {
            Prober::Dial => dials,
            #[cfg(test)]
            Prober::Fixed(answers) => {
                for (index, _) in dials {
                    let check = &mut self.checks[index];
                    match answers.get(&check.interface) {
                        Some(true) => check.seen = Seen::Passed,
                        Some(false) => check.seen = Seen::Failed,
                        None => {}
                    }
                }
                return;
            }
            #[cfg(test)]
            Prober::Redirect(to) if !rollback => dials
                .into_iter()
                .map(|(index, mut dial)| {
                    if let Some(address) = to.get(&self.checks[index].interface) {
                        dial.address = *address;
                    }
                    (index, dial)
                })
                .collect(),
            #[cfg(test)]
            Prober::Redirect(_) => dials,
        };
        // Only redirected probes care whose config it is.
        let _ = rollback;
        if dials.is_empty() {
            return;
        }
        let started = unix_now();
        let (tx, rx) = mpsc::channel();
        self.results = Some(rx);
        let spawned = std::thread::Builder::new().name("rettui-self-test".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(e) => {
                    tracing::warn!("Remote management: the self-test couldn't start: {e}");
                    for (index, _) in dials {
                        let _ = tx.send((index, false));
                    }
                    return;
                }
            };
            runtime.block_on(async move {
                let mut probes = tokio::task::JoinSet::new();
                for (index, dial) in dials {
                    let tx = tx.clone();
                    probes.spawn(async move {
                        probe(&dial, destination, started, within, attempt, |passed| {
                            let _ = tx.send((index, passed));
                        })
                        .await
                    });
                }
                while probes.join_next().await.is_some() {}
            });
        });
        if let Err(e) = spawned {
            tracing::warn!("Remote management: the self-test couldn't start: {e}");
            for index in self.dials.keys() {
                self.checks[*index].seen = Seen::Failed;
            }
            self.results = None;
        }
    }

    /// Take in what the probes found, and what the interfaces that are only
    /// checked to be online have heard.
    pub fn poll(&mut self, interfaces: &[InterfaceInfo]) {
        if let Some(results) = &self.results {
            while let Ok((index, passed)) = results.try_recv() {
                if let Some(check) = self.checks.get_mut(index) {
                    check.seen = if passed { Seen::Passed } else { Seen::Failed };
                }
            }
            // A probe thread that ended without saying (it can't) failed.
            if matches!(results.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
                for index in self.dials.keys() {
                    if self.checks[*index].seen == Seen::Waiting {
                        self.checks[*index].seen = Seen::Failed;
                    }
                }
                self.results = None;
            }
        }
        for check in self.checks.iter_mut().filter(|c| c.kind == Kind::Online) {
            let found = interfaces.iter().find(|i| i.online && i.name == check.interface);
            check.seen = match found {
                None => Seen::Waiting,
                Some(i) if i.rx_bytes > 0 => Seen::Traffic,
                // Once heard, it stays heard.
                Some(_) if check.seen == Seen::Traffic => Seen::Traffic,
                Some(_) => Seen::NoTraffic,
            };
        }
    }

    /// End the run: what is still waiting failed (a self-test not answered,
    /// an interface not online).
    pub fn close(&mut self) {
        self.results = None;
        for check in self.checks.iter_mut().filter(|c| c.seen == Seen::Waiting) {
            check.seen = Seen::Failed;
        }
    }

    /// The first interface whose self-test failed.
    pub fn failed(&self) -> Option<&str> {
        self.checks.iter().find(|c| c.kind == Kind::SelfTest && c.seen == Seen::Failed).map(|c| c.interface.as_str())
    }

    /// The interfaces whose self-test isn't done yet.
    pub fn waiting(&self) -> impl Iterator<Item = &str> {
        self.checks.iter().filter(|c| c.kind == Kind::SelfTest && c.seen == Seen::Waiting).map(|c| c.interface.as_str())
    }

    /// The checks for `/state`: at most [`MAX_CHECKS`].
    pub fn to_json(&self) -> Value {
        Value::Array(
            self.checks
                .iter()
                .take(MAX_CHECKS)
                .map(|c| json!({ "interface": cut(&c.interface, MAX_PROBLEM), "kind": c.kind.as_str(), "result": c.seen.as_str() }))
                .collect(),
        )
    }

    /// The checks in a line, for the audit log; `None` without any.
    pub fn summary(&self) -> Option<String> {
        if self.checks.is_empty() {
            return None;
        }
        let said: Vec<String> = self
            .checks
            .iter()
            .take(MAX_CHECKS)
            .map(|c| format!("{} {} {}", cut(&c.interface, MAX_PROBLEM), c.kind.as_str(), c.seen.as_str()))
            .collect();
        Some(said.join("; "))
    }
}

/// The IP address a listener on `listen` is reached at from here: the
/// any-address (or nothing) as the loopback address of its family, any
/// other address as it is. A host name is `None`: it isn't looked up, as a
/// slow or changed answer would make the test say something of the resolver.
fn reachable(listen: &str) -> Option<IpAddr> {
    let listen = listen.trim();
    if listen.is_empty() {
        return Some(Ipv4Addr::LOCALHOST.into());
    }
    let ip: IpAddr = listen.strip_prefix('[').and_then(|l| l.strip_suffix(']')).unwrap_or(listen).parse().ok()?;
    Some(match ip {
        IpAddr::V4(v4) if v4.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(v6) if v6.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    })
}

/// How to dial the interface `name` of `config`, if it takes clients and
/// can be tested from here.
fn dial_of(config: &Config, name: &str) -> Option<Dial> {
    let section = config.subsection("interfaces", name)?;
    let interface = interface_factory::synthesize_interface(name, section).ok()?;
    let (host, port, kiss, hold) = match &interface {
        // Bound to a device, it listens on whatever address the device has.
        InterfaceConfig::TcpServer(server) if server.device.is_none() => {
            (server.listen_ip.as_deref()?, server.listen_port, server.kiss_framing, Duration::ZERO)
        }
        InterfaceConfig::Backbone(backbone) if backbone.target_host.is_none() && backbone.device.is_none() => {
            let hold = match backbone.block_fast_flapping {
                true if backbone.fast_flapping_threshold > MAX_HOLD => return None,
                true => backbone.fast_flapping_threshold + Duration::from_secs(1),
                false => Duration::ZERO,
            };
            (backbone.listen_on.as_deref()?, backbone.port, false, hold)
        }
        _ => return None,
    };
    let post_init =
        InterfacePostInit::try_from_section(section).ok()?.with_default_ifac_size(interface_factory::default_ifac_size_for(&interface));
    // An interface that sends nothing can't answer.
    if !post_init.outgoing {
        return None;
    }
    let ifac = if post_init.ifac_network_name.is_some() || post_init.ifac_passphrase.is_some() {
        let key = rns_identity::ifac::derive_ifac_key(
            post_init.ifac_network_name.as_deref(),
            post_init.ifac_passphrase.as_deref().map(String::as_str),
        )
        .ok()?;
        Some(Ifac { key: zeroize::Zeroizing::new(key), size: post_init.ifac_size.unwrap_or(post_init.default_ifac_size) })
    } else {
        None
    };
    let stamp_cost = match stamp_cost(config.section("stamps"), post_init.pr_cost) {
        Some(cost) if cost > MAX_STAMP_COST => return None,
        cost => cost,
    };
    Some(Dial { address: SocketAddr::new(reachable(host)?, port), kiss, ifac, stamp_cost, hold })
}

/// The cost of the stamp a path request needs on an interface: `None`
/// unless the config enforces stamps.
fn stamp_cost(stamps: Option<&ConfigSection>, interface_cost: Option<f64>) -> Option<f64> {
    use rns_transport::stamps::StampPolicy;
    let stamps = stamps?;
    if stamps.get_bool("enforce_stamps") != Some(true) {
        return None;
    }
    let global = stamps.get_float("pr_cost").map(StampPolicy::clamp_cost).unwrap_or(StampPolicy::default().pr_cost);
    Some(interface_cost.unwrap_or(global))
}

/// A path request for `destination`, with a fresh tag (a repeated one is
/// ignored), stamped if `stamp_cost` says so, as a packet; `None` if the
/// stamp wasn't found by `stamp_by`. A stamp is made over the tag, so each
/// request needs a new one: none can be used twice.
fn path_request(destination: [u8; 16], stamp_cost: Option<f64>, stamp_by: Instant) -> Option<Vec<u8>> {
    use rns_wire::flags::{DestinationType, HeaderType, PacketFlags, PacketType, TransportType};
    let tag: [u8; 16] = rand::random();
    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&destination);
    if let Some(cost) = stamp_cost {
        // Destination, a transport id (a client's own, random), tag, stamp.
        let mut material = destination.to_vec();
        material.extend_from_slice(&tag);
        let control = rns_identity::stamps::StampControl { deadline: Some(stamp_by), ..Default::default() };
        let (stamp, _) =
            rns_identity::stamps::generate_stamp(&material, cost, 0, rns_identity::stamps::NETWORK_STAMP_SIZE, &control).ok()??;
        data.extend_from_slice(&rand::random::<[u8; 16]>());
        data.extend_from_slice(&tag);
        data.extend_from_slice(&[rns_identity::announce::ANNOUNCE_EXT_MARKER, 0]);
        data.extend_from_slice(&stamp);
    } else {
        data.extend_from_slice(&tag);
    }
    let header = rns_wire::header::PacketHeader {
        flags: PacketFlags {
            header_type: HeaderType::Header1,
            context_flag: false,
            transport_type: TransportType::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
        },
        hops: 0,
        transport_id: None,
        destination_hash: rns_identity::destination::Destination::hash_from_name_and_identity(PATH_REQUEST, None),
        context: rns_wire::context::PacketContext::None,
    };
    let mut raw = header.pack();
    raw.extend_from_slice(&data);
    Some(raw)
}

/// Whether the packet `raw` (IFAC removed) is a path response announcing
/// `destination`, made at or after `started` (unix seconds, the time an
/// announce carries in the last 5 bytes of its random hash), that its
/// signature and its address bear out. An older announce, played back by
/// something else on the port, isn't an answer.
fn announces(raw: &[u8], destination: &[u8; 16], started: u64) -> bool {
    let Ok(packet) = rns_wire::packet::Packet::from_raw(raw) else { return false };
    let header = &packet.header;
    if header.flags.packet_type != rns_wire::flags::PacketType::Announce
        || header.destination_hash != *destination
        || header.context != rns_wire::context::PacketContext::PathResponse
    {
        return false;
    }
    let Ok(announce) = rns_identity::announce::AnnounceData::unpack(packet.data(), header.flags.context_flag) else { return false };
    let mut time = [0u8; 8];
    time[3..].copy_from_slice(&announce.random_hash[5..10]);
    u64::from_be_bytes(time) >= started.saturating_sub(ANNOUNCE_SLACK) && announce.validate(destination).is_ok()
}

/// Frames in the interface's framing, taken from a stream of bytes.
enum Deframer {
    Hdlc(rns_interface::hdlc::HdlcDeframer),
    Kiss(rns_interface::kiss::KissByteDeframer),
}

impl Deframer {
    fn new(kiss: bool) -> Self {
        if kiss {
            Deframer::Kiss(rns_interface::kiss::KissByteDeframer::new(MAX_FRAME))
        } else {
            Deframer::Hdlc(rns_interface::hdlc::HdlcDeframer::with_limits(MAX_FRAME, rns_wire::constants::HEADER_MINSIZE, Some(MAX_FRAME)))
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        match self {
            Deframer::Hdlc(deframer) => deframer.feed(bytes),
            Deframer::Kiss(deframer) => deframer
                .feed(bytes)
                .into_iter()
                .filter_map(|event| match event {
                    rns_interface::kiss::KissEvent::Data(frame) => Some(frame),
                    rns_interface::kiss::KissEvent::Ready => None,
                })
                .collect(),
        }
    }
}

/// Dial `dial` and ask for a path to `destination` until an announce of it
/// comes back (`report(true)`) or `within` is over (`report(false)`), with a
/// new request every `attempt` and at most [`MAX_CONNECTIONS`] connections.
/// The last connection is then held open for `dial.hold`, from when it was
/// made, before it is closed. `started` is when the test began (unix
/// seconds): an announce made before it doesn't count.
pub async fn probe(dial: &Dial, destination: [u8; 16], started: u64, within: Duration, attempt: Duration, report: impl FnOnce(bool)) {
    let until = tokio::time::Instant::now() + within;
    let mut held: Option<(TcpStream, tokio::time::Instant)> = None;
    let mut passed = false;
    let mut connections = 0;
    'connections: while connections < MAX_CONNECTIONS {
        let left = until.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        // Refused (not listening yet, or not at all): tried again until the time is up.
        let Some(mut stream) = connect(dial, left.min(attempt)).await else {
            tokio::time::sleep(left.min(Duration::from_secs(1))).await;
            continue;
        };
        connections += 1;
        let opened = tokio::time::Instant::now();
        let mut deframer = Deframer::new(dial.kiss);
        let mut read = 0usize;
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            let now = tokio::time::Instant::now();
            if now >= until {
                held = Some((stream, opened));
                break 'connections;
            }
            let answer_by = (now + attempt).min(until);
            // No stamp in time: no request this round, but the connection
            // is still read (an answer to an earlier one counts) and the
            // next round tries again.
            if let Some(request) = framed_request(dial, destination, now.into_std() + STAMP_WITHIN.min(until - now)).await
                && stream.write_all(&request).await.is_err()
            {
                break;
            }
            let closed = loop {
                let bytes = match tokio::time::timeout_at(answer_by, stream.read(&mut buffer)).await {
                    Err(_) => break false,
                    Ok(Ok(0) | Err(_)) => break true,
                    Ok(Ok(n)) => n,
                };
                read += bytes;
                if read > MAX_READ {
                    break true;
                }
                let found = deframer.feed(&buffer[..bytes]).into_iter().any(|frame| match &dial.ifac {
                    Some(ifac) => rns_transport::ifac::ifac_verify(&frame, &ifac.key, ifac.size)
                        .is_some_and(|raw| announces(&raw, &destination, started)),
                    None => !rns_transport::ifac::has_ifac_flag(&frame) && announces(&frame, &destination, started),
                });
                if found {
                    passed = true;
                    held = Some((stream, opened));
                    break 'connections;
                }
            };
            if closed {
                break;
            }
        }
    }
    report(passed);
    if let Some((stream, opened)) = held {
        if let Some(until) = opened.checked_add(dial.hold) {
            tokio::time::sleep_until(until).await;
        }
        drop(stream);
    }
}

/// A connection to the listener, within `wait`.
async fn connect(dial: &Dial, wait: Duration) -> Option<TcpStream> {
    let stream = tokio::time::timeout(wait, TcpStream::connect(dial.address)).await.ok()?.ok()?;
    let _ = stream.set_nodelay(true);
    Some(stream)
}

/// A path request, with IFAC and in the interface's framing; `None` if its
/// stamp wasn't found by `stamp_by`. A stamp is made off the async threads.
async fn framed_request(dial: &Dial, destination: [u8; 16], stamp_by: Instant) -> Option<Vec<u8>> {
    let cost = dial.stamp_cost;
    let raw = tokio::task::spawn_blocking(move || path_request(destination, cost, stamp_by)).await.ok()??;
    let raw = match &dial.ifac {
        Some(ifac) => rns_transport::ifac::try_ifac_sign(&raw, &ifac.key, ifac.size).ok()?,
        None => raw,
    };
    Some(rns_interface::tcp::frame_for_tcp(&raw, dial.kiss))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use rns_runtime::prelude::*;

    use super::*;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-self-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// Reticulum, in this process, with the interfaces of `interfaces`, and
    /// a destination of its own: the handle, and the destination's address.
    async fn reticulum(dir: &std::path::Path, head: &str, interfaces: &str) -> (ReticulumHandle, RegisteredDestination, [u8; 16]) {
        let text = format!("[reticulum]\n  share_instance = No\n  enable_transport = No\n{head}\n[interfaces]\n{interfaces}");
        std::fs::write(dir.join("config"), text).unwrap();
        let handle = rns_runtime::reticulum::init_with_options(
            Some(dir.to_str().unwrap()),
            None,
            ShutdownSignal::new(),
            Arc::new(AtomicBool::new(true)),
            InitOptions::default(),
        )
        .await
        .unwrap();
        let identity = Identity::new();
        let destination = handle.register_destination(identity, "rettui.selftest", DestinationRuntimeOptions::default()).await.unwrap();
        let hash = destination.handle.destination_hash();
        (handle, destination, hash)
    }

    async fn probed(dial: &Dial, destination: [u8; 16], within: Duration) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        probe(dial, destination, unix_now(), within, Duration::from_secs(2), |passed| {
            let _ = tx.send(passed);
        })
        .await;
        rx.await.unwrap()
    }

    fn dial(config: &str, name: &str) -> Option<Dial> {
        dial_of(&Config::parse(config).unwrap(), name)
    }

    #[test]
    fn listeners_are_dialled_and_the_rest_only_checked_online() {
        let config = "\
[reticulum]
[interfaces]
  [[Any]]
    type = TCPServerInterface
    enabled = yes
    listen_ip = 0.0.0.0
    listen_port = 4242
  [[Six]]
    type = TCPServerInterface
    enabled = yes
    listen_ip = ::
    port = 4243
    kiss_framing = yes
  [[Spine]]
    type = BackboneInterface
    enabled = yes
    listen_on = 10.0.0.1
    port = 4244
    network_name = hill
    passphrase = fake-secret
  [[Out]]
    type = BackboneInterface
    enabled = yes
    remote = hub.example.org
    port = 4244
  [[Client]]
    type = TCPClientInterface
    enabled = yes
    target_host = hub.example.org
    target_port = 4242
  [[Device]]
    type = TCPServerInterface
    enabled = yes
    device = eth0
    listen_port = 4245
  [[Device and address]]
    type = TCPServerInterface
    enabled = yes
    device = eth0
    listen_ip = 127.0.0.1
    listen_port = 4247
  [[Spine on a device]]
    type = BackboneInterface
    enabled = yes
    device = eth0
    listen_on = 127.0.0.1
    port = 4248
  [[Named]]
    type = TCPServerInterface
    enabled = yes
    listen_ip = localhost
    listen_port = 4249
  [[Slow flap]]
    type = BackboneInterface
    enabled = yes
    listen_on = 127.0.0.1
    port = 4250
    fast_flapping_threshold = 121
  [[Flap off]]
    type = BackboneInterface
    enabled = yes
    listen_on = 127.0.0.1
    port = 4251
    fast_flapping_threshold = 600
    block_fast_flapping = no
  [[Mute]]
    type = TCPServerInterface
    enabled = yes
    listen_ip = 127.0.0.1
    listen_port = 4246
    outgoing = no
  [[Local]]
    type = AutoInterface
    enabled = yes
";
        let any = dial(config, "Any").unwrap();
        assert_eq!(
            (any.address, any.kiss, any.hold, any.ifac.is_none()),
            (SocketAddr::from(([127, 0, 0, 1], 4242)), false, Duration::ZERO, true)
        );
        let six = dial(config, "Six").unwrap();
        assert_eq!((six.address, six.kiss), (SocketAddr::from((Ipv6Addr::LOCALHOST, 4243)), true));
        let spine = dial(config, "Spine").unwrap();
        assert_eq!((spine.address, spine.ifac.as_ref().map(|i| i.size)), (SocketAddr::from(([10, 0, 0, 1], 4244)), Some(16)));
        assert!(spine.hold > Duration::from_secs(20), "a Backbone listener mustn't see a fast flap");
        assert!(!format!("{spine:?}").contains("key"), "the IFAC key is never shown");
        // A device's address, a host name (never looked up) and a flap
        // threshold over two minutes can't be tested from here.
        for name in ["Out", "Client", "Device", "Device and address", "Spine on a device", "Named", "Slow flap", "Mute", "Local", "Missing"]
        {
            assert!(dial(config, name).is_none(), "{name}");
        }
        assert_eq!(dial(config, "Flap off").unwrap().hold, Duration::ZERO, "no flap guard, nothing to wait for");
        let run = Run::plan(config, &names(&["Any", "Local", "Missing"]));
        let kinds: Vec<(&str, Kind, Seen)> = run.checks.iter().map(|c| (c.interface.as_str(), c.kind, c.seen)).collect();
        assert_eq!(
            kinds,
            [("Any", Kind::SelfTest, Seen::Waiting), ("Local", Kind::Online, Seen::Waiting), ("Missing", Kind::Online, Seen::Waiting)]
        );
        let named = Run::plan(config, &names(&["Named"]));
        assert_eq!(named.checks[0].kind, Kind::Online);
        // A config Reticulum can't read has nothing to dial.
        assert!(Run::plan("[[[", &names(&["Any"])).checks[0].kind == Kind::Online);
    }

    #[test]
    fn the_any_address_is_reached_on_loopback_and_names_arent_looked_up() {
        let v4 = |a, b, c, d| Some(IpAddr::from([a, b, c, d]));
        for (listen, ip) in [
            ("0.0.0.0", v4(127, 0, 0, 1)),
            ("", v4(127, 0, 0, 1)),
            (" 0.0.0.0 ", v4(127, 0, 0, 1)),
            ("::", Some(Ipv6Addr::LOCALHOST.into())),
            ("[::]", Some(Ipv6Addr::LOCALHOST.into())),
            ("0:0::0", Some(Ipv6Addr::LOCALHOST.into())),
            ("10.0.0.1", v4(10, 0, 0, 1)),
            ("[fd00::1]", Some("fd00::1".parse().unwrap())),
            ("localhost", None),
            ("node.example.org", None),
            ("[::", None),
        ] {
            assert_eq!(reachable(listen), ip, "{listen:?}");
        }
    }

    #[test]
    fn enforced_stamps_are_made_or_the_interface_only_checked_online() {
        let base =
            "[interfaces]\n  [[In]]\n    type = TCPServerInterface\n    enabled = yes\n    listen_ip = 127.0.0.1\n    listen_port = 4242\n";
        assert_eq!(dial(base, "In").unwrap().stamp_cost, None);
        let enforced = format!("[stamps]\n  enforce_stamps = yes\n{base}");
        assert_eq!(dial(&enforced, "In").unwrap().stamp_cost, Some(rns_transport::stamps::StampPolicy::default().pr_cost));
        let cheap = format!("[stamps]\n  enforce_stamps = yes\n  pr_cost = 4\n{base}");
        assert_eq!(dial(&cheap, "In").unwrap().stamp_cost, Some(4.0));
        let most = format!("[stamps]\n  enforce_stamps = yes\n  pr_cost = 16\n{base}");
        assert_eq!(dial(&most, "In").unwrap().stamp_cost, Some(MAX_STAMP_COST));
        for cost in ["17", "40"] {
            let dear = format!("[stamps]\n  enforce_stamps = yes\n  pr_cost = {cost}\n{base}");
            assert!(dial(&dear, "In").is_none(), "{cost} is too dear to make in the time");
        }
        // One the interface asks for is too.
        let dear_here = format!("[stamps]\n  enforce_stamps = yes\n{}    pr_cost = 20\n", base);
        assert!(dial(&dear_here, "In").is_none());
        // A stamp not found in time is no request.
        assert!(path_request([7; 16], Some(60.0), Instant::now()).is_none());
        // A stamped request has the length and the stamp Reticulum checks.
        let soon = Instant::now() + STAMP_WITHIN;
        let request = path_request([7; 16], Some(4.0), soon).unwrap();
        let (header, offset) = rns_wire::header::PacketHeader::unpack(&request).unwrap();
        assert_eq!(header.destination_hash, rns_identity::destination::Destination::hash_from_name_and_identity(PATH_REQUEST, None));
        let data = &request[offset..];
        assert_eq!(data.len(), 64);
        let mut material = data[..16].to_vec();
        material.extend_from_slice(&data[32..48]);
        assert!(rns_identity::stamps::stamp_valid(&data[50..], 4.0, &material).unwrap());
        assert_eq!(path_request([7; 16], None, Instant::now()).unwrap().len() - offset, 32, "no stamp, no wait");
    }

    #[test]
    fn what_comes_back_is_checked_not_trusted() {
        let identity = Identity::new();
        let destination = Destination::hash_from_name_and_identity("rettui.selftest", Some(&identity.hash));
        let started = unix_now();
        let announce = rns_identity::announce::AnnounceData::create_at(&identity, "rettui.selftest", None, None, started).unwrap();
        let header = |hash, context| rns_wire::header::PacketHeader {
            flags: rns_wire::flags::PacketFlags {
                header_type: rns_wire::flags::HeaderType::Header1,
                context_flag: false,
                transport_type: rns_wire::flags::TransportType::Broadcast,
                destination_type: rns_wire::flags::DestinationType::Single,
                packet_type: rns_wire::flags::PacketType::Announce,
            },
            hops: 0,
            transport_id: None,
            destination_hash: hash,
            context,
        };
        let path_response = rns_wire::context::PacketContext::PathResponse;
        let packet = |hash, context, announce: &rns_identity::announce::AnnounceData| {
            let mut raw = header(hash, context).pack();
            raw.extend_from_slice(&announce.pack());
            raw
        };
        let raw = packet(destination, path_response, &announce);
        assert!(announces(&raw, &destination, started));
        // Made within the slack before the probe began, it still counts.
        assert!(announces(&raw, &destination, started + ANNOUNCE_SLACK));
        // An older one, played back (a validly signed announce of the
        // node, heard before), isn't an answer.
        let old = rns_identity::announce::AnnounceData::create_at(&identity, "rettui.selftest", None, None, started - 600).unwrap();
        assert!(!announces(&packet(destination, path_response, &old), &destination, started));
        assert!(!announces(&raw, &destination, started + ANNOUNCE_SLACK + 1));
        // Nor an announce that isn't a path response.
        assert!(!announces(&packet(destination, rns_wire::context::PacketContext::None, &announce), &destination, started));
        // Another address, a broken signature, junk and nothing are no answer.
        assert!(!announces(&raw, &[1; 16], started));
        assert!(!announces(&packet([1; 16], path_response, &announce), &[1; 16], started));
        let mut broken = raw.clone();
        let last = broken.len() - 1;
        broken[last] ^= 1;
        assert!(!announces(&broken, &destination, started));
        for junk in [&[][..], &[0x7e; 3][..], &[0xff; 40][..], &raw[..30]] {
            assert!(!announces(junk, &destination, started));
        }
        let mut deframer = Deframer::new(false);
        assert!(deframer.feed(&[0x7e; 1000]).is_empty());
        let mut kiss = Deframer::new(true);
        assert_eq!(kiss.feed(&rns_interface::tcp::frame_for_tcp(&raw, true)), std::slice::from_ref(&raw));
    }

    #[test]
    fn the_checks_of_interfaces_that_cant_be_dialled_say_what_was_heard() {
        let mut run = Run::plan("[interfaces]\n  [[Radio]]\n    type = RNodeInterface\n", &names(&["Radio"]));
        run.start(None, Duration::from_secs(1), ATTEMPT, &Prober::Dial, false);
        assert!(run.started() && run.waiting().next().is_none() && run.failed().is_none(), "nothing to dial, so no address needed");
        run.poll(&[]);
        assert_eq!(run.checks[0].seen, Seen::Waiting, "not online yet");
        let mut radio = InterfaceInfo { name: "Radio".into(), online: true, ..Default::default() };
        run.poll(std::slice::from_ref(&radio));
        assert_eq!(run.checks[0].seen, Seen::NoTraffic);
        radio.rx_bytes = 120;
        run.poll(std::slice::from_ref(&radio));
        assert_eq!(run.to_json(), json!([{ "interface": "Radio", "kind": "online", "result": "traffic" }]));
        assert_eq!(run.summary().as_deref(), Some("Radio online traffic"));
        assert_eq!(Run::default().summary(), None);
        // At most 32 are listed.
        let many: Vec<String> = (0..40).map(|n| format!("If {n}")).collect();
        assert_eq!(Run::plan("", &many).to_json().as_array().unwrap().len(), MAX_CHECKS);
        // Names are cut as problems are.
        let long = Run::plan("", &["n".repeat(500)]);
        assert_eq!(long.to_json()[0]["interface"].as_str().unwrap().len(), MAX_PROBLEM);
        assert_eq!(long.summary().unwrap(), format!("{} online waiting", "n".repeat(MAX_PROBLEM)));
    }

    /// The probe against Reticulum's own TCP server and Backbone
    /// interfaces: answered, with HDLC or KISS framing and with IFAC; a
    /// listener that only accepts, a wrong IFAC and nothing listening aren't.
    #[test]
    fn reticulum_answers_the_probe_and_a_bare_listener_doesnt() {
        let dir = test_dir("probe");
        let (plain, kiss, ifac, spine) = (free_port(), free_port(), free_port(), free_port());
        let interfaces = format!(
            "  [[Plain]]\n    type = TCPServerInterface\n    enabled = yes\n    listen_ip = 0.0.0.0\n    listen_port = {plain}\n\
             \x20 [[Kiss]]\n    type = TCPServerInterface\n    enabled = yes\n    listen_ip = 127.0.0.1\n    listen_port = {kiss}\n    kiss_framing = yes\n\
             \x20 [[Closed]]\n    type = TCPServerInterface\n    enabled = yes\n    listen_ip = 127.0.0.1\n    listen_port = {ifac}\n    network_name = hill\n    passphrase = fake-secret\n\
             \x20 [[Spine]]\n    type = BackboneInterface\n    enabled = yes\n    listen_on = 127.0.0.1\n    port = {spine}\n    fast_flapping_threshold = 1\n"
        );
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (handle, _destination, hash) = reticulum(&dir, "", &interfaces).await;
            let config = std::fs::read_to_string(dir.join("config")).unwrap();
            let config = Config::parse(&config).unwrap();
            let within = Duration::from_secs(20);
            for name in ["Plain", "Kiss", "Closed", "Spine"] {
                let dial = dial_of(&config, name).unwrap();
                assert!(probed(&dial, hash, within).await, "{name} didn't answer");
            }
            // The Backbone connection was held past its flapping threshold before it was closed.
            let spine_dial = dial_of(&config, "Spine").unwrap();
            assert_eq!(spine_dial.hold, Duration::from_secs(2));
            let started = Instant::now();
            assert!(probed(&spine_dial, hash, within).await);
            assert!(started.elapsed() >= Duration::from_secs(2), "{:?}", started.elapsed());
            // Not another address; not without the IFAC, nor with the wrong one.
            let plain_dial = dial_of(&config, "Plain").unwrap();
            assert!(!probed(&plain_dial, [9; 16], Duration::from_secs(3)).await);
            let mut wrong = dial_of(&config, "Closed").unwrap();
            wrong.ifac = None;
            assert!(!probed(&wrong, hash, Duration::from_secs(3)).await);
            wrong.ifac = Some(Ifac { key: zeroize::Zeroizing::new([3; 64]), size: 16 });
            assert!(!probed(&wrong, hash, Duration::from_secs(3)).await);
            // A listener that takes the connection and says nothing; and one that's gone.
            let bare = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let bare_port = bare.local_addr().unwrap().port();
            let accepting = tokio::spawn(async move {
                let mut kept = Vec::new();
                while let Ok((socket, _)) = bare.accept().await {
                    kept.push(socket);
                }
            });
            let silent = Dial { address: SocketAddr::from(([127, 0, 0, 1], bare_port)), ..plain_dial.clone() };
            let started = Instant::now();
            assert!(!probed(&silent, hash, Duration::from_secs(5)).await);
            assert!(started.elapsed() < Duration::from_secs(8), "the probe keeps to its time");
            // A stamp that can't be made in time: no request that round, but
            // the probe goes on trying until its time is up, not failing at once.
            let dear = Dial { stamp_cost: Some(60.0), ..silent.clone() };
            let started = Instant::now();
            assert!(!probed(&dear, hash, Duration::from_secs(3)).await);
            assert!(started.elapsed() >= Duration::from_secs(3), "{:?}", started.elapsed());
            accepting.abort();
            let gone = Dial { address: SocketAddr::from(([127, 0, 0, 1], free_port())), ..plain_dial };
            assert!(!probed(&gone, hash, Duration::from_secs(2)).await);
            handle.shutdown_and_wait().await;
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A node that enforces path request stamps drops a request without
    /// one: the probe stamps its own.
    #[test]
    fn a_node_that_enforces_stamps_answers_a_stamped_probe() {
        let dir = test_dir("stamps");
        let port = free_port();
        let interfaces =
            format!("  [[In]]\n    type = TCPServerInterface\n    enabled = yes\n    listen_ip = 127.0.0.1\n    listen_port = {port}\n");
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (handle, _destination, hash) = reticulum(&dir, "[stamps]\n  enforce_stamps = yes\n  pr_cost = 8\n", &interfaces).await;
            let config = Config::parse(&std::fs::read_to_string(dir.join("config")).unwrap()).unwrap();
            let dial = dial_of(&config, "In").unwrap();
            assert_eq!(dial.stamp_cost, Some(8.0));
            let unstamped = Dial { stamp_cost: None, ..dial.clone() };
            assert!(!probed(&unstamped, hash, Duration::from_secs(3)).await, "enforced, so dropped");
            assert!(probed(&dial, hash, Duration::from_secs(20)).await);
            handle.shutdown_and_wait().await;
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_reports_through_its_thread() {
        let dir = test_dir("run");
        let port = free_port();
        let interfaces = format!(
            "  [[Hilltop TCP]]\n    type = TCPServerInterface\n    enabled = yes\n    listen_ip = 127.0.0.1\n    listen_port = {port}\n"
        );
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let (handle, destination, hash) = runtime.block_on(reticulum(&dir, "", &interfaces));
        let config = std::fs::read_to_string(dir.join("config")).unwrap();
        let mut run = Run::plan(&config, &names(&["Hilltop TCP"]));
        run.start(None, Duration::from_secs(20), Duration::from_secs(2), &Prober::Dial, false);
        assert!(!run.started(), "it waits for the address");
        run.start(Some(hash), Duration::from_secs(20), Duration::from_secs(2), &Prober::Dial, false);
        let until = Instant::now() + Duration::from_secs(25);
        while run.waiting().next().is_some() && Instant::now() < until {
            run.poll(&[]);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(run.to_json(), json!([{ "interface": "Hilltop TCP", "kind": "self_test", "result": "passed" }]));
        // Redirected to a port nobody listens on, it fails; after a rollback, it isn't redirected.
        let redirect = Prober::Redirect(HashMap::from([("Hilltop TCP".to_string(), SocketAddr::from(([127, 0, 0, 1], free_port())))]));
        let mut run = Run::plan(&config, &names(&["Hilltop TCP"]));
        run.start(Some(hash), Duration::from_secs(2), Duration::from_secs(1), &redirect, false);
        let mut after = Run::plan(&config, &names(&["Hilltop TCP"]));
        after.start(Some(hash), Duration::from_secs(20), Duration::from_secs(2), &redirect, true);
        let until = Instant::now() + Duration::from_secs(25);
        while (run.waiting().next().is_some() || after.waiting().next().is_some()) && Instant::now() < until {
            run.poll(&[]);
            after.poll(&[]);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(run.failed(), Some("Hilltop TCP"));
        assert_eq!(after.checks[0].seen, Seen::Passed);
        drop(destination);
        runtime.block_on(handle.shutdown_and_wait());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
