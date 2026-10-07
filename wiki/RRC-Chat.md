RRC is the IRC-like chat protocol that NomadNet 1.4 also speaks, served by
hubs such as [rrcd](https://github.com/kc1awv/rrcd). It lives in the Channels
tab.

- **Hubs:** add a hub by address (`n`), or open an `rrc://hash/room` link on a
  NomadNet page. Opening a new hub from a link asks first, because connecting
  tells the hub who you are.
- **Views:** the hub view shows its status, limits, MOTD, and public rooms,
  which you can click to join. Rooms show the topic, members (on wide
  screens), `/me` actions and notices, with times to the second. Your own
  messages show `…` until the hub echoes them back.
- **Whispers:** private notices ("whispers") with another user have their own
  conversation under the hub, marked `@` where rooms have `#`, instead of
  appearing in rooms. Writing there whispers to that user; incoming whispers
  count as unread there and stand out like mentions. When two users share a
  nick, the list adds the start of their identity. `x` (or **Close** in the
  web UI) closes a conversation. Whispers saved in rooms by older versions
  are moved to their conversations.
- **Commands:** the same set as NomadNet: `/join`, `/part`, `/me`, `/nick`
  (per hub, defaulting to your display name), `/who`, `/list`, `/topic`,
  `/ping`, `/clear`, `/connect`, `/disconnect`, and hub moderation commands
  such as `/mode`, `/kick` and `/op`. `/msg <nick> [text]` (also `/w`)
  whispers on hubs that support it (with no text, it opens the
  conversation), and `/dm <nick> [text]` sends an LXMF message instead.
  `/help` lists them all.
- **Messaging a user:** click a name in the chat or the members list (or press
  `m`) for a menu: mention them (adds `@name` to what you're writing), open
  your whisper conversation, send an LXMF message, or copy their LXMF
  address or identity. Their LXMF address comes from the identity
  the hub shows, and the menu says whether they have announced it. A message
  to an address that was never announced waits until a path is found.
- **Mentioning someone:** typing `@` in a room lists who is there, plus
  anyone who has spoken there. The list narrows as you type: names starting
  with what you typed come first, then names containing it. Up and Down
  choose, Tab or Enter puts in `@name`, a click does too, and Esc closes the
  list. A mention of someone else shows in the same colour as their name
  in the chat.
- **Joins and leaves:** people joining and leaving a room show in its
  chat. `J` (TUI), or **Show joins** in a room's header (web UI), hides or
  shows them. It's one setting for every room ("Show joins and leaves"
  under Status), and hiding them loses nothing: they come back when turned
  on again.
- **Emoji:** `Ctrl-E` (or the 🙂 button) and `:name` work in the channel
  input as they do for messages (see [Emoji](Messaging#emoji)).
- **Mentions and unread:** `@yournick` mentions are highlighted (just the
  mention, not the whole message). Unread counts show on each room and in the
  sidebar, and mentions stand out from ordinary unread messages.
- **History:** each hub's messages are kept on disk between runs.
- **Long messages:** messages over the hub's size limit are offered as a
  split into several messages.
- **Searching:** `/` in the Channels tab (in the web UI, the search box
  above the hubs, or `/`) finds what was said in every hub's rooms and
  whisper conversations, as rettui keeps them: every word you type, in any
  order, in the line or the name of who said it, the newest first (200 at
  most). `Tab` (or *In #room*) keeps it to the room open. Opening one shows
  its room at that line, marked; in the web UI the whole room loads if the
  line is further back than what shows. Joins, leaves and errors aren't
  searched.
- **Drafts:** what you write stays with its room or whisper conversation,
  as in [Messaging](Messaging), and in the web UI outlives reloading the page.
- **Connections:** hubs reconnect automatically with backoff (toggle with
  `a`) and rejoin your rooms quietly. Quitting leaves hubs properly, so others
  see you go at once.
