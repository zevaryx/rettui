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
//! and read its own audit log. It can change a few keys of the Reticulum
//! config too ([`rnsedit`]: Transport node, and an interface's `enabled`,
//! mode, announce cap, bitrate and ingress control), on probation: the
//! website must confirm the change (`/confirm`) once Reticulum is back with
//! it and answers a test request on each listener it has to keep
//! ([`selftest`]), or the node puts the old text back by itself, as it does at the next
//! start if it stopped meanwhile (see [`pending`]). It can't run programs,
//! read or change keys, or change any other part of the Reticulum config,
//! nor an interface that runs a program. Requests are checked in this order:
//! method and path, size, the signing headers' form, the signature (and,
//! for a request without a valid one, the lockout after a burst of them),
//! the time, the nonce, the rate limits, and room in the job queue. Every
//! answer after the signature is checked is signed too.
//!
//! A nonce is remembered for as long as its request's time is accepted, so
//! a request can't be replayed within a run; one signed before the run
//! started is refused, as its nonce may have been used in the last run.
//! The highest request time accepted is kept on disk too
//! (`manage/last-request`), so a request dated ahead of the node's clock
//! can't be replayed after a restart either. Within the 5 minutes a
//! request's time is accepted, a request held back on the way can still be
//! delivered once, which is why the link must be private.
//!
//! The listener speaks HTTP/1.1 only, one request per connection. A
//! connection from an address outside `--manage-allow` (if given) is closed
//! before anything is read, and a request must have arrived whole within 5
//! seconds of connecting.

mod audit;
pub mod fields;
mod owner;
pub(crate) mod pending;
mod rnsedit;
pub(crate) mod selftest;
pub(crate) mod wire;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
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
#[cfg(test)]
pub(crate) use owner::Timing;
pub use pending::recover;
pub use rnsedit::{MAX_RNS_CHANGES, RnsChange};
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

/// The file in the management folder that keeps the highest request time
/// accepted (unix seconds).
const MARK_FILE: &str = "last-request";

/// Where to listen, the secret requests are signed with, the addresses a
/// connection may come from (any if empty), and whether a Reticulum config
/// change may turn interfaces or transport on (`--manage-allow-enable`).
#[derive(Debug, Clone)]
pub struct Listen {
    pub address: String,
    pub secret: Secret,
    pub allow: Vec<Network>,
    pub allow_enable: bool,
}

/// An address or a network a connection may come from (`--manage-allow`):
/// `10.0.0.5`, `10.0.0.0/24` or `fd00::/64`. An IPv4 address written as IPv6
/// is the IPv4 one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    ip: IpAddr,
    prefix: u8,
}

/// `ip` with all but its first `prefix` bits cleared.
fn masked(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4((u32::from(v4) & u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)).into()),
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0)).into()),
    }
}

