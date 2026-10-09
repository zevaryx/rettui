//! Hosting an RRC hub: rsRRCD's hub (rooms, modes, bans, invites, its
//! commands, Resources) run on this client's Reticulum instance.
//!
//! rsRRCD's [`Router`] knows nothing of the network: it's told of Links and
//! what comes over them, and says what to send. This module is the rest:
//!
//! - **Identity:** the hub has its own, not yours, so people who join learn
//!   the hub's address and not your LXMF one (as rrcd and Ratspeak do).
//! - **Files:** rsRRCD's own `config.yaml` and `rooms.yaml` in the hub's
//!   folder, so its `/reload` and `/kline` work as they do in rsRRCD (and
//!   `rrcd-rs` could run the same folder). What the settings decide is
//!   written into `config.yaml` before the hub starts; its limits and bans
//!   are left as they are there.
//! - **Destination:** `rrc.hub`, announced with the hub's name as rrcd
//!   announces it.
//! - **Console:** the hub itself, as a session no Link carries, through
//!   which the Hub panel runs hub commands. It's a server operator, and an
//!   operator of a room for the length of a command about it (rsRRCD's room
//!   commands want one).
//! - **Policy:** whether anyone may make a room by joining it (rsRRCD lets
//!   them; Ratspeak's hub doesn't).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ciborium::value::Value;
use rns_identity::destination::Destination;
use rns_runtime::prelude::*;
use rsrrcd::{Action, HubConfig, RoomRegistry, Router};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::envelope::{DEFAULT_ASPECT, Envelope, t};
use crate::config::{Paths, Settings};
use crate::net::{Hash, NetEvent};

/// The rooms file, beside rsRRCD's `config.yaml` in the hub's folder.
const ROOMS_FILE: &str = "rooms.yaml";
/// The console's session: no Link has this id.
const CONSOLE: [u8; 16] = [0xff; 16];
/// Most Links at once; more are closed as they come.
const MAX_LINKS: usize = 256;
/// Commands rsRRCD lets only a room's operators run, naming the room first.
const ROOM_COMMANDS: &[&str] = &["/topic", "/mode", "/op", "/deop", "/voice", "/devoice", "/invite", "/ban", "/unban", "/kick"];

/// How the hub runs, from the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubHostConfig {
    /// Its folder: `config.yaml` and `rooms.yaml`.
    pub dir: PathBuf,
    /// Its identity file.
    pub identity: PathBuf,
    pub name: String,
    pub greeting: Option<String>,
    /// Time between announces (`None`: only when it starts).
    pub announce_interval: Option<Duration>,
    /// Anyone may make a room by joining it; else only `operator` can.
    pub open_rooms: bool,
    /// Your identity: an operator of the hub (`/kline`, `/reload`, `/stats`).
    pub operator: Hash,
}

impl HubHostConfig {
    /// The hub the settings ask for, if any.
    pub fn from_settings(settings: &Settings, paths: &Paths, operator: Hash) -> Option<Self> {
        settings.hub_enabled.then(|| Self {
            dir: paths.rrc_hub.clone(),
            identity: paths.rrc_hub_identity.clone(),
            name: settings.hub_name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| settings.display_name.clone()),
            // One line in the settings: `\n` starts another.
            greeting: settings.hub_greeting.as_deref().map(|g| g.replace("\\n", "\n")).filter(|g| !g.trim().is_empty()),
            announce_interval: (settings.hub_announce_interval_mins > 0)
                .then(|| Duration::from_secs(settings.hub_announce_interval_mins * 60)),
            open_rooms: settings.hub_open_rooms,
            operator,
        })
    }

    /// The same hub, so these settings apply to it as it runs: only its
    /// name, greeting, announces and who makes rooms differ.
    pub fn same_hub(&self, other: &Self) -> bool {
        self.dir == other.dir && self.identity == other.identity && self.operator == other.operator
    }
}

/// The hub's identity, made the first time.
pub fn identity(path: &Path) -> Result<Identity, String> {
    if path.exists() {
        return Identity::from_file(path).map_err(|e| format!("{} isn't an identity file: {e}", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't make {}: {e}", parent.display()))?;
    }
    let identity = Identity::new();
    identity.to_file(path).map_err(|e| format!("Couldn't save the hub's identity to {}: {e}", path.display()))?;
    Ok(identity)
}

/// The hub's address and public key, once it has an identity (before it
/// first starts, it has none).
pub fn address(path: &Path) -> Option<(Hash, [u8; 64])> {
    let identity = Identity::from_file(path).ok()?;
    Some((Destination::hash_from_name_and_identity(DEFAULT_ASPECT, Some(&identity.hash)), identity.get_public_key()))
}

/// What rrcd, rsRRCD and the Go hub announce: a CBOR map naming the
/// protocol and the hub.
pub fn announce_data(name: &str) -> Vec<u8> {
    let text = |s: &str| Value::Text(s.to_string());
    let map = Value::Map(vec![(text("proto"), text("rrc")), (text("v"), Value::Integer(1.into())), (text("hub"), text(name))]);
    let mut out = Vec::new();
    ciborium::into_writer(&map, &mut out).expect("writing to a Vec cannot fail");
    out
}

/// rsRRCD's configuration, with what the settings decide written into its
/// file first (the rest, such as its limits and bans, stays as it is
/// there). The hub's identity and yours are its operators.
fn write_config(config: &HubHostConfig, hub: Hash) -> Result<HubConfig, String> {
    std::fs::create_dir_all(&config.dir).map_err(|e| format!("Couldn't make {}: {e}", config.dir.display()))?;
    let path = config.dir.join(rsrrcd::CONFIG_FILE_NAME);
    let fresh = !path.exists();
    if fresh {
        HubConfig::write_default(&path).map_err(|e| format!("{e:#}"))?;
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let mut value: serde_json::Value =
        serde_saphyr::from_str(&text).map_err(|e| format!("{} isn't YAML rsRRCD reads: {e}", path.display()))?;
    if !value.is_object() {
        value = serde_json::json!({});
    }
    let root = value.as_object_mut().expect("an object");
    let hub_section = root.entry("hub").or_insert_with(|| serde_json::json!({}));
    if !hub_section.is_object() {
        *hub_section = serde_json::json!({});
    }
    let section = hub_section.as_object_mut().expect("an object");
    section.insert("hub_name".into(), config.name.clone().into());
    match &config.greeting {
        Some(greeting) => section.insert("greeting".into(), greeting.clone().into()),
        None => section.remove("greeting"),
    };
    // Beside the folder, so it can be backed up (or not) on its own.
    let identity = match (config.identity.parent(), config.dir.parent(), config.identity.file_name()) {
        (Some(a), Some(b), Some(name)) if a == b => format!("../{}", name.to_string_lossy()),
        _ => config.identity.to_string_lossy().into_owned(),
    };
    section.insert("identity_path".into(), identity.into());
    section.insert("room_registry_path".into(), ROOMS_FILE.into());
    section.insert("announce_period_s".into(), config.announce_interval.map_or(0.0, |d| d.as_secs_f64()).into());
    // Operators: kept as written, with the hub (the console) and you.
    let mut trusted: Vec<String> = section
        .get("trusted_identities")
        .and_then(serde_json::Value::as_array)
        .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_lowercase)).collect())
        .unwrap_or_default();
    for operator in [hex::encode(hub), hex::encode(config.operator)] {
        if !trusted.contains(&operator) {
            trusted.push(operator);
        }
    }
    section.insert("trusted_identities".into(), trusted.into());
    // Member lists with joins and leaves, which clients show (rsRRCD and
    // rrcd leave them out unless asked).
    if fresh {
        section.insert("include_joined_member_list".into(), true.into());
    }
    let yaml = serde_saphyr::to_string(&value).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    crate::config::write_atomic(&path, yaml.as_bytes()).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    HubConfig::load(&path).map_err(|e| format!("{e:#}"))
}

