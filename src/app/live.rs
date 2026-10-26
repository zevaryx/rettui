//! Sharing a location live, as Columba does: updates to someone for a while
//! (or until stopped), then a message saying it's stopped. The web UI
//! shares where its device is, sending its position as it moves; the
//! terminal UI shares this station's Location. Each update replaces the
//! one before it in the conversation, so a live share is one message there.
//! Shares last only while rettui runs.
//!
//! Also here, as Sideband does them: this station's location sent to
//! everyone trusted every few minutes (the `telemetry_every_mins` setting),
//! and tracking someone, by asking where they are every few minutes. Their
//! answers replace each other in the conversation, as location updates do.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::{App, now};
use crate::lxmf::fields::Command;
use crate::lxmf::{self, DeliveryMode, Location};
use crate::net::{NetCommand, parse_hash};
use crate::store::Trust;

/// The least time between two updates of a moving device's position.
const EVERY: Duration = Duration::from_secs(60);
/// A device's position older than this isn't sent (its page has closed).
const STALE: Duration = Duration::from_secs(300);
/// How often a station's (unmoving) location is sent again.
const STATION_EVERY: Duration = Duration::from_secs(300);
/// The longest share asked for, in minutes (a week); 0 is until stopped.
const MAX_MINUTES: u64 = 7 * 24 * 60;
/// The most minutes between a tracked contact's requests (a day).
const MAX_TRACK_MINUTES: u64 = 24 * 60;

/// Where a live share's location comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// This station's Location setting.
    Station,
    /// A browser's device, as it posts its position.
    Device,
}

impl Source {
    pub fn key(self) -> &'static str {
        match self {
            Source::Station => "station",
            Source::Device => "device",
        }
    }
}

/// A live share: until when (Unix seconds; none until stopped), from where,
/// and when an update last went.
#[derive(Debug, Clone)]
pub struct Share {
    pub until: Option<f64>,
    pub source: Source,
    sent: Option<Instant>,
}

/// Someone tracked: asked where they are every `every` minutes, and the
/// request still going, if one is (no more go until it has).
#[derive(Debug, Clone)]
pub struct Track {
    pub every: u64,
    asked: Option<Instant>,
    pending: Option<u64>,
}

/// Live shares by conversation, the newest device position posted, when
/// this station's location last went to everyone trusted, and who's
/// tracked.
#[derive(Debug, Default)]
pub struct Live {
    pub shares: BTreeMap<String, Share>,
    position: Option<(Location, Instant)>,
    sent_trusted: Option<Instant>,
    pub tracks: BTreeMap<String, Track>,
    /// The telemetry collector: when this station's location last went to
    /// it, when it was last asked for everyone's, and the request going.
    pub(super) collector_sent: Option<Instant>,
    pub(super) collector_asked: Option<Instant>,
    pub(super) collector_pending: Option<u64>,
}

impl App {
    /// Start sharing a location live with `key` for `minutes` (0: until
    /// stopped), from `source`; what's happening, to say.
    pub fn start_live(&mut self, key: &str, minutes: u64, source: Source) -> Result<String, String> {
        let key = hex::encode(parse_hash(key).ok_or("An LXMF address is 32 hex characters")?);
        if minutes > MAX_MINUTES {
            return Err("A live share lasts a week at most".into());
        }
        if source == Source::Station && self.settings.own_location().is_none() {
            return Err("Set this station's Location (in Status) to share it live".into());
        }
        let until = (minutes > 0).then(|| now() + minutes as f64 * 60.0);
        self.live.shares.insert(key.clone(), Share { until, source, sent: None });
        self.live_tick();
        let name = self.store.display_name(&key);
        Ok(match until {
            Some(until) => format!("Sharing your location live with {name} until {}", crate::clock::when(until)),
            None => format!("Sharing your location live with {name} until you stop"),
        })
    }

