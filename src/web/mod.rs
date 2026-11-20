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

#[cfg(test)]
mod api_tests;
pub mod desktop;
pub mod mbtiles;
mod push;
mod routes;
mod tiles;
pub mod tls;
mod views;

pub use routes::{hello_matches, hello_nonce};

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
use crate::rrc::host::HubEvent;
use crate::store::Store;

pub const DEFAULT_ADDRESS: &str = "127.0.0.1:8740";

/// Request ids for web fetches, kept clear of the TUI browser's.
const FIRST_WEB_REQUEST: u64 = 1 << 40;

/// A page, file or image fetched for the web UI.
pub struct Fetched {
    /// What came, unless it is kept on disk (`file`): then empty.
    pub data: Vec<u8>,
    pub metadata: Option<Vec<u8>>,
    /// A file downloaded, kept on disk (see [`crate::nomad::FetchedContent::file`]).
    pub file: Option<crate::nomad::DownloadedFile>,
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
    /// Questions to rngit nodes a browser waits on the answer to.
    git: HashMap<u64, oneshot::Sender<Result<net::git::GitReply, String>>>,
    /// What the owner asked of the rngit node hosted here, a browser waits on.
    hosting: HashMap<u64, oneshot::Sender<Result<net::githost::HostReply, String>>>,
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
    /// A voice message to play as it comes.
    Play(crate::app::Autoplay),
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
            Target::Game { app, session, .. } => self.app.games.view(app, session).is_some_and(|g| g.yours),
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
        let identified = self.app.identifies_to(location.node);
        self.fetch_as(location, refresh, identified, reply);
    }

    /// [`Self::fetch`], with `identified` saying whether the request
    /// identifies this client to the node (it is what `fetch` asks the app).
    pub fn fetch_as(&mut self, location: Location, refresh: bool, identified: bool, reply: oneshot::Sender<Result<Fetched, String>>) {
        // This client's own node is read from its folder (see `own_node_content`).
        if self.app.is_own_node(location.node) {
            let id = self.next_request;
            self.next_request += 1;
            match self.app.own_node_content(id, &location, identified) {
                Some(result) => {
                    let _ = reply.send(result.map(|content| Fetched {
                        data: content.data,
                        metadata: None,
                        file: content.file,
                        cached_age: None,
                    }));
                }
                // A script page, running: its page comes as a fetch's would.
                None => _ = self.fetches.insert(id, PendingFetch { location, identified, cacheable: false, reply }),
            }
            return;
        }
        let cacheable = location.fields.is_empty()
            && !location.path.starts_with(crate::nomad::paths::FILE_PREFIX)
            && !self.app.is_own_node(location.node);
        if cacheable
            && !refresh
            && let Some(cached) = self.app.page_cache().get(location.node, &location.path, identified)
        {
            let _ = reply.send(Ok(Fetched { data: cached.data, metadata: None, file: None, cached_age: Some(cached.age) }));
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
            // Browsers ask for how a copy is going; they aren't told.
            NetEvent::GitProgress { .. } => {
                self.on_net(event);
                Scope::NONE
            }
            NetEvent::Git { id, result } if self.git.contains_key(&id) => {
                self.app.git_over(id);
                let _ = self.git.remove(&id).expect("checked above").send(result);
                Scope::NONE
            }
            // The browser gets the answer, and the app lists them again.
            NetEvent::GitHost(net::githost::GitHostEvent::Owned { id, ref result }) if self.hosting.contains_key(&id) => {
                let _ = self.hosting.remove(&id).expect("checked above").send(result.clone());
                self.on_net(event);
                scope
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
                    Fetched { data: content.data, metadata: content.metadata, file: content.file, cached_age: None }
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
    /// The RRC hub hosted here: the Hub section (and the counts in Status).
    pub const HUB: Scope = Scope(16);
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
            // Who's on the hub hosted here, and what its commands came to
            // (its starting and stopping touch more).
            NetEvent::Hub(HubEvent::State(_) | HubEvent::Reply { .. }) => Scope::HUB,
            // Only the terminal UI's browser shows it.
            NetEvent::FetchProgress { .. } | NetEvent::GitProgress { .. } => Scope::NONE,
            NetEvent::SyncProgress(_) => Scope::STATUS,
            // The announce viewer asks for those after the last it has
            // (they come several a second on a busy network); the Network
            // list hears the same announce as an Announce.
            // But an rngit node is listed in Repos.
            NetEvent::Heard(heard) if heard.kind == net::heard::Kind::Git => Scope::PEERS,
            NetEvent::Heard(_) => Scope::NONE,
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
        let parts =
            [(Scope::STATUS, "status"), (Scope::NODE, "node"), (Scope::PEERS, "peers"), (Scope::CHANNELS, "channels"), (Scope::HUB, "hub")];
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
        let note = |key: &str, body: &str| Notification::new(key[..4].to_string(), body, Target::Conversation { key: key.to_string() });
        let recent = VecDeque::from([
            (10, note(&a, "first")),
            (11, note(&b, "read since")),
            (12, note(&a, "second")),
            (13, Notification::new("3 new messages", "", Target::Summary)),
        ]);
        let owner = Owner { app, fetches: HashMap::new(), git: HashMap::new(), hosting: HashMap::new(), next_request: 0, recent };
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
mod listen_tests {
    use super::*;

    #[tokio::test]
    async fn a_port_another_program_holds_is_not_taken_from_it() {
        let holder = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let taken = holder.local_addr().unwrap();
        // Run on its own, it says so and doesn't listen elsewhere.
        let error = listen(taken, false).await.unwrap_err();
        assert!(format!("{error:#}").contains("could not listen"), "{error:#}");
        // The Android app, which writes the port it got to its port file
        // (only once it has it), takes another one.
        let elsewhere = listen(taken, true).await.unwrap().local_addr().unwrap();
        assert!(elsewhere.ip() == taken.ip() && elsewhere.port() != taken.port() && elsewhere.port() != 0, "{elsewhere}");
        // A free port is the one asked for; 0 is any, as it is.
        let free = {
            let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            probe.local_addr().unwrap()
        };
        assert_eq!(listen(free, true).await.unwrap().local_addr().unwrap(), free);
        assert_ne!(listen("127.0.0.1:0".parse().unwrap(), false).await.unwrap().local_addr().unwrap().port(), 0);
    }

    /// What an identity switch does: the web UI ends, and starts again on a copy of the same socket.
    #[tokio::test]
    async fn the_port_stays_bound_while_the_web_ui_starts_over() {
        let bound = bind_with("127.0.0.1:0", None, None).await.unwrap();
        let address = bound.local_addr().unwrap();
        // The first run ends: its copy closes.
        let first = tokio::net::TcpListener::from_std(bound.try_clone().unwrap()).unwrap();
        drop(first);
        // Another program can't take the port in between.
        let error = std::net::TcpListener::bind(address).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        // A page that reconnects meanwhile waits to be served, by the next run.
        let waiting = tokio::net::TcpStream::connect(address).await.unwrap();
        let second = tokio::net::TcpListener::from_std(bound.try_clone().unwrap()).unwrap();
        let (_, from) = second.accept().await.unwrap();
        assert_eq!(from, waiting.local_addr().unwrap());
        // An address that is wrong or held is still said so, once, as before.
        assert!(format!("{:#}", bind_with("nonsense", None, None).await.unwrap_err()).contains("--web expects an address"));
        assert!(format!("{:#}", bind_with(&address.to_string(), None, None).await.unwrap_err()).contains("could not listen"));
    }

    /// An app that sets a port file (the desktop app) is told which port it got, and keeps it between starts.
    #[tokio::test]
    async fn an_app_keeps_its_port_between_starts() {
        let dir = std::env::temp_dir().join(format!("rettui-keep-port-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("port");
        let kept = std::sync::Mutex::new(Vec::new());
        let holder = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let held = holder.local_addr().unwrap();
        // The port asked for is held: another one, said in the file.
        let first = bind_with(&held.to_string(), Some(&file), Some(&kept)).await.unwrap();
        let got = first.local_addr().unwrap();
        assert_ne!(got.port(), held.port());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), got.port().to_string());
        drop(first);
        std::fs::remove_file(&file).unwrap();
        // Started again asking for that port (after a restore): the same socket, so no other program had it meanwhile.
        let again = bind_with(&got.to_string(), Some(&file), Some(&kept)).await.unwrap();
        assert_eq!(again.local_addr().unwrap(), got);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), got.port().to_string());
        assert_eq!(std::net::TcpListener::bind(got).unwrap_err().kind(), std::io::ErrorKind::AddrInUse);
        // Another address kept meanwhile (another app in the process, as the tests are) does not let this one go.
        let other = bind_with("127.0.0.1:0", Some(&file), Some(&kept)).await.unwrap();
        assert_ne!(other.local_addr().unwrap(), got);
        let third = bind_with(&got.to_string(), Some(&file), Some(&kept)).await.unwrap();
        assert_eq!(third.local_addr().unwrap(), got);
        drop((other, third));
        assert_eq!(std::net::TcpListener::bind(got).unwrap_err().kind(), std::io::ErrorKind::AddrInUse);
        // Without keeping, the port is let go.
        drop((again, kept));
        assert!(std::net::TcpListener::bind(got).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Puts what `set_port_file` set back, however the test ends.
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            *PORT_FILE.lock().unwrap_or_else(|e| e.into_inner()) = None;
            KEPT.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    /// The only test that sets the process's port file: the desktop app's
    /// start is what prints no login link.
    #[test]
    fn setting_a_port_file_is_starting_by_an_app() {
        let _reset = Reset;
        assert!(!started_by_app());
        set_port_file(std::env::temp_dir().join("rettui-never-written"));
        assert!(started_by_app());
    }
}

#[cfg(unix)]
#[cfg(test)]
mod signal_tests {
    use super::*;

    /// A SIGTERM before the stop is first waited for (as when rettui is
    /// stopped right after it prints its link) still stops it cleanly,
    /// instead of killing it.
    #[tokio::test]
    async fn a_stop_right_after_starting_is_heard() {
        let shutdown = shutdown_signal();
        assert!(std::process::Command::new("kill").arg("-TERM").arg(std::process::id().to_string()).status().unwrap().success());
        tokio::time::timeout(std::time::Duration::from_secs(10), shutdown).await.expect("SIGTERM wasn't heard");
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
        let heard = net::heard::Heard { hash: [0; 16], kind: net::heard::Kind::Other, aspect: None, name: None, hops: 1, software: None };
        assert_eq!(Scope::of(&NetEvent::Heard(heard)), Scope::NONE);
        assert_eq!(Scope::of(&NetEvent::Announced), Scope::ALL);
        assert_eq!(Scope::of(&NetEvent::SyncStarted), Scope::ALL);
        // The hub hosted here: who's on it, apart from its starting.
        assert_eq!(Scope::of(&NetEvent::Hub(HubEvent::State(Default::default()))).name(), "hub");
        assert_eq!(Scope::of(&NetEvent::Hub(HubEvent::Stopped)), Scope::ALL);
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
    /// The port it listens on, which a client names when it asks for proof
    /// that this is rettui (see [`routes`]' `hello`).
    port: u16,
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
        if started_by_app() {
            println!("Every other browser was signed out.");
        } else {
            println!("Every other browser was signed out. The new link: {}/?token={token}", self.link_base);
        }
        Ok(token)
    }

    async fn run<T: Send + 'static>(&self, changes: bool, f: impl FnOnce(&mut Owner) -> T + Send + 'static) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        let run: JobFn = Box::new(move |owner| {
            let _ = tx.send(f(owner));
        });
        self.jobs.send(Job { run, changes }).map_err(|_| "rettui is shutting down".to_string())?;
        rx.await.map_err(|_| "That failed inside rettui (or rettui is shutting down)".to_string())
    }

    /// Read app state (browsers are not told to refresh).
    pub async fn read<T: Send + 'static>(&self, f: impl FnOnce(&mut Owner) -> T + Send + 'static) -> Result<T, String> {
        self.run(false, f).await
    }

    /// Change app state; open browsers refresh.
    pub async fn write<T: Send + 'static>(&self, f: impl FnOnce(&mut Owner) -> T + Send + 'static) -> Result<T, String> {
        self.run(true, f).await
    }

    /// Ask an rngit repository something (or change something there), and
    /// wait for the answer.
    pub async fn git(&self, repo: rngit::RepoUrl, request: net::git::GitRequest) -> Result<net::git::GitReply, String> {
        let (tx, rx) = oneshot::channel();
        self.read(move |owner| {
            let id = owner.app.git(&repo, request);
            owner.git.insert(id, tx);
        })
        .await?;
        rx.await.map_err(|_| "rettui is shutting down".to_string())?
    }

    /// Ask the rngit node hosted here to do something for its owner, and
    /// wait for what came of it.
    pub async fn host(&self, action: net::githost::HostAction) -> Result<net::githost::HostReply, String> {
        let (tx, rx) = oneshot::channel();
        self.write(move |owner| {
            let id = owner.app.host_act(action)?;
            owner.hosting.insert(id, tx);
            Ok::<_, String>(())
        })
        .await??;
        rx.await.map_err(|_| "rettui is shutting down".to_string())?
    }

    pub async fn fetch(&self, location: Location, refresh: bool) -> Result<Fetched, String> {
        let (tx, rx) = oneshot::channel();
        self.read(move |owner| owner.fetch(location, refresh, tx)).await?;
        rx.await.map_err(|_| "rettui is shutting down".to_string())?
    }

    /// Fetch from the network without identifying, whatever the user has set
    /// for that node (for what a page loaded by itself, from another node).
    pub async fn fetch_unidentified(&self, location: Location) -> Result<Fetched, String> {
        let (tx, rx) = oneshot::channel();
        self.read(move |owner| owner.fetch_as(location, true, false, tx)).await?;
        rx.await.map_err(|_| "rettui is shutting down".to_string())?
    }
}

