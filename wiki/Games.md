rettui plays two-player games with your contacts over LXMF: **Tic-Tac-Toe**,
**Four in a Row**, **Dots and Boxes**, **Gomoku**, **Reversi**, **Mancala**,
**Checkers**, **Chess**, **Hex**, **Ultimate Tic-Tac-Toe**, **Go**,
**Battleship**, **Quoridor**, **Nine Men's Morris**, **Breakthrough**,
**Amazons**, **Pente** and **Rock Paper Scissors**. Each move goes as a message to them, so a
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
first). In Battleship, whoever shoots first is drawn, and in Rock Paper
Scissors, whoever leads.

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
| Ultimate Tic-Tac-Toe | arrows and `Enter`, or a click on a square (the ones you can play are dotted in your colour); `t` to type one (`e5`) | click a ringed square |
| Go | arrows and `Enter`, or a click on a point; `p` to pass; `t` to type one (`e5`, or `pass`) | click a point; **Pass** |
| Pente | arrows and `Enter`, or a click on a point; `t` to type one (`j10`) | click a point |
| Breakthrough | arrows and `Enter` on one of your pieces, then on where it goes (the squares it can go to are marked); or a click on each; `t` to type a move by its two squares (`a2b3`) | click a piece (the squares it can go to are marked), then where it goes |
| Amazons | arrows and `Enter` on one of your amazons, then where it slides to, then where its arrow goes (each time the squares open to it are marked); or a click on each; `t` to type it (`d1d7/g7`: from d1 to d7, arrow to g7) | click an amazon, then where it goes, then the arrow's square |
| Nine Men's Morris | arrows (along the lines) and `Enter` on an empty point to place a piece, or on one of your pieces and then where it slides; after a mill, then the piece to take; or a click on each; `t` to type a move (`d2`, `d2-d3`, `d2xg7`, `d2-d3xg7`) | click an empty point to place a piece, or a piece and then where it goes, then the piece to take after a mill |
| Quoridor | arrows (over squares and the grooves between them) and `Enter` on a square to step there, or on a groove to place a wall; or a click; `t` to type a move (`e2`; `c3h` or `c3v` for a wall) | click a ringed square to step there, or a groove (it lights up) to place a wall |
| Battleship | arrows and `Enter` on the square to fire at, in their waters (on the left); `t` to type one (`e5`) | click a square of their waters |
| Rock Paper Scissors | `←→` and `Enter`, or `1`–`3` for rock, paper or scissors, or a click | click a throw |
| Accept / decline a challenge | `a` / `d` | buttons |
| Offer a draw | `D` | button |
| Answer a draw offered | `y` / `n` | buttons |
| Claim a draw (Chess, Checkers) | `C` | **Claim a draw** |
| Resign | `R` (asks first) | button (asks first) |
| Send a move that failed again | `r` | **↻ Send again** |
| Forget a game | `x` (asks first) | **Forget** |
| Back to the list | `Esc` | ← (on a phone) |

Under each board, **How to play** says in a few sentences how that game is
played and won (in the terminal, when the window is tall enough for it). The
rules in full are below.

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

**Ultimate Tic-Tac-Toe** is nine Tic-Tac-Toe boards in a three by three. The
square you take in a small board sends them to the small board in the same
place in the big one: take the top right square of any small board and they
must play in the top right small board. If that one is finished, they may
play in any small board still open. Three in a row wins a small board (a
small board filled without one counts for nobody), and three small boards in
a row win the game; if every small board is finished without that, it's a
draw. The squares you can play are marked.

**Go** is on 9×9, with area scoring and White's komi of 7.5, so there are
no draws. You can put a stone on any empty point, or pass. Stones with no
empty point next to them (a chain's whole length) are captured; a move that
would leave your own chain without one (suicide) isn't allowed, and neither
is one that makes any position the board has had before (positional superko).
Two passes in a row end the game, and every stone on the board counts, so
**capture dead stones before you pass**: rettui doesn't ask you to agree
which are dead. Each side's score is its stones plus the empty points only it
surrounds; the board's note shows it as it stands, komi counted.

**Battleship** has a fleet of five ships (lengths 5, 4, 3, 3 and 2) on each
side's 10×10 sea. rettui sets yours at random when the game starts; there's
no placing them yourself yet. You take turns firing at a square of theirs,
and each shot is answered with the next message: hit, miss or sunk. Your waters
(on the left) show what you've learned, your fleet (on the right) what
they've hit. A `?` is a shot fired and not yet answered. The first to sink all
five wins: when their last ship goes down, they **concede** (`Enter`, or
**Concede**), and you **show your fleet** (`Enter`, or **Show your fleet**) to
finish. Neither can cheat: your fleet is committed to with a fingerprint
(SHA-256 of the fleet and a secret) when the game starts, and both fleets are
shown at the end so each side can check every answer the other gave.

**Quoridor** is a race on 9×9. Black starts on the bottom (e1) and must
reach the top, White the other way. On your turn step your pawn one square
(not through a wall), or jump over their pawn next to you (or step diagonally
round it, if a wall or the edge is behind it), or place one of your ten
walls: two squares long, between squares, never crossing or overlapping
another wall, and never closing a pawn off from its goal entirely. A wall is
named by the square below and left of its middle, then `h` (running across)
or `v` (running up): `c3h` lies between ranks 3 and 4 along files c and d. The
same position a third time is a draw. The squares you can step to are ringed.

**Nine Men's Morris** has nine pieces each and 24 points. First you take
turns placing them on empty points; then you slide one along a line to the
next point, and with three left you may fly to any empty point. Three of your
pieces in a line (a mill) takes one of theirs (not one in a mill, unless all
theirs are). You lose with fewer than three pieces, or no move. A position
a third time, or 100 moves without a mill, is a draw. In moves, `d2` places,
`d2-d3` slides, and `d2xg7` or `d2-d3xg7` also takes the piece on g7.

**Breakthrough** is on 8×8, two rows of pieces each. A piece steps one square
forward, straight on (to an empty square) or diagonally (to an empty square,
or taking a piece of theirs). The first to reach their far row wins, and so
does taking every piece of theirs. There are no draws.

**Amazons** is on 10×10, four amazons each. A move has two parts: slide one
of yours like a chess queen (any distance, any of eight directions, over empty
squares), then fire an arrow the same way from where it lands. The arrow's
square is burnt for the rest of the game: nothing may land on it, or pass
over it. Whoever can't move loses; there are no draws. The board marks, as
you pick, where the amazon can go and then where the arrow can.

**Pente** is Gomoku on 19×19 with captures. Five or more of your stones in
a row wins, as in Gomoku. If your new stone and another of yours have exactly
two of theirs between them in a line, those two are taken and you score a
pair (a stone placed between two of theirs is safe). Five captured pairs also
win. A full board is a draw.

**Rock Paper Scissors** is the first to win three rounds (ties don't
count). Over messages nobody can throw at the same moment, so the leader (who
leads is drawn when the challenge is accepted) locks in a throw with a
fingerprint of it; the other player then throws in the open; and the leader
shows theirs, and locks in the next. The leader can't change a throw after
seeing the other's, and the follower never sees it before throwing.
"The match's last throw" is shown the same way, with no new one to lock in.

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
