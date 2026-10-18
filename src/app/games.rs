//! Games with contacts (see `crate::games`): what arrives is played, and
//! each of your moves goes as a message in your conversation with them,
//! so it's sent, resent and shown as messages are.

use std::collections::HashMap;

use super::App;
use crate::games::{GameRef, Prepared, Received};
use crate::lxmf::{self, DeliveryMode};
use crate::net::NetCommand;
use crate::store::{Message, MessageState};

/// The game open over the conversation in the terminal UI: which, and the
/// square or column picked (a chess move's first square once chosen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameScreen {
    pub app: String,
    pub session: String,
    pub cursor: usize,
    pub from: Option<usize>,
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

    /// Open a game over the conversation (terminal UI), its cursor on the
    /// middle of the board.
    pub fn open_game(&mut self, app: &str, session: &str) {
        let cursor = match app {
            "ttt" => 4,
            "four_in_a_row" => 3,
            // e2 for White, e7 for Black, the usual first pawns.
            _ => match self.games.view(app, session).map(|g| g.board) {
                Some(crate::games::Board::Chess { white: false, .. }) => 12,
                _ => 52,
            },
        };
        self.game_view = Some(GameScreen { app: app.to_string(), session: session.to_string(), cursor, from: None });
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
