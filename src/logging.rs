//! How much goes to `rettui.log`: the `log_level` setting, changed while
//! rettui runs (unless `RETTUI_LOG` was set when it started, which wins).

use std::sync::OnceLock;

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
