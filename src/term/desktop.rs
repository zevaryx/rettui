//! The desktop's own notifications, for the terminal UI: D-Bus on Linux and
//! the BSDs, Notification Center on macOS, toasts on Windows. They're sent
//! off the UI's path, since talking to the desktop can take a moment (or
//! time out where there's no desktop, as over SSH).
//!
//! On D-Bus, clicking one opens what it's about. The click comes back on
//! the connection that showed it (some notification servers send it to
//! that one only), so rettui shows them on a connection of its own that
//! also listens. Elsewhere a click isn't reported back.
//!
//! What's done with a notification comes back as a [`Response`] (opened,
//! replied to, marked read, a challenge answered), which the app acts on
//! with [`crate::app::App::respond`]; one read elsewhere is taken down
//! with [`Desktop::withdraw`].

use std::path::Path;
use std::sync::mpsc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::app::notify::{Notification, Response};

/// What the notifications' backend is asked to do.
pub enum Job {
    Show(Notification),
    /// Take down the one with this tag ([`crate::app::notify::Target::tag`]), if it's up.
    #[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
    Withdraw(String),
}

/// The logo, for notifications that show an icon (written to the cache
/// folder, since the desktop takes a file).
const ICON: &[u8] = include_bytes!("../web/assets/brand/icon.png");

pub struct Desktop {
    queue: Option<UnboundedSender<Job>>,
    /// Why showing one failed (once, until one works again).
    failures: mpsc::Receiver<String>,
    /// What was done with the notifications.
    responses: mpsc::Receiver<Response>,
}

impl Desktop {
    /// Start showing notifications (on the running Tokio runtime, on D-Bus).
    pub fn start(cache: &Path) -> Self {
        let icon = cache.join("icon.png");
        if std::fs::read(&icon).map_or(true, |on_disk| on_disk != ICON) {
            let _ = std::fs::create_dir_all(cache).and_then(|()| std::fs::write(&icon, ICON));
        }
        let (queue, pending) = unbounded_channel::<Job>();
        let (failed, failures) = mpsc::channel();
        let (answered, responses) = mpsc::channel();
        let started = run(pending, icon.to_string_lossy().into_owned(), failed, answered);
        // Without it there are no notifications (they're a nicety).
        Self { queue: started.then_some(queue), failures, responses }
    }

    pub fn show(&self, notification: Notification) {
        if let Some(queue) = &self.queue {
            let _ = queue.send(Job::Show(notification));
        }
    }

    /// Take down the notification with this tag ([`Target::tag`]): what it
    /// was about was read somewhere else.
    pub fn withdraw(&self, tag: String) {
        if let Some(queue) = &self.queue {
            let _ = queue.send(Job::Withdraw(tag));
        }
    }

    /// Why notifications can't be shown, if they just stopped working.
    pub fn failure(&self) -> Option<String> {
        self.failures.try_recv().ok()
    }

    /// What was done with a notification (it was opened, replied to…), if
    /// anything was, since last asked.
    pub fn response(&self) -> Option<Response> {
        self.responses.try_recv().ok()
    }
}

/// Text as it's shown: one failure said once, until one works again.
#[cfg(not(target_os = "android"))]
struct Failing {
    failed: mpsc::Sender<String>,
    failing: bool,
}

#[cfg(not(target_os = "android"))]
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

#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
fn run(pending: UnboundedReceiver<Job>, icon: String, failed: mpsc::Sender<String>, clicked: mpsc::Sender<Response>) -> bool {
    tokio::spawn(xdg::run(pending, icon, Failing { failed, failing: false }, clicked));
    true
}

#[cfg(any(windows, target_os = "macos"))]
fn run(mut pending: UnboundedReceiver<Job>, _icon: String, failed: mpsc::Sender<String>, clicked: mpsc::Sender<Response>) -> bool {
    let mut failing = Failing { failed, failing: false };
    std::thread::Builder::new()
        .name("rettui-notify".into())
        .spawn(move || {
            #[cfg(target_os = "macos")]
            macos::send_as_terminal();
            while let Some(job) = pending.blocking_recv() {
                match job {
                    Job::Show(notification) => failing.report(native::show(&notification, &clicked)),
                    Job::Withdraw(tag) => native::withdraw(&tag),
                }
            }
        })
        .is_ok()
}

