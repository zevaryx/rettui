//! Receiving: unpacking and verifying LXMF messages, however they arrived.

use std::time::Duration;

use lxmf_core::constants::{FIELD_FILE_ATTACHMENTS, FIELD_REPLY_QUOTE, FIELD_REPLY_TO};
use lxmf_core::message_api::LxMessage;
use rmpv::Value;
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use super::fields::{self, FIELD_COLUMBA_EXTENSIONS};
use super::policy::{SharedPolicy, StampCheck};
use super::{Attachment, Extras, InboundMessage, Reply, bytes_of, is_image_name};
use crate::net::{Hash, Known, NetEvent, lookup, lookup_heard};

/// How many stamps are worked out at once.
static STAMP_CHECKS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// How long to look for an unknown sender's announce to verify a message.
const SENDER_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// How many unknown senders are looked for at once. Each look sends a path
/// request on every interface and waits up to [`SENDER_LOOKUP_TIMEOUT`], and
/// anyone can send from as many unknown addresses as they like. Past it, a
/// message is kept unverified straight away, as when the look finds nothing.
/// As many as a sync parses at once, so a sync alone never meets it.
const SENDER_LOOKUPS: usize = 16;
static LOOKING_FOR_SENDERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(SENDER_LOOKUPS);

/// The most files taken from one message. A message may list thousands of
/// (even empty) files, and each one is a file on disk.
pub const MAX_ATTACHMENTS: usize = 64;

/// The files a message carries, up to [`MAX_ATTACHMENTS`], and how many more
/// it listed.
pub(super) fn attachments_of(message: &LxMessage) -> (Vec<Attachment>, usize) {
    let mut out = Vec::new();
    let mut dropped = 0;
    if let Ok(Some((format, data))) = message.image_attachment() {
        let ext = if format.is_empty() { "img".to_string() } else { format };
        out.push(Attachment { name: format!("image.{ext}"), data: data.to_vec(), image: true });
    }
    if let Some(field) = message.get_field(FIELD_FILE_ATTACHMENTS)
        && let Ok(Value::Array(entries)) = rmpv::decode::read_value(&mut super::fields::raw(field))
    {
        for entry in entries {
            let Value::Array(parts) = entry else { continue };
            if out.len() >= MAX_ATTACHMENTS {
                dropped += 1;
                continue;
            }
            let (Some(name), Some(data)) = (parts.first().and_then(bytes_of), parts.get(1).and_then(bytes_of)) else {
                continue;
            };
            let name = String::from_utf8_lossy(&name).into_owned();
            out.push(Attachment { image: is_image_name(&name), name, data });
        }
    }
    (out, dropped)
}

/// The most of a received quote kept (some clients send all of the text).
const QUOTE_KEPT: usize = 500;

/// The message `message` answers, if it's a reply: its hash as bytes (as
/// sent) or hex, and the quote if there is one.
/// (Older Columba sent its reply target as `{"reply_to": "<hex>"}` in field
/// 0x10, before it used [`FIELD_REPLY_TO`].)
pub fn reply_of(message: &LxMessage) -> Option<Reply> {
    let field = |id| fields::value(message, id);
    let to = field(FIELD_REPLY_TO).as_ref().and_then(fields::hash_of).or_else(|| {
        let Value::Map(entries) = field(FIELD_COLUMBA_EXTENSIONS)? else { return None };
        entries.iter().find(|(key, _)| key.as_str() == Some("reply_to")).and_then(|(_, value)| fields::hash_of(value))
    })?;
    let quote = field(FIELD_REPLY_QUOTE)
        .as_ref()
        .and_then(bytes_of)
        .map(|bytes| String::from_utf8_lossy(&bytes).chars().take(QUOTE_KEPT).collect::<String>())
        .filter(|quote| !quote.trim().is_empty());
    Some(Reply { to, quote })
}

/// Whether a message is from who it says: `Ok(true)` if its signature
/// matches the sender's key, `Ok(false)` if the key isn't known (kept, shown
/// as unverified), and an error if the key is known and the signature
/// doesn't match: someone else wrote it, so it's dropped, as Python LXMF
/// does. The key is the one known for the address it's from, whatever
/// that address is of: Python LXMF asks no more, so neither does this.
fn check_signature(message: &LxMessage, sender: Option<&Identity>) -> Result<bool, String> {
    let Some(sender) = sender else {
        return Ok(false);
    };
    let public = sender.get_public_key();
    // Its second half is the signing key.
    let key = public[32..].try_into().ok().and_then(|half| rns_crypto::ed25519::Ed25519PublicKey::from_bytes(half).ok());
    if key.is_some_and(|key| message.verify_signature(&key)) {
        Ok(true)
    } else {
        Err(format!("Dropped a message claiming to be from {}: its signature doesn't match their key", hex::encode(message.source_hash())))
    }
}

