//! Sending: direct delivery over a Link, with the propagation node as a
//! fallback (or the only route, in propagated mode).

use std::time::Duration;

use lxmf_core::constants::{
    FIELD_FILE_ATTACHMENTS, FIELD_REACTION, FIELD_RENDERER, FIELD_REPLY_QUOTE, FIELD_REPLY_TO, FIELD_TICKET, RENDERER_MARKDOWN,
    RENDERER_MICRON,
};
use lxmf_core::handlers::{parse_pn_announce_data, stamp_cost_from_app_data};
use lxmf_core::message_api::{DeliveryMethod, LxMessage, MessageError};
use rmpv::Value;
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use super::{Delivered, DeliveryMode, Outgoing, Sent, encode, is_image_name};
use crate::net::{Hash, Known, ensure_path, link_options, lookup};

/// How long to wait for a delivery proof after a message is on the Link.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) async fn build_message(
    identity: &Identity,
    source: Hash,
    outgoing: &Outgoing,
    app_data: Option<&[u8]>,
) -> Result<LxMessage, String> {
    let mut message =
        LxMessage::new(outgoing.to, source, "", &outgoing.content, DeliveryMethod::Direct);
    message.timestamp = outgoing.timestamp;
    message.stamp_cost = app_data.and_then(stamp_cost_from_app_data);
    message.determine_compression_support(app_data);

    // The first image goes in FIELD_IMAGE (shown inline by Sideband and
    // MeshChat); everything else is a file attachment.
    let mut files = Vec::new();
    let mut image_set = false;
    for path in &outgoing.attachments {
        let data = tokio::fs::read(path)
            .await
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());
        if !image_set && is_image_name(&name) {
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
            let ext = if ext == "jpeg" { "jpg".to_string() } else { ext };
            message.set_image_field(&ext, &data).map_err(|e| e.to_string())?;
            image_set = true;
        } else {
            files.push(Value::Array(vec![Value::from(name), Value::Binary(data)]));
        }
    }
    if !files.is_empty() {
        message
            .set_msgpack_field(FIELD_FILE_ATTACHMENTS, encode(&Value::Array(files)))
            .map_err(|e| e.to_string())?;
    }
    // Both as bytes (msgpack `bin`), as Columba and MeshChatX send them.
    if let Some(reply) = &outgoing.reply {
        message.set_field(FIELD_REPLY_TO, reply.to.to_vec());
        if let Some(quote) = &reply.quote {
            message.set_field(FIELD_REPLY_QUOTE, quote.as_bytes().to_vec());
        }
    }
    if let Some(reaction) = &outgoing.reaction {
        message
            .set_msgpack_field(FIELD_REACTION, super::fields::reaction_field(reaction))
            .map_err(|e| e.to_string())?;
    }
    // How its text is written, for clients that format it.
    if let Some(format) = outgoing.format
        && !outgoing.content.is_empty()
    {
        let renderer = match format {
            crate::markdown::TextFormat::Markdown => RENDERER_MARKDOWN,
            crate::markdown::TextFormat::Micron => RENDERER_MICRON,
        };
        message.set_msgpack_field(FIELD_RENDERER, encode(&Value::from(renderer))).map_err(|e| e.to_string())?;
    }
    // A ticket for them, and theirs to stamp this with (see `policy`).
    if let Some(ticket) = &outgoing.ticket {
        message.set_msgpack_field(FIELD_TICKET, ticket.clone()).map_err(|e| e.to_string())?;
    }
    message.outbound_ticket = outgoing.stamp_ticket;

    let key = identity
        .get_signing_key()
        .ok_or("Local identity has no signing key")?;
    message.sign(&key).map_err(|e| e.to_string())?;
    Ok(message)
}

pub async fn send(
    runtime: &ReticulumHandle,
    known: &Known,
    identity: &Identity,
    source: Hash,
    outgoing: Outgoing,
) -> Result<Sent, String> {
    if outgoing.mode == DeliveryMode::Paper {
        return Err("A paper message isn't sent: it's written as a link (see lxmf::paper)".into());
    }
    if outgoing.mode == DeliveryMode::Propagated {
        return propagate(runtime, known, identity, source, &outgoing).await;
    }
    let direct = send_direct(runtime, known, identity, source, &outgoing).await;
    match (direct, outgoing.mode, outgoing.propagation_node) {
        (Ok(hash), ..) => Ok(Sent { delivered: Delivered::Direct, hash }),
        (Err(e), DeliveryMode::Auto, Some(_)) => {
            tracing::info!("direct delivery failed ({e}); using propagation node");
            propagate(runtime, known, identity, source, &outgoing)
                .await
                .map_err(|pe| format!("direct: {e}; propagation: {pe}"))
        }
        (Err(e), ..) => Err(e),
    }
}

