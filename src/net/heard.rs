//! Every announce heard, of any kind, for the announce viewer: what
//! announced (by the name its destination announced under), its name where
//! that kind says how to read one, and how far away it is.

use sha2::{Digest, Sha256};

use super::Hash;

/// What announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Lxmf,
    Nomad,
    Propagation,
    Rrc,
    /// LXST telephony (Sideband's and MeshChat's calls).
    Phone,
    Other,
}

impl Kind {
    /// The short tag shown beside it.
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Lxmf => "PEER",
            Kind::Nomad => "NODE",
            Kind::Propagation => "PROP",
            Kind::Rrc => "RRC",
            Kind::Phone => "CALL",
            Kind::Other => "OTHER",
        }
    }

    /// As the web UI names it.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Lxmf => "lxmf",
            Kind::Nomad => "nomad",
            Kind::Propagation => "propagation",
            Kind::Rrc => "rrc",
            Kind::Phone => "phone",
            Kind::Other => "other",
        }
    }
}

/// The kinds rettui can name, by the full name they announce under.
const KNOWN: &[(&str, Kind)] = &[
    (crate::lxmf::LXMF_ASPECT, Kind::Lxmf),
    (nomad_core::NOMAD_NODE_ASPECT, Kind::Nomad),
    (crate::lxmf::PROPAGATION_ASPECT, Kind::Propagation),
    (crate::rrc::DEFAULT_ASPECT, Kind::Rrc),
    ("lxst.telephony", Kind::Phone),
];

/// An announce, as the viewer shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    pub hash: Hash,
    pub kind: Kind,
    /// The full name it announced under, when it's one rettui knows.
    pub aspect: Option<&'static str>,
    pub name: Option<String>,
    pub hops: u8,
}

/// Reticulum's name hash: the first 10 bytes of the full name's SHA-256.
fn name_hash(name: &str) -> [u8; 10] {
    let digest = Sha256::digest(name.as_bytes());
    let mut hash = [0; 10];
    hash.copy_from_slice(&digest[..10]);
    hash
}

/// What an announce is, from its name hash and app data.
pub fn heard(hash: Hash, name_hash_heard: [u8; 10], app_data: Option<&[u8]>, hops: u8) -> Heard {
    let known = KNOWN.iter().find(|(name, _)| name_hash(name) == name_hash_heard);
    let (aspect, kind) = known.map_or((None, Kind::Other), |(name, kind)| (Some(*name), *kind));
    let name = app_data.and_then(|data| match kind {
        Kind::Lxmf => super::lxmf_display_name(data),
        Kind::Nomad => crate::names::clean(&nomad_core::clamp_node_name(&String::from_utf8_lossy(data))),
        Kind::Propagation => lxmf_core::handlers::parse_pn_announce_data(data)
            .and_then(|pn| pn.metadata.get(&lxmf_core::constants::PN_META_NAME).cloned())
            .and_then(|name| crate::names::clean(&String::from_utf8_lossy(&name))),
        Kind::Rrc | Kind::Phone | Kind::Other => any_name(data),
    });
    Heard { hash, kind, aspect, name, hops }
}

/// A name in app data whose format isn't known: text, or the first text
/// in a msgpack list (as LXMF's), or a `name` in a msgpack map. Not
/// anything else, which is shown as no name rather than as noise.
fn any_name(data: &[u8]) -> Option<String> {
    if let Ok(text) = std::str::from_utf8(data) {
        let printable = text.chars().all(|c| !c.is_control() || c == ' ');
        return if printable && text.chars().count() <= 64 { crate::names::clean(text) } else { None };
    }
    let value = rmpv::decode::read_value(&mut &data[..]).ok()?;
    let text = match &value {
        rmpv::Value::Array(items) => {
            items.first()?.as_slice().map(<[u8]>::to_vec).or_else(|| items.first()?.as_str().map(|s| s.as_bytes().to_vec()))?
        }
        rmpv::Value::Map(entries) => entries
            .iter()
            .find(|(key, _)| key.as_str() == Some("name") || key.as_slice() == Some(b"name"))
            .and_then(|(_, value)| value.as_str().map(|s| s.as_bytes().to_vec()).or_else(|| value.as_slice().map(<[u8]>::to_vec)))?,
        _ => return None,
    };
    let text = std::str::from_utf8(&text).ok()?;
    if text.chars().count() > 64 {
        return None;
    }
    crate::names::clean(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_kinds_are_named_and_others_said_as_they_are() {
        let lxmf = heard([1; 16], name_hash("lxmf.delivery"), Some(b"Alex"), 2);
        assert_eq!((lxmf.kind, lxmf.aspect, lxmf.name.as_deref(), lxmf.hops), (Kind::Lxmf, Some("lxmf.delivery"), Some("Alex"), 2));
        let node = heard([2; 16], name_hash("nomadnetwork.node"), Some("Node 🌲".as_bytes()), 1);
        assert_eq!((node.kind, node.name.as_deref()), (Kind::Nomad, Some("Node 🌲")));
        let hub = heard([3; 16], name_hash("rrc.hub"), Some(b"Town square"), 0);
        assert_eq!((hub.kind, hub.name.as_deref()), (Kind::Rrc, Some("Town square")));
        // Something rettui doesn't know: no aspect; a name only if it reads
        // as one.
        let other = heard([4; 16], name_hash("example.thing"), Some(&[0x81, 0xa4, b'n', b'a', b'm', b'e', 0xa3, b'B', b'o', b'b']), 3);
        assert_eq!((other.kind, other.aspect, other.name.as_deref()), (Kind::Other, None, Some("Bob")));
        let noise = heard([5; 16], name_hash("example.thing"), Some(&[0xff, 0x00, 0x13, 0x37]), 3);
        assert_eq!(noise.name, None);
        let nothing = heard([6; 16], [0; 10], None, 0);
        assert_eq!((nothing.kind, nothing.name), (Kind::Other, None));
    }
}
