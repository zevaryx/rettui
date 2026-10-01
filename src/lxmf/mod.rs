//! LXMF messaging on top of rns-runtime Links, using lxmf-core (rsLXMF) for
//! the message format, stamps and propagation node protocol.
//!
//! - [`send`]: direct delivery and propagation node submission.
//! - [`inbound`]: unpacking and verifying received messages.
//! - [`sync`]: downloading waiting messages from a propagation node.
//! - [`paper`]: paper messages (`lxm://` links and QR codes).
//! - [`fields`]: what messages carry besides text and files (reactions,
//!   locations, commands, voice).
//! - [`policy`]: stamp costs, tickets and the size limit for incoming
//!   messages.
//! - [`pn`]: hosting a propagation node.

pub mod fields;
mod inbound;
pub mod paper;
pub mod pn;
pub mod policy;
mod send;
mod sync;

use std::path::PathBuf;

use rmpv::Value;

pub use fields::{Extras, Location, Reaction};
pub use inbound::{spawn_inbound, with_destination};
pub use policy::Policy;
pub use send::send;
pub use sync::Syncer;

use crate::net::Hash;

pub const LXMF_ASPECT: &str = "lxmf.delivery";
pub const PROPAGATION_ASPECT: &str = "lxmf.propagation";

const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryMode {
    /// Direct, falling back to the propagation node when direct fails.
    Auto,
    Direct,
    Propagated,
    /// Not sent: written as a paper message (an `lxm://` link and QR code)
    /// to pass on some other way.
    Paper,
}

impl DeliveryMode {
    pub fn label(self) -> &'static str {
        match self {
            DeliveryMode::Auto => "auto",
            DeliveryMode::Direct => "direct",
            DeliveryMode::Propagated => "propagated",
            DeliveryMode::Paper => "paper",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "" | "auto" => Some(DeliveryMode::Auto),
            "direct" => Some(DeliveryMode::Direct),
            "propagated" => Some(DeliveryMode::Propagated),
            "paper" => Some(DeliveryMode::Paper),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            DeliveryMode::Auto => DeliveryMode::Direct,
            DeliveryMode::Direct => DeliveryMode::Propagated,
            DeliveryMode::Propagated => DeliveryMode::Paper,
            DeliveryMode::Paper => DeliveryMode::Auto,
        }
    }
}

/// The message a reply answers (LXMF's `FIELD_REPLY_TO`, as Columba and
/// MeshChatX send it): its hash, and the start of its text
/// (`FIELD_REPLY_QUOTE`), so a client without it can still show what's
/// answered. Clients that don't know these fields show an ordinary message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub to: [u8; 32],
    pub quote: Option<String>,
}

/// How much of the answered message's text a reply carries: enough to
/// recognise it, without sending a long message twice.
const QUOTE_CHARS: usize = 160;

/// What a reply quotes of `text`: its start, if it has any.
pub fn quote_of(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(QUOTE_CHARS).collect())
}

/// A message sent: how it went, and its hash (what replies to it name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub delivered: Delivered,
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivered {
    /// The recipient proved receipt.
    Direct,
    /// A propagation node accepted the message for later pickup.
    Propagated,
}

#[derive(Debug)]
pub struct Outgoing {
    pub to: Hash,
    pub content: String,
    pub attachments: Vec<PathBuf>,
    pub mode: DeliveryMode,
    pub propagation_node: Option<Hash>,
    /// When it was written (Unix seconds). Fixed, so each attempt to send
    /// it (direct, then through the propagation node) is the same message,
    /// with the same hash.
    pub timestamp: f64,
    pub reply: Option<Reply>,
    /// A reaction to a message (sent with no text, as Columba does).
    pub reaction: Option<Reaction>,
    /// The recipient's ticket, to stamp it with instead of doing the work
    /// (set by the network actor).
    pub stamp_ticket: Option<[u8; 16]>,
    /// A ticket of ours for them (`FIELD_TICKET`'s value; set by the
    /// network actor).
    pub ticket: Option<Vec<u8>>,
    /// The propagation node this client hosts, when it's the one set: sends
    /// to it go straight in (set by the network actor).
    pub local_node: Option<pn::LocalNode>,
    /// How its text is written (LXMF's renderer field), if not plain.
    pub format: Option<crate::markdown::TextFormat>,
}

