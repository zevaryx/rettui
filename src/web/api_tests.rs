//! The web UI's HTTP API, driven as a browser would (see
//! [`super::testing`]): each route on a test app.

use axum::http::{Method, StatusCode, header};
use p256::elliptic_curve::Generate;
use serde_json::json;

use super::testing::{TOKEN, TestWeb};
use crate::lxmf::{Extras, InboundMessage};
use crate::net::{NetCommand, NetEvent, PeerKind};

/// Someone to talk to.
const BOB: &str = "abababababababababababababababab";

fn from_bob(n: u8, content: &str) -> InboundMessage {
    InboundMessage {
        id: Some([n; 32]),
        source: [0xab; 16],
        content: content.into(),
        timestamp: 1_800_000_000.0 + f64::from(n),
        verified: true,
        extras: Extras::default(),
        ..Default::default()
    }
}

#[tokio::test]
async fn logging_in_and_the_files_every_page_needs() {
    let web = TestWeb::start("login");
    // The link logs in: a cookie, and on to the page.
    let login = web.send(Method::GET, &format!("/?token={TOKEN}"), None, true).await;
    assert_eq!(login.status, StatusCode::SEE_OTHER);
    let cookie = login.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("rettui_token=") && cookie.contains("HttpOnly"), "{cookie}");
    // A wrong one, or none: the login page; the API says so.
    assert_eq!(web.send(Method::GET, "/?token=nope", None, true).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(web.send(Method::GET, "/", None, true).await.status, StatusCode::UNAUTHORIZED);
    let api = web.send(Method::GET, "/api/state", None, true).await;
    assert_eq!(api.status, StatusCode::UNAUTHORIZED);
    assert!(api.headers.contains_key(header::WWW_AUTHENTICATE));
    // The cookie works as well as the header.
    let cookie_header = format!("rettui_token={TOKEN}");
    let with_cookie = web.send_with(Method::GET, "/api/state", None, true, &[("cookie", &cookie_header)]).await;
    assert_eq!(with_cookie.status, StatusCode::OK);
    // What anyone may load, and the headers every answer has.
    for public in ["/brand/icon.png", "/manifest.webmanifest", "/sw.js"] {
        let answer = web.send(Method::GET, public, None, true).await;
        assert_eq!(answer.status, StatusCode::OK, "{public}");
        assert_eq!(answer.headers[header::X_FRAME_OPTIONS], "DENY");
    }
    assert_eq!(web.send(Method::GET, "/rettui-ca.crt", None, true).await.status, StatusCode::NOT_FOUND);

    // The page, script and style: gzipped when asked, "not modified" by
    // their ETag.
    for (path, kind) in [("/", "text/html"), ("/app.js", "text/javascript"), ("/style.css", "text/css")] {
        let plain = web.get(path).await;
        assert_eq!(plain.status, StatusCode::OK);
        assert!(plain.headers[header::CONTENT_TYPE].to_str().unwrap().starts_with(kind));
        let etag = plain.headers[header::ETAG].to_str().unwrap().to_string();
        let zipped = web.send_with(Method::GET, path, None, false, &[("accept-encoding", "gzip")]).await;
        assert_eq!(zipped.headers[header::CONTENT_ENCODING], "gzip");
        let again = web.send_with(Method::GET, path, None, false, &[("if-none-match", &etag)]).await;
        assert_eq!(again.status, StatusCode::NOT_MODIFIED);
    }
    assert!(web.get("/").await.text().contains(env!("CARGO_PKG_VERSION")));
    // Fonts, the logo and faces; anything else isn't there.
    assert_eq!(web.get("/fonts/FiraCodeNerdFont-Regular-Text.woff2").await.status, StatusCode::OK);
    assert_eq!(web.get("/fonts/nope.woff2").await.status, StatusCode::NOT_FOUND);
    assert_eq!(web.get("/brand/nope.png").await.status, StatusCode::NOT_FOUND);
    let face = web.get(&format!("/face/{BOB}.svg")).await;
    assert_eq!(face.headers[header::CONTENT_TYPE], "image/svg+xml");
    assert!(face.text().starts_with("<svg"));
    assert_eq!(web.get("/face/nothex.svg").await.status, StatusCode::NOT_FOUND);
    // JSON big enough is gzipped too.
    let emoji = web.send_with(Method::GET, "/api/emoji", None, false, &[("accept-encoding", "gzip")]).await;
    assert_eq!(emoji.headers[header::CONTENT_ENCODING], "gzip");
    let emoji = web.get("/api/emoji").await.json();
    assert!(emoji["groups"].as_array().unwrap().len() > 3);
}

#[tokio::test]
async fn state_settings_and_the_little_things() {
    let web = TestWeb::start("state");
    let state = web.get("/api/state").await.json();
    assert_eq!(state["net"]["state"], "starting");
    assert_eq!(state["own_certificate"], false);
    assert!(state["unread"]["messages"].is_number());
    // The log only when asked for (Status shows it).
    assert!(state.get("log").is_none());
    assert!(web.get("/api/state?log=1").await.json()["log"].is_array());

    // Settings: read, changed (all or none), and said back.
    let settings = web.get("/api/settings").await.json();
    assert!(settings.to_string().contains("display_name"));
    let saved = web.post("/api/settings", json!({ "values": { "display_name": "Amy" } })).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
    assert_eq!(web.get("/api/state").await.json()["display_name"], "Amy");
    let refused = web.post("/api/settings", json!({ "values": { "display_name": "Ben", "sync_interval_mins": "soon" } })).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.get("/api/state").await.json()["display_name"], "Amy");

    // Icons, emoji picked lately, a QR code.
    let icons = web.get("/api/icons?q=radio").await.json();
    assert!(icons.as_array().unwrap().iter().any(|i| i["name"].as_str().unwrap().contains("radio")));
    let picked = web.post("/api/emoji/recent", json!({ "emoji": "👍" })).await.json();
    assert_eq!(picked[0], "👍");
    assert_eq!(web.get("/api/emoji/recent").await.json()[0], "👍");
    assert_eq!(web.post("/api/emoji/recent", json!({ "emoji": "nope" })).await.status, StatusCode::BAD_REQUEST);
    let qr = web.get("/api/qr?text=hello").await;
    assert_eq!(qr.headers[header::CONTENT_TYPE], "image/svg+xml");

    // Announcing and syncing go to the network.
    web.post("/api/announce", json!({})).await;
    web.post("/api/sync", json!({})).await;
    web.post("/api/node/announce", json!({})).await;
    let sent = web.sent();
    assert!(sent.iter().any(|c| matches!(c, NetCommand::Announce)));
    assert!(sent.iter().any(|c| matches!(c, NetCommand::Sync)));
    assert!(sent.iter().any(|c| matches!(c, NetCommand::HostAnnounce)));
    // A propagation node by address.
    let node = "cd".repeat(16);
    assert_eq!(web.post("/api/propagation", json!({ "node": node })).await.status, StatusCode::OK);
    assert_eq!(web.get("/api/state").await.json()["propagation_node"]["hash"], node);
    assert_eq!(web.post("/api/propagation", json!({ "node": "nope" })).await.status, StatusCode::BAD_REQUEST);

    // Getting started, and the first steps.
    let guide = web.get("/api/guide").await.json();
    assert!(guide.is_object());
    assert!(web.get("/api/guide/reach").await.json().is_object() || web.get("/api/guide/reach").await.json().is_array());
    assert_eq!(web.post("/api/guide/dismiss", json!({})).await.status, StatusCode::OK);
    assert_eq!(web.post("/api/first-steps/hide", json!({})).await.status, StatusCode::OK);
    assert_eq!(web.post("/api/identity/backed-up", json!({})).await.status, StatusCode::OK);
    assert!(web.with(|app| app.settings.identity_backed_up).await);

    // Updates: checks are off in tests, so there's nothing to install.
    assert_eq!(web.post("/api/update/check", json!({})).await.status, StatusCode::OK);
    assert_eq!(web.post("/api/update/install", json!({})).await.status, StatusCode::BAD_REQUEST);
    // The cache, emptied.
    assert_eq!(web.post("/api/cache/clear", json!({})).await.json()["removed"], 0);
    // Web Push: the key, and subscriptions coming and going.
    assert!(web.get("/api/push").await.json()["key"].as_str().unwrap().len() > 40);
    let endpoint = "https://push.example/abc";
    let subscribed = web
        .post(
            "/api/push/subscribe",
            json!({ "endpoint": endpoint, "keys": {
                "p256dh": "BOrJ1zR4n7v6E5Wh6p4mT1g2cR4Y0t3j0p7cQ6bH0wS2kH9vF1sN7uA3xL5yE8dG2qZ4mJ6tP0rV9nC1bW3fK5I",
                "auth": "dGVzdGF1dGhzZWNyZXQ" } }),
        )
        .await;
    assert!(subscribed.status == StatusCode::OK || subscribed.status == StatusCode::BAD_REQUEST, "{}", subscribed.text());
    assert_eq!(web.post("/api/push/showing", json!({ "endpoint": endpoint, "showing": true })).await.status, StatusCode::OK);
    assert_eq!(web.post("/api/push/unsubscribe", json!({ "endpoint": endpoint })).await.status, StatusCode::OK);
}

