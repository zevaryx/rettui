# rettui

A [Reticulum](https://reticulum.network/) client written in Rust, for the terminal
and the browser. It does LXMF messaging, RRC (Reticulum Relay Chat), NomadNet
browsing, and hosting your own NomadNet node, and it can edit your Reticulum
configuration. It is built on:

- [rsReticulum](https://github.com/ratspeak/rsReticulum): the Reticulum protocol
- [rsLXMF](https://github.com/ratspeak/rsLXMF): LXMF messages
- [rsNomad](https://github.com/Colorado-Mesh/rsNomad): NomadNet requests and hosting
- [ratatui](https://github.com/ratatui/ratatui): the terminal UI

> [!WARNING]
> **This project is an experiment in AI-assisted development.** I am not great
> at writing user interfaces, and focus mostly on backend code. This project is
> me experimenting at offloading frontend work to AI assistance using known-good
> Rust-based Reticulum stacks, which will be similar to future projects that I am
> working on, where the backend is written by me. This means most of rettui's code
> was written by an AI coding assistant (Anthropic's Claude), with a human
> directing the work and checking the results. It has been tested, including
> end to end on a private Reticulum network and I personally use it for most of my
> Reticulum usage, but it has not had an independent code review or a security audit.
> Expect bugs. Don't rely on it for anything where privacy, security or delivery really
> matters, and check the code yourself before trusting it. The same applies to any fork
> or copy of it.

## Contents

- [Features](#features)
- [Install](#install)
- [Build](#build)
- [Run](#run)
- [Web UI](#web-ui)
- [Docker](#docker)
- [Using the TUI](#using-the-tui)
- [Notes](#notes)
- [Code layout](#code-layout)
- [Releases and CI](#releases-and-ci)
- [License](#license)

## Features

### Two interfaces

- **Terminal UI (TUI):** seven tabs (Messages, Channels, Network, Browser,
  Node, Status, Reticulum) with keyboard and mouse control. Terminals
  narrower than 110 columns get a sidebar of icons and narrower side lists, so
  80×24 works well.
- **Web UI:** `rettui --web` runs the same client in a browser, with the same
  sections and features. It updates live, switches sections instantly (each
  keeps its content and refreshes behind the scenes, even over a slow link),
  works on phones, uses a bundled Fira Code Nerd Font, and is protected by a
  login link (see [Web UI](#web-ui)).
- **Docker:** a Dockerfile and a Compose file run the web UI in a container
  (see [Docker](#docker)).
- **Command line:** send a message, listen for messages, sync, fetch a page,
  or print your address without starting a UI (see [Run](#run)).

### Messaging (LXMF)

- **Conversations:** a conversation per peer, with unread counts in the
  list and the sidebar. Start one by address (`n`), from the Network tab, or
  from an `lxmf@` link on a page.
- **Message states:** your messages show *sending…*, ✓ (delivered), *✓ via
  propagation node*, or *failed* with the reason.
- **Delivery modes:**
  - **Auto** (default): tries direct delivery first. If that fails, it hands
    the message to your propagation node.
  - **Direct:** delivers over a Link and waits for the recipient's proof.
  - **Propagated:** always goes through the propagation node.
- **Stamps and ratchets:** if the recipient's announce asks for a stamp,
  rettui generates it. It also generates the propagation node's stamp, and
  encrypts to the recipient's ratchet when one is known.
- **Propagation nodes:** the ones you hear are listed in the Network tab.
  Pick one with `p`. rettui syncs every `sync_interval_mins` (30 by default)
  or whenever you press `S`. A sync identifies you to the node, downloads your
  messages, then tells the node to delete them.
- **Attachments:**
  - The first image goes in the LXMF image field, which Sideband and MeshChat
    show inline. Other files are sent as file attachments.
  - Received attachments are saved automatically, and images get an inline
    preview. `o` opens the newest attachment with your desktop's default app.
  - Many clients refuse direct transfers over about 1 MB, and rettui warns
    before sending that much.
- **Signatures:** incoming messages are checked against the sender's key. If
  the key isn't known, rettui looks it up first (up to 10 s). Messages that
  still can't be checked are marked *unverified*.
- **Peers who are offline:** public keys from announces are kept, so you can
  write to a peer you heard earlier even while they're away.
- **Announces:** rettui announces your LXMF address when it starts (unless
  you turn that off), every `announce_interval_mins` (360 by default, 0 for
  never), and whenever you press `A`.

### Chat (RRC)

RRC is the IRC-like chat protocol that NomadNet 1.4 also speaks, served by
hubs such as [rrcd](https://github.com/kc1awv/rrcd). It lives in the Channels
tab.

- **Hubs:** add a hub by address (`n`), or open an `rrc://hash/room` link on a
  NomadNet page. Opening a new hub from a link asks first, because connecting
  tells the hub who you are.
- **Views:** the hub view shows its status, limits, MOTD, and public rooms,
  which you can click to join. Rooms show the topic, members (on wide
  screens), `/me` actions and notices, with times to the second. Your own
  messages show `…` until the hub echoes them back.
- **Whispers:** private notices ("whispers") with another user have their own
  conversation under the hub, marked `@` where rooms have `#`, instead of
  appearing in rooms. Writing there whispers to that user; incoming whispers
  count as unread there and stand out like mentions. When two users share a
  nick, the list adds the start of their identity. `x` (or **Close** in the
  web UI) closes a conversation. Whispers saved in rooms by older versions
  are moved to their conversations.
- **Commands:** the same set as NomadNet: `/join`, `/part`, `/me`, `/nick`
  (per hub, defaulting to your display name), `/who`, `/list`, `/topic`,
  `/ping`, `/clear`, `/connect`, `/disconnect`, and hub moderation commands
  such as `/mode`, `/kick` and `/op`. `/msg <nick> [text]` (also `/w`)
  whispers on hubs that support it (with no text, it opens the
  conversation), and `/dm <nick> [text]` sends an LXMF message instead.
  `/help` lists them all.
- **Messaging a user:** click a name in the chat or the members list (or press
  `m`) for a menu: open your whisper conversation, send an LXMF message, or copy
  their LXMF address or identity. Their LXMF address comes from the identity
  the hub shows, and the menu says whether they have announced it. A message
  to an address that was never announced waits until a path is found.
- **Mentioning someone:** typing `@` in a room lists who is there, plus
  anyone who has spoken there. The list narrows as you type: names starting
  with what you typed come first, then names containing it. Up and Down
  choose, Tab or Enter puts in `@name`, a click does too, and Esc closes the
  list. A mention of someone else shows in the same colour as their name
  in the chat.
- **Mentions and unread:** `@yournick` mentions are highlighted (just the
  mention, not the whole message). Unread counts show on each room and in the
  sidebar, and mentions stand out from ordinary unread messages.
- **History:** each hub's messages are kept on disk between runs.
- **Long messages:** messages over the hub's size limit are offered as a
  split into several messages.
- **Connections:** hubs reconnect automatically with backoff (toggle with
  `a`) and rejoin your rooms quietly. Quitting leaves hubs properly, so others
  see you go at once.

### NomadNet browsing

- **Finding nodes:** the Browser tab has a pane with two lists: Saved (pages
  you saved with `s`) and Nodes (every NomadNet node heard). Pages open beside
  the pane. You can also go to any address (`g`), or to your home page (`H`,
  set in the settings).
- **Micron:** most of it is supported, including 24-bit colour, alignment,
  dividers, literal blocks, links, forms (text fields, checkboxes and radio
  buttons), inline images and page colours. Long lines wrap between words.
- **Links:** page links, `lxmf@` links (open a conversation), `rrc://` links
  (open a hub room), and `/file/` downloads, which are saved to `downloads/`.
- **Identifying:** pages that personalise content need you to identify.
  Toggle it per node with `I`.
- **View source:** `u` (or the button in the page's title bar) shows the
  page's Micron with line numbers and the markup coloured. Refreshing keeps
  the source view, and `Y` copies the source exactly as the node sent it.
- **Copying:** drag across page text to copy it, `Y` copies the whole page,
  and `L` copies the selected link.
- **History:** `b` (or a right-click) goes back.
- **Cache:** pages and images are cached on disk for `cache_hours` (24 by
  default). A page's `#!c=` directive can shorten that, and `#!c=0` pages are
  never cached. Form submissions and file downloads always go to the network.
  - The address bar shows when a page came from the cache.
  - `r` refetches the page and its images. The cached copy is only replaced
    once the new one arrives, so it still works while a node is unreachable.
  - `R` clears the whole cache.
- **Images:**
  - Page images and image attachments are drawn with
    [ratatui-image](https://crates.io/crates/ratatui-image) in the best
    protocol the terminal has: Kitty graphics (Kitty, Ghostty and others),
    Sixel, iTerm2 inline images (iTerm2, WezTerm), or half blocks everywhere
    else. They scroll and clip like text, and are decoded in the background so
    the UI never waits for them.
  - **Windows Terminal** (detected by `WT_SESSION`, which is also set inside
    WSL) always gets Sixel. It may not report its cell size, which Sixel needs
    to size pictures: if they come out too big or small, set
    `RETTUI_CELL_SIZE` to your font's cell size in pixels, for example
    `RETTUI_CELL_SIZE=9x19`.
  - rettui asks the terminal what it supports at startup with a short,
    bounded query (at most 1.5 s, and only if the terminal doesn't answer).
    Konsole's Sixel and Kitty support and WezTerm's Kitty support are skipped,
    as ratatui-image recommends.
  - `RETTUI_GRAPHICS` forces a protocol: `kitty`, `sixel`, `iterm2` or
    `halfblocks`.

### Hosting a NomadNet node

rettui can host your own NomadNet node, using rsNomad's node. Pages are Micron
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
  page scripts" is on. The web UI can't change, rename or create scripts, so
  the web login can't be used to run programs on the host. The TUI can edit
  them.

### Network

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

### Settings and the Reticulum config

- **Settings editor:** the Status tab (in both UIs) edits `settings.json`:
  display name, announces, home page, propagation node, sync interval, cache
  time, node hosting, editor line wrapping and the Reticulum config directory.
  Values are checked before saving, and most changes apply straight away (the
  editor says which ones wait for the next start).
- **Reticulum config editor:** the Reticulum tab (in both UIs) edits the
  config file of the Reticulum instance rettui uses.
  - **Options:** every option rsReticulum reads, with a description and its
    default: the `[reticulum]` and `[logging]` sections, and each interface's
    own options by type (Auto, TCP, Backbone, UDP, I2P, RNode, serial, KISS,
    AX.25, pipe), plus IFAC, announce, discovery and ingress control. Keys
    rettui doesn't know are listed and kept as written.
  - **Interfaces:** add (pick a name and a type), rename, enable or disable,
    and delete them.
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

## Install

Each [release](https://github.com/zevaryx/rettui/releases) has a
ready-built `rettui` for:

| System | Archive |
| --- | --- |
| Linux, x86-64 | `rettui-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Linux, ARM64 | `rettui-<version>-aarch64-unknown-linux-gnu.tar.gz` |
| Windows, x86-64 | `rettui-<version>-x86_64-pc-windows-msvc.zip` |
| macOS, Apple silicon | `rettui-<version>-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `rettui-<version>-x86_64-apple-darwin.tar.gz` |

Unpack it and put `rettui` somewhere on your `PATH`. `SHA256SUMS` has
the checksums.

- **Linux:** the binaries need glibc 2.35 or newer (Ubuntu 22.04,
  Debian 12 or later).
- **macOS:** the binaries aren't notarised. If macOS won't open one,
  run `xattr -d com.apple.quarantine rettui`.
- **Docker:** see [Docker](#docker) for the image.

## Build

```sh
git clone --recursive <this repo>   # or: git submodule update --init
cargo build --release
```

The three libraries are git submodules under `deps/`. Each one expects the others
to be checked out next to it (for example `../rsReticulum`). They are pinned to
revisions that build together:

| Submodule | Revision | Why |
| --- | --- | --- |
| rsReticulum | `e16bd15` | The revision rsNomad's CI pins. It adds `RequestOutcome::ReplyFile` and is not on upstream `main`. |
| rsLXMF | `d7da7f3` | The last rsLXMF built against rsReticulum 1.2 at that point. Later commits need rsReticulum 1.3. |
| rsNomad | `05ad8ec` | `main` |

Move these forward together, once rsNomad supports rsReticulum 1.3.

## Run

```sh
rettui                      # the TUI
rettui --web [ADDRESS]      # the web UI (default 127.0.0.1:8740)
rettui fetch <hash>[:/page/x.mu] [--raw] [--identify] [-o FILE]
rettui send <address> "text" [-a FILE]... [--mode auto|direct|propagated]
rettui listen [--seconds N]  # announce, then print incoming messages
rettui sync [--node HASH]    # download messages from the propagation node
rettui address               # print your LXMF address
```

`--data-dir DIR` uses another data directory, and `--rns-config DIR` another
Reticulum config.

### Reticulum config

By default, rettui uses the first Reticulum config it finds in `/etc/reticulum`,
`~/.config/reticulum`, or `~/.reticulum`. If a shared instance is already running
from that config (rnsd, NomadNet, Sideband), rettui joins it. Use `--rns-config DIR`
to choose a different config. The Reticulum tab edits that config file.

### Data

rettui keeps its data in `~/.local/share/rettui/`:

- `identity`
- `settings.json`: display name, announce behaviour, home page, propagation
  node, sync interval, `cache_hours` (24 by default), node hosting,
  `wrap_lines` and the Reticulum config directory. Edit it in the Status tab
  of either UI (or by hand). Changes apply straight away, except the
  Reticulum config and "announce at start", which are used the next time
  rettui starts.
- `store.json.gz`: peers, conversations, saved pages, RRC hubs, and nodes you
  identify to, as gzip-compressed JSON (read it with
  `zcat store.json.gz | jq`). Earlier versions kept it as plain `store.json`,
  about five times larger. The first launch after upgrading converts it,
  checks that the new file reads back the same, and only then removes the
  old one. Changes are saved every 10 seconds on a background thread, so a
  large store doesn't pause the UI, and once more when rettui exits.
- `known_identities.json`: public keys from LXMF and propagation-node
  announces, so you can reach peers that are offline now. Saved every few
  seconds and when rettui exits.
- `cache/`: cached NomadNet pages and images
- `rrc/`: RRC chat history, one file per hub
- `rettui.log`: logging, set with `RETTUI_LOG=debug`
- `downloads/`: received attachments (one folder per sender) and NomadNet files
- `uploads/`: files attached to messages sent from the web UI
- `node/`: your node's `pages/` and `files/`
- `web_token`: the web UI's login secret (delete it to log every browser out)

## Web UI

`rettui --web` runs the same client in a browser instead of the terminal. All
seven sections work as they do in the TUI and update live as messages and
announces arrive. It prints a link to open:

```text
rettui web UI: http://127.0.0.1:8740/?token=…
```

- **Logging in:** the link logs that browser in with a cookie, and stays valid
  across restarts. Keep it private. Anyone with the link can read and send your
  messages as you.
- **Scripts and proxies:** send the token in a header instead of using the
  link: `Authorization: Bearer <token>`, or `X-Rettui-Token: <token>` if
  `Authorization` is already taken (for example by a proxy's own login). The
  token is in `web_token` in the data directory, and the JSON API is under
  `/api/`:

  ```sh
  curl -H "Authorization: Bearer $(cat ~/.local/share/rettui/web_token)" \
       http://127.0.0.1:8740/api/state
  ```

- **Address:** it listens on 127.0.0.1 by default. Pass an address such as
  `--web 0.0.0.0:8740` to reach it from other devices, and prefer a VPN or SSH
  tunnel over exposing it. It uses plain HTTP.
- **Browsing:** NomadNet pages are rendered to HTML by rettui, with links and
  forms handled by the web UI and no scripts allowed. `/file/` links download
  through the browser.
- **Keys:** `1`–`7` switch sections, `/` searches the Network list, and `Esc`
  leaves a text box.
- **Phones:** on narrow screens the sections stack in one column (lists above
  what they open).
- **Font:** all text uses Fira Code Nerd Font, which rettui serves itself
  (browsers never fetch fonts from a third party).
- **Stopping:** Ctrl-C or SIGTERM stops it cleanly, leaving hubs and saving
  everything.
- The TUI and the web UI can't use the same data directory at the same time.

## Docker

The web UI can run in a container. Each release publishes an image for
x86-64 and ARM64 to `ghcr.io/zevaryx/rettui`, tagged with the version (`1.2.0`,
`1.2`, `1`) and `latest`. To use it, remove `build: .` from
[compose.yaml](compose.yaml) and set `image: ghcr.io/zevaryx/rettui:latest`,
then `docker compose up -d`.

To build the image yourself instead, from a checkout with its submodules:

```sh
git submodule update --init
docker compose up -d --build
docker compose logs rettui   # the login link (http://127.0.0.1:8740/?token=…)
```

- **Data:** [compose.yaml](compose.yaml) keeps everything in `./data`
  next to it: the identity, settings, messages, cache, and the Reticulum
  config in `./data/reticulum/config`. rettui writes that config the first
  time it starts.
- **Owner of the files:** `PUID` and `PGID` (default 1000) set the user and
  group rettui runs as, so the files in `./data` belong to you. Use your
  `id -u` and `id -g`. `GUID` also works in place of `PGID`.
- **Network:** Reticulum's default AutoInterface finds peers on the local
  network by multicast, which doesn't leave Docker's default network. Either
  add an interface to `./data/reticulum/config` (for example a
  `TCPClientInterface` to a transport node, which the Reticulum section of the
  web UI can do) and restart Reticulum, or use `network_mode: host` (commented out in
  the compose file) instead of `ports:`.
- **Access:** port 8740 is published on every interface of the host, and the
  login link is still required. Change it to `"127.0.0.1:8740:8740"` to allow
  only this machine. `PORT` changes the port inside the container.
- `docker compose down` stops rettui cleanly; it saves on SIGTERM.

## Using the TUI

### Keys

| Tab | Keys |
| --- | ---- |
| All tabs | `1`–`7` switch tab, `A` announce, `S` sync with the propagation node, `Ctrl-L` redraw the screen, `q` / `Ctrl-C` quit |
| Messages | `↑↓` pick a conversation, `Enter` write, `n` new conversation by address, `y` copy the peer's address, `a` attach a file, `o` open the newest attachment, `d` cycle delivery mode, `PgUp/PgDn` scroll |
| Writing a message | `Enter` send, `Esc` stop writing, `Ctrl-V` paste, `Ctrl-O` attach, `Ctrl-X` clear attachments, `Ctrl-P` cycle delivery mode |
| Channels | `↑↓` pick a hub, room or whisper conversation, `Enter` write (text or `/commands`), `n` add a hub, `c` connect or disconnect, `a` toggle auto-connect, `x` leave a room, close a whisper conversation, or remove a hub, `y` copy an `rrc://` link, `m` message a user in the room, `PgUp/PgDn` scroll |
| User menu (click a name, or `m`) | `w` open your whisper conversation, `l` LXMF message, `Enter` pick (also copy their LXMF address or identity), `Esc` close |
| Network | `↑↓` select, `y` copy the selected address, `Enter` message a peer, browse a node, or pick a propagation node; `p` use the selected propagation node; `f` filter; `/` search by name or address (`Enter` done, `Esc` clear) |
| Browser (both panes) | `←`/`→` move between the node pane and the page, `t` switch Saved/Nodes, `g` go to an address, `y` copy the current address, `s` save the current page, `b` back, `r` refresh from the network, `R` clear the whole cache, `I` identify to this node (toggle), `H` home, `u` view the page's Micron source (toggle), `Esc` cancel loading |
| Browser, node pane | `↑↓` select, `Enter` open beside the list, `x` remove a saved page |
| Browser, page | `Tab`/`Shift-Tab` move between links and fields, `Enter` follow a link or edit a field, `L` copy the selected link, `Y` copy the whole page (the raw source when viewing source), `Ctrl-V` paste into the selected field, `Esc` clear the selection or leave the source view, `↑↓` / `PgUp`/`PgDn` scroll |
| Node | `Enter` edit the selected page, `n` new page, `r` rename, `x` delete, `h` start or stop hosting, `a` announce the node, `b` open it in the Browser, `y` copy its address, `p` switch view (editor and preview, editor only, preview only) |
| Editing a page | `Ctrl-S` save (live on the node), `Esc` back to the pages, `Alt` + the underlined letter formats (see the ribbon), `Shift`+arrows select, `Ctrl-A` select all, `Ctrl-Z` / `Ctrl-Y` undo and redo, `Ctrl-P` switch view, `Ctrl-V` paste; with the preview alone, `↑↓` / `PgUp` / `PgDn` scroll it |
| Status | `↑↓` select a setting, `Enter` edit it (or toggle), `e` edit display name, `y` copy your LXMF address, `Ctrl-R` restart Reticulum |
| Reticulum | `Tab` sections / options, `↑↓` select, `Enter` edit (toggles flip, choices open a list), `d` back to the default, `a` add an interface, `Space` enable or disable it, `r` rename, `x` delete, `t` edit the file as text, `R` reload the file, `Ctrl-R` restart Reticulum |
| Reticulum as text | `Ctrl-S` save (refused while the file can't load), `Esc` close, `Ctrl-Z` / `Ctrl-Y` undo and redo, `Ctrl-V` paste |

The footer confirms what keys did: ✓ (green) when something is done, ! (yellow)
when it can't be done right now, and ✗ (red) when it failed. Problems stay up
a little longer.

### Mouse

- Click a section in the left sidebar to switch to it.
- Click a conversation to open it, the compose box to start writing, and an
  attachment to open it with your desktop's default app.
- In Channels, click a hub or room to open it, the input box to write, a
  public room in a hub's view to join it, and a name to message that user.
- In the Network tab, click a row to select it and double-click to open it.
  Click the search bar to start searching.
- In the browser, click a saved page or node on the left to open it beside
  the list. Click the Saved and Nodes headers to switch lists.
- On a page, click links and form fields, click the address bar to go to an
  address, and right-click to go back.
- Click **view source** in the page's title bar to see its Micron source, and
  **back to page** to return.
- Drag across page text to select it. It's copied when you release the
  button, and dragging past the top or bottom edge scrolls the page.
- In the Status and Reticulum tabs, click a row to select it and double-click
  to edit it. In the editors, click to place the cursor and drag to select;
  click a ribbon button to format.
- The scroll wheel scrolls whatever is under the pointer.
- Click the version beside the name (top left) to open rettui's project page
  in your browser. In the web UI it opens in a new tab.

### Copy and paste

- Copied text goes to the system clipboard (X11 or Wayland). It's also sent
  through the terminal's OSC 52 clipboard escape, so copying works over SSH
  in terminals that support it. Set `RETTUI_CLIPBOARD=osc52` to use only
  OSC 52.
- Your terminal's paste shortcut (for example `Ctrl-Shift-V`) and `Ctrl-V`
  both paste into whatever is being edited: a message, a prompt, the selected
  form field on a page, or an editor. Line breaks become spaces in the
  single-line inputs, and are kept in the editors.
- rettui captures the mouse, so the terminal's own selection usually needs a
  modifier. Most terminals use `Shift`+drag.

## Notes

### Announces on a shared instance

rsReticulum sends one announce when a destination registers with a shared
instance, so the local daemon learns the path. This happens even when
`announce_at_start` is false. The announce is a path response, so the Python
daemon doesn't rebroadcast it to the network, but apps using the same shared
instance will see it.

### Characters left on screen

Terminals differ on how wide they draw some emoji (such as ❤️ or joined
family emoji), and the TUI only sends the cells that change. Where a terminal
draws one wider or narrower than expected, stray characters could stay behind,
so rettui repaints the whole screen when you switch tabs or panes, and
`Ctrl-L` repaints it at any time. Its own icons avoid symbols that terminals
may draw as emoji.

### Older LXMF clients

Some clients announce their name in LXMF's original format (the name itself,
not msgpack). rsLXMF only reads the newer format, so rettui reads the older
one itself, as Python LXMF does.

## Code layout

The protocol modules each sit on one of the submodules. The UI modules are split
by tab, and `app/` and `ui/` mirror each other. The web UI drives the same `App`
state as the TUI.

| Module | What it does | Built on |
| --- | --- | --- |
| `net/` | Network actor, on its own threads so heavy traffic can't hold up either UI: runtime, announces, paths and keys, commands and events | rsReticulum |
| `lxmf/` | Sending, receiving and propagation node sync | rsLXMF |
| `nomad/` | Page and file fetching, Micron parsing and layout, page cache, hosting a node and its pages | rsNomad |
| `rrc/` | RRC wire format, hub replies, hub sessions | rsReticulum |
| `reticulum/` | Reticulum config file: the options it has, editing it in place, and checking it | rsReticulum |
| `app/` | Application state per tab (`messages`, `channels`, `network`, `browser`, `node`, `reticulum`), the page editor's formatting (`format`) and input routing | |
| `ui/` | Drawing per tab, plus the sidebar, footer and prompt (`chrome`) and the text editors | ratatui |
| `term/` | Text input, the editors' text area, selection, clipboard, images (Kitty, Sixel, iTerm2 or half blocks) | ratatui-image |
| `web/` | Web UI: HTTP API, login, live updates, and the page's HTML/CSS/JS, font and logo (`web/assets`) | axum |

`cli.rs` has the shell commands (`send`, `listen`, `sync`, `fetch`), and
`store.rs` and `config.rs` hold what is saved to disk, and `app/saver.rs`
writes the store and chat history in the background.

## Releases and CI

The workflows are in [.github/workflows](.github/workflows):

- **CI** ([ci.yml](.github/workflows/ci.yml)) runs on every pull request:
  - Clippy, with warnings as errors;
  - the tests on Linux, Windows and macOS.
- **Build** ([build.yml](.github/workflows/build.yml)) builds the binaries
  for every platform in the [Install](#install) table without making a
  release:
  - Run it from *Actions > Build > Run workflow*.
  - Download the archives from the run's *Artifacts*.
  - They're named after the version and commit, for example
    `rettui-v1.2.0-a4e545b-x86_64-unknown-linux-gnu.tar.gz`.
- **Release** ([release.yml](.github/workflows/release.yml)) runs when a
  tag starting with `v` is pushed. It:
  1. checks the tag matches the version in `Cargo.toml`;
  2. builds every platform with the Build workflow;
  3. pushes the Docker image to `ghcr.io`, using those Linux binaries
     (the Dockerfile's `prebuilt` stage);
  4. creates a GitHub release with the archives, `SHA256SUMS` and
     generated notes.

  A tag containing a hyphen (`v1.3.0-rc.1`) makes a prerelease. Its image
  gets only the version tag, not `latest`.

To release, bump `version` in `Cargo.toml`, commit, then:

```sh
git tag v1.2.1
git push origin v1.2.1
```

## License

AGPL-3.0-or-later, the same as the libraries it links.

The web UI bundles [Fira Code](https://github.com/tonsky/FiraCode) as patched by
[Nerd Fonts](https://github.com/ryanoasis/nerd-fonts), under the SIL Open Font
License 1.1 (see [src/web/assets/fonts](src/web/assets/fonts)).
