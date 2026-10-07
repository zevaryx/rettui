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

rettui checks once a day whether a newer release is out. When one is:

- the version beside the name, top left, is marked ↑ (in yellow, in both
  UIs), and opens the new release's page;
- Status says which release, with a link to what's new;
- the log says so, once.

Nothing is downloaded or installed. To update, put the new release's
binary in place of the old one, and start it again; the data directory
stays as it is. With Docker, `docker compose pull` then
`docker compose up -d`; from source, `git pull --recurse-submodules` and
build again.

The check is one HTTPS request a day to GitHub's API (`api.github.com`),
through the proxy set in the environment if there is one, with rettui's
name and version as its user agent. What it found is kept in
`update-check.json` in the data directory, so starting again doesn't
ask again. If it can't reach GitHub (offline, say), it tries again an
hour later, without a word. *Check for updates* (Status,
`update_check`) off never asks.
