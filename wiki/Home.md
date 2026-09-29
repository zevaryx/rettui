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

## Four ways to use it

- **Terminal UI (TUI):** seven tabs (Messages, Channels, Network, Browser,
  Node, Status, Reticulum) with keyboard and mouse control. Terminals
  narrower than 110 columns get a sidebar of icons and narrower side lists, so
  80×24 works well (see [Using the TUI](Using-the-TUI)).
- **Web UI:** `rettui --web` runs the same client in a browser, with the same
  sections and features. It updates live, switches sections instantly (each
  keeps its content and refreshes behind the scenes, even over a slow link),
  works on phones, uses a bundled Fira Code Nerd Font, and is protected by a
  login link (see [Web UI](Web-UI)).
- **Docker:** a Dockerfile and a Compose file run the web UI in a container
  (see [Docker](Docker)).
- **Command line:** send a message, listen for messages, sync, fetch a page,
  or print your address without starting a UI (see [Running](Running)).

## Pages

**Getting started**

- [Installing](Installing): release binaries, and building from source
- [Running](Running): the commands, and which Reticulum config is used
- [Using the TUI](Using-the-TUI): keys, mouse, copy and paste
- [Web UI](Web-UI): logging in, phones, slow links, the JSON API
- [Docker](Docker): running the web UI in a container

**What it does**

- [Messaging](Messaging): LXMF conversations, delivery, attachments
- [RRC Chat](RRC-Chat): hubs, rooms, whispers and commands
- [Notifications](Notifications): desktop and browser notifications, muting
- [NomadNet Browsing](NomadNet-Browsing): pages, links, forms, images, the cache
- [Hosting a Node](Hosting-a-Node): your own NomadNet node and its page editor
- [Network](Network): announces heard, interfaces, traffic

**Settings and data**

- [Settings and Reticulum Config](Settings-and-Reticulum-Config): the settings and config editors
- [Data and Storage](Data-and-Storage): what's kept where, message history limits

**More**

- [Notes](Notes): things that may look odd, and why
- [Development](Development): code layout, CI and releases
