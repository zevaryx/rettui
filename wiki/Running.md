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
- **Connect through RMAP World** (`rmap.world:4242`) and **Connect through
  Ratspeak** (`rns.ratspeak.org:4242`, run by the makers of rsReticulum):
  each adds a TCP interface to that public entry point to your Reticulum
  config. They're ticked only when the config has nothing besides the Auto
  interface (which reaches your local network only): a fresh install needs
  a connection for discovery to hear of others, and with two, one being
  down doesn't leave you unconnected. A config with interfaces of its own
  (an entry point, a radio) isn't given them unless you tick them. Each
  operator sees your IP address, and anyone watching your connection can
  tell you use Reticulum. They stay connected alongside the entry points
  discovery finds.
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
  [Messaging](Messaging#picking-a-propagation-node-automatically)).
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

Until you've taken them all, the Status tab (in both UIs) shows five first
steps, ticked as you take them: hear from others, announce yourself,
choose a propagation node, back up your identity, and send a message. The
terminal UI shows them on one line with the next one and its key (for
example `next: Back up your identity (b)`); the web UI lists them with how
to take each. Backing up is ticked when `b` saves a copy; the web UI can't
make one, so it has an **I've done it** button for a copy you made
yourself. Once all five are taken, the line goes away.

## Commands

```sh
rettui                      # the TUI
rettui --web [ADDRESS]      # the web UI (default 127.0.0.1:8740)
rettui fetch <hash>[:/page/x.mu] [--raw] [--identify] [-o FILE]
rettui send <address> "text" [-a FILE]... [--mode auto|direct|propagated|paper]
rettui listen [--seconds N]  # announce, then print incoming messages
rettui sync [--node HASH]    # download messages from the propagation node
rettui ping <address>        # how long a Link takes to set up, and how many hops away
rettui address [--link]      # print your LXMF address (--link: as an lxma:// link with your key)
```

`--data-dir DIR` uses another data directory, and `--rns-config DIR` another
Reticulum config.

`send --mode paper` writes a [paper message](Messaging#paper-messages)
instead of sending it. It prints the `lxm://` link, and before it the QR code
if the output is a terminal wide enough to show it.

## Which Reticulum config

By default, rettui uses the first Reticulum config it finds in `/etc/reticulum`,
`~/.config/reticulum`, or `~/.reticulum`. If a shared instance is already running
from that config (rnsd, NomadNet, Sideband), rettui joins it. Use `--rns-config DIR`
to choose a different config. The Reticulum tab edits that config file.
