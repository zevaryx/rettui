//! Channels tab: RRC chat state (hubs, rooms, members and message buffers)
//! and the connection lifecycle. Hub events are handled in [`events`],
//! slash commands in [`commands`] and list/key actions in [`actions`].

mod actions;
mod commands;
mod events;
pub mod search;
pub mod users;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::widgets::ListState;
use serde::{Deserialize, Serialize};

use super::notify::{self, Notification, Target as Opens};
use super::{App, Tab};
use crate::net::{Hash, NetCommand, parse_hash};
use crate::rrc::session::SessionCommand;
use crate::rrc::{self, Envelope, Limits, t};
use crate::store::{HubConfig, NotifyLevel};
use crate::term::input::TextInput;

/// The most whisper conversations kept on one hub: anyone on it can whisper
/// from as many identities as they like, and each is a conversation listed
/// and saved. Past it, the one quiet longest goes.
pub(crate) const MAX_WHISPERS: usize = 100;

/// Other lines kept with the messages of a room or whisper conversation
/// (as many as "Chat messages kept" says) (people joining and leaving, the hub's
/// notices and errors), for this run only: counted on their own, so a busy
/// room's comings and goings never push its messages out.
const MAX_OTHER_LINES: usize = 200;
/// The most a hub's saved history may be when read (a few hundred lines for
/// each room it is kept for).
const MAX_HISTORY_BYTES: u64 = 32 * 1024 * 1024;

/// The buffer of a whisper conversation (private notices) with a user:
/// `@` and their identity in hex. Rooms can't take this form.
pub fn whisper_key(identity: &[u8]) -> String {
    format!("@{}", hex::encode(identity))
}

/// The other user of a whisper buffer, if `key` is one.
pub fn whisper_peer(key: &str) -> Option<Vec<u8>> {
    let hex = key.strip_prefix('@')?;
    (hex.len() == 32).then(|| hex::decode(hex).ok()).flatten()
}

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
    /// Someone else joining or leaving the room (hidden when joins and
    /// leaves are turned off). System lines aren't saved, so neither is this.
    #[serde(skip)]
    pub presence: bool,
    /// Our message id until the hub echoes it back.
    #[serde(skip)]
    pub pending: Option<Vec<u8>>,
    /// The message's id (hex), to know it again: some hubs replay recent
    /// messages to whoever joins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
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
            presence: false,
            pending: None,
            id: None,
        }
    }

    /// Someone else joining or leaving: `name joined` / `name left`.
    pub fn presence(text: String) -> Self {
        Self { presence: true, ..Self::new(LineKind::System, text) }
    }

    /// Something said (a message, an action or a whisper), as kept across
    /// restarts; not a join, a notice or an error.
    pub fn is_message(&self) -> bool {
        matches!(self.kind, LineKind::Msg | LineKind::Action | LineKind::Private)
    }

    /// Whether to show it, given the "Show joins and leaves" setting.
    pub fn shown(&self, show_joins: bool) -> bool {
        show_joins || !self.presence
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

    /// Byte ranges of `@name` mentions of `people` (a room's
    /// [`Hub::mentionable`]), each with who is mentioned; ranges inside
    /// `highlights` (mentions of us, shown their own way) are left out.
    pub fn user_mentions<'a>(&self, people: &'a [(String, Vec<u8>)], highlights: &[(usize, usize)]) -> Vec<(usize, usize, &'a [u8])> {
        if !matches!(self.kind, LineKind::Msg | LineKind::Action | LineKind::Private) || !self.text.contains('@') {
            return Vec::new();
        }
        let names: Vec<&str> = people.iter().map(|(name, _)| name.as_str()).collect();
        rrc::user_mentions(&self.text, &names)
            .into_iter()
            .filter(|&(start, end, _)| !highlights.iter().any(|&(s, e)| start < e && s < end))
            .map(|(start, end, i)| (start, end, people[i].1.as_slice()))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubStatus {
    Disconnected,
    Connecting(String),
    Connected,
    Failed(String),
}

/// History a hub is replaying into a room (the Go hub does on joining):
/// its opening notice, shown before the first message we hadn't seen.
struct Replay {
    header: String,
    shown: bool,
}

/// Digits of an identity shown after a nick that collides with another.
const TAG_DIGITS: usize = 6;

/// Most nicks kept as the hub gave them, per hub: those of people not in
/// a room go first, then of those never seen speaking, the newest first. A hub's rooms have a few hundred people
/// (rettui's own takes 256 links), so this holds several times that, and a
/// hub that makes up identities can't grow it, nor make the work on each new
/// nick longer than this allows.
const MAX_NICKS: usize = 1024;
/// Most members kept per room: those with no nick go first, then those
/// never seen speaking, the newest first.
const MAX_MEMBERS: usize = 1024;
/// Most identities one message about who came or went is worked out for
/// (the rest are dropped from that, as the tables are bounded).
const MAX_PRESENCE_IDS: usize = 32;
/// Lines of a room looked at when a nick's marking changes (the recent ones).
const REMARK_LINES: usize = 200;

/// A nick a hub gave someone, cleaned, how it looks (see [`Hub::shown`]), and
/// its place in the order they were given.
#[derive(Debug, Clone)]
struct Said {
    nick: String,
    looks: [String; 2],
    seq: u64,
    /// Seen speaking (not only named in a list).
    spoken: bool,
}

/// Whether two identities are one person: the same, or one is the start
/// (of at least 6 bytes, as `/who` gives) of the other. Nobody (empty) is no one.
fn same_identity(a: &[u8], b: &[u8]) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    !short.is_empty() && (short == long || (short.len() >= 6 && short.len() < long.len() && long.starts_with(short)))
}

/// The TS #39 skeleton of `text`: what it looks like, with look-alike
/// letters and brackets the same and invisible characters gone.
fn skeleton(text: &str) -> String {
    unicode_security::skeleton(text).collect()
}

/// A nick from a hub, as it may be kept and shown: cleaned (see
/// [`crate::names::clean`]), and without a trailing tag of the kind
/// [`Hub::shown`] adds (see [`without_tag`]). Used for every nick that comes
/// from a hub, wherever it comes in.
fn incoming_nick(raw: &str) -> Option<String> {
    without_tag(&crate::names::clean(raw)?)
}

/// A nick without a trailing group that looks like the tag [`Hub::shown`]
/// adds (6 hex digits) or `(?)`, so that a hub or a member can't send a nick
/// that already ends like one, to pass for one that was told apart. Looks
/// like is by skeleton: look-alike digits and letters (Cyrillic `с`, `е`,
/// `а`), look-alike brackets and spaces, and invisible characters inside or
/// after the group count. Repeated while the end looks like one.
fn without_tag(nick: &str) -> Option<String> {
    // What hex digits look like (a `1` is skeleton `l`, a `0` is `O`).
    let hex_looks = skeleton("0123456789abcdefABCDEF");
    let mut nick = nick;
    loop {
        nick = nick.trim_end_matches(|c: char| c.is_whitespace() || skeleton(c.encode_utf8(&mut [0; 4])).is_empty());
        // The nearest opening bracket (a tag is short).
        let open = nick.char_indices().rev().take(40).find(|(_, c)| skeleton(c.encode_utf8(&mut [0; 4])) == "(").map(|(at, _)| at);
        let Some(open) = open else { break };
        let flat: String = skeleton(&nick[open..]).chars().filter(|c| !c.is_whitespace()).collect();
        let inside = flat.strip_prefix('(').and_then(|rest| rest.strip_suffix(')'));
        let is_tag = inside
            .is_some_and(|inside| inside == "?" || (inside.chars().count() == TAG_DIGITS && inside.chars().all(|c| hex_looks.contains(c))));
        if !is_tag {
            break;
        }
        nick = &nick[..open];
    }
    (!nick.is_empty()).then(|| nick.to_string())
}