impl Network {
    /// Read `--manage-allow`'s value. Bits after the prefix are ignored, as
    /// firewalls do (`10.0.0.5/24` is `10.0.0.0/24`).
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = || format!("expected an address or a network, as 10.0.0.5, 10.0.0.0/24 or fd00::/64, not {text:?}");
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let written: IpAddr = address.parse().map_err(|_| bad())?;
        let ip = peer_ip(written);
        let longest = if written.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            None => longest,
            Some(p) if !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()) => p.parse::<u8>().map_err(|_| bad())?,
            Some(_) => return Err(bad()),
        };
        if prefix > longest {
            return Err(bad());
        }
        // `::ffff:10.0.0.0/120` is `10.0.0.0/24`.
        let prefix = match (written, ip) {
            (IpAddr::V6(_), IpAddr::V4(_)) => prefix.checked_sub(96).ok_or_else(bad)?,
            _ => prefix,
        };
        Ok(Self { ip: masked(ip, prefix), prefix })
    }

    /// Whether a connection from `ip` is in this network.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = peer_ip(ip);
        ip.is_ipv4() == self.ip.is_ipv4() && masked(ip, self.prefix) == self.ip
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.ip, self.prefix)
    }
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
    /// Change the Reticulum config, on probation for `confirm_seconds`.
    Reticulum {
        revision: String,
        confirm_seconds: u64,
        changes: Vec<RnsChange>,
    },
    /// Keep the Reticulum config change `id`, which is on probation.
    Confirm {
        id: String,
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
/// error never repeats what the file holds. A file its group or other users
/// can read or write is refused (on Unix), as whoever reads it can manage
/// the node, and whoever writes it can choose the secret.
pub fn load_secret(path: &Path) -> Result<Secret> {
    let unreadable = || format!("could not read the manage secret file {}", path.display());
    let file = std::fs::File::open(path).with_context(unreadable)?;
    // The file opened, not the path again, so it can't be swapped between the check and the read.
    if open_to_others(&file).with_context(unreadable)? {
        bail!(
            "the manage secret file {} can be read or changed by other users, so they could manage this node: make it readable by you only (chmod 600 {})",
            path.display(),
            path.display()
        );
    }
    let mut text = Zeroizing::new(String::new());
    // A little more than the longest, so a file that is too big is refused without reading it all.
    file.take(wire::SECRET_MAX as u64 + 1024).read_to_string(&mut text).with_context(unreadable)?;
    Secret::parse(text.trim()).map_err(|reason| anyhow!("--manage-secret-file: {reason}"))
}

/// Whether its group or other users can read or write `file` (Unix only).
fn open_to_others(file: &std::fs::File) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(file.metadata()?.permissions().mode() & 0o066 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(false)
    }
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

/// The warning when connections aren't limited to the website's address.
fn allow_warning(allow: &[Network]) -> Option<String> {
    allow.is_empty().then(|| {
        "no --manage-allow given: any address that can reach the manage port can hold its connection places and keep the website out; give --manage-allow with the website's address".to_string()
    })
}

/// The listener's limits (smaller in tests).
#[derive(Debug, Clone, Copy)]
struct Limits {
    /// The most nonces remembered (each for as long as its request's time is accepted).
    nonce_max: usize,
    /// Writes taken in a minute.
    writes_per_minute: usize,
    /// Confirms taken in a minute, counted apart from the writes, so others'
    /// writes can't keep a change from being confirmed.
    confirms_per_minute: usize,
    /// Writes refused before anything changed (another change pending, a
    /// restart running, the Reticulum limits) given back to the write limit
    /// in a minute, at most.
    refunds_per_minute: usize,
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
    /// How long after connecting the whole request (headers and body) must
    /// be in, so idle or slow connections can't hold the places.
    request_within: Duration,
    /// While requests without a valid signature are refused, how often the
    /// bad ones still coming are logged and added to the audit log.
    report_every: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            nonce_max: 10_000,
            writes_per_minute: 10,
            confirms_per_minute: 6,
            refunds_per_minute: 60,
            failures: 20,
            lockout: Duration::from_secs(60),
            queue: 8,
            answer_within: Duration::from_secs(10),
            connections: 32,
            connections_per_peer: 4,
            connection_lifetime: Duration::from_secs(15),
            request_within: Duration::from_secs(5),
            report_every: Duration::from_secs(600),
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
    confirms: VecDeque<Instant>,
    refunds: VecDeque<Instant>,
    failures: VecDeque<Instant>,
    locked_until: Option<Instant>,
    /// Bad signatures since the last report (log line and audit entry), and
    /// when that was.
    unreported: usize,
    reported_at: Option<Instant>,
}

/// What a bad signature calls for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Failure {
    /// It starts a burst (logged, once).
    first: bool,
    /// The lockout starts, or has gone on for a while: log this many bad
    /// signatures and add them to the audit log.
    report: Option<usize>,
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

    /// Count a bad signature. A burst of them starts (or extends) the
    /// lockout. Only the first of a burst is logged, and the lockout is
    /// reported when it starts and then once every `report_every` while a
    /// flood keeps it going, never for every burst: each report is a log
    /// line and an audit log write.
    fn fail(&mut self, now: Instant, limits: &Limits) -> Failure {
        // What an earlier lockout, or this one's window, still owes first.
        let mut report = self.flush(now, limits);
        let was_locked = self.locked(now).is_some();
        forget_older(&mut self.failures, now, MINUTE);
        let first = self.failures.is_empty() && !was_locked;
        if first {
            self.unreported = 0;
        }
        self.failures.push_back(now);
        self.unreported = self.unreported.saturating_add(1);
        if self.failures.len() >= limits.failures {
            self.failures.clear();
            self.locked_until = Some(now + limits.lockout);
            let due = !was_locked || self.reported_at.is_none_or(|at| now.saturating_duration_since(at) >= limits.report_every);
            if due {
                report = Some(report.unwrap_or(0).saturating_add(std::mem::take(&mut self.unreported)));
                self.reported_at = Some(now);
            }
        }
        Failure { first, report }
    }

    /// The bad signatures counted since a lockout was last reported, once
    /// they are due: when that lockout has ended, or `report_every` after
    /// the last report while it goes on. So every one is reported, once,
    /// even when a flood stops between reports.
    fn flush(&mut self, now: Instant, limits: &Limits) -> Option<usize> {
        let at = self.reported_at?;
        let locked = self.locked(now).is_some();
        if locked && now.saturating_duration_since(at) < limits.report_every {
            return None;
        }
        self.reported_at = locked.then_some(now);
        Some(std::mem::take(&mut self.unreported)).filter(|count| *count > 0)
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

    /// Count a write (or a confirm) at `now`; how long to wait if there
    /// have been too many.
    fn write(&mut self, kind: Limited, now: Instant, limits: &Limits) -> Option<Duration> {
        let (taken, max) = match kind {
            Limited::Write => (&mut self.writes, limits.writes_per_minute),
            Limited::Confirm => (&mut self.confirms, limits.confirms_per_minute),
        };
        forget_older(taken, now, MINUTE);
        if taken.len() >= max {
            return taken.front().map(|first| MINUTE.saturating_sub(now.saturating_duration_since(*first)));
        }
        taken.push_back(now);
        None
    }

    /// Give back the write (or confirm) counted at `at`, which was refused
    /// before anything changed: at most `refunds_per_minute` a minute, so
    /// refusals (which are still audited) can't come at any rate.
    fn refund(&mut self, (kind, at): Counted, now: Instant, limits: &Limits) {
        forget_older(&mut self.refunds, now, MINUTE);
        if self.refunds.len() >= limits.refunds_per_minute {
            return;
        }
        let taken = match kind {
            Limited::Write => &mut self.writes,
            Limited::Confirm => &mut self.confirms,
        };
        if let Some(index) = taken.iter().position(|taken| *taken == at) {
            taken.remove(index);
            self.refunds.push_back(now);
        }
    }
}

/// What every request handler shares.
struct Shared {
    secret: Secret,
    jobs: mpsc::Sender<Job>,
    guard: Mutex<Guard>,
    limits: Limits,
    /// When this run started listening (unix seconds), and the time a
    /// request must be signed after: the later of that and the highest time
    /// accepted in an earlier run (at most [`MAX_SKEW`] past the start, so a
    /// clock that was ahead can't lock the website out for long). The
    /// nonces of the last run are gone, so anything older may be a replay.
    started_at: u64,
    floor: u64,
    /// Where the highest time accepted is kept, if anywhere, and that time.
    mark: Option<PathBuf>,
    highest: tokio::sync::Mutex<u64>,
    /// When a mark that couldn't be saved may be logged again.
    mark_quiet: Mutex<Option<Instant>>,
}

/// The highest request time an earlier run accepted, from `path`: 0 if there
/// is none, or if it can't be read (said in the log; the start time still
/// holds back what came before this run).
fn load_mark(path: &Path) -> u64 {
    let text = match crate::config::read_limited_text(path, 64) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(e) => {
            tracing::warn!(
                "Remote management: the time of the last request can't be read ({e}), so only requests signed before this start are refused"
            );
            return 0;
        }
    };
    let text = text.trim();
    match text.parse::<u64>() {
        Ok(mark) if !text.is_empty() && text.len() <= 20 && text.bytes().all(|b| b.is_ascii_digit()) => mark,
        _ => {
            tracing::warn!(
                "Remote management: the time of the last request isn't a number, so only requests signed before this start are refused"
            );
            0
        }
    }
}

