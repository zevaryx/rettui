//! The desktop's own notifications, for the terminal UI: D-Bus on Linux and
//! the BSDs, Notification Center on macOS, toasts on Windows. They're sent
//! off the UI's path, since talking to the desktop can take a moment (or
//! time out where there's no desktop, as over SSH).
//!
//! On D-Bus, clicking one opens what it's about. The click comes back on
//! the connection that showed it (some notification servers send it to
//! that one only), so rettui shows them on a connection of its own that
//! also listens. Elsewhere a click isn't reported back.

use std::path::Path;
use std::sync::mpsc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::app::notify::{Notification, Target};

/// The logo, for notifications that show an icon (written to the cache
/// folder, since the desktop takes a file).
const ICON: &[u8] = include_bytes!("../web/assets/brand/icon.png");

pub struct Desktop {
    queue: Option<UnboundedSender<Notification>>,
    /// Why showing one failed (once, until one works again).
    failures: mpsc::Receiver<String>,
    /// What clicked notifications were about.
    clicks: mpsc::Receiver<Target>,
}

impl Desktop {
    /// Start showing notifications (on the running Tokio runtime, on D-Bus).
    pub fn start(cache: &Path) -> Self {
        let icon = cache.join("icon.png");
        if std::fs::read(&icon).map_or(true, |on_disk| on_disk != ICON) {
            let _ = std::fs::create_dir_all(cache).and_then(|()| std::fs::write(&icon, ICON));
        }
        let (queue, pending) = unbounded_channel::<Notification>();
        let (failed, failures) = mpsc::channel();
        let (clicked, clicks) = mpsc::channel();
        let started = run(pending, icon.to_string_lossy().into_owned(), failed, clicked);
        // Without it there are no notifications (they're a nicety).
        Self { queue: started.then_some(queue), failures, clicks }
    }

    pub fn show(&self, notification: Notification) {
        if let Some(queue) = &self.queue {
            let _ = queue.send(notification);
        }
    }

    /// Why notifications can't be shown, if they just stopped working.
    pub fn failure(&self) -> Option<String> {
        self.failures.try_recv().ok()
    }

    /// What a notification that was clicked is about, if one was.
    pub fn clicked(&self) -> Option<Target> {
        self.clicks.try_recv().ok()
    }
}

/// Text as it's shown: one failure said once, until one works again.
struct Failing {
    failed: mpsc::Sender<String>,
    failing: bool,
}

