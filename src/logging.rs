//! How much goes to `rettui.log`: the `log_level` setting, changed while
//! rettui runs (unless `RETTUI_LOG` was set when it started, which wins).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// How much can be logged, least first.
pub const LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace"];

/// Applies a new filter to the log file's layer.
type Reload = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

static RELOAD: OnceLock<Reload> = OnceLock::new();

/// The filter the log starts with: `RETTUI_LOG`'s, or the setting's.
pub fn starting_filter(level: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_env("RETTUI_LOG").unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level))
}

/// Whether `RETTUI_LOG` decides it instead of the setting.
pub fn from_environment() -> bool {
    std::env::var_os("RETTUI_LOG").is_some()
}

/// Called once logging is set up, with how to change its filter.
pub fn set_reloader(reload: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static) {
    let _ = RELOAD.set(Box::new(reload));
}

/// Log at `level` from now on; a note to show if it can't be.
pub fn set_level(level: &str) -> Option<String> {
    if from_environment() {
        return Some("RETTUI_LOG is set, so it decides what's logged, not Log level".into());
    }
    let reload = RELOAD.get()?;
    reload(level).err().map(|e| format!("Couldn't change the log level: {e}"))
}

/// The most one log file holds. Two files are kept (the current one and the
/// one before), so the log never takes more than twice this. At the default
/// level a day of ordinary use writes a few kilobytes, so 8 MiB is months of
/// it; at `debug` or `trace` it is minutes, which is enough to see what led
/// up to a problem. A peer that makes the libraries log once per packet can
/// no longer fill the disk through the log.
pub const LOG_FILE_LIMIT: u64 = 8 * 1024 * 1024;

/// The log file, kept under [`LOG_FILE_LIMIT`]: when a line would take it over,
/// the file becomes `<name>.1` (the one before, replaced) and a new one is
/// started. If it can't be renamed, the file is emptied instead, so the limit
/// holds either way. Writing never fails: a log that can't be written is not
/// worth stopping rettui for.
pub struct CappedLog {
    inner: Mutex<Capped>,
}

struct Capped {
    path: PathBuf,
    limit: u64,
    file: Option<File>,
    /// What the current file holds.
    written: u64,
}

fn open_append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Where the log before `path` is kept: `rettui.log.1`.
pub fn previous_log(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

impl CappedLog {
    /// Open `path` for appending (made if there is none), counting what it
    /// already holds.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_with_limit(path, LOG_FILE_LIMIT)
    }

    fn open_with_limit(path: &Path, limit: u64) -> io::Result<Self> {
        let file = open_append(path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self { inner: Mutex::new(Capped { path: path.to_path_buf(), limit, file: Some(file), written }) })
    }
}

impl Capped {
    /// Start a new file: the current one becomes the previous log. On Windows,
    /// where another program may hold the file open for a moment, the rename is
    /// retried for up to a second (`config::replace`), during which logging waits.
    fn rotate(&mut self) {
        // Closed first: Windows doesn't rename an open file.
        self.file = None;
        let moved = crate::config::replace(&self.path, &previous_log(&self.path));
        self.file = open_append(&self.path).ok();
        self.written = 0;
        if moved.is_err() {
            // Couldn't move it aside: empty it, which keeps the limit.
            self.file = OpenOptions::new().write(true).truncate(true).open(&self.path).ok().and_then(|_| open_append(&self.path).ok());
        }
    }

    fn write_line(&mut self, buf: &[u8]) {
        if self.file.is_none() {
            // A rotation couldn't open the new file: try again.
            self.file = open_append(&self.path).ok();
            self.written = self.file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
        }
        if self.written > 0 && self.written.saturating_add(buf.len() as u64) > self.limit {
            self.rotate();
        }
        if let Some(file) = &mut self.file
            && file.write_all(buf).is_ok()
        {
            self.written = self.written.saturating_add(buf.len() as u64);
        }
    }
}

/// One write to the capped log.
pub struct CappedWriter<'a>(&'a CappedLog);

