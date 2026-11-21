//! The listener driven as the website drives it: signed requests through
//! the router, with the owner loop either a real node's (an app in a temp
//! directory) or a stand-in that says what it was asked.

use std::path::PathBuf;

use axum::http::Request as HttpRequest;
use tower::ServiceExt;

use super::*;
use crate::app::App;
use crate::config::Settings;
use crate::store::Store;

const SECRET: &str = "rettui-manage-test-secret-0123456789abcdef";

fn secret() -> Secret {
    Secret::parse(SECRET).unwrap()
}

fn fresh_nonce() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

/// When the listener in a test started: ten minutes ago, so a request
/// signed now (or up to five minutes back) is after it.
fn started() -> u64 {
    unix_now() - 600
}

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rettui-manage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A request as the website signs it.
struct Signing<'a> {
    method: &'a str,
    target: &'a str,
    body: String,
    actor: &'a str,
    timestamp: u64,
    nonce: String,
    secret: Secret,
}

impl<'a> Signing<'a> {
    fn new(method: &'a str, target: &'a str, body: &str) -> Self {
        Self { method, target, body: body.into(), actor: "alice", timestamp: unix_now(), nonce: fresh_nonce(), secret: secret() }
    }

    fn get(target: &'a str) -> Self {
        Self::new("GET", target, "")
    }

    fn post(target: &'a str, body: &Value) -> Self {
        Self::new("POST", target, &body.to_string())
    }

    fn signature(&self) -> String {
        let text = wire::canonical(&self.timestamp.to_string(), &self.nonce, self.method, self.target, self.actor, self.body.as_bytes());
        format!("v1={}", wire::sign(&self.secret, &text))
    }

    /// The request, signed as it is, then sent as `method` to `target`.
    fn send_as(&self, method: &str, target: &str, body: &str, actor: &str) -> Request {
        let mut builder = HttpRequest::builder()
            .method(method)
            .uri(target)
            .header(wire::TIMESTAMP, self.timestamp.to_string())
            .header(wire::NONCE, &self.nonce)
            .header(wire::ACTOR, actor)
            .header(wire::SIGNATURE, self.signature());
        if method == "POST" {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    fn request(&self) -> Request {
        self.send_as(self.method, self.target, &self.body, self.actor)
    }
}

/// An answer, read whole.
#[derive(Debug)]
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    bytes: Vec<u8>,
    json: Value,
}

impl Answer {
    /// Whether it is signed by the node for the request with `nonce`.
    fn signed_for(&self, nonce: &str) -> bool {
        let Some(header) = self.headers.get(wire::SIGNATURE).and_then(|v| v.to_str().ok()) else { return false };
        header == wire::response_signature(&secret(), nonce, self.status.as_u16(), &self.bytes)
    }