/// Ctrl-C, SIGTERM (what `docker stop` and systemd send), or a stop asked
/// for from within ([`crate::request_stop`]). Not an `async fn`: the
/// signals are listened for from the call, not from the first poll.
pub(crate) fn shutdown_signal() -> impl std::future::Future<Output = ()> {
    let signal = os_signal();
    async move {
        tokio::select! {
            () = signal => {}
            () = crate::stop_requested() => {}
        }
    }
}

/// Ctrl-C or SIGTERM: listened for from now (not from when it's first
/// awaited), so a stop right after starting is still a clean one.
// Not an `async fn`: on Unix the signals are registered when it's called.
#[cfg_attr(not(unix), allow(clippy::manual_async_fn))]
fn os_signal() -> impl std::future::Future<Output = ()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let signals = signal(SignalKind::interrupt()).and_then(|interrupt| Ok((interrupt, signal(SignalKind::terminate())?)));
        async move {
            match signals {
                Ok((mut interrupt, mut terminate)) => {
                    tokio::select! {
                        _ = interrupt.recv() => {}
                        _ = terminate.recv() => {}
                    }
                }
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
    }
    #[cfg(not(unix))]
    async {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The login secret, created on first use and kept in the data directory
/// so links and browser sessions survive restarts.
fn load_token(paths: &Paths) -> Result<String> {
    if let Ok(token) = std::fs::read_to_string(&paths.web_token)
        && token.trim().len() >= 32
    {
        let token = token.trim();
        if token.len() > crate::stats::TOKEN_MAX {
            anyhow::bail!("the web login token in {} is longer than {} characters", paths.web_token.display(), crate::stats::TOKEN_MAX);
        }
        return Ok(token.to_string());
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

/// The environment variable the Android app sets to a file, which rettui
/// writes the port it listens on to once it is listening.
const PORT_FILE_ENV: &str = "RETTUI_PORT_FILE";

/// Whether an app started rettui (the Android app, or the desktop app, which
/// says so with [`set_port_file`]): it logs in with the token file, and shows
/// what rettui prints as its log, so no login link is printed.
fn started_by_app() -> bool {
    if PORT_FILE.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
        return true;
    }
    std::env::var_os(PORT_FILE_ENV).is_some()
}

/// Whether the web UI on `ip` is plain HTTP on an address other than
/// loopback: the link, the login cookie and everything the page shows then
/// cross the network, where anyone on it can read them. Loopback never
/// leaves this computer.
fn plain_http_beyond_loopback(ip: IpAddr, https: bool) -> bool {
    !https && !ip.to_canonical().is_loopback()
}

/// Said at start when it is: the user asked for it with `--insecure-http`,
/// so one short line, not a warning to learn to ignore.
fn plain_http_notice(address: SocketAddr) -> String {
    format!("Plain HTTP on {address}, as --insecure-http allows: the link and login are not encrypted.")
}

/// Why rettui won't serve the web UI over plain HTTP on this address.
const PLAIN_HTTP_REFUSED: &str = "the web UI would be served over plain HTTP on an address other devices can reach, so its login link \
     and cookie would cross the network unencrypted, where anyone on it can read them. Start it with --https (rettui's own \
     certificate; or --tls-cert and --tls-key), or with --insecure-http to accept that";

/// Whether rettui refuses to serve the web UI on `ip`: over plain HTTP
/// (`https` false) on an address other than loopback, unless
/// `insecure_http` (`--insecure-http`) says the user accepts it.
fn plain_http_refused(ip: IpAddr, https: bool, insecure_http: bool) -> Option<&'static str> {
    (!insecure_http && plain_http_beyond_loopback(ip, https)).then_some(PLAIN_HTTP_REFUSED)
}

/// Whether the user accepted serving beyond loopback: TLS is on, or
/// `--insecure-http` was given. Not set, [`listen`] refuses such an address,
/// so no way to start the web UI skips the check.
static PLAIN_HTTP_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set by whatever starts the web UI, from its arguments (see
/// [`check_plain_http`]). It is set on every start and never cleared in
/// between, so tests that start the web UI must not depend on its value:
/// they test [`listen_with`], which takes the decision as an argument.
pub fn accept_plain_http(accepted: bool) {
    PLAIN_HTTP_OK.store(accepted, std::sync::atomic::Ordering::SeqCst);
}

/// Stops rettui before it binds `address` (a `--web` value) when
/// [`plain_http_refused`]. An address that doesn't parse is left to
/// [`run`], which says so.
pub fn check_plain_http(address: &str, https: bool, insecure_http: bool) -> Result<()> {
    match address.parse::<SocketAddr>().ok().and_then(|a| plain_http_refused(a.ip(), https, insecure_http)) {
        Some(why) => anyhow::bail!("--web {address}: {why}"),
        None => Ok(()),
    }
}

/// The lines printed when the web UI starts, about logging in.
fn login_notice(scheme: &str, shown: &str, token: &str, by_app: bool) -> String {
    if by_app {
        return format!("rettui web UI: {scheme}://{shown}/\n");
    }
    format!("rettui web UI: {scheme}://{shown}/?token={token}\nThe link logs this browser in; keep it private. Ctrl-C stops rettui.\n")
}

#[cfg(test)]
mod login_notice_tests {
    use super::*;

    #[test]
    fn a_login_token_that_is_too_long_is_refused() {
        let dir = std::env::temp_dir().join(format!("rettui-load-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths::new(Some(dir.clone())).unwrap();
        let ok = "a".repeat(crate::stats::TOKEN_MAX);
        std::fs::write(&paths.web_token, &ok).unwrap();
        assert_eq!(load_token(&paths).unwrap(), ok);
        std::fs::write(&paths.web_token, "a".repeat(crate::stats::TOKEN_MAX + 1)).unwrap();
        let error = load_token(&paths).unwrap_err().to_string();
        assert!(error.contains("longer than 256 characters"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plain_http_beyond_loopback_is_refused_unless_accepted() {
        // (address, refused with no flag, refused with --https, refused with --insecure-http)
        let cases = [
            ("127.0.0.1", false, false, false),
            ("[::1]", false, false, false),
            ("[::ffff:127.0.0.1]", false, false, false),
            ("192.168.1.5", true, false, false),
            ("[fd00::1]", true, false, false),
            ("0.0.0.0", true, false, false),
            ("[::]", true, false, false),
        ];
        for (host, refused, tls, insecure) in cases {
            let ip = format!("{host}:8740").parse::<SocketAddr>().unwrap().ip();
            assert_eq!(plain_http_refused(ip, false, false).is_some(), refused, "{host}: no flag");
            assert_eq!(plain_http_refused(ip, true, false).is_some(), tls, "{host}: --https");
            assert_eq!(plain_http_refused(ip, false, true).is_some(), insecure, "{host}: --insecure-http");
            assert!(plain_http_refused(ip, true, true).is_none(), "{host}: both");
        }
        // What the user is told: why, and both ways forward.
        let error = check_plain_http("0.0.0.0:8740", false, false).unwrap_err().to_string();
        assert!(error.starts_with("--web 0.0.0.0:8740: "), "{error}");
        for needed in ["plain HTTP", "unencrypted", "login link", "--https", "--insecure-http"] {
            assert!(error.contains(needed), "{needed}: {error}");
        }
        assert!(check_plain_http("0.0.0.0:8740", false, true).is_ok() && check_plain_http("0.0.0.0:8740", true, false).is_ok());
        assert!(check_plain_http("127.0.0.1:8740", false, false).is_ok());
        // Not an address: left for `run`, which names the form.
        assert!(check_plain_http("nowhere", false, false).is_ok());
    }

    #[test]
    fn plain_http_beyond_loopback_gets_one_short_line() {
        let beyond = |ip: &str, https| plain_http_beyond_loopback(ip.parse().unwrap(), https);
        // Loopback never leaves this computer (an IPv4 address in IPv6 form too).
        for ip in ["127.0.0.1", "127.1.2.3", "::1", "::ffff:127.0.0.1"] {
            assert!(!beyond(ip, false) && !beyond(ip, true), "{ip}");
        }
        // A LAN address, a public one and the wildcards are reachable by others.
        for ip in ["192.168.1.5", "10.1.2.3", "203.0.113.7", "fd00::1", "0.0.0.0", "::"] {
            assert!(beyond(ip, false), "{ip}");
            assert!(!beyond(ip, true), "{ip} with TLS");
        }
        let line = plain_http_notice("192.168.1.5:8740".parse().unwrap());
        assert_eq!(line, "Plain HTTP on 192.168.1.5:8740, as --insecure-http allows: the link and login are not encrypted.");
        assert!(!line.contains('\n'));
    }

    /// Whatever starts the web UI goes through `listen`, which refuses
    /// unless it was told that plain HTTP is accepted (or TLS is on). No
    /// global is touched here, so other tests cannot disturb it.
    #[tokio::test]
    async fn binding_beyond_loopback_is_refused_until_accepted() {
        let wildcard: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let refused = format!("{:#}", listen_with(wildcard, false, false).await.unwrap_err());
        assert!(refused.contains("--insecure-http") && refused.contains("--https"), "{refused}");
        // Loopback is never refused.
        assert!(listen_with("127.0.0.1:0".parse().unwrap(), false, false).await.is_ok());
        assert!(listen_with(wildcard, false, true).await.is_ok());
    }

    #[test]
    fn the_app_is_not_given_a_link_with_the_token_to_log() {
        let link = login_notice("http", "127.0.0.1:8740", testing::TOKEN, false);
        assert!(link.contains(&format!("?token={}", testing::TOKEN)), "{link}");
        let log = login_notice("http", "127.0.0.1:8740", testing::TOKEN, true);
        assert!(log.contains("127.0.0.1:8740") && !log.contains(testing::TOKEN) && !log.contains("token="), "{log}");
    }
}

/// The same file, for a program that runs rettui in-process (the desktop
/// app) and would rather not change the environment of a running process.
static PORT_FILE: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// The socket an app's web UI listens on, kept while rettui is stopped and
/// started again in the same process (a restore): the port is never free in
/// between, for another program to take while the app's page still reconnects
/// to it with its login.
static KEPT: std::sync::Mutex<Vec<std::net::TcpListener>> = std::sync::Mutex::new(Vec::new());

/// Have the web UI write the port it listens on to `file`, once it is
/// listening (and take another port if the one asked for is held), as
/// `RETTUI_PORT_FILE` does for the Android app. Unlike that, the socket is
/// then kept for as long as the process runs, and a later [`bind`] of the same
/// address gets it again.
pub fn set_port_file(file: std::path::PathBuf) {
    *PORT_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
}

/// Where to write the port: [`set_port_file`]'s file, else the environment's.
fn port_file() -> Option<std::path::PathBuf> {
    let set = PORT_FILE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    set.or_else(|| std::env::var_os(PORT_FILE_ENV).map(std::path::PathBuf::from))
}

/// Listen on `address`; if its port is taken and `any_port`, on a free one.
async fn listen(address: SocketAddr, any_port: bool) -> Result<tokio::net::TcpListener> {
    listen_with(address, any_port, PLAIN_HTTP_OK.load(std::sync::atomic::Ordering::SeqCst)).await
}

/// [`listen`], with whether plain HTTP beyond loopback is `accepted` given.
/// The one place the web UI binds: refused here, before anything is bound,
/// unless it is accepted (see [`accept_plain_http`]).
async fn listen_with(address: SocketAddr, any_port: bool, accepted: bool) -> Result<tokio::net::TcpListener> {
    if let Some(why) = plain_http_refused(address.ip(), false, accepted) {
        anyhow::bail!("--web {address}: {why}");
    }
    match tokio::net::TcpListener::bind(address).await {
        Err(e) if any_port && e.kind() == std::io::ErrorKind::AddrInUse => {
            Ok(tokio::net::TcpListener::bind(SocketAddr::new(address.ip(), 0))
                .await
                .with_context(|| format!("could not listen on {address}"))?)
        }
        bound => Ok(bound.with_context(|| format!("could not listen on {address}"))?),
    }
}

/// Binds the web UI's address, once: [`run`] is given (a copy of) the socket,
/// so that it stays bound while the web UI starts over as another identity,
/// and no other program can take the port in between. If its port is taken
/// and a port file is set, any free port is taken, and said in the port file.
pub async fn bind(address: &str) -> Result<std::net::TcpListener> {
    let keep = PORT_FILE.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    bind_with(address, port_file().as_deref(), keep.then_some(&KEPT)).await
}

/// [`bind`] with what it takes from the process (the port file, and where to
/// keep the socket, if it is to be kept) given.
async fn bind_with(
    address: &str,
    port_file: Option<&std::path::Path>,
    keep: Option<&std::sync::Mutex<Vec<std::net::TcpListener>>>,
) -> Result<std::net::TcpListener> {
    let address: SocketAddr = address.parse().with_context(|| format!("--web expects an address like {DEFAULT_ADDRESS}, not {address}"))?;
    // The Android app asks for the port it had, but another app may be
    // holding it: then any free one, which it is told of (see below).
    // One for each address, and none let go for another: a socket that is
    // replaced frees its port for another program, while a page may still
    // reconnect to it.
    let kept = keep.and_then(|kept| {
        let kept = kept.lock().unwrap_or_else(|e| e.into_inner());
        kept.iter().find(|l| l.local_addr().is_ok_and(|a| a == address)).and_then(|l| l.try_clone().ok())
    });
    let listener = match kept {
        Some(kept) => kept,
        None => {
            let listener = listen(address, port_file.is_some()).await?.into_std().context("could not listen")?;
            if let Some(keep) = keep
                && let Ok(copy) = listener.try_clone()
            {
                keep.lock().unwrap_or_else(|e| e.into_inner()).push(copy);
            }
            listener
        }
    };
    // The port it has (not the one asked for, which may be 0), and only now,
    // that is bound, said to the app: a port file written before could be
    // read while another app held the port.
    if let Some(file) = port_file {
        let port = listener.local_addr()?.port();
        crate::config::write_atomic(file, port.to_string().as_bytes()).context("could not write the port file")?;
    }
    Ok(listener)
}

/// The web UI on `listener` (from [`bind`]), until it's stopped; or the name
/// of the identity it's switching to (it's started again as that one).
///
/// With `stats` (an address, and perhaps a token), a read-only JSON summary of counts is served
/// there too (see [`crate::stats`]).
pub async fn run(
    listener: std::net::TcpListener,
    settings: Settings,
    paths: Paths,
    identity: Identity,
    https: Option<tls::Https>,
    stats: Option<crate::stats::Listen<'_>>,
) -> Result<Option<String>> {
    let address = listener.local_addr()?;
    let token = load_token(&paths)?;
    // Certificates first: a problem with them stops rettui before it starts.
    let served = match &https {
        Some(how) => Some(tls::prepare(how, &paths.web_tls, address.ip()).context("could not set up HTTPS")?),
        None => None,
    };
    // A copy of the socket: this one closes when the web UI stops, the
    // caller's stays bound.
    // (Whether a copy of a socket shares its blocking mode is up to the system.)
    listener.set_nonblocking(true).context("could not listen")?;
    let listener = tokio::net::TcpListener::from_std(listener).context("could not listen")?;

    // Before anything starts, so a bad or taken address stops rettui.
    let started = std::time::Instant::now();
    let (stats_tx, stats_rx) = crate::stats::channel(Default::default());
    let disk = crate::stats::Disk::default();
    let with_peers = stats.as_ref().is_some_and(|listen| listen.token.is_some());
    let with_logs = with_peers && stats.as_ref().is_some_and(|listen| listen.logs);
    let stats_server = match stats {
        Some(listen) => {
            let (bound, task) = crate::stats::serve(
                listen.address,
                listen.token,
                stats_rx,
                with_logs.then(|| crate::stats::LogFiles { log: paths.log.clone(), web_token: Some(paths.web_token.clone()) }),
            )
            .await?;
            println!("{}", crate::stats::started_line(bound, with_peers, with_logs));
            Some(task)
        }
        None => None,
    };

    let identity_hash = identity.hash;
    let store = Store::load_with_db(&paths.store, &paths.db);
    store.sync_blocked();
    let (mut net_tx, mut net_rx) = net::spawn(crate::net_options(&settings, &paths, identity.clone(), store.blocked.clone()));
    let mut app = App::new(settings, paths.clone(), store, net_tx.clone(), None, identity_hash);
    // No tab is "on screen" in the web UI: unread counts are cleared by
    // browsers when they show a conversation or room.
    app.tab = Tab::Status;
    let mut owner = Owner {
        app,
        fetches: HashMap::new(),
        git: HashMap::new(),
        hosting: HashMap::new(),
        next_request: FIRST_WEB_REQUEST,
        recent: VecDeque::new(),
    };
    let push = push::WebPush::load(&paths).context("could not set up Web Push")?;
    // Ids go on from the last run's (milliseconds since 1970 at start), so a
    // browser catching up after a restart isn't confused.
    let mut next_notice = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);

    let shutdown = shutdown_signal();
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
        port: address.port(),
        https: served.is_some(),
        ca: served.as_ref().and_then(|s| s.ca.clone()),
    };
    desktop::register(&state.jobs);
    let router = routes::router(state);
    let server = match &served {
        Some(served) => {
            let listener = tls::TlsListener::new(listener, served.config.clone())?;
            tokio::spawn(async move { axum::serve(listener, router).await })
        }
        None => tokio::spawn(async move { axum::serve(listener, router).await }),
    };
    print!("{}", login_notice(scheme, &shown, &token, started_by_app()));
    println!("Scripts can send the token in an \"Authorization: Bearer\" header instead.");
    if let Some(served) = &served
        && let Some(fingerprint) = &served.fingerprint
    {
        println!("HTTPS with rettui's own certificate, for {}.", served.names.join(", "));
        println!("Browsers warn about it until a device installs rettui's certificate authority:");
        println!("open {scheme}://{shown}/rettui-ca.crt on it (SHA-256 {fingerprint}).");
    }
    if !address.ip().to_canonical().is_loopback() {
        println!("Listening on {address}: anyone with the link can use this client.");
        // Here only if accepted (see `listen`).
        if plain_http_beyond_loopback(address.ip(), served.is_some()) {
            println!("{}", plain_http_notice(address));
        }
        // For a phone on the same network: the link at this computer's
        // address, and its QR code to scan rather than type the token.
        if !started_by_app()
            && let Some(ip) = reachable_ip(address.ip())
        {
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
    let mut stats_tick = tokio::time::interval(Duration::from_secs(1));
    tokio::pin!(shutdown);
    let mut decoded = owner.app.take_decoded();
    let result = loop {
        tokio::select! {
            Some(job) = jobs.recv() => {
                // A job that panics fails its own request, not the whole web UI.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.run)(&mut owner))).is_err() {
                    tracing::error!("a web job panicked; the request failed");
                }
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
            _ = tick.tick() => {
                // A check for updates, or an install, that finished: Status
                // shows it.
                let before = owner.app.updates.shown();
                // A browser following changes is someone looking; with none
                // (a phone's screen off, say), peers are saved less often.
                owner.app.watched = changes.receiver_count() > 0;
                owner.app.on_tick();
                if owner.app.updates.shown() != before {
                    notify(&changes, Scope::STATUS);
                }
            }
            _ = save.tick() => owner.app.save_if_dirty(),
            _ = stats_tick.tick(), if stats_server.is_some() => {
                stats_tx.send_replace(crate::stats::snapshot(&owner.app, started, with_peers, &disk));
            }
            () = owner.app.save_timer() => owner.app.save_when_due(),
            () = &mut shutdown => break Ok(None),
        }
        if owner.app.take_cleanup_news() {
            notify(&changes, Scope::STATUS);
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
        for voice in owner.app.take_autoplays() {
            let _ = notices.send(Notice::Play(voice));
        }
        // The browser that read a paper message in opens its conversation.
        owner.app.paper_read = None;
        for report in push.reports() {
            owner.app.log(report);
        }
        if server.is_finished() {
            break Err(anyhow::anyhow!("the web server stopped"));
        }
        if stats_server.as_ref().is_some_and(|task| task.is_finished()) {
            break Err(anyhow::anyhow!("the stats server stopped"));
        }
        if let Some(name) = owner.app.take_switch() {
            // Browsers load again once it's back, as the other identity.
            notify(&changes, Scope::ALL);
            break Ok(Some(name));
        }
        if owner.app.take_quit() {
            break Ok(None);
        }
        if owner.app.take_rns_restart() {
            // Browsers see "starting" while the old stack stops; page loads
            // in flight belong to it.
            notify(&changes, Scope::ALL);
            for (_, pending) in owner.fetches.drain() {
                let _ = pending.reply.send(Err("Reticulum restarted while loading; load the page again".into()));
            }
            net::shutdown(&net_tx, &mut net_rx, net::Stop::Restart).await;
            (net_tx, net_rx) =
                net::spawn(crate::net_options(&owner.app.settings, &owner.app.paths, identity.clone(), owner.app.store.blocked.clone()));
            owner.app.set_network(net_tx.clone());
            notify(&changes, Scope::ALL);
        }
    };
    owner.app.save_if_dirty();
    server.abort();
    if let Some(task) = &stats_server {
        task.abort();
    }
    // Until it has ended, so this copy of the socket is closed and takes no
    // more connections for a web UI that has stopped.
    let _ = server.await;
    net::shutdown(&net_tx, &mut net_rx, net::Stop::Quit).await;
    owner.app.finish_saves();
    result
}

/// The web UI for tests: its router, on a test [`App`] whose jobs run on a
/// thread of their own (as `run`'s loop does), driven as a browser would.
/// What the app tells the network is kept (see [`testing::TestWeb::sent`]);
/// a page asked for is answered from [`testing::TestWeb::pages`], as a node
/// would, or not at all.
#[cfg(test)]
pub(crate) mod testing {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::nomad::FetchedContent;

    pub const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

    /// Pages by path (any node): their content, or why they can't be had.
    pub type Pages = Arc<Mutex<HashMap<String, Result<Vec<u8>, String>>>>;

    pub struct TestWeb {
        router: axum::Router,
        pub dir: PathBuf,
        sent: Arc<Mutex<Vec<NetCommand>>>,
        pub pages: Pages,
        /// The rngit node every repository is on (none: no answer, ever).
        pub git_node: GitNode,
        state: WebState,
    }

    pub type GitNode = Arc<Mutex<Option<Arc<crate::net::git::tests::Canned>>>>;

    /// An answer: its status, headers and body.
    pub struct Answer {
        pub status: StatusCode,
        pub headers: axum::http::HeaderMap,
        pub body: Vec<u8>,
    }

    impl Answer {
        pub fn json(&self) -> Value {
            serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
        }

        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }

        /// The error an API call answered with.
        pub fn error(&self) -> String {
            self.json()["error"].as_str().unwrap_or_default().to_string()
        }
    }

    impl TestWeb {
        /// A web UI on a new app in a fresh folder named for the test.
        pub fn start(name: &str) -> Self {
            Self::start_with(name, Settings::default(), Store::default())
        }

        /// Its own Reticulum config (never the user's), and map tiles only
        /// from an offline file, unless `settings` say otherwise.
        pub fn start_with(name: &str, mut settings: Settings, store: Store) -> Self {
            let dir = std::env::temp_dir().join(format!("rettui-web-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            if settings.rns_config.is_none() {
                settings.rns_config = Some(dir.join("rns").display().to_string());
            }
            if settings.map_tiles.as_deref() == Some(crate::config::OSM_TILES) {
                settings.map_tiles = None;
            }
            let (mut app, mut net) = crate::app::test_app_with_net(&dir, settings, store);
            app.tab = Tab::Status;
            let paths = app.paths.clone();
            let mut owner = Owner {
                app,
                fetches: HashMap::new(),
                git: HashMap::new(),
                hosting: HashMap::new(),
                next_request: FIRST_WEB_REQUEST,
                recent: VecDeque::new(),
            };
            let (jobs_tx, mut jobs) = mpsc::unbounded_channel::<Job>();
            let sent: Arc<Mutex<Vec<NetCommand>>> = Arc::default();
            let pages: Pages = Arc::default();
            let git_node: GitNode = Arc::default();
            let (kept, served, node, test_dir) = (sent.clone(), pages.clone(), git_node.clone(), dir.clone());
            std::thread::spawn(move || {
                while let Some(job) = jobs.blocking_recv() {
                    (job.run)(&mut owner);
                    owner.app.tab = Tab::Status;
                    for notification in owner.app.take_notifications() {
                        owner.recent.push_back((owner.recent.len() as u64 + 1, notification));
                    }
                    while let Ok(command) = net.try_recv() {
                        if let NetCommand::Fetch { id, path, .. } = &command
                            && let Some(page) = served.lock().unwrap().get(path).cloned()
                        {
                            // Files under `/file/disk/` come as the network task hands a
                            // downloaded file over: on disk, in the spool folder.
                            let result = page.map(|data| match path.strip_prefix("/file/disk/") {
                                Some(name) => {
                                    let spool = test_dir.join("downloading");
                                    std::fs::create_dir_all(&spool).unwrap();
                                    let spooled = spool.join(format!("{id}-{name}"));
                                    std::fs::write(&spooled, &data).unwrap();
                                    let file = crate::nomad::DownloadedFile::new(spooled, data.len() as u64);
                                    FetchedContent { data: Vec::new(), metadata: None, file: Some(file) }
                                }
                                None => FetchedContent { data, metadata: None, file: None },
                            });
                            owner.apply(NetEvent::Fetched { id: *id, result });
                        }
                        // Script pages of its own node run, as in the network task.
                        if let NetCommand::RunScript { id, script, fields, limits, .. } = &command {
                            let data = crate::nomad::script::preview(script, None, fields, limits);
                            owner.apply(NetEvent::Fetched { id: *id, result: Ok(FetchedContent { data, metadata: None, file: None }) });
                        }
                        if let NetCommand::Git { id, repo, request, .. } = &command
                            && let Some(node) = node.lock().unwrap().clone()
                        {
                            let result = crate::net::git::tests::answer(&node, repo, request.clone());
                            owner.apply(NetEvent::Git { id: *id, result });
                        }
                        // The pages of the rngit node hosted here, as the network task makes them.
                        if let NetCommand::GitPage { id, path, fields, identify } = &command {
                            let node = rngit::server::Node::new(owner.app.paths.git_host.clone());
                            let site = crate::net::gitpages::Site { name: None, address: owner.app.git_host.address.unwrap_or_default() };
                            let peer = identify.then_some(owner.app.identity_hash);
                            let data = match crate::net::gitpages::answer(&node, &site, path, fields, peer).0 {
                                crate::net::gitpages::Reply::Page(page) => page.into_bytes(),
                                crate::net::gitpages::Reply::File { data, .. } => data,
                            };
                            owner.apply(NetEvent::Fetched { id: *id, result: Ok(FetchedContent { data, metadata: None, file: None }) });
                        }
                        // The rngit node hosted here, which mirrors from `source/` in the test's folder.
                        if let NetCommand::GitHostAction { id, action } = &command {
                            let (root, source) = (owner.app.paths.git_host.clone(), test_dir.join("source"));
                            let result = crate::net::githost::owner_tests::run(&root, &source, action.clone());
                            owner.apply(NetEvent::GitHost(crate::net::githost::GitHostEvent::Owned { id: *id, result }));
                        }
                        kept.lock().unwrap().push(command);
                    }
                }
            });
            let state = WebState {
                jobs: jobs_tx,
                changes: broadcast::channel(64).0,
                notices: broadcast::channel(64).0,
                push: push::WebPush::load(&paths).unwrap(),
                identity: Arc::new(Identity::new()),
                token: Arc::new(std::sync::RwLock::new(TOKEN.to_string())),
                signed_out: tokio::sync::watch::channel(0).0,
                paths: Arc::new(paths),
                link_base: "http://127.0.0.1:8740".into(),
                port: 8740,
                https: false,
                ca: None,
            };
            TestWeb { router: routes::router(state.clone()), dir, sent, pages, git_node, state }
        }

        /// Where jobs for the owner go, for what only the desktop app does.
        pub(super) fn jobs(&self) -> mpsc::UnboundedSender<Job> {
            self.state.jobs.clone()
        }

        /// Run something on the app, as a job (and wait for it).
        pub async fn with<T: Send + 'static>(&self, f: impl FnOnce(&mut App) -> T + Send + 'static) -> T {
            self.state.write(move |owner| f(&mut owner.app)).await.unwrap()
        }

        /// Run something on the owner (the app and what the web UI keeps).
        pub async fn with_owner<T: Send + 'static>(&self, f: impl FnOnce(&mut Owner) -> T + Send + 'static) -> T {
            self.state.write(f).await.unwrap()
        }

        /// Serve a page (on any node) at `path`.
        pub fn serve(&self, path: &str, content: &str) {
            self.pages.lock().unwrap().insert(path.to_string(), Ok(content.as_bytes().to_vec()));
        }

        async fn answer(&self, request: Request<Body>) -> Answer {
            let response = self.router.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let headers = response.headers().clone();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec();
            // A job answers before the thread records what it sent; one more
            // job, run after, waits for that.
            self.state.write(|_| ()).await.unwrap();
            Answer { status, headers, body }
        }

        /// A request, logged in unless `anonymous`, with these headers.
        pub async fn send(&self, method: Method, path: &str, body: Option<Value>, anonymous: bool) -> Answer {
            self.send_with(method, path, body, anonymous, &[]).await
        }

        pub async fn send_with(
            &self,
            method: Method,
            path: &str,
            body: Option<Value>,
            anonymous: bool,
            headers: &[(&str, &str)],
        ) -> Answer {
            let mut request = Request::builder().method(method).uri(path);
            if !anonymous {
                request = request.header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
            }
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            let request = match body {
                Some(json) => request.header(header::CONTENT_TYPE, "application/json").body(Body::from(json.to_string())),
                None => request.body(Body::empty()),
            }
            .unwrap();
            self.answer(request).await
        }

        pub async fn get(&self, path: &str) -> Answer {
            self.send(Method::GET, path, None, false).await
        }

        pub async fn post(&self, path: &str, body: Value) -> Answer {
            self.send(Method::POST, path, Some(body), false).await
        }

        pub async fn delete(&self, path: &str) -> Answer {
            self.send(Method::DELETE, path, None, false).await
        }

        /// A form posted without logging in, as the login page's form does.
        pub async fn post_form(&self, path: &str, body: &str) -> Answer {
            self.post_form_with(path, body, &[]).await
        }

        /// The same, with more request headers.
        pub async fn post_form_with(&self, path: &str, body: &str, headers: &[(&str, &str)]) -> Answer {
            let mut request =
                Request::builder().method(Method::POST).uri(path).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            self.answer(request.body(Body::from(body.to_string())).unwrap()).await
        }

        /// Raw bytes posted (an upload).
        pub async fn post_bytes(&self, path: &str, content_type: &str, bytes: Vec<u8>) -> Answer {
            let request = Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(bytes))
                .unwrap();
            self.answer(request).await
        }

        /// The live-update stream's events as they come (`last` the
        /// browser's last event id), each as its text.
        pub async fn events(&self, path: &str, last: Option<&str>) -> impl futures_util::Stream<Item = String> + use<> {
            use futures_util::StreamExt;
            let mut request = Request::builder().uri(path).header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
            if let Some(last) = last {
                request = request.header("last-event-id", last);
            }
            let response = self.router.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
            response.into_body().into_data_stream().filter_map(|chunk| async move {
                let text = String::from_utf8_lossy(&chunk.ok()?).into_owned();
                // Keep-alive comments aren't events.
                (!text.starts_with(':')).then_some(text)
            })
        }

        /// What the web UI shares between requests.
        pub fn state(&self) -> &WebState {
            &self.state
        }

        /// What the app has told the network since last asked.
        pub fn sent(&self) -> Vec<NetCommand> {
            std::mem::take(&mut *self.sent.lock().unwrap())
        }
    }

    impl Drop for TestWeb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}
