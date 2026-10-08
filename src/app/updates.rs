//! Checking for a newer rettui (see `crate::update`): asked in a thread of
//! its own, once a day while Check for updates is on (it's off unless
//! turned on), or when asked (`u` in Status, Check now in the web UI), and
//! the answer kept.
//! A newer release is said once a run, and shown by the version (top left
//! in the TUI, in the web UI's sidebar) and in Status.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::{App, PromptKind, now};
use crate::update::install::{self, Install};
use crate::update::{self, Checked, Release};

pub struct Updates {
    /// What the last check found, as kept in `update-check.json`.
    pub checked: Checked,
    /// A check on its way: where its answer comes.
    asking: Option<Receiver<Result<Release, String>>>,
    /// The check on its way was asked for: its answer is said, whatever it
    /// is.
    by_hand: bool,
    /// A check was asked for this run: what it found is shown, even with
    /// Check for updates off.
    asked: bool,
    /// Why the last check asked for couldn't be made.
    pub error: Option<String>,
    /// When a check last couldn't be made (it's tried again an hour on).
    failed: Option<Instant>,
    /// The version the user was told of, this run.
    told: Option<String>,
    /// How it asks (tests don't go online).
    pub(crate) fetch: fn() -> Result<Release, String>,
    /// How this copy is updated: in place, or how instead (looked at once).
    pub install: Install,
    /// The version being installed, and where the answer comes.
    installing: Option<(String, Receiver<Result<String, String>>)>,
    /// The version installed this run, waiting for rettui to start again.
    pub installed: Option<String>,
    /// How it installs one (tests don't go online either).
    pub(crate) installer: fn(&Release) -> Result<String, String>,
}

impl Updates {
    pub fn load(path: &std::path::Path) -> Self {
        Self {
            checked: Checked::load(path),
            asking: None,
            by_hand: false,
            asked: false,
            error: None,
            failed: None,
            told: None,
            fetch: update::fetch,
            install: Install::here(),
            installing: None,
            installed: None,
            installer: install::install_release,
        }
    }

    /// Installing a newer release, now.
    pub fn installing(&self) -> bool {
        self.installing.is_some()
    }

    /// Checking for a newer release, now.
    pub fn checking(&self) -> bool {
        self.asking.is_some()
    }

    /// What Status shows of checks and installs, to tell when it changed
    /// (they finish on their own, between other events).
    pub fn shown(&self) -> impl PartialEq + use<> {
        (
            self.checking(),
            self.checked.clone(),
            self.error.clone(),
            self.asked,
            self.installing.as_ref().map(|(version, _)| version.clone()),
            self.installed.clone(),
        )
    }

    /// Ask for the newest release, in a thread of its own.
    fn ask(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let fetch = self.fetch;
        std::thread::spawn(move || {
            let _ = tx.send(fetch());
        });
        self.asking = Some(rx);
    }
}

impl App {
    /// A newer release, if one was found and update checks are on (or one
    /// was asked for this run).
    pub fn update_available(&self) -> Option<&Release> {
        (self.settings.update_check || self.updates.asked).then(|| self.updates.checked.newer_than(update::VERSION)).flatten()
    }

    /// Check for a newer release now (`u` in Status, Check now in the web
    /// UI), whether or not Check for updates is on; what it finds is said.
    pub fn check_for_updates(&mut self) -> String {
        // One on its way already (the daily one) is said when it's in.
        if !self.updates.checking() {
            self.updates.ask();
        }
        self.updates.by_hand = true;
        self.updates.error = None;
        "Checking for updates…".into()
    }

    /// What's known of newer releases, beside the version in Status: a
    /// check on its way, why the last one asked for couldn't be made, or
    /// what the last one found, and when. With Check for updates off, and
    /// none asked for this run, nothing found earlier is shown.
    pub fn version_status(&self) -> String {
        if self.updates.checking() {
            return "checking for updates…".into();
        }
        if let Some(e) = &self.updates.error {
            return format!("couldn't check for updates: {e}");
        }
        if !self.settings.update_check && !self.updates.asked {
            return "Check for updates is off".into();
        }
        let checked = &self.updates.checked;
        if checked.at == 0 {
            return "not checked for updates yet".into();
        }
        let when = crate::clock::when(checked.at as f64);
        match self.update_available() {
            Some(release) => format!("rettui {} is out (checked {when})", release.version),
            None => format!("up to date (checked {when})"),
        }
    }