/// Keep `mark` in `path`, readable by the node's user only.
fn save_mark(path: &Path, mark: u64) -> Result<()> {
    if let Some(dir) = path.parent() {
        audit::make_private_dir(dir)?;
    }
    crate::config::write_private(path, format!("{mark}\n").as_bytes())
}

/// The time a request must be signed after, for a run that started at
/// `started_at` after one that accepted up to `mark`.
fn floor_of(started_at: u64, mark: u64) -> u64 {
    started_at.max(mark.min(started_at.saturating_add(MAX_SKEW)))
}

impl Shared {
    /// Log `count` bad signatures and send them to the audit log.
    fn report_failures(&self, count: usize) {
        tracing::warn!(
            "Remote management: {count} requests with a bad signature; refusing requests without a valid signature for a minute after each {} (logged again in {} minutes at most)",
            self.limits.failures,
            self.limits.report_every.as_secs() / 60
        );
        let (reply, _) = oneshot::channel();
        let _ = self.jobs.try_send(Job { actor: "-".into(), op: Op::AuthFailures { count }, reply });
    }

    /// Report the bad signatures that are due, if any (see [`Guard::flush`]).
    fn flush_failures(&self) {
        let due = self.guard.lock().unwrap_or_else(PoisonError::into_inner).flush(Instant::now(), &self.limits);
        if let Some(count) = due {
            self.report_failures(count);
        }
    }

