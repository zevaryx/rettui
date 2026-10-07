rettui keeps its data in `~/.local/share/rettui/`:

- `identity`: the private key behind your address, in the format every
  Reticulum program uses (Python's raw 64-byte key). Lose it and the
  address is gone for good, so keep a copy: `b` in the Status tab saves one
  where you choose, readable only by you. Whoever has it can pose as you
  and read what's sent to you, so keep copies private.
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
  conversation changes. Earlier versions kept them in the store; the first
  launch after upgrading moves them.
- `archive/`: older messages (see *Message history* below), one file per
  month, such as `archive/2026-09.jsonl.gz`: one message per line, with the
  conversation it's from. Read it with `zcat archive/*.jsonl.gz | jq`.
- `known_identities.json`: public keys from LXMF and propagation-node
  announces, so you can reach peers that are offline now. Saved every few
  seconds and when rettui exits.
- `tickets.json`: [stamp tickets](Messaging#blocking-and-spam) you gave
  trusted contacts and ones they gave you. Only you can read it.
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
- `rrc/`: RRC chat history, one file per hub
- `rettui.log`: logging, as detailed as *Log level* says (Status, `log_level`: error, warn by default, info, debug or trace; it changes at once). `RETTUI_LOG=debug`, set when rettui starts, decides instead
- `downloads/`: received attachments (one folder per sender) and NomadNet files
- `uploads/`: files attached to messages sent from the web UI, and the
  smaller copies of pictures sent (see
  [Messaging](Messaging))
- `map-tiles/`: pictures for the web UI's [map](Messaging#locations-and-the-map),
  kept a month (200 MB at most); deleting it is harmless
- `node/`: your node's `pages/` and `files/`
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
