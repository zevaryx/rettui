//! Web UI (`rettui --web`): the same client in a browser.
//!
//! The web UI runs the same [`App`] as the terminal UI, owned by one loop
//! that also receives network events, exactly like the TUI's event loop.
//! HTTP handlers never touch it directly: they send it jobs (closures run
//! on `&mut Owner`) and await the answer. Browsers are told when something
//! changed over a Server-Sent Events stream, and fetch what they show.
//!
//! - [`routes`]: the HTTP API, login and static files.
//! - [`views`]: JSON snapshots of the app state.

mod routes;
mod views;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use rns_identity::identity::Identity;
use tokio::sync::{broadcast, mpsc, oneshot};

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
}

impl Owner {
    /// Fetch from a NomadNet node, from the cache when fresh (unless
    /// `refresh`). Form submissions and `/file/` downloads always go to the
    /// network, as in the terminal browser.
    pub fn fetch(&mut self, location: Location, refresh: bool, reply: oneshot::Sender<Result<Fetched, String>>) {
        // This client's own node is read from its folder (see `own_node_content`).
        if self.app.is_own_node(location.node) {
            let result = self.app.own_node_content(&location.path).map(|content| Fetched {
                data: content.data,
                metadata: None,
                cached_age: None,
            });
            let _ = reply.send(result);
            return;
        }
        let identified = self.app.identifies_to(location.node);
        let cacheable = location.fields.is_empty()
            && !location.path.starts_with(nomad_core::FILE_PREFIX)
            && !self.app.is_own_node(location.node);
        if cacheable
            && !refresh
            && let Some(cached) = self.app.page_cache().get(location.node, &location.path, identified)
        {
            let _ = reply.send(Ok(Fetched {
                data: cached.data,
                metadata: None,
                cached_age: Some(cached.age),
            }));
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
        self.fetches.insert(
            id,
            PendingFetch {
                location,
                identified,
                cacheable,
                reply,
            },
        );
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
                        self.app.page_cache().put(
                            location.node,
                            &location.path,
                            pending.identified,
                            &content.data,
                            ttl,
                        );
                    }
                    Fetched {
                        data: content.data,
                        metadata: content.metadata,
                        cached_age: None,
                    }
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

/// Shared by every HTTP handler.
#[derive(Clone)]
pub struct WebState {
    jobs: mpsc::UnboundedSender<Job>,
    changes: broadcast::Sender<u64>,
    token: String,
    paths: std::sync::Arc<Paths>,
}

impl WebState {
    async fn run<T: Send + 'static>(
        &self,
        changes: bool,
        f: impl FnOnce(&mut Owner) -> T + Send + 'static,
    ) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        let run: JobFn = Box::new(move |owner| {
            let _ = tx.send(f(owner));
        });
        self.jobs
            .send(Job { run, changes })
            .map_err(|_| "rettui is shutting down".to_string())?;
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
    crate::config::write_atomic(&paths.web_token, token.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&paths.web_token, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(token)
}

pub async fn run(settings: Settings, paths: Paths, identity: Identity, address: &str) -> Result<()> {
    let address: SocketAddr = address
        .parse()
        .with_context(|| format!("--web expects an address like {DEFAULT_ADDRESS}, not {address}"))?;
    let token = load_token(&paths)?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("could not listen on {address}"))?;

    let identity_hash = identity.hash;
    let store = Store::load(&paths.store).unwrap_or_else(|e| {
        tracing::warn!("could not load store, starting empty: {e}");
        Store::default()
    });
    let (net_tx, mut net_rx) = net::spawn(crate::net_options(&settings, &paths, identity));
    let mut app = App::new(settings, paths.clone(), store, net_tx.clone(), None, identity_hash);
    // No tab is "on screen" in the web UI: unread counts are cleared by
    // browsers when they show a conversation or room.
    app.tab = Tab::Status;
    let mut owner = Owner {
        app,
        fetches: HashMap::new(),
        next_request: FIRST_WEB_REQUEST,
    };

    let (jobs_tx, mut jobs) = mpsc::unbounded_channel::<Job>();
    let (changes, _) = broadcast::channel(64);
    let state = WebState {
        jobs: jobs_tx,
        changes: changes.clone(),
        token: token.clone(),
        paths: std::sync::Arc::new(paths),
    };
    let router = routes::router(state);
    let server = tokio::spawn(async move { axum::serve(listener, router).await });

    let shown = if address.ip().is_unspecified() {
        format!("127.0.0.1:{}", address.port())
    } else {
        address.to_string()
    };
    println!("rettui web UI: http://{shown}/?token={token}");
    println!("The link logs this browser in; keep it private. Ctrl-C stops rettui.");
    println!("Scripts can send the token in an \"Authorization: Bearer\" header instead.");
    if !address.ip().is_loopback() {
        println!("Listening on {address}: anyone with the link can use this client.");
    }

    let mut version = 0u64;
    let mut notify = |changes: &broadcast::Sender<u64>| {
        version += 1;
        let _ = changes.send(version);
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
                    notify(&changes);
                }
            }
            Some(event) = net_rx.recv() => {
                owner.on_net(event);
                // Apply any burst of events before telling browsers.
                while let Ok(event) = net_rx.try_recv() {
                    owner.on_net(event);
                }
                notify(&changes);
            }
            Some(image) = decoded.recv() => owner.app.on_decoded(image),
            _ = tick.tick() => owner.app.on_tick(),
            _ = save.tick() => owner.app.save_if_dirty(),
            () = &mut shutdown => break Ok(()),
        }
        if server.is_finished() {
            break Err(anyhow::anyhow!("the web server stopped"));
        }
    };
    owner.app.save_if_dirty();
    server.abort();
    net::shutdown(&net_tx, &mut net_rx).await;
    result
}