/// Unpack and verify one complete LXMF message (destination hash included).
/// With a `policy`, a message over the size limit, or without the stamp it
/// asks for, is dropped, and a ticket it brings is kept. (A paper message,
/// read in by hand, has none.)
pub(super) async fn parse_inbound(
    runtime: &ReticulumHandle,
    known: &Known,
    policy: Option<&SharedPolicy>,
    data: &[u8],
) -> Result<InboundMessage, String> {
    parse_inbound_holding(runtime, known, policy, data, None).await
}

/// What a message is handed to while its sender's key is looked for.
type Holding<'a> = &'a (dyn Fn(InboundMessage) + Sync);

/// [`parse_inbound`]; and if the sender's key has to be looked for (a path
/// request, and a wait of up to [`SENDER_LOOKUP_TIMEOUT`]), the message is
/// first handed to `holding`, unverified, so it can be kept meanwhile: the
/// sender has been told it was delivered already. (Not a game's move: those
/// wait, as they did.)
async fn parse_inbound_holding(
    runtime: &ReticulumHandle,
    known: &Known,
    policy: Option<&SharedPolicy>,
    data: &[u8],
    holding: Option<Holding<'_>>,
) -> Result<InboundMessage, String> {
    let mut message = LxMessage::unpack(data).map_err(|e| e.to_string())?;
    if let Some(policy) = policy {
        // The policy is let go before the stamp is worked out: that takes a while, and
        // the link manager asks the policy too.
        let check = {
            let policy = policy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if policy.too_big(data.len()) {
                return Err(format!(
                    "Dropped a message from {}: {} KB, over your limit of {} KB",
                    hex::encode(message.source_hash()),
                    data.len() / 1000,
                    policy.max_bytes / 1000
                ));
            }
            policy.stamp_check(&message)
        };
        if let StampCheck::Needs { cost, .. } = &check {
            let cost = *cost;
            // A few at a time, so a flood of stamps can't take every thread.
            let _turn = STAMP_CHECKS.acquire().await.map_err(|e| e.to_string())?;
            let (passes, back) = tokio::task::spawn_blocking(move || (check.passes(&message), message)).await.map_err(|e| e.to_string())?;
            message = back;
            if !passes {
                return Err(format!(
                    "Dropped a message from {}: it had no valid stamp (you ask for stamp cost {cost})",
                    hex::encode(message.source_hash())
                ));
            }
        }
    }
    let (attachments, dropped_attachments) = attachments_of(&message);
    // A Codec2 voice message is kept as it came. It is decoded to play when
    // it's saved, if it's from someone known: it grows about 400 times, which
    // a stranger must not be able to make happen.
    let mut inbound = InboundMessage {
        id: Some(message.message_id()),
        source: message.source_hash(),
        title: message.title_text().into_owned(),
        content: message.content_text().into_owned(),
        timestamp: message.timestamp(),
        verified: false,
        attachments,
        dropped_attachments,
        paper: false,
        reply: reply_of(&message),
        extras: Extras::of(&message),
        signal: None,
    };
    // Senders of propagated messages are often offline; a bounded lookup
    // (cache, remembered keys, then a path request) finds their key.
    let sender = match lookup_heard(runtime, known, message.source_hash()).await {
        Some(remote) => Ok(remote),
        None => match LOOKING_FOR_SENDERS.try_acquire() {
            Ok(_turn) => {
                if let Some(holding) = holding
                    && inbound.extras.game.is_none()
                {
                    holding(inbound.clone());
                }
                tokio::time::timeout(SENDER_LOOKUP_TIMEOUT, lookup(runtime, known, message.source_hash()))
                    .await
                    .unwrap_or_else(|_| Err("timed out".to_string()))
            }
            Err(_) => Err(format!("{SENDER_LOOKUPS} other senders are being looked for already")),
        },
    };
    let sender = sender
        .inspect_err(|e| {
            tracing::debug!("sender {} not known ({e}); cannot verify signature", hex::encode(message.source_hash()));
        })
        .ok();
    let verified = check_signature(&message, sender.as_ref().map(|remote| &remote.identity))?;
    tracing::debug!("message from {} verified={verified}", hex::encode(message.source_hash()));
    // Only from who it says, a ticket can be used for messages to them.
    if verified && let Some(policy) = policy {
        policy.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take_ticket(&message);
    }
    inbound.verified = verified;
    Ok(inbound)
}

