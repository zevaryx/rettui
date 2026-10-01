//! Interface trouble for the app's log. The Reticulum libraries report why
//! an interface won't connect, or dropped, only through `tracing` (which
//! goes to the log file); this passes their warnings and errors about
//! interfaces on to the log on screen, where they're easy to find.

use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Filter};

use super::NetEvent;

/// The same trouble again this soon is counted rather than logged: a
/// connection that keeps failing retries every few seconds.
const REPEAT_WINDOW: Duration = Duration::from_secs(300);

/// Said while interfaces shut down (Reticulum stopping or restarting),
/// not trouble.
const ROUTINE: &[&str] = &["transport channel closed"];

static STATE: Mutex<State> = Mutex::new(State { sink: None, names: None, recent: None });

struct State {
    /// The running network actor's events, which the app logs.
    sink: Option<UnboundedSender<NetEvent>>,
    /// Interface names by id, for messages that give only the id.
    names: Option<HashMap<u64, String>>,
    /// Lines logged lately: when, and how many repeats weren't.
    recent: Option<HashMap<String, (Instant, u32)>>,
}

/// Log to this network actor from now on (the one after a restart
/// replaces it). A restart starts afresh: trouble that's still there is
/// logged again, rather than looking fixed.
pub fn attach(sink: UnboundedSender<NetEvent>) {
    if let Ok(mut state) = STATE.lock() {
        state.sink = Some(sink);
        state.recent = None;
    }
}

/// The interfaces running now, by id.
pub fn set_names(names: impl IntoIterator<Item = (u64, String)>) {
    if let Ok(mut state) = STATE.lock() {
        state.names = Some(names.into_iter().collect());
    }
}

/// The layer to add to the `tracing` subscriber.
pub fn layer<S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>>() -> impl Layer<S> {
    InterfaceLog.with_filter(Trouble)
}

/// Warnings and errors from the Reticulum libraries.
struct Trouble;

impl<S> Filter<S> for Trouble {
    fn enabled(&self, meta: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        *meta.level() <= Level::WARN && meta.target().starts_with("rns_")
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::WARN)
    }
}

struct InterfaceLog;

impl<S: Subscriber> Layer<S> for InterfaceLog {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        // The interface crate's are all about interfaces; the runtime's
        // only some are.
        let about_interfaces = event.metadata().target().starts_with("rns_interface")
            || fields.interface.is_some()
            || fields.id.is_some()
            || fields.message.to_lowercase().contains("interface");
        if !about_interfaces || ROUTINE.contains(&fields.message.as_str()) {
            return;
        }
        let Ok(mut state) = STATE.lock() else { return };
        let Some(sink) = state.sink.clone() else { return };
        let who = fields.interface.clone().or_else(|| {
            let id = fields.id?;
            Some(state.names.as_ref().and_then(|names| names.get(&id).cloned()).unwrap_or_else(|| format!("#{id}")))
        });
        let line = fields.line(who.as_deref());
        let now = Instant::now();
        let recent = state.recent.get_or_insert_with(HashMap::new);
        let repeats = match recent.get_mut(&line) {
            Some((when, repeats)) if now.duration_since(*when) < REPEAT_WINDOW => {
                *repeats += 1;
                return;
            }
            Some((_, repeats)) => std::mem::take(repeats),
            None => 0,
        };
        recent.insert(line.clone(), (now, 0));
        if recent.len() > 256 {
            recent.retain(|_, (when, _)| now.duration_since(*when) < REPEAT_WINDOW);
        }
        drop(state);
        let hint = hint(&line);
        let mut line = match repeats {
            0 => line,
            n => format!("{line} (and {n} more time(s) since it was last logged)"),
        };
        if let Some(hint) = hint {
            line = format!("{line}. {hint}");
        }
        let _ = sink.send(NetEvent::Log(line));
    }
}

