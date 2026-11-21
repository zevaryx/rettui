//! What the node remembers between runs so that a restart (or a crash) doesn't
//! reset a limit: the propagation storage sizes of the last 24 hours, and the
//! Reticulum change limits. Both are small private files in `manage/`.
//! Times in them are unix seconds.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::unix_now;
use crate::config::{self, Paths};

/// How long a propagation storage size counts as recent.
pub const STORAGE_WINDOW: Duration = Duration::from_secs(24 * 3600);
/// The most values kept of either list (a file with more is cut).
const MAX_KEPT: usize = 1000;
/// How far ahead a saved time may be (the clock was set back a little)
/// before it is taken to be now.
const SKEW_SECS: u64 = 300;

/// A moment in the past, on the monotonic clock, so that the wall clock
/// jumping can't move it. One loaded from a file is `age` old when loaded,
/// which can be more than the computer has been up (after a reboot), so it
/// is kept as that age and the `Instant` it was loaded at.
#[derive(Debug, Clone, Copy)]
pub struct Moment {
    loaded: Instant,
    age: Duration,
}

impl Moment {
    pub fn now() -> Self {
        Self::ago(Duration::ZERO)
    }

    /// The moment `age` ago.
    pub fn ago(age: Duration) -> Self {
        Self { loaded: Instant::now(), age }
    }

    /// How long ago it was.
    pub fn elapsed(&self) -> Duration {
        self.age.saturating_add(self.loaded.elapsed())
    }

    /// The age of a unix time `at`, at unix time `now`: a time ahead of the
    /// clock is taken as now.
    fn from_unix(at: u64, now: u64) -> Self {
        let age = if at > now.saturating_add(SKEW_SECS) { 0 } else { now.saturating_sub(at) };
        Self::ago(Duration::from_secs(age))
    }

    fn to_unix(self, now: u64) -> u64 {
        now.saturating_sub(self.elapsed().as_secs())
    }
}

fn file(paths: &Paths, name: &str) -> PathBuf {
    paths.root.join("manage").join(name)
}

/// What reading a saved file gave.
enum Read<T> {
    Missing,
    Found(T),
    /// It couldn't be read or understood, and was moved aside.
    Bad,
}

/// A file's contents. One that can't be read or understood is moved aside
/// (so it isn't overwritten, and can be looked at) and said in the log.
fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Read<T> {
    let why = match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => return Read::Found(value),
            Err(e) => e.to_string(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Missing,
        Err(e) => e.to_string(),
    };
    let name = path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let aside = path.with_file_name(format!("{name}.unreadable-{}", unix_now()));
    let moved = match std::fs::rename(path, &aside) {
        Ok(()) => format!("moved it to {}", aside.display()),
        Err(e) => format!("and it couldn't be moved aside: {e}"),
    };
    tracing::warn!("Remote management: {} can't be read ({why}): {moved}", path.display());
    Read::Bad
}

fn save<T: Serialize>(path: &Path, value: &T) {
    let result = (|| -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        config::write_private(path, &serde_json::to_vec(value)?)
    })();
    if let Err(e) = result {
        tracing::warn!("Remote management: couldn't save {}: {e:#}", path.display());
    }
}

/// The propagation storage sizes (MB) of the last 24 hours, so that a cut
/// remotely is held to half of the most it was, not of what it is just now
/// (which a few small cuts in a row would get around).
///
/// Kept as the sizes that can still be the most of the window: newest last,
/// each smaller than the one before. A size that is no bigger than a newer
/// one is never the most again, so it is dropped, and the most is always the
/// first. Going up and down many times can't push it out.
#[derive(Debug, Default)]
pub struct StorageHistory {
    seen: VecDeque<(Moment, u64)>,
}

/// What `storage-history.json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedStorage {
    /// (unix second, MB), oldest first.
    #[serde(default)]
    seen: Vec<(u64, u64)>,
}

impl StorageHistory {
    const FILE: &'static str = "storage-history.json";

    pub fn load(paths: &Paths) -> Self {
        let saved = match read::<SavedStorage>(&file(paths, Self::FILE)) {
            Read::Found(saved) => saved,
            Read::Missing | Read::Bad => SavedStorage::default(),
        };
        let now = unix_now();
        let mut history = Self::default();
        for (at, mb) in saved.seen {
            let at = Moment::from_unix(at, now);
            if at.elapsed() < STORAGE_WINDOW {
                history.note(at, mb);
            }
        }
        history
    }

