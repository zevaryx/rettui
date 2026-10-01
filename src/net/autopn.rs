//! Picking a propagation node automatically (the "Pick propagation node
//! automatically" setting, off unless turned on).
//!
//! Candidates are the propagation nodes heard announcing in the last day
//! that are serving, ask a stamp cost a phone can afford, and aren't hosted
//! here (that one doesn't pass messages on to other nodes). The nearest few
//! by hops are probed: a Link is set up to each and timed, as a ping. Of
//! those that answer about as fast as the fastest, the nearest is picked.
//!
//! Hop counts only choose which nodes are probed. They travel in the packet
//! header, outside the announce's signature, so a transport node on the way
//! can make a node look closer than it is; but no node can answer faster
//! than it really is, so the measured time decides.
//!
//! A node that's picked is kept while it answers: it's checked again every
//! few hours (and when syncing with it fails), and replaced only if it
//! stops answering or another answers in under half its time, so a new
//! node can't take over by being a little faster.

use std::collections::HashMap;
use std::time::Duration;

use super::{Hash, Known, Ping};

/// Most stamp cost asked of each message: more takes a phone minutes.
pub const MAX_STAMP_COST: u8 = 20;
/// Nodes not heard for this long are left out.
pub const FRESH: Duration = Duration::from_secs(24 * 3600);
/// How many of the nearest are probed (besides the one picked before).
pub const PROBED: usize = 4;
/// How often a picked node is checked again.
pub const RECHECK: Duration = Duration::from_secs(6 * 3600);
/// Least time between checks, however often they're asked for.
pub const MIN_GAP: Duration = Duration::from_secs(60);
/// After a check that found nothing, the next.
pub const RETRY: Duration = Duration::from_secs(10 * 60);

/// A propagation node that might be picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub hash: Hash,
    pub hops: u8,
    /// When it was last heard (Unix seconds).
    pub heard: i64,
    /// The stamp cost it asks, if its announce was kept.
    pub stamp_cost: Option<u8>,
}

/// How a probe of a candidate went.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub hash: Hash,
    pub hops: u8,
    pub answer: Result<Duration, String>,
}

/// The node picked, and what was learned picking it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub node: Hash,
    pub hops: u8,
    pub rtt: Duration,
    /// Whether it's another node than before.
    pub changed: bool,
    /// How many were probed, and how many answered.
    pub probed: usize,
    pub answered: usize,
}

/// The nodes to probe: those that may be picked, nearest first (most
/// recently heard first among equals), and the one picked before.
pub fn shortlist(candidates: &HashMap<Hash, Candidate>, now: i64, current: Option<Hash>, hosted: Option<Hash>) -> Vec<Candidate> {
    let fresh = |c: &Candidate| now.saturating_sub(c.heard) <= FRESH.as_secs() as i64;
    let affordable = |c: &Candidate| c.stamp_cost.is_none_or(|cost| cost <= MAX_STAMP_COST);
    let mut list: Vec<Candidate> = candidates
        .values()
        .filter(|c| Some(c.hash) != hosted && fresh(c) && affordable(c))
        .copied()
        .collect();
    list.sort_by_key(|c| (c.hops, std::cmp::Reverse(c.heard), c.hash));
    let mut shortlist: Vec<Candidate> = list.iter().take(PROBED).copied().collect();
    // The node picked before is checked too, wherever it is in the list.
    if let Some(current) = list.iter().find(|c| Some(c.hash) == current)
        && !shortlist.contains(current)
    {
        shortlist.push(*current);
    }
    shortlist
}

/// Whether an answer time is as good as the fastest: within a quarter of
/// it and 10 ms (which is noise).
fn comparable(rtt: Duration, fastest: Duration) -> bool {
    rtt <= fastest + fastest / 4 + Duration::from_millis(10)
}

/// The node to use after probing: the nearest of those that answered about
/// as fast as the fastest, unless the one picked before answered and the
/// fastest isn't twice as fast.
pub fn choose(probes: &[Probe], current: Option<Hash>) -> Option<(Hash, u8, Duration)> {
    let answered: Vec<(Hash, u8, Duration)> =
        probes.iter().filter_map(|p| p.answer.as_ref().ok().map(|rtt| (p.hash, p.hops, *rtt))).collect();
    let fastest = answered.iter().map(|(.., rtt)| *rtt).min()?;
    let best = answered
        .iter()
        .filter(|(.., rtt)| comparable(*rtt, fastest))
        .min_by_key(|(hash, hops, rtt)| (*hops, *rtt, *hash))
        .copied()?;
    match answered.iter().find(|(hash, ..)| Some(*hash) == current) {
        Some(&kept) if fastest * 2 > kept.2 => Some(kept),
        _ => Some(best),
    }
}

