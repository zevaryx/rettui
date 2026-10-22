rettui keeps its data in `~/.local/share/rettui/`:

- `identity`: the private key behind your address, in the format every
  Reticulum program uses (Python's raw 64-byte key). Lose it and the
  address is gone for good, so keep a copy: `b` in the Status tab saves one
  where you choose, readable only by you. Whoever has it can pose as you
  and read what's sent to you, so keep copies private. `B` backs up
  everything else with it (see *Backups* below).
  To keep the address you have in Sideband, NomadNet, MeshChat or another
  rettui, `i` in the Status tab (or the getting-started guide) uses that
  program's identity file from the next start; the one it replaces is kept
  beside it as `identity.previous-<date>`. Everything that takes its
  address from the identity changes with it: a node or propagation node
  you host, and who you are to RRC hubs (the question before switching
  says which apply). Don't run one identity in two
  programs at once: messages to it would be split between them. The web UI
  can't read or change the identity (the login link would otherwise give
  away the key). Stop the web UI first (two copies of rettui on one data
  folder overwrite each other's files), then use the terminal UI, or copy
  the file over `identity` (for Docker, `./data/identity`, with the
  container stopped).
- `settings.json`: display name, announce behaviour, home page, propagation
  node, sync interval, `messages_kept` and `message_storage_mb` (see
  *Message history* below), `cache_hours` (24 by default), node and
  propagation node hosting, what incoming messages must bring (stamp cost,
  size), `wrap_lines` and the Reticulum config directory. Edit it in the Status tab
  of either UI (or by hand). Changes apply straight away, except the
  Reticulum config and "announce at start", which are used the next time
  rettui starts.
- `store.json.gz`: conversations, contacts (your names for people, notes,
  who you trust or blocked), saved pages, RRC hubs, nodes you
  identify to, and the emoji you used lately, as gzip-compressed JSON (read it with
  `zcat store.json.gz | jq`). Earlier versions kept it as plain `store.json`,
  about five times larger. The first launch after upgrading converts it,
  checks that the new file reads back the same, and only then removes the
  old one. Changes are saved every 10 seconds on a background thread, so a
  large store doesn't pause the UI, and once more when rettui exits.
- `peers.json.gz`: the LXMF peers, NomadNet nodes and propagation nodes
  heard. Announces arrive several a second on a busy network, so they are
  saved apart from the store, which is then only written again when a
  conversation changes. A new peer, or a changed name, is saved within 10
  seconds; only hearing one again, every 5 minutes. A peer not heard from in
  30 days is forgotten (with its key in `known_identities.json`), unless
  you have a conversation with it, it's a contact, a saved page or hub is
  on it, or it's your propagation node. Earlier versions kept them in the
  store; the first launch after upgrading moves them.
- `archive/`: older messages (see *Message history* below), one file per
  month, such as `archive/2026-09.jsonl.gz`: one message per line, with the
  conversation it's from. Read it with `zcat archive/*.jsonl.gz | jq`.
- `known_identities.json`: public keys from LXMF and propagation-node
  announces, so you can reach peers that are offline now. Saved every 30
  seconds (when it changed) and when rettui exits.
- `tickets.json`: [stamp tickets](Messaging#blocking-and-spam) you gave
  trusted contacts and ones they gave you. Only you can read it.
- `games.json`: your [games](Games) with contacts, as LRGP keeps them (the
  board, whose turn it is, the moves). Saved as moves are made.
- `reticulum-history/`: earlier versions of the Reticulum config, as rettui
  saved them ([History](Settings-and-Reticulum-Config)), the newest 50 of
  each config file: one `.config` file each, listed in `index.json`. Not in
  backups: the config they're of is this computer's.
- `rrc-hub/` and `rrc-hub-identity`: the [RRC hub hosted
  here](Hosting-a-Node#rrc-hub), once it has started: rsRRCD's
  `config.yaml`, its registered rooms (`rooms.yaml`), and the hub's own
  identity.
- `ratchets/`: your address's ratchets, one file per address, named as
  Python LXMF names them. As in Sideband, NomadNet and MeshChat, your
  announces carry the newest of a set of keys (a new one at most every
  half hour, as you announce), and senders who heard it encrypt to it: a
  message can't be read later with your identity's key alone (forward
  secrecy). The last 512 are kept, for messages still on their way, such
  as ones waiting on a propagation node. Deleting the folder makes new
  ones, but messages sent to the old keys can't be read any more. It
  needn't be backed up.
- `cache/`: cached NomadNet pages and images
- `rrc/`: RRC chat history, one file per hub (the newest messages of each room and whisper conversation, 500 unless "Chat messages kept", `rrc_messages_kept`, says otherwise)
- `rettui.log`: logging, as detailed as *Log level* says (Status, `log_level`: error, warn by default, info, debug or trace; it changes at once). `RETTUI_LOG=debug`, set when rettui starts, decides instead
- `downloads/`: received attachments (one folder per sender) and NomadNet files
- `uploads/`: files attached to messages sent from the web UI, and the
  smaller copies of pictures sent (see
  [Messaging](Messaging))
- `update-check.json`: what the last [update check](Installing#updating)
  found, and when
- `map-tiles/`: pictures for the web UI's [map](Messaging#locations-and-the-map),
  kept a month (200 MB at most); deleting it is harmless
- `node/`: your node's `pages/` (with pictures added in the editor in `pages/images/`) and `files/`
- `propagation/`: messages your [propagation node](Hosting-a-Node#propagation-node)
  keeps for others, one file each, encrypted for their recipients
- `web_token`: the web UI's login secret (delete it to log every browser out)
- `web-tls/`: rettui's own HTTPS certificates, with `--https` (see
  [Web UI](Web-UI)): its certificate authority (`ca.pem`, and its key
  `ca.key`) and the certificate it signed for this computer (`cert.pem`,
  `cert.key`). Delete the folder for a new authority, which devices then
  install again.
- `web_push_key`: the web UI's key for background notifications (Web Push),
  and `web_push.json`: the browsers that turned them on. Deleting the key
  makes browsers subscribe again the next time they open rettui.
- `identities/`: the [other identities](#several-identities) kept here, a
  folder each, and `active`, which says which is in use.

## Several identities

rettui can keep several identities and use one at a time, as Columba and
MeshChat can. Each is an LXMF address of its own, with its own name,
conversations, contacts, games, settings and ratchets, so switching is
being someone else: a work address and a personal one, say, or one for a
group.

`I` in the Status tab lists them (the web UI: **👤 Identities**, beside the
display name), the one in use marked:

- **New** (`n`, **+ New identity**): a new key and address, with the name
  it announces. It starts with the settings of the one in use, but nothing
  hosted (node, propagation node, hub), shared (location) or drawn (icon)
  of it; the keys of others heard so far come along, so it can write to
  them at once.
- **From a file** (`i`, terminal only): keep the identity in a file from
  Sideband, NomadNet, MeshChat or another rettui as another one, rather
  than in place of the one in use (`i` in the Status tab does that).
- **Switch** (`Enter`, **Switch**): everything is saved, then rettui
  starts over as that identity: its settings, conversations and network
  (it announces as it). The web UI comes back at the same address, with
  the same login, and loads again by itself. rettui starts as the identity
  used last.
- **Rename** (`r`), **QR code** (`c`) of its `lxma://` link, to be added as a
  contact, and **copy** that link (`y`).
- **Delete** (`x`, terminal only): an identity, with its key,
  conversations and contacts, once its name is typed. Not the first (it's
  the data directory's own) nor the one in use. Back it up first (switch
  to it, `b`) if its address may be wanted again.

The first identity is kept where it always was, in the data directory
itself; each one added is a folder in `identities/` (named by the start of
its LXMF address) laid out as the data directory is, with its own
`identity`, `settings.json`, `store.json.gz` and the rest. What isn't about
who you are is the data directory's, whichever identity is in use: the
log, the web UI's login, certificates and browsers, the update check, the
page cache, map pictures and the Reticulum config's history. A
[backup](#backups) is of the identity in use.

The web UI can't bring an identity in from a file, copy one or delete one:
its login would otherwise give the keys away. `rettui identity export FILE`
and `rettui identity import FILE --name NAME` do the first two from the
command line, and the [Android app](Android) has a screen of its own for them.

## Backups

A backup is one `.tar.gz` file holding what's yours to keep:
`settings.json`, the store (conversations, contacts, saved pages, RRC
hubs), `peers.json.gz`, the `archive/` of older messages,
`known_identities.json`, `tickets.json`, your [games](Games) (`games.json`), RRC history (`rrc/`), your node's
pages and files (`node/`), your hub's settings and rooms (`rrc-hub/`) and,
unless left out, the `identity` (and the hub's, `rrc-hub-identity`). Caches,
the log, ratchets, the propagation node's messages and the web UI's login,
certificates and push keys aren't kept: rettui makes them again.
Attachments (`downloads/`, `uploads/`) are left out unless asked for.

- **Terminal UI:** `B` in the Status tab asks where to save it (your home
  folder, `rettui-backup-<date>.tar.gz`, to begin with), with your
  identity, once what's waiting to be saved is written. The log says when
  it's done.
- **Web UI:** **Download a backup** (Status, *Backup*). It never has the
  identity (the login link would otherwise give away the key): back that
  up in the terminal UI, or with `rettui backup`. In the
  [Android app](Android), *Identity and backups* saves one with the identity.
- **Command line:** `rettui backup [FILE]` (by default
  `rettui-backup-<date>.tar.gz` here); `--with-files` adds attachments,
  `--without-identity` leaves the identity out. It reads what's saved, so
  with rettui running it's what was saved last (every 10 seconds).

A backup with the identity is as private as the identity itself: the file
is made readable only by you, and whoever has it can pose as you and read
what's sent to you. rettui never writes over a file that's there already.

To restore, stop rettui, then `rettui restore FILE` (with `--data-dir DIR`
for another data directory). Into a data directory that's empty, or new,
it just unpacks. One that has an identity or messages already is left
alone, unless `--force`: then what the backup replaces is first moved into
a `before-restore-<time>/` folder there. A restored backup without an
identity gets a new one on the next start, with new addresses: restore
yours too (`i` in the Status tab, or copy it over `identity`) to keep
them. Restoring takes only rettui's own files from the backup, and never
writes outside the data directory.

## Message history

- **Messages kept** (`messages_kept`, 1000 by default): each conversation
  keeps and shows its newest this many messages. Older ones move to
  `archive/` rather than being deleted, so the store rettui loads and saves
  stays small. `0` keeps every message in the store. The first launch after
  upgrading archives what's over it, and says so in the log.
- **Reading the archive:** a conversation's archived messages open from
  the conversation, only to read (they can't be replied to or deleted one
  by one). In the TUI, press `H` in Messages: they open over the tab, the
  newest at the bottom; `↑↓`, `PgUp`/`PgDn` and the mouse wheel scroll, and
  `Home` goes to the oldest. In the web UI, scroll to the top of the
  conversation and press **Show N archived messages**: they go in above,
  with their files and pictures. [Searching](Messaging#searching) looks
  through the messages kept, not the archive.
- **Message storage** (`message_storage_mb`, no limit by default): the most
  disk, in megabytes, that the store and the archive may use together.
  Past it, the oldest months of the archive are deleted, oldest first. If
  the messages kept alone take more, the log says so: lower *Messages kept*.
  Attachments (`downloads/`, `uploads/`) aren't counted.
- Both settings apply straight away, from the Status tab of either UI.
