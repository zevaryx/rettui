The Network tab lists what rettui hears on the network; the Status tab
shows your identity, your interfaces and the log.

- **Heard announces:** the Network tab lists the LXMF peers, NomadNet nodes
  and propagation nodes you've heard, with their names, addresses, hops and
  when they were last heard.
- **Finding:** filter by kind (`f`), or search by name or address (`/`),
  with the matches highlighted. Addresses can be pasted in any common form
  (`<hash>`, `lxmf@hash`, `hash:/page/index.mu`).
- **Acting on a row:** message a peer, browse a node, or use a propagation
  node for sync, and copy any address.
- **Status:** the Status tab shows your identity and LXMF address, the
  network state, each interface with its traffic, the last sync, and a log.
- **Interface trouble:** the log shows why an interface won't connect or
  dropped, as the Reticulum libraries report it (for example
  `Interface Dead Link: TCP connect failed: Connection refused`). It also
  shows when an interface goes offline or comes back online. The same
  trouble again within five minutes (a connection retrying) is counted
  rather than logged again, and a Reticulum restart starts the count
  afresh. `rettui.log` keeps everything.
- **Traffic:** the corner of the sidebar, under the interface count, shows how
  fast data is coming in (↓) and going out (↑) over all interfaces, updated
  every few seconds. In the web UI, hovering over it shows the totals.
