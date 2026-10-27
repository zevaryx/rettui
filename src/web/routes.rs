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
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Fetched, Notice, WebState, push, views};
use crate::app::files::unique_path;
use crate::app::notify::Notification;
use crate::app::{Location, resolve_url};
use crate::lxmf::DeliveryMode;
use crate::net::{Hash, NetCommand, parse_hash};
use crate::nomad::micron::{self, html};
use crate::nomad::pages::{Allowed, Root};
use crate::rrc;
use crate::store::{Bookmark, NotifyLevel};

const COOKIE: &str = "rettui_token";
/// Header for clients that already use `Authorization` for something else.
const TOKEN_HEADER: &str = "x-rettui-token";
/// Largest request (base64 attachments included).
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// Scripts only from this server; page content is inert HTML. Inline style
/// attributes carry Micron colours.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

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
        .route("/push", get(push_key))
        .route("/push/subscribe", post(push_subscribe))
        .route("/push/unsubscribe", post(push_unsubscribe))
        .route("/push/showing", post(push_showing))
        .route("/conversations", get(conversations).post(new_conversation))
        .route("/search", get(search_messages))
        .route("/sign-out-others", post(sign_out_others))
        .route("/icons", get(icons))
        .route("/locations", get(locations))
        .route("/locations/{key}", axum::routing::delete(forget_locations))
        .route("/update/install", post(install_update))
        .route("/update/check", post(check_update))
        .route("/backup", get(download_backup))
        .route("/announces", get(announces))
        .route("/tables", get(tables))
        .route("/map/tiles/{z}/{x}/{y}", get(map_tile))
        .route("/conversations/{key}", get(conversation))
        .route("/conversations/{key}/archive", get(conversation_archive))
        .route("/conversations/{key}/export", get(export_conversation))
        .route("/conversations/{key}/forward", post(forward_message))
        .route("/conversations/{key}/read", post(read_conversation))
        .route("/conversations/{key}/notify", post(mute_conversation))
        .route("/conversations/{key}/autoplay", post(autoplay_conversation))
        .route("/conversations/{key}/pin", post(pin_conversation))
        .route("/conversations/{key}/live", post(start_live))
        .route("/conversations/{key}/live/stop", post(stop_live))
        .route("/conversations/{key}/track", post(start_track))
        .route("/conversations/{key}/track/stop", post(stop_track))
        .route("/live/position", post(live_position))
        .route("/conversations/read-all", post(read_all_conversations))
        .route("/conversations/{key}/send", post(send_message))
        .route("/conversations/{key}/react", post(react))
        .route("/conversations/{key}/command", post(send_command))
        .route("/conversations/{key}/retry", post(retry))
        .route("/conversations/{key}/cancel", post(cancel_send))
        .route("/conversations/{key}/messages/delete", post(delete_message))
        .route("/conversations/{key}/delete", post(delete_conversation))
        .route("/conversations/{key}/contact", post(save_contact))
        .route("/conversations/{key}/ping", post(ping))
        .route("/path/{key}", get(path_state).post(find_path).delete(forget_path))
        .route("/probe/{key}", get(probe_state).post(probe))
        .route("/conversations/{key}/delivery", post(set_delivery))
        .route("/conversations/{key}/trust", post(set_trust))
        .route("/games", get(games).post(start_game))
        .route("/games/{app}/{session}", get(game).post(game_action).delete(forget_game))
        .route("/conversations/{key}/attachments/{id}/{index}", get(attachment))
        .route("/paper/read", post(read_paper))
        .route("/paper/scan", post(scan_paper))
        .route("/qr", get(qr_code))
        .route("/peers", get(peers))
        .route("/propagation", post(set_propagation))
        .route("/announce", post(announce))
        .route("/sync", post(sync))
        .route("/settings", get(get_settings).post(save_settings))
        .route("/guide", get(guide).post(apply_guide))
        .route("/guide/dismiss", post(dismiss_guide))
        .route("/guide/reach", get(guide_reach))
        .route("/identity/backed-up", post(identity_backed_up))
        .route("/first-steps/hide", post(hide_first_steps))
        .route("/channels", get(channels).post(add_hub))
        .route("/channels/{hub}/room", get(room))
        .route("/channels/search", get(search_channels))
        .route("/channels/{hub}/{action}", post(hub_action))
        .route("/page", get(page).post(submit_form))
        .route("/partial", post(partial))
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
        .route("/node/move", post(node_move))
        .route("/node/upload", post(node_upload))
        .route("/node/preview", post(node_preview))
        .route("/node/media", get(node_media))
        .route("/node/announce", post(node_announce))
        .route("/node/allowed", get(node_allowed).post(set_node_allowed))
        .route("/node/stats/reset", post(reset_node_stats))
        .route("/rncp", get(rncp_sends))
        .route("/speedtest", get(speed_test).post(set_speed_test))
        .route("/speedtest/send", post(send_speed_test))
        .route("/identity", get(identities))
        .route("/identity/lookup", post(look_up_identity))
        .route("/rncp/send", post(rncp_send))
        .route("/repos", get(repos).post(add_repo))
        .route("/repos/remove", post(remove_repo))
        .route("/repos/page", post(repos_on_page))
        .route("/repos/pages", post(set_repo_pages))
        .route("/repos/mirror", post(repo_mirror))
        .route("/repos/mirror/delete", post(repo_delete_mirror))
        .route("/repos/push", post(repo_push))
        .route("/repos/refs", get(repo_refs))
        .route("/repos/copy/files", get(repo_copy_files))
        .route("/repos/perms", get(repo_perms))
        .route("/repos/releases", get(repo_releases))
        .route("/repos/release", get(repo_release))
        .route("/repos/artifact", get(repo_artifact))
        .route("/repos/work", get(repo_work))
        .route("/repos/doc", get(repo_doc))
        .route("/repos/doc/new", post(repo_new_doc))
        .route("/repos/doc/comment", post(repo_comment))
        .route("/repos/doc/edit", post(repo_edit_doc))
        .route("/repos/doc/state", post(repo_doc_state))
        .route("/repos/doc/delete", post(repo_delete_doc))
        .route("/repos/release/new", post(repo_new_release))
        .route("/repos/release/delete", post(repo_delete_release))
        .route("/hub", get(hosted_hub))
        .route("/hub/{action}", post(hosted_hub_action))
        .route("/reticulum", get(reticulum))
        .route("/reticulum/options", post(reticulum_options))
        .route("/reticulum/interfaces", post(reticulum_interfaces))
        .route("/reticulum/text", post(reticulum_text))
        .route("/reticulum/check", post(reticulum_check))
        .route("/reticulum/lora", get(reticulum_lora))
        .route("/reticulum/restart", post(reticulum_restart))
        .route("/quit", post(quit))
        .route("/reticulum/history", get(reticulum_history))
        .route("/reticulum/history/{id}", get(reticulum_version))
        .route("/reticulum/history/{id}/restore", post(reticulum_restore))
        .route("/reticulum/profiles", get(reticulum_profiles).post(reticulum_save_profile))
        .route("/reticulum/profiles/{name}", axum::routing::delete(reticulum_delete_profile))
        .route("/reticulum/profiles/{name}/switch", post(reticulum_switch_profile))
        .route("/identities", get(identities_list).post(identities_add))
        .route("/identities/{id}/switch", post(identities_switch))
        .route("/identities/{id}/rename", post(identities_rename))
        .route("/emoji", get(emoji_list))
        .route("/emoji/recent", get(recent_emoji).post(pick_emoji))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(script))
        .route("/style.css", get(style))
        .route("/sw.js", get(service_worker))
        .route("/manifest.webmanifest", get(manifest))
        .route("/fonts/{name}", get(font))
        .route("/brand/{name}", get(brand))
        .route("/face/{address}", get(face))
        .route("/rettui-ca.crt", get(certificate_authority))
        .nest("/api", api)
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .layer(middleware::from_fn(compress))
        .with_state(state)
}