#[tokio::test]
async fn conversations_from_first_message_to_deleting_them() {
    let web = TestWeb::start("conversations");
    // A new one by address; nonsense isn't one.
    let added = web.post("/api/conversations", json!({ "address": BOB })).await.json();
    assert_eq!(added["key"], BOB);
    assert_eq!(web.post("/api/conversations", json!({ "address": "nope" })).await.status, StatusCode::BAD_REQUEST);
    // They write; it's listed, unread, and read when shown.
    web.with(|app| {
        app.set_focus(false);
        app.on_message(from_bob(1, "Hello from Bob"));
        app.on_message(from_bob(2, "Lunch at noon?"));
    })
    .await;
    let list = web.get("/api/conversations").await.json();
    let bob = list.as_array().unwrap().iter().find(|c| c["key"] == BOB).unwrap().clone();
    assert_eq!(bob["unread"], 2);
    let shown = web.get(&format!("/api/conversations/{BOB}?last=1")).await.json();
    assert_eq!(shown["messages"].as_array().unwrap().len(), 1);
    let all = web.get(&format!("/api/conversations/lxmf@{BOB}")).await.json();
    assert_eq!(all["messages"].as_array().unwrap().len(), 2);
    assert_eq!(web.get("/api/conversations/nope").await.status, StatusCode::BAD_REQUEST);
    web.post(&format!("/api/conversations/{BOB}/read"), json!({})).await;
    assert_eq!(web.with(|app| app.store.conversations[BOB].unread).await, 0);
    web.with(|app| app.on_message(from_bob(3, "Still there?"))).await;
    assert_eq!(web.post("/api/conversations/read-all", json!({})).await.json()["marked"], 1);

    // Searching what was said: everywhere, or in this conversation.
    let found = web.get("/api/search?q=lunch").await.json();
    assert!(found.to_string().contains("Lunch at noon"), "{found}");
    let within = web.get(&format!("/api/search?q=hello&in={BOB}")).await.json();
    assert!(within.to_string().contains("Hello from Bob"));

    // Writing: plain, a reply, with a file, a location; not by a mode
    // that isn't one.
    let sent = web.post(&format!("/api/conversations/{BOB}/send"), json!({ "content": "On my way" })).await.json();
    let mine = sent["id"].as_str().unwrap().to_string();
    assert!(mine.starts_with("local-"));
    let theirs = hex::encode([2u8; 32]);
    let reply = web.post(&format!("/api/conversations/{BOB}/send"), json!({ "content": "Yes!", "reply_to": theirs })).await;
    assert_eq!(reply.status, StatusCode::OK);
    let file = web
        .post(
            &format!("/api/conversations/{BOB}/send"),
            json!({ "content": "notes", "files": [{ "name": "notes.txt", "data": "aGVsbG8=" }] }),
        )
        .await;
    assert_eq!(file.status, StatusCode::OK, "{}", file.text());
    let bad_file = web.post(&format!("/api/conversations/{BOB}/send"), json!({ "files": [{ "name": "x.bin", "data": "!!!" }] })).await;
    assert_eq!(bad_file.status, StatusCode::BAD_REQUEST);
    let at = web
        .post(
            &format!("/api/conversations/{BOB}/send"),
            json!({ "content": "here", "location": { "latitude": 51.5, "longitude": -0.12, "accuracy": 8.0 } }),
        )
        .await;
    assert_eq!(at.status, StatusCode::OK, "{}", at.text());
    let nowhere = web.post(&format!("/api/conversations/{BOB}/send"), json!({ "location": { "latitude": 123.0, "longitude": 0.0 } })).await;
    assert_eq!(nowhere.status, StatusCode::BAD_REQUEST);
    let both = web
        .post(
            &format!("/api/conversations/{BOB}/send"),
            json!({ "location": { "latitude": 1.0, "longitude": 1.0 }, "files": [{ "name": "a", "data": "" }] }),
        )
        .await;
    assert_eq!(both.status, StatusCode::BAD_REQUEST);
    let no_mode = web.post(&format!("/api/conversations/{BOB}/send"), json!({ "content": "x", "mode": "pigeon" })).await;
    assert_eq!(no_mode.status, StatusCode::BAD_REQUEST);
    let bad_voice = web.post(&format!("/api/conversations/{BOB}/send"), json!({ "voice": "!!" })).await;
    assert_eq!(bad_voice.status, StatusCode::BAD_REQUEST);
    let voice_and_file =
        web.post(&format!("/api/conversations/{BOB}/send"), json!({ "voice": "", "files": [{ "name": "a", "data": "" }] })).await;
    assert_eq!(voice_and_file.status, StatusCode::BAD_REQUEST);
    assert!(web.sent().iter().filter(|c| matches!(c, NetCommand::SendMessage { .. })).count() >= 4);

    // The file sent, back again.
    let index = web.with(|app| app.store.conversations[BOB].messages.iter().find(|m| !m.attachments.is_empty()).unwrap().id.clone()).await;
    let attachment = web.get(&format!("/api/conversations/{BOB}/attachments/{index}/0")).await;
    assert_eq!(attachment.body, b"hello");
    assert!(attachment.headers[header::CONTENT_DISPOSITION].to_str().unwrap().contains("notes.txt"));
    assert_eq!(web.get(&format!("/api/conversations/{BOB}/attachments/{index}/9")).await.status, StatusCode::NOT_FOUND);

    // Reacting, retrying (only what failed), delivery and trust.
    let react = web.post(&format!("/api/conversations/{BOB}/react"), json!({ "id": theirs, "emoji": "👍" })).await;
    assert_eq!(react.status, StatusCode::OK, "{}", react.text());
    let retry = web.post(&format!("/api/conversations/{BOB}/retry"), json!({ "id": mine })).await;
    assert_eq!(retry.status, StatusCode::BAD_REQUEST);
    let failed = mine.clone();
    web.with(move |app| {
        app.store.conversations.get_mut(BOB).unwrap().find_mut(&failed).unwrap().state =
            crate::store::MessageState::Failed("no path".into());
    })
    .await;
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/retry"), json!({ "id": mine })).await.status, StatusCode::OK);
    assert_eq!(
        web.post(&format!("/api/conversations/{BOB}/retry"), json!({ "id": mine, "mode": "x" })).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/delivery"), json!({ "mode": "propagated" })).await.status, StatusCode::OK);
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/delivery"), json!({ "mode": "direct" })).await.status, StatusCode::OK);
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/delivery"), json!({ "mode": "?" })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/trust"), json!({ "trust": "trusted" })).await.status, StatusCode::OK);
    assert_eq!(
        web.post(&format!("/api/conversations/{BOB}/trust"), json!({ "trust": "best friend" })).await.status,
        StatusCode::BAD_REQUEST
    );
    // Their name and notes, pinning, muting.
    assert_eq!(
        web.post(&format!("/api/conversations/{BOB}/contact"), json!({ "alias": "Bobby", "notes": "met at the fair" })).await.status,
        StatusCode::OK
    );
    assert_eq!(web.with(|app| app.store.display_name(BOB)).await, "Bobby");
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/pin"), json!({ "pinned": true })).await.status, StatusCode::OK);
    assert!(web.with(|app| app.store.conversations[BOB].pinned).await);
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/notify"), json!({ "muted": true })).await.status, StatusCode::OK);
    let nobody = "ef".repeat(16);
    assert_eq!(web.post(&format!("/api/conversations/{nobody}/notify"), json!({ "muted": true })).await.status, StatusCode::NOT_FOUND);

    // Forwarded to someone else, exported, the archive read.
    let carol = "cd".repeat(16);
    web.post("/api/conversations", json!({ "address": carol })).await;
    let forwarded = web.post(&format!("/api/conversations/{BOB}/forward"), json!({ "id": theirs, "to": carol })).await;
    assert_eq!(forwarded.status, StatusCode::OK, "{}", forwarded.text());
    let export = web.get(&format!("/api/conversations/{BOB}/export")).await;
    assert!(export.text().contains("Lunch at noon"));
    assert!(export.headers[header::CONTENT_DISPOSITION].to_str().unwrap().contains(".txt"));
    assert!(web.get(&format!("/api/conversations/{BOB}/archive")).await.json()["messages"].is_array());

    // Live location: by the device, or the station (which has none set).
    let live = web.post(&format!("/api/conversations/{BOB}/live"), json!({ "minutes": 30, "source": "device" })).await;
    assert_eq!(live.status, StatusCode::OK, "{}", live.text());
    assert_eq!(web.post("/api/live/position", json!({ "latitude": 51.5, "longitude": -0.1 })).await.status, StatusCode::OK);
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/live/stop"), json!({})).await.status, StatusCode::OK);
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/live"), json!({ "source": "satellite" })).await.status, StatusCode::BAD_REQUEST);
    assert!(web.get("/api/locations").await.json().is_array() || web.get("/api/locations").await.json().is_object());

    // A message deleted, then the conversation.
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/messages/delete"), json!({ "id": theirs })).await.status, StatusCode::OK);
    assert_eq!(
        web.post(&format!("/api/conversations/{BOB}/messages/delete"), json!({ "id": "gone" })).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/delete"), json!({})).await.status, StatusCode::OK);
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/delete"), json!({})).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn games_over_the_api() {
    let web = TestWeb::start("games");
    web.post("/api/conversations", json!({ "address": BOB })).await;
    web.with(|app| app.games.set_me(&[0xa1; 16])).await;
    let started = web.post("/api/games", json!({ "peer": BOB, "app": "ttt" })).await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.text());
    let session = started.json()["session"].as_str().unwrap().to_string();
    let list = web.get("/api/games").await.json();
    assert_eq!(list[0]["session"], session.as_str());
    assert_eq!(web.get(&format!("/api/games?peer={BOB}")).await.json().as_array().unwrap().len(), 1);
    let game = web.get(&format!("/api/games/ttt/{session}")).await.json();
    assert_eq!(game["status"], "pending");
    assert!(game["notes"].is_array());
    assert!(game["rules"].as_str().unwrap().contains("Three of yours in a row"), "{game}");
    // Not their move to make; nor a game that isn't.
    assert_eq!(
        web.post(&format!("/api/games/ttt/{session}"), json!({ "command": "move", "picked": "4" })).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(web.get("/api/games/ttt/nope").await.status, StatusCode::NOT_FOUND);
    assert_eq!(web.post("/api/games", json!({ "peer": BOB, "app": "go" })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.delete(&format!("/api/games/ttt/{session}")).await.status, StatusCode::OK);
    assert_eq!(web.delete(&format!("/api/games/ttt/{session}")).await.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_checkers_board_over_the_api() {
    use crate::games::{Games, fields};
    let web = TestWeb::start("checkers");
    web.post("/api/conversations", json!({ "address": BOB })).await;
    web.with(|app| app.games.set_me(&[0xa1; 16])).await;
    let session = web.post("/api/games", json!({ "peer": BOB, "app": "checkers" })).await.json()["session"].as_str().unwrap().to_string();
    // Bob accepts, as his rettui would.
    let mut bob = Games::default();
    bob.set_me(&hex::decode(BOB).unwrap().try_into().unwrap());
    let wire = |prepared: &crate::games::Prepared| -> std::collections::HashMap<u8, Vec<u8>> {
        fields(&hex::decode(prepared.game.envelope.as_ref().unwrap()).unwrap()).into_iter().collect()
    };
    let challenge = web.with(move |app| app.games.challenge("checkers", BOB)).await.unwrap();
    // (A second challenge: Bob answers that one, so the first stays pending.)
    let other = challenge.game.session.clone();
    bob.receive(&hex::encode([0xa1; 16]), &wire(&challenge));
    let accept = bob.act("checkers", &other, "accept", Default::default()).unwrap();
    web.with(move |app| app.games.receive(BOB, &wire(&accept))).await;
    let game = web.get(&format!("/api/games/checkers/{other}")).await.json();
    assert_eq!((game["status"].as_str(), game["board"]["kind"].as_str()), (Some("active"), Some("checkers")));
    assert_eq!(game["board"]["cells"].as_array().unwrap().len(), 64);
    let black = game["board"]["black"].as_bool().unwrap();
    assert_eq!(game["board"]["legal"].as_array().unwrap().len(), if black { 7 } else { 0 });
    // No draw to claim yet.
    assert_eq!(web.post(&format!("/api/games/checkers/{other}"), json!({ "command": "draw_claim" })).await.status, StatusCode::BAD_REQUEST);
    if black {
        let moved = web.post(&format!("/api/games/checkers/{other}"), json!({ "command": "move", "picked": "d6 c5" })).await;
        assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
        let game = web.get(&format!("/api/games/checkers/{other}")).await.json();
        assert_eq!(game["board"]["last"], "d6 c5");
    }
    assert_eq!(web.get(&format!("/api/games/checkers/{session}")).await.json()["status"], "pending");
}

#[tokio::test]
async fn identities_over_the_api() {
    let web = TestWeb::start("identities");
    web.with(|app| {
        crate::config::load_identity(&app.paths.identity).unwrap();
    })
    .await;
    let list = || async { web.get("/api/identities").await.json()["identities"].as_array().unwrap().clone() };
    let [first] = list().await.try_into().unwrap();
    assert_eq!((first["id"].as_str(), first["active"].as_bool(), first["first"].as_bool()), (Some("-"), Some(true), Some(true)));
    assert!(first["link"].as_str().unwrap().starts_with("lxma://"));
    let made = web.post("/api/identities", json!({ "name": "Work" })).await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.text());
    assert_eq!(web.post("/api/identities", json!({ "name": " " })).await.status, StatusCode::BAD_REQUEST);
    let work = list().await[1].clone();
    assert_eq!((work["name"].as_str(), work["active"].as_bool()), (Some("Work"), Some(false)));
    let id = work["id"].as_str().unwrap().to_string();
    // Renamed: another, and the one in use (its display name).
    let renamed = web.post(&format!("/api/identities/{id}/rename"), json!({ "name": "Office" })).await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    web.post("/api/identities/-/rename", json!({ "name": "Home" })).await;
    let names: Vec<String> = list().await.iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, ["Home", "Office"]);
    // Switched: the run loop is told, and starts over as it.
    assert_eq!(web.post("/api/identities/-/switch", json!({})).await.status, StatusCode::BAD_REQUEST, "in use already");
    let switched = web.post(&format!("/api/identities/{id}/switch"), json!({})).await;
    assert_eq!(switched.json()["name"], "Office");
    assert_eq!(web.with(|app| app.take_switch()).await.as_deref(), Some("Office"));
    // No deleting, or bringing one in, from here.
    assert_eq!(web.post(&format!("/api/identities/{id}/delete"), json!({})).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn trails_on_the_map_and_deleting_someones_locations() {
    use crate::lxmf::fields::Telemetry;
    let web = TestWeb::start("trails");
    let now = crate::app::now();
    for (n, lat) in [(1u8, 51.0), (2, 51.1)] {
        let telemetry = Telemetry { location: Some(crate::lxmf::Location::at(lat, 0.0)), ..Telemetry::default() };
        let mut message = InboundMessage { extras: Extras { telemetry: Some(telemetry), ..Extras::default() }, ..from_bob(n, "") };
        message.timestamp = now - 100.0 + f64::from(n);
        web.with(move |app| app.on_message(message)).await;
    }
    let places = web.get("/api/locations").await.json();
    let bob = places["places"].as_array().unwrap().iter().find(|p| p["key"] == BOB).cloned().unwrap();
    assert_eq!(bob["trail"].as_array().unwrap().len(), 2, "{bob}");
    assert_eq!(bob["trail"][0][0], 51.0);
    assert_eq!(web.delete("/api/locations/nope").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.delete(&format!("/api/locations/{BOB}")).await.status, StatusCode::OK);
    let places = web.get("/api/locations").await.json();
    assert!(!places["places"].as_array().unwrap().iter().any(|p| p["key"] == BOB), "{places}");
}

#[tokio::test]
async fn reticulums_tables_over_the_api() {
    use crate::net::{AnnounceRate, PathInfo, Tables};
    let web = TestWeb::start("tables");
    let first = web.get("/api/tables").await.json();
    assert_eq!((first["table"].as_str(), first["read"].as_bool()), (Some("paths"), Some(false)), "{first}");
    assert!(web.sent().iter().any(|c| matches!(c, NetCommand::ReadTables)));
    assert_eq!(web.get("/api/tables?table=nope").await.status, StatusCode::BAD_REQUEST);
    let now = crate::app::now();
    let paths = (0..30u8)
        .map(|i| {
            (
                [i; 16],
                PathInfo {
                    hops: i % 5,
                    via: None,
                    interface: if i == 7 { "LoRa".into() } else { "TCP".into() },
                    expires: now as i64 + 600,
                },
            )
        })
        .collect();
    let rates = vec![AnnounceRate { hash: [7; 16], last: now - 60.0, per_hour: Some(2.0), violations: 1, blocked_until: now + 150.0 }];
    web.with_owner(move |o| o.apply(NetEvent::Tables(Ok(Tables { paths, rates })))).await;
    let read = web.get("/api/tables?limit=10").await.json();
    assert_eq!((read["total"].as_u64(), read["rows"].as_array().unwrap().len()), (Some(30), 10), "{read}");
    assert_eq!(read["rows"][0]["hops"], 0);
    let lora = web.get("/api/tables?q=lora").await.json();
    assert_eq!(lora["rows"][0]["hash"], hex::encode([7u8; 16]));
    assert!(lora["rows"][0]["text"].as_str().unwrap().contains("on LoRa"));
    let rates = web.get("/api/tables?table=rates").await.json();
    assert_eq!(rates["rows"][0]["held"], true);
    assert!(rates["rows"][0]["text"].as_str().unwrap().contains("held back for 2 min"), "{rates}");
    web.with_owner(|o| o.apply(NetEvent::Tables(Err("The path table couldn't be read".into())))).await;
    assert_eq!(web.get("/api/tables").await.json()["error"], "The path table couldn't be read");
}

#[tokio::test]
async fn interface_profiles_over_the_api() {
    let web = TestWeb::start("rns-profiles");
    let listed = web.get("/api/reticulum/profiles").await.json();
    assert!(listed["profiles"].as_array().unwrap().is_empty() && listed["now"].is_null(), "{listed}");
    web.post("/api/reticulum/interfaces", json!({ "action": "add", "name": "Local", "type": "AutoInterface" })).await;
    let saved = web.post("/api/reticulum/profiles", json!({ "name": "Home" })).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
    assert_eq!(saved.json()["profile"]["interfaces"]["Local"], true);
    assert_eq!(web.post("/api/reticulum/profiles", json!({ "name": " " })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.get("/api/reticulum/profiles").await.json()["now"], "Home");

    // A pipe interface, added in the terminal and kept on in a profile there.
    web.with(|app| {
        app.rns_add_interface("Pipe", "PipeInterface", false).unwrap();
        app.rns_set_options(&crate::reticulum::Section::Interface("Pipe".into()), &[("command", "cat")], false).unwrap();
        app.rns_save_profile("Piped").unwrap();
    })
    .await;
    // Off, and Local off, from here.
    for name in ["Pipe", "Local"] {
        let off =
            web.post("/api/reticulum/options", json!({ "section": format!("interface:{name}"), "values": { "enabled": "no" } })).await;
        assert_eq!(off.status, StatusCode::OK, "{}", off.text());
    }
    // Switching to the profile with the pipe on isn't for the web UI: it
    // would run its program.
    let refused = web.post("/api/reticulum/profiles/Piped/switch", json!({})).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text());
    assert!(!web.with(|app| app.take_rns_restart()).await);
    // Home is fine: Local back on, and Reticulum restarted.
    let home = web.post("/api/reticulum/profiles/Home/switch", json!({})).await;
    assert_eq!(home.status, StatusCode::OK, "{}", home.text());
    assert_eq!(home.json()["changed"], json!(["Local on"]));
    assert!(web.with(|app| app.take_rns_restart()).await);
    assert_eq!(web.post("/api/reticulum/profiles/Nowhere/switch", json!({})).await.status, StatusCode::BAD_REQUEST);
    // Deleted.
    assert_eq!(web.delete("/api/reticulum/profiles/Home").await.status, StatusCode::OK);
    assert_eq!(web.delete("/api/reticulum/profiles/Home").await.status, StatusCode::BAD_REQUEST);
    let names: Vec<serde_json::Value> =
        web.get("/api/reticulum/profiles").await.json()["profiles"].as_array().unwrap().iter().map(|p| p["name"].clone()).collect();
    assert_eq!(names, [json!("Piped")]);
}

#[tokio::test]
async fn the_reticulum_configs_history_over_the_api() {
    let web = TestWeb::start("rns-history");
    assert!(web.get("/api/reticulum/history").await.json()["versions"].as_array().unwrap().is_empty());
    let added = web.post("/api/reticulum/interfaces", json!({ "action": "add", "name": "Local", "type": "AutoInterface" })).await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.text());
    // A pipe interface, added in the terminal (the web UI can't).
    let pipe = "PipeInterface";
    web.with(move |app| {
        app.rns_add_interface("Pipe", pipe, false).unwrap();
        app.rns_set_options(&crate::reticulum::Section::Interface("Pipe".into()), &[("command", "cat")], false).unwrap();
    })
    .await;
    let removed = web.post("/api/reticulum/interfaces", json!({ "action": "delete", "name": "Pipe" })).await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.text());
    let versions = web.get("/api/reticulum/history").await.json()["versions"].as_array().unwrap().clone();
    let whats: Vec<&str> = versions.iter().map(|v| v["what"].as_str().unwrap()).collect();
    assert_eq!(
        whats,
        ["Removed interface Pipe", "Pipe: command = cat", "Added interface Pipe (PipeInterface)", "Added interface Local (AutoInterface)"]
    );
    assert!(versions[0]["when"].as_str().is_some_and(|w| !w.is_empty()));
    // What the newest changed: Pipe's lines removed.
    let id = |n: usize| versions[n]["id"].as_str().unwrap().to_string();
    let newest = web.get(&format!("/api/reticulum/history/{}", id(0))).await.json();
    assert!(newest["removed"].as_u64().unwrap() >= 3 && newest["added"] == 0, "{newest}");
    let removed_pipe =
        newest["lines"].as_array().unwrap().iter().any(|l| l["kind"] == "removed" && l["text"].as_str().unwrap().trim() == "[[Pipe]]");
    assert!(removed_pipe, "{newest}");
    // Compared with the file now: no differences.
    let now = web.get(&format!("/api/reticulum/history/{}?against=current", id(0))).await.json();
    assert_eq!((now["added"].as_u64(), now["removed"].as_u64()), (Some(0), Some(0)));
    // Putting back a version with the pipe's command isn't for the web UI.
    let refused = web.post(&format!("/api/reticulum/history/{}/restore", id(1)), json!({})).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text());
    assert!(!std::fs::read_to_string(web.with(|app| app.rns_path()).await).unwrap().contains("[[Pipe]]"));
    // The oldest is fine.
    let back = web.post(&format!("/api/reticulum/history/{}/restore", id(3)), json!({})).await;
    assert_eq!(back.status, StatusCode::OK, "{}", back.text());
    let versions = web.get("/api/reticulum/history").await.json()["versions"].as_array().unwrap().clone();
    assert!(versions[0]["what"].as_str().unwrap().starts_with("Put back the version of"));
    assert_eq!(web.get("/api/reticulum/history/nope").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.post("/api/reticulum/history/123/restore", json!({})).await.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_newer_games_over_the_api() {
    use crate::games::{Games, fields};
    let web = TestWeb::start("newer-games");
    web.post("/api/conversations", json!({ "address": BOB })).await;
    web.with(|app| app.games.set_me(&[0xa1; 16])).await;
    let mut bob = Games::default();
    bob.set_me(&hex::decode(BOB).unwrap().try_into().unwrap());
    let wire = |prepared: &crate::games::Prepared| -> std::collections::HashMap<u8, Vec<u8>> {
        fields(&hex::decode(prepared.game.envelope.as_ref().unwrap()).unwrap()).into_iter().collect()
    };
    // Each game's board, its size, and a move of the first mover's.
    for (app, size, first) in
        [("hex", 121, "f6"), ("gomoku", 225, "h8"), ("reversi", 64, "d3"), ("dots_and_boxes", 11, "c3h"), ("mancala", 14, "c")]
    {
        let challenge = web.with(move |a| a.games.challenge(app, BOB)).await.unwrap();
        let session = challenge.game.session.clone();
        bob.receive(&hex::encode([0xa1; 16]), &wire(&challenge));
        let accept = bob.act(app, &session, "accept", Default::default()).unwrap();
        web.with(move |a| a.games.receive(BOB, &wire(&accept))).await;
        let game = web.get(&format!("/api/games/{app}/{session}")).await.json();
        assert_eq!((game["status"].as_str(), game["board"]["kind"].as_str()), (Some("active"), Some(app)), "{game}");
        let shown = ["cells", "rows", "holes"].iter().find_map(|k| game["board"][k].as_array()).unwrap();
        assert_eq!(shown.len(), size, "{app}");
        let black = game["board"]["black"].as_bool().unwrap();
        if !black {
            let theirs = bob.act(app, &session, "move", crate::games::move_payload(app, first).unwrap()).unwrap();
            web.with(move |a| a.games.receive(BOB, &wire(&theirs))).await;
            assert_eq!(web.get(&format!("/api/games/{app}/{session}")).await.json()["board"]["last"], first);
            continue;
        }
        let bad = web.post(&format!("/api/games/{app}/{session}"), json!({ "command": "move", "picked": "zz9" })).await;
        assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{app}");
        let moved = web.post(&format!("/api/games/{app}/{session}"), json!({ "command": "move", "picked": first })).await;
        assert_eq!(moved.status, StatusCode::OK, "{app}: {}", moved.text());
        assert_eq!(web.get(&format!("/api/games/{app}/{session}")).await.json()["board"]["last"], first);
    }
}

#[tokio::test]
async fn the_network_paths_and_announces() {
    let web = TestWeb::start("network");
    web.with(|app| {
        app.on_net(NetEvent::Announce { kind: PeerKind::Lxmf, hash: [0xab; 16], name: Some("Bob".into()), hops: 2 });
        app.on_net(NetEvent::Announce { kind: PeerKind::Nomad, hash: [0xcd; 16], name: Some("Node".into()), hops: 1 });
    })
    .await;
    let peers = web.get("/api/peers").await.json();
    assert!(peers.to_string().contains("Bob") && peers.to_string().contains("Node"), "{peers}");
    let nodes = web.get("/api/peers?kind=nomad&sort=name&limit=5").await.json();
    assert!(!nodes.to_string().contains("\"Bob\""), "{nodes}");
    assert!(web.get("/api/peers?q=bo&sort=hops").await.json().to_string().contains("Bob"));
    assert!(web.get("/api/announces?kind=lxmf").await.json().is_object() || web.get("/api/announces").await.json().is_array());
    assert_eq!(web.get("/api/announces?kind=martian").await.status, StatusCode::BAD_REQUEST);

    // Paths and probes: what's known, asking (not waited for here), and
    // forgetting.
    let path = web.get(&format!("/api/path/{BOB}")).await.json();
    assert_eq!(path["state"], "none");
    assert_eq!(web.get("/api/path/nope").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.get(&format!("/api/probe/{BOB}")).await.json()["state"], "none");
    assert_eq!(web.delete(&format!("/api/path/{BOB}")).await.status, StatusCode::OK);
    assert!(web.sent().iter().any(|c| matches!(c, NetCommand::ForgetPath { .. })));
}

/// A NomadNet node's address.
const NODE: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

#[tokio::test]
async fn browsing_pages_forms_and_files() {
    let web = TestWeb::start("browse");
    web.serve(
        "/page/index.mu",
        ">Welcome\nSee `[the news`:/page/news.mu] and `F00f`[lxmf@abababababababababababababababab]`f\n`[img`:/media/cat.png]",
    );
    web.serve("/page/form.mu", "Thanks, `<name`Bob> was sent");
    web.serve("/media/cat.png", "\u{89}PNG fake");
    web.pages.lock().unwrap().insert("/page/broken.mu".into(), Err("No path to the node".into()));
    let url = format!("{NODE}:/page/index.mu");
    let page = web.get(&format!("/api/page?url={url}")).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text());
    let page = page.json();
    assert!(page["html"].as_str().unwrap().contains("Welcome"));
    assert!(page["html"].as_str().unwrap().contains(&format!("{NODE}:/page/news.mu")), "links resolve: {}", page["html"]);
    assert_eq!(page["node"], NODE);
    assert_eq!(page["saved"], false);
    assert!(page["source_html"].as_str().unwrap().contains("Welcome"));
    // Kept, so the next look comes from the cache, unless refreshed.
    let again = web.get(&format!("/api/page?url={url}")).await.json();
    assert!(again["cached_age"].is_number(), "{again}");
    let fresh = web.get(&format!("/api/page?url={url}&refresh=true")).await.json();
    assert!(fresh["cached_age"].is_null());
    // A node that can't be reached says why; nonsense isn't an address;
    // a file is downloaded instead.
    let broken = web.get(&format!("/api/page?url={NODE}:/page/broken.mu")).await;
    assert_eq!(broken.status, StatusCode::BAD_REQUEST);
    assert!(broken.error().contains("No path"));
    assert_eq!(web.get("/api/page?url=nowhere").await.status, StatusCode::BAD_REQUEST);
    let file = web.get(&format!("/api/page?url={NODE}:/file/report.pdf")).await.json();
    assert!(file["download"].as_str().unwrap().ends_with("/file/report.pdf"));
    // A form sent; a partial; a picture.
    let form = web.post("/api/page", json!({ "url": format!("{NODE}:/page/form.mu"), "fields": { "name": "Bob" } })).await;
    assert!(form.json()["html"].as_str().unwrap().contains("Thanks"));
    let partial = web.post("/api/partial", json!({ "url": format!("{NODE}:/page/form.mu") })).await;
    assert!(partial.json()["html"].as_str().unwrap().contains("Thanks"));
    let picture = web.get(&format!("/api/media?url={NODE}:/media/cat.png")).await;
    assert_eq!(picture.status, StatusCode::OK);
    assert_eq!(web.get(&format!("/api/media?url={NODE}:/page/index.mu")).await.status, StatusCode::BAD_REQUEST);
    web.serve("/file/notes.txt", "some notes");
    let download = web.get(&format!("/api/download?url={NODE}:/file/notes.txt")).await;
    assert_eq!(download.body, b"some notes");
    assert!(download.headers[header::CONTENT_DISPOSITION].to_str().unwrap().starts_with("attachment"));

    // Saved pages: once each, and taken off again.
    assert_eq!(web.post("/api/saved", json!({ "url": url })).await.status, StatusCode::OK);
    web.post("/api/saved", json!({ "url": url })).await;
    let saved = web.get("/api/saved").await.json();
    assert_eq!(saved.to_string().matches("index.mu").count(), 1, "{saved}");
    assert_eq!(web.get(&format!("/api/page?url={url}")).await.json()["saved"], true);
    web.post("/api/saved/remove", json!({ "url": url })).await;
    assert!(!web.get("/api/saved").await.json().to_string().contains("index.mu"));
    // Identifying to a node.
    assert_eq!(web.post("/api/identify", json!({ "node": NODE, "on": true })).await.status, StatusCode::OK);
    assert_eq!(web.get(&format!("/api/page?url={url}&refresh=true")).await.json()["identified"], true);
    assert_eq!(web.post("/api/identify", json!({ "node": "x", "on": true })).await.status, StatusCode::BAD_REQUEST);
    // The cache has what was loaded.
    assert!(web.post("/api/cache/clear", json!({})).await.json()["removed"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn hosting_a_node_pages_files_and_uploads() {
    let web = TestWeb::start("node");
    let node = web.get("/api/node").await.json();
    assert!(node.is_object());
    // A page made, written, read back, previewed.
    let made = web.post("/api/node/pages", json!({ "name": "about" })).await.json();
    let path = made["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("about.mu"), "{path}");
    assert_eq!(web.post("/api/node/pages", json!({ "name": "about" })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.post("/api/node/page", json!({ "path": path, "content": ">About\nAll about us" })).await.status, StatusCode::OK);
    let read = web.get(&format!("/api/node/page?path={path}")).await.json();
    assert_eq!(read["content"], ">About\nAll about us");
    assert_eq!(read["executable"], false);
    assert_eq!(web.get("/api/node/page?path=nope.mu").await.status, StatusCode::BAD_REQUEST);
    let preview = web.post("/api/node/preview", json!({ "content": ">Hi\n`[img`:/media/x.png]" })).await.json();
    assert!(preview["html"].as_str().unwrap().contains("Hi"));
    // Renamed, moved into a folder, deleted.
    let renamed = web.post("/api/node/rename", json!({ "from": path, "to": "us.mu" })).await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    let renamed = renamed.json()["path"].as_str().unwrap().to_string();
    let moved = web.post("/api/node/move", json!({ "path": renamed, "folder": "info" })).await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
    let moved = moved.json()["path"].as_str().unwrap().to_string();
    assert!(moved.starts_with("info/"), "{moved}");
    assert_eq!(web.post("/api/node/move", json!({ "path": moved, "root": "attic" })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.post("/api/node/delete", json!({ "path": moved })).await.status, StatusCode::OK);
    assert_eq!(web.post("/api/node/delete", json!({ "path": moved })).await.status, StatusCode::BAD_REQUEST);
    // Uploads: a file to download, and a picture shown with the pages.
    let file = web.post_bytes("/api/node/upload?name=notes.txt", "application/octet-stream", b"notes".to_vec()).await;
    assert_eq!(file.status, StatusCode::OK, "{}", file.text());
    assert_eq!(file.json()["image"], false);
    let mut png = Vec::new();
    image::RgbImage::from_pixel(4, 4, image::Rgb([200, 30, 30]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let picture = web.post_bytes("/api/node/upload?name=dot.png", "image/png", png).await;
    assert_eq!(picture.status, StatusCode::OK, "{}", picture.text());
    assert_eq!(picture.json()["image"], true);
    let shown = web.get("/api/node").await.json().to_string();
    assert!(shown.contains("notes.txt") && shown.contains("dot.png"), "{shown}");
    let address = picture.json()["address"].as_str().unwrap().trim_start_matches(":/media/").to_string();
    let media = web.get(&format!("/api/node/media?path={address}")).await;
    assert_eq!(media.status, StatusCode::OK);
    assert_eq!(web.get("/api/node/media?path=nope.png").await.status, StatusCode::NOT_FOUND);
    // Files can be renamed and deleted in their own root.
    assert_eq!(
        web.post("/api/node/rename", json!({ "from": "notes.txt", "to": "readme.txt", "root": "files" })).await.status,
        StatusCode::OK
    );
    assert_eq!(web.post("/api/node/delete", json!({ "path": "readme.txt", "root": "files" })).await.status, StatusCode::OK);
}

#[tokio::test]
async fn the_hub_hosted_here_and_reticulums_config() {
    let web = TestWeb::start("hub");
    let hub = web.get("/api/hub").await.json();
    assert!(hub.is_object());
    // Not running: its actions say so; unknown ones aren't actions.
    for action in
        ["announce", "run", "kick", "ban", "op", "voice", "invite", "key", "topic", "modes", "register", "delete", "rename", "disconnect"]
    {
        let answer = web.post(&format!("/api/hub/{action}"), json!({ "room": "general", "identity": "ab".repeat(16), "text": "x" })).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{action}: {}", answer.text());
    }
    assert_eq!(web.post("/api/hub/dance", json!({})).await.status, StatusCode::BAD_REQUEST);
    // Turned on in the settings.
    assert_eq!(web.post("/api/hub/enable", json!({ "on": true })).await.status, StatusCode::OK);
    assert!(web.with(|app| app.settings.hub_enabled).await);
    web.post("/api/hub/enable", json!({ "on": false })).await;

    // Reticulum's config: sections, options, interfaces, the text.
    let rns = web.get("/api/reticulum").await;
    assert_eq!(rns.status, StatusCode::OK, "{}", rns.text());
    assert!(rns.json().to_string().contains("reticulum"));
    assert_eq!(web.get("/api/reticulum?section=logging").await.status, StatusCode::OK);
    // An RNode's settings worked out, saved and as they're typed.
    assert!(web.get("/api/reticulum").await.json()["radio"].is_null());
    let lora = web.get("/api/reticulum/lora?bandwidth=125000&spreadingfactor=8&codingrate=5&txpower=7&frequency=867200000").await.json();
    assert!(lora["lines"][0].as_str().unwrap().contains("3.13 kbps"), "{lora}");
    assert!(web.get("/api/reticulum/lora?bandwidth=125000&spreadingfactor=99&codingrate=5").await.json()["lines"].is_null());
    web.post("/api/reticulum/interfaces", json!({ "action": "add", "name": "Radio", "type": "RNodeInterface" })).await;
    let radio = web.get("/api/reticulum?section=interface:Radio").await.json();
    assert!(radio["radio"][1].as_str().unwrap().contains("link budget"), "{radio}");
    web.post("/api/reticulum/interfaces", json!({ "action": "delete", "name": "Radio" })).await;
    let set = web.post("/api/reticulum/options", json!({ "section": "logging", "values": { "loglevel": "5" } })).await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.text());
    assert_eq!(web.post("/api/reticulum/options", json!({ "section": "attic", "values": {} })).await.status, StatusCode::BAD_REQUEST);
    let added = web.post("/api/reticulum/interfaces", json!({ "action": "add", "name": "Local", "type": "AutoInterface" })).await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.text());
    assert!(web.get("/api/reticulum?section=interface:Local").await.json().to_string().contains("Local"));
    assert_eq!(
        web.post("/api/reticulum/interfaces", json!({ "action": "rename", "name": "Local", "to": "Home" })).await.status,
        StatusCode::OK
    );
    assert_eq!(web.post("/api/reticulum/interfaces", json!({ "action": "delete", "name": "Home" })).await.status, StatusCode::OK);
    assert_eq!(web.post("/api/reticulum/interfaces", json!({ "action": "paint", "name": "Home" })).await.status, StatusCode::BAD_REQUEST);
    // Pipe interfaces run programs: not from the web UI.
    let pipe = web
        .post(
            "/api/reticulum/text",
            json!({ "text": "[reticulum]\n[interfaces]\n  [[Pipe]]\n    type = PipeInterface\n    command = cat\n" }),
        )
        .await;
    assert_eq!(pipe.status, StatusCode::BAD_REQUEST, "{}", pipe.text());
    let check = web.post("/api/reticulum/check", json!({ "text": "[reticulum]\n  share_instance = maybe\n" })).await.json();
    assert!(check["error"].is_string() || !check["warnings"].as_array().unwrap().is_empty(), "{check}");
    assert_eq!(web.post("/api/reticulum/text", json!({ "text": "[reticulum]\n  share_instance = Yes\n" })).await.status, StatusCode::OK);
    assert_eq!(web.post("/api/reticulum/restart", json!({})).await.status, StatusCode::OK);
    assert!(web.with(|app| app.take_rns_restart()).await);
}

#[tokio::test]
async fn channels_hubs_and_rooms() {
    let web = TestWeb::start("channels");
    let hub = "ef".repeat(16);
    let added = web.post("/api/channels", json!({ "address": format!("rrc://{hub}/general") })).await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.text());
    assert_eq!(added.json()["hub"], hub.as_str());
    assert_eq!(web.post("/api/channels", json!({ "address": "nope" })).await.status, StatusCode::BAD_REQUEST);
    let channels = web.get("/api/channels").await.json();
    assert!(channels.to_string().contains(&hub));
    let room = web.get(&format!("/api/channels/{hub}/room?name=general&last=10")).await;
    assert_eq!(room.status, StatusCode::OK, "{}", room.text());
    assert_eq!(web.get(&format!("/api/channels/{}/room", "12".repeat(16))).await.status, StatusCode::NOT_FOUND);
    assert_eq!(web.get("/api/channels/x/room").await.status, StatusCode::BAD_REQUEST);
    // What a hub's menu does, connected or not.
    for (action, body) in [
        ("auto", json!({})),
        ("join", json!({ "room": "random" })),
        ("read", json!({ "room": "general" })),
        ("notify", json!({ "room": "general", "level": "mentions" })),
        ("notify", json!({ "room": "general", "level": "default" })),
        ("leave", json!({ "room": "random" })),
        ("forget", json!({ "room": "random" })),
        ("send", json!({ "room": "general", "text": "hello" })),
        ("split", json!({ "room": "general", "parts": ["a", "b"] })),
        ("connect", json!({})),
    ] {
        let answer = web.post(&format!("/api/channels/{hub}/{action}"), body).await;
        assert_eq!(answer.status, StatusCode::OK, "{action}: {}", answer.text());
    }
    assert_eq!(web.post(&format!("/api/channels/{hub}/notify"), json!({ "level": "loud" })).await.status, StatusCode::BAD_REQUEST);
    let whisper = web.post(&format!("/api/channels/{hub}/whisper"), json!({ "src": "ab".repeat(16) })).await;
    assert_eq!(whisper.status, StatusCode::OK, "{}", whisper.text());
    assert_eq!(web.post(&format!("/api/channels/{hub}/whisper"), json!({ "src": "x" })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.post(&format!("/api/channels/{hub}/dance"), json!({})).await.status, StatusCode::BAD_REQUEST);
    assert!(
        web.get("/api/channels/search?q=hello").await.json().is_object() || web.get("/api/channels/search?q=hello").await.json().is_array()
    );
    assert_eq!(web.get(&format!("/api/channels/search?q=x&hub={hub}&room=general")).await.status, StatusCode::OK);
    assert_eq!(web.post(&format!("/api/channels/{hub}/remove"), json!({})).await.status, StatusCode::OK);
    assert_eq!(web.post(&format!("/api/channels/{hub}/remove"), json!({})).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn paper_backups_tiles_and_signing_out() {
    let web = TestWeb::start("paper");
    // A contact's link is taken as paper too; nonsense isn't either.
    let link = format!("lxma://{BOB}:{}", "11".repeat(64));
    let contact = web.post("/api/paper/read", json!({ "link": link })).await;
    assert!(contact.status == StatusCode::OK || contact.status == StatusCode::BAD_REQUEST, "{}", contact.text());
    let nonsense = web.post("/api/paper/read", json!({ "link": "lxm://nonsense" })).await;
    assert_eq!(nonsense.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.post("/api/paper/scan", json!({ "image": "!!" })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.post("/api/paper/scan", json!({ "image": "aGVsbG8=" })).await.status, StatusCode::BAD_REQUEST);

    // A backup to download.
    web.with(|app| app.on_message(from_bob(1, "keep this"))).await;
    let backup = web.get("/api/backup").await;
    assert_eq!(backup.status, StatusCode::OK, "{}", backup.text());
    assert_eq!(backup.headers[header::CONTENT_TYPE], "application/gzip");
    assert!(backup.body.starts_with(&[0x1f, 0x8b]));

    // Map tiles: none set and no offline map here.
    assert_eq!(web.get("/api/map/tiles/1/0/0").await.status, StatusCode::NOT_FOUND);

    // Signing every other browser out: a new token, in this one's cookie.
    let out = web.post("/api/sign-out-others", json!({})).await;
    assert_eq!(out.status, StatusCode::OK);
    assert!(out.headers[header::SET_COOKIE].to_str().unwrap().starts_with("rettui_token="));
    assert_eq!(web.get("/api/state").await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn notifications_a_browser_missed_while_away() {
    let web = TestWeb::start("missed");
    web.with(|app| {
        app.set_focus(false);
        app.on_message(from_bob(1, "Are you there?"));
        app.on_message(from_bob(2, "Hello?"));
    })
    .await;
    // The newest about each conversation still unread, and none once read.
    let missed = web.with_owner(|owner| owner.missed(0)).await;
    assert_eq!(missed.len(), 1, "{missed:?}");
    assert_eq!(missed[0].1.body, "Hello?");
    web.post(&format!("/api/conversations/{BOB}/read"), json!({})).await;
    assert!(web.with_owner(|owner| owner.missed(0)).await.is_empty());
}

#[tokio::test]
async fn voice_messages_played_as_they_come_over_the_api() {
    use crate::lxmf::fields::Audio;
    let web = TestWeb::start("autoplay");
    web.with(|app| app.on_message(from_bob(1, "hello"))).await;
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/autoplay"), json!({ "on": true })).await.status, StatusCode::OK);
    assert_eq!(
        web.post("/api/conversations/cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd/autoplay", json!({ "on": true })).await.status,
        StatusCode::NOT_FOUND
    );
    let conversation = web.get(&format!("/api/conversations/{BOB}")).await.json();
    assert_eq!(conversation["autoplay"], true, "{conversation}");
    let list = web.get("/api/conversations").await.json().to_string();
    assert!(list.contains("\"autoplay\":true"), "{list}");
    let played = web
        .with(|app| {
            let audio = Some(Audio::new(0x04, vec![0; 60]).decoded());
            app.on_message(InboundMessage { extras: Extras { audio, ..Extras::default() }, ..from_bob(2, "") });
            app.take_autoplays()
        })
        .await;
    assert_eq!(played.len(), 1);
    let voice = web.send(Method::GET, &format!("/api/conversations/{BOB}/attachments/{}/0", played[0].id), None, false).await;
    assert_eq!(voice.status, StatusCode::OK);
    assert_eq!(web.post(&format!("/api/conversations/{BOB}/autoplay"), json!({ "on": false })).await.status, StatusCode::OK);
}

#[tokio::test]
async fn live_updates_notifications_and_signing_out() {
    use futures_util::StreamExt;

    use super::{Notice, Scope};
    use crate::app::notify::{Notification, Target};
    let web = TestWeb::start("events");
    web.with(|app| {
        app.set_focus(false);
        app.on_message(from_bob(1, "Are you there?"));
    })
    .await;
    // A browser back after a while: what it missed first (by the header,
    // or by ?since=), then what happens.
    let mut events = Box::pin(web.events("/api/events", Some("0")).await);
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), events.next()).await.unwrap().unwrap();
    assert!(first.contains("event: notify") && first.contains("Are you there?") && first.contains("\"replay\":true"), "{first}");
    let state = web.state();
    let _ = state.changes.send((7, Scope::ALL));
    let notification = Notification { title: "Bob".into(), body: "Again".into(), target: Target::Conversation { key: BOB.into() } };
    // One it already had (by its id), then a new one, then one read.
    let _ = state.notices.send(Notice::Show(1, notification.clone()));
    let _ = state.notices.send(Notice::Show(99, notification));
    let _ = state.notices.send(Notice::Read(format!("conversation:{BOB}")));
    let mut said = String::new();
    // The two channels are read in no set order: until both have come.
    while !(said.contains("event: read") && said.contains("data: 7 all")) {
        said += &tokio::time::timeout(std::time::Duration::from_secs(5), events.next()).await.unwrap().unwrap();
    }
    assert!(said.contains("\"body\":\"Again\""), "{said}");
    assert_eq!(said.matches("event: notify").count(), 1, "{said}");

    // Too many changes at once: one event says to refetch it all.
    for version in 0..100 {
        let _ = state.changes.send((version, Scope::STATUS));
    }
    let mut said = String::new();
    while !said.contains("data: 0 all") {
        said += &tokio::time::timeout(std::time::Duration::from_secs(5), events.next()).await.unwrap().unwrap();
    }

    // Notifications only (the Android app's service): what changed isn't
    // sent at all.
    let mut quiet = Box::pin(web.events("/api/events?notices=1&since=99", None).await);
    let _ = state.changes.send((8, Scope::ALL));
    let _ = state.notices.send(Notice::Read(format!("conversation:{BOB}")));
    let next = tokio::time::timeout(std::time::Duration::from_secs(5), quiet.next()).await.unwrap().unwrap();
    assert!(next.contains("event: read") && !next.contains("8 all"), "{next}");

    // A voice message to play as it comes.
    let voice = crate::app::Autoplay { key: BOB.into(), id: "m1".into(), index: 0, path: "v.wav".into() };
    let _ = state.notices.send(Notice::Play(voice));
    let mut said = String::new();
    while !said.contains("event: play") {
        said += &tokio::time::timeout(std::time::Duration::from_secs(5), events.next()).await.unwrap().unwrap();
    }
    assert!(said.contains(&format!("\"key\":\"{BOB}\"")) && said.contains("\"index\":0"), "{said}");

    // Signing every browser out ends the stream.
    let fresh = Box::pin(web.events("/api/events?since=0", None).await);
    let _ = state.signed_out.send(1);
    let rest: Vec<String> = tokio::time::timeout(std::time::Duration::from_secs(5), fresh.collect()).await.unwrap();
    assert!(rest.iter().any(|e| e.contains("Are you there?")), "{rest:?}");
}

#[tokio::test]
async fn background_notifications_for_browsers() {
    use base64::Engine;
    let web = TestWeb::start("push");
    let key = web.get("/api/push").await.json();
    assert!(key["key"].as_str().is_some_and(|k| k.len() > 40), "{key}");
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let browser = p256::SecretKey::generate().public_key().to_sec1_bytes();
    let endpoint = "https://fcm.googleapis.com/fcm/send/test";
    let subscription = json!({ "endpoint": endpoint, "keys": { "p256dh": b64(&browser), "auth": b64(&[7; 16]) } });
    assert_eq!(web.post("/api/push/subscribe", subscription.clone()).await.status, StatusCode::OK);
    // Showing the web UI: nothing is pushed to it (nor sent anywhere here).
    assert_eq!(web.post("/api/push/showing", json!({ "endpoint": endpoint, "showing": true })).await.status, StatusCode::OK);
    web.state().push.notify(&crate::app::notify::Notification {
        title: "Bob".into(),
        body: "Hi".into(),
        target: crate::app::notify::Target::Conversation { key: BOB.into() },
    });
    assert!(web.state().push.reports().is_empty());
    assert_eq!(web.post("/api/push/showing", json!({ "endpoint": endpoint })).await.status, StatusCode::OK);
    assert_eq!(web.post("/api/push/unsubscribe", json!({ "endpoint": endpoint })).await.status, StatusCode::OK);

    // What isn't a subscription rettui can push to.
    let refused =
        |endpoint: &str, p256dh: String, auth: String| json!({ "endpoint": endpoint, "keys": { "p256dh": p256dh, "auth": auth } });
    for (body, why) in [
        (refused("https://push.example.com/x", b64(&browser), b64(&[7; 16])), "push service"),
        (refused(endpoint, "!!".into(), b64(&[7; 16])), "base64url"),
        (refused(endpoint, b64(&[1; 65]), b64(&[7; 16])), "P-256"),
        (refused(endpoint, b64(&browser), "!!".into()), "base64url"),
        (refused(endpoint, b64(&browser), b64(&[7; 3])), "16 bytes"),
    ] {
        let answer = web.post("/api/push/subscribe", body).await;
        assert!(answer.error().contains(why), "{why}: {}", answer.error());
    }
}

#[tokio::test]
async fn map_tiles_from_a_file_and_the_web() {
    let web = TestWeb::start("tiles");
    // None set: none.
    assert_eq!(web.get("/api/map/tiles/1/0/0").await.status, StatusCode::NOT_FOUND);
    let file = std::env::temp_dir().join(format!("rettui-api-tiles-{}.mbtiles", std::process::id()));
    super::mbtiles::sample(&file, "png");
    let path = file.display().to_string();
    web.with(move |app| app.settings.map_tiles_file = Some(path)).await;
    let tile = web.get("/api/map/tiles/1/0/0").await;
    assert_eq!((tile.status, tile.headers[header::CONTENT_TYPE].to_str().unwrap()), (StatusCode::OK, "image/png"));
    // One it hasn't, and the web unreachable: not found, or not fetched.
    assert_eq!(web.get("/api/map/tiles/1/1/1").await.status, StatusCode::NOT_FOUND);
    web.with(|app| app.settings.map_tiles = Some("http://127.0.0.1:1/{z}/{x}/{y}.png".into())).await;
    assert_eq!(web.get("/api/map/tiles/1/1/1").await.status, StatusCode::BAD_GATEWAY);
    web.with(|app| app.settings.map_tiles_file = None).await;
    assert_eq!(web.get("/api/map/tiles/1/1/1").await.status, StatusCode::BAD_GATEWAY);
    assert_eq!(web.get("/api/map/tiles/1/9/9").await.status, StatusCode::NOT_FOUND);
    super::mbtiles::forget();
    let _ = std::fs::remove_file(&file);
}

#[tokio::test]
async fn waiting_for_a_path_a_probe_and_a_ping() {
    use crate::app::contacts::PingState;
    use crate::app::paths::{PathLookup, ProbeState};
    use crate::net::remote::{PathInfo, Ping, Probe};
    let web = std::sync::Arc::new(TestWeb::start("waiting"));
    // Each asked, then answered while the browser waits.
    let answer = |web: &std::sync::Arc<TestWeb>, path: String| {
        let web = web.clone();
        tokio::spawn(async move { web.post(&path, json!({})).await })
    };
    let asked = answer(&web, format!("/api/path/{BOB}"));
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    web.with(|app| {
        let info = PathInfo { hops: 2, via: Some([1; 16]), interface: "TCP".into(), expires: 2_000_000_000 };
        app.path_lookups.insert(BOB.into(), PathLookup::Done(Ok(info)));
    })
    .await;
    let path = asked.await.unwrap().json();
    assert_eq!((path["state"].as_str(), path["hops"].as_u64()), (Some("found"), Some(2)));
    web.with(|app| app.path_lookups.insert(BOB.into(), PathLookup::Done(Err("no path".into())))).await;
    assert_eq!(web.get(&format!("/api/path/{BOB}")).await.json()["state"], "failed");
    web.with(|app| app.path_lookups.insert(BOB.into(), PathLookup::Waiting(None))).await;
    assert_eq!(web.get(&format!("/api/path/{BOB}")).await.json()["state"], "waiting");

    let asked = answer(&web, format!("/api/probe/{BOB}"));
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(web.get(&format!("/api/probe/{BOB}")).await.json()["state"], "waiting");
    web.with(|app| {
        let probe = Probe { rtt: std::time::Duration::from_millis(120), hops: Some(1), kind: Some("lxmf"), by_link: false };
        app.probes.insert(BOB.into(), ProbeState::Done(Ok(probe)));
    })
    .await;
    let probe = asked.await.unwrap().json();
    assert_eq!((probe["state"].as_str(), probe["ms"].as_u64()), (Some("found"), Some(120)));
    web.with(|app| app.probes.insert(BOB.into(), ProbeState::Done(Err("no answer".into())))).await;
    assert_eq!(web.get(&format!("/api/probe/{BOB}")).await.json()["state"], "failed");

    let asked = answer(&web, format!("/api/conversations/{BOB}/ping"));
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    web.with(|app| {
        let ping = Ping { rtt: std::time::Duration::from_millis(80), hops: Some(1), signal: None };
        app.pings.insert(BOB.into(), PingState::Done { at: 0, result: Ok(ping) });
    })
    .await;
    let ping = asked.await.unwrap().json();
    assert_eq!((ping["ok"].as_bool(), ping["ms"].as_u64()), (Some(true), Some(80)));
}

/// A picture of `text`'s QR code, as a PNG.
fn qr_png(text: &str) -> Vec<u8> {
    let qr = crate::lxmf::paper::Qr::new(text).unwrap();
    let scale = 6;
    let size = (qr.size() * scale) as u32;
    let picture = image::GrayImage::from_fn(size, size, |x, y| {
        image::Luma([if qr.is_dark(x as usize / scale, y as usize / scale) { 0 } else { 255 }])
    });
    let mut png = Vec::new();
    picture.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    png
}

#[tokio::test]
async fn sending_every_way_and_reading_paper() {
    use base64::Engine;
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let web = TestWeb::start("sending");
    let to_bob = format!("/api/conversations/{BOB}/send");
    let send = |body: serde_json::Value| web.post(&to_bob, body);

    // A location, a voice message, files (a picture made smaller), each
    // said wrong.
    let at = send(json!({ "location": { "latitude": 51.5, "longitude": -0.1, "accuracy": 5.0, "altitude": 30.0 } })).await;
    assert_eq!(at.status, StatusCode::OK, "{}", at.text());
    assert!(send(json!({ "location": { "latitude": 91.0, "longitude": 0.0 } })).await.error().contains("latitude"));
    assert!(send(json!({ "voice": "!!" })).await.error().contains("base64"));
    assert!(send(json!({ "voice": b64(b"x"), "files": [{ "name": "a.txt", "data": b64(b"a") }] })).await.error().contains("on its own"));
    let _ = send(json!({ "voice": b64(&[0u8; 64]) })).await;
    assert!(send(json!({ "files": [{ "name": "a.txt", "data": "!!" }] })).await.error().contains("not valid base64"));
    let mut png = Vec::new();
    image::RgbImage::from_pixel(64, 48, image::Rgb([200, 10, 10]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let sent = send(json!({ "content": "A picture", "files": [{ "name": "red.png", "data": b64(&png) }] })).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    let failed = send(json!({ "content": "x", "mode": "paper", "files": [{ "name": "a.txt", "data": b64(b"a") }] })).await;
    assert_eq!(failed.status, StatusCode::BAD_REQUEST);

    // Its attachments as files: a voice message plays in the page.
    let ogg = web.state().paths.uploads.join("voice.ogg");
    std::fs::create_dir_all(ogg.parent().unwrap()).unwrap();
    std::fs::write(&ogg, b"OggS....").unwrap();
    let wav = web.state().paths.uploads.join("voice.wav");
    std::fs::write(&wav, b"RIFF....WAVEfmt ").unwrap();
    let message: crate::store::Message = serde_json::from_value(json!({
        "id": "cd".repeat(32), "incoming": true, "title": "", "content": "", "timestamp": 1.8e9,
        "state": { "Received": { "verified": true } },
        "attachments": [
            { "name": "voice.ogg", "path": ogg, "size": 8, "image": false, "voice": "Opus" },
            { "name": "voice.wav", "path": wav, "size": 16, "image": false, "voice": "Codec2 1200" },
        ],
    }))
    .unwrap();
    let archived = message.clone();
    web.with(move |app| app.store.conversations.entry(BOB.into()).or_default().messages.push(message)).await;
    let file = |index: usize| format!("/api/conversations/{BOB}/attachments/{}/{index}", "cd".repeat(32));
    assert_eq!(web.get(&file(0)).await.headers[header::CONTENT_TYPE], "audio/ogg");
    assert_eq!(web.get(&file(1)).await.headers[header::CONTENT_TYPE], "audio/wav");
    assert_eq!(web.get(&file(2)).await.status, StatusCode::NOT_FOUND);
    // And an archived message's.
    let mut archived = archived;
    archived.id = "ef".repeat(32);
    let dir = web.state().paths.archive.clone();
    std::fs::create_dir_all(&dir).unwrap();
    let lines = crate::store::encode_archive(&[crate::store::Archived { conversation: BOB.into(), message: archived }]).unwrap();
    std::fs::write(crate::store::archive_month(&dir, 1.8e9), lines).unwrap();
    let old = web.get(&format!("/api/conversations/{BOB}/attachments/{}/0", "ef".repeat(32))).await;
    assert_eq!(old.status, StatusCode::OK);

    // Paper: a contact's QR code adds them; a message's needs Reticulum
    // up, then is read (or said not to be one).
    let them = rns_identity::identity::Identity::new();
    let their_address = rns_identity::destination::Destination::hash_from_name_and_identity(crate::lxmf::LXMF_ASPECT, Some(&them.hash));
    let contact = crate::lxmf::identity_link(their_address, &them.get_public_key());
    let scanned = web.post("/api/paper/scan", json!({ "image": b64(&qr_png(&contact)) })).await.json();
    assert!(scanned.get("error").is_none(), "{scanned}");
    assert_eq!((scanned["contact"].as_bool(), scanned["key"].as_str()), (Some(true), Some(hex::encode(their_address).as_str())));
    assert!(web.post("/api/paper/scan", json!({ "image": "!!" })).await.error().contains("base64"));
    assert!(web.post("/api/paper/scan", json!({ "image": b64(b"not a picture") })).await.error().contains("Not a picture"));
    assert!(web.post("/api/paper/scan", json!({ "image": b64(&png) })).await.error().contains("No QR code"));
    assert!(web.post("/api/paper/scan", json!({ "image": b64(&qr_png("https://example.org")) })).await.error().contains("isn't a paper"));
    assert!(web.post("/api/paper/read", json!({ "link": "lxm://nonsense" })).await.error().contains("starting"));
    web.with(|app| app.lxmf_hash = Some([0x34; 16])).await;
    assert_eq!(web.post("/api/paper/read", json!({ "link": "lxm://nonsense" })).await.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_guide_pages_partials_and_previews() {
    let web = TestWeb::start("guide-pages");
    let guide = web.get("/api/guide").await.json();
    let offered = guide["entry_points"].as_array().map_or(0, Vec::len);
    let done = web.post("/api/guide", json!({ "name": "Guided", "connect": vec![false; offered], "update_check": false })).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text());
    assert_eq!(web.get("/api/state").await.json()["display_name"], "Guided");

    // A page's links: to people, hubs and other pages; a partial; the
    // editor's preview with this node's pictures.
    web.serve(
        "/page/links.mu",
        "`[Mail`lxmf@abababababababababababababababab] `[Chat`rrc://abababababababababababababababab/lobby] `[Other`cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd:/page/x.mu]\n`(A picture`:/media/a b.png)",
    );
    let page = web.get("/api/page?url=cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd:/page/links.mu").await.json();
    let html = page["html"].as_str().unwrap();
    assert!(html.contains("lxmf@abab") && html.contains("rrc://abab") && html.contains("a%20b.png"), "{html}");
    web.serve("/page/part.mu", "`(img`:/media/a b.png)\npart");
    let partial = web.post("/api/partial", json!({ "url": "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd:/page/part.mu", "fields": {} })).await.json();
    assert!(partial["html"].as_str().unwrap().contains("part"), "{partial}");
    let node = web.with(|app| hex::encode(app.node.hash)).await;
    let preview = web
        .post(
            "/api/node/preview",
            json!({ "content": format!("`(a`{node}:/media/own.png)\n`(b`cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd:/media/x.png)") }),
        )
        .await
        .json();
    let preview = preview["html"].as_str().unwrap();
    assert!(preview.contains("api/node/media?path=own.png") && preview.contains("api/media?url=cdcd"), "{preview}");
}

/// A script page of the node hosted here runs for the web UI's Browser as
/// it would for a visitor, form fields and all.
#[cfg(unix)]
#[tokio::test]
async fn own_script_pages_run_as_visitors_get_them() {
    use std::os::unix::fs::PermissionsExt;

    use crate::config::Settings;
    use crate::store::Store;

    let web = TestWeb::start_with("scripts", Settings { node_executable_pages: true, ..Settings::default() }, Store::default());
    let (node, pages) = web.with(|app| (hex::encode(app.node.hash), app.paths.node.join("pages"))).await;
    std::fs::create_dir_all(&pages).unwrap();
    let script = pages.join("hello.mu");
    std::fs::write(&script, "#!/bin/sh\necho \">Hello\"\necho \"Hi ${field_name:-stranger}\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let url = format!("{node}:/page/hello.mu");
    let page = web.get(&format!("/api/page?url={url}")).await.json();
    assert!(page["html"].as_str().unwrap().contains("Hi stranger"), "{page}");
    let sent = web.post("/api/page", json!({ "url": url, "fields": { "field_name": "Bob" } })).await.json();
    assert!(sent["html"].as_str().unwrap().contains("Hi Bob"), "{sent}");
    // One that prints nothing says why, with its errors.
    std::fs::write(&script, "#!/bin/sh\necho 'it broke' >&2\n").unwrap();
    let page = web.get(&format!("/api/page?url={url}")).await.json();
    let html = page["html"].as_str().unwrap();
    assert!(html.contains("Visitors get nothing from this page") && html.contains("it broke"), "{html}");
}

/// Sideband commands sent from the web UI, typed as in Sideband's box.
#[tokio::test]
async fn sending_sideband_commands() {
    use crate::lxmf::fields::Command;
    let web = TestWeb::start("send-commands");
    let url = format!("/api/conversations/{BOB}/command");
    let wrong = web.post(&url, json!({ "command": "dance" })).await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    assert!(wrong.error().contains("ping, echo <text>, signal or location"), "{}", wrong.error());
    assert_eq!(web.post("/api/conversations/nobody/command", json!({ "command": "ping" })).await.status, StatusCode::BAD_REQUEST);
    for (typed, command) in [("location", Command::TelemetryRequest), ("echo hello", Command::Echo("hello".into()))] {
        let sent = web.post(&url, json!({ "command": typed })).await;
        assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
        let commands: Vec<Vec<Command>> = web
            .sent()
            .into_iter()
            .filter_map(|c| match c {
                NetCommand::SendMessage { message, .. } => Some(message.commands),
                _ => None,
            })
            .collect();
        assert_eq!(commands, [vec![command]]);
    }
    let conversation = web.get(&format!("/api/conversations/{BOB}")).await.json();
    let text = conversation.to_string();
    assert!(text.contains("Asked for their location") && text.contains("Asked for an echo of “hello”"), "{text}");
}

/// Tracking someone: asked where they are every few minutes, said in the
/// conversation, and stopped.
#[tokio::test]
async fn tracking_a_contact() {
    use crate::lxmf::fields::Command;
    let web = TestWeb::start("track");
    let url = format!("/api/conversations/{BOB}/track");
    assert_eq!(web.post(&url, json!({ "minutes": 0 })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.post("/api/conversations/nobody/track", json!({ "minutes": 5 })).await.status, StatusCode::BAD_REQUEST);
    let started = web.post(&url, json!({ "minutes": 15 })).await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.text());
    assert!(started.json()["doing"].as_str().unwrap_or_default().contains("every 15 min"), "{}", started.text());
    let asked = web
        .sent()
        .into_iter()
        .any(|c| matches!(c, NetCommand::SendMessage { message, .. } if message.commands == [Command::TelemetryRequest]));
    assert!(asked);
    let conversation = web.get(&format!("/api/conversations/{BOB}")).await.json();
    assert_eq!(conversation["track"]["every"], 15);
    assert_eq!(web.post(&format!("{url}/stop"), json!({})).await.status, StatusCode::OK);
    assert!(web.get(&format!("/api/conversations/{BOB}")).await.json()["track"].is_null());
    assert_eq!(web.post(&format!("{url}/stop"), json!({})).await.status, StatusCode::BAD_REQUEST);
}

/// A telemetry collector set from the web UI, and the locations it passes
/// on shown on the map, marked with where they came from.
#[tokio::test]
async fn a_telemetry_collector_on_the_map() {
    use crate::lxmf::Location;
    use crate::lxmf::fields::{Relayed, Telemetry};
    let web = TestWeb::start("collector");
    let values = json!({ "values": { "telemetry_collector": BOB, "collector_ask_mins": "15", "collector_serve": "trusted" } });
    let saved = web.post("/api/settings", values).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
    let wrong = web.post("/api/settings", json!({ "values": { "telemetry_collector": "nobody" } })).await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    web.with(|app| {
        let telemetry = Telemetry { location: Some(Location::at(51.5, -0.12)), ..Telemetry::default() };
        let stream = vec![Relayed { source: [0xcd; 16], time: 1_800_000_000, telemetry, appearance: None }];
        app.on_message(InboundMessage { extras: Extras { stream, ..Extras::default() }, ..from_bob(1, "") });
    })
    .await;
    let places = web.get("/api/locations").await.json();
    let text = places.to_string();
    assert!(text.contains(&"cd".repeat(16)) && text.contains("\"via\":\"<abababababab>\""), "{text}");
}

/// A message being sent says how it goes, and can be cancelled; a sync
/// says how many came.
#[tokio::test]
async fn send_progress_and_cancelling_a_send() {
    let web = TestWeb::start("cancel");
    let sent = web.post(&format!("/api/conversations/{BOB}/send"), json!({ "content": "a long one", "mode": "auto" })).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    let id = web
        .sent()
        .into_iter()
        .find_map(|c| match c {
            NetCommand::SendMessage { id, .. } => Some(id),
            _ => None,
        })
        .unwrap();
    web.with_owner(move |o| o.apply(NetEvent::SendProgress { id, text: "42%".into() })).await;
    let state = |conversation: serde_json::Value| conversation["messages"].as_array().unwrap().last().unwrap()["state"].clone();
    let shown = state(web.get(&format!("/api/conversations/{BOB}")).await.json());
    assert_eq!((shown["kind"].as_str(), shown["progress"].as_str()), (Some("sending"), Some("42%")));
    let url = format!("/api/conversations/{BOB}/cancel");
    assert_eq!(web.post(&url, json!({ "id": format!("local-{id}") })).await.status, StatusCode::OK);
    assert!(web.sent().iter().any(|c| matches!(c, NetCommand::CancelSend(sent) if *sent == id)));
    web.with_owner(move |o| o.apply(NetEvent::Delivery { id, result: Err(crate::lxmf::CANCELLED.into()) })).await;
    assert_eq!(state(web.get(&format!("/api/conversations/{BOB}")).await.json())["kind"], "cancelled");
    let again = web.post(&url, json!({ "id": format!("local-{id}") })).await;
    assert_eq!(again.status, StatusCode::BAD_REQUEST);
    // A sync, as it goes.
    web.with_owner(|o| {
        o.apply(NetEvent::SyncStarted);
        o.apply(NetEvent::SyncProgress("1 of 2 downloaded".into()));
    })
    .await;
    assert_eq!(web.get("/api/state").await.json()["sync"]["progress"], "1 of 2 downloaded");
}

#[tokio::test]
async fn who_may_see_something_on_the_node() {
    let web = TestWeb::start("node-allowed");
    let them = rns_identity::identity::Identity::new();
    let address = rns_identity::destination::Destination::hash_from_name_and_identity(crate::lxmf::LXMF_ASPECT, Some(&them.hash));
    let key = hex::encode(address);
    let known = crate::net::remote::KnownIdentities::none();
    known.lock().unwrap().remember(address, &them.get_public_key(), None);
    let contact = key.clone();
    web.with(move |app| {
        app.store.contacts.entry(contact).or_default().alias = Some("Alice".into());
        app.on_net(crate::net::NetEvent::Started { lxmf_hash: [9; 16], public_key: [1; 64], known });
    })
    .await;
    let path = web.post("/api/node/pages", json!({ "name": "club" })).await.json()["path"].as_str().unwrap().to_string();
    let q = format!("/api/node/allowed?path={path}");
    let now = web.get(&q).await.json();
    assert_eq!(now["who"], "everyone");
    assert_eq!((now["people"][0]["name"].as_str(), now["people"][0]["on"].as_bool()), (Some("Alice"), Some(false)), "{now}");
    let identity = now["people"][0]["identity"].as_str().unwrap().to_string();
    assert_eq!(identity, hex::encode(them.hash));

    // Only Alice; then no one; then anyone again.
    let set = web.post("/api/node/allowed", json!({ "path": path, "identities": [identity] })).await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.text());
    let now = web.get(&q).await.json();
    assert_eq!((now["who"].as_str(), now["people"][0]["on"].as_bool()), (Some("only"), Some(true)));
    assert_eq!(web.post("/api/node/allowed", json!({ "path": path })).await.status, StatusCode::OK);
    assert_eq!(web.get(&q).await.json()["who"], "only");
    assert_eq!(web.post("/api/node/allowed", json!({ "path": path, "everyone": true })).await.status, StatusCode::OK);
    assert_eq!(web.get(&q).await.json()["who"], "everyone");

    // Not identities, too many, not there, nowhere.
    assert!(web.post("/api/node/allowed", json!({ "path": path, "identities": ["nope"] })).await.error().contains("identity"));
    let many = vec![identity.clone(); crate::app::allowed::MAX_LISTED + 1];
    assert!(web.post("/api/node/allowed", json!({ "path": path, "identities": many })).await.error().contains("at most"));
    assert_eq!(web.get("/api/node/allowed?path=gone.mu").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.get(&format!("{q}&root=attic")).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(web.post("/api/node/allowed", json!({ "path": "../x", "everyone": true })).await.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_nodes_statistics_and_resetting_them() {
    let web = TestWeb::start("node-stats");
    web.with(|app| {
        app.node.served.pages = 5;
        app.node.served.requests = 6;
    })
    .await;
    let node = web.get("/api/node").await.json();
    assert_eq!((node["served"]["pages"].as_u64(), node["served"]["label"].as_str()), (Some(5), Some("5 pages · 0 files")), "{node}");
    assert_eq!(web.post("/api/node/stats/reset", json!({})).await.status, StatusCode::OK);
    let node = web.get("/api/node").await.json();
    assert_eq!((node["served"]["pages"].as_u64(), node["served"]["requests"].as_u64()), (Some(0), Some(0)));
    assert!(node["served"]["since"].as_f64().unwrap() > 1.7e9);
    // Status says what it served only while it's running.
    assert!(web.get("/api/state").await.json()["node_served"].is_null());
}

#[tokio::test]
async fn sending_a_file_with_rncp() {
    let web = TestWeb::start("rncp");
    let state = web.get("/api/rncp").await.json();
    assert_eq!(state["sends"].as_array().map(Vec::len), Some(0));
    assert_eq!(state["identity"].as_str().map(str::len), Some(32));
    let to = "cd".repeat(16);
    let sent = web.post_bytes(&format!("/api/rncp/send?to={to}&name=notes.txt"), "application/octet-stream", b"hello".to_vec()).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert!(web.sent().iter().any(|c| matches!(c, NetCommand::RncpSend { name, .. } if name == "notes.txt")));
    let state = web.get("/api/rncp").await.json();
    assert_eq!((state["sends"][0]["state"].as_str(), state["sends"][0]["bytes"].as_u64()), (Some("sending"), Some(5)));
    // A folder in the name is left out; a wrong address, no name, too big.
    let folder =
        web.post_bytes(&format!("/api/rncp/send?to={to}&name=..%2F..%2Fother.txt"), "application/octet-stream", b"x".to_vec()).await;
    assert_eq!(folder.status, StatusCode::OK);
    assert_eq!(web.get("/api/rncp").await.json()["sends"][1]["name"], "other.txt");
    let wrong = web.post_bytes("/api/rncp/send?to=nope&name=a.txt", "application/octet-stream", b"x".to_vec()).await;
    assert!(wrong.error().contains("32 hex"));
    let unnamed = web.post_bytes(&format!("/api/rncp/send?to={to}&name=%20"), "application/octet-stream", b"x".to_vec()).await;
    assert!(unnamed.error().contains("needs a name"));
    let big = vec![0u8; crate::app::rncp::MAX_SEND + 1];
    let big = web.post_bytes(&format!("/api/rncp/send?to={to}&name=big.bin"), "application/octet-stream", big).await;
    assert!(big.error().contains("at most"));
}

#[tokio::test]
async fn whose_an_address_is_over_the_api() {
    let web = TestWeb::start("whois");
    let rows = web.get("/api/identity").await.json();
    let mine = web.with(|app| hex::encode(app.identity_hash)).await;
    assert_eq!(rows["rows"][0]["heading"], true);
    assert_eq!(rows["rows"][1]["address"], mine.as_str());
    assert!(web.post("/api/identity/lookup", json!({ "address": "nope" })).await.error().contains("32 hex"));
    let theirs = "ab".repeat(16);
    assert_eq!(web.post("/api/identity/lookup", json!({ "address": theirs })).await.status, StatusCode::OK);
    assert!(web.sent().iter().any(|c| matches!(c, NetCommand::Identify(_))));
    let rows = web.get("/api/identity").await.json();
    assert!(rows["rows"].as_array().unwrap().iter().any(|r| r["label"] == "asking the network…"), "{rows}");
}

#[tokio::test]
async fn a_speed_test_over_the_api() {
    let web = TestWeb::start("speed");
    let test = web.get("/api/speedtest").await.json();
    assert_eq!((test["address"].as_str().map(str::len), test["size"].as_u64()), (Some(32), Some(5)));
    assert!(test["peer"].is_null() && test["sizes"][0] == "2 KB");
    assert!(web.post("/api/speedtest/send", json!({})).await.error().contains("their speed test address"));
    let theirs = "cd".repeat(16);
    assert_eq!(web.post("/api/speedtest", json!({ "peer": theirs, "size": 0 })).await.status, StatusCode::OK);
    // Shown in a browser, it listens for theirs.
    web.get("/api/speedtest").await;
    assert!(web.sent().iter().any(|c| matches!(c, NetCommand::SpeedListen(Some(_)))));
    assert_eq!(web.post("/api/speedtest/send", json!({})).await.status, StatusCode::OK);
    assert!(web.sent().iter().any(|c| matches!(c, NetCommand::SpeedSend { bytes: 2048, .. })));
    assert_eq!(web.get("/api/speedtest").await.json()["sending"], 0.0);
    assert!(web.post("/api/speedtest", json!({ "peer": "nope" })).await.error().contains("32 hex"));
    assert!(web.post("/api/speedtest", json!({ "size": 99 })).await.error().contains("sizes"));
}
