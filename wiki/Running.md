With no command, `rettui` starts the terminal UI ([Using the TUI](Using-the-TUI));
`--web` starts the [Web UI](Web-UI). The other commands work from a shell,
without a UI.

## The first start

Reticulum's default config only reaches your local network, so a new install
usually hears no one. The first time rettui starts, in either UI, it opens a
**Getting started** guide (`g` in the Status tab, or **Getting started** on
the web UI's Status page, opens it again):

- **Your name:** the display name others see in your announces.
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
  config. rettui tries each as the guide opens (a connection, closed at
  once) and shows how quickly it answered (`up · 120 ms`); one that
  refuses, doesn't answer, or whose name isn't found is greyed out and
  can't be ticked (`down · refused`), and it's tried again when the guide
  is opened a minute or more later. If none answer, the guide says so:
  the device may be offline, or a firewall may block them.

  When the config has nothing besides the Auto interface (which reaches
  your local network only), the three that answer fastest are ticked: a
  fresh install needs a connection for discovery to hear of others, and
  with a few, one being down doesn't leave you unconnected. While they're
  being tried, the ticks move to the fastest as answers come in; once you
  tick or untick one yourself, they stay as you set them. A config with
  interfaces of its own (an entry point, a radio) isn't given any unless
  you tick them; one it already has (by address) shows as in your config
  already. Each operator sees your IP address, and anyone watching
  your connection can tell you use Reticulum. They stay connected
  alongside the entry points discovery finds.
- **Also find entry points near you over time:** turns on
  interface discovery, as Reticulum's manual recommends, connecting to up to
  two entry points others announce (see
  [Settings and Reticulum Config](Settings-and-Reticulum-Config)). You'll
  connect to hosts you didn't choose. Discovery hears of entry points
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
rettui --web [ADDRESS] --tls-cert FILE --tls-key FILE   # over HTTPS, with yours
rettui fetch <hash>[:/page/x.mu] [--raw] [--identify] [-o FILE]
rettui send <address> "text" [-a FILE]... [--mode auto|direct|propagated|paper]  # address or lxma:// link
rettui listen [--seconds N]  # announce, then print incoming messages
rettui sync [--node HASH]    # download messages from the propagation node
rettui ping <address>        # how long a Link takes to set up, and how many hops away
rettui path <address> [-d]   # find the path to any destination (-d: forget it)
rettui path -t               # list every path known
rettui probe <address> [--name NAME]  # time an answer, as rnprobe does
rettui address [--link]      # print your LXMF address (--link: as an lxma:// link with your key)
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

`send --mode paper` writes a [paper message](Messaging#paper-messages)
instead of sending it. It prints the `lxm://` link, and before it the QR code
if the output is a terminal wide enough to show it.

## Which Reticulum config

By default, rettui uses the first Reticulum config it finds in `/etc/reticulum`,
`~/.config/reticulum`, or `~/.reticulum`. If a shared instance is already running
from that config (rnsd, NomadNet, Sideband), rettui joins it. Use `--rns-config DIR`
to choose a different config. The Reticulum tab edits that config file.
