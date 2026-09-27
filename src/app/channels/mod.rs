//! Channels tab: RRC chat state (hubs, rooms, members and message buffers)
//! and the connection lifecycle. Hub events are handled in [`events`],
//! slash commands in [`commands`] and list/key actions in [`actions`].

mod actions;
mod commands;
mod events;
pub mod users;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::widgets::ListState;
use serde::{Deserialize, Serialize};

use super::{App, Tab};
use crate::net::{Hash, NetCommand, parse_hash};
use crate::rrc::session::SessionCommand;
use crate::rrc::{self, Envelope, Limits, t};
use crate::store::HubConfig;
use crate::term::input::TextInput;


/// Lines kept per room (NomadNet keeps 500).
const MAX_LINES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    Msg,
    Action,
    Notice,
    /// A direct notice from another user.
    Private,
    Error,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLine {
    pub kind: LineKind,
    /// Sender identity hash (hex).
    pub src: Option<String>,
    pub nick: Option<String>,
    pub text: String,
    /// Unix milliseconds (when it arrived).
    pub ts: u64,
    #[serde(default)]
    pub mention: bool,
    /// Byte ranges of the mentions in `text`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mention_at: Vec<(usize, usize)>,
    #[serde(default)]
    pub own: bool,
    /// Our message id until the hub echoes it back.
    #[serde(skip)]
    pub pending: Option<Vec<u8>>,
}

