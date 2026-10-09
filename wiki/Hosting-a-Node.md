rettui can host three kinds of node: a NomadNet node (pages and files) and
an LXMF propagation node (messages kept for people who are offline), with
your identity, and an RRC hub (chat rooms), with one of its own.

## NomadNet node

rettui hosts a NomadNet node using rsNomad's node. Pages are Micron
files in `node/pages/` (and downloads in `node/files/`) in the data directory,
or the folder set in `node_dir`.

- **Switching it on:** press `h` in the Node tab, use **Start hosting** in
  the web UI, or set "Host a node" in the settings. The node uses your
  identity, so its address stays the same. Its name defaults to your display
  name, and it announces every `node_announce_interval_mins` (360 by default)
  or when you press `a`.
- **Editing:** the Node tab lists what's on the node and has an editor with Micron
  colouring, undo and redo, and a live preview, in both the TUI and the web
  UI. Show the editor and the preview side by side, the editor alone, or the
  preview alone (`Ctrl-P` / `p`, or the buttons in the web UI). Saving writes
  the file, and visitors get the new version straight away. New, renamed and
  deleted pages are picked up without a restart. You can also edit the files
  with any editor.
- **Formatting ribbon:** above the page editor, in both UIs, buttons put
  Micron markup around the selection, on the selected lines, or at the
  cursor: bold, italic, underline and normal (removes formatting), text and
  background colour, left/centre/right alignment, three heading levels,
  divider, literal block, comment, and inserting links, images, text fields,
  checkboxes and radio buttons, and uploading. Each has an Alt shortcut, the
  letter underlined on its button: `Alt+B` `I` `U` `N`, `F` `G` (colours),
  `L` `C` `R`, `1` `2` `3`, `V` (divider) `T` (literal) `O` (comment), `K`
  (link) `M` (image) `D` (field) `H` (checkbox) `A` (radio) `P` (upload).
  Bold, italic and underline toggle, as do headings and comments. The web UI
  also takes `Ctrl+B`, `I`, `U` and `K`, and picks colours from a palette.
- **Pictures and files:** *Upload* (`Alt+P`) adds a file to the node and puts
  it in the page where the cursor is (the selection, if any, becomes its
  label). The TUI asks for the file's path on this computer; the web UI
  picks files from the device, several at once, and files dropped on the
  page's text are added the same way. A picture (WebP, PNG, JPEG, BMP, GIF,
  TIFF) goes to `pages/images/` and is shown in the page
  (`` `(name`:/media/images/name.jpg) ``). To suit the mesh it's made at
  most 1024 pixels across and written again as a JPEG (a PNG where it's
  see-through), which leaves out its metadata, such as where a photo was
  taken; a GIF (which may move), or one that wouldn't get smaller, stays as
  it is. A picture can be up to 512 KB. Anything else goes to `files/`,
  linked for visitors to download
  (`` `[name`:/file/name] ``), up to 32 MB. Names are made safe (spaces
  become `-`), and something already there is never replaced: the new one
  gets `-2`, `-3`... on its name.
- **Folders:** the list shows `pages/` (pages, and the pictures they show)
  and `files/`, each with its folders. In the TUI, `n` makes a page in the
  folder picked, `r` renames what's picked, `m` moves it to a folder of its
  root (a new one is made for it, and one left empty goes), and `x` deletes
  it. In the web UI, each has a `⋯` menu (open, show a picture, put it in the
  open page, copy its address, rename, move, delete), and on a computer
  anything can be dragged onto a folder of its root. Links to what's moved
  or renamed (`:/page/`, `:/media/` and `:/file/` addresses) are changed in
  every page, except the one open with changes not saved (the TUI says so;
  the web UI changes them in the editor, for you to save). Scripts are never
  changed or moved this way, and the web UI can't rename or move them.
- **Selecting text in the editors:** Shift with the arrows, Home/End or
  PgUp/PgDn, `Ctrl-A` for everything, or drag with the mouse. Typing, pasting
  or deleting replaces the selection.
- **Starter page:** a new node gets an index page with a short Micron guide.
- **Your own node in the Browser:** a client can't reach a node it hosts over
  Reticulum, so rettui reads your own pages straight from the folder (never
  from the cache).
- **Scripts:** executable pages run as programs for visitors only when "Run
  page scripts" is on. The web UI can't change, rename or create scripts,
  turn "Run page scripts" on or move the node folder, so the web login can't
  be used to run programs on the host. The TUI can do all of these.

## Propagation node

A propagation node keeps messages for people who aren't online, until they
collect them, as `lxmd` and other LXMF propagation nodes do. rettui's is
built on rsLXMF's propagation node (its message store, stamp checks and
peer protocol).

- **Switching it on:** set "Host a propagation node" in the settings (Status
  tab, either UI). It announces itself (`lxmf.propagation`) a few seconds
  after it starts, then every `announce_interval_mins`, with its name
  (`pn_name`, or your display name), stamp cost and transfer limits. Its
  address comes from your identity, so it stays the same. Changing one of
  its settings restarts it; the messages it holds stay.
- **Status:** the Status tab's *Hosting messages* line shows its address,
  how many messages it holds and their size, how many it has taken in and
  given out, those it refused, and the peers connected to it now. It's also
  listed in the Network tab, where you can pick it as your own propagation
  node.
