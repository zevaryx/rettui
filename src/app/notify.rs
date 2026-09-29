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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    /// An LXMF conversation (the peer's hash).
    Conversation { key: String },
    /// An RRC room or whisper conversation (the hub's hash, the room's key).
    Room { hub: String, room: String },
    /// Several at once (a sync bringing in a backlog).
    Summary,
}

impl Target {
    /// The same for the same conversation or room.
    pub fn tag(&self) -> String {
        match self {
            Target::Conversation { key } => format!("lxmf:{key}"),
            Target::Room { hub, room } => format!("rrc:{hub}:{room}"),
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
    vec![Notification { title: format!("{count} new messages"), body, target: Target::Summary }]
}

impl App {
    /// Queue a notification, unless it's about what's on screen in a window
    /// that has the focus.
    pub(super) fn push_notification(&mut self, on_screen: bool, notification: Notification) {
        if self.focused && on_screen {
            return;
        }
        self.notifications.push(notification);
    }

    /// The notifications to show now.
    pub fn take_notifications(&mut self) -> Vec<Notification> {
        batch(std::mem::take(&mut self.notifications))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str, body: &str, key: &str) -> Notification {
        Notification { title: title.into(), body: body.into(), target: Target::Conversation { key: key.into() } }
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
        let pile: Vec<Notification> =
            ["alice", "bob", "carol", "dave", "erin"].iter().map(|n| note(n, "hi", n)).collect();
        let summed = batch(pile);
        assert_eq!(summed.len(), 1);
        assert_eq!(summed[0].title, "5 new messages");
        assert_eq!(summed[0].body, "erin, dave, carol and 2 more");
        assert_eq!(summed[0].target, Target::Summary);
    }
}