/// Android: none from here. The app rettui runs in posts its own, from the
/// web UI's live events.
#[cfg(target_os = "android")]
fn run(_pending: UnboundedReceiver<Job>, _icon: String, _failed: mpsc::Sender<String>, _clicked: mpsc::Sender<Response>) -> bool {
    false
}

/// Windows: toasts (see [`crate::toast`]), as Windows PowerShell's (a
/// program that isn't installed can't have its own), with the
/// instant-message sound. What's done with one while rettui runs comes back
/// as a [`Response`]: a click opens what it's about, and brings the console
/// window forward where Windows lets it; a reply written in it, Mark read,
/// Accept and Decline are done as the app does them.
#[cfg(windows)]
mod native {
    use std::sync::mpsc;

    use crate::app::notify::{NoticeAction, Notification, Response};
    use crate::toast::{Activation, Button, Toast};

    pub fn show(notification: &Notification, answered: &mpsc::Sender<Response>) -> Result<(), String> {
        let buttons = notification
            .actions
            .iter()
            .map(|action| match action {
                NoticeAction::Reply => Button::Reply,
                NoticeAction::MarkRead => Button::MarkRead,
                NoticeAction::Accept => Button::Accept,
                NoticeAction::Decline => Button::Decline,
            })
            .collect();
        let toast = Toast { title: notification.title.clone(), body: notification.body.clone(), tag: notification.target.tag(), buttons };
        let (answered, target) = (answered.clone(), notification.target.clone());
        crate::toast::show(&toast, move |activation| {
            let target = target.clone();
            let response = match activation {
                Activation::Open => {
                    bring_forward();
                    Response::Open(target)
                }
                Activation::Reply(text) => Response::Reply { target, text },
                Activation::MarkRead => Response::MarkRead(target),
                Activation::Accept => Response::Accept(target),
                Activation::Decline => Response::Decline(target),
            };
            let _ = answered.send(response);
        })
    }

    /// Read elsewhere: taken down.
    pub fn withdraw(tag: &str) {
        crate::toast::withdraw(tag);
    }

    /// The console window rettui runs in, restored and in front (Windows
    /// may refuse; a terminal like Windows Terminal may not pass it on).
    fn bring_forward() {
        use windows_sys::Win32::System::Console::GetConsoleWindow;
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow};
        // SAFETY: plain Win32 calls on the console's own window handle,
        // checked for null first.
        unsafe {
            let window = GetConsoleWindow();
            if window.is_null() {
                return;
            }
            if IsIconic(window) != 0 {
                ShowWindow(window, SW_RESTORE);
            }
            SetForegroundWindow(window);
        }
    }
}

/// macOS: notifications are sent as the terminal rettui runs in (with its
/// icon), so clicking one brings that terminal forward; left alone they'd
/// be Finder's, and a click would open a Finder window. A click can't be
/// told to rettui: that needs a Cocoa run loop, which a terminal program
/// doesn't run.
#[cfg(target_os = "macos")]
mod native {
    use std::sync::mpsc;

    use crate::app::notify::{Notification, Response};

    /// Not taken down: nothing keeps a handle to it.
    pub fn withdraw(_tag: &str) {}

    pub fn show(notification: &Notification, _clicked: &mpsc::Sender<Response>) -> Result<(), String> {
        // Sent as the handle goes (asynchronously).
        notify_rust::Notification::new()
            .appname("rettui")
            .summary(&notification.title)
            .body(&notification.body)
            .show()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    /// Send notifications as the terminal: the app that started it, else
    /// one known by `TERM_PROGRAM`, else Terminal.
    pub fn send_as_terminal() {
        let bundle = std::env::var("__CFBundleIdentifier")
            .ok()
            .filter(|bundle| !bundle.is_empty())
            .or_else(|| std::env::var("TERM_PROGRAM").ok().and_then(|program| super::terminal_bundle(&program)).map(String::from))
            .unwrap_or_else(|| "com.apple.Terminal".to_string());
        // Unknown here: Terminal's (the library's own fallback).
        let _ = notify_rust::set_application(&bundle);
    }
}

/// The bundle of a macOS terminal, by its `TERM_PROGRAM`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn terminal_bundle(program: &str) -> Option<&'static str> {
    Some(match program {
        "Apple_Terminal" => "com.apple.Terminal",
        "iTerm.app" => "com.googlecode.iterm2",
        "WezTerm" => "com.github.wez.wezterm",
        "ghostty" => "com.mitchellh.ghostty",
        "vscode" => "com.microsoft.VSCode",
        _ => return None,
    })
}

