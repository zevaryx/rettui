//! LXMF fields beyond text and files, read (and for reactions, written) as
//! LXMF's published field definitions describe them and as other clients
//! send them:
//!
//! - Reactions (`FIELD_REACTION`, 0x40): `{0x00: <message hash>, 0x01:
//!   <UTF-8>}` on an otherwise empty message, as Columba and MeshChatX send
//!   them. Older Columba put `{"reaction_to": <hex>, "emoji": <text>}` in
//!   field 0x10, which is read too.
//! - Telemetry (`FIELD_TELEMETRY`, 0x02): a msgpack map of sensor readings,
//!   which is how Sideband and Columba send location updates. Only the
//!   time (sensor 0x01) and location (sensor 0x02) are read. A location is
//!   `[lat, lon, altitude, speed, bearing, accuracy, updated]`, the first six
//!   as big-endian integers in bytes (degrees × 10⁶, then metres, m/s,
//!   degrees and metres × 10²).
//! - Commands (`FIELD_COMMANDS`, 0x09): a list of `{command: arguments}`
//!   maps, as Sideband sends telemetry requests and pings. rettui runs none
//!   of them; it says what was asked.
//! - Voice messages (`FIELD_AUDIO`, 0x07): `[mode, bytes]`, Opus (in an Ogg
//!   file) or Codec2.

use lxmf_core::constants::{
    FIELD_AUDIO, FIELD_COMMANDS, FIELD_CUSTOM_META, FIELD_FILE_ATTACHMENTS, FIELD_ICON_APPEARANCE, FIELD_IMAGE,
    FIELD_REACTION, FIELD_RENDERER, FIELD_REPLY_QUOTE, FIELD_REPLY_TO, FIELD_TELEMETRY, FIELD_TELEMETRY_STREAM,
    FIELD_TICKET, REACTION_CONTENT, REACTION_TO,
};
use lxmf_core::message_api::LxMessage;
use rmpv::Value;
use serde::{Deserialize, Serialize};

use super::{bytes_of, encode};

/// Older Columba sent replies and reactions as a map in field 0x10, before
/// they had fields of their own.
pub(super) const FIELD_COLUMBA_EXTENSIONS: u8 = 0x10;

/// A field's value: bytes as they were sent (a field whose value is msgpack
/// `bin`), anything else decoded.
pub(super) fn value(message: &LxMessage, id: u8) -> Option<Value> {
    let raw = message.get_field(id)?;
    if message.msgpack_field_ids.contains(&id) {
        rmpv::decode::read_value(&mut raw.as_slice()).ok()
    } else {
        Some(Value::Binary(raw.clone()))
    }
}

/// A value that may have been packed once more into bytes (as Sideband
/// sends its telemetry): the value inside.
fn unwrapped(value: Value) -> Option<Value> {
    match value {
        Value::Binary(bytes) => rmpv::decode::read_value(&mut bytes.as_slice()).ok(),
        other => Some(other),
    }
}

/// A full message hash, as bytes or hex.
pub(super) fn hash_of(value: &Value) -> Option<[u8; 32]> {
    match value {
        Value::Binary(bytes) => bytes.as_slice().try_into().ok(),
        Value::String(text) => hex::decode(text.as_str()?).ok()?.try_into().ok(),
        _ => None,
    }
}

/// The value at integer `key` of a msgpack map.
fn at(entries: &[(Value, Value)], key: u64) -> Option<&Value> {
    entries.iter().find(|(k, _)| k.as_u64() == Some(key)).map(|(_, v)| v)
}

/// The value at text `key` of a msgpack map.
fn named<'a>(entries: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    entries.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v)
}

/// A reaction to a message: its hash, and the reaction (an emoji, usually).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub to: [u8; 32],
    pub emoji: String,
}

/// The most of a reaction kept, in characters: an emoji (some are several
/// characters joined), or a short word.
const REACTION_CHARS: usize = 16;

