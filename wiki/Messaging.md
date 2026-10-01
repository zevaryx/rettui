The Messages tab: LXMF messages with other Reticulum users (Sideband,
MeshChat, NomadNet and other LXMF clients).

- **Conversations:** a conversation per peer, with unread counts in the
  list and the sidebar. Start one by address (`n`), from the Network tab, or
  from an `lxmf@` link on a page.
- **Drafts:** what you write (and attach) stays with its conversation.
  Opening another one doesn't carry it over, and coming back brings it back.
  A message that fails to send goes back to the conversation it was for.
- **Message states:** your messages show *sending…*, ✓ (delivered), *✓ via
  propagation node*, or *failed* with the reason.
- **Delivery modes:**
  - **Auto** (default): tries direct delivery first. If that fails, it hands
    the message to your propagation node.
  - **Direct:** delivers over a Link and waits for the recipient's proof.
  - **Propagated:** always goes through the propagation node.
  - **Paper:** isn't sent. It's written as a [paper message](#paper-messages)
    to pass on some other way.
- **Stamps and ratchets:** if the recipient's announce asks for a stamp,
  rettui generates it. It also generates the propagation node's stamp, and
  encrypts to the recipient's ratchet when one is known.
- **Propagation nodes:** the ones you hear are listed in the Network tab.
  Pick one with `p`. rettui syncs every `sync_interval_mins` (30 by default)
  or whenever you press `S`. A sync identifies you to the node, downloads your
  messages, then tells the node to delete them.
- **Attachments:**
  - The first image goes in the LXMF image field, which Sideband and MeshChat
    show inline. Other files are sent as file attachments.
  - Received attachments are saved automatically, and images get an inline
    preview. `o` opens the newest attachment with your desktop's default app.
  - Many clients refuse direct transfers over about 1 MB, and rettui warns
    before sending that much.
- **Signatures:** incoming messages are checked against the sender's key. If
  the key isn't known, rettui looks it up first (up to 10 s). Messages that
  still can't be checked are marked *unverified*. A message whose signature
  doesn't match its sender's known key was written by someone else: it's
  dropped, and the log says so (as Python LXMF does).
- **Peers who are offline:** public keys from announces are kept, so you can
  write to a peer you heard earlier even while they're away.
- **Announces:** rettui announces your LXMF address when it starts (unless
  you turn that off), every `announce_interval_mins` (360 by default, 0 for
  never), and whenever you press `A`.

## Paper messages

A paper message is an LXMF message that travels outside Reticulum: as an
`lxm://` link or its QR code. You can print it, show it on a screen, or send
the link any way you like. It's signed and encrypted for the recipient like
any other message, so only they can read it. The format is Python LXMF's, so
Sideband and NomadNet can read what rettui writes, and rettui can read theirs.

- **Writing one:** pick the *paper* delivery mode (`d` or `Ctrl-P` in the
  TUI, *Paper (QR code)* in the web UI), then write the message.
  - rettui only needs the recipient's key, from an announce heard at some
    point. They don't have to be online.
  - Paper messages carry text only, up to about 1,800 characters.
  - When the message is ready, its QR code opens:
    - **TUI:** `y` copies the link, and `s` saves the QR code as an SVG
      image in the downloads folder, ready to print. If the window is too
      small for the code, rettui says so, and you can still copy or save it.
    - **Web UI:** *Copy link*, *Share* (on phones), and *Print*, which
      prints the code alone.
  - The message then shows *✓ paper message*. `P` in the TUI, or its *QR
    code* button in the web UI, opens the code again.
- **Reading one:** rettui checks the message is for you, decrypts it, and
  checks the signature like any other message. It then goes into the
  sender's conversation, which opens.
  - **TUI:** press `p`, then paste the link or give the path to a picture of
    the QR code. You can also paste an `lxm://` link straight into the
    Messages tab.
  - **Web UI:** *Read paper*, beside *+ New*. Paste the link, or scan the
    code:
    - *Scan* uses the camera, in browsers that can read QR codes themselves
      (Chrome on Android and macOS). The page has to be secure (https or
      localhost).
    - *Scan (take a picture)*, or *From a picture*, works in every browser:
      take a photo or pick a picture, and rettui finds the code in it.
  - A message you've already read in is only reported, not added twice.

## Replies

A reply names the message it answers, and the start of that message's text
goes with it, so the other side sees what it answers even without that
message. It's LXMF's own reply format, as Columba and MeshChatX send it:
replies between them and rettui show as replies both ways. Clients that don't
support replies yet (Sideband, NomadNet, MeshChat) show an ordinary message.

- **In the TUI:** `r` replies to the newest message they sent, and so does
  `Ctrl-R` while writing. `↑↓` then pick another (the one chosen is marked
  in the history), and `Esc` stops replying. Clicking a message's name line
  replies to it.
- **In the web UI:** hover over a message (on a phone it's always shown) and
  click **↩ Reply**. The box shows what you're replying to; `Esc` or **×**
  stops replying.
- **Showing them:** a reply shows the first line of what it answers, under
  who wrote it. That's taken from the message itself when it's here, and
  from what the reply quoted when it isn't (it's older than the messages
  kept, or was never received). Click the quote to scroll to the message.

A message you send can be replied to once it's sent: replies name it by its
hash, which it has from then on. What you're replying to stays with the
conversation's draft.

## Emoji

Both UIs have the same two ways to add an emoji to a message, and to what you
write in a channel:

- **The picker:** `Ctrl-E` (or the 🙂 button in the web UI) opens it, at the
  emoji you used lately (both UIs share the list). Type to search by name or
  shortcode, or go through the groups (`Tab` in the TUI, the tabs in the web
  UI). `Enter` or a click puts the chosen one in, and in the web UI
  Shift-click keeps the picker open for another.
- **By name:** type `:` and the start of a name, such as `:dra`, and a list of
  the emoji it could be opens above the box. `↑↓` choose, `Tab` or `Enter`
  puts one in, and `Esc` closes the list. Typing the whole name, such as
  `:dragon:`, turns it into 🐉 right away. Names are GitHub's shortcodes
  (`:+1:`, `:tada:`); words of an emoji's Unicode name find it too.

The emoji are Unicode's, up to Unicode 15.0 (2022): many systems still draw
newer ones as boxes. In the web UI, `Ctrl-E` isn't used on a Mac or an
iPhone, where it moves to the end of the line and `Ctrl-Cmd-Space` opens the
system's own picker.
