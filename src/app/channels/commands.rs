//! The channel input line: chat messages and slash commands.

use super::{ChatLine, LineKind, Target};
use crate::app::{App, Prompt, PromptKind};
use crate::net::Hash;
use crate::rrc::session::SessionCommand;
use crate::rrc::{self, t};
use crate::term::input::TextInput;

/// Own message ids awaiting the hub's echo.
const MAX_PENDING_ECHOES: usize = 256;

pub const HELP: &[&str] = &[
    "/join <room> [key]    join a room (also /j)",
    "/part [room]          leave a room (also /leave)",
    "/me <text>            send an action",
    "/msg <nick> [text]    whisper (a private notice, if the hub supports it); no text opens the conversation",
    "/dm <nick> [text]     LXMF message to the user's address (no text: open the conversation)",
    "/nick [name]          show or set your nick on this hub",
    "/who [room]           list users (also /names)",
    "/list                 list public rooms",
    "/topic <room> [text]  view or set the topic (hub-enforced)",
    "/ping                 measure round-trip time to the hub",
    "/clear                clear this room's messages",
    "/connect, /disconnect connect or disconnect this hub (also /quit)",
    "Other commands (/mode, /kick, /ban, /op, /stats, ...) go to the hub.",
];

/// Commands the hub implements; sent verbatim.
const HUB_COMMANDS: &[&str] = &[
    "who",
    "names",
    "topic",
    "mode",
    "kick",
    "ban",
    "invite",
    "op",
    "deop",
    "voice",
    "devoice",
    "register",
    "unregister",
    "kline",
    "stats",
    "reload",
];

impl App {
    /// Send what was typed in the channel input.
    pub fn submit_channel_input(&mut self) {
        let text = self.channels.input.take();
        let Some((index, room)) = self.channels.active() else {
            if !text.trim().is_empty() {
                self.warn("Add a hub first (n)");
            }
            return;
        };
        self.channels.scroll = 0;
        if let Some(parts) = self.channel_submit(index, &room, &text) {
            let bytes = text.trim().len();
            let limit = self.channels.hubs[index].limits.max_msg_bytes;
            self.prompt = Some(Prompt {
                kind: PromptKind::ConfirmSplit { hub: self.channels.hubs[index].hash, room, parts: parts.clone() },
                title: format!("{bytes} bytes is over this hub's {limit}-byte limit. Send as {} messages? Type y", parts.len()),
                input: TextInput::default(),
            });
        }
    }

