//! The web UI's HTTP API, login and static files.
//!
//! Logging in: `rettui --web` prints a link with `?token=…`. Opening it sets
//! an HttpOnly, SameSite=Strict cookie holding the token; every other
//! request needs that cookie. SameSite also keeps other sites from sending
//! requests on the user's behalf.
//!
//! Scripts and reverse proxies can send the token in a header instead:
//! `Authorization: Bearer <token>` or `X-Rettui-Token: <token>`. Other sites
//! cannot add these headers to requests (there is no CORS), so they are as
//! safe from cross-site use as the cookie.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::PathBuf;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Fetched, WebState, views};
use crate::app::files::unique_path;
use crate::app::{Location, resolve_url};
use crate::lxmf::DeliveryMode;
use crate::net::{Hash, NetCommand, parse_hash};
use crate::nomad::micron::{self, html};
use crate::rrc;
use crate::store::{Bookmark, NotifyLevel};

const COOKIE: &str = "rettui_token";
/// Header for clients that already use `Authorization` for something else.
const TOKEN_HEADER: &str = "x-rettui-token";
/// Largest request (base64 attachments included).
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// Scripts only from this server; page content is inert HTML. Inline style
/// attributes carry Micron colours.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, axum::Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self(StatusCode::BAD_REQUEST, message)
    }
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}

fn not_found(what: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
}

type ApiResult = Result<axum::Json<Value>, ApiError>;

fn ok() -> ApiResult {
    Ok(axum::Json(json!({ "ok": true })))
}

pub fn router(state: WebState) -> Router {
    let api = Router::new()
        .route("/state", get(get_state))
        .route("/events", get(events))
        .route("/conversations", get(conversations).post(new_conversation))
        .route("/conversations/{key}", get(conversation))
        .route("/conversations/{key}/read", post(read_conversation))
        .route("/conversations/{key}/notify", post(mute_conversation))
        .route("/conversations/{key}/send", post(send_message))
        .route("/conversations/{key}/attachments/{id}/{index}", get(attachment))
        .route("/peers", get(peers))
        .route("/propagation", post(set_propagation))
        .route("/announce", post(announce))
        .route("/sync", post(sync))
        .route("/settings", get(get_settings).post(save_settings))
        .route("/channels", get(channels).post(add_hub))
        .route("/channels/{hub}/room", get(room))
        .route("/channels/{hub}/{action}", post(hub_action))
        .route("/page", get(page).post(submit_form))
        .route("/media", get(media))
        .route("/download", get(download))
        .route("/saved", get(saved).post(save_page))
        .route("/saved/remove", post(remove_saved))
        .route("/identify", post(identify))
        .route("/cache/clear", post(clear_cache))
        .route("/node", get(node))
        .route("/node/page", get(node_page).post(node_save))
        .route("/node/pages", post(node_create))
        .route("/node/rename", post(node_rename))
        .route("/node/delete", post(node_delete))
        .route("/node/preview", post(node_preview))
        .route("/node/media", get(node_media))
        .route("/node/announce", post(node_announce))
        .route("/reticulum", get(reticulum))
        .route("/reticulum/options", post(reticulum_options))
        .route("/reticulum/interfaces", post(reticulum_interfaces))
        .route("/reticulum/text", post(reticulum_text))
        .route("/reticulum/check", post(reticulum_check))
        .route("/reticulum/restart", post(reticulum_restart))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(script))
        .route("/style.css", get(style))
        .route("/sw.js", get(service_worker))
        .route("/manifest.webmanifest", get(manifest))
        .route("/fonts/{name}", get(font))
        .route("/brand/{name}", get(brand))
        .nest("/api", api)
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .layer(middleware::from_fn(compress))
        .with_state(state)
}

/// Compare secrets without an early exit.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Asks for the link, or its token: an app added to a phone's home screen
/// keeps its own cookies and has no address bar to open the link in.
const LOGIN_PAGE: &str = "<!doctype html><meta charset=utf-8><title>rettui</title>\
<meta name=viewport content=\"width=device-width, initial-scale=1\">\
<link rel=icon type=image/png href=/brand/icon.png><link rel=manifest href=/manifest.webmanifest>\
<body style=\"font-family:sans-serif;background:#16161e;color:#ddd;padding:2em;line-height:1.5\">\
<h1><img src=/brand/wordmark.png alt=rettui height=36></h1><p>Open the link that <code>rettui --web</code> printed (it ends in <code>?token=…</code>) to log in.</p>\
<form method=get action=/><p><label>Or paste its token (after <code>token=</code>):<br>\
<input name=token type=password autocomplete=current-password required \
style=\"font:inherit;padding:.4em;width:min(28em,100%);box-sizing:border-box\"></label></p><p><button style=\"font:inherit;padding:.4em 1.2em\">Log in</button></p></form>";

/// Served without logging in: the logo and icons, for the login page and an
/// app added to a home screen; the manifest (browsers fetch it without the
/// login cookie); and the service worker (browsers fetch it again on their
/// own). None of them hold anything private.
const PUBLIC: [&str; 5] = ["/brand/wordmark.png", "/brand/icon.png", "/brand/icon-512.png", "/manifest.webmanifest", "/sw.js"];

