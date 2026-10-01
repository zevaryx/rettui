Both UIs edit rettui's own settings (`settings.json`) and the config file
of the Reticulum instance it uses. Where files are kept is in
[Data and Storage](Data-and-Storage).

- **Settings editor:** the Status tab (in both UIs) edits `settings.json`:
  display name, announces, home page, propagation node (or picking one
  automatically), sync interval, how
  many messages to keep and how much disk they may use, cache time, node
  and propagation node hosting, editor line wrapping and the Reticulum
  config directory.
  Values are checked before saving, and most changes apply straight away (the
  editor says which ones wait for the next start).
- **What the web UI can't change:** the Reticulum config directory and the
  node folder, and it can turn "Run page scripts" off but not on. These decide
  what runs on the computer rettui runs on (a config's pipe interfaces run
  commands, and scripts are programs), and the web login must not be able to
  run programs there. Change them in the terminal UI, or in `settings.json`
  (for Docker, `./data/settings.json`) while rettui is stopped.
- **Reticulum config editor:** the Reticulum tab (in both UIs) edits the
  config file of the Reticulum instance rettui uses.
  - **Options:** every option rsReticulum reads, with a description and its
    default: the `[reticulum]` and `[logging]` sections, and each interface's
    own options by type (Auto, TCP, Backbone, UDP, I2P, RNode, serial, KISS,
    AX.25, pipe), plus IFAC, announce, discovery and ingress control. Keys
    rettui doesn't know are listed and kept as written.
  - **Interfaces:** add (pick a name and a type), rename, enable or disable,
    and delete them.
  - **Interface discovery:** with *Discover interfaces* and *Auto-connect*
    in `[reticulum]`, rettui finds entry points that others announce as
    discoverable and connects to up to that many; an interface marked
    *Bootstrap only* is used until they connect. Your own TCP server and
    Backbone interfaces can be published the same way (*Discoverable*).
    rettui's own Reticulum does this; a shared instance run by another
    program does its own. Known limit: rsReticulum skips announces whose
    location is left unset (Python's default), so only entry points that
    publish a location are found for now.
  - **As text:** edit the whole file, with a live check of whether Reticulum
    can load it.
  - **Safe edits:** changes are made in place, so comments and layout stay.
    Every change is checked with rsReticulum's own parser before saving, and a
    file Reticulum couldn't load is never saved. The previous version is kept
    as `config.backup`. Interfaces that will fail to start (a missing port,
    say) are shown as warnings.
  - **Applying:** Reticulum reads the file when it starts. Restart the
    Reticulum stack from inside rettui to apply changes (see below). If
    another program such as rnsd runs the shared instance, the interfaces are
    that program's, and they change when it restarts; rettui says so.
  - **Web UI:** the same editor, except that pipe interface commands (which
    run programs) can only be added or changed from the terminal.
- **Restarting Reticulum:** `Ctrl-R` in the Status or Reticulum tab (after a
  confirmation), or **Restart Reticulum** in the web UI, stops rettui's
  Reticulum stack and starts it again with the same identity, without
  quitting. Use it to apply config changes or to recover from a network
  problem. Links and transfers in progress stop (unsent messages are marked
  failed, to send again), hubs that were connected reconnect, and your node
  starts again.
- **Line wrapping:** the "Wrap editor lines" setting makes long lines in the
  page and config editors wrap instead of scrolling sideways, in both UIs.
