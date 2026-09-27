//! What RRC hub sessions report: connection changes, room membership,
//! messages and notices.

use std::time::{Duration, Instant};

use super::{ChatLine, HubStatus, LineKind, backoff};
use crate::app::App;
use crate::net::Hash;
use crate::rrc::session::RrcEvent;
use crate::rrc::{self, Envelope, t};

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
                hub.motd_until = Some(Instant::now() + Duration::from_secs(5));
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
                } else if !self.consume_notice(index, &text) {
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
                hub.silent_list = 0;
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
                hub.rooms.insert(room.clone());
                hub.buffers.entry(room.clone()).or_default();
                let members = hub.members.entry(room.clone()).or_default();
                members.extend(hashes.iter().cloned());
                members.insert(own.clone());
                // Fan-out JOINED carries the joiner's hash and nick.
                let joiner = (!self_join && hashes.len() == 1 && hashes[0] != own).then(|| hashes[0].clone());
                if let (Some(joiner), Some(nick)) = (&joiner, &env.nick) {
                    hub.nicks.insert(joiner.clone(), nick.clone());
                }
                if self_join {
                    if !silent {
                        self.record(index, &room, ChatLine::new(LineKind::System, format!("You joined #{room}")));
                    }
                    // Learn members' nicks without showing the reply.
                    self.hub_mut(index).silent_who.insert(room.clone());
                    let env = self.envelope(index, t::MSG).room(&room).text(&format!("/who {room}"));
                    self.send_env(index, &env);
                } else if let Some(joiner) = joiner {
                    let name = self.channels.hubs[index].name_of(&joiner);
                    self.record(index, &room, ChatLine::new(LineKind::System, format!("{name} joined")));
                }
            }
            t::PARTED => {
                let Some(room) = room else { return };
                let hashes = env.body_hashes();
                let hub = self.hub_mut(index);
                let self_part = hub.pending_parts.remove(&room);
                let parter = (!self_part && hashes.len() == 1 && hashes[0] != own).then(|| hashes[0].clone());
                if let (Some(parter), Some(nick)) = (&parter, &env.nick) {
                    hub.nicks.insert(parter.clone(), nick.clone());
                }
                if let Some(members) = hub.members.get_mut(&room) {
                    for h in &hashes {
                        members.remove(h);
                    }
                }
                if let Some(parter) = parter {
                    let name = self.channels.hubs[index].name_of(&parter);
                    self.record(index, &room, ChatLine::new(LineKind::System, format!("{name} left")));
                }
            }
            t::MSG | t::ACTION => {
                let is_own = env.src == own;
                let hub = self.hub_mut(index);
                if is_own && let Some(pos) = hub.sent.iter().position(|id| *id == env.id) {
                    // Our own message came back: the hub delivered it.
                    hub.sent.remove(pos);
                    let key = room.clone().unwrap_or_default();
                    if let Some(line) = hub
                        .buffers
                        .get_mut(&key)
                        .and_then(|b| b.iter_mut().rev().find(|l| l.pending.as_ref() == Some(&env.id)))
                    {
                        line.pending = None;
                    }
                    return;
                }
                if let Some(nick) = &env.nick {
                    hub.nicks.insert(env.src.clone(), nick.clone());
                }
                if let Some(room) = &room {
                    hub.members.entry(room.clone()).or_default().insert(env.src.clone());
                }
                let Some(text) = env.body_text() else { return };
                let my_nick = self.effective_nick(index).unwrap_or_default();
                let mut line = ChatLine::new(
                    if env.t == t::ACTION { LineKind::Action } else { LineKind::Msg },
                    text,
                );
                line.src = Some(hex::encode(&env.src));
                line.nick = env.nick.clone();
                line.own = is_own;
                if !is_own {
                    line.mention_at = rrc::mention_ranges(text, &my_nick);
                    line.mention = !line.mention_at.is_empty();
                }
                self.record(index, &room.unwrap_or_default(), line);
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
                if self.consume_notice(index, &text) {
                    return;
                }
                let hub = self.hub_mut(index);
                if room.is_none() && hub.motd_until.take().is_some_and(|until| Instant::now() < until) {
                    self.set_motd(index, text);
                    return;
                }
                let target = room.unwrap_or_else(|| self.reply_room(index));
                self.record(index, &target, ChatLine::new(LineKind::Notice, text));
            }
            t::ERROR => {
                let text = env.body_text().unwrap_or("(error)").to_string();
                let hub = self.hub_mut(index);
                if let Some(room) = &room
                    && hub.pending_joins.remove(room)
                {
                    // The join was refused: forget the room.
                    hub.rooms.remove(room);
                    hub.silent_joins.remove(room);
                    self.save_hubs();
                }
                let target = room.unwrap_or_else(|| self.reply_room(index));
                self.record(index, &target, ChatLine::new(LineKind::Error, text));
            }
            _ => {}
        }
    }

    /// The hub's message of the day goes to the hub view (once per text:
    /// the hub resends it whenever it welcomes us again).
    fn set_motd(&mut self, index: usize, text: String) {
        let hub = self.hub_mut(index);
        if hub.motd.as_deref() == Some(text.as_str()) {
            return;
        }
        hub.motd = Some(text.clone());
        self.record(index, "", ChatLine::new(LineKind::Notice, text));
    }

    /// Interpret hub replies we asked for silently (/list, /who) and room
    /// info. Returns true when the notice should not be shown.
    fn consume_notice(&mut self, index: usize, text: &str) -> bool {
        let hub = self.hub_mut(index);
        if let Some(rooms) = rrc::parse_room_list(text) {
            hub.available = Some(rooms);
            if hub.silent_list > 0 {
                hub.silent_list -= 1;
                return true;
            }
            return false;
        }
        if let Some((room, entries)) = rrc::parse_who(text) {
            let members = hub.members.entry(room.clone()).or_default();
            for entry in &entries {
                if entry.nick.is_none()
                    && let Ok(hash) = hex::decode(&entry.hash_hex)
                {
                    members.insert(hash);
                }
            }
            // Nicked users appear with a 12-hex prefix only.
            let known: Vec<Vec<u8>> = members.iter().cloned().collect();
            for entry in entries {
                if let Some(nick) = entry.nick
                    && let Some(hash) = known.iter().find(|h| hex::encode(h).starts_with(&entry.hash_hex))
                {
                    hub.nicks.insert(hash.clone(), nick);
                }
            }
            return hub.silent_who.remove(&room);
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
                let mode = if info.mode.len() > 1 { format!(", mode {}", info.mode) } else { String::new() };
                let summary = format!("#{} ({registered}{mode}), {topic}", info.room);
                let room = info.room.clone();
                self.record(index, &room, ChatLine::new(LineKind::System, summary));
            }
            return true;
        }
        false
    }
}