impl Write for CappedWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.inner.lock().unwrap_or_else(|e| e.into_inner()).write_line(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CappedLog {
    type Writer = CappedWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        CappedWriter(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt as _;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-logging-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn size(path: &Path) -> u64 {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }

    fn write(log: &CappedLog, text: &str) {
        let mut writer = CappedWriter(log);
        writer.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn a_log_under_the_limit_is_one_file() {
        let dir = folder("under");
        let path = dir.join("rettui.log");
        let log = CappedLog::open_with_limit(&path, 1000).unwrap();
        for i in 0..9 {
            write(&log, &format!("{i:04} {}\n", "x".repeat(94)));
        }
        assert_eq!(size(&path), 9 * 100);
        assert!(!previous_log(&path).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_full_log_becomes_the_previous_one_and_only_one_is_kept() {
        let dir = folder("rotate");
        let path = dir.join("rettui.log");
        let log = CappedLog::open_with_limit(&path, 1000).unwrap();
        for i in 0..35 {
            write(&log, &format!("{i:04} {}\n", "x".repeat(94)));
        }
        // 35 lines of 100 B with room for 10 a file: the file is on its fourth.
        assert_eq!(size(&path), 500);
        assert_eq!(size(&previous_log(&path)), 1000);
        let now = std::fs::read_to_string(&path).unwrap();
        assert!(now.starts_with("0030 ") && now.contains("0034 "), "the newest lines are in the log");
        let before = std::fs::read_to_string(previous_log(&path)).unwrap();
        assert!(before.starts_with("0020 ") && before.contains("0029 "), "the one before it is next to it");
        let names = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(names, 2, "no older file is kept");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_log_already_over_the_limit_is_moved_aside_at_the_first_line() {
        let dir = folder("existing");
        let path = dir.join("rettui.log");
        std::fs::write(&path, "o".repeat(5000)).unwrap();
        let log = CappedLog::open_with_limit(&path, 1000).unwrap();
        write(&log, "new\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(size(&previous_log(&path)), 5000);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_flood_never_takes_more_than_twice_the_limit() {
        let dir = folder("flood");
        let path = dir.join("rettui.log");
        let log = CappedLog::open_with_limit(&path, 64 * 1024).unwrap();
        let line = format!("{}\n", "w".repeat(203));
        let mut most = 0;
        // 100,000 lines of 204 B: about 20 MB, as the 300,000-frame test wrote.
        for _ in 0..100_000 {
            write(&log, &line);
            most = most.max(size(&path) + size(&previous_log(&path)));
        }
        assert!(most <= 2 * 64 * 1024, "{most} bytes on disk");
        assert!(size(&path) > 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_moved_aside_is_emptied_and_stays_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = folder("stuck");
        let path = dir.join("rettui.log");
        // A folder where the previous log goes: the rename fails.
        std::fs::create_dir_all(previous_log(&path).join("in")).unwrap();
        let log = CappedLog::open_with_limit(&path, 1000).unwrap();
        for _ in 0..50 {
            write(&log, &format!("{}\n", "y".repeat(99)));
        }
        assert!(size(&path) <= 1000, "{} bytes", size(&path));
        assert!(size(&path) > 0);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_new_file_after_rotation_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = folder("mode");
        let path = dir.join("rettui.log");
        let log = CappedLog::open_with_limit(&path, 200).unwrap();
        for _ in 0..10 {
            write(&log, &format!("{}\n", "z".repeat(99)));
        }
        for file in [path.clone(), previous_log(&path)] {
            assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn events_from_tracing_stay_within_the_limit() {
        let dir = folder("tracing");
        let path = dir.join("rettui.log");
        let log = CappedLog::open_with_limit(&path, 32 * 1024).unwrap();
        let subscriber = tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_writer(log).with_ansi(false));
        tracing::subscriber::with_default(subscriber, || {
            for i in 0..20_000 {
                tracing::warn!(interface_id = i, raw_len = 25, "inbound packet dropped: header parse failed");
            }
        });
        assert!(size(&path) + size(&previous_log(&path)) <= 64 * 1024);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().all(|l| l.contains("header parse failed")), "whole lines only");
        assert!(text.contains("interface_id=19999"), "the newest event is kept");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
