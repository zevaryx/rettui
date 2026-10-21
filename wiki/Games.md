rettui plays two-player games with your contacts over LXMF: **Tic-Tac-Toe**,
**Four in a Row**, **Dots and Boxes**, **Gomoku**, **Reversi**, **Mancala**,
**Checkers**, **Chess** and **Hex**. Each move goes as a message to them, so a
game goes wherever messages go (direct, or through a propagation node to
someone who's offline) and takes as long as you both like. The moves aren't
in your conversation with them: the Games tab shows the games.

The games are [LRGP](https://github.com/ratspeak/lrgp-rs), the Lightweight
Reticulum Gaming Protocol from Ratspeak, so you can play against anyone whose
client speaks it, not only rettui. Clients that don't see each move as a
message like `[LRGP Chess] e2e4`.

## The Games tab

Games have a tab of their own, second in the sidebar, between Messages and
Channels (`2`). It lists your games with everyone, newest first: who with
(their face, or their icon), the game, and where it stands ("Your move",
"Their move", "They challenged you", "You won by checkmate", …), with a `●`
on those waiting on you. The tab's badge counts those. Beside the list is the
selected game's board, with what you can do now, redrawn as their moves
arrive, and how your last move went while it's being sent (or if it
failed).

Above the list, **All**, **Active**, **Pending** and **Completed** filter it
(`f` goes through them): games under way, challenges not yet answered (yours
or theirs), and games that are over (won, lost, drawn, declined or expired).

A move that arrives notifies you as a message does (unless the conversation's
notifications are off, or it's on screen); opening the notification shows the
game.

## Starting a game

**`n`** in the Games tab (**+ New game** in the web UI) asks who with (from
your conversations), then which game. From a conversation, **`g`** (the
**🎲 Games** button in the web UI) goes to your newest game with them in the
Games tab, or starts a new one with them if there's none; the conversation's
title (the button, in the web UI) says when a game waits on you.

Your challenge goes as a message. They accept or decline it; whoever
challenged moves first in Tic-Tac-Toe and Four in a Row. In the others who
moves first is drawn when the challenge is accepted: White in Chess, Black
in the rest (in Dots and Boxes and Mancala, Black is just the one who goes
first).

## Playing

In the terminal, `↑↓` pick a game in the list and `Enter` (or a click on the
board) puts you on its board; `Esc` goes back to the list. On the board, the
keys below play; in the list, `f` filters it, `c` opens the conversation and
`x` forgets the game. In the web UI, pick a game and click on its board.

| | Terminal UI | Web UI |
| --- | --- | --- |
| Tic-Tac-Toe | arrows and `Enter`, or `1`–`9` (left to right, top to bottom), or click a cell | click a cell |
| Four in a Row | `←→` and `Enter`, or `1`–`7`, or click a column | click a column |
| Chess | arrows and `Enter` on one of your pieces, then on where it goes; or a click on each; `t` to type a move by its squares (`e2e4`, `e7e8n` to promote to a knight) | click a piece (the squares it can go to are marked), then where it goes; a pawn reaching the far rank asks what it becomes |
| Checkers | arrows and `Enter` on one of your pieces, then on each square it lands on; or a click on each; `t` to type a move by its squares (`a3 b4`, `a5 c3 e1` for a double jump) | click a piece, then each square it lands on (the next ones are marked) |
| Dots and Boxes | arrows (from line to line) and `Enter`, or a click on a line; `t` to type one (`c3h` from c3 to the right, `c3v` from c3 up) | click a line |
| Gomoku | arrows and `Enter`, or a click on a point; `t` to type one (`h8`) | click a point |
| Reversi | arrows and `Enter`, or a click on a square (your moves are dotted); `t` to type one (`d3`) | click a dotted square |
| Mancala | `←→` and `Enter`, or `1`–`6` (your pits, from your left), or a click on a pit | click one of your pits |
| Hex | arrows and `Enter`, or a click on a cell; `t` to type one (`f6`); `s` to swap | click a cell; **Swap** |
| Accept / decline a challenge | `a` / `d` | buttons |
| Offer a draw | `D` | button |
| Answer a draw offered | `y` / `n` | buttons |
| Claim a draw (Chess, Checkers) | `C` | **Claim a draw** |
| Resign | `R` (asks first) | button (asks first) |
| Send a move that failed again | `r` | **↻ Send again** |
| Forget a game | `x` (asks first) | **Forget** |
| Back to the list | `Esc` | ← (on a phone) |

Black sees the chess board turned round, and in the terminal the cursor
starts on your e-pawn. Moves that aren't allowed (out of turn, a full column,
an illegal chess move, a taken point, an empty pit) are refused before
anything is sent. Every game can be drawn by agreement (**Offer a draw**).

Checkers is American checkers (English draughts) on the dark squares: Black
starts at the top and moves first, so Black sees the board turned round,
your pieces at the bottom either way. Men move one square diagonally
forward, kings (ringed) either way; a man reaching the far row is crowned,
which ends its move. Jumps must be taken (the board says "you must jump"),
and once you start one you finish it: with several jumps you pick each
landing square in turn, and where only one way goes on the rest is made for
you. Whoever has no piece, or no move, left loses.

**Dots and Boxes** is played on 5×5 boxes (6×6 dots). Draw a line
between two dots; one that closes a box (or two) makes it yours, and you
draw another. Once every box is claimed, whoever has more wins.

**Gomoku** is freestyle, on a 15×15 board: five or more of your stones in a
row (across, down or diagonally) wins; a full board with no five is a draw.
In the terminal your stones are in your colour, theirs in yellow; in the web
UI they're black and white.

**Reversi** (Othello) is on 8×8, starting with four discs in the middle. A
disc must be put where it traps a line of theirs between it and one of
yours, in any of the eight directions, and every trapped disc turns over.
Your possible moves are dotted. When a player can't move, the other goes
again, and the board says so ("Your move again: they can't move"); when
neither can, whoever has more discs wins.

**Mancala** is Kalah, six pits a side with four seeds in each. Your pits
are along the bottom, from your left, and your store is on the right. Pick
a pit of yours: its seeds are sown one by one anticlockwise, into your store
but not theirs. If the last lands in your store you go again; if it lands in
an empty pit of yours, it and the seeds in the pit facing it go to your
store. When either side's pits are all empty, the seeds left go to their
owner's store, and whoever has more in their store wins (24–24 is a draw).

**Hex** is on an 11×11 rhombus of hexagons. Black wants an unbroken chain
of stones from the top edge to the bottom one (the lettered edges, in
Black's colour), White one from left to right (the numbered edges). Hex
can't be drawn. Since going first is an advantage, White's first move may
be **swap**: Black's stone becomes White's, mirrored across the diagonal
(`c7` becomes `g3`), and Black moves next.

Draws in Chess and Checkers come by agreement, or by claim: when the same
position has come up three times, or (Chess) fifty moves have gone without a
capture or a pawn move, or (Checkers) forty moves without a capture or a man
moving, **Claim a draw** (`C`) ends the game at once. The other side checks
the claim; one that doesn't hold up is only an offer.

## How it works

A game's message is an ordinary LXMF message whose text says what it is, with
two fields: `0xFB`, the protocol's name (`lrgp.v1`), and `0xFD`, the move (a
few dozen bytes of msgpack: the game, the command, the session, the move and a
random nonce). The players are their LXMF addresses, as other LRGP clients
name them, so whose turn it is matches on both sides.

- **Only checked moves are played.** A game's move is played only when its
  signature was checked against the sender's key. One that couldn't be
  checked (their key isn't known yet) shows in the conversation, as a
  message with a note, and isn't played. A
  move in a game from someone who isn't the other player, a copy of one
  already played, and one read from a paper message aren't played either.
- **Both sides check the rules.** If they send a move rettui's copy of the
  game refuses, it isn't played and they're told (an LRGP `error`), so the
  two can't drift apart unnoticed.
- **Sending:** your moves go as your messages to them do (by the
  conversation's [delivery mode](Messaging)), never on paper. One that
  fails is sent again exactly as it was, by `r` on the board (**↻ Send
  again** in the web UI) or, with *Resend when they announce* on, when they
  next announce. Other clients may send theirs
  opportunistically (in one packet, without a Link), which rettui receives
  as any message.
- **Expiry:** a challenge not answered in a day, and a game with no move for
  a week, expire on each side by its own clock; nothing is sent.
- **Limits:** someone can have at most 16 unanswered challenges waiting on
  you, and everyone together 128; more show in the conversation as messages
  but aren't played.

Blocked senders' moves are dropped with their messages, and the *Unknown
senders* setting applies to games as to messages.

## Where games are kept

In `games.json` in the [data folder](Data-and-Storage), with every game this
data folder has played (for each identity it has had), and in
[backups](Data-and-Storage#backups). The moves themselves are kept with the
conversation (only those being sent or that failed, and the newest each way).
**Forget** removes a game and its moves; a game not over isn't ended for the
other player, who isn't told. Game moves an earlier version kept in the
conversation's history move out of it when rettui starts.
