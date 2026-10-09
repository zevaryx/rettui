# Changelog

What changed in each release of rettui. Releases before v1.6.0 are described on the [releases page](https://github.com/zevaryx/rettui/releases).

## v1.7.0

Changes since v1.6.1: rettui can host an RRC hub, built on [rsRRCD](https://github.com/reticulum-spb/rsRRCD)'s. Checked with RRC clients on Python Reticulum joining it over TCP, and with rettui's own.

### Added
- **Host an RRC hub** (a setting, or `e` in the new Hub tab): rsRRCD's hub, on rettui's own Reticulum instance, that NomadNet, MeshChatX, Ratspeak, rettui and the other RRC clients join as they join rrcd. It has an identity of its own, so people who join learn its address and not your LXMF one, and it announces itself with its name (yours, or "Hub name") and greets people with "Hub greeting". You're an operator of it, and your rettui joins it as soon as it's up. Changing its settings applies as it runs, without disconnecting anyone.
- **The Hub tab** (and the web UI's Hub section): who's connected, the rooms and the bans. Kick someone from a room, ban them from one or from the whole hub, make them an operator or voiced, or disconnect them; set a room's topic and modes, register it, or make a registered room; lift bans. A console runs hub commands as typed in a room (`/stats`, `/kline list`, `/reload`...), with the hub's answers beside it.
- "Anyone makes rooms": off, only you make rooms on your hub, and others join the ones there are.
- The Status tab says how the hub is doing, and backups keep its settings, its rooms and (with your identity) its identity.
- The Network tab's announces name RRC hubs that announce their name as rrcd and rsRRCD do.

### Changed
- Eight tabs: `1`–`8` switch them.

### Docs and development
- *Hosting a Node* has a section on hosting an RRC hub.
- rsRRCD and rsRRC are submodules in `deps/`.

## v1.6.1

Changes since v1.6.0: RRC works with every hub in use, not only rrcd. Checked against rrcd (the reference hub, with its default configuration), the Go hub (rrc-hub), rsRRCD and Ratspeak's hub, and NomadNet's client.

### Added
- Commands rettui doesn't have go to the hub as typed (such as the Go hub's `/history`, `/away` and `/seen`), and `/quote` sends any text to the hub (`/quote /help` for the hub's own help).
- A keyed room's key is kept, so the room is rejoined after reconnecting.

### Changed
- `/nick` in a channel no longer sends a new HELLO, which rrcd and the Go hub take as a new session, out of every room. The nick goes with your next message, as in NomadNet.

### Fixed
- `/who` and `/names` replies: the Go hub's `[away]` marks broke the member list, and of a reply Ratspeak splits over several notices, all but the first showed in the room. rsRRCD's full member identities are used when it sends them.
- On a hub that doesn't send member lists (rrcd's default), people with nicks were left out of the member list. They're listed by nick until their identity is learned.
- The Go hub's joins and leaves (the room's whole member list each time) emptied the member list and never said who came or went.
- A greeting sent as several notices (rrcd sends a line per notice) kept only its first line; the rest showed in whatever room was open.
- Being kicked or banned from a room left it looking joined, and rettui rejoined it on reconnecting.
- Topic changes showed only after rejoining.
- History the Go hub replays on joining showed messages twice, at the time of the replay.
- A hub command typed in a whisper conversation was sent with the conversation as its room.
- Ratspeak's `(+N more)` at the end of a long `/list` showed as a room, and a `/list` sent over several notices kept only its first.
- Mentions of the nick the hub gave you (the Go hub keeps nicks unique, so you may be `zev1`) weren't highlighted.

### Docs and development
- The RRC chat page says how hubs differ and what rettui does about each difference.

## v1.6.0

Changes since v1.5.1.

### Added
- **Messaging**
  - Search messages, in one conversation or all of them (`/`).
  - Search channel history: what was said in every RRC room and whisper conversation, or the one open (`/` in Channels).
  - Pin conversations (`*`) and mark all read (`R`).
  - Forward a message (`f`), and export a conversation as text (`E`).
  - Read archived messages (`H`).
  - Message requests: *Unknown senders* can be set to show, requests or ignore.
  - Sideband/MeshChat icons are shown, and you can set your own.
  - Pictures are shrunk before sending, set by *Send pictures at*.
  - Voice messages can be recorded in the web UI and are sent as Codec2.
  - Answer Sideband's ping, echo and signal-report commands (*Answer commands*, off by default).
- **Locations**
  - Share a location (`L` / 📍) in Sideband's telemetry format, once or live (as Columba does: updates for a while, then a message saying it stopped).
  - Answer location requests (*Location requests*, off by default).
  - A map of shared locations in both UIs (`M`). Map tiles are fetched and cached by rettui.
  - An offline map: *Offline map* names an MBTiles file whose tiles the web map shows first, with no internet at all (enlarged past the file's closest zoom).
- **Network**
  - Find or forget the path to any destination: `P` / `D`, or the Path dialog in the web UI ([#10](https://github.com/zevaryx/rettui/issues/10)).
  - Probe any destination (`T`).
  - New `rettui path` and `rettui probe` commands, like rnpath and rnprobe.
  - Sort the Network list (`s`), or keep it to one interface (`i`).
  - An announce viewer (`a` in Network): every announce as it's heard, of every kind (RRC hubs, calls and others too), with when, how far and through which interface.
  - Pings report RSSI/SNR when answered over an RNode, and so do messages that came in one packet straight from the sender (📶 beside them).
  - Status shows rnstatus-style interface details, and a graph of each interface's traffic over the last ten minutes.
- **Browser**
  - Search nodes and saved pages (`/`).
  - Save a node without opening it (`s`, or ☆ in the web UI).
  - Find in the page (`f`).
- **Web UI**
  - HTTPS with `--https` (rettui's own certificate authority), or `--tls-cert` / `--tls-key` for your own certificate.
  - Works behind a reverse proxy, including under a sub-path.
  - Drafts survive reloads.
  - Light theme.
  - Keyboard shortcuts, with `?` to list them.
  - Sign out other browsers.
- **Terminal UI**
  - Colour themes (dark, light, basic).
  - The footer shows the shortest hints that fit; `?` (or `F1` while typing) lists every key.
- **Installing and updating**
  - Update checks: once a day, rettui can ask GitHub whether a newer release is out, and marks the version (↑) and Status when one is. Off by default; the getting-started guide recommends turning it on (*Check for updates*).
  - Status shows the version and what kind of build it is, with what the last update check found; `u` (or *Check now* in the web UI) checks at once, even with daily checks off.
  - Release binaries can install a newer release in their own place when asked (`U` in Status, Install in the web UI, or `rettui update`), checked against the release's `SHA256SUMS` and run once before they replace the old one. Builds from source, containers and package managers' are told how to update instead.
  - Packages: a Homebrew tap (`brew install zevaryx/rettui/rettui`) and the AUR (`rettui-bin`), published with each release, and Debian packages (x86-64 and ARM64) attached to it.
- **Settings and CLI**
  - Quiet hours: no notifications between two times of day, except (unless turned off) from trusted contacts.
  - Back up everything to one file and restore it: `B` in Status, `rettui backup` / `rettui restore`, or *Download a backup* in the web UI (without the identity).
  - 12-hour clock and date order.
  - *Log level* as a setting.
  - `rettui send` accepts `lxma://` links.

### Changed
- Path requests are retried at 0, 15 and 52 s. A stale path is replaced and the Link is tried again before giving up ([#10](https://github.com/zevaryx/rettui/issues/10)). A page loading, an RRC hub connecting and `rettui fetch` say which request is out.
- Pictures are sent at 1024 px by default. Set *Send pictures at* to original for the old behaviour.
- *Ignore unknown senders* is replaced by *Unknown senders*; existing settings carry over.
- Updated to rsReticulum 1.3.0 and rsLXMF main. rsReticulum follows the `rettui` branch of zevaryx/rsReticulum until [ratspeak/rsReticulum#26](https://github.com/ratspeak/rsReticulum/pull/26) is merged upstream.

### Fixed
- Background notifications, map tiles and update checks failed behind a proxy that inspects HTTPS: they now trust the computer's own certificates as well as the bundled ones.
- Destinations whose first path request went unanswered could only be reached after their next announce ([#10](https://github.com/zevaryx/rettui/issues/10)).
- The `P`/`T` hints were missing while a Network search was active.

### Docs and development
- A documentation site, [zevaryx.github.io/rettui](https://zevaryx.github.io/rettui), built from `wiki/` with Zensical.
- Instructions for running rettui as a systemd service.
- The code is formatted with rustfmt, and CI checks it.
- A release's notes are its section of this changelog; tagging fails without one.

### Known limitations
- Messages received over a Link (most direct messages) don't show RSSI/SNR: rsReticulum doesn't say how a Link's packets were heard.
- LXST voice calls aren't supported because of LXST's license (CC BY-NC-ND 4.0).

**Pull requests:** [#12](https://github.com/zevaryx/rettui/pull/12) Network and Browser search · [#13](https://github.com/zevaryx/rettui/pull/13) path tools · [#14](https://github.com/zevaryx/rettui/pull/14) documentation site · [#15](https://github.com/zevaryx/rettui/pull/15) dependency updates · [#16](https://github.com/zevaryx/rettui/pull/16) quality-of-life changes · [#17](https://github.com/zevaryx/rettui/pull/17) update checks and self-update · [#18](https://github.com/zevaryx/rettui/pull/18) more quality-of-life changes
