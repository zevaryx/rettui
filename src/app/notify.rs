//! Notifications: new messages, mentions and whispers the user isn't
//! looking at. The app queues them; the terminal UI shows them with the
//! desktop's own notifications, the web UI with the browser's.

use serde::Serialize;

use super::App;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// What it's about: opening it goes there, and a newer notification
    /// about the same thing replaces it.
    pub target: Target,
    /// What can be done from it, besides opening it (see [`App::respond`]).
    /// Left out of the web UI's events when there's nothing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<NoticeAction>,
}

impl Notification {
    /// One with no actions: [`App::push_notification`] adds the ones its
    /// target has.
    pub fn new(title: impl Into<String>, body: impl Into<String>, target: Target) -> Self {
        Self { title: title.into(), body: body.into(), target, actions: Vec::new() }
    }
}

/// What a notification offers besides opening it: a reply written in it,
/// marking it read, or answering a game's challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeAction {
    Reply,
    MarkRead,
    Accept,
    Decline,
}

/// What was done with a notification, as a desktop reports it (see
/// [`App::respond`]).
// Each desktop's backend builds the ones it can get (a click is all some can).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// Clicked: show what it's about.
    Open(Target),
    /// A reply was written in it.
    Reply {
        target: Target,
        text: String,
    },
    MarkRead(Target),
    Accept(Target),
    Decline(Target),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    /// An LXMF conversation (the peer's hash).
    Conversation { key: String },
    /// An RRC room or whisper conversation (the hub's hash, the room's key).
    Room { hub: String, room: String },
    /// A game (with `key`, the peer's hash): a move of it.
    Game { key: String, app: String, session: String },
    /// Several at once (a sync bringing in a backlog).
    Summary,
}

impl Target {
    /// The same for the same conversation or room.
    pub fn tag(&self) -> String {
        match self {
            Target::Conversation { key } => format!("lxmf:{key}"),
            Target::Room { hub, room } => format!("rrc:{hub}:{room}"),
            Target::Game { app, session, .. } => format!("game:{app}:{session}"),
            Target::Summary => "summary".to_string(),
        }
    }
}

/// Longest body, in characters.
const BODY_CHARS: usize = 180;
/// More than this many at once are summed up in one.
const AT_ONCE: usize = 3;

/// Message text for a notification: one paragraph, cut short.
pub fn body(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(BODY_CHARS) {
        Some((cut, _)) => format!("{}…", text[..cut].trim_end()),
        None => text,
    }
}

/// What arrived together, fit for showing: a newer notification about
/// the same thing replaces an older one, and a pile (a sync bringing in a
/// backlog) becomes one that sums it up.
pub fn batch(pending: Vec<Notification>) -> Vec<Notification> {
    let count = pending.len();
    let mut latest: Vec<Notification> = Vec::new();
    for notification in pending {
        latest.retain(|n| n.target != notification.target);
        latest.push(notification);
    }
    if latest.len() <= AT_ONCE {
        return latest;
    }
    let mut from: Vec<&str> = Vec::new();
    for n in latest.iter().rev() {
        if !from.contains(&n.title.as_str()) {
            from.push(&n.title);
        }
    }
    let shown = from.len().min(AT_ONCE);
    let mut body = from[..shown].join(", ");
    if from.len() > shown {
        body.push_str(&format!(" and {} more", from.len() - shown));
    }
    vec![Notification::new(format!("{count} new messages"), body, Target::Summary)]
}

impl App {
    /// A notification was clicked (terminal UI): show what it's about.
    pub fn open_notified(&mut self, target: Target) {
        // Whatever is over the tab gives way.
        (self.prompt, self.guide, self.contact_card, self.paper_view, self.emoji) = (None, None, None, None, None);
        match target {
            Target::Conversation { key } => self.open_conversation(key),
            Target::Room { hub, room } => {
                let Some(hub) = crate::net::parse_hash(&hub) else { return };
                if self.channels.hub_index(hub).is_none() {
                    return;
                }
                self.tab = super::Tab::Channels;
                self.channels.selected = Some(super::channels::Target { hub, room: (!room.is_empty()).then_some(room) });
                self.mark_channel_read();
            }
            Target::Game { app, session, .. } => self.show_game(&app, &session),
            Target::Summary => self.tab = super::Tab::Messages,
        }
    }

    /// Queue a notification, unless it's about what's on screen in a window
    /// that has the focus, or it's quiet hours (see [`App::quiet_for`]).
    pub(super) fn push_notification(&mut self, on_screen: bool, mut notification: Notification) {
        if self.focused && on_screen {
            return;
        }
        if self.quiet_for(&notification.target, minute_now()) {
            return;
        }
        notification.actions = self.actions_for(&notification.target);
        self.notifications.push(notification);
    }