/// The freedesktop notification service, spoken to directly: notify-rust
/// shows each on a connection of its own, which a click may not reach
/// rettui on once a newer notification has replaced it.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
mod xdg {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, mpsc};

    use futures_util::StreamExt;
    use tokio::sync::mpsc::UnboundedReceiver;
    use zbus::Variant;

    use super::{Failing, Job};
    use crate::app::notify::{NoticeAction, Notification, Response, Target};

    pub const SERVICE: &str = "org.freedesktop.Notifications";
    pub const PATH: &str = "/org/freedesktop/Notifications";

    /// The notifications on screen, by id: what each is about.
    type OnScreen = Arc<Mutex<HashMap<u32, Target>>>;

    /// What the notification server can do.
    #[derive(Clone, Copy, Default)]
    pub struct Server {
        actions: bool,
        inline_reply: bool,
    }

    /// Asked of the server once connected: buttons, and a reply field in
    /// the notification (KDE Plasma's).
    async fn capabilities(bus: &zbus::Connection) -> Server {
        let caps: Vec<String> = match bus.call_method(Some(SERVICE), PATH, Some(SERVICE), "GetCapabilities", &()).await {
            Ok(reply) => reply.body().deserialize().unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        Server { actions: caps.iter().any(|c| c == "actions"), inline_reply: caps.iter().any(|c| c == "inline-reply") }
    }

    pub async fn run(mut pending: UnboundedReceiver<Job>, icon: String, mut failing: Failing, clicked: mpsc::Sender<Response>) {
        let on_screen = OnScreen::default();
        let mut connection: Option<zbus::Connection> = None;
        let mut server = Server::default();
        // By conversation or room, the one shown: a newer one replaces it.
        let mut shown: HashMap<String, u32> = HashMap::new();
        while let Some(job) = pending.recv().await {
            let notification = match job {
                Job::Show(notification) => notification,
                // Read elsewhere: taken down, if it's still up.
                Job::Withdraw(tag) => {
                    if let (Some(bus), Some(id)) = (&connection, shown.remove(&tag)) {
                        let _ = bus.call_method(Some(SERVICE), PATH, Some(SERVICE), "CloseNotification", &(id,)).await;
                    }
                    continue;
                }
            };
            // Connected when first needed, and again if the bus went away.
            if connection.is_none() {
                let connected = match zbus::Connection::session().await {
                    // Listening before the first is shown, so no click is missed.
                    Ok(bus) => listen(&bus).await.map(|signals| {
                        tokio::spawn(clicks(signals, on_screen.clone(), clicked.clone()));
                        bus
                    }),
                    Err(e) => Err(e.to_string()),
                };
                match connected {
                    Ok(bus) => {
                        server = capabilities(&bus).await;
                        connection = Some(bus);
                    }
                    // No desktop bus under WSL: Windows shows it.
                    Err(_) if super::wsl::detected() => {
                        failing.report(super::wsl::show("powershell.exe", &notification).await);
                        continue;
                    }
                    Err(e) => {
                        failing.report(Err(e));
                        continue;
                    }
                }
            }
            let Some(bus) = &connection else { continue };
            let tag = notification.target.tag();
            // Not one that has gone from the screen: the server would update it
            // in its history without showing anything.
            let replaces = shown.get(&tag).copied().filter(|id| on_screen.lock().is_ok_and(|up| up.contains_key(id)));
            match show(bus, server, &notification, &icon, replaces).await {
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
    pub async fn show(
        bus: &zbus::Connection,
        server: Server,
        notification: &Notification,
        icon: &str,
        replaces: Option<u32>,
    ) -> Result<u32, String> {
        // Bodies may be marked up: text stays text.
        let body = notification.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let actions = action_list(server, notification);
        let mut hints: HashMap<&str, Variant> = HashMap::new();
        hints.insert("category", Variant::from("im.received"));
        hints.insert("x-kde-reply-placeholder-text", Variant::from("Reply…"));
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

    /// The buttons it gets, as (key, label) flattened: what the server can
    /// show. "default" is the click on the notification itself. Without a
    /// reply field in it, Reply opens the chat to write one.
    fn action_list(server: Server, notification: &Notification) -> Vec<&'static str> {
        let mut actions = vec!["default", "Open"];
        if !server.actions {
            return actions;
        }
        for action in &notification.actions {
            match action {
                NoticeAction::Reply if server.inline_reply => actions.extend(["inline-reply", "Reply"]),
                NoticeAction::Reply => actions.extend(["reply", "Reply"]),
                NoticeAction::MarkRead => actions.extend(["read", "Mark read"]),
                NoticeAction::Accept => actions.extend(["accept", "Accept"]),
                NoticeAction::Decline => actions.extend(["decline", "Decline"]),
            }
        }
        actions
    }

    /// The notification service's signals, on this connection.
    async fn listen(bus: &zbus::Connection) -> Result<zbus::MessageStream, String> {
        let rule =
            zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(SERVICE).build().map_err(|e| e.to_string())?;
        zbus::MessageStream::for_match_rule(rule, bus, None).await.map_err(|e| e.to_string())
    }

    /// Clicks on rettui's notifications, as what they're about; and which
    /// are gone from the screen.
    async fn clicks(mut signals: zbus::MessageStream, on_screen: OnScreen, clicked: mpsc::Sender<Response>) {
        while let Some(Ok(signal)) = signals.next().await {
            let header = signal.header();
            match header.member().map(|member| member.as_str()) {
                Some("ActionInvoked") => {
                    let Ok((id, action)) = signal.body().deserialize::<(u32, String)>() else { continue };
                    let target = on_screen.lock().ok().and_then(|on_screen| on_screen.get(&id).cloned());
                    let response = target.and_then(|target| match action.as_str() {
                        "default" | "reply" => Some(Response::Open(target)),
                        "read" => Some(Response::MarkRead(target)),
                        "accept" => Some(Response::Accept(target)),
                        "decline" => Some(Response::Decline(target)),
                        _ => None,
                    });
                    if let Some(response) = response
                        && clicked.send(response).is_err()
                    {
                        return;
                    }
                }
                Some("NotificationReplied") => {
                    let Ok((id, text)) = signal.body().deserialize::<(u32, String)>() else { continue };
                    let target = on_screen.lock().ok().and_then(|on_screen| on_screen.get(&id).cloned());
                    if let Some(target) = target.filter(|_| !text.trim().is_empty())
                        && clicked.send(Response::Reply { target, text }).is_err()
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

/// Linux under WSL, with no notification service of its own: Windows
/// shows the notification instead, as a toast made by Windows PowerShell
/// (which Windows runs for WSL). What it says is handed over in the
/// environment, never as part of the script, so no message can add to it.
/// A click can't come back.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
mod wsl {
    use std::process::Stdio;

    use base64::Engine;

    use crate::app::notify::Notification;

    pub fn detected() -> bool {
        std::env::var_os("WSL_DISTRO_NAME").is_some() || std::path::Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists()
    }

    /// Windows PowerShell 5's toast, from the title and body it's given.
    const SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
try {
    $null = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
    $null = [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]
    $title = [System.Security.SecurityElement]::Escape([string]$env:RETTUI_TOAST_TITLE)
    $body = [System.Security.SecurityElement]::Escape([string]$env:RETTUI_TOAST_BODY)
    $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
    $xml.LoadXml('<toast><visual><binding template="ToastGeneric"><text>' + $title + '</text><text>' + $body + '</text></binding></visual><audio src="ms-winsoundevent:Notification.IM"/></toast>')
    $app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
    $toast = New-Object Windows.UI.Notifications.ToastNotification $xml
    [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show($toast)
} catch {
    # Plainly, for rettui's log.
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
"#;

    /// PowerShell's `-EncodedCommand`: the script in UTF-16, as base64.
    pub fn encoded_script() -> String {
        let bytes: Vec<u8> = SCRIPT.encode_utf16().flat_map(u16::to_le_bytes).collect();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// Show it with `powershell` (`powershell.exe`, but for tests).
    pub async fn show(powershell: &str, notification: &Notification) -> Result<(), String> {
        // Passed from WSL to Windows programs only (`/u`).
        let handed = "RETTUI_TOAST_TITLE/u:RETTUI_TOAST_BODY/u";
        let wslenv = match std::env::var("WSLENV") {
            Ok(set) if !set.is_empty() => format!("{set}:{handed}"),
            _ => handed.to_string(),
        };
        let output = tokio::process::Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded_script()])
            .env("RETTUI_TOAST_TITLE", &notification.title)
            .env("RETTUI_TOAST_BODY", &notification.body)
            .env("WSLENV", wslenv)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| format!("Windows notifications through {powershell}: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!("Windows notifications: {}", String::from_utf8_lossy(&output.stderr).trim()))
        }
    }
}

#[cfg(test)]
mod platform_tests {
    #[test]
    fn macos_terminals_by_term_program() {
        assert_eq!(super::terminal_bundle("Apple_Terminal"), Some("com.apple.Terminal"));
        assert_eq!(super::terminal_bundle("iTerm.app"), Some("com.googlecode.iterm2"));
        assert_eq!(super::terminal_bundle("tmux"), None);
    }

    /// Under WSL: what's said goes in the environment, not the script.
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    #[tokio::test]
    async fn wsl_hands_windows_the_text_apart_from_the_script() {
        use base64::Engine;
        let dir = std::env::temp_dir().join(format!("rettui-wsl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Stands in for powershell.exe: writes what it was given.
        let fake = dir.join("powershell.exe");
        let seen = dir.join("seen");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n%s\\n%s\\n%s\\n' \"$4\" \"$RETTUI_TOAST_TITLE\" \"$RETTUI_TOAST_BODY\" \"$WSLENV\" > '{}'\n",
                seen.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let note =
            crate::app::notify::Notification::new("Alice's \"$(rm -rf /)\"", "<b>hi</b> & 'bye'", crate::app::notify::Target::Summary);
        super::wsl::show(fake.to_str().unwrap(), &note).await.unwrap();
        let seen = std::fs::read_to_string(&seen).unwrap();
        let lines: Vec<&str> = seen.lines().collect();
        assert_eq!(lines[0], super::wsl::encoded_script());
        assert_eq!((lines[1], lines[2]), (note.title.as_str(), note.body.as_str()));
        assert!(lines[3].ends_with("RETTUI_TOAST_TITLE/u:RETTUI_TOAST_BODY/u"), "{}", lines[3]);
        // The script is fixed: UTF-16 PowerShell, reading only the environment.
        let decoded = base64::engine::general_purpose::STANDARD.decode(lines[0]).unwrap();
        let units: Vec<u16> = decoded.chunks(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        let script = String::from_utf16(&units).unwrap();
        assert!(script.contains("$env:RETTUI_TOAST_TITLE") && !script.contains("Alice"));
        // As Windows PowerShell's toasts (its app id, exactly).
        assert!(script.contains(r"'{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'"), "{script}");
        assert!(!script.chars().any(|c| c.is_control() && c != '\n'), "no stray control characters");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::xdg::{PATH, SERVICE};
    use super::*;
    use crate::app::notify::Target;

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
            _hints: std::collections::HashMap<String, zbus::OwnedVariant>,
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
        let service =
            zbus::connection::Builder::session().name(SERVICE).serve_at(PATH, Service { senders: senders.clone() }).build().await.unwrap();
        let cache = std::env::temp_dir().join(format!("rettui-desktop-{}", std::process::id()));
        let desktop = Desktop::start(&cache);
        let about = |key: &str| Target::Conversation { key: key.into() };
        let wait = |desktop: &Desktop| {
            (0..40).find_map(|_| {
                std::thread::sleep(Duration::from_millis(50));
                desktop.response()
            })
        };
        desktop.show(Notification::new("Alice", "Hi <there>", about("aa")));
        tokio::time::sleep(Duration::from_millis(300)).await;
        // A newer one about the same conversation replaces it (same id).
        desktop.show(Notification::new("Alice", "Again", about("aa")));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let shown_by = senders.lock().unwrap().clone();
        assert_eq!(shown_by.len(), 2);
        assert_eq!(shown_by[0], shown_by[1], "one connection shows them all");
        // Clicked, as servers that tell only the one that showed it do.
        let to = shown_by[0].as_str();
        service.emit_signal(Some(to), PATH, SERVICE, "ActionInvoked", &(7u32, "default")).await.unwrap();
        assert_eq!(tokio::task::spawn_blocking(move || wait(&desktop)).await.unwrap(), Some(Response::Open(about("aa"))));
        let _ = std::fs::remove_dir_all(&cache);
    }
}
