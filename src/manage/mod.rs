//! Remote management of `rettui node` (`--manage-listen` with
//! `--manage-secret-file`), for a website its operator runs: a listener of
//! its own, off unless both flags are given, where every request is signed
//! with a secret the node and the website share (see [`wire`]). Each request
//! becomes a job that the node's owner loop runs with the app (see
//! [`Owner`]), so the listener itself never touches the app.
//!
//! It can read the node's state, change the settings in
//! [`fields::MANAGE_FIELDS`] (names, limits and intervals; services can be
//! turned off but not on), restart Reticulum, roll a settings change back,
//! and read its own audit log. It can't run programs, read or change keys,
//! or change the Reticulum config. Requests are checked in this order:
//! method and path, size, the signing headers' form, the signature (and,
//! for a request without a valid one, the lockout after a burst of them),
//! the time, the nonce, the rate limits, and room in the job queue. Every
//! answer after the signature is checked is signed too.
//!
//! A nonce is remembered for as long as its request's time is accepted, so
//! a request can't be replayed within a run; one signed before the run
//! started is refused, as its nonce may have been used in the last run.
//! Within the 5 minutes a request's time is accepted, a request held back
//! on the way can still be delivered once, which is why the link must be
//! private.

mod audit;
pub mod fields;
mod owner;
pub(crate) mod wire;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use zeroize::Zeroizing;

pub use owner::Owner;
pub use wire::Secret;

/// The largest body taken, the most changes in one request, and the
/// longest value.
pub const MAX_BODY: usize = 16 * 1024;
pub const MAX_CHANGES: usize = 32;
pub const MAX_VALUE: usize = 4096;
/// How far a request's time may be from the node's, in seconds.
pub const MAX_SKEW: u64 = 300;
/// How much longer than [`MAX_SKEW`] after its request's time a nonce is
/// remembered, in seconds, for a clock that steps while it is checked.
const NONCE_MARGIN: u64 = 5;

/// Where to listen, and the secret requests are signed with.
#[derive(Debug, Clone)]
pub struct Listen {
    pub address: String,
    pub secret: Secret,
}

/// What a request asks the owner loop to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    State,
    Settings {
        revision: String,
        changes: Vec<(String, String)>,
    },
    Restart,
    Rollback {
        id: String,
    },
    Audit {
        before: Option<u64>,
        limit: usize,
    },
    /// Not a request: the listener saw a burst of bad signatures (for the audit log).
    AuthFailures {
        count: usize,
    },
}

/// A request for the owner loop, from the website's user `actor`.
#[derive(Debug)]
pub struct Job {
    pub actor: String,
    pub op: Op,
    pub reply: oneshot::Sender<Reply>,
}

/// The owner loop's answer to a job.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: StatusCode,
    pub body: Value,
}

impl Reply {
    pub fn new(status: StatusCode, body: Value) -> Self {
        Self { status, body }
    }

    /// The error envelope: `{"error": code, "message": plain English}`.
    pub fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self::new(status, json!({ "error": code, "message": message.into() }))
    }

    /// The answer with `key` added to its body.
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        if let Some(map) = self.body.as_object_mut() {
            map.insert(key.into(), value.into());
        }
        self
    }
}

/// Unix seconds now.
pub(crate) fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The secret in `path` (`--manage-secret-file`), its contents trimmed. An
/// error never repeats what the file holds. Warns if others can read it.
pub fn load_secret(path: &Path) -> Result<Secret> {
    if crate::stats::readable_by_others(path) {
        eprintln!(
            "warning: the manage secret file {} can be read by other users; make it readable by you only (chmod 600)",
            path.display()
        );
    }
    let mut text = Zeroizing::new(String::new());
    // A little more than the longest, so a file that is too big is refused without reading it all.
    let read = std::fs::File::open(path).and_then(|f| f.take(wire::SECRET_MAX as u64 + 1024).read_to_string(&mut text));
    read.with_context(|| format!("could not read the manage secret file {}", path.display()))?;
    Secret::parse(text.trim()).map_err(|reason| anyhow!("--manage-secret-file: {reason}"))
}