/// Tokens a request carries: in the login cookie, an `Authorization: Bearer`
/// header or an `X-Rettui-Token` header.
fn presented_tokens(headers: &axum::http::HeaderMap) -> impl Iterator<Item = &str> {
    let text = |name| headers.get_all(name).into_iter().filter_map(|v| v.to_str().ok());
    let cookies = text(header::COOKIE)
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='));
    let bearer = text(header::AUTHORIZATION).filter_map(|v| {
        let (scheme, token) = v.trim().split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
    });
    let custom = text(header::HeaderName::from_static(TOKEN_HEADER)).map(str::trim);
    cookies.chain(bearer).chain(custom)
}

async fn auth(State(state): State<WebState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let query_token = request
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|p| p.strip_prefix("token=")).map(str::to_string));
    let mut response = if let Some(token) = query_token.filter(|_| path == "/") {
        if same(&token, &state.token) {
            let cookie = format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000");
            (StatusCode::SEE_OTHER, [(header::LOCATION, "/".to_string()), (header::SET_COOKIE, cookie)]).into_response()
        } else {
            (StatusCode::UNAUTHORIZED, axum::response::Html(LOGIN_PAGE)).into_response()
        }
    } else {
        let logged_in = presented_tokens(request.headers()).any(|t| same(t, &state.token));
        if logged_in || PUBLIC.contains(&path.as_str()) {
            next.run(request).await
        } else if path.starts_with("/api/") {
            let mut response = ApiError(StatusCode::UNAUTHORIZED, "not logged in".into()).into_response();
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"rettui\""));
            response
        } else {
            (StatusCode::UNAUTHORIZED, axum::response::Html(LOGIN_PAGE)).into_response()
        }
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

/// Text (JSON, the page, the script, the stylesheet) gzipped for browsers
/// that take it: mostly repeated structure, it shrinks to about a quarter,
/// which matters over a slow link. The event stream and files stay as they
/// are, and so does anything small.
async fn compress(request: Request, next: Next) -> Response {
    use std::io::Write;
    let gzip = accepts_gzip(request.headers());
    let mut response = next.run(request).await;
    let text = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| COMPRESSED_TYPES.iter().any(|p| t.starts_with(p)));
    if !text || response.headers().contains_key(header::CONTENT_ENCODING) {
        return response;
    }
    // Caches keep each encoding apart.
    if !response.headers().contains_key(header::VARY) {
        response.headers_mut().insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    if !gzip {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if bytes.len() < MIN_COMPRESSED {
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::with_capacity(bytes.len() / 4), flate2::Compression::fast());
    let Some(zipped) = encoder.write_all(&bytes).ok().and_then(|()| encoder.finish().ok()) else {
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    };
    parts.headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, axum::body::Body::from(zipped))
}

/// What [`compress`] gzips.
const COMPRESSED_TYPES: [&str; 5] = ["application/json", "text/html", "text/javascript", "text/css", "text/plain"];
/// Smaller than this isn't worth it.
const MIN_COMPRESSED: usize = 1024;

/// Whether `Accept-Encoding` takes gzip (and not with `q=0`).
fn accepts_gzip(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .into_iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            let name = parts.next().unwrap_or_default();
            let refused = parts.any(|p| p.strip_prefix("q=").and_then(|q| q.parse::<f32>().ok()) == Some(0.0));
            (name.eq_ignore_ascii_case("gzip") || name == "*") && !refused
        })
}

// ---- static files -------------------------------------------------------

/// A text file served from the build. Browsers check it's current on each
/// load (`no-cache`), and one that has it gets "not modified" by its ETag
/// rather than the file again; others get a copy gzipped once, at the best
/// level (it shrinks the script to about a quarter).
struct Asset {
    content_type: &'static str,
    body: String,
    gzip: Vec<u8>,
    etag: String,
}

impl Asset {
    fn new(content_type: &'static str, body: impl Into<String>) -> Self {
        use std::hash::{Hash, Hasher};
        use std::io::Write;
        let body = body.into();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        body.hash(&mut hasher);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        let gzip = encoder.write_all(body.as_bytes()).and_then(|()| encoder.finish()).unwrap_or_default();
        // Weak: the gzipped copy and the plain one are the same file.
        Self { content_type, body, gzip, etag: format!("W/\"{:016x}\"", hasher.finish()) }
    }

    fn serve(&self, headers: &axum::http::HeaderMap) -> Response {
        let current = headers
            .get_all(header::IF_NONE_MATCH)
            .into_iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|tag| tag.trim() == self.etag || tag.trim() == "*");
        let mut response = if current {
            StatusCode::NOT_MODIFIED.into_response()
        } else if accepts_gzip(headers) && !self.gzip.is_empty() {
            ([(header::CONTENT_TYPE, self.content_type), (header::CONTENT_ENCODING, "gzip")], self.gzip.clone()).into_response()
        } else {
            ([(header::CONTENT_TYPE, self.content_type)], self.body.clone()).into_response()
        };
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
        if let Ok(etag) = HeaderValue::from_str(&self.etag) {
            headers.insert(header::ETAG, etag);
        }
        response
    }
}

/// The page, with this build's version beside the name (a link to the
/// project page).
static INDEX: std::sync::LazyLock<Asset> = std::sync::LazyLock::new(|| {
    let page = include_str!("assets/index.html")
        .replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))
        .replace("{{PROJECT_URL}}", crate::config::PROJECT_URL);
    Asset::new("text/html; charset=utf-8", page)
});
static SCRIPT: std::sync::LazyLock<Asset> =
    std::sync::LazyLock::new(|| Asset::new("text/javascript; charset=utf-8", include_str!("assets/app.js")));
