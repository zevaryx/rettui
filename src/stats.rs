//! The opt-in public stats endpoint (`--stats-listen`): a read-only JSON
//! summary of this node, with totals only. It reads a snapshot the owner loop
//! (the web UI's, or `rettui node`'s) publishes about once a second, so it never touches the app and
//! a slow or hostile visitor can't hold the owner up.
//!
//! With a token (`RETTUI_STATS_TOKEN` or `--stats-token-file`), a request
//! that presents it in a header also gets every peer heard, so a website can
//! count peers exactly across nodes. Without the token the answer is the same
//! as ever.

use std::ffi::OsString;
use std::io::Read;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use axum::Router;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::app::App;
use crate::app::node::NodeStatus;
use crate::net::PeerKind;

/// The variable that holds the token (a flag value would show in `ps`).
pub const TOKEN_VAR: &str = "RETTUI_STATS_TOKEN";
/// The shortest and longest token accepted.
const TOKEN_MIN: usize = 16;
const TOKEN_MAX: usize = 256;
/// The most peers an authorised answer lists.
pub const PEERS_MAX: usize = 20_000;
/// The most peered nodes an authorised answer lists.
pub const PN_PEERS_MAX: usize = crate::lxmf::peering::ROWS_MAX;
/// The longest name listed, in characters.
const NAME_MAX: usize = 64;
/// The header a token can also come in (besides `Authorization: Bearer`).
const TOKEN_HEADER: &str = "x-stats-token";

/// What the endpoint answers with, as published last.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Published {
    /// The totals, for everyone. Empty before the first snapshot.
    pub public: String,
    /// The totals and the peer list, for a request with the token. Only made
    /// when a token is set.
    pub authorised: Option<String>,
}

pub type Snapshot = watch::Receiver<Arc<Published>>;

/// Where to serve the stats, and the token that unlocks the peer list.
#[derive(Debug, Clone)]
pub struct Listen<'a> {
    pub address: &'a str,
    pub token: Option<Token>,
}

/// The secret that unlocks the peer list. Only its SHA-256 is kept, so the
/// comparison takes the same time whatever is presented, and `Debug` shows nothing.
#[derive(Clone)]
pub struct Token {
    digest: [u8; 32],
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(..)")
    }
}

fn digest(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

/// Whether two digests are equal, looking at every byte either way.
fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut difference = 0u8;
    for (x, y) in a.iter().zip(b) {
        difference |= x ^ y;
    }
    std::hint::black_box(difference) == 0
}

impl Token {
    /// A token from `text`: 16 to 256 printable ASCII characters, no spaces.
    /// The error never repeats the text.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() < TOKEN_MIN {
            return Err(format!("the stats token must be at least {TOKEN_MIN} characters"));
        }
        if text.len() > TOKEN_MAX {
            return Err(format!("the stats token must be at most {TOKEN_MAX} characters"));
        }
        if !text.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("the stats token must be printable ASCII with no spaces".into());
        }
        Ok(Self { digest: digest(text) })
    }

    /// Whether `presented` is the token.
    pub fn matches(&self, presented: &str) -> bool {
        ct_eq(&self.digest, &digest(presented))
    }
}

/// The token to use: from the file (`--stats-token-file`, its contents
/// trimmed) or the `RETTUI_STATS_TOKEN` variable. None if neither is given;
/// an error if both are, or if the token is not acceptable.
pub fn load_token(file: Option<&Path>) -> Result<Option<Token>> {
    if let Some(path) = file.filter(|path| readable_by_others(path)) {
        eprintln!("warning: the stats token file {} can be read by other users; make it readable by you only (chmod 600)", path.display());
    }
    token_from(file, std::env::var_os(TOKEN_VAR))
}

/// Whether `path` can be read by its group or by everyone (Unix only).
fn readable_by_others(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o044 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

fn token_from(file: Option<&Path>, var: Option<OsString>) -> Result<Option<Token>> {
    // An empty variable counts as not set, as with the node's other variables.
    let var = var.filter(|v| !v.is_empty());
    let text = match (file, var) {
        (Some(_), Some(_)) => bail!("give the stats token as {TOKEN_VAR} or as --stats-token-file, not both"),
        (None, None) => return Ok(None),
        (None, Some(var)) => var.into_string().map_err(|_| anyhow!("{TOKEN_VAR} is not valid text"))?.trim().to_string(),
        (Some(path), None) => {
            let mut text = String::new();
            // A little more than the longest, so a file that is too big is refused without reading it all.
            let read = std::fs::File::open(path).and_then(|f| f.take(TOKEN_MAX as u64 + 1024).read_to_string(&mut text));
            read.with_context(|| format!("could not read the stats token file {}", path.display()))?;
            text.trim().to_string()
        }
    };
    let origin = if file.is_some() { "--stats-token-file".to_string() } else { TOKEN_VAR.to_string() };
    Token::parse(&text).map(Some).map_err(|reason| anyhow!("{origin}: {reason}"))
}

/// How often the disk is measured at most.
const DISK_EVERY: Duration = Duration::from_secs(60);
/// The most directory entries one measurement visits, and the longest it
/// takes, before it gives up (the size is then null).
const DISK_MAX_ENTRIES: usize = 500_000;
const DISK_MAX_TIME: Duration = Duration::from_secs(10);
/// The deepest folder walked into (each level holds a directory handle open
/// while the walk is below it), and the longest wait after failed measurements.
const DISK_MAX_DEPTH: usize = 64;
const DISK_BACKOFF_MAX: Duration = Duration::from_secs(3600);

/// What a measurement of the disk found. A field is `None` if it could not be found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Bytes of the files in the data directory (all identities, the message store and the propagation node's messages).
    pub data_bytes: Option<u64>,
    /// Size of the file system that holds the data directory, and what is free on it for an ordinary user.
    pub fs_total_bytes: Option<u64>,
    pub fs_free_bytes: Option<u64>,
}

#[derive(Debug, Default)]
struct DiskState {
    last_started: Option<Instant>,
    running: bool,
    runs: u64,
    /// Measurements in a row that found no size.
    failures: u32,
    usage: Option<Usage>,
}

/// The wait before the next measurement: `every`, doubled for each failure in a row, up to an hour.
fn wait_after(every: Duration, failures: u32) -> Duration {
    let longest = every.max(DISK_BACKOFF_MAX);
    every.saturating_mul(1u32.checked_shl(failures).unwrap_or(u32::MAX)).min(longest)
}

/// The disk measurements, made in the background at most once a minute, so
/// walking the data directory never holds up the owner loop or a request.
#[derive(Debug, Clone)]
pub struct Disk {
    state: Arc<Mutex<DiskState>>,
    every: Duration,
}

impl Default for Disk {
    fn default() -> Self {
        Self::new(DISK_EVERY)
    }
}

impl Disk {
    pub fn new(every: Duration) -> Self {
        Self { state: Arc::default(), every }
    }

