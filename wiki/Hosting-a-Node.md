rettui can host four kinds of node: a NomadNet node (pages and files) and
an LXMF propagation node (messages kept for people who are offline), with
your identity, an RRC hub (chat rooms), with one of its own, and an rngit
node (git repositories, read over Reticulum), with another of its own.

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
  or renamed (`:/page/`, `:/media/` and `:/file/` addresses, and those
  naming your node's address; not links to another node's page of the same
  name) are changed in every page, except the one open with changes not saved (the TUI says so;
  the web UI changes them in the editor, for you to save). Scripts are never
  changed or moved this way, and the web UI can't rename or move them.
- **Who may see it:** anyone, at first. To keep a page, picture or file to
  some people, press `w` in the TUI (or pick **Who may see it…** in its `⋯`
  menu in the web UI) and tick them: your contacts and those you've talked
  with show there once their key has been heard. rettui keeps the list
  beside it as `name.allowed`, a line for each identity, as NomadNet does,
  so lists made for NomadNet work too. Visitors who aren't on it, or who
  haven't identified themselves to your node, are refused. A list with no
  one on it lets no one see it (you still can, in the Browser). The node
  reads the list for each visit, so a change applies at once, and the list
  goes along when what it's for is renamed, moved or deleted. A list that's
  itself a program (executable) is left as it is: change it on the host.
- **Statistics:** the Node tab says how many pages, files and pictures the
  node has served, how many requests it had (and how many were for
  something not there), when the last one came, and since when it has
  counted, as NomadNet does. The counts are kept across restarts until you
  reset them, with `R` in the TUI or **Reset** in the web UI. While the node
  runs, Status shows them too.
- **Selecting text in the editors:** Shift with the arrows, Home/End or
  PgUp/PgDn, `Ctrl-A` for everything, or drag with the mouse. Typing, pasting
  or deleting replaces the selection.
- **Starter page:** a new node gets an index page with a short Micron guide.
- **Your own node in the Browser:** a client can't reach a node it hosts over
  Reticulum, so rettui reads your own pages straight from the folder (never
  from the cache), and runs your script pages itself (see below).
- **Scripts:** executable pages run as programs for visitors only when "Run
  page scripts" is on. The web UI can't change, rename or create scripts,
  turn "Run page scripts" on or move the node folder, so the web login can't
  be used to run programs on the host. The TUI can do all of these.

### Script pages

A script page is a program in `pages/` that prints the page, as on
NomadNet: any file with its execute bit set (`chmod +x page.mu`), usually
starting with a line such as `#!/usr/bin/env python3`. Each visitor who
opens it runs it, and gets what it prints. Scripts run only on Linux, macOS
and other Unix systems, and only while "Run page scripts" is on: while it's
off, visitors get the script's own text, so keep secrets out of it.

- **What a script gets:** no shell and nothing to read on its input. Its
  environment holds only, as NomadNet gives it, `link_id` (the visitor's
  link), `remote_identity` (the identity hash of a visitor who identified),
  each `field_…` a form sent and each `var_…` a link sent, and `PATH`, which
  is just `/usr/bin:/bin`. So `#!/usr/bin/env python3` finds Python only if
  it's in `/usr/bin` or `/bin`. Where it isn't (Homebrew, `/usr/local`, pyenv
  or a virtual environment), start the page with Python's full path instead,
  such as `#!/opt/homebrew/bin/python3`.
- **Limits:** a script has 10 seconds to finish and may print up to 512 KB.
  Past either, the visitor gets nothing. What it writes to its error output
  never reaches visitors; a script that fails but printed something still
  sends that.
- **Trying one:** open it on your own node in the Browser (`b` in the Node
  tab). It runs as it would for a visitor, with what a form sends, and as you
  if you identify to your node. When a visitor would get nothing, the page
  says why, shows what the script wrote to its error output, and, for a first
  line naming a program that scripts can't find, says where that program is
  on your computer. In the web UI, this runs scripts already on the node, as
  any visitor could; the web UI still can't write them or turn them on.