    /// What can be done from a notification about `target`: a conversation
    /// or a room can be answered and marked read; a challenge can be
    /// accepted or declined while it waits for an answer.
    pub fn actions_for(&self, target: &Target) -> Vec<NoticeAction> {
        match target {
            Target::Conversation { .. } => vec![NoticeAction::Reply, NoticeAction::MarkRead],
            Target::Room { hub, .. } => match crate::net::parse_hash(hub).and_then(|hash| self.channels.hub_index(hash)) {
                Some(_) => vec![NoticeAction::Reply, NoticeAction::MarkRead],
                None => Vec::new(),
            },
            Target::Game { app, session, .. } => match self.games.view(app, session) {
                Some(game) if game.actions.iter().any(|a| a == "accept") => vec![NoticeAction::Accept, NoticeAction::Decline],
                _ => Vec::new(),
            },
            Target::Summary => Vec::new(),
        }
    }

    /// Do what was done with a notification, in the app: the one place
    /// every desktop's notifications come back to, so they all act alike.
    /// A reply goes the way set for that conversation (or the room's
    /// text, in its limit) and marks it read; Accept and Decline answer a
    /// challenge as the Games tab does. Why it couldn't, if it couldn't.
    pub fn respond(&mut self, response: Response) -> Result<(), String> {
        match response {
            Response::Open(target) => {
                self.open_notified(target);
                Ok(())
            }
            Response::Reply { target: Target::Conversation { key }, text } => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    return Ok(());
                }
                let mode = self.store.contact(&key).delivery.unwrap_or(crate::lxmf::DeliveryMode::Auto);
                self.send_message(key.clone(), text, Vec::new(), mode, None).map_err(|(error, ..)| error)?;
                self.mark_conversation_read(&key);
                Ok(())
            }
            Response::Reply { target: Target::Room { hub, room }, text } => {
                let (index, room) = self.notified_room(&hub, room)?;
                // Too long for the hub: sent in parts, as the web UI's
                // "send as several" does.
                if let Some(parts) = self.channel_submit(index, &room, &text) {
                    for part in parts {
                        self.channel_submit(index, &room, &part);
                    }
                }
                self.mark_room_read(index, &room);
                Ok(())
            }
            Response::Reply { .. } => Err("There's nothing to reply to".into()),
            Response::MarkRead(Target::Conversation { key }) => {
                self.mark_conversation_read(&key);
                Ok(())
            }
            Response::MarkRead(Target::Room { hub, room }) => {
                let (index, room) = self.notified_room(&hub, room)?;
                self.mark_room_read(index, &room);
                Ok(())
            }
            Response::MarkRead(_) => Ok(()),
            Response::Accept(Target::Game { app, session, .. }) => self.game_action(&app, &session, "accept", None),
            Response::Decline(Target::Game { app, session, .. }) => self.game_action(&app, &session, "decline", None),
            Response::Accept(_) | Response::Decline(_) => Err("There's no challenge to answer".into()),
        }
    }

    /// The hub's index, and the room, a notification about a room names.
    fn notified_room(&self, hub: &str, room: String) -> Result<(usize, String), String> {
        let hash = crate::net::parse_hash(hub).ok_or("There's no such hub")?;
        let index = self.channels.hub_index(hash).ok_or("There's no such hub")?;
        Ok((index, room))
    }

    /// Whether quiet hours hold back a notification about `target` at
    /// `minute` (of the day, local time): inside them, all but trusted
    /// contacts' messages (if they're let through) wait to be seen in the
    /// app. Messages still arrive and count as unread.
    pub fn quiet_for(&self, target: &Target, minute: u32) -> bool {
        let Some(hours) = self.settings.quiet_hours.as_deref().and_then(QuietHours::parse) else { return false };
        if !hours.contains(minute) {
            return false;
        }
        let trusted = match target {
            Target::Conversation { key } | Target::Game { key, .. } => self.store.contact(key).trust == crate::store::Trust::Trusted,
            _ => false,
        };
        !(trusted && self.settings.quiet_hours_trusted)
    }

    /// The notifications to show now.
    pub fn take_notifications(&mut self) -> Vec<Notification> {
        batch(std::mem::take(&mut self.notifications))
    }

    /// Something with unread messages was read.
    pub(super) fn read(&mut self, target: Target) {
        self.reads.push(target.tag());
    }

    /// What was read since last asked (notification tags).
    pub fn take_reads(&mut self) -> Vec<String> {
        std::mem::take(&mut self.reads)
    }
}

/// The minute of the day now, local time.
fn minute_now() -> u32 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() * 60 + now.minute()
}

/// Quiet hours: from one time of day to another, the end the next day if
/// it's earlier (`22:00-07:00`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    /// Minutes from midnight.
    pub from: u32,
    pub to: u32,
}

