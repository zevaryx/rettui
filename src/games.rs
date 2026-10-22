//! Games over LXMF: LRGP (the Lightweight Reticulum Gaming Protocol), as
//! Ratspeak's lrgp-rs and lrgp-py play it. Tic-Tac-Toe, Four in a Row,
//! Dots and Boxes, Gomoku, Reversi, Mancala, Checkers, Chess and Hex, two
//! players, each move one LXMF message.
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

/// Not LRGP's: claiming a draw in chess or checkers (threefold
/// repetition, fifty or forty moves), sent as a draw offer that says why.
pub const DRAW_CLAIM: &str = "draw_claim";

/// The games there are, in the order they're offered.
pub const GAMES: [&str; 9] = ["ttt", "four_in_a_row", "dots_and_boxes", "gomoku", "reversi", "mancala", "checkers", "chess", "hex"];

/// A game's name, as shown.
pub fn game_name(app: &str) -> &str {
    match app {
        "ttt" => "Tic-Tac-Toe",
        "four_in_a_row" => "Four in a Row",
        "checkers" => "Checkers",
        "chess" => "Chess",
        "hex" => "Hex",
        "gomoku" => "Gomoku",
        "reversi" => "Reversi",
        "dots_and_boxes" => "Dots and Boxes",
        "mancala" => "Mancala",
        other => other,
    }
}

/// How a game is played, in a few sentences, shown beside its board.
pub fn game_rules(app: &str) -> &'static str {
    match app {
        "ttt" => {
            "Take turns putting your mark in an empty cell. Three of yours in a row, across, down or diagonally, wins. \
             A full board with no three in a row is a draw."
        }
        "four_in_a_row" => {
            "Take turns dropping a piece into a column: it falls to the lowest empty space. Four of yours in a row, \
             across, down or diagonally, wins. A full board with no four in a row is a draw."
        }
        "dots_and_boxes" => {
            "Take turns drawing a line between two neighbouring dots. A line that closes a box (or two) makes it yours, \
             and you draw another. When every box is claimed, whoever has more wins."
        }
        "gomoku" => {
            "Take turns putting a stone on an empty point. Five or more of yours in an unbroken row, across, down or \
             diagonally, wins. A full board with no five in a row is a draw."
        }
        "reversi" => {
            "Put a disc where it traps a straight line of theirs between it and one of yours, in any direction: every \
             trapped disc turns to your colour. Your possible moves are dotted. If you can't move, they go again; when \
             neither of you can, whoever has more discs wins."
        }
        "mancala" => {
            "Pick one of your pits (along the bottom): its seeds are sown one by one anticlockwise, into your store (on \
             the right) but not theirs. If the last lands in your store, go again; if it lands in an empty pit of yours, \
             it and the seeds facing it go to your store. When one side's pits are empty, each keeps what's left on \
             their side, and whoever has more in their store wins."
        }
        "checkers" => {
            "Pieces move one square diagonally forward, on the dark squares; one reaching the far row is crowned a \
             king, which moves forward or back. Take a piece by jumping over it to the empty square beyond, and keep \
             jumping while you can. Jumps must be taken. Whoever has no pieces, or no move, left loses."
        }
        "chess" => {
            "Each piece moves its own way: the king one square, the queen any distance in any direction, rooks \
             straight, bishops diagonally, knights in an L that jumps, pawns forward (taking diagonally, and becoming \
             another piece on the far rank). Win by checkmate: their king attacked, with no move that saves it. With no legal move but not in check, it's \
             stalemate, a draw."
        }
        "hex" => {
            "Take turns putting a stone on an empty cell. Black wins by joining the top and bottom edges with an \
             unbroken chain of their stones, White by joining left and right. There are no draws. White's first move \
             may be a swap: taking Black's first stone as their own."
        }
        _ => "",
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
    /// 64 squares, a8 first: `b`/`w` men, `B`/`W` kings, or ` `. Whether
    /// you're Black (who moves first, from the top), the moves you can make
    /// (square paths, as `a3 b4` or `a5 c3 e1`), the last move, and
    /// whether a capture must be made.
    Checkers { cells: Vec<char>, black: bool, legal: Vec<String>, last: Option<String>, must_capture: bool },
    /// 121 cells, a11 first: `b`, `w` or ` `. Whether you're Black (who
    /// moves first, and joins the top and bottom; White joins the left and
    /// right), the cells you can play (`f6`, and `swap` as White's first
    /// move), and the last move.
    Hex { cells: Vec<char>, black: bool, legal: Vec<String>, last: Option<String> },
    /// 225 points, a15 first: `b`, `w` or ` `; as Hex's otherwise.
    Gomoku { cells: Vec<char>, black: bool, legal: Vec<String>, last: Option<String> },
    /// 64 squares, a8 first: `b`, `w` or ` `; as Hex's otherwise, with each
    /// side's discs, and whether the last move left the other side without
    /// one (so its mover moves again).
    Reversi { cells: Vec<char>, black: bool, legal: Vec<String>, last: Option<String>, black_count: u64, white_count: u64, skipped: bool },
    /// The grid, eleven rows of eleven, top first: `+` a dot, `-` and `|`
    /// lines drawn (` ` not), and boxes `b`, `w` or ` `. Whether you're
    /// Black (who moves first), the lines you can draw (`c3h` to the right
    /// of c3, `c3v` up from it), the last, each side's boxes, and whether
    /// the last move claimed one (so its mover moves again).
    DotsAndBoxes {
        rows: Vec<String>,
        black: bool,
        legal: Vec<String>,
        last: Option<String>,
        black_boxes: u64,
        white_boxes: u64,
        extra_turn: bool,
    },
    /// The fourteen holes' seeds, as they're sown: Black's pits `a`–`f`,
    /// Black's store, White's `g`–`l`, White's store. Whether you're Black
    /// (who moves first), the pits you can sow, the last, and whether it
    /// ended in its mover's store (so they move again).
    Mancala { holes: Vec<u64>, black: bool, legal: Vec<String>, last: Option<String>, extra_turn: bool },
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
            Board::Checkers { cells, .. } | Board::Reversi { cells, .. } => chunk(cells, 8),
            Board::Hex { cells, .. } => chunk(cells, 11),
            Board::Gomoku { cells, .. } => chunk(cells, 15),
            Board::DotsAndBoxes { rows, .. } => rows.clone(),
            Board::Mancala { holes, .. } => vec![holes.iter().map(u64::to_string).collect::<Vec<_>>().join(",")],
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
    grid_square_name(index, 8)
}

