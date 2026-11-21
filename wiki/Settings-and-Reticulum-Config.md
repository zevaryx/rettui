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
  editor says which ones wait for the next start). Text settings (names, the
  hub greeting, paths and commands) can't hold control characters, such as a
  line break, a tab or an escape, invisible characters (zero-width spaces,
  direction marks, Hangul fillers, tag characters and the like), or
  characters that change the text's direction, and can't be made only of
  spaces and characters that show nothing: names and the greeting are
  announced or sent to others. The zero-width joiner and the emoji
  variation selector are fine, so emoji work (flags of regions such as
  England's, made with tag characters, don't). A line
  break in the hub greeting is kept as `\n` (how you write one), and rngit
  access rules may be one a line, with tabs between words, as in a
  `.allowed` file. A value already in `settings.json` (written by hand, or by
  an older version) is kept, and can be saved again as it is.
- **What the web UI can't change:** the Reticulum config directory and the
  node folder, *Script PATH* (where page scripts look for programs), *Who may
  send files* and *Received files folder* (for rncp), nor
  *Use rngit's identity* (which reads another program's
  identity key, see [Repos](Repos#who-you-are-to-a-node)), and it can turn
  "Run page scripts" and "Receive files with rncp" off but not on (and
  "Disconnect Bootstrap only interfaces" too, which lets entry points found
  take the place of yours). These decide
  what runs on the computer rettui runs on (a config's pipe interfaces run
  commands, and scripts are programs), and the web login must not be able to
  run programs there. Change them in the terminal UI, or in `settings.json`
  (for Docker, `./data/settings.json`) while rettui is stopped.
- **Reticulum config editor:** the Reticulum tab (in both UIs) edits the
  config file of the Reticulum instance rettui uses.
  - **Options:** every option rsReticulum reads, with a description and its
    default: the `[reticulum]`, `[logging]`, `[stamps]` and `[filter]` sections, and each interface's
    own options by type (Auto, TCP, Backbone, UDP, I2P, RNode, serial, KISS,
    AX.25, pipe), plus IFAC, announce, discovery and ingress control. Keys
    rettui doesn't know are listed and kept as written.
  - **IFAC size is in bits:** rsReticulum 1.5 reads `ifac_size` in bits, as
    Python Reticulum does (128 is 16 bytes); before 1.5 it was bytes. The
    first time rettui 2.0 starts it changes each interface's old size to bits
    and adds `ifac_size_unit = bits` beside it (Python ignores that line), and
    keeps the old file as `config.backup`. A size that already has
    `ifac_size_unit` is left alone.
  - **What runs programs:** a pipe interface's command, an interface's
    `location_cmd`, and a `reachable_on` (or `discovery_reachable_on`) that
    isn't a host name or IP address (Python Reticulum runs a path there to
    learn the address) run programs, so the web UI can't set or turn them on.
  - **Entry point:** *Transport node* on its own opens no port, so nobody
    can connect to the node. *Entry point* (the `E` key in the terminal UI)
    turns Transport node on and adds a TCP server interface listening on
    `0.0.0.0` port 4242, then asks you to restart Reticulum. Give it your
    public host name and it also announces itself, so others can find it by
    interface discovery. Clients connect with a `TCPClientInterface` to that
    host and port; if you set an IFAC name or passphrase on the interface,
    they need the same. When Transport node is on and no interface lets
    others connect, the editors and the log say so. In Docker, publish the
    port too (see [Docker](Docker)). The interface says 10 Mbps: that is
    Reticulum's guess for a TCP interface, not a limit.
  - **Interfaces:** add (pick a name and a type), rename, enable or disable,
    and delete them. Reticulum starts an interface only when it says
    `enabled = Yes` (or `interface_enabled = Yes`), as Python Reticulum
    does; one with neither is off, and rettui says so when Reticulum
    starts, lists it as off on the Status page, and says so in its log and
    among the config's warnings. Interfaces rettui adds always say which.
  - **Interface discovery:** with *Discover interfaces* in `[reticulum]`,
    Reticulum finds entry points that others announce as discoverable. It
    hears of them through the connections you already have, so it needs
    one to start. Something then has to connect to them:
    - **rettui** (what the getting-started guide sets up): *Discovered
      entry points* in rettui's settings (System) says how many, with
      *Auto-connect* (`autoconnect_discovered_interfaces`) left at 0.
      rettui connects to the best ones found (run by transport nodes, heard
      in the last 3 days; those known longest first), drops one that isn't
      up within 12 seconds and tries another (one dropped is tried again
      after 5 minutes). Your own interfaces stay connected, whether marked
      *Bootstrap only* or not: anyone can announce an entry point, so those
      found are added to yours and don't replace them. A name announced is
      resolved first, and not connected to if any address it gives is this
      device's. With *Disconnect Bootstrap only interfaces* (System, off by
      default, from the next start) rettui handles interfaces marked
      *Bootstrap only* as Python Reticulum does: they're disconnected once
      that many found entry points are up, and connected again as soon as
      none is. A TCP client interface can be brought back; another kind
      marked *Bootstrap only* stays connected. Turn it on only if you want
      entry points found to take the place of those you chose.
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
    found, drops one, or (with *Disconnect Bootstrap only interfaces* on)
    disconnects or reconnects the *Bootstrap only* ones. Your own interfaces
    marked *Discoverable* are announced too.
  - **As text:** edit the whole file, with a live check of whether Reticulum
    can load it.
  - **Safe edits:** changes are made in place, so comments and layout stay.
    Every change is checked with rsReticulum's own parser before saving, and a
    file Reticulum couldn't load is never saved. The previous version is kept
    as `config.backup`, readable by you only (a config can hold IFAC
    passphrases), as is a config rettui makes. Interfaces that will fail to start (a missing port,
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
    or change a pipe interface's command, or turn one on, can't be put back
    (the terminal can).
  - **Interface profiles:** named sets of interfaces to switch between,
    as in NomadNet 1.4: at home with the Auto interface, say, and out with
    a radio and an entry point. `P` in the Reticulum tab (**⇄ Profiles** in
    the web UI) lists them, the one the file is as now marked ●. *Keep the
    interfaces as they are now* saves which interfaces are on and off under
    a name (one of that name is replaced). Switching to one (`Enter`, or
    **Switch**) turns its interfaces on and off as it has them, leaves
    interfaces added since as they are, and restarts Reticulum so they
    apply. It's an edit like any other, kept in the history as "Switched to
    profile Home: Hill off". `x` (**Delete**) deletes a profile; the
    interfaces stay as they are. Profiles are kept in
    `interface-profiles.json` in rettui's [data folder](Data-and-Storage),
    shared by every identity. In the web UI, a profile that would turn on a
    pipe interface can't be switched to (the terminal can).
  - **Applying:** Reticulum reads the file when it starts. Restart the
    Reticulum stack from inside rettui to apply changes (see below). If
    another program such as rnsd runs the shared instance, the interfaces are
    that program's, and they change when it restarts; rettui says so.
  - **Web UI:** the same editor, except that pipe interface commands (which
    run programs) can only be added, changed or turned on from the terminal,
    and a pipe interface can't be copied there. Turning one off is fine.
- **Restarting Reticulum:** `Ctrl-R` in the Status or Reticulum tab (after a
  confirmation), or **Restart Reticulum** in the web UI, stops rettui's
  Reticulum stack and starts it again with the same identity, without
  quitting. Use it to apply config changes or to recover from a network
  problem. Links and transfers in progress stop (unsent messages are marked
  failed, to send again), hubs that were connected reconnect, and your node
  starts again.
- **Line wrapping:** the "Wrap editor lines" setting makes long lines in the
  page and config editors wrap instead of scrolling sideways, in both UIs.
