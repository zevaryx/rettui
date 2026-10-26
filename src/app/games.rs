//! Games with contacts (see `crate::games`): what arrives is played, and
//! each of your moves goes as a message to them, so it's sent and resent
//! as messages are. Moves are kept apart from the conversation's messages
//! (`Conversation::game_moves`): the Games tab shows them.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};

use super::notify::{Notification, Target};
use super::{App, PromptKind};
use crate::games::{Board, GAMES, GameRef, GameView, Prepared, Received};
use crate::lxmf::{self, DeliveryMode};
use crate::net::NetCommand;
use crate::store::{Message, MessageState};

/// Starting a new game (`n` in the Games tab, or `g` in a conversation with
/// no game yet): who with, then which game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameMenu {
    /// Who with, once picked.
    pub peer: Option<String>,
    pub selected: usize,
}

/// Which games the Games tab lists: all of them, or by whether they're
/// still going (active), waiting to be accepted (pending), or over
/// (completed, declined or expired).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GameFilter {
    #[default]
    All,
    Active,
    Pending,
    Completed,
}

impl GameFilter {
    pub const ALL: [GameFilter; 4] = [GameFilter::All, GameFilter::Active, GameFilter::Pending, GameFilter::Completed];

    pub fn label(self) -> &'static str {
        match self {
            GameFilter::All => "All",
            GameFilter::Active => "Active",
            GameFilter::Pending => "Pending",
            GameFilter::Completed => "Completed",
        }
    }

    /// Whether a game with this status (lrgp's) is listed.
    pub fn shows(self, status: &str) -> bool {
        match self {
            GameFilter::All => true,
            GameFilter::Active => status == "active",
            GameFilter::Pending => status == "pending",
            GameFilter::Completed => !matches!(status, "active" | "pending"),
        }
    }

    fn next(self) -> Self {
        let at = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

/// A row of the new-game picker.
#[derive(Debug, Clone)]
pub enum MenuItem {
    /// Someone to play (a conversation's address).
    Contact(String),
    New(&'static str),
}

/// What a game's prompts ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GamePrompt {
    /// A move typed: chess or checkers (`e7e8n` to promote to a knight,
    /// `a5 c3 e1` to jump twice), a square (`h8`) or a line (`c3h`).
    Move {
        app: String,
        session: String,
    },
    ConfirmResign {
        app: String,
        session: String,
    },
    ConfirmForget {
        app: String,
        session: String,
    },
}

/// Where the open game's board is on screen, for clicks: its squares' top
/// left, their size, and how many there are across and down.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoardRegion {
    pub origin: Position,
    pub cell: (u16, u16),
    pub across: u16,
    pub down: u16,
    /// How far right each row starts from the one above (Hex's).
    pub skew: u16,
    /// Every other column one wide, from the first (Dots and Boxes' dots
    /// and the lines up from them), between those `cell` wide.
    pub narrow_even: bool,
    /// A gap of one after every three columns and rows (Ultimate
    /// Tic-Tac-Toe's small boards).
    pub groups: bool,
    /// The whole box.
    pub area: Rect,
}

impl BoardRegion {
    /// The square at `at` (as shown, row by row from the top left).
    pub fn square(&self, at: Position) -> Option<usize> {
        let (w, h) = self.cell;
        if w == 0 || h == 0 || at.y < self.origin.y {
            return None;
        }
        let mut row = (at.y - self.origin.y) / h;
        if self.groups {
            // Three rows, then a gap.
            if row % 4 == 3 {
                return None;
            }
            row = row / 4 * 3 + row % 4;
        }
        let x = at.x.checked_sub(self.origin.x + row * self.skew)?;
        let col = if self.narrow_even {
            x / (w + 1) * 2 + u16::from(x % (w + 1) != 0)
        } else if self.groups {
            if x / w % 4 == 3 {
                return None;
            }
            x / w / 4 * 3 + x / w % 4
        } else {
            x / w
        };
        (col < self.across && row < self.down).then_some((row * self.across + col) as usize)
    }
}

/// The game selected in the Games tab (terminal UI): which, and the square
/// or column picked (a chess move's first square once chosen, a checkers
/// move's squares so far).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameScreen {
    pub app: String,
    pub session: String,
    pub cursor: usize,
    pub from: Option<usize>,
    /// A checkers move's squares, picked so far: the piece, then where it
    /// lands (each jump of several).
    pub path: Vec<usize>,
    /// The cursor's been put on your side of a chess or checkers board (which side
    /// that is isn't known until the challenge is accepted).
    pub placed: bool,
}

/// What became of a game message received.
pub(super) enum Play {
    /// Shown, as a move of this game (if it is one), with what to say.
    Show(Option<GameRef>, Option<String>),
    /// Not shown: a copy of one already here.
    Drop,
}

impl App {
    /// Play a game message from `key` (`fields`: its two LRGP fields), if
    /// it can be: only one whose signature was checked, so it's from them.
    pub(super) fn receive_game(&mut self, key: &str, message: &lxmf::InboundMessage, fields: &HashMap<u8, Vec<u8>>) -> Play {
        let name = self.store.display_name(key);
        if message.paper {
            return Play::Show(None, Some("A game's move, read from paper: not played".into()));
        }
        if !message.verified {
            return Play::Show(None, Some("A game's move, not played: their key isn't known, so it couldn't be checked".into()));
        }
        match self.games.receive(key, fields) {
            Received::Replay => Play::Drop,
            Received::Rejected(why) => {
                self.log(format!("A game message from {name} wasn't played: {why}"));
                Play::Show(None, Some(format!("A game's move, not played: {why}")))
            }
            Received::TheirError { game, text } => {
                self.games_dirty = true;
                Play::Show(Some(game), Some(text))
            }
            Received::Applied { game, answer, refused } => {
                self.games_dirty = true;
                if let Some(answer) = answer {
                    // Best effort: they hear the move wasn't played.
                    if let Err(e) = self.send_game(key, answer) {
                        self.log(format!("Couldn't tell {name} their move wasn't played: {e}"));
                    }
                }
                Play::Show(Some(game), refused)
            }
        }
    }

    /// Challenge `key` (a hex address) to a game of `app`.
    pub fn start_game(&mut self, key: &str, app: &str) -> Result<String, String> {
        let prepared = self.games.challenge(app, key)?;
        let session = prepared.game.session.clone();
        self.games_dirty = true;
        self.send_game(key, prepared)?;
        Ok(session)
    }

    /// Do `command` in a game: accept, decline, resign, offer a draw or
    /// answer one, or `move` (with what was picked: a cell, a column, a
    /// chess move).
    pub fn game_action(&mut self, app: &str, session: &str, command: &str, picked: Option<&str>) -> Result<(), String> {
        if command == "resend" {
            return self.resend_moves(app, session);
        }
        let peer = self.games.view(app, session).map(|g| g.peer).ok_or("There's no such game")?;
        let payload = match (command, picked) {
            ("move", Some(picked)) => crate::games::move_payload(app, picked)?,
            ("move", None) => return Err("Pick where to move".into()),
            _ => HashMap::new(),
        };
        let prepared = self.games.act(app, session, command, payload)?;
        self.games_dirty = true;
        self.send_game(&peer, prepared)
    }

    /// A move received (`stored`, of `game`), played: kept, and they're
    /// told of it as of a message, unless it's on screen.
    pub(super) fn keep_received_move(&mut self, key: &str, stored: Message, game: &GameRef) {
        let label = stored.opening();
        let conversation = self.store.conversations.entry(key.to_string()).or_default();
        let muted = conversation.muted;
        conversation.game_moves.push(stored);
        conversation.game_moves.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
        conversation.keep_moves();
        self.store_dirty = true;
        let name = self.store.display_name(key);
        self.log(format!("{name}: {label}"));
        if self.is_request(key) || !self.settings.notify_messages || muted {
            return;
        }
        let on_screen = self.tab == super::Tab::Games
            && self.game_menu.is_none()
            && self.game_view.as_ref().is_some_and(|s| s.app == game.app && s.session == game.session);
        let target = Target::Game { key: key.to_string(), app: game.app.clone(), session: game.session.clone() };
        self.push_notification(on_screen, Notification { title: name, body: label, target });
    }

    /// How your last move of a game went, if it isn't there yet, and what
    /// came with their last (a move refused, an error): the Games tab's
    /// note on a game.
    pub fn game_notes(&self, game: &GameView) -> Vec<String> {
        let Some(conversation) = self.store.conversations.get(&game.peer) else { return Vec::new() };
        let mut notes = Vec::new();
        if let Some(sent) = conversation.last_move(&game.session, false) {
            match &sent.state {
                MessageState::Sending => notes.push("Your last move is being sent…".to_string()),
                MessageState::Failed(why) => {
                    let again = if self.settings.resend_on_announce { "it goes again when they announce, or" } else { "it can be" };
                    notes.push(format!("Your last move wasn't sent ({why}): {again} sent again now"));
                }
                _ => {}
            }
        }
        if let Some(received) = conversation.last_move(&game.session, true) {
            notes.extend(received.notes.iter().cloned());
        }
        notes
    }

    /// Moves of a game that failed to send, oldest first.
    fn unsent_moves(&self, game: &GameView) -> Vec<String> {
        let Some(conversation) = self.store.conversations.get(&game.peer) else { return Vec::new() };
        conversation
            .game_moves
            .iter()
            .filter(|m| !m.incoming && matches!(m.state, MessageState::Failed(_)))
            .filter(|m| m.game.as_ref().is_some_and(|g| g.session == game.session))
            .map(|m| m.id.clone())
            .collect()
    }

