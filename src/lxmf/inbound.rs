//! Receiving: unpacking and verifying LXMF messages, however they arrived.

use std::time::Duration;

use lxmf_core::constants::FIELD_FILE_ATTACHMENTS;
use lxmf_core::message_api::LxMessage;
use rmpv::Value;
use rns_crypto::ed25519::Ed25519PublicKey;
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use super::{Attachment, InboundMessage, bytes_of, is_image_name};
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

fn signing_key(identity: &Identity) -> Option<Ed25519PublicKey> {
    let public = identity.get_public_key();
    let bytes: [u8; 32] = public[32..64].try_into().ok()?;
    Ed25519PublicKey::from_bytes(&bytes).ok()
}

/// Unpack and verify one complete LXMF message (destination hash included).
pub async fn parse_inbound(
    runtime: &ReticulumHandle,
    known: &Known,
    data: &[u8],
) -> Result<InboundMessage, String> {
    let mut message = LxMessage::unpack(data).map_err(|e| e.to_string())?;
    // Senders of propagated messages are often offline; a bounded lookup
    // (cache, remembered keys, then a path request) finds their key.
    let sender = tokio::time::timeout(
        SENDER_LOOKUP_TIMEOUT,
        lookup(runtime, known, message.source_hash),
    )
    .await
    .unwrap_or_else(|_| Err("timed out".to_string()));
    let verified = match sender {
        Ok(remote) => signing_key(&remote.identity)
            .map(|key| message.verify(&key))
            .unwrap_or(false),
        Err(e) => {
            tracing::debug!(
                "sender {} not known ({e}); cannot verify signature",
                hex::encode(message.source_hash)
            );
            false
        }
    };
    tracing::debug!("message from {} verified={verified}", hex::encode(message.source_hash));
    let attachments = attachments_of(&message);
    Ok(InboundMessage {
        id: message.message_id.or(message.hash),
        source: message.source_hash,
        title: message.title,
        content: message.content,
        timestamp: message.timestamp,
        verified,
        attachments,
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
    data: &[u8],
    ev: &mpsc::UnboundedSender<NetEvent>,
) {
    let event = match parse_inbound(runtime, known, data).await {
        Ok(message) => NetEvent::Message(message),
        Err(e) => NetEvent::Log(format!("Dropped malformed LXMF message: {e}")),
    };
    let _ = ev.send(event);
}

pub fn spawn_inbound(
    runtime: &ReticulumHandle,
    known: &Known,
    data: Vec<u8>,
    ev: &mpsc::UnboundedSender<NetEvent>,
) {
    let (runtime, known, ev) = (runtime.clone(), known.clone(), ev.clone());
    tokio::spawn(async move { deliver_inbound(&runtime, &known, &data, &ev).await });
}
