For working on rettui: where things are in the code, and how builds and
releases are made. To build it yourself, see [Installing](Installing#building-from-source).

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
| `app/` | Application state per tab (`messages`, `channels`, `network`, `browser`, `node`, `reticulum`), the page editor's formatting (`format`), notifications (`notify`) and input routing | |
| `ui/` | Drawing per tab, plus the sidebar, footer and prompt (`chrome`), the keys of each mode for the footer and the `?` list (`keys`), and the text editors | ratatui |
| `term/` | Text input, the editors' text area, selection, clipboard, images (Kitty, Sixel, iTerm2 or half blocks), desktop notifications (`desktop`) | ratatui-image, notify-rust |
| `web/` | Web UI: HTTP API, login, live updates and notifications, and the page's HTML/CSS/JS, service worker, font and logo (`web/assets`) | axum |

`cli.rs` has the shell commands (`send`, `listen`, `sync`, `fetch`), and
`store.rs` and `config.rs` hold what is saved to disk, and `app/saver.rs`
writes the store and chat history in the background.

## Releases and CI

The workflows are in [.github/workflows](https://github.com/zevaryx/rettui/tree/main/.github/workflows):

- **CI** ([ci.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/ci.yml)) runs on every pull request:
  - Clippy, with warnings as errors;
  - the tests on Linux, Windows and macOS.
- **Build** ([build.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/build.yml)) builds the binaries
  for every platform in the [Installing](Installing) table without making a
  release:
  - Run it from *Actions > Build > Run workflow* (tick *Linux only* for
    just the Linux binaries).
  - Download the archives from the run's *Artifacts*.
  - They're named after the version and commit, for example
    `rettui-v1.2.0-a4e545b-x86_64-unknown-linux-gnu.tar.gz`.
- **Release** ([release.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/release.yml)) runs when a
  tag starting with `v` is pushed. It:
  1. checks the tag matches the version in `Cargo.toml`;
  2. builds every platform with the Build workflow;
  3. pushes the Docker image to `ghcr.io`, using those Linux binaries
     (the Dockerfile's `prebuilt` stage);
  4. creates a GitHub release with the archives, `SHA256SUMS` and
     generated notes.

  A tag containing a hyphen (`v1.3.0-rc.1`) makes a prerelease. Its image
  gets only the version tag, not `latest`.
- **Branch image** ([docker-branch.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/docker-branch.yml))
  runs on every push to a branch other than `main`. It builds the Linux
  binaries and pushes an image tagged with the branch name: pushing to
  `dev` gives `ghcr.io/zevaryx/rettui:dev`, and `feature/x` gives
  `:feature-x`. A newer push to the same branch cancels an unfinished run.
- **Wiki** ([wiki.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/wiki.yml))
  copies `wiki/` to this wiki when it changes on `main` or `dev`, or when
  run from *Actions > Wiki > Run workflow*. Edit the pages in `wiki/`, not here: the
  next publish replaces the wiki with them.

Both image workflows push through
[docker.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/docker.yml).

To release, bump `version` in `Cargo.toml`, commit, then:

```sh
git tag v1.2.1
git push origin v1.2.1
```