    /// Whether a move of yours in this game failed to send (and can be
    /// sent again).
    pub fn game_unsent(&self, game: &GameView) -> bool {
        !self.unsent_moves(game).is_empty()
    }

    /// Send a game's moves that failed again, the same moves (`r`, or
    /// **Send again** in the web UI).
    fn resend_moves(&mut self, app: &str, session: &str) -> Result<(), String> {
        let game = self.games.view(app, session).ok_or("There's no such game")?;
        let unsent = self.unsent_moves(&game);
        if unsent.is_empty() {
            return Err("Nothing of this game is waiting to be sent".into());
        }
        let mut mode = self.delivery_for(&game.peer);
        if mode == DeliveryMode::Paper || (mode == DeliveryMode::Propagated && self.propagation_node().is_none()) {
            mode = DeliveryMode::Auto;
        }
        for id in unsent {
            self.retry_message(&game.peer, &id, mode)?;
        }
        Ok(())
    }

    /// Forget a game (and its moves).
    pub fn forget_game(&mut self, app: &str, session: &str) -> Result<(), String> {
        let peer = self.games.view(app, session).map(|g| g.peer);
        if !self.games.remove(app, session)? {
            return Err("There's no such game".into());
        }
        if let Some(conversation) = peer.and_then(|peer| self.store.conversations.get_mut(&peer)) {
            conversation.game_moves.retain(|m| m.game.as_ref().is_none_or(|g| g.session != session));
            self.store_dirty = true;
        }
        self.games_dirty = true;
        if self.game_view.as_ref().is_some_and(|g| g.app == app && g.session == session) {
            self.game_view = None;
            self.game_playing = false;
        }
        Ok(())
    }

    /// Send a game's action to `key`, as a message (kept with the
    /// conversation's moves): its text says what it is for clients without
    /// games. Never on paper
    /// (the move would be lost to the game); by how messages to them go
    /// otherwise.
    fn send_game(&mut self, key: &str, prepared: Prepared) -> Result<(), String> {
        let to = crate::net::parse_hash(key).ok_or("An LXMF address is 32 hex characters")?;
        let mut mode = self.delivery_for(key);
        if mode == DeliveryMode::Paper || (mode == DeliveryMode::Propagated && self.propagation_node().is_none()) {
            mode = DeliveryMode::Auto;
        }
        let envelope = prepared.game.envelope.as_deref().and_then(|e| hex::decode(e).ok()).ok_or("The game's move couldn't be packed")?;
        let id = self.store.next_local_id;
        self.store.next_local_id += 1;
        let timestamp = super::now();
        let conversation = self.store.conversations.entry(key.to_string()).or_default();
        conversation.game_moves.push(Message {
            id: format!("local-{id}"),
            content: prepared.text.clone(),
            timestamp,
            state: MessageState::Sending,
            game: Some(prepared.game),
            ..Message::default()
        });
        conversation.keep_moves();
        self.store_dirty = true;
        self.update_policy(false);
        let appearance = self.own_appearance();
        let message =
            lxmf::Outgoing { appearance, game: Some(envelope), ..lxmf::Outgoing::text(to, prepared.text, Vec::new(), mode, timestamp) };
        self.send(NetCommand::SendMessage { id, message: Box::new(message) });
        Ok(())
    }

    /// The new-game picker's rows: whoever you have a conversation with,
    /// then (once one is picked) a game of each kind.
    pub fn game_menu_items(&self) -> Vec<MenuItem> {
        match &self.game_menu {
            None => Vec::new(),
            Some(GameMenu { peer: None, .. }) => self.conversation_order().into_iter().map(MenuItem::Contact).collect(),
            Some(GameMenu { peer: Some(_), .. }) => GAMES.iter().map(|app| MenuItem::New(app)).collect(),
        }
    }

    /// Start a new game: with `peer`, or with whoever's picked first.
    pub(super) fn open_game_menu(&mut self, peer: Option<String>) {
        self.game_menu = Some(GameMenu { peer, selected: 0 });
    }

    /// Your games, newest first, that the filter lets through (the Games
    /// tab's list).
    pub fn game_list(&self) -> Vec<GameView> {
        self.games.list(None).into_iter().filter(|g| self.game_filter.shows(&g.status)).collect()
    }

    /// List the games `filter` lets through, the first selected.
    pub(super) fn filter_games(&mut self, filter: GameFilter) {
        if self.game_filter != filter {
            (self.game_filter, self.game_view, self.game_selected, self.game_playing) = (filter, None, 0, false);
            self.regions.games_first = 0;
        }
        self.sync_game_selection();
    }

    /// Keep the Games tab's selection on the same game as the list changes
    /// (a move puts its game first), or on a game at all.
    pub fn sync_game_selection(&mut self) {
        let list = self.game_list();
        let at = self.game_view.as_ref().and_then(|s| list.iter().position(|g| g.app == s.app && g.session == s.session));
        match at {
            Some(at) => self.game_selected = at,
            None => match list.get(self.game_selected.min(list.len().saturating_sub(1))) {
                Some(game) => {
                    self.game_selected = self.game_selected.min(list.len() - 1);
                    let (app, session) = (game.app.clone(), game.session.clone());
                    self.open_game(&app, &session);
                }
                None => {
                    self.game_view = None;
                    self.game_playing = false;
                }
            },
        }
        self.place_game_cursor();
    }

    /// Select the game at `index` in the list.
    pub(super) fn select_game(&mut self, index: usize) {
        let list = self.game_list();
        let Some(game) = list.get(index.min(list.len().saturating_sub(1))) else { return };
        self.game_selected = index.min(list.len() - 1);
        let same = self.game_view.as_ref().is_some_and(|s| s.app == game.app && s.session == game.session);
        if !same {
            let (app, session) = (game.app.clone(), game.session.clone());
            self.open_game(&app, &session);
        }
    }

