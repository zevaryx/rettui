//! The opt-in public stats endpoint (`--stats-listen`): a read-only JSON
//! summary of this node, with counts only. It reads a snapshot the web UI's
//! owner loop publishes about once a second, so it never touches the app and
//! a slow or hostile visitor can't hold the owner up.

use std::net::SocketAddr;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::Router;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::app::App;
use crate::net::PeerKind;

/// The most interfaces listed (the totals still count them all).
const MAX_LISTED: usize = 32;

/// What the endpoint answers with: the JSON, as published last.
pub type Snapshot = watch::Receiver<String>;

/// The JSON for `app`, as of now. Counts only: no addresses, hashes, names
/// of peers or clients.
pub fn snapshot(app: &App, started: Instant) -> String {
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
    let list: Vec<Value> = app
        .interfaces
        .iter()
        .take(MAX_LISTED)
        .map(|i| json!({ "name": i.name, "online": i.online, "clients": i.clients, "rx_bytes": i.rx_bytes, "tx_bytes": i.tx_bytes }))
        .collect();
    json!({
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
            "list": list,
        },
        "peers_heard": { "total": lxmf + nomad + propagation, "lxmf": lxmf, "nomad": nomad, "propagation": propagation },
        "announces_last_minute": app.announce_log.last_minute(),
    })
    .to_string()
}

/// Answer `GET` and `HEAD` for `/` and `/stats.json` from the snapshot.
fn router(snapshot: Snapshot) -> Router {
    Router::new().fallback(move |method: Method, uri: axum::http::Uri| {
        let body = snapshot.borrow().clone();
        async move { answer(&method, uri.path(), body) }
    })
}

fn answer(method: &Method, path: &str, body: String) -> Response {
    if path != "/" && path != "/stats.json" {
        return (StatusCode::NOT_FOUND, "not found\n").into_response();
    }
    if body.is_empty() {
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
    let mut response = if method == Method::HEAD { Response::new(axum::body::Body::empty()) } else { body.clone().into_response() };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if method == Method::HEAD {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    }
    response
}

/// Listen on `address`, and serve until the task is stopped. Fails now if
/// the address can't be used, so rettui doesn't start without what was asked.
pub async fn serve(address: &str, snapshot: Snapshot) -> Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let address: SocketAddr =
        address.parse().with_context(|| format!("--stats-listen expects an address like 127.0.0.1:8741, not {address}"))?;
    let listener = tokio::net::TcpListener::bind(address).await.with_context(|| format!("could not listen for stats on {address}"))?;
    let bound = listener.local_addr()?;
    let app = router(snapshot);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((bound, task))
}

/// A channel for the snapshot, starting as `initial`.
pub fn channel(initial: String) -> (watch::Sender<String>, Snapshot) {
    watch::channel(initial)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;
    use crate::net::InterfaceInfo;
    use crate::store::{Peer, Store};

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
        let json: Value = serde_json::from_str(&snapshot(&app, Instant::now())).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["rettui_version"], env!("CARGO_PKG_VERSION"));
        assert!(json["generated_at"].as_u64().unwrap() > 1_700_000_000);
        assert_eq!(json["uptime_seconds"], 0);
        assert_eq!(json["interfaces"]["total"], 3);
        assert_eq!(json["interfaces"]["online"], 2);
        assert_eq!(json["interfaces"]["clients"], 7);
        assert_eq!(json["interfaces"]["rx_bytes"], 11);
        assert_eq!(json["interfaces"]["tx_bytes"], 22);
        let list = json["interfaces"]["list"].as_array().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], json!({ "name": "Public entry point", "online": true, "clients": 3, "rx_bytes": 10, "tx_bytes": 20 }));
        assert_eq!(list[1]["clients"], Value::Null);
        assert_eq!(json["peers_heard"], json!({ "total": 4, "lxmf": 2, "nomad": 1, "propagation": 1 }));
        assert_eq!(json["announces_last_minute"], 0);
        let text = json.to_string();
        assert!(!text.contains("Secret Name") && !text.contains("0000000000000000000000000000000"), "{text}");
        let keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 7, "{keys:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_interface_reports_clients_as_null_and_the_list_is_capped() {
        let (mut app, dir) = app("capped");
        app.interfaces = (0..40).map(|i| InterfaceInfo { name: format!("i{i}"), online: true, ..Default::default() }).collect();
        let json: Value = serde_json::from_str(&snapshot(&app, Instant::now())).unwrap();
        assert_eq!(json["interfaces"]["clients"], Value::Null);
        assert_eq!(json["interfaces"]["total"], 40);
        assert_eq!(json["interfaces"]["list"].as_array().unwrap().len(), MAX_LISTED);
        let _ = std::fs::remove_dir_all(dir);
    }

    async fn ask(router: &Router, method: &str, path: &str) -> Response {
        router.clone().oneshot(Request::builder().method(method).uri(path).body(Body::empty()).unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn the_router_answers_get_and_head_and_refuses_the_rest() {
        let (app, dir) = app("router");
        let (_tx, rx) = channel(snapshot(&app, Instant::now()));
        let router = router(rx);
        for path in ["/stats.json", "/"] {
            let response = ask(&router, "GET", path).await;
            assert_eq!(response.status(), StatusCode::OK);
            let headers = response.headers();
            assert_eq!(headers[header::CONTENT_TYPE], "application/json");
            assert_eq!(headers[header::CACHE_CONTROL], "no-store");
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
        let (_tx, rx) = channel(String::new());
        assert_eq!(ask(&router(rx), "GET", "/stats.json").await.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_new_snapshot_is_what_the_next_visitor_gets() {
        let (tx, rx) = channel("{\"a\":1}".into());
        let router = router(rx);
        tx.send_replace("{\"a\":2}".into());
        let body = axum::body::to_bytes(ask(&router, "GET", "/").await.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"{\"a\":2}");
    }

    #[tokio::test]
    async fn serving_fails_on_a_bad_or_taken_address_and_answers_over_tcp() {
        let (_tx, rx) = channel("{}".into());
        let error = serve("not an address", rx.clone()).await.unwrap_err();
        assert!(format!("{error:#}").contains("--stats-listen expects"), "{error:#}");
        let holder = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let taken = holder.local_addr().unwrap().to_string();
        let error = serve(&taken, rx.clone()).await.unwrap_err();
        assert!(format!("{error:#}").contains("could not listen for stats"), "{error:#}");
        let (bound, task) = serve("127.0.0.1:0", rx).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(bound).await.unwrap();
        stream.write_all(b"GET /stats.json HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).await.unwrap();
        assert!(text.starts_with("HTTP/1.1 200") && text.ends_with("{}"), "{text}");
        task.abort();
    }
}