/// Probe the shortlist, one node at a time (to keep the traffic down), and
/// pick.
pub async fn evaluate(
    runtime: &rns_runtime::prelude::ReticulumHandle,
    known: &Known,
    shortlist: Vec<Candidate>,
    current: Option<Hash>,
) -> Result<Pick, String> {
    if shortlist.is_empty() {
        return Err("no propagation node heard in the last day that could be used".into());
    }
    let mut probes = Vec::new();
    for candidate in &shortlist {
        let answer = super::remote::ping(runtime, known, candidate.hash).await.map(|Ping { rtt, .. }| rtt);
        tracing::debug!("propagation node {} probed: {answer:?}", hex::encode(candidate.hash));
        probes.push(Probe { hash: candidate.hash, hops: candidate.hops, answer });
    }
    let answered = probes.iter().filter(|p| p.answer.is_ok()).count();
    let (node, hops, rtt) = choose(&probes, current).ok_or_else(|| format!("none of the {} nearest answered", probes.len()))?;
    Ok(Pick { node, hops, rtt, changed: Some(node) != current, probed: probes.len(), answered })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn candidate(n: u8, hops: u8, hours_ago: i64, stamp_cost: Option<u8>) -> Candidate {
        Candidate { hash: [n; 16], hops, heard: NOW - hours_ago * 3600, stamp_cost }
    }

    fn probe(n: u8, hops: u8, ms: Option<u64>) -> Probe {
        Probe { hash: [n; 16], hops, answer: ms.map(Duration::from_millis).ok_or_else(|| "no answer".to_string()) }
    }

    #[test]
    fn the_nearest_usable_nodes_are_probed() {
        let all: HashMap<Hash, Candidate> = [
            candidate(1, 2, 1, Some(16)),
            candidate(2, 2, 2, Some(16)),
            candidate(3, 3, 1, Some(16)),
            candidate(4, 2, 30, Some(16)),  // not heard for a day
            candidate(5, 1, 1, Some(40)),   // asks too much
            candidate(6, 1, 1, Some(16)),   // hosted here
            candidate(7, 5, 1, None),       // its announce wasn't kept: allowed
            candidate(8, 4, 1, Some(16)),
            candidate(9, 6, 1, Some(16)),   // the one picked before, far away
        ]
        .into_iter()
        .map(|c| (c.hash, c))
        .collect();
        let hashes = |list: Vec<Candidate>| list.iter().map(|c| c.hash[0]).collect::<Vec<_>>();
        assert_eq!(hashes(shortlist(&all, NOW, None, Some([6; 16]))), [1, 2, 3, 8]);
        assert_eq!(hashes(shortlist(&all, NOW, Some([9; 16]), Some([6; 16]))), [1, 2, 3, 8, 9]);
        assert!(shortlist(&HashMap::new(), NOW, None, None).is_empty());
    }

    #[test]
    fn the_fastest_answer_wins_and_a_working_pick_is_kept() {
        // As measured on a test network: A near and fast, B as near but
        // slow, C a hop further and fast, D made to look near (its hop
        // count forged) but slow, E unreachable.
        let probes = [probe(1, 2, Some(3)), probe(2, 2, Some(506)), probe(3, 3, Some(4)), probe(4, 3, Some(408)), probe(5, 2, None)];
        assert_eq!(choose(&probes, None).map(|(h, ..)| h[0]), Some(1));
        // C, picked before, still answers and A isn't twice as fast: kept.
        assert_eq!(choose(&probes, Some([3; 16])).map(|(h, ..)| h[0]), Some(3));
        // B, picked before, is far slower: replaced.
        assert_eq!(choose(&probes, Some([2; 16])).map(|(h, ..)| h[0]), Some(1));
        // E, picked before, stopped answering: replaced.
        assert_eq!(choose(&probes, Some([5; 16])).map(|(h, ..)| h[0]), Some(1));
        // About as fast (timing noise): the nearer.
        assert_eq!(choose(&[probe(3, 3, Some(3)), probe(1, 2, Some(4))], None).map(|(h, ..)| h[0]), Some(1));
        assert_eq!(choose(&[probe(3, 3, Some(200)), probe(1, 2, Some(240))], None).map(|(h, ..)| h[0]), Some(1));
        // Clearly faster, though further: the faster.
        assert_eq!(choose(&[probe(3, 3, Some(4)), probe(1, 2, Some(60))], None).map(|(h, ..)| h[0]), Some(3));
        assert_eq!(choose(&[probe(5, 2, None)], None), None);
    }
}