    /// Remember `mb` seen at `at`.
    fn note(&mut self, at: Moment, mb: u64) {
        while self.seen.front().is_some_and(|(seen, _)| seen.elapsed() >= STORAGE_WINDOW) {
            self.seen.pop_front();
        }
        while self.seen.back().is_some_and(|(_, old)| *old <= mb) {
            self.seen.pop_back();
        }
        self.seen.push_back((at, mb));
        // Over the cap: the smallest go, never the most.
        while self.seen.len() > MAX_KEPT {
            self.seen.pop_back();
        }
    }

    /// The most the storage was in the last 24 hours, `current` (what it is
    /// now) included.
    pub fn reference(&self, current: u64) -> u64 {
        self.seen.iter().filter(|(at, _)| at.elapsed() < STORAGE_WINDOW).map(|(_, mb)| *mb).fold(current, u64::max)
    }

    /// Note that the storage went from `from` to `to` now, and keep it.
    pub fn changed(&mut self, paths: &Paths, from: u64, to: u64) {
        self.note(Moment::now(), from);
        self.note(Moment::now(), to);
        let now = unix_now();
        save(&file(paths, Self::FILE), &SavedStorage { seen: self.seen.iter().map(|(at, mb)| (at.to_unix(now), *mb)).collect() });
    }

    /// A history of (age, MB) entries, oldest first.
    #[cfg(test)]
    pub(super) fn with(seen: Vec<(Duration, u64)>) -> Self {
        let mut history = Self::default();
        for (age, mb) in seen {
            history.note(Moment::ago(age), mb);
        }
        history
    }
}

/// What the Reticulum change limits have counted, as saved.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedLimits {
    #[serde(default)]
    applies: Vec<u64>,
    #[serde(default)]
    held_until: Option<u64>,
    #[serde(default)]
    restarted: Option<u64>,
}

/// The Reticulum change limits, on the monotonic clock for measuring and
/// saved as unix seconds.
#[derive(Debug, Default)]
pub struct ChangeLimits {
    /// When the config changes in the last window were applied.
    pub applies: VecDeque<Moment>,
    /// No config change until then (one was put back by itself).
    pub held_until: Option<Instant>,
    /// When Reticulum was last restarted from afar.
    pub restarted: Option<Moment>,
}

impl ChangeLimits {
    const FILE: &'static str = "limits.json";

    /// The limits saved by an earlier run. Changes older than `window` are
    /// no longer counted; the hold is kept at most `hold` from now. A file
    /// that can't be read is set aside, and then the next change waits `hold`,
    /// as it can't be known what was counted.
    pub fn load(paths: &Paths, window: Duration, hold: Duration) -> Self {
        match read(&file(paths, Self::FILE)) {
            Read::Found(saved) => Self::from_saved(saved, unix_now(), Instant::now(), window, hold),
            Read::Missing => Self::default(),
            Read::Bad => {
                let limits = Self { held_until: Some(Instant::now() + hold), ..Self::default() };
                limits.save(paths);
                limits
            }
        }
    }

    fn from_saved(saved: SavedLimits, now: u64, instant: Instant, window: Duration, hold: Duration) -> Self {
        let mut applies: Vec<Moment> =
            saved.applies.iter().map(|at| Moment::from_unix(*at, now)).filter(|at| at.elapsed() < window).collect();
        applies.sort_by_key(|at| std::cmp::Reverse(at.elapsed()));
        let extra = applies.len().saturating_sub(MAX_KEPT);
        applies.drain(..extra);
        let held_until = saved.held_until.and_then(|until| {
            let left = Duration::from_secs(until.saturating_sub(now)).min(hold + Duration::from_secs(SKEW_SECS));
            (!left.is_zero()).then(|| instant + left)
        });
        let restarted = saved.restarted.map(|at| Moment::from_unix(at, now));
        Self { applies: applies.into(), held_until, restarted }
    }

    pub fn save(&self, paths: &Paths) {
        let (now, instant) = (unix_now(), Instant::now());
        let saved = SavedLimits {
            applies: self.applies.iter().map(|at| at.to_unix(now)).collect(),
            held_until: self.held_until.filter(|until| *until > instant).map(|until| now + (until - instant).as_secs().max(1)),
            restarted: self.restarted.map(|at| at.to_unix(now)),
        };
        save(&file(paths, Self::FILE), &saved);
    }

