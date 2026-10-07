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
//!   maps, as Sideband sends telemetry requests, pings, echoes and signal
//!   report requests. rettui says what was asked, and answers pings, echoes
//!   and signal reports as Sideband does if it's set to (`answer_commands`).
//! - Voice messages (`FIELD_AUDIO`, 0x07): `[mode, bytes]`, Opus (in an Ogg
//!   file) or Codec2.
//! - Icons (`FIELD_ICON_APPEARANCE`, 0x04): `[name, foreground, background]`,
//!   a Material Design Icon's name and two colours of three bytes (red,
//!   green, blue), as Sideband and MeshChat send them. rettui sends its own
//!   if one's set.

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

/// Someone's icon: a Material Design Icon by name (see `crate::icons`), in
/// a colour on a colour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    pub icon: String,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
}

/// A colour sent as three bytes, or (to be lenient) as `#rrggbb`.
fn colour_of(value: &Value) -> Option<[u8; 3]> {
    match value {
        Value::Binary(bytes) => bytes.as_slice().try_into().ok(),
        Value::String(text) => crate::icons::parse_colour(text.as_str()?),
        _ => None,
    }
}

/// The icon a message carries, if it's well formed: a name of letters,
/// digits and dashes, and two colours.
pub fn appearance_of(message: &LxMessage) -> Option<Appearance> {
    let Value::Array(parts) = value(message, FIELD_ICON_APPEARANCE)? else { return None };
    let [name, foreground, background, ..] = parts.as_slice() else { return None };
    let icon = String::from_utf8(bytes_of(name)?).ok()?.trim().to_lowercase();
    let fine = |c: char| c.is_ascii_alphanumeric() || c == '-';
    if icon.is_empty() || icon.len() > crate::icons::MAX_NAME || !icon.chars().all(fine) {
        return None;
    }
    Some(Appearance { icon, foreground: colour_of(foreground)?, background: colour_of(background)? })
}