    /// Stop sharing live with `key`, and tell them (as Columba does).
    pub fn stop_live(&mut self, key: &str) -> Result<(), String> {
        let key = hex::encode(parse_hash(key).ok_or("An LXMF address is 32 hex characters")?);
        if self.live.shares.remove(&key).is_none() {
            return Err("You aren't sharing your location live with them".into());
        }
        self.end_live(&key, "Live sharing stopped");
        Ok(())
    }

    /// A device's position, from a browser sharing it live.
    pub fn device_position(&mut self, location: Location) {
        self.live.position = Some((location, Instant::now()));
        self.live_tick();
    }

    /// The live share with `key`, if there's one.
    pub fn live_share(&self, key: &str) -> Option<&Share> {
        self.live.shares.get(key)
    }

    /// Track `key`: ask where they are every `minutes`; what's happening,
    /// to say.
    pub fn start_track(&mut self, key: &str, minutes: u64) -> Result<String, String> {
        let key = hex::encode(parse_hash(key).ok_or("An LXMF address is 32 hex characters")?);
        if !(1..=MAX_TRACK_MINUTES).contains(&minutes) {
            return Err(format!("Ask every 1 to {MAX_TRACK_MINUTES} minutes (a day)"));
        }
        if self.store.contact(&key).trust == Trust::Blocked {
            return Err("They're blocked: unblock them to track them".into());
        }
        let track = self.live.tracks.entry(key.clone()).or_insert(Track { every: minutes, asked: None, pending: None });
        track.every = minutes;
        self.live_tick();
        let name = self.store.display_name(&key);
        Ok(format!("Asking {name} where they are every {minutes} min, while rettui runs: their answers go on the map"))
    }

    /// Stop tracking `key`.
    pub fn stop_track(&mut self, key: &str) -> Result<(), String> {
        let key = hex::encode(parse_hash(key).ok_or("An LXMF address is 32 hex characters")?);
        self.live.tracks.remove(&key).map(|_| ()).ok_or_else(|| "You aren't tracking them".into())
    }

    /// Whether `key` is tracked, and how often.
    pub fn track(&self, key: &str) -> Option<&Track> {
        self.live.tracks.get(key)
    }

    /// A tracked contact's request sent, or not: true if `id` was one.
    pub(super) fn on_track_delivery(&mut self, id: u64, result: &Result<lxmf::Sent, String>) -> bool {
        let Some((key, track)) = self.live.tracks.iter_mut().find(|(_, t)| t.pending == Some(id)) else { return false };
        track.pending = None;
        let key = key.clone();
        if let Err(e) = result {
            let name = self.store.display_name(&key);
            self.log(format!("Couldn't ask {name} where they are: {e}"));
        }
        true
    }

    /// Requests in progress belong to the old stack: the next goes when due.
    pub(super) fn tracks_restarting(&mut self) {
        for track in self.live.tracks.values_mut() {
            track.pending = None;
        }
        self.live.collector_pending = None;
    }

    /// Send what's due: a share that's ended stops; a station's location
    /// goes every few minutes; a device's as it moves, at most once a
    /// minute. Then this station's location to everyone trusted, and
    /// tracked contacts' requests.
    pub(super) fn live_tick(&mut self) {
        self.trusted_tick();
        self.track_tick();
        self.collector_tick();
        if self.live.shares.is_empty() {
            return;
        }
        let ended: Vec<String> =
            self.live.shares.iter().filter(|(_, s)| s.until.is_some_and(|u| now() >= u)).map(|(k, _)| k.clone()).collect();
        for key in ended {
            self.live.shares.remove(&key);
            self.end_live(&key, "Live sharing ended");
        }
        let due: Vec<(String, Location)> = self
            .live
            .shares
            .iter()
            .filter_map(|(key, share)| {
                let location = match share.source {
                    Source::Station if share.sent.is_none_or(|t| t.elapsed() >= STATION_EVERY) => self.settings.own_location()?,
                    Source::Device => {
                        let (location, at) = self.live.position?;
                        // Not one from long ago (an earlier share's).
                        let recent = at.elapsed() < STALE;
                        let fresh = recent && share.sent.is_none_or(|t| at > t && t.elapsed() >= EVERY);
                        if !fresh {
                            return None;
                        }
                        location
                    }
                    Source::Station => return None,
                };
                Some((key.clone(), location))
            })
            .collect();
        for (key, location) in due {
            if let Some(share) = self.live.shares.get_mut(&key) {
                share.sent = Some(Instant::now());
            }
            self.send_live(&key, location);
        }
    }

