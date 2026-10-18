rettui plays two-player games with your contacts over LXMF: **Tic-Tac-Toe**,
**Four in a Row** and **Chess**. Each move is a message in your conversation
with them, so a game goes wherever messages go (direct, or through a
propagation node to someone who's offline) and takes as long as you both
like.

The games are [LRGP](https://github.com/ratspeak/lrgp-rs), the Lightweight
Reticulum Gaming Protocol from Ratspeak, so you can play against anyone whose
client speaks it, not only rettui. Clients that don't see each move as a
message like `[LRGP Chess] e2e4`.

## Starting a game

In the **terminal UI**, open the conversation and press `g`: the menu lists
your games with them, newest first (a `●` marks those waiting on you), then a
new game of each kind. `Enter` opens a game or challenges them to a new one.

In the **web UI**, the conversation's **🎲 Games** button (or `g`) does the
same. When a game waits on you, the button says how many, and so does the
conversation's title in the terminal.

Your challenge goes as a message. They accept or decline it; whoever
challenged moves first (in Chess the colours are drawn when the challenge is
accepted, and White moves first).

## Playing

A game opens over the conversation with its board, where it stands ("Your
move", "Their move", "They offered a draw", "You won by checkmate", …) and
what you can do now. It's redrawn as their moves arrive.

| | Terminal UI | Web UI |
| --- | --- | --- |
| Tic-Tac-Toe | arrows and `Enter`, or `1`–`9` (left to right, top to bottom), or click a cell | click a cell |
| Four in a Row | `←→` and `Enter`, or `1`–`7`, or click a column | click a column |
| Chess | arrows and `Enter` on one of your pieces, then on where it goes; or a click on each; `t` to type a move by its squares (`e2e4`, `e7e8n` to promote to a knight) | click a piece (the squares it can go to are marked), then where it goes; a pawn reaching the far rank asks what it becomes |
| Accept / decline a challenge | `a` / `d` | buttons |
| Offer a draw | `D` | button |
| Answer a draw offered | `y` / `n` | buttons |
| Resign | `R` (asks first) | button (asks first) |
| Forget a game | `x` (asks first) | **Forget** |
| Close | `Esc` | `Esc` or × |

Black sees the chess board turned round, and in the terminal the cursor
starts on your e-pawn. Moves that aren't allowed (out of turn, a full column,
an illegal chess move) are refused before anything is sent.

In the history, each move shows as what it did (`🎲 Chess · e2e4`); clicking
it opens the game, and so does `g` on a [picked message](Messaging#message-actions).

## How it works

A game's message is an ordinary LXMF message whose text says what it is, with
two fields: `0xFB`, the protocol's name (`lrgp.v1`), and `0xFD`, the move (a
few dozen bytes of msgpack: the game, the command, the session, the move and a
random nonce). The players are their LXMF addresses, as other LRGP clients
name them, so whose turn it is matches on both sides.

- **Only checked moves are played.** A game's move is played only when its
  signature was checked against the sender's key. One that couldn't be
  checked (their key isn't known yet) shows with a note and isn't played. A
  move in a game from someone who isn't the other player, a copy of one
  already played, and one read from a paper message aren't played either.
- **Both sides check the rules.** If they send a move rettui's copy of the
  game refuses, it isn't played and they're told (an LRGP `error`), so the
  two can't drift apart unnoticed.
- **Sending:** your moves go as your messages to them do (by the
  conversation's [delivery mode](Messaging)), never on paper. One that
  fails is sent again exactly as it was, by **↻ Retry** (`t` on a picked
  message) or, with *Resend when they announce* on, when they next
  announce. Other clients may send theirs
  opportunistically (in one packet, without a Link), which rettui receives
  as any message.
- **Expiry:** a challenge not answered in a day, and a game with no move for
  a week, expire on each side by its own clock; nothing is sent.
- **Limits:** someone can have at most 16 unanswered challenges waiting on
  you, and everyone together 128; more show as messages but aren't played.

Blocked senders' moves are dropped with their messages, and the *Unknown
senders* setting applies to games as to messages.

## Where games are kept

In `games.json` in the [data folder](Data-and-Storage), with every game this
data folder has played (for each identity it has had), and in
[backups](Data-and-Storage#backups). **Forget** removes a game from it; a
game not over isn't ended for the other player, who isn't told.
