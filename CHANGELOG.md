# Changelog

What changed in each release of rettui. Releases before v1.6.0 are described on the [releases page](https://github.com/zevaryx/rettui/releases).

## v1.6.0

Changes since v1.5.1.

### Added
- **Messaging**
  - Search messages, in one conversation or all of them (`/`).
  - Pin conversations (`*`) and mark all read (`R`).
  - Forward a message (`f`), and export a conversation as text (`E`).
  - Read archived messages (`H`).
  - Message requests: *Unknown senders* can be set to show, requests or ignore.
  - Sideband/MeshChat icons are shown, and you can set your own.
  - Pictures are shrunk before sending, set by *Send pictures at*.
  - Voice messages can be recorded in the web UI and are sent as Codec2.
  - Answer Sideband's ping, echo and signal-report commands (*Answer commands*, off by default).
- **Locations**
  - Share a location (`L` / 📍) in Sideband's telemetry format.
  - Answer location requests (*Location requests*, off by default).
  - A map of shared locations in both UIs (`M`). Map tiles are fetched and cached by rettui.
- **Network**
  - Find or forget the path to any destination: `P` / `D`, or the Path dialog in the web UI ([#10](https://github.com/zevaryx/rettui/issues/10)).
  - Probe any destination (`T`).
  - New `rettui path` and `rettui probe` commands, like rnpath and rnprobe.
  - Sort the Network list (`s`), or keep it to one interface (`i`).
  - Pings report RSSI/SNR when answered over an RNode.
  - Status shows rnstatus-style interface details.
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
- **Settings and CLI**
  - Update checks: once a day, rettui can ask GitHub whether a newer release is out, and marks the version (↑) and Status when one is. Off by default; the getting-started guide recommends turning it on (*Check for updates*).
  - Release binaries can install a newer release in their own place when asked (`U` in Status, Install in the web UI, or `rettui update`), checked against the release's `SHA256SUMS` and run once before they replace the old one. Builds from source, containers and package managers' are told how to update instead.
  - 12-hour clock and date order.
  - *Log level* as a setting.
  - `rettui send` accepts `lxma://` links.

### Changed
- Path requests are retried at 0, 15 and 52 s. A stale path is replaced and the Link is tried again before giving up ([#10](https://github.com/zevaryx/rettui/issues/10)).
- Pictures are sent at 1024 px by default. Set *Send pictures at* to original for the old behaviour.
- *Ignore unknown senders* is replaced by *Unknown senders*; existing settings carry over.
- Updated to rsReticulum 1.3.0 and rsLXMF main. rsReticulum follows the `rettui` branch of zevaryx/rsReticulum until [ratspeak/rsReticulum#26](https://github.com/ratspeak/rsReticulum/pull/26) is merged upstream.

### Fixed
- Destinations whose first path request went unanswered could only be reached after their next announce ([#10](https://github.com/zevaryx/rettui/issues/10)).
- The `P`/`T` hints were missing while a Network search was active.

### Docs and development
- A documentation site, [zevaryx.github.io/rettui](https://zevaryx.github.io/rettui), built from `wiki/` with Zensical.
- Instructions for running rettui as a systemd service.
- The code is formatted with rustfmt, and CI checks it.

### Known limitations
- Received messages don't show RSSI/SNR; this needs an rsReticulum change.
- LXST voice calls aren't supported because of LXST's license (CC BY-NC-ND 4.0).

**Pull requests:** [#12](https://github.com/zevaryx/rettui/pull/12) Browser search · [#13](https://github.com/zevaryx/rettui/pull/13) path tools · [#16](https://github.com/zevaryx/rettui/pull/16) quality-of-life changes