/// Who is connected, the rooms and the bans, for the Hub panel.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HubSnapshot {
    pub people: Vec<HubPerson>,
    pub rooms: Vec<HubRoom>,
    /// Identities banned from the whole hub (hex).
    pub klines: Vec<String>,
    /// Links whose identity isn't known yet.
    pub unidentified: usize,
    pub stats: HubStats,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HubStats {
    /// Messages, notices and actions passed on.
    pub messages: u64,
    pub joins: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub rate_limited: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HubPerson {
    /// Their identity (hex).
    pub identity: String,
    pub nick: Option<String>,
    pub rooms: Vec<String>,
    /// When they connected (Unix seconds).
    pub since: i64,
    /// Links open (the same identity can connect twice).
    pub links: usize,
    /// A hub operator.
    pub operator: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HubRoom {
    pub name: String,
    pub registered: bool,
    /// As `/mode` says it: `+imnt`, or `(none)`.
    pub modes: String,
    pub topic: Option<String>,
    /// Identities (hex) of who's in it.
    pub members: Vec<String>,
    pub founder: Option<String>,
    pub operators: Vec<String>,
    pub voiced: Vec<String>,
    pub banned: Vec<String>,
    /// Its key (`+k`), for those who may join with it.
    pub key: Option<String>,
    /// Invited (hex), whose invites haven't run out.
    pub invited: Vec<String>,
}

/// Something to do, from the Hub panel.
#[derive(Debug, Clone)]
pub enum HubAction {
    Announce,
    /// A hub command, run by the hub itself (an operator everywhere):
    /// `/kick lobby <identity>`, `/kline add <identity>`, `/stats`...
    Run(String),
    /// Register (or unregister) a room, made if need be: a registered room
    /// keeps its modes and topic while nobody's in it.
    Register {
        room: String,
        on: bool,
    },
    /// Close a room: everyone in it is taken out, and it goes with its
    /// settings and bans (neither rsRRCD nor RRC has a command for it).
    Delete(String),
    /// Move a room to another name, with its settings and bans. RRC can't
    /// move people from one room to another: those in it are told the new
    /// name and taken out of the old one.
    Rename {
        from: String,
        to: String,
    },
    /// Close every Link of an identity.
    Disconnect(Hash),
    /// New settings, for the hub as it runs (see
    /// [`HubHostConfig::same_hub`]): written to `config.yaml`, which the hub
    /// reloads, as with `/reload`.
    Configure(HubHostConfig),
}

/// What the hub is doing.
#[derive(Debug)]
pub enum HubEvent {
    Started {
        hash: Hash,
        public_key: [u8; 64],
    },
    Stopped,
    Failed(String),
    State(HubSnapshot),
    /// What a command from the panel came to.
    Reply {
        text: String,
        error: bool,
    },
}

/// The hub, running.
pub struct HostedHub {
    pub hash: Hash,
    pub public_key: [u8; 64],
    actions: mpsc::UnboundedSender<HubAction>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl HostedHub {
    /// Start the hub on the running Reticulum instance; it reports on
    /// `events` (as [`NetEvent::Hub`]) until it stops.
    pub async fn start(runtime: &ReticulumHandle, config: &HubHostConfig, events: mpsc::UnboundedSender<NetEvent>) -> Result<Self, String> {
        let prepare = config.clone();
        let (identity, router) = tokio::task::spawn_blocking(move || -> Result<(Identity, Router), String> {
            let identity = identity(&prepare.identity)?;
            let hub_config = write_config(&prepare, identity.hash)?;
            let router = Router::load(hub_config, identity.hash).map_err(|e| format!("{e:#}"))?;
            Ok((identity, router))
        })
        .await
        .map_err(|e| e.to_string())??;
        let hash = Destination::hash_from_name_and_identity(DEFAULT_ASPECT, Some(&identity.hash));
        let public_key = identity.get_public_key();
        let max_resource = router.config.max_resource_bytes;
        let destination = runtime
            .register_destination(
                identity.clone(),
                DEFAULT_ASPECT,
                DestinationRuntimeOptions {
                    resource_strategy: ResourceStrategy::AcceptApp,
                    resource_accept: Some(ResourceAcceptPolicy::new(move |_, advertisement| advertisement.data_size <= max_resource)),
                    default_app_data: Some(announce_data(&config.name)),
                    ..DestinationRuntimeOptions::default()
                },
            )
            .await
            .map_err(|e| format!("Could not register the hub: {e}"))?;
        let (actions_tx, actions) = mpsc::unbounded_channel();
        let (stop, stopped) = oneshot::channel();
        let mut core = Core::new(router, identity.hash, config.clone(), events);
        core.open_console();
        let hub = Hub { handle: destination.handle.clone(), core };
        let task = tokio::spawn(hub.run(destination, actions, stopped));
        Ok(Self { hash, public_key, actions: actions_tx, stop: Some(stop), task: Some(task) })
    }

    pub fn act(&self, action: HubAction) {
        let _ = self.actions.send(action);
    }

    /// Stop the hub (its Links closed, so people see it go), waiting a
    /// little for it.
    pub async fn stop(mut self) {
        drop(self.stop.take());
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
        }
    }
}

/// The hub's task: rsRRCD's router and the destination it's on.
struct Hub {
    handle: DestinationHandle,
    core: Core,
}

/// The hub, but sending: what the router says, what the console asks and
/// what the panel is shown.
struct Core {
    router: Router,
    /// The hub's identity hash (its console's).
    identity: Hash,
    config: HubHostConfig,
    events: mpsc::UnboundedSender<NetEvent>,
    /// Links, and when they came (Unix seconds).
    connected: HashMap<[u8; 16], i64>,
    /// The snapshot last sent.
    shown: Option<HubSnapshot>,
}

fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

impl Hub {
    async fn run(
        mut self,
        mut destination: RegisteredDestination,
        mut actions: mpsc::UnboundedReceiver<HubAction>,
        mut stopped: oneshot::Receiver<()>,
    ) {
        // Announce (after interfaces have had a moment), then every so often.
        let announce_soon = tokio::time::sleep(Duration::from_secs(3));
        tokio::pin!(announce_soon);
        let mut announced = false;
        let timer = |interval: Option<Duration>| {
            let every = interval.unwrap_or(Duration::from_secs(86_400));
            tokio::time::interval_at(tokio::time::Instant::now() + every, every)
        };
        let mut announce_timer = timer(self.core.config.announce_interval);
        let mut maintenance = tokio::time::interval(Duration::from_secs(1));
        self.core.report();
        loop {
            let events = &mut destination.events;
            tokio::select! {
                _ = &mut stopped => break,
                () = &mut announce_soon, if !announced => {
                    announced = true;
                    self.announce().await;
                }
                _ = announce_timer.tick(), if self.core.config.announce_interval.is_some() => self.announce().await,
                _ = maintenance.tick() => {
                    let actions = self.core.router.liveness_tick();
                    self.apply(actions).await;
                    self.core.report();
                }
                Some(action) = actions.recv() => {
                    if let HubAction::Configure(config) = &action
                        && config.announce_interval != self.core.config.announce_interval
                    {
                        announce_timer = timer(config.announce_interval);
                    }
                    self.act(action).await;
                    self.core.report();
                }
                Some(link) = events.links_established.recv() => {
                    if self.core.connected.len() >= MAX_LINKS {
                        let _ = self.handle.close_link(link, CloseReason::DestinationClosed, true).await;
                        continue;
                    }
                    self.core.connected.insert(link, unix_now() as i64);
                    self.core.router.established(link);
                }
                Some((link, peer)) = events.links_identified.recv() => {
                    let actions = self.core.router.identified(link, peer);
                    self.apply(actions).await;
                }
                Some((data, link)) = events.link_packets.recv() => {
                    let actions = match self.core.screen(link, &data) {
                        Some(refusal) => refusal,
                        None => self.core.router.packet(link, &data),
                    };
                    self.apply(actions).await;
                }
                Some(done) = events.resource_completions.recv() => {
                    let actions = self.core.router.resource_received(done.link_id, done.data);
                    self.apply(actions).await;
                }
                Some(link) = events.links_closed.recv() => {
                    self.core.connected.remove(&link);
                    let actions = self.core.router.closed(link);
                    self.apply(actions).await;
                }
                // Nothing else is used; taken so nothing waits on it.
                Some(_) = events.packets.recv() => {}
                Some(_) = events.link_packet_proofs.recv() => {}
                Some(_) = events.resource_proofs.recv() => {}
                Some(_) = events.resource_events.recv() => {}
                Some(_) = events.channel_messages.recv() => {}
                else => break,
            }
        }
        // Leave properly: everyone sees the hub go at once.
        let links: Vec<[u8; 16]> = self.core.connected.keys().copied().collect();
        for link in links {
            let actions = self.core.router.closed(link);
            self.apply(actions).await;
            let _ = self.handle.close_link(link, CloseReason::DestinationClosed, true).await;
        }
        let _ = destination.close().await;
    }

    async fn announce(&mut self) {
        let options =
            DestinationAnnounceOptions { app_data: Some(announce_data(&self.core.config.name)), ..DestinationAnnounceOptions::default() };
        match self.handle.announce(options).await {
            Ok(()) => self.core.router.state.counters.announces += 1,
            Err(e) => tracing::warn!("hub announce failed: {e}"),
        }
    }

    /// Send what the router says to send; what's for the console is a
    /// reply to the panel.
    async fn apply(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Send(CONSOLE, payload) | Action::SendResource(CONSOLE, payload) => self.core.reply(&payload),
                Action::Send(link, payload) => {
                    if let Err(e) = self.handle.send_link_packet(link, payload).await {
                        tracing::debug!("hub send failed: {e}");
                    }
                }
                Action::SendResource(link, payload) => {
                    if let Err(e) = self.handle.send_link_resource(link, payload, false).await {
                        tracing::debug!("hub resource send failed: {e}");
                    }
                }
                Action::Close(CONSOLE) => {}
                Action::Close(link) => {
                    let _ = self.handle.close_link(link, CloseReason::DestinationClosed, true).await;
                    // Others are told now (its close comes later, and does
                    // nothing more then).
                    self.core.connected.remove(&link);
                    let closed = self.core.router.closed(link);
                    Box::pin(self.apply(closed)).await;
                }
            }
        }
    }

    async fn act(&mut self, action: HubAction) {
        match action {
            HubAction::Announce => {
                self.announce().await;
                self.core.emit(HubEvent::Reply { text: "Announced the hub".into(), error: false });
            }
            HubAction::Run(text) => {
                let actions = self.core.run_command(&text);
                self.apply(actions).await;
            }
            HubAction::Register { room, on } => {
                let (reply, actions) = self.core.register(&room, on);
                self.core.emit(reply);
                self.apply(actions).await;
            }
            HubAction::Delete(room) => {
                let (reply, actions) = self.core.delete_room(&room);
                self.core.emit(reply);
                self.apply(actions).await;
            }
            HubAction::Rename { from, to } => {
                let (reply, actions) = self.core.rename_room(&from, &to);
                self.core.emit(reply);
                self.apply(actions).await;
            }
            HubAction::Disconnect(identity) => {
                let links: Vec<[u8; 16]> =
                    self.core.router.state.sessions.iter().filter(|(_, s)| s.peer == Some(identity)).map(|(link, _)| *link).collect();
                let count = links.len();
                self.apply(links.into_iter().map(Action::Close).collect()).await;
                let text = if count == 0 { "They aren't connected".to_string() } else { format!("Disconnected {}", hex::encode(identity)) };
                self.core.emit(HubEvent::Reply { text, error: count == 0 });
            }
            HubAction::Configure(config) => self.configure(config).await,
        }
    }

    /// New settings: into `config.yaml`, which the hub reloads (its reply
    /// goes to the panel); a new name is announced at once.
    async fn configure(&mut self, config: HubHostConfig) {
        let (prepare, hub) = (config.clone(), self.core.identity);
        match tokio::task::spawn_blocking(move || write_config(&prepare, hub)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return self.core.emit(HubEvent::Reply { text: e, error: true }),
            Err(e) => return self.core.emit(HubEvent::Reply { text: e.to_string(), error: true }),
        }
        let renamed = config.name != self.core.config.name;
        self.core.config = config;
        let actions = self.core.run_command("/reload");
        self.apply(actions).await;
        if renamed {
            if let Err(e) = self.handle.set_default_app_data(Some(announce_data(&self.core.config.name))).await {
                tracing::debug!("hub app data not set: {e}");
            }
            self.announce().await;
        }
    }
}

