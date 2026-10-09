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
/// rsRRCD adds members' full identities to `/who` replies for clients that
/// ask (`K_USER_LIST`).
const CAP_USER_LIST: u64 = 4;

// K_USER_LIST entry keys.
const B_USER_IDENTITY: u64 = 0;
const B_USER_NICK: u64 = 1;

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
        (
            key(B_HELLO_CAPS),
            Value::Map(vec![
                (key(CAP_RESOURCE_ENVELOPE), Value::Bool(true)),
                (key(CAP_ACTION), Value::Bool(true)),
                (key(CAP_USER_LIST), Value::Bool(true)),
            ]),
        ),
    ])
}

/// Whether a capability is on: `true`, or a number other than 0 (the Go
/// hub sends 1).
fn cap_on(value: &Value) -> bool {
    value.as_bool().unwrap_or_else(|| as_u64(value).is_some_and(|n| n != 0))
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
        welcome.direct_notices = get(caps, CAP_DIRECT_NOTICE).is_some_and(cap_on);
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
    Some(lines.filter_map(listed_room).collect())
}

/// The rest of a `/list` reply that came in several notices, a line or so
/// each (rrcd sends long text that way): every line indented like the
/// reply's. `None` if the notice is something else.
pub fn parse_room_list_more(text: &str) -> Option<Vec<(String, Option<String>)>> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let room_line = |line: &&str| {
        let Some(rest) = line.strip_prefix("  ") else { return false };
        let name = rest.split_once(" - ").map_or(rest, |(name, _)| name);
        is_more(rest) || !name.is_empty() && !name.contains(char::is_whitespace)
    };
    (!lines.is_empty() && lines.iter().all(room_line)).then(|| lines.into_iter().filter_map(listed_room).collect())
}

/// Ratspeak ends a `/list` reply too long for one packet with `(+N more)`.
fn is_more(line: &str) -> bool {
    line.trim().strip_prefix("(+").and_then(|rest| rest.strip_suffix(" more)")).is_some_and(|n| n.parse::<u32>().is_ok())
}

/// One room line of a `/list` reply: `name` or `name - topic`.
fn listed_room(line: &str) -> Option<(String, Option<String>)> {
    let line = line.trim();
    if line.is_empty() || is_more(line) {
        return None;
    }
    let (name, topic) = match line.split_once(" - ") {
        Some((name, topic)) => (name, Some(topic.trim().to_string()).filter(|t| !t.is_empty())),
        None => (line, None),
    };
    let name = normalize_room(name);
    (!name.is_empty()).then_some((name, topic))
}

/// One `/who` entry: a nick with a 12-hex identity prefix, or a bare full
/// identity hash for users without a nick. Structured lists (rsRRCD) give
/// the full identity with the nick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoEntry {
    pub nick: Option<String>,
    pub hash_hex: String,
}

/// `/who` (or `/names`) reply: `members in <room>: nick (hex12), <hex32>,
/// ...`. Hubs differ in the details, which this takes in its stride:
/// - the Go hub (rrc-hub) marks people `[away]` after their entry, and
///   lists links not yet identified as `(unidentified)`;
/// - Ratspeak splits a long reply over several notices, each starting
///   `members in <room>: ` again (each is parsed as it comes).
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
    let mut entries = Vec::new();
    // Entries are ", "-separated, but nicks may contain ", " and "(...)", so
    // anchor on the trailing " (hex12)" or a bare 32-hex hash.
    let mut current = String::new();
    for piece in body.split(", ") {
        if !current.is_empty() {
            current.push_str(", ");
        }
        current.push_str(piece);
        if let Some(entry) = who_entry(&current) {
            entries.extend(entry);
            current.clear();
        }
    }
    Some((room, entries))
}

/// A whole `/who` entry (`Some`), which may be someone the reply can't
/// name (`Some(None)`); `None` if `text` is only the start of one.
fn who_entry(text: &str) -> Option<Option<WhoEntry>> {
    let is_hex = |s: &str, len: usize| s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit());
    // Notes after an entry, like the Go hub's " [away]".
    let mut core = text.trim();
    while let Some(start) = core.rfind(" [")
        && core.ends_with(']')
    {
        core = core[..start].trim_end();
    }
    if core == "(unidentified)" {
        return Some(None);
    }
    if is_hex(core, 32) {
        return Some(Some(WhoEntry { nick: None, hash_hex: core.to_lowercase() }));
    }
    let open = core.rfind(" (")?;
    let prefix = core.strip_suffix(')')?.get(open + 2..)?;
    is_hex(prefix, 12).then(|| Some(WhoEntry { nick: Some(core[..open].trim().to_string()), hash_hex: prefix.to_lowercase() }))
}

/// The structured member list rsRRCD adds to a `/who` reply
/// (`K_USER_LIST`): full identities, as hex text, with nicks.
pub fn parse_user_list(env: &Envelope) -> Option<Vec<WhoEntry>> {
    let Some(Value::Array(users)) = &env.user_list else {
        return None;
    };
    let entries = users
        .iter()
        .filter_map(|user| {
            let Value::Map(user) = user else { return None };
            let identity = get(user, B_USER_IDENTITY)?;
            let hash_hex = match identity {
                Value::Bytes(b) => hex::encode(b),
                Value::Text(t) => t.to_lowercase(),
                _ => return None,
            };
            (hash_hex.len() == 32 && hash_hex.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| WhoEntry { nick: get(user, B_USER_NICK).and_then(text).filter(|n| !n.is_empty()), hash_hex })
        })
        .collect();
    Some(entries)
}

