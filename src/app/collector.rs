//! Telemetry collectors, as Sideband has them: a station that keeps
//! everyone's newest locations and passes them on when asked, so a group
//! sees each other on the map without each asking each. rettui sends its
//! location to one and asks it for everyone's (the `telemetry_collector`
//! settings), reads what one passes on, and can be one itself
//! (`collector_serve`).

use std::time::{Duration, Instant};

use super::{App, now};
use crate::lxmf::fields::{Command, MAX_STREAM, PassedOn, Relayed};
use crate::lxmf::{self, DeliveryMode};
use crate::net::{NetCommand, parse_hash};
use crate::store::{Message, MessageState, RelayedPlace, Trust};

/// The most locations kept from collectors; the oldest go first.
pub const MAX_RELAYED: usize = 1_000;
/// How far back the first request to a collector asks (a week, as
/// Sideband asks).
const FIRST_ASK: i64 = 7 * 24 * 60 * 60;

/// "1 location", "3 locations".
fn locations(n: usize) -> String {
    if n == 1 { "1 location".into() } else { format!("{n} locations") }
}

impl App {
    /// Others' locations a collector passed on, from `key`: kept if they
    /// came from a contact you trust or your collector, newer ones in place
    /// of older; what's said of them on the message.
    pub(super) fn take_stream(&mut self, key: &str, stream: &[Relayed]) -> String {
        let count = locations(stream.len());
        let trusted = self.store.contact(key).trust == Trust::Trusted;
        if !trusted && self.settings.telemetry_collector.as_deref() != Some(key) {
            return format!("Passed on {count}, not used: only those from contacts you trust or your Telemetry collector are");
        }
        let own = self.lxmf_hash.map(hex::encode);
        let mut taken = 0;
        for entry in stream {
            let Some(location) = entry.telemetry.location else { continue };
            let source = hex::encode(entry.source);
            if own.as_deref() == Some(source.as_str()) || source == key {
                continue;
            }
            // Not one from the future, which no later update would replace.
            let at = (entry.time as f64).min(now());
            if self.store.relayed.get(&source).is_some_and(|r| r.at >= at) {
                continue;
            }
            self.store.relayed.insert(source, RelayedPlace { location, at, via: key.to_string() });
            taken += 1;
        }
        while self.store.relayed.len() > MAX_RELAYED {
            let oldest = self.store.relayed.iter().min_by(|a, b| a.1.at.total_cmp(&b.1.at)).map(|(k, _)| k.clone());
            let Some(oldest) = oldest else { break };
            self.store.relayed.remove(&oldest);
        }
        self.store_dirty = true;
        match taken {
            0 => format!("Passed on {count}, none newer than those on the map"),
            n => format!("Passed on {count}: {} new on the map", locations(n)),
        }
    }

