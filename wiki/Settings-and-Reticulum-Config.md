Both UIs edit rettui's own settings (`settings.json`) and the config file
of the Reticulum instance it uses. Where files are kept is in
[Data and Storage](Data-and-Storage).

- **Settings editor:** the Status tab (in both UIs) edits `settings.json`:
  display name, announces, home page, propagation node (or picking one
  automatically), sync interval, how
  many messages to keep and how much disk they may use, cache time, node
  and propagation node hosting, editor line wrapping and the Reticulum
  config directory.
  They're in groups, one shown at a time, chosen from the tabs above them:
  *Profile* (your name, icon and announces), *Messages*, *Location*,
  *Browsing*, *Hosting* (a node, a propagation node, an RRC hub),
  *Display*, *Notifications* and *System*. In the terminal UI, `Tab` (or
  `←` `→`) moves between them. In the web UI, a group with changes not yet
  saved is marked •; *Save* saves them all, whichever group they're in.
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
  - **Interface discovery:** with *Discover interfaces* in `[reticulum]`,
    Reticulum finds entry points that others announce as discoverable. It
    hears of them through the connections you already have, so it needs
    one to start. Something then has to connect to them:
    - **rettui** (what the getting-started guide sets up): *Discovered
      entry points* in rettui's settings (System) says how many, with
      *Auto-connect* (`autoconnect_discovered_interfaces`) left at 0.
      rettui connects to the best ones found (run by transport nodes, heard
      in the last 3 days), drops one that isn't up within 12 seconds and
      tries another (one dropped is tried again after 5 minutes), and
      handles interfaces marked *Bootstrap only* as Python Reticulum does:
      they're disconnected once that many found entry points are up, and
      connected again as soon as none is. A TCP client interface can be
      brought back; another kind marked *Bootstrap only* stays connected.
    - **Reticulum**, with *Auto-connect* set to how many. rsReticulum
      drops interfaces marked *Bootstrap only* as soon as it starts
      connecting to that many, before any of them is up, never brings
      them back, and keeps one that never comes up instead of trying
      another; after a restart, the ones it found before drop them
      straight away. So a config that does it this way is warned of any
      interface marked *Bootstrap only*, and the guide doesn't mark the
      entry points it adds that way. Earlier versions of the guide set
      discovery up like this; it still works, and the guide leaves it.

    rettui's own Reticulum does this; a shared instance run by another
    program does its own. With discovery on, the log says so each time
    rettui's Reticulum starts, and says as it connects to an entry point
    found, drops one, or disconnects or reconnects the *Bootstrap only*
    ones. Your own interfaces marked *Discoverable* are announced too.
  - **As text:** edit the whole file, with a live check of whether Reticulum
    can load it.
  - **Safe edits:** changes are made in place, so comments and layout stay.
    Every change is checked with rsReticulum's own parser before saving, and a
    file Reticulum couldn't load is never saved. The previous version is kept
    as `config.backup`. Interfaces that will fail to start (a missing port,
    say) are shown as warnings.
  - **History:** each time rettui saves the file it keeps that version, with
    what was done ("Removed interface Hilltop", "Logging: loglevel = 5",
    "Edited as text"), and, the first time, the file as it was before rettui
    changed it; a file another program changed meanwhile is kept too
    ("Changed outside rettui"). `H` in the Reticulum tab (**🕘 History** in
    the web UI) lists them, newest first, beside how the one picked
    differs, line by line: from the version before it (*What it changed*),
    or from the file now (*What putting it back would change*; `c` switches
    between them). `Enter` (**Put this version back**) puts it back, after
    asking: it's checked and saved as any edit is, and kept in the history
    in its turn, so it can be undone the same way. The newest 50 versions of
    each config file are kept, in `reticulum-history/` in rettui's
    [data folder](Data-and-Storage). In the web UI, a version that would add
    or change a pipe interface's command can't be put back (the terminal
    can).
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
