//! Hub tab: the RRC hub hosted here (rsRRCD's, see [`crate::rrc::host`]).
//! Starting and stopping it with the settings, your own client on it, and
//! the panel for managing it: who's connected, the rooms, the bans.
//!
//! The actions (`hub_*`) are shared by the TUI and the web UI. Most are hub
//! commands, run by the hub itself (see [`HubAction::Run`]), so they do
//! what the same command typed in a room does, and the hub says how it
//! went.

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::node::NodeStatus;
use super::{App, PromptKind};
use crate::config::Paths;
use crate::net::{Hash, NetCommand};
use crate::rrc::host::{self, HubAction, HubEvent, HubHostConfig, HubSnapshot};

/// Replies kept for the panel.
const REPLIES: usize = 50;

/// What the panel's list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubPane {
    People,
    Rooms,
    Bans,
}

impl HubPane {
    pub const ALL: [HubPane; 3] = [HubPane::People, HubPane::Rooms, HubPane::Bans];

    pub fn title(self) -> &'static str {
        match self {
            HubPane::People => "People",
            HubPane::Rooms => "Rooms",
            HubPane::Bans => "Bans",
        }
    }
}

/// A ban, as the Bans list has them: from the whole hub, or from a room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ban {
    pub room: Option<String>,
    /// Their identity (hex).
    pub identity: String,
}

/// What a prompt in the panel asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubPrompt {
    /// A hub command, as typed in a room (`/stats`, `/kline list`).
    Command,
    Topic(String),
    Modes(String),
    NewRoom,
    /// The room to kick someone (their identity) from, to ban them from, or
    /// to make them an operator in, or voiced (or not).
    Kick(String),
    Ban(String),
    Op(String),
    Voice(String),
    /// Ban someone from the whole hub.
    ConfirmKline(String),
}

/// The hub hosted here, as the app knows it.
pub struct HubHost {
    pub status: NodeStatus,
    /// Its address and key, once it has an identity (after it first
    /// starts).
    pub address: Option<(Hash, [u8; 64])>,
    pub snapshot: HubSnapshot,
    /// What the network actor was last asked to run.
    config: Option<HubHostConfig>,
    pub pane: HubPane,
    pub people: ListState,
    pub rooms: ListState,
    pub bans: ListState,
    /// What commands from the panel came to, newest last.
    pub replies: VecDeque<(String, bool)>,
    /// How many replies there have been (the web UI says new ones).
    pub replied: u64,
}

