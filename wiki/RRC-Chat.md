RRC is the IRC-like chat protocol that NomadNet 1.4 also speaks, served by
hubs such as [rrcd](https://github.com/kc1awv/rrcd) (the reference hub),
[rrc-hub](https://github.com/thatSFguy/reticulum-relay-chat) (in Go),
[rsRRCD](https://github.com/reticulum-spb/rsRRCD) and the hub Ratspeak can
host. rettui works with each of them, as they differ (see
[Hubs differ](#hubs-differ)). It lives in the Channels tab.

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
- **Commands:** the same set as NomadNet: `/join <room> [key]`, `/part`,
  `/me`, `/nick` (per hub, defaulting to your display name), `/who` (or
  `/names`), `/list`, `/topic`, `/ping`, `/clear`, `/connect`,
  `/disconnect`, and hub moderation commands such as `/mode`, `/kick` and
  `/op`. `/msg <nick> [text]` (also `/w`) whispers on hubs that support it
  (with no text, it opens the conversation), and `/dm <nick> [text]` sends
  an LXMF message instead. `/help` lists them all. Commands rettui doesn't
  have go to the hub as typed, since hubs have their own (rrc-hub's
  `/history`, `/away` or `/seen`, say), and the hub says if it doesn't
  have one. `/quote <text>` sends any text to the hub: `/quote /help` for
  the hub's own help.
- **Nicks:** a new nick (`/nick`) goes with your next message, as in
  NomadNet. A hub may give you another than you asked for (rrc-hub keeps
  nicks unique, so you may be `zev1`): your messages show the one it gave,
  and mentions of either count.
- **Keyed rooms:** the key you join a `+k` room with is kept, to rejoin it
  after reconnecting (until it stops working, or you leave the room).
- **Kicked or banned:** you're out of the room (it keeps its messages) and
  rettui doesn't rejoin it.
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

## Hubs differ

The RRC specification leaves a lot to each hub, and the hubs in use fill it
in differently. What rettui does about it:

- **Members:** rrcd sends no member lists unless its operator turns
  `include_joined_member_list` on, and its `/who` gives only the start of
  the identity of anyone with a nick. Such people are listed by nick until
  rettui learns their identity (when they say something); until then their
  name opens no menu. On such a hub rettui asks `/who` quietly when
  someone joins, to list them. rrc-hub instead sends everyone in the room with every
  join and leave. rsRRCD adds full identities to `/who` replies. Ratspeak
  splits a big room's member list over several messages.
- **`/who` and `/names`:** replies come in one message (rrcd), with
  `[away]` after people who are away (rrc-hub), or in several messages
  that each start `members in <room>:` (Ratspeak, for big rooms). rettui
  takes them all.
- **`/list`:** Ratspeak ends a list too long for one message with
  `(+N more)`; a list sent a line per message is put together.
- **Greeting:** rrcd and rrc-hub send a greeting of several lines as a
  message per line (and a long one as a file transfer). The hub view shows
  them together, once.
- **History:** rrc-hub replays a room's recent messages to whoever joins,
  between `--- N messages from earlier ---` and `--- end of history ---`.
  rettui leaves out the messages it already has, shows the rest at the time
  they were said, and doesn't notify you of them.