impl Core {
    fn new(router: Router, identity: Hash, config: HubHostConfig, events: mpsc::UnboundedSender<NetEvent>) -> Self {
        Self { router, identity, config, events, connected: HashMap::new(), shown: None }
    }

    fn emit(&self, event: HubEvent) {
        let _ = self.events.send(NetEvent::Hub(event));
    }

    /// The console: a session for the hub itself, welcomed like any other.
    fn open_console(&mut self) {
        self.router.established(CONSOLE);
        let _ = self.router.identified(CONSOLE, self.identity);
        let hello = Envelope::new(t::HELLO, &self.identity).body(super::hello_body()).nick(Some("hub"));
        let _ = self.router.packet(CONSOLE, &hello.encode());
    }

    /// What rsRRCD isn't given, answered here instead:
    /// - a JOIN that would make a room, when only the operator may;
    /// - commands rsRRCD can't take (it panics, and a panic anywhere tears
    ///   the terminal UI down): `/ban <room> list` or `/invite <room> list`
    ///   for a room there isn't, and those naming someone nobody here
    ///   matches (see [`Core::unmatched`]).
    fn screen(&mut self, link: [u8; 16], data: &[u8]) -> Option<Vec<Action>> {
        let env = Envelope::decode(data).ok()?;
        let state = &self.router.state;
        let session = state.sessions.get(&link)?;
        let peer = session.peer?;
        let no_such_room = |room: &str, text: &str| {
            let error = Envelope::new(t::ERROR, &self.identity).room(room).text(text);
            Some(vec![Action::Send(link, error.encode())])
        };
        match env.t {
            t::JOIN if !self.config.open_rooms => {
                let room = state.normalize_room(env.room.as_deref()?, &self.router.config).ok()?;
                if peer == self.config.operator || state.rooms.contains_key(&room) {
                    return None;
                }
                no_such_room(&room, "no such room (only the hub's host makes rooms here)")
            }
            // Commands, once it has said hello (before, rsRRCD takes
            // anything for one).
            t::MSG | t::NOTICE if session.welcomed => {
                let text = env.body_text()?.trim();
                if !text.starts_with('/') {
                    return None;
                }
                if let Some(room) = unlisted_room(&self.router, text) {
                    return no_such_room(&room, "no such room");
                }
                self.unmatched(link, peer, env.room.as_deref(), text)
            }
            _ => None,
        }
    }

