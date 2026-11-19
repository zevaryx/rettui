With no command, `rettui` starts the terminal UI ([Using the TUI](Using-the-TUI));
`--web` starts the [Web UI](Web-UI). The other commands work from a shell,
without a UI.

## The first start

Reticulum's default config only reaches your local network, so a new install
usually hears no one. The first time rettui starts, in either UI, it opens a
**Getting started** guide (`g` in the Status tab, or **Getting started** on
the web UI's Status page, opens it again):

- **Your name:** the display name others see in your announces. Until you
  choose one it is `rettui` and the first 10 characters of your LXMF
  address (such as `rettui 3fa9c2e1b7`), so it differs from everyone
  else's. An older install still called `rettui user` gets this name too
  when it next starts; a name you chose stays.
- **Identity** (terminal UI): use the identity file from Sideband,
  NomadNet, MeshChat or another rettui, to keep that address (see
  [Data and Storage](Data-and-Storage)). The web UI says where the file is
  instead.
- **Entry points:** public entry points to connect through, the ones
  Colorado Mesh's [mesh-client](https://github.com/Colorado-Mesh/mesh-client)
  recommends, under the same headings (all but RNS Dublin Mainnet, whose
  address no longer exists):

  | Group | Entry point | Address |
  |---|---|---|
  | Primary & global backbone | RNS Between The Borders | `reticulum.betweentheborders.com:4242` |
  | | RMAP World | `rmap.world:4242` |
  | | RNS Simply Equipped | `rns.simplyequipped.com:4242` |
  | | RNS Beleth | `rns.beleth.net:4242` |
  | North America | MichMesh | `rns.michmesh.net:7822` |
  | Specialty | Ratspeak & Colorado Mesh | `rns.ratspeak.org:4242` |

  Each ticked adds a TCP interface to that entry point to your Reticulum
  config. **Opening the guide connects to no one**: the entry points are
  listed with `not checked yet` beside them, and a line above says nothing
  has been contacted. On a new install the first three in the list are
  ticked to start with. The first-run guide **asks** (a dialog in the web UI,
  a prompt in the terminal UI: `y`, or `n` / Enter for no) whether rettui may
  connect to the listed entry points now, to see which answer fastest; the
  people who run them will see your IP address. **Yes** tries each one (a
  connection, closed at once) and moves the ticks to the fastest as answers
  arrive. **No** (the default; so is Esc) leaves the ticks as they are and
  tries nothing, and the question is not asked again; pressing **Apply**
  then connects to the three ticked. Only the first-run guide asks, and the
  web UI does not ask again when the page is reloaded in the same browser
  session. A button (a row in the terminal UI) reads **Allow connections**
  until a check was allowed, and does what Yes does; after that it reads
  **Check which can be reached** and repeats the check, saying "Checked a
  moment ago" if the last one is under a minute old. The answers are shown
  (`up · 120 ms`); one that refuses, doesn't answer, or whose name isn't found
  is greyed out and can't be ticked (`down · refused`). If none answer, the
  guide says so: the device may be offline, or a firewall may block them.
  **Apply** connects to the ones ticked whether or not they were checked.
  *Also find entry points near you over time* lets rettui connect to entry
  points that other nodes announce, so those are chosen by others, not from
  this list.

  When the config has nothing besides the Auto interface (which reaches
  your local network only), the three first (and, once you have checked, the three that answer fastest) are ticked: a
  fresh install needs a connection for discovery to hear of others, and
  with a few, one being down doesn't leave you unconnected. While they're
  being tried, the ticks move to the fastest as answers come in; once you
  tick or untick one yourself, they stay as you set them. A config with
  interfaces of its own (an entry point, a radio) isn't given any unless
  you tick them; one it already has (by address) shows as in your config
  already. Each operator sees your IP address, and anyone watching
  your connection can tell you use Reticulum. With discovery ticked too,
  rettui also connects to two of the entry points discovery finds, and
  these ones stay connected either way. (With *Disconnect Bootstrap only
  interfaces* on, in the settings, they're added as *Bootstrap only* instead:
  used until two found are up, then disconnected, and connected again as soon
  as none of those is.)
- **Also find entry points near you over time:** turns on
  interface discovery, as Reticulum's manual recommends, and sets
  *Discovered entry points* to 2: rettui connects to two entry points
  others announce, and swaps one that stays down for another (see
  [Settings and Reticulum Config](Settings-and-Reticulum-Config)). You'll
  connect to hosts you didn't choose, though never one that is this
  computer itself (however it's written) or a Tor address. Discovery hears of entry points
  through a connection you already have, so it needs one to start. Like
  the entry points, it's ticked only for a config with nothing besides the
  Auto interface: a config you've set up yourself is changed only as you
  ask. When the config is Python Reticulum's (in `~/.reticulum`,
  `~/.config/reticulum` or `/etc/reticulum`), the guide says it's shared
  with NomadNet, Sideband and rnsd, since changes to it are theirs too.
- **Pick a propagation node automatically** (see
  [Messaging](Messaging#picking-a-propagation-node-automatically)):
  ticked the first time the guide opens, unless you picked a node by hand,
  with a warning under it while it's ticked: the node picked sees who your
  messages are for and when you collect them, and could lose them. Opened
  again later, it's as you set it.
- **Check for updates once a day (recommended):** not ticked to start
  with. Ticked, rettui asks GitHub once a day whether a newer release is
  out, and says so (see [Updating](Installing#updating)); GitHub sees your
  IP address, so leave it off if you use Reticulum to stay off the
  internet. It's *Check for updates* in Status too.
- **Learn more:** Reticulum's manual (getting started, understanding
  Reticulum), RMAP World's map and
  directory.rns.recipes for finding entry points, using a LoRa radio
  ([RNode Radios](RNode-Radios): rettui doesn't flash radios, the page
  covers the tools that do), the words you'll meet
  ([Glossary](Glossary)), and this wiki.

Nothing changes until you choose **Apply**; if the Reticulum config changed,
Reticulum restarts to connect. As each entry point connects, rettui says
so; for one that hasn't within 45 seconds, it says why, in plain words, and
keeps trying
(in the log, and in the footer in the terminal UI). **Not now** (or Esc) closes it for good. If
another program (such as rnsd) runs the shared instance, the guide leaves
its config alone and says so. Installs from before the guide existed don't
see it at start.

### First steps

On a new install, the Status tab (in both UIs) shows five first steps,
ticked as you take them: hear from others, announce yourself, choose a
propagation node, back up your identity, and send a message. The terminal
UI shows them on one line with the next one and its key (for example
`next: Back up your identity (b)`); the web UI lists them with how to take
each. Backing up is ticked when `b` saves a copy; the web UI can't make
one, so it has an **I've done it** button for a copy you made yourself.
Once all five are taken the line goes away; `x` (terminal UI) or **Hide**
(web UI) hides it sooner, for good. Installs from before the first steps
existed don't show them.

## Commands

```sh
rettui                      # the TUI
rettui --web [ADDRESS]      # the web UI (default 127.0.0.1:8740)
rettui --web [ADDRESS] --https                          # over HTTPS, with rettui's own certificate
rettui --web ADDRESS --insecure-http                    # plain HTTP on an address others can reach (refused without this)
rettui --web [ADDRESS] --tls-cert FILE --tls-key FILE   # over HTTPS, with yours
rettui --web [ADDRESS] --stats-listen ADDRESS           # also a read-only stats page (see below)
rettui node --stats-listen ADDRESS                      # the same, for a node with no UI
rettui node --stats-listen ADDRESS --stats-token-file PATH   # and a peer list for whoever has the token (or RETTUI_STATS_TOKEN)
rettui fetch <hash>[:/page/x.mu] [--raw] [--identify] [-o FILE]
rettui send <address> "text" [-a FILE]... [--mode auto|direct|propagated|paper]  # address or lxma:// link
rettui listen [--seconds N]  # announce, then print incoming messages
rettui sync [--node HASH]    # download messages from the propagation node
rettui ping <address>        # how long a Link takes to set up, and how many hops away
rettui path <address> [-d]   # find the path to any destination (-d: forget it)
rettui path -t               # list every path known
rettui probe <address> [--name NAME]  # time an answer, as rnprobe does
rettui update [--yes]  # check for a newer release, and install it (see Installing)
rettui backup [FILE] [--with-files] [--without-identity] [--passphrase | --key KEY | --no-passphrase]  # everything to one file, sealed if you like (see Data and Storage)
rettui backup-key            # turn a passphrase into a key that seals backups without it
rettui unseal FILE [OUT] [--identity ID]  # open a sealed backup without restoring it (see Sealed Backups)
rettui restore FILE [--force] [--keep-terminal-settings]  # restore one, with rettui stopped (a sealed one asks its passphrase)
rettui settings export FILE [--unreadable]  # save a copy of the settings (or the file rettui set aside because it couldn't read it)
rettui settings import FILE [--keep-terminal-settings]  # use the settings in a file, with rettui stopped (refused if it can't be read)
rettui identity export FILE  # save a copy of the identity in use (never over another file)
rettui identity import FILE --name NAME  # keep another identity from a file, to switch to
rettui migrate-propagation   # convert the hosted propagation node's message store after upgrading to 2.0, with rettui stopped (see Hosting a Node)
rettui address [--link]      # print your LXMF address (--link: as an lxma:// link with your key)
rettui node [--entry-point PORT] [--listen-ip IP] [--reachable-on HOST] [--name NAME]  # a public node with no UI, as rnsd and lxmd (see Hosting a Node); each flag is also a RETTUI_NODE_* variable
```

`--data-dir DIR` uses another data directory, and `--rns-config DIR` another
Reticulum config.

`path` and `probe` take any destination's address: an LXMF address, a
NomadNet node, a propagation node or an RRC hub. A path not known is asked
for up to three times over a minute; each request after the first is
printed as it goes. With rettui, rnsd or another Reticulum program running
as the shared instance, `path` shows, finds and forgets that instance's
paths; otherwise, the paths the last run with this Reticulum config saved.
`probe --name` gives a destination's full name (such as `rnsh.listen`) for
kinds rettui can't tell, which are sent a probe packet they must prove. See
[Network](Network) for how paths and probes work.

`identity export` and `identity import` work on identity files from any
Reticulum program (Sideband, NomadNet, MeshChat, another rettui), all in the
same format. `import` keeps the identity as another one beside the one in use,
which stays in use: switch to it in Status > Identities (`I` in the terminal
UI). See [Data and Storage](Data-and-Storage#several-identities).

`send --mode paper` writes a [paper message](Messaging#paper-messages)
instead of sending it. It prints the `lxm://` link, and before it the QR code
if the output is a terminal wide enough to show it.

## The stats endpoint

A public node can show a few numbers on a status page. `--stats-listen
ADDRESS` (with `--web`, or after `rettui node`) serves a read-only summary at
`/stats.json` on its own address, with no login. It is off unless you give it, and it publishes
totals only: no addresses, no names of peers or interfaces, no clients' addresses, and no row for
each interface or client. It can't change anything and has no way into the web UI.

```sh
rettui --web 127.0.0.1:8740 --stats-listen 127.0.0.1:8741
rettui node --stats-listen 127.0.0.1:8741    # or, on a node with no UI
curl http://127.0.0.1:8741/stats.json
```

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
    "messages_sent_to_peers": 800, "messages_received_from_peers": 400, "unhandled_messages": 35
  },
  "disk": { "data_bytes": 52000000 }
}
```

`propagation` (the propagation node's stored messages, limit, peers and
message counts) is there only while that node runs, and `disk` (the size of
the data directory, and for a request with the token also the size and free space of its file system, measured in the background at most
once a minute) once measured. Both are totals with no addresses; see the
[field notes](Hosting-a-Node#a-stats-page-for-a-status-page).

- `generated_at` is unix seconds, and the numbers are updated every second.
  `uptime_seconds` counts from when rettui started.
- `interfaces.clients` adds up the clients connected to the node's server
  interfaces, and is `null` when none report any (so does an interface's
  own `clients` when it is not a server). Interfaces are counted, not
  listed, so their names and each client's details stay private.
- Only `GET` and `HEAD` for `/` and `/stats.json` are answered (others get
  405 or 404), and the reply is marked `Cache-Control: no-store`. In the
  first moments after start, before there are numbers, it answers 503.
- A listener that can't be opened (the port is taken, say) stops rettui
  at start, with the reason. It is not saved in the settings: give it on
  every run.
- With `--stats-token-file PATH` (or the variable `RETTUI_STATS_TOKEN`; not
  both), a request that has the token in `Authorization: Bearer TOKEN` or
  `X-Stats-Token: TOKEN` also gets `"authorised": true`, a `peers` list
  with every peer's address, type and name, and a `propagation_peers` list of the nodes it is peered with. A token in the URL is not read,
  because URLs end up in logs, and a wrong or missing token gets the totals
  above, with status 200. See [Hosting a Node](Hosting-a-Node#the-peer-list-for-whoever-has-the-token)
  for the details and what to keep in mind: treat the token as a secret.

The listener has no TLS, no limits on who asks and no timeouts of its own, so
bind it to `127.0.0.1` or a private address and let a reverse proxy
publish it. With nginx, on the host that serves `https://node.example/`:

```nginx
location = /stats.json {
    proxy_pass http://127.0.0.1:8741/stats.json;
    proxy_read_timeout 5s;
    limit_except GET HEAD { deny all; }
}
```

## Which Reticulum config

By default, rettui uses the first Reticulum config it finds in `/etc/reticulum`,
`~/.config/reticulum`, or `~/.reticulum`. If a shared instance is already running
from that config (rnsd, NomadNet, Sideband), rettui joins it. Use `--rns-config DIR`
to choose a different config. The Reticulum tab edits that config file.

## Running as a service

To keep the web UI up without a terminal open (on a home server, or a
Raspberry Pi with an RNode), run `rettui --web` as a systemd service. The
login link is printed to the service's log; it's also in `web_token` in the
data directory (`https://…/?token=` followed by its contents).

**For your own user** (no root needed), save this as
`~/.config/systemd/user/rettui.service`, with the path to rettui (`which
rettui`) and the flags you want:

```ini
[Unit]
Description=rettui web UI
Wants=network-online.target
After=network-online.target

[Service]
ExecStart=%h/.cargo/bin/rettui --web 127.0.0.1:8740
Restart=on-failure
RestartSec=10

[Install]
WantedBy=default.target
```

Then:

```sh
systemctl --user daemon-reload
systemctl --user enable --now rettui
loginctl enable-linger "$USER"     # keep it running while you're logged out
journalctl --user -u rettui -e     # the login link, and anything printed
```

**For the whole machine**, as a user of its own, save it as
`/etc/systemd/system/rettui.service`:

```ini
[Unit]
Description=rettui web UI
Wants=network-online.target
After=network-online.target rnsd.service

[Service]
User=rettui
Group=rettui
# An RNode on a serial port needs the dialout group (uucp on some systems).
SupplementaryGroups=dialout
ExecStart=/usr/local/bin/rettui --data-dir /var/lib/rettui --rns-config /var/lib/rettui/reticulum --web 0.0.0.0:8740 --https
StateDirectory=rettui
Restart=on-failure
RestartSec=10
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

```sh
sudo useradd --system --home-dir /var/lib/rettui --shell /usr/sbin/nologin rettui
sudo systemctl daemon-reload
sudo systemctl enable --now rettui
sudo journalctl -u rettui -e
```

Things to know:

- **The Reticulum config:** Reticulum keeps its storage beside its config,
  so the service must be able to write there. The user unit uses the first
  config found (below), as for you. The system unit above keeps its own in
  `/var/lib/rettui/reticulum` (made on the first start; set it up in the
  web UI's Reticulum section), since `ProtectSystem=strict` makes `/etc`
  read-only and `ProtectHome=true` hides home directories.
- **rnsd:** to join an rnsd service as a shared instance instead, use its
  config (`--rns-config /etc/reticulum`), add `ReadWritePaths=/etc/reticulum`,
  and keep `After=rnsd.service` so rettui starts once it's up.
- **Listening:** `0.0.0.0` reaches it from other devices. Prefer `--https`
  (see [Web UI](Web-UI)), a VPN, or a reverse proxy in front of it,
  as anyone with the link can read and send your messages.
- **Logs:** what rettui prints goes to the journal; its own log is
  `rettui.log` in the data directory, as detailed as *Log level* (Status)
  says.
- **Stopping:** `systemctl stop` lets rettui leave its RRC hubs and save
  everything, as Ctrl-C does.

**A public node with no UI:** for a server that only relays and keeps
messages for others, use [`rettui node`](Hosting-a-Node#a-public-node-with-no-ui)
in place of `--web` in either unit, for example
`ExecStart=/usr/local/bin/rettui --data-dir /var/lib/rettui node`. It
needs no login link, and prints its status to the journal every 10
minutes. With the system unit, `node` keeps its Reticulum config in
`/var/lib/rettui/reticulum` too.