/// Whether the listener may use `address`: loopback and private addresses
/// (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7, fe80::/10) are fine,
/// every address (`0.0.0.0`, `::`) is allowed with a warning (Docker needs
/// it), and any other address is refused. The warning, if any.
pub fn check_bind(address: &SocketAddr) -> Result<Option<String>> {
    let ip = peer_ip(address.ip());
    if ip.is_unspecified() {
        return Ok(Some(format!(
            "--manage-listen {address} is reachable from every network this machine is on; firewall it so only the website can connect"
        )));
    }
    let private = match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || (v4.octets()[0] == 100 && v4.octets()[1] & 0xc0 == 64),
        IpAddr::V6(v6) => v6.is_loopback() || v6.segments()[0] & 0xfe00 == 0xfc00 || v6.segments()[0] & 0xffc0 == 0xfe80,
    };
    if !private {
        bail!("--manage-listen refuses a public address ({ip}); put it on a private network, firewalled so only the website can connect");
    }
    Ok(None)
}

/// The listener's limits (smaller in tests).
#[derive(Debug, Clone, Copy)]
struct Limits {
    /// The most nonces remembered (each for as long as its request's time is accepted).
    nonce_max: usize,
    /// Writes taken in a minute.
    writes_per_minute: usize,
    /// Bad signatures in a minute before requests without a valid one are
    /// refused, and for how long.
    failures: usize,
    lockout: Duration,
    /// Jobs waiting for the owner loop, and how long one may take.
    queue: usize,
    answer_within: Duration,
    /// Connections open at once, from one address at once, and how long
    /// one may stay open (one request each: every answer closes it).
    connections: usize,
    connections_per_peer: usize,
    connection_lifetime: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            nonce_max: 10_000,
            writes_per_minute: 10,
            failures: 20,
            lockout: Duration::from_secs(60),
            queue: 8,
            answer_within: Duration::from_secs(10),
            connections: 32,
            connections_per_peer: 4,
            connection_lifetime: Duration::from_secs(15),
        }
    }
}

const MINUTE: Duration = Duration::from_secs(60);

/// What the listener remembers between requests.
#[derive(Debug, Default)]
struct Guard {
    /// Each nonce seen, with the last unix second it is remembered for.
    nonces: HashMap<String, u64>,
    /// The same nonces, soonest forgotten first.
    nonce_expiry: BTreeSet<(u64, String)>,
    writes: VecDeque<Instant>,
    failures: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

enum Nonce {
    Fresh,
    Seen,
    Full,
}

/// Whole seconds until `wait` is over, at least 1.
fn seconds(wait: Duration) -> u64 {
    wait.as_secs() + u64::from(wait.subsec_nanos() > 0 || wait.is_zero())
}

fn forget_older(times: &mut VecDeque<Instant>, now: Instant, age: Duration) {
    while times.front().is_some_and(|at| now.saturating_duration_since(*at) >= age) {
        times.pop_front();
    }
}

impl Guard {
    /// How long requests without a valid signature are still refused for,
    /// after a burst of them.
    fn locked(&mut self, now: Instant) -> Option<Duration> {
        match self.locked_until {
            Some(until) if now < until => Some(until - now),
            Some(_) => {
                self.locked_until = None;
                None
            }
            None => None,
        }
    }

    /// Count a bad signature: whether it starts a burst (the only one
    /// logged), and whether it starts (or extends) the lockout.
    fn fail(&mut self, now: Instant, limits: &Limits) -> (bool, bool) {
        forget_older(&mut self.failures, now, MINUTE);
        let first = self.failures.is_empty();
        self.failures.push_back(now);
        if self.failures.len() >= limits.failures {
            self.failures.clear();
            self.locked_until = Some(now + limits.lockout);
            return (first, true);
        }
        (first, false)
    }

