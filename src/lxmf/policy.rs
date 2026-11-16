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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lxmf_core::constants::{FIELD_TICKET, TICKET_EXPIRY, TICKET_GRACE};
use lxmf_core::message_api::LxMessage;
use lxmf_core::ticket::{Ticket, TicketStore, TicketStoreSnapshot};
use serde::{Deserialize, Serialize};

use crate::net::Hash;

/// What [`Policy::stamp_check`] found.
pub enum StampCheck {
    Accepted,
    Needs { cost: u8, tokens: Vec<Vec<u8>> },
}

impl StampCheck {
    /// Whether `message` has what is needed (the slow part).
    pub fn passes(&self, message: &LxMessage) -> bool {
        match self {
            StampCheck::Accepted => true,
            StampCheck::Needs { cost, tokens } => message.validate_stamp_with_tickets(*cost, Some(tokens)),
        }
    }
}

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
    /// Who gave a ticket kept to stamp messages to them with, and when it
    /// runs out: [`TicketStore`] doesn't say how many it holds.
    learned: HashMap<Hash, f64>,
    path: PathBuf,
    /// Tickets changed since the last save.
    dirty: bool,
}

pub type SharedPolicy = Arc<Mutex<Policy>>;

/// The tickets file: rsLXMF's snapshot, with its map of last deliveries
/// as a list (JSON keys are text).
/// The most `tickets.json` may be when read: it holds a few tickets (about
/// 100 bytes each) for each contact.
const MAX_TICKETS_BYTES: u64 = 16 * 1024 * 1024;
/// The most tickets kept that others gave, one per sender (about 160 bytes
/// each in the file): a contact sends one, so this is far more contacts than
/// anyone has, and enough that strangers can't fill the file.
const MAX_LEARNED_TICKETS: usize = 4096;
/// The longest a ticket that someone gave is kept, from when it arrived.
/// LXMF issues them for [`TICKET_EXPIRY`]; the grace period covers a clock
/// that is a few days out. One that says it lasts longer is cut to this,
/// so it is culled in time.
const MAX_LEARNED_LIFETIME: f64 = (TICKET_EXPIRY + TICKET_GRACE) as f64;

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
        let saved: Saved =
            crate::config::read_limited(path, MAX_TICKETS_BYTES).ok().and_then(|t| serde_json::from_slice(&t).ok()).unwrap_or_default();
        // What others gave: not past what they may be, nor more than are
        // kept (a file from before this, or restored, may hold any).
        let at = now();
        let mut learned = HashMap::new();
        let mut outbound = Vec::new();
        for mut ticket in saved.outbound.into_iter().filter(|ticket| ticket.is_valid(at)) {
            if !learned.contains_key(&ticket.destination_hash) && learned.len() >= MAX_LEARNED_TICKETS {
                continue;
            }
            ticket.expires = ticket.expires.min(at + MAX_LEARNED_LIFETIME);
            learned.insert(ticket.destination_hash, ticket.expires);
            outbound.push(ticket);
        }
        let mut tickets = TicketStore::new();
        tickets.replace_snapshot(TicketStoreSnapshot {
            outbound,
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
            learned,
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

    /// What a message must have: nothing more, or a stamp of this cost (or
    /// one of these tickets), unless none is asked or it's from a contact.
    /// Checking the stamp takes a while, so it's left to the caller, after the
    /// policy is let go.
    pub fn stamp_check(&self, message: &LxMessage) -> StampCheck {
        let Some(cost) = self.stamp_cost else { return StampCheck::Accepted };
        if self.exempt.contains(&message.source_hash()) {
            return StampCheck::Accepted;
        }
        let tokens = self.tickets.inbound_tokens(&message.source_hash(), now()).into_iter().map(|t| t.to_vec()).collect();
        StampCheck::Needs { cost, tokens }
    }

    /// Whether a message has what it must: a valid stamp (by proof of work
    /// or a ticket of yours), unless none is asked or it's from a contact.
    #[cfg(test)]
    pub fn stamp_accepted(&self, message: &LxMessage) -> bool {
        self.stamp_check(message).passes(message)
    }

    /// Keep a ticket a (verified) message brought, to stamp messages to its
    /// sender with.
    pub fn take_ticket(&mut self, message: &LxMessage) {
        let Some(field) = message.get_field(FIELD_TICKET) else { return };
        let at = now();
        if let Some(ticket) = Ticket::decode_field(message.source_hash(), super::fields::raw(field), at) {
            self.learn(ticket, at);
        }
    }

    /// Keep `ticket` to stamp messages to its sender with: for no longer
    /// than [`MAX_LEARNED_LIFETIME`] (one that says it lasts for ever would
    /// never be culled), and not past [`MAX_LEARNED_TICKETS`] senders, whose
    /// expired tickets go first (a sender already kept is always updated).
    fn learn(&mut self, mut ticket: Ticket, at: f64) {
        ticket.expires = ticket.expires.min(at + MAX_LEARNED_LIFETIME);
        if !self.learned.contains_key(&ticket.destination_hash) && self.learned.len() >= MAX_LEARNED_TICKETS {
            self.tickets.cull(at);
            self.learned.retain(|_, expires| *expires > at);
            if self.learned.len() >= MAX_LEARNED_TICKETS {
                tracing::debug!("a ticket was not kept: {MAX_LEARNED_TICKETS} senders have given one that is still good");
                return;
            }
        }
        self.learned.insert(ticket.destination_hash, ticket.expires);
        self.tickets.remember_outbound(ticket);
        self.dirty = true;
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
            let at = now();
            policy.tickets.cull(at);
            policy.learned.retain(|_, expires| *expires > at);
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
    use lxmf_core::message_api::{DeliveryMethod, LxMessageDraft};
    use rns_runtime::prelude::Identity;

    use super::*;

    /// A message from `from`, as received, stamped with `stamp` if any.
    fn received(from: Hash, stamp: impl FnOnce(&mut LxMessageDraft)) -> LxMessage {
        let mut message = LxMessageDraft::new([1; 16], from, "", "hi", DeliveryMethod::Direct).unwrap();
        stamp(&mut message);
        let message = super::super::send::finish(message.sign(&Identity::new().get_signing_key().unwrap()).unwrap()).unwrap();
        LxMessage::unpack(message.wire_bytes()).unwrap()
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
        assert!(!policy.stamp_accepted(&received([2; 16], |_| {})));
        let stamped = received([2; 16], |m| {
            m.stamp_cost = Some(4);
        });
        assert!(policy.stamp_accepted(&stamped));
        // A contact needs none.
        policy.set_contacts(Vec::new(), vec![[2; 16]]);
        assert!(policy.stamp_accepted(&received([2; 16], |_| {})));
        // Nor does anyone when no stamp is asked.
        policy.stamp_cost = None;
        assert!(policy.stamp_accepted(&received([3; 16], |_| {})));
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
        let reply = received(alice, |m| {
            m.outbound_ticket = Some(ticket.token);
        });
        // Cost 20 would take a long time by proof of work: the ticket does it.
        shared.lock().unwrap().set_contacts(Vec::new(), Vec::new());
        assert!(shared.lock().unwrap().stamp_accepted(&reply));
        assert!(!shared.lock().unwrap().stamp_accepted(&received(alice, |_| {})));
        // Tickets survive a restart.
        Policy::save_in_background(&shared).await;
        let again = Policy::load(&path, Some(20), 0);
        assert!(again.lock().unwrap().stamp_accepted(&reply));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn their_tickets_stamp_messages_to_them() {
        let (path, shared) = policy("outbound", None);
        let mut policy = shared.lock().unwrap();
        let ticket = Ticket::new([9; 16], [2; 16], now() + 3600.0);
        let mut message = LxMessageDraft::new([1; 16], [2; 16], "", "hi", DeliveryMethod::Direct).unwrap();
        message.set_msgpack_field(FIELD_TICKET, ticket.encode_field().unwrap()).unwrap();
        let message = message.sign(&Identity::new().get_signing_key().unwrap()).unwrap().finalize(None).unwrap();
        let received = LxMessage::unpack(message.wire_bytes()).unwrap();
        policy.take_ticket(&received);
        assert_eq!(policy.for_outgoing([2; 16]).0, Some([9; 16]));
        assert_eq!(policy.for_outgoing([3; 16]).0, None);
        assert!(!policy.too_big(10_000_000));
        policy.max_bytes = 500_000;
        assert!(policy.too_big(500_001) && !policy.too_big(500_000));
        assert_eq!((stamp_cost(0), stamp_cost(8), stamp_cost(999)), (None, Some(8), Some(254)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn tickets_given_are_cut_to_a_normal_life_and_kept_for_only_so_many_senders() {
        let (path, shared) = policy("learned", None);
        let mut policy = shared.lock().unwrap();
        let at = 1_800_000_000.0;
        let from = |n: usize| {
            let mut sender = [0u8; 16];
            sender[..8].copy_from_slice(&(n as u64 + 1).to_be_bytes());
            sender
        };
        // One that says it lasts for ever lasts as long as a ticket may.
        policy.learn(Ticket::new([1; 16], from(0), 1e300), at);
        assert!(policy.tickets.find_outbound(&from(0), at + MAX_LEARNED_LIFETIME - 1.0).is_some());
        assert!(policy.tickets.find_outbound(&from(0), at + MAX_LEARNED_LIFETIME + 1.0).is_none());
        // Senders up to the limit: then no more, though one kept is updated.
        for n in 1..MAX_LEARNED_TICKETS + 100 {
            policy.learn(Ticket::new([2; 16], from(n), at + 1000.0), at);
        }
        assert_eq!(policy.tickets.snapshot().outbound.len(), MAX_LEARNED_TICKETS);
        assert!(policy.tickets.find_outbound(&from(MAX_LEARNED_TICKETS + 50), at).is_none());
        policy.learn(Ticket::new([3; 16], from(1), at + 2000.0), at);
        assert_eq!(policy.tickets.find_outbound(&from(1), at).map(|t| t.token), Some([3; 16]));
        assert_eq!(policy.tickets.snapshot().outbound.len(), MAX_LEARNED_TICKETS);
        // Once those have run out, room is made.
        policy.learn(Ticket::new([4; 16], from(MAX_LEARNED_TICKETS + 50), at + 5000.0), at + 1500.0);
        assert_eq!(policy.tickets.find_outbound(&from(MAX_LEARNED_TICKETS + 50), at + 1500.0).map(|t| t.token), Some([4; 16]));
        assert!(policy.tickets.snapshot().outbound.len() < 10);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn tickets_read_from_the_file_are_cut_and_counted_too() {
        let path = std::env::temp_dir().join(format!("rettui-tickets-cut-{}.json", std::process::id()));
        let at = now();
        let from = |n: usize| {
            let mut sender = [0u8; 16];
            sender[..8].copy_from_slice(&(n as u64 + 1).to_be_bytes());
            sender
        };
        let mut saved = Saved::default();
        saved.outbound.push(Ticket::new([1; 16], from(0), 1e300));
        saved.outbound.push(Ticket::new([1; 16], from(1), at - 5.0));
        for n in 2..MAX_LEARNED_TICKETS + 50 {
            saved.outbound.push(Ticket::new([1; 16], from(n), at + 3600.0));
        }
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let policy = Policy::load(&path, None, 0);
        let kept = policy.lock().unwrap().tickets.snapshot().outbound;
        assert_eq!(kept.len(), MAX_LEARNED_TICKETS, "no more than the limit, none that had run out");
        assert!(kept.iter().all(|ticket| ticket.expires <= at + MAX_LEARNED_LIFETIME + 5.0), "none that lasts for ever");
        assert!(kept.iter().all(|ticket| ticket.destination_hash != from(1)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_tickets_file_of_far_more_than_tickets_is_not_read() {
        let path = std::env::temp_dir().join(format!("rettui-tickets-big-{}.json", std::process::id()));
        let ticket = |at: f64| format!(r#"{{"outbound":[],"inbound":[],"last_deliveries":[["{}",{at}]]}}"#, "ab".repeat(16));
        // A valid file, padded past the limit: ignored. Within it: read.
        std::fs::write(&path, format!("{}{}", ticket(5.0), " ".repeat(MAX_TICKETS_BYTES as usize))).unwrap();
        let policy = Policy::load(&path, None, 1000);
        assert!(policy.lock().unwrap().tickets.snapshot().last_deliveries.is_empty());
        std::fs::write(&path, ticket(5.0)).unwrap();
        let policy = Policy::load(&path, None, 1000);
        assert_eq!(policy.lock().unwrap().tickets.snapshot().last_deliveries.len(), 1);
        let _ = std::fs::remove_file(&path);
    }
}
