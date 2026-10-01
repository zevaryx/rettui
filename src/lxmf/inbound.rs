//! Receiving: unpacking and verifying LXMF messages, however they arrived.

use std::time::Duration;

use lxmf_core::constants::{FIELD_FILE_ATTACHMENTS, FIELD_REPLY_QUOTE, FIELD_REPLY_TO};
use lxmf_core::message_api::LxMessage;
use rmpv::Value;
use rns_crypto::ed25519::Ed25519PublicKey;
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use super::fields::{self, FIELD_COLUMBA_EXTENSIONS};
use super::policy::SharedPolicy;
use super::{Attachment, Extras, InboundMessage, Reply, bytes_of, is_image_name};
use crate::net::{Hash, Known, NetEvent, lookup};

/// How long to look for an unknown sender's announce to verify a message.
const SENDER_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn attachments_of(message: &LxMessage) -> Vec<Attachment> {
    let mut out = Vec::new();
    if let Ok(Some((format, data))) = message.image_attachment() {
        let ext = if format.is_empty() { "img".to_string() } else { format };
        out.push(Attachment {
            name: format!("image.{ext}"),
            data: data.to_vec(),
            image: true,
        });
    }
    if let Some(field) = message.get_field(FIELD_FILE_ATTACHMENTS)
        && let Ok(Value::Array(entries)) = rmpv::decode::read_value(&mut field.as_slice())
    {
        for entry in entries {
            let Value::Array(parts) = entry else { continue };
            let (Some(name), Some(data)) = (
                parts.first().and_then(bytes_of),
                parts.get(1).and_then(bytes_of),
            ) else {
                continue;
            };
            let name = String::from_utf8_lossy(&name).into_owned();
            out.push(Attachment {
                image: is_image_name(&name),
                name,
                data,
            });
        }
    }
    out
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

fn signing_key(identity: &Identity) -> Option<Ed25519PublicKey> {
    let public = identity.get_public_key();
    let bytes: [u8; 32] = public[32..64].try_into().ok()?;
    Ed25519PublicKey::from_bytes(&bytes).ok()
}

/// Whether a message is from who it says: `Ok(true)` if its signature
/// matches the sender's key, `Ok(false)` if the key isn't known (kept, shown
/// as unverified), and an error if the key is known and the signature
/// doesn't match: someone else wrote it, so it's dropped, as Python LXMF
/// does.
fn check_signature(message: &mut LxMessage, sender: Option<&Identity>) -> Result<bool, String> {
    let Some(key) = sender.and_then(signing_key) else {
        return Ok(false);
    };
    if message.verify(&key) {
        Ok(true)
    } else {
        Err(format!(
            "Dropped a message claiming to be from {}: its signature doesn't match their key",
            hex::encode(message.source_hash)
        ))
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
    let mut message = LxMessage::unpack(data).map_err(|e| e.to_string())?;
    if let Some(policy) = policy {
        let policy = policy.lock().unwrap();
        if policy.too_big(data.len()) {
            return Err(format!(
                "Dropped a message from {}: {} KB, over your limit of {} KB",
                hex::encode(message.source_hash),
                data.len() / 1000,
                policy.max_bytes / 1000
            ));
        }
        if !policy.stamp_accepted(&mut message) {
            return Err(format!(
                "Dropped a message from {}: it had no valid stamp (you ask for stamp cost {})",
                hex::encode(message.source_hash),
                policy.stamp_cost.unwrap_or_default()
            ));
        }
    }
    // Senders of propagated messages are often offline; a bounded lookup
    // (cache, remembered keys, then a path request) finds their key.
    let sender = tokio::time::timeout(
        SENDER_LOOKUP_TIMEOUT,
        lookup(runtime, known, message.source_hash),
    )
    .await
    .unwrap_or_else(|_| Err("timed out".to_string()));
    let sender = sender
        .inspect_err(|e| {
            tracing::debug!("sender {} not known ({e}); cannot verify signature", hex::encode(message.source_hash));
        })
        .ok();
    let verified = check_signature(&mut message, sender.as_ref().map(|remote| &remote.identity))?;
    tracing::debug!("message from {} verified={verified}", hex::encode(message.source_hash));
    // Only from who it says, a ticket can be used for messages to them.
    if verified && let Some(policy) = policy {
        policy.lock().unwrap().take_ticket(&message);
    }
    let attachments = attachments_of(&message);
    let reply = reply_of(&message);
    let mut extras = Extras::of(&message);
    // A Codec2 voice message is decoded to play; it can take a moment.
    if let Some(audio) = extras.audio.take() {
        extras.audio = Some(tokio::task::spawn_blocking(move || audio.decoded()).await.map_err(|e| e.to_string())?);
    }
    Ok(InboundMessage {
        id: message.message_id.or(message.hash),
        source: message.source_hash,
        title: message.title,
        content: message.content,
        timestamp: message.timestamp,
        verified,
        attachments,
        paper: false,
        reply,
        extras,
    })
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
    ev: &mpsc::UnboundedSender<NetEvent>,
) {
    let event = match parse_inbound(runtime, known, Some(policy), data).await {
        Ok(message) => NetEvent::Message(Box::new(message)),
        Err(e) if e.starts_with("Dropped") => NetEvent::Log(e),
        Err(e) => NetEvent::Log(format!("Dropped malformed LXMF message: {e}")),
    };
    let _ = ev.send(event);
}

pub fn spawn_inbound(
    runtime: &ReticulumHandle,
    known: &Known,
    policy: &SharedPolicy,
    data: Vec<u8>,
    ev: &mpsc::UnboundedSender<NetEvent>,
) {
    let (runtime, known, policy, ev) = (runtime.clone(), known.clone(), policy.clone(), ev.clone());
    tokio::spawn(async move { deliver_inbound(&runtime, &known, &policy, &data, &ev).await });
}

#[cfg(test)]
mod tests {
    use lxmf_core::message_api::DeliveryMethod;

    use super::*;

    /// A message from `sender`, signed, as it arrives.
    fn signed_by(sender: &Identity, content: &str) -> Vec<u8> {
        let mut message = LxMessage::new([1; 16], [2; 16], "", content, DeliveryMethod::Direct);
        message.sign(&sender.get_signing_key().unwrap()).unwrap();
        message.pack().unwrap()
    }

    #[test]
    fn forged_messages_are_dropped_and_unknown_senders_unverified() {
        let (alice, mallory) = (Identity::new(), Identity::new());
        let data = signed_by(&alice, "hello");
        let check = |data: &[u8], sender: Option<&Identity>| check_signature(&mut LxMessage::unpack(data).unwrap(), sender);
        assert_eq!(check(&data, Some(&alice)), Ok(true));
        // Not known yet: kept, as unverified.
        assert_eq!(check(&data, None), Ok(false));
        // Signed by someone else, or changed on the way: dropped.
        assert!(check(&data, Some(&mallory)).unwrap_err().starts_with("Dropped a message claiming to be from 02020202"));
        let at = data.windows(5).position(|w| w == b"hello").unwrap();
        let mut changed = data.clone();
        changed[at..at + 5].copy_from_slice(b"jello");
        assert!(check(&changed, Some(&alice)).is_err());
    }
}