static STYLE: std::sync::LazyLock<Asset> =
    std::sync::LazyLock::new(|| Asset::new("text/css; charset=utf-8", include_str!("assets/style.css")));
/// Makes the web UI an app a phone can add to its home screen (and so show
/// notifications, on an iPhone).
static MANIFEST: std::sync::LazyLock<Asset> =
    std::sync::LazyLock::new(|| Asset::new("application/manifest+json", include_str!("assets/manifest.webmanifest")));

async fn manifest(headers: axum::http::HeaderMap) -> Response {
    MANIFEST.serve(&headers)
}

static SERVICE_WORKER: std::sync::LazyLock<Asset> =
    std::sync::LazyLock::new(|| Asset::new("text/javascript; charset=utf-8", include_str!("assets/sw.js")));

async fn index(headers: axum::http::HeaderMap) -> Response {
    INDEX.serve(&headers)
}

async fn script(headers: axum::http::HeaderMap) -> Response {
    SCRIPT.serve(&headers)
}

async fn style(headers: axum::http::HeaderMap) -> Response {
    STYLE.serve(&headers)
}

/// Shows notifications for the page (phone browsers only let a service
/// worker show them) and opens what one is about when it's tapped.
async fn service_worker(headers: axum::http::HeaderMap) -> Response {
    SERVICE_WORKER.serve(&headers)
}

/// The bundled Fira Code Nerd Font (see `assets/fonts/README.md`). The files
/// never change for a build, so browsers may cache them for good.
async fn font(Path(name): Path<String>) -> Response {
    let body: &'static [u8] = match name.as_str() {
        "FiraCodeNerdFont-Regular-Text.woff2" => include_bytes!("assets/fonts/FiraCodeNerdFont-Regular-Text.woff2"),
        "FiraCodeNerdFont-Bold-Text.woff2" => include_bytes!("assets/fonts/FiraCodeNerdFont-Bold-Text.woff2"),
        "FiraCodeNerdFont-Icons.woff2" => include_bytes!("assets/fonts/FiraCodeNerdFont-Icons.woff2"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, "font/woff2"), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")], body).into_response()
}

/// The logo (`assets/brand/`, made from `assets/logo.png`).
async fn brand(Path(name): Path<String>) -> Response {
    let body: &'static [u8] = match name.as_str() {
        "wordmark.png" => include_bytes!("assets/brand/wordmark.png"),
        "icon.png" => include_bytes!("assets/brand/icon.png"),
        "icon-512.png" => include_bytes!("assets/brand/icon-512.png"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], body).into_response()
}

// ---- state and live updates ----------------------------------------------

async fn get_state(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| views::state(&o.app)).await?))
}