    /// Answer `key`'s collector request (for locations since `since`, Unix
    /// seconds), if *Be a collector* says so: the newest location of
    /// everyone who sent one (not those from another collector), and this
    /// station's.
    pub(super) fn answer_collector_request(&mut self, key: &str, since: i64) -> Result<String, String> {
        let allowed = match self.settings.collector_serve.as_str() {
            "trusted" => self.store.contact(key).trust == Trust::Trusted,
            "contacts" => self.is_known(key),
            _ => return Err("Not answered: Be a collector is off".into()),
        };
        if !allowed {
            return Err(format!("Not answered: Be a collector is set to {} only", self.settings.collector_serve));
        }
        let asked = format!("{key}:collector");
        let now_at = Instant::now();
        if self.answered.get(&asked).is_some_and(|at| now_at.duration_since(*at) < super::messages::ANSWER_EVERY) {
            return Err("Not answered: one was answered less than a minute ago".into());
        }
        let entries: Vec<PassedOn> = self
            .locations()
            .into_iter()
            .filter(|place| place.via.is_none() && place.key.as_deref() != Some(key))
            .filter_map(|place| {
                let time = place.at? as i64;
                let source = parse_hash(place.key.as_deref()?)?;
                let appearance = self.store.contact(place.key.as_deref()?).icon;
                (time > since).then_some(PassedOn { source, time, location: place.location, appearance })
            })
            .take(MAX_STREAM)
            .collect();
        let location = self.settings.own_location();
        if entries.is_empty() && location.is_none() {
            return Err("Not answered: no locations newer than those asked for".into());
        }
        self.answered.insert(asked, now_at);
        let to = parse_hash(key).ok_or("An LXMF address is 32 hex characters")?;
        let mode = match self.delivery_for(key) {
            DeliveryMode::Paper => DeliveryMode::Auto,
            mode => mode,
        };
        if mode == DeliveryMode::Propagated && self.propagation_node().is_none() {
            return Err("Not answered: no propagation node is selected".into());
        }
        let id = self.store.next_local_id;
        self.store.next_local_id += 1;
        let timestamp = now();
        let note = format!("Passed on {}, as a collector", locations(entries.len()));
        self.store.conversations.entry(key.to_string()).or_default().messages.push(Message {
            id: format!("local-{id}"),
            timestamp,
            state: MessageState::Sending,
            location,
            notes: vec![note.clone()],
            ..Message::default()
        });
        self.store_dirty = true;
        self.keep_conversation_selection();
        let message = lxmf::Outgoing {
            location,
            sensors: self.own_sensors(location.is_some()),
            stream: (!entries.is_empty()).then(|| lxmf::fields::stream_field(&entries)),
            appearance: self.own_appearance(),
            ..lxmf::Outgoing::text(to, String::new(), Vec::new(), mode, timestamp)
        };
        self.send(NetCommand::SendMessage { id, message: Box::new(message) });
        Ok(note)
    }

    /// Send this station's location to the collector, and ask it for
    /// everyone's, when each is due.
    pub(super) fn collector_tick(&mut self) {
        let Some(collector) = self.settings.telemetry_collector.clone() else { return };
        if self.lxmf_hash.map(hex::encode).as_deref() == Some(collector.as_str()) {
            return;
        }
        let every = |minutes: u64| Duration::from_secs(minutes * 60);
        let send = self.settings.collector_send_mins;
        let location = self.settings.own_location();
        if let (true, Some(location)) = (send > 0 && self.live.collector_sent.is_none_or(|t| t.elapsed() >= every(send)), location) {
            self.live.collector_sent = Some(Instant::now());
            // Shared with it live already: those updates go.
            if self.live_share(&collector).is_none() {
                self.send_live(&collector, location);
            }
        }
        let ask = self.settings.collector_ask_mins;
        if ask > 0 && self.live.collector_pending.is_none() && self.live.collector_asked.is_none_or(|t| t.elapsed() >= every(ask)) {
            // What's new since the newest it passed on.
            let newest = self.store.relayed.values().filter(|r| r.via == collector).map(|r| r.at as i64).max();
            let since = newest.unwrap_or(now() as i64 - FIRST_ASK);
            self.live.collector_asked = Some(Instant::now());
            match self.send_command_shown(&collector, Command::CollectorRequest(since), false) {
                Ok(id) => self.live.collector_pending = Some(id),
                Err(e) => self.log(format!("Couldn't ask the telemetry collector for locations: {e}")),
            }
        }
    }

