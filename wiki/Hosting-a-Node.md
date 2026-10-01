rettui can host two kinds of node with your identity: a NomadNet node
(pages and files), and an LXMF propagation node (messages kept for people
who are offline).

## NomadNet node

rettui hosts a NomadNet node using rsNomad's node. Pages are Micron
files in `node/pages/` (and downloads in `node/files/`) in the data directory,
or the folder set in `node_dir`.

- **Switching it on:** press `h` in the Node tab, use **Start hosting** in
  the web UI, or set "Host a node" in the settings. The node uses your
  identity, so its address stays the same. Its name defaults to your display
  name, and it announces every `node_announce_interval_mins` (360 by default)
  or when you press `a`.
- **Editing:** the Node tab lists the pages and has an editor with Micron
  colouring, undo and redo, and a live preview, in both the TUI and the web
  UI. Show the editor and the preview side by side, the editor alone, or the
  preview alone (`Ctrl-P` / `p`, or the buttons in the web UI). Saving writes
  the file, and visitors get the new version straight away. New, renamed and
  deleted pages are picked up without a restart. You can also edit the files
  with any editor.
- **Formatting ribbon:** above the page editor, in both UIs, buttons put
  Micron markup around the selection, on the selected lines, or at the
  cursor: bold, italic, underline and normal (removes formatting), text and
  background colour, left/centre/right alignment, three heading levels,
  divider, literal block, comment, and inserting links, images, text fields,
  checkboxes and radio buttons. Each has an Alt shortcut, the letter
  underlined on its button: `Alt+B` `I` `U` `N`, `F` `G` (colours), `L` `C`
  `R`, `1` `2` `3`, `V` (divider) `T` (literal) `O` (comment), `K` (link) `M`
  (image) `D` (field) `H` (checkbox) `A` (radio). Bold, italic and underline
  toggle, as do headings and comments. The web UI also takes `Ctrl+B`, `I`,
  `U` and `K`, and picks colours from a palette.
- **Selecting text in the editors:** Shift with the arrows, Home/End or
  PgUp/PgDn, `Ctrl-A` for everything, or drag with the mouse. Typing, pasting
  or deleting replaces the selection.
- **Starter page:** a new node gets an index page with a short Micron guide.
- **Your own node in the Browser:** a client can't reach a node it hosts over
  Reticulum, so rettui reads your own pages straight from the folder (never
  from the cache).
- **Scripts:** executable pages run as programs for visitors only when "Run
  page scripts" is on. The web UI can't change, rename or create scripts,
  turn "Run page scripts" on or move the node folder, so the web login can't
  be used to run programs on the host. The TUI can do all of these.

## Propagation node

A propagation node keeps messages for people who aren't online, until they
collect them, as `lxmd` and other LXMF propagation nodes do. rettui's is
built on rsLXMF's propagation node (its message store, stamp checks and
peer protocol).

- **Switching it on:** set "Host a propagation node" in the settings (Status
  tab, either UI). It announces itself (`lxmf.propagation`) a few seconds
  after it starts, then every `announce_interval_mins`, with its name
  (`pn_name`, or your display name), stamp cost and transfer limits. Its
  address comes from your identity, so it stays the same. Changing one of
  its settings restarts it; the messages it holds stay.
- **Status:** the Status tab's *Hosting messages* line shows its address,
  how many messages it holds and their size, how many it has taken in and
  given out, those it refused, and the peers connected to it now. It's also
  listed in the Network tab, where you can pick it as your own propagation
  node.
- **What it takes:** each message sent through it brings a propagation stamp
  of at least `pn_stamp_cost` (16 by default, LXMF's own; at least 13) less
  LXMF's flexibility of 3, and is at most `pn_transfer_kb` (256 KB by
  default). Clients send one message at a time; only peers send several.
  Messages without a valid stamp, too big, or over the storage limit are
  refused.
- **Storage:** messages are kept encrypted for their recipients in
  `propagation/` in the data directory, at most `pn_storage_mb` (500 MB by
  default). LXMF's 30-day expiry applies, and past the limit the oldest go
  first.
- **Collecting:** a client identifies itself on its Link and asks for its
  messages (`/get`); the node sends them, then deletes the ones it got.
- **Messages for you:** when the node takes a message addressed to you, it
  delivers it straight to your conversations instead of keeping it. With
  your own node picked as your propagation node, rettui hands messages to it
  directly (a client can't open a Link to itself), and a sync has nothing to
  download.
- **Peers:** other propagation nodes that peer with yours (proving the work
  of LXMF's peering key, cost 18) offer it the messages it doesn't have, and
  send them several at a time. Yours doesn't peer with other nodes itself,
  as `lxmd` with `autopeer` off: messages sent through it stay here until
  their recipients collect them, and other nodes only get them by peering
  with it. Tested against Python LXMF 1.2: its clients send through and
  collect from rettui's node, and its propagation nodes peer with it.