    fn code(&self) -> &str {
        self.json["error"].as_str().unwrap_or_default()
    }
}

/// What answers the jobs.
enum Owners {
    /// A real node.
    Node(Box<(App, Owner)>),
    /// A stand-in that answers 200 with the op it was given, and keeps them.
    Echo(Vec<Op>),
}

struct Harness {
    router: Router,
    jobs: mpsc::Receiver<Job>,
    owners: Owners,
    /// Every answer given, to check none has the secret.
    seen: Vec<String>,
    dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        // No answer, header or saved file ever holds the secret.
        for text in &self.seen {
            assert!(!text.contains(SECRET), "the secret is in an answer: {text}");
        }
        let audit = self.dir.join("manage/audit.jsonl");
        if let Ok(text) = std::fs::read_to_string(audit) {
            assert!(!text.contains(SECRET));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn harness_with(name: &str, limits: Limits, node: bool) -> Harness {
    harness_started(name, limits, node, started())
}

/// A harness whose listener started at `started_at` (unix seconds).
fn harness_started(name: &str, limits: Limits, node: bool, started_at: u64) -> Harness {
    harness_marked(name, limits, node, started_at, None)
}

/// A harness that keeps the highest request time accepted in `mark`.
fn harness_marked(name: &str, limits: Limits, node: bool, started_at: u64, mark: Option<PathBuf>) -> Harness {
    let dir = test_dir(name);
    let (jobs_tx, jobs) = mpsc::channel(limits.queue);
    let router = router(secret(), jobs_tx, limits, started_at, mark);
    let owners = if node {
        let (app, _commands) = crate::app::test_app_with_net(&dir, Settings { pn_enabled: true, ..Settings::default() }, Store::default());
        let owner = Owner::new(&app.paths);
        Owners::Node(Box::new((app, owner)))
    } else {
        Owners::Echo(Vec::new())
    };
    Harness { router, jobs, owners, seen: Vec::new(), dir }
}

fn echo(name: &str) -> Harness {
    harness_with(name, Limits::default(), false)
}

fn node(name: &str) -> Harness {
    harness_with(name, Limits::default(), true)
}

impl Harness {
    fn run(&mut self, job: Job) {
        match &mut self.owners {
            Owners::Node(node) => {
                let (app, owner) = &mut **node;
                owner.run(app, job)
            }
            Owners::Echo(ops) => {
                ops.push(job.op.clone());
                let _ = job.reply.send(Reply::new(StatusCode::OK, json!({ "op": format!("{:?}", job.op), "actor": job.actor })));
            }
        }
    }

    /// Send `request`, running the jobs it makes as the owner loop would.
    async fn call(&mut self, request: Request) -> Answer {
        let router = self.router.clone();
        let mut pending = tokio::spawn(async move { router.oneshot(request).await });
        let response = loop {
            tokio::select! {
                done = &mut pending => break done.unwrap().unwrap(),
                Some(job) = self.jobs.recv() => self.run(job),
            }
        };
        // Jobs made without an answer awaited (a burst of bad signatures).
        while let Ok(job) = self.jobs.try_recv() {
            self.run(job);
        }
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec();
        self.seen.push(format!("{:?} {}", parts.headers, String::from_utf8_lossy(&bytes)));
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Answer { status: parts.status, headers: parts.headers, bytes, json }
    }

    async fn send(&mut self, signing: &Signing<'_>) -> Answer {
        self.call(signing.request()).await
    }

    async fn state(&mut self) -> Value {
        let answer = self.send(&Signing::get("/manage/v1/state")).await;
        assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
        answer.json
    }

    fn app(&mut self) -> &mut App {
        match &mut self.owners {
            Owners::Node(node) => &mut node.0,
            Owners::Echo(_) => panic!("no node"),
        }
    }

    fn ops(&self) -> &[Op] {
        match &self.owners {
            Owners::Echo(ops) => ops,
            Owners::Node(..) => panic!("a real node"),
        }
    }
}

#[tokio::test]
async fn a_signed_request_is_answered_and_the_answer_is_signed() {
    let mut h = echo("signed");
    let signing = Signing::get("/manage/v1/state");
    let answer = h.send(&signing).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert!(answer.signed_for(&signing.nonce));
    assert!(!answer.signed_for(&fresh_nonce()), "for this request only");
    assert_eq!(answer.json["actor"], "alice");
    assert_eq!(answer.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(answer.headers[header::CONTENT_TYPE], "application/json");
    assert_eq!(answer.headers[header::CONNECTION], "close");
    assert_eq!(h.ops(), [Op::State]);
}

#[tokio::test]
async fn a_wrong_or_missing_signature_is_refused_unsigned() {
    let mut h = echo("wrong");
    let mut wrong = Signing::get("/manage/v1/state");
    wrong.secret = Secret::parse(&"w".repeat(40)).unwrap();
    let answer = h.send(&wrong).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::UNAUTHORIZED, "unauthorised"));
    assert!(answer.headers.get(wire::SIGNATURE).is_none(), "nothing is signed for whoever lacks the secret");
    assert_eq!(answer.headers[header::CACHE_CONTROL], "no-store");
    let bare = HttpRequest::builder().uri("/manage/v1/state").body(Body::empty()).unwrap();
    assert_eq!(h.call(bare).await.code(), "unauthorised");
    assert!(h.ops().is_empty());
}

#[tokio::test]
async fn a_request_signed_for_something_else_is_refused() {
    let mut h = echo("swapped");
    let body = json!({ "revision": "0".repeat(32), "changes": { "pn_name": "Hilltop" } }).to_string();
    let signing = Signing::new("POST", "/manage/v1/settings", &body);
    for (method, target, body, actor) in [
        ("POST", "/manage/v1/restart", body.as_str(), "alice"),
        ("POST", "/manage/v1/rollback", body.as_str(), "alice"),
        ("POST", "/manage/v1/settings", r#"{"revision":"00000000000000000000000000000000","changes":{"pn_name":"Other"}}"#, "alice"),
        ("POST", "/manage/v1/settings", body.as_str(), "mallory"),
    ] {
        let answer = h.call(signing.send_as(method, target, body, actor)).await;
        assert_eq!(answer.code(), "unauthorised", "{method} {target} {actor}");
    }
    // A GET signed for one path, sent to another, or with a query added.
    let get = Signing::get("/manage/v1/state");
    assert_eq!(h.call(get.send_as("GET", "/manage/v1/audit", "", "alice")).await.code(), "unauthorised");
    assert_eq!(h.call(get.send_as("GET", "/manage/v1/state?x=1", "", "alice")).await.code(), "unauthorised");
    assert!(h.ops().is_empty());
    // The one it was signed for is fine.
    assert_eq!(h.send(&signing).await.status, StatusCode::OK);
}

#[tokio::test]
async fn the_time_must_be_close_and_a_skew_is_said_only_to_the_secret_holder() {
    let mut h = echo("time");
    for offset in [-400i64, 400, 100_000] {
        let mut signing = Signing::get("/manage/v1/state");
        signing.timestamp = unix_now().saturating_add_signed(offset);
        let answer = h.send(&signing).await;
        assert_eq!((answer.status, answer.code()), (StatusCode::UNAUTHORIZED, "clock_skew"), "{offset}");
        assert!(answer.signed_for(&signing.nonce));
        assert!(answer.json["node_time"].as_u64().unwrap().abs_diff(unix_now()) < 5);
        assert!(answer.json["message"].as_str().unwrap().contains("minutes off"));
    }
    // A wrong signature with a wrong time is only unauthorised.
    let mut both = Signing::get("/manage/v1/state");
    both.timestamp -= 1000;
    both.secret = Secret::parse(&"w".repeat(40)).unwrap();
    let answer = h.send(&both).await;
    assert_eq!(answer.code(), "unauthorised");
    assert!(answer.json.get("node_time").is_none());
    // Within five minutes either way is fine.
    for offset in [-290i64, 290] {
        let mut signing = Signing::get("/manage/v1/state");
        signing.timestamp = unix_now().saturating_add_signed(offset);
        assert_eq!(h.send(&signing).await.status, StatusCode::OK, "{offset}");
    }
}

#[tokio::test]
async fn a_nonce_works_once() {
    let mut h = echo("nonce");
    let signing = Signing::post("/manage/v1/restart", &json!({}));
    assert_eq!(h.send(&signing).await.status, StatusCode::OK);
    let again = h.send(&signing).await;
    assert_eq!((again.status, again.code()), (StatusCode::UNAUTHORIZED, "unauthorised"));
    // Unsigned: on the way, it can't stand in for the answer the first one got.
    assert!(again.headers.get(wire::SIGNATURE).is_none());
    assert!(!again.signed_for(&signing.nonce));
    assert_eq!(h.ops(), [Op::Restart], "the replay did nothing");
}

#[tokio::test]
async fn a_full_nonce_cache_refuses_rather_than_forgets() {
    let mut h = harness_with("nonce-full", Limits { nonce_max: 3, ..Limits::default() }, false);
    for _ in 0..3 {
        assert_eq!(h.send(&Signing::get("/manage/v1/state")).await.status, StatusCode::OK);
    }
    let signing = Signing::get("/manage/v1/state");
    let answer = h.send(&signing).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::SERVICE_UNAVAILABLE, "nonce_cache_full"));
    assert!(answer.signed_for(&signing.nonce));
    assert_eq!(h.ops().len(), 3);
}

#[test]
fn nonces_are_kept_while_their_requests_time_is_accepted() {
    let limits = Limits { nonce_max: 2, ..Limits::default() };
    let mut guard = Guard::default();
    let now = 1_800_000_000;
    let keep = MAX_SKEW + NONCE_MARGIN;
    assert!(matches!(guard.nonce("a", now, now, &limits), Nonce::Fresh));
    // Signed five minutes ahead (a website's clock that is): kept five minutes longer.
    assert!(matches!(guard.nonce("b", now + MAX_SKEW, now, &limits), Nonce::Fresh));
    assert!(matches!(guard.nonce("a", now, now + keep, &limits), Nonce::Seen), "kept until its time is past the window");
    assert!(matches!(guard.nonce("c", now, now + keep, &limits), Nonce::Full));
    // Once its time can't be accepted any more, the first is forgotten and there is room.
    assert!(matches!(guard.nonce("c", now + 10, now + keep + 1, &limits), Nonce::Fresh));
    assert!(matches!(guard.nonce("b", now + 10, now + keep + 1, &limits), Nonce::Seen), "its own time is later");
    assert!(matches!(guard.nonce("b", now + 10, now + MAX_SKEW + keep, &limits), Nonce::Seen));
    assert!(matches!(guard.nonce("a", now + MAX_SKEW + keep, now + MAX_SKEW + keep + 1, &limits), Nonce::Fresh));
    // A clock stepped back keeps them longer, never shorter.
    assert!(matches!(guard.nonce("a", now, now, &limits), Nonce::Seen));
}

#[tokio::test]
async fn a_nonce_is_refused_for_as_long_as_its_time_is_accepted() {
    // A request signed 4 minutes ahead is accepted until 9 minutes from now;
    // replayed at any time in between, it is refused.
    let mut h = echo("nonce-ahead");
    let mut ahead = Signing::get("/manage/v1/state");
    ahead.timestamp = unix_now() + 240;
    assert_eq!(h.send(&ahead).await.status, StatusCode::OK);
    let again = h.send(&ahead).await;
    assert_eq!((again.status, again.code()), (StatusCode::UNAUTHORIZED, "unauthorised"));
    assert_eq!(h.ops().len(), 1);
}

#[tokio::test]
async fn a_request_signed_before_the_listener_started_is_refused() {
    // A run that started a minute ago, as after a restart: a request signed
    // in the last run, still within five minutes, is refused, and the reason
    // is signed for the website only.
    let now = unix_now();
    let mut first_run = harness_started("restart-run1", Limits::default(), false, now - 200);
    let mut captured = Signing::post("/manage/v1/restart", &json!({}));
    captured.timestamp = now - 60;
    assert_eq!(first_run.send(&captured).await.status, StatusCode::OK);
    assert_eq!(first_run.send(&captured).await.code(), "unauthorised", "a replay in the same run");
    drop(first_run);

    let mut second_run = harness_started("restart-run2", Limits::default(), false, now - 30);
    let answer = second_run.send(&captured).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::UNAUTHORIZED, "before_start"));
    assert!(answer.signed_for(&captured.nonce));
    assert_eq!(answer.json["started_at"], now - 30);
    assert!(answer.json["message"].as_str().unwrap().contains("sign it again"));
    // Signed in the second it started, too.
    let mut same_second = Signing::get("/manage/v1/state");
    same_second.timestamp = now - 30;
    assert_eq!(second_run.send(&same_second).await.code(), "before_start");
    // Unsigned, the same request only learns it is unauthorised.
    let mut forged = Signing::post("/manage/v1/restart", &json!({}));
    forged.timestamp = captured.timestamp;
    forged.secret = Secret::parse(&"w".repeat(40)).unwrap();
    let answer = second_run.send(&forged).await;
    assert_eq!(answer.code(), "unauthorised");
    assert!(answer.json.get("started_at").is_none() && answer.headers.get(wire::SIGNATURE).is_none());
    // Signed afresh, it is taken.
    let fresh = Signing::post("/manage/v1/restart", &json!({}));
    assert_eq!(second_run.send(&fresh).await.status, StatusCode::OK);
    assert_eq!(second_run.ops(), [Op::Restart], "the replay did nothing");
}