impl ChatLine {
    fn new(kind: LineKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            src: None,
            nick: None,
            text: text.into(),
            ts: rrc::now_ms(),
            mention: false,
            mention_at: Vec::new(),
            own: false,
            pending: None,
        }
    }

    /// Byte ranges of `text` to highlight as mentions of us. Lines saved
    /// before ranges were kept are matched against `nick` again.
    pub fn highlights(&self, nick: &str) -> Vec<(usize, usize)> {
        match (self.mention, self.mention_at.is_empty()) {
            (false, _) => Vec::new(),
            (true, false) => self.mention_at.clone(),
            (true, true) => rrc::mention_ranges(&self.text, nick),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubStatus {
    Disconnected,
    Connecting(String),
    Connected,
    Failed(String),
}

pub struct Hub {
    pub hash: Hash,
    pub aspect: String,
    pub name: String,
    pub nick: Option<String>,
    pub auto_connect: bool,
    pub status: HubStatus,
    hub_identity: Option<Vec<u8>>,
    pub hub_name: Option<String>,
    pub motd: Option<String>,
    pub limits: Limits,
    pub direct_notices: bool,
    /// Joined rooms (rejoined automatically after reconnecting).
    pub rooms: BTreeSet<String>,
    /// Message buffers; the empty key is the hub's own buffer.
    pub buffers: BTreeMap<String, Vec<ChatLine>>,
    pub unread: HashMap<String, usize>,
    pub mentions: HashSet<String>,
    pub members: HashMap<String, BTreeSet<Vec<u8>>>,
    pub nicks: HashMap<Vec<u8>, String>,
    pub topics: HashMap<String, String>,
    /// Public rooms from `/list`.
    pub available: Option<Vec<(String, Option<String>)>>,
    pending_joins: HashSet<String>,
    silent_joins: HashSet<String>,
    quiet_info: HashSet<String>,
    pending_parts: HashSet<String>,
    silent_who: HashSet<String>,
    silent_list: u32,
    sent: VecDeque<Vec<u8>>,
    /// Room-less notices right after WELCOME are the MOTD.
    motd_until: Option<Instant>,
    reconnect_at: Option<Instant>,
    attempts: u32,
    manual: bool,
    history_dirty: bool,
}

impl Hub {
    fn new(hash: Hash, aspect: String, name: String) -> Self {
        Self {
            hash,
            aspect,
            name,
            nick: None,
            auto_connect: true,
            status: HubStatus::Disconnected,
            hub_identity: None,
            hub_name: None,
            motd: None,
            limits: Limits::default(),
            direct_notices: false,
            rooms: BTreeSet::new(),
            buffers: BTreeMap::from([(String::new(), Vec::new())]),
            unread: HashMap::new(),
            mentions: HashSet::new(),
            members: HashMap::new(),
            nicks: HashMap::new(),
            topics: HashMap::new(),
            available: None,
            pending_joins: HashSet::new(),
            silent_joins: HashSet::new(),
            quiet_info: HashSet::new(),
            pending_parts: HashSet::new(),
            silent_who: HashSet::new(),
            silent_list: 0,
            sent: VecDeque::new(),
            motd_until: None,
            reconnect_at: None,
            attempts: 0,
            manual: false,
            history_dirty: false,
        }
    }

    fn config(&self) -> HubConfig {
        HubConfig {
            hash: hex::encode(self.hash),
            aspect: self.aspect.clone(),
            name: self.name.clone(),
            rooms: self.rooms.iter().cloned().collect(),
            nick: self.nick.clone(),
            auto_connect: self.auto_connect,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.status == HubStatus::Connected
    }

    /// Nick for a member: learned nick, else a short hash.
    pub fn name_of(&self, hash: &[u8]) -> String {
        self.nicks
            .get(hash)
            .cloned()
            .unwrap_or_else(|| hex::encode(hash).chars().take(12).collect())
    }

    /// Rooms shown in the list: joined ones plus parted rooms with history.
    pub fn listed_rooms(&self) -> Vec<String> {
        let mut rooms: BTreeSet<String> = self.rooms.clone();
        rooms.extend(self.buffers.keys().filter(|k| !k.is_empty()).cloned());
        rooms.into_iter().collect()
    }

    /// A room's members as (name, identity), sorted by name.
    pub fn members_of(&self, room: &str) -> Vec<(String, Vec<u8>)> {
        let mut members: Vec<(String, Vec<u8>)> = self
            .members
            .get(room)
            .map(|m| m.iter().map(|h| (self.name_of(h), h.clone())).collect())
            .unwrap_or_default();
        members.sort_by_key(|(n, _)| n.to_lowercase());
        members
    }

    fn push(&mut self, room: &str, line: ChatLine) {
        let buffer = self.buffers.entry(room.to_string()).or_default();
        buffer.push(line);
        if buffer.len() > MAX_LINES {
            buffer.drain(..buffer.len() - MAX_LINES);
        }
        self.history_dirty = true;
    }

    fn history_path(&self, dir: &Path) -> PathBuf {
        let mut name = hex::encode(self.hash);
        if self.aspect != rrc::DEFAULT_ASPECT {
            name.push_str("__");
            name.push_str(&hex::encode(&rns_crypto::sha::full_hash(self.aspect.as_bytes())[..4]));
        }
        dir.join(format!("{name}.json"))
    }

    /// Chat lines worth keeping across restarts.
    fn save_history(&mut self, dir: &Path) -> std::io::Result<()> {
        let keep: BTreeMap<&String, Vec<&ChatLine>> = self
            .buffers
            .iter()
            .filter(|(room, _)| !room.is_empty())
            .map(|(room, lines)| {
                let lines = lines
                    .iter()
                    .filter(|l| matches!(l.kind, LineKind::Msg | LineKind::Action | LineKind::Private))
                    .collect();
                (room, lines)
            })
            .collect();
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string(&keep).map_err(std::io::Error::other)?;
        crate::config::write_atomic(&self.history_path(dir), json.as_bytes())
            .map_err(std::io::Error::other)?;
        self.history_dirty = false;
        Ok(())
    }

    fn load_history(&mut self, dir: &Path) {
        let Ok(text) = std::fs::read_to_string(self.history_path(dir)) else {
            return;
        };
        if let Ok(rooms) = serde_json::from_str::<BTreeMap<String, Vec<ChatLine>>>(&text) {
            for (room, lines) in rooms {
                self.buffers.insert(room, lines);
            }
        }
    }
}

/// What the Channels list has selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub hub: Hash,
    /// `None` for the hub itself.
    pub room: Option<String>,
}

#[derive(Default)]
pub struct Channels {
    pub hubs: Vec<Hub>,
    pub list: ListState,
    pub selected: Option<Target>,
    pub input: TextInput,
    pub typing: bool,
    pub scroll: usize,
    /// Popups: actions for a user, or picking one of the room's members.
    pub menu: Option<users::UserMenu>,
    pub picker: Option<users::MemberPicker>,
}

/// A row of the Channels list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Hub(usize),
    Room(usize, String),
}

impl Channels {
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (i, hub) in self.hubs.iter().enumerate() {
            rows.push(Row::Hub(i));
            rows.extend(hub.listed_rooms().into_iter().map(|r| Row::Room(i, r)));
        }
        rows
    }

    pub fn hub_index(&self, hash: Hash) -> Option<usize> {
        self.hubs.iter().position(|h| h.hash == hash)
    }

    /// Index and room key ("" for the hub buffer) of the selection.
    pub fn active(&self) -> Option<(usize, String)> {
        let target = self.selected.as_ref()?;
        let index = self.hub_index(target.hub)?;
        Some((index, target.room.clone().unwrap_or_default()))
    }

    pub fn total_unread(&self) -> (usize, bool) {
        let count = self.hubs.iter().flat_map(|h| h.unread.values()).sum();
        let mention = self.hubs.iter().any(|h| !h.mentions.is_empty());
        (count, mention)
    }

    fn select_row(&mut self, row: &Row) {
        self.selected = Some(match row {
            Row::Hub(i) => Target {
                hub: self.hubs[*i].hash,
                room: None,
            },
            Row::Room(i, room) => Target {
                hub: self.hubs[*i].hash,
                room: Some(room.clone()),
            },
        });
        self.scroll = 0;
    }

    pub fn load(configs: &[HubConfig], history_dir: &Path) -> Self {
        let mut channels = Channels::default();
        for config in configs {
            let Some(hash) = parse_hash(&config.hash) else { continue };
            let mut hub = Hub::new(hash, config.aspect.clone(), config.name.clone());
            hub.nick = config.nick.clone();
            hub.auto_connect = config.auto_connect;
            hub.rooms = config.rooms.iter().map(|r| rrc::normalize_room(r)).collect();
            hub.load_history(history_dir);
            channels.hubs.push(hub);
        }
        if let Some(first) = channels.hubs.first() {
            channels.selected = Some(Target {
                hub: first.hash,
                room: first.rooms.iter().next().cloned(),
            });
        }
        channels
    }
}