    /// rsRRCD's commands naming someone (`/kick`, `/op`, `/deop`, `/voice`,
    /// `/devoice`, `/mode +o`..., `/ban`, `/unban`, `/invite`, `/kline`)
    /// panic when nobody here matches the name: those are answered here. A
    /// full identity is acted on as rsRRCD would (a ban, an invite or a
    /// kline for someone who isn't here); anything else isn't found, as
    /// rsRRCD says. `None`: rsRRCD can take it (someone matches, or it
    /// says no before looking).
    fn unmatched(&mut self, link: [u8; 16], peer: Hash, context: Option<&str>, text: &str) -> Option<Vec<Action>> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        let command = parts.first()?.to_ascii_lowercase();
        let router = &self.router;
        let (state, config) = (&router.state, &router.config);
        // The room (none: the whole hub), whom, and whether a full identity
        // does (else nobody matching isn't found).
        let (room, token, by_identity) = match command.as_str() {
            "/kline" => {
                let add_del = parts.get(1).is_some_and(|op| op.eq_ignore_ascii_case("add") || op.eq_ignore_ascii_case("del"));
                if !add_del || !config.trusted_identities.contains(&peer) {
                    return None;
                }
                (None, *parts.get(2)?, true)
            }
            "/op" | "/deop" | "/voice" | "/devoice" | "/kick" => (Some(*parts.get(1)?), *parts.get(2)?, false),
            "/mode" if parts.get(2).is_some_and(|flag| ["+o", "-o", "+v", "-v"].iter().any(|f| flag.eq_ignore_ascii_case(f))) => {
                (Some(parts[1]), parts.get(3).copied().unwrap_or_default(), false)
            }
            "/ban" | "/invite" if parts.get(2).is_some_and(|op| op.eq_ignore_ascii_case("add") || op.eq_ignore_ascii_case("del")) => {
                (Some(parts[1]), *parts.get(3)?, true)
            }
            "/unban" => (Some(*parts.get(1)?), *parts.get(2)?, true),
            _ => return None,
        };
        // Not a room operator's: rsRRCD says so before looking.
        let room = match room {
            Some(room) => {
                let room = state.normalize_room(room, config).ok()?;
                if !state.rooms.get(&room)?.is_operator(&peer) {
                    return None;
                }
                Some(room)
            }
            None => None,
        };
        // Who rsRRCD would find: in the room (an invite: anywhere).
        let within = room.as_deref().filter(|_| command != "/invite");
        if matches_anyone(router, token, within) {
            return None;
        }
        let say = |kind: u64, text: &str| {
            let mut env = Envelope::new(kind, &self.identity).text(text);
            if let Some(room) = context {
                env = env.room(room);
            }
            vec![Action::Send(link, env.encode())]
        };
        let identity = hex::decode(token.strip_prefix("0x").unwrap_or(token)).ok().and_then(|id| <Hash>::try_from(id).ok());
        let Some(target) = identity.filter(|_| by_identity) else {
            return Some(say(t::NOTICE, &format!("target '{token}' not found")));
        };
        let add =
            !(command == "/unban" || parts.get(if command == "/kline" { 1 } else { 2 }).is_some_and(|op| op.eq_ignore_ascii_case("del")));
        let Some(room) = room else {
            // A kline (nobody with that identity is connected).
            let banned = &mut self.router.config.banned_identities;
            if add && !banned.contains(&target) {
                banned.push(target);
            }
            if !add {
                banned.retain(|id| *id != target);
            }
            if let Err(e) = self.router.config.save_banned_identities() {
                return Some(say(t::ERROR, &format!("kline persist failed: {e}")));
            }
            return Some(say(t::NOTICE, &format!("kline {} for {}", if add { "added" } else { "removed" }, hex::encode(target))));
        };
        let timeout = self.router.config.room_invite_timeout_s;
        let state = self.router.state.rooms.get_mut(&room)?;
        let said = match (command.as_str(), add) {
            ("/invite", true) if state.key.is_some() || state.invite_only => {
                state.invited.insert(target, unix_now() + timeout);
                format!("invite added in {room} (expires in {}s)", timeout as u64)
            }
            ("/invite", true) => format!("invite sent to {token} for {room}"),
            ("/invite", false) => {
                state.invited.remove(&target);
                format!("invite removed in {room}")
            }
            (_, true) => {
                state.banned.insert(target);
                format!("ban added in {room}")
            }
            (_, false) => {
                state.banned.remove(&target);
                format!("ban removed in {room}")
            }
        };
        state.last_used_ts = unix_now();
        if let Err(e) = RoomRegistry::save(&self.router.config.room_registry_path, &self.router.state.rooms) {
            return Some(say(t::ERROR, &format!("room persist failed: {e:#}")));
        }
        Some(say(t::NOTICE, &said))
    }

    /// A notice or error for the console: what a panel command came to.
    fn reply(&self, payload: &[u8]) {
        let (text, error) = match Envelope::decode(payload) {
            Ok(env) if matches!(env.t, t::NOTICE | t::ERROR) => match env.body_text() {
                Some(text) => (text.to_string(), env.t == t::ERROR),
                None => return,
            },
            Ok(_) => return,
            // A long reply comes as a Resource: its text.
            Err(_) => (String::from_utf8_lossy(payload).into_owned(), false),
        };
        self.emit(HubEvent::Reply { text, error });
    }

    /// A hub command, as the console. Room commands want a room operator:
    /// the hub is one for the length of the command.
    fn run_command(&mut self, text: &str) -> Vec<Action> {
        if let Some(room) = unlisted_room(&self.router, text) {
            self.emit(HubEvent::Reply { text: format!("There's no room {room}"), error: true });
            return Vec::new();
        }
        let parts: Vec<&str> = text.split_whitespace().collect();
        let command = parts.first().map(|c| c.to_lowercase()).unwrap_or_default();
        let room = parts
            .get(1)
            .filter(|_| ROOM_COMMANDS.contains(&command.as_str()))
            .and_then(|room| self.router.state.normalize_room(room, &self.router.config).ok());
        let lent = room
            .as_ref()
            .is_some_and(|room| self.router.state.rooms.get_mut(room).is_some_and(|state| state.operators.insert(self.identity)));
        // Someone kicked is told; the others in the room, below.
        let kicked = (command == "/kick")
            .then(|| parts.get(2).and_then(|who| hex::decode(who).ok()).and_then(|id| <[u8; 16]>::try_from(id).ok()))
            .flatten()
            .and_then(|identity| Some((identity, *self.router.state.links_by_identity.get(&identity)?)))
            .filter(|(_, link)| room.as_ref().and_then(|room| self.router.state.rooms.get(room)).is_some_and(|r| r.members.contains(link)));
        let mut actions = match self.unmatched(CONSOLE, self.identity, None, text) {
            Some(answered) => answered,
            None => self.router.packet(CONSOLE, &Envelope::new(t::MSG, &self.identity).text(text).encode()),
        };
        // Done, though the hub said nothing back (it tells the room).
        let answered = actions.iter().any(|a| matches!(a, Action::Send(CONSOLE, _) | Action::SendResource(CONSOLE, _)));
        if !answered {
            self.emit(HubEvent::Reply { text: format!("Done: {text}"), error: false });
        }
        if let Some(room) = &room {
            if lent && let Some(state) = self.router.state.rooms.get_mut(room) {
                state.operators.remove(&self.identity);
            }
            if lent && self.router.state.rooms.get(room).is_some_and(|state| state.registered) {
                let _ = RoomRegistry::save(&self.router.config.room_registry_path, &self.router.state.rooms);
            }
            if let Some((identity, link)) = kicked {
                actions.extend(self.parted(room, identity, link));
            }
        }
        actions
    }

    /// Tell the room someone kicked from it has gone (rsRRCD only tells
    /// them), as a leave would.
    fn parted(&self, room: &str, identity: Hash, link: [u8; 16]) -> Vec<Action> {
        let Some(state) = self.router.state.rooms.get(room) else { return Vec::new() };
        if state.members.contains(&link) {
            return Vec::new(); // not kicked
        }
        let nick = self.router.state.sessions.get(&link).and_then(|s| s.nick.clone());
        let mut env = Envelope::new(t::PARTED, &self.identity).room(room).nick(nick.as_deref());
        if self.router.config.include_joined_member_list {
            env = env.body(Value::Array(vec![Value::Bytes(identity.to_vec())]));
        }
        let payload = env.encode();
        state.members.iter().map(|member| Action::Send(*member, payload.clone())).collect()
    }

    /// Register or unregister a room, telling who's in it.
    fn register(&mut self, room: &str, on: bool) -> (HubEvent, Vec<Action>) {
        let fail = |text: String| (HubEvent::Reply { text, error: true }, Vec::new());
        let name = match self.room_name(room) {
            Ok(name) => name,
            Err(e) => return fail(e),
        };
        let rooms = &mut self.router.state.rooms;
        if on {
            let state = rooms.entry(name.clone()).or_default();
            state.registered = true;
            // As rsRRCD registers a room: no messages from outside it, and
            // topics set by its operators.
            state.no_outside_messages = true;
            state.topic_ops_only = true;
            state.last_used_ts = unix_now();
            if state.founder.is_none() {
                state.founder = Some(self.config.operator);
            }
        } else {
            let Some(state) = rooms.get_mut(&name) else { return fail(format!("There's no room {name}")) };
            state.registered = false;
            if state.members.is_empty() {
                rooms.remove(&name);
            }
        }
        if let Err(e) = RoomRegistry::save(&self.router.config.room_registry_path, &self.router.state.rooms) {
            return fail(format!("Couldn't save the rooms: {e:#}"));
        }
        let said = if on { "room registered" } else { "room unregistered" };
        let payload = Envelope::new(t::NOTICE, &self.identity).room(&name).text(said).encode();
        let actions = self
            .router
            .state
            .rooms
            .get(&name)
            .map(|state| state.members.iter().map(|member| Action::Send(*member, payload.clone())).collect())
            .unwrap_or_default();
        let text = if on { format!("Registered {name}") } else { format!("Unregistered {name}") };
        (HubEvent::Reply { text, error: false }, actions)
    }

    /// A room's name as rettui names rooms (`#` is decoration), and then as
    /// rsRRCD does.
    fn room_name(&self, room: &str) -> Result<String, String> {
        self.router.state.normalize_room(&super::normalize_room(room), &self.router.config).map_err(|e| e.to_string())
    }

    /// Take everyone out of a room, telling them why first: as a kick does,
    /// so their clients leave it (rettui's, NomadNet's).
    fn take_out(&mut self, room: &str, members: &HashSet<[u8; 16]>, why: &str) -> Vec<Action> {
        let notice = Envelope::new(t::NOTICE, &self.identity).room(room).text(why).encode();
        let kicked = Envelope::new(t::ERROR, &self.identity).room(room).text(&format!("kicked from {room}")).encode();
        let mut actions = Vec::new();
        for link in members {
            if let Some(session) = self.router.state.sessions.get_mut(link) {
                session.rooms.remove(room);
            }
            actions.push(Action::Send(*link, notice.clone()));
            actions.push(Action::Send(*link, kicked.clone()));
        }
        actions
    }

    /// Close a room: everyone in it taken out, and it's gone (registered or
    /// not) with its settings and bans. Anyone may make it again by joining
    /// it, where anyone makes rooms.
    fn delete_room(&mut self, room: &str) -> (HubEvent, Vec<Action>) {
        let fail = |text: String| (HubEvent::Reply { text, error: true }, Vec::new());
        let name = match self.room_name(room) {
            Ok(name) => name,
            Err(e) => return fail(e),
        };
        let Some(state) = self.router.state.rooms.remove(&name) else { return fail(format!("There's no room {name}")) };
        let actions = self.take_out(&name, &state.members, &format!("{name} has been closed by the hub's host"));
        if let Err(e) = RoomRegistry::save(&self.router.config.room_registry_path, &self.router.state.rooms) {
            return (HubEvent::Reply { text: format!("Deleted {name}, but couldn't save the rooms: {e:#}"), error: true }, actions);
        }
        (HubEvent::Reply { text: format!("Deleted {name}"), error: false }, actions)
    }

    /// Move a room to another name, with its topic, settings, key,
    /// operators, bans and invites. It's registered, so it's there while
    /// those in it come back: RRC can't move them, so they're told the new
    /// name and taken out of the old one.
    fn rename_room(&mut self, from: &str, to: &str) -> (HubEvent, Vec<Action>) {
        let fail = |text: String| (HubEvent::Reply { text, error: true }, Vec::new());
        let (from, to) = match (self.room_name(from), self.room_name(to)) {
            (Ok(from), Ok(to)) => (from, to),
            (Err(e), _) | (_, Err(e)) => return fail(e),
        };
        if to.split_whitespace().count() != 1 {
            return fail("Room names are one word".into());
        }
        if from == to {
            return fail(format!("It's {to} already"));
        }
        if self.router.state.rooms.contains_key(&to) {
            return fail(format!("There's a room {to} already"));
        }
        let Some(mut state) = self.router.state.rooms.remove(&from) else { return fail(format!("There's no room {from}")) };
        let members = std::mem::take(&mut state.members);
        state.registered = true;
        state.last_used_ts = unix_now();
        self.router.state.rooms.insert(to.clone(), state);
        let actions = self.take_out(&from, &members, &format!("{from} is now {to}: join it there (/join {to})"));
        if let Err(e) = RoomRegistry::save(&self.router.config.room_registry_path, &self.router.state.rooms) {
            return (HubEvent::Reply { text: format!("Renamed {from} to {to}, but couldn't save the rooms: {e:#}"), error: true }, actions);
        }
        (HubEvent::Reply { text: format!("Renamed {from} to {to}"), error: false }, actions)
    }

    /// Send the panel what changed.
    fn report(&mut self) {
        let snapshot = self.snapshot();
        if self.shown.as_ref() != Some(&snapshot) {
            self.shown = Some(snapshot.clone());
            self.emit(HubEvent::State(snapshot));
        }
    }

    fn snapshot(&self) -> HubSnapshot {
        let state = &self.router.state;
        let trusted = &self.router.config.trusted_identities;
        let mut people: HashMap<Hash, HubPerson> = HashMap::new();
        let mut unidentified = 0;
        for (link, session) in &state.sessions {
            if *link == CONSOLE {
                continue;
            }
            let Some(peer) = session.peer else {
                unidentified += 1;
                continue;
            };
            let since = self.connected.get(link).copied().unwrap_or_default();
            let person = people.entry(peer).or_insert_with(|| HubPerson {
                identity: hex::encode(peer),
                nick: None,
                rooms: Vec::new(),
                since,
                links: 0,
                operator: trusted.contains(&peer),
            });
            person.links += 1;
            person.since = if person.since == 0 { since } else { person.since.min(since) };
            if session.nick.is_some() {
                person.nick.clone_from(&session.nick);
            }
            person.rooms.extend(session.rooms.iter().cloned());
        }
        let mut people: Vec<HubPerson> = people.into_values().collect();
        for person in &mut people {
            person.rooms.sort();
            person.rooms.dedup();
        }
        people.sort_by(|a, b| {
            let name = |p: &HubPerson| p.nick.clone().unwrap_or_else(|| p.identity.clone()).to_lowercase();
            name(a).cmp(&name(b)).then_with(|| a.identity.cmp(&b.identity))
        });
        let now = unix_now();
        let mut rooms: Vec<HubRoom> = state
            .rooms
            .iter()
            .map(|(name, room)| {
                let mut members: Vec<String> = room
                    .members
                    .iter()
                    .filter(|link| **link != CONSOLE)
                    .filter_map(|link| state.sessions.get(link)?.peer.map(hex::encode))
                    .collect();
                members.sort();
                members.dedup();
                HubRoom {
                    name: name.clone(),
                    registered: room.registered,
                    modes: modes(room),
                    topic: room.topic.clone(),
                    members,
                    founder: room.founder.map(hex::encode),
                    operators: hexes(room.operators.iter().filter(|id| **id != self.identity)),
                    voiced: hexes(room.voiced.iter()),
                    banned: hexes(room.banned.iter()),
                    key: room.key.clone(),
                    invited: hexes(room.invited.iter().filter(|(_, until)| **until > now).map(|(id, _)| id)),
                }
            })
            .collect();
        rooms.sort_by(|a, b| a.name.cmp(&b.name));
        let counters = &state.counters;
        HubSnapshot {
            people,
            rooms,
            klines: hexes(self.router.config.banned_identities.iter()),
            unidentified,
            stats: HubStats {
                messages: counters.messages_forwarded + counters.notices_forwarded + counters.actions_forwarded,
                joins: counters.joins,
                bytes_in: counters.bytes_in,
                bytes_out: counters.bytes_out,
                rate_limited: counters.rate_limited,
            },
        }
    }
}