impl HubHost {
    pub fn new(paths: &Paths, config: Option<HubHostConfig>) -> Self {
        Self {
            status: if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off },
            address: host::address(&paths.rrc_hub_identity),
            snapshot: HubSnapshot::default(),
            config,
            pane: HubPane::People,
            people: ListState::default().with_selected(Some(0)),
            rooms: ListState::default().with_selected(Some(0)),
            bans: ListState::default().with_selected(Some(0)),
            replies: VecDeque::new(),
            replied: 0,
        }
    }

    /// After a Reticulum restart, it starts again (with the settings).
    pub(super) fn restarting(&mut self, config: Option<HubHostConfig>) {
        self.status = if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off };
        self.snapshot = HubSnapshot::default();
        self.config = config;
    }

    pub fn running(&self) -> bool {
        self.status == NodeStatus::Running
    }

    /// The name it runs with (or would).
    pub fn name(&self) -> Option<&str> {
        self.config.as_ref().map(|c| c.name.as_str())
    }

    /// Its `rrc://` link, for sharing.
    pub fn link(&self) -> Option<String> {
        self.address.map(|(hash, _)| format!("rrc://{}", hex::encode(hash)))
    }

    /// Bans: the whole hub's first, then each room's.
    pub fn bans(&self) -> Vec<Ban> {
        let hub = self.snapshot.klines.iter().map(|identity| Ban { room: None, identity: identity.clone() });
        let rooms = self
            .snapshot
            .rooms
            .iter()
            .flat_map(|room| room.banned.iter().map(|identity| Ban { room: Some(room.name.clone()), identity: identity.clone() }));
        hub.chain(rooms).collect()
    }

    /// The name someone goes by on the hub: their nick, else the start of
    /// their identity.
    pub fn name_of(&self, identity: &str) -> String {
        self.snapshot
            .people
            .iter()
            .find(|p| p.identity == identity)
            .and_then(|p| p.nick.clone())
            .unwrap_or_else(|| identity.chars().take(12).collect())
    }

    pub fn list(&mut self) -> &mut ListState {
        match self.pane {
            HubPane::People => &mut self.people,
            HubPane::Rooms => &mut self.rooms,
            HubPane::Bans => &mut self.bans,
        }
    }

    pub fn len(&self) -> usize {
        match self.pane {
            HubPane::People => self.snapshot.people.len(),
            HubPane::Rooms => self.snapshot.rooms.len(),
            HubPane::Bans => self.bans().len(),
        }
    }

    fn selected(&self, list: &ListState, len: usize) -> Option<usize> {
        list.selected().filter(|&i| i < len)
    }

    /// The person selected in the People list (their identity).
    pub fn person(&self) -> Option<&host::HubPerson> {
        self.snapshot.people.get(self.selected(&self.people, self.snapshot.people.len())?)
    }

    pub fn room(&self) -> Option<&host::HubRoom> {
        self.snapshot.rooms.get(self.selected(&self.rooms, self.snapshot.rooms.len())?)
    }

    pub fn ban(&self) -> Option<Ban> {
        let bans = self.bans();
        let i = self.selected(&self.bans, bans.len())?;
        bans.into_iter().nth(i)
    }

    /// Keep the selections in their lists as they change.
    fn clamp(&mut self) {
        let lens = [self.snapshot.people.len(), self.snapshot.rooms.len(), self.bans().len()];
        for (list, len) in [&mut self.people, &mut self.rooms, &mut self.bans].into_iter().zip(lens) {
            if list.selected().is_none_or(|i| i >= len) {
                list.select(Some(len.saturating_sub(1)));
            }
        }
    }
}

/// A room name for a command: one word (rsRRCD reads up to a space).
fn one_word(room: &str) -> Result<&str, &'static str> {
    let room = room.trim().trim_start_matches('#');
    match room.split_whitespace().count() {
        0 => Err("Which room?"),
        1 => Ok(room),
        _ => Err("Room names are one word"),
    }
}

