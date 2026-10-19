//! Games over LXMF: LRGP (the Lightweight Reticulum Gaming Protocol), as
//! Ratspeak's lrgp-rs and lrgp-py play it. Tic-Tac-Toe, Four in a Row and
//! Chess, two players, each move one LXMF message.
//!
//! A game message is an ordinary LXMF message whose text says what it is
//! for clients without games (`[LRGP Chess] e2e4`), with two fields:
//! `0xFB`, the string `lrgp.v1`, and `0xFD`, the envelope: a msgpack map of
//! the game and version (`chess.1`), the command (`challenge`, `move`, …),
//! the session id, the move and a nonce. Both are native msgpack values,
//! not bytes.
//!
//! lrgp's router keeps the rules and every game's state, refuses replays,
//! and binds each game to the two players: their LXMF addresses (hex), as
//! Python LXMF's `source_hash` gives them. So what a game says about whose
//! turn it is matches on both sides, whichever client each player uses.
//! What's received is only played when its signature was checked (the
//! sender's address is then theirs).
//!
//! The games are kept in `games.json` in the data folder: every session, as
//! lrgp keeps it, for every identity this data folder has had.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use lrgp::app_base::IncomingDispatch;
use lrgp::constants::{
    CMD_ACCEPT, CMD_CHALLENGE, CMD_DECLINE, CMD_DRAW_ACCEPT, CMD_DRAW_DECLINE, CMD_DRAW_OFFER, CMD_ERROR, CMD_RESIGN, FIELD_CUSTOM_META,
    FIELD_CUSTOM_TYPE, KEY_ERR_CODE, KEY_ERR_MSG, KEY_ERR_REF, STATUS_ACTIVE, STATUS_COMPLETED, STATUS_DECLINED, STATUS_EXPIRED,
    STATUS_PENDING,
};
use lrgp::router::LrgpRouter;
use lrgp::session::Session;
use rmpv::Value;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// The games there are, in the order they're offered.
pub const GAMES: [&str; 3] = ["ttt", "four_in_a_row", "chess"];

/// A game's name, as shown.
pub fn game_name(app: &str) -> &str {
    match app {
        "ttt" => "Tic-Tac-Toe",
        "four_in_a_row" => "Four in a Row",
        "chess" => "Chess",
        other => other,
    }
}

/// What a message of a game is: which game and session, and what it did.
/// Kept on the message (see `store::Message::game`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameRef {
    pub app: String,
    pub session: String,
    pub command: String,
    /// A sent one's envelope (field `0xFD`, msgpack, in hex), so sending it
    /// again sends the very same move, as LRGP asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<String>,
}

impl GameRef {
    /// What it did, in a few words, beside the game's name: the message's
    /// text less its `[LRGP …]` tag (which says what it was for clients
    /// without games).
    pub fn label(&self, text: &str) -> String {
        let said =
            text.trim().strip_prefix("[LRGP").and_then(|rest| rest.split_once(']')).map(|(_, said)| said.trim()).unwrap_or(text.trim());
        let said = if said.is_empty() { self.command.replace('_', " ") } else { said.to_string() };
        format!("{} · {said}", game_name(&self.app))
    }
}

/// An action to send: the message's text, and the fields that make it a
/// game's.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub game: GameRef,
    pub text: String,
}

/// The two LXMF fields a game message carries, ready to set as native
/// msgpack values: the protocol's name, and the envelope.
pub fn fields(envelope: &[u8]) -> [(u8, Vec<u8>); 2] {
    let mut name = Vec::new();
    rmpv::encode::write_value(&mut name, &Value::from(lrgp::constants::PROTOCOL_TYPE)).expect("writing to a Vec");
    [(FIELD_CUSTOM_TYPE, name), (FIELD_CUSTOM_META, envelope.to_vec())]
}

/// What came of a game message received.
#[derive(Debug)]
pub enum Received {
    /// Played (or, when `answer` is set, refused by the game: the answer
    /// is an `error` to send back).
    Applied { game: GameRef, answer: Option<Prepared>, refused: Option<String> },
    /// They said something we sent was wrong.
    TheirError { game: GameRef, text: String },
    /// A copy of one already received.
    Replay,
    /// Not played, and why.
    Rejected(String),
}