/// rettui's own certificate authority, for a device to install (with
/// `--https` and no certificate of its own): public, like any CA
/// certificate.
async fn certificate_authority(State(state): State<WebState>) -> Response {
    match &state.ca {
        Some(der) => (
            [(header::CONTENT_TYPE, "application/x-x509-ca-cert"), (header::CONTENT_DISPOSITION, "attachment; filename=\"rettui-ca.crt\"")],
            der.as_ref().clone(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Compare secrets without an early exit.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Asks for the link, or its token: an app added to a phone's home screen
/// keeps its own cookies and has no address bar to open the link in.
const LOGIN_PAGE: &str = "<!doctype html><meta charset=utf-8><title>rettui</title>\
<meta name=viewport content=\"width=device-width, initial-scale=1\">\
<link rel=icon type=image/png href=brand/icon.png><link rel=manifest href=manifest.webmanifest crossorigin=use-credentials>\
<body style=\"font-family:sans-serif;background:#16161e;color:#ddd;padding:2em;line-height:1.5\">\
<h1><img src=brand/wordmark.png alt=rettui height=36></h1><p>Open the link that <code>rettui --web</code> printed (it ends in <code>?token=…</code>) to log in.</p>\
<form method=get action=.><p><label>Or paste its token (after <code>token=</code>):<br>\
<input name=token type=password autocomplete=current-password required \
style=\"font:inherit;padding:.4em;width:min(28em,100%);box-sizing:border-box\"></label></p><p><button style=\"font:inherit;padding:.4em 1.2em\">Log in</button></p></form>";

/// Served without logging in: the logo and icons, for the login page and an
/// app added to a home screen; the manifest (browsers fetch it without the
/// login cookie); and the service worker (browsers fetch it again on their
/// own). None of them hold anything private.
const PUBLIC: [&str; 6] =
    ["/brand/wordmark.png", "/brand/icon.png", "/brand/icon-512.png", "/manifest.webmanifest", "/sw.js", "/rettui-ca.crt"];

/// Tokens a request carries: in the login cookie, an `Authorization: Bearer`
/// header or an `X-Rettui-Token` header.
fn presented_tokens(headers: &axum::http::HeaderMap) -> impl Iterator<Item = &str> {
    let text = |name| headers.get_all(name).into_iter().filter_map(|v| v.to_str().ok());
    let cookies = text(header::COOKIE).flat_map(|v| v.split(';')).filter_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='));
    let bearer = text(header::AUTHORIZATION).filter_map(|v| {
        let (scheme, token) = v.trim().split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
    });
    let custom = text(header::HeaderName::from_static(TOKEN_HEADER)).map(str::trim);
    cookies.chain(bearer).chain(custom)
}

async fn auth(State(state): State<WebState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let query_token = request.uri().query().and_then(|q| q.split('&').find_map(|p| p.strip_prefix("token=")).map(str::to_string));
    let mut response = if let Some(token) = query_token.filter(|_| path == "/") {
        if same(&token, &state.token()) {
            let secure = if state.https { "; Secure" } else { "" };
            let cookie = format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000{secure}");
            (StatusCode::SEE_OTHER, [(header::LOCATION, "./".to_string()), (header::SET_COOKIE, cookie)]).into_response()
        } else {
            (StatusCode::UNAUTHORIZED, axum::response::Html(LOGIN_PAGE)).into_response()
        }
    } else {
        let current = state.token();
        let logged_in = presented_tokens(request.headers()).any(|t| same(t, &current));
        if logged_in || PUBLIC.contains(&path.as_str()) {
            next.run(request).await
        } else if path.starts_with("/api/") {
            let mut response = ApiError(StatusCode::UNAUTHORIZED, "not logged in".into()).into_response();
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"rettui\""));
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
const COMPRESSED_TYPES: [&str; 6] = ["application/json", "text/html", "text/javascript", "text/css", "text/plain", "image/svg+xml"];
/// Smaller than this isn't worth it.
const MIN_COMPRESSED: usize = 1024;

/// Whether `Accept-Encoding` takes gzip (and not with `q=0`).
fn accepts_gzip(headers: &axum::http::HeaderMap) -> bool {
    headers.get_all(header::ACCEPT_ENCODING).into_iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).any(|coding| {
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

/// The emoji pickers' list (see [`crate::emoji`]): each group's emoji as
/// `[emoji, name, shortcodes]`. The same for a build, so a browser that has
/// it gets "not modified".
static EMOJI: std::sync::LazyLock<Asset> = std::sync::LazyLock::new(|| {
    let groups: Vec<Value> = crate::emoji::groups()
        .iter()
        .map(|group| {
            let emoji: Vec<Value> = group.emoji.iter().map(|e| json!([e.as_str(), e.name(), e.shortcodes().collect::<Vec<_>>()])).collect();
            json!({ "name": group.name, "icon": group.icon, "emoji": emoji })
        })
        .collect();
    Asset::new("application/json", json!({ "groups": groups }).to_string())
});

async fn emoji_list(headers: axum::http::HeaderMap) -> Response {
    EMOJI.serve(&headers)
}

/// The emoji picked lately, newest first (in both UIs).
async fn recent_emoji(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| json!(o.app.store.recent_emoji)).await?))
}

#[derive(Deserialize)]
struct PickedEmoji {
    emoji: String,
}

/// An emoji was picked: it goes first among the recently used. Nothing a
/// browser shows changes, so they aren't told to refresh.
async fn pick_emoji(State(state): State<WebState>, body: axum::Json<PickedEmoji>) -> ApiResult {
    let emoji = crate::emoji::get(&body.emoji).ok_or_else(|| bad("not an emoji"))?.as_str();
    let recent = state
        .read(move |o| {
            o.app.remember_emoji(emoji);
            json!(o.app.store.recent_emoji)
        })
        .await?;
    Ok(axum::Json(recent))
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

/// Someone's face (LXMFace's picture of an LXMF address), for those without
/// an icon of their own: `/face/<address>.svg`. An address always gives the
/// same face, so browsers may keep it.
async fn face(Path(address): Path<String>) -> Response {
    let address = address.strip_suffix(".svg").unwrap_or(&address).to_ascii_lowercase();
    if address.len() != 32 || !address.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let svg = crate::icons::face::svg(&address, 64);
    ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "private, max-age=31536000, immutable")], svg).into_response()
}

// ---- state and live updates ----------------------------------------------

/// Sign every other browser out: a new login secret, kept by this
/// browser's cookie (scripts using the old one need the new one, from the
/// data directory's `web_token`).
async fn sign_out_others(State(state): State<WebState>) -> Result<Response, ApiError> {
    let token = state.new_token().map_err(bad)?;
    let secure = if state.https { "; Secure" } else { "" };
    let cookie = format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000{secure}");
    let _ =
        state.write(|o| o.app.log("Signed every other browser out of the web UI (a new login link is printed where rettui runs)")).await;
    Ok(([(header::SET_COOKIE, cookie)], axum::Json(json!({ "ok": true }))).into_response())
}

#[derive(Deserialize)]
struct IconQuery {
    #[serde(default)]
    q: String,
}

/// Icons whose names hold every word asked for, to pick one: names and
/// characters.
async fn icons(Query(query): Query<IconQuery>) -> axum::Json<Value> {
    let found = crate::icons::search(&query.q, 120);
    axum::Json(json!(found.iter().map(|(name, glyph)| json!({ "name": name, "glyph": glyph.to_string() })).collect::<Vec<_>>()))
}

/// Install the newer release found in this one's place (it carries on
/// downloading; the status says how it went).
/// Check for a newer release now; what it finds comes as a notice.
async fn check_update(State(state): State<WebState>) -> ApiResult {
    let doing = state.write(|o| o.app.check_for_updates()).await?;
    Ok(axum::Json(json!({ "ok": true, "doing": doing })))
}

async fn install_update(State(state): State<WebState>) -> ApiResult {
    let doing = state.write(|o| o.app.install_update()).await?.map_err(bad)?;
    Ok(axum::Json(json!({ "ok": true, "doing": doing })))
}

/// A backup of settings, contacts and messages, to download: never with
/// the identity, nor attachments (see [`crate::backup`]).
async fn download_backup(State(state): State<WebState>) -> Result<Response, ApiError> {
    let dir = state.paths.cache.clone();
    std::fs::create_dir_all(&dir).map_err(|e| bad(format!("Couldn't make {}: {e}", dir.display())))?;
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let path = dir.join(format!("backup-{nanos}.tar.gz"));
    let (done, made) = tokio::sync::oneshot::channel();
    let to = path.clone();
    state.write(move |o| o.app.back_up_for_download(to, done)).await?;
    made.await.map_err(|_| bad("The backup wasn't made"))?.map_err(|e| bad(format!("Couldn't back up: {e}")))?;
    let body = send_then_delete(path).await.map_err(|e| bad(format!("Couldn't read the backup: {e}")))?;
    let disposition = format!("attachment; filename=\"{}\"", crate::backup::default_name());
    Ok(([(header::CONTENT_TYPE, "application/gzip".to_string()), (header::CONTENT_DISPOSITION, disposition)], body).into_response())
}

/// A file's contents as a response body, read as it's sent; the file is
/// deleted once it's sent (or the download stops).
async fn send_then_delete(path: PathBuf) -> std::io::Result<axum::body::Body> {
    use tokio::io::AsyncReadExt;
    struct Sending {
        file: Option<tokio::fs::File>,
        path: PathBuf,
    }
    impl Drop for Sending {
        fn drop(&mut self) {
            self.file = None;
            let _ = std::fs::remove_file(&self.path);
        }
    }
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
    };
    let sending = Sending { file: Some(file), path };
    let chunks = futures_util::stream::unfold(sending, |mut sending| async move {
        let mut chunk = vec![0; 64 * 1024];
        let read = sending.file.as_mut()?.read(&mut chunk).await;
        match read {
            Ok(0) => None,
            Ok(n) => {
                chunk.truncate(n);
                Some((Ok(axum::body::Bytes::from(chunk)), sending))
            }
            Err(e) => {
                // Said, then the end.
                sending.file = None;
                Some((Err(e), sending))
            }
        }
    });
    Ok(axum::body::Body::from_stream(chunks))
}

#[derive(Deserialize)]
struct LiveBody {
    /// How long: minutes, 0 until stopped.
    #[serde(default)]
    minutes: u64,
    /// `device` (positions come from the page) or `station` (the Location
    /// setting).
    source: String,
}

/// Start sharing a location live with them.
async fn start_live(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<LiveBody>) -> ApiResult {
    let key = address(&key)?;
    let source = match body.source.as_str() {
        "device" => crate::app::live::Source::Device,
        "station" => crate::app::live::Source::Station,
        other => return Err(bad(format!("unknown source {other}"))),
    };
    let doing = state.write(move |o| o.app.start_live(&key, body.minutes, source)).await?.map_err(bad)?;
    Ok(axum::Json(json!({ "ok": true, "doing": doing })))
}

/// Stop sharing a location live with them (they're told).
async fn stop_live(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    let key = address(&key)?;
    state.write(move |o| o.app.stop_live(&key)).await?.map_err(bad)?;
    ok()
}

#[derive(Deserialize)]
struct TrackBody {
    /// Minutes between asking where they are.
    minutes: u64,
}

/// Track them: ask where they are every few minutes.
async fn start_track(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<TrackBody>) -> ApiResult {
    let key = address(&key)?;
    let doing = state.write(move |o| o.app.start_track(&key, body.minutes)).await?.map_err(bad)?;
    Ok(axum::Json(json!({ "ok": true, "doing": doing })))
}

/// Stop tracking them.
async fn stop_track(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    let key = address(&key)?;
    state.write(move |o| o.app.stop_track(&key)).await?.map_err(bad)?;
    ok()
}

/// Where the device sharing its location live is now.
async fn live_position(State(state): State<WebState>, axum::Json(body): axum::Json<LocationBody>) -> ApiResult {
    let location = body.checked()?;
    state.write(move |o| o.app.device_position(location)).await?;
    ok()
}

/// Everyone's newest location, for the map.
async fn locations(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| views::locations(&o.app)).await?))
}

/// Delete what's known of where someone has been: their place and trail
/// go from the map (their messages stay).
async fn forget_locations(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    let key = crate::net::parse_hash(&key).map(hex::encode).ok_or_else(|| bad("An address is 32 hex characters"))?;
    state.write(move |o| o.app.forget_locations(&key)).await?;
    ok()
}

/// A few map tiles fetched at once, at most (the rest wait their turn):
/// what a map being looked at needs, not more.
static FETCHING_TILES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// A map tile, from the Map tiles setting's source: kept, or fetched and
/// kept (see `tiles`).
async fn map_tile(State(state): State<WebState>, Path((z, x, y)): Path<(u32, u32, u32)>) -> Result<Response, ApiError> {
    let (template, file) = state.read(|o| (o.app.settings.map_tiles.clone(), o.app.settings.map_tiles_file.clone())).await?;
    let cache = [(header::CACHE_CONTROL, "private, max-age=86400")];
    // The offline map's first.
    if let Some(file) = &file {
        let file = file.clone();
        let found = tokio::task::spawn_blocking(move || super::mbtiles::tile(std::path::Path::new(&file), z, x, y))
            .await
            .map_err(|e| bad(e.to_string()))?;
        match found {
            Ok(Some(tile)) => return Ok(([(header::CONTENT_TYPE, tile.kind)], cache, tile.data).into_response()),
            Ok(None) => {}
            Err(e) => tracing::debug!("offline map tile {z}/{x}/{y}: {e}"),
        }
    }
    // Else from the web; failing that (or with none set), a part of one
    // the offline map has further out, enlarged.
    let fetched = match template {
        Some(template) => web_tile(&state, template, z, x, y).await,
        None => Err(ApiError(StatusCode::NOT_FOUND, "no map tiles are set (Map tiles), and the offline map hasn't this one".into())),
    };
    match (fetched, file) {
        (Ok(tile), _) => Ok(([(header::CONTENT_TYPE, tile.kind)], cache, tile.data).into_response()),
        (Err(e), Some(file)) => {
            let enlarged = tokio::task::spawn_blocking(move || super::mbtiles::enlarged(std::path::Path::new(&file), z, x, y))
                .await
                .map_err(|e| bad(e.to_string()))?;
            match enlarged {
                Ok(Some(tile)) => Ok(([(header::CONTENT_TYPE, tile.kind)], cache, tile.data).into_response()),
                _ => Err(e),
            }
        }
        (Err(e), None) => Err(e),
    }
}