fn backoff(attempts: u32) -> Duration {
    Duration::from_secs(2u64.saturating_pow(attempts.min(6)).clamp(2, 60))
}

impl App {
    fn hub_mut(&mut self, index: usize) -> &mut Hub {
        &mut self.channels.hubs[index]
    }

    fn effective_nick(&self, index: usize) -> Option<String> {
        let hub = &self.channels.hubs[index];
        let nick = hub.nick.clone().unwrap_or_else(|| self.settings.display_name.clone());
        rrc::normalize_nick(&nick, hub.limits.max_nick_bytes)
    }

    /// Save hub list changes into the store.
    fn save_hubs(&mut self) {
        self.store.rrc_hubs = self.channels.hubs.iter().map(Hub::config).collect();
        self.store_dirty = true;
    }

    pub fn save_rrc_history(&mut self) {
        let dir = self.paths.rrc_history.clone();
        let mut failed = Vec::new();
        for hub in self.channels.hubs.iter_mut().filter(|h| h.history_dirty) {
            if let Err(e) = hub.save_history(&dir) {
                failed.push(format!("Could not save chat history for {}: {e}", hub.name));
            }
        }
        for line in failed {
            self.log(line);
        }
    }

    /// Whether a buffer is on screen right now.
    fn is_viewing(&self, index: usize, room: &str) -> bool {
        self.tab == Tab::Channels
            && self.channels.active().is_some_and(|(i, r)| i == index && r == room)
    }

    /// Add a line to a buffer, counting it unread unless it is on screen.
    fn record(&mut self, index: usize, room: &str, line: ChatLine) {
        let counts = !line.own && matches!(line.kind, LineKind::Msg | LineKind::Action | LineKind::Private);
        let mention = line.mention || line.kind == LineKind::Private;
        let viewing = self.is_viewing(index, room);
        let hub = self.hub_mut(index);
        hub.push(room, line);
        if counts && !viewing {
            *hub.unread.entry(room.to_string()).or_default() += 1;
            if mention {
                hub.mentions.insert(room.to_string());
            }
        }
    }

    /// Room for a reply that names no room: what the user is looking at on
    /// this hub, else the hub buffer.
    fn reply_room(&self, index: usize) -> String {
        match self.channels.active() {
            Some((i, room)) if i == index => room,
            _ => String::new(),
        }
    }

    pub fn mark_channel_read(&mut self) {
        if self.tab != Tab::Channels {
            return;
        }
        if let Some((index, room)) = self.channels.active() {
            self.mark_room_read(index, &room);
        }
    }

    pub fn mark_room_read(&mut self, index: usize, room: &str) {
        let hub = self.hub_mut(index);
        hub.unread.remove(room);
        hub.mentions.remove(room);
    }

    /// Join a room the user asked for (saved, and connecting if needed).
    pub fn join_room(&mut self, index: usize, room: &str) {
        self.join(index, room, None, false);
    }