/// One game, as the UIs show it: from the point of view of `me`.
#[derive(Debug, Clone, Serialize)]
pub struct GameView {
    pub app: String,
    pub name: String,
    pub session: String,
    /// The other player (hex).
    pub peer: String,
    pub status: String,
    /// Whether it's your move now (or yours to accept).
    pub yours: bool,
    /// You challenged them.
    pub started: bool,
    /// Where it stands, in words: "Your move", "They won", ….
    pub summary: String,
    /// A draw offered and not answered: by you (`Some(false)`), or by
    /// them, for you to answer (`Some(true)`).
    pub draw_offer: Option<bool>,
    /// What you can do now (LRGP commands, `move` aside).
    pub actions: Vec<String>,
    pub board: Board,
    /// When it last changed (Unix seconds).
    pub updated: f64,
}

/// A game's board.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Board {
    /// Nine cells, `X`, `O` or ` `, row by row; yours are `mine`.
    TicTacToe { cells: Vec<char>, mine: char },
    /// Six rows of seven, `A`, `B` or ` `, top row first; yours are
    /// `mine`. Where the last piece fell.
    FourInARow { cells: Vec<char>, mine: char, last: Option<usize> },
    /// The position (FEN), whether you're White, the moves you can make
    /// (UCI, as `e2e4`), the last move, and whether the side to move is in
    /// check.
    Chess { fen: String, white: bool, legal: Vec<String>, last: Option<String>, check: bool, moves: Vec<String> },
}

impl Board {
    /// The squares as rows of characters (top first).
    #[cfg(test)]
    pub fn rows(&self) -> Vec<String> {
        let chunk = |cells: &[char], width: usize| cells.chunks(width).map(|row| row.iter().collect()).collect();
        match self {
            Board::TicTacToe { cells, .. } => chunk(cells, 3),
            Board::FourInARow { cells, .. } => chunk(cells, 7),
            Board::Chess { fen, .. } => chess_squares(fen).chunks(8).map(|row| row.iter().collect()).collect(),
        }
    }
}

/// A FEN position's 64 squares, a8 first: a piece's letter (white upper
/// case) or a space.
pub fn chess_squares(fen: &str) -> Vec<char> {
    let mut squares = Vec::with_capacity(64);
    for c in fen.split_whitespace().next().unwrap_or("").chars() {
        match c {
            '/' => {}
            '1'..='8' => squares.extend(std::iter::repeat_n(' ', c as usize - '0' as usize)),
            piece => squares.push(piece),
        }
    }
    squares.resize(64, ' ');
    squares
}

/// A chess square's name (`e4`) from its index, a8 = 0.
pub fn square_name(index: usize) -> String {
    let file = (b'a' + (index % 8) as u8) as char;
    let rank = 8 - index / 8;
    format!("{file}{rank}")
}

pub struct Games {
    router: LrgpRouter,
    /// This client's LXMF address (hex), once Reticulum is up.
    me: Option<String>,
}

impl Default for Games {
    fn default() -> Self {
        Self { router: LrgpRouter::with_builtin_apps(), me: None }
    }
}

