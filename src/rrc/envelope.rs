//! The RRC envelope: a CBOR map with small integer keys.

use std::time::{SystemTime, UNIX_EPOCH};

use ciborium::value::{Integer, Value};

pub const VERSION: u64 = 1;
pub const DEFAULT_ASPECT: &str = "rrc.hub";

/// Message types.
pub mod t {
    pub const HELLO: u64 = 1;
    pub const WELCOME: u64 = 2;
    pub const JOIN: u64 = 10;
    pub const JOINED: u64 = 11;
    pub const PART: u64 = 12;
    pub const PARTED: u64 = 13;
    pub const MSG: u64 = 20;
    pub const NOTICE: u64 = 21;
    pub const ACTION: u64 = 22;
    pub const PING: u64 = 30;
    pub const PONG: u64 = 31;
    pub const ERROR: u64 = 40;
    pub const RESOURCE_ENVELOPE: u64 = 50;
}

// Envelope keys.
const K_V: u64 = 0;
const K_T: u64 = 1;
const K_ID: u64 = 2;
const K_TS: u64 = 3;
const K_SRC: u64 = 4;
const K_ROOM: u64 = 5;
const K_BODY: u64 = 6;
const K_NICK: u64 = 7;
const K_DST: u64 = 8;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn key(k: u64) -> Value {
    Value::Integer(Integer::from(k))
}

pub(super) fn as_u64(value: &Value) -> Option<u64> {
    value.as_integer().and_then(|i| u64::try_from(i).ok())
}

pub(super) fn get(map: &[(Value, Value)], k: u64) -> Option<&Value> {
    map.iter()
        .find(|(key, _)| as_u64(key) == Some(k))
        .map(|(_, value)| value)
}

pub(super) fn bytes(value: &Value) -> Option<Vec<u8>> {
    value.as_bytes().cloned()
}

pub(super) fn text(value: &Value) -> Option<String> {
    value.as_text().map(str::to_string)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub t: u64,
    pub id: Vec<u8>,
    pub ts: u64,
    /// Sender identity hash (the hub rewrites this to the real sender).
    pub src: Vec<u8>,
    pub room: Option<String>,
    pub body: Option<Value>,
    pub nick: Option<String>,
    /// Direct-notice recipient identity (rrcd extension).
    pub dst: Option<Vec<u8>>,
}

impl Envelope {
    pub fn new(t: u64, src: &[u8]) -> Self {
        Self {
            t,
            id: rand::random::<[u8; 8]>().to_vec(),
            ts: now_ms(),
            src: src.to_vec(),
            room: None,
            body: None,
            nick: None,
            dst: None,
        }
    }

    pub fn room(mut self, room: &str) -> Self {
        self.room = Some(room.to_string());
        self
    }

    pub fn body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }

    pub fn text(self, body: &str) -> Self {
        self.body(Value::Text(body.to_string()))
    }

    pub fn nick(mut self, nick: Option<&str>) -> Self {
        self.nick = nick.filter(|n| !n.is_empty()).map(str::to_string);
        self
    }

    pub fn dst(mut self, dst: &[u8]) -> Self {
        self.dst = Some(dst.to_vec());
        self
    }

    pub fn body_text(&self) -> Option<&str> {
        self.body.as_ref().and_then(Value::as_text)
    }

    /// Hashes carried in a JOINED/PARTED body.
    pub fn body_hashes(&self) -> Vec<Vec<u8>> {
        match &self.body {
            Some(Value::Array(items)) => items.iter().filter_map(bytes).collect(),
            _ => Vec::new(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut map = vec![
            (key(K_V), key(VERSION)),
            (key(K_T), key(self.t)),
            (key(K_ID), Value::Bytes(self.id.clone())),
            (key(K_TS), key(self.ts)),
            (key(K_SRC), Value::Bytes(self.src.clone())),
        ];
        if let Some(room) = &self.room {
            map.push((key(K_ROOM), Value::Text(room.clone())));
        }
        if let Some(body) = &self.body {
            map.push((key(K_BODY), body.clone()));
        }
        if let Some(nick) = &self.nick {
            map.push((key(K_NICK), Value::Text(nick.clone())));
        }
        if let Some(dst) = &self.dst {
            map.push((key(K_DST), Value::Bytes(dst.clone())));
        }
        let mut out = Vec::new();
        ciborium::into_writer(&Value::Map(map), &mut out).expect("writing to a Vec cannot fail");
        out
    }

    pub fn decode(data: &[u8]) -> Result<Self, String> {
        let value: Value = ciborium::from_reader(data).map_err(|e| format!("bad CBOR: {e}"))?;
        let Value::Map(map) = value else {
            return Err("envelope is not a map".into());
        };
        let version = get(&map, K_V).and_then(as_u64).ok_or("missing version")?;
        if version != VERSION {
            return Err(format!("unsupported RRC version {version}"));
        }
        Ok(Self {
            t: get(&map, K_T).and_then(as_u64).ok_or("missing type")?,
            id: get(&map, K_ID).and_then(bytes).ok_or("missing id")?,
            ts: get(&map, K_TS).and_then(as_u64).unwrap_or(0),
            src: get(&map, K_SRC).and_then(bytes).ok_or("missing sender")?,
            room: get(&map, K_ROOM).and_then(text),
            body: get(&map, K_BODY).cloned(),
            nick: get(&map, K_NICK).and_then(text),
            dst: get(&map, K_DST).and_then(bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip() {
        let env = Envelope::new(t::MSG, &[7; 16])
            .room("general")
            .text("hello")
            .nick(Some("zev"))
            .dst(&[9; 16]);
        let back = Envelope::decode(&env.encode()).unwrap();
        assert_eq!(back, env);
        assert_eq!(back.body_text(), Some("hello"));
    }

    #[test]
    fn decodes_python_encoding() {
        // cbor2.dumps({0:1, 1:20, 2:b'\x01'*8, 3:1700000000000, 4:b'\x02'*16,
        //              5:'general', 6:'hi', 7:'bob'})
        let hex = "a80001011402480101010101010101031b0000018bcfe56800045002020202020202020202020202020202056767656e6572616c066268690763626f62";
        let bytes = hex::decode(hex).unwrap();
        let env = Envelope::decode(&bytes).unwrap();
        assert_eq!((env.t, env.room.as_deref(), env.body_text()), (t::MSG, Some("general"), Some("hi")));
        assert_eq!(env.nick.as_deref(), Some("bob"));
        assert_eq!(env.ts, 1_700_000_000_000);
    }
}