pub struct Hub {
    pub hash: Hash,
    pub aspect: String,
    pub name: String,
    pub nick: Option<String>,
    pub auto_connect: bool,
    /// Notifications for the hub's rooms and whispers (mentions and whispers
    /// when unset), and rooms set otherwise.
    pub notify: Option<NotifyLevel>,
    pub room_notify: BTreeMap<String, NotifyLevel>,
    pub status: HubStatus,
    hub_identity: Option<Vec<u8>>,
    pub hub_name: Option<String>,
    pub motd: Option<String>,
    /// The greeting so far, while it comes in several notices, the one
    /// before it, and whether any of it has been shown (none is while it's
    /// the greeting we had).
    motd_parts: Option<(String, Option<String>, bool)>,
    pub limits: Limits,
    pub direct_notices: bool,
    /// The hub hosted here: its public key, to reach it directly.
    pub local_key: Option<[u8; 64]>,
    /// Joined rooms (rejoined automatically after reconnecting).
    pub rooms: BTreeSet<String>,
    /// Keys of keyed (`+k`) rooms, to rejoin them.
    keys: BTreeMap<String, String>,
    /// Message buffers; the empty key is the hub's own buffer.
    pub buffers: BTreeMap<String, Vec<ChatLine>>,
    pub unread: HashMap<String, usize>,
    pub mentions: HashSet<String>,
    pub members: HashMap<String, BTreeSet<Vec<u8>>>,
    /// What each person goes by here, as shown (see [`Hub::shown`]).
    pub nicks: HashMap<Vec<u8>, String>,
    /// The nicks as the hub gave them, cleaned, and how they look.
    raw_nicks: HashMap<Vec<u8>, Said>,
    /// Whose nick has each skeleton (either form), to find who it collides with.
    by_look: HashMap<String, HashSet<Vec<u8>>>,
    nick_seq: u64,
    pub topics: HashMap<String, String>,
    /// Public rooms from `/list`.
    pub available: Option<Vec<(String, Option<String>)>>,
    pending_joins: HashSet<String>,
    silent_joins: HashSet<String>,
    quiet_info: HashSet<String>,
    pending_parts: HashSet<String>,
    /// Rooms whose `/who` we asked for quietly, until when to expect (more
    /// of) the reply.
    silent_who: HashMap<String, Instant>,
    /// When each room's last `/who` reply came (a reply's next part comes
    /// soon after).
    who_at: HashMap<String, Instant>,
    /// Rooms with a quiet `/who` not answered yet, since when.
    who_asked: HashMap<String, Instant>,
    silent_list: u32,
    /// Until when the rest of a `/list` reply may come, and whether it's
    /// quiet.
    list_more: Option<(Instant, bool)>,
    /// Rooms whose history the hub is replaying.
    replay: HashMap<String, Replay>,
    sent: VecDeque<Vec<u8>>,
    /// Room-less notices right after WELCOME (and each other) are the MOTD.
    motd_until: Option<Instant>,
    reconnect_at: Option<Instant>,
    attempts: u32,
    manual: bool,
    history_dirty: bool,
}

impl Hub {
    pub(crate) fn new(hash: Hash, aspect: String, name: String) -> Self {
        Self {
            hash,
            aspect,
            name,
            nick: None,
            auto_connect: true,
            notify: None,
            room_notify: BTreeMap::new(),
            status: HubStatus::Disconnected,
            hub_identity: None,
            hub_name: None,
            motd: None,
            motd_parts: None,
            limits: Limits::default(),
            direct_notices: false,
            local_key: None,
            rooms: BTreeSet::new(),
            keys: BTreeMap::new(),
            buffers: BTreeMap::from([(String::new(), Vec::new())]),
            unread: HashMap::new(),
            mentions: HashSet::new(),
            members: HashMap::new(),
            nicks: HashMap::new(),
            raw_nicks: HashMap::new(),
            by_look: HashMap::new(),
            nick_seq: 0,
            topics: HashMap::new(),
            available: None,
            pending_joins: HashSet::new(),
            silent_joins: HashSet::new(),
            quiet_info: HashSet::new(),
            pending_parts: HashSet::new(),
            silent_who: HashMap::new(),
            who_at: HashMap::new(),
            who_asked: HashMap::new(),
            silent_list: 0,
            list_more: None,
            replay: HashMap::new(),
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
            room_keys: self.keys.iter().filter(|(room, _)| self.rooms.contains(*room)).map(|(r, k)| (r.clone(), k.clone())).collect(),
            nick: self.nick.clone(),
            auto_connect: self.auto_connect,
            notify: self.notify,
            room_notify: self.room_notify.clone(),
        }
    }

    /// What in a room (or whisper conversation) gets a notification: its
    /// own level, else the hub's.
    pub fn notify_level(&self, room: &str) -> NotifyLevel {
        self.room_notify.get(room).copied().or(self.notify).unwrap_or(NotifyLevel::Mentions)
    }

    pub fn is_connected(&self) -> bool {
        self.status == HubStatus::Connected
    }

    /// Nick for a member: learned nick, else a short hash.
    pub fn name_of(&self, hash: &[u8]) -> String {
        self.nicks.get(hash).cloned().unwrap_or_else(|| hex::encode(hash).chars().take(12).collect())
    }

    /// Whether `id` is in a room with us (or is us).
    fn is_here(&self, id: &[u8]) -> bool {
        self.members.values().any(|room| room.contains(id))
    }

    /// Whether `nick`, which the hub gives `id`, looks like ours (`mine`, or
    /// the one the hub gave us) or like another's in a room with us: the
    /// same TS #39 skeleton as, as written or in lower case, as the hosted
    /// hub tests it. The same nick of two people is a collision too. An
    /// empty `id` (nobody is named) is no one of those.
    fn collides(&self, own: &[u8], mine: Option<&str>, id: &[u8], nick: &str) -> bool {
        if same_identity(id, own) {
            return false;
        }
        let looks = crate::rrc::host::looks(nick);
        let ours = match self.raw_nicks.get(own) {
            Some(said) => said.looks.iter().any(|look| looks.contains(look)),
            None => mine.is_some_and(|mine| crate::rrc::host::looks(mine).iter().any(|look| looks.contains(look))),
        };
        ours || looks.iter().any(|look| {
            self.by_look
                .get(look)
                .is_some_and(|ids| ids.iter().any(|other| !same_identity(other, id) && !same_identity(other, own) && self.is_here(other)))
        })
    }