impl App {
    /// Start, restart or stop the hub to match the settings (nothing, if
    /// what it runs with didn't change).
    pub(super) fn apply_hub_settings(&mut self) {
        let config = HubHostConfig::from_settings(&self.settings, &self.paths, self.identity_hash);
        if config == self.hub.config {
            return;
        }
        // Changed as it runs (see HubHostConfig::same_hub), else restarted.
        let in_place = self.hub.running() && matches!((&config, &self.hub.config), (Some(new), Some(old)) if new.same_hub(old));
        if !in_place {
            self.hub.status = if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off };
        }
        if config.is_none() {
            self.hub.snapshot = HubSnapshot::default();
        }
        self.hub.config = config.clone();
        self.send(NetCommand::Hub(config));
    }

    pub(super) fn on_hub(&mut self, event: HubEvent) {
        match event {
            HubEvent::Started { hash, public_key } => {
                self.hub.status = NodeStatus::Running;
                self.hub.address = Some((hash, public_key));
                self.log(format!("Hosting RRC hub {}", hex::encode(hash)));
                // Yours to chat in, as anyone's is.
                let name = self.hub.name().unwrap_or(&self.settings.display_name).to_string();
                self.connect_own_hub(hash, public_key, &name);
            }
            HubEvent::Stopped => {
                self.hub.status = NodeStatus::Off;
                self.hub.snapshot = HubSnapshot::default();
                if let Some((hash, _)) = self.hub.address {
                    self.release_own_hub(hash);
                }
                self.log("Stopped hosting the RRC hub");
            }
            HubEvent::Failed(e) => {
                self.log(format!("RRC hub: {e}"));
                self.hub.status = NodeStatus::Failed(e);
            }
            // The last of a hub that has since stopped: not shown.
            HubEvent::State(_) if self.hub.status == NodeStatus::Off => {}
            HubEvent::State(snapshot) => {
                self.hub.snapshot = snapshot;
                self.hub.clamp();
            }
            HubEvent::Reply { text, error } => {
                self.hub.replies.push_back((text.clone(), error));
                self.hub.replied += 1;
                if self.hub.replies.len() > REPLIES {
                    self.hub.replies.pop_front();
                }
                // Several lines (`/stats`): the first, in the footer.
                let first = text.lines().next().unwrap_or_default().to_string();
                if error { self.warn(first) } else { self.confirm(first) }
            }
        }
    }

    /// The hub hosted here, not running (yet): your client connects to it
    /// once it's up, not before.
    pub(crate) fn own_hub_waiting(&self, hash: Hash) -> bool {
        self.hub.address.is_some_and(|(own, _)| own == hash) && !self.hub.running()
    }

    /// Something for the hub to do, if it's running.
    pub fn hub_act(&mut self, action: HubAction) -> Result<(), String> {
        if !self.hub.running() {
            return Err("The hub isn't running (turn it on with e, or Host an RRC hub in the settings)".into());
        }
        self.send(NetCommand::HubAction(action));
        Ok(())
    }

    /// A hub command, as typed in a room (a `/` added if it's missing).
    pub fn hub_run(&mut self, command: &str) -> Result<(), String> {
        let command = command.trim();
        if command.is_empty() {
            return Err("Nothing to run".into());
        }
        let command = if command.starts_with('/') { command.to_string() } else { format!("/{command}") };
        self.hub_act(HubAction::Run(command))
    }

    pub fn hub_kick(&mut self, room: &str, identity: &str) -> Result<(), String> {
        let room = one_word(room)?;
        self.hub_run(&format!("/kick {room} {identity}"))
    }

    /// Ban someone from a room (`None`: from the whole hub), or lift it.
    pub fn hub_ban(&mut self, room: Option<&str>, identity: &str, on: bool) -> Result<(), String> {
        let how = if on { "add" } else { "del" };
        match room {
            Some(room) => {
                let room = one_word(room)?;
                self.hub_run(&format!("/ban {room} {how} {identity}"))
            }
            None => self.hub_run(&format!("/kline {how} {identity}")),
        }
    }

    pub fn hub_op(&mut self, room: &str, identity: &str, on: bool) -> Result<(), String> {
        let room = one_word(room)?;
        self.hub_run(&format!("/{} {room} {identity}", if on { "op" } else { "deop" }))
    }

    pub fn hub_voice(&mut self, room: &str, identity: &str, on: bool) -> Result<(), String> {
        let room = one_word(room)?;
        self.hub_run(&format!("/{} {room} {identity}", if on { "voice" } else { "devoice" }))
    }

    pub fn hub_topic(&mut self, room: &str, topic: &str) -> Result<(), String> {
        let room = one_word(room)?;
        if topic.trim().is_empty() {
            return Err("A topic, please (rsRRCD can't clear one)".into());
        }
        self.hub_run(&format!("/topic {room} {}", topic.trim()))
    }

    /// Room modes, as `/mode` takes them: `+m`, `-i`, `+k key`... several
    /// at once, separated by spaces (a key follows its `+k`).
    pub fn hub_modes(&mut self, room: &str, modes: &str) -> Result<(), String> {
        let room = one_word(room)?.to_string();
        let words: Vec<&str> = modes.split_whitespace().collect();
        if words.is_empty() {
            return Err("Modes, such as +m or -i (+k key sets a key)".into());
        }
        let mut i = 0;
        while i < words.len() {
            let flag = words[i];
            if !(flag.starts_with('+') || flag.starts_with('-')) || flag.len() != 2 {
                return Err(format!("{flag} isn't a mode (+m, -i, +k key...)"));
            }
            if flag.eq_ignore_ascii_case("+k") {
                let Some(key) = words.get(i + 1) else { return Err("+k takes a key".into()) };
                self.hub_run(&format!("/mode {room} +k {key}"))?;
                i += 2;
            } else {
                self.hub_run(&format!("/mode {room} {flag}"))?;
                i += 1;
            }
        }
        Ok(())
    }

    pub fn hub_register(&mut self, room: &str, on: bool) -> Result<(), String> {
        let room = one_word(room)?.to_string();
        self.hub_act(HubAction::Register { room, on })
    }

    pub fn hub_disconnect(&mut self, identity: &str) -> Result<(), String> {
        let identity: Hash = hex::decode(identity).ok().and_then(|id| id.try_into().ok()).ok_or("Not an identity")?;
        self.hub_act(HubAction::Disconnect(identity))
    }

    pub fn hub_announce(&mut self) -> Result<(), String> {
        self.hub_act(HubAction::Announce)
    }

    /// Turn hosting on or off (the *Host an RRC hub* setting).
    pub fn hub_toggle(&mut self) {
        let on = !self.settings.hub_enabled;
        match self.update_settings(&[("hub_enabled", if on { "true" } else { "false" })]) {
            Ok(_) => self.confirm(if on { "Starting the hub" } else { "Stopping the hub" }),
            Err(e) => self.fail(e),
        }
    }

    fn hub_result(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.warn(e);
        }
    }

    pub(super) fn hub_prompt(&mut self, prompt: HubPrompt, text: &str) {
        let yes = text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes");
        let result = match prompt {
            _ if text.is_empty() || text == "/" => Ok(()),
            HubPrompt::Command => self.hub_run(text),
            HubPrompt::Topic(room) => self.hub_topic(&room, text),
            HubPrompt::Modes(room) => self.hub_modes(&room, text),
            HubPrompt::NewRoom => self.hub_register(text, true),
            HubPrompt::Kick(identity) => self.hub_kick(text, &identity),
            HubPrompt::Ban(identity) => self.hub_ban(Some(text), &identity, true),
            HubPrompt::Op(identity) => {
                // Make them an operator, or no longer one if they are.
                let room = text.trim().trim_start_matches('#').to_lowercase();
                let is_op = self.hub.snapshot.rooms.iter().any(|r| r.name == room && r.operators.contains(&identity));
                self.hub_op(&room, &identity, !is_op)
            }
            HubPrompt::Voice(identity) => {
                let room = text.trim().trim_start_matches('#').to_lowercase();
                let voiced = self.hub.snapshot.rooms.iter().any(|r| r.name == room && r.voiced.contains(&identity));
                self.hub_voice(&room, &identity, !voiced)
            }
            HubPrompt::ConfirmKline(identity) if yes => self.hub_ban(None, &identity, true),
            HubPrompt::ConfirmKline(_) => Ok(()),
        };
        self.hub_result(result);
    }

    /// Ask for a room for an action on someone: the one they're in, if
    /// just one, already filled in.
    fn ask_room(&mut self, prompt: HubPrompt, verb: &str, person: &host::HubPerson) {
        let name = person.nick.clone().unwrap_or_else(|| person.identity.chars().take(12).collect());
        let rooms = person.rooms.join(", ");
        let initial = if person.rooms.len() == 1 { person.rooms[0].as_str() } else { "" };
        let title =
            if rooms.is_empty() { format!("{verb} {name} in which room?") } else { format!("{verb} {name} in which room? ({rooms})") };
        self.open_prompt(PromptKind::Hub(prompt), &title, initial);
    }

    pub(crate) fn hub_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('e') => return self.hub_toggle(),
            KeyCode::Char('y') => {
                if let Some(link) = self.hub.link() {
                    self.copy(&link, "the hub's link");
                }
                return;
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                let at = HubPane::ALL.iter().position(|p| *p == self.hub.pane).unwrap_or(0);
                self.hub.pane = HubPane::ALL[(at + 1) % HubPane::ALL.len()];
                return;
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                let at = HubPane::ALL.iter().position(|p| *p == self.hub.pane).unwrap_or(0);
                self.hub.pane = HubPane::ALL[(at + HubPane::ALL.len() - 1) % HubPane::ALL.len()];
                return;
            }
            KeyCode::Down | KeyCode::Char('j') => return self.move_hub_selection(1),
            KeyCode::Up | KeyCode::Char('k') => return self.move_hub_selection(-1),
            _ => {}
        }
        if !self.hub.running() {
            if matches!(key.code, KeyCode::Char(_)) {
                self.warn("The hub isn't running: e starts it");
            }
            return;
        }
        match (self.hub.pane, key.code) {
            (_, KeyCode::Char('a')) => {
                let result = self.hub_announce();
                self.hub_result(result);
            }
            (_, KeyCode::Char(':')) => {
                self.open_prompt(PromptKind::Hub(HubPrompt::Command), "Hub command (as in a room: /stats, /kline list...)", "/")
            }
            (_, KeyCode::Char('n')) => self.open_prompt(PromptKind::Hub(HubPrompt::NewRoom), "New registered room", ""),
            (HubPane::People, code) => {
                let Some(person) = self.hub.person().cloned() else { return };
                let identity = person.identity.clone();
                match code {
                    KeyCode::Char('K') => self.ask_room(HubPrompt::Kick(identity), "Kick", &person),
                    KeyCode::Char('b') => self.ask_room(HubPrompt::Ban(identity), "Ban", &person),
                    KeyCode::Char('o') => self.ask_room(HubPrompt::Op(identity), "Make operator (or not)", &person),
                    KeyCode::Char('v') => self.ask_room(HubPrompt::Voice(identity), "Voice (or not)", &person),
                    KeyCode::Char('B') => {
                        let name = self.hub.name_of(&identity);
                        let title = format!("Ban {name} from the whole hub (disconnected now, and every time)? Type y");
                        self.open_prompt(PromptKind::Hub(HubPrompt::ConfirmKline(identity)), &title, "");
                    }
                    KeyCode::Char('d') => {
                        let result = self.hub_disconnect(&identity);
                        self.hub_result(result);
                    }
                    _ => {}
                }
            }
            (HubPane::Rooms, code) => {
                let Some(room) = self.hub.room().cloned() else { return };
                match code {
                    KeyCode::Char('t') => {
                        let title = format!("Topic of #{}", room.name);
                        self.open_prompt(PromptKind::Hub(HubPrompt::Topic(room.name)), &title, room.topic.as_deref().unwrap_or(""));
                    }
                    KeyCode::Char('m') => {
                        let title = format!(
                            "Modes for #{} (now {}): +m moderated, +i invite only, +t topic by operators, +n members only, +p private, +k key",
                            room.name, room.modes
                        );
                        self.open_prompt(PromptKind::Hub(HubPrompt::Modes(room.name)), &title, "");
                    }
                    KeyCode::Char('r') => {
                        let result = self.hub_register(&room.name, !room.registered);
                        self.hub_result(result);
                    }
                    _ => {}
                }
            }
            (HubPane::Bans, KeyCode::Char('x') | KeyCode::Delete) => {
                if let Some(ban) = self.hub.ban() {
                    let result = self.hub_ban(ban.room.as_deref(), &ban.identity, false);
                    self.hub_result(result);
                }
            }
            _ => {}
        }
    }

    fn move_hub_selection(&mut self, delta: isize) {
        let len = self.hub.len();
        if len == 0 {
            return;
        }
        let list = self.hub.list();
        let at = list.selected().unwrap_or(0).min(len - 1);
        list.select(Some(at.saturating_add_signed(delta).min(len - 1)));
    }

    pub(crate) fn click_hub(&mut self, at: Position) {
        if let Some((_, pane)) = self.regions.hub_panes.iter().find(|(rect, _)| rect.contains(at)) {
            self.hub.pane = *pane;
            return;
        }
        let list = self.regions.hub_list;
        if list.contains(at) {
            let index = self.hub.list().offset() + (at.y - list.y) as usize;
            if index < self.hub.len() {
                self.hub.list().select(Some(index));
            }
        }
    }

    pub(crate) fn scroll_hub(&mut self, delta: isize) {
        self.move_hub_selection(delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::channels::HubStatus;
    use crate::config::Settings;
    use crate::net::NetEvent;
    use crate::rrc::host::{HubPerson, HubRoom};
    use crate::store::Store;

    fn commands(rx: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>) -> Vec<NetCommand> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// What the panel asked the hub to run.
    fn runs(rx: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>) -> Vec<String> {
        commands(rx)
            .into_iter()
            .filter_map(|c| match c {
                NetCommand::HubAction(HubAction::Run(text)) => Some(text),
                _ => None,
            })
            .collect()
    }

    fn snapshot() -> HubSnapshot {
        let amy = "a".repeat(32);
        HubSnapshot {
            people: vec![HubPerson {
                identity: amy.clone(),
                nick: Some("amy".into()),
                rooms: vec!["lobby".into()],
                since: 0,
                links: 1,
                operator: false,
            }],
            rooms: vec![HubRoom {
                name: "lobby".into(),
                registered: false,
                modes: "(none)".into(),
                topic: None,
                members: vec![amy.clone()],
                founder: None,
                operators: vec![amy],
                voiced: Vec::new(),
                banned: vec!["c".repeat(32)],
            }],
            klines: vec!["b".repeat(32)],
            ..HubSnapshot::default()
        }
    }

    #[test]
    fn the_settings_start_change_and_stop_the_hub_and_you_join_it() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut rx) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        assert_eq!(app.hub.status, NodeStatus::Off);
        app.hub_toggle();
        let config = commands(&mut rx).into_iter().find_map(|c| match c {
            NetCommand::Hub(config) => Some(config),
            _ => None,
        });
        let config = config.flatten().expect("a hub to start");
        assert_eq!((config.name.as_str(), config.open_rooms, config.operator), ("rettui user", true, [0; 16]));
        assert_eq!(app.hub.status, NodeStatus::Starting);

        // Up: it's in your hub list, reached with its key, and connected.
        app.on_net(NetEvent::Hub(HubEvent::Started { hash: [7; 16], public_key: [1; 64] }));
        assert!(app.hub.running());
        let index = app.channels.hub_index([7; 16]).expect("your hub in the list");
        assert_eq!(app.channels.hubs[index].local_key, Some([1; 64]));
        assert_eq!(app.channels.hubs[index].name, "rettui user");
        let connect = commands(&mut rx).into_iter().find_map(|c| match c {
            NetCommand::RrcConnect { hub, key, .. } => Some((hub, key)),
            _ => None,
        });
        assert_eq!(connect, Some(([7; 16], Some([1; 64]))));

        // A new greeting: the same hub, as it runs.
        app.update_settings(&[("hub_greeting", "Hello\\nthere")]).unwrap();
        let config = commands(&mut rx).into_iter().find_map(|c| match c {
            NetCommand::Hub(config) => config,
            _ => None,
        });
        assert_eq!(config.and_then(|c| c.greeting).as_deref(), Some("Hello\nthere"));
        assert!(app.hub.running(), "still running");

        // Off: you leave it, and don't try again.
        app.update_settings(&[("hub_enabled", "false")]).unwrap();
        assert!(commands(&mut rx).iter().any(|c| matches!(c, NetCommand::Hub(None))));
        app.on_net(NetEvent::Hub(HubEvent::Stopped));
        assert_eq!(app.hub.status, NodeStatus::Off);
        let hub = &app.channels.hubs[index];
        assert!(matches!(hub.status, HubStatus::Disconnected));
        // While it's off (or starting), you don't connect to it.
        assert!(app.own_hub_waiting([7; 16]));
        app.start_channels();
        assert!(!commands(&mut rx).iter().any(|c| matches!(c, NetCommand::RrcConnect { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn panel_actions_are_hub_commands() {
        let dir = std::env::temp_dir().join(format!("rettui-hub-actions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings { hub_enabled: true, ..Settings::default() };
        let (mut app, mut rx) = crate::app::test_app_with_net(&dir, settings, Store::default());
        // Not running: nothing's sent.
        assert!(app.hub_run("/stats").is_err());
        app.on_net(NetEvent::Hub(HubEvent::Started { hash: [7; 16], public_key: [1; 64] }));
        app.on_net(NetEvent::Hub(HubEvent::State(snapshot())));
        commands(&mut rx);
        let amy = "a".repeat(32);
        app.hub_run("stats").unwrap();
        app.hub_kick("#lobby", &amy).unwrap();
        app.hub_ban(Some("lobby"), &amy, true).unwrap();
        app.hub_ban(None, &amy, false).unwrap();
        app.hub_op("lobby", &amy, false).unwrap();
        app.hub_topic("lobby", " Say hi ").unwrap();
        app.hub_modes("lobby", "+m +k secret -i").unwrap();
        assert_eq!(
            runs(&mut rx),
            [
                "/stats".to_string(),
                format!("/kick lobby {amy}"),
                format!("/ban lobby add {amy}"),
                format!("/kline del {amy}"),
                format!("/deop lobby {amy}"),
                "/topic lobby Say hi".into(),
                "/mode lobby +m".into(),
                "/mode lobby +k secret".into(),
                "/mode lobby -i".into(),
            ]
        );
        assert!(app.hub_kick("two words", &amy).is_err());
        assert!(app.hub_modes("lobby", "+k").is_err());
        assert!(app.hub_modes("lobby", "m").is_err());

        // Keys: o asks which room (theirs filled in), and takes away what
        // they have; B asks first; x lifts the ban picked.
        app.hub_key(KeyEvent::from(KeyCode::Char('o')));
        let prompt = app.prompt.take().expect("a prompt");
        assert_eq!((prompt.kind.clone(), prompt.input.text()), (PromptKind::Hub(HubPrompt::Op(amy.clone())), "lobby"));
        app.hub_prompt(HubPrompt::Op(amy.clone()), "lobby");
        assert_eq!(runs(&mut rx), [format!("/deop lobby {amy}")]);
        app.hub_key(KeyEvent::from(KeyCode::Char('B')));
        assert!(matches!(app.prompt.take().map(|p| p.kind), Some(PromptKind::Hub(HubPrompt::ConfirmKline(_)))));
        app.hub_prompt(HubPrompt::ConfirmKline(amy.clone()), "n");
        assert!(runs(&mut rx).is_empty());
        app.hub_prompt(HubPrompt::ConfirmKline(amy.clone()), "y");
        assert_eq!(runs(&mut rx), [format!("/kline add {amy}")]);
        app.hub_key(KeyEvent::from(KeyCode::Tab));
        app.hub_key(KeyEvent::from(KeyCode::Char('r')));
        assert!(
            commands(&mut rx).iter().any(|c| matches!(c, NetCommand::HubAction(HubAction::Register { room, on: true }) if room == "lobby"))
        );
        app.hub_key(KeyEvent::from(KeyCode::Tab));
        // Bans: the whole hub's first, then the rooms'.
        app.hub_key(KeyEvent::from(KeyCode::Down));
        app.hub_key(KeyEvent::from(KeyCode::Char('x')));
        assert_eq!(runs(&mut rx), [format!("/ban lobby del {}", "c".repeat(32))]);

        // Replies: the newest kept, the first line said.
        app.on_net(NetEvent::Hub(HubEvent::Reply { text: "Hub stats\nusers: 1".into(), error: false }));
        assert_eq!(app.hub.replies.back().map(|(text, _)| text.as_str()), Some("Hub stats\nusers: 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