impl Games {
    /// The games kept in `path`, if there are any; problems with single
    /// games are returned to log.
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        let games = Self::default();
        let mut problems = Vec::new();
        let sessions: Vec<Session> = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(sessions) => sessions,
                Err(e) => {
                    problems.push(format!("Couldn't read the games in {}: {e}", path.display()));
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                problems.push(format!("Couldn't read {}: {e}", path.display()));
                Vec::new()
            }
        };
        for session in sessions {
            let what = format!("{} game {}", game_name(&session.app_id), session.session_id);
            if let Err(e) = games.router.restore_session(session) {
                problems.push(format!("Couldn't restore the {what}: {e}"));
            }
        }
        (games, problems)
    }

    /// Every game, for `games.json`.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut sessions = Vec::new();
        for app in GAMES {
            sessions.extend(self.router.list_sessions(app, None).context("listing games")?);
        }
        sessions.sort_by(|a, b| a.created_at.total_cmp(&b.created_at).then_with(|| a.session_id.cmp(&b.session_id)));
        Ok(serde_json::to_vec_pretty(&sessions)?)
    }

    pub fn set_me(&mut self, me: &[u8; 16]) {
        self.me = Some(hex::encode(me));
    }

    fn me(&self) -> Result<&str, String> {
        self.me.as_deref().ok_or_else(|| "Games start once Reticulum is up".to_string())
    }

    /// Play a game message from `sender` (hex; its signature checked) with
    /// these fields (`0xFB`, `0xFD`, as received, msgpack).
    pub fn receive(&self, sender: &str, fields: &HashMap<u8, Vec<u8>>) -> Received {
        let Ok(me) = self.me() else { return Received::Rejected("Reticulum isn't up yet".into()) };
        let envelope = match lrgp::transport::extract_envelope(fields) {
            Ok(Some(envelope)) => envelope,
            Ok(None) => return Received::Rejected("not a game message".into()),
            Err(e) => return Received::Rejected(format!("malformed: {e}")),
        };
        let text = |key: &str| envelope.get(key).and_then(lrgp::envelope::value_as_str).unwrap_or_default().to_string();
        let (app_version, session, command) = (text("a"), text("s"), text("c"));
        let app = lrgp::envelope::parse_app_version(&app_version).map(|(app, _)| app.to_string()).unwrap_or(app_version);
        let game = GameRef { app: app.clone(), session: session.clone(), command: command.clone(), envelope: None };
        match self.router.dispatch_incoming(&envelope, sender, me) {
            Ok(IncomingDispatch::Replay) => Received::Replay,
            Ok(IncomingDispatch::RemoteError(e)) => {
                Received::TheirError { game, text: format!("{} says that wasn't right: {} ({})", game_name(&app), e.message, e.code) }
            }
            Ok(IncomingDispatch::Applied(result)) => {
                let Some(error) = result.error else { return Received::Applied { game, answer: None, refused: None } };
                let say = |key: &str| error.get(key).and_then(Json::as_str).unwrap_or_default().to_string();
                let (code, message) = (say(KEY_ERR_CODE), say(KEY_ERR_MSG));
                let refused = Some(format!("Not played: {}", if message.is_empty() { &code } else { &message }));
                // Told to them, so their game and ours can be put right.
                let payload = HashMap::from([
                    (KEY_ERR_CODE.to_string(), Value::from(if code.is_empty() { "invalid_move" } else { code.as_str() })),
                    (KEY_ERR_MSG.to_string(), Value::from(if message.is_empty() { "rejected" } else { message.as_str() })),
                    (KEY_ERR_REF.to_string(), Value::from(command.as_str())),
                ]);
                let answer = self.prepare(&app, CMD_ERROR, &session, &payload, None).ok();
                Received::Applied { game, answer, refused }
            }
            Err(e) => Received::Rejected(e.to_string()),
        }
    }

    /// Challenge `peer` (hex) to `app`.
    pub fn challenge(&self, app: &str, peer: &str) -> Result<Prepared, String> {
        self.prepare(app, CMD_CHALLENGE, "", &HashMap::new(), Some(peer))
    }

    /// Do `command` in a game of ours. A move is given as the game takes
    /// it: a cell (`{"i": 4}`), a column (`{"c": 3}`), or a chess move
    /// (`{"m": "e2e4"}`).
    pub fn act(&self, app: &str, session: &str, command: &str, payload: HashMap<String, Value>) -> Result<Prepared, String> {
        let peer = self.session(app, session).map(|s| s.contact_hash).ok_or("There's no such game")?;
        self.prepare(app, command, session, &payload, Some(&peer))
    }

    fn prepare(
        &self,
        app: &str,
        command: &str,
        session: &str,
        payload: &HashMap<String, Value>,
        peer: Option<&str>,
    ) -> Result<Prepared, String> {
        let me = self.me()?;
        if !GAMES.contains(&app) {
            return Err(format!("There's no game called {app}"));
        }
        let prepared = match peer {
            Some(peer) => self.router.dispatch_outgoing_to(app, 1, command, session, payload, me, peer),
            None => self.router.dispatch_outgoing(app, 1, command, session, payload, me),
        }
        .map_err(|e| said(&e))?;
        let envelope = lrgp::envelope::pack_to_bytes(&prepared.envelope).map_err(|e| e.to_string())?;
        Ok(Prepared {
            game: GameRef {
                app: app.to_string(),
                session: prepared.session_id,
                command: command.to_string(),
                envelope: Some(hex::encode(envelope)),
            },
            text: prepared.fallback_text,
        })
    }

    /// Forget a game (one that's over, usually).
    pub fn remove(&self, app: &str, session: &str) -> Result<bool, String> {
        let me = self.me()?;
        self.router.remove_session(app, session, me).map_err(|e| e.to_string())
    }

    fn session(&self, app: &str, session: &str) -> Option<Session> {
        let me = self.me.as_deref()?;
        self.router.with_app(app, |game| game.get_session_record(session, me)).flatten()
    }

    /// One game, as shown.
    pub fn view(&self, app: &str, session: &str) -> Option<GameView> {
        let me = self.me.as_deref()?;
        let state = self.router.with_app(app, |game| game.get_session_state(session, me))?;
        view_of(app, me, &state)
    }

    /// Our games with `peer` (hex), or all of them, newest first.
    pub fn list(&self, peer: Option<&str>) -> Vec<GameView> {
        let Some(me) = self.me.as_deref() else { return Vec::new() };
        let mut views: Vec<GameView> = GAMES
            .iter()
            .flat_map(|app| self.router.list_sessions(app, Some(me)).unwrap_or_default())
            .filter(|s| peer.is_none_or(|peer| s.contact_hash == peer))
            .filter_map(|s| self.view(&s.app_id, &s.session_id))
            .collect();
        views.sort_by(|a, b| b.updated.total_cmp(&a.updated));
        views
    }

    /// How many games wait on you (to accept, or to move), with `peer` or
    /// with anyone.
    pub fn waiting(&self, peer: Option<&str>) -> usize {
        self.list(peer).iter().filter(|g| g.yours).count()
    }
}

