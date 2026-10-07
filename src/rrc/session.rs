//! One RRC hub connection: an identified Link, the HELLO/WELCOME handshake,
//! keepalive PING/PONG, and notices delivered as Link Resources. Everything
//! else is decoded and passed to the app, which owns the chat state.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rns_identity::destination::Destination;
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use crate::net::{Hash, Known, NetEvent, lookup};
use crate::rrc::{self, Envelope, t};

/// NomadNet retries HELLO up to five times before giving up.
const HELLO_ATTEMPTS: u32 = 5;
const HELLO_INTERVAL: Duration = Duration::from_secs(5);
/// Largest notice/MOTD accepted as a Resource (NomadNet's default).
const MAX_RESOURCE_BYTES: usize = 256 * 1024;
/// How long a RESOURCE_ENVELOPE announcement waits for its Resource.
const RESOURCE_EXPECTATION: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum SessionCommand {
    Send(Vec<u8>),
    /// Re-HELLO (e.g. after a nick change; the hub then re-welcomes).
    Hello(Option<String>),
    Ping,
    Disconnect,
}

#[derive(Debug)]
pub enum RrcEvent {
    Status(String),
    Welcome {
        welcome: rrc::Welcome,
        hub_identity: Vec<u8>,
    },
    Envelope(Envelope),
    /// A notice or MOTD that arrived as a Resource.
    ResourceText {
        kind: String,
        room: Option<String>,
        text: String,
    },
    Pong {
        rtt_ms: u64,
    },
    SendFailed(String),
    /// The session ended; `None` when we asked for it.
    Disconnected(Option<String>),
}

pub struct SessionConfig {
    pub hub: Hash,
    pub aspect: String,
    pub nick: Option<String>,
}

/// Running hub sessions, keyed by hub address (owned by the network actor),
/// and every session task, so shutting down can wait for links to close.
#[derive(Default)]
pub struct Sessions(HashMap<Hash, mpsc::UnboundedSender<SessionCommand>>, Vec<tokio::task::JoinHandle<()>>);

impl Sessions {
    /// Start a session unless one is already running for this hub.
    pub fn connect(
        &mut self,
        runtime: &ReticulumHandle,
        known: &Known,
        identity: &Identity,
        config: SessionConfig,
        ev: &mpsc::UnboundedSender<NetEvent>,
    ) {
        if self.0.get(&config.hub).is_some_and(|tx| !tx.is_closed()) {
            return;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        self.0.insert(config.hub, tx);
        self.1.retain(|task| !task.is_finished());
        self.1.push(tokio::spawn(run(runtime.clone(), known.clone(), identity.clone(), config, rx, ev.clone())));
    }

    /// Close every hub link (so hubs see us leave at once, rather than
    /// when their watchdog gives up), waiting a few seconds at most.
    pub async fn close_all(self) {
        for tx in self.0.into_values() {
            let _ = tx.send(SessionCommand::Disconnect);
        }
        let all = futures_util::future::join_all(self.1);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), all).await;
    }

    pub fn command(&mut self, hub: Hash, command: SessionCommand, ev: &mpsc::UnboundedSender<NetEvent>) {
        let tx = match command {
            SessionCommand::Disconnect => self.0.remove(&hub),
            _ => self.0.get(&hub).cloned(),
        };
        let is_send = matches!(command, SessionCommand::Send(_));
        let delivered = tx.is_some_and(|tx| tx.send(command).is_ok());
        if is_send && !delivered {
            let _ = ev.send(NetEvent::Rrc { hub, event: RrcEvent::SendFailed("not connected to this hub".into()) });
        }
    }
}

fn hello(identity_hash: &[u8], nick: Option<&str>) -> Vec<u8> {
    Envelope::new(t::HELLO, identity_hash).body(rrc::hello_body()).nick(nick).encode()
}

async fn run(
    runtime: ReticulumHandle,
    known: Known,
    identity: Identity,
    config: SessionConfig,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    ev: mpsc::UnboundedSender<NetEvent>,
) {
    let hub = config.hub;
    let emit = |event: RrcEvent| {
        let _ = ev.send(NetEvent::Rrc { hub, event });
    };
    let reason = session(&runtime, &known, &identity, config, &mut commands, &emit).await;
    emit(RrcEvent::Disconnected(reason));
}