impl QuietHours {
    /// `22:00-07:00`, `22-7`, `9:30 to 12`: two times of day (hours, or
    /// hours and minutes), apart.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().replace(['–', '—'], "-").to_lowercase().replace(" to ", "-");
        let (from, to) = text.split_once('-')?;
        let time = |t: &str| -> Option<u32> {
            let (h, m) = t.trim().split_once(':').unwrap_or((t.trim(), "0"));
            let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
            (h < 24 && m < 60).then_some(h * 60 + m)
        };
        let (from, to) = (time(from)?, time(to)?);
        (from != to).then_some(Self { from, to })
    }

    /// Whether `minute` (of the day) is inside them.
    pub fn contains(self, minute: u32) -> bool {
        if self.from < self.to { (self.from..self.to).contains(&minute) } else { minute >= self.from || minute < self.to }
    }

    /// As settings.json keeps them: `22:00-07:00`.
    pub fn label(self) -> String {
        format!("{:02}:{:02}-{:02}:{:02}", self.from / 60, self.from % 60, self.to / 60, self.to % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_hours_as_typed_and_across_midnight() {
        let night = QuietHours::parse("22:00-07:00").unwrap();
        assert_eq!(night, QuietHours { from: 22 * 60, to: 7 * 60 });
        assert_eq!(QuietHours::parse(" 22 - 7 "), Some(night));
        assert_eq!(QuietHours::parse("22:00–07:00"), Some(night));
        assert_eq!(QuietHours::parse("9:30 to 12").unwrap().label(), "09:30-12:00");
        for wrong in ["", "22", "25-7", "22:60-7", "7-7", "night"] {
            assert_eq!(QuietHours::parse(wrong), None, "{wrong}");
        }
        assert!(night.contains(23 * 60) && night.contains(0) && night.contains(6 * 60 + 59));
        assert!(!night.contains(7 * 60) && !night.contains(12 * 60) && !night.contains(21 * 60 + 59));
        let lunch = QuietHours::parse("12-13").unwrap();
        assert!(lunch.contains(12 * 60 + 30) && !lunch.contains(13 * 60) && !lunch.contains(11 * 60));
    }

    #[test]
    fn quiet_hours_hold_back_all_but_trusted_contacts() {
        let dir = std::env::temp_dir().join(format!("rettui-quiet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = crate::config::Settings { quiet_hours: Some("22:00-07:00".into()), ..crate::config::Settings::default() };
        let mut app = crate::app::test_app(&dir, settings, crate::store::Store::default());
        let (friend, stranger) = ("aa".repeat(16), "bb".repeat(16));
        app.store.update_contact(&friend, |c| c.trust = crate::store::Trust::Trusted);
        let to = |key: &str| Target::Conversation { key: key.into() };
        let room = Target::Room { hub: "cc".repeat(16), room: "general".into() };
        let (night, day) = (23 * 60, 12 * 60);
        assert!(app.quiet_for(&to(&stranger), night) && app.quiet_for(&room, night));
        // Trusted contacts get through, unless that's turned off.
        assert!(!app.quiet_for(&to(&friend), night));
        app.settings.quiet_hours_trusted = false;
        assert!(app.quiet_for(&to(&friend), night));
        // Outside them, or with none set: nothing held back.
        assert!(!app.quiet_for(&to(&stranger), day));
        app.settings.quiet_hours = None;
        assert!(!app.quiet_for(&to(&stranger), night));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn note(title: &str, body: &str, key: &str) -> Notification {
        Notification::new(title, body, Target::Conversation { key: key.into() })
    }

    #[test]
    fn a_clicked_notification_opens_what_its_about() {
        use crate::app::Tab;
        let dir = std::env::temp_dir().join(format!("rettui-notified-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        app.tab = Tab::Status;
        app.open_guide();
        let key = "ab".repeat(16);
        app.open_notified(Target::Conversation { key: key.clone() });
        assert_eq!((app.tab, app.active_conversation.as_deref()), (Tab::Messages, Some(key.as_str())));
        assert!(app.guide.is_none(), "what was over the tab gives way");
        // A room on a hub that's there; one that isn't is left alone.
        let hub = [0xcd; 16];
        app.add_hub(hub, "rrc.hub", None);
        app.tab = Tab::Messages;
        app.open_notified(Target::Room { hub: hex::encode(hub), room: "#general".into() });
        assert_eq!(app.tab, Tab::Channels);
        assert_eq!(app.channels.selected, Some(crate::app::channels::Target { hub, room: Some("#general".into()) }));
        app.open_notified(Target::Room { hub: "ef".repeat(16), room: "#general".into() });
        assert_eq!(app.channels.selected.as_ref().map(|t| t.hub), Some(hub));
        app.open_notified(Target::Summary);
        assert_eq!(app.tab, Tab::Messages);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bodies_are_one_short_paragraph() {
        assert_eq!(body("  hello\n\nthere  "), "hello there");
        let long = "word ".repeat(100);
        let cut = body(&long);
        assert!(cut.ends_with('…') && cut.chars().count() <= BODY_CHARS + 1);
    }

    #[test]
    fn newer_replace_older_and_piles_are_summed_up() {
        let two = batch(vec![note("alice", "one", "a"), note("bob", "hi", "b"), note("alice", "two", "a")]);
        assert_eq!(two, vec![note("bob", "hi", "b"), note("alice", "two", "a")]);
        let pile: Vec<Notification> = ["alice", "bob", "carol", "dave", "erin"].iter().map(|n| note(n, "hi", n)).collect();
        let summed = batch(pile);
        assert_eq!(summed.len(), 1);
        assert_eq!(summed[0].title, "5 new messages");
        assert_eq!(summed[0].body, "erin, dave, carol and 2 more");
        assert_eq!(summed[0].target, Target::Summary);
    }
}