    /// This station's location to every contact trusted (but those it's
    /// shared with live already), every `telemetry_every_mins`.
    fn trusted_tick(&mut self) {
        let minutes = self.settings.telemetry_every_mins;
        if minutes == 0 || self.live.sent_trusted.is_some_and(|t| t.elapsed() < Duration::from_secs(minutes * 60)) {
            return;
        }
        let Some(location) = self.settings.own_location() else { return };
        self.live.sent_trusted = Some(Instant::now());
        if let Some(own) = self.lxmf_hash {
            self.export_telemetry(&hex::encode(own), &location, super::now());
        }
        let trusted: Vec<String> = self
            .store
            .contacts
            .iter()
            .filter(|(key, contact)| contact.trust == Trust::Trusted && !self.live.shares.contains_key(*key))
            // The collector gets it on its own timer, if it has one.
            .filter(|(key, _)| self.settings.collector_send_mins == 0 || self.settings.telemetry_collector.as_ref() != Some(*key))
            .map(|(key, _)| key.clone())
            .collect();
        for key in trusted {
            self.send_live(&key, location);
        }
    }

    /// Ask tracked contacts where they are, when it's time and the last
    /// request has gone.
    fn track_tick(&mut self) {
        let due: Vec<String> = self
            .live
            .tracks
            .iter()
            .filter(|(_, t)| t.pending.is_none() && t.asked.is_none_or(|a| a.elapsed() >= Duration::from_secs(t.every * 60)))
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            let sent = self.send_command_shown(&key, Command::TelemetryRequest, false);
            if let Err(e) = &sent {
                let name = self.store.display_name(&key);
                self.log(format!("Couldn't ask {name} where they are: {e}"));
            }
            if let Some(track) = self.live.tracks.get_mut(&key) {
                track.asked = Some(Instant::now());
                track.pending = sent.ok();
            }
        }
    }

    /// An update: shown in place of the one before it.
    pub(super) fn send_live(&mut self, key: &str, location: Location) {
        let mode = match self.delivery_for(key) {
            DeliveryMode::Paper => DeliveryMode::Auto,
            mode => mode,
        };
        match self.share_location(key.to_string(), location, String::new(), mode, None) {
            Ok(id) => {
                let id = format!("local-{id}");
                if let Some(conversation) = self.store.conversations.get_mut(key) {
                    conversation.messages.retain(|m| !m.live || m.id == id);
                    if let Some(message) = conversation.messages.iter_mut().find(|m| m.id == id) {
                        message.live = true;
                    }
                }
            }
            Err(e) => self.log(format!("Couldn't send a live location update: {e}")),
        }
    }

    /// Tell `key` the share has stopped, and say so on its last update.
    fn end_live(&mut self, key: &str, note: &str) {
        if let Some(conversation) = self.store.conversations.get_mut(key) {
            for message in conversation.messages.iter_mut().filter(|m| m.live) {
                message.live = false;
                message.notes.push(note.to_string());
            }
            self.store_dirty = true;
        }
        let Some(to) = parse_hash(key) else { return };
        let mode = match self.delivery_for(key) {
            DeliveryMode::Paper => DeliveryMode::Auto,
            mode => mode,
        };
        if mode == DeliveryMode::Propagated && self.propagation_node().is_none() {
            return self.log("Couldn't say the live share stopped: no propagation node is selected");
        }
        let id = self.store.next_local_id;
        self.store.next_local_id += 1;
        let message = lxmf::Outgoing {
            cease: true,
            appearance: self.own_appearance(),
            ..lxmf::Outgoing::text(to, String::new(), Vec::new(), mode, now())
        };
        self.send(NetCommand::SendMessage { id, message: Box::new(message) });
        let name = self.store.display_name(key);
        self.log(format!("{note}: {name}"));
    }
}