/// Tile `z`/`x`/`y` from `template` on the web, as kept if it's fresh.
async fn web_tile(state: &WebState, template: String, z: u32, x: u32, y: u32) -> Result<super::tiles::Tile, ApiError> {
    if super::tiles::tile_url(&template, z, x, y).is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "there's no such tile".into()));
    }
    let dir = state.paths.map_tiles.clone();
    let (kept_dir, kept_template) = (dir.clone(), template.clone());
    let kept = tokio::task::spawn_blocking(move || super::tiles::fresh(&kept_dir, &kept_template, z, x, y))
        .await
        .map_err(|e| bad(e.to_string()))?;
    match kept {
        Some(tile) => Ok(tile),
        None => {
            let _turn = FETCHING_TILES.acquire().await.map_err(|e| bad(e.to_string()))?;
            tokio::task::spawn_blocking(move || super::tiles::tile(&dir, &template, z, x, y))
                .await
                .map_err(|e| bad(e.to_string()))?
                .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))
        }
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    /// One conversation's key, to search only there.
    #[serde(rename = "in")]
    within: Option<String>,
}

#[derive(Deserialize)]
struct ChannelSearchQuery {
    q: String,
    /// One room's hub and buffer key, to search only there.
    #[serde(default)]
    hub: Option<String>,
    #[serde(default)]
    room: Option<String>,
}

/// Channel lines matching the search words, newest first (see
/// [`crate::app::channels::search`]).
async fn search_channels(State(state): State<WebState>, Query(query): Query<ChannelSearchQuery>) -> ApiResult {
    let within = match &query.hub {
        Some(hub) => Some((hub_hash(hub)?, query.room.clone().unwrap_or_default())),
        None => None,
    };
    let view = state.read(move |o| views::channel_search(&o.app, &query.q, within.as_ref().map(|(h, r)| (*h, r.as_str())))).await?;
    Ok(axum::Json(view))
}

/// Messages matching the search words, newest first (see
/// [`crate::app::search`]).
async fn search_messages(State(state): State<WebState>, Query(query): Query<SearchQuery>) -> ApiResult {
    Ok(axum::Json(state.read(move |o| views::search(&o.app, &query.q, query.within.as_deref())).await?))
}

#[derive(Deserialize)]
struct StateQuery {
    /// With the log (up to 500 lines): only Status shows it, and the state
    /// is fetched every few seconds on a busy network.
    log: Option<String>,
}

async fn get_state(State(state): State<WebState>, Query(query): Query<StateQuery>) -> ApiResult {
    let mut view = state.read(|o| views::state(&o.app)).await?;
    if query.log.is_none()
        && let Some(view) = view.as_object_mut()
    {
        view.remove("log");
    }
    // With --https and rettui's own certificate: devices install its
    // authority from the Status page.
    view["own_certificate"] = json!(state.ca.is_some());
    Ok(axum::Json(view))
}

#[derive(Deserialize)]
struct EventsQuery {
    /// The last notification this browser had (see [`events`]).
    since: Option<u64>,
    /// `1`: notifications only, not what changed (the Android app's
    /// service, which only shows notifications, isn't woken by every
    /// announce).
    notices: Option<String>,
}

/// A `notify` event: a notification, with its id (the browser's last event
/// id, for catching up). `replay`: one it missed.
fn notify_event(id: u64, notification: &Notification, replay: bool) -> Option<Event> {
    Event::default()
        .event("notify")
        .id(id.to_string())
        .json_data(json!({
            "title": notification.title,
            "body": notification.body,
            "target": notification.target,
            // Newer ones with the same tag replace older ones.
            "tag": notification.target.tag(),
            "replay": replay,
        }))
        .ok()
}

/// One event per change, `<version> <scope>` (browsers refetch what they
/// show of it); a `notify` event per notification (each browser decides
/// whether to show it); and a `read` event when what one was about is read
/// (browsers close it).
///
/// A browser reconnecting says the last notification it had (the
/// `Last-Event-ID` header, which browsers send by themselves, or `?since=`),
/// and first gets those it missed that are still unread.
async fn events(State(state): State<WebState>, Query(query): Query<EventsQuery>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    // Listening before looking back, so nothing falls in between.
    let receivers = (state.changes.subscribe(), state.notices.subscribe());
    let since = headers.get("last-event-id").and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok()).or(query.since);
    let missed = match since {
        Some(since) => state.read(move |o| o.missed(since)).await.unwrap_or_default(),
        None => Vec::new(),
    };
    // Live notifications from before these were sent already.
    let caught_up = missed.iter().map(|(id, _)| *id).max().unwrap_or(0);
    let changes_wanted = query.notices.as_deref() != Some("1");
    let replay: Vec<Result<Event, Infallible>> = missed.iter().filter_map(|(id, n)| notify_event(*id, n, true)).map(Ok).collect();
    // Every browser signed out since: the stream ends (and a reconnect
    // needs the new token).
    let mut signed_out = state.signed_out.subscribe();
    signed_out.mark_unchanged();
    let live = futures_util::stream::unfold((receivers, signed_out), move |((mut changes, mut notices), mut signed_out)| async move {
        use tokio::sync::broadcast::error::RecvError;
        let event = loop {
            tokio::select! {
                _ = signed_out.changed() => return None,
                change = changes.recv(), if changes_wanted => break match change {
                    Ok((version, scope)) => Event::default().data(format!("{version} {}", scope.name())),
                    // Missed some: one event covers them.
                    Err(RecvError::Lagged(_)) => Event::default().data("0 all"),
                    Err(RecvError::Closed) => return None,
                },
                notice = notices.recv() => match notice {
                    Ok(Notice::Show(id, _)) if id <= caught_up => continue,
                    Ok(Notice::Show(id, notification)) => match notify_event(id, &notification, false) {
                        Some(event) => break event,
                        None => continue,
                    },
                    Ok(Notice::Read(tag)) => match Event::default().event("read").json_data(json!({ "tag": tag })) {
                        Ok(event) => break event,
                        Err(_) => continue,
                    },
                    Ok(Notice::Play(voice)) => {
                        match Event::default().event("play").json_data(json!({ "key": voice.key, "id": voice.id, "index": voice.index })) {
                            Ok(event) => break event,
                            Err(_) => continue,
                        }
                    }
                    // Old news by now (a reconnect catches up on the unread).
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                },
            }
        };
        Some((Ok(event), ((changes, notices), signed_out)))
    });
    let stream = Sse::new(futures_util::StreamExt::chain(futures_util::stream::iter(replay), live)).keep_alive(KeepAlive::default());
    // Behind nginx, sent as they come rather than buffered (other proxies
    // pass event streams on as they are).
    ([(header::HeaderName::from_static("x-accel-buffering"), "no")], stream)
}

// ---- Web Push ---------------------------------------------------------------

/// This server's key, which browsers subscribe with.
async fn push_key(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(json!({ "key": state.push.public_key() })))
}

#[derive(Deserialize)]
struct PushKeys {
    p256dh: String,
    auth: String,
}

/// A browser's `PushSubscription`, as its `toJSON()` gives it.
#[derive(Deserialize)]
struct SubscribeBody {
    endpoint: String,
    keys: PushKeys,
}

async fn push_subscribe(State(state): State<WebState>, axum::Json(body): axum::Json<SubscribeBody>) -> ApiResult {
    let subscription = push::Subscription { endpoint: body.endpoint, p256dh: body.keys.p256dh, auth: body.keys.auth };
    state.push.subscribe(subscription)?;
    ok()
}

#[derive(Deserialize)]
struct EndpointBody {
    endpoint: String,
    #[serde(default)]
    showing: bool,
}

async fn push_unsubscribe(State(state): State<WebState>, axum::Json(body): axum::Json<EndpointBody>) -> ApiResult {
    state.push.unsubscribe(&body.endpoint)?;
    ok()
}

/// A browser with background notifications says whether it shows the web
/// UI (it's pushed to only when it doesn't).
async fn push_showing(State(state): State<WebState>, axum::Json(body): axum::Json<EndpointBody>) -> ApiResult {
    state.push.set_showing(&body.endpoint, body.showing);
    ok()
}

// ---- messages -------------------------------------------------------------

async fn conversations(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| views::conversations(&o.app)).await?))
}

/// Any destination's address (a peer, a node, a hub), as hex.
fn destination(text: &str) -> Result<String, ApiError> {
    parse_hash(text).map(hex::encode).ok_or_else(|| bad("An address is 32 hex characters"))
}

/// A path lookup as the web UI shows it.
fn path_json(lookup: Option<&crate::app::paths::PathLookup>) -> serde_json::Value {
    use crate::app::paths::{PathLookup, lookup_label};
    let Some(lookup) = lookup else { return json!({ "state": "none", "text": "not looked for yet" }) };
    let text = lookup_label(lookup, chrono::Utc::now().timestamp());
    match lookup {
        PathLookup::Waiting(_) => json!({ "state": "waiting", "text": text }),
        PathLookup::Done(Ok(info)) => json!({
            "state": "found", "text": text, "hops": info.hops, "interface": info.interface,
            "via": info.via.map(hex::encode), "expires": info.expires,
        }),
        PathLookup::Done(Err(_)) => json!({ "state": "failed", "text": text }),
    }
}

/// How finding a path to a destination is going (without asking).
async fn path_state(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    let key = destination(&key)?;
    Ok(axum::Json(state.read(move |o| path_json(o.app.path_lookups.get(&key))).await?))
}

