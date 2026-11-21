rettui can host four kinds of node: a NomadNet node (pages and files) and
an LXMF propagation node (messages kept for people who are offline), with
your identity, an RRC hub (chat rooms), with one of its own, and an rngit
node (git repositories, read over Reticulum), with another of its own.

## NomadNet node

rettui hosts a NomadNet node itself. Pages are Micron
files in `node/pages/` (and downloads in `node/files/`) in the data directory,
or the folder set in `node_dir`.

- **Forks and mirrors:** someone who may make repositories in a group (`c`) can
  have the node fork or mirror a repository, and `sync` brings one up to date.
  The source can be an `rns://` repository on another rngit node or an
  `https://` one (GitHub, say), fetched by rettui itself; `http://` and `ssh`
  are refused, because the node runs no programs. An `https://` source can't
  carry a login or point at an address on your own network, redirects aren't
  followed, and a fetch stops when it takes too long or comes to more than
  *rngit repository limit*. The node fetches with its own identity, so give `c`
  only to people you trust to have it read what it may read. Mirrors are
  followed again once a day; a fork only when someone syncs it. The maker
  becomes the repository's administrator. A node can't fork or mirror from itself.
  A sync keeps branches and tags the source has dropped, as rngit does; switch on
  *rngit mirrors drop what sources drop* (`git_host_prune`) to have it remove them.
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
  with any editor, or copy pages into the folder: rettui looks for new ones
  every minute.
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
  gets `-2`, `-3`... on its name. To add a picture or file without putting
  it in a page, press `u` in the TUI's list (it asks for the path), or
  **+ New** › **Upload a file** in the web UI, whose **+ New** asks whether
  to make a page or upload files. Pages link to it later by its address
  (**Copy its address** in its ⋯ menu).
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
  itself a program (executable) is left as it is: change it on the host. A
  list that isn't a file at all (a folder, a pipe) lets no one in. Copies an
  editor leaves beside a file are never served, as they have no list of their
  own: names ending in `~`, `.bak`, `.orig`, `.old`, `.tmp`, `.swp`, `.save`
  or `.rej`, names wrapped in `#`, and any name with `.allowed` in it.
- **Statistics:** the Node tab says how many pages, files and pictures the
  node has served, how many requests it had (and how many were for
  something not there), when the last one came, and since when it has
  counted, as NomadNet does. The counts are kept across restarts until you
  reset them, with `R` in the TUI or **Reset** in the web UI. While the node
  runs, Status shows them too. It also says (a Visitors line) how many
  different visitors who identified themselves have come since the node
  started (a visitor who doesn't identify can't be told apart, and this
  number starts again with the node).
- **Selecting text in the editors:** Shift with the arrows, Home/End or
  PgUp/PgDn, `Ctrl-A` for everything, or drag with the mouse. Typing, pasting
  or deleting replaces the selection.
- **Starter page:** a new node gets an index page with a short Micron guide.
- **Your own node in the Browser:** a client can't reach a node it hosts over
  Reticulum, so rettui reads your own pages straight from the folder (never
  from the cache), and runs your script pages itself (see below).
- **Scripts:** executable pages run as programs for visitors only when "Run
  page scripts" is on. The web UI can't change, rename or create scripts,
  turn "Run page scripts" on, change *Script PATH* or move the node folder,
  so the web login can't
  be used to run programs on the host. The TUI can do all of these.

### Script pages

A script page is a program in `pages/` that prints the page, as on
NomadNet: any file with its execute bit set (`chmod +x page.mu`), usually
starting with a line such as `#!/usr/bin/env python3`. Each visitor who
opens it runs it, and gets what it prints. Scripts run only on Linux, macOS
and other Unix systems, and only while "Run page scripts" is on: while it's
off, a script is never sent, and a visitor who asks for one gets the same
"not found" page as for a page that isn't there.

- **What a script gets:** no shell and nothing to read on its input. Its
  environment holds only, as NomadNet gives it, `link_id` (the visitor's
  link), `remote_identity` (the identity hash of a visitor who identified),
  each `field_…` a form sent and each `var_…` a link sent, and `PATH`, which
  is `/usr/bin:/bin` unless *Script PATH* (Settings, Hosting; terminal only)
  says otherwise: a list of folders joined with colons, or `host` for the
  `PATH` rettui itself was started with, as NomadNet does. So
  `#!/usr/bin/env python3` finds Python if it's in a folder on that path.
  Where it isn't (Homebrew, `/usr/local`, pyenv or a virtual environment),
  put its folder in *Script PATH*, or start the page with Python's full path,
  such as `#!/opt/homebrew/bin/python3` or `#!/srv/node/venv/bin/python`.
  Use full paths: a script runs in the folder rettui was started in, not its
  page's folder, so `#!../venv/bin/python` or a relative *Script PATH* folder
  doesn't find it.
- **Limits:** a script has 10 seconds to finish (*Script time limit*, up to
  120) and may print up to 512 KB. Past either, or if it can't be run, the
  visitor gets a "Page failed" page saying it couldn't be made. A
  running script holds up the node's other requests until it ends, so a long
  limit makes everyone wait. What it writes to its error output
  never reaches visitors; a script that fails but printed something still
  sends that.
- **Trying one:** open it on your own node in the Browser (`b` in the Node
  tab). It runs as it would for a visitor, with what a form sends, and as you
  if you identify to your node. When a visitor would get nothing, the page
  says why, shows what the script wrote to its error output, and, for a first
  line naming a program that scripts can't find, says where that program is
  on your computer. In the web UI, this runs scripts already on the node, as
  any visitor could; the web UI still can't write them or turn them on.
- **Docker:** rettui's image has Python 3 (`/usr/bin/python3`), so
  `#!/usr/bin/env python3` works there. For any other program, build an image
  on rettui's with it added.

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
- **Limits on what it accepts:** a copy of a message it already holds isn't
  checked again, a new message is checked once, outside the node's lock, a
  transfer with 16 bad stamps is refused, a Link that has sent 16 messages
  with bad stamps in all is closed (only 1, while Links nothing was stored
  from have sent 64 in the last 10 seconds: a flood over many Links), an
  `/offer` without a valid peering key is turned away before it is noted,
  and a peer can't ask for more than 10,240 KB in one sync. Transfers wait
  to be checked in turns, one Link after another, so one sender's backlog
  can't hold up everyone else's; Links a new message was stored from go
  first, and Links that sent bad stamps last. At most 4,096 wait (40 MB at
  most); when that's full, a Link that sent bad stamps loses its oldest
  first, then one nothing was stored from yet. A message dropped then is
  lost without the sender knowing (it was told the message arrived, as
  Python's nodes tell it), which takes a flood. What a client sent just
  before it went is still checked.
  While such a flood has used up the shared budget, an honest client that
  hasn't had a message stored here yet is closed at its first bad message
  too: one with a stamp made for an older stamp cost, or a link packet
  that isn't a transfer. It can send again once the flood is over. A Link
  a new message was stored from keeps its place in front while it sends
  only copies of messages held: they cost little to check.
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
  (`pn_autopeer`, off until you turn it on) it peers with each propagation node it hears
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

### Upgrading the propagation node

rettui 2.0 keeps the node's messages in the store rsLXMF 1.4 uses, which
can't take over the folder an earlier version left in `propagation/`. If
the node doesn't start, saying it can't open the message store, stop
rettui and run:

    rettui migrate-propagation

(in Docker, after `docker compose down`:
`docker compose run --rm rettui migrate-propagation`). It moves the old
messages to `propagation.legacy-quarantine` beside the store and starts a
new one. The old messages are kept but not handed out again, since nothing
can show where they came from, so people who had messages waiting on your
node must send them again. Start rettui afterwards as usual. A node that
never hosted a propagation node before needs nothing.

## A public node with no UI

`rettui node` runs a public node on a server, with no terminal UI and no
web UI, as `rnsd` and `lxmd` do together: a transport node that others
connect to, and a propagation node.

Use a data directory of its own for the node (`--data-dir`, as below), so
its identity, settings and Reticulum config are the node's alone.

    rettui --data-dir /var/lib/rettui-node node
    rettui --data-dir /var/lib/rettui-node node --entry-point 4965 --reachable-on rns.example.org --name "Hilltop"

The first run sets the node up, and prints each change it saves, after the
file it's in:

- **Reticulum config:** *Transport node* on (`enable_transport = Yes`),
  and a TCP server interface named `Public entry point`, listening on
  `0.0.0.0` port 4242 in gateway mode (`mode = gateway`, so it answers path
  requests for those connecting, as a public node should). If the config
  has a TCP server already (or a Backbone interface that listens), that one
  is used instead, and only gets `mode = gateway` if it has no mode.
- **Settings:** *Host a propagation node* (`pn_enabled`) on, and
  `rns_config` set to the node's own Reticulum config if it had none (see
  below).