#[tokio::test]
async fn a_burst_of_bad_signatures_locks_out_only_unsigned_requests() {
    let mut h = echo("burst");
    let mut wrong = Signing::get("/manage/v1/state");
    wrong.secret = Secret::parse(&"w".repeat(40)).unwrap();
    for n in 0..20 {
        wrong.nonce = fresh_nonce();
        assert_eq!(h.send(&wrong).await.code(), "unauthorised", "{n}");
    }
    // The burst is recorded for the audit log.
    assert_eq!(h.ops(), [Op::AuthFailures { count: 20 }]);
    // Bad and missing signatures are now refused with a wait, unsigned.
    wrong.nonce = fresh_nonce();
    let answer = h.send(&wrong).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
    assert!(answer.headers.get(wire::SIGNATURE).is_none());
    let wait = answer.json["retry_after"].as_u64().unwrap();
    assert!((59..=60).contains(&wait), "{wait}");
    assert_eq!(answer.headers[header::RETRY_AFTER], wait.to_string().as_str());
    let bare = h.call(HttpRequest::builder().uri("/manage/v1/state").body(Body::empty()).unwrap()).await;
    assert_eq!((bare.status, bare.code()), (StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
    // The website, signing correctly, is still answered.
    let right = Signing::get("/manage/v1/state");
    let answer = h.send(&right).await;
    assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
    assert!(answer.signed_for(&right.nonce));
    assert_eq!(h.ops(), [Op::AuthFailures { count: 20 }, Op::State]);
    // A flood while it lasts keeps it going, but is neither logged nor
    // written to the audit log again for every 20 (the pentest's flood
    // rewrote the audit file for each).
    for _ in 0..200 {
        wrong.nonce = fresh_nonce();
        assert_eq!(h.send(&wrong).await.status, StatusCode::TOO_MANY_REQUESTS);
    }
    assert_eq!(h.ops(), [Op::AuthFailures { count: 20 }, Op::State], "one report for the lockout");
    assert_eq!(h.send(&Signing::get("/manage/v1/state")).await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_flood_on_a_real_node_writes_the_audit_log_once() {
    let mut h = node("flood");
    let audit = h.dir.join("manage/audit.jsonl");
    let mut wrong = Signing::get("/manage/v1/state");
    wrong.secret = Secret::parse(&"w".repeat(40)).unwrap();
    let mut written = None;
    for n in 0..300 {
        wrong.nonce = fresh_nonce();
        h.send(&wrong).await;
        if n == 19 {
            written = Some(std::fs::metadata(&audit).and_then(|m| m.modified()).expect("the lockout is in the audit log"));
        }
    }
    assert_eq!(std::fs::metadata(&audit).and_then(|m| m.modified()).ok(), written, "the audit log was written again");
    let entries: Vec<Value> = std::fs::read_to_string(&audit).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(entries.len(), 1);
    assert_eq!((entries[0]["op"].as_str(), entries[0]["count"].as_u64()), (Some("auth_failures"), Some(20)));
    let logged = h.app().log.iter().filter(|l| l.contains("requests with a bad signature")).count();
    assert_eq!(logged, 1, "{:?}", h.app().log);
}

#[test]
fn a_lockout_turns_away_unsigned_requests_only() {
    let limits = Limits { failures: 2, ..Limits::default() };
    let (jobs, _received) = mpsc::channel(limits.queue);
    let shared = Shared::new(secret(), jobs, limits, started(), None);
    let signed = || {
        let signing = Signing::get("/manage/v1/state");
        wire::Signed::parse(signing.request().headers())
    };
    assert_eq!(admit(&shared, None, false).unwrap_err().status(), StatusCode::UNAUTHORIZED);
    assert_eq!(admit(&shared, None, false).unwrap_err().status(), StatusCode::UNAUTHORIZED, "the one that starts it");
    assert_eq!(admit(&shared, None, false).unwrap_err().status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(admit(&shared, signed(), false).is_ok());
    assert!(admit(&shared, signed(), true).is_ok());
}

#[test]
fn the_lockout_ends_and_failures_count_within_a_minute() {
    let limits = Limits::default();
    let mut guard = Guard::default();
    let start = Instant::now();
    // Failures spread over more than a minute don't add up.
    for n in 0..40u64 {
        let failure = guard.fail(start + Duration::from_secs(n * 4), &limits);
        assert_eq!(failure.report, None, "{n}");
        assert_eq!(failure.first, n == 0, "only the start of a burst is logged ({n})");
    }
    let mut guard = Guard::default();
    for n in 0..19 {
        assert_eq!(guard.fail(start, &limits).report, None, "{n}");
    }
    assert_eq!(guard.fail(start, &limits).report, Some(20));
    assert_eq!(guard.locked(start + Duration::from_secs(59)), Some(Duration::from_secs(1)));
    assert_eq!(guard.locked(start + Duration::from_secs(60)), None);
    assert_eq!(guard.locked(start + Duration::from_secs(61)), None);
    assert_eq!((seconds(Duration::ZERO), seconds(Duration::from_millis(1)), seconds(Duration::from_secs(60))), (1, 1, 60));
}

#[test]
fn a_lockout_is_reported_when_it_starts_and_then_once_a_while() {
    let limits = Limits::default();
    let mut guard = Guard::default();
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut reports = Vec::new();
    // A flood of one bad signature every 10 ms for 25 minutes.
    for n in 0..150_000u64 {
        let failure = guard.fail(at(n * 10), &limits);
        assert!(!failure.first || n == 0, "the start of a burst is logged once while it lasts ({n})");
        if let Some(count) = failure.report {
            reports.push((n, count));
        }
    }
    // When it starts, then every 10 minutes, each with what came since.
    assert_eq!(reports.len(), 3, "{reports:?}");
    assert_eq!(reports[0], (19, 20));
    assert!(reports[1].0 * 10 >= 600_000 && reports[1].0 * 10 < 601_000, "{reports:?}");
    assert!(reports[1].1 > 50_000, "the real count since the last report, not 20: {reports:?}");
    // Nothing more while it lasts and the window isn't over; once the flood
    // stops and the lockout ends, the rest is reported, so every one is
    // counted, once.
    assert_eq!(guard.flush(at(150_000 * 10), &limits), None);
    let later = at(150_000 * 10 + 120_000);
    let rest = guard.flush(later, &limits).expect("the rest is owed");
    assert_eq!(reports.iter().map(|(_, count)| count).sum::<usize>() + rest, 150_000);
    assert_eq!(guard.flush(later, &limits), None, "reported once");
    // A new lockout after the last one ended is reported at once; failures
    // that didn't make a burst don't count towards it.
    assert!(guard.locked(later).is_none());
    for n in 0..5u64 {
        assert_eq!(guard.fail(later + Duration::from_secs(n), &limits).report, None);
    }
    let again = later + Duration::from_secs(120);
    let reported: Vec<_> = (0..20u64).filter_map(|n| guard.fail(again + Duration::from_millis(n), &limits).report).collect();
    assert_eq!(reported, [20]);
}

#[tokio::test]
async fn writes_are_limited_to_ten_a_minute() {
    let mut h = echo("writes");
    for n in 0..10 {
        assert_eq!(h.send(&Signing::post("/manage/v1/restart", &json!({}))).await.status, StatusCode::OK, "{n}");
    }
    let signing = Signing::post("/manage/v1/restart", &json!({}));
    let answer = h.send(&signing).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
    assert!(answer.signed_for(&signing.nonce));
    assert!(answer.json["retry_after"].as_u64().unwrap() >= 59);
    // Reads aren't writes.
    assert_eq!(h.send(&Signing::get("/manage/v1/state")).await.status, StatusCode::OK);
    assert_eq!(h.ops().len(), 11);
}

#[tokio::test]
async fn sizes_and_counts_are_capped() {
    let mut h = echo("sizes");
    // 16 KiB and one byte more.
    let big = format!(r#"{{"revision":"{}","changes":{{"hub_greeting":"{}"}}}}"#, "0".repeat(32), "x".repeat(MAX_BODY));
    let answer = h.send(&Signing::new("POST", "/manage/v1/settings", &big)).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::PAYLOAD_TOO_LARGE, "too_large"));
    assert!(answer.headers.get(wire::SIGNATURE).is_none(), "refused before the signature is checked");
    // Without a length given, the body is still cut off.
    let signing = Signing::new("POST", "/manage/v1/settings", &big);
    let mut request = signing.request();
    request.headers_mut().remove(header::CONTENT_LENGTH);
    let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(axum::body::Bytes::from(big.clone()))]);
    *request.body_mut() = Body::from_stream(stream);
    assert_eq!(h.call(request).await.status, StatusCode::PAYLOAD_TOO_LARGE);

    let changes = |n: usize| -> Value {
        let map: serde_json::Map<String, Value> = (0..n).map(|i| (format!("key{i}"), json!("v"))).collect();
        json!({ "revision": "0".repeat(32), "changes": map })
    };
    let answer = h.send(&Signing::post("/manage/v1/settings", &changes(33))).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::BAD_REQUEST, "bad_request"));
    assert!(answer.json["message"].as_str().unwrap().contains("32"));
    assert_eq!(h.send(&Signing::post("/manage/v1/settings", &changes(32))).await.status, StatusCode::OK);
    let long = json!({ "revision": "0".repeat(32), "changes": { "hub_greeting": "x".repeat(MAX_VALUE + 1) } });
    assert_eq!(h.send(&Signing::post("/manage/v1/settings", &long)).await.code(), "bad_request");
    let longest = json!({ "revision": "0".repeat(32), "changes": { "hub_greeting": "x".repeat(MAX_VALUE) } });
    assert_eq!(h.send(&Signing::post("/manage/v1/settings", &longest)).await.status, StatusCode::OK);
    assert_eq!(h.ops().len(), 2);
}

