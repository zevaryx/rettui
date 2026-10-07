//! Hub handshake bodies (HELLO, WELCOME), Resource announcements, and the
//! text replies the hub sends as notices (`/list`, `/who`, room info).

use ciborium::value::Value;

use super::envelope::{Envelope, as_u64, bytes, get, key, text};
use super::text::normalize_room;

// HELLO body keys and capabilities.
const B_HELLO_NAME: u64 = 0;
const B_HELLO_VER: u64 = 1;
const B_HELLO_CAPS: u64 = 2;
const CAP_RESOURCE_ENVELOPE: u64 = 0;
const CAP_ACTION: u64 = 1;
pub const CAP_DIRECT_NOTICE: u64 = 2;

// WELCOME body keys and limits.
const B_WELCOME_HUB: u64 = 0;
const B_WELCOME_VER: u64 = 1;
const B_WELCOME_CAPS: u64 = 2;
const B_WELCOME_LIMITS: u64 = 3;

// RESOURCE_ENVELOPE body keys.
const B_RES_ID: u64 = 0;
const B_RES_KIND: u64 = 1;
const B_RES_SIZE: u64 = 2;
const B_RES_SHA256: u64 = 3;
const B_RES_ENCODING: u64 = 4;

/// HELLO body advertising what this client understands.
pub fn hello_body() -> Value {
    Value::Map(vec![
        (key(B_HELLO_NAME), Value::Text("rettui".into())),
        (key(B_HELLO_VER), Value::Text(env!("CARGO_PKG_VERSION").into())),
        (key(B_HELLO_CAPS), Value::Map(vec![(key(CAP_RESOURCE_ENVELOPE), Value::Bool(true)), (key(CAP_ACTION), Value::Bool(true))])),
    ])
}

/// Hub limits from WELCOME (defaults match rrcd and NomadNet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_nick_bytes: usize,
    pub max_room_bytes: usize,
    pub max_msg_bytes: usize,
    pub max_rooms: usize,
    pub msgs_per_minute: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_nick_bytes: 32, max_room_bytes: 64, max_msg_bytes: 350, max_rooms: 32, msgs_per_minute: 240 }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Welcome {
    pub hub_name: Option<String>,
    pub version: Option<String>,
    pub direct_notices: bool,
    pub limits: Limits,
}

pub fn parse_welcome(body: Option<&Value>) -> Welcome {
    let mut welcome = Welcome::default();
    let Some(Value::Map(map)) = body else {
        return welcome;
    };
    welcome.hub_name = get(map, B_WELCOME_HUB).and_then(text);
    welcome.version = get(map, B_WELCOME_VER).and_then(text);
    if let Some(Value::Map(caps)) = get(map, B_WELCOME_CAPS) {
        welcome.direct_notices = get(caps, CAP_DIRECT_NOTICE).is_some_and(|v| v.as_bool().unwrap_or(false));
    }
    if let Some(Value::Map(limits)) = get(map, B_WELCOME_LIMITS) {
        let limit = |k: u64, default: usize| get(limits, k).and_then(as_u64).map_or(default, |v| v as usize);
        let d = Limits::default();
        welcome.limits = Limits {
            max_nick_bytes: limit(0, d.max_nick_bytes),
            max_room_bytes: limit(1, d.max_room_bytes),
            max_msg_bytes: limit(2, d.max_msg_bytes),
            max_rooms: limit(3, d.max_rooms),
            msgs_per_minute: limit(4, d.msgs_per_minute),
        };
    }
    welcome
}

/// Announcement of a text notice arriving as a Link Resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceAnnouncement {
    pub kind: String,
    pub size: usize,
    pub sha256: Option<Vec<u8>>,
    pub encoding: String,
    pub room: Option<String>,
}

pub fn parse_resource_envelope(env: &Envelope) -> Option<ResourceAnnouncement> {
    let Some(Value::Map(map)) = &env.body else {
        return None;
    };
    get(map, B_RES_ID).and_then(bytes)?;
    let size = get(map, B_RES_SIZE).and_then(as_u64).filter(|s| *s > 0)? as usize;
    Some(ResourceAnnouncement {
        kind: get(map, B_RES_KIND).and_then(text)?,
        size,
        sha256: get(map, B_RES_SHA256).and_then(bytes),
        encoding: get(map, B_RES_ENCODING).and_then(text).unwrap_or_else(|| "utf-8".into()),
        room: env.room.as_deref().map(normalize_room).filter(|r| !r.is_empty()),
    })
}

/// `/list` reply: rooms and topics, or `None` if the notice is not one.
pub fn parse_room_list(text: &str) -> Option<Vec<(String, Option<String>)>> {
    if text.trim() == "No public rooms registered" {
        return Some(Vec::new());
    }
    let mut lines = text.lines();
    if !lines.next()?.trim_start().starts_with("Registered public rooms") {
        return None;
    }
    Some(
        lines
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|line| match line.split_once(" - ") {
                Some((name, topic)) => (normalize_room(name), Some(topic.trim().to_string()).filter(|t| !t.is_empty())),
                None => (normalize_room(line), None),
            })
            .collect(),
    )
}

