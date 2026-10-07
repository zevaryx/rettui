//! Web UI (`rettui --web`): the same client in a browser.
//!
//! The web UI runs the same [`App`] as the terminal UI, owned by one loop
//! that also receives network events, exactly like the TUI's event loop.
//! HTTP handlers never touch it directly: they send it jobs (closures run
//! on `&mut Owner`) and await the answer. Browsers are told when something
//! changed over a Server-Sent Events stream (and what it touched, see
//! [`Scope`]), and fetch what they show.
//! Notifications go out on the same stream; each browser shows them unless
//! it's showing what they're about. A browser reconnecting catches up on
//! those it missed, while they're unread; reading something closes its
//! notifications in every browser. Browsers that aren't showing the web UI
//! get them by Web Push, where they've turned it on.
//!
//! - [`routes`]: the HTTP API, login and static files.
//! - [`views`]: JSON snapshots of the app state.
//! - [`push`]: Web Push.

pub mod mbtiles;
mod push;
mod routes;
mod tiles;
pub mod tls;
mod views;

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};
use rns_identity::identity::Identity;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::app::notify::Notification;
use crate::app::{App, Location, Tab};
use crate::config::{Paths, Settings};
use crate::net::{self, NetCommand, NetEvent};
use crate::nomad::micron;
use crate::store::Store;

pub const DEFAULT_ADDRESS: &str = "127.0.0.1:8740";

/// Request ids for web fetches, kept clear of the TUI browser's.
const FIRST_WEB_REQUEST: u64 = 1 << 40;

/// A page, file or image fetched for the web UI.
pub struct Fetched {
    pub data: Vec<u8>,
    pub metadata: Option<Vec<u8>>,
    /// Set when it came from the cache.
    pub cached_age: Option<Duration>,
}

struct PendingFetch {
    location: Location,
    identified: bool,
    cacheable: bool,
    reply: oneshot::Sender<Result<Fetched, String>>,
}

/// Everything the web UI's jobs can use.
pub struct Owner {
    pub app: App,
    fetches: HashMap<u64, PendingFetch>,
    next_request: u64,
    /// The latest notifications and their ids, for browsers catching up.
    recent: VecDeque<(u64, Notification)>,
}

/// How many notifications are kept for browsers catching up.
const RECENT_NOTICES: usize = 100;

/// What goes to every open browser, besides changes.
#[derive(Debug, Clone)]
pub enum Notice {
    /// A notification, and its id: a browser that reconnects says the last
    /// one it had, and gets those it missed.
    Show(u64, Notification),
    /// What this tag is about was read: close notifications about it.
    Read(String),
}

impl Owner {
    /// Notifications after `since` whose conversation or room is still
    /// unread: what a browser missed while its connection was down (the
    /// latest one about each).
    pub fn missed(&self, since: u64) -> Vec<(u64, Notification)> {
        use crate::app::notify::Target;
        let unread = |target: &Target| match target {
            Target::Conversation { key } => self.app.store.conversations.get(key).is_some_and(|c| c.unread > 0),
            Target::Room { hub, room } => net::parse_hash(hub)
                .and_then(|hash| self.app.channels.hub_index(hash))
                .is_some_and(|index| self.app.channels.hubs[index].unread.get(room).is_some_and(|n| *n > 0)),
            // It sums up a sync: what it was about shows as unread anyway.
            Target::Summary => false,
        };
        let mut missed: Vec<(u64, Notification)> = Vec::new();
        for (id, notification) in self.recent.iter().filter(|(id, n)| *id > since && unread(&n.target)) {
            missed.retain(|(_, n)| n.target != notification.target);
            missed.push((*id, notification.clone()));
        }
        missed
    }