#[tokio::test]
async fn bodies_and_queries_of_the_wrong_shape_are_bad_requests() {
    let mut h = harness_with("shapes", Limits { writes_per_minute: 100, ..Limits::default() }, false);
    let revision = "0".repeat(32);
    for (target, body) in [
        ("/manage/v1/settings", "not json".to_string()),
        ("/manage/v1/settings", "{}".to_string()),
        ("/manage/v1/settings", format!(r#"{{"revision":"{revision}","changes":{{"pn_name":5}}}}"#)),
        ("/manage/v1/settings", format!(r#"{{"revision":"{revision}","changes":["pn_name"]}}"#)),
        ("/manage/v1/settings", format!(r#"{{"revision":"{revision}","changes":{{"pn_name":"a","pn_name":"b"}}}}"#)),
        ("/manage/v1/settings", format!(r#"{{"revision":"{revision}","changes":{{}},"extra":1}}"#)),
        ("/manage/v1/settings", r#"{"revision":"ABC","changes":{}}"#.to_string()),
        ("/manage/v1/settings", format!(r#"{{"revision":"{}","changes":{{}}}}"#, "A".repeat(32))),
        ("/manage/v1/restart", "".to_string()),
        ("/manage/v1/restart", r#"{"now":true}"#.to_string()),
        ("/manage/v1/restart", "[]".to_string()),
        ("/manage/v1/rollback", r#"{"id":"0123"}"#.to_string()),
        ("/manage/v1/rollback", r#"{"id":"0123456789ABCDEF"}"#.to_string()),
        ("/manage/v1/rollback", "{}".to_string()),
    ] {
        let answer = h.send(&Signing::new("POST", target, &body)).await;
        assert_eq!((answer.status, answer.code()), (StatusCode::BAD_REQUEST, "bad_request"), "{target} {body}");
        assert!(!answer.json["message"].as_str().unwrap().is_empty());
    }
    // JSON only.
    let signing = Signing::post("/manage/v1/restart", &json!({}));
    let mut request = signing.request();
    request.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    assert_eq!(h.call(request).await.code(), "bad_request");
    let signing = Signing::post("/manage/v1/restart", &json!({}));
    let mut request = signing.request();
    request.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
    assert_eq!(h.call(request).await.status, StatusCode::OK);
    // Queries.
    for target in [
        "/manage/v1/state?x=1",
        "/manage/v1/audit?limit=0",
        "/manage/v1/audit?limit=101",
        "/manage/v1/audit?limit=a",
        "/manage/v1/audit?before=1&before=2",
        "/manage/v1/audit?other=1",
        "/manage/v1/audit?before",
        "/manage/v1/audit?before=-1",
        "/manage/v1/audit?before=99999999999999999999999",
    ] {
        assert_eq!(h.send(&Signing::get(target)).await.code(), "bad_request", "{target}");
    }
    assert_eq!(h.send(&Signing::get("/manage/v1/audit?before=7&limit=100")).await.status, StatusCode::OK);
    assert_eq!(h.send(&Signing::get("/manage/v1/audit")).await.status, StatusCode::OK);
    assert_eq!(h.ops()[1..], [Op::Audit { before: Some(7), limit: 100 }, Op::Audit { before: None, limit: 50 }]);
}

#[tokio::test]
async fn a_setting_name_that_cant_be_one_is_a_bad_request() {
    let mut h = harness_with("names", Limits { writes_per_minute: 100, ..Limits::default() }, false);
    let revision = "0".repeat(32);
    let long = format!("a{}", "b".repeat(64));
    for key in [
        "pn\u{1b}[31mname",
        "pn_name\n",
        "pn_name\r\nfake",
        "pn\0name",
        "\u{202e}pn_name",
        "pn_name\u{200b}",
        "",
        "Pn_name",
        "1pn",
        "_pn",
        "pn-name",
        "pn name",
        "pn.name",
        "pñ",
        long.as_str(),
    ] {
        let body = json!({ "revision": revision, "changes": { "pn_name": "ok", key: "v" } });
        let answer = h.send(&Signing::post("/manage/v1/settings", &body)).await;
        assert_eq!((answer.status, answer.code()), (StatusCode::BAD_REQUEST, "bad_request"), "{key:?}");
        assert!(answer.json["message"].as_str().unwrap().contains("lowercase letter"));
        assert!(answer.json.get("key").is_none(), "the name isn't echoed");
    }
    assert!(h.ops().is_empty());
    // The longest and shortest names that can be one reach the owner loop (which refuses them, not being settings).
    let longest = format!("a{}", "b".repeat(63));
    let body = json!({ "revision": revision, "changes": { longest: "v", "a": "v", "x9_": "v" } });
    assert_eq!(h.send(&Signing::post("/manage/v1/settings", &body)).await.status, StatusCode::OK);
    assert_eq!(h.ops().len(), 1);
}

#[test]
fn connections_are_counted_per_address_and_given_back() {
    let peers = Arc::new(Peers::default());
    let a: IpAddr = "10.0.0.1".parse().unwrap();
    let b: IpAddr = "10.0.0.2".parse().unwrap();
    let mapped = peer_ip("::ffff:10.0.0.1".parse().unwrap());
    assert_eq!(mapped, a, "an IPv4 address written as IPv6 is the same peer");
    assert_eq!(peer_ip("fd00::1".parse().unwrap()), "fd00::1".parse::<IpAddr>().unwrap());
    let first = peers.take(a, 2).unwrap();
    let second = peers.take(mapped, 2).unwrap();
    assert!(peers.take(a, 2).is_none(), "a third from one address");
    let other = peers.take(b, 2).expect("another address has its own places");
    assert_eq!((peers.open(a), peers.open(b)), (2, 1));
    drop(first);
    assert_eq!(peers.open(a), 1);
    let third = peers.take(a, 2).expect("given back when one closes");
    drop((second, third, other));
    assert_eq!((peers.open(a), peers.open(b)), (0, 0));
    assert!(peers.0.lock().unwrap().is_empty(), "nothing is kept for an address with none open");
    assert!(peers.take(a, 0).is_none());
}

#[test]
fn refused_connections_are_logged_once_a_minute() {
    let mut quiet = None;
    let start = Instant::now();
    assert!(log_now(&mut quiet, start));
    assert!(!log_now(&mut quiet, start + Duration::from_secs(1)));
    assert!(!log_now(&mut quiet, start + Duration::from_secs(59)));
    assert!(log_now(&mut quiet, start + Duration::from_secs(60)));
    assert!(!log_now(&mut quiet, start + Duration::from_secs(61)));
}

#[tokio::test]
async fn one_address_can_hold_only_a_few_connections() {
    use tokio::io::AsyncWriteExt;
    let limits = Limits {
        connections: 8,
        connections_per_peer: 2,
        connection_lifetime: Duration::from_secs(10),
        request_within: Duration::from_secs(10),
        ..Limits::default()
    };
    let (bound, task, _jobs) = serve_with(listen_on("127.0.0.1:0", vec![]), limits, None).await.unwrap();
    let hold = || async {
        let mut slow = TcpStream::connect(bound).await.unwrap();
        slow.write_all(b"GET /manage/v1/state HTTP/1.1\r\nHost: x\r\n").await.unwrap();
        slow
    };
    let mut first = hold().await;
    let _second = hold().await;
    // Places are free overall, but this address has its two: a third is closed at once.
    let mut third = TcpStream::connect(bound).await.unwrap();
    let _ = third.write_all(b"GET /nothing HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert_eq!(read_until_closed(&mut third, Duration::from_secs(2)).await, Some(vec![]), "the third wasn't closed unanswered");
    assert!(read_until_closed(&mut first, Duration::from_millis(100)).await.is_none(), "a held one was closed");
    // One closed by the client gives its place back, well before its time is up.
    drop(first);
    let until = Instant::now() + Duration::from_secs(5);
    let answered = loop {
        let mut next = TcpStream::connect(bound).await.unwrap();
        let _ = next.write_all(b"GET /nothing HTTP/1.1\r\nHost: x\r\n\r\n").await;
        let read = read_until_closed(&mut next, Duration::from_secs(2)).await.unwrap_or_default();
        if !read.is_empty() || Instant::now() > until {
            break read;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(String::from_utf8_lossy(&answered).to_lowercase().starts_with("http/1.1 404"), "{answered:?}");
    task.abort();
}

#[tokio::test]
async fn other_paths_and_methods_are_404_and_405() {
    let mut h = echo("paths");
    for target in
        ["/", "/manage/v1", "/manage/v1/state/", "/manage/v2/state", "//manage/v1/state", "/manage/v1/confirm", "/manage/v1/reticulum"]
    {
        let answer = h.call(HttpRequest::builder().uri(target).body(Body::empty()).unwrap()).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{target}");
        assert!(answer.headers.get(wire::SIGNATURE).is_none());
    }
    let answer = h.send(&Signing::new("POST", "/manage/v1/state", "{}")).await;
    assert_eq!((answer.status, answer.headers[header::ALLOW].to_str().unwrap()), (StatusCode::METHOD_NOT_ALLOWED, "GET"));
    let answer = h.send(&Signing::get("/manage/v1/settings")).await;
    assert_eq!((answer.status, answer.headers[header::ALLOW].to_str().unwrap()), (StatusCode::METHOD_NOT_ALLOWED, "POST"));
    assert!(h.ops().is_empty());
}

#[tokio::test]
async fn a_full_queue_or_a_slow_owner_is_said_plainly() {
    let limits = Limits { queue: 1, answer_within: Duration::from_millis(100), ..Limits::default() };
    let (jobs_tx, mut jobs) = mpsc::channel(limits.queue);
    let router = router(secret(), jobs_tx, limits, started(), None);
    // Nobody runs the jobs: the first waits and times out, and stays queued.
    let signing = Signing::get("/manage/v1/state");
    let response = router.clone().oneshot(signing.request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("\"timeout\""));
    let response = router.clone().oneshot(Signing::get("/manage/v1/state").request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("\"queue_full\""));
    assert!(jobs.try_recv().is_ok());
    // The owner loop gone (the node stopping): no queue either.
    drop(jobs);
    let response = router.oneshot(Signing::get("/manage/v1/state").request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn settings_through_the_listener_change_the_node() {
    let mut h = node("settings");
    let state = h.state().await;
    let revision = state["settings_revision"].as_str().unwrap().to_string();
    assert_eq!(state["pending"], Value::Null);
    let change =
        |revision: &str, changes: Value| Signing::post("/manage/v1/settings", &json!({ "revision": revision, "changes": changes }));
    let answer = h.send(&change(&revision, json!({ "pn_name": "Hilltop" }))).await;
    assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
    assert_eq!(answer.json["applied"], json!([{ "key": "pn_name", "from": "", "to": "Hilltop" }]));
    assert_eq!(h.app().settings.pn_name.as_deref(), Some("Hilltop"));
    // The same revision again: someone else changed it meanwhile.
    let answer = h.send(&change(&revision, json!({ "pn_name": "Other" }))).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::CONFLICT, "stale_revision"));
    let revision = answer.json["settings_revision"].as_str().unwrap().to_string();
    // Not on the list, and turn-off-only.
    for (key, value) in [("rns_config", "/tmp"), ("node_executable_pages", "true"), ("node_enabled", "true"), ("voice_recorder", "sh")] {
        let answer = h.send(&change(&revision, json!({ key: value }))).await;
        assert_eq!((answer.status, answer.code(), answer.json["key"].as_str()), (StatusCode::FORBIDDEN, "not_allowed", Some(key)), "{key}");
    }
    let answer = h.send(&change(&revision, json!({ "pn_enabled": "false" }))).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert!(!h.app().settings.pn_enabled);
    // Rolling that back would turn the propagation node on again: refused.
    let id = answer.json["id"].as_str().unwrap().to_string();
    let answer = h.send(&Signing::post("/manage/v1/rollback", &json!({ "id": id }))).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::FORBIDDEN, "not_allowed"));
    assert!(!h.app().saved_settings().unwrap().pn_enabled);
    // The audit log has them all, newest first, a page at a time.
    let answer = h.send(&Signing::get("/manage/v1/audit?limit=2")).await;
    let entries = answer.json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(answer.json["more"], true);
    assert_eq!((entries[0]["op"].as_str(), entries[0]["result"].as_str()), (Some("rollback"), Some("refused")));
    let before = entries[1]["seq"].as_u64().unwrap();
    let older = h.send(&Signing::get(&format!("/manage/v1/audit?before={before}&limit=100"))).await;
    assert_eq!(older.json["more"], false);
    assert_eq!(older.json["entries"].as_array().unwrap().len() as u64, before - 1);
}

#[tokio::test]
async fn a_restart_through_the_listener_is_one_at_a_time() {
    let mut h = node("restart");
    let answer = h.send(&Signing::post("/manage/v1/restart", &json!({}))).await;
    assert_eq!((answer.status, answer.json["state"].as_str()), (StatusCode::ACCEPTED, Some("restarting")));
    assert!(h.app().take_rns_restart());
    let again = h.send(&Signing::post("/manage/v1/restart", &json!({}))).await;
    assert_eq!((again.status, again.code()), (StatusCode::CONFLICT, "restarting"));
    let state = h.state().await;
    assert_eq!(state["restart"]["id"], answer.json["id"]);
    assert!(state["restart_allowed_at"].as_u64().unwrap() > unix_now());
}

#[test]
fn only_private_addresses_are_listened_on() {
    let check = |text: &str| check_bind(&text.parse().unwrap());
    for fine in [
        "127.0.0.1:1",
        "127.1.2.3:1",
        "10.200.0.2:8750",
        "172.16.0.1:1",
        "172.31.255.255:1",
        "192.168.1.1:1",
        "100.64.0.1:1",
        "100.127.255.255:1",
        "[::1]:1",
        "[fd00::1]:1",
        "[fc00::1]:1",
        "[fe80::1]:1",
        "[febf::1]:1",
        "[::ffff:10.0.0.1]:1",
    ] {
        assert_eq!(check(fine).unwrap(), None, "{fine}");
    }
    for everywhere in ["0.0.0.0:8750", "[::]:8750", "[::ffff:0.0.0.0]:1"] {
        assert!(check(everywhere).unwrap().unwrap().contains("firewall"), "{everywhere}");
    }
    for public in [
        "8.8.8.8:1",
        "172.32.0.1:1",
        "100.128.0.1:1",
        "100.63.0.1:1",
        "169.254.0.1:1",
        "192.0.2.1:1",
        "[2001:db8::1]:1",
        "[fec0::1]:1",
        "[::ffff:8.8.8.8]:1",
        "224.0.0.1:1",
    ] {
        let error = check(public).unwrap_err().to_string();
        assert!(error.contains("refuses a public address"), "{public}: {error}");
    }
}

#[tokio::test]
async fn serving_refuses_bad_and_public_addresses() {
    let dir = test_dir("serve-refuses");
    let paths = crate::config::Paths::new(Some(dir.join("data"))).unwrap();
    let listen = |address: &str| listen_on(address, vec![]);
    let error = serve(listen("not an address"), &paths).await.unwrap_err().to_string();
    assert!(error.contains("--manage-listen expects"), "{error}");
    assert!(serve(listen("8.8.8.8:8750"), &paths).await.unwrap_err().to_string().contains("public"));
    // A port taken already.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();
    assert!(format!("{:#}", serve(listen(&address), &paths).await.unwrap_err()).contains("could not listen"));
    // Debug never shows the secret.
    let shown = format!("{:?}", listen("127.0.0.1:0"));
    assert!(shown.contains("Secret(..)") && !shown.contains(SECRET), "{shown}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn listen_on(address: &str, allow: Vec<Network>) -> Listen {
    Listen { address: address.into(), secret: secret(), allow }
}

#[test]
fn the_secret_file_is_read_trimmed_and_never_repeated() {
    let dir = test_dir("secret-file");
    let file = dir.join("secret");
    let write = |bytes: &[u8]| crate::config::write_private(&file, bytes).unwrap();
    write(format!("  {SECRET}\n").as_bytes());
    assert!(load_secret(&file).unwrap().is_in(SECRET));
    for (text, why) in [("short-secret-value", "at least 32"), (&*format!("{SECRET} with spaces"), "no spaces")] {
        write(text.as_bytes());
        let error = format!("{:#}", load_secret(&file).unwrap_err());
        assert!(error.contains("--manage-secret-file") && error.contains(why), "{error}");
        assert!(!error.contains(text.trim()), "{error}");
    }
    // Too big to be a secret: refused without reading it all.
    write("a".repeat(10_000).as_bytes());
    assert!(format!("{:#}", load_secret(&file).unwrap_err()).contains("at most 256"));
    write(&[0xff, 0xfe, 0x00]);
    assert!(format!("{:#}", load_secret(&file).unwrap_err()).contains("could not read the manage secret file"));
    let error = format!("{:#}", load_secret(&dir.join("missing")).unwrap_err());
    assert!(error.contains("could not read the manage secret file"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_secret_file_others_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = test_dir("secret-mode");
    let file = dir.join("secret");
    crate::config::write_private(&file, SECRET.as_bytes()).unwrap();
    // Read or written by its group or others: whoever writes it chooses the secret.
    for mode in [0o640, 0o604, 0o644, 0o444, 0o660, 0o620, 0o602, 0o622] {
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        let error = format!("{:#}", load_secret(&file).unwrap_err());
        assert!(error.contains("can be read or changed by other users") && error.contains("chmod 600"), "{mode:o}: {error}");
        assert!(!error.contains(SECRET), "{error}");
    }
    for mode in [0o600, 0o400, 0o700] {
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(load_secret(&file).unwrap().is_in(SECRET), "{mode:o}");
    }
    // Through a link, it is the file itself that is checked.
    let link = dir.join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(load_secret(&link).is_ok());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(format!("{:#}", load_secret(&link).unwrap_err()).contains("chmod 600"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Read from `stream` until it closes; what was read, or None if it is
/// still open after `wait`.
async fn read_until_closed(stream: &mut TcpStream, wait: Duration) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut read = Vec::new();
    tokio::time::timeout(wait, async {
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => read.extend_from_slice(&buffer[..n]),
            }
        }
    })
    .await
    .ok()?;
    Some(read)
}

#[tokio::test]
async fn a_slow_or_idle_connection_is_closed_and_connections_are_capped() {
    use tokio::io::AsyncWriteExt;
    let limits = Limits { connections: 1, connection_lifetime: Duration::from_millis(300), ..Limits::default() };
    let (bound, task, _jobs) = serve_with(listen_on("127.0.0.1:0", vec![]), limits, None).await.unwrap();
    // Headers that never end: closed when the connection's time is up.
    let mut slow = TcpStream::connect(bound).await.unwrap();
    slow.write_all(b"GET /manage/v1/state HTTP/1.1\r\nHost: x\r\n").await.unwrap();
    // While it holds the one place, another connection is closed at once.
    let mut other = TcpStream::connect(bound).await.unwrap();
    let _ = other.write_all(b"GET /manage/v1/state HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert_eq!(read_until_closed(&mut other, Duration::from_millis(200)).await, Some(vec![]), "the second wasn't closed");
    assert!(read_until_closed(&mut slow, Duration::from_secs(5)).await.is_some(), "the slow connection is still open");
    // Its place is free again: a whole request is answered (unsigned here) and the connection closed.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut next = TcpStream::connect(bound).await.unwrap();
    next.write_all(b"GET /nothing HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    let answer = read_until_closed(&mut next, Duration::from_secs(5)).await.expect("the answer didn't close the connection");
    let text = String::from_utf8_lossy(&answer).to_lowercase();
    assert!(text.starts_with("http/1.1 404") && text.contains("connection: close") && text.contains("cache-control: no-store"), "{text}");
    task.abort();
}

#[tokio::test]
async fn the_state_over_the_router_carries_the_redacted_reticulum_config_and_is_signed() {
    let mut h = node("reticulum-config");
    let rns = h.dir.join("rns");
    std::fs::create_dir_all(&rns).unwrap();
    let secrets = ["fake-passphrase-1", "fake-network-name-2", "/dev/fake-port-3", "fake-password-4", "fake-ifac-5", "fake-command-6"];
    let config = format!(
        "[reticulum]\n enable_transport = yes\n[interfaces]\n [[Radio]]\n  type = RNodeInterface\n  port = {}\n  passphrase = {}\n  network_name = {}\n  password = {}\n  ifac_netkey = {}\n [[Shell]]\n  type = PipeInterface\n  command = {}\n",
        secrets[2], secrets[0], secrets[1], secrets[3], secrets[4], secrets[5]
    );
    std::fs::write(rns.join("config"), config).unwrap();
    h.app().settings.rns_config = Some(rns.display().to_string());
    let signing = Signing::get("/manage/v1/state");
    let answer = h.send(&signing).await;
    assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
    assert!(answer.signed_for(&signing.nonce));
    let text = String::from_utf8_lossy(&answer.bytes).into_owned();
    for fake in secrets {
        assert!(!text.contains(fake), "{fake} leaked: {text}");
    }
    assert!(!text.contains("\"command\""), "a pipe's other keys are not even named: {text}");
    let block = &answer.json["reticulum"];
    assert_eq!(block["revision"].as_str().map(str::len), Some(32));
    assert_eq!(block["shared_instance"], false);
    let sections = block["sections"].as_array().unwrap();
    let shell = sections.iter().find(|s| s["path"] == json!(["interfaces", "Shell"])).unwrap();
    assert_eq!(shell["keys"], json!([{ "key": "type", "value": "PipeInterface", "set": true }]));
    let radio = sections.iter().find(|s| s["path"] == json!(["interfaces", "Radio"])).unwrap();
    assert!(radio["keys"].as_array().unwrap().contains(&json!({ "key": "passphrase", "value": null, "set": true })));
    // Another read gives the same revision.
    assert_eq!(h.state().await["reticulum"]["revision"], block["revision"]);
}

#[tokio::test]
async fn a_huge_reticulum_config_keeps_the_state_small() {
    let mut h = node("reticulum-huge");
    let rns = h.dir.join("rns");
    std::fs::create_dir_all(&rns).unwrap();
    let mut config = String::from("[interfaces]\n");
    for i in 0..128 {
        config.push_str(&format!("[[Interface {i:03}]]\ntype = TCPClientInterface\n"));
        for k in 0..64 {
            config.push_str(&format!("setting_{k:02} = {}\n", "v".repeat(100)));
        }
    }
    std::fs::write(rns.join("config"), config).unwrap();
    h.app().settings.rns_config = Some(rns.display().to_string());
    let signing = Signing::get("/manage/v1/state");
    let answer = h.send(&signing).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert!(answer.bytes.len() < 64 * 1024, "{}", answer.bytes.len());
    assert_eq!(answer.json["reticulum"]["truncated"], true);
}

#[tokio::test]
async fn a_request_dated_ahead_cant_be_replayed_after_a_restart() {
    // The pentest's: a request signed 280 seconds ahead of the node's clock,
    // accepted, the node restarted, and the same request sent again.
    let keep = test_dir("mark");
    let mark = keep.join("manage").join(MARK_FILE);
    let now = unix_now();
    let mut first_run = harness_marked("mark-run1", Limits::default(), false, now - 100, Some(mark.clone()));
    let mut ahead = Signing::post("/manage/v1/restart", &json!({}));
    ahead.timestamp = now + 280;
    assert_eq!(first_run.send(&ahead).await.status, StatusCode::OK);
    assert_eq!(std::fs::read_to_string(&mark).unwrap(), format!("{}\n", now + 280), "kept before it was carried out");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&mark).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // An older one doesn't move it back.
    assert_eq!(first_run.send(&Signing::get("/manage/v1/state")).await.status, StatusCode::OK);
    assert_eq!(std::fs::read_to_string(&mark).unwrap().trim(), (now + 280).to_string());
    drop(first_run);

    // The next run starts a few seconds later, before that time.
    let mut second_run = harness_marked("mark-run2", Limits::default(), false, now + 5, Some(mark.clone()));
    let answer = second_run.send(&ahead).await;
    assert_eq!((answer.status, answer.code()), (StatusCode::UNAUTHORIZED, "before_start"), "{answer:?}");
    assert_eq!((answer.json["started_at"].as_u64(), answer.json["signed_after"].as_u64()), (Some(now + 5), Some(now + 280)));
    // Without the mark, the start alone wouldn't have held it back.
    let mut unmarked = harness_started("mark-none", Limits::default(), false, now + 5);
    assert_eq!(unmarked.send(&ahead).await.status, StatusCode::OK);
    drop(unmarked);
    // Signed at that time exactly is refused too; signed after it, taken.
    let mut same = Signing::get("/manage/v1/state");
    same.timestamp = now + 280;
    assert_eq!(second_run.send(&same).await.code(), "before_start");
    let mut later = Signing::get("/manage/v1/state");
    later.timestamp = now + 281;
    assert_eq!(second_run.send(&later).await.status, StatusCode::OK);
    assert_eq!(second_run.ops(), [Op::State], "the replay did nothing");
    drop(second_run);
    let _ = std::fs::remove_dir_all(&keep);
}

#[test]
fn the_kept_time_is_read_safely() {
    let dir = test_dir("mark-read");
    let path = dir.join(MARK_FILE);
    assert_eq!(load_mark(&path), 0, "none kept yet");
    for (text, read) in [("1800000000\n", 1_800_000_000), (" 42 ", 42)] {
        std::fs::write(&path, text).unwrap();
        assert_eq!(load_mark(&path), read, "{text:?}");
    }
    for bad in ["", "abc", "-5", "1e9", "99999999999999999999999", "12 34", "\u{0}"] {
        std::fs::write(&path, bad).unwrap();
        assert_eq!(load_mark(&path), 0, "{bad:?}");
    }
    std::fs::write(&path, "1".repeat(1000)).unwrap();
    assert_eq!(load_mark(&path), 0, "too long to be one");
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    assert_eq!(load_mark(&path), 0, "not text");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    assert_eq!(load_mark(&path), 0, "a folder");
    // A time from the past changes nothing; one far ahead (a clock that was
    // wrong) holds requests back for five minutes at most.
    assert_eq!(floor_of(1000, 0), 1000);
    assert_eq!(floor_of(1000, 900), 1000);
    assert_eq!(floor_of(1000, 1200), 1200);
    assert_eq!(floor_of(1000, u64::MAX), 1000 + MAX_SKEW);
    assert_eq!(floor_of(u64::MAX, u64::MAX), u64::MAX);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_mark_that_cant_be_kept_doesnt_stop_management() {
    let dir = test_dir("mark-unsaved");
    // Its folder is a file, so it can't be made.
    std::fs::write(dir.join("manage"), "x").unwrap();
    let mut h = harness_marked("mark-unsaved-h", Limits::default(), false, started(), Some(dir.join("manage").join(MARK_FILE)));
    assert_eq!(h.send(&Signing::get("/manage/v1/state")).await.status, StatusCode::OK);
    assert_eq!(h.send(&Signing::get("/manage/v1/state")).await.status, StatusCode::OK);
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allowed_addresses_and_networks_are_read_and_matched() {
    let net = |text: &str| Network::parse(text).unwrap();
    let ip = |text: &str| text.parse::<IpAddr>().unwrap();
    let website = net("10.0.0.5");
    assert_eq!(website.to_string(), "10.0.0.5/32");
    assert!(website.contains(ip("10.0.0.5")) && website.contains(ip("::ffff:10.0.0.5")));
    assert!(!website.contains(ip("10.0.0.6")) && !website.contains(ip("::1")));
    let lan = net("10.0.0.77/24");
    assert_eq!(lan.to_string(), "10.0.0.0/24", "bits after the prefix are ignored");
    assert!(lan.contains(ip("10.0.0.1")) && lan.contains(ip("10.0.0.255")) && !lan.contains(ip("10.0.1.1")));
    let v6 = net("fd00:1:2:3::9/64");
    assert_eq!(v6.to_string(), "fd00:1:2:3::/64");
    assert!(v6.contains(ip("fd00:1:2:3:ffff::1")) && !v6.contains(ip("fd00:1:2:4::1")) && !v6.contains(ip("10.0.0.1")));
    assert_eq!(net("::ffff:10.0.0.0/120"), net("10.0.0.0/24"), "IPv4 written as IPv6");
    assert_eq!(net("::ffff:10.0.0.5"), website);
    assert!(net("0.0.0.0/0").contains(ip("8.8.8.8")) && !net("0.0.0.0/0").contains(ip("::1")));
    assert!(net("::/0").contains(ip("2001:db8::1")));
    assert!(net("192.168.1.1/32").contains(ip("192.168.1.1")) && net("::1/128").contains(ip("::1")));
    for bad in [
        "",
        "website.example",
        "10.0.0.1/",
        "10.0.0.1/33",
        "fd00::/129",
        "10.0.0.1/-1",
        "10.0.0.1/+8",
        "10.0.0.1/0x8",
        "10.0.0.1/24/8",
        "10.0.0.1:8750",
        "[fd00::1]",
        " 10.0.0.1",
        "::ffff:10.0.0.0/95",
        "10.0.0.1/0008",
    ] {
        let error = Network::parse(bad).unwrap_err();
        assert!(error.contains("10.0.0.0/24"), "{bad:?}: {error}");
    }
    assert_eq!(
        started_line("127.0.0.1:8750".parse().unwrap(), &[]),
        "Remote management at http://127.0.0.1:8750/manage/v1/ (signed requests only)"
    );
    assert!(started_line("127.0.0.1:8750".parse().unwrap(), &[website, v6]).ends_with("from 10.0.0.5/32, fd00:1:2:3::/64 only)"));
}

/// Send `bytes` on a new connection to `bound`; what came back before it
/// closed (None if still open after `wait`).
async fn exchange(bound: SocketAddr, bytes: &[u8], wait: Duration) -> Option<Vec<u8>> {
    use tokio::io::AsyncWriteExt;
    let mut stream = TcpStream::connect(bound).await.ok()?;
    let _ = stream.write_all(bytes).await;
    read_until_closed(&mut stream, wait).await
}

#[tokio::test]
async fn a_connection_from_an_address_not_allowed_is_closed_unread() {
    // Only 10.9.9.9 may connect: this test, on loopback, may not.
    let limits = Limits { connections: 1, connections_per_peer: 1, ..Limits::default() };
    let (bound, task, _jobs) = serve_with(listen_on("127.0.0.1:0", vec![Network::parse("10.9.9.9").unwrap()]), limits, None).await.unwrap();
    for _ in 0..5 {
        let read = exchange(bound, b"GET /nothing HTTP/1.1\r\nHost: x\r\n\r\n", Duration::from_secs(2)).await;
        assert_eq!(read, Some(vec![]), "answered, or left open");
    }
    task.abort();
    // Allowed, the same request is answered; and the places weren't taken by the refused ones.
    let (bound, task, _jobs) =
        serve_with(listen_on("127.0.0.1:0", vec![Network::parse("127.0.0.0/8").unwrap()]), limits, None).await.unwrap();
    for _ in 0..3 {
        let read = exchange(bound, b"GET /nothing HTTP/1.1\r\nHost: x\r\n\r\n", Duration::from_secs(5)).await.unwrap();
        assert!(String::from_utf8_lossy(&read).to_lowercase().starts_with("http/1.1 404"), "{read:?}");
    }
    task.abort();
}

#[tokio::test]
async fn idle_and_half_sent_requests_are_closed_after_a_few_seconds() {
    // The pentest's: every place held by connections that send nothing or
    // half a request, renewed as they close. Each is closed once its
    // request is late, long before the connection's own time is up.
    use tokio::io::AsyncWriteExt;
    let limits = Limits {
        connections: 4,
        connections_per_peer: 4,
        connection_lifetime: Duration::from_secs(30),
        request_within: Duration::from_millis(400),
        ..Limits::default()
    };
    let (bound, task, mut jobs) = serve_with(listen_on("127.0.0.1:0", vec![]), limits, None).await.unwrap();
    let started = Instant::now();
    let mut held = Vec::new();
    for send in [
        &b""[..],
        b"",
        b"GET /manage/v1/state HTTP/1.1\r\nHost: x\r\n",
        b"POST /manage/v1/restart HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n{",
    ] {
        let mut stream = TcpStream::connect(bound).await.unwrap();
        stream.write_all(send).await.unwrap();
        held.push(stream);
    }
    let mut answers = Vec::new();
    for stream in &mut held {
        answers.push(read_until_closed(stream, Duration::from_secs(5)).await.expect("still open after 5 seconds"));
    }
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    // The one whose body was late is told so; the rest are just closed.
    let late = String::from_utf8_lossy(&answers[3]).to_lowercase();
    assert!(late.starts_with("http/1.1 408") && late.contains("request_timeout"), "{late}");
    assert!(answers[..3].iter().all(|a| a.is_empty() || String::from_utf8_lossy(a).to_lowercase().starts_with("http/1.1 408")));
    // The places are free: a signed request is answered, though its
    // answer takes longer than the request had to arrive. (Signed a little
    // after the listener started, as a request signed then is refused.)
    let mut signing = Signing::get("/manage/v1/state");
    signing.timestamp += 2;
    let request = format!(
        "GET /manage/v1/state HTTP/1.1\r\nHost: x\r\n{}: {}\r\n{}: {}\r\n{}: alice\r\n{}: {}\r\n\r\n",
        wire::TIMESTAMP,
        signing.timestamp,
        wire::NONCE,
        signing.nonce,
        wire::ACTOR,
        wire::SIGNATURE,
        signing.signature()
    );
    let owner = tokio::spawn(async move {
        let job = jobs.recv().await.unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = job.reply.send(Reply::new(StatusCode::OK, json!({ "ok": true })));
        jobs
    });
    let read = exchange(bound, request.as_bytes(), Duration::from_secs(5)).await.unwrap();
    let text = String::from_utf8_lossy(&read).to_lowercase();
    assert!(text.starts_with("http/1.1 200") && text.contains(r#"{"ok":true}"#), "{text}");
    let _ = owner.await;
    task.abort();
}

#[tokio::test]
async fn http2_is_not_spoken() {
    // The pentest's: `curl --http2-prior-knowledge` got an HTTP/2 answer,
    // which carries many requests on one connection.
    let (bound, task, _jobs) = serve_with(listen_on("127.0.0.1:0", vec![]), Limits::default(), None).await.unwrap();
    // The HTTP/2 preface, then an empty SETTINGS frame.
    let mut preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
    preface.extend_from_slice(&[0, 0, 0, 4, 0, 0, 0, 0, 0]);
    let read = exchange(bound, &preface, Duration::from_secs(5)).await.expect("left open");
    // An HTTP/2 server answers with its own SETTINGS frame (type 4); this one
    // answers as HTTP/1.1, or not at all.
    assert!(read.is_empty() || read.starts_with(b"HTTP/1.1 "), "{read:?}");
    assert!(!(read.len() >= 9 && read[3] == 4), "{read:?}");
    // An upgrade to h2c is not taken either.
    let read = exchange(
        bound,
        b"GET /nothing HTTP/1.1\r\nHost: x\r\nConnection: Upgrade, HTTP2-Settings\r\nUpgrade: h2c\r\nHTTP2-Settings: AAMAAABkAAQCAAAAAAIAAAAA\r\n\r\n",
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let text = String::from_utf8_lossy(&read).to_lowercase();
    assert!(text.starts_with("http/1.1 404") && !text.contains("101 switching"), "{text}");
    task.abort();
}

#[tokio::test]
async fn an_audit_page_fits_what_a_website_takes() {
    // The review's: long greetings saved a few times made a default page
    // of 50 bigger than the 64 KiB a website takes.
    let mut h = harness_with("audit-budget", Limits { writes_per_minute: 1000, ..Limits::default() }, true);
    for n in 0..12 {
        let state = h.state().await;
        let revision = state["settings_revision"].as_str().unwrap().to_string();
        let greeting = format!("{n}{}", "g".repeat(4000));
        let peers: Vec<String> = (0..100).map(|i| format!("{:032x}", n * 1000 + i)).collect();
        let changes = json!({ "hub_greeting": greeting, "pn_static_peers": peers.join(",") });
        let answer = h.send(&Signing::post("/manage/v1/settings", &json!({ "revision": revision, "changes": changes }))).await;
        assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
    }
    let answer = h.send(&Signing::get("/manage/v1/audit")).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert!(answer.bytes.len() < 48 * 1024 + 64, "{}", answer.bytes.len());
    let entries = answer.json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 12);
    assert_eq!(answer.json["more"], false);
    assert!(entries[0]["changes"][0]["to"].as_str().unwrap().ends_with('…'));
    // A rollback still has the whole value to go back to.
    let id = entries[0]["id"].as_str().unwrap();
    let answer = h.send(&Signing::post("/manage/v1/rollback", &json!({ "id": id }))).await;
    assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
    let greeting = h.app().settings.hub_greeting.clone().unwrap();
    assert!(greeting.starts_with("10") && greeting.len() == 4002, "{}", greeting.len());
}

#[tokio::test]
async fn bad_signatures_owed_when_a_flood_stops_are_reported_with_their_real_count() {
    let limits = Limits { failures: 3, lockout: Duration::from_millis(100), ..Limits::default() };
    let (jobs, mut received) = mpsc::channel(limits.queue);
    let shared = Shared::new(secret(), jobs, limits, started(), None);
    for _ in 0..10 {
        let _ = admit(&shared, None, false);
    }
    let first = received.try_recv().expect("the lockout is reported when it starts");
    assert_eq!(first.op, Op::AuthFailures { count: 3 });
    assert!(received.try_recv().is_err(), "not again for each burst");
    // Nothing is owed while it lasts; once it has ended, the other 7 are.
    shared.flush_failures();
    assert!(received.try_recv().is_err());
    tokio::time::sleep(Duration::from_millis(150)).await;
    shared.flush_failures();
    assert_eq!(received.try_recv().expect("the rest").op, Op::AuthFailures { count: 7 });
    shared.flush_failures();
    assert!(received.try_recv().is_err(), "once");
}

#[tokio::test]
async fn the_listener_reports_what_is_owed_by_itself() {
    // With nobody sending anything more, the listener's own timer reports
    // the rest of a flood once its lockout has ended.
    let limits = Limits { failures: 2, lockout: Duration::from_millis(50), ..Limits::default() };
    let (bound, task, mut jobs) = serve_with(listen_on("127.0.0.1:0", vec![]), limits, None).await.unwrap();
    for _ in 0..5 {
        exchange(bound, b"GET /manage/v1/state HTTP/1.1\r\nHost: x\r\n\r\n", Duration::from_secs(5)).await.unwrap();
    }
    let mut counts = Vec::new();
    let until = Instant::now() + Duration::from_secs(12);
    while counts.iter().sum::<usize>() < 5 && Instant::now() < until {
        if let Ok(Some(job)) = tokio::time::timeout(Duration::from_millis(500), jobs.recv()).await
            && let Op::AuthFailures { count } = job.op
        {
            counts.push(count);
        }
    }
    assert_eq!((counts.first(), counts.iter().sum::<usize>()), (Some(&2), 5), "{counts:?}");
    task.abort();
}

#[test]
fn starting_without_manage_allow_warns() {
    let warning = allow_warning(&[]).unwrap();
    assert!(
        warning.contains("no --manage-allow") && warning.contains("website's address") && warning.contains("connection places"),
        "{warning}"
    );
    assert_eq!(allow_warning(&[Network::parse("10.0.0.1").unwrap()]), None);
}
