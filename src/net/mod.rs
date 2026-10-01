//! Reticulum network actor.
//!
//! Owns the rns-runtime handle, the local `lxmf.delivery` destination, the
//! NomadNet Link cache, the RRC hub sessions, and the nodes hosted here. The
//! UI talks to it only through [`NetCommand`] and [`NetEvent`] channels so
//! rendering never waits on the network. The protocol work itself lives in [`crate::lxmf`],
//! [`crate::nomad`] and [`crate::rrc`].

pub mod iface_log;
mod remote;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use lxmf_core::handlers::{display_name_from_app_data, get_announce_app_data, parse_pn_announce_data};
use nomad_core::NOMAD_NODE_ASPECT;
use rns_runtime::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};

use crate::lxmf::pn::{HostedPn, LocalNode, PnConfig, PnStats};
use crate::lxmf::{self, InboundMessage, LXMF_ASPECT, PROPAGATION_ASPECT};
use crate::nomad::host::{self, HostConfig};
use crate::nomad::{self, FetchedContent, LinkCache};
use crate::rrc::session::{self as rrc_session, RrcEvent, SessionCommand};

pub use remote::{Known, KnownIdentities, ensure_path, link_options, lookup};

const STATS_INTERVAL: Duration = Duration::from_secs(5);

/// How long an event loop applies a burst of network events before it
/// draws or answers requests again; the rest wait for the next turn.
pub const BURST_BUDGET: Duration = Duration::from_millis(8);

/// Take waiting events until the queue is empty or the burst budget is spent.
pub fn drain_burst(events: &mut mpsc::UnboundedReceiver<NetEvent>, mut apply: impl FnMut(NetEvent)) {
    let started = std::time::Instant::now();
    while started.elapsed() < BURST_BUDGET {
        match events.try_recv() {
            Ok(event) => apply(event),
            Err(_) => break,
        }
    }
}

pub type Hash = [u8; 16];

