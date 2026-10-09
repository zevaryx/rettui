//! What RRC hub sessions report: connection changes, room membership,
//! messages and notices.
//!
//! Hubs differ in more than the protocol says. What's handled here, beyond
//! rrcd (the reference hub):
//! - the Go hub (rrc-hub) sends the whole member list with every JOINED and
//!   PARTED, not just who came or went, and replays a room's recent
//!   messages to whoever joins;
//! - Ratspeak splits long replies (`/who`, a big room's member list) over
//!   several packets;
//! - rsRRCD adds members' full identities to `/who` replies;
//! - every hub but the one sending it as a Resource splits a long greeting
//!   over several notices.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::{ChatLine, HubStatus, LineKind, Replay, backoff};
use crate::app::App;
use crate::net::Hash;
use crate::rrc::session::RrcEvent;
use crate::rrc::{self, Envelope, t};

/// How long after WELCOME (and then after each part) room-less notices
/// are the hub's greeting.
const MOTD_GAP: Duration = Duration::from_secs(5);
/// How long the rest of a `/list` reply may take to come.
const LIST_PARTS: Duration = Duration::from_secs(60);
/// How long a quiet `/who` waits for its reply, and then for more parts.
const WHO_WAIT: Duration = Duration::from_secs(60);
const WHO_PARTS: Duration = Duration::from_secs(10);

impl App {
    pub fn on_rrc(&mut self, hash: Hash, event: RrcEvent) {
        let Some(index) = self.channels.hub_index(hash) else {
            return; // hub removed meanwhile
        };
        match event {
            RrcEvent::Status(text) => self.hub_mut(index).status = HubStatus::Connecting(text),
            RrcEvent::Welcome { welcome, hub_identity } => {
                let hub = self.hub_mut(index);
                hub.status = HubStatus::Connected;
                hub.attempts = 0;
                hub.hub_identity = Some(hub_identity);
                hub.limits = welcome.limits;
                hub.direct_notices = welcome.direct_notices;
                hub.motd_until = Some(Instant::now() + MOTD_GAP);
                hub.motd_parts = None;
                if let Some(name) = welcome.hub_name {
                    // Replace a placeholder name with the hub's own.
                    if hub.name.starts_with('<') {
                        hub.name = name.clone();
                    }
                    hub.hub_name = Some(name);
                }
                self.save_hubs();
                let hub = self.hub_mut(index);
                let label = hub.hub_name.clone().unwrap_or_else(|| hub.name.clone());
                self.record(index, "", ChatLine::new(LineKind::System, format!("Connected to {label}")));
                // Rejoin quietly and refresh the public room list.
                let rooms: Vec<String> = self.channels.hubs[index].rooms.iter().cloned().collect();
                for room in rooms {
                    self.join(index, &room, None, true);
                }
                self.hub_mut(index).silent_list += 1;
                let env = self.envelope(index, t::MSG).text("/list");
                self.send_env(index, &env);
            }
            RrcEvent::Envelope(env) => self.on_envelope(index, env),
            RrcEvent::ResourceText { kind, room, text } => {
                if kind == "motd" {
                    self.set_motd(index, text);
                } else if self.consume_notice(index, &text, None) != Some(true) {
                    let room = room.unwrap_or_else(|| self.reply_room(index));
                    self.record(index, &room, ChatLine::new(LineKind::Notice, text));
                }
            }
            RrcEvent::Pong { rtt_ms } => {
                let room = self.reply_room(index);
                self.record(index, &room, ChatLine::new(LineKind::System, format!("Pong from hub: {rtt_ms} ms")));
            }
            RrcEvent::SendFailed(e) => {
                let room = self.reply_room(index);
                self.record(index, &room, ChatLine::new(LineKind::Error, format!("Not sent: {e}")));
            }
            RrcEvent::Disconnected(reason) => {
                let hub = self.hub_mut(index);
                hub.members.clear();
                hub.pending_joins.clear();
                hub.silent_joins.clear();
                hub.pending_parts.clear();
                hub.silent_who.clear();
                hub.who_at.clear();
                hub.who_asked.clear();
                hub.silent_list = 0;
                hub.list_more = None;
                hub.replay.clear();
                hub.motd_until = None;
                let was_connected = hub.is_connected();
                let retry = !hub.manual && hub.auto_connect;
                hub.status = match &reason {
                    Some(e) if !was_connected => HubStatus::Failed(e.clone()),
                    _ => HubStatus::Disconnected,
                };
                let mut text = match &reason {
                    Some(e) => format!("Disconnected: {e}"),
                    None => "Disconnected".to_string(),
                };
                if retry {
                    hub.attempts += 1;
                    let wait = backoff(hub.attempts);
                    hub.reconnect_at = Some(Instant::now() + wait);
                    text.push_str(&format!(" (retrying in {}s)", wait.as_secs()));
                }
                self.record(index, "", ChatLine::new(LineKind::System, text));
            }
        }
    }

