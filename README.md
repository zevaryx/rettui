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

## What it does

- **Messaging (LXMF):** conversations with any LXMF client, direct or through
  a propagation node, with attachments, stamps and ratchets.
- **Chat (RRC):** hubs, rooms and whispers over Reticulum Relay Chat, the
  IRC-like protocol NomadNet 1.4 also speaks.
- **NomadNet:** browse pages (Micron, forms, images, a page cache), and host
  your own node with a page editor and live preview.
- **Propagation node:** keep messages for people who are offline, as `lxmd`
  does (other nodes can peer with it).
- **Notifications:** the desktop's own in the terminal, the browser's (phones
  too) in the web UI, with per-conversation and per-room muting.
- **Reticulum config:** edit every option of your Reticulum config, checked
  with rsReticulum's own parser before it's saved.

It has two interfaces with the same features: a **terminal UI** with keyboard
and mouse control (80×24 is enough), and a **web UI** (`rettui --web`) that
works on phones and over slow links. It also runs in Docker, and from the
command line for scripts.

## Quick start

Download `rettui` for Linux, Windows or macOS from the
[releases](https://github.com/zevaryx/rettui/releases) and put it on your
`PATH` (or install the release's Debian package:
`sudo apt install ./rettui_<version>_amd64.deb`; see
[Installing](https://github.com/zevaryx/rettui/wiki/Installing#packages)
for Arch and Homebrew), then:

```sh
rettui          # the terminal UI
rettui --web    # the web UI: open the link it prints
```

The web UI's link logs a browser in: keep it private, since anyone with it
can read and send your messages. It listens on 127.0.0.1 unless you give it
another address.

With Docker, from a checkout:

```sh
git clone --recursive https://github.com/zevaryx/rettui && cd rettui
docker compose up -d --build
docker compose logs rettui    # the login link
```

To build it yourself: `git clone --recursive` this repository, then
`cargo build --release`.

## Documentation

Everything else is on the documentation site,
[zevaryx.github.io/rettui](https://zevaryx.github.io/rettui/), and in the
[wiki](https://github.com/zevaryx/rettui/wiki): both are built from [wiki/](wiki) in this repository.

- **Getting started:** [Installing](https://github.com/zevaryx/rettui/wiki/Installing) ·
  [Running](https://github.com/zevaryx/rettui/wiki/Running) · [Using the TUI](https://github.com/zevaryx/rettui/wiki/Using-the-TUI) ·
  [Web UI](https://github.com/zevaryx/rettui/wiki/Web-UI) · [Docker](https://github.com/zevaryx/rettui/wiki/Docker)
- **What it does:** [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging) · [RRC Chat](https://github.com/zevaryx/rettui/wiki/RRC-Chat) ·
  [Notifications](https://github.com/zevaryx/rettui/wiki/Notifications) ·
  [NomadNet Browsing](https://github.com/zevaryx/rettui/wiki/NomadNet-Browsing) ·
  [Hosting a Node](https://github.com/zevaryx/rettui/wiki/Hosting-a-Node) · [Network](https://github.com/zevaryx/rettui/wiki/Network)
- **Settings and data:**
  [Settings and Reticulum Config](https://github.com/zevaryx/rettui/wiki/Settings-and-Reticulum-Config) ·
  [Data and Storage](https://github.com/zevaryx/rettui/wiki/Data-and-Storage)
- **More:** [Notes](https://github.com/zevaryx/rettui/wiki/Notes) · [Development](https://github.com/zevaryx/rettui/wiki/Development)

## License

AGPL-3.0-or-later, the same as the libraries it links.

The web UI bundles [Fira Code](https://github.com/tonsky/FiraCode) as patched by
[Nerd Fonts](https://github.com/ryanoasis/nerd-fonts), under the SIL Open Font
License 1.1 (see [src/web/assets/fonts](src/web/assets/fonts)).
