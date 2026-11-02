//! Interfaces others make known: the ones interface discovery finds (an
//! entry point, an RNode's radio settings, an I2P address), and the ones
//! Sideband 2.x sends in its telemetry (its *Interfaces* sensor, `0x1B`),
//! which say the same things in the same keys. Both are listed, put on the
//! map when they say where they are, and can be added to the Reticulum
//! config by hand.
//!
//! Anyone can announce an interface or send telemetry, so what's read is
//! capped, and nothing is connected to or added without being asked.

use rmpv::Value;
use rns_transport::discovery::{DiscoveredInterface, DiscoveryStatus};
use serde::{Deserialize, Serialize};

use crate::lxmf::Location;

/// The most interfaces read from one telemetry message.
pub const MOST_SENT: usize = 32;
/// The most interfaces discovery lists (it keeps every one heard, for days).
pub const MOST_FOUND: usize = 500;
/// Longest text kept of a name, an address or a network name.
const LONGEST: usize = 128;

/// Keys in a discovery announce, and in Sideband's Interfaces sensor
/// (RNS.Discovery's).
mod key {
    pub const INTERFACE_TYPE: u64 = 0x00;
    pub const TRANSPORT: u64 = 0x01;
    pub const REACHABLE_ON: u64 = 0x02;
    pub const LATITUDE: u64 = 0x03;
    pub const LONGITUDE: u64 = 0x04;
    pub const HEIGHT: u64 = 0x05;
    pub const PORT: u64 = 0x06;
    pub const IFAC_NETNAME: u64 = 0x07;
    pub const IFAC_NETKEY: u64 = 0x08;
    pub const FREQUENCY: u64 = 0x09;
    pub const BANDWIDTH: u64 = 0x0A;
    pub const SPREADINGFACTOR: u64 = 0x0B;
    pub const CODINGRATE: u64 = 0x0C;
    pub const MODULATION: u64 = 0x0D;
    pub const CHANNEL: u64 = 0x0E;
    /// Sideband puts the network's id one below the transport id.
    pub const NETWORK_ID: u64 = 0xFD;
    pub const TRANSPORT_ID: u64 = 0xFE;
}

/// An interface someone made known.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FoundInterface {
    /// As it's announced.
    pub name: String,
    /// Reticulum's type name, as `BackboneInterface` or `RNodeInterface`.
    pub kind: String,
    /// Whether the node it's on passes traffic on for others.
    #[serde(default)]
    pub transport: bool,
    /// The node's transport identity, in hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    /// A host name or IP address (an I2P one for I2P).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable_on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    /// Metres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// IFAC: the network's name and passphrase, if it has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    /// Radio settings: Hz, Hz, and LoRa's spreading factor and coding rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spreading_factor: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_rate: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modulation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u16>,
    /// What discovery knows of it (none for one sent in telemetry): how far
    /// away, its announce's stamp value, when it was first and last heard
    /// (Unix seconds), how often, and whether it's still about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hops: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp_value: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heard: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Text from the network, tidied: no control characters, not too long.
fn tidy(text: &str) -> Option<String> {
    let text: String = text.chars().filter(|c| !c.is_control()).take(LONGEST).collect();
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => tidy(s.as_str()?),
        Value::Binary(b) => tidy(std::str::from_utf8(b).ok()?),
        _ => None,
    }
}