/// A reaction as shown: no control characters, trimmed, not too long.
pub fn clean_reaction(text: &str) -> Option<String> {
    let text: String = text.chars().filter(|c| !c.is_control()).take(REACTION_CHARS).collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The reaction a message carries, if it is one.
pub fn reaction_of(message: &LxMessage) -> Option<Reaction> {
    let text = |value: &Value| bytes_of(value).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let (to, emoji) = if let Some(Value::Map(entries)) = value(message, FIELD_REACTION) {
        (at(&entries, u64::from(REACTION_TO)).and_then(hash_of)?, at(&entries, u64::from(REACTION_CONTENT)).and_then(text)?)
    } else if let Some(Value::Map(entries)) = value(message, FIELD_COLUMBA_EXTENSIONS) {
        (named(&entries, "reaction_to").and_then(hash_of)?, named(&entries, "emoji").and_then(text)?)
    } else {
        return None;
    };
    Some(Reaction { to, emoji: clean_reaction(&emoji)? })
}

/// `FIELD_REACTION`'s value for a reaction (both as bytes, as Columba
/// sends them).
pub fn reaction_field(reaction: &Reaction) -> Vec<u8> {
    encode(&Value::Map(vec![
        (Value::from(REACTION_TO), Value::Binary(reaction.to.to_vec())),
        (Value::from(REACTION_CONTENT), Value::Binary(reaction.emoji.as_bytes().to_vec())),
    ]))
}

/// Where someone was, from a location update.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres above sea level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude: Option<f64>,
    /// Metres a second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    /// Degrees from north.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearing: Option<f64>,
    /// How far off it may be, in metres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<f64>,
    /// When it was taken (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<i64>,
}

impl Location {
    /// It on a map (OpenStreetMap).
    pub fn map_url(&self) -> String {
        let (lat, lon) = (self.latitude, self.longitude);
        format!("https://www.openstreetmap.org/?mlat={lat:.6}&mlon={lon:.6}#map=16/{lat:.6}/{lon:.6}")
    }

    /// Its coordinates, and how accurate they are if that's known.
    pub fn label(&self) -> String {
        let mut text = format!("{:.5}, {:.5}", self.latitude, self.longitude);
        if let Some(accuracy) = self.accuracy.filter(|a| *a > 0.0) {
            text.push_str(&format!(" (±{accuracy:.0} m)"));
        }
        text
    }
}

/// A telemetry update: when it was taken, and the location in it if any.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Telemetry {
    pub time: Option<i64>,
    pub location: Option<Location>,
}

/// Sensor numbers in a telemetry map.
const SENSOR_TIME: u64 = 0x01;
const SENSOR_LOCATION: u64 = 0x02;

/// A number in a location reading: a float as it is, else the integer
/// (packed in bytes, big-endian, `signed` or not) divided by `scale`.
fn reading(value: &Value, signed: bool, scale: f64) -> Option<f64> {
    let raw = match value {
        Value::F64(v) => return v.is_finite().then_some(*v),
        Value::F32(v) => return v.is_finite().then_some(f64::from(*v)),
        Value::Integer(n) => n.as_f64()?,
        Value::Binary(bytes) => match (bytes.as_slice(), signed) {
            ([a, b, c, d], true) => f64::from(i32::from_be_bytes([*a, *b, *c, *d])),
            ([a, b, c, d], false) => f64::from(u32::from_be_bytes([*a, *b, *c, *d])),
            ([a, b], _) => f64::from(u16::from_be_bytes([*a, *b])),
            _ => return None,
        },
        _ => return None,
    };
    Some(raw / scale)
}

fn location_of(value: &Value) -> Option<Location> {
    let Value::Array(parts) = value else { return None };
    let part = |i: usize, signed: bool, scale: f64| parts.get(i).and_then(|v| reading(v, signed, scale));
    let latitude = part(0, true, 1e6)?;
    let longitude = part(1, true, 1e6)?;
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return None;
    }
    Some(Location {
        latitude,
        longitude,
        altitude: part(2, true, 1e2),
        speed: part(3, false, 1e2),
        bearing: part(4, true, 1e2),
        accuracy: part(5, false, 1e2),
        updated: parts.get(6).and_then(Value::as_i64),
    })
}

/// The telemetry a message carries, if any.
pub fn telemetry_of(message: &LxMessage) -> Option<Telemetry> {
    let Value::Map(sensors) = unwrapped(value(message, FIELD_TELEMETRY)?)? else {
        return None;
    };
    Some(Telemetry {
        time: at(&sensors, SENSOR_TIME).and_then(Value::as_i64),
        location: at(&sensors, SENSOR_LOCATION).and_then(location_of),
    })
}

/// A command another client asks for (Sideband's numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    TelemetryRequest,
    Ping,
    Echo,
    SignalReport,
    /// A plugin's, or one rettui doesn't know.
    Other(u64),
}