/// A square's name on a board `size` across (`h8`), from its index: the
/// top left (`a` on the top rank) is 0.
pub fn grid_square_name(index: usize, size: usize) -> String {
    let file = (b'a' + (index % size) as u8) as char;
    let rank = size - index / size;
    format!("{file}{rank}")
}

/// A square's index on a board `size` across (the top left is 0), from its
/// name (`h8`).
pub fn grid_square_index(name: &str, size: usize) -> Option<usize> {
    let (&file, rank) = name.as_bytes().split_first()?;
    let file = file.checked_sub(b'a')? as usize;
    let rank: usize = std::str::from_utf8(rank).ok()?.parse().ok()?;
    (file < size && (1..=size).contains(&rank) && !name[1..].starts_with('0')).then(|| (size - rank) * size + file)
}

/// The Dots and Boxes line (`c3h`, `c3v`) at a place in its grid (row and
/// column of eleven, the top row 0), if there's a line there.
pub fn dots_line(row: usize, col: usize) -> Option<String> {
    let up = 10usize.checked_sub(row)?;
    if col > 10 {
        return None;
    }
    let file = |c: usize| (b'a' + (c / 2) as u8) as char;
    match (up % 2, col % 2) {
        // A dot row, between two dots: the line to the right.
        (0, 1) => Some(format!("{}{}h", file(col), up / 2 + 1)),
        // Between two dot rows, under a dot: the line up.
        (1, 0) => Some(format!("{}{}v", file(col), up / 2 + 1)),
        _ => None,
    }
}

/// Where a Dots and Boxes line (`c3h`) is in the grid: its row and column.
pub fn dots_place(line: &str) -> Option<(usize, usize)> {
    let (dot, way) = line.split_at(line.char_indices().last()?.0);
    let at = grid_square_index(dot, 6)?;
    let (row, col) = ((at / 6) * 2, (at % 6) * 2);
    match way {
        // Dots on the f file have no line to their right.
        "h" if col < 10 => Some((row, col + 1)),
        "v" => Some((row.checked_sub(1)?, col)),
        _ => None,
    }
}