/// A built message's hash (signing sets it).
fn hash_of(message: &LxMessage) -> Result<[u8; 32], String> {
    message.hash.ok_or_else(|| "The message has no hash after signing".to_string())
}

async fn send_direct(
    runtime: &ReticulumHandle,
    known: &Known,
    identity: &Identity,
    source: Hash,
    outgoing: &Outgoing,
) -> Result<[u8; 32], String> {
    // A Link needs a path, not just a known key.
    ensure_path(runtime, outgoing.to).await?;
    let recipient = lookup(runtime, known, outgoing.to).await?;
    let mut message = build_message(identity, source, outgoing, recipient.app_data.as_deref()).await?;
    let hash = hash_of(&message)?;
    // Proof-of-work stamps can take a while at higher costs.
    let packed = tokio::task::spawn_blocking(move || {
        message.get_stamp();
        message.pack()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let LinkSession {
        handle, mut events, ..
    } = crate::net::connect(runtime, outgoing.to, Identity::new(), link_options("rettui.lxmf", false), &|_| {})
        .await
        .map_err(|e| format!("Link failed: {e}"))?;

    let result = if packed.len() <= handle.mdu() {
        match handle.send_packet(packed).await {
            Ok(receipt) => wait_for_proof(&mut events, receipt.packet_hash).await,
            Err(e) => Err(e.to_string()),
        }
    } else {
        send_resource(&handle, packed).await
    };
    handle.close().await;
    result.map(|()| hash)
}

async fn propagate(
    runtime: &ReticulumHandle,
    known: &Known,
    identity: &Identity,
    source: Hash,
    outgoing: &Outgoing,
) -> Result<Sent, String> {
    let node = outgoing
        .propagation_node
        .ok_or("No propagation node selected (pick one in the Network tab)")?;
    // The recipient may be offline: only its key is needed, not a path.
    let recipient = lookup(runtime, known, outgoing.to).await?;
    // This client's own node takes it without a Link.
    let local = outgoing.local_node.clone().filter(|local| local.hash == node);
    let stamp_cost = match &local {
        Some(local) => local.stamp_cost(),
        None => {
            ensure_path(runtime, node).await?;
            let node_info = lookup(runtime, known, node).await?;
            node_info.app_data.as_deref().and_then(parse_pn_announce_data).map_or(0, |d| d.stamp_cost)
        }
    };

    let mut message =
        build_message(identity, source, outgoing, recipient.app_data.as_deref()).await?;
    let hash = hash_of(&message)?;
    let (recipient_identity, ratchet) = (recipient.identity, recipient.ratchet);
    let packed = tokio::task::spawn_blocking(move || {
        message.get_stamp();
        message.pack_propagated_encrypted_with_stamp(
            |plaintext| {
                recipient_identity
                    .encrypt(plaintext, ratchet.as_ref())
                    .map_err(|e| MessageError::PackFailed(e.to_string()))
            },
            stamp_cost,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?
    .0;

    if let Some(local) = local {
        tokio::task::spawn_blocking(move || local.submit(&packed)).await.map_err(|e| e.to_string())??;
        return Ok(Sent { delivered: Delivered::Propagated, hash });
    }
    let LinkSession { handle, .. } = crate::net::connect(runtime, node, identity.clone(), link_options("rettui.propagation", true), &|_| {})
        .await
        .map_err(|e| format!("Link to propagation node failed: {e}"))?;
    let result = send_resource(&handle, packed).await;
    handle.close().await;
    result.map(|()| Sent { delivered: Delivered::Propagated, hash })
}

async fn send_resource(handle: &LinkSessionHandle, data: Vec<u8>) -> Result<(), String> {
    let transfer = handle
        .send_resource_bytes(
            data,
            ResourceOptions {
                auto_compress: true,
                metadata: None,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    match tokio::time::timeout(DELIVERY_TIMEOUT, transfer.concluded()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(format!("Transfer failed: {e}")),
        Err(_) => Err("Timed out waiting for transfer".to_string()),
    }
}

async fn wait_for_proof(
    events: &mut mpsc::Receiver<LinkSessionEvent>,
    packet_hash: [u8; 32],
) -> Result<(), String> {
    let wait = async {
        while let Some(event) = events.recv().await {
            match event {
                LinkSessionEvent::PacketDelivered { packet_hash: h } if h == packet_hash => {
                    return Ok(());
                }
                LinkSessionEvent::Closed { reason } => {
                    return Err(format!("Link closed before delivery ({reason:?})"));
                }
                _ => {}
            }
        }
        Err("Link closed before delivery".to_string())
    };
    tokio::time::timeout(DELIVERY_TIMEOUT, wait)
        .await
        .unwrap_or_else(|_| Err("Timed out waiting for delivery proof".to_string()))
}