/// Find the path to a destination, asking for one if it isn't known, and
/// wait (a while) for how it went.
async fn find_path(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    use crate::app::paths::PathLookup;
    let key = destination(&key)?;
    let asked = key.clone();
    state.write(move |o| o.app.find_path_to(&asked)).await?.map_err(bad)?;
    // Path requests go out over about a minute; this covers them.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(80);
    loop {
        let wanted = key.clone();
        let (done, json) = state
            .read(move |o| {
                let lookup = o.app.path_lookups.get(&wanted);
                (matches!(lookup, Some(PathLookup::Done(_))), path_json(lookup))
            })
            .await?;
        if done {
            return Ok(axum::Json(json));
        }
        if tokio::time::Instant::now() > deadline {
            return Err(bad("Still looking: the log will say how it went"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}

/// A probe as the web UI shows it.
fn probe_json(state: Option<&crate::app::paths::ProbeState>) -> serde_json::Value {
    use crate::app::paths::{ProbeState, probe_state_label};
    let Some(state) = state else { return json!({ "state": "none", "text": "not probed yet" }) };
    let text = probe_state_label(state);
    match state {
        ProbeState::Waiting(_) => json!({ "state": "waiting", "text": text }),
        ProbeState::Done(Ok(probe)) => json!({
            "state": "found", "text": text, "ms": probe.rtt.as_millis() as u64, "hops": probe.hops,
            "kind": probe.kind, "by_link": probe.by_link,
        }),
        ProbeState::Done(Err(_)) => json!({ "state": "failed", "text": text }),
    }
}

/// How probing a destination is going (without asking).
async fn probe_state(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    let key = destination(&key)?;
    Ok(axum::Json(state.read(move |o| probe_json(o.app.probes.get(&key))).await?))
}

/// Probe a destination, as rnprobe does, and wait (a while) for the answer.
async fn probe(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    use crate::app::paths::ProbeState;
    let key = destination(&key)?;
    let asked = key.clone();
    state.write(move |o| o.app.probe(&asked)).await?.map_err(bad)?;
    // Finding a path takes up to about a minute; then the probe or Link.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(110);
    loop {
        let wanted = key.clone();
        let (done, json) = state
            .read(move |o| {
                let probe = o.app.probes.get(&wanted);
                (matches!(probe, Some(ProbeState::Done(_))), probe_json(probe))
            })
            .await?;
        if done {
            return Ok(axum::Json(json));
        }
        if tokio::time::Instant::now() > deadline {
            return Err(bad("No answer yet: the log will say how it went"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}

/// Forget the path to a destination: the next use asks for a fresh one.
async fn forget_path(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    let key = destination(&key)?;
    state.write(move |o| o.app.forget_path(&key)).await?.map_err(bad)?;
    Ok(axum::Json(json!({})))
}

fn address(text: &str) -> Result<String, ApiError> {
    let text = text.trim();
    let text = text.strip_prefix("lxmf@").or_else(|| text.strip_prefix("lxmf://")).unwrap_or(text);
    parse_hash(text).map(hex::encode).ok_or_else(|| bad("An LXMF address is 32 hex characters"))
}

#[derive(Deserialize)]
struct AddressBody {
    address: String,
}

/// A conversation with an address, or an `lxma://` link (whose public key
/// is remembered, so you can write before they announce).
async fn new_conversation(State(state): State<WebState>, axum::Json(body): axum::Json<AddressBody>) -> ApiResult {
    let key = state.write(move |o| o.app.add_contact(&body.address)).await?.map_err(bad)?;
    Ok(axum::Json(json!({ "key": key })))
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

/// Mark every conversation read.
async fn read_all_conversations(State(state): State<WebState>) -> ApiResult {
    let marked = state.write(|o| o.app.mark_all_read()).await?;
    Ok(axum::Json(json!({ "ok": true, "marked": marked })))
}

#[derive(Deserialize)]
struct PinBody {
    pinned: bool,
}

/// Pin a conversation to the top of the list, or unpin it.
async fn pin_conversation(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<PinBody>) -> ApiResult {
    let key = address(&key)?;
    state.write(move |o| o.app.set_pinned(&key, body.pinned)).await?.map_err(bad)?;
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
struct AutoplayBody {
    on: bool,
}

/// Play a conversation's voice messages as they come, or not.
async fn autoplay_conversation(State(state): State<WebState>, Path(key): Path<String>, body: axum::Json<AutoplayBody>) -> ApiResult {
    let on = body.on;
    if !state.write(move |o| o.app.set_conversation_autoplay(&key, on)).await? {
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
    /// The id of the message it replies to.
    #[serde(default)]
    reply_to: Option<String>,
    /// A voice message recorded in the page: a WAV file (8 kHz mono 16-bit),
    /// base64. It's sent as Codec2.
    #[serde(default)]
    voice: Option<String>,
    /// A location to share (sent as Sideband sends one).
    #[serde(default)]
    location: Option<LocationBody>,
}

#[derive(Deserialize)]
struct LocationBody {
    latitude: f64,
    longitude: f64,
    /// Metres, as the browser gives them.
    #[serde(default)]
    accuracy: Option<f64>,
    #[serde(default)]
    altitude: Option<f64>,
}

impl LocationBody {
    fn checked(self) -> Result<crate::lxmf::Location, ApiError> {
        let mut at = crate::lxmf::Location::parse(&format!("{}, {}", self.latitude, self.longitude))
            .ok_or_else(|| bad("A location is a latitude from -90 to 90 and a longitude from -180 to 180"))?;
        at.accuracy = self.accuracy.filter(|a| a.is_finite() && *a >= 0.0);
        at.altitude = self.altitude.filter(|a| a.is_finite());
        Ok(at)
    }
}

async fn send_message(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<SendBody>) -> ApiResult {
    let key = address(&key)?;
    let mode = DeliveryMode::parse(&body.mode).ok_or_else(|| bad(format!("unknown delivery mode {}", body.mode)))?;
    if let Some(location) = body.location {
        if !body.files.is_empty() || body.voice.is_some() {
            return Err(bad("A location goes on its own, or with text"));
        }
        let location = location.checked()?;
        let (content, reply) = (body.content, body.reply_to);
        let id = state.write(move |o| o.app.share_location(key, location, content, mode, reply)).await?.map_err(bad)?;
        return Ok(axum::Json(json!({ "ok": true, "id": format!("local-{id}") })));
    }
    if let Some(recording) = &body.voice {
        if !body.files.is_empty() {
            return Err(bad("A voice message goes on its own, or with text, not with files"));
        }
        let recording =
            base64::engine::general_purpose::STANDARD.decode(recording.as_bytes()).map_err(|_| bad("The recording isn't valid base64"))?;
        let (content, reply) = (body.content, body.reply_to);
        let id = state.write(move |o| o.app.send_voice(key, content, &recording, mode, reply)).await?.map_err(bad)?;
        return Ok(axum::Json(json!({ "ok": true, "id": format!("local-{id}") })));
    }
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
    // Pictures made smaller (as the setting has it), away from the app:
    // it takes a while with photos.
    if files.iter().any(|f| crate::lxmf::is_image_path(f)) && mode != DeliveryMode::Paper {
        let size = state.read(|o| o.app.settings.picture_size.clone()).await?;
        files = tokio::task::spawn_blocking(move || crate::app::shrink::shrink_files(files, &size, &dir))
            .await
            .map_err(|e| bad(e.to_string()))?;
    }
    let result =
        state.write(move |o| o.app.send_message(key, body.content, files, mode, body.reply_to).map_err(|(e, _, files)| (e, files))).await?;
    match result {
        // Its id: a paper message's link shows on it once written.
        Ok(id) => Ok(axum::Json(json!({ "ok": true, "id": format!("local-{id}") }))),
        Err((e, files)) => {
            for file in files {
                let _ = tokio::fs::remove_file(file).await;
            }
            Err(bad(e))
        }
    }
}

#[derive(Deserialize)]
struct ReactBody {
    /// The message reacted to (its id).
    id: String,
    emoji: String,
}

/// React to a message (sent like one, with no text).
async fn react(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<ReactBody>) -> ApiResult {
    let key = address(&key)?;
    state.write(move |o| o.app.send_reaction(&key, &body.id, &body.emoji)).await?.map_err(bad)?;
    ok()
}

#[derive(Deserialize)]
struct CommandBody {
    /// As typed in Sideband's command box: `ping`, `echo <text>`, `signal`
    /// or `location`.
    command: String,
}

/// Send a Sideband command (sent like a message, with no text).
async fn send_command(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<CommandBody>) -> ApiResult {
    let key = address(&key)?;
    let command = crate::lxmf::fields::Command::parse(&body.command).map_err(bad)?;
    let id = state.write(move |o| o.app.send_command(&key, command)).await?.map_err(bad)?;
    Ok(axum::Json(json!({ "id": format!("local-{id}") })))
}

#[derive(Deserialize)]
struct RetryBody {
    id: String,
    #[serde(default)]
    mode: String,
}

#[derive(Deserialize)]
struct DeliveryBody {
    mode: String,
}

/// Keep how messages to someone go (paper isn't kept: it's for one message).
async fn set_delivery(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<DeliveryBody>) -> ApiResult {
    let key = address(&key)?;
    let mode = DeliveryMode::parse(&body.mode).ok_or_else(|| bad(format!("unknown delivery mode {}", body.mode)))?;
    state.write(move |o| o.app.set_delivery(&key, mode)).await?.map_err(bad)?;
    ok()
}

#[derive(Deserialize)]
struct GamesQuery {
    /// Only the games with this address (hex).
    #[serde(default)]
    peer: Option<String>,
}

/// Your games (with someone, or with anyone), newest first, each with who
/// it's with: their name, and their icon if they have one.
async fn games(State(state): State<WebState>, Query(query): Query<GamesQuery>) -> ApiResult {
    let peer = query.peer.as_deref().map(address).transpose()?;
    let games = state
        .read(move |o| {
            o.app
                .games
                .list(peer.as_deref())
                .into_iter()
                .map(|game| {
                    let mut value = json!(game);
                    value["peer_name"] = json!(o.app.store.display_name(&game.peer));
                    value["icon"] = views::icon(o.app.store.contacts.get(&game.peer).and_then(|c| c.icon.as_ref()));
                    value
                })
                .collect::<Vec<_>>()
        })
        .await?;
    Ok(axum::Json(json!(games)))
}

async fn game(State(state): State<WebState>, Path((app, session)): Path<(String, String)>) -> ApiResult {
    let view = state
        .read(move |o| {
            o.app.games.view(&app, &session).map(|game| {
                let mut value = json!(game);
                value["peer_name"] = json!(o.app.store.display_name(&game.peer));
                // How your last move went, and what came with theirs.
                value["notes"] = json!(o.app.game_notes(&game));
                value["unsent"] = json!(o.app.game_unsent(&game));
                value["rules"] = json!(crate::games::game_rules(&game.app));
                value
            })
        })
        .await?;
    Ok(axum::Json(view.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "There's no such game".into()))?))
}

#[derive(Deserialize)]
struct StartGame {
    peer: String,
    app: String,
}

/// Challenge someone: the new game's session.
async fn start_game(State(state): State<WebState>, axum::Json(body): axum::Json<StartGame>) -> ApiResult {
    let peer = address(&body.peer)?;
    let session = state.write(move |o| o.app.start_game(&peer, &body.app)).await?.map_err(bad)?;
    Ok(axum::Json(json!({ "session": session })))
}

#[derive(Deserialize)]
struct GameAction {
    command: String,
    /// A move: a cell, a column, a chess move (`e2e4`), a checkers move
    /// (`a5 c3 e1`), a square (`h8`, or Hex's `swap`, or Go's `pass`), a line
    /// (`c3h`), a pit (`c`), a move by its squares (`a2b3`, `d1d7/g7`,
    /// `d2-d3xg7`), a wall (`c3h`), a shot (`e5`) or a throw (`r`, `p`, `s`);
    /// empty, to concede or show a Battleship fleet or a match's last throw.
    #[serde(default)]
    picked: Option<String>,
}

async fn game_action(
    State(state): State<WebState>,
    Path((app, session)): Path<(String, String)>,
    axum::Json(body): axum::Json<GameAction>,
) -> ApiResult {
    state.write(move |o| o.app.game_action(&app, &session, &body.command, body.picked.as_deref())).await?.map_err(bad)?;
    ok()
}

async fn forget_game(State(state): State<WebState>, Path((app, session)): Path<(String, String)>) -> ApiResult {
    state.write(move |o| o.app.forget_game(&app, &session)).await?.map_err(bad)?;
    ok()
}

/// Send a message that failed again (the same message), by `mode`.
async fn retry(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<RetryBody>) -> ApiResult {
    let key = address(&key)?;
    let mode = DeliveryMode::parse(&body.mode).ok_or_else(|| bad(format!("unknown delivery mode {}", body.mode)))?;
    state.write(move |o| o.app.retry_message(&key, &body.id, mode)).await?.map_err(bad)?;
    ok()
}

#[derive(Deserialize)]
struct MessageId {
    id: String,
}

/// Stop sending a message.
async fn cancel_send(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<MessageId>) -> ApiResult {
    let key = address(&key)?;
    state.write(move |o| o.app.cancel_send(&key, &body.id)).await?.map_err(bad)?;
    ok()
}

/// Delete a message (and the files rettui saved for it).
async fn delete_message(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<MessageId>) -> ApiResult {
    let key = address(&key)?;
    state.write(move |o| o.app.delete_message(&key, &body.id)).await?.map_err(bad)?;
    ok()
}

/// Delete a conversation, its archived messages and the files rettui saved
/// for it.
async fn delete_conversation(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    let key = address(&key)?;
    if !state.write(move |o| o.app.delete_conversation(&key)).await? {
        return Err(not_found("conversation"));
    }
    ok()
}

#[derive(Deserialize)]
struct ContactBody {
    /// Your name for them; empty uses the one they announce.
    #[serde(default)]
    alias: String,
    #[serde(default)]
    notes: String,
}

/// Ping someone, and wait (a while) for how it went.
async fn ping(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    use crate::app::contacts::{PingState, ping_label};
    let key = address(&key)?;
    let asked = key.clone();
    state.write(move |o| o.app.ping(&asked)).await?.map_err(bad)?;
    // A path request and a Link each wait up to a while; this covers both.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(70);
    loop {
        let wanted = key.clone();
        let ping = state.read(move |o| o.app.pings.get(&wanted).cloned()).await?;
        match ping {
            Some(PingState::Done { at, result }) => {
                let answer = result.as_ref().ok().copied();
                let text = ping_label(&PingState::Done { at, result });
                return Ok(axum::Json(json!({
                    "ok": answer.is_some(),
                    "text": text,
                    "ms": answer.map(|p| p.rtt.as_millis() as u64),
                    "hops": answer.and_then(|p| p.hops),
                })));
            }
            _ if tokio::time::Instant::now() > deadline => return Err(bad("No answer yet: the log will say how it went")),
            _ => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    }
}

async fn save_contact(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<ContactBody>) -> ApiResult {
    let key = address(&key)?;
    state
        .write(move |o| {
            o.app.set_contact_name(&key, &body.alias)?;
            o.app.set_contact_notes(&key, &body.notes)
        })
        .await?
        .map_err(bad)?;
    ok()
}

#[derive(Deserialize)]
struct TrustBody {
    /// `trusted`, `untrusted` (left as is), `unknown` or `blocked`.
    trust: String,
}

/// Trust someone, leave them as is, stop trusting them, block them (which
/// deletes the conversation) or unblock them.
async fn set_trust(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<TrustBody>) -> ApiResult {
    let key = address(&key)?;
    let trust = crate::store::Trust::parse(&body.trust).ok_or_else(|| bad(format!("unknown trust {}", body.trust)))?;
    state.write(move |o| o.app.set_trust(&key, trust)).await?.map_err(bad)?;
    ok()
}

#[derive(Deserialize)]
struct PaperBody {
    /// The `lxm://` link (from a QR code, or pasted).
    link: String,
}

/// Read in a paper message: the conversation it goes to, answered at once
/// (it's decrypted here to see), and it arrives there like one received.
async fn read_paper(State(state): State<WebState>, axum::Json(body): axum::Json<PaperBody>) -> ApiResult {
    read_paper_link(&state, body.link.trim().to_string()).await
}

#[derive(Deserialize)]
struct PictureBody {
    /// The picture, base64.
    image: String,
}

/// Read in a paper message from a picture of its QR code (for browsers
/// that can't find one themselves, or can't use the camera).
async fn scan_paper(State(state): State<WebState>, axum::Json(body): axum::Json<PictureBody>) -> ApiResult {
    let picture =
        base64::engine::general_purpose::STANDARD.decode(body.image.as_bytes()).map_err(|_| bad("The picture is not valid base64"))?;
    let link =
        tokio::task::spawn_blocking(move || crate::lxmf::paper::scan(&picture)).await.map_err(|e| bad(e.to_string()))?.map_err(bad)?;
    read_paper_link(&state, link).await
}

async fn read_paper_link(state: &WebState, link: String) -> ApiResult {
    // A contact's link (or its QR code) is taken here too.
    if link.trim().starts_with("lxma://") {
        let key = state.write(move |o| o.app.add_contact(&link)).await?.map_err(bad)?;
        return Ok(axum::Json(json!({ "key": key, "known": false, "contact": true })));
    }
    let (own, ratchets) = state.read(|o| (o.app.lxmf_hash, o.app.paths.ratchets.clone())).await?;
    let own = own.ok_or_else(|| bad("Reticulum is still starting"))?;
    let ring = crate::lxmf::ratchets::ring_path(&ratchets, &own);
    let data = crate::lxmf::paper::open(&state.identity, own, &ring, &link).map_err(bad)?;
    let message = lxmf_core::message_api::LxMessage::unpack(&data).map_err(|_| bad("Not a message rettui can read"))?;
    let key = hex::encode(message.source_hash);
    // Read in before: said, rather than nothing happening.
    let id = message.message_id.or(message.hash).map(hex::encode);
    let known = state
        .write(move |o| {
            let conversation = o.app.store.conversations.get(&key);
            let known = id.is_some_and(|id| conversation.is_some_and(|c| c.messages.iter().any(|m| m.id == id)));
            if !known {
                o.app.read_paper(&link)?;
            }
            Ok::<_, String>((key, known))
        })
        .await?;
    let (key, known) = known.map_err(bad)?;
    Ok(axum::Json(json!({ "key": key, "known": known })))
}

#[derive(Deserialize)]
struct TextQuery {
    text: String,
}

/// A QR code of `text`, as SVG (a paper message's link).
async fn qr_code(Query(query): Query<TextQuery>) -> Result<Response, ApiError> {
    let svg = crate::lxmf::paper::Qr::new(&query.text).map_err(bad)?.svg();
    Ok(([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "private, max-age=86400")], svg).into_response())
}

/// Header-safe file name.
fn file_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control() && *c != '"' && *c != '\\').collect();
    if name.is_empty() { "download".into() } else { name }
}

fn file_response(data: Vec<u8>, name: &str, inline: bool) -> Response {
    // Voice messages (Opus in an Ogg file, or Codec2 decoded to WAV) play
    // in the page.
    let content_type = if data.starts_with(b"OggS") {
        "audio/ogg"
    } else if crate::lxmf::voice::is_wav(&data) {
        "audio/wav"
    } else {
        image::guess_format(&data).map(|f| f.to_mime_type()).unwrap_or("application/octet-stream")
    };
    let shown = content_type.starts_with("image/") || content_type.starts_with("audio/");
    let disposition = format!("{}; filename=\"{}\"", if inline && shown { "inline" } else { "attachment" }, file_name(name));
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

/// A conversation's messages in the archive (see `store::read_archive`),
/// oldest first; read away from the app, as there may be many months.
async fn conversation_archive(State(state): State<WebState>, Path(key): Path<String>) -> ApiResult {
    let key = address(&key)?;
    let dir = state.paths.archive.clone();
    let messages = tokio::task::spawn_blocking(move || crate::store::read_archive(&dir, &key))
        .await
        .map_err(|e| bad(e.to_string()))?
        .map_err(|e| bad(format!("Couldn't read the archive: {e}")))?;
    Ok(axum::Json(views::archived(messages)))
}

/// A conversation as text, archived messages and all, to save.
async fn export_conversation(State(state): State<WebState>, Path(key): Path<String>) -> Result<Response, ApiError> {
    let key = address(&key)?;
    let wanted = key.clone();
    let (name, kept) = state
        .read(move |o| {
            (o.app.store.display_name(&wanted), o.app.store.conversations.get(&wanted).map(|c| c.messages.clone()).unwrap_or_default())
        })
        .await?;
    let dir = state.paths.archive.clone();
    let (text, file) = tokio::task::spawn_blocking(move || {
        let archived = crate::store::read_archive(&dir, &key).map_err(|e| format!("Couldn't read the archive: {e}"))?;
        let text = crate::app::forward::transcript(&name, &key, archived.iter().chain(&kept));
        let stamp = chrono::Local::now().format("%Y-%m-%d");
        Ok::<_, String>((text, format!("{name}-{stamp}.txt")))
    })
    .await
    .map_err(|e| bad(e.to_string()))?
    .map_err(bad)?;
    let disposition = format!("attachment; filename=\"{}\"", file_name(&file));
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, disposition)], text)
        .into_response())
}

#[derive(Deserialize)]
struct ForwardBody {
    /// The message forwarded (its id), and who to.
    id: String,
    to: String,
}

/// Send a message of this conversation on to someone else: its text and
/// copies of its files.
async fn forward_message(State(state): State<WebState>, Path(key): Path<String>, axum::Json(body): axum::Json<ForwardBody>) -> ApiResult {
    let key = address(&key)?;
    let to = address(&body.to)?;
    let id = state.write(move |o| o.app.forward_message(&key, &body.id, &to)).await?.map_err(bad)?;
    Ok(axum::Json(json!({ "ok": true, "id": format!("local-{id}") })))
}

async fn attachment(State(state): State<WebState>, Path((key, id, index)): Path<(String, String, usize)>) -> Result<Response, ApiError> {
    let (wanted, asked) = (key.clone(), id.clone());
    let found = state
        .read(move |o| {
            let message = o.app.store.conversations.get(&key)?.messages.iter().find(|m| m.id == id)?;
            let attachment = message.attachments.get(index)?;
            Some((attachment.path.clone(), attachment.name.clone()))
        })
        .await?;
    // Not among those kept: an archived message's, maybe.
    let found = match found {
        Some(found) => Some(found),
        None => {
            let (dir, wanted) = (state.paths.archive.clone(), address(&wanted)?);
            tokio::task::spawn_blocking(move || {
                let messages = crate::store::read_archive(&dir, &wanted).ok()?;
                let attachment = messages.into_iter().find(|m| m.id == asked)?.attachments.into_iter().nth(index)?;
                Some((attachment.path, attachment.name))
            })
            .await
            .ok()
            .flatten()
        }
    };
    let (path, name) = found.ok_or_else(|| not_found("attachment"))?;
    let data = tokio::fs::read(&path).await.map_err(|_| not_found("attachment file"))?;
    Ok(file_response(data, &name, true))
}

// ---- network and status ---------------------------------------------------

#[derive(Deserialize)]
struct PeersQuery {
    /// `lxmf`, `nomad` or `propagation`; every kind when left out. `blocked`:
    /// blocked contacts, heard or not.
    kind: Option<String>,
    #[serde(default)]
    q: String,
    limit: Option<usize>,
    /// `heard` (the default), `name` or `hops`.
    sort: Option<String>,
    /// Only those whose path goes through this interface.
    via: Option<String>,
}

async fn peers(State(state): State<WebState>, Query(query): Query<PeersQuery>) -> ApiResult {
    let sort = query.sort.as_deref().and_then(crate::app::network::NetSort::parse).unwrap_or_default();
    Ok(axum::Json(
        state
            .read(move |o| {
                views::peers(&o.app, query.kind.as_deref(), &query.q, query.limit, sort, query.via.as_deref().filter(|v| !v.is_empty()))
            })
            .await?,
    ))
}

#[derive(Deserialize)]
struct AnnouncesQuery {
    /// Only those after this one (by its `seq`).
    after: Option<u64>,
    /// `lxmf`, `nomad`, `propagation` or `other`; every kind when left out.
    kind: Option<String>,
    #[serde(default)]
    q: String,
    /// Only those whose path goes through this interface.
    via: Option<String>,
}

/// The announce viewer's: every announce heard, oldest first.
async fn announces(State(state): State<WebState>, Query(query): Query<AnnouncesQuery>) -> ApiResult {
    let filter = match query.kind.as_deref().filter(|k| !k.is_empty()) {
        None => crate::app::announces::HeardFilter::All,
        Some(kind) => crate::app::announces::HeardFilter::parse(kind).ok_or_else(|| bad(format!("unknown kind {kind}")))?,
    };
    let via = query.via.filter(|v| !v.is_empty());
    Ok(axum::Json(state.read(move |o| views::announces(&o.app, query.after, filter, &query.q, via.as_deref())).await?))
}

#[derive(Deserialize)]
struct TablesQuery {
    /// `paths` (the default) or `rates`.
    table: Option<String>,
    #[serde(default)]
    q: String,
    limit: Option<usize>,
}

/// Most rows of a table sent at once.
const MAX_TABLE_ROWS: usize = 2000;

/// Reticulum's path table or announce-rate table, searched. Asking keeps
/// them read every few seconds for a while.
async fn tables(State(state): State<WebState>, Query(query): Query<TablesQuery>) -> ApiResult {
    let which = match query.table.as_deref().filter(|t| !t.is_empty()) {
        None => crate::app::tables::Table::Paths,
        Some(table) => crate::app::tables::Table::parse(table).ok_or_else(|| bad(format!("unknown table {table}")))?,
    };
    let limit = query.limit.unwrap_or(500).min(MAX_TABLE_ROWS);
    let view = state
        .write(move |o| {
            o.app.tables_wanted();
            views::tables(&o.app, which, &query.q, limit)
        })
        .await?;
    Ok(axum::Json(view))
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

// ---- getting started ----------------------------------------------------------

/// The guide, trying its entry points as it opens.
async fn guide(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(
        state
            .write(|o| {
                o.app.check_entry_points();
                views::guide(&o.app)
            })
            .await?,
    ))
}

async fn guide_reach(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| views::guide_reach(&o.app)).await?))
}

#[derive(Deserialize)]
struct GuideBody {
    name: String,
    /// For each entry point offered, in order: connect through it.
    #[serde(default)]
    connect: Vec<bool>,
    #[serde(default)]
    discover: bool,
    #[serde(default)]
    auto_propagation: bool,
    /// Missing (an older page): off, as it is to start with.
    #[serde(default)]
    update_check: bool,
}

/// Apply the guide's choices (Reticulum restarts if its config changes).
async fn apply_guide(State(state): State<WebState>, axum::Json(body): axum::Json<GuideBody>) -> ApiResult {
    let choices = crate::app::guide::GuideChoices {
        name: body.name,
        connect: body.connect,
        discover: body.discover,
        auto_propagation: body.auto_propagation,
        update_check: body.update_check,
    };
    let done = state.write(move |o| o.app.apply_guide(&choices, true)).await??;
    Ok(axum::Json(json!({ "done": done })))
}

async fn dismiss_guide(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.finish_guide()).await?;
    ok()
}