    /// Check `nonce`, from a request signed at `timestamp`, at unix second
    /// `now`. A nonce is kept until its request's time is no longer
    /// accepted (by the same clock that checks it), so it can't be used again
    /// while the request could be.
    fn nonce(&mut self, nonce: &str, timestamp: u64, now: u64, limits: &Limits) -> Nonce {
        while self.nonce_expiry.first().is_some_and(|(until, _)| now > *until) {
            if let Some((_, old)) = self.nonce_expiry.pop_first() {
                self.nonces.remove(&old);
            }
        }
        if self.nonces.contains_key(nonce) {
            return Nonce::Seen;
        }
        // Full: refuse rather than forget one, which would let it be replayed.
        if self.nonces.len() >= limits.nonce_max {
            return Nonce::Full;
        }
        let until = timestamp.saturating_add(MAX_SKEW + NONCE_MARGIN);
        self.nonces.insert(nonce.to_string(), until);
        self.nonce_expiry.insert((until, nonce.to_string()));
        Nonce::Fresh
    }

    /// Count a write; how long to wait if there have been too many.
    fn write(&mut self, now: Instant, limits: &Limits) -> Option<Duration> {
        forget_older(&mut self.writes, now, MINUTE);
        if self.writes.len() >= limits.writes_per_minute {
            return self.writes.front().map(|first| MINUTE.saturating_sub(now.saturating_duration_since(*first)));
        }
        self.writes.push_back(now);
        None
    }
}

/// What every request handler shares.
struct Shared {
    secret: Secret,
    jobs: mpsc::Sender<Job>,
    guard: Mutex<Guard>,
    limits: Limits,
    /// When this run started listening (unix seconds): a request signed
    /// then or before is refused, as the nonces of the last run are gone.
    started_at: u64,
}

/// The paths there are, and the method each takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    State,
    Settings,
    Restart,
    Rollback,
    Audit,
}

impl Route {
    fn find(path: &str) -> Option<Self> {
        Some(match path {
            "/manage/v1/state" => Route::State,
            "/manage/v1/settings" => Route::Settings,
            "/manage/v1/restart" => Route::Restart,
            "/manage/v1/rollback" => Route::Rollback,
            "/manage/v1/audit" => Route::Audit,
            _ => return None,
        })
    }

    fn method(self) -> Method {
        match self {
            Route::State | Route::Audit => Method::GET,
            Route::Settings | Route::Restart | Route::Rollback => Method::POST,
        }
    }
}

/// An answer, signed for the request with `nonce` if given (every answer
/// after the signature is checked).
fn answer(signer: Option<(&Secret, &str)>, status: StatusCode, body: &Value) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    let signature = signer.map(|(secret, nonce)| wire::response_signature(secret, nonce, status.as_u16(), &bytes));
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    // One request per connection, so a connection's time limit is the request's.
    headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
    if let Some(signature) = signature.and_then(|s| HeaderValue::from_str(&s).ok()) {
        headers.insert(wire::SIGNATURE, signature);
    }
    if let Some(wait) = body.get("retry_after").and_then(Value::as_u64) {
        headers.insert(header::RETRY_AFTER, HeaderValue::from(wait));
    }
    response
}

fn error(signer: Option<(&Secret, &str)>, status: StatusCode, code: &str, message: &str) -> Response {
    answer(signer, status, &json!({ "error": code, "message": message }))
}

/// The settings body's `changes`, in the order given (duplicates kept, to refuse them).
struct Changes(Vec<(String, String)>);