    /// What a check asked for found, said.
    fn say_checked(&mut self) {
        let latest = self.updates.checked.latest.clone();
        match self.update_available().cloned() {
            Some(release) => {
                self.updates.told = Some(release.version.clone());
                let how = match &self.updates.install {
                    Install::InPlace { .. } => "U in Status installs it".to_string(),
                    Install::Elsewhere(how) => how.clone(),
                };
                self.notify(format!("rettui {} is out (this is {}): {how}", release.version, update::VERSION));
            }
            None => match latest {
                Some(release) if release.version != update::VERSION => {
                    self.notify(format!("This rettui ({}) is newer than the latest release ({})", update::VERSION, release.version))
                }
                _ => self.notify(format!("rettui {} is the newest release", update::VERSION)),
            },
        }
    }

    /// Take a check's answer, say once if there's a newer release, and ask
    /// again when a day has gone by (an hour, after one that couldn't be
    /// made).
    pub(super) fn updates_tick(&mut self) {
        if let Some((version, answer)) = &self.updates.installing {
            match answer.try_recv() {
                Ok(Ok(done)) => {
                    self.updates.installed = Some(version.clone());
                    self.updates.installing = None;
                    self.notify(done);
                }
                Ok(Err(e)) => {
                    self.updates.installing = None;
                    self.fail(e);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.updates.installing = None,
            }
        }
        if let Some(asking) = &self.updates.asking {
            match asking.try_recv() {
                Ok(Ok(release)) => {
                    self.updates.asking = None;
                    self.updates.failed = None;
                    self.updates.checked = Checked { at: now() as i64, latest: Some(release) };
                    if let Err(e) = self.updates.checked.save(&self.paths.update_check) {
                        self.log(format!("Couldn't keep what the update check found: {e}"));
                    }
                    if std::mem::take(&mut self.updates.by_hand) {
                        self.updates.asked = true;
                        self.say_checked();
                    }
                }
                Ok(Err(e)) => {
                    self.updates.asking = None;
                    self.updates.failed = Some(Instant::now());
                    if std::mem::take(&mut self.updates.by_hand) {
                        self.warn(format!("Couldn't check for updates: {e}"));
                        self.updates.error = Some(e);
                    } else {
                        // Offline is usual for a mesh client: not worth the
                        // log, unless asked.
                        tracing::debug!("{e}");
                    }
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
            self.updates.ask();
        }
    }

    /// Install the newer release found in this one's place (in a thread
    /// of its own): what's happening, or why it can't be.
    pub fn install_update(&mut self) -> Result<String, String> {
        let release = self.update_available().cloned().ok_or("There's no newer release to install (Check for updates finds them)")?;
        if let Install::Elsewhere(how) = &self.updates.install {
            return Err(how.clone());
        }
        if self.updates.installing() {
            return Err(format!("Installing rettui {} already", release.version));
        }
        if self.updates.installed.as_deref() == Some(release.version.as_str()) {
            return Err(format!("rettui {} is installed: start rettui again to use it", release.version));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let installer = self.updates.installer;
        let wanted = release.clone();
        std::thread::spawn(move || {
            let _ = tx.send(installer(&wanted));
        });
        self.updates.installing = Some((release.version.clone(), rx));
        let doing = format!("Installing rettui {}: downloading it", release.version);
        self.log(doing.clone());
        Ok(doing)
    }

    /// Ask before installing the newer release (`U` in Status), or say why
    /// it can't be.
    pub(super) fn ask_install_update(&mut self) {
        let Some(release) = self.update_available().cloned() else {
            return self.warn("There's no newer release to install (Check for updates finds them)");
        };
        match &self.updates.install {
            Install::InPlace { exe, .. } => {
                let question = format!("Install rettui {} in place of {}? (y/n)", release.version, exe.display());
                self.open_prompt(PromptKind::ConfirmInstallUpdate, &question, "");
            }
            Install::Elsewhere(how) => self.warn(how.clone()),
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

    /// Settings with update checks turned on.
    fn on() -> crate::config::Settings {
        crate::config::Settings { update_check: true, ..crate::config::Settings::default() }
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
        let mut app = crate::app::test_app(&dir, on(), crate::store::Store::default());
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
        let mut again = crate::app::test_app(&dir, on(), crate::store::Store::default());
        again.updates.fetch = || panic!("asked again within a day");
        again.on_tick();
        assert_eq!(again.update_available(), Some(&found));
        // Off, as it is to start with: nothing shown, nothing asked.
        let off = crate::config::Settings::default();
        assert!(!off.update_check);
        let _ = std::fs::remove_dir_all(&dir);
        let mut quiet = crate::app::test_app(&dir, off, crate::store::Store::default());
        quiet.updates.fetch = || panic!("asked with update checks off");
        quiet.on_tick();
        assert!(quiet.update_available().is_none() && quiet.updates.asking.is_none());
        assert_eq!(quiet.version_url(), crate::config::PROJECT_URL);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_check_asked_for_says_what_it_found_with_daily_checks_off() {
        let dir = std::env::temp_dir().join(format!("rettui-updates-by-hand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        assert!(!app.settings.update_check);
        assert_eq!(app.version_status(), "Check for updates is off");
        // This version is the newest.
        app.updates.fetch = || Ok(Release { version: crate::update::VERSION.into(), url: crate::config::PROJECT_URL.into() });
        app.check_for_updates();
        assert_eq!(app.version_status(), "checking for updates…");
        settle(&mut app);
        assert!(app.notice.as_ref().unwrap().text.ends_with("is the newest release"));
        assert!(app.version_status().starts_with("up to date (checked "));
        assert!(app.update_available().is_none());
        // A build newer than the latest release (as one from dev is).
        app.updates.fetch = || Ok(Release { version: "0.1.0".into(), url: crate::config::PROJECT_URL.into() });
        app.check_for_updates();
        settle(&mut app);
        assert!(app.notice.as_ref().unwrap().text.contains("is newer than the latest release (0.1.0)"));
        // One that can't be made says why, as the daily one doesn't.
        app.updates.fetch = || Err("Couldn't reach GitHub: offline".into());
        app.check_for_updates();
        settle(&mut app);
        assert!(app.notice.as_ref().unwrap().text.starts_with("Couldn't check for updates: Couldn't reach GitHub"));
        assert!(app.version_status().starts_with("couldn't check for updates"));
        // A newer one: said, shown and installable, though daily checks are
        // off (it was asked for), and not said again by them.
        app.updates.fetch = release;
        app.check_for_updates();
        settle(&mut app);
        assert!(app.notice.as_ref().unwrap().text.starts_with("rettui 99.0.0 is out"));
        assert_eq!(app.update_available().map(|r| r.version.as_str()), Some("99.0.0"));
        assert!(app.version_status().starts_with("rettui 99.0.0 is out (checked "));
        app.settings.update_check = true;
        app.on_tick();
        assert_eq!(app.log.iter().filter(|l| l.contains("rettui 99.0.0 is out")).count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_check_that_fails_is_tried_again_later_and_an_older_release_is_not_news() {
        let dir = std::env::temp_dir().join(format!("rettui-updates-failed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, on(), crate::store::Store::default());
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

    #[test]
    fn a_newer_release_is_installed_when_asked_where_it_can_be() {
        let dir = std::env::temp_dir().join(format!("rettui-updates-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, on(), crate::store::Store::default());
        // Nothing found yet: nothing to install.
        assert!(app.install_update().unwrap_err().contains("no newer release"));
        app.updates.fetch = release;
        app.on_tick();
        settle(&mut app);
        // A build from source (as tests are) says how instead.
        assert!(app.install_update().unwrap_err().starts_with("Built from source"));
        // A release build: installed, in a thread, and said.
        app.updates.install = super::Install::InPlace { exe: dir.join("rettui"), target: "x86_64-unknown-linux-gnu" };
        app.updates.installer = |r| Ok(format!("Installed rettui {}: start rettui again to use it", r.version));
        assert!(app.install_update().unwrap().contains("Installing rettui 99.0.0"));
        assert!(app.install_update().unwrap_err().contains("already"));
        for _ in 0..200 {
            app.on_tick();
            if !app.updates.installing() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(app.updates.installed.as_deref(), Some("99.0.0"));
        assert!(app.log.iter().any(|l| l.contains("Installed rettui 99.0.0")));
        assert!(app.install_update().unwrap_err().contains("start rettui again"));
        // One that fails says why.
        app.updates.installed = None;
        app.updates.installer = |_| Err("The download isn't what the release's SHA256SUMS says".into());
        app.install_update().unwrap();
        for _ in 0..200 {
            app.on_tick();
            if !app.updates.installing() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.updates.installed.is_none() && app.log.iter().any(|l| l.contains("SHA256SUMS")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
