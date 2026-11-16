//! What the user chose to keep, told to Reticulum so a flood of announces
//! from strangers cannot push it out of the path table or the remembered
//! destinations (which are capped). Two marks, both from the library's own
//! transport queries:
//!
//! - **retained** (`RetainDestination`): contacts, the propagation node, saved
//!   NomadNet nodes, RRC hubs and rngit nodes. Kept as long as the list says
//!   so, released (`UnretainDestination`) when it no longer does. The list is
//!   cut at [`MAX_RETAINED`], and comes only from what the user saved, never
//!   from what a peer sent.
//! - **used** (`UseDestination`): a destination the user wrote to, pinged or
//!   opened a link to. Marked once per run, up to [`MAX_USED`] of them.
//!
//! The library only knows a destination once it has heard it announce, so a
//! retain for one it has not heard comes back false and is tried again.

use std::collections::{BTreeSet, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};

use rns_runtime::prelude::ReticulumHandle;
use rns_transport::messages::{TransportQuery, TransportQueryResponse};

use super::Hash;

/// Most destinations retained at once. Contacts come first in the list, so
/// a user with more than this loses the least important, not the people.
pub const MAX_RETAINED: usize = 2048;
/// Most destinations marked used in one run. The library keeps a mark until
/// it expires, and this keeps one run from marking without limit.
pub const MAX_USED: usize = 1024;
/// How often (in stats intervals) a retain the library could not take yet
/// is tried again: 60 s.
pub const RETRY_EVERY: u32 = 12;

/// The transport queries the marks go through: the runtime's handle, or a
/// stand-in in tests.
pub trait Control: Send + Sync {
    fn query(&self, query: TransportQuery) -> impl Future<Output = Option<TransportQueryResponse>> + Send;
}

impl<T: Control> Control for Arc<T> {
    fn query(&self, query: TransportQuery) -> impl Future<Output = Option<TransportQueryResponse>> + Send {
        (**self).query(query)
    }
}

impl Control for ReticulumHandle {
    fn query(&self, query: TransportQuery) -> impl Future<Output = Option<TransportQueryResponse>> + Send {
        self.query_control(query)
    }
}

#[derive(Default)]
struct State {
    wanted: Vec<Hash>,
    held: BTreeSet<Hash>,
    used: HashSet<Hash>,
}

#[derive(Default)]
pub struct Pins {
    state: Mutex<State>,
    /// One sync at a time, so a retain cannot overtake the release before it.
    gate: tokio::sync::Mutex<()>,
}

impl Pins {
    /// The destinations to retain, most important first: duplicates are
    /// dropped and the list is cut at [`MAX_RETAINED`].
    pub fn want(&self, destinations: impl IntoIterator<Item = Hash>) {
        let mut seen = HashSet::new();
        let wanted = destinations.into_iter().filter(|d| seen.insert(*d)).take(MAX_RETAINED).collect();
        self.state.lock().unwrap().wanted = wanted;
    }

    /// Whether the library has not yet been told everything: something
    /// wanted is not held, or something held is not wanted.
    pub fn unsettled(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.held.len() != state.wanted.len() || state.wanted.iter().any(|d| !state.held.contains(d))
    }

    /// Retain what is wanted and release what is not. A destination the
    /// library does not know yet stays wanted and is tried again next time.
    pub async fn sync(&self, control: &impl Control) {
        let _one_at_a_time = self.gate.lock().await;
        let (add, drop): (Vec<Hash>, Vec<Hash>) = {
            let state = self.state.lock().unwrap();
            let wanted: HashSet<&Hash> = state.wanted.iter().collect();
            (
                state.wanted.iter().filter(|d| !state.held.contains(*d)).copied().collect(),
                state.held.iter().filter(|d| !wanted.contains(d)).copied().collect(),
            )
        };
        for dest in drop {
            if control.query(TransportQuery::UnretainDestination { dest }).await.is_some() {
                self.state.lock().unwrap().held.remove(&dest);
            }
        }
        for dest in add {
            if let Some(TransportQueryResponse::BoolResult(true)) = control.query(TransportQuery::RetainDestination { dest }).await {
                self.state.lock().unwrap().held.insert(dest);
            }
        }
    }

    /// Whether `destination` is to be marked used now: not yet in this run,
    /// and the run has not used up its marks. Says yes once per destination.
    fn take_use(&self, destination: Hash) -> bool {
        let mut state = self.state.lock().unwrap();
        state.used.len() < MAX_USED && state.used.insert(destination)
    }
}

