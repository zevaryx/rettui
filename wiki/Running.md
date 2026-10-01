With no command, `rettui` starts the terminal UI ([Using the TUI](Using-the-TUI));
`--web` starts the [Web UI](Web-UI). The other commands work from a shell,
without a UI.

## Commands

```sh
rettui                      # the TUI
rettui --web [ADDRESS]      # the web UI (default 127.0.0.1:8740)
rettui fetch <hash>[:/page/x.mu] [--raw] [--identify] [-o FILE]
rettui send <address> "text" [-a FILE]... [--mode auto|direct|propagated|paper]
rettui listen [--seconds N]  # announce, then print incoming messages
rettui sync [--node HASH]    # download messages from the propagation node
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