    /// Fetch from a NomadNet node, from the cache when fresh (unless
    /// `refresh`). Form submissions and `/file/` downloads always go to the
    /// network, as in the terminal browser.
    pub fn fetch(&mut self, location: Location, refresh: bool, reply: oneshot::Sender<Result<Fetched, String>>) {
        // This client's own node is read from its folder (see `own_node_content`).
        if self.app.is_own_node(location.node) {
            let result =
                self.app.own_node_content(&location.path).map(|content| Fetched { data: content.data, metadata: None, cached_age: None });
            let _ = reply.send(result);
            return;
        }
        let identified = self.app.identifies_to(location.node);
        let cacheable =
            location.fields.is_empty() && !location.path.starts_with(nomad_core::FILE_PREFIX) && !self.app.is_own_node(location.node);
        if cacheable
            && !refresh
            && let Some(cached) = self.app.page_cache().get(location.node, &location.path, identified)
        {
            let _ = reply.send(Ok(Fetched { data: cached.data, metadata: None, cached_age: Some(cached.age) }));
            return;
        }
        let id = self.next_request;
        self.next_request += 1;
        self.app.send(NetCommand::Fetch {
            id,
            node: location.node,
            path: location.path.clone(),
            fields: location.fields.clone(),
            identify: identified,
        });
        self.fetches.insert(id, PendingFetch { location, identified, cacheable, reply });
    }

    /// Take in a network event; what browsers showing it should refetch.
    /// The counters come every few seconds whether or not they changed:
    /// unchanged, nobody is told.
    fn apply(&mut self, event: NetEvent) -> Scope {
        let scope = Scope::of(&event);
        match event {
            // Loads a browser asked for: it gets the answer itself.
            NetEvent::Fetched { id, .. } if self.fetches.contains_key(&id) => {
                self.on_net(event);
                Scope::NONE
            }
            NetEvent::Interfaces(_) => {
                let before = (self.app.interfaces.clone(), self.app.traffic.rates);
                self.on_net(event);
                if (&self.app.interfaces, self.app.traffic.rates) == (&before.0, before.1) { Scope::NONE } else { scope }
            }
            NetEvent::Host(net::HostEvent::Stats(_)) => {
                let counts = |o: &Self| o.app.node.stats.as_ref().and_then(|s| serde_json::to_value(s).ok());
                let before = counts(self);
                self.on_net(event);
                if counts(self) == before { Scope::NONE } else { scope }
            }
            NetEvent::Pn(net::PnEvent::Stats(_)) => {
                let before = self.app.pn.stats.clone();
                self.on_net(event);
                if self.app.pn.stats == before { Scope::NONE } else { scope }
            }
            _ => {
                self.on_net(event);
                scope
            }
        }
    }

    fn on_net(&mut self, event: NetEvent) {
        match event {
            NetEvent::Fetched { id, result } if self.fetches.contains_key(&id) => {
                let pending = self.fetches.remove(&id).expect("checked above");
                let result = result.map(|content| {
                    if pending.cacheable {
                        let location = &pending.location;
                        let ttl = (!location.path.starts_with("/media/"))
                            .then(|| micron::cache_directive(&String::from_utf8_lossy(&content.data)))
                            .flatten()
                            .map(Duration::from_secs);
                        self.app.page_cache().put(location.node, &location.path, pending.identified, &content.data, ttl);
                    }
                    Fetched { data: content.data, metadata: content.metadata, cached_age: None }
                });
                let _ = pending.reply.send(result);
            }
            other => self.app.on_net(other),
        }
    }
}

type JobFn = Box<dyn FnOnce(&mut Owner) + Send>;

struct Job {
    run: JobFn,
    /// Whether browsers should refresh afterwards.
    changes: bool,
}

/// What a change touched, so browsers refetch only what shows it: the
/// interface counters every few seconds, or announces arriving several a
/// second, would otherwise refetch whatever is on screen (a long
/// conversation is hundreds of kilobytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope(u8);

impl Scope {
    /// The status (the sidebar, and Status): interface counters, the log.
    pub const STATUS: Scope = Scope(1);
    /// The hosted node's counters.
    pub const NODE: Scope = Scope(2);
    /// Peers heard (announces): the Network list, the Browser's nodes, and
    /// how many are known.
    pub const PEERS: Scope = Scope(4);
    /// RRC: the Channels section, and the unread counts in the sidebar.
    pub const CHANNELS: Scope = Scope(8);
    /// Nothing any browser shows.
    pub const NONE: Scope = Scope(0);
    /// Anything.
    pub const ALL: Scope = Scope(u8::MAX);