impl<'de> Deserialize<'de> for Changes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Pairs;
        impl<'de> serde::de::Visitor<'de> for Pairs {
            type Value = Changes;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object of settings and their values as text")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Changes, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry::<String, String>()? {
                    pairs.push(pair);
                }
                Ok(Changes(pairs))
            }
        }
        deserializer.deserialize_map(Pairs)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsBody {
    revision: String,
    changes: Changes,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RollbackBody {
    id: String,
}

fn lower_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `key` looks like a setting's name: a lowercase letter, then up to
/// 63 lowercase letters, digits or `_`. Anything else (a control character,
/// a bidi override) is refused before it reaches an answer or the log.
fn setting_name(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && key.len() <= 64
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// `/audit`'s query: `before` and `limit` (1 to 100, default 50), each at most once.
fn audit_query(query: Option<&str>) -> Result<Op, &'static str> {
    const BAD: &str = "/audit takes before=<sequence number> and limit=<1 to 100>, each at most once";
    let (mut before, mut limit) = (None, None);
    for pair in query.unwrap_or_default().split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').ok_or(BAD)?;
        if value.is_empty() || value.len() > 20 || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(BAD);
        }
        let number: u64 = value.parse().map_err(|_| BAD)?;
        let slot = match key {
            "before" => &mut before,
            "limit" => &mut limit,
            _ => return Err(BAD),
        };
        if slot.replace(number).is_some() {
            return Err(BAD);
        }
    }
    let limit = match limit {
        None => 50,
        Some(n @ 1..=100) => n as usize,
        Some(_) => return Err(BAD),
    };
    Ok(Op::Audit { before, limit })
}

/// What a signed request asks for, read from its query and body.
fn parse_op(route: Route, query: Option<&str>, headers: &HeaderMap, body: &[u8]) -> Result<Op, &'static str> {
    if route == Route::Audit {
        return audit_query(query);
    }
    if query.is_some_and(|q| !q.is_empty()) {
        return Err("this path takes no query");
    }
    if route == Route::State {
        return Ok(Op::State);
    }
    let mut types = headers.get_all(header::CONTENT_TYPE).iter();
    let json = types
        .next()
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next().is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json")))
        && types.next().is_none();
    if !json {
        return Err("the body must be sent as Content-Type: application/json");
    }
    match route {
        Route::Settings => {
            let parsed: SettingsBody = serde_json::from_slice(body)
                .map_err(|_| "the body must be {\"revision\": \"<32 hex>\", \"changes\": {\"<setting>\": \"<value as text>\"}}")?;
            if !lower_hex(&parsed.revision, 32) {
                return Err("revision must be 32 lowercase hex characters, as /state gives it");
            }
            let changes = parsed.changes.0;
            if changes.len() > MAX_CHANGES {
                return Err("at most 32 changes at once");
            }
            if !changes.iter().all(|(key, _)| setting_name(key)) {
                return Err("a setting's name is a lowercase letter, then up to 63 lowercase letters, digits or _");
            }
            if changes.iter().any(|(_, value)| value.len() > MAX_VALUE) {
                return Err("a value may be at most 4096 bytes");
            }
            if changes.iter().enumerate().any(|(at, (key, _))| changes[..at].iter().any(|(k, _)| k == key)) {
                return Err("a setting is given twice");
            }
            Ok(Op::Settings { revision: parsed.revision, changes })
        }
        Route::Restart => {
            let empty = serde_json::from_slice::<serde_json::Map<String, Value>>(body).is_ok_and(|map| map.is_empty());
            if !empty {
                return Err("the body must be {}");
            }
            Ok(Op::Restart)
        }
        Route::Rollback => {
            let parsed: RollbackBody = serde_json::from_slice(body).map_err(|_| "the body must be {\"id\": \"<16 hex>\"}")?;
            if !lower_hex(&parsed.id, 16) {
                return Err("id must be 16 lowercase hex characters, as a change's id");
            }
            Ok(Op::Rollback { id: parsed.id })
        }
        Route::State | Route::Audit => Err("not a write"),
    }
}