/// One `/who` entry: a nick with a 12-hex identity prefix, or a bare full
/// identity hash for users without a nick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoEntry {
    pub nick: Option<String>,
    pub hash_hex: String,
}

/// `/who` reply: `members in <room>: nick (hex12), <hex32>, ...`.
pub fn parse_who(text: &str) -> Option<(String, Vec<WhoEntry>)> {
    let rest = text.strip_prefix("members in ")?;
    let (room, body) = rest.split_once(": ")?;
    let room = normalize_room(room);
    if room.is_empty() {
        return None;
    }
    let body = body.trim();
    if body.is_empty() || body == "(none)" {
        return Some((room, Vec::new()));
    }
    let is_hex = |s: &str, len: usize| s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit());
    let mut entries = Vec::new();
    // Entries are ", "-separated, but nicks may contain ", " and "(...)", so
    // anchor on the trailing " (hex12)" or a bare 32-hex hash.
    let mut current = String::new();
    for piece in body.split(", ") {
        if !current.is_empty() {
            current.push_str(", ");
        }
        current.push_str(piece);
        if is_hex(&current, 32) {
            entries.push(WhoEntry { nick: None, hash_hex: current.to_lowercase() });
            current.clear();
        } else if let Some(open) = current.rfind(" (")
            && current.ends_with(')')
            && is_hex(&current[open + 2..current.len() - 1], 12)
        {
            entries.push(WhoEntry {
                nick: Some(current[..open].trim().to_string()),
                hash_hex: current[open + 2..current.len() - 1].to_lowercase(),
            });
            current.clear();
        }
    }
    Some((room, entries))
}

/// Room status notice sent after joining.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomInfo {
    pub room: String,
    pub registered: bool,
    pub mode: String,
    pub topic: Option<String>,
}

/// `room <r>: registered; mode=+nt; topic=<text>`.
pub fn parse_room_info(text: &str) -> Option<RoomInfo> {
    let rest = text.strip_prefix("room ")?;
    let (room, info) = rest.split_once(": ")?;
    let (head, topic) = info.split_once("topic=")?;
    let topic = topic.trim();
    let mode = head.split_once("mode=").map(|(_, m)| m.trim().trim_end_matches(';').trim().to_string()).unwrap_or_default();
    Some(RoomInfo {
        room: normalize_room(room),
        registered: head.trim_start().starts_with("registered"),
        mode,
        topic: (topic != "(none)" && !topic.is_empty()).then(|| topic.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_limits_and_caps() {
        let body = Value::Map(vec![
            (key(0), Value::Text("Test Hub".into())),
            (key(2), Value::Map(vec![(key(2), Value::Bool(true))])),
            (key(3), Value::Map(vec![(key(2), key(200)), (key(0), key(16))])),
        ]);
        let w = parse_welcome(Some(&body));
        assert_eq!(w.hub_name.as_deref(), Some("Test Hub"));
        assert!(w.direct_notices);
        assert_eq!((w.limits.max_msg_bytes, w.limits.max_nick_bytes), (200, 16));
        assert_eq!(w.limits.max_rooms, 32);
    }

    #[test]
    fn hub_notice_parsers() {
        let list = parse_room_list("Registered public rooms:\n  general - Chat about anything\n  mesh\n").unwrap();
        assert_eq!(list, vec![("general".into(), Some("Chat about anything".into())), ("mesh".into(), None),]);
        assert_eq!(parse_room_list("No public rooms registered"), Some(vec![]));
        assert_eq!(parse_room_list("something else"), None);

        let (room, who) =
            parse_who("members in General: zev (0123456789ab), odd, nick (x) (abcdefabcdef), 00112233445566778899aabbccddeeff").unwrap();
        assert_eq!(room, "general");
        assert_eq!(who.len(), 3);
        assert_eq!(who[0].nick.as_deref(), Some("zev"));
        assert_eq!(who[1].nick.as_deref(), Some("odd, nick (x)"));
        assert_eq!(who[2], WhoEntry { nick: None, hash_hex: "00112233445566778899aabbccddeeff".into() });
        assert_eq!(parse_who("members in x: (none)").unwrap().1, vec![]);

        let info = parse_room_info("room general: registered; mode=+nt; topic=Welcome all").unwrap();
        assert_eq!((info.room.as_str(), info.registered, info.mode.as_str()), ("general", true, "+nt"));
        assert_eq!(info.topic.as_deref(), Some("Welcome all"));
        let info = parse_room_info("room x: unregistered; mode=+; topic=(none)").unwrap();
        assert_eq!((info.registered, info.topic), (false, None));
    }
}