/// Mark `destination` used in the background, if it hasn't been in this run
/// (the caller is a command that goes on to send or open a link).
pub fn mark_used<C: Control + Clone + 'static>(pins: &Arc<Pins>, control: &C, destination: Hash) {
    if pins.take_use(destination) {
        let control = control.clone();
        tokio::spawn(async move {
            control.query(TransportQuery::UseDestination { dest: destination }).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library that knows some destinations and records what it was asked.
    #[derive(Default)]
    struct Fake {
        known: Mutex<HashSet<Hash>>,
        retained: Mutex<HashSet<Hash>>,
        used: Mutex<Vec<Hash>>,
    }

    impl Control for Fake {
        async fn query(&self, query: TransportQuery) -> Option<TransportQueryResponse> {
            let known = |d: &Hash| self.known.lock().unwrap().contains(d);
            Some(TransportQueryResponse::BoolResult(match query {
                TransportQuery::RetainDestination { dest } => known(&dest) && self.retained.lock().unwrap().insert(dest),
                TransportQuery::UnretainDestination { dest } => known(&dest) && self.retained.lock().unwrap().remove(&dest),
                TransportQuery::UseDestination { dest } => {
                    self.used.lock().unwrap().push(dest);
                    known(&dest)
                }
                _ => return None,
            }))
        }
    }

    fn fake(known: &[u8]) -> Fake {
        let fake = Fake::default();
        fake.known.lock().unwrap().extend(known.iter().map(|b| [*b; 16]));
        fake
    }

    #[tokio::test]
    async fn retains_what_the_library_knows_and_tries_the_rest_again() {
        let (pins, library) = (Pins::default(), fake(&[1, 2]));
        pins.want([[1; 16], [2; 16], [3; 16]]);
        assert!(pins.unsettled());
        pins.sync(&library).await;
        assert_eq!(library.retained.lock().unwrap().len(), 2);
        assert!(pins.unsettled(), "[3] has not been heard, so it stays wanted");
        library.known.lock().unwrap().insert([3; 16]);
        pins.sync(&library).await;
        assert_eq!(library.retained.lock().unwrap().len(), 3);
        assert!(!pins.unsettled());
    }

    #[tokio::test]
    async fn releases_what_is_no_longer_wanted() {
        let (pins, library) = (Pins::default(), fake(&[1, 2]));
        pins.want([[1; 16], [2; 16]]);
        pins.sync(&library).await;
        pins.want([[2; 16]]);
        assert!(pins.unsettled());
        pins.sync(&library).await;
        assert_eq!(*library.retained.lock().unwrap(), HashSet::from([[2; 16]]));
        assert!(!pins.unsettled());
    }

    #[test]
    fn the_wanted_list_is_deduplicated_and_cut_keeping_the_first() {
        let pins = Pins::default();
        let many = (0..MAX_RETAINED as u32 + 500).map(|n| {
            let mut hash = [0u8; 16];
            hash[..4].copy_from_slice(&n.to_be_bytes());
            hash
        });
        pins.want([[9; 16]].into_iter().chain([[9; 16]]).chain(many));
        let state = pins.state.lock().unwrap();
        assert_eq!(state.wanted.len(), MAX_RETAINED);
        assert_eq!(state.wanted[0], [9; 16], "what comes first is kept");
    }

    #[tokio::test]
    async fn a_destination_is_marked_used_once_and_the_run_has_a_limit() {
        let (pins, library) = (Arc::new(Pins::default()), Arc::new(fake(&[1])));
        mark_used(&pins, &library, [1; 16]);
        mark_used(&pins, &library, [1; 16]);
        for n in 0..MAX_USED as u32 + 100 {
            let mut hash = [0u8; 16];
            hash[..4].copy_from_slice(&n.to_be_bytes());
            hash[15] = 1;
            mark_used(&pins, &library, hash);
        }
        while library.used.lock().unwrap().len() < MAX_USED {
            tokio::task::yield_now().await;
        }
        tokio::task::yield_now().await;
        let used = library.used.lock().unwrap();
        assert_eq!(used.len(), MAX_USED, "the run's limit, counting the first");
        assert_eq!(used.iter().filter(|d| **d == [1; 16]).count(), 1, "once for each destination");
    }
}
