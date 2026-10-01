rettui keeps its data in `~/.local/share/rettui/`:

- `identity`
- `settings.json`: display name, announce behaviour, home page, propagation
  node, sync interval, `messages_kept` and `message_storage_mb` (see
  *Message history* below), `cache_hours` (24 by default), node hosting,
  `wrap_lines` and the Reticulum config directory. Edit it in the Status tab
  of either UI (or by hand). Changes apply straight away, except the
  Reticulum config and "announce at start", which are used the next time
  rettui starts.
- `store.json.gz`: conversations, saved pages, RRC hubs, nodes you
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
- `cache/`: cached NomadNet pages and images
- `rrc/`: RRC chat history, one file per hub
- `rettui.log`: logging, set with `RETTUI_LOG=debug`
- `downloads/`: received attachments (one folder per sender) and NomadNet files
- `uploads/`: files attached to messages sent from the web UI
- `node/`: your node's `pages/` and `files/`
- `web_token`: the web UI's login secret (delete it to log every browser out)
- `web_push_key`: the web UI's key for background notifications (Web Push),
  and `web_push.json`: the browsers that turned them on. Deleting the key
  makes browsers subscribe again the next time they open rettui.

## Message history

- **Messages kept** (`messages_kept`, 1000 by default): each conversation
  keeps and shows its newest this many messages. Older ones move to
  `archive/` rather than being deleted, so the store rettui loads and saves
  stays small. `0` keeps every message in the store. The first launch after
  upgrading archives what's over it, and says so in the log.
- **Message storage** (`message_storage_mb`, no limit by default): the most
  disk, in megabytes, that the store and the archive may use together.
  Past it, the oldest months of the archive are deleted, oldest first. If
  the messages kept alone take more, the log says so: lower *Messages kept*.
  Attachments (`downloads/`, `uploads/`) aren't counted.
- Both settings apply straight away, from the Status tab of either UI.