async fn hide_first_steps(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.hide_first_steps()).await?;
    ok()
}

/// The user saved a copy of the identity themselves (the web UI can't).
async fn identity_backed_up(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.identity_backed_up()).await?;
    ok()
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
                    let identity =
                        hex::decode(body.src.trim()).ok().filter(|id| id.len() == 16).ok_or_else(|| bad("not a user identity"))?;
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
                .map(|l| format!("api/media?url={}", query_escape(&l.url())))
        },
    );
    let url = location.url();
    let saved_url = url.clone();
    let (node_name, identified, saved) = state
        .read(move |o| {
            (o.app.store.display_name(&hex::encode(node)), o.app.identifies_to(node), o.app.store.saved.iter().any(|b| b.url == saved_url))
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

/// Where on the node: `pages` (the default) or `files`.
fn node_root(id: Option<&str>) -> Result<Root, ApiError> {
    Root::from_id(id.unwrap_or("pages")).ok_or_else(|| bad("Not pages or files"))
}

#[derive(Deserialize)]
struct RenameBody {
    from: String,
    to: String,
    #[serde(default)]
    root: Option<String>,
}

/// Rename (or move) something on the node; links to it in the pages
/// follow. Never a script, from here.
async fn node_rename(State(state): State<WebState>, axum::Json(body): axum::Json<RenameBody>) -> ApiResult {
    let root = node_root(body.root.as_deref())?;
    let path = state.write(move |o| o.app.node_rename_in(root, &body.from, &body.to, false)).await??;
    Ok(axum::Json(json!({ "path": path })))
}

#[derive(Deserialize)]
struct NodeItemBody {
    path: String,
    #[serde(default)]
    root: Option<String>,
    /// For a move: the folder (empty: the top).
    #[serde(default)]
    folder: String,
}

async fn node_delete(State(state): State<WebState>, axum::Json(body): axum::Json<NodeItemBody>) -> ApiResult {
    let root = node_root(body.root.as_deref())?;
    state.write(move |o| o.app.node_delete_in(root, &body.path)).await??;
    ok()
}

async fn node_move(State(state): State<WebState>, axum::Json(body): axum::Json<NodeItemBody>) -> ApiResult {
    let root = node_root(body.root.as_deref())?;
    let path = state.write(move |o| o.app.node_move(root, &body.path, &body.folder, false)).await??;
    Ok(axum::Json(json!({ "path": path })))
}

#[derive(Deserialize)]
struct UploadQuery {
    name: String,
}

/// A picture or a file added to the node (its bytes as the body): a
/// picture goes with the pages, made smaller, anything else is a file to
/// download. Where pages find it.
async fn node_upload(State(state): State<WebState>, Query(query): Query<UploadQuery>, body: axum::body::Bytes) -> ApiResult {
    // A picture is made smaller before the app is held.
    let upload = tokio::task::spawn_blocking(move || crate::app::node::Upload::prepare(&query.name, &body))
        .await
        .map_err(|e| ApiError::from(e.to_string()))??;
    let added = state.write(move |o| o.app.node_add_upload(upload)).await??;
    Ok(axum::Json(json!({ "address": added.address, "image": added.image, "shrunk": added.shrunk })))
}

#[derive(Deserialize)]
struct PartialBody {
    url: String,
    /// The form values and variables it asks for, as a form sends them.
    #[serde(default)]
    fields: BTreeMap<String, String>,
}

/// A page's partial, loaded on its own (never from the cache): its HTML.
/// Partials in it aren't loaded.
async fn partial(State(state): State<WebState>, axum::Json(body): axum::Json<PartialBody>) -> ApiResult {
    let mut location = location(&body.url)?;
    location.fields = body.fields;
    let node = location.node;
    let fetched = state.fetch(location, true).await?;
    let page = micron::parse_partial(&String::from_utf8_lossy(&fetched.data));
    let html = page.to_html(
        |url| link_target(url, node),
        |url| {
            resolve_url(url, Some(node))
                .filter(|l| l.path.starts_with("/media/"))
                .map(|l| format!("api/media?url={}", query_escape(&l.url())))
        },
    );
    Ok(axum::Json(json!({ "html": html })))
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
                Some(rel) if location.node == node => format!("api/node/media?path={}", query_escape(rel)),
                _ => format!("api/media?url={}", query_escape(&location.url())),
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

#[derive(Deserialize)]
struct AllowedQuery {
    path: String,
    #[serde(default)]
    root: Option<String>,
}

/// Who may see something on the node: `everyone` (no list), `only` those
/// ticked in `people`, or `script` (a list that's a program, changed on the
/// host). `people` are contacts and those talked with, and anyone else on
/// the list.
async fn node_allowed(State(state): State<WebState>, Query(query): Query<AllowedQuery>) -> ApiResult {
    let root = node_root(query.root.as_deref())?;
    let (allowed, people) = state.read(move |o| o.app.node_allowed(root, &query.path)).await??;
    let who = match allowed {
        Allowed::Everyone => "everyone",
        Allowed::Only(_) => "only",
        Allowed::Script => "script",
    };
    let people: Vec<Value> =
        people.iter().map(|p| json!({ "identity": hex::encode(p.identity), "name": p.name, "key": p.key, "on": p.on })).collect();
    Ok(axum::Json(json!({ "who": who, "people": people })))
}

#[derive(Deserialize)]
struct SetAllowedBody {
    path: String,
    #[serde(default)]
    root: Option<String>,
    /// Anyone may: the list is removed.
    #[serde(default)]
    everyone: bool,
    /// Otherwise only these identities (hex).
    #[serde(default)]
    identities: Vec<String>,
}

async fn set_node_allowed(State(state): State<WebState>, axum::Json(body): axum::Json<SetAllowedBody>) -> ApiResult {
    let root = node_root(body.root.as_deref())?;
    let only = if body.everyone {
        None
    } else {
        if body.identities.len() > crate::app::allowed::MAX_LISTED {
            return Err(bad(format!("A list has at most {} people", crate::app::allowed::MAX_LISTED)));
        }
        Some(body.identities.iter().map(|i| parse_hash(i).ok_or_else(|| bad("Not an identity hash"))).collect::<Result<Vec<_>, _>>()?)
    };
    state.write(move |o| o.app.set_node_allowed(root, &body.path, only)).await??;
    ok()
}

/// The speed test: both addresses, the size, how it's going and what
/// happened. Asking keeps rettui taking tests from them a while.
async fn speed_test(State(state): State<WebState>) -> ApiResult {
    state
        .write(|o| {
            o.app.speed_wanted();
            let test = &o.app.speed;
            axum::Json(json!({
                "address": hex::encode(o.app.speed_address()),
                "peer": test.peer.map(hex::encode),
                "listening": test.listening.is_some(),
                "size": test.size,
                "sizes": crate::app::speedtest::SIZES.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
                "sending": test.sending,
                "receiving": test.receiving,
                "lines": test.lines,
            }))
        })
        .await
        .map_err(ApiError::from)
}

#[derive(Deserialize)]
struct SpeedBody {
    #[serde(default)]
    peer: Option<String>,
    #[serde(default)]
    size: Option<usize>,
}

async fn set_speed_test(State(state): State<WebState>, axum::Json(body): axum::Json<SpeedBody>) -> ApiResult {
    state
        .write(move |o| {
            if let Some(peer) = &body.peer {
                o.app.speed_set_peer(peer)?;
            }
            if let Some(size) = body.size {
                o.app.speed_set_size(size)?;
            }
            Ok::<_, String>(())
        })
        .await??;
    ok()
}

async fn send_speed_test(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.speed_send()).await??;
    ok()
}

/// Your addresses, and whose the address looked up last is, as rows.
async fn identities(State(state): State<WebState>) -> ApiResult {
    state.read(|o| axum::Json(json!({ "rows": o.app.whois_rows() }))).await.map_err(ApiError::from)
}

#[derive(Deserialize)]
struct LookupBody {
    address: String,
}

async fn look_up_identity(State(state): State<WebState>, axum::Json(body): axum::Json<LookupBody>) -> ApiResult {
    state.write(move |o| o.app.whois(&body.address)).await??;
    ok()
}

/// Files sent to rncp listeners lately, the identity they must allow, and
/// the largest that may be sent.
async fn rncp_sends(State(state): State<WebState>) -> ApiResult {
    use crate::net::RncpState;
    state
        .read(|o| {
            let sends: Vec<Value> = o
                .app
                .rncp
                .iter()
                .map(|s| {
                    let (state, progress, error) = match &s.state {
                        RncpState::Sending(done) => ("sending", *done, None),
                        RncpState::Sent { .. } => ("sent", 1.0, None),
                        RncpState::Failed(e) => ("failed", 0.0, Some(e.clone())),
                    };
                    json!({
                        "to": s.to, "to_name": o.app.store.display_name(&s.to), "name": s.name, "bytes": s.bytes, "at": s.at,
                        "state": state, "progress": progress, "error": error,
                    })
                })
                .collect();
            axum::Json(json!({ "sends": sends, "identity": hex::encode(o.app.identity_hash), "max": crate::app::rncp::MAX_SEND }))
        })
        .await
        .map_err(ApiError::from)
}

#[derive(Deserialize)]
struct RncpQuery {
    to: String,
    name: String,
}

/// Send what the browser picked (never a file on this computer) to an
/// rncp listener.
async fn rncp_send(State(state): State<WebState>, Query(query): Query<RncpQuery>, body: axum::body::Bytes) -> ApiResult {
    state.write(move |o| o.app.rncp_send(&query.to, &query.name, body.to_vec())).await??;
    ok()
}

async fn reset_node_stats(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.reset_node_stats()).await?;
    ok()
}

async fn node_announce(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.send(NetCommand::HostAnnounce)).await?;
    ok()
}