    /// `nick` as shown for `id`, as the hub gave it unless it collides (see
    /// [`Hub::collides`]): then with the start of the identity after it, as
    /// `/who` lists people, `саrol (0312f2)`, or `(?)` when the hub names
    /// nobody. Every party to a collision is marked, the first to speak
    /// and the last alike.
    pub fn shown(&self, own: &[u8], mine: Option<&str>, id: &[u8], nick: &str) -> String {
        if !self.collides(own, mine, id, nick) {
            return nick.to_string();
        }
        if id.is_empty() {
            format!("{nick} (?)")
        } else {
            format!("{nick} ({})", hex::encode(id).chars().take(TAG_DIGITS).collect::<String>())
        }
    }

    fn put_raw(&mut self, id: &[u8], said: Said) {
        self.remove_raw(id);
        for look in &said.looks {
            self.by_look.entry(look.clone()).or_default().insert(id.to_vec());
        }
        self.raw_nicks.insert(id.to_vec(), said);
    }

    fn remove_raw(&mut self, id: &[u8]) {
        let Some(said) = self.raw_nicks.remove(id) else { return };
        for look in &said.looks {
            if let Some(ids) = self.by_look.get_mut(look) {
                ids.remove(id);
                if ids.is_empty() {
                    self.by_look.remove(look);
                }
            }
        }
    }

    /// The hub says `id` goes by `nick` (cleaned): kept as it was said, and
    /// what is shown is worked out again for those it can change: this
    /// one, and a few who share a skeleton with the old nick or the new
    /// (in a group of three or more, each still has another to collide with).
    pub fn learn_nick(&mut self, own: &[u8], mine: Option<&str>, id: &[u8], nick: String) {
        if id.is_empty() || self.raw_nicks.get(id).is_some_and(|said| said.nick == nick) && self.nicks.contains_key(id) {
            return;
        }
        let looks = crate::rrc::host::looks(&nick);
        let mut classes: Vec<String> = looks.to_vec();
        if let Some(old) = self.raw_nicks.get(id) {
            classes.extend(old.looks.iter().cloned());
        }
        self.nick_seq += 1;
        let seq = self.nick_seq;
        let spoken = self.raw_nicks.get(id).is_some_and(|said| said.spoken);
        self.put_raw(id, Said { nick, looks, seq, spoken });
        self.refresh_groups(own, mine, &[id.to_vec()], classes, Some(id));
        self.limit_tables(own);
    }

    /// `id` was seen speaking: its nick is among those kept longest.
    pub fn mark_spoken(&mut self, id: &[u8]) {
        if let Some(said) = self.raw_nicks.get_mut(id) {
            said.spoken = true;
        }
    }

    /// What is shown, worked out again for `ids` and for the few in the
    /// look groups of their nicks (and of `classes`, those of nicks that
    /// were replaced) that it can change: in a group of three or more, each
    /// still has another to collide with. `changed`: the one whose nick
    /// changed, so its lines keep what was said.
    fn refresh_groups(&mut self, own: &[u8], mine: Option<&str>, ids: &[Vec<u8>], classes: Vec<String>, changed: Option<&[u8]>) {
        let ids: Vec<&Vec<u8>> = ids.iter().take(MAX_PRESENCE_IDS).collect();
        let named: HashSet<&Vec<u8>> = ids.iter().copied().collect();
        let mut classes: HashSet<String> = classes.into_iter().collect();
        for id in &ids {
            if let Some(said) = self.raw_nicks.get(*id) {
                classes.extend(said.looks.iter().cloned());
            }
        }
        let mut touched: HashSet<Vec<u8>> = ids.iter().map(|id| (*id).clone()).collect();
        for class in &classes {
            if let Some(group) = self.by_look.get(class) {
                touched.extend(group.iter().filter(|other| !named.contains(other) && self.is_here(other)).take(3).cloned());
            }
        }
        for other in touched {
            self.refresh(own, mine, &other, changed == Some(other.as_slice()));
        }
    }

    /// Who is in a room changed for `ids` (joined, left, or listed): what is
    /// shown is worked out again for them and their look groups only.
    pub fn members_changed(&mut self, own: &[u8], mine: Option<&str>, ids: &[Vec<u8>]) {
        self.refresh_groups(own, mine, ids, Vec::new(), None);
    }

    /// The nick the hub gave us is no longer known (ours was changed).
    pub fn forget_own_nick(&mut self, own: &[u8]) {
        self.remove_raw(own);
    }

    /// What is shown for `id`, worked out again; if only its marking changed
    /// (not the nick), the recent lines it is on, in the rooms it is in,
    /// follow.
    fn refresh(&mut self, own: &[u8], mine: Option<&str>, id: &[u8], nick_changed: bool) {
        let Some(nick) = self.raw_nicks.get(id).map(|said| said.nick.clone()) else { return };
        let shown = self.shown(own, mine, id, &nick);
        let old = self.nicks.insert(id.to_vec(), shown.clone());
        let Some(old) = old.filter(|old| !nick_changed && *old != shown) else { return };
        let src = hex::encode(id);
        for (room, lines) in &mut self.buffers {
            if !self.members.get(room).is_some_and(|members| members.contains(id)) {
                continue;
            }
            for line in lines.iter_mut().rev().take(REMARK_LINES) {
                if line.src.as_deref() == Some(src.as_str()) && line.nick.as_deref() == Some(old.as_str()) {
                    line.nick = Some(shown.clone());
                }
            }
        }
    }

    /// What is shown for every nick the hub gave, worked out again (when who
    /// is in the rooms, or our nick, changed).
    pub fn remark(&mut self, own: &[u8], mine: Option<&str>) {
        let all: Vec<Vec<u8>> = self.raw_nicks.keys().cloned().collect();
        for id in all {
            self.refresh(own, mine, &id, false);
        }
    }

    /// Keep the tables a hub fills within bounds ([`MAX_NICKS`],
    /// [`MAX_MEMBERS`]), dropping a quarter more than is over, so it isn't
    /// done again at once: nicks of those not in a room first, the oldest
    /// first; members with the oldest nicks first (those never given one
    /// before the rest). We are never dropped.
    pub fn limit_tables(&mut self, own: &[u8]) {
        if self.raw_nicks.len() > MAX_NICKS {
            // Dropped first: those not in a room, then those in one who
            // were never seen speaking, then the rest; the newest first in
            // each, so a flood of new identities removes itself and not
            // the long-standing members it would pass for.
            let mut order: Vec<(u8, std::cmp::Reverse<u64>, Vec<u8>)> = self
                .raw_nicks
                .iter()
                .filter(|(id, _)| !same_identity(id, own))
                .map(|(id, said)| {
                    (
                        if !self.is_here(id) {
                            0
                        } else if !said.spoken {
                            1
                        } else {
                            2
                        },
                        std::cmp::Reverse(said.seq),
                        id.clone(),
                    )
                })
                .collect();
            order.sort();
            for (_, _, id) in order.into_iter().take(self.raw_nicks.len() - MAX_NICKS * 3 / 4) {
                self.remove_raw(&id);
                self.nicks.remove(&id);
            }
        }
        for members in self.members.values_mut() {
            if members.len() > MAX_MEMBERS {
                // Those with no nick first, then those never seen speaking,
                // then the rest, the newest first in each.
                let mut order: Vec<(u8, std::cmp::Reverse<u64>, Vec<u8>)> = members
                    .iter()
                    .filter(|id| !same_identity(id, own))
                    .map(|id| match self.raw_nicks.get(id) {
                        None => (0, std::cmp::Reverse(0), id.clone()),
                        Some(said) => (if said.spoken { 2 } else { 1 }, std::cmp::Reverse(said.seq), id.clone()),
                    })
                    .collect();
                order.sort();
                for (_, _, id) in order.into_iter().take(members.len() - MAX_MEMBERS * 3 / 4) {
                    members.remove(&id);
                }
            }
        }
    }