/// What to do about common trouble, in plain words: the libraries' errors
/// name the symptom ("Connection refused (os error 111)"), not the cause.
/// The wording differs by system, so each is matched in its Linux, macOS
/// and Windows forms.
pub fn hint(line: &str) -> Option<&'static str> {
    let line = line.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| line.contains(word));
    let device = has(&["serial", "rnode", "kiss", "tty"]);
    if has(&["max reconnect tries reached"]) {
        Some("It has stopped trying: restart Reticulum to try again")
    } else if has(&["failed to lookup address", "name or service not known", "temporary failure in name resolution", "nodename nor servname", "no such host is known"]) {
        Some("The host name couldn't be looked up: check this computer's internet connection and the interface's host name")
    } else if has(&["network is unreachable", "no route to host", "network unreachable"]) {
        Some("This computer has no route there: check its internet connection")
    } else if has(&["i2p", "sam "]) && has(&["refused"]) {
        Some("No I2P router is answering: I2P interfaces need one running (i2pd, or I2P with its SAM bridge on)")
    } else if has(&["connection refused", "actively refused"]) {
        Some("Nothing accepted the connection: the host may be down, or the port is wrong. Another entry point, or interface discovery, can stand in")
    } else if has(&["timed out", "timeout"]) {
        Some("No answer: the host may be down, or a firewall is in the way")
    } else if has(&["address already in use", "address in use", "only one usage of each socket address"]) {
        Some("Another program already uses that port, such as another Reticulum (rnsd) or a second rettui: stop it, or pick another port")
    } else if has(&["device or resource busy", "resource busy"]) {
        Some("Another program has the device open (rnsd, rnodeconf, or a second rettui): close it first")
    } else if device && has(&["no such file or directory", "no such device", "cannot find the file", "not found"]) {
        Some("The device isn't there: check the radio is plugged in and the interface's port (such as /dev/ttyUSB0 or /dev/ttyACM0, or COM3 on Windows)")
    } else if device && has(&["permission denied", "access is denied"]) {
        Some("Not allowed to open the device: on Linux, add yourself to the group that owns it (often dialout) and log in again")
    } else if has(&["permission denied", "access is denied"]) {
        Some("Not allowed: listening on a port below 1024 needs administrator rights, so pick a higher one")
    } else {
        None
    }
}

/// What a message says, and about which interface.
#[derive(Default)]
struct Fields {
    message: String,
    /// The interface's name.
    interface: Option<String>,
    /// Or its id.
    id: Option<u64>,
    error: Option<String>,
    /// Anything else, as `key=value`.
    other: Vec<String>,
}