/// Steps 4 to 7 for a request whose signature checked out as `signed`
/// (None if it didn't): the signature (with the lockout after a burst of bad
/// ones), the time, the nonce and, for a write, the rate limit.
fn admit(shared: &Shared, signed: Option<wire::Signed>, write: bool) -> Result<wire::Signed, Box<Response>> {
    let now = Instant::now();
    let limits = &shared.limits;
    let mut guard = shared.guard.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(signed) = signed else {
        // Only requests without a valid signature are locked out, so whoever
        // can reach the port can't lock the website out by sending bad ones.
        // They are still counted while it lasts, so a flood keeps it going.
        let was_locked = guard.locked(now).is_some();
        let (first, locked) = guard.fail(now, limits);
        let wait = if was_locked { guard.locked(now) } else { None };
        drop(guard);
        if first {
            tracing::warn!("Remote management: a request with a bad or missing signature (more in the next minute aren't logged)");
        }
        if locked {
            tracing::warn!(
                "Remote management: {} requests with a bad signature in a minute; refusing requests without a valid signature for a minute",
                limits.failures
            );
            let (reply, _) = oneshot::channel();
            let _ = shared.jobs.try_send(Job { actor: "-".into(), op: Op::AuthFailures { count: limits.failures }, reply });
        }
        if let Some(wait) = wait {
            let message = "Too many requests with a bad signature: requests without a valid signature are refused for a minute";
            return Err(Box::new(answer(
                None,
                StatusCode::TOO_MANY_REQUESTS,
                &json!({ "error": "rate_limited", "message": message, "retry_after": seconds(wait) }),
            )));
        }
        return Err(Box::new(error(None, StatusCode::UNAUTHORIZED, "unauthorised", "The request isn't signed with this node's secret")));
    };
    let signer = Some((&shared.secret, signed.nonce.as_str()));
    let node_time = unix_now();
    if node_time.abs_diff(signed.timestamp) > MAX_SKEW {
        let message = format!(
            "This node's clock is {} minutes off from the request's time, so signed requests can't be trusted; fix the time on the node or the website",
            node_time.abs_diff(signed.timestamp).div_ceil(60)
        );
        return Err(Box::new(answer(
            signer,
            StatusCode::UNAUTHORIZED,
            &json!({ "error": "clock_skew", "message": message, "node_time": node_time }),
        )));
    }
    if signed.timestamp <= shared.started_at {
        let message =
            "This request was signed before the node last started, so it may have been sent before: sign it again with the current time";
        return Err(Box::new(answer(
            signer,
            StatusCode::UNAUTHORIZED,
            &json!({ "error": "before_start", "message": message, "node_time": node_time, "started_at": shared.started_at }),
        )));
    }
    match guard.nonce(&signed.nonce, signed.timestamp, node_time, limits) {
        Nonce::Fresh => {}
        Nonce::Seen => {
            return Err(Box::new(error(
                signer,
                StatusCode::UNAUTHORIZED,
                "unauthorised",
                "This request was sent before: each nonce works once",
            )));
        }
        Nonce::Full => {
            return Err(Box::new(error(
                signer,
                StatusCode::SERVICE_UNAVAILABLE,
                "nonce_cache_full",
                "Too many requests in the last ten minutes: try again later",
            )));
        }
    }
    if write && let Some(wait) = guard.write(now, limits) {
        let message = "At most 10 changes a minute: try again shortly";
        return Err(Box::new(answer(
            signer,
            StatusCode::TOO_MANY_REQUESTS,
            &json!({ "error": "rate_limited", "message": message, "retry_after": seconds(wait) }),
        )));
    }
    drop(guard);
    Ok(signed)
}