    /// The last measurement of `dir` (none until the first one is done),
    /// starting a new one on a blocking thread if the last is older than the
    /// interval. Without a Tokio runtime nothing is measured.
    pub fn current(&self, dir: &Path) -> Option<Usage> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let due = state.last_started.is_none_or(|at| at.elapsed() >= wait_after(self.every, state.failures));
        if due
            && !state.running
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            state.running = true;
            state.last_started = Some(Instant::now());
            state.runs += 1;
            let (shared, dir) = (self.state.clone(), dir.to_path_buf());
            runtime.spawn_blocking(move || {
                let usage = measure(&dir);
                let mut state = shared.lock().unwrap_or_else(PoisonError::into_inner);
                state.failures = if usage.data_bytes.is_some() { 0 } else { state.failures.saturating_add(1) };
                state.usage = Some(usage);
                state.running = false;
            });
        }
        state.usage
    }
}

/// The size of the files under `dir` and of the file system they are on.
pub fn measure(dir: &Path) -> Usage {
    let (fs_total_bytes, fs_free_bytes) = match filesystem(dir) {
        Some((total, free)) => (Some(total), Some(free)),
        None => (None, None),
    };
    Usage { data_bytes: directory_size(dir, DISK_MAX_ENTRIES, DISK_MAX_TIME, DISK_MAX_DEPTH), fs_total_bytes, fs_free_bytes }
}

/// Bytes in the files under `dir`, not following links. None if `dir` can't
/// be read, or there are more than `max_entries` entries or it takes longer
/// than `max_time`, or folders are nested deeper than [`DISK_MAX_DEPTH`] (a
/// partial sum would mislead). Hard links count once for each name. A folder that can't be read
/// further down, or a file that goes away meanwhile, counts as empty.
fn directory_size(dir: &Path, max_entries: usize, max_time: Duration, max_depth: usize) -> Option<u64> {
    let started = Instant::now();
    let mut pending = vec![std::fs::read_dir(dir).ok()?];
    let (mut total, mut seen) = (0u64, 0usize);
    while let Some(entries) = pending.last_mut() {
        let Some(entry) = entries.next() else {
            pending.pop();
            continue;
        };
        seen += 1;
        if seen > max_entries || started.elapsed() > max_time {
            return None;
        }
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if pending.len() >= max_depth {
                return None;
            }
            if let Ok(inner) = std::fs::read_dir(entry.path()) {
                pending.push(inner);
            }
        } else if kind.is_file() {
            total = total.saturating_add(entry.metadata().map_or(0, |m| m.len()));
        }
    }
    Some(total)
}

/// The total and the free bytes of the file system that holds `dir`: the one
/// `dir` is on, not those mounted below it, and shared by every folder on it.
#[cfg(unix)]
fn filesystem(dir: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `path` is a NUL-terminated string that outlives the call, and `stat`
    // is a buffer of the size statvfs fills in; it is read only when the call succeeds.
    let stat = unsafe {
        if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return None;
        }
        stat.assume_init()
    };
    // The field types differ between systems.
    #[allow(clippy::unnecessary_cast)]
    let (unit, blocks, free) = (stat.f_frsize as u64, stat.f_blocks as u64, stat.f_bavail as u64);
    // Some systems leave the fragment size 0: the block size is then the unit.
    let unit = if unit == 0 { stat.f_bsize as u64 } else { unit };
    Some((blocks.saturating_mul(unit), free.saturating_mul(unit)))
}

/// Not measured on this system.
#[cfg(not(unix))]
fn filesystem(_dir: &Path) -> Option<(u64, u64)> {
    None
}

/// The `propagation` object: counts of what the propagation node holds and
/// has done, if it is running. No addresses of peers or senders.
fn propagation_object(app: &App) -> Option<Value> {
    if !app.settings.pn_enabled || app.pn.status != NodeStatus::Running {
        return None;
    }
    let stats = app.pn.stats.as_ref()?;
    let limit = crate::lxmf::pn::PnConfig::from_settings(&app.settings, &app.paths).map(|config| config.storage_bytes);
    Some(json!({
        "enabled": true,
        "stored_messages": stats.messages,
        "stored_bytes": stats.bytes,
        "storage_limit_bytes": limit,
        "peers_connected": stats.peers,
        "peers_total": stats.peered,
        "peer_limit": app.settings.pn_max_peers,
        "static_peers": app.settings.static_peers().len(),
        "autopeer": app.settings.pn_autopeer,
        "autopeer_depth": app.settings.pn_autopeer_depth,
        "peers_alive": stats.peering.alive,
        "peers_static": stats.peering.statics,
        "peers_auto": stats.peering.peered.saturating_sub(stats.peering.statics),
        "messages_received": stats.received,
        "messages_served": stats.served,
        "messages_offered_to_peers": stats.peering.offered,
        "messages_sent_to_peers": stats.sent_to_peers,
        "messages_received_from_peers": stats.received_from_peers,
        "unhandled_messages": stats.peering.unhandled,
    }))
}

/// The JSON for `app`, as of now. Totals only: no addresses, hashes, or names
/// of peers, interfaces or clients, and no row per interface or client. With
/// `with_peers` (a token is set), the authorised answer is made too. `disk`
/// supplies the disk object once it has a first measurement.
pub fn snapshot(app: &App, started: Instant, with_peers: bool, disk: &Disk) -> Arc<Published> {
    let generated_at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let online = app.interfaces.iter().filter(|i| i.online).count();
    let mut clients: Option<u64> = None;
    let (mut rx, mut tx) = (0u64, 0u64);
    for interface in &app.interfaces {
        rx = rx.saturating_add(interface.rx_bytes);
        tx = tx.saturating_add(interface.tx_bytes);
        if let Some(n) = interface.clients {
            clients = Some(clients.unwrap_or(0).saturating_add(n));
        }
    }
    let (mut lxmf, mut nomad, mut propagation) = (0u64, 0u64, 0u64);
    for peer in app.store.peers.values() {
        match peer.kind {
            PeerKind::Lxmf => lxmf += 1,
            PeerKind::Nomad => nomad += 1,
            PeerKind::Propagation => propagation += 1,
        }
    }
    let mut json = json!({
        "version": 1,
        "rettui_version": env!("CARGO_PKG_VERSION"),
        "generated_at": generated_at,
        "uptime_seconds": started.elapsed().as_secs(),
        "interfaces": {
            "total": app.interfaces.len(),
            "online": online,
            "clients": clients,
            "rx_bytes": rx,
            "tx_bytes": tx,
        },
        "peers_heard": { "total": lxmf + nomad + propagation, "lxmf": lxmf, "nomad": nomad, "propagation": propagation },
        "announces_last_minute": app.announce_log.last_minute(),
    });
    if let Some(map) = json.as_object_mut() {
        if let Some(pn) = propagation_object(app) {
            map.insert("propagation".into(), pn);
        }
        if let Some(usage) = disk.current(&app.paths.root) {
            map.insert(
                "disk".into(),
                json!({ "data_bytes": usage.data_bytes, "fs_total_bytes": usage.fs_total_bytes, "fs_free_bytes": usage.fs_free_bytes }),
            );
        }
    }
    let public = json.to_string();
    let authorised = with_peers.then(|| {
        let (peers, truncated) = peer_list(app);
        if let Some(map) = json.as_object_mut() {
            map.insert("authorised".into(), Value::Bool(true));
            map.insert("peers".into(), Value::Array(peers));
            if let Some((list, cut)) = propagation_peers(app) {
                map.insert("propagation_peers".into(), Value::Array(list));
                if cut {
                    map.insert("propagation_peers_truncated".into(), Value::Bool(true));
                }
            }
            if truncated {
                map.insert("peers_truncated".into(), Value::Bool(true));
            }
        }
        json.to_string()
    });
    Arc::new(Published { public, authorised })
}

