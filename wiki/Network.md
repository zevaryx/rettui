The Network tab lists what rettui hears on the network; the Status tab
shows your identity, your interfaces and the log.

- **Heard announces:** the Network tab lists the LXMF peers, NomadNet nodes
  and propagation nodes you've heard, with their names, addresses, hops and
  when they were last heard.
- **Names are cleaned:** a name someone announces could otherwise pass for
  another, or spill over the screen. As NomadNet does, rettui drops
  characters that hide or turn text around (direction overrides,
  zero-width and private-use ones), reads compatibility forms as the
  letters they look like (`ＡＤＭＩＮ` as `ADMIN`), keeps at most four
  accents on a letter, makes blank-looking characters and line breaks one
  space, and keeps 128 characters. Unlike NomadNet, emoji stay. Names
  heard before this are cleaned as rettui starts. Your own name, and your
  propagation node's, go out the same way, emoji included, as Sideband
  and NomadNet send them (the LXMF library rettui uses would drop the
  emoji, so rettui builds its announces itself).
- **Hearing others takes time:** there's no list to download; each peer
  and node announces on its own schedule, many only every few hours, so a
  new install's list fills over its first hours. Announcing yourself (`A`,
  or **Announce** in the web UI) lets others find you. While nothing is
  connected, the empty list says so and points to the getting-started
  guide.
- **Finding:** filter by kind (`f`), or search by name or address (`/`),
  with the matches highlighted. Addresses can be pasted in any common form
  (`<hash>`, `lxmf@hash`, `hash:/page/index.mu`).
- **Sorting and interfaces:** the list is last heard first; `s` in the TUI
  (or the order menu in the web UI) sorts it by name or nearest first
  instead. Each row says which interface its path goes through, when one's
  known (from the path table, read every half minute), and `i` (or the
  interface menu) keeps the list to those through one interface: what's
  heard over the LoRa radio, say, rather than the internet.
- **Acting on a row:** message a peer, browse a node, or use a propagation
  node for sync, and copy any address.
- **Announces as they're heard:** `a` (or **Announces** in the web UI)
  shows every announce in place of the list, as it arrives, of every kind:
  LXMF peers, NomadNet and propagation nodes, RRC hubs, LXST calls, and
  whatever else announces on the network (tagged OTHER, with a name when
  its announce carries one as text). The list keeps one row per peer or
  node; this shows each announce, with when it was heard, so it's where to
  see who's announcing how often, or what's on a new interface. It follows
  the newest; moving up holds it where you are (`End`, or scrolling to the
  bottom in the web UI, follows again). Filter by kind (`f`), search
  (`/`) and keep to an interface (`i`) as in the list; `Enter` opens what
  rettui can, and `P`, `T` and `y` find a path, probe and copy. rettui
  keeps the newest 1000 heard since it started. A burst of announces on
  an interface that just came up is held back by Reticulum (as it is for
  every program on it), so some may show up later, or not at all.
- **Paths:** to reach a destination, rettui needs a path to it: which
  interface it goes out on, and through which transport node. Announces
  bring paths; for one not heard yet, rettui asks for a path, up to three
  times over a minute (at once, then after 15 and 52 seconds). One request
  often goes unanswered: it's lost on the way, or reaches a node that
  hasn't heard of the destination yet. For 45 seconds, Reticulum nodes take
  further requests for a destination as part of the one they're still
  working on, so the third request waits until then. Loading a page,
  connecting to a hub, and sending a message, syncing or pinging all do
  this; while it goes on, a hub connecting, and a page loading in the TUI,
  say which request they're on.
- **Stale paths:** a path can go stale: the destination moved, or a node on
  the way went down. When a Link over a known path gets no answer, rettui
  forgets the path, asks for a fresh one, and tries once more (except for
  a ping, which times one try).
- **Finding or forgetting a path:** in the TUI, `P` finds the path to the
  selected row (or to any address typed), and `D` forgets the selected
  row's path. In the web UI, **Path** on each row opens a dialog with the
  path and **Find path**, **Probe** and **Forget path** buttons, and **Find
  a path…** above the list opens it for any address. A path shows as its
  hops, the transport node it goes through, the interface, and how long
  it's kept unless heard again ("2 hops via <0a1b2c3d…> on RMAP World,
  kept for 6 days"). Forget a path that has gone stale, and the next use
  asks for a fresh one.
- **Probing:** `T` (or **Probe**) times an answer from the selected
  destination, as `rnprobe` does, finding a path first. LXMF addresses
  prove the packets they get, so they're sent a probe packet ("answered in
  182 ms, 2 hops away"). NomadNet nodes, propagation nodes and RRC hubs
  don't, so `rnprobe` to one always times out; rettui times setting up a
  Link to them instead, closed straight away, and says so ("a Link opened
  in 240 ms").
- **From the command line:** `rettui path <address>` finds and prints a
  path, `-d` forgets it, and `-t` lists every path known; `rettui probe
  <address>` probes (see [Running](Running#commands)). While rettui is the
  shared instance (the first Reticulum program started with
  `share_instance` on), these use its paths, and Reticulum's own `rnpath`
  and `rnprobe` work against it too.
- **Status:** the Status tab shows your identity and LXMF address, your
  first steps until they're taken (see
  [Running](Running#first-steps)), the network state, each interface with its traffic, the last sync, the
  propagation node you host (if any), and a log.
- **Interface trouble:** the log shows why an interface won't connect or
  dropped, as the Reticulum libraries report it (for example
  `Interface Dead Link: TCP connect failed: Connection refused`). It also
  shows when an interface goes offline or comes back online. The same
  trouble again within five minutes (a connection retrying) is counted
  rather than logged again, and a Reticulum restart starts the count
  afresh. `rettui.log` keeps everything.
- **What to do about it:** common trouble gets a plain-words hint after
  the error: a host name that can't be looked up (check the internet
  connection and the host name), a connection refused (the entry point may
  be down or the port wrong), no answer (down, or a firewall), no route (no
  internet), a port already in use (another Reticulum, such as rnsd, or a
  second rettui), a radio that isn't plugged in, a serial port you may not
  open (on Linux, the `dialout` group), a device another program has open,
  no I2P router running, no IPv6 for the Auto interface (common in Docker
  containers), and an interface that has stopped retrying.
  The command-line commands print the same lines.
- **Interfaces:** the Status tab lists each one, online (●) or not (○),
  with what it has received (↓) and sent (↑), and what rnstatus shows of
  it: its rate (for a radio, the air rate), MTU, mode if not *full*,
  clients (for a server), and announces queued or held back and packets
  dropped, when there are any.
- **Signal:** a [ping](Messaging#contacts) says how well the answer was
  heard (RSSI, SNR and link quality, as rnprobe does) when it came in over
  a radio that reports it, such as an RNode. Received messages don't show
  it: the Reticulum library rettui uses doesn't pass those readings on for
  them.
- **Traffic:** the corner of the sidebar, under the interface count, shows how
  fast data is coming in (↓) and going out (↑) over all interfaces, updated
  every few seconds. In the web UI, hovering over it shows the totals.