impl Outgoing {
    /// A text message (with files, maybe), not a reply.
    pub fn text(to: Hash, content: String, attachments: Vec<PathBuf>, mode: DeliveryMode, timestamp: f64) -> Self {
        Self {
            to,
            content,
            attachments,
            mode,
            propagation_node: None,
            timestamp,
            reply: None,
            reaction: None,
            stamp_ticket: None,
            ticket: None,
            local_node: None,
            format: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub data: Vec<u8>,
    pub image: bool,
}

#[derive(Debug, Clone, Default)]
pub struct InboundMessage {
    pub id: Option<[u8; 32]>,
    pub source: Hash,
    pub title: String,
    pub content: String,
    pub timestamp: f64,
    pub verified: bool,
    pub attachments: Vec<Attachment>,
    /// Read in from a paper message, not received over the network.
    pub paper: bool,
    /// The message it answers, if it's a reply.
    pub reply: Option<Reply>,
    /// Reactions, locations, commands and voice.
    pub extras: Extras,
}

fn is_image_name(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn is_image_path(path: &std::path::Path) -> bool {
    path.file_name()
        .is_some_and(|n| is_image_name(&n.to_string_lossy()))
}

fn encode(value: &Value) -> Vec<u8> {
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, value).expect("writing to a Vec cannot fail");
    buf
}

/// An LXMF address with its identity's public key, as Columba shares
/// contacts in QR codes: `lxma://<address>:<public key>`, both hex. With
/// the key, a message can be written before the address is heard.
pub fn identity_link(address: Hash, public_key: &[u8; 64]) -> String {
    format!("lxma://{}:{}", hex::encode(address), hex::encode(public_key))
}

/// The address and public key in an `lxma://` link, if the key is the
/// address's own.
pub fn parse_identity_link(text: &str) -> Option<(Hash, [u8; 64])> {
    let rest = text.trim().strip_prefix("lxma://")?;
    let (address, key) = rest.split_once([':', '/'])?;
    let address: Hash = hex::decode(address).ok()?.try_into().ok()?;
    let key: [u8; 64] = hex::decode(key.trim_end_matches('/')).ok()?.try_into().ok()?;
    let identity = rns_runtime::prelude::Identity::from_public_key(&key).ok()?;
    let own = rns_identity::destination::Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&identity.hash));
    (own == address).then_some((address, key))
}

/// Someone to write to, as typed or scanned: an address, or an `lxma://`
/// link (which brings the public key too). Errors say what's wrong.
pub fn parse_contact(text: &str) -> Result<(Hash, Option<[u8; 64]>), String> {
    if text.trim().starts_with("lxma://") {
        return parse_identity_link(text)
            .map(|(address, key)| (address, Some(key)))
            .ok_or_else(|| "Not a valid lxma:// link (its key doesn't match its address)".to_string());
    }
    crate::net::parse_hash(text).map(|address| (address, None)).ok_or_else(|| {
        "An LXMF address is 32 hex characters (or an lxma:// link)".to_string()
    })
}

