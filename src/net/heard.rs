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
    /// An rngit node (Git over Reticulum).
    Git,
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
            Kind::Git => "GIT",
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
            Kind::Git => "git",
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
    (super::git::GIT_ASPECT, Kind::Git),
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
    /// For a propagation node, what runs it, as LXMF 1.2 announces it
    /// ("lxmd 1.2.1", "rettui 1.2.0").
    pub software: Option<String>,
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
        Kind::Rrc => hub_name(data).or_else(|| any_name(data)),
        Kind::Phone | Kind::Git | Kind::Other => any_name(data),
    });
    let software = app_data
        .filter(|_| kind == Kind::Propagation)
        .and_then(lxmf_core::handlers::parse_pn_announce_data)
        .and_then(|pn| pn_software(&pn.metadata));
    Heard { hash, kind, aspect, name, hops, software }
}

/// What runs a propagation node, from its announce's metadata: the
/// implementation's name and version, as Sideband shows them ("Running
/// lxmd 1.2.1"). Either alone, if that's all it says.
pub fn pn_software(metadata: &std::collections::HashMap<u8, Vec<u8>>) -> Option<String> {
    const IMPL_NAME: u8 = 0xFE;
    let text = |key: u8| {
        let value = metadata.get(&key)?;
        let text: String = String::from_utf8_lossy(value).chars().filter(|c| !c.is_control()).take(32).collect();
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    };
    match (text(IMPL_NAME), text(lxmf_core::constants::PN_META_VERSION)) {
        (Some(name), Some(version)) => Some(format!("{name} {version}")),
        (one, other) => one.or(other),
    }
}

/// An RRC hub's name, as rrcd, rsRRCD, the Go hub and rettui announce it:
/// a CBOR map with the `hub`'s name (and `proto`, `v`).
fn hub_name(data: &[u8]) -> Option<String> {
    let ciborium::Value::Map(entries) = ciborium::from_reader::<ciborium::Value, _>(data).ok()? else { return None };
    let name = entries.iter().find(|(key, _)| key.as_text() == Some("hub"))?.1.as_text()?;
    if name.chars().count() > 64 {
        return None;
    }
    crate::names::clean(name)
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
    fn what_runs_a_propagation_node_is_read_from_its_announce() {
        use lxmf_core::handlers::{PropagationNodeAnnounceData, get_propagation_node_app_data};
        let mut data = PropagationNodeAnnounceData::new(true, 256, 10_240, 16, 3, 18);
        data.metadata.insert(lxmf_core::constants::PN_META_NAME, b"Hill".to_vec());
        let bare = get_propagation_node_app_data(&data);
        // lxmd packs its name and version as text, not bytes.
        let mut value = rmpv::decode::read_value(&mut &bare[..]).unwrap();
        if let rmpv::Value::Array(parts) = &mut value
            && let Some(rmpv::Value::Map(meta)) = parts.get_mut(6)
        {
            meta.push((rmpv::Value::from(0xFE), rmpv::Value::from("lxmd")));
            meta.push((rmpv::Value::from(lxmf_core::constants::PN_META_VERSION), rmpv::Value::from("1.2.1")));
        }
        let mut lxmd = Vec::new();
        rmpv::encode::write_value(&mut lxmd, &value).unwrap();
        let node = heard([4; 16], name_hash(crate::lxmf::PROPAGATION_ASPECT), Some(&lxmd), 1);
        assert_eq!((node.name.as_deref(), node.software.as_deref()), (Some("Hill"), Some("lxmd 1.2.1")));
        // One that doesn't say, and one that isn't a propagation node.
        assert_eq!(heard([4; 16], name_hash(crate::lxmf::PROPAGATION_ASPECT), Some(&bare), 1).software, None);
        assert_eq!(heard([1; 16], name_hash("lxmf.delivery"), Some(&lxmd), 1).software, None);
    }

    #[test]
    fn known_kinds_are_named_and_others_said_as_they_are() {
        let lxmf = heard([1; 16], name_hash("lxmf.delivery"), Some(b"Alex"), 2);
        assert_eq!((lxmf.kind, lxmf.aspect, lxmf.name.as_deref(), lxmf.hops), (Kind::Lxmf, Some("lxmf.delivery"), Some("Alex"), 2));
        let node = heard([2; 16], name_hash("nomadnetwork.node"), Some("Node 🌲".as_bytes()), 1);
        assert_eq!((node.kind, node.name.as_deref()), (Kind::Nomad, Some("Node 🌲")));
        let hub = heard([3; 16], name_hash("rrc.hub"), Some(b"Town square"), 0);
        assert_eq!((hub.kind, hub.name.as_deref()), (Kind::Rrc, Some("Town square")));
        // As hubs announce themselves: a CBOR map.
        let data = crate::rrc::host::announce_data("Hilltop hub");
        let hub = heard([3; 16], name_hash("rrc.hub"), Some(&data), 0);
        assert_eq!(hub.name.as_deref(), Some("Hilltop hub"));
        let git = heard([7; 16], name_hash("git.repositories"), Some(b"Forge"), 1);
        assert_eq!((git.kind, git.kind.tag(), git.kind.key(), git.name.as_deref()), (Kind::Git, "GIT", "git", Some("Forge")));
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
