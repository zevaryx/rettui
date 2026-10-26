//! Where people have been: each location someone sends, or a collector
//! passes on, is kept on their trail (the newest [`MAX_POINTS`], for
//! [`KEEP_FOR`]), which the map draws as a line to where they are now.
//! Deleting someone's locations takes them, and their place, off the map.

use super::{App, now};
use crate::lxmf::Location;
use crate::store::{TrailPoint, Trust};

/// Places kept on each person's trail.
pub const MAX_POINTS: usize = 100;
/// How long a place is kept on a trail (seconds).
pub const KEEP_FOR: f64 = 30.0 * 86_400.0;
/// Most people with a trail kept (a collector can pass on hundreds).
pub const MAX_TRAILED: usize = 200;
/// Places this close (in degrees, about a metre) are the same place.
const SAME_PLACE: f64 = 0.000_01;

impl App {
    /// Put a place `key` was at `at` (Unix seconds) on their trail.
    pub(super) fn keep_trail(&mut self, key: &str, location: &Location, at: f64) {
        if self.store.contact(key).trust == Trust::Blocked || self.store.locations_forgotten.get(key).is_some_and(|f| at <= *f) {
            return;
        }
        let since = now() - KEEP_FOR;
        if at < since || !location.latitude.is_finite() || !location.longitude.is_finite() {
            return;
        }
        let points = self.store.trails.entry(key.to_string()).or_default();
        if points.iter().any(|p| p.at == at) {
            return;
        }
        let point = TrailPoint { latitude: location.latitude, longitude: location.longitude, at };
        let i = points.partition_point(|p| p.at < at);
        // Still in the same place as just before: only the time moves on.
        let same = |p: &TrailPoint| (p.latitude - point.latitude).abs() < SAME_PLACE && (p.longitude - point.longitude).abs() < SAME_PLACE;
        if i == points.len() && points.last().is_some_and(same) {
            points[i - 1].at = at;
        } else {
            points.insert(i, point);
        }
        points.retain(|p| p.at >= since);
        let extra = points.len().saturating_sub(MAX_POINTS);
        points.drain(..extra);
        // Too many people: the one heard from longest ago goes.
        while self.store.trails.len() > MAX_TRAILED {
            let newest = |points: &Vec<TrailPoint>| points.last().map_or(0.0, |p| p.at);
            let oldest = self.store.trails.iter().min_by(|a, b| newest(a.1).total_cmp(&newest(b.1))).map(|(k, _)| k.clone());
            let Some(oldest) = oldest else { break };
            self.store.trails.remove(&oldest);
        }
        self.store_dirty = true;
    }

    /// Pass a place on to the MQTT server, if there is one (as Sideband's
    /// *Telemetry to MQTT*). Not anyone's who is blocked.
    pub(super) fn export_telemetry(&mut self, key: &str, location: &Location, at: f64) {
        let Some(server) = self.settings.telemetry_mqtt.as_deref().and_then(|s| crate::net::mqtt::Server::parse(s).ok()) else { return };
        if self.store.contact(key).trust == Trust::Blocked {
            return;
        }
        let topics = crate::net::mqtt::topics(key, location, at);
        self.send(crate::net::NetCommand::Mqtt { server, topics });
    }

    /// Where `key` has been, oldest first.
    pub fn trail(&self, key: &str) -> &[TrailPoint] {
        self.store.trails.get(key).map_or(&[], Vec::as_slice)
    }

    /// Delete what's known of where `key` has been: their trail, what
    /// collectors passed on, and (from the map) the locations their
    /// messages brought, which stay in the conversation.
    pub fn forget_locations(&mut self, key: &str) {
        self.store.trails.remove(key);
        self.store.relayed.remove(key);
        self.store.locations_forgotten.insert(key.to_string(), now());
        self.store_dirty = true;
    }

