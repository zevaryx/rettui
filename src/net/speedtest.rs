//! A speed test between two people, as NomadNet 1.4's (Utilities > Speed
//! Test) runs one, so either side can be NomadNet: each has a speed test
//! address (`nomadnetwork.utilities.speedtest`, from their identity), and
//! enters the other's. The sender opens a Link to the other's, identifies,
//! and sends a Resource of random bytes; the receiver takes it only from
//! the address it entered, and each side times it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::RngCore;
use rns_runtime::link_manager::{LinkResourceDirection, LinkResourceEvent};
use rns_runtime::prelude::*;
use tokio::sync::{mpsc, oneshot};

use super::{Hash, NetEvent, remote};

/// The speed test's Reticulum name, as NomadNet has it.
pub const ASPECT: &str = "nomadnetwork.utilities.speedtest";
/// Largest test sent (held in memory while it goes).
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
/// Longest a test may take, once the Link is up.
const TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// NomadNet waits this long after identifying, so the other side has taken
/// it in before the transfer is offered.
const AFTER_IDENTIFY: Duration = Duration::from_millis(600);

/// How a speed test is going.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeedEvent {
    /// Listening for the address given, or not (`None`).
    Listening(Result<Option<Hash>, String>),
    /// What's happening, in words ("Finding a path", "Link up: RTT 40 ms").
    Note(String),
    /// Someone else's test, turned away.
    Refused,
    /// How much has gone (`sending`) or come, 0 to 1.
    Progress {
        sending: bool,
        done: f32,
    },
    Done {
        sending: bool,
        bytes: usize,
        secs: f64,
    },
    Failed {
        sending: bool,
        why: String,
    },
}

/// The speed test address for an identity.
pub fn address(identity: Hash) -> Hash {
    rns_identity::destination::Destination::hash_from_name_and_identity(ASPECT, Some(&identity))
}

/// Listening for one person's tests, until stopped.
pub struct Listener {
    stop: Option<oneshot::Sender<()>>,
    done: Option<oneshot::Receiver<()>>,
}

impl Listener {
    /// Stop, and wait until the destination is gone: one listening again
    /// registers the same one, which the old one's going mustn't take
    /// away.
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(done) = self.done.take() {
            let _ = done.await;
        }
    }
}

fn say(ev: &mpsc::UnboundedSender<NetEvent>, event: SpeedEvent) {
    let _ = ev.send(NetEvent::Speed(event));
}

/// Take tests from the speed test address `from` only, at ours.
pub async fn listen(runtime: &ReticulumHandle, identity: Identity, from: Hash, ev: mpsc::UnboundedSender<NetEvent>) -> Listener {
    // Links identified as `from`: only they may send a test.
    let allowed: Arc<Mutex<HashSet<[u8; 16]>>> = Arc::default();
    let gate = {
        let allowed = allowed.clone();
        IdentityGatePolicy::new(move |link, identity| {
            let ok = address(identity) == from;
            if ok {
                allowed.lock().unwrap().insert(link);
            }
            ok
        })
    };
    let accept = {
        let allowed = allowed.clone();
        ResourceAcceptPolicy::new(move |link, advertisement| {
            allowed.lock().unwrap().contains(&link) && advertisement.data_size <= MAX_BYTES * 2
        })
    };
    let options = DestinationRuntimeOptions {
        accepts_links: true,
        resource_strategy: ResourceStrategy::AcceptApp,
        resource_accept: Some(accept),
        identity_gate: Some(gate),
        ..DestinationRuntimeOptions::default()
    };
    let (stop, mut stopped) = oneshot::channel();
    let registered = match runtime.register_destination(identity, ASPECT, options).await {
        Ok(registered) => registered,
        Err(e) => {
            say(&ev, SpeedEvent::Listening(Err(format!("Could not listen for speed tests: {e}"))));
            return Listener { stop: None, done: None };
        }
    };
    say(&ev, SpeedEvent::Listening(Ok(Some(from))));
    let (finished, done) = oneshot::channel();
    tokio::spawn(async move {
        let mut registered = registered;
        let events = &mut registered.events;
        // The test coming in: its link, when it started, how much it is.
        let mut coming: Option<([u8; 16], Instant, usize)> = None;
        let mut shown = 0;
        // Stopped when asked (another listener may be starting already, and
        // says so itself), or because the destination went.
        let asked = loop {
            tokio::select! {
                _ = &mut stopped => break true,
                Some((link, identity)) = events.links_identified.recv() => {
                    if address(identity) == from {
                        say(&ev, SpeedEvent::Note(format!("A test is coming from {}", hex::encode(from))));
                    } else {
                        let _ = link;
                        say(&ev, SpeedEvent::Refused);
                    }
                }
                Some(event) = events.resource_events.recv() => match event {
                    LinkResourceEvent::Started { link_id, direction: LinkResourceDirection::Inbound, data_size, .. } => {
                        coming = Some((link_id, Instant::now(), data_size));
                        shown = 0;
                        say(&ev, SpeedEvent::Progress { sending: false, done: 0.0 });
                    }
                    LinkResourceEvent::Progress { direction: LinkResourceDirection::Inbound, transferred, total, .. } if total > 0 => {
                        let done = transferred as f32 / total as f32;
                        let step = (done * 20.0) as u32;
                        if step > shown {
                            shown = step;
                            say(&ev, SpeedEvent::Progress { sending: false, done });
                        }
                    }
                    _ => {}
                },
                Some(completion) = events.resource_completions.recv() => {
                    let started = coming.take().filter(|(link, ..)| *link == completion.link_id).map(|(_, at, _)| at);
                    let secs = started.map_or(0.0, |at| at.elapsed().as_secs_f64()).max(0.001);
                    say(&ev, SpeedEvent::Done { sending: false, bytes: completion.data.len(), secs });
                }
                Some(link) = events.links_closed.recv() => {
                    allowed.lock().unwrap().remove(&link);
                    if coming.is_some_and(|(at, ..)| at == link) {
                        coming = None;
                        say(&ev, SpeedEvent::Failed { sending: false, why: "the link closed before it had all come".into() });
                    }
                }
                else => break false,
            }
        };
        let _ = registered.close().await;
        if !asked {
            say(&ev, SpeedEvent::Listening(Err("Stopped taking speed tests".into())));
        }
        let _ = finished.send(());
    });
    Listener { stop: Some(stop), done: Some(done) }
}