/// A topic notice: what `/topic <room>` says (`topic for <room>: <text>`,
/// `(none)` without one), or the hub telling the room it changed (`topic
/// for <room> is now: <text>`, `(cleared)`). The room, and its topic.
pub fn parse_topic(text: &str) -> Option<(String, Option<String>)> {
    let rest = text.strip_prefix("topic for ")?;
    let (room, topic, none) = match rest.split_once(" is now: ") {
        Some((room, topic)) if !room.contains(": ") => (room, topic, "(cleared)"),
        _ => {
            let (room, topic) = rest.split_once(": ")?;
            (room, topic, "(none)")
        }
    };
    let room = normalize_room(room);
    let topic = topic.trim();
    (!room.is_empty()).then(|| (room, (topic != none && !topic.is_empty()).then(|| topic.to_string())))
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

    #[test]
    fn who_replies_from_other_hubs() {
        // The Go hub marks people away, and lists links not identified yet.
        let (_, who) = parse_who(
            "members in lobby: amy (0123456789ab) [away], (unidentified), bob, jr (abcdefabcdef), 00112233445566778899aabbccddeeff [away]",
        )
        .unwrap();
        let names: Vec<_> = who.iter().map(|e| (e.nick.as_deref(), e.hash_hex.as_str())).collect();
        assert_eq!(names, [(Some("amy"), "0123456789ab"), (Some("bob, jr"), "abcdefabcdef"), (None, "00112233445566778899aabbccddeeff")]);
        // A nick that itself ends in brackets is still the nick.
        let (_, who) = parse_who("members in x: [mod] sam [x] (0123456789ab)").unwrap();
        assert_eq!(who[0].nick.as_deref(), Some("[mod] sam [x]"));
        // Ratspeak sends a long reply in parts, each with the header again.
        let first = parse_who("members in lobby: a (0123456789ab), b (abcdefabcdef)").unwrap();
        let second = parse_who("members in lobby: c (111111111111)").unwrap();
        assert_eq!((first.1.len(), second.0.as_str(), second.1.len()), (2, "lobby", 1));

        // rsRRCD adds the full identities when asked (K_USER_LIST).
        let mut env = Envelope::new(crate::rrc::t::NOTICE, &[9; 16]).text("members in lobby: amy (0101010101010)");
        assert_eq!(parse_user_list(&env), None);
        env.user_list = Some(Value::Array(vec![
            Value::Map(vec![(key(0), Value::Text("0101010101010101010101010101010A".into())), (key(1), Value::Text("amy".into()))]),
            Value::Map(vec![(key(0), Value::Bytes(vec![2; 16]))]),
            Value::Map(vec![(key(0), Value::Text("short".into()))]),
        ]));
        let users = parse_user_list(&env).unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!((users[0].nick.as_deref(), users[0].hash_hex.as_str()), (Some("amy"), "0101010101010101010101010101010a"));
        assert_eq!((users[1].nick.as_deref(), users[1].hash_hex.as_str()), (None, "02020202020202020202020202020202"));
        // ...which rsRRCD only does for clients that ask.
        let Value::Map(hello) = hello_body() else { panic!() };
        let Some(Value::Map(caps)) = get(&hello, B_HELLO_CAPS) else { panic!() };
        assert!(get(caps, CAP_USER_LIST).is_some_and(cap_on));
    }

    #[test]
    fn list_replies_from_other_hubs() {
        // Ratspeak ends a list too long for a packet with a count.
        let list = parse_room_list("Registered public rooms:\n  general - Chat\n  #mesh\n  (+3 more)").unwrap();
        assert_eq!(list, vec![("general".into(), Some("Chat".into())), ("mesh".into(), None)]);
        // rrcd sends long text a line per notice: the rest of the list.
        assert_eq!(parse_room_list("Registered public rooms:"), Some(vec![]));
        assert_eq!(
            parse_room_list_more("  general - Chat about anything"),
            Some(vec![("general".into(), Some("Chat about anything".into()))])
        );
        assert_eq!(
            parse_room_list_more("  mesh\n  radio - RNodes"),
            Some(vec![("mesh".into(), None), ("radio".into(), Some("RNodes".into()))])
        );
        assert_eq!(parse_room_list_more("  (+2 more)"), Some(vec![]));
        assert_eq!(parse_room_list_more("Welcome to the hub"), None);
        assert_eq!(parse_room_list_more("  two words"), None);
    }

    #[test]
    fn topics_and_capabilities() {
        assert_eq!(parse_topic("topic for General: Hello: world"), Some(("general".into(), Some("Hello: world".into()))));
        assert_eq!(parse_topic("topic for x: (none)"), Some(("x".into(), None)));
        assert_eq!(parse_topic("topic for x is now: New one"), Some(("x".into(), Some("New one".into()))));
        assert_eq!(parse_topic("topic for x is now: (cleared)"), Some(("x".into(), None)));
        assert_eq!(parse_topic("the topic for x"), None);
        // The Go hub sends capabilities as 1 rather than true.
        let body = Value::Map(vec![(key(2), Value::Map(vec![(key(2), key(1))]))]);
        assert!(parse_welcome(Some(&body)).direct_notices);
        let body = Value::Map(vec![(key(2), Value::Map(vec![(key(2), key(0))]))]);
        assert!(!parse_welcome(Some(&body)).direct_notices);
    }
}