    /// Keys in the Games tab: the list, the board once you're on it, or the
    /// new-game picker.
    pub(super) fn games_key(&mut self, key: KeyEvent) {
        self.sync_game_selection();
        if self.game_menu.is_some() {
            return self.game_menu_key(key);
        }
        if self.game_playing && self.game_view.is_some() {
            return self.game_view_key(key);
        }
        let count = self.game_list().len();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') if count > 0 => self.select_game(self.game_selected + 1),
            KeyCode::Up | KeyCode::Char('k') if count > 0 => self.select_game(self.game_selected.saturating_sub(1)),
            KeyCode::Home => self.select_game(0),
            KeyCode::End => self.select_game(count.saturating_sub(1)),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if self.game_view.is_some() => self.game_playing = true,
            KeyCode::Char('n') => self.open_game_menu(None),
            KeyCode::Char('f') => self.filter_games(self.game_filter.next()),
            KeyCode::Char('c') => {
                if let Some(peer) = self.game_view.as_ref().and_then(|s| self.games.view(&s.app, &s.session)).map(|g| g.peer) {
                    self.open_conversation(peer);
                    self.switch_tab(super::Tab::Messages);
                }
            }
            KeyCode::Char('x') => {
                if let Some(game) = self.game_view.as_ref().and_then(|s| self.games.view(&s.app, &s.session)) {
                    self.ask_forget_game(&game);
                }
            }
            _ => {}
        }
    }

    fn game_menu_key(&mut self, key: KeyEvent) {
        let count = self.game_menu_items().len();
        let Some(menu) = self.game_menu.as_mut() else { return };
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => menu.selected = (menu.selected + 1).min(count.saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => menu.selected = menu.selected.saturating_sub(1),
            KeyCode::Enter => self.pick_menu_item(),
            KeyCode::Esc | KeyCode::Char('q') => self.game_menu = None,
            _ => {}
        }
    }

    /// Pick the picker's selected row: someone to play, or the game to
    /// challenge them to (then shown, selected, in the Games tab).
    fn pick_menu_item(&mut self) {
        let Some(menu) = self.game_menu.clone() else { return };
        match (self.game_menu_items().into_iter().nth(menu.selected), menu.peer) {
            (Some(MenuItem::Contact(peer)), _) => self.game_menu = Some(GameMenu { peer: Some(peer), selected: 0 }),
            (Some(MenuItem::New(app)), Some(peer)) => match self.start_game(&peer, app) {
                Ok(session) => {
                    self.game_menu = None;
                    let name = self.store.display_name(&peer);
                    self.notify(format!("Challenged {name} to {}", crate::games::game_name(app)));
                    self.show_game(app, &session);
                }
                Err(e) => self.warn(e),
            },
            _ => {}
        }
    }

    /// A click on the new-game picker (its row, from the top of the list).
    pub(super) fn click_game_menu(&mut self, row: Option<usize>) {
        let count = self.game_menu_items().len();
        match (row, self.game_menu.as_mut()) {
            (Some(row), Some(menu)) if row < count => {
                menu.selected = row;
                self.pick_menu_item();
            }
            _ => self.game_menu = None,
        }
    }

    /// A click in the Games tab: on the list, select that game; on the
    /// board, play there.
    pub(super) fn click_games(&mut self, at: Position) {
        self.sync_game_selection();
        let list = self.regions.games_list;
        if let Some(&(_, filter)) = self.regions.games_filters.iter().find(|(rect, _)| rect.contains(at)) {
            self.filter_games(filter);
        } else if list.contains(at) {
            self.game_playing = false;
            self.select_game(self.regions.games_first + (at.y - list.y) as usize / 2);
        } else if self.regions.game_board.area.contains(at) && self.game_view.is_some() {
            self.game_playing = true;
            self.click_game(at);
        }
    }

    fn ask_forget_game(&mut self, game: &GameView) {
        let prompt = GamePrompt::ConfirmForget { app: game.app.clone(), session: game.session.clone() };
        let unfinished = if matches!(game.status.as_str(), "pending" | "active") { " It isn't over: they won't be told." } else { "" };
        self.open_prompt(PromptKind::Game(prompt), &format!("Forget this game of {}?{unfinished} Type y", game.name), "");
    }

    fn game_view_key(&mut self, key: KeyEvent) {
        self.place_game_cursor();
        let Some(screen) = self.game_view.clone() else { return };
        let Some(game) = self.games.view(&screen.app, &screen.session) else {
            self.game_view = None;
            return;
        };
        let (app, session) = (screen.app.as_str(), screen.session.as_str());
        let can = |command: &str| game.actions.iter().any(|a| a == command);
        let result = match key.code {
            // Back to the list.
            KeyCode::Esc | KeyCode::Char('q') => {
                self.game_playing = false;
                Ok(())
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Char('h' | 'l' | 'k' | 'j') => {
                self.move_game_cursor(&game, key.code);
                Ok(())
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.play_at_cursor(&game),
            // A cell or column by number, as on a keypad (1 the first); a pit
            // of yours, from your left.
            KeyCode::Char(c @ '1'..='9') if matches!(game.app.as_str(), "ttt" | "four_in_a_row") => {
                let picked = c as usize - '1' as usize;
                self.game_action(app, session, "move", Some(&picked.to_string()))
            }
            KeyCode::Char(c @ '1'..='6') if game.app == "mancala" => {
                if let Some(screen) = self.game_view.as_mut() {
                    screen.cursor = c as usize - '1' as usize;
                }
                self.play_at_cursor(&game)
            }
            KeyCode::Char('s') if typed_moves(&game) && game.app == "hex" => self.game_action(app, session, "move", Some("swap")),
            KeyCode::Char('p') if game.app == "go" => self.game_action(app, session, "move", Some("pass")),
            // A throw by number: rock, paper, scissors.
            KeyCode::Char(c @ '1'..='3') if game.app == "rps" => {
                if let Some(screen) = self.game_view.as_mut() {
                    screen.cursor = c as usize - '1' as usize;
                }
                self.play_at_cursor(&game)
            }
            KeyCode::Char('a') if can("accept") => self.game_action(app, session, "accept", None),
            KeyCode::Char('d') if can("decline") => self.game_action(app, session, "decline", None),
            KeyCode::Char('D') if can("draw_offer") => self.game_action(app, session, "draw_offer", None),
            KeyCode::Char('D' | 'y') if can("draw_accept") => self.game_action(app, session, "draw_accept", None),
            KeyCode::Char('n') if can("draw_decline") => self.game_action(app, session, "draw_decline", None),
            KeyCode::Char('C') if can(crate::games::DRAW_CLAIM) => self.game_action(app, session, crate::games::DRAW_CLAIM, None),
            KeyCode::Char('R') if can("resign") => {
                let prompt = GamePrompt::ConfirmResign { app: app.into(), session: session.into() };
                self.open_prompt(PromptKind::Game(prompt), &format!("Resign this game of {}? Type y", game.name), "");
                Ok(())
            }
            KeyCode::Char('t') if typed_moves(&game) && game.yours && game.status == "active" => {
                let prompt = GamePrompt::Move { app: app.into(), session: session.into() };
                let ask = match game.app.as_str() {
                    "chess" => "Your move, by its squares (e2e4; e7e8n promotes to a knight)",
                    "checkers" => "Your move, by its squares (a3 b4; a5 c3 e1 jumps twice)",
                    "dots_and_boxes" => "Your line, by a dot and which way (c3h to the right of c3, c3v up from it)",
                    "hex" => "Your move, by its cell (f6; swap, as White's first move, takes Black's)",
                    "go" => "Your move, by its point (e5; pass)",
                    "breakthrough" => "Your move, by its squares (a2b3)",
                    "amazons" => "Your move, by where from, where to and the arrow (d1d7/g7)",
                    "morris" => "Your move, by its points (d2 places; d2-d3 slides; d2xg7 also takes g7)",
                    "quoridor" => "Your move, by a square to step to (e2) or a wall (c3h across, c3v up from c3)",
                    "battleship" => "Your shot, by its square (e5)",
                    _ => "Your move, by its square (d3)",
                };
                self.open_prompt(PromptKind::Game(prompt), ask, "");
                Ok(())
            }
            KeyCode::Char('r') if self.game_unsent(&game) => self.game_action(app, session, "resend", None),
            KeyCode::Char('x') => {
                self.ask_forget_game(&game);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.warn(e);
        }
    }

    /// Move the cursor on the board, as it's shown (Black sees the chess
    /// board turned round, and the checkers one).
    fn move_game_cursor(&mut self, game: &GameView, code: KeyCode) {
        let Some(screen) = self.game_view.as_mut() else { return };
        let (dx, dy): (i32, i32) = match code {
            KeyCode::Left | KeyCode::Char('h') => (-1, 0),
            KeyCode::Right | KeyCode::Char('l') => (1, 0),
            KeyCode::Up | KeyCode::Char('k') => (0, -1),
            _ => (0, 1),
        };
        let (width, height, turned): (usize, usize, bool) = match &game.board {
            Board::TicTacToe { .. } => (3, 3, false),
            Board::FourInARow { .. } => (7, 1, false),
            Board::Chess { white, .. } => (8, 8, !white),
            Board::Checkers { black, .. } => (8, 8, *black),
            Board::Reversi { .. } => (8, 8, false),
            Board::Hex { .. } => (11, 11, false),
            Board::Gomoku { .. } => (15, 15, false),
            Board::Mancala { .. } => (6, 1, false),
            Board::UltimateTicTacToe { .. } | Board::Go { .. } => (9, 9, false),
            Board::Pente { .. } => (19, 19, false),
            Board::Breakthrough { .. } => (8, 8, false),
            Board::Amazons { .. } | Board::Battleship { .. } => (10, 10, false),
            Board::Quoridor { .. } => (17, 17, false),
            Board::Rps { .. } => (3, 1, false),
            Board::Morris { .. } => {
                screen.cursor = morris_step(screen.cursor, dx, dy);
                return;
            }
            Board::DotsAndBoxes { .. } => {
                screen.cursor = dots_step(screen.cursor, dx, dy);
                return;
            }
        };
        let (dx, dy) = if turned { (-dx, -dy) } else { (dx, dy) };
        let (x, y) = ((screen.cursor % width) as i32 + dx, (screen.cursor / width) as i32 + dy);
        if (0..width as i32).contains(&x) && (0..height as i32).contains(&y) {
            screen.cursor = y as usize * width + x as usize;
        }
    }

    /// Play the square or column under the cursor: for chess, the first
    /// square picks a piece of yours, the second moves it there; for
    /// checkers, the next squares are where it lands, until the move's
    /// whole.
    fn play_at_cursor(&mut self, game: &GameView) -> Result<(), String> {
        let Some(screen) = self.game_view.clone() else { return Ok(()) };
        if let Board::Checkers { cells, black, legal, must_capture, .. } = &game.board {
            return self.play_checkers(game, &screen, cells, *black, legal, *must_capture);
        }
        match &game.board {
            Board::Breakthrough { legal, .. } | Board::Amazons { legal, .. } | Board::Morris { legal, .. } => {
                return self.play_steps(game, &screen, legal);
            }
            Board::Rps { legal, phase, .. } => {
                if !game.yours || game.status != "active" || game.draw_offer == Some(true) {
                    return Err("It isn't your move".into());
                }
                // The match's last throw is only shown.
                let throw =
                    if phase == "reveal" && legal.is_empty() { String::new() } else { ["r", "p", "s"][screen.cursor.min(2)].to_string() };
                if !throw.is_empty() && !legal.contains(&throw) {
                    return Err("You can't throw that now".into());
                }
                return self.game_action(&screen.app, &screen.session, "move", Some(&throw));
            }
            Board::Battleship { phase, .. } if phase != "shoot" && !phase.is_empty() => {
                if !game.yours || game.status != "active" || game.draw_offer == Some(true) {
                    return Err("It isn't your move".into());
                }
                return self.game_action(&screen.app, &screen.session, "move", Some(""));
            }
            _ => {}
        }
        // One square, line or pit: played if it's a move you can make.
        let named = match &game.board {
            Board::UltimateTicTacToe { legal, .. } => {
                Some((crate::games::grid_square_name(screen.cursor, 9), legal, "You can't play there"))
            }
            Board::Go { legal, .. } => {
                Some((crate::games::grid_square_name(screen.cursor, 9), legal, "You can't play there: it's taken, or a capture or repeat"))
            }
            Board::Pente { legal, .. } => Some((crate::games::grid_square_name(screen.cursor, 19), legal, "That point is taken")),
            Board::Battleship { legal, .. } => {
                Some((crate::games::grid_square_name(screen.cursor, 10), legal, "You've fired there already"))
            }
            Board::Quoridor { legal, .. } => {
                let m = crate::games::quoridor_move(screen.cursor / 17, screen.cursor % 17, legal)
                    .ok_or("Put the cursor on a square you can step to, or between squares for a wall")?;
                Some((m, legal, "That isn't a legal move"))
            }
            Board::Hex { legal, .. } => Some((crate::games::grid_square_name(screen.cursor, 11), legal, "That cell is taken")),
            Board::Gomoku { legal, .. } => Some((crate::games::grid_square_name(screen.cursor, 15), legal, "That point is taken")),
            Board::Reversi { legal, .. } => {
                Some((crate::games::grid_square_name(screen.cursor, 8), legal, "A disc there must turn over some of theirs"))
            }
            Board::DotsAndBoxes { legal, .. } => {
                let line =
                    crate::games::dots_line(screen.cursor / 11, screen.cursor % 11).ok_or("Put the cursor on a line between two dots")?;
                Some((line, legal, "That line's drawn already"))
            }
            Board::Mancala { black, legal, .. } => {
                let (pit, _) = crate::games::mancala_pits(*black)[screen.cursor.min(5)];
                Some((pit.to_string(), legal, "That pit is empty"))
            }
            _ => None,
        };
        if let Some((named, legal, taken)) = named {
            if !game.yours || game.status != "active" || game.draw_offer == Some(true) {
                return Err("It isn't your move".into());
            }
            if !legal.contains(&named) {
                return Err(taken.into());
            }
            return self.game_action(&screen.app, &screen.session, "move", Some(&named));
        }
        let Board::Chess { fen, white, legal, .. } = &game.board else {
            return self.game_action(&screen.app, &screen.session, "move", Some(&screen.cursor.to_string()));
        };
        let squares = crate::games::chess_squares(fen);
        let own = |i: usize| squares.get(i).is_some_and(|p| p.is_ascii_alphabetic() && p.is_ascii_uppercase() == *white);
        match screen.from {
            Some(from) if from == screen.cursor => self.set_chess_from(None),
            _ if own(screen.cursor) => {
                if !legal.iter().any(|m| m.starts_with(&crate::games::square_name(screen.cursor))) {
                    return Err(if game.yours { "That piece can't move" } else { "It isn't your move" }.into());
                }
                self.set_chess_from(Some(screen.cursor));
            }
            Some(from) => {
                let uci = crate::games::chess_move(legal, from, screen.cursor).ok_or("That isn't a legal move")?;
                self.game_action(&screen.app, &screen.session, "move", Some(&uci))?;
                self.set_chess_from(None);
            }
            None => return Err("Pick one of your pieces first".into()),
        }
        Ok(())
    }

    fn play_checkers(
        &mut self,
        game: &GameView,
        screen: &GameScreen,
        cells: &[char],
        black: bool,
        legal: &[String],
        must_capture: bool,
    ) -> Result<(), String> {
        let at = screen.cursor;
        let own = cells.get(at).is_some_and(|p| p.to_ascii_lowercase() == if black { 'b' } else { 'w' });
        // Your piece: picked (again, to start over), or put down.
        if screen.path.len() <= 1 && own || screen.path.first() == Some(&at) {
            if screen.path == [at] {
                self.set_checkers_path(Vec::new());
            } else if crate::games::checkers_moves(legal, &[at]).is_empty() {
                return Err(match () {
                    _ if !game.yours => "It isn't your move",
                    _ if must_capture => "You must jump, with a piece that can",
                    _ => "That piece can't move",
                }
                .into());
            } else {
                self.set_checkers_path(vec![at]);
            }
            return Ok(());
        }
        if screen.path.is_empty() {
            return Err("Pick one of your pieces first".into());
        }
        let path = [screen.path.as_slice(), &[at]].concat();
        match crate::games::checkers_moves(legal, &path)[..] {
            [] if must_capture => Err("You must jump".into()),
            [] => Err("That isn't a legal move".into()),
            // One move goes this way: made, all its jumps.
            [whole] => {
                let whole = whole.clone();
                self.game_action(&screen.app, &screen.session, "move", Some(&whole))?;
                self.set_checkers_path(Vec::new());
                Ok(())
            }
            // Several go on from here: where to next?
            _ => {
                self.set_checkers_path(path);
                Ok(())
            }
        }
    }

    /// Breakthrough, Amazons and Nine Men's Morris: the squares of a move
    /// are picked one by one (a piece, where it goes, an arrow's square or
    /// a piece taken), and it's made once they name a legal one.
    fn play_steps(&mut self, game: &GameView, screen: &GameScreen, legal: &[String]) -> Result<(), String> {
        if !game.yours || game.status != "active" || game.draw_offer == Some(true) {
            return Err("It isn't your move".into());
        }
        let app = screen.app.as_str();
        let at = crate::games::step_name(app, screen.cursor);
        if at.is_empty() {
            return Err("Put the cursor on a point".into());
        }
        let named: Vec<String> = screen.path.iter().map(|i| crate::games::step_name(app, *i)).collect();
        // The piece picked, again: put down.
        if named.len() == 1 && named[0] == at {
            self.set_checkers_path(Vec::new());
            return Ok(());
        }
        let next = [named.as_slice(), std::slice::from_ref(&at)].concat();
        let (done, going) = crate::games::steps_moves(legal, &next);
        if let Some(whole) = done {
            let whole = whole.clone();
            self.game_action(app, &screen.session, "move", Some(&whole))?;
            self.set_checkers_path(Vec::new());
        } else if !going.is_empty() {
            self.set_checkers_path([screen.path.as_slice(), &[screen.cursor]].concat());
        } else if !screen.path.is_empty() && {
            // Another piece of yours to move instead.
            let (done, going) = crate::games::steps_moves(legal, std::slice::from_ref(&at));
            done.is_some() || !going.is_empty()
        } {
            self.set_checkers_path(vec![screen.cursor]);
        } else {
            return Err(if screen.path.is_empty() { "Nothing to move from there" } else { "That isn't a legal move" }.into());
        }
        Ok(())
    }

    fn set_checkers_path(&mut self, path: Vec<usize>) {
        if let Some(screen) = self.game_view.as_mut() {
            screen.path = path;
        }
    }

    fn set_chess_from(&mut self, from: Option<usize>) {
        if let Some(screen) = self.game_view.as_mut() {
            screen.from = from;
        }
    }

    /// A click on the board: on a square or column, play it (as Enter does
    /// there).
    fn click_game(&mut self, at: Position) {
        let board = self.regions.game_board;
        let (Some(shown), Some(screen)) = (board.square(at), self.game_view.clone()) else { return };
        let Some(game) = self.games.view(&screen.app, &screen.session) else { return };
        // Black sees either board turned round.
        let index = match &game.board {
            Board::Chess { white: false, .. } | Board::Checkers { black: true, .. } => 63 - shown,
            // Morris' points are scattered over its 7 by 7 grid.
            Board::Morris { .. } => {
                let Some(point) = crate::games::MORRIS_POINTS.iter().position(|(_, col, row)| row * 7 + col == shown) else { return };
                point
            }
            _ => shown,
        };
        if let Some(screen) = self.game_view.as_mut() {
            screen.cursor = index;
        }
        if let Err(e) = self.play_at_cursor(&game) {
            self.warn(e);
        }
    }

    /// A game's prompt answered.
    pub(super) fn submit_game_prompt(&mut self, prompt: GamePrompt, text: &str) {
        let yes = text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes");
        let result = match prompt {
            GamePrompt::Move { app, session } if !text.is_empty() => self.game_action(&app, &session, "move", Some(text)),
            GamePrompt::ConfirmResign { app, session } if yes => self.game_action(&app, &session, "resign", None),
            GamePrompt::ConfirmForget { app, session } if yes => self.forget_game(&app, &session),
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.warn(e);
        }
    }

    /// Show a game in the Games tab, selected (a notification about it, or
    /// one just started).
    pub(crate) fn show_game(&mut self, app: &str, session: &str) {
        let Some(game) = self.games.view(app, session) else {
            return self.warn("That game isn't kept any more");
        };
        self.read(Target::Game { key: game.peer, app: app.to_string(), session: session.to_string() });
        self.picked = None;
        // Shown in the list, whatever it's filtered by.
        if !self.game_filter.shows(&game.status) {
            self.game_filter = GameFilter::All;
        }
        self.open_game(app, session);
        self.sync_game_selection();
        self.game_playing = false;
        self.switch_tab(super::Tab::Games);
    }

    /// `g` in a conversation: your newest game with them in the Games tab,
    /// or a new one with them if there's none.
    pub(super) fn games_with(&mut self, peer: &str) {
        match self.games.list(Some(peer)).first() {
            Some(game) => {
                let (app, session) = (game.app.clone(), game.session.clone());
                self.show_game(&app, &session);
            }
            None => {
                self.switch_tab(super::Tab::Games);
                self.open_game_menu(Some(peer.to_string()));
            }
        }
    }

    /// Make a game the one on the board (terminal UI), its cursor on the
    /// middle of the board.
    pub fn open_game(&mut self, app: &str, session: &str) {
        let cursor = match app {
            "ttt" => 4,
            "four_in_a_row" => 3,
            "mancala" => 0,
            // d5, by the four discs Reversi starts with.
            "reversi" => 27,
            "hex" => 60,
            "gomoku" => 112,
            "ultimate_ttt" | "go" => 40,
            "pente" => 180,
            // d2, a point to start from.
            "morris" => 10,
            "battleship" => 44,
            // e5, the middle square.
            "quoridor" => 144,
            // Paper.
            "rps" => 1,
            // c3v, the middle of the grid.
            "dots_and_boxes" => 59,
            _ => 52,
        };
        let placed = !matches!(app, "chess" | "checkers");
        self.game_view =
            Some(GameScreen { app: app.to_string(), session: session.to_string(), cursor, from: None, path: Vec::new(), placed });
        self.place_game_cursor();
    }

    /// Once a chess or checkers game has begun, put the cursor on a piece
    /// of yours: e2 or e7, the king's pawn, in chess; c3 or f6, on the
    /// front row, in checkers.
    pub fn place_game_cursor(&mut self) {
        let Some(screen) = self.game_view.as_ref().filter(|s| !s.placed) else { return };
        let Some(game) = self.games.view(&screen.app, &screen.session).filter(|g| g.status == "active") else { return };
        let cursor = match game.board {
            Board::Chess { white, .. } => {
                if white {
                    52
                } else {
                    12
                }
            }
            Board::Checkers { black, .. } => {
                if black {
                    21
                } else {
                    42
                }
            }
            _ => return,
        };
        if let Some(screen) = self.game_view.as_mut() {
            (screen.cursor, screen.placed) = (cursor, true);
        }
    }
}

/// Whether a game's moves can be typed (`t`): those with squares or lines
/// to name.
fn typed_moves(game: &GameView) -> bool {
    matches!(
        game.app.as_str(),
        "chess"
            | "checkers"
            | "hex"
            | "gomoku"
            | "reversi"
            | "dots_and_boxes"
            | "ultimate_ttt"
            | "go"
            | "pente"
            | "breakthrough"
            | "amazons"
            | "morris"
            | "quoridor"
            | "battleship"
    )
}

/// The Nine Men's Morris cursor (a point's index in `MORRIS_POINTS`) moved
/// one way: to the nearest point that way, keeping to its row or column
/// as far as it goes.
fn morris_step(cursor: usize, dx: i32, dy: i32) -> usize {
    let points = &crate::games::MORRIS_POINTS;
    let Some(&(_, col, row)) = points.get(cursor) else { return 0 };
    let (col, row) = (col as i32, row as i32);
    points
        .iter()
        .enumerate()
        .filter(|(_, (_, c, r))| {
            let (c, r) = (*c as i32, *r as i32);
            if dx != 0 { (c - col).signum() == dx } else { (r - row).signum() == dy }
        })
        .min_by_key(|(_, (_, c, r))| {
            let (c, r) = (*c as i32, *r as i32);
            if dx != 0 { ((r - row).abs(), (c - col).abs()) } else { ((c - col).abs(), (r - row).abs()) }
        })
        .map_or(cursor, |(i, _)| i)
}

/// The Dots and Boxes cursor (a place in the grid of eleven, row by row)
/// moved one way: along a row from line to line, over the dots and boxes
/// between; up or down, to the nearest line in the next row (lines to the
/// right of dots and lines up from them, by turns).
fn dots_step(cursor: usize, dx: i32, dy: i32) -> usize {
    let (row, col) = ((cursor / 11) as i32, (cursor % 11) as i32);
    let fine = |r: i32, c: i32| (0..11).contains(&r) && (0..11).contains(&c) && (r + c) % 2 == 1;
    let to = if dy == 0 {
        [(row, col + 2 * dx), (row, col + dx)].into_iter().find(|(r, c)| fine(*r, *c))
    } else {
        // Down to the line from the dot below on the right, up to the one on
        // the left: so one undoes the other.
        [(row + dy, col), (row + dy, col + dy), (row + dy, col - dy)].into_iter().find(|(r, c)| fine(*r, *c))
    };
    to.map_or(cursor, |(r, c)| (r * 11 + c) as usize)
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::config::Settings;
    use crate::games::Board::Hex;
    use crate::store::Store;

    const AMY: [u8; 16] = [0xa1; 16];
    const BEN: [u8; 16] = [0xb2; 16];

    struct Player {
        app: App,
        net: UnboundedReceiver<NetCommand>,
        dir: std::path::PathBuf,
        sent: u8,
    }

    fn player(name: &str, me: [u8; 16]) -> Player {
        let dir = std::env::temp_dir().join(format!("rettui-games-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        app.games.set_me(&me);
        Player { app, net, dir, sent: 0 }
    }

    /// What `from` sent last, as the other side receives it.
    fn arrives(from: &mut Player, me: [u8; 16]) -> lxmf::InboundMessage {
        let outgoing = loop {
            match from.net.try_recv().expect("a message was sent") {
                NetCommand::SendMessage { message, .. } => break *message,
                _ => continue,
            }
        };
        from.sent += 1;
        let fields = outgoing.game.as_deref().map(|e| crate::games::fields(e).into_iter().collect());
        lxmf::InboundMessage {
            id: Some([me[0].wrapping_add(from.sent); 32]),
            source: me,
            content: outgoing.content,
            timestamp: super::super::now(),
            verified: true,
            extras: lxmf::Extras { game: fields, ..Default::default() },
            ..Default::default()
        }
    }

    /// The last game move kept with `key`.
    fn last(player: &Player, key: [u8; 16]) -> Message {
        player.app.store.conversations[&hex::encode(key)].game_moves.last().unwrap().clone()
    }

    #[test]
    fn clicks_land_on_squares() {
        let board = BoardRegion {
            origin: Position::new(10, 5),
            cell: (3, 1),
            across: 8,
            down: 8,
            area: Rect::new(0, 0, 50, 20),
            ..Default::default()
        };
        assert_eq!(board.square(Position::new(10, 5)), Some(0));
        assert_eq!(board.square(Position::new(12, 5)), Some(0));
        assert_eq!(board.square(Position::new(13, 6)), Some(9));
        assert_eq!(board.square(Position::new(33, 12)), Some(63));
        assert_eq!(board.square(Position::new(34, 12)), None);
        assert_eq!(board.square(Position::new(9, 5)), None);
        // Four in a Row: anywhere in a column.
        let columns = BoardRegion { cell: (4, 7), across: 7, down: 1, ..board };
        assert_eq!(columns.square(Position::new(22, 11)), Some(3));
    }

    #[test]
    fn two_players_through_their_conversation() {
        let (mut amy, mut ben) = (player("amy", AMY), player("ben", BEN));
        let session = amy.app.start_game(&hex::encode(BEN), "four_in_a_row").unwrap();
        // Shown on Amy's side as a move of hers.
        let mine = last(&amy, BEN);
        assert_eq!(mine.opening(), "🎲 Four in a Row · Sent a challenge!");
        assert_eq!(mine.game.as_ref().unwrap().session, session);

        let challenge = arrives(&mut amy, AMY);
        ben.app.on_message(challenge.clone());
        let got = last(&ben, AMY);
        assert_eq!((got.game.unwrap().command.as_str(), got.notes.len()), ("challenge", 0));
        assert_eq!(ben.app.games.waiting(Some(&hex::encode(AMY))), 1);
        // Not in the conversation: nothing unread there, and a
        // notification that opens the game.
        let conversation = &ben.app.store.conversations[&hex::encode(AMY)];
        assert_eq!((conversation.messages.len(), conversation.unread, conversation.game_moves.len()), (0, 0, 1));
        let notified = ben.app.take_notifications();
        assert!(
            matches!(&notified[..], [n] if n.target == Target::Game { key: hex::encode(AMY), app: "four_in_a_row".into(), session: session.clone() })
        );
        // The same move again, in another message: not kept twice.
        ben.app.on_message(lxmf::InboundMessage { id: Some([0x77; 32]), ..challenge.clone() });
        ben.app.on_message(challenge);
        assert_eq!(ben.app.store.conversations[&hex::encode(AMY)].game_moves.len(), 1);

        ben.app.game_action("four_in_a_row", &session, "accept", None).unwrap();
        amy.app.on_message(arrives(&mut ben, BEN));
        amy.app.game_action("four_in_a_row", &session, "move", Some("3")).unwrap();
        ben.app.on_message(arrives(&mut amy, AMY));
        let view = ben.app.games.view("four_in_a_row", &session).unwrap();
        assert_eq!((view.summary.as_str(), view.board.rows()[5].as_str()), ("Your move", "   A   "));
        // Only the newest move each way is kept, once it's gone.
        let mut moves = amy.app.store.conversations[&hex::encode(BEN)].clone();
        moves.game_moves.iter_mut().for_each(|m| m.state = MessageState::Delivered);
        moves.keep_moves();
        let kept: Vec<_> = moves.game_moves.iter().map(|m| (m.incoming, m.game.as_ref().unwrap().command.clone())).collect();
        assert_eq!(kept, [(true, "accept".to_string()), (false, "move".to_string())]);
        // Out of turn: refused here, nothing sent.
        assert_eq!(amy.app.game_action("four_in_a_row", &session, "move", Some("4")).unwrap_err(), "Not your turn");
        assert!(amy.net.try_recv().is_err());

        // Saved, and there again after a restart.
        amy.app.finish_saves();
        let (mut again, problems) = crate::games::Games::load(&amy.app.paths.games);
        again.set_me(&AMY);
        assert!(problems.is_empty());
        assert_eq!(again.view("four_in_a_row", &session).unwrap().summary, "Their move");
        for p in [amy, ben] {
            std::fs::remove_dir_all(p.dir).unwrap();
        }
    }

    #[test]
    fn unchecked_moves_are_shown_not_played_and_failed_ones_go_again_the_same() {
        let (mut amy, mut ben) = (player("amy2", AMY), player("ben2", BEN));
        amy.app.start_game(&hex::encode(BEN), "ttt").unwrap();
        let challenge = arrives(&mut amy, AMY);
        let envelope = last(&amy, BEN).game.unwrap().envelope;
        ben.app.on_message(lxmf::InboundMessage { verified: false, ..challenge });
        // Not played, so not a game's move: in the conversation, saying so.
        let got = ben.app.store.conversations[&hex::encode(AMY)].messages.last().unwrap().clone();
        assert!(got.game.is_none() && got.notes[0].contains("couldn't be checked"), "{:?}", got.notes);
        assert_eq!(ben.app.games.list(None).len(), 0);

        // It failed: sent again, the very same move.
        let id = last(&amy, BEN).id;
        amy.app.store.conversations.get_mut(&hex::encode(BEN)).unwrap().game_moves.last_mut().unwrap().state =
            MessageState::Failed("no path".into());
        let game = amy.app.games.list(None).remove(0);
        assert_eq!(
            amy.app.game_notes(&game),
            ["Your last move wasn't sent (no path): it goes again when they announce, or sent again now"]
        );
        assert!(amy.app.game_unsent(&game));
        amy.app.game_action("ttt", &game.session, "resend", None).unwrap();
        let again = loop {
            match amy.net.try_recv().unwrap() {
                NetCommand::SendMessage { message, .. } => break message,
                _ => continue,
            }
        };
        assert_eq!(again.game.map(hex::encode), envelope);
        amy.app.store.conversations.get_mut(&hex::encode(BEN)).unwrap().game_moves.last_mut().unwrap().state =
            MessageState::Failed("no path".into());
        assert_eq!(amy.app.retry_message(&hex::encode(BEN), &id, DeliveryMode::Paper).unwrap_err(), "A game's move can't go on paper");
        // Forgotten: its moves with it.
        let session = game.session.clone();
        amy.app.forget_game("ttt", &session).unwrap();
        assert!(amy.app.store.conversations[&hex::encode(BEN)].game_moves.is_empty());
        for p in [amy, ben] {
            std::fs::remove_dir_all(p.dir).unwrap();
        }
    }

    fn press(player: &mut Player, code: KeyCode) {
        player.app.on_key(KeyEvent::new(code, crossterm::event::KeyModifiers::NONE));
    }

    #[test]
    fn chess_at_the_keyboard_from_the_picker_to_resigning() {
        let (mut amy, mut ben) = (player("chess-amy", AMY), player("chess-ben", BEN));
        amy.app.add_contact(&hex::encode(BEN)).unwrap();
        amy.app.tab = super::super::Tab::Games;
        // The picker: someone, then the game (moving about it first).
        press(&mut amy, KeyCode::Char('n'));
        for code in [KeyCode::Char('j'), KeyCode::Char('k'), KeyCode::Down, KeyCode::Up, KeyCode::Char('z')] {
            press(&mut amy, code);
        }
        press(&mut amy, KeyCode::Enter);
        let chess = amy.app.game_menu_items().iter().position(|item| matches!(item, MenuItem::New("chess"))).unwrap();
        amy.app.click_game_menu(Some(chess));
        assert!(amy.app.game_menu.is_none(), "{:?}", amy.app.log);
        let session = amy.app.game_view.as_ref().unwrap().session.clone();
        press(&mut amy, KeyCode::Char('n'));
        press(&mut amy, KeyCode::Esc);
        amy.app.click_game_menu(None);

        // Ben's side: accepted by its key.
        ben.app.on_message(arrives(&mut amy, AMY));
        ben.app.show_game("chess", &session);
        ben.app.tab = super::super::Tab::Games;
        press(&mut ben, KeyCode::Enter);
        press(&mut ben, KeyCode::Char('a'));
        amy.app.on_message(arrives(&mut ben, BEN));

        // Whoever is white moves e2e4 by the cursor: a piece that can't
        // move, an empty square, then the pawn and where it goes.
        amy.app.show_game("chess", &session);
        amy.app.tab = super::super::Tab::Games;
        press(&mut amy, KeyCode::Enter);
        let amy_white = matches!(amy.app.games.view("chess", &session).unwrap().board, Board::Chess { white: true, .. });
        let ((white, white_key), (black, black_key)) =
            if amy_white { ((&mut amy, AMY), (&mut ben, BEN)) } else { ((&mut ben, BEN), (&mut amy, AMY)) };
        let at = |player: &mut Player, cursor: usize| {
            player.app.game_view.as_mut().unwrap().cursor = cursor;
            press(player, KeyCode::Enter);
        };
        at(white, 60);
        at(white, 30);
        at(white, 52);
        at(white, 52);
        for code in [
            KeyCode::Up,
            KeyCode::Up,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Char('k'),
            KeyCode::Char('j'),
            KeyCode::Char('h'),
            KeyCode::Char('l'),
        ] {
            press(white, code);
        }
        at(white, 36);
        at(white, 52);
        at(white, 20);
        at(white, 36);
        black.app.on_message(arrives(white, white_key));

        // Black answers by typing the move, then offers a draw, which white
        // turns down; white resigns.
        press(black, KeyCode::Char('t'));
        for c in "e7e5".chars() {
            press(black, KeyCode::Char(c));
        }
        press(black, KeyCode::Enter);
        white.app.on_message(arrives(black, black_key));
        press(black, KeyCode::Char('D'));
        white.app.on_message(arrives(black, black_key));
        press(white, KeyCode::Char('n'));
        black.app.on_message(arrives(white, white_key));
        press(white, KeyCode::Char('R'));
        press(white, KeyCode::Char('y'));
        press(white, KeyCode::Enter);
        black.app.on_message(arrives(white, white_key));
        assert_ne!(black.app.games.view("chess", &session).unwrap().status, "active", "{:?}", white.app.log);

        // Off the board, forgotten (asked first).
        press(&mut amy, KeyCode::Char('q'));
        press(&mut amy, KeyCode::Char('c'));
        amy.app.tab = super::super::Tab::Games;
        press(&mut amy, KeyCode::Enter);
        press(&mut amy, KeyCode::Char('x'));
        press(&mut amy, KeyCode::Char('y'));
        press(&mut amy, KeyCode::Enter);
        assert!(amy.app.games.view("chess", &session).is_none());
        amy.app.show_game("chess", &session);
        for p in [amy, ben] {
            let _ = std::fs::remove_dir_all(p.dir);
        }
    }

    #[test]
    fn checkers_at_the_keyboard_a_jump_and_a_claim_not_yet_there() {
        let (mut amy, mut ben) = (player("checkers-amy", AMY), player("checkers-ben", BEN));
        let session = amy.app.start_game(&hex::encode(BEN), "checkers").unwrap();
        ben.app.on_message(arrives(&mut amy, AMY));
        ben.app.game_action("checkers", &session, "accept", None).unwrap();
        amy.app.on_message(arrives(&mut ben, BEN));
        for p in [&mut amy, &mut ben] {
            p.app.show_game("checkers", &session);
            p.app.game_playing = true;
        }
        let amy_black = matches!(amy.app.games.view("checkers", &session).unwrap().board, Board::Checkers { black: true, .. });
        let ((black, black_key), (white, white_key)) =
            if amy_black { ((&mut amy, AMY), (&mut ben, BEN)) } else { ((&mut ben, BEN), (&mut amy, AMY)) };
        // The cursor starts on a piece of yours, on the front row.
        assert_eq!(black.app.game_view.as_ref().unwrap().cursor, 21);
        assert_eq!(white.app.game_view.as_ref().unwrap().cursor, 42);
        let sq = |s: &str| (8 - (s.as_bytes()[1] - b'0') as usize) * 8 + (s.as_bytes()[0] - b'a') as usize;
        let at = |player: &mut Player, square: &str| {
            player.app.game_view.as_mut().unwrap().cursor = sq(square);
            press(player, KeyCode::Enter);
        };
        let warned = |player: &Player| player.app.log.back().cloned().unwrap_or_default();
        // Not White's move yet; a back-row piece of Black's is stuck.
        at(white, "c3");
        assert!(warned(white).contains("It isn't your move"), "{}", warned(white));
        at(black, "a7");
        assert!(warned(black).contains("That piece can't move"));
        // Picked, put down, picked again; an empty square first.
        at(black, "c5");
        assert!(warned(black).contains("Pick one of your pieces first"));
        at(black, "d6");
        at(black, "d6");
        assert!(black.app.game_view.as_ref().unwrap().path.is_empty());
        at(black, "d6");
        at(black, "d4");
        assert!(warned(black).contains("That isn't a legal move"));
        at(black, "c5");
        white.app.on_message(arrives(black, black_key));
        // White types its move.
        press(white, KeyCode::Char('t'));
        for c in "e3 d4".chars() {
            press(white, KeyCode::Char(c));
        }
        press(white, KeyCode::Enter);
        black.app.on_message(arrives(white, white_key));
        // Black must jump: another piece won't do; the jump, by its squares.
        at(black, "b6");
        assert!(warned(black).contains("You must jump"));
        at(black, "c5");
        at(black, "a3");
        assert!(warned(black).contains("You must jump"));
        at(black, "e3");
        white.app.on_message(arrives(black, black_key));
        let view = white.app.games.view("checkers", &session).unwrap();
        assert!(matches!(&view.board, Board::Checkers { last: Some(last), must_capture: true, .. } if last == "c5 e3"));
        // Moving about the board: White sees it the right way up.
        white.app.game_view.as_mut().unwrap().cursor = sq("d2");
        press(white, KeyCode::Up);
        assert_eq!(white.app.game_view.as_ref().unwrap().cursor, sq("d3"));
        press(black, KeyCode::Up);
        // No draw to claim; C does nothing.
        assert!(!view.actions.iter().any(|a| a == crate::games::DRAW_CLAIM));
        press(white, KeyCode::Char('C'));
        assert_eq!(white.app.games.view("checkers", &session).unwrap().status, "active");
        for p in [amy, ben] {
            let _ = std::fs::remove_dir_all(p.dir);
        }
    }

    /// A game of `app` begun by Amy and accepted by Ben, both on its board:
    /// (first mover, their key), (second, theirs), and the session.
    fn begun(amy: &mut Player, ben: &mut Player, app: &str) -> String {
        let session = amy.app.start_game(&hex::encode(BEN), app).unwrap();
        ben.app.on_message(arrives(amy, AMY));
        ben.app.game_action(app, &session, "accept", None).unwrap();
        amy.app.on_message(arrives(ben, BEN));
        for p in [amy, ben] {
            p.app.show_game(app, &session);
            p.app.game_playing = true;
        }
        session
    }

    #[test]
    fn the_newer_games_at_the_keyboard_and_by_clicks() {
        let (mut amy, mut ben) = (player("new-amy", AMY), player("new-ben", BEN));
        let warned = |player: &Player| player.app.log.back().cloned().unwrap_or_default();

        // Mancala: 3 sows your third pit; it ends in your store, so again.
        let session = begun(&mut amy, &mut ben, "mancala");
        let amy_first = amy.app.games.view("mancala", &session).unwrap().yours;
        let ((first, first_key), (second, _)) =
            if amy_first { ((&mut amy, AMY), (&mut ben, BEN)) } else { ((&mut ben, BEN), (&mut amy, AMY)) };
        press(second, KeyCode::Char('1'));
        assert!(warned(second).contains("It isn't your move"), "{}", warned(second));
        press(first, KeyCode::Char('3'));
        second.app.on_message(arrives(first, first_key));
        assert_eq!(first.app.games.view("mancala", &session).unwrap().summary, "Your move again");
        // The cursor along your pits; an empty one can't be sown.
        press(first, KeyCode::Right);
        press(first, KeyCode::Left);
        assert_eq!(first.app.game_view.as_ref().unwrap().cursor, 2);
        press(first, KeyCode::Enter);
        assert!(warned(first).contains("That pit is empty"), "{}", warned(first));

        // Dots and Boxes: the cursor goes from line to line.
        let session = begun(&mut amy, &mut ben, "dots_and_boxes");
        let amy_first = amy.app.games.view("dots_and_boxes", &session).unwrap().yours;
        let ((first, first_key), (second, second_key)) =
            if amy_first { ((&mut amy, AMY), (&mut ben, BEN)) } else { ((&mut ben, BEN), (&mut amy, AMY)) };
        let cursor = |p: &Player| p.app.game_view.as_ref().unwrap().cursor;
        assert_eq!(crate::games::dots_line(cursor(first) / 11, cursor(first) % 11).as_deref(), Some("c3v"));
        press(first, KeyCode::Right);
        assert_eq!(crate::games::dots_line(cursor(first) / 11, cursor(first) % 11).as_deref(), Some("d3v"));
        press(first, KeyCode::Down);
        assert_eq!(crate::games::dots_line(cursor(first) / 11, cursor(first) % 11).as_deref(), Some("d3h"));
        press(first, KeyCode::Up);
        assert_eq!(crate::games::dots_line(cursor(first) / 11, cursor(first) % 11).as_deref(), Some("d3v"));
        press(first, KeyCode::Enter);
        second.app.on_message(arrives(first, first_key));
        assert!(matches!(&second.app.games.view("dots_and_boxes", &session).unwrap().board,
            Board::DotsAndBoxes { last: Some(l), .. } if l == "d3v"));
        // Typed; then the same line again, refused.
        press(second, KeyCode::Char('t'));
        for c in "a1h".chars() {
            press(second, KeyCode::Char(c));
        }
        press(second, KeyCode::Enter);
        first.app.on_message(arrives(second, second_key));
        first.app.game_view.as_mut().unwrap().cursor = 10 * 11 + 1;
        press(first, KeyCode::Enter);
        assert!(warned(first).contains("That line's drawn already"), "{}", warned(first));
        // A dot isn't a line; the cursor stays on the grid at its edge.
        first.app.game_view.as_mut().unwrap().cursor = 0;
        press(first, KeyCode::Enter);
        assert!(warned(first).contains("Put the cursor on a line"));
        assert_eq!(dots_step(1, -1, 0), 1);
        assert_eq!(dots_step(1, 0, -1), 1);

        // Hex: a click on a cell; White swaps it with s.
        let session = begun(&mut amy, &mut ben, "hex");
        let amy_first = amy.app.games.view("hex", &session).unwrap().yours;
        let ((first, first_key), (second, second_key)) =
            if amy_first { ((&mut amy, AMY), (&mut ben, BEN)) } else { ((&mut ben, BEN), (&mut amy, AMY)) };
        first.app.regions.game_board = BoardRegion {
            origin: Position::new(10, 5),
            cell: (2, 1),
            across: 11,
            down: 11,
            skew: 1,
            area: Rect::new(0, 0, 60, 30),
            ..Default::default()
        };
        // Row 2 (rank 9) starts two to the right: its third cell, c9.
        first.app.click_games(Position::new(10 + 2 + 4, 7));
        second.app.on_message(arrives(first, first_key));
        assert!(matches!(&second.app.games.view("hex", &session).unwrap().board, Hex { last: Some(l), .. } if l == "c9"));
        press(second, KeyCode::Char('s'));
        first.app.on_message(arrives(second, second_key));
        let Hex { cells, .. } = first.app.games.view("hex", &session).unwrap().board else { panic!() };
        assert_eq!(cells[crate::games::grid_square_index("i3", 11).unwrap()], 'w');
        // Once swapped, s isn't a move: nothing's sent.
        press(first, KeyCode::Char('s'));
        assert!(first.net.try_recv().is_err());

        // Gomoku and Reversi from the cursor; Reversi only where it turns
        // a disc over.
        for app in ["gomoku", "reversi"] {
            let session = begun(&mut amy, &mut ben, app);
            let amy_first = amy.app.games.view(app, &session).unwrap().yours;
            let ((first, first_key), (second, _)) =
                if amy_first { ((&mut amy, AMY), (&mut ben, BEN)) } else { ((&mut ben, BEN), (&mut amy, AMY)) };
            if app == "reversi" {
                first.app.game_view.as_mut().unwrap().cursor = 0;
                press(first, KeyCode::Enter);
                assert!(warned(first).contains("must turn over"), "{}", warned(first));
                // d3: up from d5, twice.
                first.app.game_view.as_mut().unwrap().cursor = 27;
                press(first, KeyCode::Down);
                press(first, KeyCode::Down);
            }
            press(first, KeyCode::Enter);
            second.app.on_message(arrives(first, first_key));
            let view = second.app.games.view(app, &session).unwrap();
            assert_eq!(view.summary, "Your move", "{app}: {:?}", first.app.log);
            press(second, KeyCode::Enter);
            if app == "gomoku" {
                assert!(warned(second).contains("That point is taken"), "{}", warned(second));
            }
        }
        for p in [amy, ben] {
            let _ = std::fs::remove_dir_all(p.dir);
        }
    }

    #[test]
    fn the_lrgp_games_at_the_keyboard_and_by_clicks() {
        let (mut amy, mut ben) = (player("lrgp-amy", AMY), player("lrgp-ben", BEN));
        let warned = |player: &Player| player.app.log.back().cloned().unwrap_or_default();
        let at = |p: &mut Player, cursor: usize| p.app.game_view.as_mut().unwrap().cursor = cursor;
        macro_rules! sides {
            ($app:expr, $session:expr) => {{
                let amy_first = amy.app.games.view($app, $session).unwrap().yours;
                if amy_first { ((&mut amy, AMY), (&mut ben, BEN)) } else { ((&mut ben, BEN), (&mut amy, AMY)) }
            }};
        }
        let last_of = |p: &Player, app: &str, session: &str| match p.app.games.view(app, session).unwrap().board {
            Board::UltimateTicTacToe { last, .. }
            | Board::Go { last, .. }
            | Board::Pente { last, .. }
            | Board::Breakthrough { last, .. }
            | Board::Amazons { last, .. }
            | Board::Morris { last, .. }
            | Board::Quoridor { last, .. } => last,
            other => panic!("{other:?}"),
        };

        // Games played by a point under the cursor: its own colour, and one
        // taken can't be played again.
        for (app, point, taken) in
            [("ultimate_ttt", 40, "You can't play there"), ("go", 40, "You can't play there"), ("pente", 180, "That point is taken")]
        {
            let session = begun(&mut amy, &mut ben, app);
            let ((first, first_key), (second, second_key)) = sides!(app, &session);
            at(first, point);
            press(first, KeyCode::Enter);
            second.app.on_message(arrives(first, first_key));
            let name = crate::games::grid_square_name(point, if app == "pente" { 19 } else { 9 });
            assert_eq!(last_of(second, app, &session).as_deref(), Some(name.as_str()), "{app}");
            at(second, point);
            press(second, KeyCode::Enter);
            assert!(warned(second).contains(taken), "{app}: {}", warned(second));
            if app == "go" {
                // Two passes end it.
                press(second, KeyCode::Char('p'));
                first.app.on_message(arrives(second, second_key));
                press(first, KeyCode::Char('p'));
                second.app.on_message(arrives(first, first_key));
                assert!(
                    second.app.games.view(app, &session).unwrap().summary.starts_with("You won")
                        || second.app.games.view(app, &session).unwrap().summary.starts_with("They won")
                );
            }
        }

        // Breakthrough: a piece, then where it goes; the piece again puts it down.
        let session = begun(&mut amy, &mut ben, "breakthrough");
        let ((first, first_key), (second, _)) = sides!("breakthrough", &session);
        press(first, KeyCode::Enter);
        assert_eq!(first.app.game_view.as_ref().unwrap().path, vec![52]);
        press(first, KeyCode::Enter);
        assert!(first.app.game_view.as_ref().unwrap().path.is_empty());
        press(first, KeyCode::Enter);
        // Not a way to go: nothing sent.
        at(first, 4);
        press(first, KeyCode::Enter);
        assert!(warned(first).contains("isn't a legal move"), "{}", warned(first));
        // Another piece instead, then up from it.
        at(first, 51);
        press(first, KeyCode::Enter);
        assert_eq!(first.app.game_view.as_ref().unwrap().path, vec![51]);
        press(first, KeyCode::Up);
        press(first, KeyCode::Enter);
        second.app.on_message(arrives(first, first_key));
        assert_eq!(last_of(second, "breakthrough", &session).as_deref(), Some("d2d3"));
        // Nothing to move from an empty square.
        at(second, 28);
        press(second, KeyCode::Enter);
        assert!(warned(second).contains("Nothing to move from there"), "{}", warned(second));

        // Amazons: where from, where to, and the arrow.
        let session = begun(&mut amy, &mut ben, "amazons");
        let ((first, first_key), (second, _)) = sides!("amazons", &session);
        for point in [93, 33, 36] {
            at(first, point);
            press(first, KeyCode::Enter);
        }
        second.app.on_message(arrives(first, first_key));
        assert_eq!(last_of(second, "amazons", &session).as_deref(), Some("d1d7/g7"));

        // Nine Men's Morris: the cursor goes along the lines; a point is placed.
        assert_eq!((morris_step(10, 0, -1), morris_step(10, -1, 0), morris_step(10, 1, 0), morris_step(10, 0, 1)), (11, 3, 18, 9));
        assert_eq!((morris_step(7, 1, 0), morris_step(16, -1, 0), morris_step(0, -1, 0), morris_step(99, 1, 0)), (16, 7, 0, 0));
        let session = begun(&mut amy, &mut ben, "morris");
        let ((first, first_key), (second, _)) = sides!("morris", &session);
        press(first, KeyCode::Left);
        assert_eq!(first.app.game_view.as_ref().unwrap().cursor, 3);
        press(first, KeyCode::Enter);
        second.app.on_message(arrives(first, first_key));
        assert_eq!(last_of(second, "morris", &session).as_deref(), Some("b2"));
        // A click on a point (its 7 by 7 grid): d2 is column 3, row 5.
        second.app.regions.game_board = BoardRegion {
            origin: Position::new(4, 2),
            cell: (6, 2),
            across: 7,
            down: 7,
            area: Rect::new(0, 0, 60, 30),
            ..Default::default()
        };
        second.app.click_games(Position::new(4 + 18, 2 + 10));
        assert_eq!(last(second, first_key).game.unwrap().command, "move");
        // Between points: nothing.
        let sent = second.net.try_recv().is_ok();
        second.app.click_games(Position::new(4 + 6, 2));
        assert!(sent && second.net.try_recv().is_err());

        // Quoridor: a step, then a wall in a groove.
        let session = begun(&mut amy, &mut ben, "quoridor");
        let ((first, first_key), (second, second_key)) = sides!("quoridor", &session);
        at(first, 14 * 17 + 8);
        press(first, KeyCode::Enter);
        second.app.on_message(arrives(first, first_key));
        assert_eq!(last_of(second, "quoridor", &session).as_deref(), Some("e2"));
        at(second, 11 * 17 + 4);
        press(second, KeyCode::Enter);
        first.app.on_message(arrives(second, second_key));
        assert_eq!(last_of(first, "quoridor", &session).as_deref(), Some("c3h"));
        // Not a square or groove to play from here.
        at(first, 0);
        press(first, KeyCode::Enter);
        assert!(warned(first).contains("Put the cursor on a square"), "{}", warned(first));

        // Battleship: a shot at the square under the cursor.
        let session = begun(&mut amy, &mut ben, "battleship");
        let ((first, first_key), (second, second_key)) = sides!("battleship", &session);
        press(first, KeyCode::Enter);
        second.app.on_message(arrives(first, first_key));
        let Board::Battleship { theirs, .. } = first.app.games.view("battleship", &session).unwrap().board else { panic!() };
        assert_eq!(theirs[44], '?');
        // Their shot answers it; the cursor can't fire where you have.
        at(second, 0);
        press(second, KeyCode::Enter);
        first.app.on_message(arrives(second, second_key));
        at(first, 44);
        press(first, KeyCode::Enter);
        assert!(warned(first).contains("fired there already"), "{}", warned(first));
        let Board::Battleship { theirs, .. } = first.app.games.view("battleship", &session).unwrap().board else { panic!() };
        assert_ne!(theirs[44], '?');

        // Rock Paper Scissors: a throw by number.
        let session = begun(&mut amy, &mut ben, "rps");
        let ((first, first_key), (second, second_key)) = sides!("rps", &session);
        press(second, KeyCode::Char('1'));
        assert!(warned(second).contains("It isn't your move"), "{}", warned(second));
        press(first, KeyCode::Char('1'));
        second.app.on_message(arrives(first, first_key));
        press(second, KeyCode::Char('2'));
        first.app.on_message(arrives(second, second_key));
        // Rock locked in, paper played: shown with the next throw.
        press(first, KeyCode::Char('3'));
        second.app.on_message(arrives(first, first_key));
        let Board::Rps { rounds, my_score, .. } = second.app.games.view("rps", &session).unwrap().board else { panic!() };
        assert_eq!((rounds, my_score), (vec![('p', 'r')], 1));
        for p in [amy, ben] {
            let _ = std::fs::remove_dir_all(p.dir);
        }
    }

    #[test]
    fn clicks_in_a_board_of_small_boards_skip_the_gaps() {
        // Ultimate Tic-Tac-Toe: cells two wide, a gap of two and a row after every three.
        let grid = BoardRegion { origin: Position::new(2, 0), cell: (2, 1), across: 9, down: 9, groups: true, ..Default::default() };
        assert_eq!(grid.square(Position::new(2, 0)), Some(0));
        assert_eq!(grid.square(Position::new(7, 0)), Some(2));
        assert_eq!(grid.square(Position::new(8, 0)), None);
        assert_eq!(grid.square(Position::new(10, 0)), Some(3));
        assert_eq!(grid.square(Position::new(2, 3)), None);
        assert_eq!(grid.square(Position::new(2, 4)), Some(27));
        assert_eq!(grid.square(Position::new(2 + 21, 10)), Some(8 * 9 + 8));
    }

    #[test]
    fn clicks_on_dots_and_boxes_land_on_lines() {
        // Dots one wide, the lines between three.
        let grid = BoardRegion { origin: Position::new(2, 0), cell: (3, 1), across: 11, down: 11, narrow_even: true, ..Default::default() };
        assert_eq!(grid.square(Position::new(2, 0)), Some(0));
        assert_eq!(grid.square(Position::new(3, 0)), Some(1));
        assert_eq!(grid.square(Position::new(5, 0)), Some(1));
        assert_eq!(grid.square(Position::new(6, 1)), Some(13));
        assert_eq!(grid.square(Position::new(1, 0)), None);
    }

    #[test]
    fn moves_read_from_paper_or_unchecked_or_not_for_a_game() {
        let (mut amy, mut ben) = (player("paper-amy", AMY), player("paper-ben", BEN));
        amy.app.start_game(&hex::encode(BEN), "ttt").unwrap();
        let challenge = arrives(&mut amy, AMY);
        ben.app.on_message(lxmf::InboundMessage { paper: true, ..challenge.clone() });
        ben.app.on_message(lxmf::InboundMessage { id: Some([0x55; 32]), verified: false, ..challenge.clone() });
        let mut broken = challenge;
        broken.id = Some([0x56; 32]);
        if let Some(fields) = broken.extras.game.as_mut() {
            fields.values_mut().for_each(|v| v.truncate(1));
        }
        ben.app.on_message(broken);
        let conversation = &ben.app.store.conversations[&hex::encode(AMY)];
        let notes: Vec<String> = conversation.messages.iter().chain(&conversation.game_moves).flat_map(|m| m.notes.clone()).collect();
        assert!(notes.iter().any(|n| n.contains("read from paper")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("couldn't be checked")), "{notes:?}");
        for p in [amy, ben] {
            let _ = std::fs::remove_dir_all(p.dir);
        }
    }
}