/// Answer one request (see the module docs for the order of checks).
async fn handle(shared: Arc<Shared>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Some(route) = Route::find(parts.uri.path()) else {
        return error(None, StatusCode::NOT_FOUND, "not_found", "There's nothing at this path");
    };
    if parts.method != route.method() {
        let mut response = error(None, StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", "This path takes another method");
        response.headers_mut().insert(header::ALLOW, HeaderValue::from_static(if route.method() == Method::GET { "GET" } else { "POST" }));
        return response;
    }
    let too_large = || error(None, StatusCode::PAYLOAD_TOO_LARGE, "too_large", "The body may be at most 16 KiB");
    let declared = parts.headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|length| length > MAX_BODY as u64) {
        return too_large();
    }
    let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else { return too_large() };
    let target = parts.uri.path_and_query().map_or_else(|| parts.uri.path(), |pq| pq.as_str());
    let signed = wire::Signed::parse(&parts.headers).filter(|s| s.verify(&shared.secret, parts.method.as_str(), target, &body));
    let signed = match admit(&shared, signed, route == Route::Settings || route == Route::Restart || route == Route::Rollback) {
        Ok(signed) => signed,
        Err(refused) => return *refused,
    };
    let signer = Some((&shared.secret, signed.nonce.as_str()));
    let limits = shared.limits;
    let op = match parse_op(route, parts.uri.query(), &parts.headers, &body) {
        Ok(op) => op,
        Err(message) => return error(signer, StatusCode::BAD_REQUEST, "bad_request", message),
    };
    let (reply, answered) = oneshot::channel();
    if shared.jobs.try_send(Job { actor: signed.actor.clone(), op, reply }).is_err() {
        return error(signer, StatusCode::SERVICE_UNAVAILABLE, "queue_full", "The node has too many requests waiting: try again shortly");
    }
    match tokio::time::timeout(limits.answer_within, answered).await {
        Ok(Ok(reply)) => answer(signer, reply.status, &reply.body),
        _ => error(
            signer,
            StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            "The node didn't answer in time; it may still carry the request out, so load its state again",
        ),
    }
}

/// The router for a run that started listening at `started_at` (unix seconds).
fn router(secret: Secret, jobs: mpsc::Sender<Job>, limits: Limits, started_at: u64) -> Router {
    let shared = Arc::new(Shared { secret, jobs, guard: Mutex::new(Guard::default()), limits, started_at });
    Router::new().fallback(move |request: Request| handle(shared.clone(), request))
}

/// An address, an IPv4 one written as IPv6 (`::ffff:a.b.c.d`) as IPv4, so it
/// is checked (and a peer counted) the same either way.
fn peer_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        ip => ip,
    }
}

/// The connections open from each address.
#[derive(Debug, Default)]
struct Peers(Mutex<HashMap<IpAddr, usize>>);

impl Peers {
    /// A place for one more connection from `ip`, if it has fewer than `max` open.
    fn take(self: &Arc<Self>, ip: IpAddr, max: usize) -> Option<PeerSlot> {
        let mut open = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let count = open.get(&ip).copied().unwrap_or(0);
        if count >= max {
            return None;
        }
        open.insert(ip, count + 1);
        Some(PeerSlot { peers: self.clone(), ip })
    }

    #[cfg(test)]
    fn open(&self, ip: IpAddr) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).get(&ip).copied().unwrap_or(0)
    }
}

/// One connection's place among its address's, given back when it closes.
struct PeerSlot {
    peers: Arc<Peers>,
    ip: IpAddr,
}

impl Drop for PeerSlot {
    fn drop(&mut self) {
        let mut open = self.peers.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = open.get_mut(&self.ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.ip);
            }
        }
    }
}

/// Whether a refused connection is logged now: once a minute at most, so a
/// flood doesn't fill the log. `quiet_until` is when the next may be.
fn log_now(quiet_until: &mut Option<Instant>, now: Instant) -> bool {
    if quiet_until.is_some_and(|until| now < until) {
        return false;
    }
    *quiet_until = Some(now + MINUTE);
    true
}

/// The management listener's connections: at most so many at once, and so
/// many from one address (more are closed at once), each closed when its
/// time is up, so a client that sends slowly or not at all can't hold the
/// listener, and one address can't take every place.
struct Bounded {
    listener: TcpListener,
    slots: Arc<Semaphore>,
    peers: Arc<Peers>,
    per_peer: usize,
    lifetime: Duration,
    /// When a refused connection may be logged again.
    quiet_until: Option<Instant>,
}

