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
    let dir = test_dir(name);
    let (jobs_tx, jobs) = mpsc::channel(limits.queue);
    let router = router(secret(), jobs_tx, limits, started_at);
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
    assert!(again.signed_for(&signing.nonce));
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
    // The flood is still counted while it lasts: 18 more make another burst.
    for _ in 0..18 {
        wrong.nonce = fresh_nonce();
        assert_eq!(h.send(&wrong).await.status, StatusCode::TOO_MANY_REQUESTS);
    }
    assert_eq!(h.ops().len(), 3);
    assert_eq!(h.ops()[2], Op::AuthFailures { count: 20 });
    assert_eq!(h.send(&Signing::get("/manage/v1/state")).await.status, StatusCode::OK);
}

#[test]
fn a_lockout_turns_away_unsigned_requests_only() {
    let limits = Limits { failures: 2, ..Limits::default() };
    let (jobs, _received) = mpsc::channel(limits.queue);
    let shared = Shared { secret: secret(), jobs, guard: Mutex::new(Guard::default()), limits, started_at: started() };
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
        let (first, locked) = guard.fail(start + Duration::from_secs(n * 4), &limits);
        assert!(!locked, "{n}");
        assert_eq!(first, n == 0, "only the start of a burst is logged ({n})");
    }
    let mut guard = Guard::default();
    for n in 0..19 {
        assert!(!guard.fail(start, &limits).1, "{n}");
    }
    assert!(guard.fail(start, &limits).1);
    assert_eq!(guard.locked(start + Duration::from_secs(59)), Some(Duration::from_secs(1)));
    assert_eq!(guard.locked(start + Duration::from_secs(60)), None);
    assert_eq!(guard.locked(start + Duration::from_secs(61)), None);
    assert_eq!((seconds(Duration::ZERO), seconds(Duration::from_millis(1)), seconds(Duration::from_secs(60))), (1, 1, 60));
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
    let limits = Limits { connections: 8, connections_per_peer: 2, connection_lifetime: Duration::from_secs(10), ..Limits::default() };
    let (bound, task, _jobs) = serve_with(Listen { address: "127.0.0.1:0".into(), secret: secret() }, limits).await.unwrap();
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
    let router = router(secret(), jobs_tx, limits, started());
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
    let listen = |address: &str| Listen { address: address.into(), secret: secret() };
    let error = serve(listen("not an address")).await.unwrap_err().to_string();
    assert!(error.contains("--manage-listen expects"), "{error}");
    assert!(serve(listen("8.8.8.8:8750")).await.unwrap_err().to_string().contains("public"));
    // A port taken already.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();
    assert!(format!("{:#}", serve(listen(&address)).await.unwrap_err()).contains("could not listen"));
    // Debug never shows the secret.
    let shown = format!("{:?}", listen("127.0.0.1:0"));
    assert!(shown.contains("Secret(..)") && !shown.contains(SECRET), "{shown}");
}

#[test]
fn the_secret_file_is_read_trimmed_and_never_repeated() {
    let dir = test_dir("secret-file");
    let file = dir.join("secret");
    std::fs::write(&file, format!("  {SECRET}\n")).unwrap();
    assert!(load_secret(&file).unwrap().is_in(SECRET));
    for (text, why) in [("short-secret-value", "at least 32"), (&*format!("{SECRET} with spaces"), "no spaces")] {
        std::fs::write(&file, text).unwrap();
        let error = format!("{:#}", load_secret(&file).unwrap_err());
        assert!(error.contains("--manage-secret-file") && error.contains(why), "{error}");
        assert!(!error.contains(text.trim()), "{error}");
    }
    // Too big to be a secret: refused without reading it all.
    std::fs::write(&file, "a".repeat(10_000)).unwrap();
    assert!(format!("{:#}", load_secret(&file).unwrap_err()).contains("at most 256"));
    std::fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
    assert!(format!("{:#}", load_secret(&file).unwrap_err()).contains("could not read the manage secret file"));
    let error = format!("{:#}", load_secret(&dir.join("missing")).unwrap_err());
    assert!(error.contains("could not read the manage secret file"), "{error}");
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
    let (bound, task, _jobs) = serve_with(Listen { address: "127.0.0.1:0".into(), secret: secret() }, limits).await.unwrap();
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