    /// The request to the collector sent, or not: true if `id` was it.
    pub(super) fn on_collector_delivery(&mut self, id: u64, result: &Result<lxmf::Sent, String>) -> bool {
        if self.live.collector_pending != Some(id) {
            return false;
        }
        self.live.collector_pending = None;
        if let Err(e) = result {
            self.log(format!("Couldn't ask the telemetry collector for locations: {e}"));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lxmf::fields::Telemetry;
    use crate::lxmf::{Extras, InboundMessage, Location};
    use crate::store::{Contact, Store};

    const COLLECTOR: &str = "abababababababababababababababab";
    const ALICE: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    fn sent(commands: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>) -> Vec<(u64, lxmf::Outgoing)> {
        std::iter::from_fn(|| commands.try_recv().ok())
            .filter_map(|c| match c {
                NetCommand::SendMessage { id, message } => Some((id, *message)),
                _ => None,
            })
            .collect()
    }

    fn passed_on(source: u8, time: i64, latitude: f64) -> Relayed {
        let location = Some(Location::at(latitude, 10.0));
        Relayed { source: [source; 16], time, telemetry: Telemetry { location, ..Telemetry::default() }, appearance: None }
    }

    fn from(key: &str, n: u8, extras: Extras) -> InboundMessage {
        let source = parse_hash(key).unwrap();
        InboundMessage {
            id: Some([n; 32]),
            source,
            timestamp: 1_800_000_000.0 + f64::from(n),
            verified: true,
            extras,
            ..Default::default()
        }
    }

    fn setup(name: &str, settings: crate::config::Settings) -> (App, tokio::sync::mpsc::UnboundedReceiver<NetCommand>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("rettui-collector-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::default();
        store.contacts.insert(COLLECTOR.into(), Contact { trust: Trust::Trusted, ..Contact::default() });
        let (app, commands) = crate::app::test_app_with_net(&dir, settings, store);
        (app, commands, dir)
    }

    #[test]
    fn locations_a_collector_passes_on_go_on_the_map() {
        let (mut app, _commands, dir) = setup("read", crate::config::Settings::default());
        app.lxmf_hash = Some([0xee; 16]);
        let stream = vec![passed_on(0xcd, 1_800_000_000, 1.0), passed_on(0xee, 1_800_000_000, 2.0), passed_on(0xab, 1_800_000_000, 3.0)];
        app.on_message(from(COLLECTOR, 1, Extras { stream: stream.clone(), ..Extras::default() }));
        // Alice's only: not this station's own, nor the sender's.
        assert_eq!(app.store.relayed.keys().collect::<Vec<_>>(), [ALICE]);
        let note = &app.store.conversations[COLLECTOR].messages[0].notes;
        assert_eq!(note, &["Passed on 3 locations: 1 location new on the map"]);
        let alice = app.locations().into_iter().find(|p| p.key.as_deref() == Some(ALICE)).unwrap();
        assert_eq!((alice.location.latitude, alice.via.is_some()), (1.0, true));
        // An older one doesn't replace it; a newer one does.
        app.on_message(from(COLLECTOR, 2, Extras { stream: vec![passed_on(0xcd, 1_700_000_000, 5.0)], ..Extras::default() }));
        assert!(app.store.conversations[COLLECTOR].messages[1].notes[0].contains("none newer"));
        app.on_message(from(COLLECTOR, 3, Extras { stream: vec![passed_on(0xcd, 1_800_000_100, 6.0)], ..Extras::default() }));
        assert_eq!(app.store.relayed[ALICE].location.latitude, 6.0);
        // Her own, newer, comes first.
        let own =
            Extras { telemetry: Some(Telemetry { location: Some(Location::at(7.0, 7.0)), ..Telemetry::default() }), ..Extras::default() };
        app.on_message(from(ALICE, 200, own));
        let alice = app.locations().into_iter().find(|p| p.key.as_deref() == Some(ALICE)).unwrap();
        assert_eq!((alice.location.latitude, alice.via), (7.0, None));
        // From someone not trusted, nor the collector: not used.
        let stranger = "efefefefefefefefefefefefefefefef";
        app.on_message(from(stranger, 4, Extras { stream: vec![passed_on(0x12, 1_800_000_000, 1.0)], ..Extras::default() }));
        assert!(app.store.conversations[stranger].messages[0].notes[0].contains("not used"));
        assert!(!app.store.relayed.contains_key(&"12".repeat(16)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn being_a_collector_answers_with_everyones_newest() {
        let settings = crate::config::Settings { location: Some("1, 2".into()), ..crate::config::Settings::default() };
        let (mut app, mut commands, dir) = setup("serve", settings);
        let here =
            Extras { telemetry: Some(Telemetry { location: Some(Location::at(9.0, 9.0)), ..Telemetry::default() }), ..Extras::default() };
        app.on_message(from(ALICE, 5, here));
        let ask = |since: i64| Extras { commands: vec![Command::CollectorRequest(since)], ..Extras::default() };
        // Off: answered as a location request (Location requests is off too).
        app.on_message(from(COLLECTOR, 6, ask(0)));
        assert!(sent(&mut commands).is_empty());
        assert!(app.store.conversations[COLLECTOR].messages[0].notes.iter().any(|n| n.contains("Location requests is off")));
        app.settings.collector_serve = "trusted".into();
        app.on_message(from(COLLECTOR, 7, ask(0)));
        let answer = sent(&mut commands);
        assert_eq!(answer.len(), 1);
        let (_, message) = &answer[0];
        assert_eq!(message.location, Some(Location::at(1.0, 2.0)));
        let stream = rmpv::decode::read_value(&mut message.stream.as_deref().unwrap()).unwrap();
        let entries = stream.as_array().unwrap();
        assert_eq!(entries.len(), 1, "Alice's, not the asker's own");
        assert_eq!(entries[0].as_array().unwrap()[0].as_slice(), Some(&[0xcd; 16][..]));
        let notes: Vec<&String> = app.store.conversations[COLLECTOR].messages.iter().flat_map(|m| &m.notes).collect();
        assert!(notes.iter().any(|n| *n == "Passed on 1 location, as a collector"), "{notes:?}");
        assert!(notes.iter().any(|n| *n == "Answered: Passed on 1 location, as a collector"), "{notes:?}");
        // Not again within a minute; and nothing older than asked for.
        app.on_message(from(COLLECTOR, 8, ask(0)));
        assert!(sent(&mut commands).is_empty());
        app.answered.clear();
        app.on_message(from(COLLECTOR, 9, ask(1_900_000_000)));
        let only_here = sent(&mut commands);
        assert!(only_here.len() == 1 && only_here[0].1.stream.is_none());
        // Someone not trusted isn't answered.
        let stranger = "efefefefefefefefefefefefefefefef";
        app.on_message(from(stranger, 10, ask(0)));
        assert!(sent(&mut commands).is_empty());
        assert!(app.store.conversations[stranger].messages[0].notes.iter().any(|n| n.contains("trusted only")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sending_to_and_asking_a_collector_on_a_timer() {
        use crate::lxmf::{Delivered, Sent};
        use crate::net::NetEvent;
        let settings = crate::config::Settings {
            location: Some("1, 2".into()),
            telemetry_collector: Some(COLLECTOR.into()),
            collector_send_mins: 30,
            collector_ask_mins: 10,
            ..crate::config::Settings::default()
        };
        let (mut app, mut commands, dir) = setup("timer", settings);
        app.collector_tick();
        let first = sent(&mut commands);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].1.location, Some(Location::at(1.0, 2.0)));
        let Command::CollectorRequest(since) = first[1].1.commands[0] else { panic!("{:?}", first[1].1.commands) };
        assert!(since < now() as i64 - 6 * 24 * 3600, "a week back, the first time");
        // The request isn't shown; the location is, as a live update.
        assert_eq!(app.store.conversations[COLLECTOR].messages.len(), 1);
        // Not again while it's going, even when due.
        app.live.collector_asked = Some(Instant::now() - Duration::from_secs(601));
        app.collector_tick();
        assert!(sent(&mut commands).is_empty());
        app.on_net(NetEvent::Delivery { id: first[1].0, result: Ok(Sent { delivered: Delivered::Direct, hash: [1; 32] }) });
        // Next time, from the newest it passed on.
        app.store
            .relayed
            .insert(ALICE.into(), RelayedPlace { location: Location::at(0.0, 0.0), at: 1_800_000_000.0, via: COLLECTOR.into() });
        app.collector_tick();
        let again = sent(&mut commands);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].1.commands, [Command::CollectorRequest(1_800_000_000)]);
        app.on_net(NetEvent::Delivery { id: again[0].0, result: Err("no path".into()) });
        assert!(app.live.collector_pending.is_none());
        assert!(app.log.iter().any(|l| l.contains("telemetry collector") && l.contains("no path")));
        // Not to this station itself.
        app.live = Default::default();
        app.lxmf_hash = parse_hash(COLLECTOR);
        app.collector_tick();
        assert!(sent(&mut commands).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