/// The peered propagation nodes one by one, by address, up to
/// [`PN_PEERS_MAX`] (what the node keeps); and whether some were left out.
/// For the authorised answer only. None unless the node runs.
fn propagation_peers(app: &App) -> Option<(Vec<Value>, bool)> {
    if !app.settings.pn_enabled || app.pn.status != NodeStatus::Running {
        return None;
    }
    let stats = app.pn.stats.as_ref()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let ago = |at: Option<u64>| at.map(|at| now.saturating_sub(at));
    let rows = stats.peering.rows.iter().take(PN_PEERS_MAX);
    let list = rows
        .map(|row| {
            json!({
                "address": hex::encode(row.hash),
                "name": clean_name(row.name.as_deref()),
                "static": row.is_static,
                "alive": row.alive,
                "state": row.state,
                "depth": row.hops,
                "last_heard_seconds_ago": ago(row.last_heard),
                "last_sync_seconds_ago": ago(row.last_sync),
                "messages_offered": row.offered,
                "unhandled": row.unhandled,
                "peering_cost": row.peering_cost,
            })
        })
        .collect();
    Some((list, stats.peering.peered > stats.peering.rows.len().min(PN_PEERS_MAX)))
}

/// Every peer heard, by address, up to [`PEERS_MAX`]; and whether some were left out.
fn peer_list(app: &App) -> (Vec<Value>, bool) {
    let mut rows: Vec<(&str, &crate::store::Peer)> = app
        .store
        .peers
        .iter()
        .filter(|(address, _)| address.len() == 32 && address.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .map(|(address, peer)| (address.as_str(), peer))
        .collect();
    let truncated = rows.len() > PEERS_MAX;
    if truncated {
        rows.select_nth_unstable_by(PEERS_MAX, |a, b| a.0.cmp(b.0));
        rows.truncate(PEERS_MAX);
    }
    rows.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let peers = rows
        .into_iter()
        .map(|(address, peer)| {
            let kind = match peer.kind {
                PeerKind::Lxmf => "lxmf",
                PeerKind::Nomad => "nomad",
                PeerKind::Propagation => "propagation",
            };
            json!({ "address": address, "type": kind, "name": clean_name(peer.name.as_deref()) })
        })
        .collect();
    (peers, truncated)
}

/// Characters that draw nothing or change how text is drawn (general
/// categories Cf, Zl and Zp): bidi overrides and isolates, zero-width
/// characters, joiners, the line and paragraph separators. Control characters
/// (Cc) are `char::is_control`.
fn is_invisible(c: char) -> bool {
    matches!(u32::from(c),
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E
        | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// A name as listed: no control or invisible characters, at most [`NAME_MAX`] characters, and null if nothing is left.
fn clean_name(name: Option<&str>) -> Option<String> {
    let name: String = name?.chars().filter(|c| !c.is_control() && !is_invisible(*c)).take(NAME_MAX).collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The token in the request, in `Authorization: Bearer` or `X-Stats-Token`
/// (never the URL, which ends up in proxy logs), if it is the right one.
fn is_authorised(token: &Token, headers: &HeaderMap) -> bool {
    let bearer = headers.get_all(header::AUTHORIZATION).iter().filter_map(|v| v.to_str().ok()).filter_map(|v| {
        let (scheme, rest) = (v.get(..7)?, v.get(7..)?);
        scheme.get(..6)?.eq_ignore_ascii_case("bearer").then_some(())?;
        scheme.ends_with(' ').then_some(rest.trim())
    });
    let custom = headers.get_all(TOKEN_HEADER).iter().filter_map(|v| v.to_str().ok()).map(str::trim);
    bearer.chain(custom).any(|presented| token.matches(presented))
}

/// Answer `GET` and `HEAD` for `/` and `/stats.json` from the snapshot.
fn router(snapshot: Snapshot, token: Option<Token>) -> Router {
    Router::new().fallback(move |method: Method, uri: axum::http::Uri, headers: HeaderMap| {
        let published = snapshot.borrow().clone();
        let token_set = token.is_some();
        let authorised = token.as_ref().is_some_and(|token| is_authorised(token, &headers));
        async move { answer(&method, uri.path(), &published, authorised, token_set) }
    })
}

fn answer(method: &Method, path: &str, published: &Published, authorised: bool, token_set: bool) -> Response {
    if path != "/" && path != "/stats.json" {
        return (StatusCode::NOT_FOUND, "not found\n").into_response();
    }
    if published.public.is_empty() {
        // Listening, but the first snapshot isn't made yet (rettui is starting).
        let mut response = (StatusCode::SERVICE_UNAVAILABLE, "starting\n").into_response();
        response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("2"));
        return response;
    }
    if method != Method::GET && method != Method::HEAD {
        let mut response = (StatusCode::METHOD_NOT_ALLOWED, "only GET and HEAD\n").into_response();
        response.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    let full = if authorised { published.authorised.as_deref() } else { None };
    let body = full.unwrap_or(&published.public);
    let mut response = if method == Method::HEAD { Response::new(axum::body::Body::empty()) } else { body.to_string().into_response() };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if full.is_some() {
        // The same URL answers differently by header: no shared cache may keep this one.
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    } else {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if token_set {
        // Both answers: a cache that did keep the public one must still ask again when a token is sent.
        headers.insert(header::VARY, HeaderValue::from_static("Authorization, X-Stats-Token"));
    }
    if method == Method::HEAD {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    }
    response
}

/// What is printed when the stats page is up.
pub fn started_line(bound: SocketAddr, with_peers: bool) -> String {
    if with_peers {
        format!("Stats at http://{bound}/stats.json (totals for everyone; a request with the stats token also gets every peer heard)")
    } else {
        format!("Stats (totals only, no login) at http://{bound}/stats.json")
    }
}

/// Listen on `address`, and serve until the task is stopped. Fails now if
/// the address can't be used, so rettui doesn't start without what was asked.
/// With a `token`, a request that presents it also gets the peer list.
pub async fn serve(address: &str, token: Option<Token>, snapshot: Snapshot) -> Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let address: SocketAddr =
        address.parse().with_context(|| format!("--stats-listen expects an address like 127.0.0.1:8741, not {address}"))?;
    let listener = tokio::net::TcpListener::bind(address).await.with_context(|| format!("could not listen for stats on {address}"))?;
    let bound = listener.local_addr()?;
    let app = router(snapshot, token);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((bound, task))
}

/// A channel for the snapshot, starting as `initial`.
pub fn channel(initial: Arc<Published>) -> (watch::Sender<Arc<Published>>, Snapshot) {
    watch::channel(initial)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::lxmf::peering::{PeerRow, PeeringStats};
    use crate::net::InterfaceInfo;
    use crate::store::{Peer, Store};

    fn published(public: &str) -> Arc<Published> {
        Arc::new(Published { public: public.into(), authorised: None })
    }

    fn peer(kind: PeerKind) -> Peer {
        Peer { kind, name: Some("Secret Name".into()), hops: 1, last_seen: 0 }
    }

    fn app(name: &str) -> (App, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("rettui-stats-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::default();
        for (i, kind) in [PeerKind::Lxmf, PeerKind::Lxmf, PeerKind::Nomad, PeerKind::Propagation].into_iter().enumerate() {
            store.peers.insert(format!("{:032x}", i + 1), peer(kind));
        }
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), store);
        app.interfaces = vec![
            InterfaceInfo {
                name: "Public entry point".into(),
                online: true,
                rx_bytes: 10,
                tx_bytes: 20,
                clients: Some(3),
                ..Default::default()
            },
            InterfaceInfo { name: "Uplink".into(), online: false, rx_bytes: 1, tx_bytes: 2, clients: None, ..Default::default() },
            InterfaceInfo { name: "Second server".into(), online: true, clients: Some(4), ..Default::default() },
        ];
        (app, dir)
    }

    #[test]
    fn the_snapshot_has_the_counts_and_nothing_else() {
        let (app, dir) = app("snapshot");
        let json: Value = serde_json::from_str(&snapshot(&app, Instant::now(), false, &Disk::default()).public).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["rettui_version"], env!("CARGO_PKG_VERSION"));
        assert!(json["generated_at"].as_u64().unwrap() > 1_700_000_000);
        assert_eq!(json["uptime_seconds"], 0);
        assert_eq!(json["interfaces"]["total"], 3);
        assert_eq!(json["interfaces"]["online"], 2);
        assert_eq!(json["interfaces"]["clients"], 7);
        assert_eq!(json["interfaces"]["rx_bytes"], 11);
        assert_eq!(json["interfaces"]["tx_bytes"], 22);
        let interfaces = json["interfaces"].as_object().unwrap();
        let keys: Vec<_> = interfaces.keys().map(String::as_str).collect();
        assert_eq!(keys, ["clients", "online", "rx_bytes", "total", "tx_bytes"], "no per-interface rows");
        assert_eq!(json["peers_heard"], json!({ "total": 4, "lxmf": 2, "nomad": 1, "propagation": 1 }));
        assert_eq!(json["announces_last_minute"], 0);
        let text = json.to_string();
        assert!(!text.contains("Secret Name") && !text.contains("0000000000000000000000000000000"), "{text}");
        assert!(!text.contains("Public entry point") && !text.contains("Uplink") && !text.contains("Second server"), "{text}");
        let keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 7, "{keys:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_interface_reports_clients_as_null_and_no_list_is_published() {
        let (mut app, dir) = app("no-clients");
        app.interfaces = (0..40).map(|i| InterfaceInfo { name: format!("i{i}"), online: true, ..Default::default() }).collect();
        let json: Value = serde_json::from_str(&snapshot(&app, Instant::now(), false, &Disk::default()).public).unwrap();
        assert_eq!(json["interfaces"]["clients"], Value::Null);
        assert_eq!(json["interfaces"]["total"], 40);
        assert_eq!(json["interfaces"]["online"], 40);
        assert!(json["interfaces"].get("list").is_none(), "{json}");
        assert!(!json.to_string().contains("i39"), "{json}");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn running_pn(app: &mut App, stats: crate::lxmf::pn::PnStats) {
        app.settings.pn_enabled = true;
        app.pn.status = NodeStatus::Running;
        app.pn.stats = Some(stats);
    }

    #[test]
    fn the_propagation_object_is_there_only_while_the_node_runs() {
        let (mut app, dir) = app("pn");
        let json = |app: &App| serde_json::from_str::<Value>(&snapshot(app, Instant::now(), true, &Disk::default()).public).unwrap();
        assert!(json(&app).get("propagation").is_none(), "off");
        app.settings.pn_enabled = true;
        app.pn.status = NodeStatus::Starting;
        assert!(json(&app).get("propagation").is_none(), "starting");
        app.pn.status = NodeStatus::Running;
        assert!(json(&app).get("propagation").is_none(), "running, but no numbers yet");
        app.settings.pn_storage_mb = 50;
        app.settings.pn_max_peers = 7;
        app.settings.pn_autopeer = true;
        app.settings.pn_autopeer_depth = 4;
        let id = "00112233445566778899aabbccddeeff";
        app.settings.pn_static_peers = Some(format!("{id}, not a hash, {}", "ff".repeat(16)));
        running_pn(
            &mut app,
            crate::lxmf::pn::PnStats {
                messages: 3,
                bytes: 1234,
                received: 9,
                served: 4,
                peers: 2,
                peered: 5,
                sent_to_peers: 6,
                received_from_peers: 2,
                peering: PeeringStats { peered: 5, alive: 3, statics: 2, offered: 11, unhandled: 8, ..Default::default() },
                ..Default::default()
            },
        );
        let json = json(&app);
        assert_eq!(
            json["propagation"],
            json!({
                "enabled": true, "stored_messages": 3, "stored_bytes": 1234, "storage_limit_bytes": 50_000_000,
                "peers_connected": 2, "peers_total": 5, "peer_limit": 7, "static_peers": 2, "autopeer": true,
                "autopeer_depth": 4, "peers_alive": 3, "peers_static": 2, "peers_auto": 3,
                "messages_received": 9, "messages_served": 4, "messages_offered_to_peers": 11, "messages_sent_to_peers": 6,
                "messages_received_from_peers": 2, "unhandled_messages": 8
            })
        );
        assert!(!json.to_string().contains(id), "no peer is named");
        // The existing keys are as before.
        assert_eq!(json["version"], 1);
        assert_eq!(json["peers_heard"]["total"], 4);
        app.pn.status = NodeStatus::Failed("no".into());
        assert!(
            serde_json::from_str::<Value>(&snapshot(&app, Instant::now(), false, &Disk::default()).public)
                .unwrap()
                .get("propagation")
                .is_none()
        );
        app.pn.status = NodeStatus::Running;
        app.settings.pn_enabled = false;
        assert!(
            serde_json::from_str::<Value>(&snapshot(&app, Instant::now(), false, &Disk::default()).public)
                .unwrap()
                .get("propagation")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn row(n: u8, name: Option<&str>) -> PeerRow {
        PeerRow {
            hash: [n; 16],
            name: name.map(String::from),
            is_static: n == 1,
            alive: true,
            state: "idle",
            hops: Some(2),
            last_heard: Some(1_000),
            last_sync: None,
            offered: 5,
            unhandled: 3,
            peering_cost: 18,
        }
    }

    #[test]
    fn the_peered_nodes_are_listed_only_in_the_authorised_answer() {
        let (mut app, dir) = app("pn-peers");
        let rows = vec![row(1, Some("Node \u{202E}One\u{7}")), row(2, None)];
        running_pn(
            &mut app,
            crate::lxmf::pn::PnStats { peered: 2, peering: PeeringStats { peered: 2, rows, ..Default::default() }, ..Default::default() },
        );
        let published = snapshot(&app, Instant::now(), true, &Disk::default());
        let public: Value = serde_json::from_str(&published.public).unwrap();
        assert!(public.get("propagation_peers").is_none() && !published.public.contains(&hex::encode([1u8; 16])), "{public}");
        let full: Value = serde_json::from_str(published.authorised.as_deref().unwrap()).unwrap();
        let list = full["propagation_peers"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["address"], hex::encode([1u8; 16]));
        assert_eq!(list[0]["name"], "Node One");
        assert_eq!(list[0]["static"], true);
        assert_eq!(list[0]["state"], "idle");
        assert_eq!(list[0]["depth"], 2);
        assert!(list[0]["last_heard_seconds_ago"].as_u64().unwrap() > 1_000_000);
        assert_eq!(list[0]["last_sync_seconds_ago"], Value::Null);
        assert_eq!(list[1]["name"], Value::Null);
        assert_eq!(
            (list[0]["messages_offered"].as_u64(), list[0]["unhandled"].as_u64(), list[0]["peering_cost"].as_u64()),
            (Some(5), Some(3), Some(18))
        );
        assert!(full.get("propagation_peers_truncated").is_none());
        // More peered than listed: it says so.
        app.pn.stats.as_mut().unwrap().peering.peered = 300;
        let full: Value =
            serde_json::from_str(snapshot(&app, Instant::now(), true, &Disk::default()).authorised.as_deref().unwrap()).unwrap();
        assert_eq!(full["propagation_peers_truncated"], true);
        // No node running: no list, and no key.
        app.pn.status = NodeStatus::Off;
        let full: Value =
            serde_json::from_str(snapshot(&app, Instant::now(), true, &Disk::default()).authorised.as_deref().unwrap()).unwrap();
        assert!(full.get("propagation_peers").is_none() && full.get("propagation").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-stats-disk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_size_of_a_directory_adds_up_its_files() {
        let dir = scratch("size");
        let limits = (DISK_MAX_ENTRIES, DISK_MAX_TIME);
        assert_eq!(directory_size(&dir, limits.0, limits.1, DISK_MAX_DEPTH), Some(0));
        std::fs::write(dir.join("a"), [0u8; 100]).unwrap();
        std::fs::create_dir_all(dir.join("x/y")).unwrap();
        std::fs::write(dir.join("x/b"), [0u8; 1000]).unwrap();
        std::fs::write(dir.join("x/y/c"), [0u8; 20]).unwrap();
        assert_eq!(directory_size(&dir, limits.0, limits.1, DISK_MAX_DEPTH), Some(1120));
        // A link is not followed.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&dir, dir.join("x/loop")).unwrap();
            assert_eq!(directory_size(&dir, limits.0, limits.1, DISK_MAX_DEPTH), Some(1120));
        }
        // Too many entries, or too long: no number rather than a partial one.
        assert_eq!(directory_size(&dir, 2, limits.1, DISK_MAX_DEPTH), None);
        assert_eq!(directory_size(&dir, limits.0, Duration::ZERO, DISK_MAX_DEPTH), None);
        // A directory that is not there, or a file.
        assert_eq!(directory_size(&dir.join("missing"), limits.0, limits.1, DISK_MAX_DEPTH), None);
        assert_eq!(directory_size(&dir.join("a"), limits.0, limits.1, DISK_MAX_DEPTH), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn folders_nested_too_deep_are_not_walked() {
        let dir = scratch("deep");
        let mut path = dir.clone();
        for _ in 0..70 {
            path.push("d");
        }
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("f"), [0u8; 1]).unwrap();
        assert_eq!(directory_size(&dir, DISK_MAX_ENTRIES, DISK_MAX_TIME, DISK_MAX_DEPTH), None);
        assert_eq!(directory_size(&dir, DISK_MAX_ENTRIES, DISK_MAX_TIME, 100), Some(1));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_wait_doubles_with_each_failure_up_to_an_hour() {
        let every = Duration::from_secs(60);
        let waits: Vec<u64> = (0..8).map(|n| wait_after(every, n).as_secs()).collect();
        assert_eq!(waits, [60, 120, 240, 480, 960, 1920, 3600, 3600]);
        assert_eq!(wait_after(every, 500), DISK_BACKOFF_MAX, "no overflow");
        assert_eq!(wait_after(Duration::ZERO, 3), Duration::ZERO);
        assert_eq!(wait_after(Duration::from_secs(7200), 3), Duration::from_secs(7200), "never below the interval");
    }

    #[tokio::test]
    async fn failed_measurements_are_tried_less_often_and_a_success_resets_it() {
        let dir = scratch("backoff");
        let gone = dir.join("missing");
        let disk = Disk::new(Duration::from_secs(1));
        let failures = |disk: &Disk| disk.state.lock().unwrap().failures;
        measured(&disk, &gone).await;
        assert_eq!(failures(&disk), 1);
        // The wait is now 2 s: asking again soon doesn't measure again.
        for _ in 0..30 {
            disk.current(&dir);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(runs(&disk), 1);
        // Once it is over, and the folder is there, a measurement works and resets the count.
        disk.state.lock().unwrap().last_started = None;
        for _ in 0..500 {
            disk.current(&dir);
            if failures(&disk) == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(failures(&disk), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_measurement_has_the_size_and_the_file_system() {
        let dir = scratch("measure");
        std::fs::write(dir.join("a"), [0u8; 4321]).unwrap();
        let usage = measure(&dir);
        assert_eq!(usage.data_bytes, Some(4321));
        if cfg!(unix) {
            let (total, free) = (usage.fs_total_bytes.unwrap(), usage.fs_free_bytes.unwrap());
            assert!(total > 0 && free <= total, "{usage:?}");
        } else {
            assert_eq!((usage.fs_total_bytes, usage.fs_free_bytes), (None, None));
        }
        // A directory that is not there: everything is null.
        let gone = measure(&dir.join("missing"));
        assert_eq!(gone, Usage::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    async fn measured(disk: &Disk, dir: &Path) -> Usage {
        for _ in 0..500 {
            if let Some(usage) = disk.current(dir) {
                return usage;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no measurement");
    }

    fn runs(disk: &Disk) -> u64 {
        disk.state.lock().unwrap().runs
    }

    #[tokio::test]
    async fn the_disk_is_measured_in_the_background_and_at_most_once_per_interval() {
        let dir = scratch("throttle");
        std::fs::write(dir.join("a"), [0u8; 10]).unwrap();
        let disk = Disk::default();
        assert_eq!(disk.current(&dir), None, "the first call only starts the measurement");
        assert_eq!(measured(&disk, &dir).await.data_bytes, Some(10));
        std::fs::write(dir.join("b"), [0u8; 5]).unwrap();
        for _ in 0..20 {
            assert_eq!(disk.current(&dir).unwrap().data_bytes, Some(10), "the last one is kept for a minute");
        }
        assert_eq!(runs(&disk), 1);
        // With no wait between, it measures again, and the new size shows.
        let eager = Disk::new(Duration::ZERO);
        measured(&eager, &dir).await;
        for _ in 0..500 {
            if eager.current(&dir).and_then(|u| u.data_bytes) == Some(15) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(eager.current(&dir).unwrap().data_bytes, Some(15));
        assert!(runs(&eager) >= 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn without_a_runtime_nothing_is_measured() {
        let dir = scratch("no-runtime");
        let disk = Disk::default();
        assert_eq!(disk.current(&dir), None);
        assert_eq!(runs(&disk), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_disk_object_is_added_once_measured_and_is_null_on_failure() {
        let (app, dir) = app("disk");
        let disk = Disk::default();
        let json = |disk: &Disk| serde_json::from_str::<Value>(&snapshot(&app, Instant::now(), true, disk).public).unwrap();
        assert!(json(&disk).get("disk").is_none(), "before the first measurement");
        std::fs::write(dir.join("f"), [0u8; 77]).unwrap();
        let usage = measured(&disk, &app.paths.root).await;
        let value = json(&disk);
        assert_eq!(value["disk"]["data_bytes"], usage.data_bytes.unwrap());
        assert!(value["disk"]["data_bytes"].as_u64().unwrap() >= 77);
        let keys: Vec<_> = value["disk"].as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["data_bytes", "fs_free_bytes", "fs_total_bytes"]);
        assert_eq!(value["version"], 1);
        assert_eq!(value.as_object().unwrap().len(), 8);
        // On failure the fields are null.
        let failed = Disk::default();
        let gone = dir.join("missing");
        measured(&failed, &gone).await;
        let text = serde_json::to_value(failed.current(&gone).map(|u| json!({ "d": u.data_bytes, "t": u.fs_total_bytes }))).unwrap();
        assert_eq!(text, json!({ "d": null, "t": null }));
        // The authorised answer has it too.
        let both = snapshot(&app, Instant::now(), true, &disk);
        let authorised: Value = serde_json::from_str(both.authorised.as_deref().unwrap()).unwrap();
        assert!(authorised["disk"].is_object());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_new_keys_reach_a_visitor_over_the_router() {
        let (mut app, dir) = app("router-new");
        running_pn(&mut app, crate::lxmf::pn::PnStats { messages: 1, ..Default::default() });
        let disk = Disk::default();
        measured(&disk, &app.paths.root).await;
        let (_tx, rx) = channel(snapshot(&app, Instant::now(), false, &disk));
        let (_, json) = json_of(ask(&router(rx, None), "GET", "/stats.json").await).await;
        assert_eq!(json["propagation"]["stored_messages"], 1);
        assert!(json["disk"]["data_bytes"].is_u64());
        assert_eq!(json["interfaces"]["clients"], 7);
        let _ = std::fs::remove_dir_all(dir);
    }

    async fn ask(router: &Router, method: &str, path: &str) -> Response {
        router.clone().oneshot(Request::builder().method(method).uri(path).body(Body::empty()).unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn the_router_answers_get_and_head_and_refuses_the_rest() {
        let (app, dir) = app("router");
        let (_tx, rx) = channel(snapshot(&app, Instant::now(), false, &Disk::default()));
        let router = router(rx, None);
        for path in ["/stats.json", "/"] {
            let response = ask(&router, "GET", path).await;
            assert_eq!(response.status(), StatusCode::OK);
            let headers = response.headers();
            assert_eq!(headers[header::CONTENT_TYPE], "application/json");
            assert_eq!(headers[header::CACHE_CONTROL], "no-store");
            assert!(headers.get(header::VARY).is_none(), "a node with no token answers as before");
            assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
            let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
            let json: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["interfaces"]["clients"], 7);
        }
        let head = ask(&router, "HEAD", "/stats.json").await;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(axum::body::to_bytes(head.into_body(), 1 << 20).await.unwrap().is_empty());
        for method in ["POST", "PUT", "DELETE"] {
            let response = ask(&router, method, "/stats.json").await;
            assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{method}");
            assert_eq!(response.headers()[header::ALLOW], "GET, HEAD");
        }
        assert_eq!(ask(&router, "GET", "/admin").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(ask(&router, "POST", "/admin").await.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn before_the_first_snapshot_it_says_it_is_starting() {
        let (_tx, rx) = channel(Arc::default());
        assert_eq!(ask(&router(rx, None), "GET", "/stats.json").await.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_new_snapshot_is_what_the_next_visitor_gets() {
        let (tx, rx) = channel(published("{\"a\":1}"));
        let router = router(rx, None);
        tx.send_replace(published("{\"a\":2}"));
        let body = axum::body::to_bytes(ask(&router, "GET", "/").await.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"{\"a\":2}");
    }

    #[tokio::test]
    async fn serving_fails_on_a_bad_or_taken_address_and_answers_over_tcp() {
        let (_tx, rx) = channel(published("{}"));
        let error = serve("not an address", None, rx.clone()).await.unwrap_err();
        assert!(format!("{error:#}").contains("--stats-listen expects"), "{error:#}");
        let holder = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let taken = holder.local_addr().unwrap().to_string();
        let error = serve(&taken, None, rx.clone()).await.unwrap_err();
        assert!(format!("{error:#}").contains("could not listen for stats"), "{error:#}");
        let (bound, task) = serve("127.0.0.1:0", None, rx).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(bound).await.unwrap();
        stream.write_all(b"GET /stats.json HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).await.unwrap();
        assert!(text.starts_with("HTTP/1.1 200") && text.ends_with("{}"), "{text}");
        task.abort();
    }

    const SECRET: &str = "correct-horse-battery-staple";

    fn token() -> Token {
        Token::parse(SECRET).unwrap()
    }

    /// A router with a token, over the test app's snapshot (made with the peer list).
    fn locked(name: &str) -> (Router, std::path::PathBuf) {
        let (app, dir) = app(name);
        let (_tx, rx) = channel(snapshot(&app, Instant::now(), true, &Disk::default()));
        (router(rx, Some(token())), dir)
    }

    async fn ask_with(router: &Router, method: &str, path: &str, headers: &[(&str, &str)]) -> Response {
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        router.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
    }

    async fn json_of(response: Response) -> (axum::http::HeaderMap, Value) {
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), 1 << 24).await.unwrap();
        (headers, serde_json::from_slice(&body).unwrap())
    }

    #[test]
    fn a_token_must_be_16_to_256_printable_characters_without_spaces() {
        assert!(Token::parse(&"a".repeat(16)).is_ok());
        assert!(Token::parse(&"a".repeat(256)).is_ok());
        assert!(Token::parse("~!@#$%^&*()_+-=[]{}|;:,.<>?/").is_ok());
        for (bad, reason) in [
            ("", "at least 16"),
            ("short", "at least 16"),
            (&"a".repeat(15), "at least 16"),
            (&"a".repeat(257), "at most 256"),
            ("has a space in it, 16+", "no spaces"),
            ("tab\tseparated-token", "no spaces"),
            ("line\nbreak-in-the-token", "no spaces"),
            ("non-ascii-token-\u{e9}\u{e9}\u{e9}", "printable ASCII"),
            ("control\u{7}character-token", "printable ASCII"),
        ] {
            let error = Token::parse(bad).err().unwrap_or_else(|| panic!("{bad:?} was accepted"));
            assert!(error.contains(reason), "{bad:?}: {error}");
            assert!(!error.contains(bad) || bad.is_empty(), "the error repeats the token: {error}");
        }
    }

    #[test]
    fn the_token_comes_from_the_variable_or_the_file_but_not_both() {
        let dir = std::env::temp_dir().join(format!("rettui-stats-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("token");
        std::fs::write(&file, format!("  {SECRET}\n")).unwrap();
        // The file's contents are trimmed.
        let from_file = token_from(Some(&file), None).unwrap().unwrap();
        assert!(from_file.matches(SECRET) && !from_file.matches(&format!(" {SECRET}")));
        let from_var = token_from(None, Some(format!("{SECRET}\n").into())).unwrap().unwrap();
        assert!(from_var.matches(SECRET));
        // None given, or an empty variable, is no token.
        assert!(token_from(None, None).unwrap().is_none());
        assert!(token_from(None, Some(OsString::new())).unwrap().is_none());
        // Both is an error, and an empty variable doesn't count as given.
        let error = token_from(Some(&file), Some(SECRET.into())).unwrap_err().to_string();
        assert!(error.contains("not both") && !error.contains(SECRET), "{error}");
        assert!(token_from(Some(&file), Some(OsString::new())).unwrap().is_some());
        // A bad token names where it came from, and never shows it.
        let error = token_from(None, Some("too short".into())).unwrap_err().to_string();
        assert!(error.contains(TOKEN_VAR) && error.contains("at least 16") && !error.contains("too short"), "{error}");
        std::fs::write(&file, "also short\n").unwrap();
        let error = token_from(Some(&file), None).unwrap_err().to_string();
        assert!(error.contains("--stats-token-file") && error.contains("at least 16"), "{error}");
        std::fs::write(&file, "").unwrap();
        assert!(token_from(Some(&file), None).is_err(), "an empty file");
        std::fs::write(&file, "x".repeat(100_000)).unwrap();
        assert!(token_from(Some(&file), None).unwrap_err().to_string().contains("at most 256"));
        std::fs::write(&file, [0xff, 0xfe, 0xfd]).unwrap();
        assert!(token_from(Some(&file), None).is_err(), "not text");
        let error = token_from(Some(&dir.join("missing")), None).unwrap_err();
        assert!(format!("{error:#}").contains("could not read the stats token file"), "{error:#}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn digests_compare_equal_only_when_every_byte_is() {
        let a = digest("one");
        assert!(ct_eq(&a, &digest("one")));
        assert!(!ct_eq(&a, &digest("two")));
        // A difference in only the first or only the last byte is seen.
        for at in [0, 31] {
            let mut b = a;
            b[at] ^= 1;
            assert!(!ct_eq(&a, &b), "byte {at}");
        }
        let token = token();
        assert!(token.matches(SECRET));
        assert!(!token.matches("") && !token.matches(&SECRET[..SECRET.len() - 1]) && !token.matches(&format!("{SECRET}x")));
        assert_eq!(format!("{token:?}"), "Token(..)");
    }

    #[tokio::test]
    async fn the_token_in_a_header_adds_the_peers() {
        let (router, dir) = locked("authorised");
        let bearer = format!("Bearer {SECRET}");
        let lower = format!("bearer   {SECRET}");
        for headers in [
            vec![("authorization", bearer.as_str())],
            vec![("authorization", lower.as_str())],
            vec![("x-stats-token", SECRET)],
            vec![("X-Stats-Token", SECRET)],
            // The wrong one first doesn't hide the right one.
            vec![("authorization", "Bearer nope"), ("x-stats-token", SECRET)],
        ] {
            let response = ask_with(&router, "GET", "/stats.json", &headers).await;
            assert_eq!(response.status(), StatusCode::OK, "{headers:?}");
            let (head, json) = json_of(response).await;
            assert_eq!(head[header::CACHE_CONTROL], "private, no-store");
            assert_eq!(head[header::VARY], "Authorization, X-Stats-Token");
            assert_eq!(head[header::CONTENT_TYPE], "application/json");
            assert_eq!(json["authorised"], true);
            assert_eq!(json["interfaces"]["clients"], 7);
            assert_eq!(json["peers_heard"]["total"], 4);
            assert!(json.get("peers_truncated").is_none());
            assert_eq!(
                json["peers"],
                json!([
                    { "address": format!("{:032x}", 1), "type": "lxmf", "name": "Secret Name" },
                    { "address": format!("{:032x}", 2), "type": "lxmf", "name": "Secret Name" },
                    { "address": format!("{:032x}", 3), "type": "nomad", "name": "Secret Name" },
                    { "address": format!("{:032x}", 4), "type": "propagation", "name": "Secret Name" },
                ])
            );
        }
        // HEAD says how long the full answer is, and sends none of it.
        let head = ask_with(&router, "HEAD", "/", &[("x-stats-token", SECRET)]).await;
        assert_eq!(head.headers()[header::CACHE_CONTROL], "private, no-store");
        let length: usize = head.headers()[header::CONTENT_LENGTH].to_str().unwrap().parse().unwrap();
        assert!(axum::body::to_bytes(head.into_body(), 1 << 20).await.unwrap().is_empty());
        let full =
            axum::body::to_bytes(ask_with(&router, "GET", "/", &[("x-stats-token", SECRET)]).await.into_body(), 1 << 24).await.unwrap();
        assert_eq!(length, full.len());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What a visitor without the token gets: the same as a node with no token.
    async fn assert_public(router: &Router, path: &str, headers: &[(&str, &str)], vary: bool) {
        let response = ask_with(router, "GET", path, headers).await;
        assert_eq!(response.status(), StatusCode::OK, "no 401: a wrong token must look like none");
        let (head, json) = json_of(response).await;
        assert_eq!(head[header::CACHE_CONTROL], "no-store");
        assert_eq!(head.get(header::VARY).is_some(), vary, "Vary is sent when a token is set, on both answers");
        assert!(json.get("peers").is_none() && json.get("authorised").is_none() && json.get("peers_truncated").is_none(), "{json}");
        assert_eq!(json.as_object().unwrap().len(), 7);
        assert!(!json.to_string().contains("Secret Name"));
    }

    #[tokio::test]
    async fn a_missing_wrong_or_misplaced_token_gets_the_public_answer() {
        let (router, dir) = locked("public");
        assert_public(&router, "/stats.json", &[], true).await;
        assert_public(&router, "/stats.json", &[("authorization", "Bearer wrong-token-of-the-right-length")], true).await;
        assert_public(&router, "/stats.json", &[("x-stats-token", "wrong-token-of-the-right-length")], true).await;
        assert_public(&router, "/stats.json", &[("x-stats-token", "")], true).await;
        // Not the Bearer scheme, or the token cut short or with more on it.
        let basic = format!("Basic {SECRET}");
        let bare = SECRET.to_string();
        let more = format!("Bearer {SECRET}x");
        let tokens = format!("Bearer {SECRET} {SECRET}");
        for value in [&basic, &bare, &more, &tokens, &"Bearer".to_string(), &"B".to_string(), &"é".to_string()] {
            assert_public(&router, "/stats.json", &[("authorization", value)], true).await;
        }
        // In the URL it would end up in proxy logs, so it is not read from there.
        for path in [format!("/stats.json?token={SECRET}"), format!("/?token={SECRET}&x=1"), format!("/stats.json?x-stats-token={SECRET}")]
        {
            assert_public(&router, &path, &[], true).await;
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn without_a_token_set_the_headers_are_ignored() {
        let (app, dir) = app("no-token");
        // Even a snapshot that has the list (it shouldn't) is not served without a token.
        let (_tx, rx) = channel(snapshot(&app, Instant::now(), true, &Disk::default()));
        let router = router(rx, None);
        let bearer = format!("Bearer {SECRET}");
        for headers in [vec![("authorization", bearer.as_str())], vec![("x-stats-token", SECRET)], vec![("x-stats-token", "")]] {
            assert_public(&router, "/stats.json", &headers, false).await;
        }
        let (_tx, rx) = channel(snapshot(&app, Instant::now(), false, &Disk::default()));
        assert!(rx.borrow().authorised.is_none(), "no list is built without a token");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_token_does_not_change_the_other_answers() {
        let (locked_router, dir) = locked("others");
        let headers = [("x-stats-token", SECRET)];
        assert_eq!(ask_with(&locked_router, "GET", "/admin", &headers).await.status(), StatusCode::NOT_FOUND);
        let response = ask_with(&locked_router, "POST", "/stats.json", &headers).await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let (_tx, rx) = channel(Arc::default());
        let response = ask_with(&router(rx, Some(token())), "GET", "/", &headers).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_public_json_is_the_same_with_or_without_a_token() {
        let (app, dir) = app("same-public");
        let started = Instant::now();
        let (without, with) = (snapshot(&app, started, false, &Disk::default()), snapshot(&app, started, true, &Disk::default()));
        let strip = |text: &str| {
            let mut json: Value = serde_json::from_str(text).unwrap();
            json.as_object_mut().unwrap().remove("generated_at");
            json
        };
        assert_eq!(strip(&without.public), strip(&with.public));
        assert!(without.authorised.is_none() && with.authorised.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_peer_list_is_sorted_capped_and_cleaned() {
        let (mut app, dir) = app("peers");
        app.store.peers.clear();
        let named = |name: Option<&str>| Peer { name: name.map(String::from), ..peer(PeerKind::Nomad) };
        app.store.peers.insert(format!("{:032x}", 0xB), named(Some("  Upper\u{1b}[31m\ncase\t ")));
        app.store.peers.insert(format!("{:032x}", 0xA), named(None));
        app.store.peers.insert(format!("{:032x}", 0xC), named(Some("\u{7}\n  ")));
        app.store.peers.insert(format!("{:032x}", 0xD), named(Some(&"\u{e9}".repeat(100))));
        // Not a lower-case 32 digit address: left out, not trusted.
        app.store.peers.insert("short".into(), named(Some("bad")));
        app.store.peers.insert("z".repeat(32), named(Some("bad")));
        app.store.peers.insert(format!("{:032X}", 0xE), named(Some("bad")));
        let json: Value =
            serde_json::from_str(&snapshot(&app, Instant::now(), true, &Disk::default()).authorised.clone().unwrap()).unwrap();
        let peers = json["peers"].as_array().unwrap();
        let addresses: Vec<_> = peers.iter().map(|p| p["address"].as_str().unwrap().to_string()).collect();
        assert_eq!(addresses, (0xA..=0xD).map(|n| format!("{n:032x}")).collect::<Vec<_>>());
        assert_eq!(peers[0]["name"], Value::Null);
        assert_eq!(peers[1]["name"], "Upper[31mcase", "controls stripped, ends trimmed");
        assert_eq!(peers[2]["name"], Value::Null, "nothing left");
        assert_eq!(peers[3]["name"].as_str().unwrap().chars().count(), NAME_MAX);
        assert!(json.get("peers_truncated").is_none());
        // peers_heard still counts every peer.
        assert_eq!(json["peers_heard"]["total"], 7);

        // More than the cap: the lowest addresses, in order, and it says so.
        app.store.peers.clear();
        for n in 0..PEERS_MAX + 5 {
            app.store.peers.insert(format!("{:032x}", n * 3), peer(PeerKind::Lxmf));
        }
        let json: Value =
            serde_json::from_str(&snapshot(&app, Instant::now(), true, &Disk::default()).authorised.clone().unwrap()).unwrap();
        let peers = json["peers"].as_array().unwrap();
        assert_eq!(peers.len(), PEERS_MAX);
        assert_eq!(json["peers_truncated"], true);
        assert_eq!(peers[0]["address"], format!("{:032x}", 0));
        assert_eq!(peers[PEERS_MAX - 1]["address"], format!("{:032x}", (PEERS_MAX - 1) * 3));
        assert!(peers.windows(2).all(|w| w[0]["address"].as_str() < w[1]["address"].as_str()));
        assert_eq!(json["peers_heard"]["total"], PEERS_MAX + 5);
        // Exactly the cap is not truncated.
        app.store.peers.retain(|address, _| address.as_str() < format!("{:032x}", PEERS_MAX * 3).as_str());
        assert_eq!(app.store.peers.len(), PEERS_MAX);
        let json: Value =
            serde_json::from_str(&snapshot(&app, Instant::now(), true, &Disk::default()).authorised.clone().unwrap()).unwrap();
        assert!(json.get("peers_truncated").is_none() && json["peers"].as_array().unwrap().len() == PEERS_MAX);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn over_tcp_the_token_is_read_from_the_header() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (app, dir) = app("tcp");
        let (_tx, rx) = channel(snapshot(&app, Instant::now(), true, &Disk::default()));
        let (bound, task) = serve("127.0.0.1:0", Some(token()), rx).await.unwrap();
        for (extra, listed) in
            [("", false), ("X-Stats-Token: nope-nope-nope-nope\r\n", false), (&format!("X-Stats-Token: {SECRET}\r\n"), true)]
        {
            let mut stream = tokio::net::TcpStream::connect(bound).await.unwrap();
            stream.write_all(format!("GET /stats.json HTTP/1.1\r\nHost: x\r\n{extra}Connection: close\r\n\r\n").as_bytes()).await.unwrap();
            let mut text = String::new();
            stream.read_to_string(&mut text).await.unwrap();
            assert!(text.starts_with("HTTP/1.1 200"), "{text}");
            assert_eq!(text.contains("\"peers\":["), listed, "{text}");
        }
        task.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invisible_and_direction_characters_are_dropped_from_names() {
        // Right-to-left override, zero-width space, line separator, paragraph separator, isolates, BOM, soft hyphen, tag characters.
        let name = Some("a\u{202E}b\u{200B}c\u{2028}d\u{2029}e\u{2066}f\u{2069}g\u{FEFF}h\u{AD}i\u{E0041}j");
        assert_eq!(clean_name(name).as_deref(), Some("abcdefghij"));
        assert_eq!(clean_name(Some("\u{202E}\u{200B}\u{2028}")), None);
        // Ordinary text in other scripts, and emoji with a joiner's neighbours, is kept.
        assert_eq!(clean_name(Some("Zoë 日本 ✓")).as_deref(), Some("Zoë 日本 ✓"));
    }

    #[cfg(unix)]
    #[test]
    fn a_token_file_others_can_read_is_noticed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rettui-stats-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("token");
        std::fs::write(&file, SECRET).unwrap();
        for (mode, shared) in [(0o600, false), (0o400, false), (0o640, true), (0o604, true), (0o644, true)] {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(readable_by_others(&file), shared, "{mode:o}");
        }
        assert!(!readable_by_others(&dir.join("missing")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