/// One event per change, `<version> <scope>` (browsers refetch what they
/// show of it), and a `notify` event per notification (each browser decides
/// whether to show it).
async fn events(State(state): State<WebState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receivers = (state.changes.subscribe(), state.notices.subscribe());
    let stream = futures_util::stream::unfold(receivers, |(mut changes, mut notices)| async move {
        use tokio::sync::broadcast::error::RecvError;
        let event = loop {
            tokio::select! {
                change = changes.recv() => break match change {
                    Ok((version, scope)) => Event::default().data(format!("{version} {}", scope.name())),
                    // Missed some: one event covers them.
                    Err(RecvError::Lagged(_)) => Event::default().data("0 all"),
                    Err(RecvError::Closed) => return None,
                },
                notice = notices.recv() => match notice {
                    Ok(notification) => match Event::default().event("notify").json_data(json!({
                        "title": notification.title,
                        "body": notification.body,
                        "target": notification.target,
                        // Newer ones with the same tag replace older ones.
                        "tag": notification.target.tag(),
                    })) {
                        Ok(event) => break event,
                        Err(_) => continue,
                    },
                    // Old news by now.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                },
            }
        };
        Some((Ok(event), (changes, notices)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ---- messages -------------------------------------------------------------

async fn conversations(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| views::conversations(&o.app)).await?))
}

fn address(text: &str) -> Result<String, ApiError> {
    let text = text.trim();
    let text = text.strip_prefix("lxmf@").or_else(|| text.strip_prefix("lxmf://")).unwrap_or(text);
    parse_hash(text)
        .map(hex::encode)
        .ok_or_else(|| bad("An LXMF address is 32 hex characters"))
}

#[derive(Deserialize)]
struct AddressBody {
    address: String,
}

async fn new_conversation(State(state): State<WebState>, axum::Json(body): axum::Json<AddressBody>) -> ApiResult {
    let key = address(&body.address)?;
    let reply = key.clone();
    state
        .write(move |o| {
            o.app.store.conversations.entry(key).or_default();
            o.app.store_dirty = true;
        })
        .await?;
    Ok(axum::Json(json!({ "key": reply })))
}

#[derive(Deserialize)]
struct Newest {
    /// Only the newest this many messages.
    last: Option<usize>,
}

async fn conversation(State(state): State<WebState>, Path(key): Path<String>, Query(query): Query<Newest>) -> ApiResult {
    let key = address(&key)?;
    Ok(axum::Json(state.read(move |o| views::conversation(&o.app, &key, query.last)).await?))
}

async fn read_conversation(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    state.write(move |o| o.app.mark_conversation_read(&key)).await?;
    ok()
}

#[derive(Deserialize)]
struct MuteBody {
    muted: bool,
}

/// Turn a conversation's notifications off or back on.
async fn mute_conversation(State(state): State<WebState>, Path(key): Path<String>, body: axum::Json<MuteBody>) -> ApiResult {
    let muted = body.muted;
    if !state.write(move |o| o.app.set_conversation_muted(&key, muted)).await? {
        return Err(not_found("conversation"));
    }
    ok()
}

#[derive(Deserialize)]
struct Upload {
    name: String,
    /// Base64.
    data: String,
}

#[derive(Deserialize)]
struct SendBody {
    #[serde(default)]
    content: String,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    files: Vec<Upload>,
}

async fn send_message(
    State(state): State<WebState>,
    Path(key): Path<String>,
    axum::Json(body): axum::Json<SendBody>,
) -> ApiResult {
    let key = address(&key)?;
    let mode = match body.mode.as_str() {
        "" | "auto" => DeliveryMode::Auto,
        "direct" => DeliveryMode::Direct,
        "propagated" => DeliveryMode::Propagated,
        other => return Err(bad(format!("unknown delivery mode {other}"))),
    };
    // Uploaded files are kept like received ones, so the message can show them.
    let dir = state.paths.uploads.clone();
    let mut files: Vec<PathBuf> = Vec::new();
    for upload in body.files {
        let data = base64::engine::general_purpose::STANDARD
            .decode(upload.data.as_bytes())
            .map_err(|_| bad(format!("{} is not valid base64", upload.name)))?;
        tokio::fs::create_dir_all(&dir).await.map_err(|e| bad(e.to_string()))?;
        let path = unique_path(&dir, &upload.name);
        tokio::fs::write(&path, data).await.map_err(|e| bad(e.to_string()))?;
        files.push(path);
    }
    let result = state
        .write(move |o| o.app.send_message(key, body.content, files, mode).map_err(|(e, _, files)| (e, files)))
        .await?;
    match result {
        Ok(()) => ok(),
        Err((e, files)) => {
            for file in files {
                let _ = tokio::fs::remove_file(file).await;
            }
            Err(bad(e))
        }
    }
}

/// Header-safe file name.
fn file_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    if name.is_empty() { "download".into() } else { name }
}

fn file_response(data: Vec<u8>, name: &str, inline: bool) -> Response {
    let content_type = image::guess_format(&data)
        .map(|f| f.to_mime_type())
        .unwrap_or("application/octet-stream");
    let disposition = format!(
        "{}; filename=\"{}\"",
        if inline && content_type.starts_with("image/") { "inline" } else { "attachment" },
        file_name(name)
    );
    (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, "private, max-age=3600".to_string()),
        ],
        data,
    )
        .into_response()
}

async fn attachment(
    State(state): State<WebState>,
    Path((key, id, index)): Path<(String, String, usize)>,
) -> Result<Response, ApiError> {
    let found = state
        .read(move |o| {
            let message = o.app.store.conversations.get(&key)?.messages.iter().find(|m| m.id == id)?;
            let attachment = message.attachments.get(index)?;
            Some((attachment.path.clone(), attachment.name.clone()))
        })
        .await?;
    let (path, name) = found.ok_or_else(|| not_found("attachment"))?;
    let data = tokio::fs::read(&path).await.map_err(|_| not_found("attachment file"))?;
    Ok(file_response(data, &name, true))
}

// ---- network and status ---------------------------------------------------

#[derive(Deserialize)]
struct PeersQuery {
    /// `lxmf`, `nomad` or `propagation`; every kind when left out.
    kind: Option<String>,
    #[serde(default)]
    q: String,
    limit: Option<usize>,
}

async fn peers(State(state): State<WebState>, Query(query): Query<PeersQuery>) -> ApiResult {
    Ok(axum::Json(
        state.read(move |o| views::peers(&o.app, query.kind.as_deref(), &query.q, query.limit)).await?,
    ))
}

#[derive(Deserialize)]
struct NodeBody {
    node: String,
}

async fn set_propagation(State(state): State<WebState>, axum::Json(body): axum::Json<NodeBody>) -> ApiResult {
    let node = parse_hash(&body.node).ok_or_else(|| bad("not a destination address"))?;
    state.write(move |o| o.app.set_propagation_node(&hex::encode(node))).await?;
    ok()
}

async fn announce(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.send(NetCommand::Announce)).await?;
    ok()
}

async fn sync(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.send(NetCommand::Sync)).await?;
    ok()
}

async fn get_settings(State(state): State<WebState>) -> ApiResult {
    let view = state
        .read(|o| {
            let saved = o.app.saved_settings()?;
            Ok::<_, String>(views::settings(&o.app, &saved))
        })
        .await??;
    Ok(axum::Json(view))
}

#[derive(Deserialize)]
struct SettingsBody {
    /// Setting key -> new value as text; all are checked before any is saved.
    values: BTreeMap<String, String>,
}

async fn save_settings(State(state): State<WebState>, axum::Json(body): axum::Json<SettingsBody>) -> ApiResult {
    let notes = state
        .write(move |o| {
            let changes: Vec<(&str, &str)> = body.values.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            o.app.update_settings_from_web(&changes)
        })
        .await??;
    Ok(axum::Json(json!({ "notes": notes })))
}

// ---- channels -------------------------------------------------------------

async fn channels(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| views::channels(&o.app)).await?))
}