/// An lrgp error in words.
fn said(error: &lrgp::errors::LrgpError) -> String {
    use lrgp::errors::LrgpError;
    match error {
        LrgpError::Validation { message, .. } => message.clone(),
        LrgpError::SessionNotFound(_) => "There's no such game".into(),
        LrgpError::SessionExpired(_) => "That game has expired".into(),
        other => other.to_string(),
    }
}

fn view_of(app: &str, me: &str, state: &HashMap<String, Json>) -> Option<GameView> {
    let text = |value: Option<&Json>| value.and_then(Json::as_str).unwrap_or_default().to_string();
    let meta = state.get("metadata")?.as_object()?;
    let field = |key: &str| text(meta.get(key));
    let status = text(state.get("status"));
    let peer = text(state.get("contact_hash"));
    let started = text(state.get("initiator")) == me;
    let turn = field("turn");
    let winner = field("winner");
    let terminal = field("terminal");
    let draw_by = field("draw_offered_by");
    let draw_offer = (meta.get("draw_offered").and_then(Json::as_bool) == Some(true) && !draw_by.is_empty()).then(|| draw_by != me);

    let yours = match status.as_str() {
        STATUS_PENDING => !started,
        STATUS_ACTIVE => turn == me || draw_offer == Some(true),
        _ => false,
    };
    let summary = match status.as_str() {
        STATUS_PENDING if started => "Waiting for them to accept".to_string(),
        STATUS_PENDING => "They challenged you".to_string(),
        STATUS_DECLINED => if started { "They declined" } else { "You declined" }.to_string(),
        STATUS_EXPIRED => "Expired".to_string(),
        STATUS_COMPLETED => match terminal.as_str() {
            "draw" => format!("Draw{}", draw_reason(meta)),
            "resign" if winner == me => "They resigned: you won".to_string(),
            "resign" => "You resigned".to_string(),
            _ if winner.is_empty() => "Draw".to_string(),
            _ if winner == me => format!("You won{}", win_reason(meta)),
            _ => format!("They won{}", win_reason(meta)),
        },
        STATUS_ACTIVE => match draw_offer {
            Some(true) => "They offered a draw".to_string(),
            _ if turn == me => format!("Your move{}", check_note(meta)),
            _ => "Their move".to_string(),
        },
        other => other.to_string(),
    };
    let mut actions = Vec::new();
    match status.as_str() {
        STATUS_PENDING if !started => actions.extend([CMD_ACCEPT, CMD_DECLINE]),
        STATUS_ACTIVE => {
            match draw_offer {
                Some(true) => actions.extend([CMD_DRAW_ACCEPT, CMD_DRAW_DECLINE]),
                Some(false) => {}
                None => actions.push(CMD_DRAW_OFFER),
            }
            actions.push(CMD_RESIGN);
        }
        _ => {}
    }
    let marker = |key: &str| field(key).chars().next().unwrap_or(' ');
    let cells = |board: String, size: usize| {
        let mut cells: Vec<char> = board.chars().map(|c| if c == '_' { ' ' } else { c }).collect();
        cells.resize(size, ' ');
        cells
    };
    let board = match app {
        "ttt" => Board::TicTacToe { cells: cells(field("board"), 9), mine: marker("my_marker") },
        "four_in_a_row" => Board::FourInARow {
            cells: cells(field("board"), 42),
            mine: marker("my_marker"),
            last: meta.get("last_cell").and_then(Json::as_u64).map(|c| c as usize),
        },
        "chess" => Board::Chess {
            fen: field("fen"),
            white: field("my_color") == "w",
            // lrgp lists the side to move's: only yours are offered.
            legal: (status == STATUS_ACTIVE && turn == me && draw_offer != Some(true))
                .then(|| meta.get("legal_moves"))
                .flatten()
                .and_then(Json::as_array)
                .map(|moves| moves.iter().filter_map(Json::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
            last: Some(field("last_move")).filter(|m| !m.is_empty()),
            check: meta.get("in_check").and_then(Json::as_bool).unwrap_or(false),
            moves: meta
                .get("moves")
                .and_then(Json::as_array)
                .map(|moves| moves.iter().filter_map(Json::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
        },
        _ => return None,
    };
    Some(GameView {
        app: app.to_string(),
        name: game_name(app).to_string(),
        session: text(state.get("session_id")),
        peer,
        status,
        yours,
        started,
        summary,
        draw_offer,
        actions: actions.into_iter().map(str::to_string).collect(),
        board,
        updated: state.get("last_action_at").and_then(Json::as_f64).unwrap_or_default(),
    })
}

/// How a chess game was won, after "You won".
fn win_reason(meta: &serde_json::Map<String, Json>) -> &'static str {
    match meta.get("terminal_reason").and_then(Json::as_str) {
        Some("cm") => " by checkmate",
        _ => "",
    }
}

/// How a chess game was drawn, after "Draw".
fn draw_reason(meta: &serde_json::Map<String, Json>) -> &'static str {
    match meta.get("terminal_reason").and_then(Json::as_str) {
        Some("sm") => ": stalemate",
        Some("ins") => ": not enough pieces to mate",
        Some("3fr") => ": threefold repetition",
        Some("50m") => ": fifty-move rule",
        Some("agr") => " by agreement",
        _ => "",
    }
}

fn check_note(meta: &serde_json::Map<String, Json>) -> &'static str {
    if meta.get("in_check").and_then(Json::as_bool) == Some(true) { " (check)" } else { "" }
}

/// A move as the game takes it, from what a player picked: a cell (0–8)
/// for Tic-Tac-Toe, a column (0–6) for Four in a Row, a chess move (UCI;
/// a pawn reaching the far rank is made a queen unless it says otherwise).
pub fn move_payload(app: &str, picked: &str) -> Result<HashMap<String, Value>, String> {
    let number = |key: &str, max: u64| -> Result<HashMap<String, Value>, String> {
        let n: u64 = picked.trim().parse().map_err(|_| format!("Pick 0–{max}"))?;
        if n > max {
            return Err(format!("Pick 0–{max}"));
        }
        Ok(HashMap::from([(key.to_string(), Value::from(n))]))
    };
    match app {
        "ttt" => number("i", 8),
        "four_in_a_row" => number("c", 6),
        "chess" => {
            let uci = picked.trim().to_ascii_lowercase();
            let fine = uci.len() >= 4 && uci.len() <= 5 && uci.is_ascii();
            if !fine {
                return Err("Write a chess move as its squares, e.g. e2e4 (e7e8q to promote)".into());
            }
            Ok(HashMap::from([("m".to_string(), Value::from(uci))]))
        }
        _ => Err(format!("There's no game called {app}")),
    }
}

/// A chess move from one square to another (indices, a8 = 0), among the
/// legal ones: promoting to a queen, as most players would.
pub fn chess_move(legal: &[String], from: usize, to: usize) -> Option<String> {
    let uci = format!("{}{}", square_name(from), square_name(to));
    legal.iter().find(|m| **m == uci).or_else(|| legal.iter().find(|m| **m == format!("{uci}q"))).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMY: [u8; 16] = [0xaa; 16];
    const BEN: [u8; 16] = [0xbb; 16];

    fn player(me: [u8; 16]) -> Games {
        let mut games = Games::default();
        games.set_me(&me);
        games
    }

    /// Fields as an LXMF message brings them.
    fn wire(prepared: &Prepared) -> HashMap<u8, Vec<u8>> {
        fields(&hex::decode(prepared.game.envelope.as_ref().unwrap()).unwrap()).into_iter().collect()
    }

    fn deliver(to: &Games, from: [u8; 16], prepared: &Prepared) -> Received {
        to.receive(&hex::encode(from), &wire(prepared))
    }

    fn played(received: Received) {
        assert!(matches!(received, Received::Applied { refused: None, .. }), "{received:?}");
    }

    #[test]
    fn tic_tac_toe_from_challenge_to_win_on_both_sides() {
        let (amy, ben) = (player(AMY), player(BEN));
        let challenge = amy.challenge("ttt", &hex::encode(BEN)).unwrap();
        assert_eq!(challenge.text, "[LRGP TTT] Sent a challenge!");
        played(deliver(&ben, AMY, &challenge));
        let session = challenge.game.session.clone();
        let theirs = ben.view("ttt", &session).unwrap();
        assert_eq!(
            (theirs.summary.as_str(), theirs.yours, theirs.actions.clone()),
            ("They challenged you", true, vec!["accept".into(), "decline".into()])
        );
        assert_eq!(amy.view("ttt", &session).unwrap().summary, "Waiting for them to accept");

        played(deliver(&amy, BEN, &ben.act("ttt", &session, "accept", HashMap::new()).unwrap()));
        // The challenger goes first.
        assert_eq!(amy.view("ttt", &session).unwrap().summary, "Your move");
        assert_eq!(ben.view("ttt", &session).unwrap().summary, "Their move");

        for (who, cell) in [(&amy, 4), (&ben, 0), (&amy, 2), (&ben, 6), (&amy, 8), (&ben, 1), (&amy, 5)] {
            let (other, from) = if std::ptr::eq(who, &amy) { (&ben, AMY) } else { (&amy, BEN) };
            let prepared = who.act("ttt", &session, "move", move_payload("ttt", &cell.to_string()).unwrap()).unwrap();
            played(deliver(other, from, &prepared));
        }
        let (won, lost) = (amy.view("ttt", &session).unwrap(), ben.view("ttt", &session).unwrap());
        assert_eq!((won.summary.as_str(), lost.summary.as_str()), ("You won", "They won"));
        assert_eq!(won.board.rows(), vec!["OOX", " XX", "O X"]);
        assert!(won.actions.is_empty() && !won.yours);
    }

    #[test]
    fn out_of_turn_and_replays_and_strangers_are_refused() {
        let (amy, ben) = (player(AMY), player(BEN));
        let challenge = amy.challenge("four_in_a_row", &hex::encode(BEN)).unwrap();
        played(deliver(&ben, AMY, &challenge));
        let session = challenge.game.session.clone();
        // The same message again is a replay, not a second challenge.
        assert!(matches!(deliver(&ben, AMY, &challenge), Received::Replay));
        played(deliver(&amy, BEN, &ben.act("four_in_a_row", &session, "accept", HashMap::new()).unwrap()));
        // Not Ben's turn: refused before it's sent.
        let early = ben.act("four_in_a_row", &session, "move", move_payload("four_in_a_row", "3").unwrap());
        assert_eq!(early.unwrap_err(), "Not your turn");
        // A move in their game from someone else isn't played.
        let mover = amy.act("four_in_a_row", &session, "move", move_payload("four_in_a_row", "3").unwrap()).unwrap();
        assert!(matches!(deliver(&ben, [0xcc; 16], &mover), Received::Rejected(_)));
        played(deliver(&ben, AMY, &mover));
        let view = ben.view("four_in_a_row", &session).unwrap();
        assert_eq!(view.board.rows()[5], "   A   ");
        assert!(matches!(view.board, Board::FourInARow { last: Some(38), mine: 'B', .. }));
        assert_eq!((view.summary.as_str(), view.yours), ("Your move", true));
    }

    #[test]
    fn draw_offers_and_resigning() {
        let (amy, ben) = (player(AMY), player(BEN));
        let challenge = amy.challenge("ttt", &hex::encode(BEN)).unwrap();
        let session = challenge.game.session.clone();
        played(deliver(&ben, AMY, &challenge));
        played(deliver(&amy, BEN, &ben.act("ttt", &session, "accept", HashMap::new()).unwrap()));
        played(deliver(&ben, AMY, &amy.act("ttt", &session, "draw_offer", HashMap::new()).unwrap()));
        let (offered, asked) = (amy.view("ttt", &session).unwrap(), ben.view("ttt", &session).unwrap());
        assert_eq!(offered.draw_offer, Some(false));
        assert_eq!((asked.summary.as_str(), asked.yours, asked.draw_offer), ("They offered a draw", true, Some(true)));
        assert_eq!(asked.actions, vec!["draw_accept", "draw_decline", "resign"]);
        played(deliver(&amy, BEN, &ben.act("ttt", &session, "draw_decline", HashMap::new()).unwrap()));
        played(deliver(&ben, AMY, &amy.act("ttt", &session, "resign", HashMap::new()).unwrap()));
        assert_eq!(amy.view("ttt", &session).unwrap().summary, "You resigned");
        assert_eq!(ben.view("ttt", &session).unwrap().summary, "They resigned: you won");
    }

    #[test]
    fn chess_board_moves_and_mate() {
        let (amy, ben) = (player(AMY), player(BEN));
        let challenge = amy.challenge("chess", &hex::encode(BEN)).unwrap();
        let session = challenge.game.session.clone();
        played(deliver(&ben, AMY, &challenge));
        played(deliver(&amy, BEN, &ben.act("chess", &session, "accept", HashMap::new()).unwrap()));
        let Board::Chess { white: amy_white, legal, .. } = amy.view("chess", &session).unwrap().board else { panic!() };
        let (white, black, white_key, black_key) = if amy_white { (&amy, &ben, AMY, BEN) } else { (&ben, &amy, BEN, AMY) };
        assert_eq!(legal.len(), if amy_white { 20 } else { 0 });
        // Fool's mate, the moves picked as squares.
        let squares = |m: &str| {
            let at = |s: &str| (8 - (s.as_bytes()[1] - b'0') as usize) * 8 + (s.as_bytes()[0] - b'a') as usize;
            (at(&m[..2]), at(&m[2..4]))
        };
        for (i, m) in ["f2f3", "e7e5", "g2g4", "d8h4"].iter().enumerate() {
            let (who, other, from) = if i % 2 == 0 { (white, black, white_key) } else { (black, white, black_key) };
            let Board::Chess { legal, .. } = who.view("chess", &session).unwrap().board else { panic!() };
            let (a, b) = squares(m);
            let uci = chess_move(&legal, a, b).unwrap();
            played(deliver(other, from, &who.act("chess", &session, "move", move_payload("chess", &uci).unwrap()).unwrap()));
        }
        assert_eq!(black.view("chess", &session).unwrap().summary, "You won by checkmate");
        let lost = white.view("chess", &session).unwrap();
        assert_eq!(lost.summary, "They won by checkmate");
        assert_eq!(lost.board.rows()[4], "      Pq");
    }

    #[test]
    fn kept_and_restored() {
        let (amy, ben) = (player(AMY), player(BEN));
        let challenge = amy.challenge("ttt", &hex::encode(BEN)).unwrap();
        played(deliver(&ben, AMY, &challenge));
        let path = std::env::temp_dir().join(format!("rettui-games-{}.json", std::process::id()));
        std::fs::write(&path, ben.encode().unwrap()).unwrap();
        let (mut again, problems) = Games::load(&path);
        let _ = std::fs::remove_file(&path);
        assert!(problems.is_empty(), "{problems:?}");
        again.set_me(&BEN);
        let view = again.view("ttt", &challenge.game.session).unwrap();
        assert_eq!((view.peer, view.summary.as_str()), (hex::encode(AMY), "They challenged you"));
        // And it goes on from there.
        let accept = again.act("ttt", &challenge.game.session, "accept", HashMap::new()).unwrap();
        played(deliver(&amy, BEN, &accept));
        assert_eq!(again.waiting(None), 0);
        assert_eq!(amy.waiting(Some(&hex::encode(BEN))), 1);
    }

    #[test]
    fn labels_and_moves_picked() {
        let game = GameRef { app: "chess".into(), session: "0011223344556677".into(), command: "move".into(), envelope: None };
        assert_eq!(game.label("[LRGP Chess] e2e4"), "Chess · e2e4");
        assert_eq!(game.label(""), "Chess · move");
        assert!(move_payload("ttt", "9").is_err());
        assert_eq!(move_payload("four_in_a_row", "6").unwrap()["c"], Value::from(6));
        assert_eq!(square_name(0), "a8");
        assert_eq!(square_name(63), "h1");
        assert_eq!(chess_squares("8/8/8/8/8/8/8/K6k w - - 0 1")[56], 'K');
    }

    #[test]
    fn games_that_cant_be_read_are_said_and_the_rest_kept() {
        let dir = std::env::temp_dir().join(format!("rettui-games-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Not JSON; not a file; neither stops the rest.
        let broken = dir.join("broken.json");
        std::fs::write(&broken, "not json").unwrap();
        let (_, problems) = Games::load(&broken);
        assert!(problems[0].contains("Couldn't read the games"), "{problems:?}");
        let (_, problems) = Games::load(&dir);
        assert!(problems[0].contains("Couldn't read"), "{problems:?}");
        assert!(Games::load(&dir.join("missing.json")).1.is_empty());
        assert_eq!(game_name("other"), "other");
        // Not set up yet: nothing played.
        let games = Games::default();
        assert!(matches!(games.receive("ab", &HashMap::new()), Received::Rejected(_)));
        let mut games = Games::default();
        games.set_me(&[1; 16]);
        assert!(matches!(games.receive("ab", &HashMap::new()), Received::Rejected(why) if why.contains("not a game")));
        let garbled = HashMap::from([(FIELD_CUSTOM_TYPE, vec![0xc1]), (FIELD_CUSTOM_META, vec![0xc1])]);
        assert!(matches!(games.receive("ab", &garbled), Received::Rejected(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