/// A share as the web UI shows it.
pub fn describe(key: &str, share: &Share) -> serde_json::Value {
    serde_json::json!({ "key": key, "until": share.until, "source": share.source.key() })
}

/// What's said of tracking someone in a conversation's header.
pub fn track_label(track: &Track) -> String {
    format!("📡 asking every {} min", track.every)
}

/// What's said of a live share in a conversation's header.
pub fn label(share: &Share) -> String {
    match share.until {
        Some(until) => format!("📍 live until {}", crate::clock::short(until)),
        None => "📍 live".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Message;

    fn sent(commands: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>) -> Vec<lxmf::Outgoing> {
        std::iter::from_fn(|| commands.try_recv().ok())
            .filter_map(|c| match c {
                NetCommand::SendMessage { message, .. } => Some(*message),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_device_shared_live_one_update_at_a_time_then_stopped() {
        let dir = std::env::temp_dir().join(format!("rettui-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut commands) =
            crate::app::test_app_with_net(&dir, crate::config::Settings::default(), crate::store::Store::default());
        let key = "ab".repeat(16);
        assert!(app.start_live(&key, 15, Source::Device).unwrap().contains("until"));
        // Nothing until the device says where it is.
        assert!(sent(&mut commands).is_empty());
        app.device_position(Location::at(51.5, -0.12));
        let first = sent(&mut commands);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].location, Some(Location::at(51.5, -0.12)));
        // Moving again at once: not yet (a minute apart at least).
        app.device_position(Location::at(51.6, -0.12));
        assert!(sent(&mut commands).is_empty());
        // A minute on: the next goes, in place of the one before.
        app.live.shares.get_mut(&key).unwrap().sent = Some(Instant::now() - EVERY - Duration::from_secs(1));
        app.device_position(Location::at(51.7, -0.12));
        assert_eq!(sent(&mut commands).len(), 1);
        let live: Vec<&Message> = app.store.conversations[&key].messages.iter().filter(|m| m.live).collect();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].location, Some(Location::at(51.7, -0.12)));
        assert_eq!(app.store.conversations[&key].messages.len(), 1);
        // Stopped: they're told, and the last update says so.
        app.stop_live(&key).unwrap();
        let stop = sent(&mut commands);
        assert!(stop.len() == 1 && stop[0].cease && stop[0].location.is_none());
        let last = &app.store.conversations[&key].messages[0];
        assert!(!last.live && last.notes == ["Live sharing stopped"]);
        assert!(app.stop_live(&key).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_station_shared_live_until_it_ends() {
        let dir = std::env::temp_dir().join(format!("rettui-live-station-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut commands) =
            crate::app::test_app_with_net(&dir, crate::config::Settings::default(), crate::store::Store::default());
        let key = "cd".repeat(16);
        // No Location set: nothing to share.
        assert!(app.start_live(&key, 60, Source::Station).unwrap_err().contains("Location"));
        app.settings.location = Some("10, 20".into());
        app.start_live(&key, 60, Source::Station).unwrap();
        assert_eq!(sent(&mut commands)[0].location, Some(Location::at(10.0, 20.0)));
        // Not again for a few minutes.
        app.live_tick();
        assert!(sent(&mut commands).is_empty());
        // Ended: they're told it stopped, and it's gone.
        app.live.shares.get_mut(&key).unwrap().until = Some(now() - 1.0);
        app.live_tick();
        assert!(sent(&mut commands).iter().any(|m| m.cease));
        assert!(app.live_share(&key).is_none());
        assert_eq!(app.store.conversations[&key].messages[0].notes, ["Live sharing ended"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn location_sent_to_everyone_trusted_on_a_timer() {
        use crate::store::Contact;
        let dir = std::env::temp_dir().join(format!("rettui-live-trusted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (trusted, other, shared) = ("ab".repeat(16), "cd".repeat(16), "ef".repeat(16));
        let mut store = crate::store::Store::default();
        for (key, trust) in [(&trusted, Trust::Trusted), (&other, Trust::Untrusted), (&shared, Trust::Trusted)] {
            store.contacts.insert(key.clone(), Contact { trust, ..Contact::default() });
        }
        let settings = crate::config::Settings { telemetry_every_mins: 10, ..crate::config::Settings::default() };
        let (mut app, mut commands) = crate::app::test_app_with_net(&dir, settings, store);
        // No Location: nothing to send.
        app.live_tick();
        assert!(sent(&mut commands).is_empty());
        app.settings.location = Some("10, 20".into());
        // Only to those trusted, and once to one it's shared with live.
        app.start_live(&shared, 0, Source::Station).unwrap();
        let first: Vec<(String, Option<Location>)> = sent(&mut commands).iter().map(|m| (hex::encode(m.to), m.location)).collect();
        let at = Some(Location::at(10.0, 20.0));
        assert_eq!(first, [(trusted.clone(), at), (shared.clone(), at)]);
        app.live_tick();
        assert!(sent(&mut commands).is_empty(), "not again for ten minutes");
        // Ten minutes on: again, in place of the one before.
        app.live.sent_trusted = Some(Instant::now() - Duration::from_secs(601));
        app.live_tick();
        assert_eq!(sent(&mut commands).len(), 1);
        let messages = &app.store.conversations[&trusted].messages;
        assert!(messages.len() == 1 && messages[0].live);
        // Off: no more.
        app.settings.telemetry_every_mins = 0;
        app.live.sent_trusted = None;
        app.live_tick();
        assert!(sent(&mut commands).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tracking_someone_asks_where_they_are_every_few_minutes() {
        use crate::lxmf::{Delivered, Sent};
        use crate::net::NetEvent;
        let dir = std::env::temp_dir().join(format!("rettui-live-track-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut commands) =
            crate::app::test_app_with_net(&dir, crate::config::Settings::default(), crate::store::Store::default());
        let key = "ab".repeat(16);
        assert!(app.start_track(&key, 0).is_err());
        assert!(app.start_track(&key, MAX_TRACK_MINUTES + 1).is_err());
        assert!(app.start_track("nobody", 5).is_err());
        assert!(app.start_track(&key, 5).unwrap().contains("every 5 min"));
        let asked = sent(&mut commands);
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].commands, [Command::TelemetryRequest]);
        // Requests aren't shown: only the answers are.
        assert!(app.store.conversations.get(&key).is_none_or(|c| c.messages.is_empty()));
        assert_eq!(track_label(app.track(&key).unwrap()), "📡 asking every 5 min");
        // Not again while that one is going, even when it's time.
        let id = app.live.tracks[&key].pending.unwrap();
        app.live.tracks.get_mut(&key).unwrap().asked = Some(Instant::now() - Duration::from_secs(301));
        app.live_tick();
        assert!(sent(&mut commands).is_empty());
        // It failed (said in the log); the next goes when it's time.
        app.on_net(NetEvent::Delivery { id, result: Err("no path".into()) });
        assert!(app.log.iter().any(|l| l.contains("Couldn't ask") && l.contains("no path")));
        app.live_tick();
        let again = sent(&mut commands);
        assert_eq!(again.len(), 1);
        let id = app.live.tracks[&key].pending.unwrap();
        app.on_net(NetEvent::Delivery { id, result: Ok(Sent { delivered: Delivered::Direct, hash: [1; 32] }) });
        assert!(app.live.tracks[&key].pending.is_none());
        app.live_tick();
        assert!(sent(&mut commands).is_empty(), "not for five minutes");
        // A restart: what was going is forgotten, so tracking goes on.
        app.live.tracks.get_mut(&key).unwrap().pending = Some(99);
        app.tracks_restarting();
        assert!(app.live.tracks[&key].pending.is_none());
        app.stop_track(&key).unwrap();
        assert!(app.stop_track(&key).is_err());
        assert!(app.track(&key).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