async fn add_hub(State(state): State<WebState>, axum::Json(body): axum::Json<AddressBody>) -> ApiResult {
    let (hash, aspect, room) =
        rrc::parse_link(&body.address).ok_or_else(|| bad("A hub address is 32 hex characters (or an rrc:// link)"))?;
    let reply = json!({ "hub": hex::encode(hash), "room": room });
    state.write(move |o| o.app.add_hub(hash, &aspect, room)).await?;
    Ok(axum::Json(reply))
}

#[derive(Deserialize)]
struct RoomQuery {
    #[serde(default)]
    name: String,
    /// Only the newest this many lines.
    last: Option<usize>,
}

fn hub_hash(text: &str) -> Result<Hash, ApiError> {
    parse_hash(text).ok_or_else(|| bad("not a hub address"))
}

async fn room(State(state): State<WebState>, Path(hub): Path<String>, Query(query): Query<RoomQuery>) -> ApiResult {
    let hash = hub_hash(&hub)?;
    let room = rrc::normalize_room(&query.name);
    let view = state
        .read(move |o| {
            let index = o.app.channels.hub_index(hash)?;
            Some(views::room(&o.app, &o.app.channels.hubs[index], &room, query.last))
        })
        .await?;
    view.map(axum::Json).ok_or_else(|| not_found("hub"))
}

#[derive(Deserialize, Default)]
struct HubBody {
    #[serde(default)]
    room: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    parts: Vec<String>,
    /// A user's identity (hex), for `whisper`.
    #[serde(default)]
    src: String,
    /// For `notify`: `all`, `mentions`, `off`, or `default` (a room follows
    /// its hub; a hub notifies of mentions and whispers).
    #[serde(default)]
    level: String,
}

async fn hub_action(
    State(state): State<WebState>,
    Path((hub, action)): Path<(String, String)>,
    body: Option<axum::Json<HubBody>>,
) -> ApiResult {
    let hash = hub_hash(&hub)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    let room = rrc::normalize_room(&body.room);
    let reply = state
        .write(move |o| -> Result<Value, ApiError> {
            let app = &mut o.app;
            let index = app.channels.hub_index(hash).ok_or_else(|| not_found("hub"))?;
            match action.as_str() {
                "connect" => app.toggle_connection(index),
                "auto" => app.toggle_auto_connect_at(index),
                "remove" => app.remove_hub(hash),
                "join" if !room.is_empty() => app.join_room(index, &room),
                "leave" if !room.is_empty() => app.leave_room(index, &room),
                "forget" if !room.is_empty() => app.forget_room(index, &room),
                "read" => app.mark_room_read(index, &room),
                "send" => {
                    let split = app.channel_submit(index, &room, &body.text);
                    return Ok(json!({ "split": split }));
                }
                "split" => app.confirm_split(hash, &room, &body.parts),
                "notify" => {
                    let level = match body.level.as_str() {
                        "default" => None,
                        other => Some(NotifyLevel::parse(other).ok_or_else(|| bad(format!("unknown notification level {other}")))?),
                    };
                    app.set_notify_level(index, &room, level);
                }
                // Open the whisper conversation with a user.
                "whisper" => {
                    let identity = hex::decode(body.src.trim()).ok().filter(|id| id.len() == 16).ok_or_else(|| bad("not a user identity"))?;
                    let key = app.open_whisper(index, &identity);
                    return Ok(json!({ "ok": true, "room": key }));
                }
                _ => return Err(bad(format!("unknown hub action {action}"))),
            }
            Ok(json!({ "ok": true, "room": room }))
        })
        .await??;
    Ok(axum::Json(reply))
}

// ---- browser --------------------------------------------------------------

/// Where a page link leads, for the web UI's script: an absolute NomadNet
/// address, or an `lxmf@` / `rrc://` link as written.
fn link_target(url: &str, node: Hash) -> String {
    let url = url.trim();
    if ["lxmf@", "lxmf://", "rrc://", "rrc@"].iter().any(|p| url.starts_with(p)) {
        return url.to_string();
    }
    resolve_url(url, Some(node)).map_or_else(|| url.to_string(), |l| l.url())
}