    /// Send a line typed for a room (or the hub buffer, `room` empty): a
    /// message or a `/command`. Text over the hub's limit is not sent; its
    /// parts are returned so the user can confirm sending them.
    pub fn channel_submit(&mut self, index: usize, room: &str, text: &str) -> Option<Vec<String>> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if let Some(command) = text.strip_prefix('/') {
            self.channel_command(index, room, command.trim());
            return None;
        }
        if room.is_empty() {
            self.record(index, "", ChatLine::new(LineKind::Error, "Join a room first: /join <room>"));
            return None;
        }
        if let Some(peer) = super::whisper_peer(room) {
            let limit = self.channels.hubs[index].limits.max_msg_bytes;
            if text.len() > limit {
                return Some(rrc::split_message(text, limit));
            }
            if let Err(e) = self.rrc_whisper(index, &peer, text) {
                self.record(index, room, ChatLine::new(LineKind::Error, e));
            }
            return None;
        }
        if !self.channels.hubs[index].is_connected() {
            self.record(index, room, ChatLine::new(LineKind::Error, "Not connected; connecting now"));
            self.connect_hub(index);
            return None;
        }
        let limit = self.channels.hubs[index].limits.max_msg_bytes;
        if text.len() > limit {
            return Some(rrc::split_message(text, limit));
        }
        self.send_chat(index, room, t::MSG, text);
        None
    }

    pub fn send_chat(&mut self, index: usize, room: &str, kind: u64, text: &str) {
        let env = self.envelope(index, kind).room(room).text(text);
        let mut line = ChatLine::new(if kind == t::ACTION { LineKind::Action } else { LineKind::Msg }, text);
        line.src = Some(hex::encode(self.identity_hash));
        line.nick = env.nick.clone();
        line.own = true;
        line.pending = Some(env.id.clone());
        let hub = self.hub_mut(index);
        hub.sent.push_back(env.id.clone());
        if hub.sent.len() > MAX_PENDING_ECHOES {
            hub.sent.pop_front();
        }
        self.send_env(index, &env);
        self.record(index, room, line);
    }

    fn channel_command(&mut self, index: usize, room: &str, command: &str) {
        let (name, arg) = command.split_once(char::is_whitespace).unwrap_or((command, ""));
        let (name, arg) = (name.to_lowercase(), arg.trim());
        let connected = self.channels.hubs[index].is_connected();
        let error = |app: &mut App, text: String| {
            app.record(index, room, ChatLine::new(LineKind::Error, text));
        };
        // Room commands make no sense in a whisper conversation.
        if super::whisper_peer(room).is_some() && matches!(name.as_str(), "me" | "part" | "leave" | "topic" | "who" | "names") {
            return error(self, format!("/{name} is for rooms; this is a whisper conversation (x closes it)"));
        }
        let needs_connection = matches!(name.as_str(), "me" | "msg" | "ping" | "list") || HUB_COMMANDS.contains(&name.as_str());
        if needs_connection && !connected {
            return error(self, format!("/{name}: not connected (use /connect)"));
        }
        match name.as_str() {
            "help" => {
                for line in HELP {
                    self.record(index, room, ChatLine::new(LineKind::System, *line));
                }
            }
            "join" | "j" => {
                let mut words = arg.split_whitespace();
                let Some(target) = words.next() else {
                    return error(self, "Usage: /join <room> [key]".into());
                };
                let target = rrc::normalize_room(target);
                self.join(index, &target, words.next(), false);
                self.channels.selected = Some(Target { hub: self.channels.hubs[index].hash, room: Some(target) });
                self.mark_channel_read();
            }
            "part" | "leave" => {
                let target = if arg.is_empty() { room.to_string() } else { rrc::normalize_room(arg) };
                if target.is_empty() {
                    return error(self, "Usage: /part <room>".into());
                }
                self.part(index, &target);
            }
            "me" => {
                if arg.is_empty() || room.is_empty() {
                    return error(self, "Usage: /me <text> (in a room)".into());
                }
                let limit = self.channels.hubs[index].limits.max_msg_bytes;
                if arg.len() > limit {
                    return error(self, format!("Action too long (max {limit} bytes)"));
                }
                self.send_chat(index, room, t::ACTION, arg);
            }
            "msg" | "query" | "whisper" | "w" => self.private_notice(index, room, arg),
            "dm" | "lxmf" => self.dm_command(index, room, arg),
            "nick" => {
                if arg.is_empty() {
                    let nick = self.effective_nick(index).unwrap_or_else(|| "(none)".into());
                    let source = if self.channels.hubs[index].nick.is_some() { "this hub" } else { "display name" };
                    self.record(index, room, ChatLine::new(LineKind::System, format!("Nick: {nick} (from {source})")));
                    return;
                }
                let max = self.channels.hubs[index].limits.max_nick_bytes;
                let Some(nick) = rrc::normalize_nick(arg, max) else {
                    return error(self, format!("Nicks are 1-{max} bytes on one line"));
                };
                self.hub_mut(index).nick = Some(nick.clone());
                self.save_hubs();
                if connected {
                    // A new HELLO updates the nick; the hub re-welcomes us
                    // and we rejoin rooms quietly.
                    self.session(index, SessionCommand::Hello(Some(nick.clone())));
                }
                self.record(index, room, ChatLine::new(LineKind::System, format!("Nick set to {nick}")));
            }
            "ping" => self.session(index, SessionCommand::Ping),
            "list" => {
                let env = self.envelope(index, t::MSG).text("/list");
                self.send_env(index, &env);
            }
            "clear" => {
                let hub = self.hub_mut(index);
                if let Some(buffer) = hub.buffers.get_mut(room) {
                    buffer.clear();
                    hub.history_dirty = true;
                }
            }
            "connect" => self.connect_hub(index),
            "disconnect" | "quit" => self.disconnect_hub(index),
            _ if HUB_COMMANDS.contains(&name.as_str()) => {
                let text = if arg.is_empty() { format!("/{name}") } else { format!("/{name} {arg}") };
                let mut env = self.envelope(index, t::MSG).text(&text);
                if !room.is_empty() {
                    env = env.room(room);
                }
                self.send_env(index, &env);
            }
            _ => error(self, format!("Unknown command /{name} (try /help)")),
        }
    }

    /// `/msg <nick|hash-prefix> <text>`: a direct notice through the hub.
    fn private_notice(&mut self, index: usize, room: &str, arg: &str) {
        let fail = |app: &mut App, text: &str| {
            app.record(index, room, ChatLine::new(LineKind::Error, text.to_string()));
        };
        if !self.channels.hubs[index].direct_notices {
            return fail(self, "This hub does not support private notices");
        }
        let (who, text) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
        if who.is_empty() {
            return fail(self, "Usage: /msg <nick> [text]");
        }
        let target = match self.find_rrc_user(index, who) {
            Ok(target) => target,
            Err(e) => return fail(self, e),
        };
        // No text: open the conversation to write in.
        if text.trim().is_empty() {
            self.open_whisper(index, &target);
            return;
        }
        if let Err(e) = self.rrc_whisper(index, &target, text) {
            return fail(self, &e);
        }
        if super::whisper_key(&target) != room {
            let name = self.channels.hubs[index].name_of(&target);
            let note = format!("Whispered to {name}; your conversation is under the hub as @ {name}");
            self.record(index, room, ChatLine::new(LineKind::System, note));
        }
    }

    pub fn confirm_split(&mut self, hub: Hash, room: &str, parts: &[String]) {
        let Some(index) = self.channels.hub_index(hub) else { return };
        for part in parts {
            match super::whisper_peer(room) {
                Some(peer) => {
                    if let Err(e) = self.rrc_whisper(index, &peer, part) {
                        self.record(index, room, ChatLine::new(LineKind::Error, e));
                    }
                }
                None => self.send_chat(index, room, t::MSG, part),
            }
        }
    }
}
