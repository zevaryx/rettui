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

use crate::store::Trust;
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
                // The hub hosted here, stopped: tried again once it's back
                // (see connect_own_hub), not before.
                let waiting = self.own_hub_waiting(hash);
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
                let retry = !hub.manual && hub.auto_connect && !waiting;
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

    /// Whether the identity `src` (an RRC sender) is someone blocked: the block list is of
    /// LXMF addresses, which come from identities.
    fn identity_is_blocked(&self, src: &[u8]) -> bool {
        super::users::lxmf_address(src).is_some_and(|address| self.store.contact(&hex::encode(address)).trust == Trust::Blocked)
    }

    fn on_envelope(&mut self, index: usize, env: Envelope) {
        self.on_envelope_unbounded(index, env);
        let own = self.identity_hash.to_vec();
        if let Some(hub) = self.channels.hubs.get_mut(index) {
            hub.limit_tables(&own);
        }
    }

    fn on_envelope_unbounded(&mut self, index: usize, mut env: Envelope) {
        // A hub chooses every nick it shows us: cleaned as announced names are
        // (no control, invisible or direction characters, a bounded length),
        // as the hub hosted here cleans the nicks it accepts. A nick of
        // nothing else is none.
        // And without a tag of the kind added to a nick that collides with
        // another (see `Hub::shown`), so one can't be forged.
        env.nick = env.nick.take().and_then(|nick| super::incoming_nick(&nick));
        let own = self.identity_hash.to_vec();
        let mine = self.effective_nick(index);
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
                    hub.learn_nick(&own, mine.as_deref(), joiner, nick.clone());
                }
                // Who is here changed: what is told apart, for them and their look groups.
                let changed: Vec<Vec<u8>> = joined.iter().take(super::MAX_PRESENCE_IDS).cloned().collect();
                hub.members_changed(&own, mine.as_deref(), &changed);
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
                    let nick = self.channels.hubs[index].shown(&own, mine.as_deref(), &[], nick);
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
                    let named: Vec<Vec<u8>> =
                        members.iter().filter(|m| hub.raw_nicks.get(*m).is_some_and(|said| said.nick == *nick)).cloned().collect();
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
                    hub.learn_nick(&own, mine.as_deref(), parter, nick.clone());
                }
                let changed: Vec<Vec<u8>> = left.iter().take(super::MAX_PRESENCE_IDS).cloned().collect();
                hub.members_changed(&own, mine.as_deref(), &changed);
                if hashes.is_empty()
                    && let Some(nick) = &env.nick
                {
                    let nick = self.channels.hubs[index].shown(&own, mine.as_deref(), &[], nick);
                    self.record(index, &room, ChatLine::presence(format!("{nick} left")));
                }
                for parter in left {
                    let name = self.channels.hubs[index].name_of(&parter);
                    self.record(index, &room, ChatLine::presence(format!("{name} left")));
                }
            }
            t::MSG | t::ACTION => {
                let is_own = env.src == own;
                // Someone blocked is blocked in rooms and whispers too: nothing of theirs is shown.
                if !is_own && self.identity_is_blocked(&env.src) {
                    return;
                }
                // Only rooms we're in (or were, and still have): a hub could
                // otherwise add any number of rooms to the list, and to the
                // history saved.
                if let Some(room) = &room
                    && !self.channels.hubs[index].has_room(room)
                {
                    tracing::debug!("ignored a message for #{room}, a room not joined");
                    return;
                }
                let key = room.clone().unwrap_or_default();
                let id = hex::encode(&env.id);
                let hub = self.hub_mut(index);
                if is_own && let Some(pos) = hub.sent.iter().position(|id| *id == env.id) {
                    // Our own message came back: the hub delivered it, under
                    // the nick it gives us (which may not be the one we
                    // asked for: the Go hub keeps nicks unique).
                    hub.sent.remove(pos);
                    if let Some(nick) = &env.nick {
                        hub.learn_nick(&own, mine.as_deref(), &own, nick.clone());
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
                    // Who's here now (a replayed sender may have left): in the
                    // room before the nick is told apart from the others'.
                    if let Some(room) = &room {
                        hub.add_member(room, &env.src);
                    }
                    if let Some(nick) = &env.nick {
                        hub.learn_nick(&own, mine.as_deref(), &env.src, nick.clone());
                        hub.mark_spoken(&env.src);
                    }
                }
                let Some(text) = env.body_text() else { return };
                if let Some(header) = header {
                    self.record(index, &key, ChatLine::new(LineKind::System, header));
                }
                let mut line = ChatLine::new(if env.t == t::ACTION { LineKind::Action } else { LineKind::Msg }, text);
                line.id = Some(id);
                line.src = Some(hex::encode(&env.src));
                line.nick = env.nick.as_deref().map(|nick| self.channels.hubs[index].shown(&own, mine.as_deref(), &env.src, nick));
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
                        hub.learn_nick(&own, mine.as_deref(), &env.src, nick.clone());
                        hub.mark_spoken(&env.src);
                    }
                    let nick = env.nick.as_deref().map(|nick| self.channels.hubs[index].shown(&own, mine.as_deref(), &env.src, nick));
                    // It goes in the whisper conversation with them.
                    let mut line = ChatLine::new(LineKind::Private, text);
                    line.src = Some(hex::encode(&env.src));
                    line.nick = nick;
                    let key = super::whisper_key(&env.src);
                    let shown = self.channels.active().filter(|(i, _)| *i == index).map(|(_, room)| room);
                    self.hub_mut(index).make_room_for_whisper(&key, shown.as_deref());
                    self.record(index, &key, line);
                    return;
                }
                if !from_hub {
                    // Another member's notice to a room: only the hub's set
                    // the topic, lists or history, so it's said, by them
                    // (shown as the hub's, it could pass for it).
                    let mut line = ChatLine::new(LineKind::Msg, text);
                    line.src = Some(hex::encode(&env.src));
                    let hub = &self.channels.hubs[index];
                    line.nick = match &env.nick {
                        Some(nick) => Some(hub.shown(&own, mine.as_deref(), &env.src, nick)),
                        None => hub.nicks.get(&env.src).cloned(),
                    };
                    let target = self.room_or_reply(index, room);
                    self.record(index, &target, line);
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
                let target = self.room_or_reply(index, room);
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
                let target = self.room_or_reply(index, target);
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
    /// unless they are the greeting we had. When it turns out to be another
    /// one, the lines held back as the old greeting's start are shown with
    /// the first that differs, so the new one shows whole.
    fn add_motd(&mut self, index: usize, part: String) {
        let hub = self.hub_mut(index);
        let (text, before, shown) = match hub.motd_parts.take() {
            Some((so_far, before, shown)) => (format!("{so_far}\n{part}"), before, shown),
            None => (part.clone(), hub.motd.clone(), false),
        };
        let known = before.as_deref().is_some_and(|old| old == text || old.starts_with(&format!("{text}\n")));
        // Shown so far: nothing, while it's the greeting we had.
        let show = match (known, shown) {
            (true, _) => None,
            (false, true) => Some(part),
            (false, false) => Some(text.clone()),
        };
        hub.motd_parts = Some((text.clone(), before, shown || !known));
        hub.motd = Some(text);
        if let Some(show) = show {
            self.record(index, "", ChatLine::new(LineKind::Notice, show));
        }
    }

    /// Interpret hub replies (/list, /who, room info and topics): `None`
    /// if `text` isn't one, else whether to keep it quiet (we asked for it
    /// silently, or show it another way). `users` is a structured member
    /// list sent with a `/who` reply.
    fn consume_notice(&mut self, index: usize, text: &str, users: Option<Vec<rrc::WhoEntry>>) -> Option<bool> {
        let now = Instant::now();
        let (mine, own) = (self.effective_nick(index), self.identity_hash.to_vec());
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
                let members = hub.members.entry(room.clone()).or_default();
                let id = members.iter().find(|m| m.len() == 16 && m.starts_with(&hash)).cloned().unwrap_or(hash);
                members.insert(id.clone());
                if let Some(nick) = entry.nick.and_then(|nick| super::incoming_nick(&nick)) {
                    hub.learn_nick(&own, mine.as_deref(), &id, nick);
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
    fn nicks_a_hub_sends_are_cleaned_before_they_are_kept_or_shown() {
        let mut hubs = Hubs::connected("hostile-nicks");
        hubs.join("lobby", &[OWN, AMY, BOB, CAT]);
        // Zero-width, direction override, terminal escapes, blank fillers,
        // a fullwidth form, and one far too long.
        let long = "W".repeat(240);
        for (from, sent, shown) in [
            (AMY, "ca\u{200b}rol", Some("carol")),
            (BOB, "evil\u{202e}gnp.exe", Some("evilgnp.exe")),
            (CAT, "x\u{1b}[31mred\u{7}", Some("x[31mred")),
            ([0xd1; 16], "\u{3164}\u{3164}", None),
            ([0xd2; 16], "\u{ff21}\u{ff2d}\u{ff39}", Some("AMY")),
            ([0xd3; 16], long.as_str(), Some(&long[..crate::names::MAX_CHARS])),
        ] {
            let env = hubs.said("lobby", from, sent, "hi");
            assert_eq!(env.nick.as_deref(), Some(sent), "the hub sent it as it is");
            let line = hubs.hub().buffers["lobby"].last().unwrap();
            assert_eq!(line.nick.as_deref(), shown, "{sent:?}");
        }
        // Joins and parts name them the same way.
        hubs.membership(t::JOINED, "lobby", Some(&[[0xd4; 16]]), Some("ne\u{200b}w\u{202e}"));
        assert!(hubs.lines("lobby").iter().any(|l| l == "new joined"), "{:?}", hubs.lines("lobby"));
        // So do the members of a /who reply, with full identities too.
        let dan = [0xd5; 16];
        let mut env = Envelope::new(t::NOTICE, &HUB_ID).text(&format!("members in lobby: da\u{202e}n ({})", &hex::encode(dan)[..12]));
        env.user_list = Some(Value::Array(vec![Value::Map(vec![
            (Value::Integer(Integer::from(0)), Value::Text(hex::encode(dan))),
            (Value::Integer(Integer::from(1)), Value::Text("da\u{202e}\u{200b}n".into())),
        ])]));
        hubs.deliver(env);
        assert_eq!(hubs.hub().name_of(&dan), "dan");
        hubs.notice(Some("lobby"), &format!("members in lobby: ev\u{1b}il ({})", &hex::encode([0xd6; 16])[..12]));
        // (known by a prefix of the identity until it's seen)
        assert!(hubs.hub().nicks.values().any(|nick| nick == "evil"));
        assert!(hubs.hub().nicks.values().all(|nick| !nick.chars().any(|c| c.is_control() || c == '\u{202e}' || c == '\u{200b}')));
    }

    #[test]
    fn nicks_that_look_alike_are_told_apart_for_every_party() {
        let mut hubs = Hubs::connected("look-alike-nicks");
        hubs.app.channels.hubs[0].nick = Some("zev".into());
        hubs.join("lobby", &[OWN, AMY, BOB, CAT]);
        let tag = |id: [u8; 16]| hex::encode(id)[..6].to_string();
        let last = |hubs: &Hubs| hubs.hub().buffers["lobby"].last().unwrap().nick.clone();
        let nick_of = |hubs: &Hubs, id: [u8; 16]| hubs.hub().nicks.get(id.as_slice()).cloned();
        // Alone with its nick, someone is shown as they are.
        hubs.said("lobby", AMY, "carol", "hi");
        assert_eq!(last(&hubs).as_deref(), Some("carol"));
        // Then a copy in Cyrillic letters (U+0441 U+0430) speaks: both are
        // marked, the real one that was here first too, in what was said.
        hubs.said("lobby", BOB, "\u{0441}\u{0430}rol", "send me your key");
        assert_eq!(last(&hubs), Some(format!("\u{0441}\u{0430}rol ({})", tag(BOB))));
        assert_eq!(nick_of(&hubs, AMY), Some(format!("carol ({})", tag(AMY))));
        assert_eq!(hubs.hub().buffers["lobby"].iter().find(|l| l.text == "hi").unwrap().nick, Some(format!("carol ({})", tag(AMY))));
        hubs.said("lobby", AMY, "carol", "again");
        assert_eq!(last(&hubs), Some(format!("carol ({})", tag(AMY))), "the same whoever spoke first");
        // The same nick exactly, from another identity, is a collision.
        hubs.said("lobby", CAT, "carol", "me");
        assert_eq!(last(&hubs), Some(format!("carol ({})", tag(CAT))));
        // Someone else's tag can't be written into a nick, nor a "(?)".
        let dan = [0xd4; 16];
        hubs.said("lobby", dan, &format!("carol ({})", tag(AMY)), "forged");
        assert_eq!(last(&hubs), Some(format!("carol ({})", tag(dan))));
        hubs.said("lobby", [0xd5; 16], "carol (?) ( 0312F2 )", "forged");
        assert_eq!(last(&hubs), Some(format!("carol ({})", tag([0xd5; 16]))));
        // When the others have gone, the one left is shown as they are.
        for gone in [BOB, CAT, dan, [0xd5; 16]] {
            hubs.membership(t::PARTED, "lobby", Some(&[gone]), None);
        }
        assert_eq!(nick_of(&hubs, AMY).as_deref(), Some("carol"));
        assert_eq!(hubs.hub().buffers["lobby"].iter().find(|l| l.text == "hi").unwrap().nick.as_deref(), Some("carol"));
        // Ours, with a joiner after it, or one letter in Cyrillic, from
        // another identity: marked. Ours is not.
        let eve = [0xe5; 16];
        let fay = [0xf6; 16];
        hubs.said("lobby", eve, "zev\u{200d}", "it's me");
        assert_eq!(last(&hubs), Some(format!("zev ({})", tag(eve))));
        hubs.said("lobby", fay, "z\u{0435}v", "no, me");
        assert_eq!(last(&hubs), Some(format!("z\u{0435}v ({})", tag(fay))));
        // A hub naming nobody: a join with no identity, or no sender.
        hubs.membership(t::JOINED, "lobby", None, Some("zev"));
        assert!(hubs.lines("lobby").iter().any(|l| l == "zev (?) joined"), "{:?}", hubs.lines("lobby"));
        hubs.membership(t::PARTED, "lobby", None, Some("Z\u{0435}V"));
        assert!(hubs.lines("lobby").iter().any(|l| l == "Z\u{0435}V (?) left"), "{:?}", hubs.lines("lobby"));
        let mut nameless = Envelope::new(t::MSG, &[]).room("lobby").text("who am I").nick(Some("carol"));
        nameless.src = Vec::new();
        hubs.deliver(nameless);
        assert_eq!(last(&hubs).as_deref(), Some("carol (?)"));
        // A join that lists everyone, and /who: marked as well.
        let gus = [0x77; 16];
        hubs.membership(t::JOINED, "lobby", Some(&[OWN, AMY, gus]), Some("c\u{0430}rol"));
        assert_eq!(nick_of(&hubs, gus), Some(format!("c\u{0430}rol ({})", tag(gus))));
        assert!(hubs.lines("lobby").iter().any(|l| *l == format!("c\u{0430}rol ({}) joined", tag(gus))), "{:?}", hubs.lines("lobby"));
        let hal = [0x88; 16];
        hubs.notice(Some("lobby"), &format!("members in lobby: ZEV ({})", &hex::encode(hal)[..12]));
        assert!(hubs.hub().nicks.values().any(|n| *n == format!("ZEV ({})", &hex::encode(hal)[..6])), "{:?}", hubs.hub().nicks);
        // Nicks that don't collide are left alone: emoji with their joiners,
        // and one that ends in brackets that are no tag.
        hubs.said("lobby", [0x31; 16], "\u{1F469}\u{200d}\u{1F469}\u{200d}\u{1F467} fam", "hi");
        assert_eq!(last(&hubs).as_deref(), Some("\u{1F469}\u{200d}\u{1F469}\u{200d}\u{1F467} fam"));
        hubs.said("lobby", [0x32; 16], "dave (2)", "hi");
        assert_eq!(last(&hubs).as_deref(), Some("dave (2)"));
    }

    #[test]
    fn a_tag_cannot_be_forged_in_a_nick_on_any_path() {
        let mut hubs = Hubs::connected("forged-tags");
        hubs.join("lobby", &[OWN, AMY, BOB]);
        let tag = hex::encode(AMY)[..6].to_string();
        hubs.said("lobby", AMY, "carol", "real");
        for (n, forged) in [
            format!("carol ({tag})\u{200d}"),
            format!("carol ({tag}) \u{200b}\u{202e}"),
            "carol (\u{0441}\u{0430}1\u{0435}f2)".to_string(),
            format!("carol \u{2768}{tag}\u{2769}"),
            format!("carol \u{ff08}{tag}\u{ff09}"),
            format!("carol ( {tag} )"),
            format!("carol ({tag}) (?)"),
            "carol (?)".to_string(),
            "carol(?)\u{2060}".to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            let from = [0x40 + n as u8; 16];
            hubs.said("lobby", from, &forged, "forged");
            let line = hubs.hub().buffers["lobby"].last().unwrap().nick.clone().unwrap();
            assert_eq!(line, format!("carol ({})", &hex::encode(from)[..6]), "{forged:?}");
        }
        // The same on the /who path.
        let eve = [0xe5; 16];
        let mut env = Envelope::new(t::NOTICE, &HUB_ID).text(&format!("members in lobby: carol ({})", &hex::encode(eve)[..12]));
        env.user_list = Some(Value::Array(vec![Value::Map(vec![
            (Value::Integer(Integer::from(0)), Value::Text(hex::encode(eve))),
            (Value::Integer(Integer::from(1)), Value::Text(format!("carol ({tag})\u{200d}"))),
        ])]));
        hubs.deliver(env);
        assert_eq!(hubs.hub().nicks.get(eve.as_slice()), Some(&format!("carol ({})", &hex::encode(eve)[..6])));
        // Not tags: kept.
        hubs.said("lobby", [0x70; 16], "dave (2)", "x");
        assert_eq!(hubs.hub().buffers["lobby"].last().unwrap().nick.as_deref(), Some("dave (2)"));
        hubs.said("lobby", [0x71; 16], "eve (12345678)", "x");
        assert_eq!(hubs.hub().buffers["lobby"].last().unwrap().nick.as_deref(), Some("eve (12345678)"));
    }

    #[test]
    fn a_hub_rotating_nicks_and_people_cannot_make_the_client_slow_or_grow() {
        let mut hubs = Hubs::connected("rotating-nicks");
        hubs.join("lobby", &[OWN, AMY, BOB]);
        hubs.said("lobby", AMY, "carol", "x");
        let (own, started) = ([0u8; 16], std::time::Instant::now());
        let hub = &mut hubs.app.channels.hubs[0];
        // 10,000 nicks from identities that come and go, many of one look
        // (two cases of a word, and Cyrillic letters in it) in the room.
        for n in 0..10_000u32 {
            let mut id = [0u8; 16];
            id[..4].copy_from_slice(&(n % 5_000 + 1).to_be_bytes());
            id[4] = 7;
            let nick = match n % 3 {
                0 => format!("carol{}", n % 7),
                1 => format!("c\u{0430}rol{}", n % 7),
                _ => format!("CAROL{}", n % 7),
            };
            hub.add_member("lobby", &id);
            hub.learn_nick(&own, Some("zev"), &id, nick);
            hub.limit_tables(&own);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
        assert!(
            hub.raw_nicks.len() <= super::super::MAX_NICKS && hub.nicks.len() <= super::super::MAX_NICKS + 8,
            "{} nicks",
            hub.raw_nicks.len()
        );
        assert!(hub.members["lobby"].len() <= super::super::MAX_MEMBERS, "{} members", hub.members["lobby"].len());
        assert!(hub.by_look.values().map(|ids| ids.len()).sum::<usize>() <= 2 * super::super::MAX_NICKS, "the index follows the nicks");
        // The most recent are kept.
        let mut last = [0u8; 16];
        last[..4].copy_from_slice(&5_000u32.to_be_bytes());
        last[4] = 7;
        assert!(hub.members["lobby"].contains(last.as_slice()) && hub.raw_nicks.contains_key(last.as_slice()));
    }

    #[test]
    fn joins_and_parts_among_many_departed_look_alikes_stay_cheap() {
        let mut hubs = Hubs::connected("many-departed");
        hubs.join("lobby", &[OWN, AMY]);
        hubs.said("lobby", AMY, "carol", "x");
        let own = [0u8; 16];
        let id = |n: u32| {
            let mut id = [0u8; 16];
            id[..4].copy_from_slice(&n.to_be_bytes());
            id[4] = 9;
            id
        };
        // A thousand who named themselves with one look and have left.
        for n in 1..=1000u32 {
            hubs.app.channels.hubs[0].learn_nick(&own, None, &id(n), format!("c\u{0430}rol{}", n % 2));
        }
        let started = std::time::Instant::now();
        for n in 2000..5000u32 {
            let who = id(n);
            hubs.membership(t::JOINED, "lobby", Some(&[who]), Some("carol"));
            hubs.membership(t::PARTED, "lobby", Some(&[who]), None);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
        let hub = hubs.hub();
        assert!(hub.raw_nicks.len() <= super::super::MAX_NICKS);
    }

    #[test]
    fn a_flood_of_new_identities_does_not_push_out_the_members_who_were_here() {
        let mut hubs = Hubs::connected("flood-members");
        hubs.join("lobby", &[OWN, AMY, BOB]);
        hubs.said("lobby", AMY, "carol", "hello");
        hubs.said("lobby", BOB, "bob", "hello");
        let own = [0u8; 16];
        let id = |n: u32| {
            let mut id = [0u8; 16];
            id[..4].copy_from_slice(&n.to_be_bytes());
            id[4] = 9;
            id
        };
        // Three thousand come, some named in a list, some speaking.
        for n in 1..=3000u32 {
            let hub = &mut hubs.app.channels.hubs[0];
            hub.add_member("lobby", &id(n));
            hub.learn_nick(&own, None, &id(n), format!("flood{n}"));
            if n % 2 == 0 {
                hub.mark_spoken(&id(n));
            }
            hub.limit_tables(&own);
        }
        let hub = hubs.hub();
        assert!(hub.members["lobby"].len() <= super::super::MAX_MEMBERS);
        assert!(
            hub.members["lobby"].contains(AMY.as_slice()) && hub.members["lobby"].contains(BOB.as_slice()),
            "members who spoke were dropped"
        );
        assert_eq!(hub.nicks.get(AMY.as_slice()).map(String::as_str), Some("carol"));
        // An impostor now shows as one.
        hubs.said("lobby", CAT, "c\u{0430}rol", "send me your key");
        let tag = hex::encode(CAT)[..6].to_string();
        assert_eq!(hubs.hub().buffers["lobby"].last().unwrap().nick, Some(format!("c\u{0430}rol ({tag})")));
        assert_eq!(hubs.hub().nicks.get(AMY.as_slice()), Some(&format!("carol ({})", &hex::encode(AMY)[..6])));
    }

    #[test]
    fn a_join_listing_a_thousand_identities_of_one_nick_is_cheap_and_bounded() {
        let mut hubs = Hubs::connected("big-join");
        hubs.join("lobby", &[OWN, AMY]);
        hubs.said("lobby", AMY, "carol", "x");
        let ids: Vec<[u8; 16]> = (1..=1024u32)
            .map(|n| {
                let mut id = [0u8; 16];
                id[..4].copy_from_slice(&n.to_be_bytes());
                id[4] = 5;
                id
            })
            .collect();
        let started = std::time::Instant::now();
        for _ in 0..200 {
            hubs.membership(t::JOINED, "lobby", Some(&ids), Some("c\u{0430}rol"));
            hubs.membership(t::PARTED, "lobby", Some(&ids), None);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
        assert!(hubs.hub().raw_nicks.len() <= super::super::MAX_NICKS);
    }

    #[test]
    fn an_empty_identity_is_not_the_same_as_everyone() {
        assert!(!super::super::same_identity(&[], &[1; 16]));
        assert!(!super::super::same_identity(&[], &[]));
        assert!(!super::super::same_identity(&[1; 3], &[1; 16]), "a prefix this short names nobody");
        assert!(super::super::same_identity(&[1; 6], &[1; 16]));
        assert!(!super::super::same_identity(&[1; 16], &[2; 16]));
    }

    #[test]
    fn someone_blocked_is_not_shown_in_a_room() {
        let mut hubs = Hubs::connected("blocked");
        hubs.join("lobby", &[AMY, BOB]);
        hubs.said("lobby", AMY, "amy", "hello");
        let address = hex::encode(super::super::users::lxmf_address(&AMY).unwrap());
        hubs.app.store.update_contact(&address, |c| c.trust = Trust::Blocked);
        hubs.said("lobby", AMY, "amy", "still here");
        hubs.said("lobby", BOB, "bob", "hi all");
        assert_eq!(hubs.lines("lobby"), ["You joined #lobby", "hello", "hi all"]);
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
    fn hubs_and_pages_said_in_a_room_are_links() {
        let mut hubs = Hubs::connected("links");
        hubs.join("lobby", &[OWN, AMY]);
        let other_hub = format!("rrc://{}/radio", "12".repeat(16));
        hubs.said("lobby", AMY, "amy", &format!("come to {other_hub} later"));
        let app = &mut hubs.app;
        app.tab = crate::app::Tab::Channels;
        app.channels.selected = Some(super::super::Target { hub: HUB, room: Some("lobby".into()) });
        app.channels.typing = false;
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        // Too long for the narrow view (beside the members): one link on
        // two rows, each part opening it.
        let links = app.regions.channel_links.clone();
        assert!(links.len() == 2 && links.iter().all(|(_, url)| *url == other_hub), "{links:?}");
        assert_eq!(links.iter().map(|(rect, _)| rect.width as usize).sum::<usize>(), other_hub.len());
        let (rect, _) = links[1];
        // A hub not added yet: asked first (it learns your identity).
        let click = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: rect.x + 1,
            row: rect.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        app.on_mouse(click);
        assert!(matches!(app.prompt.as_ref().map(|p| &p.kind), Some(crate::app::PromptKind::ConfirmHub { .. })));
    }

    #[test]
    fn chat_text_is_selected_and_copied_with_the_mouse() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut hubs = Hubs::connected("select");
        hubs.join("lobby", &[OWN, AMY]);
        hubs.said("lobby", AMY, "amy", "the repeater is up");
        hubs.said("lobby", AMY, "amy", "on the hill");
        let app = &mut hubs.app;
        app.tab = crate::app::Tab::Channels;
        app.channels.selected = Some(super::super::Target { hub: HUB, room: Some("lobby".into()) });
        app.channels.typing = false;
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let area = app.regions.channel_history;
        let at = |app: &App, text: &str| {
            let row = app.regions.chat_text.iter().position(|r| r.contains(text)).unwrap();
            let col = app.regions.chat_text[row].find(text).unwrap();
            (area.x + col as u16, area.y + row as u16)
        };
        let mouse = |app: &mut App, kind, (x, y): (u16, u16)| {
            app.on_mouse(MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
        };
        // From "repeater" to the end of "hill", across two lines.
        let (from, (hill_x, hill_y)) = (at(app, "repeater"), at(app, "hill"));
        mouse(app, MouseEventKind::Down(MouseButton::Left), from);
        mouse(app, MouseEventKind::Drag(MouseButton::Left), (hill_x + 3, hill_y));
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let shown = terminal.backend().buffer()[(from.0, from.1)].modifier;
        assert!(shown.contains(ratatui::style::Modifier::REVERSED), "the selection shows");
        mouse(app, MouseEventKind::Up(MouseButton::Left), (hill_x + 3, hill_y));
        let copied = app.clipboard.last.clone().unwrap();
        assert!(copied.starts_with("repeater is up\n") && copied.ends_with("<amy> on the hill"), "{copied:?}");
        // A click alone copies nothing, and lets go of the selection.
        app.clipboard.last = None;
        mouse(app, MouseEventKind::Down(MouseButton::Left), from);
        mouse(app, MouseEventKind::Up(MouseButton::Left), from);
        assert!(app.clipboard.last.is_none() && app.chat_selection.is_none());
    }

    #[test]
    fn how_many_messages_are_kept_is_a_setting() {
        let mut hubs = Hubs::connected("kept");
        hubs.join("lobby", &[OWN, AMY]);
        for i in 0..60 {
            hubs.said("lobby", AMY, "amy", &format!("message {i}"));
        }
        let messages = |hubs: &Hubs| hubs.hub().buffers["lobby"].iter().filter(|l| l.is_message()).count();
        assert_eq!(messages(&hubs), 60);
        // Fewer: the oldest go at once.
        hubs.app.update_settings(&[("rrc_messages_kept", "50")]).unwrap();
        assert_eq!(messages(&hubs), 50);
        assert_eq!(hubs.hub().buffers["lobby"].iter().find(|l| l.is_message()).unwrap().text, "message 10");
        hubs.said("lobby", AMY, "amy", "one more");
        assert_eq!(messages(&hubs), 50);
        assert!(hubs.app.update_settings(&[("rrc_messages_kept", "10")]).is_err(), "50 at least");
        assert_eq!(hubs.app.settings.rrc_messages_kept, 50);
    }

    #[test]
    fn earlier_chat_lines_show_their_date_unless_turned_off() {
        let mut hubs = Hubs::connected("dates");
        hubs.join("lobby", &[OWN, AMY]);
        hubs.said("lobby", AMY, "amy", "hello there");
        let app = &mut hubs.app;
        app.tab = crate::app::Tab::Channels;
        app.channels.selected = Some(super::super::Target { hub: HUB, room: Some("lobby".into()) });
        app.channels.typing = false;
        // When it arrived: a line said live is stamped on arrival, which can
        // be a millisecond (or a second) after the envelope was made.
        let said = app.channels.hubs[0].buffers["lobby"].iter().find(|l| l.text == "hello there").unwrap().ts;
        let when = chrono::TimeZone::timestamp_millis_opt(&chrono::Local, said as i64).single().unwrap();
        let line = |app: &mut App| {
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30)).unwrap();
            terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
            let buffer = terminal.backend().buffer();
            (0..30)
                .map(|y| (0..140).map(|x| buffer[(x, y)].symbol()).collect::<String>())
                .find(|row| row.contains("hello there"))
                .unwrap_or_default()
        };
        // Today's line: its time alone, as every line today would say the
        // same date.
        assert!(app.settings.chat_dates && line(app).contains(&format!("{} <amy> hello there", crate::clock::time(&when, true))));
        // An earlier day's: its date too.
        let earlier = said - 3 * 86_400_000;
        app.channels.hubs[0].buffers.get_mut("lobby").unwrap().iter_mut().filter(|l| l.ts == said).for_each(|l| l.ts = earlier);
        let when = chrono::TimeZone::timestamp_millis_opt(&chrono::Local, earlier as i64).single().unwrap();
        let dated = format!("{} <amy> hello there", crate::clock::dated(&when, true));
        assert!(line(app).contains(&dated), "{}", line(app));
        // D: the time alone.
        app.on_key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('D'), crossterm::event::KeyModifiers::SHIFT));
        assert!(!app.settings.chat_dates);
        assert!(line(app).contains(&format!("{} <amy> hello there", crate::clock::time(&when, true))), "{}", line(app));
        assert!(!line(app).contains(&crate::clock::day(&when)), "{}", line(app));
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

        // A new greeting starting as the old one did: shown whole.
        hubs.app.on_rrc(HUB, RrcEvent::Disconnected(None));
        hubs.welcome();
        hubs.notice(None, "Welcome to the hilltop hub");
        hubs.notice(None, "Net tonight at 8");
        hubs.notice(None, "Be nice");
        let lines = hubs.lines("");
        assert_eq!(
            lines[lines.len() - 2..],
            ["Welcome to the hilltop hub\nNet tonight at 8".to_string(), "Be nice".to_string()],
            "{lines:?}"
        );
        assert_eq!(hubs.hub().motd.as_deref(), Some("Welcome to the hilltop hub\nNet tonight at 8\nBe nice"));
    }

    #[test]
    fn only_the_hub_sets_topics_and_history() {
        let mut hubs = Hubs::connected("member-notice");
        hubs.join("lobby", &[OWN, AMY, BOB]);
        let from_amy = |text: &str| Envelope::new(t::NOTICE, &AMY).room("lobby").text(text).nick(Some("amy"));
        // A member's notices are what they said, by them: not a topic, nor
        // the start of history (which would let their messages say when).
        hubs.deliver(from_amy("topic for lobby is now: send coins here"));
        hubs.deliver(from_amy("--- 1 message from earlier ---"));
        let mut old = Envelope::new(t::MSG, &BOB).room("lobby").text("long ago").nick(Some("bob"));
        old.ts = 1_000;
        hubs.deliver(old);
        assert!(!hubs.hub().topics.contains_key("lobby"));
        let lines = &hubs.hub().buffers["lobby"];
        let amy: Vec<_> = lines.iter().filter(|l| l.src.as_deref() == Some(hex::encode(AMY).as_str())).collect();
        assert_eq!(amy.len(), 2);
        assert!(amy.iter().all(|l| l.kind == LineKind::Msg && l.nick.as_deref() == Some("amy")));
        assert_ne!(lines.iter().find(|l| l.text == "long ago").unwrap().ts, 1_000);
        // The hub's still are.
        hubs.notice(Some("lobby"), "topic for lobby is now: antennas");
        assert_eq!(hubs.hub().topics.get("lobby").map(String::as_str), Some("antennas"));
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

    #[test]
    fn a_hub_cannot_add_rooms_or_whisper_conversations_without_end() {
        let mut hubs = Hubs::connected("unjoined");
        hubs.join("lobby", &[OWN, AMY]);
        // Messages and notices for rooms never joined make no rooms.
        for n in 0..50 {
            hubs.said(&format!("room{n}"), AMY, "amy", "over here");
        }
        hubs.notice(Some("elsewhere"), "the topic of #elsewhere");
        hubs.said("lobby", AMY, "amy", "hello");
        assert_eq!(hubs.hub().buffers.keys().collect::<Vec<_>>(), ["", "lobby"]);
        // The notice is shown where a reply would be: the room on screen.
        assert_eq!(hubs.lines("lobby"), ["You joined #lobby", "the topic of #elsewhere", "hello"]);
        // Whispers from more people than are kept: the quietest go.
        let from = |n: usize| -> [u8; 16] {
            let mut id = [0x77; 16];
            id[..8].copy_from_slice(&(n as u64).to_be_bytes());
            id
        };
        for n in 0..super::super::MAX_WHISPERS + 20 {
            let mut env = Envelope::new(t::NOTICE, &from(n)).text("psst");
            env.dst = Some(OWN.to_vec());
            hubs.deliver(env);
        }
        let whispers = hubs.hub().buffers.keys().filter(|k| super::super::whisper_peer(k).is_some()).count();
        assert_eq!(whispers, super::super::MAX_WHISPERS);
        assert!(hubs.hub().nicks.len() <= super::super::MAX_WHISPERS + 2, "and their nicks: {}", hubs.hub().nicks.len());
        let newest = super::super::whisper_key(&from(super::super::MAX_WHISPERS + 19));
        assert_eq!(hubs.lines(&newest), ["psst"]);
    }
}