- **Docker:** rettui's image has no Python. A script in it needs a program
  that's there (`/bin/sh`), or an image built on rettui's with Python added.

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
  send them several at a time.
- **Passing messages on (peering):** yours peers with other propagation
  nodes too, as `lxmd` does, so a message sent through it reaches its
  recipient whichever node they collect from. With *Auto-peer*
  (`pn_autopeer`, on) it peers with each propagation node it hears
  announcing within *Auto-peer depth* (`pn_autopeer_depth`, 4 hops), up to
  *Most peers* (`pn_max_peers`, 20). One that moves further away, or stops
  being a propagation node, is no longer peered with, and one asking for
  more work than LXMF allows (a peering cost over 26) isn't peered with at
  all. *Static peers* (`pn_static_peers`, addresses separated by commas)
  are always peered with, however far, once their announce has been heard.
  Every 24 seconds, one peer with messages it hasn't had is offered them,
  and sent those it wants; what each has had is kept with the node, so
  after a restart only new ones are offered. Turning *Auto-peer* off keeps
  messages here, and with static peers only. The *Peered with* line in
  Status (and the web UI's) says how many nodes it peers with and how many
  messages it has passed on.

Tested against Python LXMF 1.2: its clients send through and collect from
rettui's node, and its propagation nodes peer with it.

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
  If you use another identity (or the hub gets a new one), the old one is
  no longer an operator; operators added to its `config.yaml` by hand stay.
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

## rngit repositories

rettui can serve git repositories to [rngit](https://github.com/zevaryx/rngit-rs)
clients, so people clone them over Reticulum with `git clone rns://<address>/<group>/<name>.git`
(the stock `rngit` helper, `git-remote-rns` and rettui's [Repos tab](Repos) all
work). This part only reads: cloning and fetching. Pushing, creating
repositories, releases and work documents are answered "not allowed" for now;
use stock rngit for those.

- **Switching it on:** set "Host rngit repositories" in Settings › Hosting
  (`git_host_enabled`). It announces itself (`git.repositories`) a few seconds
  after it starts, then every six hours. Status shows its address, as
  `rns://<address>`, and how many requests it has answered and refused.
- **Its repositories:** a folder with a folder for each group and
  `name.git` repositories in each (`git-host/` in the data directory, or the
  folder in "rngit repositories folder", `git_host_dir`, which only the
  terminal UI can set). Make them with `git init --bare` or `git clone --bare`.
  [Backups](Data-and-Storage#backups) keep the node's identity, not the
  repositories.
- **Its own identity:** like the hub, the node has an identity of its own
  (`git-host-identity` in the data directory), made the first time it starts,
  so its address stays the same. Back it up with yours.
- **Who may read:** nobody does until you say so. Put a `.allowed` file beside
  a repository (`demo.git.allowed`), or a `group.allowed` in a group's folder,
  with rules like `r:all` (anyone), `r:<32 hex identity>` (one person) or
  `adm:<32 hex>`, one on each line or separated by commas. "rngit access"
  (`git_host_access`, terminal only) gives the same rules to every repository.
  A repository someone may not read looks like one that isn't there. A line
  that isn't a rule is skipped, so a typo can only take access away. Anyone
  who doesn't identify themselves is `d7db22f63b453c23bb0688dde565b7c1`, as in
  rngit.
- **Limits:** a clone's bundle is built in memory and sent in one piece, up to
  256 MB, and building one stops after two minutes. A request over 1 MB is
  refused. Recent bundles are kept for ten minutes (64 MB in all), so the next
  clone of the same thing is quick; a branch that moves is a new bundle.
  rettui serves one request at a time, so a big clone makes the next client
  wait.
- **Security:** the web UI can switch it on and off, and name it, but can't pick
  the folder or who may read. Nothing in a repository is run: hooks are never
  called. Names that aren't plain folder names (`..`, a leading dot, a path)
  are refused.

`rngit serve DIR [RULES]` of the `rngit` command serves a folder the same way
without rettui.