impl Fields {
    fn line(&self, who: Option<&str>) -> String {
        let mut line = match who {
            Some(who) => format!("Interface {who}: "),
            None => "Interface: ".to_string(),
        };
        let mut message = self.message.chars();
        if let Some(first) = message.next() {
            line.extend(first.to_uppercase());
            line.push_str(message.as_str());
        }
        if let Some(error) = &self.error {
            let _ = write!(line, ": {error}");
        }
        if !self.other.is_empty() {
            let _ = write!(line, " ({})", self.other.join(", "));
        }
        line
    }
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "name" | "interface" | "interface_name" => self.interface = Some(value.to_string()),
            "error" | "err" => self.error = Some(value.to_string()),
            "interface_id" | "id" => match value.parse() {
                Ok(id) => self.id = Some(id),
                Err(_) => self.interface = Some(value.to_string()),
            },
            other => self.other.push(format!("{other}={value}")),
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "interface_id" | "id" => self.id = Some(value),
            _ => self.record_str(field, &value.to_string()),
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        match u64::try_from(value) {
            Ok(value) => self.record_u64(field, value),
            Err(_) => self.record_str(field, &value.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn interface_trouble_reaches_the_log_once_in_a_while() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        attach(tx);
        set_names([(7, "TCPInterface[Testnet]".to_string())]);
        let subscriber = tracing_subscriber::registry().with(layer());
        tracing::subscriber::with_default(subscriber, || {
            let e = "Connection refused (os error 111)";
            tracing::warn!(target: "rns_interface::tcp", name = "Testnet", error = %e, "TCP connect failed");
            tracing::warn!(target: "rns_interface::tcp", name = "Testnet", error = %e, "TCP connect failed");
            tracing::warn!(target: "rns_interface::tcp", interface_id = 7u64, error = "reset", "TCP read error");
            tracing::warn!(target: "rns_interface::tcp", interface_id = 9u64, "transport channel closed");
            tracing::warn!(target: "rns_interface::tcp", error = "broken pipe", "TCP write error");
            tracing::warn!(target: "rns_runtime::reticulum", "failed to spawn interface: no such device");
            // Not about interfaces, or not trouble.
            tracing::warn!(target: "rns_runtime::reticulum", "RPC server error: busy");
            tracing::info!(target: "rns_interface::tcp", name = "Testnet", "reconnecting in 5s");
            tracing::warn!(target: "rettui", "something else");
        });
        let lines: Vec<String> = std::iter::from_fn(|| match rx.try_recv() {
            Ok(NetEvent::Log(line)) => Some(line),
            _ => None,
        })
        .collect();
        assert_eq!(lines, [
            "Interface Testnet: TCP connect failed: Connection refused (os error 111). Nothing accepted the connection: the host may be down, or the port is wrong. Another entry point, or interface discovery, can stand in",
            "Interface TCPInterface[Testnet]: TCP read error: reset",
            "Interface: TCP write error: broken pipe",
            "Interface: Failed to spawn interface: no such device",
        ]);
    }

    #[test]
    fn common_trouble_gets_a_plain_hint() {
        // As rettui logged them with broken interfaces, and the same on
        // other systems.
        let cases = [
            ("Interface Bad DNS: TCP connect failed: failed to lookup address information: Name or service not known", "looked up"),
            ("Interface Bad DNS: TCP connect failed: failed to lookup address information: nodename nor servname provided, or not known", "looked up"),
            ("Interface Bad DNS: TCP connect failed: No such host is known. (os error 11001)", "looked up"),
            ("Interface Refused: TCP connect failed: Connection refused (os error 111)", "Nothing accepted"),
            ("Interface Refused: TCP connect failed: No connection could be made because the target machine actively refused it. (os error 10061)", "Nothing accepted"),
            ("Interface Blackhole: TCP connect timed out", "No answer"),
            ("Interface No I2P: I2P client: failed to connect to SAM bridge: SAM I/O error: Connection refused (os error 111)", "I2P router"),
            ("Interface: Failed to spawn interface: TCP server: I/O error: Address already in use (os error 98)", "already uses that port"),
            ("Interface: Failed to spawn interface: RNode: send failed: rnode serial open: No such file or directory", "plugged in"),
            ("Interface: Failed to spawn interface: Serial: send failed: serial open: Permission denied (os error 13)", "dialout"),
            ("Interface: Failed to spawn interface: RNode: send failed: rnode serial open: Device or resource busy", "close it first"),
            ("Interface: Failed to spawn interface: TCP server: I/O error: Permission denied (os error 13)", "below 1024"),
            ("Interface Testnet: Max reconnect tries reached", "restart Reticulum"),
            ("Interface Down: Network is unreachable (os error 101)", "no route"),
        ];
        for (line, expected) in cases {
            let hint = hint(line).unwrap_or_else(|| panic!("no hint for {line}"));
            assert!(hint.contains(expected), "{line} → {hint}");
        }
        assert_eq!(hint("Interface TCPInterface[Testnet]: TCP read error: reset"), None);
        // A pipe interface's missing program isn't a missing radio.
        assert_eq!(hint("Interface: Failed to spawn interface: Pipe: spawn command: No such file or directory"), None);
    }
}