/// Send a test of `bytes` random bytes to the speed test address `to`,
/// identified as `identity` (so they know it's us).
pub async fn send(runtime: &ReticulumHandle, identity: Identity, to: Hash, bytes: usize, ev: mpsc::UnboundedSender<NetEvent>) {
    let failed = |why: String| SpeedEvent::Failed { sending: true, why };
    let note = |text: String| say(&ev, SpeedEvent::Note(text));
    let progress = |text: String| note(format!("{}…", text[..1].to_uppercase() + &text[1..]));
    let asked = Instant::now();
    let session = match remote::connect(runtime, to, identity, remote::link_options("rettui.speedtest", true), &progress).await {
        Ok(session) => session,
        Err(e) => return say(&ev, failed(e)),
    };
    let handle = session.handle;
    note(format!("Link up in {} ms, {} bytes a packet", asked.elapsed().as_millis(), handle.mdu()));
    tokio::time::sleep(AFTER_IDENTIFY).await;
    let mut data = vec![0u8; bytes.min(MAX_BYTES)];
    rand::thread_rng().fill_bytes(&mut data);
    let size = data.len();
    let started = Instant::now();
    let transfer = match handle.send_resource_bytes(data, ResourceOptions { auto_compress: false, metadata: None }).await {
        Ok(transfer) => transfer,
        Err(e) => {
            handle.close().await;
            return say(&ev, failed(format!("Could not start the test: {e}")));
        }
    };
    say(&ev, SpeedEvent::Progress { sending: true, done: 0.0 });
    let mut watch = transfer.progress();
    let concluded = transfer.concluded();
    let deadline = tokio::time::sleep(TIMEOUT);
    tokio::pin!(concluded, deadline);
    let mut shown = 0;
    let outcome = loop {
        tokio::select! {
            result = &mut concluded => break result.map_err(|e| format!("They didn't take it ({e})")),
            Ok(()) = watch.changed() => {
                let done = *watch.borrow_and_update() as f32;
                let step = (done * 20.0) as u32;
                if step > shown {
                    shown = step;
                    say(&ev, SpeedEvent::Progress { sending: true, done });
                }
            }
            () = &mut deadline => break Err("It took over 30 minutes".into()),
        }
    };
    handle.close().await;
    say(
        &ev,
        match outcome {
            Ok(_) => SpeedEvent::Done { sending: true, bytes: size, secs: started.elapsed().as_secs_f64().max(0.001) },
            Err(why) => failed(why),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_is_nomadnets() {
        // As RNS.Destination.hash_from_name_and_identity names it.
        let identity = [7u8; 16];
        let name = rns_crypto::sha::sha256(ASPECT.as_bytes());
        let mut material = name[..10].to_vec();
        material.extend_from_slice(&identity);
        assert_eq!(address(identity).to_vec(), rns_crypto::sha::sha256(&material)[..16].to_vec());
    }
}
