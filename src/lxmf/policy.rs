//! What incoming messages must bring, and the tickets that spare them it.
//!
//! - **Stamp cost:** the proof-of-work asked of senders, announced with the
//!   LXMF address (as Python LXMF does). Messages without a valid stamp are
//!   dropped, unless they're from a contact: someone trusted, or someone
//!   you've written to.
//! - **Tickets:** a trusted contact is sent a ticket with your messages
//!   (LXMF's `FIELD_TICKET`); their client stamps later messages with it
//!   instead of doing the work. Tickets they send are used the same way for
//!   messages to them. rsLXMF's [`TicketStore`] keeps both kinds.
//! - **Size:** the largest message taken; larger transfers are refused
//!   before they're downloaded.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lxmf_core::constants::{FIELD_TICKET, TICKET_EXPIRY};
use lxmf_core::message_api::LxMessage;
use lxmf_core::ticket::{Ticket, TicketStore, TicketStoreSnapshot};
use serde::{Deserialize, Serialize};

use crate::net::Hash;

pub struct Policy {
    /// The stamp cost asked of senders, if any.
    pub stamp_cost: Option<u8>,
    /// The largest message taken, in bytes (0: any size).
    pub max_bytes: u64,
    /// Senders spared the stamp: your contacts.
    exempt: HashSet<Hash>,
    /// Contacts given tickets.
    trusted: HashSet<Hash>,
    tickets: TicketStore,
    path: PathBuf,
    /// Tickets changed since the last save.
    dirty: bool,
}

pub type SharedPolicy = Arc<Mutex<Policy>>;

/// The tickets file: rsLXMF's snapshot, with its map of last deliveries
/// as a list (JSON keys are text).
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    outbound: Vec<Ticket>,
    inbound: Vec<Ticket>,
    last_deliveries: Vec<(String, f64)>,
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

/// A stamp cost from a setting: 0 is none; LXMF's highest is 254.
pub fn stamp_cost(setting: u64) -> Option<u8> {
    (setting > 0).then(|| setting.min(254) as u8)
}

impl Policy {
    /// The policy, with the tickets saved at `path`.
    pub fn load(path: &Path, stamp_cost: Option<u8>, max_bytes: u64) -> SharedPolicy {
        let saved: Saved = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut tickets = TicketStore::new();
        tickets.replace_snapshot(TicketStoreSnapshot {
            outbound: saved.outbound,
            inbound: saved.inbound,
            last_deliveries: saved
                .last_deliveries
                .into_iter()
                .filter_map(|(hash, at)| Some((hex::decode(hash).ok()?.try_into().ok()?, at)))
                .collect(),
            ..TicketStoreSnapshot::default()
        });
        Arc::new(Mutex::new(Self {
            stamp_cost,
            max_bytes,
            exempt: HashSet::new(),
            trusted: HashSet::new(),
            tickets,
            path: path.to_path_buf(),
            dirty: false,
        }))
    }

    /// Who's spared the stamp, and who is given tickets.
    pub fn set_contacts(&mut self, trusted: Vec<Hash>, exempt: Vec<Hash>) {
        self.trusted = trusted.into_iter().collect();
        self.exempt = exempt.into_iter().chain(self.trusted.iter().copied()).collect();
    }

    /// Whether a message `size` bytes long is over the limit.
    pub fn too_big(&self, size: usize) -> bool {
        self.max_bytes > 0 && size as u64 > self.max_bytes
    }

    /// Whether a message has what it must: a valid stamp (by proof of work
    /// or a ticket of yours), unless none is asked or it's from a contact.
    pub fn stamp_accepted(&self, message: &mut LxMessage) -> bool {
        let Some(cost) = self.stamp_cost else { return true };
        if self.exempt.contains(&message.source_hash) {
            return true;
        }
        let tokens: Vec<Vec<u8>> =
            self.tickets.inbound_tokens(&message.source_hash, now()).into_iter().map(|t| t.to_vec()).collect();
        message.validate_stamp_with_tickets(cost, Some(&tokens))
    }

    /// Keep a ticket a (verified) message brought, to stamp messages to its
    /// sender with.
    pub fn take_ticket(&mut self, message: &LxMessage) {
        let Some(field) = message.get_field(FIELD_TICKET) else { return };
        if let Some(ticket) = Ticket::decode_field(message.source_hash, field, now()) {
            self.tickets.remember_outbound(ticket);
            self.dirty = true;
        }
    }

    /// For a message to `to`: their ticket to stamp it with, and a ticket of
    /// yours to give them (`FIELD_TICKET`'s value), if they're trusted and
    /// you ask for stamps (at most one a day, as Python LXMF).
    pub fn for_outgoing(&mut self, to: Hash) -> (Option<[u8; 16]>, Option<Vec<u8>>) {
        let now = now();
        let theirs = self.tickets.find_outbound(&to, now).map(|t| t.token);
        let ours = if self.stamp_cost.is_some() && self.trusted.contains(&to) {
            let issued = self.tickets.issue_for(to, TICKET_EXPIRY, now);
            self.dirty |= issued.is_some();
            issued.and_then(|t| t.encode_field().ok())
        } else {
            None
        };
        (theirs, ours)
    }