/// The display name in an LXMF delivery announce: the msgpack format, or
/// the original one where the data is just the name in UTF-8 (still sent by
/// older clients; Python LXMF reads both).
pub fn lxmf_display_name(data: &[u8]) -> Option<String> {
    let first = *data.first()?;
    let msgpack_array = (0x90..=0x9f).contains(&first) || first == 0xdc;
    if msgpack_array {
        return display_name_from_app_data(data);
    }
    let name: String = std::str::from_utf8(data).ok()?.chars().filter(|c| !c.is_control()).take(128).collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// A destination hash from hex, tolerating `<...>` around it.
pub fn parse_hash(text: &str) -> Option<Hash> {
    let text = text.trim().trim_start_matches('<').trim_end_matches('>');
    hex::decode(text).ok()?.try_into().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerKind {
    Lxmf,
    Nomad,
    Propagation,
}

#[derive(Debug)]
pub enum NetCommand {
    Announce,
    /// Write a paper message (answered with [`NetEvent::Paper`]).
    WritePaper { id: u64, paper: lxmf::paper::Paper },
    /// Read in a paper message (an `lxm://` link).
    ReadPaper(String),
    SetDisplayName(String),
    /// The stamp cost asked of senders (announced) and the largest message
    /// taken, in bytes (0: any).
    SetPolicy { stamp_cost: Option<u8>, max_bytes: u64 },
    /// Contacts: those given stamp tickets (trusted), and those spared the
    /// stamp (trusted, or written to).
    SetContacts { trusted: Vec<Hash>, exempt: Vec<Hash> },
    /// Block (or unblock) the identity behind an LXMF address in Reticulum.
    /// `quiet`: no line in the log when it's done (re-applied at start).
    Blackhole { to: Hash, block: bool, quiet: bool },
    SetPropagationNode(Option<Hash>),
    /// New automatic announce and sync intervals (`None` turns one off).
    SetIntervals {
        announce: Option<Duration>,
        sync: Option<Duration>,
    },
    Sync,
    /// Send a message (its propagation node is the one set here).
    SendMessage { id: u64, message: lxmf::Outgoing },
    Fetch {
        id: u64,
        node: Hash,
        path: String,
        fields: BTreeMap<String, String>,
        identify: bool,
    },
    /// Connect to an RRC hub (no-op if a session is already running).
    RrcConnect {
        hub: Hash,
        aspect: String,
        nick: Option<String>,
    },
    /// Pass a command to a running hub session.
    Rrc { hub: Hash, command: SessionCommand },
    /// Start (or restart) the hosted node, or stop it with `None`.
    Host(Option<HostConfig>),
    /// Pick up added, renamed or deleted pages.
    HostReload,
    HostAnnounce,
    /// Start (or restart, when it changed) the hosted propagation node, or
    /// stop it with `None`.
    Propagation(Option<PnConfig>),
    /// Close hub links and stop Reticulum; [`NetEvent::Stopped`] follows.
    Shutdown(Stop),
}

/// Why the actor is stopping, which sets how long it may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// rettui is quitting: keep it short.
    Quit,
    /// Reticulum is being restarted in place: let it release its sockets
    /// (a shared instance's port), so the new instance can take them.
    Restart,
}

impl Stop {
    fn runtime_wait(self) -> Duration {
        match self {
            Stop::Quit => Duration::from_millis(1500),
            Stop::Restart => Duration::from_secs(6),
        }
    }

    fn total_wait(self) -> Duration {
        match self {
            Stop::Quit => Duration::from_secs(3),
            Stop::Restart => Duration::from_secs(12),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceInfo {
    pub name: String,
    pub online: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Debug)]
pub enum NetEvent {
    Started { lxmf_hash: Hash },
    StartFailed(String),
    Announce {
        kind: PeerKind,
        hash: Hash,
        name: Option<String>,
        hops: u8,
    },
    Announced,
    Message(Box<InboundMessage>),
    Delivery { id: u64, result: Result<lxmf::Sent, String> },
    /// A paper message written: its `lxm://` link and hash.
    Paper { id: u64, result: Result<(String, [u8; 32]), String> },
    Fetched { id: u64, result: Result<FetchedContent, String> },
    SyncStarted,
    Synced(Result<usize, String>),
    Interfaces(Vec<InterfaceInfo>),
    Log(String),
    Rrc { hub: Hash, event: RrcEvent },
    Host(HostEvent),
    Pn(PnEvent),
    /// The actor has shut down (after [`NetCommand::Shutdown`]).
    Stopped,
}

/// What the hosted node is doing.
#[derive(Debug)]
pub enum HostEvent {
    Started { hash: Hash },
    Stopped,
    Failed(String),
    Stats(nomad_core::NomadServeStats),
}

/// What the hosted propagation node is doing.
#[derive(Debug)]
pub enum PnEvent {
    Started { hash: Hash },
    Stopped,
    Failed(String),
    Stats(PnStats),
}

pub struct NetOptions {
    pub rns_config: Option<String>,
    pub identity: Identity,
    pub display_name: String,
    pub announce_at_start: bool,
    pub announce_interval: Option<Duration>,
    pub propagation_node: Option<Hash>,
    pub sync_interval: Option<Duration>,
    pub known_identities: PathBuf,
    /// Node to host from the start, if any.
    pub host: Option<HostConfig>,
    /// Propagation node to host from the start, if any.
    pub propagation: Option<PnConfig>,
    /// The stamp cost asked of senders, and the largest message taken.
    pub stamp_cost: Option<u8>,
    pub max_message_bytes: u64,
    /// Where stamp tickets are kept.
    pub tickets: PathBuf,
}

/// LXMF announce data: the display name, and the stamp cost asked.
fn app_data(display_name: &str, stamp_cost: Option<u8>) -> Vec<u8> {
    get_announce_app_data(Some(display_name), stamp_cost)
}

/// Start the network actor on a runtime of its own, so heavy traffic
/// (thousands of announces to verify) never holds up the threads that
/// run the interface: the TUI's timers, or the web server's requests.
pub fn spawn(options: NetOptions) -> (mpsc::UnboundedSender<NetCommand>, mpsc::UnboundedReceiver<NetEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    iface_log::attach(ev_tx.clone());
    let started = std::thread::Builder::new().name("rettui-net".into()).spawn({
        let ev_tx = ev_tx.clone();
        move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .thread_name("rettui-net-worker")
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = ev_tx.send(NetEvent::StartFailed(format!("Could not start the network runtime: {e}")));
                    return;
                }
            };
            runtime.block_on(async {
                if let Err(error) = run(options, cmd_rx, ev_tx.clone()).await {
                    let _ = ev_tx.send(NetEvent::StartFailed(error));
                }
            });
            // Tasks still running (links, fetches) belonged to this stack.
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
    });
    if let Err(e) = started {
        let _ = ev_tx.send(NetEvent::StartFailed(format!("Could not start the network thread: {e}")));
    }
    (cmd_tx, ev_rx)
}

/// Start the runtime without registering any destination, for one-shot CLI use.
pub async fn start_runtime(rns_config: Option<&str>) -> Result<ReticulumHandle, String> {
    init(
        rns_config,
        None,
        ShutdownSignal::new(),
        Arc::new(AtomicBool::new(true)),
    )
    .await
    .map_err(|e| format!("Reticulum failed to start: {e}"))
}

/// Announces a subscription holds while the actor is busy. Busy networks
/// (or a node coming online) deliver hundreds at once; the runtime drops
/// what doesn't fit.
const ANNOUNCE_BUFFER: usize = 8192;

async fn subscribe(runtime: &ReticulumHandle, aspect: &str) -> Result<AnnounceSubscription, String> {
    runtime
        .subscribe_announces_with_capacity(Some(aspect.to_string()), false, ANNOUNCE_BUFFER)
        .await
        .map_err(|e| format!("Could not subscribe to {aspect} announces: {e}"))
}

async fn run(
    options: NetOptions,
    mut cmd_rx: mpsc::UnboundedReceiver<NetCommand>,
    ev: mpsc::UnboundedSender<NetEvent>,
) -> Result<(), String> {
    let runtime = start_runtime(options.rns_config.as_deref()).await?;
    for (name, reason) in runtime.startup_interface_failures() {
        let _ = ev.send(NetEvent::Log(format!("Interface {name} failed: {reason}")));
    }

    let identity = options.identity;
    let known = KnownIdentities::load(&options.known_identities);
    let policy = lxmf::Policy::load(&options.tickets, options.stamp_cost, options.max_message_bytes);
    let mut display_name = options.display_name;
    let mut propagation_node = options.propagation_node;
    // Transfers over the size limit are refused before they're downloaded.
    let size_gate = {
        let (policy, ev) = (policy.clone(), ev.clone());
        ResourceAcceptPolicy::new(move |_link, advertisement| {
            let policy = policy.lock().unwrap();
            if !policy.too_big(advertisement.data_size) {
                return true;
            }
            let _ = ev.send(NetEvent::Log(format!(
                "Refused a {} KB message: over your limit of {} KB",
                advertisement.data_size / 1000,
                policy.max_bytes / 1000
            )));
            false
        })
    };
    let mut delivery = runtime
        .register_destination(
            identity.clone(),
            LXMF_ASPECT,
            DestinationRuntimeOptions {
                proof_strategy: ProofStrategy::ProveAll,
                resource_strategy: ResourceStrategy::AcceptApp,
                resource_accept: Some(size_gate),
                default_app_data: Some(app_data(&display_name, options.stamp_cost)),
                ..DestinationRuntimeOptions::default()
            },
        )
        .await
        .map_err(|e| format!("Could not register LXMF destination: {e}"))?;
    let lxmf_hash = delivery.handle.destination_hash();
    let _ = ev.send(NetEvent::Started { lxmf_hash });

    let mut lxmf_announces = subscribe(&runtime, LXMF_ASPECT).await?;
    let mut nomad_announces = subscribe(&runtime, NOMAD_NODE_ASPECT).await?;
    let mut pn_announces = subscribe(&runtime, PROPAGATION_ASPECT).await?;

    let links: LinkCache = Arc::new(Mutex::new(HashMap::new()));

    // The hosted node starts in a task (it announces first) and comes back
    // here; `host_generation` drops nodes from superseded starts.
    let (host_tx, mut host_rx) = mpsc::unbounded_channel::<(u64, Result<nomad_core::NomadNode, String>)>();
    let mut host_generation = 0u64;
    let mut hosted: Option<nomad_core::NomadNode> = None;
    let launch = |generation: u64, config: HostConfig| {
        let (runtime, identity, tx) = (runtime.clone(), identity.clone(), host_tx.clone());
        tokio::spawn(async move {
            let _ = tx.send((generation, host::start(&runtime, &identity, &config).await));
        });
    };
    if let Some(config) = options.host.clone() {
        launch(host_generation, config);
    }
    // The propagation node is started (and stopped) in tasks, one at a time:
    // the slot's lock is held from stopping the old node until the new one
    // is in place. Messages it takes for us come back on `pn_deliver`.
    let pn_slot: Arc<Mutex<Option<HostedPn>>> = Arc::default();
    let (pn_tx, mut pn_rx) = mpsc::unbounded_channel::<(u64, Result<Option<LocalNode>, String>)>();
    let (pn_deliver_tx, mut pn_deliver) = mpsc::unbounded_channel::<Vec<u8>>();
    let pn_generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut pn_config = options.propagation.clone();
    let mut pn_local: Option<LocalNode> = None;
    let launch_pn = |config: Option<PnConfig>| {
        let generation = pn_generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let (runtime, identity, slot, current, tx, deliver) =
            (runtime.clone(), identity.clone(), pn_slot.clone(), pn_generation.clone(), pn_tx.clone(), pn_deliver_tx.clone());
        tokio::spawn(async move {
            let mut slot = slot.lock().await;
            if let Some(old) = slot.take() {
                old.stop().await;
            }
            // A later change replaces this one: it starts what's wanted.
            if current.load(std::sync::atomic::Ordering::SeqCst) != generation {
                return;
            }
            let result = match config {
                None => Ok(None),
                Some(config) => HostedPn::start(&runtime, &identity, lxmf_hash, &config, deliver).await.map(|node| {
                    let local = node.local();
                    *slot = Some(node);
                    Some(local)
                }),
            };
            let _ = tx.send((generation, result));
        });
    };
    if pn_config.is_some() {
        launch_pn(pn_config.clone());
    }
    let mut rrc_sessions = rrc_session::Sessions::default();
    let mut stop = Stop::Quit;
    let syncer = lxmf::Syncer::new(runtime.clone(), known.clone(), policy.clone(), identity.clone(), lxmf_hash, ev.clone());
    // Known identities (and tickets) are saved every few seconds, one save
    // at a time.
    let mut known_save: Option<tokio::task::JoinHandle<()>> = None;
    let announce_data = |name: &str| app_data(name, policy.lock().unwrap().stamp_cost);
    let mut announces_missed = 0u64;
    // Syncing from the node hosted here: its messages for us were delivered
    // as they came in.
    let sync = |node: Option<Hash>, local: &Option<LocalNode>| match local {
        Some(local) if node == Some(local.hash) => {
            let _ = ev.send(NetEvent::Synced(Ok(0)));
        }
        _ => syncer.start(node),
    };

    // Give interfaces a moment to come up before announcing or syncing.
    let startup = tokio::time::sleep(Duration::from_secs(3));
    tokio::pin!(startup);
    let mut startup_pending = true;
    let day = Duration::from_secs(86_400);
    let mut announce_interval = options.announce_interval;
    let mut sync_interval = options.sync_interval;
    // A timer's first tick is one interval from now, not immediately.
    let timer = |every: Option<Duration>| {
        let every = every.unwrap_or(day);
        tokio::time::interval_at(tokio::time::Instant::now() + every, every)
    };
    let mut announce_timer = timer(announce_interval);
    let mut sync_timer = timer(sync_interval);
    let mut stats_timer = tokio::time::interval(STATS_INTERVAL);

    loop {
        tokio::select! {
            () = &mut startup, if startup_pending => {
                startup_pending = false;
                if options.announce_at_start {
                    announce(&delivery.handle, announce_data(&display_name), &ev).await;
                }
                if propagation_node.is_some() && sync_interval.is_some() {
                    sync(propagation_node, &pn_local);
                }
            }
            _ = announce_timer.tick(), if announce_interval.is_some() => {
                announce(&delivery.handle, announce_data(&display_name), &ev).await;
            }
            _ = sync_timer.tick(), if sync_interval.is_some() && propagation_node.is_some() => {
                sync(propagation_node, &pn_local);
            }
            _ = stats_timer.tick() => {
                if known_save.as_ref().is_none_or(|save| save.is_finished()) {
                    let (known, policy) = (known.clone(), policy.clone());
                    known_save = Some(tokio::spawn(async move {
                        KnownIdentities::save_in_background(&known).await;
                        lxmf::Policy::save_in_background(&policy).await;
                    }));
                }
                let missed = lxmf_announces.dropped_events() + nomad_announces.dropped_events() + pn_announces.dropped_events();
                if missed > announces_missed {
                    let _ = ev.send(NetEvent::Log(format!(
                        "Missed {} announce(s): more arrived at once than could be taken in",
                        missed - announces_missed
                    )));
                    announces_missed = missed;
                }
                if let Ok(stats) = runtime.interface_stats().await {
                    iface_log::set_names(stats.interfaces.iter().map(|i| (i.id, i.name.clone())));
                    let interfaces = stats
                        .interfaces
                        .into_iter()
                        .map(|i| InterfaceInfo {
                            name: i.name,
                            online: i.online,
                            rx_bytes: i.rx_bytes,
                            tx_bytes: i.tx_bytes,
                        })
                        .collect();
                    let _ = ev.send(NetEvent::Interfaces(interfaces));
                }
                if let Some(node) = &hosted {
                    let _ = ev.send(NetEvent::Host(HostEvent::Stats(node.stats())));
                }
                if let Ok(slot) = pn_slot.try_lock()
                    && let Some(node) = slot.as_ref()
                {
                    let _ = ev.send(NetEvent::Pn(PnEvent::Stats(node.stats())));
                }
            }
            command = cmd_rx.recv() => {
                let Some(command) = command else { break };
                match command {
                    NetCommand::Shutdown(why) => {
                        stop = why;
                        break;
                    }
                    NetCommand::Announce => announce(&delivery.handle, announce_data(&display_name), &ev).await,
                    NetCommand::SetDisplayName(name) => {
                        display_name = name;
                        if let Err(e) = delivery.handle.set_default_app_data(Some(announce_data(&display_name))).await {
                            let _ = ev.send(NetEvent::Log(format!("Could not update announce data: {e}")));
                        }
                    }
                    NetCommand::SetPolicy { stamp_cost, max_bytes } => {
                        let changed = {
                            let mut policy = policy.lock().unwrap();
                            policy.max_bytes = max_bytes;
                            std::mem::replace(&mut policy.stamp_cost, stamp_cost) != stamp_cost
                        };
                        // Senders learn a new cost from the announce.
                        if changed {
                            if let Err(e) = delivery.handle.set_default_app_data(Some(announce_data(&display_name))).await {
                                let _ = ev.send(NetEvent::Log(format!("Could not update announce data: {e}")));
                            }
                            announce(&delivery.handle, announce_data(&display_name), &ev).await;
                        }
                    }
                    NetCommand::SetContacts { trusted, exempt } => policy.lock().unwrap().set_contacts(trusted, exempt),
                    NetCommand::Blackhole { to, block, quiet } => {
                        let (runtime, known, ev) = (runtime.clone(), known.clone(), ev.clone());
                        tokio::spawn(async move {
                            let result = blackhole(&runtime, &known, to, block).await;
                            if result.is_err() || !quiet {
                                let _ = ev.send(NetEvent::Log(result.unwrap_or_else(|e| e)));
                            }
                        });
                    }
                    NetCommand::SetPropagationNode(node) => propagation_node = node,
                    NetCommand::SetIntervals { announce, sync } => {
                        if announce != announce_interval {
                            announce_interval = announce;
                            announce_timer = timer(announce);
                        }
                        if sync != sync_interval {
                            sync_interval = sync;
                            sync_timer = timer(sync);
                        }
                    }
                    NetCommand::Sync => sync(propagation_node, &pn_local),
                    NetCommand::WritePaper { id, paper } => {
                        let (runtime, known, identity, ev) =
                            (runtime.clone(), known.clone(), identity.clone(), ev.clone());
                        tokio::spawn(async move {
                            let result = lxmf::paper::write(&runtime, &known, &identity, lxmf_hash, paper).await;
                            let _ = ev.send(NetEvent::Paper { id, result });
                        });
                    }
                    NetCommand::ReadPaper(link) => lxmf::paper::read(&runtime, &known, &identity, lxmf_hash, link, &ev),
                    NetCommand::SendMessage { id, message } => {
                        let (runtime, known, identity, ev, policy) =
                            (runtime.clone(), known.clone(), identity.clone(), ev.clone(), policy.clone());
                        let (stamp_ticket, ticket) = policy.lock().unwrap().for_outgoing(message.to);
                        let local_node = pn_local.clone();
                        let outgoing = lxmf::Outgoing { propagation_node, stamp_ticket, ticket, local_node, ..message };
                        let (to, gives_ticket) = (outgoing.to, outgoing.ticket.is_some());
                        tokio::spawn(async move {
                            let result = lxmf::send(&runtime, &known, &identity, lxmf_hash, outgoing).await;
                            if gives_ticket && result.is_ok() {
                                policy.lock().unwrap().ticket_delivered(to);
                            }
                            let _ = ev.send(NetEvent::Delivery { id, result });
                        });
                    }
                    NetCommand::RrcConnect { hub, aspect, nick } => {
                        let config = rrc_session::SessionConfig { hub, aspect, nick };
                        rrc_sessions.connect(&runtime, &known, &identity, config, &ev);
                    }
                    NetCommand::Rrc { hub, command } => rrc_sessions.command(hub, command, &ev),
                    NetCommand::Host(config) => {
                        host_generation += 1;
                        if let Some(node) = hosted.take() {
                            node.shutdown();
                        }
                        match config {
                            Some(config) => launch(host_generation, config),
                            None => {
                                let _ = ev.send(NetEvent::Host(HostEvent::Stopped));
                            }
                        }
                    }
                    NetCommand::HostReload => {
                        if let Some(Err(e)) = hosted.as_ref().map(|node| node.reload_routes()) {
                            let _ = ev.send(NetEvent::Log(format!("Could not reload node pages: {e}")));
                        }
                    }
                    NetCommand::HostAnnounce => {
                        let result = match &hosted {
                            Some(node) => node.announce_now().await.map_err(|e| e.to_string()),
                            None => Err("the node is not running".to_string()),
                        };
                        let line = match result {
                            Ok(()) => "Announced the node".to_string(),
                            Err(e) => format!("Node announce failed: {e}"),
                        };
                        let _ = ev.send(NetEvent::Log(line));
                    }
                    NetCommand::Propagation(config) => {
                        if config != pn_config {
                            pn_config = config.clone();
                            pn_local = None;
                            launch_pn(config);
                        }
                    }
                    NetCommand::Fetch { id, node, path, fields, identify } => {
                        let (runtime, links, ev) = (runtime.clone(), links.clone(), ev.clone());
                        let identity = identify.then(|| identity.clone());
                        tokio::spawn(async move {
                            let result = nomad::fetch(&runtime, &links, node, &path, &fields, identity).await;
                            let _ = ev.send(NetEvent::Fetched { id, result });
                        });
                    }
                }
            }
            Some(announce) = lxmf_announces.recv() => {
                if let Some(key) = &announce.public_key {
                    known.lock().unwrap().remember(announce.destination_hash, key, announce.app_data.as_deref());
                }
                let name = announce.app_data.as_deref().and_then(lxmf_display_name);
                let _ = ev.send(NetEvent::Announce {
                    kind: PeerKind::Lxmf,
                    hash: announce.destination_hash,
                    name,
                    hops: announce.hops,
                });
            }
            Some(announce) = nomad_announces.recv() => {
                let name = announce
                    .app_data
                    .as_deref()
                    .map(|d| nomad_core::clamp_node_name(&String::from_utf8_lossy(d)))
                    .filter(|n| !n.is_empty());
                let _ = ev.send(NetEvent::Announce {
                    kind: PeerKind::Nomad,
                    hash: announce.destination_hash,
                    name,
                    hops: announce.hops,
                });
            }
            Some(announce) = pn_announces.recv() => {
                // Only list nodes that are actually serving.
                let Some(data) = announce.app_data.as_deref().and_then(parse_pn_announce_data) else {
                    continue;
                };
                if !data.node_state {
                    continue;
                }
                if let Some(key) = &announce.public_key {
                    known.lock().unwrap().remember(announce.destination_hash, key, announce.app_data.as_deref());
                }
                let name = data
                    .metadata
                    .get(&lxmf_core::constants::PN_META_NAME)
                    .map(|n| String::from_utf8_lossy(n).into_owned());
                let _ = ev.send(NetEvent::Announce {
                    kind: PeerKind::Propagation,
                    hash: announce.destination_hash,
                    name,
                    hops: announce.hops,
                });
            }
            // Opportunistic delivery: a single packet carrying everything after
            // the destination hash.
            Some(packet) = delivery.events.packets.recv() => {
                lxmf::spawn_inbound(&runtime, &known, &policy, lxmf::with_destination(lxmf_hash, packet.data), &ev);
            }
            // Direct delivery over a Link, as a packet or as a Resource.
            Some((data, _link_id)) = delivery.events.link_packets.recv() => {
                lxmf::spawn_inbound(&runtime, &known, &policy, lxmf::with_destination(lxmf_hash, data), &ev);
            }
            Some((generation, result)) = host_rx.recv() => {
                // A start that was superseded (or stopped) meanwhile is dropped.
                if generation != host_generation {
                    continue;
                }
                let event = match result {
                    Ok(node) => {
                        let hash = node.destination_hash();
                        hosted = Some(node);
                        HostEvent::Started { hash }
                    }
                    Err(e) => HostEvent::Failed(e),
                };
                let _ = ev.send(NetEvent::Host(event));
            }
            Some(completion) = delivery.events.resource_completions.recv() => {
                lxmf::spawn_inbound(&runtime, &known, &policy, lxmf::with_destination(lxmf_hash, completion.data), &ev);
            }
            // Sent to us through the propagation node hosted here.
            Some(data) = pn_deliver.recv() => lxmf::spawn_inbound(&runtime, &known, &policy, data, &ev),
            Some((generation, result)) = pn_rx.recv() => {
                if generation != pn_generation.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                let event = match result {
                    Ok(Some(local)) => {
                        let hash = local.hash;
                        pn_local = Some(local);
                        PnEvent::Started { hash }
                    }
                    Ok(None) => PnEvent::Stopped,
                    Err(e) => PnEvent::Failed(e),
                };
                let _ = ev.send(NetEvent::Pn(event));
            }
        }
    }

    // Save known identities (and tickets) while hubs are told we are leaving.
    let final_save = {
        let (known, policy, previous) = (known.clone(), policy.clone(), known_save.take());
        tokio::spawn(async move {
            if let Some(save) = previous {
                let _ = save.await;
            }
            KnownIdentities::save_in_background(&known).await;
            lxmf::Policy::save_in_background(&policy).await;
        })
    };
    // Hubs first, so they see us leave at once. The rest is tidying up: it
    // can wait on open links (e.g. a peer's LXMF link), so it is bounded.
    rrc_sessions.close_all().await;
    drop(hosted);
    let quick = Duration::from_millis(500);
    let _ = tokio::time::timeout(quick, async {
        if let Some(node) = pn_slot.lock().await.take() {
            node.stop().await;
        }
    })
    .await;
    let _ = tokio::time::timeout(quick, async {
        let _ = lxmf_announces.close().await;
        let _ = nomad_announces.close().await;
        let _ = pn_announces.close().await;
    })
    .await;
    let _ = tokio::time::timeout(quick, delivery.close()).await;
    let _ = tokio::time::timeout(quick, final_save).await;
    let _ = tokio::time::timeout(stop.runtime_wait(), runtime.shutdown_and_wait()).await;
    let _ = ev.send(NetEvent::Stopped);
    Ok(())
}

/// Ask the actor to shut down and wait until it has (bounded; see [`Stop`]).
pub async fn shutdown(
    commands: &mpsc::UnboundedSender<NetCommand>,
    events: &mut mpsc::UnboundedReceiver<NetEvent>,
    why: Stop,
) {
    if commands.send(NetCommand::Shutdown(why)).is_err() {
        return;
    }
    let stopped = async {
        while let Some(event) = events.recv().await {
            if matches!(event, NetEvent::Stopped) {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(why.total_wait(), stopped).await;
}

/// Block (or unblock) the identity behind LXMF address `to` in Reticulum:
/// its announces and traffic are dropped by the transport. Says what it did.
async fn blackhole(runtime: &ReticulumHandle, known: &Known, to: Hash, block: bool) -> Result<String, String> {
    use rns_transport::blackhole::BlackholeReason;
    use rns_transport::messages::TransportQuery;
    let address = hex::encode(to);
    let remote = lookup(runtime, known, to).await.map_err(|e| {
        format!("Could not change the block on {address} in Reticulum: their identity isn't known ({e}); rettui still ignores their messages")
    })?;
    let hash = remote.identity.hash;
    let query = if block {
        TransportQuery::BlackholeIdentity { hash, ttl: None, reason: BlackholeReason::Manual, reason_label: Some("blocked in rettui".into()) }
    } else {
        TransportQuery::UnblackholeIdentity { hash }
    };
    runtime
        .query_control_result(query)
        .await
        .map(|_| format!("{} {address}'s identity in Reticulum", if block { "Blocked" } else { "Unblocked" }))
        .map_err(|e| format!("Could not change the block on {address} in Reticulum: {e}"))
}

async fn announce(
    destination: &DestinationHandle,
    app_data: Vec<u8>,
    ev: &mpsc::UnboundedSender<NetEvent>,
) {
    let options = DestinationAnnounceOptions {
        app_data: Some(app_data),
        ..DestinationAnnounceOptions::default()
    };
    match destination.announce(options).await {
        Ok(_) => {
            let _ = ev.send(NetEvent::Announced);
        }
        Err(e) => {
            let _ = ev.send(NetEvent::Log(format!("Announce failed: {e}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lxmf_names_in_both_announce_formats() {
        // Current format: msgpack [name, stamp cost].
        let packed = [0x92, 0xc4, 0x03, b'b', b'o', b'b', 0xc0];
        assert_eq!(lxmf_display_name(&packed).as_deref(), Some("bob"));
        // Original format: the name itself.
        assert_eq!(lxmf_display_name(b"pybot").as_deref(), Some("pybot"));
        assert_eq!(lxmf_display_name(b"a\x07b").as_deref(), Some("ab"));
        assert_eq!(lxmf_display_name(b"").as_deref(), None);
        assert_eq!(lxmf_display_name(&[0xff, 0xfe]).as_deref(), None);
    }
}
