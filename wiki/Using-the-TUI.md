`rettui` starts the terminal UI: eight tabs (Messages, Channels, Network,
Browser, Node, Status, Reticulum, Hub) with keyboard and mouse control. Terminals
narrower than 110 columns get a sidebar of icons and narrower side lists, so
80×24 works well.

**Colours, times and dates** are settings (Status): *Terminal colours* is
dark (for a dark terminal, the default), light (for a light one) or basic
(16 colours, for a terminal without full colour); *Clock* is 24-hour or
12-hour, and *Dates* month-day (Oct 07), day-month (07 Oct) or
year-month-day (2025-10-07). Clock and Dates go for the web UI too, and
for exported conversations.

## Keys

The footer shows the most used keys of what's on screen, as many as fit,
and `? keys` at its right edge: `?` lists them all, with those that work in
every tab. While typing, `?` is typed, so it says `F1 keys` instead, and F1
lists them. Any key, or a click, closes the list.

| Tab | Keys |
| --- | ---- |
| All tabs | `1`–`8` switch tab, `A` announce, `S` sync with the propagation node, `?` list the keys of what's on screen (`F1` while typing), `Ctrl-L` redraw the screen, `q` / `Ctrl-C` quit |
| Messages | `↑↓` pick a conversation, `Enter` write, `/` [search messages](Messaging#searching) (`Tab` this conversation or all, `Enter` open the one picked), `r` [reply](Messaging#replies) to their newest message, `m` pick a message, `c` [contact card](Messaging#contacts), `X` delete the conversation, `n` new conversation by address, `y` copy the peer's address, `a` attach a file, `o` open the newest attachment, `d` cycle delivery mode, `p` read a [paper message](Messaging#paper-messages), `P` show the newest paper message written, `N` turn the conversation's notifications off or on, `H` read its [archived messages](Data-and-Storage#message-history), `L` [share a location](Messaging#locations-and-the-map) (`live 1h` shares it live; `L` again stops that), `M` [the map](Messaging#locations-and-the-map) of locations shared, `*` pin it to the top (or unpin it), `R` mark every conversation read, `E` [export](Messaging#message-actions) the conversation as text, `PgUp/PgDn` scroll a page, `Home`/`End` the oldest or newest |
| A picked message (`m`) | `↑↓` pick another, `r` / `Enter` reply, `e` [react](Messaging#reactions-locations-commands-and-voice-messages), `y` copy its text, `f` [forward](Messaging#message-actions) it, `o` open its file or location, `t` [send it again](Messaging#message-actions) if it failed, `x` delete it, `Esc` done |
| The map (`M`) | `↑↓` pick a place, `+`/`-` or the wheel zoom in on it and out, `0` all of them, `Enter` open the conversation, `o` open the place on OpenStreetMap, `Esc` close |
| Contact card (`c`) | `r` your name for them, `e` notes, `t` [trust](Messaging#blocking-and-spam) (or stop), `l` leave an unknown sender as is, `b` block (or unblock), `y` copy their address, `X` delete the conversation, `Esc` close |
| Writing a message | `Enter` send, `Esc` stop writing (or first stop replying), `Ctrl-R` reply (again: to the one before), `↑↓` while replying another message, `Ctrl-E` [emoji](Messaging#emoji), `:name` an emoji by name, `Ctrl-V` paste, `Ctrl-O` attach, `Ctrl-X` clear attachments, `Ctrl-P` cycle delivery mode |
| Emoji picker (`Ctrl-E` while writing a message or in a channel, `e` on a picked message) | type to search, arrows choose, `Tab`/`Shift-Tab` next or previous group, `Enter` put it in (or react with it), `Esc` close; click one to put it in |
| An emoji name typed after `:` | `↑↓` choose, `Tab`/`Enter` put it in, `Esc` close the list |
| A paper message's QR code | `y` copy the link, `s` save the code as an SVG image, any other key (or a click) closes it |
| Channels | `↑↓` pick a hub, room or whisper conversation, `Enter` write (text or `/commands`), `/` [search what was said](RRC-Chat), `n` add a hub, `c` connect or disconnect, `a` toggle auto-connect, `J` show or hide joins and leaves, `N` next notification choice for the hub, room or whisper conversation, `x` leave a room, close a whisper conversation, or remove a hub, `y` copy an `rrc://` link, `m` message a user in the room, `PgUp/PgDn` scroll a page, `Home`/`End` the oldest or newest |
| User menu (click a name, or `m`) | `w` open your whisper conversation, `l` LXMF message, `Enter` pick (also copy their LXMF address or identity), `Esc` close |
| Network | `↑↓` select, `y` copy the selected address, `Enter` message a peer, browse a node, or pick a propagation node; `p` use the selected propagation node; `b` block or unblock an LXMF peer; `P` [find a path](Network) to the selected address (or one typed), `D` forget its path, `T` probe it; `f` filter (all, LXMF peers, NomadNet nodes, propagation nodes, blocked); `s` sort (last heard, name, nearest); `i` keep to one interface (through each in turn, then any); `/` search by name or address (`Enter` done, `Esc` clear); `a` [announces as they're heard](Network) (again, or `Esc`, for the list) |
| Browser (both panes) | `←`/`→` move between the node pane and the page, `t` switch Saved/Nodes, `/` [search](NomadNet-Browsing) both lists (`Enter` done, `Esc` clear), `g` go to an address, `y` copy the current address, `s` save the current page, `b` back, `r` refresh from the network, `R` clear the whole cache, `I` identify to this node (toggle), `H` home, `u` view the page's Micron source (toggle), `Esc` cancel loading |
| Browser, node pane | `↑↓` select, `Enter` open beside the list, `s` save the selected node's home page (Nodes), `x` remove a saved page (Saved), `Esc` clear the search |
| Browser, page | `Tab`/`Shift-Tab` move between links and fields, `Enter` follow a link or edit a field, `f` (or `Ctrl-F`) [find in the page](NomadNet-Browsing) (`Enter`/`↓` next, `↑` previous, `Esc` done), `L` copy the selected link, `Y` copy the whole page (the raw source when viewing source), `Ctrl-V` paste into the selected field, `Esc` clear the selection or leave the source view, `↑↓` / `PgUp`/`PgDn` scroll |
| Node | `Enter` edit the selected page, `n` new page, `r` rename, `x` delete, `h` start or stop hosting, `a` announce the node, `b` open it in the Browser, `y` copy its address, `p` switch view (editor and preview, editor only, preview only) |
| Editing a page | `Ctrl-S` save (live on the node), `Esc` back to the pages, `Alt` + the underlined letter formats (see the ribbon), `Shift`+arrows select, `Ctrl-A` select all, `Ctrl-Z` / `Ctrl-Y` undo and redo, `Ctrl-P` switch view, `Ctrl-V` paste; with the preview alone, `↑↓` / `PgUp` / `PgDn` scroll it |
| Status | `↑↓` select a setting, `Enter` edit it (or toggle), `e` edit display name, `y` copy your LXMF address, `c` show it as a QR code, `g` the getting-started guide, `b` back up your identity, `B` [back up everything](Data-and-Storage#backups), `i` use another identity (from the next start), `x` hide the first steps, `u` [check for a newer release](Installing#updating) now, `U` install one found, `Ctrl-R` restart Reticulum |
| Reticulum | `Tab` sections / options, `↑↓` select, `Enter` edit (toggles flip, choices open a list), `d` back to the default, `a` add an interface, `Space` enable or disable it, `r` rename, `x` delete, `t` edit the file as text, `R` reload the file, `Ctrl-R` restart Reticulum |
| Reticulum as text | `Ctrl-S` save (refused while the file can't load), `Esc` close, `Ctrl-Z` / `Ctrl-Y` undo and redo, `Ctrl-V` paste |
| Hub | [The RRC hub hosted here](Hosting-a-Node#rrc-hub): `e` start or stop hosting, `y` copy its `rrc://` link, `Tab` / `←→` People, Rooms, Bans, `↑↓` select, `a` announce, `:` run a hub command (`/stats`, `/kline list`...), `n` new registered room; People: `K` kick from a room, `b` ban from a room, `o` make operator of a room (or not), `v` voice (or not), `B` ban from the whole hub, `d` disconnect; Rooms: `t` topic, `m` modes, `r` register or unregister, `R` rename, `D` delete; Bans: `x` lift the ban |

A file asked for in a prompt (an attachment, a picture of a paper
message, an identity file, where to save a backup) can be dragged onto the
terminal: the quotes, `\` escapes or `file://` address the terminal adds
are understood. A path that names a file as typed is always taken as it is.

The footer confirms what keys did: ✓ (green) when something is done, ! (yellow)
when it can't be done right now, and ✗ (red) when it failed. Problems stay up
a little longer.

## Mouse

- Click a section in the left sidebar to switch to it.
- Click a conversation to open it, the compose box to start writing, and an
  attachment to open it with your desktop's default app. Click the name line
  of a message to pick it, then its buttons to reply, react, copy, retry or
  delete it. Click
  a reply's quote to see what it answers, and a location to see it on a map.
- In Channels, click a hub or room to open it, the input box to write, a
  public room in a hub's view to join it, and a name to message that user.
- In the Network tab, click a row to select it and double-click to open it.
  Click the search bar to start searching.
- In the browser, click a saved page or node on the left to open it beside
  the list. Click the Saved and Nodes headers to switch lists.
- On a page, click links and form fields, click the address bar to go to an
  address, and right-click to go back.
- Click **view source** in the page's title bar to see its Micron source, and
  **back to page** to return.
- Drag across page text to select it. It's copied when you release the
  button, and dragging past the top or bottom edge scrolls the page.
- In the Status and Reticulum tabs, click a row to select it and double-click
  to edit it. In the editors, click to place the cursor and drag to select;
  click a ribbon button to format.
- The scroll wheel scrolls whatever is under the pointer.
- Click the version beside the name (top left) to open rettui's project page
  in your browser. In the web UI it opens in a new tab.

## Copy and paste

- Copied text goes to the system clipboard (X11 or Wayland). It's also sent
  through the terminal's OSC 52 clipboard escape, so copying works over SSH
  in terminals that support it. Set `RETTUI_CLIPBOARD=osc52` to use only
  OSC 52.
- Your terminal's paste shortcut (for example `Ctrl-Shift-V`) and `Ctrl-V`
  both paste into whatever is being edited: a message, a prompt, the selected
  form field on a page, or an editor. Line breaks become spaces in the
  single-line inputs, and are kept in the editors.
- rettui captures the mouse, so the terminal's own selection usually needs a
  modifier. Most terminals use `Shift`+drag.