pub fn with_destination(lxmf_hash: Hash, data: Vec<u8>) -> Vec<u8> {
    if data.len() >= 16 && data[..16] == lxmf_hash {
        data
    } else {
        let mut full = lxmf_hash.to_vec();
        full.extend_from_slice(&data);
        full
    }
}

pub(super) async fn deliver_inbound(
    runtime: &ReticulumHandle,
    known: &Known,
    policy: &SharedPolicy,
    data: &[u8],
    heard: Heard,
    ev: &mpsc::UnboundedSender<NetEvent>,
) {
    // Kept while its sender's key is looked for, then confirmed (as a
    // Message) or taken back (Forged).
    let held = std::sync::Mutex::new(None);
    let holding = |message: InboundMessage| {
        *held.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = message.id.map(|id| (message.source, id));
        let _ = ev.send(NetEvent::Checking(Box::new(message)));
    };
    let event = match parse_inbound_holding(runtime, known, Some(policy), data, Some(&holding)).await {
        Ok(mut message) => {
            message.signal = match heard {
                Heard::Unknown => None,
                Heard::Packet(raw) => crate::net::remote::Signal::of_packet(runtime, &raw).await,
            };
            NetEvent::Message(Box::new(message))
        }
        Err(e) if e.starts_with("Dropped") => {
            if let Some((source, id)) = held.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
                let _ = ev.send(NetEvent::Forged { source, id });
            }
            NetEvent::Log(e)
        }
        Err(e) => NetEvent::Log(format!("Dropped malformed LXMF message: {e}")),
    };
    let _ = ev.send(event);
}

/// How a message came, to say how it was heard.
pub enum Heard {
    /// Over a Link (whose packets rettui doesn't see), or not straight from
    /// the sender (from a propagation node).
    Unknown,
    /// In this one packet, straight from the sender.
    Packet(Vec<u8>),
}

pub fn spawn_inbound(
    runtime: &ReticulumHandle,
    known: &Known,
    policy: &SharedPolicy,
    data: Vec<u8>,
    heard: Heard,
    ev: &mpsc::UnboundedSender<NetEvent>,
) {
    let (runtime, known, policy, ev) = (runtime.clone(), known.clone(), policy.clone(), ev.clone());
    tokio::spawn(async move { deliver_inbound(&runtime, &known, &policy, &data, heard, &ev).await });
}

#[cfg(test)]
mod tests {
    use lxmf_core::message_api::{DeliveryMethod, LxMessageDraft};

    use super::*;

    /// A message from `sender`'s address, signed with `key`'s, as it arrives.
    fn signed_by(sender: &Identity, key: &Identity, content: &str) -> Vec<u8> {
        let from = rns_identity::destination::Destination::hash_from_name_and_identity(super::super::LXMF_ASPECT, Some(&sender.hash));
        signed_from(from, key, content)
    }

    fn signed_from(from: Hash, key: &Identity, content: &str) -> Vec<u8> {
        let message = LxMessageDraft::new([1; 16], from, "", content, DeliveryMethod::Direct).unwrap();
        message.sign(&key.get_signing_key().unwrap()).unwrap().finalize(None).unwrap().pack().unwrap()
    }

    #[test]
    fn forged_messages_are_dropped_and_unknown_senders_unverified() {
        let (alice, mallory) = (Identity::new(), Identity::new());
        let data = signed_by(&alice, &alice, "hello");
        let check = |data: &[u8], sender: Option<&Identity>| check_signature(&LxMessage::unpack(data).unwrap(), sender);
        assert_eq!(check(&data, Some(&alice)), Ok(true));
        // Not known yet: kept, as unverified.
        assert_eq!(check(&data, None), Ok(false));
        // Someone else's key for the address, signed by someone else, or
        // changed on the way: dropped.
        let from = hex::encode(&data[16..20]);
        assert!(check(&data, Some(&mallory)).unwrap_err().starts_with(&format!("Dropped a message claiming to be from {from}")));
        assert!(check(&signed_by(&alice, &mallory, "hello"), Some(&alice)).is_err());
        let at = data.windows(5).position(|w| w == b"hello").unwrap();
        let mut changed = data.clone();
        changed[at..at + 5].copy_from_slice(b"jello");
        assert!(check(&changed, Some(&alice)).is_err());
        // From another of Alice's addresses, signed by her: hers, as in
        // Python LXMF, which checks the signature and no more.
        let node = rns_identity::destination::Destination::hash_from_name_and_identity("nomadnetwork.node", Some(&alice.hash));
        assert_eq!(check(&signed_from(node, &alice, "hello"), Some(&alice)), Ok(true));
        assert!(check(&signed_from(node, &mallory, "hello"), Some(&alice)).is_err());
    }