impl Bounded {
    fn refused(&mut self, why: std::fmt::Arguments<'_>) {
        if log_now(&mut self.quiet_until, Instant::now()) {
            tracing::warn!("Remote management: refused a connection: {why} (more in the next minute aren't logged)");
        }
    }
}

impl axum::serve::Listener for Bounded {
    type Io = Deadline;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (tcp, peer) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(e) => {
                    tracing::debug!("remote management accept: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let ip = peer_ip(peer.ip());
            let Some(peer_slot) = self.peers.take(ip, self.per_peer) else {
                let per_peer = self.per_peer;
                self.refused(format_args!("{ip} has {per_peer} open already"));
                continue;
            };
            let Ok(slot) = self.slots.clone().try_acquire_owned() else {
                drop(peer_slot);
                self.refused(format_args!("every place is taken"));
                continue;
            };
            let _ = tcp.set_nodelay(true);
            let deadline = Box::pin(tokio::time::sleep(self.lifetime));
            return (Deadline { io: tcp, deadline, _slot: slot, _peer: peer_slot }, peer);
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

/// A connection that fails every read and write once its time is up. Its
/// places are given back when it is dropped.
struct Deadline {
    io: TcpStream,
    deadline: Pin<Box<tokio::time::Sleep>>,
    _slot: OwnedSemaphorePermit,
    _peer: PeerSlot,
}

impl Deadline {
    fn expired(&mut self, cx: &mut TaskContext<'_>) -> Option<std::io::Error> {
        self.deadline.as_mut().poll(cx).is_ready().then(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "the connection's time is up"))
    }
}

impl AsyncRead for Deadline {
    fn poll_read(self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if let Some(e) = this.expired(cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for Deadline {
    fn poll_write(self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if let Some(e) = this.expired(cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.io).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if let Some(e) = this.expired(cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

/// Listen on `listen.address` and serve until the task is stopped. Fails
/// now if the address can't be used or is a public one, so the node doesn't
/// start without what was asked. The jobs come out of the receiver, for the
/// owner loop to run with [`Owner::run`].
pub async fn serve(listen: Listen) -> Result<(SocketAddr, tokio::task::JoinHandle<()>, mpsc::Receiver<Job>)> {
    serve_with(listen, Limits::default()).await
}

async fn serve_with(listen: Listen, limits: Limits) -> Result<(SocketAddr, tokio::task::JoinHandle<()>, mpsc::Receiver<Job>)> {
    let address: SocketAddr =
        listen.address.parse().with_context(|| format!("--manage-listen expects an address like 10.0.0.2:8750, not {}", listen.address))?;
    if let Some(warning) = check_bind(&address)? {
        eprintln!("warning: {warning}");
        tracing::warn!("{warning}");
    }
    let listener = TcpListener::bind(address).await.with_context(|| format!("could not listen for remote management on {address}"))?;
    let bound = listener.local_addr()?;
    let (jobs, received) = mpsc::channel(limits.queue);
    let app = router(listen.secret, jobs, limits, unix_now());
    let listener = Bounded {
        listener,
        slots: Arc::new(Semaphore::new(limits.connections)),
        peers: Arc::default(),
        per_peer: limits.connections_per_peer,
        lifetime: limits.connection_lifetime,
        quiet_until: None,
    };
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((bound, task, received))
}

/// What is printed when the listener is up.
pub fn started_line(bound: SocketAddr) -> String {
    format!("Remote management at http://{bound}/manage/v1/ (signed requests only)")
}

/// The next job for the owner loop, or never if management is off.
pub async fn next_job(jobs: &mut Option<mpsc::Receiver<Job>>) -> Option<Job> {
    match jobs {
        Some(jobs) => jobs.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests;