    pub fn leave_room(&mut self, index: usize, room: &str) {
        self.part(index, room);
    }

    pub fn connect_hub(&mut self, index: usize) {
        let nick = self.effective_nick(index);
        let hub = self.hub_mut(index);
        if matches!(hub.status, HubStatus::Connected | HubStatus::Connecting(_)) {
            return;
        }
        hub.manual = false;
        hub.reconnect_at = None;
        hub.status = HubStatus::Connecting("Starting".into());
        let command = NetCommand::RrcConnect {
            hub: hub.hash,
            aspect: hub.aspect.clone(),
            nick,
        };
        self.send(command);
    }

    fn disconnect_hub(&mut self, index: usize) {
        let hub = self.hub_mut(index);
        hub.manual = true;
        hub.reconnect_at = None;
        hub.attempts = 0;
        hub.status = HubStatus::Disconnected;
        self.session(index, SessionCommand::Disconnect);
    }

    /// Connect auto-connect hubs once the network is up.
    pub fn start_channels(&mut self) {
        for index in 0..self.channels.hubs.len() {
            if self.channels.hubs[index].auto_connect {
                self.connect_hub(index);
            }
        }
    }

    /// Reconnect hubs whose backoff has elapsed.
    pub fn channels_tick(&mut self) {
        let due: Vec<usize> = self
            .channels
            .hubs
            .iter()
            .enumerate()
            .filter(|(_, h)| h.reconnect_at.is_some_and(|at| at <= Instant::now()))
            .map(|(i, _)| i)
            .collect();
        for index in due {
            self.hub_mut(index).reconnect_at = None;
            self.hub_mut(index).status = HubStatus::Disconnected;
            self.connect_hub(index);
        }
    }

    /// Pass a command to this hub's session in the network actor.
    fn session(&mut self, index: usize, command: SessionCommand) {
        let hub = self.channels.hubs[index].hash;
        self.send(NetCommand::Rrc { hub, command });
    }

    fn send_env(&mut self, index: usize, env: &Envelope) {
        self.session(index, SessionCommand::Send(env.encode()));
    }

    fn envelope(&self, index: usize, kind: u64) -> Envelope {
        Envelope::new(kind, &self.identity_hash).nick(self.effective_nick(index).as_deref())
    }

    fn join(&mut self, index: usize, room: &str, key: Option<&str>, silent: bool) {
        let room = rrc::normalize_room(room);
        let max = self.channels.hubs[index].limits.max_room_bytes;
        if room.is_empty() || room.len() > max {
            let reply = self.reply_room(index);
            self.record(index, &reply, ChatLine::new(LineKind::Error, format!("Room names are 1-{max} bytes")));
            return;
        }
        let hub = self.hub_mut(index);
        hub.rooms.insert(room.clone());
        hub.buffers.entry(room.clone()).or_default();
        if hub.is_connected() {
            hub.pending_joins.insert(room.clone());
            if silent {
                hub.silent_joins.insert(room.clone());
                hub.quiet_info.insert(room.clone());
            }
            let mut env = self.envelope(index, t::JOIN).room(&room);
            if let Some(key) = key.filter(|k| !k.is_empty()) {
                env = env.text(key);
            }
            self.send_env(index, &env);
        } else if !silent {
            self.connect_hub(index);
        }
        if !silent {
            self.save_hubs();
        }
    }

    fn part(&mut self, index: usize, room: &str) {
        let room = rrc::normalize_room(room);
        let connected = self.channels.hubs[index].is_connected();
        if connected {
            self.hub_mut(index).pending_parts.insert(room.clone());
            let env = self.envelope(index, t::PART).room(&room);
            self.send_env(index, &env);
        }
        let hub = self.hub_mut(index);
        hub.rooms.remove(&room);
        hub.members.remove(&room);
        self.record(index, &room, ChatLine::new(LineKind::System, format!("You left #{room}")));
        self.save_hubs();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(20), Duration::from_secs(60));
    }

    #[test]
    fn rows_list_hubs_then_rooms() {
        let mut channels = Channels::default();
        let mut hub = Hub::new([1; 16], rrc::DEFAULT_ASPECT.into(), "Hub".into());
        hub.rooms.insert("b".into());
        hub.buffers.insert("a".into(), Vec::new()); // parted, with history
        channels.hubs.push(hub);
        assert_eq!(
            channels.rows(),
            vec![Row::Hub(0), Row::Room(0, "a".into()), Row::Room(0, "b".into())]
        );
    }
}