    /// Hold the next change for `hold` from now (a change was put back by
    /// itself), in the saved limits of a node that isn't running yet.
    pub fn hold_saved(paths: &Paths, hold: Duration) {
        let path = file(paths, Self::FILE);
        let mut saved = match read::<SavedLimits>(&path) {
            Read::Found(saved) => saved,
            Read::Missing | Read::Bad => SavedLimits::default(),
        };
        let until = unix_now() + hold.as_secs();
        saved.held_until = Some(saved.held_until.map_or(until, |old| old.max(until)));
        save(&path, &saved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);
    const DAY: Duration = STORAGE_WINDOW;

    fn temp(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("rettui-manage-recent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Paths::new(Some(dir)).unwrap()
    }

    fn aside(paths: &Paths) -> usize {
        std::fs::read_dir(paths.root.join("manage"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".unreadable-"))
            .count()
    }

    #[test]
    fn the_storage_reference_is_the_most_in_the_last_day() {
        let history = StorageHistory::with(vec![(Duration::from_secs(2000), 1000), (Duration::from_secs(1000), 500)]);
        assert_eq!(history.reference(250), 1000);
        assert_eq!(history.reference(2000), 2000, "what it is now counts too");
        // After a day the old value no longer counts; the later one still does.
        let history = StorageHistory::with(vec![(DAY + HOUR, 1000), (Duration::from_secs(1000), 500)]);
        assert_eq!(history.reference(250), 500);
        let history = StorageHistory::with(vec![(DAY + HOUR, 1000), (DAY + Duration::from_secs(1), 500)]);
        assert_eq!(history.reference(250), 250);
    }

    #[test]
    fn going_up_and_down_many_times_cant_push_out_the_most() {
        let mut history = StorageHistory::with(vec![(Duration::from_secs(60), 10_000)]);
        let paths = temp("toggle");
        for n in 0..600 {
            let (from, to) = if n % 2 == 0 { (5000, 5001) } else { (5001, 5000) };
            history.changed(&paths, from, to);
        }
        assert_eq!(history.reference(5000), 10_000);
        assert!(history.seen.len() < 10, "{}", history.seen.len());
        // So a cut below half of that is still refused, after a restart too.
        assert_eq!(StorageHistory::load(&paths).reference(5000), 10_000);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn over_the_cap_the_smallest_go_and_never_the_most() {
        let mut history = StorageHistory::default();
        for mb in (1..=(MAX_KEPT as u64 + 200)).rev() {
            history.note(Moment::now(), mb);
        }
        assert_eq!(history.seen.len(), MAX_KEPT);
        assert_eq!(history.reference(0), MAX_KEPT as u64 + 200);
    }

    #[test]
    fn storage_history_survives_a_restart_and_a_bad_file_is_set_aside() {
        let paths = temp("storage");
        StorageHistory::default().changed(&paths, 1000, 500);
        assert_eq!(StorageHistory::load(&paths).reference(500), 1000);
        std::fs::write(file(&paths, StorageHistory::FILE), b"not json").unwrap();
        assert_eq!(StorageHistory::load(&paths).reference(7), 7);
        assert_eq!(aside(&paths), 1);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn a_saved_time_ahead_of_the_clock_cant_outlast_the_window() {
        let ahead = Moment::from_unix(1_000_000 + 10 * 86_400, 1_000_000);
        assert!(ahead.elapsed() < Duration::from_secs(5));
        let mut history = StorageHistory::default();
        history.note(ahead, 9000);
        assert_eq!(history.reference(1), 9000);
    }

    #[test]
    fn change_limits_survive_a_restart() {
        let paths = temp("limits");
        let mut limits = ChangeLimits::default();
        limits.applies.push_back(Moment::now());
        limits.applies.push_back(Moment::now());
        limits.held_until = Some(Instant::now() + Duration::from_secs(600));
        limits.restarted = Some(Moment::now());
        limits.save(&paths);
        let loaded = ChangeLimits::load(&paths, HOUR, Duration::from_secs(900));
        assert_eq!(loaded.applies.len(), 2);
        let left = loaded.held_until.unwrap() - Instant::now();
        assert!((590..=600).contains(&left.as_secs()), "{left:?}");
        assert!(loaded.restarted.is_some());
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn saved_times_are_old_enough_or_taken_as_now() {
        let (now, instant) = (1_000_000, Instant::now());
        let hold = Duration::from_secs(900);
        let saved = SavedLimits {
            // One long ago (forgotten), one recent, one far in the future (just now).
            applies: vec![now - 7200, now - 60, now + 100_000],
            held_until: Some(now + 100_000),
            restarted: Some(now + 100_000),
        };
        let loaded = ChangeLimits::from_saved(saved, now, instant, HOUR, hold);
        assert_eq!(loaded.applies.len(), 2);
        // A hold far ahead is cut to the hold there is (and a little skew).
        assert!(loaded.held_until.unwrap() - instant <= hold + Duration::from_secs(SKEW_SECS));
        assert!(loaded.restarted.unwrap().elapsed() < Duration::from_secs(5), "a restart in the future is a restart now");
        // A hold that has ended is gone.
        let ended = SavedLimits { held_until: Some(now - 1), ..SavedLimits::default() };
        assert!(ChangeLimits::from_saved(ended, now, instant, HOUR, hold).held_until.is_none());
    }

    #[test]
    fn a_time_older_than_the_computer_has_been_up_stays_old() {
        // The uptime is far less than 100 days (a reboot): such a time must still read as old.
        let now = 100_000_000;
        let hundred_days = 100 * 86_400;
        let saved = SavedLimits { applies: vec![now - hundred_days], held_until: None, restarted: Some(now - hundred_days) };
        let loaded = ChangeLimits::from_saved(saved, now, Instant::now(), Duration::from_secs(200 * 86_400), Duration::ZERO);
        assert!(loaded.applies[0].elapsed() >= Duration::from_secs(hundred_days));
        assert!(loaded.restarted.unwrap().elapsed() >= Duration::from_secs(hundred_days));
        // And a time well inside the window keeps its age, not "now".
        let recent = SavedLimits { applies: vec![now - 3000], ..SavedLimits::default() };
        let loaded = ChangeLimits::from_saved(recent, now, Instant::now(), HOUR, Duration::ZERO);
        assert!(loaded.applies[0].elapsed() >= Duration::from_secs(3000));
    }

    #[test]
    fn a_hold_is_saved_for_the_next_run_and_not_shortened() {
        let paths = temp("hold");
        ChangeLimits::hold_saved(&paths, Duration::from_secs(900));
        let loaded = ChangeLimits::load(&paths, HOUR, Duration::from_secs(900));
        assert!(loaded.held_until.unwrap() > Instant::now() + Duration::from_secs(880));
        ChangeLimits::hold_saved(&paths, Duration::from_secs(10));
        let loaded = ChangeLimits::load(&paths, HOUR, Duration::from_secs(900));
        assert!(loaded.held_until.unwrap() > Instant::now() + Duration::from_secs(880));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn a_limits_file_that_cant_be_read_is_set_aside_and_holds_the_next_change() {
        let hold = Duration::from_secs(900);
        let left = |limits: &ChangeLimits| limits.held_until.expect("held").saturating_duration_since(Instant::now());
        // Not JSON.
        let paths = temp("corrupt");
        std::fs::create_dir_all(paths.root.join("manage")).unwrap();
        std::fs::write(file(&paths, ChangeLimits::FILE), b"{ nope").unwrap();
        let loaded = ChangeLimits::load(&paths, HOUR, hold);
        assert!(left(&loaded) > Duration::from_secs(880) && loaded.applies.is_empty());
        assert_eq!(aside(&paths), 1);
        // The hold was saved, so a next run keeps it.
        assert!(left(&ChangeLimits::load(&paths, HOUR, hold)) > Duration::from_secs(880));
        let _ = std::fs::remove_dir_all(&paths.root);
        // Not a file that can be read (a folder): the same.
        let paths = temp("unreadable");
        std::fs::create_dir_all(file(&paths, ChangeLimits::FILE)).unwrap();
        let loaded = ChangeLimits::load(&paths, HOUR, hold);
        assert!(left(&loaded) > Duration::from_secs(880));
        assert_eq!(aside(&paths), 1);
        // A hold written at the start doesn't turn a corrupt file into an empty one either.
        std::fs::write(file(&paths, ChangeLimits::FILE), b"garbage").unwrap();
        ChangeLimits::hold_saved(&paths, hold);
        assert!(left(&ChangeLimits::load(&paths, HOUR, hold)) > Duration::from_secs(880));
        let _ = std::fs::remove_dir_all(&paths.root);
    }
}
