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
| rsReticulum | `e16bd15` | The revision rsNomad's CI pins. It adds `RequestOutcome::ReplyFile` and is not on upstream `main`. |
| rsLXMF | `d7da7f3` | The last rsLXMF built against rsReticulum 1.2 at that point. Later commits need rsReticulum 1.3. |
| rsNomad | `05ad8ec` | `main` |

Move these forward together, once rsNomad supports rsReticulum 1.3.