fn query_escape(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn location(url: &str) -> Result<Location, ApiError> {
    resolve_url(url, None).ok_or_else(|| bad(format!("Not a NomadNet address: {url}")))
}

async fn render_page(state: &WebState, location: Location, fetched: Fetched) -> ApiResult {
    let node = location.node;
    let source = String::from_utf8_lossy(&fetched.data).into_owned();
    let page = micron::parse(&source);
    let html = page.to_html(
        |url| link_target(url, node),
        |url| {
            resolve_url(url, Some(node))
                .filter(|l| l.path.starts_with("/media/"))
                .map(|l| format!("/api/media?url={}", query_escape(&l.url())))
        },
    );
    let url = location.url();
    let saved_url = url.clone();
    let (node_name, identified, saved) = state
        .read(move |o| {
            (
                o.app.store.display_name(&hex::encode(node)),
                o.app.identifies_to(node),
                o.app.store.saved.iter().any(|b| b.url == saved_url),
            )
        })
        .await?;
    Ok(axum::Json(json!({
        "url": url,
        "node": hex::encode(node),
        "node_name": node_name,
        "path": location.path,
        "html": html,
        "source": source,
        "source_html": html::source_html(&source),
        "cached_age": fetched.cached_age.map(|a| a.as_secs()),
        "identified": identified,
        "saved": saved,
    })))
}

#[derive(Deserialize)]
struct PageQuery {
    url: String,
    #[serde(default)]
    refresh: bool,
}

async fn page(State(state): State<WebState>, Query(query): Query<PageQuery>) -> ApiResult {
    let location = location(&query.url)?;
    if location.path.starts_with(nomad_core::FILE_PREFIX) {
        return Ok(axum::Json(json!({ "download": location.url() })));
    }
    let fetched = state.fetch(location.clone(), query.refresh).await?;
    render_page(&state, location, fetched).await
}

#[derive(Deserialize)]
struct FormBody {
    url: String,
    #[serde(default)]
    fields: BTreeMap<String, String>,
}

async fn submit_form(State(state): State<WebState>, axum::Json(body): axum::Json<FormBody>) -> ApiResult {
    let mut location = location(&body.url)?;
    location.fields = body.fields;
    let fetched = state.fetch(location.clone(), false).await?;
    render_page(&state, location, fetched).await
}

#[derive(Deserialize)]
struct UrlQuery {
    url: String,
}

async fn media(State(state): State<WebState>, Query(query): Query<UrlQuery>) -> Result<Response, ApiError> {
    let location = location(&query.url)?;
    if !location.path.starts_with("/media/") {
        return Err(bad("not a media path"));
    }
    let name = location.path.rsplit('/').next().unwrap_or("image").to_string();
    let fetched = state.fetch(location, false).await?;
    Ok(file_response(fetched.data, &name, true))
}

async fn download(State(state): State<WebState>, Query(query): Query<UrlQuery>) -> Result<Response, ApiError> {
    let location = location(&query.url)?;
    let fallback = location.path.rsplit('/').next().unwrap_or("download").to_string();
    let fetched = state.fetch(location, false).await?;
    let name = nomad_core::reply_file_name(fetched.metadata.as_deref()).unwrap_or(fallback);
    Ok(file_response(fetched.data, &name, false))
}

async fn saved(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| views::saved(&o.app)).await?))
}

#[derive(Deserialize)]
struct UrlBody {
    url: String,
}

async fn save_page(State(state): State<WebState>, axum::Json(body): axum::Json<UrlBody>) -> ApiResult {
    let location = location(&body.url)?;
    state
        .write(move |o| {
            let url = location.url();
            if !o.app.store.saved.iter().any(|b| b.url == url) {
                let name = o.app.bookmark_name(&location);
                o.app.log(format!("Saved {name}"));
                o.app.store.saved.push(Bookmark { name, url });
                o.app.store_dirty = true;
            }
        })
        .await?;
    ok()
}

async fn remove_saved(State(state): State<WebState>, axum::Json(body): axum::Json<UrlBody>) -> ApiResult {
    state
        .write(move |o| {
            let before = o.app.store.saved.len();
            o.app.store.saved.retain(|b| b.url != body.url);
            o.app.store_dirty |= o.app.store.saved.len() != before;
        })
        .await?;
    ok()
}

#[derive(Deserialize)]
struct IdentifyBody {
    node: String,
    on: bool,
}

async fn identify(State(state): State<WebState>, axum::Json(body): axum::Json<IdentifyBody>) -> ApiResult {
    let node = parse_hash(&body.node).ok_or_else(|| bad("not a node address"))?;
    state.write(move |o| o.app.set_identify(node, body.on)).await?;
    ok()
}

async fn clear_cache(State(state): State<WebState>) -> ApiResult {
    let removed = state
        .write(|o| {
            let removed = o.app.page_cache().clear();
            o.app.log(format!("Cleared {removed} cached page(s) and image(s)"));
            removed
        })
        .await?;
    Ok(axum::Json(json!({ "removed": removed })))
}

// ---- hosted node ----------------------------------------------------------
//
// The web UI may create and edit ordinary pages but never scripts
// (executable pages): with scripts enabled that would let anyone holding the
// token run programs on the host.

async fn node(State(state): State<WebState>) -> ApiResult {
    let view = state
        .read(|o| {
            o.app.refresh_pages();
            views::node(&o.app)
        })
        .await?;
    Ok(axum::Json(view))
}

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

async fn node_page(State(state): State<WebState>, Query(query): Query<PathQuery>) -> ApiResult {
    let path = query.path;
    let view = state
        .read(move |o| {
            let (content, executable) = o.app.node_read(&path)?;
            Ok::<_, String>(json!({
                "path": path,
                "content": content,
                "executable": executable,
                "url": o.app.node_page_url(&path),
            }))
        })
        .await??;
    Ok(axum::Json(view))
}

#[derive(Deserialize)]
struct PageBody {
    path: String,
    content: String,
}

async fn node_save(State(state): State<WebState>, axum::Json(body): axum::Json<PageBody>) -> ApiResult {
    state.write(move |o| o.app.node_save(&body.path, &body.content, false)).await??;
    ok()
}

#[derive(Deserialize)]
struct NewPageBody {
    name: String,
}

async fn node_create(State(state): State<WebState>, axum::Json(body): axum::Json<NewPageBody>) -> ApiResult {
    let path = state.write(move |o| o.app.node_create(&body.name)).await??;
    Ok(axum::Json(json!({ "path": path })))
}

#[derive(Deserialize)]
struct RenameBody {
    from: String,
    to: String,
}