impl Command {
    fn from_id(id: u64) -> Self {
        match id {
            0x01 => Command::TelemetryRequest,
            0x02 => Command::Ping,
            0x03 => Command::Echo,
            0x04 => Command::SignalReport,
            other => Command::Other(other),
        }
    }

    /// What it asked, in words.
    pub fn describe(self) -> String {
        match self {
            Command::TelemetryRequest => "Asked for your location (rettui doesn't share it)".into(),
            Command::Ping => "Pinged you with a command (rettui doesn't answer commands)".into(),
            Command::Echo => "Sent an echo command (rettui doesn't answer commands)".into(),
            Command::SignalReport => "Asked for a signal report (rettui doesn't answer commands)".into(),
            Command::Other(id) => format!("Sent a command rettui doesn't run ({id:#04x})"),
        }
    }
}

/// The commands a message carries.
pub fn commands_of(message: &LxMessage) -> Vec<Command> {
    let Some(Value::Array(list)) = value(message, FIELD_COMMANDS).and_then(unwrapped) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| match entry {
            Value::Map(commands) => Some(commands),
            _ => None,
        })
        .flatten()
        .filter_map(|(id, _)| id.as_u64().map(Command::from_id))
        .collect()
}

/// Whether a message says its sender stopped sharing their location
/// (Columba's `{"cease": true}` in `FIELD_CUSTOM_META`).
pub fn ceased(message: &LxMessage) -> bool {
    let Some(Value::Map(meta)) = value(message, FIELD_CUSTOM_META).and_then(unwrapped) else {
        return false;
    };
    named(&meta, "cease").and_then(Value::as_bool).unwrap_or(false)
}

/// A voice message: the audio mode, and the recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audio {
    pub mode: u8,
    pub data: Vec<u8>,
}

impl Audio {
    /// An Ogg file (Opus in its container), which players can open.
    pub fn playable(&self) -> bool {
        self.data.starts_with(b"OggS")
    }

    /// The codec, as shown.
    pub fn codec(&self) -> String {
        let codec2 = ["450PWB", "450", "700C", "1200", "1300", "1400", "1600", "2400", "3200"];
        match self.mode {
            0x01..=0x09 => format!("Codec2 {}", codec2[usize::from(self.mode - 1)]),
            0x10..=0x19 => "Opus".into(),
            _ if self.playable() => "Opus".into(),
            mode => format!("audio mode {mode:#04x}"),
        }
    }

    /// The file it's saved as. Only Ogg files play elsewhere; Codec2 needs
    /// a decoder, so it's kept as it came.
    pub fn file_name(&self) -> String {
        if self.playable() {
            "voice-message.ogg".into()
        } else if (0x01..=0x09).contains(&self.mode) {
            format!("voice-message-{}.codec2", self.codec().trim_start_matches("Codec2 ").to_lowercase())
        } else {
            "voice-message.audio".into()
        }
    }
}

/// The voice message a message carries, if it's well formed.
pub fn audio_of(message: &LxMessage) -> Option<Audio> {
    let field = message.audio_field().ok()??;
    (!field.bytes.is_empty()).then(|| Audio { mode: field.mode, data: field.bytes.to_vec() })
}

/// Fields rettui reads, or knows to leave alone (an icon's appearance, a
/// stamp ticket): anything else is mentioned when a message has nothing
/// else to show.
const KNOWN_FIELDS: [u8; 15] = [
    FIELD_TELEMETRY,
    FIELD_TELEMETRY_STREAM,
    FIELD_ICON_APPEARANCE,
    FIELD_FILE_ATTACHMENTS,
    FIELD_IMAGE,
    FIELD_AUDIO,
    FIELD_COMMANDS,
    FIELD_TICKET,
    FIELD_RENDERER,
    FIELD_REPLY_TO,
    FIELD_REPLY_QUOTE,
    FIELD_REACTION,
    FIELD_CUSTOM_META,
    FIELD_COLUMBA_EXTENSIONS,
    // Command results (Sideband's answers to commands rettui never sends).
    lxmf_core::constants::FIELD_RESULTS,
];

