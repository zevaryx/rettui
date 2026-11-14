//! Linux: the freedesktop notification service over D-Bus, with what the
//! server supports. A click opens what it's about; **Mark read**, **Accept**
//! and **Decline** are buttons; and where the server has inline replies
//! (KDE Plasma's) a **Reply** field is in the notification itself. On a
//! server without them, Reply is left out, and a click opens the chat.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use zbus::Connection;
use zbus::Variant;

use super::{Action, Backend, Event, Request, Sink};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// Longest title and body sent (a server may refuse more).
const TITLE: usize = 200;
const BODY: usize = 1000;

enum Job {
    Show(Request),
    Close(String),
}

pub struct Notifications(UnboundedSender<Job>);

impl Notifications {
    pub fn start(sink: Sink) -> Self {
        let (queue, jobs) = unbounded_channel();
        tauri::async_runtime::spawn(run(jobs, sink));
        Self(queue)
    }
}

impl Backend for Notifications {
    fn show(&self, request: Request) {
        let _ = self.0.send(Job::Show(request));
    }

    fn close(&self, tag: &str) {
        let _ = self.0.send(Job::Close(tag.to_string()));
    }
}

/// What the server offers.
#[derive(Default, Clone, Copy)]
struct Server {
    actions: bool,
    inline_reply: bool,
}

/// The notifications up, by the server's id: what each was about.
type Up = Arc<Mutex<HashMap<u32, Request>>>;

async fn run(mut jobs: tokio::sync::mpsc::UnboundedReceiver<Job>, sink: Sink) {
    let up = Up::default();
    let mut bus: Option<(Connection, Server)> = None;
    let mut shown: HashMap<String, u32> = HashMap::new();
    while let Some(job) = jobs.recv().await {
        if bus.is_none() {
            bus = connect(&up, &sink).await;
        }
        let Some((connection, server)) = &bus else { continue };
        match job {
            Job::Show(request) => match show(connection, *server, &request, shown.get(&request.tag).copied()).await {
                Ok(id) => {
                    shown.insert(request.tag.clone(), id);
                    if let Ok(mut up) = up.lock() {
                        up.insert(id, request);
                    }
                }
                // The bus went away: connected again for the next.
                Err(_) => bus = None,
            },
            Job::Close(tag) => {
                if let Some(id) = shown.remove(&tag) {
                    let _ = connection.call_method(Some(SERVICE), PATH, Some(SERVICE), "CloseNotification", &(id,)).await;
                    if let Ok(mut up) = up.lock() {
                        up.remove(&id);
                    }
                }
            }
        }
    }
}

async fn connect(up: &Up, sink: &Sink) -> Option<(Connection, Server)> {
    let connection = Connection::session().await.ok()?;
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(SERVICE).build().ok()?;
    // Listening before the first is shown, so no click is missed.
    let signals = zbus::MessageStream::for_match_rule(rule, &connection, None).await.ok()?;
    let capabilities: Vec<String> = connection
        .call_method(Some(SERVICE), PATH, Some(SERVICE), "GetCapabilities", &())
        .await
        .ok()
        .and_then(|reply| reply.body().deserialize().ok())
        .unwrap_or_default();
    let server =
        Server { actions: capabilities.iter().any(|c| c == "actions"), inline_reply: capabilities.iter().any(|c| c == "inline-reply") };
    tokio::spawn(signals_of(signals, up.clone(), sink.clone()));
    Some((connection, server))
}

async fn show(bus: &Connection, server: Server, request: &Request, replaces: Option<u32>) -> Result<u32, String> {
    let mut actions: Vec<&str> = Vec::new();
    if server.actions {
        // "default" is the click on the notification itself.
        actions.extend(["default", "Open"]);
        for action in &request.actions {
            match action {
                Action::Read => actions.extend(["read", "Mark read"]),
                Action::Accept => actions.extend(["accept", "Accept"]),
                Action::Decline => actions.extend(["decline", "Decline"]),
                Action::Reply if server.inline_reply => actions.extend(["inline-reply", "Reply"]),
                Action::Reply | Action::Open => {}
            }
        }
    }
    let mut hints: HashMap<&str, Variant> = HashMap::new();
    hints.insert("desktop-entry", Variant::from("rettui-desktop"));
    hints.insert("category", Variant::from("im.received"));
    hints.insert("x-kde-reply-placeholder-text", Variant::from("Reply…"));
    // Bodies may be marked up: text stays text.
    let body: String = request.body.chars().take(BODY).collect::<String>().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let title: String = request.title.chars().take(TITLE).collect();
    let reply = bus
        .call_method(
            Some(SERVICE),
            PATH,
            Some(SERVICE),
            "Notify",
            &("rettui", replaces.unwrap_or(0), "rettui-desktop", title.as_str(), body.as_str(), actions, hints, -1i32),
        )
        .await
        .map_err(|e| e.to_string())?;
    reply.body().deserialize::<u32>().map_err(|e| e.to_string())
}

/// Clicks, buttons and replies, as what they were about.
async fn signals_of(mut signals: zbus::MessageStream, up: Up, sink: Sink) {
    while let Some(Ok(signal)) = signals.next().await {
        let header = signal.header();
        let (id, action, text) = match header.member().map(|member| member.as_str()) {
            Some("ActionInvoked") => {
                let Ok((id, key)) = signal.body().deserialize::<(u32, String)>() else { continue };
                let action = match key.as_str() {
                    "default" => Action::Open,
                    "read" => Action::Read,
                    "accept" => Action::Accept,
                    "decline" => Action::Decline,
                    _ => continue,
                };
                (id, action, None)
            }
            Some("NotificationReplied") => {
                let Ok((id, text)) = signal.body().deserialize::<(u32, String)>() else { continue };
                if text.trim().is_empty() {
                    continue;
                }
                (id, Action::Reply, Some(text))
            }
            Some("NotificationClosed") => {
                if let Ok((id, _)) = signal.body().deserialize::<(u32, u32)>()
                    && let Ok(mut up) = up.lock()
                {
                    up.remove(&id);
                }
                continue;
            }
            _ => continue,
        };
        let request = up.lock().ok().and_then(|up| up.get(&id).cloned());
        if let Some(request) = request {
            sink(Event { action, tag: request.tag, target: request.target, text });
        }
    }
}