async fn node_rename(State(state): State<WebState>, axum::Json(body): axum::Json<RenameBody>) -> ApiResult {
    let path = state.write(move |o| o.app.node_rename(&body.from, &body.to, false)).await??;
    Ok(axum::Json(json!({ "path": path })))
}

async fn node_delete(State(state): State<WebState>, axum::Json(body): axum::Json<PathQuery>) -> ApiResult {
    state.write(move |o| o.app.node_delete(&body.path)).await??;
    ok()
}

#[derive(Deserialize)]
struct PreviewBody {
    content: String,
}

/// Render unsaved Micron for the editor's live preview. Links resolve against
/// this node; its images are read from the node folder.
async fn node_preview(State(state): State<WebState>, axum::Json(body): axum::Json<PreviewBody>) -> ApiResult {
    let node = state.read(|o| o.app.node.hash).await?;
    let page = micron::parse(&body.content);
    let html = page.to_html(
        |url| link_target(url, node),
        |url| {
            let location = resolve_url(url, Some(node)).filter(|l| l.path.starts_with("/media/"))?;
            Some(match location.path.strip_prefix("/media/") {
                Some(rel) if location.node == node => format!("/api/node/media?path={}", query_escape(rel)),
                _ => format!("/api/media?url={}", query_escape(&location.url())),
            })
        },
    );
    Ok(axum::Json(json!({ "html": html })))
}

async fn node_media(State(state): State<WebState>, Query(query): Query<PathQuery>) -> Result<Response, ApiError> {
    let path = query.path;
    let name = path.rsplit('/').next().unwrap_or("image").to_string();
    let data = state.read(move |o| o.app.node_media(&path)).await?.map_err(|_| not_found("image"))?;
    Ok(file_response(data, &name, true))
}

async fn node_announce(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.send(NetCommand::HostAnnounce)).await?;
    ok()
}

// ---- Reticulum config -----------------------------------------------------
//
// Web edits are `restricted`: they may not add or change pipe interface
// commands, which run programs (the same reason scripts are read-only here).

#[derive(Deserialize)]
struct SectionQuery {
    section: Option<String>,
}

async fn reticulum(State(state): State<WebState>, Query(query): Query<SectionQuery>) -> ApiResult {
    let view = state.read(move |o| views::reticulum(&o.app, query.section.as_deref())).await??;
    Ok(axum::Json(view))
}

fn section(id: &str) -> Result<crate::reticulum::Section, ApiError> {
    crate::reticulum::Section::from_id(id).ok_or_else(|| bad(format!("No section {id}")))
}

#[derive(Deserialize)]
struct RnsOptionsBody {
    section: String,
    /// Key -> value as text; empty unsets. All are applied or none.
    values: BTreeMap<String, String>,
}

async fn reticulum_options(State(state): State<WebState>, axum::Json(body): axum::Json<RnsOptionsBody>) -> ApiResult {
    let section = section(&body.section)?;
    let warnings = state
        .write(move |o| {
            let changes: Vec<(&str, &str)> = body.values.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            o.app.rns_set_options(&section, &changes, true)
        })
        .await??;
    Ok(axum::Json(json!({ "warnings": warnings })))
}

#[derive(Deserialize)]
struct RnsInterfaceBody {
    /// `add`, `rename` or `delete`.
    action: String,
    name: String,
    to: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

async fn reticulum_interfaces(State(state): State<WebState>, axum::Json(body): axum::Json<RnsInterfaceBody>) -> ApiResult {
    let warnings = state
        .write(move |o| match body.action.as_str() {
            "add" => o.app.rns_add_interface(&body.name, body.kind.as_deref().unwrap_or("AutoInterface"), true),
            "rename" => o.app.rns_rename_interface(&body.name, body.to.as_deref().unwrap_or_default(), true),
            "delete" => o.app.rns_remove_interface(&body.name, true),
            other => Err(format!("Unknown action {other}")),
        })
        .await??;
    Ok(axum::Json(json!({ "warnings": warnings })))
}

#[derive(Deserialize)]
struct RnsTextBody {
    text: String,
}

/// Restart the Reticulum stack. It happens right after this request, in the
/// owner loop; browsers follow it through the usual state updates.
async fn reticulum_restart(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.request_rns_restart()).await?;
    ok()
}

/// Check text without saving it (the text editor's live status).
async fn reticulum_check(axum::Json(body): axum::Json<RnsTextBody>) -> ApiResult {
    let (_, check) = crate::reticulum::check(&body.text);
    Ok(axum::Json(json!({ "error": check.error, "warnings": check.warnings })))
}