    fn on_envelope(&mut self, index: usize, env: Envelope) {
        let own = self.identity_hash.to_vec();
        let room = env.room.as_deref().map(rrc::normalize_room).filter(|r| !r.is_empty());
        match env.t {
            t::JOINED => {
                let Some(room) = room else { return };
                let hashes = env.body_hashes();
                let hub = self.hub_mut(index);
                let self_join = hub.pending_joins.remove(&room);
                let silent = hub.silent_joins.remove(&room);
                // With us in it, the body is everyone in the room now (the
                // Go hub); without, it's who joined (rrcd and most others),
                // or the rest of a long member list (Ratspeak).
                let everyone = hashes.contains(&own);
                if self_join || everyone {
                    hub.rooms.insert(room.clone());
                    hub.buffers.entry(room.clone()).or_default();
                } else if !hub.rooms.contains(&room) {
                    return; // a room we've left
                }
                let members = hub.members.entry(room.clone()).or_default();
                let mut joined: Vec<Vec<u8>> = Vec::new();
                if self_join {
                    *members = hashes.iter().cloned().collect();
                } else if everyone {
                    let now: BTreeSet<Vec<u8>> = hashes.iter().cloned().collect();
                    joined = now.difference(members).cloned().collect();
                    *members = now;
                } else {
                    // Someone already here (joining twice) isn't news.
                    let new: Vec<Vec<u8>> = hashes.iter().filter(|h| hub.add_member(&room, h)).cloned().collect();
                    if hashes.len() == 1 {
                        joined = new;
                    }
                }
                hub.members.entry(room.clone()).or_default().insert(own.clone());
                joined.retain(|h| *h != own);
                // Hubs name the one who joined.
                if let ([joiner], Some(nick)) = (joined.as_slice(), &env.nick) {
                    hub.nicks.insert(joiner.clone(), nick.clone());
                }
                if self_join {
                    if !silent {
                        self.record(index, &room, ChatLine::new(LineKind::System, format!("You joined #{room}")));
                    }
                    // Learn members' nicks without showing the reply.
                    self.quiet_who(index, &room);
                } else if hashes.is_empty()
                    && let Some(nick) = &env.nick
                {
                    // A hub that doesn't list members (rrcd, unless set
                    // to) still names them: ask quietly who it is.
                    self.record(index, &room, ChatLine::presence(format!("{nick} joined")));
                    let asked = self.channels.hubs[index].who_asked.get(&room).is_some_and(|at| at.elapsed() < WHO_WAIT);
                    if !asked {
                        self.quiet_who(index, &room);
                    }
                } else {
                    for joiner in joined {
                        let name = self.channels.hubs[index].name_of(&joiner);
                        self.record(index, &room, ChatLine::presence(format!("{name} joined")));
                    }
                }
            }
            t::PARTED => {
                let Some(room) = room else { return };
                let hashes = env.body_hashes();
                let hub = self.hub_mut(index);
                if hub.pending_parts.remove(&room) || !hub.rooms.contains(&room) {
                    return; // our own leaving (done already), or a room we've left
                }
                let members = hub.members.entry(room.clone()).or_default();
                let left: Vec<Vec<u8>> = if hashes.contains(&own) {
                    // Who is still here (the Go hub): the others have gone.
                    let now: BTreeSet<Vec<u8>> = hashes.iter().cloned().collect();
                    let gone = members.difference(&now).cloned().collect();
                    *members = now;
                    gone
                } else if hashes.is_empty()
                    && let Some(nick) = &env.nick
                {
                    // No identity, just the nick (rrcd, unless set to).
                    let named: Vec<Vec<u8>> = members.iter().filter(|m| hub.nicks.get(*m) == Some(nick)).cloned().collect();
                    if let [one] = named.as_slice() {
                        members.remove(one);
                    }
                    Vec::new()
                } else {
                    // Gone, whether known by all of their identity or its start.
                    members.retain(|m| !hashes.iter().any(|h| h.starts_with(m)));
                    if hashes.len() == 1 { hashes.clone() } else { Vec::new() }
                };
                if let ([parter], Some(nick)) = (left.as_slice(), &env.nick) {
                    hub.nicks.insert(parter.clone(), nick.clone());
                }
                if hashes.is_empty()
                    && let Some(nick) = &env.nick
                {
                    self.record(index, &room, ChatLine::presence(format!("{nick} left")));
                }
                for parter in left {
                    let name = self.channels.hubs[index].name_of(&parter);
                    self.record(index, &room, ChatLine::presence(format!("{name} left")));
                }
            }
            t::MSG | t::ACTION => {
                let is_own = env.src == own;
                let key = room.clone().unwrap_or_default();
                let id = hex::encode(&env.id);
                let hub = self.hub_mut(index);
                if is_own && let Some(pos) = hub.sent.iter().position(|id| *id == env.id) {
                    // Our own message came back: the hub delivered it, under
                    // the nick it gives us (which may not be the one we
                    // asked for: the Go hub keeps nicks unique).
                    hub.sent.remove(pos);
                    if let Some(nick) = &env.nick {
                        hub.nicks.insert(own.clone(), nick.clone());
                    }
                    if let Some(line) =
                        hub.buffers.get_mut(&key).and_then(|b| b.iter_mut().rev().find(|l| l.pending.as_ref() == Some(&env.id)))
                    {
                        line.pending = None;
                        if env.nick.is_some() {
                            line.nick = env.nick.clone();
                        }
                    }
                    return;
                }
                // Hubs that replay history send messages we may have.
                if hub.buffers.get(&key).is_some_and(|lines| lines.iter().any(|l| l.id.as_deref() == Some(id.as_str()))) {
                    return;
                }
                let replaying = room.as_ref().and_then(|r| hub.replay.get_mut(r));
                let replayed = replaying.is_some();
                let header = replaying.filter(|r| !r.shown).map(|r| {
                    r.shown = true;
                    r.header.clone()
                });
                if !replayed {
                    // Who's here now (a replayed sender may have left).
                    if let Some(nick) = &env.nick {
                        hub.nicks.insert(env.src.clone(), nick.clone());
                    }
                    if let Some(room) = &room {
                        hub.add_member(room, &env.src);
                    }
                }
                let Some(text) = env.body_text() else { return };
                if let Some(header) = header {
                    self.record(index, &key, ChatLine::new(LineKind::System, header));
                }
                let mut line = ChatLine::new(if env.t == t::ACTION { LineKind::Action } else { LineKind::Msg }, text);
                line.id = Some(id);
                line.src = Some(hex::encode(&env.src));
                line.nick = env.nick.clone();
                line.own = is_own;
                if replayed && env.ts > 0 && env.ts <= rrc::now_ms() {
                    // When it was said, not now.
                    line.ts = env.ts;
                }
                if !is_own {
                    line.mention_at = self.own_mentions(index, text);
                    line.mention = !line.mention_at.is_empty();
                }
                self.record_line(index, &key, line, !replayed);
            }
            t::NOTICE => {
                let Some(text) = env.body_text().map(str::to_string) else { return };
                let from_hub = self.channels.hubs[index].hub_identity.as_ref() == Some(&env.src);
                if env.dst.is_some() && !from_hub {
                    // A private notice from another user.
                    let hub = self.hub_mut(index);
                    if let Some(nick) = &env.nick {
                        hub.nicks.insert(env.src.clone(), nick.clone());
                    }
                    // It goes in the whisper conversation with them.
                    let mut line = ChatLine::new(LineKind::Private, text);
                    line.src = Some(hex::encode(&env.src));
                    line.nick = env.nick.clone();
                    self.record(index, &super::whisper_key(&env.src), line);
                    return;
                }
                if let Some(room) = &room
                    && self.replay_marker(index, room, &text)
                {
                    return;
                }
                let reply = self.consume_notice(index, &text, rrc::parse_user_list(&env));
                if reply == Some(true) {
                    return;
                }
                let hub = self.hub_mut(index);
                // The greeting, maybe in several notices (but not the Go
                // hub's "--- 2 mention(s) while you were away ---").
                let greeting = reply.is_none() && room.is_none() && !text.starts_with("--- ");
                if greeting && hub.motd_until.is_some_and(|until| Instant::now() < until) {
                    hub.motd_until = Some(Instant::now() + MOTD_GAP);
                    self.add_motd(index, text);
                    return;
                }
                let target = room.unwrap_or_else(|| self.reply_room(index));
                self.record(index, &target, ChatLine::new(LineKind::Notice, text));
            }
            t::ERROR => {
                let text = env.body_text().unwrap_or("(error)").to_string();
                let hub = self.hub_mut(index);
                let mut target = room.clone();
                match &room {
                    Some(room) if hub.pending_joins.remove(room) => {
                        // The join was refused: forget the room, and a key
                        // that didn't work.
                        hub.rooms.remove(room);
                        hub.silent_joins.remove(room);
                        if text == "bad key (+k)" {
                            hub.keys.remove(room);
                        }
                        self.save_hubs();
                    }
                    Some(room) if removed_from(&text).is_some_and(|r| r == *room) => {
                        // Kicked or banned: the hub has taken us out.
                        hub.rooms.remove(room);
                        hub.members.remove(room);
                        self.save_hubs();
                    }
                    // Not every hub names the room this is about.
                    None if text == "too many rooms" && hub.pending_joins.len() == 1 => {
                        let room = hub.pending_joins.drain().next().unwrap_or_default();
                        hub.rooms.remove(&room);
                        hub.silent_joins.remove(&room);
                        target = Some(room);
                        self.save_hubs();
                    }
                    _ => {}
                }
                let target = target.unwrap_or_else(|| self.reply_room(index));
                self.record(index, &target, ChatLine::new(LineKind::Error, text));
            }
            _ => {}
        }
    }

