//! Games with contacts (see `crate::games`): what arrives is played, and
//! each of your moves goes as a message in your conversation with them,
//! so it's sent, resent and shown as messages are.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};

use super::{App, PromptKind};
use crate::games::{Board, GAMES, GameRef, GameView, Prepared, Received};
use crate::lxmf::{self, DeliveryMode};
use crate::net::NetCommand;
use crate::store::{Message, MessageState};

/// The games menu (`g` in a conversation): your games with them, then a
/// new one of each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameMenu {
    pub peer: String,
    pub selected: usize,
}

/// A row of the games menu.
#[derive(Debug, Clone)]
pub enum MenuItem {
    Game(Box<GameView>),
    New(&'static str),
}

/// What a game's prompts ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GamePrompt {
    /// A chess move, typed (`e7e8n` to promote to a knight).
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
    /// The whole box.
    pub area: Rect,
}

impl BoardRegion {
    /// The square at `at` (as shown, row by row from the top left).
    pub fn square(&self, at: Position) -> Option<usize> {
        let (w, h) = self.cell;
        if w == 0 || h == 0 || at.x < self.origin.x || at.y < self.origin.y {
            return None;
        }
        let (col, row) = ((at.x - self.origin.x) / w, (at.y - self.origin.y) / h);
        (col < self.across && row < self.down).then_some((row * self.across + col) as usize)
    }
}