fn float_of(value: &Value) -> Option<f64> {
    let n = match value {
        Value::F64(v) => *v,
        Value::F32(v) => f64::from(*v),
        Value::Integer(n) => n.as_f64()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// A 16-byte id, in hex.
fn id_of(value: &Value) -> Option<String> {
    match value {
        Value::Binary(b) if b.len() == 16 => Some(hex::encode(b)),
        _ => None,
    }
}

impl FoundInterface {
    /// One discovery found.
    pub fn from_discovered(record: &DiscoveredInterface) -> Self {
        let info = &record.info;
        let tidy_opt = |s: &Option<String>| s.as_deref().and_then(tidy);
        Self {
            name: tidy(&info.name).unwrap_or_else(|| "(unnamed)".into()),
            kind: tidy(&info.interface_type).unwrap_or_default(),
            transport: info.transport_enabled,
            transport_id: Some(hex::encode(info.transport_id)),
            network_id: (record.network_id != info.transport_id).then(|| hex::encode(record.network_id)),
            reachable_on: tidy_opt(&info.reachable_on),
            port: info.port,
            latitude: info.latitude.filter(|v| (-90.0..=90.0).contains(v)),
            longitude: info.longitude.filter(|v| (-180.0..=180.0).contains(v)),
            height: info.height.filter(|v| v.is_finite()),
            network_name: tidy_opt(&info.ifac_netname),
            passphrase: tidy_opt(&info.ifac_netkey),
            frequency: info.frequency,
            bandwidth: info.bandwidth,
            spreading_factor: info.spreading_factor,
            coding_rate: info.coding_rate,
            modulation: tidy_opt(&info.modulation),
            channel: info.channel,
            hops: Some(record.hops),
            stamp_value: Some(record.stamp_value),
            discovered: Some(record.discovered),
            last_heard: Some(record.last_heard),
            heard_count: Some(record.heard_count),
            status: record.status.map(|s| {
                match s {
                    DiscoveryStatus::Available => "available",
                    DiscoveryStatus::Unknown => "unknown",
                    DiscoveryStatus::Stale => "stale",
                }
                .to_string()
            }),
        }
    }

    /// The interfaces in Sideband's Interfaces sensor: a map of each one's
    /// name to its keys. Ones without a type are left out.
    pub fn from_sensor(value: &Value) -> Vec<Self> {
        let Value::Map(entries) = value else { return Vec::new() };
        entries
            .iter()
            .take(MOST_SENT)
            .filter_map(|(name, keys)| {
                let Value::Map(keys) = keys else { return None };
                let at = |k: u64| keys.iter().find(|(key, _)| key.as_u64() == Some(k)).map(|(_, v)| v);
                let text = |k: u64| at(k).and_then(text_of);
                let int = |k: u64| at(k).and_then(Value::as_u64);
                let float = |k: u64| at(k).and_then(float_of);
                Some(Self {
                    name: text_of(name).unwrap_or_else(|| "(unnamed)".into()),
                    kind: text(key::INTERFACE_TYPE)?,
                    transport: at(key::TRANSPORT).and_then(Value::as_bool).unwrap_or(false),
                    transport_id: at(key::TRANSPORT_ID).and_then(id_of),
                    network_id: at(key::NETWORK_ID).and_then(id_of),
                    reachable_on: text(key::REACHABLE_ON),
                    port: int(key::PORT).and_then(|p| u16::try_from(p).ok()),
                    latitude: float(key::LATITUDE).filter(|v| (-90.0..=90.0).contains(v)),
                    longitude: float(key::LONGITUDE).filter(|v| (-180.0..=180.0).contains(v)),
                    height: float(key::HEIGHT),
                    network_name: text(key::IFAC_NETNAME),
                    passphrase: text(key::IFAC_NETKEY),
                    frequency: int(key::FREQUENCY),
                    bandwidth: int(key::BANDWIDTH),
                    spreading_factor: int(key::SPREADINGFACTOR).and_then(|v| u8::try_from(v).ok()),
                    coding_rate: int(key::CODINGRATE).and_then(|v| u8::try_from(v).ok()),
                    modulation: text(key::MODULATION),
                    channel: int(key::CHANNEL).and_then(|v| u16::try_from(v).ok()),
                    ..Self::default()
                })
            })
            .collect()
    }

    /// Which one this is, the same each time it's heard (Reticulum's
    /// discovery hash: its node's transport id and its name), in hex.
    pub fn id(&self) -> String {
        let transport: [u8; 16] =
            self.transport_id.as_deref().and_then(|t| hex::decode(t).ok()).and_then(|b| b.try_into().ok()).unwrap_or_default();
        hex::encode(&rns_transport::discovery::discovery_hash(&transport, &self.name)[..16])
    }

    /// Its type, briefly: "TCP", "RNode", "I2P".
    pub fn kind_label(&self) -> &str {
        match self.kind.as_str() {
            "BackboneInterface" => "Backbone",
            "TCPServerInterface" | "TCPClientInterface" => "TCP",
            "RNodeInterface" => "RNode",
            "WeaveInterface" => "Weave",
            "I2PInterface" => "I2P",
            "KISSInterface" => "KISS",
            other => other.strip_suffix("Interface").unwrap_or(other),
        }
    }

    /// The radio's settings, as "868.1 MHz · 125 kHz · SF 8 · CR 5".
    pub fn radio(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(f) = self.frequency {
            parts.push(format!("{} MHz", trim(f as f64 / 1e6, 3)));
        }
        if let Some(b) = self.bandwidth {
            parts.push(format!("{} kHz", trim(b as f64 / 1e3, 1)));
        }
        if let Some(sf) = self.spreading_factor {
            parts.push(format!("SF {sf}"));
        }
        if let Some(cr) = self.coding_rate {
            parts.push(format!("CR {cr}"));
        }
        if let Some(m) = &self.modulation {
            parts.push(m.clone());
        }
        if let Some(c) = self.channel {
            parts.push(format!("channel {c}"));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// Where to reach it: `host:port`, an I2P address, or its radio.
    pub fn reach(&self) -> Option<String> {
        match (&self.reachable_on, self.port) {
            (Some(host), Some(port)) if host.contains(':') => Some(format!("[{host}]:{port}")),
            (Some(host), Some(port)) => Some(format!("{host}:{port}")),
            (Some(host), None) => Some(host.clone()),
            (None, _) => self.radio(),
        }
    }

    /// One line: "TCP · example.net:4242".
    pub fn summary(&self) -> String {
        match self.reach() {
            Some(reach) => format!("{} · {reach}", self.kind_label()),
            None => self.kind_label().to_string(),
        }
    }

    pub fn location(&self) -> Option<Location> {
        let mut location = Location::at(self.latitude?, self.longitude?);
        location.altitude = self.height;
        Some(location)
    }

    /// What's known of it, as lines of label and value.
    pub fn details(&self, now: u64) -> Vec<(&'static str, String)> {
        let mut lines = vec![("Type", self.kind_label().to_string())];
        if let Some(reach) = match (&self.reachable_on, self.port) {
            (Some(_), _) => self.reach(),
            _ => None,
        } {
            lines.push(("Reachable on", reach));
        }
        if let Some(radio) = self.radio() {
            lines.push(("Radio", radio));
        }
        lines.push(("Transport", if self.transport { "on: passes traffic on for others" } else { "off" }.to_string()));
        if let Some(id) = &self.transport_id {
            lines.push(("Transport id", id.clone()));
        }
        if let Some(name) = &self.network_name {
            lines.push(("Network name", name.clone()));
        }
        if self.passphrase.is_some() {
            lines.push(("Passphrase", "set (it's added with it)".into()));
        }
        if let Some(location) = self.location() {
            lines.push(("Location", location.label()));
        }
        if let Some(hops) = self.hops {
            lines.push(("Distance", format!("{hops} hop{}", if hops == 1 { "" } else { "s" })));
        }
        if let (Some(last), Some(count)) = (self.last_heard, self.heard_count) {
            let status = self.status.as_deref().map(|s| format!("{s}, ")).unwrap_or_default();
            lines.push((
                "Heard",
                format!("{status}{} ago, {count} time{}", span(now.saturating_sub(last)), if count == 1 { "" } else { "s" }),
            ));
        }
        if let Some(first) = self.discovered {
            lines.push(("First heard", format!("{} ago", span(now.saturating_sub(first)))));
        }
        if let Some(value) = self.stamp_value {
            lines.push(("Stamp value", value.to_string()));
        }
        lines
    }

    /// The interface to add to the Reticulum config for it: its type and
    /// keys, with values written as the config file has them, and whether
    /// it's added off (an RNode, which needs its port set first). Only
    /// types rettui can connect to, at addresses that aren't this device.
    pub fn config(&self) -> Result<ConfigEntry, String> {
        use crate::reticulum::doc::format_value;
        let mut keys = Vec::new();
        let (kind, off) = match self.kind.as_str() {
            "BackboneInterface" | "TCPServerInterface" => {
                let host = self.reachable_on.as_deref().filter(|h| host_ok(h)).ok_or("It doesn't say a host that can be connected to")?;
                if !super::discovered::reachable(host) {
                    return Err(format!("{host} is this device, or a network rettui doesn't connect to"));
                }
                let port = self.port.filter(|p| *p > 0).ok_or("It doesn't say which port")?;
                keys.push(("target_host", host.to_string()));
                keys.push(("target_port", port.to_string()));
                ("TCPClientInterface", false)
            }
            "I2PInterface" => {
                let peer = self.reachable_on.as_deref().filter(|h| h.len() >= 52 && host_ok(h)).ok_or("It doesn't say an I2P address")?;
                keys.push(("peers", format!("{},", format_value(peer))));
                ("I2PInterface", false)
            }
            "RNodeInterface" => {
                let (Some(f), Some(b), Some(sf), Some(cr)) = (self.frequency, self.bandwidth, self.spreading_factor, self.coding_rate)
                else {
                    return Err("It doesn't say all its radio settings".into());
                };
                keys.push(("frequency", f.to_string()));
                keys.push(("bandwidth", b.to_string()));
                keys.push(("spreadingfactor", sf.to_string()));
                keys.push(("codingrate", cr.to_string()));
                ("RNodeInterface", true)
            }
            _ => return Err(format!("A {} interface can't be added this way", self.kind_label())),
        };
        if let Some(name) = &self.network_name {
            keys.push(("network_name", format_value(name)));
        }
        if let Some(passphrase) = &self.passphrase {
            keys.push(("passphrase", format_value(passphrase)));
        }
        Ok(ConfigEntry { kind, keys, off })
    }

    /// A name for it in the config: its own, without what a section name
    /// can't have.
    pub fn config_name(&self) -> String {
        let name: String = self.name.chars().filter(|c| !matches!(c, '[' | ']' | '#' | '"' | '\'')).collect();
        let name = name.trim();
        if name.is_empty() { format!("{} {}", self.kind_label(), &self.id()[..8]) } else { name.to_string() }
    }
}

/// An interface to add to the Reticulum config: its type, its keys (values
/// as the file has them), and whether it's added off.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigEntry {
    pub kind: &'static str,
    pub keys: Vec<(&'static str, String)>,
    pub off: bool,
}

/// A host name, or an IPv4, IPv6 or I2P address: nothing else.
fn host_ok(host: &str) -> bool {
    !host.is_empty() && host.len() <= 255 && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
}

/// A number with at most `places` decimals, no trailing zeros.
fn trim(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text }
}

/// How long, in words: "40 s", "3 min", "5 h", "6 days".
fn span(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        3600..172_800 => format!("{} h", secs / 3600),
        _ => format!("{} days", secs / 86_400),
    }
}

/// What discovery has found, newest first, at most [`MOST_FOUND`].
pub fn listed(records: &[DiscoveredInterface]) -> Vec<FoundInterface> {
    let mut found: Vec<FoundInterface> = records.iter().map(FoundInterface::from_discovered).collect();
    found.sort_by(|a, b| b.last_heard.cmp(&a.last_heard).then_with(|| a.name.cmp(&b.name)));
    found.truncate(MOST_FOUND);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sideband's Interfaces sensor, as it packs it.
    fn sensor() -> Value {
        let entry = |pairs: Vec<(u64, Value)>| Value::Map(pairs.into_iter().map(|(k, v)| (Value::from(k), v)).collect());
        Value::Map(vec![
            (
                Value::from("Hill"),
                entry(vec![
                    (key::INTERFACE_TYPE, Value::from("BackboneInterface")),
                    (key::TRANSPORT, Value::from(true)),
                    (key::TRANSPORT_ID, Value::Binary(vec![0xAB; 16])),
                    (key::REACHABLE_ON, Value::from("hill.example.net")),
                    (key::PORT, Value::from(4242)),
                    (key::LATITUDE, Value::F64(51.5)),
                    (key::LONGITUDE, Value::F64(-0.12)),
                    (key::IFAC_NETNAME, Value::from("valley")),
                    (key::IFAC_NETKEY, Value::from("s3cret")),
                ]),
            ),
            (
                Value::from("Radio"),
                entry(vec![
                    (key::INTERFACE_TYPE, Value::from("RNodeInterface")),
                    (key::FREQUENCY, Value::from(868_100_000u64)),
                    (key::BANDWIDTH, Value::from(125_000u64)),
                    (key::SPREADINGFACTOR, Value::from(8)),
                    (key::CODINGRATE, Value::from(5)),
                ]),
            ),
            // No type: left out.
            (Value::from("Odd"), entry(vec![(key::PORT, Value::from(1))])),
            (Value::from("Not a map"), Value::from(3)),
        ])
    }

    #[test]
    fn sidebands_interfaces_sensor_is_read() {
        let found = FoundInterface::from_sensor(&sensor());
        assert_eq!(found.len(), 2);
        let hill = &found[0];
        assert_eq!((hill.name.as_str(), hill.summary().as_str()), ("Hill", "Backbone · hill.example.net:4242"));
        assert_eq!(hill.transport_id.as_deref(), Some("ab".repeat(16).as_str()));
        assert_eq!(hill.location().map(|l| (l.latitude, l.longitude)), Some((51.5, -0.12)));
        assert_eq!(found[1].summary(), "RNode · 868.1 MHz · 125 kHz · SF 8 · CR 5");
        assert!(found[1].location().is_none());
        // The same one is the same id each time, and another isn't.
        assert_eq!(hill.id(), FoundInterface::from_sensor(&sensor())[0].id());
        assert_ne!(hill.id(), found[1].id());
    }

    #[test]
    fn what_comes_from_the_network_is_capped_and_tidied() {
        let many: Vec<(Value, Value)> = (0..100)
            .map(|i| {
                (
                    Value::from(format!("n{i}\n{}", "x".repeat(500))),
                    Value::Map(vec![(Value::from(key::INTERFACE_TYPE), Value::from("TCPServerInterface"))]),
                )
            })
            .collect();
        let found = FoundInterface::from_sensor(&Value::Map(many));
        assert_eq!(found.len(), MOST_SENT);
        assert!(found.iter().all(|f| f.name.chars().count() <= LONGEST && !f.name.contains('\n')));
        assert!(FoundInterface::from_sensor(&Value::from("nonsense")).is_empty());
        // A place off the globe is no place.
        let off = Value::Map(vec![(
            Value::from("Off"),
            Value::Map(vec![
                (Value::from(key::INTERFACE_TYPE), Value::from("TCPServerInterface")),
                (Value::from(key::LATITUDE), Value::F64(200.0)),
                (Value::from(key::LONGITUDE), Value::F64(10.0)),
            ]),
        )]);
        assert!(FoundInterface::from_sensor(&off)[0].location().is_none());
    }

    #[test]
    fn config_for_an_entry_point_a_radio_and_what_cant_be_added() {
        let found = FoundInterface::from_sensor(&sensor());
        let ConfigEntry { kind, keys, off } = found[0].config().unwrap();
        assert_eq!((kind, off), ("TCPClientInterface", false));
        assert!(keys.contains(&("target_host", "hill.example.net".into())) && keys.contains(&("target_port", "4242".into())));
        assert!(keys.contains(&("network_name", "valley".into())) && keys.contains(&("passphrase", "s3cret".into())));
        let ConfigEntry { kind, keys, off } = found[1].config().unwrap();
        assert_eq!((kind, off), ("RNodeInterface", true));
        assert!(keys.contains(&("frequency", "868100000".into())));
        // This device, an address that's no host, and a type rettui doesn't
        // add are refused.
        let at = |host: &str| FoundInterface { reachable_on: Some(host.into()), ..found[0].clone() };
        assert!(at("127.0.0.1").config().is_err() && at("localhost").config().is_err());
        assert!(at("evil.example\n[[x]]").config().is_err());
        let weave = FoundInterface { kind: "WeaveInterface".into(), ..found[1].clone() };
        assert!(weave.config().unwrap_err().contains("Weave"));
        // A name the config can't have is tidied.
        let odd = FoundInterface { name: "[Hill] #1".into(), ..found[0].clone() };
        assert_eq!(odd.config_name(), "Hill 1");
    }
}
