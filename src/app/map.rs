//! Locations: sharing one (`L` in Messages; the web UI's 📍), and a map of
//! where everyone was, as their newest location update said (`M` in
//! Messages; the web UI's map).

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, PromptKind};
use crate::lxmf::{DeliveryMode, Location};
use crate::store::Trust;

/// Someone's newest location, or this station's.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// Their address; none for this station.
    pub key: Option<String>,
    pub name: String,
    pub location: Location,
    /// When it came (Unix seconds); none for this station's.
    pub at: Option<f64>,
}

/// The map over the Messages tab: the place picked, and how far in it's
/// zoomed (none: all of them in view).
#[derive(Debug, Clone, Default)]
pub struct MapView {
    pub selected: usize,
    /// Degrees of latitude in view, around the place picked.
    pub span: Option<f64>,
    /// How many degrees of latitude all of them took, as last drawn (where
    /// zooming in starts).
    pub fitted: f64,
}

/// The most and least of the world in view, in degrees of latitude.
const WIDEST: f64 = 160.0;
const NARROWEST: f64 = 0.002;
/// The least in view around places all in one spot (a few kilometres).
const CLOSEST_FIT: f64 = 0.05;

/// What's in view, as `([west, east], [south, north])`, for a map `width`
/// by `height` cells: around the place picked if zoomed in, else all of
/// them with room around. Cells are about twice as tall as wide, and
/// longitude is narrower away from the equator, so the map isn't stretched.
pub fn bounds(places: &[Placed], view: &MapView, width: u16, height: u16) -> ([f64; 2], [f64; 2]) {
    let (center, mut lat_span) = match (view.span, places.get(view.selected)) {
        (Some(span), Some(place)) => ((place.location.latitude, place.location.longitude), span),
        _ => {
            let lats = places.iter().map(|p| p.location.latitude);
            let lons = places.iter().map(|p| p.location.longitude);
            let (south, north) = lats.fold((90.0f64, -90.0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
            let (west, east) = lons.fold((180.0f64, -180.0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
            if places.is_empty() {
                ((0.0, 0.0), WIDEST)
            } else {
                let span = ((north - south) * 1.4).max((east - west) * 1.4 * ((north + south) / 2.0).to_radians().cos() / 2.0);
                (((north + south) / 2.0, (east + west) / 2.0), span.max(CLOSEST_FIT))
            }
        }
    };
    lat_span = lat_span.min(WIDEST);
    // As wide as the cells make it, on the ground.
    let shape = f64::from(width.max(1)) / (2.0 * f64::from(height.max(1)));
    let squeeze = center.0.to_radians().cos().max(0.05);
    let lon_span = (lat_span * shape / squeeze).min(360.0);
    let south = (center.0 - lat_span / 2.0).max(-90.0);
    let north = (south + lat_span).min(90.0);
    let south = north - lat_span;
    ([center.1 - lon_span / 2.0, center.1 + lon_span / 2.0], [south, north])
}

impl App {
    /// Everyone's newest location, the newest first (this station's first
    /// of all, if set): from the newest of their messages that brought one,
    /// unless they've since said they stopped sharing it. Blocked
    /// contacts' are left out.
    pub fn locations(&self) -> Vec<Placed> {
        let stopped = |m: &crate::store::Message| m.notes.iter().any(|n| n == crate::lxmf::fields::STOPPED_SHARING);
        let mut placed: Vec<Placed> = self
            .store
            .conversations
            .iter()
            .filter(|(key, _)| self.store.contact(key).trust != Trust::Blocked)
            .filter_map(|(key, conversation)| {
                let newest = conversation.messages.iter().rev().filter(|m| m.incoming).find(|m| m.location.is_some() || stopped(m))?;
                Some(Placed {
                    key: Some(key.clone()),
                    name: self.store.display_name(key),
                    location: newest.location?,
                    at: Some(newest.timestamp),
                })
            })
            .collect();
        placed.sort_by(|a, b| b.at.unwrap_or(0.0).total_cmp(&a.at.unwrap_or(0.0)).then_with(|| a.name.cmp(&b.name)));
        if let Some(location) = self.settings.own_location() {
            placed.insert(0, Placed { key: None, name: "This station".into(), location, at: None });
        }
        placed
    }

    /// Open the map, on the open conversation's place if they've shared one.
    pub(super) fn open_map(&mut self) {
        let places = self.locations();
        if places.is_empty() {
            return self.confirm("No one has shared a location yet (L shares one)");
        }
        let open = self.active_conversation.as_deref();
        let selected = places.iter().position(|p| p.key.as_deref().is_some_and(|k| Some(k) == open)).unwrap_or(0);
        self.map = Some(MapView { selected, span: None, fitted: WIDEST });
    }

    pub(super) fn map_key(&mut self, key: KeyEvent) {
        let places = self.locations();
        let Some(map) = self.map.as_mut() else { return };
        let Some(last) = places.len().checked_sub(1) else {
            self.map = None;
            return;
        };
        map.selected = map.selected.min(last);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('M') => self.map = None,
            KeyCode::Up | KeyCode::Char('k') => map.selected = map.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => map.selected = (map.selected + 1).min(last),
            KeyCode::Home => map.selected = 0,
            KeyCode::End => map.selected = last,
            KeyCode::Char('+') | KeyCode::Char('=') => map.span = Some((map.span.unwrap_or(map.fitted) / 2.0).max(NARROWEST)),
            // Out until all of them are in view again.
            KeyCode::Char('-') => map.span = map.span.map(|s| s * 2.0).filter(|s| *s < map.fitted.min(WIDEST)),
            KeyCode::Char('0') | KeyCode::Char('a') => map.span = None,
            KeyCode::Char('o') => {
                let url = places[map.selected].location.map_url();
                self.open_url(&url);
            }
            KeyCode::Enter => {
                if let Some(key) = places[map.selected].key.clone() {
                    self.map = None;
                    self.open_conversation(key);
                }
            }
            _ => {}
        }
    }

    /// The wheel over the map zooms it.
    pub(super) fn zoom_map(&mut self, closer: bool) {
        if let Some(map) = self.map.as_mut() {
            map.span = if closer {
                Some((map.span.unwrap_or(map.fitted) / 2.0).max(NARROWEST))
            } else {
                map.span.map(|s| s * 2.0).filter(|s| *s < map.fitted.min(WIDEST))
            };
        }
    }

    /// Ask where to say you are (`L`): this station's location to start;
    /// or, while sharing live with them, whether to stop.
    pub(super) fn open_share_location(&mut self) {
        let Some(key) = self.active_conversation.clone() else { return };
        if self.live_share(&key).is_some() {
            let question = format!("Stop sharing your location live with {}? (y/n)", self.store.display_name(&key));
            return self.open_prompt(PromptKind::ConfirmStopLive(key), &question, "");
        }
        let here = self.settings.location.clone().unwrap_or_default();
        let title = "Share a location (latitude, longitude), or this station's live (live 15m, 1h, 8h or on)";
        self.open_prompt(PromptKind::ShareLocation(key), title, &here);
    }

    /// Share the location typed with `key`, the way their messages go; or,
    /// for `live …`, this station's, live.
    pub(super) fn submit_share_location(&mut self, key: String, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(how_long) = text.trim().strip_prefix("live") {
            let minutes = match how_long.trim().to_lowercase().as_str() {
                "on" | "" => Some(0),
                h if h.ends_with('h') => h.trim_end_matches('h').trim().parse::<u64>().ok().map(|h| h * 60),
                m => m.trim_end_matches('m').trim().parse::<u64>().ok().filter(|m| *m > 0),
            };
            let Some(minutes) = minutes else {
                return self.warn("How long to share live: live 15m, live 1h, live 8h, or live on (until stopped)");
            };
            return match self.start_live(&key, minutes, super::live::Source::Station) {
                Ok(doing) => self.confirm(doing),
                Err(e) => self.warn(e),
            };
        }
        let Some(location) = Location::parse(text) else {
            return self.warn("A location is latitude, longitude in degrees, as 51.5074, -0.1278");
        };
        let mode = if key == self.active_conversation.clone().unwrap_or_default() { self.delivery_mode } else { self.delivery_for(&key) };
        if mode == DeliveryMode::Paper {
            return self.warn("A paper message carries text only: pick another delivery (d) to share a location");
        }
        let name = self.store.display_name(&key);
        match self.share_location(key, location, String::new(), mode, None) {
            Ok(_) => self.confirm(format!("Shared {} with {name}", location.label())),
            Err(e) => self.warn(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::store::{Conversation, Message, MessageState, Store};

    fn place(lat: f64, lon: f64) -> Placed {
        Placed { key: None, name: String::new(), location: Location::at(lat, lon), at: None }
    }

    #[test]
    fn all_in_view_unstretched_or_zoomed_on_one() {
        let places = [place(51.5, -0.12), place(48.85, 2.35)];
        let ([west, east], [south, north]) = bounds(&places, &MapView::default(), 80, 20);
        for p in &places {
            assert!((west..=east).contains(&p.location.longitude) && (south..=north).contains(&p.location.latitude));
        }
        // As wide on the ground as the cells are: 80 by 20 cells is twice
        // as wide as tall.
        let ground = (east - west) * 50.0f64.to_radians().cos() / (north - south);
        assert!((ground - 2.0).abs() < 0.1, "{ground}");
        // Zoomed in, around the one picked.
        let view = MapView { selected: 1, span: Some(0.01), fitted: 5.0 };
        let (lons, lats) = bounds(&places, &view, 80, 20);
        assert!(((lats[0] + lats[1]) / 2.0 - 48.85).abs() < 1e-9 && ((lons[0] + lons[1]) / 2.0 - 2.35).abs() < 1e-9);
        assert!((lats[1] - lats[0] - 0.01).abs() < 1e-9);
        // One place alone: a few kilometres around it, not a point.
        let (_, [south, north]) = bounds(&places[..1], &MapView::default(), 80, 20);
        assert!(north - south >= CLOSEST_FIT - 1e-9);
        // Near a pole, it stays on the globe.
        let (_, [south, north]) = bounds(&[place(89.9, 0.0)], &MapView { span: Some(10.0), ..MapView::default() }, 80, 20);
        assert!(north <= 90.0 && (north - south - 10.0).abs() < 1e-9);
    }

    #[test]
    fn newest_locations_on_the_map_and_its_keys() {
        let dir = std::env::temp_dir().join(format!("rettui-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (alice, bob, carol) = ("aa".repeat(16), "bb".repeat(16), "cc".repeat(16));
        let from = |at: f64, location: Option<Location>, notes: Vec<String>| Message {
            id: format!("in-{at}"),
            incoming: true,
            timestamp: at,
            state: MessageState::Received { verified: true },
            location,
            notes,
            ..Message::default()
        };
        let mut store = Store::default();
        // Alice moved; Bob stopped sharing; Carol never shared.
        let alice_messages = vec![from(1.0, Some(Location::at(1.0, 1.0)), vec![]), from(3.0, Some(Location::at(2.0, 2.0)), vec![])];
        store.conversations.insert(alice.clone(), Conversation { messages: alice_messages, ..Conversation::default() });
        let stopped = vec![crate::lxmf::fields::STOPPED_SHARING.to_string()];
        let bob_messages = vec![from(2.0, Some(Location::at(5.0, 5.0)), vec![]), from(4.0, None, stopped)];
        store.conversations.insert(bob.clone(), Conversation { messages: bob_messages, ..Conversation::default() });
        let carol_messages = vec![Message { content: "hi".into(), ..from(5.0, None, vec![]) }];
        store.conversations.insert(carol.clone(), Conversation { messages: carol_messages, ..Conversation::default() });
        let settings = crate::config::Settings { location: Some("10, 20".into()), ..crate::config::Settings::default() };
        let mut app = crate::app::test_app(&dir, settings, store);
        let places = app.locations();
        assert_eq!(places.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["This station", &app.store.display_name(&alice)]);
        assert_eq!(places[1].location, Location::at(2.0, 2.0));
        assert_eq!(places[1].at, Some(3.0));

        // M opens it, on the open conversation's place; keys go to it.
        app.tab = crate::app::Tab::Messages;
        app.active_conversation = Some(alice.clone());
        let press = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        press(&mut app, KeyCode::Char('M'));
        assert_eq!(app.map.as_ref().map(|m| m.selected), Some(1));
        press(&mut app, KeyCode::Up);
        assert_eq!(app.map.as_ref().unwrap().selected, 0);
        press(&mut app, KeyCode::Char('+'));
        assert!(app.map.as_ref().unwrap().span.is_some());
        press(&mut app, KeyCode::Char('0'));
        assert!(app.map.as_ref().unwrap().span.is_none());
        // Enter on someone opens your conversation with them.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(app.map.is_none() && app.active_conversation.as_deref() == Some(alice.as_str()));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('M'));
        press(&mut app, KeyCode::Char('q'));
        assert!(app.map.is_none() && !app.should_quit);

        // L shares a location, starting from this station's.
        app.composing = false;
        app.active_conversation = Some(carol.clone());
        press(&mut app, KeyCode::Char('L'));
        assert_eq!(app.prompt.as_ref().map(|p| p.input.text().to_string()).as_deref(), Some("10, 20"));
        press(&mut app, KeyCode::Enter);
        let sent = app.store.conversations[&carol].messages.last().unwrap();
        assert!(!sent.incoming && sent.location == Some(Location::at(10.0, 20.0)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