    /// What a user goes by here: their nick now, else the one on their
    /// latest line (someone who has left), else the start of their identity.
    pub fn display_name(&self, hash: &[u8]) -> String {
        if let Some(nick) = self.nicks.get(hash) {
            return nick.clone();
        }
        let src = hex::encode(hash);
        self.buffers
            .values()
            .flat_map(|lines| lines.iter())
            .filter(|l| l.src.as_deref() == Some(src.as_str()) && !l.own)
            .max_by_key(|l| l.ts)
            .and_then(|l| l.nick.clone())
            .unwrap_or_else(|| self.name_of(hash))
    }

    /// Rooms shown in the list: joined ones plus parted rooms with history.
    pub fn listed_rooms(&self) -> Vec<String> {
        let mut rooms: BTreeSet<String> = self.rooms.clone();
        rooms.extend(self.buffers.keys().filter(|k| !k.is_empty() && whisper_peer(k).is_none()).cloned());
        rooms.into_iter().collect()
    }

    /// The other user's name in a whisper conversation: their nick if known
    /// now, else the one on their latest whisper (nicks aren't saved).
    pub fn whisper_name(&self, key: &str) -> String {
        let Some(peer) = whisper_peer(key) else { return key.to_string() };
        if let Some(nick) = self.nicks.get(&peer) {
            return nick.clone();
        }
        self.buffers
            .get(key)
            .and_then(|lines| lines.iter().rev().find(|l| !l.own).and_then(|l| l.nick.clone()))
            .unwrap_or_else(|| self.name_of(&peer))
    }

    /// Whisper conversations, as (buffer key, the other user's name), by name.
    pub fn whispers(&self) -> Vec<(String, String)> {
        let mut whispers: Vec<(String, String)> =
            self.buffers.keys().filter(|key| whisper_peer(key).is_some()).map(|key| (key.clone(), self.whisper_name(key))).collect();
        // Several people can share a nick: tell them apart by identity.
        let names: Vec<String> = whispers.iter().map(|(_, name)| name.to_lowercase()).collect();
        for (key, name) in &mut whispers {
            if names.iter().filter(|n| **n == name.to_lowercase()).count() > 1 {
                name.push_str(&format!(" ({})", &key[1..7]));
            }
        }
        whispers.sort_by_key(|(_, name)| name.to_lowercase());
        whispers
    }

    /// Add a member of `room` by their full identity, in place of the
    /// start of it (all a hub's `/who` gives for someone with a nick).
    /// Whether they're new here.
    fn add_member(&mut self, room: &str, identity: &[u8]) -> bool {
        let members = self.members.entry(room.to_string()).or_default();
        let partial: Vec<Vec<u8>> = members.iter().filter(|m| m.len() < identity.len() && identity.starts_with(m)).cloned().collect();
        for prefix in &partial {
            members.remove(prefix);
        }
        let new = members.insert(identity.to_vec()) && partial.is_empty();
        for prefix in &partial {
            if let Some(nick) = self.nicks.get(prefix).cloned() {
                self.nicks.entry(identity.to_vec()).or_insert(nick);
            }
            if let Some(said) = self.raw_nicks.get(prefix).cloned()
                && !self.raw_nicks.contains_key(identity)
            {
                self.put_raw(identity, said);
            }
        }
        new
    }

    /// A room's members as (name, identity), sorted by name. Someone may be
    /// known by the start of their identity only (see
    /// [`add_member`](Self::add_member)).
    pub fn members_of(&self, room: &str) -> Vec<(String, Vec<u8>)> {
        let mut members: Vec<(String, Vec<u8>)> =
            self.members.get(room).map(|m| m.iter().map(|h| (self.name_of(h), h.clone())).collect()).unwrap_or_default();
        members.sort_by_key(|(n, _)| n.to_lowercase());
        members
    }

    /// Who can be mentioned in `room`: its members and anyone who has
    /// spoken there, as (name, identity), sorted by name. Someone no
    /// longer here goes by the nick on their latest line. Each name is
    /// listed once (nicks aren't unique): a member first, else whoever
    /// used it last.
    pub fn mentionable(&self, room: &str) -> Vec<(String, Vec<u8>)> {
        let mut people = Vec::new();
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        let mut add = |name: String, id: Vec<u8>| {
            if !ids.contains(&id) && names.insert(name.to_lowercase()) {
                ids.insert(id.clone());
                people.push((name, id));
            }
        };
        for (name, id) in self.members_of(room) {
            add(name, id);
        }
        for line in self.buffers.get(room).map(Vec::as_slice).unwrap_or_default().iter().rev() {
            let Some(id) = line.src.as_deref().and_then(|s| hex::decode(s).ok()) else { continue };
            let name = match (self.nicks.get(&id), &line.nick) {
                (Some(nick), _) | (None, Some(nick)) => nick.clone(),
                (None, None) => self.name_of(&id),
            };
            add(name, id);
        }
        people.sort_by_key(|(n, _)| n.to_lowercase());
        people
    }

    /// Whether `room` is joined, or still has its buffer (left, but kept).
    pub(crate) fn has_room(&self, room: &str) -> bool {
        self.rooms.contains(room) || self.buffers.contains_key(room)
    }

    /// Before a whisper conversation `key` is started: if there are
    /// [`MAX_WHISPERS`] already, the one whose last line is oldest goes
    /// (not `shown`, the one on screen).
    pub(crate) fn make_room_for_whisper(&mut self, key: &str, shown: Option<&str>) {
        if self.buffers.contains_key(key) {
            return;
        }
        let whispers = || self.buffers.iter().filter(|(k, _)| whisper_peer(k).is_some());
        if whispers().count() < MAX_WHISPERS {
            return;
        }
        let last = |lines: &Vec<ChatLine>| lines.last().map_or(0, |l| l.ts);
        let Some(oldest) =
            whispers().filter(|(k, _)| Some(k.as_str()) != shown).min_by_key(|(_, lines)| last(lines)).map(|(k, _)| k.clone())
        else {
            return;
        };
        tracing::debug!("{MAX_WHISPERS} whisper conversations on {}: the quietest, {oldest}, goes", self.name);
        self.buffers.remove(&oldest);
        self.unread.remove(&oldest);
        self.mentions.remove(&oldest);
        // Their nick too, unless they're in a room with us.
        if let Some(peer) = whisper_peer(&oldest)
            && !self.members.values().any(|members| members.contains(&peer))
        {
            self.nicks.remove(&peer);
            self.remove_raw(&peer);
        }
        self.history_dirty = true;
    }

    /// Add a line, the oldest of its sort going past their limit (`keep`
    /// messages, [`MAX_OTHER_LINES`] others).
    fn push(&mut self, room: &str, line: ChatLine, keep: usize) {
        let buffer = self.buffers.entry(room.to_string()).or_default();
        let message = line.is_message();
        buffer.push(line);
        let limit = if message { keep } else { MAX_OTHER_LINES };
        drop_oldest(buffer, message, limit);
        self.history_dirty = true;
    }