/// What a message carries besides its text and files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extras {
    pub reaction: Option<Reaction>,
    pub telemetry: Option<Telemetry>,
    pub commands: Vec<Command>,
    pub audio: Option<Audio>,
    /// Its sender stopped sharing their location.
    pub ceased: bool,
    /// How its text is written (the renderer field), if not plain.
    pub format: Option<crate::markdown::TextFormat>,
    /// Fields rettui doesn't know.
    pub unknown: Vec<u8>,
}

impl Extras {
    pub fn of(message: &LxMessage) -> Self {
        Self {
            reaction: reaction_of(message),
            telemetry: telemetry_of(message),
            commands: commands_of(message),
            audio: audio_of(message),
            ceased: ceased(message),
            format: value(message, FIELD_RENDERER)
                .and_then(|v| v.as_u64())
                .and_then(|r| u8::try_from(r).ok())
                .and_then(crate::markdown::TextFormat::from_renderer),
            unknown: message.fields.keys().copied().filter(|id| !KNOWN_FIELDS.contains(id)).collect(),
        }
    }

    /// What it says in words, beside the text: commands asked for,
    /// telemetry without a location, a location share that stopped. With
    /// `bare` (nothing else to show), fields rettui can't show are named.
    pub fn notes(&self, bare: bool) -> Vec<String> {
        let mut notes: Vec<String> = Vec::new();
        if self.telemetry.as_ref().is_some_and(|t| t.location.is_none()) {
            notes.push("Sent a telemetry update without a location".into());
        }
        if self.ceased {
            notes.push("Stopped sharing their location".into());
        }
        for command in &self.commands {
            let note = command.describe();
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
        if bare && notes.is_empty() && self.telemetry.is_none() && self.reaction.is_none() {
            notes.push(match self.unknown.as_slice() {
                [] => "Sent an empty message".into(),
                ids => {
                    let ids: Vec<String> = ids.iter().map(|id| format!("{id:#04x}")).collect();
                    format!("Sent something rettui can't show (LXMF field {})", ids.join(", "))
                }
            });
        }
        notes
    }
}

#[cfg(test)]
mod tests {
    use lxmf_core::message_api::DeliveryMethod;

    use super::*;

    fn message() -> LxMessage {
        LxMessage::new([1; 16], [2; 16], "", "", DeliveryMethod::Direct)
    }

    /// Through the wire and back, as a receiver sees it.
    fn received(message: &LxMessage) -> LxMessage {
        let mut signed = message.clone();
        let key = rns_runtime::prelude::Identity::new().get_signing_key().unwrap();
        signed.sign(&key).unwrap();
        LxMessage::unpack(&signed.pack().unwrap()).unwrap()
    }

    #[test]
    fn reactions_both_ways_and_from_older_columba() {
        let mut sent = message();
        let reaction = Reaction { to: [9; 32], emoji: "👍".into() };
        sent.set_msgpack_field(FIELD_REACTION, reaction_field(&reaction)).unwrap();
        let got = received(&sent);
        assert_eq!(reaction_of(&got), Some(reaction));
        // On the wire: {0x00: bin 32, 0x01: bin}, as Columba sends it.
        let packed = sent.pack_payload().unwrap();
        let mut wire = vec![0x40, 0x82, 0x00, 0xc4, 32];
        wire.extend([9; 32]);
        assert!(packed.windows(wire.len()).any(|w| w == wire.as_slice()));
        // Older Columba: string keys in field 0x10, the hash in hex.
        let mut old = message();
        let map = Value::Map(vec![
            (Value::from("reaction_to"), Value::from(hex::encode([7u8; 32]))),
            (Value::from("emoji"), Value::from("❤️")),
            (Value::from("sender"), Value::from("ignored")),
        ]);
        old.set_msgpack_field(FIELD_COLUMBA_EXTENSIONS, encode(&map)).unwrap();
        assert_eq!(reaction_of(&received(&old)), Some(Reaction { to: [7; 32], emoji: "❤️".into() }));
        // Nothing to react with, or no target: not a reaction.
        let mut empty = message();
        let map = Value::Map(vec![(Value::from(0), Value::Binary(vec![1; 32])), (Value::from(1), Value::Binary(b"\n ".to_vec()))]);
        empty.set_msgpack_field(FIELD_REACTION, encode(&map)).unwrap();
        assert_eq!(reaction_of(&received(&empty)), None);
        assert_eq!(clean_reaction("\u{1b}[31m🎉 "), Some("[31m🎉".into()));
    }

