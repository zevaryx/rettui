## Release binaries

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
- **Docker:** see [Docker](Docker) for the image.

## Packages

- **Debian, Ubuntu, Raspberry Pi OS:** each release has a Debian package
  for x86-64 (`rettui_<version>_amd64.deb`) and ARM64
  (`rettui_<version>_arm64.deb`): `sudo apt install ./rettui_<version>_amd64.deb`.
  It puts `rettui` in `/usr/bin`; a newer release's package installs over
  it the same way.
- **Arch Linux:** each release has the `PKGBUILD` of the AUR's
  `rettui-bin` package (the ready-built binary): put it in a folder of
  its own and run `makepkg -si` there. Once it's published to the AUR,
  an AUR helper installs it (`yay -S rettui-bin`).
- **Homebrew** (macOS and Linux): each release has a formula,
  `rettui.rb`, for a tap; once there's one, `brew install` installs from
  it.

Installed by a package manager, rettui leaves [updating](#updating) to it.

## Building from source

```sh
git clone --recursive <this repo>   # or: git submodule update --init
cargo build --release
```

The three libraries are git submodules under `deps/`. Each one expects the others
to be checked out next to it (for example `../rsReticulum`). They are pinned to
revisions that build together:

| Submodule | Revision | Why |
| --- | --- | --- |
| rsReticulum | `6825661` | 1.3.0 (upstream `main`) with [ratspeak/rsReticulum#26](https://github.com/ratspeak/rsReticulum/pull/26) merged in: `RequestOutcome::ReplyFile`, file metadata and the requester's identity, which node hosting needs. It's the `rettui` branch of [zevaryx/rsReticulum](https://github.com/zevaryx/rsReticulum/tree/rettui) until that pull request is merged upstream. |
| rsLXMF | `4a0abec` | `main` (it needs rsReticulum 1.3) |
| rsNomad | `05ad8ec` | `main` (its tests pass against rsReticulum 1.3 too) |

Once ratspeak/rsReticulum#26 is merged, point `deps/rsReticulum` back at
ratspeak/rsReticulum in `.gitmodules` and pin its `main`.

## Updating

rettui can check once a day whether a newer release is out. It's
recommended, but off until you turn it on: tick **Check for updates once
a day** in the getting-started guide, or turn on *Check for updates*
(Status, `update_check`). When a newer release is out:

- the version beside the name, top left, is marked ↑ (in yellow, in both
  UIs), and opens the new release's page;
- Status says which release, with a link to what's new;
- the log says so, once.

Checking downloads and installs nothing. To update:

- **A binary from the releases page** can install the new release itself,
  when you ask: `U` in the terminal UI's Status tab, **Install** beside the
  update in the web UI's Status page, or `rettui update` (which checks
  first, whether or not *Check for updates* is on, and asks before
  installing; `--yes` doesn't ask). It downloads the release's archive for
  your system from GitHub with its `SHA256SUMS`, checks the one against
  the other, and runs the new `rettui` once (`--version`) to be sure it
  works on your computer before it replaces the old one. rettui carries on
  as it was: start it again (for a service, restart it) to use the new
  one. The checksums come from the same release, so they catch a download
  gone wrong, not a release that isn't rettui's.
- **By hand:** put the new release's binary in place of the old one, and
  start it again. The data directory stays as it is.
- **Docker:** `docker compose pull`, then `docker compose up -d`. (A
  container's rettui doesn't replace itself: it would be lost with the
  container.)
- **From source:** `git pull --recurse-submodules`, and build again.
- **A package manager's** (a Debian package, the AUR's, Homebrew's):
  update it with that one, as with anything else it installed.

The check is one HTTPS request a day to GitHub's API (`api.github.com`),
through the proxy set in the environment if there is one, with rettui's
name and version as its user agent. Like rettui's other requests to the
web (background notifications, map tiles), it trusts this computer's
certificates as well as the ones rettui comes with, so it works behind a
proxy that inspects HTTPS. What it found is kept in
`update-check.json` in the data directory, so starting again doesn't
ask again. If it can't reach GitHub (offline, say), it tries again an
hour later, without a word. GitHub sees your IP address, as any website
does, so leave it off if you use Reticulum to stay off the internet
(over Tor or I2P, say). Off, it never asks.