    /// At most `keep` messages in each room and whisper conversation (after
    /// "Chat messages kept" went down).
    pub(crate) fn keep_messages(&mut self, keep: usize) {
        for buffer in self.buffers.values_mut() {
            if drop_oldest(buffer, true, keep) {
                self.history_dirty = true;
            }
        }
    }

    fn history_path(&self, dir: &Path) -> PathBuf {
        let mut name = hex::encode(self.hash);
        if self.aspect != rrc::DEFAULT_ASPECT {
            name.push_str("__");
            name.push_str(&hex::encode(&rns_crypto::sha::full_hash(self.aspect.as_bytes())[..4]));
        }
        dir.join(format!("{name}.json"))
    }

    /// Chat lines worth keeping across restarts, as the history file's
    /// contents (encoded later, off the event loop).
    fn history_snapshot(&mut self) -> BTreeMap<String, Vec<ChatLine>> {
        self.history_dirty = false;
        self.buffers
            .iter()
            .filter(|(room, _)| !room.is_empty())
            .map(|(room, lines)| {
                let lines = lines.iter().filter(|l| l.is_message()).cloned().collect();
                (room.clone(), lines)
            })
            .collect()
    }

    fn load_history(&mut self, dir: &Path) {
        let Ok(text) = crate::config::read_limited_text(&self.history_path(dir), MAX_HISTORY_BYTES) else {
            return;
        };
        if let Ok(rooms) = serde_json::from_str::<BTreeMap<String, Vec<ChatLine>>>(&text) {
            for (room, lines) in rooms {
                self.buffers.insert(room, lines);
            }
        }
        self.move_whispers_out_of_rooms();
    }

    /// History from before whispers had their own conversations kept them in
    /// rooms: move them to the conversation with the other user. Received
    /// whispers name their sender; sent ones only the recipient's name, so
    /// they move when that name belongs to someone who whispered to us.
    fn move_whispers_out_of_rooms(&mut self) {
        let is_old_whisper = |key: &str, line: &ChatLine| line.kind == LineKind::Private && whisper_peer(key).is_none();
        let mut senders: HashMap<String, Vec<u8>> = HashMap::new();
        for (key, lines) in &self.buffers {
            for line in lines.iter().filter(|l| is_old_whisper(key, l) && !l.own) {
                if let (Some(nick), Some(src)) = (&line.nick, line.src.as_deref().and_then(|s| hex::decode(s).ok())) {
                    senders.insert(nick.clone(), src);
                }
            }
        }
        let mut moved: Vec<(String, ChatLine)> = Vec::new();
        for (key, lines) in &mut self.buffers {
            if whisper_peer(key).is_some() {
                continue;
            }
            lines.retain(|line| {
                if line.kind != LineKind::Private {
                    return true;
                }
                let peer = if line.own {
                    line.nick.as_ref().and_then(|nick| senders.get(nick).cloned())
                } else {
                    line.src.as_deref().and_then(|s| hex::decode(s).ok())
                };
                let Some(peer) = peer.filter(|p| p.len() == 16) else { return true };
                let mut line = line.clone();
                if line.own {
                    // Sent lines kept the recipient's name; in the
                    // conversation they are ours.
                    line.nick = None;
                }
                moved.push((whisper_key(&peer), line));
                false
            });
        }
        if moved.is_empty() {
            return;
        }
        for (key, line) in moved {
            self.buffers.entry(key).or_default().push(line);
        }
        for (key, lines) in &mut self.buffers {
            if whisper_peer(key).is_some() {
                lines.sort_by_key(|l| l.ts);
            }
        }
        // Parted rooms that held nothing but whispers go from the list.
        let joined = &self.rooms;
        self.buffers.retain(|key, lines| !lines.is_empty() || key.is_empty() || joined.contains(key));
        self.history_dirty = true;
    }
}

/// The oldest lines of a sort (messages, or the others) past `limit`
/// gone. Whether any were.
fn drop_oldest(buffer: &mut Vec<ChatLine>, messages: bool, limit: usize) -> bool {
    let alike = buffer.iter().filter(|l| l.is_message() == messages).count();
    let mut over = alike.saturating_sub(limit);
    if over == 0 {
        return false;
    }
    buffer.retain(|l| {
        let drop = over > 0 && l.is_message() == messages;
        over -= usize::from(drop);
        !drop
    });
    true
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
    /// A line a search found: its hub, buffer and place, marked while its
    /// room is open; and the place to scroll to, once.
    pub found: Option<(Hash, String, usize)>,
    pub jump_to: Option<usize>,
    /// Popups: actions for a user, or picking one of the room's members.
    pub menu: Option<users::UserMenu>,
    pub picker: Option<users::MemberPicker>,
    /// The list `@` opens while typing: which name is chosen, and the `@`
    /// (byte in the input) it was closed for with Esc.
    pub mention_pick: usize,
    pub mention_dismissed: Option<usize>,
    /// Unsent lines of the rooms not on screen, and the room the line in
    /// `input` belongs to (see [`Channels::sync_draft`]).
    drafts: HashMap<String, TextInput>,
    draft_for: Option<String>,
}

#[cfg(test)]
mod draft_tests {
    use super::*;

    #[test]
    fn each_room_keeps_its_own_unsent_line() {
        let mut channels = Channels::default();
        let room = |name: &str| Some(Target { hub: [1; 16], room: Some(name.to_string()) });
        channels.selected = room("general");
        channels.sync_draft();
        channels.input.insert_str("half a thought");
        channels.selected = room("random");
        channels.sync_draft();
        assert_eq!(channels.input.text(), "", "the line doesn't follow to another room");
        channels.input.insert_str("hello random");
        channels.selected = room("general");
        channels.sync_draft();
        assert_eq!(channels.input.text(), "half a thought");
        channels.selected = room("random");
        channels.sync_draft();
        assert_eq!(channels.input.text(), "hello random");
    }
}

impl Channels {
    /// Keep what's written with the room it was written in: once the
    /// selection has moved (it moves from many places), put the line away and
    /// bring back the new room's, rather than carry it over to be sent there.
    pub fn sync_draft(&mut self) {
        let key = self.selected.as_ref().map(|t| format!("{}/{}", hex::encode(t.hub), t.room.as_deref().unwrap_or("")));
        if key == self.draft_for {
            return;
        }
        let line = std::mem::take(&mut self.input);
        if let Some(old) = self.draft_for.take()
            && !line.text().is_empty()
        {
            self.drafts.insert(old, line);
        }
        self.input = key.as_ref().and_then(|k| self.drafts.remove(k)).unwrap_or_default();
        self.draft_for = key;
        self.mention_pick = 0;
        self.mention_dismissed = None;
    }
}

/// A row of the Channels list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Hub(usize),
    Room(usize, String),
    /// A whisper conversation: hub, buffer key (see [`whisper_key`]).
    Whisper(usize, String),
}