    fn new(secret: Secret, jobs: mpsc::Sender<Job>, limits: Limits, started_at: u64, mark: Option<PathBuf>) -> Self {
        let highest = mark.as_deref().map_or(0, load_mark);
        Self {
            secret,
            jobs,
            guard: Mutex::new(Guard::default()),
            limits,
            started_at,
            floor: floor_of(started_at, highest),
            mark,
            highest: tokio::sync::Mutex::new(highest),
            mark_quiet: Mutex::new(None),
        }
    }

    /// Keep `timestamp` on disk if it is the highest accepted yet, before
    /// the request is carried out, so it can't be replayed after a restart.
    /// A mark that can't be saved is logged and the request still goes on:
    /// the start time holds back what came before a restart all the same,
    /// and the node must stay manageable (to turn things off) on a full disk.
    async fn keep_mark(&self, timestamp: u64) {
        let Some(path) = &self.mark else { return };
        let mut highest = self.highest.lock().await;
        if timestamp <= *highest {
            return;
        }
        let path = path.clone();
        let saved = tokio::task::spawn_blocking(move || save_mark(&path, timestamp)).await;
        *highest = timestamp;
        let error = match saved {
            Ok(Ok(())) => return,
            Ok(Err(e)) => format!("{e:#}"),
            Err(e) => e.to_string(),
        };
        if log_now(&mut self.mark_quiet.lock().unwrap_or_else(PoisonError::into_inner), Instant::now()) {
            tracing::warn!(
                "Remote management: couldn't keep the time of the last request ({error}); more in the next minute aren't logged"
            );
        }
    }
}

/// The paths there are, and the method each takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    State,
    Settings,
    Restart,
    Rollback,
    Audit,
    Reticulum,
    Confirm,
}

impl Route {
    fn find(path: &str) -> Option<Self> {
        Some(match path {
            "/manage/v1/state" => Route::State,
            "/manage/v1/settings" => Route::Settings,
            "/manage/v1/restart" => Route::Restart,
            "/manage/v1/rollback" => Route::Rollback,
            "/manage/v1/audit" => Route::Audit,
            "/manage/v1/reticulum" => Route::Reticulum,
            "/manage/v1/confirm" => Route::Confirm,
            _ => return None,
        })
    }

    fn method(self) -> Method {
        match self {
            Route::State | Route::Audit => Method::GET,
            Route::Settings | Route::Restart | Route::Rollback | Route::Reticulum | Route::Confirm => Method::POST,
        }
    }