/// The room of `/ban <room> list` or `/invite <room> list`, when there's no
/// such room (see [`Core::screen`]).
fn unlisted_room(router: &Router, text: &str) -> Option<String> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [command, room, operation, ..] = parts.as_slice() else { return None };
    let listing = matches!(command.to_lowercase().as_str(), "/ban" | "/invite") && operation.eq_ignore_ascii_case("list");
    let room = router.state.normalize_room(room, &router.config).ok()?;
    (listing && !router.state.rooms.contains_key(&room)).then_some(room)
}

/// Whether anyone connected answers to `token` (their nick, or the start of
/// their identity, six hex digits or more), in `room` if given: as rsRRCD
/// looks for whom a command names.
fn matches_anyone(router: &Router, token: &str, room: Option<&str>) -> bool {
    let token = token.trim().to_lowercase();
    if token.is_empty() {
        return false;
    }
    let hex_token = token.strip_prefix("0x").unwrap_or(&token);
    let by_hash = hex_token.len() >= 6 && hex_token.len().is_multiple_of(2) && hex_token.chars().all(|c| c.is_ascii_hexdigit());
    router.state.sessions.values().any(|session| {
        let Some(peer) = session.peer else { return false };
        let nick = session.nick.as_ref().is_some_and(|nick| nick.to_lowercase() == token);
        room.is_none_or(|room| session.rooms.contains(room)) && (nick || (by_hash && hex::encode(peer).starts_with(hex_token)))
    })
}