    /// Telemetry as Sideband packs it: a msgpack map, in bytes.
    fn telemetry(location: Value) -> LxMessage {
        let sensors = Value::Map(vec![(Value::from(1), Value::from(1_790_000_000)), (Value::from(2), location)]);
        let mut message = message();
        message.set_field(FIELD_TELEMETRY, encode(&sensors));
        message
    }

    #[test]
    fn locations_as_sideband_and_columba_send_them() {
        let int = |v: i32| Value::Binary(v.to_be_bytes().to_vec());
        let location = Value::Array(vec![
            int(51_507_350),
            int(-127_758),
            int(3_550),
            Value::Binary(250u32.to_be_bytes().to_vec()),
            int(9_000),
            Value::Binary(1_200u16.to_be_bytes().to_vec()),
            Value::from(1_789_999_990),
        ]);
        let got = telemetry_of(&received(&telemetry(location))).unwrap();
        assert_eq!(got.time, Some(1_790_000_000));
        let at = got.location.unwrap();
        assert_eq!((at.latitude, at.longitude), (51.50735, -0.127758));
        assert_eq!((at.altitude, at.speed, at.bearing, at.accuracy), (Some(35.5), Some(2.5), Some(90.0), Some(12.0)));
        assert_eq!(at.updated, Some(1_789_999_990));
        assert_eq!(at.label(), "51.50735, -0.12776 (±12 m)");
        assert!(at.map_url().contains("mlat=51.507350&mlon=-0.127758"));
        // Off the globe: no location (the telemetry is still there).
        let wrong = Value::Array(vec![int(95_000_000), int(0)]);
        let got = telemetry_of(&received(&telemetry(wrong))).unwrap();
        assert_eq!(got.location, None);
        let extras = Extras { telemetry: Some(got), ..Extras::default() };
        assert_eq!(extras.notes(true), ["Sent a telemetry update without a location"]);
    }

    #[test]
    fn commands_and_what_they_say() {
        let mut ping = message();
        let list = Value::Array(vec![
            Value::Map(vec![(Value::from(0x02), Value::Array(Vec::new()))]),
            Value::Map(vec![(Value::from(0x01), Value::Array(vec![Value::from(0), Value::from(false)]))]),
            Value::Map(vec![(Value::from(0x33), Value::Nil)]),
        ]);
        ping.set_msgpack_field(FIELD_COMMANDS, encode(&list)).unwrap();
        let extras = Extras::of(&received(&ping));
        assert_eq!(extras.commands, [Command::Ping, Command::TelemetryRequest, Command::Other(0x33)]);
        let notes = extras.notes(true);
        assert_eq!(notes.len(), 3);
        assert!(notes[0].starts_with("Pinged you") && notes[2].contains("0x33"), "{notes:?}");
    }

    #[test]
    fn voice_messages_and_unknown_fields() {
        let mut voice = message();
        voice.set_audio_field(0x10, b"OggS rest of the file").unwrap();
        let extras = Extras::of(&received(&voice));
        let audio = extras.audio.unwrap();
        assert!(audio.playable());
        assert_eq!((audio.codec().as_str(), audio.file_name().as_str()), ("Opus", "voice-message.ogg"));
        let codec2 = Audio { mode: 0x04, data: vec![1, 2, 3] };
        assert!(!codec2.playable());
        assert_eq!((codec2.codec().as_str(), codec2.file_name().as_str()), ("Codec2 1200", "voice-message-1200.codec2"));
        // A field nobody knows, on an empty message, is named; an icon's
        // appearance isn't.
        let mut odd = message();
        odd.set_field(0x77, vec![1]);
        odd.set_msgpack_field(FIELD_ICON_APPEARANCE, encode(&Value::Array(Vec::new()))).unwrap();
        let extras = Extras::of(&received(&odd));
        assert_eq!(extras.unknown, [0x77]);
        assert_eq!(extras.notes(true), ["Sent something rettui can't show (LXMF field 0x77)"]);
        assert!(extras.notes(false).is_empty());
        assert_eq!(Extras::default().notes(true), ["Sent an empty message"]);
    }

    #[test]
    fn a_stopped_location_share() {
        let mut stop = message();
        let meta = encode(&Value::Map(vec![(Value::from("cease"), Value::from(true))]));
        stop.set_field(FIELD_CUSTOM_META, meta);
        assert!(ceased(&received(&stop)));
        assert!(!ceased(&received(&message())));
    }
}