async fn reticulum_text(State(state): State<WebState>, axum::Json(body): axum::Json<RnsTextBody>) -> ApiResult {
    let warnings = state.write(move |o| o.app.rns_save_text(&body.text, true)).await??;
    Ok(axum::Json(json!({ "warnings": warnings })))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::http::HeaderMap;

    use super::*;

    fn tokens(pairs: &[(&'static str, &str)]) -> Vec<String> {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_str(value).unwrap());
        }
        presented_tokens(&headers).map(str::to_string).collect()
    }

    #[test]
    fn tokens_come_from_cookie_or_headers() {
        assert_eq!(tokens(&[("cookie", "theme=dark; rettui_token=abc")]), ["abc"]);
        assert_eq!(tokens(&[("authorization", "Bearer abc")]), ["abc"]);
        assert_eq!(tokens(&[("authorization", "bearer  abc ")]), ["abc"]);
        assert_eq!(tokens(&[("x-rettui-token", " abc")]), ["abc"]);
        // Other schemes and look-alike cookies are not tokens.
        assert!(tokens(&[("authorization", "Basic abc")]).is_empty());
        assert!(tokens(&[("cookie", "rettui_tokenx=abc")]).is_empty());
        assert!(tokens(&[]).is_empty());
    }

    #[test]
    fn secrets_compare_exactly() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd") && !same("abc", "ab") && !same("", "a"));
    }

    #[test]
    fn gzip_is_taken_unless_refused() {
        let takes = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_str(value).unwrap());
            accepts_gzip(&headers)
        };
        assert!(takes("gzip, deflate, br, zstd") && takes("GZIP") && takes("br;q=1.0, gzip;q=0.5") && takes("*"));
        assert!(!takes("br") && !takes("identity") && !takes("gzip;q=0") && !takes("gzip; q=0.0"));
        assert!(!accepts_gzip(&HeaderMap::new()));
    }

    /// A response from `router` (behind [`compress`]) to a GET of `path`:
    /// its headers (lowercase names) and body, as sent.
    async fn fetch(router: Router, path: &str, accept: Option<&str>) -> (HashMap<String, String>, Vec<u8>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router.layer(middleware::from_fn(compress))).await });
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let accept = accept.map(|a| format!("Accept-Encoding: {a}\r\n")).unwrap_or_default();
        stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n{accept}Connection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        server.abort();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let headers = head
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
            .collect();
        let mut body = raw[split + 4..].to_vec();
        // Chunked (a compressed body has no known length).
        if head.to_lowercase().contains("transfer-encoding: chunked") {
            let mut out = Vec::new();
            let mut rest = &body[..];
            while let Some(end) = rest.windows(2).position(|w| w == b"\r\n") {
                let size = usize::from_str_radix(std::str::from_utf8(&rest[..end]).unwrap().trim(), 16).unwrap();
                if size == 0 {
                    break;
                }
                out.extend_from_slice(&rest[end + 2..end + 2 + size]);
                rest = &rest[end + 2 + size + 2..];
            }
            body = out;
        }
        (headers, body)
    }

    #[test]
    fn the_script_is_not_sent_again_to_a_browser_that_has_it() {
        let asset = Asset::new("text/javascript", "console.log('rettui');".repeat(200));
        let request = |pairs: &[(&'static str, &str)]| {
            let mut headers = HeaderMap::new();
            for (name, value) in pairs {
                headers.append(*name, HeaderValue::from_str(value).unwrap());
            }
            asset.serve(&headers)
        };
        let first = request(&[("accept-encoding", "gzip")]);
        let etag = first.headers()[header::ETAG].to_str().unwrap().to_string();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CONTENT_ENCODING], "gzip");
        assert!(etag.starts_with("W/\""));
        // A browser that has this one gets "not modified"; one with another
        // (an older build) gets the file.
        assert_eq!(request(&[("if-none-match", &etag)]).status(), StatusCode::NOT_MODIFIED);
        let other = request(&[("if-none-match", "W/\"0123\"")]);
        assert_eq!(other.status(), StatusCode::OK);
        assert!(!other.headers().contains_key(header::CONTENT_ENCODING));
        // A changed file has another tag.
        assert_ne!(Asset::new("text/javascript", "something else").etag, etag);
    }

    #[tokio::test]
    async fn text_is_gzipped_and_the_rest_left_alone() {
        use std::io::Read;
        let big = json!({ "lines": vec!["the same words again and again"; 400] }).to_string();
        let router = || {
            let big = big.clone();
            Router::new()
                .route("/big", get(move || async move { ([(header::CONTENT_TYPE, "application/json")], big) }))
                .route("/small", get(|| async { axum::Json(json!({ "ok": true })) }))
                .route("/png", get(|| async { ([(header::CONTENT_TYPE, "image/png")], vec![7u8; 50_000]) }))
                .route("/events", get(|| async { ([(header::CONTENT_TYPE, "text/event-stream")], "data: 1\n\n".repeat(500)) }))
        };
        let (headers, body) = fetch(router(), "/big", Some("gzip, br")).await;
        assert_eq!(headers.get("content-encoding").map(String::as_str), Some("gzip"));
        assert_eq!(headers.get("vary").map(String::as_str), Some("accept-encoding"));
        let mut unzipped = String::new();
        flate2::read::GzDecoder::new(&body[..]).read_to_string(&mut unzipped).unwrap();
        assert_eq!(unzipped, big);
        assert!(body.len() * 10 < big.len(), "{} of {}", body.len(), big.len());
        // Browsers that don't take it, and small answers, get it as it is.
        let (headers, body) = fetch(router(), "/big", None).await;
        assert!(!headers.contains_key("content-encoding") && body == big.as_bytes());
        assert_eq!(headers.get("vary").map(String::as_str), Some("accept-encoding"));
        let (headers, body) = fetch(router(), "/small", Some("gzip")).await;
        assert!(!headers.contains_key("content-encoding") && body == br#"{"ok":true}"#);
        // Images and the event stream are left alone.
        for path in ["/png", "/events"] {
            let (headers, _) = fetch(router(), path, Some("gzip")).await;
            assert!(!headers.contains_key("content-encoding"), "{path}");
        }
    }
}