impl Failing {
    fn report(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => self.failing = false,
            Err(e) if !self.failing => {
                self.failing = true;
                let _ = self.failed.send(e);
            }
            Err(_) => {}
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run(pending: UnboundedReceiver<Notification>, icon: String, failed: mpsc::Sender<String>, clicked: mpsc::Sender<Target>) -> bool {
    tokio::spawn(xdg::run(pending, icon, Failing { failed, failing: false }, clicked));
    true
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn run(mut pending: UnboundedReceiver<Notification>, icon: String, failed: mpsc::Sender<String>, _clicked: mpsc::Sender<Target>) -> bool {
    let mut failing = Failing { failed, failing: false };
    std::thread::Builder::new()
        .name("rettui-notify".into())
        .spawn(move || {
            while let Some(notification) = pending.blocking_recv() {
                let mut desktop = notify_rust::Notification::new();
                desktop.appname("rettui").summary(&notification.title).body(&notification.body).icon(&icon);
                failing.report(desktop.show().map(|_| ()).map_err(|e| e.to_string()));
            }
        })
        .is_ok()
}

/// The freedesktop notification service, spoken to directly: notify-rust
/// shows each on a connection of its own, which a click may not reach
/// rettui on once a newer notification has replaced it.
#[cfg(all(unix, not(target_os = "macos")))]
mod xdg {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, mpsc};

    use futures_util::StreamExt;
    use tokio::sync::mpsc::UnboundedReceiver;
    use zbus::zvariant::Value;

    use super::Failing;
    use crate::app::notify::{Notification, Target};

    pub const SERVICE: &str = "org.freedesktop.Notifications";
    pub const PATH: &str = "/org/freedesktop/Notifications";

    /// The notifications on screen, by id: what each is about.
    type OnScreen = Arc<Mutex<HashMap<u32, Target>>>;

    pub async fn run(mut pending: UnboundedReceiver<Notification>, icon: String, mut failing: Failing, clicked: mpsc::Sender<Target>) {
        let on_screen = OnScreen::default();
        let mut connection: Option<zbus::Connection> = None;
        // By conversation or room, the one shown: a newer one replaces it.
        let mut shown: HashMap<String, u32> = HashMap::new();
        while let Some(notification) = pending.recv().await {
            // Connected when first needed, and again if the bus went away.
            if connection.is_none() {
                match zbus::Connection::session().await {
                    Ok(bus) => {
                        // Listening before the first is shown, so no click is missed.
                        match listen(&bus).await {
                            Ok(signals) => {
                                tokio::spawn(clicks(signals, on_screen.clone(), clicked.clone()));
                                connection = Some(bus);
                            }
                            Err(e) => failing.report(Err(e)),
                        }
                    }
                    Err(e) => failing.report(Err(e.to_string())),
                }
            }
            let Some(bus) = &connection else { continue };
            let tag = notification.target.tag();
            match show(bus, &notification, &icon, shown.get(&tag).copied()).await {
                Ok(id) => {
                    shown.insert(tag, id);
                    if let Ok(mut on_screen) = on_screen.lock() {
                        on_screen.insert(id, notification.target);
                    }
                    failing.report(Ok(()));
                }
                Err(e) => {
                    connection = None;
                    failing.report(Err(e));
                }
            }
        }
    }

    /// Show one (in place of `replaces`, if that's still up): its id.
    pub async fn show(bus: &zbus::Connection, notification: &Notification, icon: &str, replaces: Option<u32>) -> Result<u32, String> {
        // Bodies may be marked up: text stays text.
        let body = notification.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        // "default" is the action of clicking the notification itself.
        let actions = vec!["default", "Open"];
        let hints: HashMap<&str, Value> = HashMap::new();
        let reply = bus
            .call_method(
                Some(SERVICE),
                PATH,
                Some(SERVICE),
                "Notify",
                &("rettui", replaces.unwrap_or(0), icon, notification.title.as_str(), body.as_str(), actions, hints, -1i32),
            )
            .await
            .map_err(|e| e.to_string())?;
        reply.body().deserialize::<u32>().map_err(|e| e.to_string())
    }

    /// The notification service's signals, on this connection.
    async fn listen(bus: &zbus::Connection) -> Result<zbus::MessageStream, String> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(SERVICE)
            .map_err(|e| e.to_string())?
            .build();
        zbus::MessageStream::for_match_rule(rule, bus, None).await.map_err(|e| e.to_string())
    }

    /// Clicks on rettui's notifications, as what they're about; and which
    /// are gone from the screen.
    async fn clicks(mut signals: zbus::MessageStream, on_screen: OnScreen, clicked: mpsc::Sender<Target>) {
        while let Some(Ok(signal)) = signals.next().await {
            let header = signal.header();
            match header.member().map(|member| member.as_str()) {
                Some("ActionInvoked") => {
                    let Ok((id, action)) = signal.body().deserialize::<(u32, String)>() else { continue };
                    let target = on_screen.lock().ok().and_then(|on_screen| on_screen.get(&id).cloned());
                    if let Some(target) = target.filter(|_| action == "default")
                        && clicked.send(target).is_err()
                    {
                        return;
                    }
                }
                Some("NotificationClosed") => {
                    if let Ok((id, _)) = signal.body().deserialize::<(u32, u32)>()
                        && let Ok(mut on_screen) = on_screen.lock()
                    {
                        on_screen.remove(&id);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::xdg::{PATH, SERVICE};
    use super::*;

    /// A notification service that remembers who showed each.
    struct Service {
        senders: Arc<Mutex<Vec<String>>>,
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Service {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &self,
            #[zbus(header)] header: zbus::message::Header<'_>,
            _app: &str,
            replaces: u32,
            _icon: &str,
            _summary: &str,
            _body: &str,
            actions: Vec<String>,
            _hints: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
            _timeout: i32,
        ) -> u32 {
            assert_eq!(actions, ["default", "Open"]);
            self.senders.lock().unwrap().push(header.sender().unwrap().to_string());
            if replaces == 0 { 7 } else { replaces }
        }
    }

    /// Needs a session bus: `dbus-run-session -- cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a D-Bus session bus"]
    async fn clicking_a_notification_opens_what_its_about() {
        let senders = Arc::new(Mutex::new(Vec::new()));
        let service = zbus::connection::Builder::session()
            .unwrap()
            .name(SERVICE)
            .unwrap()
            .serve_at(PATH, Service { senders: senders.clone() })
            .unwrap()
            .build()
            .await
            .unwrap();
        let cache = std::env::temp_dir().join(format!("rettui-desktop-{}", std::process::id()));
        let desktop = Desktop::start(&cache);
        let about = |key: &str| Target::Conversation { key: key.into() };
        let wait = |desktop: &Desktop| {
            (0..40).find_map(|_| {
                std::thread::sleep(Duration::from_millis(50));
                desktop.clicked()
            })
        };
        desktop.show(Notification { title: "Alice".into(), body: "Hi <there>".into(), target: about("aa") });
        tokio::time::sleep(Duration::from_millis(300)).await;
        // A newer one about the same conversation replaces it (same id).
        desktop.show(Notification { title: "Alice".into(), body: "Again".into(), target: about("aa") });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let shown_by = senders.lock().unwrap().clone();
        assert_eq!(shown_by.len(), 2);
        assert_eq!(shown_by[0], shown_by[1], "one connection shows them all");
        // Clicked, as servers that tell only the one that showed it do.
        let to = shown_by[0].as_str();
        service.emit_signal(Some(to), PATH, SERVICE, "ActionInvoked", &(7u32, "default")).await.unwrap();
        assert_eq!(tokio::task::spawn_blocking(move || wait(&desktop)).await.unwrap(), Some(about("aa")));
        let _ = std::fs::remove_dir_all(&cache);
    }
}