- **What it takes:** each message sent through it brings a propagation stamp
  of at least `pn_stamp_cost` (16 by default, LXMF's own; at least 13) less
  LXMF's flexibility of 3, and is at most `pn_transfer_kb` (256 KB by
  default). Clients send one message at a time; only peers send several.
  Messages without a valid stamp, too big, or over the storage limit are
  refused.
- **Storage:** messages are kept encrypted for their recipients in
  `propagation/` in the data directory, at most `pn_storage_mb` (500 MB by
  default). LXMF's 30-day expiry applies, and past the limit the oldest go
  first.
- **Collecting:** a client identifies itself on its Link and asks for its
  messages (`/get`); the node sends them, then deletes the ones it got.
- **Messages for you:** when the node takes a message addressed to you, it
  delivers it straight to your conversations instead of keeping it. With
  your own node picked as your propagation node, rettui hands messages to it
  directly (a client can't open a Link to itself), and a sync has nothing to
  download.
- **Peers:** other propagation nodes that peer with yours (proving the work
  of LXMF's peering key, cost 18) offer it the messages it doesn't have, and
  send them several at a time. Yours doesn't peer with other nodes itself,
  as `lxmd` with `autopeer` off: messages sent through it stay here until
  their recipients collect them, and other nodes only get them by peering
  with it. Tested against Python LXMF 1.2: its clients send through and
  collect from rettui's node, and its propagation nodes peer with it.

## RRC hub

An RRC hub is what people chat on in [RRC](RRC-Chat): rooms, their topics
and modes, operators, bans. rettui runs [rsRRCD](https://github.com/reticulum-spb/rsRRCD)'s
hub on its own Reticulum instance, so NomadNet, MeshChatX, Ratspeak, rettui
and the other RRC clients can join it as they join rrcd.

- **Switching it on:** press `e` in the Hub tab, use **Start hosting** in the
  web UI's Hub section, or set "Host an RRC hub" in the settings. It
  announces itself (`rrc.hub`) a few seconds after it starts, with its name,
  then every `hub_announce_interval_mins` (360 by default; 0 announces only
  when it starts), or when you press `a`. Its name is `hub_name`, or your
  display name; `hub_greeting` is sent to everyone who connects (`\n`
  starts a new line). Changing these applies to the hub as it runs: nobody
  is disconnected.
- **Its own identity:** the hub has an identity of its own, made the first
  time it starts (`rrc-hub-identity` in the data directory), so its address
  stays the same and people who join learn the hub's address, not your LXMF
  one. You're an operator of it (as is the hub itself), wherever you are.
- **You on it:** the hub is added to your hubs in Channels once it's up, and
  rettui joins it straight away (a client can't find a path to a destination
  it hosts, so yours connects with the hub's key). Stopping the hub leaves it.
- **Who makes rooms:** as on most hubs, anyone may make a room by joining one
  that isn't there, and is its founder (an operator of it). With "Anyone makes
  rooms" off, only you can: others join the rooms there are.
- **The Hub tab:** who's connected (their nick, identity, rooms and when they
  came), the rooms (registered or not, modes, topic, who's in them, their
  operators) and the bans, of the whole hub and of each room. Pick someone
  to kick them from a room (`K`), ban them from one (`b`), make them an
  operator of one (`o`) or voiced in one (`v`), each again to undo,
  disconnect them (`d`), or ban them from the whole hub (`B`, a *kline*:
  they're disconnected, and every time they come back). Pick a room to set
  its topic (`t`) or modes (`m`: `+m` moderated, `+i` invite only, `+t`
  topic by operators, `+n` members only, `+p` private, `+k key`; `-` takes
  one off), or to register it (`r`). `n` makes a registered room, which
  stays, with its topic and modes, while nobody is in it. In Bans, `x`
  lifts the ban picked.
- **The web UI's Hub section:** a room opens a page of its settings: its
  topic, a switch for each mode, its key, who's in it (with operator and
  voice to turn on or off, kick and ban), who's invited and who's banned,
  with renaming and deleting it at the bottom. A person opens theirs: the
  rooms they're in, with the same for each, and disconnecting or banning
  them from the hub. Bans each have a *Lift* button.
- **Renaming and deleting rooms:** neither rsRRCD nor RRC has a command for
  them, so the hub does it itself. Renaming a room (`R` in the Hub tab)
  moves its topic, settings, key, operators and bans to the new name, and
  registers it. RRC can't move people from one room to another, so those
  in it are told the new name and taken out of the old one, to rejoin it
  there. Deleting a room (`D`, asked first) takes everyone out of it,
  telling them it's been closed, and it's gone with its settings and bans;
  where anyone makes rooms, anyone may make it again by joining it.
- **Hub commands:** `:` in the Hub tab (or the web UI's console) runs a
  command as typed in a room, as the hub itself: `/stats`, `/kline list`,
  `/who lobby`, `/reload`... The hub's answers are listed beside it. The
  room commands (`/topic`, `/mode`, `/op`, `/kick`, `/ban`...) work in any
  room from here, though rsRRCD only lets a room's operators run them.
- **Its files:** rsRRCD's own `config.yaml` and `rooms.yaml` (the registered
  rooms) in `rrc-hub/` in the data directory, so its `/reload` and `/kline`
  work as they do in rsRRCD. rettui writes the settings above into
  `config.yaml` (and lists you and the hub as `trusted_identities`); the rest
  (limits such as `max_rooms_per_session` or `rate_limit_msgs_per_minute`,
  operators of your choosing, the hub's bans) is yours to change there,
  then `/reload`. [Backups](Data-and-Storage#backups) keep the folder, and
  the hub's identity with yours.