/// The game open over the conversation in the terminal UI: which, and the
/// square or column picked (a chess move's first square once chosen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameScreen {
    pub app: String,
    pub session: String,
    pub cursor: usize,
    pub from: Option<usize>,
    /// The cursor's been put on your side of a chess board (which side
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

    /// Forget a game.
    pub fn forget_game(&mut self, app: &str, session: &str) -> Result<(), String> {
        if !self.games.remove(app, session)? {
            return Err("There's no such game".into());
        }
        self.games_dirty = true;
        if self.game_view.as_ref().is_some_and(|g| g.app == app && g.session == session) {
            self.game_view = None;
        }
        Ok(())
    }

    /// Send a game's action to `key`, as a message in your conversation:
    /// its text says what it is for clients without games. Never on paper
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
        self.store.conversations.entry(key.to_string()).or_default().messages.push(Message {
            id: format!("local-{id}"),
            content: prepared.text.clone(),
            timestamp,
            state: MessageState::Sending,
            game: Some(prepared.game),
            ..Message::default()
        });
        self.store_dirty = true;
        self.keep_conversation_selection();
        self.update_policy(false);
        let appearance = self.own_appearance();
        let message =
            lxmf::Outgoing { appearance, game: Some(envelope), ..lxmf::Outgoing::text(to, prepared.text, Vec::new(), mode, timestamp) };
        self.send(NetCommand::SendMessage { id, message: Box::new(message) });
        Ok(())
    }

    /// The games menu's rows.
    pub fn game_menu_items(&self) -> Vec<MenuItem> {
        let Some(menu) = &self.game_menu else { return Vec::new() };
        let games = self.games.list(Some(&menu.peer)).into_iter().map(|g| MenuItem::Game(Box::new(g)));
        games.chain(GAMES.iter().map(|app| MenuItem::New(app))).collect()
    }

    /// Open the games menu for the open conversation.
    pub(super) fn open_game_menu(&mut self) {
        if let Some(peer) = self.active_conversation.clone() {
            self.game_menu = Some(GameMenu { peer, selected: 0 });
        }
    }

    /// Keys while a game, or the games menu, is open.
    pub(super) fn games_key(&mut self, key: KeyEvent) {
        if self.game_view.is_some() {
            self.game_view_key(key);
        } else if self.game_menu.is_some() {
            self.game_menu_key(key);
        }
    }

    fn game_menu_key(&mut self, key: KeyEvent) {
        let count = self.game_menu_items().len();
        let Some(menu) = self.game_menu.as_mut() else { return };
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => menu.selected = (menu.selected + 1).min(count.saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => menu.selected = menu.selected.saturating_sub(1),
            KeyCode::Enter => self.pick_menu_item(),
            KeyCode::Esc | KeyCode::Char('g') | KeyCode::Char('q') => self.game_menu = None,
            _ => {}
        }
    }

    /// Open the menu's selected game, or start the new one picked.
    fn pick_menu_item(&mut self) {
        let Some(menu) = self.game_menu.clone() else { return };
        match self.game_menu_items().into_iter().nth(menu.selected) {
            Some(MenuItem::Game(game)) => {
                self.game_menu = None;
                self.open_game(&game.app, &game.session);
            }
            Some(MenuItem::New(app)) => match self.start_game(&menu.peer, app) {
                Ok(session) => {
                    self.game_menu = None;
                    let name = self.store.display_name(&menu.peer);
                    self.notify(format!("Challenged {name} to {}", crate::games::game_name(app)));
                    self.open_game(app, &session);
                }
                Err(e) => self.warn(e),
            },
            None => {}
        }
    }

    /// A click on the games menu (its row, from the top of the list).
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
            KeyCode::Esc | KeyCode::Char('q') => {
                self.game_view = None;
                Ok(())
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Char('h' | 'l' | 'k' | 'j') => {
                self.move_game_cursor(&game, key.code);
                Ok(())
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.play_at_cursor(&game),
            // A cell or column by number, as on a keypad (1 the first).
            KeyCode::Char(c @ '1'..='9') if game.app != "chess" => {
                let picked = c as usize - '1' as usize;
                self.game_action(app, session, "move", Some(&picked.to_string()))
            }
            KeyCode::Char('a') if can("accept") => self.game_action(app, session, "accept", None),
            KeyCode::Char('d') if can("decline") => self.game_action(app, session, "decline", None),
            KeyCode::Char('D') if can("draw_offer") => self.game_action(app, session, "draw_offer", None),
            KeyCode::Char('D' | 'y') if can("draw_accept") => self.game_action(app, session, "draw_accept", None),
            KeyCode::Char('n') if can("draw_decline") => self.game_action(app, session, "draw_decline", None),
            KeyCode::Char('R') if can("resign") => {
                let prompt = GamePrompt::ConfirmResign { app: app.into(), session: session.into() };
                self.open_prompt(PromptKind::Game(prompt), &format!("Resign this game of {}? Type y", game.name), "");
                Ok(())
            }
            KeyCode::Char('t') if game.app == "chess" && game.yours && game.status == "active" => {
                let prompt = GamePrompt::Move { app: app.into(), session: session.into() };
                self.open_prompt(PromptKind::Game(prompt), "Your move, by its squares (e2e4; e7e8n promotes to a knight)", "");
                Ok(())
            }
            KeyCode::Char('x') => {
                let prompt = GamePrompt::ConfirmForget { app: app.into(), session: session.into() };
                let unfinished =
                    if matches!(game.status.as_str(), "pending" | "active") { " It isn't over: they won't be told." } else { "" };
                self.open_prompt(PromptKind::Game(prompt), &format!("Forget this game of {}?{unfinished} Type y", game.name), "");
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.warn(e);
        }
    }

    /// Move the cursor on the board, as it's shown (Black sees the chess
    /// board turned round).
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
        };
        let (dx, dy) = if turned { (-dx, -dy) } else { (dx, dy) };
        let (x, y) = ((screen.cursor % width) as i32 + dx, (screen.cursor / width) as i32 + dy);
        if (0..width as i32).contains(&x) && (0..height as i32).contains(&y) {
            screen.cursor = y as usize * width + x as usize;
        }
    }

    /// Play the square or column under the cursor: for chess, the first
    /// square picks a piece of yours, the second moves it there.
    fn play_at_cursor(&mut self, game: &GameView) -> Result<(), String> {
        let Some(screen) = self.game_view.clone() else { return Ok(()) };
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

    fn set_chess_from(&mut self, from: Option<usize>) {
        if let Some(screen) = self.game_view.as_mut() {
            screen.from = from;
        }
    }

    /// A click while a game is open: on a square or column, play it (as
    /// Enter does there); outside the box, close it.
    pub(super) fn click_game(&mut self, at: Position) {
        let board = self.regions.game_board;
        if !board.area.contains(at) {
            self.game_view = None;
            return;
        }
        let (Some(shown), Some(screen)) = (board.square(at), self.game_view.clone()) else { return };
        let Some(game) = self.games.view(&screen.app, &screen.session) else { return };
        // Black's chess board is shown turned round.
        let index = match &game.board {
            Board::Chess { white: false, .. } => 63 - shown,
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

    /// Open the game a move in the history is of (a click on it).
    pub(super) fn open_game_of(&mut self, app: &str, session: &str) {
        if self.games.view(app, session).is_none() {
            return self.warn("That game isn't kept any more");
        }
        self.picked = None;
        self.open_game(app, session);
    }

    /// Open the game the picked message (at `index`) is a move of.
    pub(super) fn open_picked_game(&mut self, index: usize) {
        let conversation = self.active_conversation.as_deref().and_then(|key| self.store.conversations.get(key));
        match conversation.and_then(|c| c.messages.get(index)).and_then(|m| m.game.clone()) {
            Some(game) => self.open_game_of(&game.app, &game.session),
            None => self.warn("That message isn't a game's"),
        }
    }

    /// Open a game over the conversation (terminal UI), its cursor on the
    /// middle of the board.
    pub fn open_game(&mut self, app: &str, session: &str) {
        let cursor = match app {
            "ttt" => 4,
            "four_in_a_row" => 3,
            _ => 52,
        };
        self.game_view =
            Some(GameScreen { app: app.to_string(), session: session.to_string(), cursor, from: None, placed: app != "chess" });
        self.place_game_cursor();
    }

    /// Once a chess game has begun, put the cursor on your first pawn to
    /// move: e2 for White, e7 for Black.
    pub fn place_game_cursor(&mut self) {
        let Some(screen) = self.game_view.as_ref().filter(|s| !s.placed) else { return };
        let Some(game) = self.games.view(&screen.app, &screen.session).filter(|g| g.status == "active") else { return };
        let Board::Chess { white, .. } = game.board else { return };
        if let Some(screen) = self.game_view.as_mut() {
            (screen.cursor, screen.placed) = (if white { 52 } else { 12 }, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::config::Settings;
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

    fn last(player: &Player, key: [u8; 16]) -> Message {
        player.app.store.conversations[&hex::encode(key)].messages.last().unwrap().clone()
    }

    #[test]
    fn clicks_land_on_squares() {
        let board = BoardRegion { origin: Position::new(10, 5), cell: (3, 1), across: 8, down: 8, area: Rect::new(0, 0, 50, 20) };
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
        // The same move again, in another message: not shown twice.
        let count = ben.app.store.conversations[&hex::encode(AMY)].messages.len();
        ben.app.on_message(lxmf::InboundMessage { id: Some([0x77; 32]), ..challenge });
        assert_eq!(ben.app.store.conversations[&hex::encode(AMY)].messages.len(), count);

        ben.app.game_action("four_in_a_row", &session, "accept", None).unwrap();
        amy.app.on_message(arrives(&mut ben, BEN));
        amy.app.game_action("four_in_a_row", &session, "move", Some("3")).unwrap();
        ben.app.on_message(arrives(&mut amy, AMY));
        let view = ben.app.games.view("four_in_a_row", &session).unwrap();
        assert_eq!((view.summary.as_str(), view.board.rows()[5].as_str()), ("Your move", "   A   "));
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
        let got = last(&ben, AMY);
        assert!(got.game.is_none() && got.notes[0].contains("couldn't be checked"), "{:?}", got.notes);
        assert_eq!(ben.app.games.list(None).len(), 0);

        // It failed: sent again, the very same move.
        let id = last(&amy, BEN).id;
        amy.app.store.conversations.get_mut(&hex::encode(BEN)).unwrap().messages.last_mut().unwrap().state =
            MessageState::Failed("no path".into());
        amy.app.retry_message(&hex::encode(BEN), &id, DeliveryMode::Auto).unwrap();
        let again = loop {
            match amy.net.try_recv().unwrap() {
                NetCommand::SendMessage { message, .. } => break message,
                _ => continue,
            }
        };
        assert_eq!(again.game.map(hex::encode), envelope);
        amy.app.store.conversations.get_mut(&hex::encode(BEN)).unwrap().messages.last_mut().unwrap().state =
            MessageState::Failed("no path".into());
        assert_eq!(amy.app.retry_message(&hex::encode(BEN), &id, DeliveryMode::Paper).unwrap_err(), "A game's move can't go on paper");
        for p in [amy, ben] {
            std::fs::remove_dir_all(p.dir).unwrap();
        }
    }
}
