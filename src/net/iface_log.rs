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
        let line = match repeats {
            0 => line,
            n => format!("{line} (and {n} more time(s) since it was last logged)"),
        };
        let _ = sink.send(NetEvent::Log(line));
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
            "Interface Testnet: TCP connect failed: Connection refused (os error 111)",
            "Interface TCPInterface[Testnet]: TCP read error: reset",
            "Interface: TCP write error: broken pipe",
            "Interface: Failed to spawn interface: no such device",
        ]);
    }
}