    /// A message that gave `to` a ticket got there.
    pub fn ticket_delivered(&mut self, to: Hash) {
        self.tickets.mark_ticket_delivered(to, now());
        self.dirty = true;
    }

    /// Write the tickets if they changed, off the async threads.
    pub async fn save_in_background(policy: &SharedPolicy) {
        let (path, saved) = {
            let mut policy = policy.lock().unwrap();
            if !policy.dirty {
                return;
            }
            policy.dirty = false;
            policy.tickets.cull(now());
            let snapshot = policy.tickets.snapshot();
            let saved = Saved {
                outbound: snapshot.outbound,
                inbound: snapshot.inbound,
                last_deliveries: snapshot.last_deliveries.into_iter().map(|(hash, at)| (hex::encode(hash), at)).collect(),
            };
            (policy.path.clone(), saved)
        };
        let written = tokio::task::spawn_blocking(move || {
            let text = serde_json::to_string(&saved)?;
            crate::config::write_private(&path, text.as_bytes())
        })
        .await;
        if let Ok(Err(e)) = written {
            tracing::warn!("could not save stamp tickets: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use lxmf_core::message_api::DeliveryMethod;
    use rns_runtime::prelude::Identity;

    use super::*;

    /// A message from `from`, as received, stamped with `stamp` if any.
    fn received(from: Hash, stamp: impl FnOnce(&mut LxMessage)) -> LxMessage {
        let mut message = LxMessage::new([1; 16], from, "", "hi", DeliveryMethod::Direct);
        message.sign(&Identity::new().get_signing_key().unwrap()).unwrap();
        stamp(&mut message);
        LxMessage::unpack(&message.pack().unwrap()).unwrap()
    }

    fn policy(name: &str, cost: Option<u8>) -> (PathBuf, SharedPolicy) {
        let path = std::env::temp_dir().join(format!("rettui-tickets-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let policy = Policy::load(&path, cost, 0);
        (path, policy)
    }

    #[test]
    fn stamps_are_asked_of_strangers_only() {
        let (path, policy) = policy("stamps", Some(4));
        let mut policy = policy.lock().unwrap();
        // No stamp: dropped; a valid one (cost 4 takes a moment): taken.
        assert!(!policy.stamp_accepted(&mut received([2; 16], |_| {})));
        let mut stamped = received([2; 16], |m| {
            m.stamp_cost = Some(4);
            m.get_stamp();
        });
        assert!(policy.stamp_accepted(&mut stamped));
        // A contact needs none.
        policy.set_contacts(Vec::new(), vec![[2; 16]]);
        assert!(policy.stamp_accepted(&mut received([2; 16], |_| {})));
        // Nor does anyone when no stamp is asked.
        policy.stamp_cost = None;
        assert!(policy.stamp_accepted(&mut received([3; 16], |_| {})));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn tickets_go_to_trusted_contacts_and_stamp_their_messages() {
        let (path, shared) = policy("tickets", Some(20));
        let (alice, bob) = ([2; 16], [3; 16]);
        let field = {
            let mut policy = shared.lock().unwrap();
            policy.set_contacts(vec![alice], Vec::new());
            assert_eq!(policy.for_outgoing(bob).1, None, "only trusted contacts get one");
            let (_, field) = policy.for_outgoing(alice);
            policy.ticket_delivered(alice);
            // Once a day.
            assert_eq!(policy.for_outgoing(alice).1, None);
            field.unwrap()
        };
        // Alice's client keeps it, and stamps her messages to us with it.
        let ticket = Ticket::decode_field([1; 16], &field, now()).unwrap();
        let mut reply = received(alice, |m| {
            m.outbound_ticket = Some(ticket.token);
            m.get_stamp();
        });
        // Cost 20 would take a long time by proof of work: the ticket does it.
        shared.lock().unwrap().set_contacts(Vec::new(), Vec::new());
        assert!(shared.lock().unwrap().stamp_accepted(&mut reply));
        assert!(!shared.lock().unwrap().stamp_accepted(&mut received(alice, |_| {})));
        // Tickets survive a restart.
        Policy::save_in_background(&shared).await;
        let again = Policy::load(&path, Some(20), 0);
        assert!(again.lock().unwrap().stamp_accepted(&mut reply));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn their_tickets_stamp_messages_to_them() {
        let (path, shared) = policy("outbound", None);
        let mut policy = shared.lock().unwrap();
        let ticket = Ticket::new([9; 16], [2; 16], now() + 3600.0);
        let mut message = LxMessage::new([1; 16], [2; 16], "", "hi", DeliveryMethod::Direct);
        message.set_msgpack_field(FIELD_TICKET, ticket.encode_field().unwrap()).unwrap();
        message.sign(&Identity::new().get_signing_key().unwrap()).unwrap();
        let received = LxMessage::unpack(&message.pack().unwrap()).unwrap();
        policy.take_ticket(&received);
        assert_eq!(policy.for_outgoing([2; 16]).0, Some([9; 16]));
        assert_eq!(policy.for_outgoing([3; 16]).0, None);
        assert!(!policy.too_big(10_000_000));
        policy.max_bytes = 500_000;
        assert!(policy.too_big(500_001) && !policy.too_big(500_000));
        assert_eq!((stamp_cost(0), stamp_cost(8), stamp_cost(999)), (None, Some(8), Some(254)));
        let _ = std::fs::remove_file(path);
    }
}