// ---- rngit repositories (the Repos section) ------------------------------

async fn repos(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| views::repos(&o.app)).await?))
}

async fn add_repo(State(state): State<WebState>, axum::Json(body): axum::Json<UrlBody>) -> ApiResult {
    let url = state.write(move |o| o.app.add_repo(&body.url)).await??;
    Ok(axum::Json(json!({ "url": url })))
}

#[derive(Deserialize)]
struct PageSource {
    source: String,
}

/// Keep every repository a page links to. The browser sends the page's
/// source; what's kept is only what `rns://` links in it say.
async fn repos_on_page(State(state): State<WebState>, axum::Json(body): axum::Json<PageSource>) -> ApiResult {
    let added = state.write(move |o| o.app.add_repos_linked(&body.source)).await?.map_err(bad)?;
    Ok(axum::Json(json!({ "added": added })))
}

#[derive(Deserialize)]
struct RepoPages {
    node: String,
    /// The NomadNet node with its pages; left out to forget it.
    #[serde(default)]
    pages: Option<String>,
}

/// Keep which NomadNet node has an rngit node's pages.
async fn set_repo_pages(State(state): State<WebState>, axum::Json(body): axum::Json<RepoPages>) -> ApiResult {
    let node = crate::net::parse_hash(&body.node).ok_or_else(|| bad("A node is 32 hex characters"))?;
    let pages = match body.pages.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(text) => Some(crate::net::parse_hash(text.trim_matches(['<', '>'])).ok_or_else(|| bad("An address is 32 hex characters"))?),
        None => None,
    };
    state.write(move |o| o.app.set_page_node(node, pages)).await?;
    ok()
}