    /// Whether a location `key` sent at `at` was deleted.
    pub(super) fn location_forgotten(&self, key: &str, at: f64) -> bool {
        self.store.locations_forgotten.get(key).is_some_and(|f| at <= *f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lxmf::InboundMessage;
    use crate::lxmf::fields::{Extras, Telemetry};
    use crate::store::{Contact, Store};

    const ALICE: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    fn from_alice(at: f64, latitude: f64) -> InboundMessage {
        let telemetry = Telemetry { location: Some(Location::at(latitude, 10.0)), ..Telemetry::default() };
        InboundMessage {
            id: Some([(at as u64 % 251) as u8; 32]),
            source: [0xcd; 16],
            timestamp: at,
            verified: true,
            extras: Extras { telemetry: Some(telemetry), ..Extras::default() },
            ..Default::default()
        }
    }

    #[test]
    fn where_someone_has_been_is_kept_on_their_trail() {
        let dir = std::env::temp_dir().join(format!("rettui-trails-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), Store::default());
        let now = now();
        // Each update replaces the one before in the conversation, but
        // stays on the trail, in time order (one downloaded late too).
        app.on_message(from_alice(now - 300.0, 1.0));
        app.on_message(from_alice(now - 100.0, 3.0));
        app.on_message(from_alice(now - 200.0, 2.0));
        let lats: Vec<f64> = app.trail(ALICE).iter().map(|p| p.latitude).collect();
        assert_eq!(lats, [1.0, 2.0, 3.0]);
        assert_eq!(app.locations().iter().find(|p| p.key.as_deref() == Some(ALICE)).unwrap().location.latitude, 3.0);
        // Still in the same place: the time moves on, no new point.
        app.on_message(from_alice(now - 50.0, 3.0));
        assert_eq!(app.trail(ALICE).len(), 3);
        assert_eq!(app.trail(ALICE)[2].at, now - 50.0);
        // Too old, or the same time again: not kept.
        app.keep_trail(ALICE, &Location::at(9.0, 9.0), now - KEEP_FOR - 10.0);
        app.keep_trail(ALICE, &Location::at(9.0, 9.0), now - 50.0);
        assert_eq!(app.trail(ALICE).len(), 3);
        // Only the newest are kept.
        for i in 0..MAX_POINTS {
            app.keep_trail(ALICE, &Location::at(10.0 + i as f64 / 100.0, 10.0), now + i as f64);
        }
        assert_eq!(app.trail(ALICE).len(), MAX_POINTS);
        assert_eq!(app.trail(ALICE)[0].latitude, 10.0);
        // And for so many people.
        for i in 0..MAX_TRAILED as u32 + 5 {
            app.keep_trail(&format!("{i:032x}"), &Location::at(1.0, 1.0), now + f64::from(i));
        }
        assert_eq!(app.store.trails.len(), MAX_TRAILED);
        assert!(!app.store.trails.contains_key(&format!("{:032x}", 0)));

        // Deleted: off the map, trail and all, though the message stays;
        // newer ones are kept again.
        app.forget_locations(ALICE);
        assert!(app.trail(ALICE).is_empty() && !app.locations().iter().any(|p| p.key.as_deref() == Some(ALICE)));
        assert!(app.store.conversations[ALICE].messages.iter().any(|m| m.location.is_some()));
        app.keep_trail(ALICE, &Location::at(5.0, 5.0), now - 10.0);
        assert!(app.trail(ALICE).is_empty());
        app.on_message(from_alice(now + 1000.0, 4.0));
        assert_eq!(app.trail(ALICE).len(), 1);
        assert!(app.locations().iter().any(|p| p.key.as_deref() == Some(ALICE)));
        // Blocked: not kept.
        let bob = "abababababababababababababababab";
        app.store.contacts.insert(bob.into(), Contact { trust: Trust::Blocked, ..Contact::default() });
        app.keep_trail(bob, &Location::at(1.0, 1.0), now);
        assert!(app.trail(bob).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn places_passed_on_to_mqtt_as_sideband_does() {
        use crate::net::NetCommand;
        let dir = std::env::temp_dir().join(format!("rettui-mqtt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = crate::config::Settings { telemetry_mqtt: Some("mqtt://ann:pw@broker.local:1884".into()), ..Default::default() };
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, settings, Store::default());
        let mut published = || {
            let mut topics = Vec::new();
            while let Ok(command) = net.try_recv() {
                if let NetCommand::Mqtt { server, topics: these } = command {
                    assert_eq!((server.host.as_str(), server.port, server.user.as_deref()), ("broker.local", 1884, Some("ann")));
                    topics.extend(these);
                }
            }
            topics
        };
        app.on_message(from_alice(now() - 5.0, 51.5));
        let topics = published();
        let latitude = format!("lxmf/telemetry/{ALICE}/location/latitude");
        assert!(topics.iter().any(|(t, v)| *t == latitude && v == "51.5"), "{topics:?}");
        // Not anyone's who is blocked, nor with no server.
        app.store.contacts.insert(ALICE.into(), Contact { trust: Trust::Blocked, ..Contact::default() });
        app.on_message(from_alice(now() - 4.0, 52.0));
        assert!(published().is_empty());
        app.store.contacts.clear();
        app.update_settings(&[("telemetry_mqtt", "")]).unwrap();
        app.on_message(from_alice(now() - 3.0, 53.0));
        assert!(published().is_empty());
        assert!(app.update_settings(&[("telemetry_mqtt", "mqtt://host:port")]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