/// The Mancala pits on your side (`a`–`f` for Black, `g`–`l` for White),
/// from your left: and their holes in `Board::Mancala`'s `holes`.
pub fn mancala_pits(black: bool) -> [(char, usize); 6] {
    std::array::from_fn(|i| if black { ((b'a' + i as u8) as char, i) } else { ((b'g' + i as u8) as char, i + 7) })
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
    /// it: a cell (`{"i": 4}`), a column (`{"c": 3}`), or a chess or
    /// checkers move (`{"m": "e2e4"}`, `{"m": "a5 c3 e1"}`).
    pub fn act(&self, app: &str, session: &str, command: &str, payload: HashMap<String, Value>) -> Result<Prepared, String> {
        let session_record = self.session(app, session).ok_or("There's no such game")?;
        if command == DRAW_CLAIM {
            let reason = session_record.metadata.get("draw_offer_reason").and_then(Json::as_str).unwrap_or_default();
            if reason.is_empty() {
                return Err("There's no draw to claim".into());
            }
            let claim = HashMap::from([("r".to_string(), Value::from(reason))]);
            return self.prepare(app, CMD_DRAW_OFFER, session, &claim, Some(&session_record.contact_hash));
        }
        self.prepare(app, command, session, &payload, Some(&session_record.contact_hash))
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
    let score = score(meta);
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
            "draw" => format!("Draw{}", draw_reason(meta, score)),
            "resign" if winner == me => "They resigned: you won".to_string(),
            "resign" => "You resigned".to_string(),
            // Checkers says a resignation is a win, for the other side.
            "win" if field("terminal_reason") == "rsn" && winner == me => "They resigned: you won".to_string(),
            "win" if field("terminal_reason") == "rsn" => "You resigned".to_string(),
            _ if winner.is_empty() => "Draw".to_string(),
            _ if winner == me => format!("You won{}", win_reason(meta, "they", score)),
            _ => format!("They won{}", win_reason(meta, "you", score.map(|(mine, theirs)| (theirs, mine)))),
        },
        STATUS_ACTIVE => match draw_offer {
            Some(true) => "They offered a draw".to_string(),
            _ if turn == me => format!("Your move{}{}", check_note(meta), again(meta, "they")),
            _ => format!("Their move{}", again(meta, "you")),
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
            // A draw that can be claimed ends the game at once.
            if draw_offer.is_none() && !field("draw_offer_reason").is_empty() {
                actions.push(DRAW_CLAIM);
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
    // lrgp lists the side to move's: only yours are offered.
    let legal = || -> Vec<String> {
        (status == STATUS_ACTIVE && turn == me && draw_offer != Some(true))
            .then(|| meta.get("legal_moves"))
            .flatten()
            .and_then(Json::as_array)
            .map(|moves| moves.iter().filter_map(Json::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let last = || Some(field("last_move")).filter(|m| !m.is_empty());
    let count = |key: &str| meta.get(key).and_then(Json::as_u64).unwrap_or(0);
    let flag = |key: &str| meta.get(key).and_then(Json::as_bool).unwrap_or(false);
    let black = field("my_color") == "b";
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
            legal: legal(),
            last: last(),
            check: meta.get("in_check").and_then(Json::as_bool).unwrap_or(false),
            moves: meta
                .get("moves")
                .and_then(Json::as_array)
                .map(|moves| moves.iter().filter_map(Json::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
        },
        "checkers" => {
            Board::Checkers { cells: cells(field("board"), 64), black, legal: legal(), last: last(), must_capture: flag("must_capture") }
        }
        "hex" => Board::Hex { cells: cells(field("board"), 121), black, legal: legal(), last: last() },
        "gomoku" => Board::Gomoku { cells: cells(field("board"), 225), black, legal: legal(), last: last() },
        "reversi" => Board::Reversi {
            cells: cells(field("board"), 64),
            black,
            legal: legal(),
            last: last(),
            black_count: count("black_count"),
            white_count: count("white_count"),
            skipped: flag("skipped_turn"),
        },
        "dots_and_boxes" => {
            let mut grid: Vec<char> = field("board").chars().map(|c| if c == '_' { ' ' } else { c }).collect();
            grid.resize(121, ' ');
            Board::DotsAndBoxes {
                rows: grid.chunks(11).map(|row| row.iter().collect()).collect(),
                black,
                legal: legal(),
                last: last(),
                black_boxes: count("black_boxes"),
                white_boxes: count("white_boxes"),
                extra_turn: flag("extra_turn"),
            }
        }
        "mancala" => {
            let mut holes: Vec<u64> = field("board").split(',').map(|n| n.trim().parse().unwrap_or(0)).collect();
            holes.resize(14, 0);
            Board::Mancala { holes, black, legal: legal(), last: last(), extra_turn: flag("extra_turn") }
        }
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

/// How a game was won, after "You won": `loser` is who lost (`they` or
/// `you`), `score` the winner's and the loser's discs, boxes or seeds.
fn win_reason(meta: &serde_json::Map<String, Json>, loser: &str, score: Option<(u64, u64)>) -> String {
    match meta.get("terminal_reason").and_then(Json::as_str) {
        Some("cm") => " by checkmate".into(),
        Some("np") => format!(": {loser} have no pieces left"),
        Some("nm") => format!(": {loser} can't move"),
        Some("con") => format!(" by joining {} sides", if loser == "they" { "your" } else { "their" }),
        Some("5ir") => " with five in a row".into(),
        Some("cnt") => score.map(|(won, lost)| format!(", {won}–{lost}")).unwrap_or_default(),
        _ => String::new(),
    }
}

/// How a game was drawn, after "Draw".
fn draw_reason(meta: &serde_json::Map<String, Json>, score: Option<(u64, u64)>) -> String {
    match meta.get("terminal_reason").and_then(Json::as_str) {
        Some("sm") => ": stalemate".into(),
        Some("ins") => ": not enough pieces to mate".into(),
        Some("3fr") => ": threefold repetition".into(),
        Some("50m") => ": fifty-move rule".into(),
        Some("40m") => ": forty-move rule".into(),
        Some("ful") => ": the board is full".into(),
        Some("cnt") => score.map(|(mine, theirs)| format!(", {mine}–{theirs}")).unwrap_or_default(),
        Some("agr") => " by agreement".into(),
        _ => String::new(),
    }
}

/// Your discs, boxes or seeds (in your store) and theirs, in a game that
/// counts them.
fn score(meta: &serde_json::Map<String, Json>) -> Option<(u64, u64)> {
    let pair = ["count", "boxes", "store"].iter().find_map(|what| {
        let get = |side: &str| meta.get(&format!("{side}_{what}")).and_then(Json::as_u64);
        Some((get("black")?, get("white")?))
    })?;
    let black = meta.get("my_color").and_then(Json::as_str) == Some("b");
    Some(if black { pair } else { (pair.1, pair.0) })
}

/// After "Your move" or "Their move": whether it's the same player's again
/// (a box claimed, the last seed in their store, or `other` with no move).
fn again(meta: &serde_json::Map<String, Json>, other: &str) -> String {
    let flag = |key: &str| meta.get(key).and_then(Json::as_bool) == Some(true);
    if flag("skipped_turn") {
        format!(" again: {other} can't move")
    } else if flag("extra_turn") {
        " again".into()
    } else {
        String::new()
    }
}

fn check_note(meta: &serde_json::Map<String, Json>) -> &'static str {
    if meta.get("in_check").and_then(Json::as_bool) == Some(true) { " (check)" } else { "" }
}

/// A move as the game takes it, from what a player picked: a cell (0–8)
/// for Tic-Tac-Toe, a column (0–6) for Four in a Row, a chess move (UCI;
/// a pawn reaching the far rank is made a queen unless it says otherwise),
/// a checkers move (its squares, `a3 b4` or `a5 c3 e1`; `a5-c3-e1` and
/// `a5xc3xe1` are taken too), a square for Hex, Gomoku and Reversi (`h8`;
/// Hex's `swap` too), a line for Dots and Boxes (`c3h`, `c3v`), or a pit
/// for Mancala (`a`–`l`).
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
        "checkers" => {
            let path: Vec<String> = picked
                .to_ascii_lowercase()
                .split(|c: char| c.is_whitespace() || c == '-' || c == 'x')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            let square = |s: &String| s.len() == 2 && matches!(s.as_bytes()[0], b'a'..=b'h') && matches!(s.as_bytes()[1], b'1'..=b'8');
            if path.len() < 2 || !path.iter().all(square) {
                return Err("Write a checkers move as its squares, e.g. a3 b4 (a5 c3 e1 for a double jump)".into());
            }
            Ok(HashMap::from([("m".to_string(), Value::from(path.join(" ")))]))
        }
        "hex" | "gomoku" | "reversi" => {
            let size = match app {
                "hex" => 11,
                "gomoku" => 15,
                _ => 8,
            };
            let square = picked.trim().to_ascii_lowercase();
            if !(app == "hex" && square == "swap") && grid_square_index(&square, size).is_none() {
                let last = grid_square_name(size - 1, size);
                let swap = if app == "hex" { ", or swap" } else { "" };
                return Err(format!("Write a square, a1 to {last}{swap}"));
            }
            Ok(HashMap::from([("m".to_string(), Value::from(square))]))
        }
        "dots_and_boxes" => {
            let line = picked.trim().to_ascii_lowercase();
            if dots_place(&line).is_none_or(|(row, col)| dots_line(row, col).as_deref() != Some(line.as_str())) {
                return Err("Write a line as a dot and which way: c3h to the right of c3, c3v up from it".into());
            }
            Ok(HashMap::from([("m".to_string(), Value::from(line))]))
        }
        "mancala" => {
            let pit = picked.trim().to_ascii_lowercase();
            if !matches!(pit.as_bytes(), [b'a'..=b'l']) {
                return Err("Write a pit, a to l".into());
            }
            Ok(HashMap::from([("m".to_string(), Value::from(pit))]))
        }
        _ => Err(format!("There's no game called {app}")),
    }
}

/// The checkers moves that go by `path` (square indices, a8 = 0, from the
/// piece moved on): the one that ends there, else those that go on.
pub fn checkers_moves<'a>(legal: &'a [String], path: &[usize]) -> Vec<&'a String> {
    let named: Vec<String> = path.iter().map(|i| square_name(*i)).collect();
    let named = named.join(" ");
    if let Some(done) = legal.iter().find(|m| **m == named) {
        return vec![done];
    }
    legal.iter().filter(|m| m.starts_with(&format!("{named} "))).collect()
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
    fn checkers_jumps_must_be_made_and_resigning() {
        let (amy, ben) = (player(AMY), player(BEN));
        let challenge = amy.challenge("checkers", &hex::encode(BEN)).unwrap();
        assert_eq!(challenge.text, "[LRGP Checkers] Sent a challenge!");
        let session = challenge.game.session.clone();
        played(deliver(&ben, AMY, &challenge));
        played(deliver(&amy, BEN, &ben.act("checkers", &session, "accept", HashMap::new()).unwrap()));
        let Board::Checkers { black: amy_black, legal, cells, .. } = amy.view("checkers", &session).unwrap().board else { panic!() };
        let (black, white, black_key, white_key) = if amy_black { (&amy, &ben, AMY, BEN) } else { (&ben, &amy, BEN, AMY) };
        // Black moves first: seven ways to go.
        assert_eq!(legal.len(), if amy_black { 7 } else { 0 });
        assert_eq!(cells.iter().filter(|c| **c == 'b').count(), 12);
        let play = |who: &Games, other: &Games, from: [u8; 16], path: &str| {
            let prepared = who.act("checkers", &session, "move", move_payload("checkers", path).unwrap()).unwrap();
            assert_eq!(prepared.text, format!("[LRGP Checkers] {}", path.replace('-', " ")));
            played(deliver(other, from, &prepared));
        };
        play(black, white, black_key, "d6 c5");
        play(white, black, white_key, "e3-d4");
        // Black must take: only the jump is offered.
        let Board::Checkers { legal, must_capture, .. } = black.view("checkers", &session).unwrap().board else { panic!() };
        assert_eq!((legal, must_capture), (vec!["c5 e3".to_string()], true));
        assert!(black.act("checkers", &session, "move", move_payload("checkers", "b6 a5").unwrap()).is_err());
        play(black, white, black_key, "c5 e3");
        let view = white.view("checkers", &session).unwrap();
        let Board::Checkers { legal, must_capture, last, cells, .. } = view.board else { panic!() };
        let mut legal = legal;
        legal.sort();
        assert_eq!((legal, must_capture, last.as_deref()), (vec!["d2 f4".to_string(), "f2 d4".to_string()], true, Some("c5 e3")));
        assert_eq!(cells.iter().filter(|c| **c == 'w').count(), 11);
        assert!(!view.actions.iter().any(|a| a == DRAW_CLAIM));
        assert_eq!(white.act("checkers", &session, DRAW_CLAIM, HashMap::new()).unwrap_err(), "There's no draw to claim");
        // From the squares picked: the one move that ends there.
        let at = |s: &str| (8 - (s.as_bytes()[1] - b'0') as usize) * 8 + (s.as_bytes()[0] - b'a') as usize;
        let Board::Checkers { legal, .. } = white.view("checkers", &session).unwrap().board else { panic!() };
        assert_eq!(checkers_moves(&legal, &[at("d2")]), vec!["d2 f4"]);
        assert_eq!(checkers_moves(&legal, &[at("d2"), at("f4")]), vec!["d2 f4"]);
        assert!(checkers_moves(&legal, &[at("d2"), at("c3")]).is_empty());
        play(white, black, white_key, "d2 f4");
        played(deliver(black, white_key, &white.act("checkers", &session, "resign", HashMap::new()).unwrap()));
        assert_eq!(white.view("checkers", &session).unwrap().summary, "You resigned");
        assert_eq!(black.view("checkers", &session).unwrap().summary, "They resigned: you won");
        assert!(move_payload("checkers", "a3").is_err());
        assert!(move_payload("checkers", "a3 z9").is_err());
        assert_eq!(move_payload("checkers", " A5xC3xE1 ").unwrap()["m"], Value::from("a5 c3 e1"));
        // How a checkers game ends, said of whoever lost.
        let meta = |reason: &str| serde_json::Map::from_iter([("terminal_reason".to_string(), Json::from(reason))]);
        assert_eq!(win_reason(&meta("np"), "they", None), ": they have no pieces left");
        assert_eq!(win_reason(&meta("nm"), "you", None), ": you can't move");
        assert_eq!(draw_reason(&meta("40m"), None), ": forty-move rule");
    }

    /// A game of `app` begun and accepted: its session, and who moves
    /// first (Black) and second, with their keys.
    fn begun<'a>(amy: &'a Games, ben: &'a Games, app: &str) -> (String, [(&'a Games, [u8; 16]); 2]) {
        let challenge = amy.challenge(app, &hex::encode(BEN)).unwrap();
        let session = challenge.game.session.clone();
        played(deliver(ben, AMY, &challenge));
        played(deliver(amy, BEN, &ben.act(app, &session, "accept", HashMap::new()).unwrap()));
        let first = amy.view(app, &session).unwrap().yours;
        (session, if first { [(amy, AMY), (ben, BEN)] } else { [(ben, BEN), (amy, AMY)] })
    }

    /// Play `picked` as `who`, and deliver it.
    fn play(app: &str, session: &str, who: (&Games, [u8; 16]), other: (&Games, [u8; 16]), picked: &str) -> Prepared {
        let prepared = who.0.act(app, session, "move", move_payload(app, picked).unwrap()).unwrap();
        played(deliver(other.0, who.1, &prepared));
        prepared
    }

    #[test]
    fn gomoku_five_in_a_row() {
        let (amy, ben) = (player(AMY), player(BEN));
        let (session, [black, white]) = begun(&amy, &ben, "gomoku");
        let Board::Gomoku { legal, black: is_black, cells, .. } = black.0.view("gomoku", &session).unwrap().board else { panic!() };
        assert_eq!((legal.len(), is_black, cells.len()), (225, true, 225));
        for (b, w) in [("h8", "a1"), ("i8", "a2"), ("j8", "a3"), ("k8", "a4")] {
            play("gomoku", &session, black, white, b);
            play("gomoku", &session, white, black, w);
        }
        // Taken: refused before it's sent.
        assert!(black.0.act("gomoku", &session, "move", move_payload("gomoku", "a1").unwrap()).is_err());
        let won = play("gomoku", &session, black, white, "l8");
        assert_eq!(won.text, "[LRGP Gomoku] l8 (game won)");
        assert_eq!(black.0.view("gomoku", &session).unwrap().summary, "You won with five in a row");
        let lost = white.0.view("gomoku", &session).unwrap();
        assert_eq!(lost.summary, "They won with five in a row");
        assert_eq!(lost.board.rows()[7], "       bbbbb   ");
        assert!(matches!(lost.board, Board::Gomoku { last: Some(ref m), ref legal, black: false, .. } if m == "l8" && legal.is_empty()));
        assert!(move_payload("gomoku", "p1").is_err() && move_payload("gomoku", "a16").is_err() && move_payload("gomoku", "a01").is_err());
        assert_eq!(move_payload("gomoku", " O15 ").unwrap()["m"], Value::from("o15"));
    }

    #[test]
    fn hex_swapped_then_joined_and_resigned() {
        let (amy, ben) = (player(AMY), player(BEN));
        let (session, [black, white]) = begun(&amy, &ben, "hex");
        play("hex", &session, black, white, "c7");
        // White may take Black's stone, mirrored: c7 becomes g3.
        let Board::Hex { legal, .. } = white.0.view("hex", &session).unwrap().board else { panic!() };
        assert_eq!((legal.len(), legal.last().map(String::as_str)), (121, Some("swap")));
        play("hex", &session, white, black, "swap");
        let view = black.0.view("hex", &session).unwrap();
        assert_eq!(view.summary, "Your move");
        let Board::Hex { cells, legal, last, .. } = view.board else { panic!() };
        assert_eq!(cells[grid_square_index("g3", 11).unwrap()], 'w');
        assert_eq!(cells.iter().filter(|c| **c != ' ').count(), 1);
        assert!(!legal.contains(&"swap".to_string()) && last.as_deref() == Some("swap"));
        // Black joins top and bottom down the f file; White plays along a.
        for rank in 1..=10 {
            play("hex", &session, black, white, &format!("f{rank}"));
            play("hex", &session, white, black, &format!("a{rank}"));
        }
        play("hex", &session, black, white, "f11");
        assert_eq!(black.0.view("hex", &session).unwrap().summary, "You won by joining your sides");
        assert_eq!(white.0.view("hex", &session).unwrap().summary, "They won by joining their sides");
        assert!(move_payload("hex", "l1").is_err());
        assert_eq!(move_payload("hex", "SWAP").unwrap()["m"], Value::from("swap"));
        assert!(move_payload("gomoku", "swap").is_err());
        // Another, resigned.
        let (session, [black, white]) = begun(&amy, &ben, "hex");
        played(deliver(white.0, black.1, &black.0.act("hex", &session, "resign", HashMap::new()).unwrap()));
        assert_eq!(white.0.view("hex", &session).unwrap().summary, "They resigned: you won");
    }

    #[test]
    fn reversi_counts_and_a_draw_by_agreement() {
        let (amy, ben) = (player(AMY), player(BEN));
        let (session, [black, white]) = begun(&amy, &ben, "reversi");
        let Board::Reversi { legal, black_count, white_count, .. } = black.0.view("reversi", &session).unwrap().board else { panic!() };
        assert_eq!((legal, black_count, white_count), (vec!["d3".to_string(), "c4".into(), "f5".into(), "e6".into()], 2, 2));
        assert!(black.0.act("reversi", &session, "move", move_payload("reversi", "a1").unwrap()).is_err());
        play("reversi", &session, black, white, "d3");
        let view = white.0.view("reversi", &session).unwrap();
        let Board::Reversi { cells, black_count, white_count, skipped, .. } = &view.board else { panic!() };
        assert_eq!((*black_count, *white_count, *skipped), (4, 1, false));
        assert_eq!(cells[grid_square_index("d4", 8).unwrap()], 'b');
        assert_eq!(view.board.rows()[5], "   b    ");
        // A draw, agreed.
        played(deliver(black.0, white.1, &white.0.act("reversi", &session, "draw_offer", HashMap::new()).unwrap()));
        played(deliver(white.0, black.1, &black.0.act("reversi", &session, "draw_accept", HashMap::new()).unwrap()));
        assert_eq!(white.0.view("reversi", &session).unwrap().summary, "Draw by agreement");
        // How a game of discs ends, and moves again when one side can't.
        let meta = |pairs: &[(&str, Json)]| serde_json::Map::from_iter(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())));
        let counted =
            meta(&[("my_color", "w".into()), ("black_count", 20.into()), ("white_count", 44.into()), ("terminal_reason", "cnt".into())]);
        assert_eq!(score(&counted), Some((44, 20)));
        assert_eq!(win_reason(&counted, "they", score(&counted)), ", 44–20");
        assert_eq!(
            draw_reason(
                &meta(&[
                    ("terminal_reason", "cnt".into()),
                    ("my_color", "b".into()),
                    ("black_count", 32.into()),
                    ("white_count", 32.into())
                ]),
                Some((32, 32))
            ),
            ", 32–32"
        );
        assert_eq!(draw_reason(&meta(&[("terminal_reason", "ful".into())]), None), ": the board is full");
        assert_eq!(again(&meta(&[("skipped_turn", true.into())]), "they"), " again: they can't move");
    }

    #[test]
    fn dots_and_boxes_a_box_claimed_moves_again() {
        let (amy, ben) = (player(AMY), player(BEN));
        let (session, [black, white]) = begun(&amy, &ben, "dots_and_boxes");
        let Board::DotsAndBoxes { legal, rows, .. } = black.0.view("dots_and_boxes", &session).unwrap().board else { panic!() };
        assert_eq!((legal.len(), rows.len(), rows[0].as_str()), (60, 11, "+ + + + + +"));
        play("dots_and_boxes", &session, black, white, "a1h");
        play("dots_and_boxes", &session, white, black, "a1v");
        play("dots_and_boxes", &session, black, white, "a2h");
        // White closes the box: White's, and White's move again.
        play("dots_and_boxes", &session, white, black, "b1v");
        let view = white.0.view("dots_and_boxes", &session).unwrap();
        assert_eq!((view.summary.as_str(), view.yours), ("Your move again", true));
        assert_eq!(black.0.view("dots_and_boxes", &session).unwrap().summary, "Their move again");
        let Board::DotsAndBoxes { rows, black_boxes, white_boxes, extra_turn, .. } = view.board else { panic!() };
        assert_eq!((&rows[8][..3], &rows[9][..3], &rows[10][..3]), ("+-+", "|w|", "+-+"));
        assert_eq!((black_boxes, white_boxes, extra_turn), (0, 1, true));
        // Lines and where they are in the grid.
        assert_eq!((dots_line(10, 1).as_deref(), dots_line(9, 0).as_deref(), dots_line(9, 1)), (Some("a1h"), Some("a1v"), None));
        assert_eq!((dots_place("a1h"), dots_place("a1v"), dots_place("f6h"), dots_place("a6v")), (Some((10, 1)), Some((9, 0)), None, None));
        assert_eq!(dots_line(0, 9).as_deref(), Some("e6h"));
        assert!(move_payload("dots_and_boxes", "f1h").is_err() && move_payload("dots_and_boxes", "c3").is_err());
        // Whatever is typed: refused, not a panic.
        assert!(move_payload("dots_and_boxes", "é").is_err() && move_payload("dots_and_boxes", "c3é").is_err());
        assert_eq!(dots_place(""), None);
        assert_eq!(move_payload("dots_and_boxes", "C3V").unwrap()["m"], Value::from("c3v"));
    }

    #[test]
    fn mancala_sowing_into_your_store_moves_again() {
        let (amy, ben) = (player(AMY), player(BEN));
        let (session, [black, white]) = begun(&amy, &ben, "mancala");
        let Board::Mancala { holes, legal, .. } = black.0.view("mancala", &session).unwrap().board else { panic!() };
        assert_eq!((holes, legal.len()), (vec![4, 4, 4, 4, 4, 4, 0, 4, 4, 4, 4, 4, 4, 0], 6));
        // c's four seeds: d, e, f, and the last in Black's store.
        play("mancala", &session, black, white, "c");
        let view = black.0.view("mancala", &session).unwrap();
        assert_eq!(view.summary, "Your move again");
        let Board::Mancala { holes, extra_turn, .. } = &view.board else { panic!() };
        assert_eq!((&holes[..7], *extra_turn), (&[4, 4, 0, 5, 5, 5, 1][..], true));
        // An empty pit can't be sown; nor can theirs.
        assert!(black.0.act("mancala", &session, "move", move_payload("mancala", "c").unwrap()).is_err());
        assert!(black.0.act("mancala", &session, "move", move_payload("mancala", "g").unwrap()).is_err());
        play("mancala", &session, black, white, "a");
        let Board::Mancala { legal, .. } = white.0.view("mancala", &session).unwrap().board else { panic!() };
        assert_eq!(legal, ["g", "h", "i", "j", "k", "l"]);
        assert_eq!(mancala_pits(false)[0], ('g', 7));
        assert_eq!(mancala_pits(true)[5], ('f', 5));
        assert!(move_payload("mancala", "m").is_err() && move_payload("mancala", "ab").is_err());
        assert_eq!(move_payload("mancala", "L").unwrap()["m"], Value::from("l"));
        assert_eq!(
            score(&serde_json::Map::from_iter([
                ("my_color".to_string(), Json::from("b")),
                ("black_store".to_string(), Json::from(30)),
                ("white_store".to_string(), Json::from(18)),
            ])),
            Some((30, 18))
        );
    }

    #[test]
    fn every_game_is_named_and_its_squares() {
        for app in GAMES {
            assert_ne!(game_name(app), app, "{app}");
            assert!(!game_rules(app).is_empty(), "{app}");
        }
        assert_eq!((grid_square_name(0, 11), grid_square_name(120, 11)), ("a11".to_string(), "k1".to_string()));
        assert_eq!((grid_square_index("a11", 11), grid_square_index("k1", 11), grid_square_index("l1", 11)), (Some(0), Some(120), None));
        assert_eq!((grid_square_index("a0", 8), grid_square_index("", 8), grid_square_index("a", 8)), (None, None, None));
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
        assert_eq!(game_rules("other"), "");
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