/// `FIELD_ICON_APPEARANCE`'s value for an icon.
pub fn appearance_field(appearance: &Appearance) -> Vec<u8> {
    encode(&Value::Array(vec![
        Value::from(appearance.icon.as_str()),
        Value::Binary(appearance.foreground.to_vec()),
        Value::Binary(appearance.background.to_vec()),
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

    /// Just where: no height, speed, heading or accuracy.
    pub fn at(latitude: f64, longitude: f64) -> Self {
        Self { latitude, longitude, altitude: None, speed: None, bearing: None, accuracy: None, updated: None }
    }

    /// A location as typed: latitude and longitude in degrees, apart by a
    /// comma or a space (`51.5074, -0.1278`), or a `geo:` link.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix("geo:").map_or(text, |rest| rest.split([';', '?']).next().unwrap_or(""));
        let mut parts = text.split([',', ' ']).filter(|p| !p.is_empty()).map(|p| p.trim().parse::<f64>());
        let (Some(Ok(latitude)), Some(Ok(longitude))) = (parts.next(), parts.next()) else { return None };
        // An altitude may follow a `geo:` link's two; nothing else may.
        if parts.next().is_some_and(|p| p.is_err()) || parts.next().is_some() {
            return None;
        }
        let fine = latitude.is_finite() && longitude.is_finite();
        (fine && (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude))
            .then(|| Self::at(latitude, longitude))
    }

    /// How far it is to `other`, in metres (along the ground, the Earth
    /// taken as round).
    pub fn distance_to(&self, other: &Location) -> f64 {
        let (a, b) = (self.latitude.to_radians(), other.latitude.to_radians());
        let dlat = b - a;
        let dlon = (other.longitude - self.longitude).to_radians();
        let h = (dlat / 2.0).sin().powi(2) + a.cos() * b.cos() * (dlon / 2.0).sin().powi(2);
        2.0 * EARTH_RADIUS * h.sqrt().min(1.0).asin()
    }

    /// Which way `other` is from here, in degrees from north (setting out).
    pub fn bearing_to(&self, other: &Location) -> f64 {
        let (a, b) = (self.latitude.to_radians(), other.latitude.to_radians());
        let dlon = (other.longitude - self.longitude).to_radians();
        let y = dlon.sin() * b.cos();
        let x = a.cos() * b.sin() - a.sin() * b.cos() * dlon.cos();
        y.atan2(x).to_degrees().rem_euclid(360.0)
    }

    /// How far and which way `other` is: `350 m NE`, `12.4 km S`.
    pub fn away(&self, other: &Location) -> String {
        let metres = self.distance_to(other);
        let distance = match metres {
            m if m < 1000.0 => format!("{m:.0} m"),
            m if m < 100_000.0 => format!("{:.1} km", m / 1000.0),
            m => format!("{:.0} km", m / 1000.0),
        };
        if metres < 1.0 {
            return "here".into();
        }
        const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
        let point = POINTS[((self.bearing_to(other) + 22.5) / 45.0) as usize % 8];
        format!("{distance} {point}")
    }
}

/// The Earth's mean radius, in metres.
const EARTH_RADIUS: f64 = 6_371_008.8;

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

/// `FIELD_TELEMETRY`'s value for a location, as Sideband packs it (and
/// Columba reads it): sensor readings in msgpack, in bytes, with the time
/// and the location, all seven of its parts there (Sideband reads none if
/// one is missing), each a big-endian integer. What isn't known is 0.
pub fn telemetry_field(location: &Location, time: i64) -> Vec<u8> {
    let int = |v: Option<f64>, scale: f64| (v.unwrap_or(0.0) * scale).round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let unsigned = |v: Option<f64>| (v.unwrap_or(0.0) * 1e2).round().clamp(0.0, f64::from(u32::MAX)) as u32;
    let accuracy = (location.accuracy.unwrap_or(0.0) * 1e2).round().clamp(0.0, f64::from(u16::MAX)) as u16;
    let reading = Value::Array(vec![
        Value::Binary(int(Some(location.latitude), 1e6).to_be_bytes().to_vec()),
        Value::Binary(int(Some(location.longitude), 1e6).to_be_bytes().to_vec()),
        Value::Binary(int(location.altitude, 1e2).to_be_bytes().to_vec()),
        Value::Binary(unsigned(location.speed).to_be_bytes().to_vec()),
        Value::Binary(int(location.bearing, 1e2).to_be_bytes().to_vec()),
        Value::Binary(accuracy.to_be_bytes().to_vec()),
        Value::from(location.updated.unwrap_or(time)),
    ]);
    encode(&Value::Map(vec![(Value::from(SENSOR_TIME), Value::from(time)), (Value::from(SENSOR_LOCATION), reading)]))
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    TelemetryRequest,
    Ping,
    /// What to send back.
    Echo(String),
    SignalReport,
    /// A plugin's, or one rettui doesn't know.
    Other(u64),
}

impl Command {
    fn from(id: u64, argument: &Value) -> Self {
        match id {
            0x01 => Command::TelemetryRequest,
            0x02 => Command::Ping,
            0x03 => Command::Echo(bytes_of(argument).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()),
            0x04 => Command::SignalReport,
            other => Command::Other(other),
        }
    }

    /// What it asked, in words.
    pub fn describe(&self) -> String {
        match self {
            Command::TelemetryRequest => "Asked for your location".into(),
            Command::Ping => "Pinged you with a command".into(),
            Command::Echo(text) => format!("Asked for an echo of “{}”", text.trim()),
            Command::SignalReport => "Asked for a signal report".into(),
            Command::Other(id) => format!("Sent a command rettui doesn't run ({id:#04x})"),
        }
    }

    /// The answer Sideband gives, for those rettui answers: a ping's, an
    /// echo's, and a signal report's (rettui isn't given the readings of a
    /// message received, so it says so, as Sideband does without them).
    pub fn answer(&self) -> Option<String> {
        match self {
            Command::Ping => Some("Ping reply".into()),
            Command::Echo(text) => Some(format!("Echo reply: {text}")),
            Command::SignalReport => Some("No reception info available".into()),
            Command::TelemetryRequest | Command::Other(_) => None,
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
        .filter_map(|(id, argument)| id.as_u64().map(|id| Command::from(id, argument)))
        .collect()
}

/// What's said of a message saying its sender stopped sharing their
/// location.
pub const STOPPED_SHARING: &str = "Stopped sharing their location";

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
    /// A Codec2 recording decoded to a WAV file, if its mode can be (see
    /// [`Audio::decoded`]).
    pub wav: Option<Vec<u8>>,
}

impl Audio {
    pub fn new(mode: u8, data: Vec<u8>) -> Self {
        Self { mode, data, wav: None }
    }

    /// With a Codec2 recording decoded, if its mode can be. Ten minutes
    /// take about a second: not for the async threads.
    pub fn decoded(self) -> Self {
        let wav = super::voice::codec2_wav(self.mode, &self.data);
        Self { wav, ..self }
    }

    /// An Ogg file (Opus in its container), or Codec2 decoded to WAV: what
    /// players can open.
    pub fn playable(&self) -> bool {
        self.data.starts_with(b"OggS") || self.wav.is_some()
    }

    /// The file it's saved as, and what's in it.
    pub fn file(&self) -> (String, &[u8]) {
        (self.file_name(), self.wav.as_deref().unwrap_or(&self.data))
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

    /// The file it's saved as: Ogg, Codec2 decoded to WAV, or Codec2 as it
    /// came if its mode can't be decoded.
    pub fn file_name(&self) -> String {
        if self.data.starts_with(b"OggS") {
            "voice-message.ogg".into()
        } else if self.wav.is_some() {
            "voice-message.wav".into()
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
    (!field.bytes.is_empty()).then(|| Audio::new(field.mode, field.bytes.to_vec()))
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
    /// Its sender's icon.
    pub appearance: Option<Appearance>,
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
            appearance: appearance_of(message),
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
            notes.push(STOPPED_SHARING.into());
        }
        for command in &self.commands {
            let note = command.describe();
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
        // Their icon on its own isn't a message to mention.
        if bare && notes.is_empty() && self.telemetry.is_none() && self.reaction.is_none() && self.appearance.is_none() {
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
    fn a_location_sent_as_sideband_packs_it() {
        let at = Location { altitude: Some(35.5), accuracy: Some(12.0), ..Location::at(51.50735, -0.127758) };
        let mut sent = message();
        sent.set_field(FIELD_TELEMETRY, telemetry_field(&at, 1_790_000_000));
        let got = telemetry_of(&received(&sent)).unwrap();
        assert_eq!(got.time, Some(1_790_000_000));
        let back = got.location.unwrap();
        assert_eq!((back.latitude, back.longitude, back.altitude, back.accuracy), (51.50735, -0.127758, Some(35.5), Some(12.0)));
        // What isn't known goes as 0, so Sideband reads all of it.
        assert_eq!((back.speed, back.bearing, back.updated), (Some(0.0), Some(0.0), Some(1_790_000_000)));
        // In bytes, as Sideband sends it: {1: time, 2: [bin 4, bin 4, ...]}.
        let Some(Value::Binary(packed)) = value(&received(&sent), FIELD_TELEMETRY) else { panic!("not bytes") };
        assert_eq!(&packed[..2], [0x82, 0x01]);
        assert!(packed.windows(6).any(|w| w == [0xc4, 4, 0x03, 0x11, 0xf0, 0x96]));
        // An accuracy too big for its two bytes is as big as they hold.
        let rough = Location { accuracy: Some(5000.0), ..at };
        let mut sent = message();
        sent.set_field(FIELD_TELEMETRY, telemetry_field(&rough, 1));
        assert_eq!(telemetry_of(&received(&sent)).unwrap().location.unwrap().accuracy, Some(655.35));
    }

    #[test]
    fn locations_typed_and_how_far_apart() {
        let london = Location::parse("51.5074, -0.1278").unwrap();
        assert_eq!((london.latitude, london.longitude), (51.5074, -0.1278));
        assert_eq!(Location::parse("51.5074 -0.1278"), Some(london));
        assert_eq!(Location::parse(" geo:51.5074,-0.1278;u=35 "), Some(london));
        assert_eq!(Location::parse("geo:51.5074,-0.1278,20"), Some(london));
        for wrong in ["", "51.5", "north", "95, 0", "0, 181", "1, 2, 3, 4", "1, 2, x", "NaN, 0"] {
            assert_eq!(Location::parse(wrong), None, "{wrong}");
        }
        let paris = Location::at(48.8566, 2.3522);
        let km = london.distance_to(&paris) / 1000.0;
        assert!((km - 343.5).abs() < 1.0, "{km}");
        let bearing = london.bearing_to(&paris);
        assert!((bearing - 148.1).abs() < 1.0, "{bearing}");
        assert_eq!(london.away(&paris), "344 km SE");
        assert_eq!(paris.away(&london), "344 km NW");
        let near = Location::at(51.5074, -0.1228);
        assert_eq!(london.away(&near), "346 m E");
        assert_eq!(london.away(&Location::at(51.6, -0.1278)), "10.3 km N");
        assert_eq!(london.away(&london), "here");
    }

    #[test]
    fn commands_and_what_they_say() {
        let mut ping = message();
        // As Sideband sends them: a ping, a telemetry request, an echo (its
        // text as bytes), a signal report request, and a plugin's.
        let list = Value::Array(vec![
            Value::Map(vec![(Value::from(0x02), Value::Boolean(true))]),
            Value::Map(vec![(Value::from(0x01), Value::Array(vec![Value::from(0), Value::from(false)]))]),
            Value::Map(vec![(Value::from(0x03), Value::Binary(b"anyone there?".to_vec()))]),
            Value::Map(vec![(Value::from(0x04), Value::Boolean(true))]),
            Value::Map(vec![(Value::from(0x33), Value::Nil)]),
        ]);
        ping.set_msgpack_field(FIELD_COMMANDS, encode(&list)).unwrap();
        let extras = Extras::of(&received(&ping));
        let echo = Command::Echo("anyone there?".into());
        assert_eq!(extras.commands, [Command::Ping, Command::TelemetryRequest, echo.clone(), Command::SignalReport, Command::Other(0x33)]);
        let notes = extras.notes(true);
        assert_eq!(notes.len(), 5);
        assert!(notes[0].starts_with("Pinged you") && notes[2].contains("“anyone there?”") && notes[4].contains("0x33"), "{notes:?}");
        // Sideband's answers, for those rettui answers.
        let answers: Vec<Option<String>> = extras.commands.iter().map(Command::answer).collect();
        assert_eq!(answers, [
            Some("Ping reply".into()),
            None,
            Some("Echo reply: anyone there?".into()),
            Some("No reception info available".into()),
            None,
        ]);
    }

    #[test]
    fn voice_messages_and_unknown_fields() {
        let mut voice = message();
        voice.set_audio_field(0x10, b"OggS rest of the file").unwrap();
        let extras = Extras::of(&received(&voice));
        let audio = extras.audio.unwrap();
        assert!(audio.playable());
        assert_eq!((audio.codec().as_str(), audio.file_name().as_str()), ("Opus", "voice-message.ogg"));
        // Codec2 that decodes is saved as WAV; what doesn't, as it came.
        let codec2 = Audio::new(0x04, vec![0; 12]).decoded();
        assert!(codec2.playable());
        assert_eq!((codec2.codec().as_str(), codec2.file_name().as_str()), ("Codec2 1200", "voice-message.wav"));
        assert!(super::super::voice::is_wav(codec2.file().1));
        let c700 = Audio::new(0x03, vec![0; 12]).decoded();
        assert!(!c700.playable());
        assert_eq!((c700.file_name().as_str(), c700.file().1), ("voice-message-700c.codec2", &[0u8; 12][..]));
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

    #[test]
    fn icons_as_sideband_and_meshchat_send_them() {
        let tower = Appearance { icon: "radio-tower".into(), foreground: [1, 2, 3], background: [250, 251, 252] };
        let mut sent = message();
        sent.set_msgpack_field(FIELD_ICON_APPEARANCE, appearance_field(&tower)).unwrap();
        assert_eq!(appearance_of(&received(&sent)), Some(tower.clone()));
        // On the wire: {0x04: [str, bin 3, bin 3]}, as Python's msgpack packs
        // Sideband's [icon, fg, bg].
        let packed = sent.pack_payload().unwrap();
        let mut wire = vec![0x04, 0x93, 0xab];
        wire.extend(b"radio-tower");
        wire.extend([0xc4, 3, 1, 2, 3, 0xc4, 3, 250, 251, 252]);
        assert!(packed.windows(wire.len()).any(|w| w == wire.as_slice()));
        // Colours as text are taken too; a name that can't be an icon's, or
        // a colour that isn't one, isn't.
        let icon = |name: &str, fg: Value| {
            let mut m = message();
            let parts = Value::Array(vec![Value::from(name), fg, Value::Binary(vec![0, 0, 0])]);
            m.set_msgpack_field(FIELD_ICON_APPEARANCE, encode(&parts)).unwrap();
            appearance_of(&received(&m))
        };
        assert_eq!(icon("Account", Value::from("#ffffff")).map(|a| (a.icon, a.foreground)), Some(("account".into(), [255; 3])));
        assert_eq!(icon("no good!", Value::Binary(vec![0, 0, 0])), None);
        assert_eq!(icon("account", Value::Binary(vec![0, 0])), None);
        // Known: an icon alone isn't "something rettui can't show".
        let extras = Extras::of(&received(&sent));
        assert_eq!(extras.appearance, Some(tower));
        assert!(extras.notes(true).is_empty());
    }
}