    /// Ask who is in `room`, without showing the reply.
    fn quiet_who(&mut self, index: usize, room: &str) {
        let now = Instant::now();
        let hub = self.hub_mut(index);
        hub.silent_who.insert(room.to_string(), now + WHO_WAIT);
        hub.who_asked.insert(room.to_string(), now);
        let env = self.envelope(index, t::MSG).room(room).text(&format!("/who {room}"));
        self.send_env(index, &env);
    }

    /// Mentions of us in `text`: by our nick, or the one the hub gives us
    /// if that's another.
    fn own_mentions(&self, index: usize, text: &str) -> Vec<(usize, usize)> {
        let mut nicks: Vec<String> = self.effective_nick(index).into_iter().collect();
        if let Some(given) = self.channels.hubs[index].nicks.get(self.identity_hash.as_slice())
            && !nicks.iter().any(|n| n.eq_ignore_ascii_case(given))
        {
            nicks.push(given.clone());
        }
        let names: Vec<&str> = nicks.iter().map(String::as_str).collect();
        rrc::user_mentions(text, &names).into_iter().map(|(start, end, _)| (start, end)).collect()
    }

    /// The notices around history the Go hub replays into a room:
    /// `--- 5 messages from earlier ---` (or `from the last 2 hours`) and
    /// `--- end of history ---`. They're shown only around messages we
    /// hadn't seen. Returns whether `text` was one.
    fn replay_marker(&mut self, index: usize, room: &str, text: &str) -> bool {
        let hub = self.hub_mut(index);
        if text == "--- end of history ---" {
            if hub.replay.remove(room).is_some_and(|replay| replay.shown) {
                self.record(index, room, ChatLine::new(LineKind::System, text));
            }
            return true;
        }
        let mut words = text.strip_prefix("--- ").and_then(|t| t.strip_suffix(" ---")).unwrap_or_default().split(' ');
        let opens = words.next().is_some_and(|n| n.parse::<u32>().is_ok())
            && matches!(words.next(), Some("message" | "messages"))
            && words.next() == Some("from");
        if opens {
            hub.replay.insert(room.to_string(), Replay { header: text.to_string(), shown: false });
        }
        opens
    }

