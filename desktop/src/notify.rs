//! Notifications in the desktop app, which the webview can't show itself.
//!
//! The page decides what to notify (it has the events, the settings and
//! what's on screen) and asks for it through `notify_show`, naming the
//! buttons the notification should have. A [`Backend`] shows it with the
//! system's own notifications, and tells back what was done with it
//! through the [`Sink`], as an [`Event`] the page acts on with the web UI's
//! API, as the Android app does: open what it's about, reply, mark it
//! read, accept or decline a challenge. Without a backend (a system that
//! has none yet) `notify_show` says so and the page uses the notification
//! plugin, which can show a notification and report a click.
//!
//! One backend per system, in `notify/<system>.rs`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, State};

#[cfg(target_os = "linux")]
mod linux;

/// What the user did with a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Clicked it: open what it's about (the window comes forward too).
    Open,
    /// Wrote a reply in it (`Event::text`).
    Reply,
    /// Marked what it's about read.
    Read,
    /// Accepted a game's challenge.
    Accept,
    /// Declined it.
    Decline,
}

/// A notification to show: the page's `notify` event, with the buttons it
/// should have besides the click itself (`Open` is always there).
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub title: String,
    pub body: String,
    /// What it's about, as the page's `openTarget` takes it; a newer
    /// notification with the same tag replaces an older one.
    pub tag: String,
    pub target: Value,
    #[serde(default)]
    pub actions: Vec<Action>,
}

/// What came of a notification, for the page.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub action: Action,
    pub tag: String,
    pub target: Value,
    /// The reply's text, with `Action::Reply`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Where a backend sends events. Safe to call from any thread.
pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// The system's notifications.
pub trait Backend: Send + Sync {
    /// Show it, or replace the one with its tag. Mustn't block: talking to
    /// the desktop can take a moment.
    fn show(&self, request: Request);
    /// Take down the notification with `tag`, if it's up (what it was about
    /// was read).
    fn close(&self, tag: &str);
}

/// The backend this system has, if any.
#[allow(unused_variables)]
fn backend(sink: Sink) -> Option<Box<dyn Backend>> {
    #[cfg(target_os = "linux")]
    return Some(Box::new(linux::Notifications::start(sink)));
    #[cfg(not(target_os = "linux"))]
    None
}

/// Managed state: the backend, once started.
pub struct Notifier(Option<Box<dyn Backend>>);

/// Start notifications. Events reach the page as a call of
/// `window.rettuiNotified(event)` in the main window, and an `Open`
/// brings the window forward first.
pub fn start(app: &AppHandle) -> Notifier {
    let handle = app.clone();
    Notifier(backend(Arc::new(move |event: Event| {
        if event.action == Action::Open {
            crate::show(&handle);
        }
        let Ok(json) = serde_json::to_string(&event) else { return };
        if let Some(window) = handle.get_webview_window("main") {
            let _ = window.eval(format!("window.rettuiNotified&&window.rettuiNotified({json})"));
        }
    })))
}

/// Show a notification. Whether the app did: not, and the page uses the
/// notification plugin.
#[tauri::command]
pub fn notify_show(notifier: State<Notifier>, request: Request) -> bool {
    match &notifier.0 {
        Some(backend) => {
            backend.show(request);
            true
        }
        None => false,
    }
}

/// Take a notification down.
#[tauri::command]
pub fn notify_close(notifier: State<Notifier>, tag: String) {
    if let Some(backend) = &notifier.0 {
        backend.close(&tag);
    }
}