/// Runs until the Link closes; returns why (or `None` if we disconnected).
async fn session(
    runtime: &ReticulumHandle,
    known: &Known,
    identity: &Identity,
    config: SessionConfig,
    commands: &mut mpsc::UnboundedReceiver<SessionCommand>,
    emit: &(impl Fn(RrcEvent) + Send + Sync),
) -> Option<String> {
    let hub = config.hub;
    emit(RrcEvent::Status("Finding hub".into()));
    let progress = |text: String| emit(RrcEvent::Status(text));
    if let Err(e) = crate::net::find_path(runtime, hub, &progress).await {
        return Some(e);
    }
    let remote = match lookup(runtime, known, hub).await {
        Ok(remote) => remote,
        Err(e) => return Some(e),
    };
    // The address must really be this aspect's destination for that identity.
    if Destination::hash_from_name_and_identity(&config.aspect, Some(&remote.identity.hash)) != hub {
        return Some(format!("hub address does not match aspect {}", config.aspect));
    }

    emit(RrcEvent::Status("Connecting".into()));
    // The hub drops everything until the Link identifies us.
    let options = crate::net::link_options("rettui.rrc", true);
    let LinkSession { handle, mut events, mut resource_offers } =
        match crate::net::connect(runtime, hub, identity.clone(), options, &progress).await {
            Ok(session) => session,
            Err(e) => return Some(format!("Link failed: {e}")),
        };

    let own = identity.hash;
    let mut nick = config.nick;
    emit(RrcEvent::Status("Identified, sending HELLO".into()));
    let _ = handle.send_packet(hello(&own, nick.as_deref())).await;
    let mut hello_attempts = 1;
    let mut welcomed = false;
    let mut hello_timer = tokio::time::interval(HELLO_INTERVAL);
    hello_timer.tick().await;

    let mut pings: HashMap<Vec<u8>, Instant> = HashMap::new();
    let mut expected: Vec<(rrc::ResourceAnnouncement, Instant)> = Vec::new();
    let (resource_tx, mut resource_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(SessionCommand::Send(payload)) => {
                    if payload.len() > handle.mdu() {
                        emit(RrcEvent::SendFailed(format!(
                            "message too large for this link ({} > {} bytes)",
                            payload.len(),
                            handle.mdu()
                        )));
                    } else if let Err(e) = handle.send_packet(payload).await {
                        emit(RrcEvent::SendFailed(e.to_string()));
                    }
                }
                Some(SessionCommand::Hello(new_nick)) => {
                    nick = new_nick;
                    let _ = handle.send_packet(hello(&own, nick.as_deref())).await;
                }
                Some(SessionCommand::Ping) => {
                    let body = rand::random::<[u8; 8]>().to_vec();
                    pings.retain(|_, sent| sent.elapsed() < Duration::from_secs(60));
                    pings.insert(body.clone(), Instant::now());
                    let ping = Envelope::new(t::PING, &own).body(ciborium::Value::Bytes(body));
                    if let Err(e) = handle.send_packet(ping.encode()).await {
                        emit(RrcEvent::SendFailed(e.to_string()));
                    }
                }
                Some(SessionCommand::Disconnect) | None => {
                    handle.close().await;
                    break None;
                }
            },
            event = events.recv() => match event {
                Some(LinkSessionEvent::Packet { data, .. }) => {
                    let env = match Envelope::decode(&data) {
                        Ok(env) => env,
                        Err(e) => {
                            tracing::debug!("undecodable RRC packet from hub: {e}");
                            continue;
                        }
                    };
                    match env.t {
                        t::PING => {
                            let mut pong = Envelope::new(t::PONG, &own);
                            pong.body = env.body;
                            let _ = handle.send_packet(pong.encode()).await;
                        }
                        t::PONG => {
                            let key = env.body.as_ref().and_then(|b| b.as_bytes().cloned());
                            if let Some(sent) = key.and_then(|k| pings.remove(&k)) {
                                emit(RrcEvent::Pong { rtt_ms: sent.elapsed().as_millis() as u64 });
                            }
                        }
                        t::WELCOME => {
                            welcomed = true;
                            emit(RrcEvent::Welcome {
                                welcome: rrc::parse_welcome(env.body.as_ref()),
                                hub_identity: env.src,
                            });
                        }
                        t::RESOURCE_ENVELOPE => {
                            if let Some(announcement) = rrc::parse_resource_envelope(&env) {
                                expected.retain(|(_, at)| at.elapsed() < RESOURCE_EXPECTATION);
                                expected.push((announcement, Instant::now()));
                            }
                        }
                        _ => emit(RrcEvent::Envelope(env)),
                    }
                }
                Some(LinkSessionEvent::Closed { reason }) => break Some(format!("link closed ({reason:?})")),
                Some(_) => {}
                None => break Some("link closed".into()),
            },
            Some(offer) = resource_offers.recv() => {
                if offer.data_size() > MAX_RESOURCE_BYTES {
                    tracing::debug!("refusing {} byte resource from hub", offer.data_size());
                    let _ = offer.reject().await;
                    continue;
                }
                let tx = resource_tx.clone();
                tokio::spawn(async move {
                    if let Ok(transfer) = offer.accept().await
                        && let Ok(received) = transfer.concluded().await
                    {
                        let _ = tx.send(received.data);
                    }
                });
            }
            Some(data) = resource_rx.recv() => {
                // Match the Resource to its announcement by size, then check
                // the announced SHA-256 before trusting it.
                expected.retain(|(_, at)| at.elapsed() < RESOURCE_EXPECTATION);
                let found = expected.iter().position(|(a, _)| a.size == data.len());
                let announcement = found.map(|i| expected.remove(i).0);
                if let Some(sha) = announcement.as_ref().and_then(|a| a.sha256.as_ref())
                    && rns_crypto::sha::full_hash(&data).as_slice() != sha.as_slice()
                {
                    tracing::warn!("hub resource failed its SHA-256 check");
                    continue;
                }
                let (kind, room) = announcement
                    .map_or(("blob".to_string(), None), |a| (a.kind, a.room));
                if kind == "notice" || kind == "motd" {
                    emit(RrcEvent::ResourceText {
                        kind,
                        room,
                        text: String::from_utf8_lossy(&data).into_owned(),
                    });
                }
            }
            _ = hello_timer.tick(), if !welcomed => {
                if hello_attempts >= HELLO_ATTEMPTS {
                    handle.close().await;
                    break Some("no WELCOME from hub".into());
                }
                hello_attempts += 1;
                emit(RrcEvent::Status(format!("Sending HELLO (attempt {hello_attempts})")));
                let _ = handle.send_packet(hello(&own, nick.as_deref())).await;
            }
        }
    }
}