    /// The limit it counts against: confirms have their own (one is bound
    /// to its change and its maker), reads none.
    fn limited(self) -> Option<Limited> {
        match self {
            Route::State | Route::Audit => None,
            Route::Confirm => Some(Limited::Confirm),
            Route::Settings | Route::Restart | Route::Rollback | Route::Reticulum => Some(Limited::Write),
        }
    }
}

/// The rate limits a request counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Limited {
    Write,
    Confirm,
}

/// A request counted against a limit, and when.
type Counted = (Limited, Instant);

/// Whether the owner's answer refused a write before anything changed, so
/// it is given back to its limit: another change is pending, a restart or a
/// change is running, or a Reticulum limit holds it back; for a confirm, any
/// refusal (another user's, or one for no pending change, can't use up the
/// confirms its maker needs).
fn changed_nothing(kind: Limited, reply: &Reply) -> bool {
    let code = reply.body.get("error").and_then(Value::as_str);
    if kind == Limited::Confirm {
        return !reply.status.is_success();
    }
    match reply.status {
        StatusCode::CONFLICT => matches!(code, Some("pending" | "restarting" | "busy")),
        StatusCode::TOO_MANY_REQUESTS => code == Some("rate_limited"),
        _ => false,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReticulumBody {
    revision: String,
    confirm_seconds: u64,
    changes: Vec<RnsChangeBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RnsChangeBody {
    path: Vec<String>,
    key: String,
    value: String,
}

/// The shortest and longest probation a change may ask for, in seconds.
pub const CONFIRM_SECONDS: std::ops::RangeInclusive<u64> = 60..=900;
/// The longest value in a Reticulum config change.
const MAX_RNS_VALUE: usize = 32;

/// Whether `key` can be a Reticulum config key in a request: 1 to 32
/// lowercase letters or `_`.
fn rns_key(key: &str) -> bool {
    (1..=32).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// `/reticulum`'s body, checked for its shape (what may change, and to
/// what, is the owner loop's to check).
fn reticulum_op(body: &[u8]) -> Result<Op, &'static str> {
    let parsed: ReticulumBody = serde_json::from_slice(body).map_err(|_| {
        "the body must be {\"revision\": \"<32 hex>\", \"confirm_seconds\": <60 to 900>, \"changes\": [{\"path\": [..], \"key\": \"..\", \"value\": \"..\"}]}"
    })?;
    if !lower_hex(&parsed.revision, 32) {
        return Err("revision must be 32 lowercase hex characters, as reticulum.revision in /state gives it");
    }
    if !CONFIRM_SECONDS.contains(&parsed.confirm_seconds) {
        return Err("confirm_seconds must be from 60 to 900");
    }
    if parsed.changes.is_empty() || parsed.changes.len() > MAX_RNS_CHANGES {
        return Err("from 1 to 16 changes at once");
    }
    let mut changes: Vec<RnsChange> = Vec::with_capacity(parsed.changes.len());
    for change in parsed.changes {
        let path_ok = match change.path.as_slice() {
            [section] => section == "reticulum",
            [section, name] => section == "interfaces" && rnsedit::interface_name_ok(name),
            _ => false,
        };
        if !path_ok {
            return Err(
                "a path is [\"reticulum\"] or [\"interfaces\", \"<name>\"], a name being 1 to 64 bytes with no spaces around it, no control characters and none of [ ] # \" '",
            );
        }
        if !rns_key(&change.key) {
            return Err("a key is 1 to 32 lowercase letters or _");
        }
        if change.value.is_empty() || change.value.len() > MAX_RNS_VALUE {
            return Err("a value is 1 to 32 bytes");
        }
        if changes.iter().any(|c| c.path == change.path && c.key == change.key) {
            return Err("the same key of the same section is given twice");
        }
        changes.push(RnsChange { path: change.path, key: change.key, value: change.value });
    }
    Ok(Op::Reticulum { revision: parsed.revision, confirm_seconds: parsed.confirm_seconds, changes })
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
        Route::Rollback | Route::Confirm => {
            let parsed: RollbackBody = serde_json::from_slice(body).map_err(|_| "the body must be {\"id\": \"<16 hex>\"}")?;
            if !lower_hex(&parsed.id, 16) {
                return Err("id must be 16 lowercase hex characters, as a change's id");
            }
            Ok(if route == Route::Confirm { Op::Confirm { id: parsed.id } } else { Op::Rollback { id: parsed.id } })
        }
        Route::Reticulum => reticulum_op(body),
        Route::State | Route::Audit => Err("not a write"),
    }
}

/// Steps 4 to 7 for a request whose signature checked out as `signed`
/// (None if it didn't): the signature (with the lockout after a burst of bad
/// ones), the time, the nonce and, for a write, the rate limit; with it, when
/// a write was counted (to give it back if it changes nothing).
fn admit(
    shared: &Shared,
    signed: Option<wire::Signed>,
    limited: Option<Limited>,
) -> Result<(wire::Signed, Option<Counted>), Box<Response>> {
    let now = Instant::now();
    let limits = &shared.limits;
    let mut guard = shared.guard.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(signed) = signed else {
        // Only requests without a valid signature are locked out, so whoever
        // can reach the port can't lock the website out by sending bad ones.
        // They are still counted while it lasts, so a flood keeps it going.
        let was_locked = guard.locked(now).is_some();
        let failure = guard.fail(now, limits);
        let wait = if was_locked { guard.locked(now) } else { None };
        drop(guard);
        if failure.first {
            tracing::warn!("Remote management: a request with a bad or missing signature (more in the next minute aren't logged)");
        }
        if let Some(count) = failure.report {
            shared.report_failures(count);
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
    if signed.timestamp <= shared.floor {
        let message =
            "This request was signed before the node last started, so it may have been sent before: sign it again with the current time";
        return Err(Box::new(answer(
            signer,
            StatusCode::UNAUTHORIZED,
            &json!({
                "error": "before_start",
                "message": message,
                "node_time": node_time,
                "started_at": shared.started_at,
                "signed_after": shared.floor,
            }),
        )));
    }
    match guard.nonce(&signed.nonce, signed.timestamp, node_time, limits) {
        Nonce::Fresh => {}
        Nonce::Seen => {
            // Not signed: a signed refusal for this nonce could be handed to
            // the website in place of the real answer, which was carried out.
            return Err(Box::new(error(
                None,
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
    if let Some(kind) = limited
        && let Some(wait) = guard.write(kind, now, limits)
    {
        let message = match kind {
            Limited::Write => format!("At most {} changes a minute: try again shortly", limits.writes_per_minute),
            Limited::Confirm => format!("At most {} confirms a minute: try again shortly", limits.confirms_per_minute),
        };
        return Err(Box::new(answer(
            signer,
            StatusCode::TOO_MANY_REQUESTS,
            &json!({ "error": "rate_limited", "message": message, "retry_after": seconds(wait) }),
        )));
    }
    drop(guard);
    Ok((signed, limited.map(|kind| (kind, now))))
}

/// When a connection's request must be in by, and the flag that says it is
/// (which lifts that deadline for the answer).
#[derive(Debug, Clone)]
struct Arrival {
    by: Instant,
    received: Arc<AtomicBool>,
}

/// Answer one request (see the module docs for the order of checks).
async fn handle(shared: Arc<Shared>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let arrival = parts.extensions.get::<Arrival>().cloned();
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
    let read = axum::body::to_bytes(body, MAX_BODY);
    let read = match &arrival {
        Some(arrival) => tokio::time::timeout_at(arrival.by.into(), read).await.ok(),
        None => Some(read.await),
    };
    let late = arrival.as_ref().is_some_and(|a| Instant::now() >= a.by);
    let body = match read {
        Some(Ok(body)) => body,
        _ if late => {
            return error(None, StatusCode::REQUEST_TIMEOUT, "request_timeout", "The request has to arrive within 5 seconds of connecting");
        }
        _ => return too_large(),
    };
    if let Some(arrival) = &arrival {
        arrival.received.store(true, Ordering::Release);
    }
    let target = parts.uri.path_and_query().map_or_else(|| parts.uri.path(), |pq| pq.as_str());
    let signed = wire::Signed::parse(&parts.headers).filter(|s| s.verify(&shared.secret, parts.method.as_str(), target, &body));
    let (signed, counted) = match admit(&shared, signed, route.limited()) {
        Ok(admitted) => admitted,
        Err(refused) => return *refused,
    };
    shared.keep_mark(signed.timestamp).await;
    let signer = Some((&shared.secret, signed.nonce.as_str()));
    let limits = shared.limits;
    // A write that changed nothing goes back to the limit, so others can't
    // fill it with requests the node turns away.
    let refund = || {
        if let Some(counted) = counted {
            shared.guard.lock().unwrap_or_else(PoisonError::into_inner).refund(counted, Instant::now(), &limits);
        }
    };
    let op = match parse_op(route, parts.uri.query(), &parts.headers, &body) {
        Ok(op) => op,
        Err(message) => {
            refund();
            return error(signer, StatusCode::BAD_REQUEST, "bad_request", message);
        }
    };
    let (reply, answered) = oneshot::channel();
    if shared.jobs.try_send(Job { actor: signed.actor.clone(), op, reply }).is_err() {
        refund();
        return error(signer, StatusCode::SERVICE_UNAVAILABLE, "queue_full", "The node has too many requests waiting: try again shortly");
    }
    match tokio::time::timeout(limits.answer_within, answered).await {
        Ok(Ok(reply)) => {
            if counted.is_some_and(|(kind, _)| changed_nothing(kind, &reply)) {
                refund();
            }
            answer(signer, reply.status, &reply.body)
        }
        _ => error(
            signer,
            StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            "The node didn't answer in time; it may still carry the request out, so load its state again",
        ),
    }
}

/// The router for a run that started listening at `started_at` (unix
/// seconds), keeping the highest request time accepted in `mark`, if given
/// (for tests, which drive it without a listener).
#[cfg(test)]
fn router(secret: Secret, jobs: mpsc::Sender<Job>, limits: Limits, started_at: u64, mark: Option<PathBuf>) -> Router {
    router_for(Arc::new(Shared::new(secret, jobs, limits, started_at, mark)))
}

fn router_for(shared: Arc<Shared>) -> Router {
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

/// The management listener's connections: only from the addresses allowed
/// (if any are given), at most so many at once, and so many from one
/// address (more are closed at once), each closed when its time is up, so a
/// client that sends slowly or not at all can't hold the listener, and one
/// address can't take every place.
struct Bounded {
    listener: TcpListener,
    allow: Vec<Network>,
    slots: Arc<Semaphore>,
    peers: Arc<Peers>,
    per_peer: usize,
    lifetime: Duration,
    request_within: Duration,
    /// When a refused connection may be logged again.
    quiet_until: Option<Instant>,
}

impl Bounded {
    fn refused(&mut self, why: std::fmt::Arguments<'_>) {
        if log_now(&mut self.quiet_until, Instant::now()) {
            tracing::warn!("Remote management: refused a connection: {why} (more in the next minute aren't logged)");
        }
    }

    /// The next connection that may be served, with when its request must be in by.
    async fn accept(&mut self) -> (Deadline, Arrival) {
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
            // Closed before it takes a place or anything is read from it.
            if !self.allow.is_empty() && !self.allow.iter().any(|network| network.contains(ip)) {
                drop(tcp);
                self.refused(format_args!("{ip} isn't in --manage-allow"));
                continue;
            }
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
            let now = Instant::now();
            let arrival = Arrival { by: now + self.request_within, received: Arc::default() };
            let deadline = Box::pin(tokio::time::sleep(self.lifetime));
            let request_by = Box::pin(tokio::time::sleep_until(arrival.by.into()));
            let io = Deadline { io: tcp, deadline, request_by, received: arrival.received.clone(), _slot: slot, _peer: peer_slot };
            return (io, arrival);
        }
    }
}

/// A connection that fails every read and write once its time is up, and
/// every read once its request is late (until it is in). Its places are
/// given back when it is dropped.
struct Deadline {
    io: TcpStream,
    deadline: Pin<Box<tokio::time::Sleep>>,
    request_by: Pin<Box<tokio::time::Sleep>>,
    received: Arc<AtomicBool>,
    _slot: OwnedSemaphorePermit,
    _peer: PeerSlot,
}

impl Deadline {
    fn expired(&mut self, cx: &mut TaskContext<'_>) -> Option<std::io::Error> {
        self.deadline.as_mut().poll(cx).is_ready().then(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "the connection's time is up"))
    }

    fn late(&mut self, cx: &mut TaskContext<'_>) -> Option<std::io::Error> {
        (!self.received.load(Ordering::Acquire) && self.request_by.as_mut().poll(cx).is_ready())
            .then(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "the request didn't arrive in time"))
    }
}

impl AsyncRead for Deadline {
    fn poll_read(self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if let Some(e) = this.expired(cx).or_else(|| this.late(cx)) {
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
/// owner loop to run with [`Owner::run`]. The highest request time accepted
/// is kept in the data directory of `paths` (`manage/last-request`).
pub async fn serve(listen: Listen, paths: &crate::config::Paths) -> Result<(SocketAddr, tokio::task::JoinHandle<()>, mpsc::Receiver<Job>)> {
    serve_with(listen, Limits::default(), Some(paths.root.join("manage").join(MARK_FILE))).await
}

/// Serve one connection: HTTP/1.1 only (no HTTP/2, which would carry many
/// requests on one connection past the limits), one request, then closed.
async fn serve_connection(io: Deadline, arrival: Arrival, app: Router) {
    use hyper::service::Service as _;
    let service = hyper_util::service::TowerToHyperService::new(app);
    let service = hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
        request.extensions_mut().insert(arrival.clone());
        service.call(request)
    });
    let served =
        hyper::server::conn::http1::Builder::new().keep_alive(false).serve_connection(hyper_util::rt::TokioIo::new(io), service).await;
    if let Err(e) = served {
        tracing::debug!("remote management connection: {e}");
    }
}

async fn serve_with(
    listen: Listen,
    limits: Limits,
    mark: Option<PathBuf>,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>, mpsc::Receiver<Job>)> {
    let address: SocketAddr =
        listen.address.parse().with_context(|| format!("--manage-listen expects an address like 10.0.0.2:8750, not {}", listen.address))?;
    for warning in check_bind(&address)?.into_iter().chain(allow_warning(&listen.allow)) {
        eprintln!("warning: {warning}");
        tracing::warn!("{warning}");
    }
    let listener = TcpListener::bind(address).await.with_context(|| format!("could not listen for remote management on {address}"))?;
    let bound = listener.local_addr()?;
    let (jobs, received) = mpsc::channel(limits.queue);
    let shared = Arc::new(Shared::new(listen.secret, jobs, limits, unix_now(), mark));
    let app = router_for(shared.clone());
    let mut listener = Bounded {
        listener,
        allow: listen.allow,
        slots: Arc::new(Semaphore::new(limits.connections)),
        peers: Arc::default(),
        per_peer: limits.connections_per_peer,
        lifetime: limits.connection_lifetime,
        request_within: limits.request_within,
        quiet_until: None,
    };
    let task = tokio::spawn(async move {
        // Bad signatures still owed to the log when a flood stops are
        // reported within this long.
        let mut flush = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                (io, arrival) = listener.accept() => {
                    tokio::spawn(serve_connection(io, arrival, app.clone()));
                }
                _ = flush.tick() => shared.flush_failures(),
            }
        }
    });
    Ok((bound, task, received))
}

/// What is printed when the listener is up, with the addresses allowed (if
/// limited).
pub fn started_line(bound: SocketAddr, allow: &[Network]) -> String {
    if allow.is_empty() {
        return format!("Remote management at http://{bound}/manage/v1/ (signed requests only)");
    }
    let from: Vec<String> = allow.iter().map(Network::to_string).collect();
    format!("Remote management at http://{bound}/manage/v1/ (signed requests only, from {} only)", from.join(", "))
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