fn bytes_of(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Binary(b) => Some(b.clone()),
        Value::String(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use lxmf_core::message_api::LxMessage;
    use rns_runtime::prelude::*;

    use super::inbound::attachments_of;
    use super::send::build_message;
    use super::*;

    #[tokio::test]
    async fn attachments_round_trip() {
        let dir = std::env::temp_dir().join(format!("rettui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (img, doc) = (dir.join("pic.png"), dir.join("notes.txt"));
        std::fs::write(&img, b"not really a png").unwrap();
        std::fs::write(&doc, b"hello").unwrap();

        let identity = Identity::new();
        let outgoing = Outgoing::text([1; 16], "hi".into(), vec![img, doc], DeliveryMode::Direct, 1_800_000_000.0);
        let message = build_message(&identity, [2; 16], &outgoing, None).await.unwrap();
        let unpacked = LxMessage::unpack(&message.pack().unwrap()).unwrap();
        let attachments = attachments_of(&unpacked);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(attachments.len(), 2);
        assert!(attachments[0].image);
        assert_eq!(attachments[0].name, "image.png");
        assert_eq!(attachments[1].name, "notes.txt");
        assert_eq!(attachments[1].data, b"hello");
        assert_eq!(unpacked.content, "hi");
    }

    #[test]
    fn identity_links_carry_the_key_of_their_address() {
        let identity = Identity::new();
        let address = rns_identity::destination::Destination::hash_from_name_and_identity(LXMF_ASPECT, Some(&identity.hash));
        let key = identity.get_public_key();
        let link = identity_link(address, &key);
        assert_eq!(link.len(), "lxma://".len() + 32 + 1 + 128);
        assert_eq!(parse_identity_link(&link), Some((address, key)));
        assert_eq!(parse_contact(&format!("  {link} ")), Ok((address, Some(key))));
        // Upper case, or a slash between them, as some write it.
        let other = format!("lxma://{}/{}", hex::encode_upper(address), hex::encode(key));
        assert_eq!(parse_identity_link(&other), Some((address, key)));
        // Someone else's key for the address: no.
        let theirs = identity_link(address, &Identity::new().get_public_key());
        assert_eq!(parse_identity_link(&theirs), None);
        assert!(parse_contact(&theirs).is_err());
        assert_eq!(parse_contact(&format!("<{}>", hex::encode(address))), Ok((address, None)));
        assert!(parse_contact("hello").is_err());
    }

    fn outgoing(reply: Option<Reply>) -> Outgoing {
        Outgoing { reply, ..Outgoing::text([1; 16], "yes, at noon".into(), Vec::new(), DeliveryMode::Direct, 1_800_000_000.5) }
    }

    #[tokio::test]
    async fn replies_name_the_message_they_answer() {
        let identity = Identity::new();
        let reply = Reply { to: [7; 32], quote: Some("Lunch tomorrow?".into()) };
        let message = build_message(&identity, [2; 16], &outgoing(Some(reply.clone())), None).await.unwrap();
        let packed = message.pack().unwrap();
        assert_eq!(super::inbound::reply_of(&LxMessage::unpack(&packed).unwrap()), Some(reply));
        // Field 0x30 as msgpack bytes (bin 8, 32 long), as Columba and
        // MeshChatX send it.
        let mut on_wire = vec![0x30, 0xc4, 32];
        on_wire.extend([7; 32]);
        assert!(packed.windows(on_wire.len()).any(|w| w == on_wire));
        // Not a reply: no fields.
        let plain = build_message(&identity, [2; 16], &outgoing(None), None).await.unwrap();
        assert_eq!(super::inbound::reply_of(&LxMessage::unpack(&plain.pack().unwrap()).unwrap()), None);
    }

    #[tokio::test]
    async fn every_attempt_at_a_message_has_the_same_hash() {
        // Direct, then through the propagation node: the same message, so
        // replies to it find it whichever way it went.
        let identity = Identity::new();
        let first = build_message(&identity, [2; 16], &outgoing(None), None).await.unwrap();
        let second = build_message(&identity, [2; 16], &outgoing(None), Some(&[0x91, 0xc0])).await.unwrap();
        assert!(first.hash.is_some());
        assert_eq!(first.hash, second.hash);
    }

    #[test]
    fn older_columba_replies_are_read_too() {
        let mut message = LxMessage::new([1; 16], [2; 16], "", "ok", lxmf_core::message_api::DeliveryMethod::Direct);
        let extensions = Value::Map(vec![(Value::from("reply_to"), Value::from(hex::encode([9u8; 32])))]);
        message.set_msgpack_field(0x10, encode(&extensions)).unwrap();
        assert_eq!(super::inbound::reply_of(&message), Some(Reply { to: [9; 32], quote: None }));
    }

    #[test]
    fn quotes_are_the_start_of_the_text() {
        assert_eq!(quote_of("  hello  "), Some("hello".into()));
        assert_eq!(quote_of(" \n "), None);
        assert_eq!(quote_of(&"é".repeat(400)).map(|q| q.chars().count()), Some(QUOTE_CHARS));
    }
}