async fn remove_repo(State(state): State<WebState>, axum::Json(body): axum::Json<UrlBody>) -> ApiResult {
    state.write(move |o| o.app.remove_repo(&body.url)).await?;
    ok()
}

/// Make a copy of a repository on this computer, or bring it up to date.
/// The browser says which repository and no more: the folder is rettui's
/// choice, in its data folder. It runs on after the browser leaves, so the
/// copy is noted either way.
async fn repo_mirror(State(state): State<WebState>, axum::Json(body): axum::Json<UrlBody>) -> ApiResult {
    let repo = repo_url(&body.url)?;
    let task = {
        let state = state.clone();
        let repo = repo.clone();
        tokio::spawn(async move {
            let asked = repo.clone();
            let request = state
                .write(move |o| {
                    o.app.add_repo(&asked.to_string())?;
                    o.app.mirror_request(&asked)
                })
                .await??;
            let reply = state.git(repo, request).await?;
            state
                .write(move |o| {
                    if let crate::net::git::GitReply::Mirrored(copy) = &reply {
                        o.app.record_mirror(copy);
                    }
                    views::git_reply(&o.app, &reply)
                })
                .await
        })
    };
    match task.await {
        Ok(Ok(reply)) => Ok(axum::Json(reply)),
        Ok(Err(e)) => Err(ApiError(StatusCode::BAD_GATEWAY, e)),
        Err(_) => Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The copy stopped unexpectedly".into())),
    }
}

#[derive(Deserialize)]
struct PushBody {
    url: String,
    to: String,
    /// `add`, `overwrite` or `same`.
    #[serde(default)]
    how: String,
}

/// Send a repository's copy on this computer to another repository. The
/// browser says which two and how, and no more: the copy's folder is
/// rettui's choice.
async fn repo_push(State(state): State<WebState>, axum::Json(body): axum::Json<PushBody>) -> ApiResult {
    let (from, to) = (repo_url(&body.url)?, repo_url(&body.to)?);
    let how = crate::app::repos::PushHow::parse(&body.how).ok_or_else(|| bad("How is add, overwrite or same"))?;
    let request = {
        let (from, to) = (from.clone(), to.clone());
        state.write(move |o| o.app.push_copy_request(&from, &to, how)).await?.map_err(bad)?
    };
    let reply = state.git(to, request).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    Ok(axum::Json(state.read(move |o| views::git_reply(&o.app, &reply)).await?))
}

/// Delete a repository's copy from this computer.
async fn repo_delete_mirror(State(state): State<WebState>, axum::Json(body): axum::Json<UrlBody>) -> ApiResult {
    let url = repo_url(&body.url)?.to_string();
    state.write(move |o| o.app.remove_mirror(&url)).await?.map_err(bad)?;
    ok()
}

fn repo_url(text: &str) -> Result<rngit::RepoUrl, ApiError> {
    crate::app::repos::parse_repo(text).map_err(bad)
}

fn work_scope(text: &str) -> Result<rngit::client::WorkScope, ApiError> {
    crate::app::repos::parse_scope(text).ok_or_else(|| bad("A work list is active, completed, proposed or all"))
}

/// Ask a repository, and answer with what it said (as the Repos section
/// shows it). A node that refuses, or can't be reached, is a 502.
async fn ask_repo(state: &WebState, url: &str, request: crate::net::git::GitRequest) -> ApiResult {
    let repo = repo_url(url)?;
    let reply = state.git(repo, request).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    Ok(axum::Json(state.read(move |o| views::git_reply(&o.app, &reply)).await?))
}

/// The branches and tags. A node that can't be asked leaves the copy on this
/// computer, if there is one, to answer.
async fn repo_refs(State(state): State<WebState>, Query(query): Query<UrlQuery>) -> ApiResult {
    let repo = repo_url(&query.url)?;
    match state.git(repo.clone(), crate::net::git::GitRequest::Refs).await {
        Ok(reply) => Ok(axum::Json(state.read(move |o| views::git_reply(&o.app, &reply)).await?)),
        Err(e) => {
            let from_copy = state.read(move |o| o.app.copy_refs(&repo)).await?;
            match from_copy {
                Some(refs) => {
                    let reply = crate::net::git::GitReply::Refs(refs);
                    let mut shown = state.read(move |o| views::git_reply(&o.app, &reply)).await?;
                    shown["from_copy"] = true.into();
                    Ok(axum::Json(shown))
                }
                None => Err(ApiError(StatusCode::BAD_GATEWAY, e)),
            }
        }
    }
}

#[derive(Deserialize)]
struct CopyQuery {
    url: String,
    /// The branch or tag, by its full name.
    r#ref: String,
    #[serde(default)]
    path: String,
    /// `folder` (the default), `file` or `log`.
    #[serde(default)]
    show: String,
}

