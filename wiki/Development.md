For working on rettui: where things are in the code, and how builds and
releases are made. To build it yourself, see [Installing](Installing#building-from-source).

## Code layout

The protocol modules each sit on one of the submodules. The UI modules are split
by tab, and `app/` and `ui/` mirror each other. The web UI drives the same `App`
state as the TUI.

| Module | What it does | Built on |
| --- | --- | --- |
| `net/` | Network actor, on its own threads so heavy traffic can't hold up either UI: runtime, announces, paths and keys, commands and events; rngit requests over Links (`git`) | rsReticulum, rngit-rs |
| `lxmf/` | Sending, receiving and propagation node sync | rsLXMF |
| `nomad/` | Page and file fetching, Micron parsing and layout, page cache, hosting a node and its pages | rsNomad |
| `rrc/` | RRC wire format, hub replies, hub sessions | rsReticulum |
| `reticulum/` | Reticulum config file: the options it has, editing it in place, and checking it | rsReticulum |
| `app/` | Application state per tab (`messages`, `channels`, `network`, `browser`, `node`, `reticulum`, `repos`), the page editor's formatting (`format`), notifications (`notify`) and input routing | |
| `ui/` | Drawing per tab, plus the sidebar, footer and prompt (`chrome`), the keys of each mode for the footer and the `?` list (`keys`), and the text editors | ratatui |
| `term/` | Text input, the editors' text area, selection, clipboard, images (Kitty, Sixel, iTerm2 or half blocks), desktop notifications (`desktop`) | ratatui-image, notify-rust |
| `desktop/` | The desktop app (`rettui-desktop`): the web UI in a window of its own, a package of its own on Tauri | Tauri |
| `web/` | Web UI: HTTP API, login, live updates and notifications, and the page's HTML/CSS/JS, service worker, font and logo (`web/assets`) | axum |

`main.rs` only calls `lib.rs`, which has the command line and starts the
TUI or the web UI; the [Android app](Android) loads the same library and
calls it through `android.rs` (JNI). `cli.rs` has the shell commands (`send`, `listen`, `sync`, `fetch`), and
`store.rs` and `config.rs` hold what is saved to disk, and `app/saver.rs`
writes the store and chat history in the background.

## Releases and CI

The workflows are in [.github/workflows](https://github.com/zevaryx/rettui/tree/main/.github/workflows):

- **CI** ([ci.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/ci.yml)) runs on every pull request:
  - a formatting check: run `cargo fmt` before committing (`rustfmt.toml`
    has rettui's style; `cargo fmt --check` says what it would change);
  - Clippy, with warnings as errors (and the desktop app, which needs
    WebKitGTK, checked on Linux);
  - the tests on Linux, Windows and macOS.
- **Build** ([build.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/build.yml)) builds the binaries
  for every platform in the [Installing](Installing) table without making a
  release:
  - Run it from *Actions > Build > Run workflow* (tick *Linux only* for
    just the Linux binaries).
  - Download the archives from the run's *Artifacts*.
  - They're named after the version and commit, for example
    `rettui-v1.2.0-a4e545b-x86_64-unknown-linux-gnu.tar.gz`.
- **Desktop** ([desktop.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/desktop.yml)) builds
  the desktop app (`desktop/`, `rettui-desktop`) for the same platforms as Build:
  - Run it from *Actions > Desktop > Run workflow* (tick *Linux only* for
    just the Linux builds), and download from the run's *Artifacts*.
  - It runs rettui in the same process and shows its web UI in the
    system's webview (WebKitGTK on Linux, WebView2 on Windows, WKWebView
    on macOS). It's a package of its own so that `rettui` never links
    them: `cargo build` at the top builds rettui alone.
  - It makes, unsigned: a `.deb` (and on x86-64 an AppImage) and a plain
    `.tar.gz` for Linux, an installer `.exe` for Windows and a `.dmg` for
    macOS, named like `rettui-desktop-v1.11.0-x86_64-unknown-linux-gnu.deb`.
    macOS asks to allow an unsigned app in *System Settings > Privacy &
    Security*, and Windows SmartScreen warns before running it.
- **Android** ([android.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/android.yml)) builds a test
  APK of the [Android app](Android) (`android/`):
  - Run it from *Actions > Android > Run workflow*, or push to
    `feature/android`.
  - Download the APK from the run's *Artifacts*.
  - It cross-builds rettui with the NDK (`android/build-rust.sh`), then
    runs Gradle. The `ANDROID_KEYSTORE` secret, if set, signs every build
    with one key.
  - Each build's version code is the minutes since late 2023, so any
    build, test or release, installs over an earlier one.
  - The Release workflow calls it for a release build (not debuggable,
    so `adb` can't read the app's files), signed with the same key.
- **Release** ([release.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/release.yml)) runs when a
  tag starting with `v` is pushed. Before tagging, add the release to
  [CHANGELOG.md](https://github.com/zevaryx/rettui/blob/main/CHANGELOG.md) under a `## v1.7.0` heading (a
  prerelease, `v1.7.0-rc.1`, may have its own, or uses the release's). It:
  1. checks the tag matches the version in `Cargo.toml`, and that
     `CHANGELOG.md` has its section;
  2. builds every platform with the Build workflow, the desktop app with
     the Desktop workflow, and the Android app
     (`rettui-v1.7.0-android-arm64-v8a.apk`) with the Android workflow;
  3. pushes the Docker image to `ghcr.io`, using those Linux binaries
     (the Dockerfile's `prebuilt` stage);
  4. makes Debian packages of the Linux binaries
     (`.github/scripts/package-deb.sh`): `rettui_1.7.0_amd64.deb` and
     `rettui_1.7.0_arm64.deb`;
  5. for a release (not a prerelease), writes a Homebrew formula
     (`rettui.rb`, by `.github/scripts/homebrew-formula.sh`) and the
     AUR's `rettui-bin` and `rettui-desktop-bin` packages (`PKGBUILD` and
     `PKGBUILD-desktop`, by `.github/scripts/aur-pkgbuild.sh`) from the
     checksums;
  6. creates a GitHub release with the archives, the packages, the
     desktop app's installers, the APK,
     `SHA256SUMS`, `rettui.rb` and `PKGBUILD`, and the tag's section of
     `CHANGELOG.md` (printed by `.github/scripts/release-notes.sh v1.7.0`)
     as its notes, with the Docker image's name;
  7. for a release, publishes the formula to the Homebrew tap and the
     package to the AUR (see *Homebrew and the AUR* below).

  A tag containing a hyphen (`v1.3.0-rc.1`) makes a prerelease. Its image
  gets only the version tag, not `latest`. *Run workflow* on a version tag
  does the same as pushing it.
- **Prerelease** ([prerelease.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/prerelease.yml))
  makes a development prerelease of `dev`, on command: *Actions >
  Prerelease > Run workflow*, on `dev`. It:
  1. picks the version it leads up to: the one typed in, else
     `Cargo.toml`'s if that isn't released yet, else the next minor
     version after the newest release (`1.10.0` after `v1.9.0`);
  2. numbers it from that version's earlier ones: `v1.10.0-dev.1`, then
     `v1.10.0-dev.2`;
  3. sets that version in `Cargo.toml` and `Cargo.lock`, and adds a
     section for it to `CHANGELOG.md` (what it is, a link to the commits
     since the last release, then the version's own section if there is
     one yet), all by `.github/scripts/prepare-prerelease.sh`;
  4. commits that and tags the commit, without changing `dev`;
  5. starts the Release workflow on the tag, which builds and publishes
     it as a prerelease (no Homebrew or AUR, no `latest` image).

  Update checks leave prereleases out unless *Include prereleases in
  update checks* is on, so they're mostly for those who download one.
- **Branch image** ([docker-branch.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/docker-branch.yml))
  runs on every push to a branch other than `main`. It builds the Linux
  binaries and pushes an image tagged with the branch name: pushing to
  `dev` gives `ghcr.io/zevaryx/rettui:dev`, and `feature/x` gives
  `:feature-x`. A newer push to the same branch cancels an unfinished run.
- **Wiki** ([wiki.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/wiki.yml))
  copies `wiki/` to this wiki when it changes on `main` or `dev`, or when
  run from *Actions > Wiki > Run workflow*. Edit the pages in `wiki/`, not here: the
  next publish replaces the wiki with them.
- **Docs** ([docs.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/docs.yml))
  builds the [documentation site](https://zevaryx.github.io/rettui/) from
  `wiki/` with Zensical, and publishes it to GitHub Pages from `main`. Pull
  requests that change `wiki/` only build it: a link to a page or heading
  that doesn't exist, or a page left out of `_Sidebar.md`, fails the build.
  `python3 docs/build.py serve` shows it locally (see
  [docs/README.md](https://github.com/zevaryx/rettui/blob/main/docs/README.md)).

Both image workflows push through
[docker.yml](https://github.com/zevaryx/rettui/blob/main/.github/workflows/docker.yml).

To release, bump `version` in `Cargo.toml`, commit, then:

```sh
git tag v1.2.1
git push origin v1.2.1
```

### Homebrew and the AUR

The Release workflow's last job, *Homebrew and AUR*, publishes a release
(not a prerelease) to both, from the release's `SHA256SUMS`. Each half
runs once its secrets are set (*Settings → Secrets and variables →
Actions*), and is skipped with a notice until then. Running it again
(*Re-run jobs*) pushes nothing new, so it's safe to re-run after fixing a
secret.

- **Homebrew:** the tap is a GitHub repository named `homebrew-rettui`
  (`zevaryx/homebrew-rettui`; another, as `owner/name`, in the
  repository variable `HOMEBREW_TAP`). The secret
  `HOMEBREW_TAP_DEPLOY_KEY` is the private half of an SSH key whose
  public half is the tap's deploy key, with write access (tap's
  *Settings → Deploy keys*). The job writes the formula to
  `Formula/rettui.rb` (`.github/scripts/publish-homebrew.sh`). Then
  `brew install zevaryx/rettui/rettui` installs rettui, and
  `brew upgrade` the next release.
- **AUR:** the secret `AUR_SSH_PRIVATE_KEY` is the private half of an SSH
  key whose public half is on the AUR account (*My Account → SSH Public
  Key*); `AUR_USERNAME` and `AUR_EMAIL` are the PKGBUILD's maintainer
  line and its commits' author, both public on the AUR. The job checks
  the AUR's host key against the fingerprint aur.archlinux.org
  publishes, writes `.SRCINFO` with `makepkg` (in an Arch Linux
  container), and pushes to `rettui-bin` and `rettui-desktop-bin`
  (`.github/scripts/publish-aur.sh`); the first push makes each package.
  Then `yay -S rettui-bin` (or any AUR helper) installs rettui, and
  `yay -S rettui-desktop-bin` the desktop app.

By hand, from any release's `SHA256SUMS`: `homebrew-formula.sh` and
`aur-pkgbuild.sh` write the formula and the package, and
`publish-homebrew.sh` and `publish-aur.sh` push them, as the job does.