impl Channels {
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (i, hub) in self.hubs.iter().enumerate() {
            rows.push(Row::Hub(i));
            rows.extend(hub.listed_rooms().into_iter().map(|r| Row::Room(i, r)));
            rows.extend(hub.whispers().into_iter().map(|(key, _)| Row::Whisper(i, key)));
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
        // A line found is marked until another room is picked.
        self.found = None;
        self.jump_to = None;
        self.selected = Some(match row {
            Row::Hub(i) => Target { hub: self.hubs[*i].hash, room: None },
            Row::Room(i, room) | Row::Whisper(i, room) => Target { hub: self.hubs[*i].hash, room: Some(room.clone()) },
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
            hub.notify = config.notify;
            hub.room_notify = config.room_notify.clone();
            hub.rooms = config.rooms.iter().map(|r| rrc::normalize_room(r)).collect();
            hub.keys = config.room_keys.clone();
            hub.load_history(history_dir);
            channels.hubs.push(hub);
        }
        if let Some(first) = channels.hubs.first() {
            channels.selected = Some(Target { hub: first.hash, room: first.rooms.iter().next().cloned() });
        }
        channels
    }
}

/// How long before trying a hub again: soon at first, then less and less
/// often, up to every 15 minutes (each try asks the network for a path and
/// a Link, which keeps a phone's radio awake).
fn backoff(attempts: u32) -> Duration {
    Duration::from_secs(2u64.saturating_pow(attempts.min(10)).clamp(2, 15 * 60))
}

impl App {
    fn hub_mut(&mut self, index: usize) -> &mut Hub {
        &mut self.channels.hubs[index]
    }

    fn effective_nick(&self, index: usize) -> Option<String> {
        let hub = &self.channels.hubs[index];
        match &hub.nick {
            Some(nick) => rrc::normalize_nick(nick, hub.limits.max_nick_bytes),
            // A display name longer than the hub allows is cut to fit, not dropped.
            None => rrc::nick_from_name(&self.settings.display_name, hub.limits.max_nick_bytes),
        }
    }

    /// Save hub list changes into the store.
    fn save_hubs(&mut self) {
        self.store.rrc_hubs = self.channels.hubs.iter().map(Hub::config).collect();
        self.store_dirty = true;
    }

    /// Queue the chat history of hubs with new lines for saving.
    pub fn save_rrc_history(&mut self) {
        let dir = self.paths.rrc_history.clone();
        for hub in self.channels.hubs.iter_mut().filter(|h| h.history_dirty) {
            let keep = hub.history_snapshot();
            let dir = dir.clone();
            self.saver.write(hub.history_path(&dir), format!("chat history for {}", hub.name), move || {
                std::fs::create_dir_all(&dir)?;
                Ok(serde_json::to_vec(&keep)?)
            });
        }
    }

    /// Whether a buffer is on screen right now, in a window that has the
    /// focus (what arrives while the user is away counts as unread).
    fn is_viewing(&self, index: usize, room: &str) -> bool {
        self.focused && self.tab == Tab::Channels && self.channels.active().is_some_and(|(i, r)| i == index && r == room)
    }

    /// Add a line to a buffer, counting it unread unless it is on screen.
    fn record(&mut self, index: usize, room: &str, line: ChatLine) {
        self.record_line(index, room, line, true);
    }

    /// [`record`](Self::record), with or without a notification (none for
    /// history a hub replays).
    fn record_line(&mut self, index: usize, room: &str, line: ChatLine, notify: bool) {
        let counts = !line.own && matches!(line.kind, LineKind::Msg | LineKind::Action | LineKind::Private);
        let mention = line.mention || line.kind == LineKind::Private;
        let viewing = self.is_viewing(index, room);
        let notification =
            if notify && counts && self.settings.notify_rrc { self.rrc_notification(index, room, &line, mention) } else { None };
        let keep = self.settings.rrc_messages_kept as usize;
        let hub = self.hub_mut(index);
        hub.push(room, line, keep);
        if counts && !viewing {
            *hub.unread.entry(room.to_string()).or_default() += 1;
            if mention {
                hub.mentions.insert(room.to_string());
            }
        }
        if let Some(notification) = notification {
            self.push_notification(viewing, notification);
        }
    }

    /// A notification for someone else's line, if the room's level asks for
    /// one (whispers count as mentions).
    fn rrc_notification(&self, index: usize, room: &str, line: &ChatLine, mention: bool) -> Option<Notification> {
        let hub = &self.channels.hubs[index];
        match hub.notify_level(room) {
            NotifyLevel::All => {}
            NotifyLevel::Mentions if mention => {}
            _ => return None,
        }
        let name = line
            .nick
            .clone()
            .or_else(|| line.src.as_deref().and_then(|s| hex::decode(s).ok()).map(|h| hub.name_of(&h)))
            .unwrap_or_else(|| "Someone".to_string());
        let text = if line.kind == LineKind::Action { format!("* {name} {}", line.text) } else { line.text.clone() };
        let title = if whisper_peer(room).is_some() {
            format!("{name} (whisper)")
        } else if room.is_empty() {
            format!("{name} on {}", hub.hub_name.as_deref().unwrap_or(&hub.name))
        } else if line.kind == LineKind::Action {
            format!("#{room}")
        } else {
            format!("{name} in #{room}")
        };
        let target = Opens::Room { hub: hex::encode(hub.hash), room: room.to_string() };
        Some(Notification::new(title, notify::body(&text), target))
    }