It only adds what's missing, and never removes or changes a value you set,
so you can edit the config and settings afterwards, and every later run
needs no flags. Every run also says, as notes rather than changes, which
interface is the entry point, and what it left as it was: a TCP server on
that port that's turned off (`enabled = No`) stays off, and `--listen-ip`
doesn't move a server that's there. The flags are kept the same way (all but the `--stats-…` and `--manage-…` ones, which are for one run).
Each can also be given as an environment variable, for a systemd unit
(`Environment=` or `EnvironmentFile=`), a Docker Compose file or a shell, so
a node can be set up with no command line. A flag given on the command line
wins over its variable. An empty variable counts as not set, as Compose
passes one when you write `NAME: ""`. A value the flag wouldn't take, or two
that can't go together (`--no-entry-point` with `--entry-point`,
`--listen-ip` or `--reachable-on`, `--no-propagation` with `--name`, whether
from flags, variables or one of each), stops the node with a message that
names the variable. Switches take `yes`, `true` or `1` for on, and `no`,
`false` or `0` for off, in any case. What a variable sets is kept the same
way as its flag.

| Flag | Environment variable | What it does |
|------|----------------------|--------------|
| `--entry-point PORT` | `RETTUI_NODE_ENTRY_POINT` | Others connect on this port: a TCP server on it is added unless one is there. Without it, an existing TCP server is used, else one is added on 4242. |
| `--listen-ip IP` | `RETTUI_NODE_LISTEN_IP` | The address a new TCP server listens on (default `0.0.0.0`, every address). One already there keeps its own. |
| `--reachable-on HOST` | `RETTUI_NODE_REACHABLE_ON` | The host name or IP address others reach the node at: the TCP server is announced with it, for interface discovery (`discoverable` and `reachable_on`). Letters, digits, `.`, `-`, `_` and `:` only, with `[ ]` around an IPv6 address. |
| `--no-entry-point` | `RETTUI_NODE_NO_ENTRY_POINT` | Adds no TCP server: others connect over the interfaces already in the config. Transport node is still turned on. |
| `--name NAME` | `RETTUI_NODE_NAME` | The name the propagation node announces (`pn_name`; otherwise your display name). |
| `--no-propagation` | `RETTUI_NODE_NO_PROPAGATION` | Doesn't turn the propagation node on. One turned on before stays on: set `pn_enabled` to `false` in `settings.json` to stop it. Can't be used with `--name`. |
| `--stats-listen ADDRESS` | none (`STATS_LISTEN` in Docker) | Serves a read-only stats page on this address, such as `127.0.0.1:8741` (see below). Off unless given, and not saved: give it on every run. |
| `--stats-token-file PATH` | `RETTUI_STATS_TOKEN` is the secret itself (`STATS_TOKEN_FILE` is the file, in Docker) | A secret that lets a request for the stats also list every peer heard (see below). Needs `--stats-listen`. Off unless given, and not saved. |
| `--stats-logs` | none | Lets a request with the stats token also read the end of the log at `/logs` (see below). Needs `--stats-listen`. Off unless given, and not saved: without it `/logs` is a 404, token or not. |
| `--manage-listen ADDRESS` | none (`MANAGE_LISTEN` in Docker) | Lets a website you run change a few settings and a few keys of the Reticulum config (on probation), and restart Reticulum, with signed requests to this address (see [Remote management](#remote-management)). Needs `--manage-secret-file`. Off unless given, and not saved. |
| `--manage-secret-file PATH` | none (`MANAGE_SECRET_FILE` in Docker) | The file with the secret those requests are signed with, readable by you only. Needs `--manage-listen`. There is no variable for the secret itself. |
| `--manage-allow ADDRESS` | none (`MANAGE_ALLOW` in Docker, separated by commas) | Takes management connections only from this address or network (such as `10.200.0.1` or `10.200.0.0/24`): the website's. Give it once for each. Needs `--manage-listen`. Not saved. |
| `--manage-allow-enable` | none (`MANAGE_ALLOW_ENABLE=yes` in Docker) | Lets a Reticulum config change from the website turn interfaces and Transport **on**, which opens the node to others (see [Reticulum config changes on probation](#reticulum-config-changes-on-probation)). Without it, only turning them off and the other keys are allowed. Needs `--manage-listen`. Off unless given, and not saved. |

Things to know:

- **What's public:** a node announces itself. Its identity, LXMF address
  and display name are public, and the propagation node announces the
  display name too, unless `--name` gives it one of its own. The first run
  says so.
- **Which Reticulum config:** without `--rns-config`, and with no
  `rns_config` in `settings.json`, the node uses a config of its own in
  `reticulum/` in the data directory (made the first time), so setting it up
  changes no other Reticulum program's config. It saves that as
  `rns_config`, so `rettui` and `rettui --web` with the same data directory
  use it too. With `rns_config` or `--rns-config`, it sets up that one
  (`--rns-config` is used for that run only, not saved). Before changing
  the file it keeps it as it was, as `config.backup` beside it and as a
  version in the config's history (see
  [Settings and Reticulum Config](Settings-and-Reticulum-Config)). A new
  config and `config.backup` are readable by you only, as a config can hold
  IFAC passphrases.
- **What it serves:** the config and settings can't be changed from the
  network: the node has no login and no web UI, reads no input, and never
  adds or turns on a pipe interface or anything else that runs a program.
  The one exception is [remote management](#remote-management), off unless
  you turn it on, which can change a few settings and turn services off,
  never on.
  But the services already on in the settings are served to everyone who
  connects: a NomadNet node (and its page scripts, if
  `node_executable_pages` is on), an RRC hub, rngit repositories, and files
  received with rncp. Each one that's on is named when the node starts,
  with the setting that turns it off (in `settings.json`, with rettui
  stopped).
- **Hardening:** open only the entry point's port (4242) in the firewall.
  The propagation node takes messages from anyone with a valid stamp, up to
  `pn_transfer_kb` each and `pn_storage_mb` in all, so size those for the
  disk. For a node that isn't public, listen on one address
  (`--listen-ip`), or give the interface an IFAC network name and
  passphrase (Settings and Reticulum Config) so only those who know them
  can connect. In Docker, publish only `-p 4242:4242`.
- **Its log:** it prints its LXMF address, the propagation node's address
  and how it is, where it listens and how many interfaces are online when it
  starts and every 10 minutes, and what rettui's Status log says as it
  happens (interfaces coming and going, announces). `rettui.log` in the data
  directory has the rest.
- **Stopping:** Ctrl-C or SIGTERM (`systemctl stop`, `docker stop`) saves
  everything and leaves the network cleanly. See
  [Running as a service](Running#running-as-a-service) for a systemd unit,
  and [Docker](Docker) for a container.

### A stats page for a status page

With `--stats-listen ADDRESS` the node also serves a read-only summary of
itself at `/stats.json` on that address, with no login. It is off unless you
give it, and it publishes totals only: no addresses, no names of peers or
interfaces, no clients' addresses, and no row for each interface or client.
It can't change anything. The numbers are updated every second.

    rettui --data-dir /var/lib/rettui-node node --stats-listen 127.0.0.1:8741
    curl http://127.0.0.1:8741/stats.json

```json
{
  "version": 1,
  "rettui_version": "2.1.0",
  "generated_at": 1763528000,
  "uptime_seconds": 12345,
  "interfaces": {
    "total": 3, "online": 3, "clients": 17, "rx_bytes": 123, "tx_bytes": 456, "announce_queue": 5
  },
  "announce_queue": {
    "queued": 5, "limit": 12288, "full": false, "interface_limit": 4096, "fullest": 4, "interfaces_full": 0
  },
  "peers_heard": { "total": 100, "lxmf": 60, "nomad": 30, "propagation": 10 },
  "announces_last_minute": 12,
  "propagation": {
    "enabled": true, "stored_messages": 240, "stored_bytes": 3100000, "storage_limit_bytes": 500000000,
    "peers_connected": 2, "peers_total": 6, "peers_alive": 5, "peers_static": 2, "peers_auto": 4,
    "peer_limit": 50, "static_peers": 2, "autopeer": true, "autopeer_depth": 4,
    "messages_received": 900, "messages_served": 700, "messages_offered_to_peers": 1500,
    "messages_sent_to_peers": 800, "messages_received_from_peers": 400, "unhandled_messages": 35,
    "unhandled_distinct": 20
  },
  "disk": { "data_bytes": 52000000 }
}
```

A request with the stats token (below) gets the same, and `disk` also has
`"fs_total_bytes": 50000000000, "fs_free_bytes": 31000000000`.

`propagation` is there only while the node's propagation node runs, and `disk`
once the first measurement is done (a second or so after start). New keys may
be added in the future, so ignore the ones you don't know. Everything in them
is a total: no peer or sender is named.

- `stored_messages` and `stored_bytes` are the messages the propagation node
  holds now and their size (what it keeps for clients and other nodes, not
  the size on disk). `storage_limit_bytes` is the limit in Settings (*Storage
  (MB)*), in bytes.
- `peers_connected` is how many other nodes are connected to it now, sending
  it messages. `peers_total` is how many it is peered with, as the Status tab's
  *Peered with* line counts them: `peers_static` named in Settings and
  `peers_auto` found from announces. `peers_alive` of them answered when last
  tried. `peer_limit`, `static_peers`, `autopeer` and `autopeer_depth` are the
  settings (`static_peers` counts the valid addresses).
- The message counts are since the node started. `messages_received` is every
  message it took in to keep, `messages_received_from_peers` those that came
  from other nodes, `messages_served` those it gave out to their recipients,
  `messages_offered_to_peers` the offers made to peers (a message offered to
  two peers counts twice, and the count is kept in the peers' saved state, so
  it can include earlier runs) and `messages_sent_to_peers` those they took.
  `unhandled_messages` is how many are waiting to be offered to peers, added
  up over the peers: a message waiting for 20 peers counts 20, so it can be far
  more than `stored_messages`. `unhandled_distinct` counts each such message
  once, so it never exceeds `stored_messages`. A message a peer will not take
  (its stamp is below the peer's lowest accepted cost) is not counted as
  waiting for that peer, until the peer announces a lower cost. Both are worked
  out when a peer is next due, so they can be a little old.
- `disk.data_bytes` is the size of the files in the whole data directory (the
  messages, the stores, the log and every identity's folder), without
  following links. `fs_total_bytes` and `fs_free_bytes`, only in the answer for
  a request with the token, are the size of the file system that holds it and
  what is free there for an ordinary user (not given on Windows: `null`). Walking the directory is done in the background,
  at most once a minute, so these may be up to a minute old. If it can't be
  read, or has more than 500000 entries or takes more than 10 seconds, the
  field is `null`. After a failed measurement it waits twice as long before
  trying again, up to an hour.

Everyone who can reach the listener sees `propagation` and `disk.data_bytes` (counts and sizes of what the node keeps, with no addresses); the size and free space of the disk (`disk.fs_total_bytes`, `disk.fs_free_bytes`) tell what kind of host this is, so only a request with the token gets them, and a node with no token never shows them. If even the rest bothers you, keep the listener private (bind it to `127.0.0.1` or restrict it in the proxy).

`announce_queue` is how many announces are waiting to go out on the node's
interfaces, which a status page can watch to see that the queue doesn't stay
full. Reticulum keeps up to 4096 waiting per interface, and drops new
announces for an interface whose queue is at that. It is public, as it is only
counts. All the numbers are added up over the interfaces that report a queue,
with none of their names:

- `queued` is the announces waiting, over all of them (`interfaces.announce_queue`
  is the same number), and `limit` is what they can hold: `interface_limit`
  times the interfaces that report a queue.
- `full` is `true` when at least one interface's queue is at its limit, and
  `interfaces_full` is how many are. `fullest` is the longest queue of any
  one interface. Alert on `full` staying `true` (or `fullest` staying at
  `interface_limit`), not on `queued` against `limit`, which stays low while
  one interface is stuck.
- If no interface reports a queue (none are up yet), `queued`, `limit`,
  `full`, `fullest` and `interfaces_full` are `null` and `interface_limit` is
  still there.

`interfaces.clients` adds up the clients connected to the node's server
interfaces (`null` when none report any). The other fields, and how the
listener answers, are in [Running](Running#the-stats-endpoint). The
listener has no TLS and no limits on who asks, so bind it to `127.0.0.1` or
a private address and let a reverse proxy publish it. With nginx, on the
host that serves `https://node.example/`:

```nginx
location = /stats.json {
    proxy_pass http://127.0.0.1:8741/stats.json;
    proxy_read_timeout 5s;
    limit_except GET HEAD { deny all; }
}
```

In Docker, set `STATS_LISTEN` (see [Docker](Docker)). An address that can't
be opened (the port is taken, say) stops the node at start, with the reason.

#### The peer list, for whoever has the token

The stats page lists no peers unless you set a token. With one, a request that
presents it gets the same JSON plus every peer the node has heard: its
address, its type and its name. A website can use this to count the peers
of several nodes exactly, without counting one twice when two nodes heard it,
including nodes behind a proxy. Everyone else still gets the totals above.

Put the token in a file, or in the `RETTUI_STATS_TOKEN` variable (the flag
takes a path, never the secret, so it doesn't show in `ps`), not both. Prefer
the file (in Docker, a Docker secret): the environment is visible in
`/proc` and `docker inspect`, and programs rettui starts inherit it. Use a
random token, as made below, not a word. On Linux and macOS the node warns at
start if the file can be read by other users.

    head -c 32 /dev/urandom | base64 | tr -d '/+=\n' > /etc/rettui-stats-token
    chmod 600 /etc/rettui-stats-token
    rettui node --stats-listen 127.0.0.1:8741 --stats-token-file /etc/rettui-stats-token
    curl -H "Authorization: Bearer $(cat /etc/rettui-stats-token)" http://127.0.0.1:8741/stats.json

A token is 16 to 256 printable ASCII characters except spaces, quotes (`"`
and `'`) and backslash (the file's contents are trimmed). One that isn't, or both a file and the variable,
stops the node at start, with the reason. It is not saved in the settings.

A request presents it as `Authorization: Bearer TOKEN` or as `X-Stats-Token:
TOKEN`. A `?token=` in the URL is **not** read: URLs are written to proxy and
server logs and browser history, and headers are not. A request with a wrong
token, or none, gets the totals with status 200 (not 401), so nobody can tell
a wrong guess from no token, and a node with no token ignores the headers.
The token is compared in constant time and never logged.

```json
{
  "version": 1,
  "authorised": true,
  "peers": [
    { "address": "00112233445566778899aabbccddeeff", "type": "lxmf", "name": "Amy" },
    { "address": "ffeeddccbbaa99887766554433221100", "type": "propagation", "name": null }
  ],
  "...": "and the rest of the totals above"
}
```

- `type` is `lxmf`, `nomad` or `propagation`. `name` is what the peer
  announced, with control characters removed and cut to 64 characters, or
  `null`.
- The list is sorted by address and has at most 20000 peers. Above that,
  the list keeps the lowest addresses and is flagged `"peers_truncated": true`;
  `peers_heard` still counts them all.
- `name` is text the peer chose, so treat it as untrusted: escape it wherever
  you show it. Control characters, invisible characters (zero-width, bidi
  overrides and isolates, line and paragraph separators) are removed, but
  it can still hold `<`, quotes and anything else.
- The answer has `Cache-Control: private, no-store` and `Vary: Authorization,
  X-Stats-Token`, so a cache doesn't keep it or give it to someone without the token.

#### The end of the log, for whoever has the token

With a token set and `--stats-logs` given, `GET /logs` gives the end of
`rettui.log` (the newest lines of the current file, not `rettui.log.1`), so a
status page can show what the node has been doing. It is off unless you give
`--stats-logs`, because a log can hold more than you want anyone with the stats
token to see (anyone who learns the token would read it). It is read-only and
token-only: without `--stats-logs`, and for a request with no token, a wrong
one, or a node with no token set at all, the answer is 404, as for any path
that doesn't exist, so it can't be found by probing. The token goes in a header as
above, never in the URL (`?token=` is ignored, and gets the 404). Only `GET`
and `HEAD` are answered.

    curl -H "X-Stats-Token: $(cat /etc/rettui-stats-token)" "http://127.0.0.1:8741/logs?lines=50&level=warn"

- `lines` is how many, from 1 to 500 (default 200). `level` is `error`, `warn`,
  `info` (the default) or `debug`: that level and the more severe
  ones. `trace` is never served and gets 400. What you get is also limited by
  what the node logs (*Log level* in Settings, or `RETTUI_LOG`). A bad value
  gets 400 with a short message. One read runs at a time: a request that comes
  while another is being answered gets 503 with `Retry-After: 2`.
- The answer is oldest first:

```json
{
  "level": "info",
  "truncated": false,
  "lines": [
    { "time": "2025-01-31T12:34:56.123456Z", "level": "warn", "text": "rettui::net: peer unreachable" },
    { "time": null, "level": null, "text": "a second line of the same message" }
  ]
}
```

- `level` in a line is that line's own level. A line that doesn't start with a
  time and a level (the second line of a message, say) has `null` for both,
  and is shown when the line before it is. At the start of what was read,
  before any line with a level, such lines are left out.
- Only the last 1 MiB of the file is read. Each line is cut to 1000
  characters and the whole answer to about 256 KiB, keeping the newest lines.
  `truncated` is `true` when a line was cut, when lines were left out for size,
  or when the part read held fewer lines than asked for but the file has more.
  A missing log is an empty list, not an error.
- Before it is sent, rettui takes out of each line, replacing it with `[redacted]`:
  - the stats token, and under `rettui --web` the web UI's login token;
  - the query of every URL and request path (`https://host/x?key=...` and
    `GET /x?key=... HTTP/1.1` become `https://host/x?[redacted]` and
    `GET /x?[redacted] HTTP/1.1`), and a `user:password@` before a host;
  - everything after the first `authorization`, `token`, `cookie`, `api-key`,
    `api_key`, `apikey`, `secret`, `password`, `passwd` or `bearer` in the line
    (in any case, also when written with invisible characters or
    `%`-encoded), so `Authorization: Bearer ...`, `Cookie: ...` and `token=...`
    lose their values however they are printed. This takes more than needed at
    times (`no token given` becomes `no token [redacted]`).

  The home directory of the user running the node becomes `~`. Line breaks
  inside one log entry are written as `\n`, so text from a peer can't add a
  line that passes for an entry of its own. The newest line, if rettui is
  still writing it, is treated as cut.
- This catches the secrets rettui knows of and the usual ways of writing one,
  but not every secret in text that libraries and peers put in the log (a key
  printed with no name before it, say). Give `--stats-logs` only to someone you
  trust with the token, and keep the log to trusted viewers. `text` comes from
  the log, which holds what peers sent: escape it wherever you show it.
- The answer has `Cache-Control: no-store`.

#### The peered nodes, for whoever has the token

A request with the token also gets `propagation_peers`, one entry for each
node the propagation node is peered with (only while it runs), sorted by
address, at most 256 (`"propagation_peers_truncated": true` if there are more):

```json
"propagation_peers": [
  {
    "address": "00112233445566778899aabbccddeeff", "name": "Far node", "static": false, "alive": true,
    "state": "idle", "depth": 3, "last_heard_seconds_ago": 840, "last_sync_seconds_ago": 52,
    "messages_offered": 120, "unhandled": 4, "sync_failures": 0, "last_attempt_seconds_ago": 52,
    "next_attempt_in_seconds": null, "last_failure": null, "peering_cost": 18
  }
]
```

`address` is the node's, `name` what it announced (cleaned like a peer's name
above; `null` if none), `state` what the node is doing with it (`idle`,
`link_establishing`, `link_ready`, `request_sent`, `response_received` or
`transferring`), `depth` how many hops away it was when last heard (`null` if
not heard since the node started), and the `…_seconds_ago` fields are `null`
if never. `messages_offered` counts what was offered to it, `unhandled` what
waits to be offered, `sync_failures` how many syncs with it failed since it was
last heard announcing, `last_attempt_seconds_ago` when a sync was last tried
(`null` if never; a failed try counts too, unlike `last_sync_seconds_ago`),
`next_attempt_in_seconds` how long until it may be tried again (`null` if
now), `last_failure` why the last sync with it failed (`null` if it didn't,
or the last one worked), and `peering_cost` is the work its peering key costs.
`last_failure` is one of `no_link` (no Link could be made), `no_answer` (the
offer got no answer), `refused_invalid_data` (it couldn't read the offer),
`bad_key` (it refused the peering key), `no_access` (it doesn't let this node
peer), `busy`, `timeout`, `bad_answer` (an answer rettui couldn't read) or `io`
(the key or the messages couldn't be made or sent). It is kept in memory only,
so it is `null` after a restart until the next sync. The log (at debug level)
has one line for each offer, with the peer, how many messages were offered and
how the request was sent. Peers are synced one at a time, the one tried longest
ago first. The public answer never has this list.

Security notes:

- **Treat the token as a secret.** Whoever has it can read every address and
  name the node has heard, which is more than the node announces about
  itself. It can't change anything or reach the web UI. To change the token,
  change the file or variable and restart the node.
- **The list is every peer the node has heard**, not only the ones that
  talk to this node. Give the token only to a website you run.
- **Keep the stats behind HTTPS if the token crosses a network.** The
  listener itself has no TLS: with a token, a reverse proxy that serves HTTPS
  is needed (or bind the listener to `127.0.0.1` and ask from the same machine).
  In nginx, pass the headers on (they are by default), don't log them, and
  limit how often a client may ask, so the token can't be guessed quickly:

  ```nginx
  limit_req_zone $binary_remote_addr zone=rettui_stats:1m rate=2r/s;

  location = /stats.json {
      limit_req zone=rettui_stats burst=10 nodelay;
      proxy_pass http://127.0.0.1:8741/stats.json;
      proxy_read_timeout 5s;
      limit_except GET HEAD { deny all; }
  }
  ```

  A random token (the `head -c 32 /dev/urandom` command above) makes guessing
  hopeless; the limit is a second layer. If only your website's server needs the
  peer list, a third is to serve it on a separate address that only that
  server may reach (`allow` its address, `deny all`), and to have the public
  location drop the headers (`proxy_set_header X-Stats-Token "";` and
  `proxy_set_header Authorization "";`).
- A node with a token sends `Vary: Authorization, X-Stats-Token` on both
  answers, public and listed.

### Remote management

With `--manage-listen ADDRESS` and `--manage-secret-file PATH`, a website you
run (the rettui website's *Manage* page) can look after the node: see its
state, change a few settings, change a few keys of the Reticulum config on
probation, restart Reticulum, roll a settings change back, and read what was
done. It is off unless you give both flags, and they are
not saved: give them on every run. The node itself has no page for it, and
the terminal and web UIs have no part in it.

**Setting it up:**

1. Make a secret for this node only (each node needs its own), readable by
   the node's user only:

       head -c 48 /dev/urandom | base64 | tr -d '/+=\n' > /etc/rettui-manage-secret
       chmod 600 /etc/rettui-manage-secret

   A secret is 32 to 256 printable ASCII characters, with no spaces, quotes
   (`"` and `'`) or backslash; the file's contents are trimmed. One that
   isn't stops the node at start, with the reason, which never repeats the
   secret. On Linux and macOS a file that its group or other users can read
   or write stops the node too ("can be read or changed by other users"), as
   anyone who reads it can manage the node and anyone who writes it can
   choose the secret: `chmod 600` it. The file is checked once it is open, so
   it can't be swapped for another between the check and the read.
   There is no environment variable for it: programs the node starts (a pipe
   interface) inherit the environment. It must not be the stats token: a node
   given the same text for both stops at start, as whoever has the stats
   token could otherwise manage the node.
2. Start the node with a private address the website can reach, and the
   website's address:

       rettui --data-dir /var/lib/rettui-node node --manage-listen 10.200.0.2:8750 --manage-secret-file /etc/rettui-manage-secret --manage-allow 10.200.0.1

   It prints `Remote management at http://10.200.0.2:8750/manage/v1/ (signed
   requests only, from 10.200.0.1/32 only)` when it's up.
3. Give the same secret, and the address as `manage_url`, to the website
   (see its README), and restart it.

**Which addresses:** a loopback address, a private one (`10.x`, `172.16.x` to
`172.31.x`, `192.168.x`, `100.64.x` to `100.127.x`, `fc00::/7`) or a link-local
IPv6 one (`fe80::/10`). `0.0.0.0` and `::` (every address, as Docker needs)
are allowed, with a warning. Any other address is refused at start
("refuses a public address"): put the node behind a private network
(WireGuard, Tailscale) instead. An address that can't be used stops the node
at start, as a stats address does.

**Only the website may reach the port: this is required, not a
suggestion.** Give `--manage-allow` with the website's address (or its
network), firewall the port to the website, or both. Anyone else who can
reach it can't change anything, but can take the listener's connection
places (at most 4 from one address, 32 in all) and keep the website out for
as long as they keep connecting. With `--manage-allow`, a connection from
any other address is closed before it takes a place or anything is read
from it (and logged once a minute at most). Without it, every address may
connect, as before, and the node warns at start ("no --manage-allow
given"). It checks the address the connection comes from, so
behind Docker's published port, a NAT or a proxy, give the address the node
sees (the proxy's, or the Docker network's gateway) and firewall the rest.
A reverse proxy in front of it is only safe if it lets only the website
through (an IP allow-list, or client certificates) and passes each request's
path and query on exactly as sent, since they are signed: a proxy open to
anyone gives everyone a way in to those places, and one that rewrites the
path makes every request fail its signature.

**What it can do:**

- Read the node's version, the network's state (starting, running or
  failed, its interfaces, and whether it uses another program's shared
  instance) and the managed settings, with their labels and help.
- Read the Reticulum config, redacted (`reticulum` in the state): its
  sections and keys, sorted, with a revision (32 hex characters) that
  changes when what is shown changes. Only these keys show a value: `type`,
  `enabled`, `interface_enabled`, `mode`, `interface_mode`, `announce_cap`,
  `bitrate`, `ingress_control`, `enable_transport`, `share_instance`,
  `listen_ip`, `listen_port`, `target_host`, `target_port`, `reachable_on`
  and `discoverable`. Even then a value is shown only if it looks like that
  key's values: an IP address for `listen_ip`, a host name or IP address for
  `target_host` and `reachable_on`, a port (1 to 65535) for the ports, yes, no, true, false,
  on or off for the switches, a plain word (letters, digits and `_`) for
  `type` and the modes, and a number for `announce_cap` and `bitrate`.
  Anything else (`target_host = user:secret@host`, a path or command in
  `reachable_on`, a value with an unclosed quote) shows as set only. Every other key (passphrases, network names, IFAC
  keys, passwords, tokens, device paths, commands) shows only whether it is
  set, and its value never leaves the node. An interface with a
  `command` or `location_cmd`, a `type` that contains "pipe" (any case, or
  over several lines), a `reachable_on` or `discovery_reachable_on` that
  isn't a host name or IP address (Python Reticulum runs such a value as a
  program), or no readable `type`, shows only its `type` and
  `enabled`, not even the names of its other keys, however long its section
  is. The config is read as untrusted text:
  at most 1 MiB, 128 sections, 64 keys a section, about 24 KiB of view and
  values of 256 bytes are shown (`truncated` says if something was left out,
  or a line was over 2048 bytes, or a multi-line value never ended), and control
  characters are written out as `\u{..}`. If the file can't be read, the
  state has `"reticulum": null`. `editable` lists the sections a change can
  go to, and `reticulum_enable_allowed` says whether a change may turn
  things on (see [Reticulum config changes on
  probation](#reticulum-config-changes-on-probation)).
- Change these settings: display name, announce interval, the propagation
  node's name, stamp cost, storage, transfer limit, auto-peering, peer depth
  and limit, and static peers, the NomadNet node's name and announce
  interval, the RRC hub's name, greeting and announce interval, the rngit
  host's name, and the log level. They are checked as the settings editors
  check them (no names or greeting with control characters, such as tabs
  or escapes, invisible characters, or characters that change the text's
  direction, and none that shows nothing; a line break in the greeting is
  kept as `\n`), saved
  in `settings.json`, and apply at once. Some values only the terminal (or
  `settings.json`) can set, as they could leak more or lose data: the log
  level `trace` (remotely, `debug` at most), 0 propagation peers, less than
  32 MB of propagation storage, and a cut to less than half the storage
  there is (500 MB can go to 250 MB remotely; make a bigger cut in the
  terminal, and note that rolling back a big raise is such a cut). These
  are refused with `not_allowed`.
- Turn the propagation node, the NomadNet node, the RRC hub, the rngit host
  and its pages **off**. Only the terminal (or `settings.json`) turns them
  on: a service turned on from afar could expose data.
- Restart Reticulum, at most once every 5 minutes. The node says when it is
  back: `up` when the network runs and every interface that was online
  before is again, or `unhealthy` (with the interfaces that aren't) after 90
  seconds. With another program's shared instance, a restart only
  reconnects to it.
- Roll a settings change back, by putting the values it changed back, as
  long as nothing changed them since. A rollback goes through the same
  checks, so rolling back "turn the hub off" is refused: turn it on in the
  terminal.
- Change a few keys of the Reticulum config: Transport node on or off, and
  an interface's `enabled`, mode, announce cap, bitrate and ingress control.
  Turning Transport or an interface **on** opens the node to others, so it
  needs `--manage-allow-enable` on the node. Each change is tested first and put back by the node itself unless the
  website confirms it (see [Reticulum config changes on
  probation](#reticulum-config-changes-on-probation)).

**What it can't do:** run programs or choose what runs, change any other key
of the Reticulum config, add, rename or remove interfaces, change an
interface that runs a program (a pipe, one with `location_cmd`, or one
whose `reachable_on` isn't an address) or has no `type`, turn things on
without `--manage-allow-enable`, change folders, paths, commands, the rngit access
rule or anything the web UI may not change, read, export or delete identity
keys, read messages, or reach the web UI. A request may change at most 32
settings, with values of at most 4096 bytes, or 16 keys of the Reticulum
config, with values of at most 32 bytes, in a body of at most 16 KiB.

#### Reticulum config changes on probation

A change to the Reticulum config from afar could cut the node off from the
website that made it, so it is only kept once the website has seen the
node come back with it:

1. The website sends up to 16 changes with the config's revision (from the
   state) and how long to test them for (`confirm_seconds`, 60 to 900; the
   website's `manage_confirm_seconds`, 300 by default). Only these can
   change: `enable_transport` in `[reticulum]` (yes or no), and in an
   interface's own section `enabled` (yes or no; `interface_enabled` is
   changed if that is the name the file uses), `mode` (full, gateway,
   access_point, roaming or boundary; or `interface_mode`), `announce_cap`
   (a percentage over 0 and at most 100, with at most two decimals),
   `bitrate` (5 to 10000000000 bits a second) and `ingress_control` (yes or
   no). Turning an interface or Transport **on** is refused (`not_allowed`)
   unless the node runs with `--manage-allow-enable` (`MANAGE_ALLOW_ENABLE=yes`
   in Docker); turning them off, and the other keys, need nothing. With the
   flag, the answer's `notes` say what a change opens ("Hilltop TCP: This
   opens 0.0.0.0:4242 to anyone who can reach it", "This interface announces
   its address", "This node will relay traffic for others"), quoting only
   values of the shape the redacted view shows; the node's log has a
   `warning:` line for it, and its audit entry has the same `notes`.
2. The node changes those lines in place and nothing else: comments, other
   keys, secrets and the order of the file stay byte for byte as they were.
   The changed line may lose odd spacing around `=` and quotes around the
   old value; a switch keeps the file's style (`Yes`, `true`, `ON`...). A
   key that isn't there yet is added to its section. Anything that could be
   read two ways is refused (`ambiguous`), not guessed at: a section given
   twice, or spelled another way (`[[Hilltop]]` and `[[hilltop]]`), a key
   given twice, in another case, or under both of its names, a value over
   several lines, a triple-quoted value that never ends, a file over 1 MiB,
   a file with characters Python Reticulum reads as line breaks and
   rsReticulum doesn't (vertical tab, form feed, U+001C to U+001E, U+0085,
   U+2028, U+2029),
   a file that isn't plain UTF-8 or mixes line endings, a line starting
   with `[` that isn't a plain section header (`[[Shell # pipe]]`, `[[A]] x`,
   `[[A]`, `[]`, `[x] = 1`), a config that is a symbolic link, and a change
   that Reticulum's own parser would read as anything but the keys asked
   for. A config Reticulum wouldn't load is refused (`invalid_value`, "Reticulum
   couldn't load the config with this change (see the node's log)"): what
   Reticulum said, which can quote the file, stays in the node's log, as it
   does for every problem the node reports. It can't be done while
   another program's shared instance is used (`shared_instance`), while
   Reticulum isn't running (`busy`), within 5 minutes of a restart from
   afar, more than 4 times an hour, or for 15 minutes after a change was
   rolled back by itself (`rate_limited`, with `retry_after` in seconds).
3. The old text is kept in the config history (Reticulum tab, `H`) and the
   new one saved, with the change on probation in `manage/pending.json`
   (readable by the node's user only; it holds checksums of the file before
   and after, not its text). Reticulum restarts with it.
4. Once Reticulum runs again with every interface that was online before
   (but those the change turns off), the change is on probation until
   `confirm_seconds` later. The website checks it can still reach the node
   and then confirms the change, as the website user who made it: then it is
   kept. Settings can change meanwhile; another Reticulum change or a restart
   waits (`pending`).
5. If Reticulum isn't back within 90 seconds, fails, or the change isn't
   confirmed in time, the node puts the old text back by itself and restarts
   Reticulum with it (this restart isn't held back by the 5-minute limit,
   but a new change after it is). If the file was changed on this computer
   meanwhile, it is left as it is and the audit log says so. If the old text
   can't be written (a full disk, a folder it can't write to), the node
   says so once in its log and tries again every half second, and at the
   next start; only a change whose old text is nowhere to be found is given
   up (`rollback_failed`, said loudly in the log).

**If the node stops during the test** (a crash, a reboot, `systemctl stop`),
the change isn't kept: at the next start of rettui (as a node, or the
terminal or web UI on the same data directory), before Reticulum starts, it
puts the old text back if the file is still as the change left it, with or
without the `--manage-…` flags, and says so in its output, on the Status
tab and in the audit log. A file changed since is left alone. A website restart while a change is
on probation means nobody confirms it, so the node rolls it back.

**Putting a config back by hand:** every version the node saved is in the
config history (`reticulum-history/` in the data directory, readable by the
node's user only (copies an older version left readable by others are made
private when rettui starts), with `index.json` saying what each was: "Remote change
<id> by <user>, on probation: ...", "Rolled back remote change <id>: ..."),
and the file before the last save is `config.backup` beside the config. With
the node stopped, copy the version you want over the config, or put it back
from the Reticulum tab's history in the terminal UI.

**How requests are checked:** each request is signed with the secret
(HMAC-SHA256 over its time, a one-time nonce, the method, the path and query,
the website's user and the body), and so is each answer, so neither can be
forged; the secret itself never crosses the network. A request must be
within 5 minutes of the node's clock (a node whose clock is off says so, but
only to whoever has the secret), and each nonce works once: the node
remembers it for as long as its request's time is accepted, so a request
sent again is refused while the node runs. After a restart, a request signed
before the node started is refused (`before_start`, said only to whoever has
the secret), as its nonce may have been used before; so is one signed at or
before the latest time the node accepted in an earlier run (kept in
`manage/last-request` in the data directory), so a request dated ahead of
the node's clock can't be sent again after a restart either. A missing or
unreadable `last-request` is taken as none; a time in it far ahead (a clock
that was wrong) holds requests back for 5 minutes at most. A request sent
again within the run is refused unsigned, so its refusal can't stand in for
the answer the first one got. What signing can't
stop: someone on the path between the website and the node who holds a
request back and delivers it later, once, within those 5 minutes. That is
why the link must be private (a private network, or TLS). At most 10
changes a minute are taken; a change the node turns away before anything
changed (another change pending, a restart running, a rate limit) or a
request it can't read doesn't count (up to 60 a minute), so another website
user can't use up the limit with them. A confirm has a limit of its own, 6 a
minute, and a refused one doesn't count either, so nobody can keep a change
from being confirmed (which would roll it back). After 20 requests with a bad signature in a
minute, requests without a valid signature are refused for a minute (and the
refusals still count, so a flood keeps it going); requests the website signs
are still answered. The lockout is logged and added to the audit log when it
starts, then once every 10 minutes at most while a flood keeps it going, not
for every 20 requests. The listener speaks HTTP/1.1 only (not HTTP/2, which
would carry many requests on one connection), closes every connection after
one answer, holds at most 32 at once, at most 4 from one address, closes one
whose request hasn't arrived whole (headers and body) within 5 seconds of
connecting, and closes any after 15 seconds, so a client that sends slowly
or not at all can't hold it; a refused connection is logged once a minute at
most. It has no TLS: keep it on a private network, or behind a proxy that
adds TLS and lets only the website through (see above).

**The audit log:** every change, restart, rollback and refusal, and each burst
of bad signatures, is kept in `manage/audit.jsonl` in the data directory
(readable by the node's user only): when, which website user, what changed
from what to what, and how it ended. A Reticulum config change is
`reticulum`, with the section of each key (`path`) and its old value only if
the redacted view would show it (else `(hidden)`); its `confirm` and its
`auto_rollback` (`rolled_back`, or `failed` with the reason) have the same id,
and one that turned something on has `notes` saying what it opened. Bursts of bad signatures in a row share
one entry for up to an hour, with a `count` of the bad signatures since its
time: all of them, including those that come after the lockout is first
reported (added when the next report is due, or within seconds of the
lockout ending once the flood stops). An `/audit` answer stays under 48 KiB, with values of more than 200
bytes cut (ending in `…`) and `more` set if entries were left for the next
page; the log itself keeps them whole, for rollbacks. It keeps the last 2000 entries (fewer if they come to more than 2 MiB);
when it is full, the oldest refusals go first, so the changes a rollback
needs stay. The node's own log has a line for each too, with control
characters in anything a request sent written out (`\u{1b}`), so it can't
break the log's lines. The secret is never written to it, to the log, or to
an answer.

**Threat model:**

| Who | What they could do | What limits it |
|-----|--------------------|----------------|
| Someone who can reach the port without the secret | Nothing but be refused; while they keep connecting, take the listener's connection places and keep the website out (4 per address, 32 in all, each for 5 seconds until its request is in) | `--manage-allow` with the website's address, or a firewall: required; a proxy in front lets only the website through |
| Someone with this node's secret | Change the settings and the few Reticulum keys above and restart Reticulum on this node; with `--manage-allow-enable`, also turn on an interface that is off (a TCP server then opens its port) or Transport; no programs, keys or other Reticulum keys | Leave `--manage-allow-enable` off; change the secret; the audit log; the rate limits; a change that cuts the website off is rolled back |
| Whoever controls the website | The same, on every node it manages | The short list above; a secret per node; remove the flags on a node to turn it off |
| Someone who records the traffic | Read the settings as they pass (no TLS) | A private network or TLS proxy; the secret is never sent |
| Someone on the path who can hold traffic back | Deliver a request the website sent, once, up to 5 minutes late (a replay is refused) | A private network or TLS proxy |

**Lockout recovery:** the listener runs beside Reticulum, not over it, so a
Reticulum restart that goes wrong doesn't cut it off. If the website can't
manage the node (a lost secret, a port someone else keeps busy, a clock
that's off), SSH in: correct the clock, firewall the port, write a new secret
to the file and restart the node (and give the website the same one), or stop
the node and change `settings.json` by hand, or put the Reticulum config
back from its history (see above). To turn remote management off,
start the node without the `--manage-…` flags (in Docker, without
`MANAGE_LISTEN`).

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
  starts a new line; at most 4096 bytes). Changing these applies to the hub as it runs: nobody
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
- **Who makes rooms:** only you, to start with: others join the rooms there
  are. With "Anyone makes rooms" (`hub_open_rooms`) on, as on most hubs, anyone
  may make a room by joining one that isn't there, and is its founder (an
  operator of it). A hub set up before this kept its setting.
- **Closed rooms and bans:** someone who isn't in a room that is invite-only, has a key or is private can't post to it, list who's in it or read its topic, and only people in a room (or its operators) set its topic. A ban takes the person out on every connection they have.
- **Nicks and idle links:** a nick is cleaned of control and invisible characters, and one someone else has (any case), or "hub", isn't given: the person is told, and keeps the one they had. Someone can only leave rooms they're in. Links are pinged every 60 seconds and closed if they don't answer within 30 more (`ping_interval_s` and `ping_timeout_s` in the hub's `config.yaml`; a value of 0 means these defaults).
- **Limits:** one identity may hold at most 8 connections, a nick sent the older way is cleaned like any other, and a nick that looks like another (the same letters from another alphabet) isn't given. Joining, leaving and closing links don't rewrite `rooms.yaml` each time (at most every 5 seconds); at most 200 rooms may be registered, 8 per visitor. A link that doesn't say who it is within 30 seconds is closed, and no more than 64 may wait.
- **A damaged rooms file:** if a crash leaves `rooms.yaml` cut off, it is put aside as `rooms.yaml.damaged` and `rooms.yaml.bak` (the rooms as they were when the hub last started) is used.
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

rettui can serve git repositories to [rngit](https://github.com/rettui-dev/rngit-rs)
clients, so people clone them over Reticulum with `git clone rns://<address>/<group>/<name>.git`
(the stock `rngit` helper, `git-remote-rns` and rettui's [Repos tab](Repos) all
work), and push to it. Cloning, fetching, pushing, creating repositories and
deleting branches and tags work, and so do releases, work documents,
changing who may do what, and forks, mirrors and sync.

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
- **Who may read:** nobody does until you say so. The easiest way is the
  table under [Managing it](#rngit-repositories) below. Under it, each
  repository has a `.allowed` file beside it (`demo.git.allowed`), and a group
  a `group.allowed` in its folder, with rules like `r:all` (anyone),
  `r:<32 hex identity>` (one person) or `adm:<32 hex>`, one on each line or
  separated by commas. "rngit access" (`git_host_access`) in Settings › Hosting
  gives the same rules to every repository. A repository someone may not read
  looks like one that isn't there, so they're told "Not found". A line
  that isn't a rule is skipped, so a typo can only take access away.
  `r` reads, `w` pushes and deletes branches and tags, `rw` both, and `c` in a
  group's rules (a `group.allowed`, or "rngit access") lets someone make
  repositories there, if they identify themselves; the group's folder has to
  exist already. Whoever makes a repository is its admin (`adm:` in its
  `.allowed`). `adm` includes everything. Anyone
  who doesn't identify themselves is `d7db22f63b453c23bb0688dde565b7c1`, as in
  rngit.
- **Releases:** a release of a repository is a tag with files and notes.
  Anyone who may read can list, read and download them (if they identify
  themselves, as rngit asks), and `rel` in the rules (`rel:all`, or
  `rel:<32 hex>`, or `adm`) lets someone publish, make latest and delete
  them. The tag has to be pushed first. They are kept beside the repository, in
  `demo.git.releases/<tag>/`, as stock rngit keeps them, with their notes, the
  files, each file's `.rsg` signature and the signed `manifest.rsm` the
  publisher made; the node keeps those and checks nothing, readers verify them.
  A tag is named with letters, digits and `._-+`, a file by a plain name.
  A file may be 64 MB and a release 256 MB, and all of a repository's releases
  count towards its size limit. A release can only be added to while it is
  being published.
- **Limits:** a clone's bundle or a push is held in memory and handled in one
  piece, up to *rngit push limit* (256 MB), and building or unpacking one
  stops after two minutes. A repository may come to *rngit repository limit*
  (1 GB) on disk, and a push that would go over is refused. A pushed pack is
  read before it is kept: it may hold 2 million objects, none over 128 MB
  once unpacked (a small delta that would grow into a huge file counts as the
  huge file), and 2 GB together. A fork, mirror or sync from an `rns://`
  source is read the same way before it is kept; from an `https://` source no
  object over 128 MB is unpacked. A group holds 1,000 repositories. Only
  branches and tags can be pushed; a branch moves forward unless the push is
  forced, and a tag stays where it is. A request over 1 MB is
  refused. Recent bundles are kept for ten minutes (64 MB in all), so the next
  clone of the same thing is quick; a branch that moves is a new bundle.
  rettui serves one request at a time, so a big clone makes the next client
  wait.
- **Managing it:** `H` in the [Repos tab](Repos), or **⌂ Hosted here** at the
  top of the web UI's Repos list, lists what the node hosts, with each one's
  size and, for a fork or mirror, its source and when it last synced. From
  there you can mirror (`m`, **Mirror**) or fork (`f`, **Fork**) a repository
  from an `rns://` or `https://` URL, named after the source unless you name it
  (`group/name`; the group is made if it isn't there), make an empty one to push
  to (`n`, **New**), sync a fork or mirror now (`s`, **Sync**), rename one, even
  into another group (`r`, **Rename**: its releases, work documents and rules go
  with it), copy its clone URL (`y`) and delete it with its releases and work
  documents (`X`, **Delete**, asked first). These act on the folder as its
  owner, whatever the rules say, so the node's rules don't have to give you
  anything. The node has to be running. Each one says whether anyone can clone
  it, or only those its rules name.
- **Who may use each one:** click a repository (or `Enter` on it in the
  terminal, **Who may use it** in the web UI) for a table of people and
  permissions: Read (clone), Push, Releases, Issues, Propose (work documents),
  Stats and Admin. Tick *Anyone* for everyone, *No one* to shut everyone out
  whatever the group says, or the people who may: your contacts and those you've
  talked with are listed once their keys are known, and you can add anyone by
  their LXMF address or identity hash (`a` in the terminal). A permission with
  nothing ticked is left to the group, then to *rngit access*. `g` (**Its
  group…**) does the same for the repository's group, with a Create column for
  who may make repositories in it. Saving writes the `.allowed` file, which the
  node reads for each request, so it applies at once; comments in it are
  dropped. A mirror or fork you make has no `.allowed` of its own, so until you
  tick something, the group's rules and *rngit access* decide who reads it.
- **Its pages:** as stock rngit does with `serve_nomadnet`, the node also
  serves NomadNet pages to browse what it hosts, at a NomadNet address of the
  same identity, announced with the node's name. They list its groups and
  repositories (with each one's description, from its `description` file),
  and for each repository its clone command, its files, its README (Markdown
  formatted, with links to other files in it made into page links; one over
  8 MB isn't shown), each file
  with its lines numbered and its syntax coloured, its history 50 commits a page,
  each commit with the files it changed, and its branches and tags. A file is
  shown 1,000 lines a page; one over 8 MB can only be downloaded, and one over
  1 MB is shown without colour. Markdown and Micron files are shown formatted,
  with a link to their source. Every visitor sees only the repositories they may
  read, as for a clone, and anyone who doesn't identify sees what
  `d7db22f63b453c23bb0688dde565b7c1` may. The pages use stock rngit's paths and
  page variables (`/page/repo.mu` with `g` and `r`, `/page/blob.mu` with `ref`
  and `path`...), so links made for one work on the other. `b` in the list of
  what's hosted (**Pages** in the web UI) opens them in the Browser: a
  repository's, or with nothing picked the list of them all. You see them as a
  visitor would, made here, since a client can't open a link to itself.
  *rngit pages* (`git_host_pages`, on) in Settings › Hosting turns them off.
  Colouring knows Rust, Python, C and C++, Go, JavaScript and TypeScript, Java,
  Kotlin, C#, Swift, Zig, shell, Lua, Ruby, SQL, TOML, INI, YAML, JSON, HTML,
  XML, CSS, Makefiles, Dockerfiles, diffs, Markdown and Micron, by a file's
  name, or for a script by its `#!` line.
- **Security:** the web UI can switch it on and off, name it, make, mirror,
  fork, sync, rename and delete repositories, say who may use them and turn its
  pages on or off, but can't pick the folder. Nothing in a repository is run: hooks are never
  called. Names that aren't plain folder names (`..`, a leading dot, a path)
  are refused.

`rngit serve DIR [RULES]` of the `rngit` command serves a folder the same way
without rettui.

## Blocked contacts

Someone you've blocked (in Contacts) is turned away by everything you host: your node doesn't answer them, your hub closes their connection, your propagation node refuses their requests, and your git host refuses them. They can only be told apart once they identify themselves to the service, so a visitor who doesn't is treated as anyone. The git host reads the block list when it starts: switch it off and on to apply a change, and it knows only people rettui has heard announce.
