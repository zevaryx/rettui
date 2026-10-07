//! Sharing a location live, as Columba does: updates to someone for a while
//! (or until stopped), then a message saying it's stopped. The web UI
//! shares where its device is, sending its position as it moves; the
//! terminal UI shares this station's Location. Each update replaces the
//! one before it in the conversation, so a live share is one message there.
//! Shares last only while rettui runs.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::{App, now};
use crate::lxmf::{self, DeliveryMode, Location};
use crate::net::{NetCommand, parse_hash};

/// The least time between two updates of a moving device's position.
const EVERY: Duration = Duration::from_secs(60);
/// A device's position older than this isn't sent (its page has closed).
const STALE: Duration = Duration::from_secs(300);
/// How often a station's (unmoving) location is sent again.
const STATION_EVERY: Duration = Duration::from_secs(300);
/// The longest share asked for, in minutes (a week); 0 is until stopped.
const MAX_MINUTES: u64 = 7 * 24 * 60;

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

/// Live shares by conversation, and the newest device position posted.
#[derive(Debug, Default)]
pub struct Live {
    pub shares: BTreeMap<String, Share>,
    position: Option<(Location, Instant)>,
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

    /// Send what's due: a share that's ended stops; a station's location
    /// goes every few minutes; a device's as it moves, at most once a
    /// minute.
    pub(super) fn live_tick(&mut self) {
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

    /// An update: shown in place of the one before it.
    fn send_live(&mut self, key: &str, location: Location) {
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
}
