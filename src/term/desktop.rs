//! The desktop's own notifications, for the terminal UI: D-Bus on Linux and
//! the BSDs, Notification Center on macOS, toasts on Windows. They're sent
//! from a thread of their own, since talking to the desktop can take a
//! moment (or time out where there's no desktop, as over SSH).

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;

use crate::app::notify::Notification;

/// The logo, for notifications that show an icon (written to the cache
/// folder, since the desktop takes a file).
const ICON: &[u8] = include_bytes!("../web/assets/brand/icon.png");

pub struct Desktop {
    queue: Option<mpsc::Sender<Notification>>,
    /// Why showing one failed (once, until one works again).
    failures: mpsc::Receiver<String>,
}

impl Desktop {
    pub fn start(cache: &Path) -> Self {
        let icon = cache.join("icon.png");
        if std::fs::read(&icon).map_or(true, |on_disk| on_disk != ICON) {
            let _ = std::fs::create_dir_all(cache).and_then(|()| std::fs::write(&icon, ICON));
        }
        let (queue, pending) = mpsc::channel::<Notification>();
        let (failed, failures) = mpsc::channel();
        let started = std::thread::Builder::new().name("rettui-notify".into()).spawn(move || {
            let mut shown = HashMap::new();
            let mut failing = false;
            for notification in pending {
                match show(&notification, &icon, &mut shown) {
                    Ok(()) => failing = false,
                    Err(e) if !failing => {
                        failing = true;
                        let _ = failed.send(e);
                    }
                    Err(_) => {}
                }
            }
        });
        // Without the thread there are no notifications (they're a nicety).
        Self { queue: started.is_ok().then_some(queue), failures }
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
}

fn show(notification: &Notification, icon: &Path, shown: &mut HashMap<String, u32>) -> Result<(), String> {
    let mut desktop = notify_rust::Notification::new();
    desktop.appname("rettui").summary(&notification.title).icon(&icon.to_string_lossy());
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Bodies may be marked up there: text stays text.
        let body = notification.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        desktop.body(&body);
        // A newer one about the same conversation or room replaces it.
        let tag = notification.target.tag();
        if let Some(id) = shown.get(&tag) {
            desktop.id(*id);
        }
        let handle = desktop.show().map_err(|e| e.to_string())?;
        shown.insert(tag, handle.id());
        Ok(())
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = shown;
        desktop.body(&notification.body);
        desktop.show().map(|_| ()).map_err(|e| e.to_string())
    }
}
