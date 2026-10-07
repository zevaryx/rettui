//! Checking for a newer rettui (see `crate::update`): asked in a thread of
//! its own, once a day while Check for updates is on, and the answer kept.
//! A newer release is said once a run, and shown by the version (top left
//! in the TUI, in the web UI's sidebar) and in Status.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::{App, now};
use crate::update::{self, Checked, Release};

pub struct Updates {
    /// What the last check found, as kept in `update-check.json`.
    pub checked: Checked,
    /// A check on its way: where its answer comes.
    asking: Option<Receiver<Result<Release, String>>>,
    /// When a check last couldn't be made (it's tried again an hour on).
    failed: Option<Instant>,
    /// The version the user was told of, this run.
    told: Option<String>,
    /// How it asks (tests don't go online).
    pub(crate) fetch: fn() -> Result<Release, String>,
}

impl Updates {
    pub fn load(path: &std::path::Path) -> Self {
        Self { checked: Checked::load(path), asking: None, failed: None, told: None, fetch: update::fetch }
    }
}

impl App {
    /// A newer release, if one was found and update checks are on.
    pub fn update_available(&self) -> Option<&Release> {
        self.settings.update_check.then(|| self.updates.checked.newer_than(update::VERSION)).flatten()
    }

    /// Take a check's answer, say once if there's a newer release, and ask
    /// again when a day has gone by (an hour, after one that couldn't be
    /// made).
    pub(super) fn updates_tick(&mut self) {
        if let Some(asking) = &self.updates.asking {
            match asking.try_recv() {
                Ok(Ok(release)) => {
                    self.updates.asking = None;
                    self.updates.failed = None;
                    self.updates.checked = Checked { at: now() as i64, latest: Some(release) };
                    if let Err(e) = self.updates.checked.save(&self.paths.update_check) {
                        self.log(format!("Couldn't keep what the update check found: {e}"));
                    }
                }
                Ok(Err(e)) => {
                    self.updates.asking = None;
                    self.updates.failed = Some(Instant::now());
                    // Offline is usual for a mesh client: not worth the log.
                    tracing::debug!("{e}");
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.updates.asking = None,
            }
        }
        if !self.settings.update_check {
            return;
        }
        if let Some(release) = self.update_available().cloned()
            && self.updates.told.as_deref() != Some(release.version.as_str())
        {
            self.updates.told = Some(release.version.clone());
            self.notify(format!(
                "rettui {} is out (this is {}): click the version, top left, for what's new",
                release.version,
                update::VERSION
            ));
        }
        let since = Duration::from_secs((now() as i64 - self.updates.checked.at).unsigned_abs());
        let retry = self.updates.failed.is_none_or(|at| at.elapsed() >= update::RETRY);
        if since >= update::EVERY && retry {
            let (tx, rx) = std::sync::mpsc::channel();
            let fetch = self.updates.fetch;
            std::thread::spawn(move || {
                let _ = tx.send(fetch());
            });
            self.updates.asking = Some(rx);
        }
    }

    /// Where the version opens: the newer release's page, if there is one,
    /// else rettui's.
    pub fn version_url(&self) -> String {
        self.update_available().map_or_else(|| crate::config::PROJECT_URL.to_string(), |r| r.url.clone())
    }
}

#[cfg(test)]
mod tests {
    use crate::update::{Checked, Release};

    fn release() -> Result<Release, String> {
        Ok(Release { version: "99.0.0".into(), url: "https://github.com/zevaryx/rettui/releases/tag/v99.0.0".into() })
    }

    /// Tick until the check's answer is in.
    fn settle(app: &mut crate::app::App) {
        for _ in 0..200 {
            app.on_tick();
            if app.updates.asking.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the check never answered");
    }

    #[test]
    fn a_newer_release_is_found_kept_and_said_once() {
        let dir = std::env::temp_dir().join(format!("rettui-updates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        app.updates.fetch = release;
        app.on_tick();
        settle(&mut app);
        app.on_tick();
        let found = app.update_available().cloned().expect("a newer release");
        assert_eq!(found.version, "99.0.0");
        assert_eq!(app.version_url(), found.url);
        assert!(app.log.iter().filter(|l| l.contains("rettui 99.0.0 is out")).count() == 1);
        // Kept: the next start knows without asking, and a day goes by
        // before it asks again.
        assert_eq!(Checked::load(&app.paths.update_check).latest, Some(found.clone()));
        app.on_tick();
        assert!(app.updates.asking.is_none());
        let mut again = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        again.updates.fetch = || panic!("asked again within a day");
        again.on_tick();
        assert_eq!(again.update_available(), Some(&found));
        // Turned off: nothing shown, nothing asked.
        let off = crate::config::Settings { update_check: false, ..crate::config::Settings::default() };
        let _ = std::fs::remove_dir_all(&dir);
        let mut quiet = crate::app::test_app(&dir, off, crate::store::Store::default());
        quiet.updates.fetch = || panic!("asked with update checks off");
        quiet.on_tick();
        assert!(quiet.update_available().is_none() && quiet.updates.asking.is_none());
        assert_eq!(quiet.version_url(), crate::config::PROJECT_URL);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_check_that_fails_is_tried_again_later_and_an_older_release_is_not_news() {
        let dir = std::env::temp_dir().join(format!("rettui-updates-failed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        app.updates.fetch = || Err("Couldn't check for updates: offline".into());
        app.on_tick();
        settle(&mut app);
        // Not again at once: an hour on.
        app.updates.fetch = || panic!("asked again at once");
        app.on_tick();
        assert!(app.updates.asking.is_none() && app.update_available().is_none());
        // This version, or an older one: nothing to say.
        app.updates.failed = None;
        app.updates.fetch = || Ok(Release { version: "0.1.0".into(), url: "https://github.com/zevaryx/rettui/releases/tag/v0.1.0".into() });
        app.on_tick();
        settle(&mut app);
        assert!(app.update_available().is_none());
        assert!(!app.log.iter().any(|l| l.contains("is out")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