/// Identities as hex, in order.
fn hexes<'a>(ids: impl Iterator<Item = &'a Hash>) -> Vec<String> {
    let mut list: Vec<String> = ids.map(hex::encode).collect();
    list.sort();
    list
}

/// A room's modes as `/mode` says them (rsRRCD's order).
fn modes(room: &rsrrcd::Room) -> String {
    let flags: String = [
        (room.invite_only, 'i'),
        (room.key.is_some(), 'k'),
        (room.moderated, 'm'),
        (room.no_outside_messages, 'n'),
        (room.private, 'p'),
        (room.registered, 'r'),
        (room.topic_ops_only, 't'),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect();
    if flags.is_empty() { "(none)".into() } else { format!("+{flags}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUB: Hash = [9; 16];
    const YOU: Hash = [1; 16];
    const AMY: Hash = [2; 16];
    const BOB: Hash = [3; 16];

    fn config(dir: &Path, open_rooms: bool) -> HubHostConfig {
        HubHostConfig {
            dir: dir.join("rrc-hub"),
            identity: dir.join("rrc-hub-identity"),
            name: "Hilltop".into(),
            greeting: Some("Hello\nthere".into()),
            announce_interval: Some(Duration::from_secs(600)),
            open_rooms,
            operator: YOU,
        }
    }

    /// The hub's logic over a fresh folder, its console open; what it tells
    /// the panel comes out of the receiver.
    fn core(dir: &Path, open_rooms: bool) -> (Core, mpsc::UnboundedReceiver<NetEvent>) {
        let _ = std::fs::remove_dir_all(dir);
        let config = config(dir, open_rooms);
        let hub_config = write_config(&config, HUB).unwrap();
        let (events, rx) = mpsc::unbounded_channel();
        let mut core = Core::new(Router::load(hub_config, HUB).unwrap(), HUB, config, events);
        core.open_console();
        (core, rx)
    }

    /// Someone connects (on Link `link`) and says hello.
    fn connect(core: &mut Core, link: u8, who: Hash, nick: &str) {
        core.router.established([link; 16]);
        core.connected.insert([link; 16], 1_800_000_000);
        let _ = core.router.identified([link; 16], who);
        let hello = Envelope::new(t::HELLO, &who).body(crate::rrc::hello_body()).nick(Some(nick));
        let _ = core.router.packet([link; 16], &hello.encode());
    }

    /// What `who` (on Link `link`) sends, passing the screen first, as the
    /// hub does.
    fn send(core: &mut Core, link: u8, env: Envelope) -> Vec<Action> {
        let data = env.encode();
        match core.screen([link; 16], &data) {
            Some(refused) => refused,
            None => core.router.packet([link; 16], &data),
        }
    }

    fn join(core: &mut Core, link: u8, who: Hash, room: &str) -> Vec<Action> {
        send(core, link, Envelope::new(t::JOIN, &who).room(room))
    }

    /// What each Link is sent: (link, type, room, text).
    fn sent(actions: &[Action]) -> Vec<(u8, u64, Option<String>, Option<String>)> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Send(link, payload) => {
                    let env = Envelope::decode(payload).ok()?;
                    Some((link[0], env.t, env.room.clone(), env.body_text().map(str::to_string)))
                }
                _ => None,
            })
            .collect()
    }

    fn replies(rx: &mut mpsc::UnboundedReceiver<NetEvent>) -> Vec<(String, bool)> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|e| match e {
                NetEvent::Hub(HubEvent::Reply { text, error }) => Some((text, error)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn its_settings_go_into_rsrrcds_config_and_the_rest_is_left_as_written() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = config(&dir, true);
        let hub = write_config(&config, HUB).unwrap();
        assert_eq!((hub.hub_name.as_str(), hub.greeting.as_deref(), hub.announce_period_s), ("Hilltop", Some("Hello\nthere"), 600.0));
        assert_eq!(hub.trusted_identities, [HUB, YOU]);
        assert!(hub.include_joined_member_list);
        assert_eq!(hub.room_registry_path, config.dir.join(ROOMS_FILE));
        assert_eq!(hub.identity_path, config.dir.join("../rrc-hub-identity"));
        // Changed by hand: kept, but for what the settings decide.
        let path = config.dir.join(rsrrcd::CONFIG_FILE_NAME);
        let mut value: serde_json::Value = serde_saphyr::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let section = value["hub"].as_object_mut().unwrap();
        section.insert("max_rooms_per_session".into(), 3.into());
        section.insert("include_joined_member_list".into(), false.into());
        section.insert("hub_name".into(), "Elsewhere".into());
        section["trusted_identities"].as_array_mut().unwrap().push(hex::encode(AMY).into());
        std::fs::write(&path, serde_saphyr::to_string(&value).unwrap()).unwrap();
        let hub = write_config(&HubHostConfig { greeting: None, announce_interval: None, ..config.clone() }, HUB).unwrap();
        assert_eq!((hub.hub_name.as_str(), hub.greeting, hub.announce_period_s), ("Hilltop", None, 0.0));
        assert_eq!((hub.max_rooms_per_session, hub.include_joined_member_list), (3, false));
        assert_eq!(hub.trusted_identities, [HUB, YOU, AMY]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hubs_announce_their_name_as_rrcd_does() {
        let value: Value = ciborium::from_reader(&announce_data("Hilltop")[..]).unwrap();
        let map = value.as_map().unwrap();
        let get = |key: &str| map.iter().find(|(k, _)| k.as_text() == Some(key)).map(|(_, v)| v.clone());
        assert_eq!(get("proto").and_then(|v| v.as_text().map(str::to_string)).as_deref(), Some("rrc"));
        assert_eq!(get("hub").and_then(|v| v.as_text().map(str::to_string)).as_deref(), Some("Hilltop"));
    }

    #[test]
    fn the_console_runs_room_commands_without_being_an_operator_of_the_room() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-console-{}", std::process::id()));
        let (mut core, mut rx) = core(&dir, true);
        connect(&mut core, 1, AMY, "amy");
        connect(&mut core, 2, BOB, "bob");
        join(&mut core, 1, AMY, "lobby");
        join(&mut core, 2, BOB, "lobby");
        // Amy made it, so it's hers; the hub sets its topic all the same.
        let actions = core.run_command("/topic lobby Say hi");
        assert_eq!(core.router.state.rooms["lobby"].topic.as_deref(), Some("Say hi"));
        assert!(!core.router.state.rooms["lobby"].operators.contains(&HUB), "lent for the command only");
        assert!(sent(&actions).iter().any(|(link, ..)| *link == 2), "{:?}", sent(&actions));
        // A kick: Amy's told, and so is Bob, who stays.
        let actions = core.run_command(&format!("/kick lobby {}", hex::encode(AMY)));
        let sent = sent(&actions);
        assert!(sent.iter().any(|(link, ..)| *link == 1), "{sent:?}");
        assert!(sent.iter().any(|(link, kind, room, _)| (*link, *kind, room.as_deref()) == (2, t::PARTED, Some("lobby"))), "{sent:?}");
        let snapshot = core.snapshot();
        assert_eq!(snapshot.rooms[0].members, [hex::encode(BOB)]);
        // rsRRCD can't list the bans of a room there isn't: never asked.
        assert!(core.run_command("/ban nowhere list").is_empty());
        assert_eq!(replies(&mut rx).last(), Some(&("There's no room nowhere".to_string(), true)));
        let refused = send(&mut core, 2, Envelope::new(t::MSG, &BOB).text("/invite nowhere list"));
        assert_eq!(sent_kinds(&refused), [t::ERROR]);
        // What the console's told is a reply for the panel.
        core.reply(&Envelope::new(t::NOTICE, &HUB).text("Hub stats").encode());
        assert_eq!(replies(&mut rx), [("Hub stats".to_string(), false)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rooms_are_deleted_and_renamed_with_those_in_them_told() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-delete-{}", std::process::id()));
        let (mut core, _rx) = core(&dir, true);
        connect(&mut core, 1, AMY, "amy");
        connect(&mut core, 2, BOB, "bob");
        join(&mut core, 1, AMY, "lobby");
        join(&mut core, 2, BOB, "lobby");
        core.run_command("/topic lobby Say hi");
        core.run_command(&format!("/ban lobby add {}", hex::encode([5u8; 16])));
        // Renamed: its topic, founder and bans go with it; it's registered;
        // those in it are told where, and taken out.
        let (reply, actions) = core.rename_room("#Lobby", "hall");
        assert!(matches!(reply, HubEvent::Reply { ref text, error: false } if text == "Renamed lobby to hall"), "{reply:?}");
        let said = sent(&actions);
        for link in [1, 2] {
            let theirs: Vec<_> = said.iter().filter(|(to, ..)| *to == link).collect();
            assert_eq!(theirs.len(), 2, "{said:?}");
            assert_eq!((theirs[0].1, theirs[0].3.as_deref()), (t::NOTICE, Some("lobby is now hall: join it there (/join hall)")));
            assert_eq!((theirs[1].1, theirs[1].2.as_deref(), theirs[1].3.as_deref()), (t::ERROR, Some("lobby"), Some("kicked from lobby")));
        }
        let rooms = &core.router.state.rooms;
        assert!(!rooms.contains_key("lobby"));
        let hall = &rooms["hall"];
        assert_eq!((hall.topic.as_deref(), hall.founder, hall.registered), (Some("Say hi"), Some(AMY), true));
        assert!(hall.banned.contains(&[5; 16]) && hall.members.is_empty());
        assert!(core.router.state.sessions.values().all(|s| s.rooms.is_empty()));
        assert!(RoomRegistry::load(&core.router.config.room_registry_path).unwrap().contains_key("hall"));
        // They can come back to it under its new name.
        join(&mut core, 1, AMY, "hall");
        assert_eq!(core.router.state.rooms["hall"].members.len(), 1);
        // Not over another room, nor to itself, nor from one there isn't.
        join(&mut core, 2, BOB, "den");
        for (from, to, error) in [
            ("hall", "den", "There's a room den already"),
            ("hall", "hall", "It's hall already"),
            ("nowhere", "else", "There's no room nowhere"),
        ] {
            let (reply, actions) = core.rename_room(from, to);
            assert!(matches!(reply, HubEvent::Reply { ref text, error: true } if text == error), "{reply:?}");
            assert!(actions.is_empty());
        }
        // Deleted: whoever's in it is taken out, and it's gone, saved too.
        let (reply, actions) = core.delete_room("hall");
        assert!(matches!(reply, HubEvent::Reply { ref text, error: false } if text == "Deleted hall"), "{reply:?}");
        assert_eq!(
            sent(&actions),
            [
                (1, t::NOTICE, Some("hall".into()), Some("hall has been closed by the hub's host".into())),
                (1, t::ERROR, Some("hall".into()), Some("kicked from hall".into())),
            ]
        );
        assert!(!core.router.state.rooms.contains_key("hall"));
        assert!(!RoomRegistry::load(&core.router.config.room_registry_path).unwrap().contains_key("hall"));
        assert!(matches!(core.delete_room("hall").0, HubEvent::Reply { error: true, .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// rsRRCD panics on commands naming someone nobody here matches: none
    /// of them reaches it, from a room's founder or from the panel.
    #[test]
    fn commands_naming_nobody_here_never_reach_rsrrcd() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-unmatched-{}", std::process::id()));
        let (mut core, mut rx) = core(&dir, true);
        connect(&mut core, 1, AMY, "amy");
        connect(&mut core, 2, BOB, "bob");
        join(&mut core, 1, AMY, "lobby");
        let gone = hex::encode([5u8; 16]);
        // Amy founded the room: an operator of it, over her Link.
        for command in ["/kick lobby nobody", "/op lobby nobody", "/voice lobby bob", "/mode lobby +o", "/mode lobby -v nobody"] {
            let answer = send(&mut core, 1, Envelope::new(t::MSG, &AMY).room("lobby").text(command));
            let said = sent(&answer);
            assert_eq!(said.len(), 1, "{command}: {said:?}");
            assert_eq!((said[0].0, said[0].1, said[0].2.as_deref()), (1, t::NOTICE, Some("lobby")), "{command}");
            assert!(said[0].3.as_deref().is_some_and(|text| text.contains("not found")), "{command}: {said:?}");
        }
        // Someone who isn't here, by identity: banned, invited, unbanned.
        let answer = send(&mut core, 1, Envelope::new(t::MSG, &AMY).text(&format!("/ban lobby add {gone}")));
        assert_eq!(sent(&answer)[0].3.as_deref(), Some("ban added in lobby"));
        assert!(core.router.state.rooms["lobby"].banned.contains(&[5; 16]));
        let answer = send(&mut core, 1, Envelope::new(t::MSG, &AMY).text(&format!("/invite lobby add {gone}")));
        assert_eq!(sent(&answer)[0].3.as_deref(), Some(format!("invite sent to {gone} for lobby").as_str()));
        let answer = send(&mut core, 1, Envelope::new(t::MSG, &AMY).text(&format!("/unban lobby {gone}")));
        assert_eq!(sent(&answer)[0].3.as_deref(), Some("ban removed in lobby"));
        assert!(core.router.state.rooms["lobby"].banned.is_empty());
        // By a name nobody has: not found, nothing changed.
        let answer = send(&mut core, 1, Envelope::new(t::MSG, &AMY).text("/ban lobby add carol"));
        assert_eq!(sent(&answer)[0].3.as_deref(), Some("target 'carol' not found"));
        // Bob isn't an operator of it: rsRRCD tells him so itself.
        let answer = send(&mut core, 2, Envelope::new(t::MSG, &BOB).text("/kick lobby nobody"));
        assert_eq!(sent_kinds(&answer), [t::ERROR]);
        // Someone it does match: rsRRCD's to do.
        let answer = send(&mut core, 1, Envelope::new(t::MSG, &AMY).text("/voice lobby amy"));
        assert!(core.router.state.rooms["lobby"].voiced.contains(&AMY), "{:?}", sent(&answer));

        // The panel: a kline of someone gone, and lifted (what Bans does).
        replies(&mut rx);
        console(&mut core, &format!("/kline add {gone}"));
        assert!(core.router.config.banned_identities.contains(&[5; 16]));
        console(&mut core, &format!("/kline del {gone}"));
        assert!(core.router.config.banned_identities.is_empty());
        let saved = HubConfig::load(&core.router.config.config_path).unwrap();
        assert!(saved.banned_identities.is_empty());
        console(&mut core, "/kick lobby nobody");
        console(&mut core, &format!("/ban lobby add {gone}"));
        assert_eq!(
            replies(&mut rx),
            [
                (format!("kline added for {gone}"), false),
                (format!("kline removed for {gone}"), false),
                ("target 'nobody' not found".to_string(), false),
                ("ban added in lobby".to_string(), false),
            ]
        );
        // What rsRRCD answers only the room for is done all the same.
        console(&mut core, "/topic lobby Hello");
        assert_eq!(replies(&mut rx), [("Done: /topic lobby Hello".to_string(), false)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A command from the panel, what's for the console a reply (as the
    /// hub's task does).
    fn console(core: &mut Core, text: &str) -> Vec<Action> {
        let actions = core.run_command(text);
        for action in &actions {
            if let Action::Send(CONSOLE, payload) = action {
                core.reply(payload);
            }
        }
        actions
    }

    fn sent_kinds(actions: &[Action]) -> Vec<u64> {
        sent(actions).into_iter().map(|(_, kind, ..)| kind).collect()
    }

    #[test]
    fn only_you_make_rooms_unless_anyone_may() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-rooms-{}", std::process::id()));
        let (mut core, _rx) = core(&dir, false);
        connect(&mut core, 1, AMY, "amy");
        connect(&mut core, 2, YOU, "me");
        let refused = join(&mut core, 1, AMY, "lobby");
        assert_eq!(sent(&refused)[0].3.as_deref(), Some("no such room (only the hub's host makes rooms here)"));
        assert!(core.router.state.rooms.is_empty());
        // You may; then anyone may join it.
        join(&mut core, 2, YOU, "lobby");
        join(&mut core, 1, AMY, "lobby");
        assert_eq!(core.router.state.rooms["lobby"].members.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rooms_registered_from_the_panel_are_saved_and_said() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-register-{}", std::process::id()));
        let (mut core, _rx) = core(&dir, true);
        connect(&mut core, 1, AMY, "amy");
        join(&mut core, 1, AMY, "lobby");
        let (reply, actions) = core.register("#Lobby", true);
        assert!(matches!(reply, HubEvent::Reply { ref text, error: false } if text == "Registered lobby"), "{reply:?}");
        assert_eq!(sent(&actions), [(1, t::NOTICE, Some("lobby".into()), Some("room registered".into()))]);
        let saved = RoomRegistry::load(&core.router.config.room_registry_path).unwrap();
        assert!(saved["lobby"].registered);
        // A new one, nobody in it, stays.
        let _ = core.register("quiet", true);
        let snapshot = core.snapshot();
        let quiet = snapshot.rooms.iter().find(|r| r.name == "quiet").unwrap();
        assert!(quiet.registered && quiet.members.is_empty() && quiet.modes.contains('r'));
        assert_eq!(quiet.founder.as_deref(), Some(hex::encode(YOU).as_str()));
        // Unregistered and empty: gone.
        let _ = core.register("quiet", false);
        assert!(!core.router.state.rooms.contains_key("quiet"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_panel_sees_people_not_the_console() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-snapshot-{}", std::process::id()));
        let (mut core, mut rx) = core(&dir, true);
        connect(&mut core, 1, AMY, "amy");
        connect(&mut core, 2, YOU, "me");
        connect(&mut core, 3, YOU, "me");
        join(&mut core, 1, AMY, "lobby");
        core.router.established([4; 16]);
        core.report();
        let snapshot = std::iter::from_fn(|| rx.try_recv().ok())
            .find_map(|e| match e {
                NetEvent::Hub(HubEvent::State(snapshot)) => Some(snapshot),
                _ => None,
            })
            .unwrap();
        let people: Vec<(Option<&str>, usize, bool)> = snapshot.people.iter().map(|p| (p.nick.as_deref(), p.links, p.operator)).collect();
        assert_eq!(people, [(Some("amy"), 1, false), (Some("me"), 2, true)]);
        assert_eq!(snapshot.people[0].rooms, ["lobby"]);
        assert_eq!(snapshot.unidentified, 1);
        // Told again only when something changed.
        core.report();
        assert!(rx.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