/// Read the copy of a repository on this computer: a folder, a file or the log
/// of a branch or tag. The browser names the repository, the ref and a path
/// inside it; the folder of the copy is rettui's.
async fn repo_copy_files(State(state): State<WebState>, Query(query): Query<CopyQuery>) -> ApiResult {
    let repo = repo_url(&query.url)?;
    let dir = state.read(move |o| o.app.copy_dir(&repo)).await?.map_err(bad)?;
    let (refname, path) = (query.r#ref, query.path);
    let (file, log) = match query.show.as_str() {
        "" | "folder" => (false, false),
        "file" => (true, false),
        "log" => (false, true),
        _ => return Err(bad("Show a folder, a file or the log")),
    };
    let view = tokio::task::spawn_blocking(move || crate::app::repo_files::read(&dir, &refname, &path, file, log))
        .await
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Reading the copy stopped unexpectedly".into()))?;
    match view {
        crate::app::repo_files::BrowseView::Failed(e) => Err(ApiError(StatusCode::BAD_REQUEST, e)),
        view => Ok(axum::Json(views::browse_json(&view))),
    }
}

async fn repo_perms(State(state): State<WebState>, Query(query): Query<UrlQuery>) -> ApiResult {
    ask_repo(&state, &query.url, crate::net::git::GitRequest::Perms).await
}

async fn repo_releases(State(state): State<WebState>, Query(query): Query<UrlQuery>) -> ApiResult {
    ask_repo(&state, &query.url, crate::net::git::GitRequest::Releases).await
}

async fn repo_work(State(state): State<WebState>, Query(query): Query<UrlQuery>) -> ApiResult {
    ask_repo(&state, &query.url, crate::net::git::GitRequest::Work(rngit::client::WorkScope::All)).await
}

#[derive(Deserialize)]
struct ReleaseQuery {
    url: String,
    tag: String,
}

async fn repo_release(State(state): State<WebState>, Query(query): Query<ReleaseQuery>) -> ApiResult {
    ask_repo(&state, &query.url, crate::net::git::GitRequest::Release(query.tag)).await
}

#[derive(Deserialize)]
struct ArtifactQuery {
    url: String,
    tag: String,
    name: String,
}

/// A release's file, checked against the release's signature, to the
/// browser.
async fn repo_artifact(State(state): State<WebState>, Query(query): Query<ArtifactQuery>) -> Result<Response, ApiError> {
    use crate::net::git::{GitReply, GitRequest};
    let repo = repo_url(&query.url)?;
    let reply = state
        .git(repo, GitRequest::Artifact { tag: query.tag, name: query.name })
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    match reply {
        GitReply::Artifact { name, data, .. } => Ok(file_response(data, &name, false)),
        _ => Err(ApiError(StatusCode::BAD_GATEWAY, "The node answered something else".into())),
    }
}

#[derive(Deserialize)]
struct DocQuery {
    url: String,
    id: u64,
    scope: String,
}

async fn repo_doc(State(state): State<WebState>, Query(query): Query<DocQuery>) -> ApiResult {
    let scope = work_scope(&query.scope)?;
    ask_repo(&state, &query.url, crate::net::git::GitRequest::WorkView { id: query.id, scope }).await
}

#[derive(Deserialize)]
struct NewDocBody {
    url: String,
    title: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    propose: bool,
}

async fn repo_new_doc(State(state): State<WebState>, axum::Json(body): axum::Json<NewDocBody>) -> ApiResult {
    let request = crate::app::repos::new_doc_request(body.propose, &body.title, &body.text)?;
    ask_repo(&state, &body.url, request).await
}

#[derive(Deserialize)]
struct DocBody {
    url: String,
    id: u64,
    scope: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    done: bool,
}

async fn repo_comment(State(state): State<WebState>, axum::Json(body): axum::Json<DocBody>) -> ApiResult {
    let request = crate::app::repos::comment_request(body.id, work_scope(&body.scope)?, &body.text)?;
    ask_repo(&state, &body.url, request).await
}

async fn repo_edit_doc(State(state): State<WebState>, axum::Json(body): axum::Json<DocBody>) -> ApiResult {
    let request = crate::app::repos::edit_request(body.id, work_scope(&body.scope)?, &body.title, &body.text)?;
    ask_repo(&state, &body.url, request).await
}

/// Mark a document done, or active again.
async fn repo_doc_state(State(state): State<WebState>, axum::Json(body): axum::Json<DocBody>) -> ApiResult {
    use crate::net::git::GitRequest;
    let request = if body.done { GitRequest::WorkComplete(body.id) } else { GitRequest::WorkActivate(body.id) };
    ask_repo(&state, &body.url, request).await
}

async fn repo_delete_doc(State(state): State<WebState>, axum::Json(body): axum::Json<DocBody>) -> ApiResult {
    let scope = work_scope(&body.scope)?;
    ask_repo(&state, &body.url, crate::net::git::GitRequest::WorkDelete { id: body.id, scope }).await
}

#[derive(Deserialize)]
struct ReleaseFile {
    name: String,
    /// Base64.
    data: String,
}

#[derive(Deserialize)]
struct NewReleaseBody {
    url: String,
    tag: String,
    #[serde(default)]
    notes: String,
    files: Vec<ReleaseFile>,
}

/// Publish a release with files the browser picked (never files on this
/// computer).
async fn repo_new_release(State(state): State<WebState>, axum::Json(body): axum::Json<NewReleaseBody>) -> ApiResult {
    let mut files = Vec::new();
    for file in body.files {
        let data = base64::engine::general_purpose::STANDARD
            .decode(file.data.as_bytes())
            .map_err(|_| bad(format!("{} isn't base64", file.name)))?;
        files.push((file.name, data));
    }
    let request = crate::app::repos::release_request(&body.tag, &body.notes, files)?;
    ask_repo(&state, &body.url, request).await
}

async fn repo_delete_release(State(state): State<WebState>, axum::Json(body): axum::Json<ReleaseQuery>) -> ApiResult {
    ask_repo(&state, &body.url, crate::net::git::GitRequest::ReleaseDelete(body.tag)).await
}

// ---- the RRC hub hosted here ----------------------------------------------

async fn hosted_hub(State(state): State<WebState>) -> ApiResult {
    Ok(axum::Json(state.read(|o| views::hub(&o.app)).await?))
}

#[derive(Deserialize, Default)]
struct HostedHubBody {
    /// The room an action is about (none: the whole hub, for `ban`).
    #[serde(default)]
    room: Option<String>,
    /// Whom it's about (hex).
    #[serde(default)]
    identity: String,
    /// A command, a topic, modes.
    #[serde(default)]
    text: String,
    /// On or off: hosting, a ban, operator, voice, registered.
    #[serde(default)]
    on: Option<bool>,
}

/// What the Hub section does: hub commands, as its panel in the terminal
/// runs them. The hub says how each went (a reply, in `GET /hub`).
async fn hosted_hub_action(
    State(state): State<WebState>,
    Path(action): Path<String>,
    body: Option<axum::Json<HostedHubBody>>,
) -> ApiResult {
    let body = body.map(|b| b.0).unwrap_or_default();
    state
        .write(move |o| -> Result<(), ApiError> {
            let app = &mut o.app;
            let on = body.on.unwrap_or(true);
            let room = body.room.as_deref().unwrap_or_default();
            let identity = body.identity.trim();
            let result = match action.as_str() {
                "enable" => app.update_settings_from_web(&[("hub_enabled", if on { "true" } else { "false" })]).map(|_| ()),
                "announce" => app.hub_announce(),
                "run" => app.hub_run(&body.text),
                "kick" => app.hub_kick(room, identity),
                "ban" => app.hub_ban(body.room.as_deref(), identity, on),
                "op" => app.hub_op(room, identity, on),
                "voice" => app.hub_voice(room, identity, on),
                "invite" => app.hub_invite(room, identity, on),
                // The key in `text`; off takes it off.
                "key" => app.hub_room_key(room, on.then_some(body.text.as_str())),
                "topic" => app.hub_topic(room, &body.text),
                "modes" => app.hub_modes(room, &body.text),
                "register" => app.hub_register(room, on),
                "delete" => app.hub_delete_room(room),
                // The new name in `text`.
                "rename" => app.hub_rename_room(room, &body.text),
                "disconnect" => app.hub_disconnect(identity),
                _ => return Err(bad(format!("unknown hub action {action}"))),
            };
            result.map_err(bad)
        })
        .await??;
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

/// Stop rettui, as the terminal UI's `q` does: everything is saved, and the
/// web server ends right after this answer.
async fn quit(State(state): State<WebState>) -> ApiResult {
    state.write(|o| o.app.request_quit()).await?;
    ok()
}

/// Check text without saving it (the text editor's live status).
/// What an RNode's radio settings (as being typed) come to.
async fn reticulum_lora(Query(values): Query<BTreeMap<String, String>>) -> ApiResult {
    let radio = crate::reticulum::lora::Radio::from_values(|key| values.get(key).map(String::as_str));
    Ok(axum::Json(json!({ "lines": radio.map(|r| r.estimate().lines()) })))
}

async fn reticulum_check(axum::Json(body): axum::Json<RnsTextBody>) -> ApiResult {
    let (_, check) = crate::reticulum::check(&body.text);
    Ok(axum::Json(json!({ "error": check.error, "warnings": check.warnings })))
}

async fn reticulum_text(State(state): State<WebState>, axum::Json(body): axum::Json<RnsTextBody>) -> ApiResult {
    let warnings = state.write(move |o| o.app.rns_save_text(&body.text, true)).await??;
    Ok(axum::Json(json!({ "warnings": warnings })))
}

/// Earlier versions of the config file, newest first.
async fn reticulum_history(State(state): State<WebState>) -> ApiResult {
    let versions = state.read(|o| o.app.rns_versions()).await?;
    let listed: Vec<Value> =
        versions.iter().map(|v| json!({ "id": v.id, "time": v.time, "when": crate::clock::when(v.time), "what": v.what })).collect();
    Ok(axum::Json(json!({ "versions": listed })))
}

#[derive(Deserialize)]
struct VersionQuery {
    /// `current`: compared with the file now (what putting it back would
    /// change), else with the version before it (what it changed).
    #[serde(default)]
    against: Option<String>,
}

/// How a version differs, line by line.
async fn reticulum_version(State(state): State<WebState>, Path(id): Path<String>, Query(query): Query<VersionQuery>) -> ApiResult {
    let current = query.against.as_deref() == Some("current");
    let lines = state.read(move |o| o.app.rns_compare(&id, current)).await?.map_err(bad)?;
    let (added, removed) = crate::reticulum::history::counts(&lines);
    Ok(axum::Json(json!({ "lines": lines, "added": added, "removed": removed })))
}

/// The interface profiles, and the one the config file is as now.
async fn reticulum_profiles(State(state): State<WebState>) -> ApiResult {
    let (profiles, now) = state.read(|o| (o.app.rns_profiles().list(), o.app.rns_profile_now())).await?;
    Ok(axum::Json(json!({ "profiles": profiles, "now": now })))
}

#[derive(Deserialize)]
struct ProfileBody {
    name: String,
}

/// Keep the interfaces, on and off as they are now, as a profile.
async fn reticulum_save_profile(State(state): State<WebState>, axum::Json(body): axum::Json<ProfileBody>) -> ApiResult {
    let profile = state.write(move |o| o.app.rns_save_profile(&body.name)).await?.map_err(bad)?;
    Ok(axum::Json(json!({ "profile": profile })))
}

async fn reticulum_delete_profile(State(state): State<WebState>, Path(name): Path<String>) -> ApiResult {
    state.write(move |o| o.app.rns_profiles().delete(&name)).await?.map_err(bad)?;
    ok()
}

/// Switch to a profile and restart Reticulum (as an edit from here: a pipe
/// interface it would turn on is refused).
async fn reticulum_switch_profile(State(state): State<WebState>, Path(name): Path<String>) -> ApiResult {
    let (changed, warnings) = state.write(move |o| o.app.rns_switch_profile(&name, true)).await??;
    Ok(axum::Json(json!({ "changed": changed, "warnings": warnings })))
}

/// Put an earlier version back (as an edit from here: no new pipe
/// commands).
async fn reticulum_restore(State(state): State<WebState>, Path(id): Path<String>) -> ApiResult {
    let warnings = state.write(move |o| o.app.rns_restore(&id, true)).await??;
    Ok(axum::Json(json!({ "warnings": warnings })))
}

/// An identity's id in the API: `-` for the first (whose own is empty).
fn identity_id(id: &str) -> String {
    if id == "-" { String::new() } else { id.to_string() }
}

/// The identities kept here (see `crate::identities`). Their keys stay in
/// the terminal: the web UI can't bring one in, copy one or delete one.
async fn identities_list(State(state): State<WebState>) -> ApiResult {
    let list = state.read(|o| o.app.identities()).await?;
    let listed: Vec<Value> = list
        .iter()
        .map(|p| {
            json!({
                "id": if p.id.is_empty() { "-" } else { p.id.as_str() },
                "name": p.name,
                "address": hex::encode(p.lxmf),
                "link": p.link,
                "active": p.active,
                "first": p.id.is_empty(),
            })
        })
        .collect();
    Ok(axum::Json(json!({ "identities": listed, "most": crate::identities::MOST })))
}

#[derive(Deserialize)]
struct IdentityName {
    name: String,
}

/// Keep a new identity.
async fn identities_add(State(state): State<WebState>, axum::Json(body): axum::Json<IdentityName>) -> ApiResult {
    let profile = state.write(move |o| o.app.add_identity(&body.name, None)).await??;
    Ok(axum::Json(json!({ "id": profile.id, "name": profile.name, "address": hex::encode(profile.lxmf) })))
}

/// Switch: rettui saves everything and starts over as the other identity;
/// browsers load again once it's back.
async fn identities_switch(State(state): State<WebState>, Path(id): Path<String>) -> ApiResult {
    let id = identity_id(&id);
    let profile = state.write(move |o| o.app.switch_identity(&id)).await??;
    Ok(axum::Json(json!({ "name": profile.name, "address": hex::encode(profile.lxmf) })))
}

async fn identities_rename(State(state): State<WebState>, Path(id): Path<String>, axum::Json(body): axum::Json<IdentityName>) -> ApiResult {
    let id = identity_id(&id);
    state.write(move |o| o.app.rename_identity(&id, &body.name)).await??;
    Ok(axum::Json(json!({ "ok": true })))
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
        let headers =
            head.lines().skip(1).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect();
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