    /// The hub's message of the day goes to the hub view, whole (the hub
    /// resends it whenever it welcomes us again: shown once per text).
    fn set_motd(&mut self, index: usize, text: String) {
        let hub = self.hub_mut(index);
        if hub.motd.as_deref() == Some(text.as_str()) {
            return;
        }
        hub.motd = Some(text.clone());
        self.record(index, "", ChatLine::new(LineKind::Notice, text));
    }

    /// One more notice of a greeting sent in several. Its lines are shown
    /// unless they are the greeting we had.
    fn add_motd(&mut self, index: usize, part: String) {
        let hub = self.hub_mut(index);
        let (text, before) = match hub.motd_parts.take() {
            Some((so_far, before)) => (format!("{so_far}\n{part}"), before),
            None => (part.clone(), hub.motd.clone()),
        };
        let known = before.as_deref().is_some_and(|old| old == text || old.starts_with(&format!("{text}\n")));
        hub.motd_parts = Some((text.clone(), before));
        hub.motd = Some(text);
        if !known {
            self.record(index, "", ChatLine::new(LineKind::Notice, part));
        }
    }

    /// Interpret hub replies (/list, /who, room info and topics): `None`
    /// if `text` isn't one, else whether to keep it quiet (we asked for it
    /// silently, or show it another way). `users` is a structured member
    /// list sent with a `/who` reply.
    fn consume_notice(&mut self, index: usize, text: &str, users: Option<Vec<rrc::WhoEntry>>) -> Option<bool> {
        let now = Instant::now();
        let hub = self.hub_mut(index);
        if let Some(rooms) = rrc::parse_room_list(text) {
            hub.available = Some(rooms);
            let silent = hub.silent_list > 0;
            if silent {
                hub.silent_list -= 1;
            }
            hub.list_more = Some((now + LIST_PARTS, silent));
            return Some(silent);
        }
        // The rest of a list sent a line or so per notice.
        if let Some((until, silent)) = hub.list_more
            && now < until
            && let Some(rooms) = rrc::parse_room_list_more(text)
        {
            let available = hub.available.get_or_insert_with(Vec::new);
            available.extend(rooms.into_iter().filter(|(name, _)| !available.iter().any(|(n, _)| n == name)).collect::<Vec<_>>());
            hub.list_more = Some((now + LIST_PARTS, silent));
            return Some(silent);
        }
        if let Some((room, entries)) = rrc::parse_who(text) {
            // A reply's first part (more may follow soon, from Ratspeak)
            // is a fresh list: whoever was known by a prefix only may
            // have gone.
            let first = hub.who_at.insert(room.clone(), now).is_none_or(|at| now.duration_since(at) > WHO_PARTS);
            hub.who_asked.remove(&room);
            let members = hub.members.entry(room.clone()).or_default();
            if first {
                members.retain(|m| m.len() == 16);
            }
            // Full identities (rsRRCD) or, for people with nicks, only
            // the start of one: kept as that until they're seen.
            let entries = users.unwrap_or(entries);
            for entry in entries {
                let Ok(hash) = hex::decode(&entry.hash_hex) else { continue };
                let id = members.iter().find(|m| m.len() == 16 && m.starts_with(&hash)).cloned().unwrap_or(hash);
                members.insert(id.clone());
                if let Some(nick) = entry.nick {
                    hub.nicks.insert(id, nick);
                }
            }
            // A reply too long for one packet may come in parts, each
            // starting "members in" again (Ratspeak): all quiet.
            let silent = hub.silent_who.get(&room).is_some_and(|until| now < *until);
            if silent {
                hub.silent_who.insert(room, now + WHO_PARTS);
            } else {
                hub.silent_who.remove(&room);
            }
            return Some(silent);
        }
        if let Some(info) = rrc::parse_room_info(text) {
            match &info.topic {
                Some(topic) => hub.topics.insert(info.room.clone(), topic.clone()),
                None => hub.topics.remove(&info.room),
            };
            if !hub.quiet_info.remove(&info.room) {
                // Shown as a short summary instead of the raw notice.
                let topic = info.topic.map_or("no topic".to_string(), |t| format!("topic: {t}"));
                let registered = if info.registered { "registered" } else { "unregistered" };
                let modes = info.mode.len() > 1 && info.mode != "(none)";
                let mode = if modes { format!(", mode {}", info.mode) } else { String::new() };
                let summary = format!("#{} ({registered}{mode}), {topic}", info.room);
                let room = info.room.clone();
                self.record(index, &room, ChatLine::new(LineKind::System, summary));
            }
            return Some(true);
        }
        // A topic asked for or changed: still shown, as said.
        let (room, topic) = rrc::parse_topic(text)?;
        match topic {
            Some(topic) => hub.topics.insert(room, topic),
            None => hub.topics.remove(&room),
        };
        Some(false)
    }
}

