The Network tab lists what rettui hears on the network; the Status tab
shows your identity, your interfaces and the log.

- **Heard announces:** the Network tab lists the LXMF peers, NomadNet nodes
  and propagation nodes you've heard, with their names, addresses, hops and
  when they were last heard.
- **Hearing others takes time:** there's no list to download; each peer
  and node announces on its own schedule, many only every few hours, so a
  new install's list fills over its first hours. Announcing yourself (`A`,
  or **Announce** in the web UI) lets others find you. While nothing is
  connected, the empty list says so and points to the getting-started
  guide.
- **Finding:** filter by kind (`f`), or search by name or address (`/`),
  with the matches highlighted. Addresses can be pasted in any common form
  (`<hash>`, `lxmf@hash`, `hash:/page/index.mu`).
- **Acting on a row:** message a peer, browse a node, or use a propagation
  node for sync, and copy any address.
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
  no I2P router running, and an interface that has stopped retrying.
  The command-line commands print the same lines.
- **Traffic:** the corner of the sidebar, under the interface count, shows how
  fast data is coming in (↓) and going out (↑) over all interfaces, updated
  every few seconds. In the web UI, hovering over it shows the totals.