    /// Where a line about `room` goes: there, if it's a room we have, else
    /// where a reply that names no room goes (a hub could otherwise add any
    /// number of rooms).
    fn room_or_reply(&self, index: usize, room: Option<String>) -> String {
        match room {
            Some(room) if self.channels.hubs[index].has_room(&room) => room,
            _ => self.reply_room(index),
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
        let had = hub.unread.remove(room).is_some_and(|n| n > 0);
        hub.mentions.remove(room);
        if had {
            let target = crate::app::notify::Target::Room { hub: hex::encode(hub.hash), room: room.to_string() };
            self.read(target);
        }
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
        let command = NetCommand::RrcConnect { hub: hub.hash, aspect: hub.aspect.clone(), nick, key: hub.local_key };
        self.send(command);
    }

    /// The hub hosted here is running: in the hub list (added the first
    /// time), reached with its key, and connected unless it's set not to be.
    pub(crate) fn connect_own_hub(&mut self, hash: Hash, key: [u8; 64], name: &str) {
        let index = match self.channels.hubs.iter().position(|h| h.hash == hash && h.aspect == rrc::DEFAULT_ASPECT) {
            Some(index) => index,
            None => {
                self.channels.hubs.push(Hub::new(hash, rrc::DEFAULT_ASPECT.into(), name.to_string()));
                self.save_hubs();
                self.channels.hubs.len() - 1
            }
        };
        // Connected before a Reticulum restart (auto or not): again now.
        let rejoin = self.rejoin_hubs.contains(&hash);
        self.rejoin_hubs.retain(|h| *h != hash);
        let hub = self.hub_mut(index);
        hub.local_key = Some(key);
        if (rejoin || hub.auto_connect && !hub.manual) && !matches!(hub.status, HubStatus::Connected | HubStatus::Connecting(_)) {
            hub.reconnect_at = None;
            hub.status = HubStatus::Disconnected;
            self.connect_hub(index);
        }
    }

    /// The hub hosted here has stopped: leave it, without trying again until
    /// it's back.
    pub(crate) fn release_own_hub(&mut self, hash: Hash) {
        let Some(index) = self.channels.hubs.iter().position(|h| h.hash == hash && h.aspect == rrc::DEFAULT_ASPECT) else { return };
        let hub = self.hub_mut(index);
        hub.reconnect_at = None;
        if matches!(hub.status, HubStatus::Connected | HubStatus::Connecting(_)) {
            hub.status = HubStatus::Disconnected;
            self.session(index, SessionCommand::Disconnect);
        }
    }

    fn disconnect_hub(&mut self, index: usize) {
        // Not rejoined later either (see connect_own_hub).
        let hash = self.channels.hubs[index].hash;
        self.rejoin_hubs.retain(|h| *h != hash);
        let hub = self.hub_mut(index);
        hub.manual = true;
        hub.reconnect_at = None;
        hub.attempts = 0;
        hub.status = HubStatus::Disconnected;
        self.session(index, SessionCommand::Disconnect);
    }

    /// Connect auto-connect hubs once the network is up.
    /// Before Reticulum restarts: every hub session ends with the old stack.
    /// Returns the hubs that were connected (or connecting), to reconnect
    /// once the new stack is up, and marks them all disconnected.
    pub(crate) fn channels_before_restart(&mut self) -> Vec<Hash> {
        let mut rejoin = Vec::new();
        for hub in &mut self.channels.hubs {
            if matches!(hub.status, HubStatus::Connected | HubStatus::Connecting(_)) || hub.reconnect_at.is_some() {
                rejoin.push(hub.hash);
            }
            hub.status = HubStatus::Disconnected;
            hub.reconnect_at = None;
            hub.attempts = 0;
            hub.members.clear();
        }
        rejoin
    }

    pub fn start_channels(&mut self) {
        for index in 0..self.channels.hubs.len() {
            // The hub hosted here, once it's up (see connect_own_hub).
            let hub = &self.channels.hubs[index];
            if hub.auto_connect && !self.own_hub_waiting(hub.hash) {
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
            // The hub hosted here, since stopped: connect_own_hub connects
            // once it's back.
            if self.own_hub_waiting(self.channels.hubs[index].hash) {
                self.hub_mut(index).reconnect_at = None;
                continue;
            }
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
        // Hubs want a keyed room's key every time: keep it for rejoining.
        if let Some(key) = key.filter(|k| !k.is_empty()) {
            hub.keys.insert(room.clone(), key.to_string());
        }
        let key = hub.keys.get(&room).cloned();
        if hub.is_connected() {
            hub.pending_joins.insert(room.clone());
            if silent {
                hub.silent_joins.insert(room.clone());
                hub.quiet_info.insert(room.clone());
            }
            let mut env = self.envelope(index, t::JOIN).room(&room);
            if let Some(key) = key {
                env = env.text(&key);
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
        hub.keys.remove(&room);
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
        assert_eq!(backoff(6), Duration::from_secs(64));
        assert_eq!(backoff(20), Duration::from_secs(15 * 60));
    }

    #[test]
    fn a_history_file_of_far_more_than_history_is_not_read() {
        let dir = std::env::temp_dir().join(format!("rettui-rrc-history-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let load = || {
            let mut hub = Hub::new([1; 16], rrc::DEFAULT_ASPECT.into(), "Hub".into());
            hub.load_history(&dir);
            hub
        };
        let path = load().history_path(&dir);
        let line = r#"{"kind":"Msg","src":null,"nick":"amy","text":"hi","ts":1}"#;
        let good = format!(r#"{{"general":[{line}]}}"#);
        std::fs::write(&path, &good).unwrap();
        assert_eq!(load().buffers["general"].len(), 1, "history that fits is read");
        std::fs::write(&path, format!("{good}{}", " ".repeat(MAX_HISTORY_BYTES as usize))).unwrap();
        assert!(!load().buffers.contains_key("general"), "history over the limit isn't");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rows_list_hubs_then_rooms() {
        let mut channels = Channels::default();
        let mut hub = Hub::new([1; 16], rrc::DEFAULT_ASPECT.into(), "Hub".into());
        hub.rooms.insert("b".into());
        hub.buffers.insert("a".into(), Vec::new()); // parted, with history
        channels.hubs.push(hub);
        assert_eq!(channels.rows(), vec![Row::Hub(0), Row::Room(0, "a".into()), Row::Room(0, "b".into())]);
    }

    #[test]
    fn whispers_are_their_own_rows() {
        let (zed, amy) = ([0xaa; 16], [0x11; 16]);
        assert_eq!(whisper_peer(&whisper_key(&zed)), Some(zed.to_vec()));
        assert_eq!(whisper_peer("general"), None);
        assert_eq!(whisper_peer("@short"), None);
        let mut channels = Channels::default();
        let mut hub = Hub::new([1; 16], rrc::DEFAULT_ASPECT.into(), "Hub".into());
        hub.rooms.insert("general".into());
        hub.nicks.insert(zed.to_vec(), "zed".into());
        hub.nicks.insert(amy.to_vec(), "Amy".into());
        hub.buffers.insert(whisper_key(&zed), Vec::new());
        hub.buffers.insert(whisper_key(&amy), Vec::new());
        channels.hubs.push(hub);
        // Rooms first, then whispers by name; whispers are never rooms.
        assert_eq!(
            channels.rows(),
            vec![Row::Hub(0), Row::Room(0, "general".into()), Row::Whisper(0, whisper_key(&amy)), Row::Whisper(0, whisper_key(&zed)),]
        );
        assert_eq!(channels.hubs[0].listed_rooms(), ["general"]);
    }

    #[test]
    fn mentionable_people_are_listed_once_each() {
        let (bot, old_bot, amy, gone) = ([0xb0; 16], [0xb1; 16], [0x11; 16], [0x22; 16]);
        let mut hub = Hub::new([1; 16], rrc::DEFAULT_ASPECT.into(), "Hub".into());
        hub.members.insert("general".into(), [bot.to_vec(), amy.to_vec()].into());
        hub.nicks.insert(bot.to_vec(), "pybot".into());
        hub.nicks.insert(amy.to_vec(), "Amy".into());
        let said = |id: [u8; 16], nick: &str, text: &str| {
            let mut line = ChatLine::new(LineKind::Msg, text);
            line.src = Some(hex::encode(id));
            line.nick = Some(nick.into());
            line
        };
        // An earlier pybot (another identity, same nick) and someone who left.
        hub.buffers.insert(
            "general".into(),
            vec![said(old_bot, "PyBot", "old"), said(gone, "zed", "bye"), said(amy, "Amy", "hi @pybot and @zed")],
        );
        let people = hub.mentionable("general");
        let names: Vec<&str> = people.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Amy", "pybot", "zed"]);
        // The member keeps the name, so a mention is theirs.
        assert_eq!(people[1].1, bot.to_vec());
        let line = &hub.buffers["general"][2];
        let mentions: Vec<(usize, usize, Vec<u8>)> =
            line.user_mentions(&people, &[]).into_iter().map(|(s, e, id)| (s, e, id.to_vec())).collect();
        assert_eq!(mentions, [(3, 9, bot.to_vec()), (14, 18, gone.to_vec())]);
        // A mention of us (highlighted another way) isn't coloured as well.
        assert_eq!(line.user_mentions(&people, &[(3, 9)]).len(), 1);
    }

    #[test]
    fn comings_and_goings_never_push_messages_out() {
        let mut hub = Hub::new([1; 16], rrc::DEFAULT_ASPECT.into(), "Hub".into());
        // A busy room: a join and a leave for every message.
        const MAX_MESSAGES: usize = 500;
        for i in 0..MAX_MESSAGES + 50 {
            hub.push("lobby", ChatLine::presence(format!("amy joined {i}")), MAX_MESSAGES);
            hub.push("lobby", ChatLine::new(LineKind::Msg, format!("message {i}")), MAX_MESSAGES);
            hub.push("lobby", ChatLine::presence(format!("amy left {i}")), MAX_MESSAGES);
        }
        let lines = &hub.buffers["lobby"];
        let messages: Vec<&str> = lines.iter().filter(|l| l.is_message()).map(|l| l.text.as_str()).collect();
        assert_eq!(messages.len(), MAX_MESSAGES);
        assert_eq!((messages[0], *messages.last().unwrap()), ("message 50", "message 549"));
        assert_eq!(lines.iter().filter(|l| !l.is_message()).count(), MAX_OTHER_LINES);
        assert_eq!(lines.last().unwrap().text, "amy left 549");
        // All of them are kept across restarts.
        assert_eq!(hub.history_snapshot()["lobby"].len(), MAX_MESSAGES);
        // Fewer kept from now on: the oldest go, the other lines stay.
        hub.keep_messages(100);
        let lines = &hub.buffers["lobby"];
        assert_eq!(lines.iter().filter(|l| l.is_message()).count(), 100);
        assert_eq!(lines.iter().find(|l| l.is_message()).unwrap().text, "message 450");
        assert_eq!(lines.iter().filter(|l| !l.is_message()).count(), MAX_OTHER_LINES);
        assert!(hub.history_dirty);
    }

    #[test]
    fn old_whispers_move_out_of_rooms() {
        let zed = [0xaa; 16];
        let mut hub = Hub::new([1; 16], rrc::DEFAULT_ASPECT.into(), "Hub".into());
        hub.rooms.insert("general".into());
        let mut incoming = ChatLine::new(LineKind::Private, "psst");
        incoming.src = Some(hex::encode(zed));
        incoming.nick = Some("zed".into());
        incoming.ts = 1;
        let mut sent = ChatLine::new(LineKind::Private, "hi zed");
        sent.nick = Some("zed".into());
        sent.own = true;
        sent.ts = 2;
        let mut unknown = ChatLine::new(LineKind::Private, "hi stranger");
        unknown.nick = Some("stranger".into());
        unknown.own = true;
        let chat = ChatLine::new(LineKind::Msg, "hello room");
        hub.buffers.insert("general".into(), vec![chat, incoming]);
        hub.buffers.insert("parted".into(), vec![sent, unknown]);
        hub.move_whispers_out_of_rooms();
        let texts = |key: &str| hub.buffers.get(key).map(|l| l.iter().map(|l| l.text.clone()).collect::<Vec<_>>());
        assert_eq!(texts(&whisper_key(&zed)), Some(vec!["psst".into(), "hi zed".into()]));
        assert_eq!(texts("general"), Some(vec!["hello room".into()]));
        // A sent whisper whose recipient can't be told stays where it was.
        assert_eq!(texts("parted"), Some(vec!["hi stranger".into()]));
        assert!(hub.history_dirty);
    }

    #[test]
    fn saved_hubs_and_their_history_with_old_whispers_moved_out() {
        let dir = std::env::temp_dir().join(format!("rettui-channels-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let carol = "cc".repeat(16);
        let config: crate::store::HubConfig = serde_json::from_value(serde_json::json!({
            "hash": "01".repeat(16), "aspect": "rrc.other", "name": "Hilltop", "rooms": ["Lobby"], "nick": "me",
            "auto_connect": true, "room_keys": { "lobby": "k" },
        }))
        .unwrap();
        let bad: crate::store::HubConfig =
            serde_json::from_value(serde_json::json!({ "hash": "nope", "aspect": "rrc", "name": "x", "rooms": [], "auto_connect": false }))
                .unwrap();
        let history = serde_json::json!({
            "lobby": [
                { "kind": "Msg", "src": carol, "nick": "carol", "text": "hi", "ts": 1 },
                { "kind": "Private", "src": carol, "nick": "carol", "text": "psst", "ts": 2 },
                { "kind": "Private", "nick": "carol", "text": "back at you", "ts": 3, "own": true },
                { "kind": "Private", "nick": "stranger", "text": "to someone unknown", "ts": 4, "own": true },
            ],
        });
        let name = Hub::new([1; 16], "rrc.other".into(), String::new()).history_path(&dir);
        std::fs::write(&name, history.to_string()).unwrap();
        let mut channels = Channels::load(&[bad, config], &dir);
        assert_eq!(channels.hubs.len(), 1);
        let hub = &channels.hubs[0];
        let whisper = format!("@{carol}");
        assert_eq!(hub.buffers[&whisper].len(), 2, "{:?}", hub.buffers.keys());
        assert_eq!(hub.buffers["lobby"].len(), 2);
        assert_eq!(channels.selected.as_ref().map(|t| t.room.clone()), Some(Some("lobby".into())));
        // Names: from a line once they've gone, and told apart when two
        // whisperers share one.
        let carol_id = hex::decode(&carol).unwrap();
        assert_eq!(hub.display_name(&carol_id), "carol");
        assert_eq!(hub.whisper_name(&whisper), "carol");
        assert_eq!(hub.whisper_name("lobby"), "lobby");
        let hub = &mut channels.hubs[0];
        let other = format!("@{}", "dd".repeat(16));
        hub.buffers.insert(
            other.clone(),
            serde_json::from_value(serde_json::json!([{ "kind": "Private", "nick": "carol", "text": "me too", "ts": 5 }])).unwrap(),
        );
        let names: Vec<String> = hub.whispers().into_iter().map(|(_, name)| name).collect();
        assert!(names.iter().all(|n| n.starts_with("carol (")), "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notifications_as_each_level_and_place_has_them() {
        let dir = std::env::temp_dir().join(format!("rettui-channels-notify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        let mut hub = Hub::new([1; 16], rrc::DEFAULT_ASPECT.into(), "Hilltop".into());
        hub.notify = Some(NotifyLevel::All);
        app.channels.hubs.push(hub);
        let line = |kind: LineKind, nick: Option<&str>| {
            let mut line = ChatLine::new(kind, "hello");
            line.nick = nick.map(str::to_string);
            line.src = Some("cc".repeat(16));
            line
        };
        let titles: Vec<String> = [
            ("lobby", line(LineKind::Msg, Some("carol"))),
            ("lobby", line(LineKind::Action, None)),
            ("", line(LineKind::Notice, Some("carol"))),
            (format!("@{}", "cc".repeat(16)).as_str(), line(LineKind::Private, Some("carol"))),
        ]
        .iter()
        .map(|(room, line)| app.rrc_notification(0, room, line, false).unwrap().title)
        .collect();
        assert_eq!(titles, ["carol in #lobby", "#lobby", "carol on Hilltop", "carol (whisper)"]);
        app.channels.hubs[0].notify = Some(NotifyLevel::Off);
        assert!(app.rrc_notification(0, "lobby", &line(LineKind::Msg, None), true).is_none());
        // A reconnect due: tried again.
        app.channels.hubs[0].reconnect_at = Some(Instant::now());
        app.channels_tick();
        assert!(app.channels.hubs[0].reconnect_at.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