    /// The tests that take every lookup turn, or need one, one at a time
    /// (the turns are shared by the whole program).
    static LOOKUP_TURNS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn only_a_few_unknown_senders_are_looked_for_at_once() {
        let _alone = LOOKUP_TURNS.lock().await;
        let dir = std::env::temp_dir().join(format!("rettui-sender-lookups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config"), "[reticulum]\n  enable_transport = No\n  share_instance = No\n\n[interfaces]\n").unwrap();
        let runtime = crate::net::start_runtime(Some(&dir.display().to_string())).await.unwrap();
        let known = crate::net::KnownIdentities::none();
        let alice = Identity::new();
        // Every turn taken (a flood of strangers being looked for): the
        // next is kept unverified at once, rather than wait for its own look.
        let taken = LOOKING_FOR_SENDERS.acquire_many(SENDER_LOOKUPS as u32).await.unwrap();
        let started = std::time::Instant::now();
        let message = parse_inbound(&runtime, &known, None, &signed_by(&alice, &alice, "hello")).await.unwrap();
        assert!(!message.verified);
        assert!(started.elapsed() < SENDER_LOOKUP_TIMEOUT / 4, "not looked for: {:?}", started.elapsed());
        drop(taken);
        // Someone whose key is known is verified whatever the others do.
        let _taken = LOOKING_FOR_SENDERS.acquire_many(SENDER_LOOKUPS as u32).await.unwrap();
        let from = rns_identity::destination::Destination::hash_from_name_and_identity(super::super::LXMF_ASPECT, Some(&alice.hash));
        known.lock().unwrap().remember(from, &alice.get_public_key(), None);
        assert!(parse_inbound(&runtime, &known, None, &signed_by(&alice, &alice, "hello")).await.unwrap().verified);
        runtime.shutdown_and_wait().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_message_is_handed_over_before_its_senders_key_is_looked_for() {
        let _alone = LOOKUP_TURNS.lock().await;
        let dir = std::env::temp_dir().join(format!("rettui-holding-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config"), "[reticulum]\n  enable_transport = No\n  share_instance = No\n\n[interfaces]\n").unwrap();
        let runtime = crate::net::start_runtime(Some(&dir.display().to_string())).await.unwrap();
        let known = crate::net::KnownIdentities::none();
        let policy = crate::lxmf::Policy::load(&dir.join("tickets.json"), None, 1_000_000);
        let (ev, mut events) = mpsc::unbounded_channel();
        let alice = Identity::new();
        // Nobody knows Alice's key: her message is handed over at once,
        // unverified, then again once the look is over (no path here).
        let started = std::time::Instant::now();
        let hello = signed_by(&alice, &alice, "hello");
        let ((), (first, handed)) = tokio::join!(deliver_inbound(&runtime, &known, &policy, &hello, Heard::Unknown, &ev), async {
            let first = events.recv().await.unwrap();
            (first, started.elapsed())
        });
        assert!(handed < SENDER_LOOKUP_TIMEOUT / 4, "handed over after {handed:?}");
        let NetEvent::Checking(held) = first else { panic!("{first:?}") };
        assert!(!held.verified && held.content == "hello");
        let Ok(NetEvent::Message(last)) = events.try_recv() else { panic!() };
        assert_eq!((last.id, last.verified), (held.id, false));
        // Her key known: verified straight away, nothing handed over before.
        let from = rns_identity::destination::Destination::hash_from_name_and_identity(super::super::LXMF_ASPECT, Some(&alice.hash));
        known.lock().unwrap().remember(from, &alice.get_public_key(), None);
        deliver_inbound(&runtime, &known, &policy, &signed_by(&alice, &alice, "again"), Heard::Unknown, &ev).await;
        assert!(matches!(events.try_recv(), Ok(NetEvent::Message(m)) if m.verified));
        assert!(events.try_recv().is_err());
        runtime.shutdown_and_wait().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