/// The room an ERROR says we were taken out of: `kicked from <room>` or
/// `banned from <room>` (every hub words it so).
fn removed_from(text: &str) -> Option<String> {
    let room = text.strip_prefix("kicked from ").or_else(|| text.strip_prefix("banned from "))?;
    Some(rrc::normalize_room(room)).filter(|r| !r.is_empty())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ciborium::value::{Integer, Value};
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::net::NetCommand;
    use crate::rrc::session::SessionCommand;

    const HUB: Hash = [0xcd; 16];
    const HUB_ID: [u8; 16] = [0xee; 16];
    // The test app's identity is all zeros.
    const OWN: [u8; 16] = [0; 16];
    const AMY: [u8; 16] = [0xa1; 16];
    const BOB: [u8; 16] = [0xb2; 16];
    const CAT: [u8; 16] = [0xc3; 16];

    struct Hubs {
        app: App,
        net: UnboundedReceiver<NetCommand>,
        dir: PathBuf,
    }

    impl Hubs {
        /// An app connected to one hub.
        fn connected(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rettui-rrc-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let (mut app, net) = crate::app::test_app_with_net(&dir, crate::config::Settings::default(), crate::store::Store::default());
            app.add_hub(HUB, "rrc.hub", None);
            let mut hubs = Self { app, net, dir };
            hubs.welcome();
            hubs
        }

        fn welcome(&mut self) {
            let welcome = rrc::Welcome { direct_notices: true, ..Default::default() };
            self.app.on_rrc(HUB, RrcEvent::Welcome { welcome, hub_identity: HUB_ID.to_vec() });
        }

        fn hub(&self) -> &super::super::Hub {
            &self.app.channels.hubs[0]
        }

        fn deliver(&mut self, env: Envelope) {
            self.app.on_rrc(HUB, RrcEvent::Envelope(env));
        }

        fn notice(&mut self, room: Option<&str>, text: &str) {
            let mut env = Envelope::new(t::NOTICE, &HUB_ID).text(text);
            env.room = room.map(str::to_string);
            self.deliver(env);
        }

        /// JOINED or PARTED, with `hashes` as the body (`None`: no body).
        fn membership(&mut self, kind: u64, room: &str, hashes: Option<&[[u8; 16]]>, nick: Option<&str>) {
            let mut env = Envelope::new(kind, &HUB_ID).room(room).nick(nick);
            env.body = hashes.map(|hashes| Value::Array(hashes.iter().map(|h| Value::Bytes(h.to_vec())).collect()));
            self.deliver(env);
        }

        fn said(&mut self, room: &str, from: [u8; 16], nick: &str, text: &str) -> Envelope {
            let env = Envelope::new(t::MSG, &from).room(room).text(text).nick(Some(nick));
            self.deliver(env.clone());
            env
        }

        fn type_in(&mut self, room: &str, text: &str) {
            self.app.channel_submit(0, room, text);
        }

        /// What was sent to the hub since last asked.
        fn sent(&mut self) -> Vec<Envelope> {
            let mut sent = Vec::new();
            while let Ok(command) = self.net.try_recv() {
                if let NetCommand::Rrc { command: SessionCommand::Send(bytes), .. } = command {
                    sent.push(Envelope::decode(&bytes).unwrap());
                }
            }
            sent
        }

        fn lines(&self, room: &str) -> Vec<String> {
            self.hub().buffers.get(room).map(|b| b.iter().map(|l| l.text.clone()).collect()).unwrap_or_default()
        }

        /// Join `room` (by typing /join), with `hashes` as the hub's reply.
        fn join(&mut self, room: &str, hashes: &[[u8; 16]]) {
            self.type_in("", &format!("/join {room}"));
            self.membership(t::JOINED, room, Some(hashes), None);
        }
    }

    impl Drop for Hubs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn who_and_names_replies_from_every_hub() {
        let mut hubs = Hubs::connected("who");
        hubs.join("lobby", &[OWN, AMY, BOB, CAT]);
        // Joining asks the hub who's there, quietly.
        let asked = hubs.sent().into_iter().filter_map(|e| e.body_text().map(str::to_string)).collect::<Vec<_>>();
        assert!(asked.contains(&"/who lobby".to_string()));
        // Ratspeak splits a long reply, the Go hub marks people away.
        let amy = &hex::encode(AMY)[..12];
        let bob = &hex::encode(BOB)[..12];
        let cat = &hex::encode(CAT)[..12];
        hubs.notice(None, &format!("members in lobby: amy ({amy}) [away], bob ({bob})"));
        hubs.notice(None, &format!("members in lobby: cat ({cat}), (unidentified)"));
        assert_eq!(hubs.lines("lobby"), ["You joined #lobby"]);
        assert_eq!(hubs.lines(""), ["Connected to <cdcdcdcdcdcd>"]);
        let names: Vec<String> = [AMY, BOB, CAT].iter().map(|h| hubs.hub().name_of(h)).collect();
        assert_eq!(names, ["amy", "bob", "cat"]);

        // Asked for by typing, the reply shows (in the room it was typed in).
        hubs.type_in("lobby", "/names");
        let sent = hubs.sent();
        assert_eq!((sent[0].body_text(), sent[0].room.as_deref()), (Some("/names"), Some("lobby")));
        hubs.notice(Some("lobby"), &format!("members in lobby: amy ({amy})"));
        assert_eq!(hubs.lines("lobby").last().unwrap(), &format!("members in lobby: amy ({amy})"));

        // rsRRCD sends full identities too: someone we hadn't seen joins
        // the members with their nick.
        let dan = [0xd4; 16];
        let mut env = Envelope::new(t::NOTICE, &HUB_ID).text(&format!("members in lobby: dan ({})", &hex::encode(dan)[..12]));
        let user = |hash: [u8; 16], nick: &str| {
            Value::Map(vec![
                (Value::Integer(Integer::from(0)), Value::Text(hex::encode(hash))),
                (Value::Integer(Integer::from(1)), Value::Text(nick.into())),
            ])
        };
        env.user_list = Some(Value::Array(vec![user(dan, "dan")]));
        hubs.deliver(env);
        assert!(hubs.hub().members["lobby"].contains(dan.as_slice()));
        assert_eq!(hubs.hub().name_of(&dan), "dan");
    }

    #[test]
    fn members_on_a_hub_without_member_lists() {
        // rrcd's default: no identities with JOINED and PARTED, and only
        // the start of one for nicked people in /who.
        let mut hubs = Hubs::connected("prefixes");
        hubs.type_in("", "/join lobby");
        hubs.membership(t::JOINED, "lobby", None, None);
        let amy = &hex::encode(AMY)[..12];
        hubs.notice(None, &format!("members in lobby: amy ({amy}), {}", hex::encode(BOB)));
        let names = |hubs: &Hubs| hubs.hub().members_of("lobby").into_iter().map(|(name, id)| (name, id.len())).collect::<Vec<_>>();
        let expected = [("000000000000", 16), ("amy", 6), ("b2b2b2b2b2b2", 16)].map(|(name, len)| (name.to_string(), len));
        assert_eq!(names(&hubs), expected);
        // Partly known, she can be mentioned but not messaged yet.
        assert!(hubs.app.find_rrc_user(0, "amy").is_err());
        // Once she speaks, she's known in full.
        hubs.said("lobby", AMY, "amy", "hi");
        assert!(names(&hubs).contains(&("amy".to_string(), 16)) && !names(&hubs).iter().any(|(_, len)| *len == 6));
        assert_eq!(hubs.app.find_rrc_user(0, "amy"), Ok(AMY.to_vec()));
        // A newcomer is named, and asked about quietly.
        hubs.sent();
        hubs.membership(t::JOINED, "lobby", None, Some("cat"));
        assert_eq!(hubs.sent().iter().filter_map(|e| e.body_text()).collect::<Vec<_>>(), ["/who lobby"]);
        hubs.notice(None, &format!("members in lobby: amy ({amy}), cat ({})", &hex::encode(CAT)[..12]));
        // Leaving, by nick alone.
        hubs.membership(t::PARTED, "lobby", None, Some("amy"));
        assert!(!names(&hubs).iter().any(|(name, _)| name == "amy"));
        assert_eq!(hubs.lines("lobby"), ["You joined #lobby", "hi", "cat joined", "amy left"]);
    }

    #[test]
    fn list_replies_in_parts() {
        let mut hubs = Hubs::connected("list");
        // The quiet /list sent on connecting, answered a line at a time.
        hubs.notice(None, "Registered public rooms:");
        hubs.notice(None, "  general - Chat about anything");
        hubs.notice(None, "  mesh");
        // ...or with Ratspeak's count of what didn't fit.
        let available = |hubs: &Hubs| hubs.hub().available.clone().unwrap().into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(available(&hubs), ["general", "mesh"]);
        assert_eq!(hubs.lines("").len(), 1);
        // Asked for by typing, it shows; it's not the greeting either.
        hubs.type_in("", "/list");
        hubs.notice(None, "Registered public rooms:\n  radio\n  (+4 more)");
        assert_eq!(available(&hubs), ["radio"]);
        assert_eq!(hubs.hub().motd, None);
        assert_eq!(hubs.lines("").len(), 2);
    }

    #[test]
    fn joins_and_leaves_from_every_kind_of_hub() {
        let mut hubs = Hubs::connected("members");
        hubs.join("lobby", &[OWN, AMY]);
        // rrcd: who joined, with their nick.
        hubs.membership(t::JOINED, "lobby", Some(&[BOB]), Some("bob"));
        // The Go hub: everyone there now.
        hubs.membership(t::JOINED, "lobby", Some(&[OWN, AMY, BOB, CAT]), None);
        hubs.membership(t::PARTED, "lobby", Some(&[OWN, CAT]), None);
        // A hub without member lists still names them.
        hubs.membership(t::JOINED, "lobby", None, Some("zed"));
        // rrcd again: who left.
        hubs.membership(t::PARTED, "lobby", Some(&[CAT]), Some("cat"));
        // Someone here already isn't news.
        hubs.membership(t::JOINED, "lobby", Some(&[OWN]), None);
        assert_eq!(
            hubs.lines("lobby"),
            ["You joined #lobby", "bob joined", "c3c3c3c3c3c3 joined", "a1a1a1a1a1a1 left", "bob left", "zed joined", "cat left"]
        );
        assert_eq!(hubs.hub().members["lobby"], BTreeSet::from([OWN.to_vec()]));

        // Ratspeak sends a big room's member list in parts.
        hubs.join("big", &[OWN, AMY]);
        hubs.membership(t::JOINED, "big", Some(&[BOB, CAT]), None);
        assert_eq!(hubs.lines("big"), ["You joined #big"]);
        assert_eq!(hubs.hub().members["big"].len(), 4);
        // Our own leaving, as each hub confirms it, adds nothing.
        hubs.type_in("big", "/part");
        hubs.membership(t::PARTED, "big", Some(&[AMY, BOB, CAT]), None);
        assert_eq!(hubs.lines("big"), ["You joined #big", "You left #big"]);
        // Nor does news of a room we've left.
        hubs.membership(t::JOINED, "big", Some(&[AMY]), Some("amy"));
        assert!(!hubs.hub().rooms.contains("big"));
    }

    #[test]
    fn a_new_nick_goes_with_messages_and_keys_with_joins() {
        let mut hubs = Hubs::connected("nick");
        hubs.join("lobby", &[OWN]);
        hubs.sent();
        // No new HELLO (hubs would take us out of every room): the nick
        // goes with what we send.
        hubs.type_in("lobby", "/nick zev");
        assert!(hubs.sent().is_empty());
        hubs.type_in("lobby", "hello");
        let sent = hubs.sent();
        assert_eq!((sent[0].nick.as_deref(), sent[0].body_text()), (Some("zev"), Some("hello")));
        assert!(hubs.hub().rooms.contains("lobby"));

        // A keyed room's key is kept to rejoin it after reconnecting...
        hubs.type_in("", "/join vault s3cret");
        assert_eq!(hubs.sent()[0].body_text(), Some("s3cret"));
        hubs.membership(t::JOINED, "vault", Some(&[OWN]), None);
        hubs.app.on_rrc(HUB, RrcEvent::Disconnected(Some("link closed".into())));
        hubs.welcome();
        let joins: Vec<Envelope> = hubs.sent().into_iter().filter(|e| e.t == t::JOIN).collect();
        let vault = joins.iter().find(|e| e.room.as_deref() == Some("vault")).unwrap();
        assert_eq!(vault.body_text(), Some("s3cret"));
        assert_eq!(hubs.app.store.rrc_hubs[0].room_keys["vault"], "s3cret");
        // ...until it stops working.
        let mut refused = Envelope::new(t::ERROR, &HUB_ID).room("vault").text("bad key (+k)");
        refused.room = Some("vault".into());
        hubs.deliver(refused);
        assert!(!hubs.hub().rooms.contains("vault") && !hubs.hub().keys.contains_key("vault"));
    }

    #[test]
    fn kicks_bans_refusals_and_topics() {
        let mut hubs = Hubs::connected("kick");
        hubs.join("lobby", &[OWN, AMY]);
        hubs.join("ops", &[OWN]);
        hubs.deliver(Envelope::new(t::ERROR, &HUB_ID).room("lobby").text("kicked from lobby"));
        hubs.deliver(Envelope::new(t::ERROR, &HUB_ID).room("ops").text("banned from ops"));
        // Out of both rooms (not rejoined later), their messages kept.
        assert!(hubs.hub().rooms.is_empty());
        assert_eq!(hubs.lines("lobby").last().unwrap(), "kicked from lobby");
        assert!(hubs.app.store.rrc_hubs[0].rooms.is_empty());

        // rrcd doesn't say which join was one too many.
        hubs.type_in("", "/join more");
        hubs.deliver(Envelope::new(t::ERROR, &HUB_ID).text("too many rooms"));
        assert!(!hubs.hub().rooms.contains("more"));
        assert_eq!(hubs.lines("more").last().unwrap(), "too many rooms");

        // Topics, as set and as asked for.
        hubs.join("lobby", &[OWN]);
        hubs.notice(Some("lobby"), "room lobby: unregistered; mode=(none); topic=(none)");
        hubs.notice(Some("lobby"), "topic for lobby is now: Antennas");
        assert_eq!(hubs.hub().topics["lobby"], "Antennas");
        hubs.notice(None, "topic for lobby: (none)");
        assert!(!hubs.hub().topics.contains_key("lobby"));
        hubs.app.channels.selected = Some(super::super::Target { hub: HUB, room: Some("lobby".into()) });
        let lines = hubs.lines("lobby");
        assert!(lines.contains(&"topic for lobby is now: Antennas".to_string()));
        // A room without modes says so plainly.
        assert!(!lines.iter().any(|l| l.contains("(none)") && l.starts_with('#')), "{lines:?}");
    }

    #[test]
    fn a_greeting_in_several_notices() {
        let mut hubs = Hubs::connected("motd");
        hubs.notice(None, "Welcome to the hilltop hub");
        hubs.notice(None, "Be nice");
        // Not the Go hub's mentions while we were away.
        hubs.notice(None, "--- 1 mention(s) while you were away ---");
        assert_eq!(hubs.hub().motd.as_deref(), Some("Welcome to the hilltop hub\nBe nice"));
        assert_eq!(
            hubs.lines(""),
            ["Connected to <cdcdcdcdcdcd>", "Welcome to the hilltop hub", "Be nice", "--- 1 mention(s) while you were away ---"]
        );
        // Welcomed again (reconnecting), the same greeting isn't shown again.
        hubs.app.on_rrc(HUB, RrcEvent::Disconnected(None));
        hubs.welcome();
        hubs.notice(None, "Welcome to the hilltop hub");
        hubs.notice(None, "Be nice");
        assert_eq!(hubs.lines("").iter().filter(|l| *l == "Be nice").count(), 1);
        assert_eq!(hubs.hub().motd.as_deref(), Some("Welcome to the hilltop hub\nBe nice"));
    }

    #[test]
    fn replayed_history_once_with_its_times() {
        let mut hubs = Hubs::connected("replay");
        hubs.join("lobby", &[OWN, AMY]);
        let seen = hubs.said("lobby", AMY, "amy", "the repeater is up");
        // Rejoining, the Go hub replays what was said: once is enough.
        let mut earlier = Envelope::new(t::MSG, &BOB).room("lobby").text("and the antenna?").nick(Some("bob"));
        earlier.ts = rrc::now_ms() - 3_600_000;
        hubs.notice(Some("lobby"), "--- 2 messages from earlier ---");
        hubs.deliver(seen.clone());
        hubs.deliver(earlier.clone());
        hubs.notice(Some("lobby"), "--- end of history ---");
        assert_eq!(
            hubs.lines("lobby"),
            ["You joined #lobby", "the repeater is up", "--- 2 messages from earlier ---", "and the antenna?", "--- end of history ---"]
        );
        let line = hubs.hub().buffers["lobby"].iter().find(|l| l.text == "and the antenna?").unwrap();
        assert_eq!(line.ts, earlier.ts);
        // Nothing new: nothing shown, not even the brackets.
        hubs.notice(Some("lobby"), "--- 2 messages from the last 2 hours ---");
        hubs.deliver(seen);
        hubs.deliver(earlier);
        hubs.notice(Some("lobby"), "--- end of history ---");
        assert_eq!(hubs.lines("lobby").len(), 5);
        // Ids are kept with the history.
        let saved = hubs.app.channels.hubs[0].history_snapshot();
        assert!(saved["lobby"].iter().all(|l| l.id.is_some()));

        // Our own message, as the hub gives it back: the Go hub may give
        // us another nick than we asked for, and that's a mention too.
        hubs.type_in("lobby", "/nick zev");
        hubs.type_in("lobby", "hi");
        let mine = hubs.sent().pop().unwrap();
        hubs.deliver(Envelope { nick: Some("zev1".into()), ..mine.clone() });
        // Replayed later, ours is known too.
        hubs.deliver(Envelope { nick: Some("zev1".into()), ..mine });
        assert_eq!(hubs.lines("lobby").iter().filter(|l| *l == "hi").count(), 1);
        let line = hubs.hub().buffers["lobby"].last().unwrap().clone();
        assert!(line.own && line.pending.is_none());
        assert_eq!(line.nick.as_deref(), Some("zev1"));
        hubs.said("lobby", AMY, "amy", "@zev1 hello");
        assert!(hubs.hub().buffers["lobby"].last().unwrap().mention);
        hubs.said("lobby", AMY, "amy", "@zev hello");
        assert!(hubs.hub().buffers["lobby"].last().unwrap().mention);
    }

    #[test]
    fn hub_commands_from_a_whisper_name_no_room() {
        let mut hubs = Hubs::connected("whisper");
        hubs.sent();
        let key = super::super::whisper_key(&AMY);
        hubs.app.channels.hubs[0].buffers.insert(key.clone(), Vec::new());
        hubs.type_in(&key, "/history");
        hubs.type_in(&key, "/quote /help");
        let sent = hubs.sent();
        assert_eq!(
            sent.iter().map(|e| (e.body_text().unwrap(), e.room.clone())).collect::<Vec<_>>(),
            [("/history", None), ("/help", None)]
        );
    }
}