    fn of(event: &NetEvent) -> Self {
        match event {
            NetEvent::Interfaces(_) | NetEvent::Log(_) | NetEvent::Pn(net::PnEvent::Stats(_)) => Scope::STATUS,
            NetEvent::Host(net::HostEvent::Stats(_)) => Scope::NODE,
            NetEvent::Announce { .. } => Scope::PEERS,
            // Hub connections, rooms and whispers (their notifications go
            // out on their own).
            NetEvent::Rrc { .. } => Scope::CHANNELS,
            // Only the terminal UI's browser shows it.
            NetEvent::FetchProgress { .. } => Scope::NONE,
            _ => Scope::ALL,
        }
    }

    /// What two changes touched together.
    fn and(self, other: Self) -> Self {
        Scope(self.0 | other.0)
    }

    /// As browsers get it: `all`, or the parts, such as `status,peers`.
    pub fn name(self) -> String {
        if self == Scope::ALL {
            return "all".into();
        }
        let parts = [(Scope::STATUS, "status"), (Scope::NODE, "node"), (Scope::PEERS, "peers"), (Scope::CHANNELS, "channels")];
        parts.iter().filter(|(part, _)| self.0 & part.0 != 0).map(|(_, name)| *name).collect::<Vec<_>>().join(",")
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;
    use crate::app::notify::Target;
    use crate::store::{Conversation, Store};

    #[test]
    fn catching_up_brings_the_latest_still_unread() {
        let dir = std::env::temp_dir().join(format!("rettui-missed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = ("aa".repeat(16), "bb".repeat(16));
        let mut store = Store::default();
        store.conversations.insert(a.clone(), Conversation { unread: 2, ..Default::default() });
        store.conversations.insert(b.clone(), Conversation { unread: 0, ..Default::default() });
        let app = crate::app::test_app(&dir, Settings::default(), store);
        let note = |key: &str, body: &str| Notification {
            title: key[..4].to_string(),
            body: body.into(),
            target: Target::Conversation { key: key.to_string() },
        };
        let recent = VecDeque::from([
            (10, note(&a, "first")),
            (11, note(&b, "read since")),
            (12, note(&a, "second")),
            (13, Notification { title: "3 new messages".into(), body: String::new(), target: Target::Summary }),
        ]);
        let owner = Owner { app, fetches: HashMap::new(), next_request: 0, recent };
        let bodies = |since| owner.missed(since).into_iter().map(|(id, n)| (id, n.body)).collect::<Vec<_>>();
        // The latest about each conversation still unread, and nothing read.
        assert_eq!(bodies(0), [(12, "second".to_string())]);
        assert_eq!(bodies(12), []);
        assert_eq!(bodies(11), [(12, "second".to_string())]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod address_tests {
    use super::*;

    #[test]
    fn the_address_other_devices_reach() {
        let lan: IpAddr = "192.168.1.5".parse().unwrap();
        assert_eq!(reachable_ip(lan), Some(lan));
        // Listening on all of them: the one traffic goes out from, if any
        // (never loopback, which another device can't reach).
        if let Some(ip) = reachable_ip("0.0.0.0".parse().unwrap()) {
            assert!(!ip.is_loopback() && !ip.is_unspecified(), "{ip}");
        }
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn counters_the_log_and_announces_touch_only_their_part() {
        assert_eq!(Scope::of(&NetEvent::Interfaces(Vec::new())), Scope::STATUS);
        assert_eq!(Scope::of(&NetEvent::Log("Interface x went offline".into())), Scope::STATUS);
        let announce = NetEvent::Announce { kind: net::PeerKind::Lxmf, hash: [0; 16], name: None, hops: 1 };
        assert_eq!(Scope::of(&announce), Scope::PEERS);
        assert_eq!(Scope::of(&NetEvent::Announced), Scope::ALL);
        assert_eq!(Scope::of(&NetEvent::SyncStarted), Scope::ALL);
        // A burst touches what any of it did.
        assert_eq!(Scope::STATUS.and(Scope::STATUS).name(), "status");
        assert_eq!(Scope::STATUS.and(Scope::PEERS).name(), "status,peers");
        assert_eq!(Scope::PEERS.and(Scope::NODE).and(Scope::STATUS).name(), "status,node,peers");
        assert_eq!(Scope::STATUS.and(Scope::ALL).name(), "all");
        assert_eq!(Scope::ALL.and(Scope::NODE), Scope::ALL);
        assert_eq!(Scope::STATUS.and(Scope::CHANNELS).name(), "status,channels");
        assert_eq!(Scope::NONE.and(Scope::PEERS), Scope::PEERS);
    }
}

/// Shared by every HTTP handler.
#[derive(Clone)]
pub struct WebState {
    jobs: mpsc::UnboundedSender<Job>,
    /// Each change: a version, and what it touched.
    changes: broadcast::Sender<(u64, Scope)>,
    /// Notifications, and what was read, for every open browser.
    notices: broadcast::Sender<Notice>,
    push: push::WebPush,
    /// This client's identity, to open paper messages (see
    /// [`routes`]' `read_paper`).
    identity: std::sync::Arc<Identity>,
    /// The login secret (see [`routes`]); replaced to sign every browser
    /// out, which `signed_out` counts so live-update streams end.
    token: std::sync::Arc<std::sync::RwLock<String>>,
    signed_out: tokio::sync::watch::Sender<u64>,
    paths: std::sync::Arc<Paths>,
    /// How the web UI is reached, for the new link once the token changes.
    link_base: String,
    /// Served over HTTPS (`--https`), so the login cookie is `Secure`.
    https: bool,
    /// rettui's own certificate authority (DER), for devices to install.
    ca: Option<std::sync::Arc<Vec<u8>>>,
}

impl WebState {
    fn token(&self) -> String {
        self.token.read().unwrap().clone()
    }

    /// Sign every browser out: a new login secret (saved), live-update
    /// streams ended, and push notifications to browsers stopped. The new
    /// link is printed where rettui runs. The new token.
    fn new_token(&self) -> Result<String, String> {
        let token = hex::encode(rand::random::<[u8; 24]>());
        crate::config::write_private(&self.paths.web_token, token.as_bytes()).map_err(|e| format!("Couldn't save the new token: {e}"))?;
        *self.token.write().unwrap() = token.clone();
        self.signed_out.send_modify(|n| *n += 1);
        self.push.unsubscribe_all()?;
        println!("Every other browser was signed out. The new link: {}/?token={token}", self.link_base);
        Ok(token)
    }

    async fn run<T: Send + 'static>(&self, changes: bool, f: impl FnOnce(&mut Owner) -> T + Send + 'static) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        let run: JobFn = Box::new(move |owner| {
            let _ = tx.send(f(owner));
        });
        self.jobs.send(Job { run, changes }).map_err(|_| "rettui is shutting down".to_string())?;
        rx.await.map_err(|_| "rettui is shutting down".to_string())
    }

    /// Read app state (browsers are not told to refresh).
    pub async fn read<T: Send + 'static>(&self, f: impl FnOnce(&mut Owner) -> T + Send + 'static) -> Result<T, String> {
        self.run(false, f).await
    }

    /// Change app state; open browsers refresh.
    pub async fn write<T: Send + 'static>(&self, f: impl FnOnce(&mut Owner) -> T + Send + 'static) -> Result<T, String> {
        self.run(true, f).await
    }

    pub async fn fetch(&self, location: Location, refresh: bool) -> Result<Fetched, String> {
        let (tx, rx) = oneshot::channel();
        self.read(move |owner| owner.fetch(location, refresh, tx)).await?;
        rx.await.map_err(|_| "rettui is shutting down".to_string())?
    }
}

/// Ctrl-C, or SIGTERM (what `docker stop` and systemd send).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The login secret, created on first use and kept in the data directory
/// so links and browser sessions survive restarts.
fn load_token(paths: &Paths) -> Result<String> {
    if let Ok(token) = std::fs::read_to_string(&paths.web_token)
        && token.trim().len() >= 32
    {
        return Ok(token.trim().to_string());
    }
    let token = hex::encode(rand::random::<[u8; 24]>());
    crate::config::write_private(&paths.web_token, token.as_bytes())?;
    Ok(token)
}

/// The address other devices on the network reach this computer at: the
/// one the web UI listens on, or (listening on all of them) the one its
/// traffic goes out from. None inside a container, whose own address
/// other devices can't reach (they use the host's), or if there's none.
fn reachable_ip(listening: IpAddr) -> Option<IpAddr> {
    if !listening.is_unspecified() {
        return Some(listening);
    }
    if std::path::Path::new("/.dockerenv").exists() || std::path::Path::new("/run/.containerenv").exists() {
        return None;
    }
    // Connecting a UDP socket sends nothing: it only picks the route (to
    // documentation addresses, which no one uses).
    let outgoing = |bind: &str, to: &str| {
        let socket = std::net::UdpSocket::bind(bind).ok()?;
        socket.connect(to).ok()?;
        Some(socket.local_addr().ok()?.ip())
    };
    outgoing("0.0.0.0:0", "192.0.2.1:9")
        .or_else(|| outgoing("[::]:0", "[2001:db8::1]:9"))
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
}

pub async fn run(settings: Settings, paths: Paths, identity: Identity, address: &str, https: Option<tls::Https>) -> Result<()> {
    let address: SocketAddr = address.parse().with_context(|| format!("--web expects an address like {DEFAULT_ADDRESS}, not {address}"))?;
    let token = load_token(&paths)?;
    // Certificates first: a problem with them stops rettui before it starts.
    let served = match &https {
        Some(how) => Some(tls::prepare(how, &paths.web_tls, address.ip()).context("could not set up HTTPS")?),
        None => None,
    };
    let listener = tokio::net::TcpListener::bind(address).await.with_context(|| format!("could not listen on {address}"))?;

    let identity_hash = identity.hash;
    let store = Store::load(&paths.store).unwrap_or_else(|e| {
        tracing::warn!("could not load store, starting empty: {e}");
        Store::default()
    });
    let (mut net_tx, mut net_rx) = net::spawn(crate::net_options(&settings, &paths, identity.clone()));
    let mut app = App::new(settings, paths.clone(), store, net_tx.clone(), None, identity_hash);
    // No tab is "on screen" in the web UI: unread counts are cleared by
    // browsers when they show a conversation or room.
    app.tab = Tab::Status;
    let mut owner = Owner { app, fetches: HashMap::new(), next_request: FIRST_WEB_REQUEST, recent: VecDeque::new() };
    let push = push::WebPush::load(&paths).context("could not set up Web Push")?;
    // Ids go on from the last run's (milliseconds since 1970 at start), so a
    // browser catching up after a restart isn't confused.
    let mut next_notice = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);

    let scheme = if served.is_some() { "https" } else { "http" };
    let shown = if address.ip().is_unspecified() { format!("127.0.0.1:{}", address.port()) } else { address.to_string() };
    let (jobs_tx, mut jobs) = mpsc::unbounded_channel::<Job>();
    let (changes, _) = broadcast::channel(64);
    let (notices, _) = broadcast::channel(64);
    let state = WebState {
        jobs: jobs_tx,
        changes: changes.clone(),
        notices: notices.clone(),
        push: push.clone(),
        identity: std::sync::Arc::new(identity.clone()),
        token: std::sync::Arc::new(std::sync::RwLock::new(token.clone())),
        signed_out: tokio::sync::watch::channel(0).0,
        paths: std::sync::Arc::new(paths),
        link_base: format!("{scheme}://{shown}"),
        https: served.is_some(),
        ca: served.as_ref().and_then(|s| s.ca.clone()),
    };
    let router = routes::router(state);
    let server = match &served {
        Some(served) => {
            let listener = tls::TlsListener::new(listener, served.config.clone())?;
            tokio::spawn(async move { axum::serve(listener, router).await })
        }
        None => tokio::spawn(async move { axum::serve(listener, router).await }),
    };
    println!("rettui web UI: {scheme}://{shown}/?token={token}");
    println!("The link logs this browser in; keep it private. Ctrl-C stops rettui.");
    println!("Scripts can send the token in an \"Authorization: Bearer\" header instead.");
    if let Some(served) = &served
        && let Some(fingerprint) = &served.fingerprint
    {
        println!("HTTPS with rettui's own certificate, for {}.", served.names.join(", "));
        println!("Browsers warn about it until a device installs rettui's certificate authority:");
        println!("open {scheme}://{shown}/rettui-ca.crt on it (SHA-256 {fingerprint}).");
    }
    if !address.ip().is_loopback() {
        println!("Listening on {address}: anyone with the link can use this client.");
        // For a phone on the same network: the link at this computer's
        // address, and its QR code to scan rather than type the token.
        if let Some(ip) = reachable_ip(address.ip()) {
            let link = format!("{scheme}://{}/?token={token}", SocketAddr::new(ip, address.port()));
            println!("From another device on this network: {link}");
            crate::cli::print_qr(&link);
        }
    }

    let mut version = 0u64;
    let mut notify = |changes: &broadcast::Sender<(u64, Scope)>, scope: Scope| {
        version += 1;
        let _ = changes.send((version, scope));
    };
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut save = tokio::time::interval(Duration::from_secs(10));
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut decoded = owner.app.take_decoded();
    let result = loop {
        tokio::select! {
            Some(job) = jobs.recv() => {
                (job.run)(&mut owner);
                owner.app.tab = Tab::Status;
                if job.changes {
                    notify(&changes, Scope::ALL);
                }
            }
            Some(event) = net_rx.recv() => {
                let mut scope = owner.apply(event);
                // Apply a burst of events before telling browsers.
                net::drain_burst(&mut net_rx, |event| scope = scope.and(owner.apply(event)));
                if scope != Scope::NONE {
                    notify(&changes, scope);
                }
            }
            Some(image) = decoded.recv() => owner.app.on_decoded(image),
            _ = tick.tick() => owner.app.on_tick(),
            _ = save.tick() => owner.app.save_if_dirty(),
            () = &mut shutdown => break Ok(()),
        }
        for notification in owner.app.take_notifications() {
            next_notice += 1;
            push.notify(&notification);
            owner.recent.push_back((next_notice, notification.clone()));
            if owner.recent.len() > RECENT_NOTICES {
                owner.recent.pop_front();
            }
            let _ = notices.send(Notice::Show(next_notice, notification));
        }
        for tag in owner.app.take_reads() {
            let _ = notices.send(Notice::Read(tag));
        }
        // The browser that read a paper message in opens its conversation.
        owner.app.paper_read = None;
        for report in push.reports() {
            owner.app.log(report);
        }
        if server.is_finished() {
            break Err(anyhow::anyhow!("the web server stopped"));
        }
        if owner.app.take_rns_restart() {
            // Browsers see "starting" while the old stack stops; page loads
            // in flight belong to it.
            notify(&changes, Scope::ALL);
            for (_, pending) in owner.fetches.drain() {
                let _ = pending.reply.send(Err("Reticulum restarted while loading; load the page again".into()));
            }
            net::shutdown(&net_tx, &mut net_rx, net::Stop::Restart).await;
            (net_tx, net_rx) = net::spawn(crate::net_options(&owner.app.settings, &owner.app.paths, identity.clone()));
            owner.app.set_network(net_tx.clone());
            notify(&changes, Scope::ALL);
        }
    };
    owner.app.save_if_dirty();
    server.abort();
    net::shutdown(&net_tx, &mut net_rx, net::Stop::Quit).await;
    owner.app.finish_saves();
    result
}
