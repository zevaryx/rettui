# Changelog

What changed in each release of rettui. Releases before v1.6.0 are described on the [releases page](https://github.com/zevaryx/rettui/releases).

## v1.10.0

Changes since v1.9.0: git repositories on rngit nodes (their branches, releases and work documents, and a copy of one kept on your computer) in a new Repos tab, nine more games (Ultimate Tic-Tac-Toe, Go, Battleship, Quoridor, Nine Men's Morris, Breakthrough, Amazons, Pente and Rock Paper Scissors), a first version of an Android app, attached to the release, the web UI reworked for phones, how to play each game beside its board, copying an identity, or keeping another, from the command line, development prereleases of `dev` on command, the web UI's questions in dialogs of its own, tabs in the Browser, your own node's script pages run in the Browser, Sideband commands sent, how a send or sync is going and cancelling a send, your location sent on a timer and someone tracked, telemetry collectors, the propagation node you host passing messages on to other nodes, interface profiles to switch between, Reticulum's path table and announce rates, trails of where people have been on the map, pages kept to those you pick, what your node has served, files sent to rncp listeners, speed tests with rettui or NomadNet, whose an address is, what an RNode's settings come to, voice messages played as they come, LXMF comments and continuations shown for what they are, places passed on to an MQTT server, paper messages printed from the terminal, what others send (messages, pages, pictures, RRC notices, entry points) kept from losing your saved messages, stopping or holding up rettui, using up its memory or passing for an RRC hub, the web UI kept from starting a pipe interface's program, and less battery, disk and network used (most of all on Android).

### Added
- **Repos tab:** read and write git repositories hosted on [rngit](https://github.com/zevaryx/rngit-rs) nodes, in both UIs (`0` in the terminal). Add one by its `rns://` URL, or open an `rns://` link in a message or page; rngit nodes that announce are listed too. Each shows its branches and tags, its releases (whose files download checked against the release's signature), its work documents with their comments (and whether the author's signature checks out), and its permissions. If the node lets you, you can write, propose, comment on, edit, finish and delete documents, and publish and delete releases. *Use rngit's identity* (terminal only) asks as the `rngit` command's identity, and *rngit timeout* sets how long a request may take. `m` (**Copy to this computer** in the web UI) keeps a copy of a repository with all its branches and tags in rettui's data folder, to read or clone from, and brings it up to date later, removing what the node no longer has; `M` deletes it. Committing and pushing aren't in rettui. See [Repos](https://github.com/zevaryx/rettui/wiki/Repos).
- **Stop rettui from the web UI:** a **Stop rettui** button on Status, and one in a phone's More menu, save everything and close rettui, as `q` does in the terminal. They ask first. See [Web UI](https://github.com/zevaryx/rettui/wiki/Web-UI).
- **More room for pages on a phone:** in the web UI's Browser, scrolling a page down shrinks the top bars to the back arrow, smoothly, from the first bit you scroll; scrolling up (or back to the top) brings them back. See [NomadNet browsing](https://github.com/zevaryx/rettui/wiki/NomadNet-Browsing).
- **NomadNet files saved where you choose:** *NomadNet files folder* (Browsing settings, `nomad_files_dir`) names the folder files saved from NomadNet pages go in, instead of `downloads/` in the data directory. It's set in the terminal UI or settings.json, not the web UI. See [Data and Storage](https://github.com/zevaryx/rettui/wiki/Data-and-Storage).
- **Nine more games:** Ultimate Tic-Tac-Toe, Go (9×9, with White's komi of 7.5), Battleship, Quoridor, Nine Men's Morris, Breakthrough, Amazons, Pente and Rock Paper Scissors, over LRGP as lrgp-rs plays them, in both UIs. Battleship and Rock Paper Scissors hide a choice behind a fingerprint until it's safe to show it, so neither player can cheat; your Battleship fleet is placed at random. In the terminal, `p` passes in Go and `1`–`3` throw in Rock Paper Scissors; picking a move square by square (Breakthrough, Amazons, Nine Men's Morris) marks where it can go next. The Games tab also wraps the line under a board when the pane is narrow. See [Games](https://github.com/zevaryx/rettui/wiki/Games).
- **Tabs in the Browser:** several pages open at once, each with its own history. A saved page or node picked from the list opens in a new tab (an empty tab shown takes it), and the tabs are always shown, named across the top of the address bar; **Open pages in new tabs** in Settings turns that off. `T` opens the selected link in a new tab (in the list, the page or node in the tab shown), `W` closes the tab, `[` and `]` move between them, and in the terminal `Alt+1` to `Alt+9` go to one. Ctrl+click or a middle-click opens a link in a new tab, **+** after the tabs opens an empty one, and **×** on a tab, or a middle-click on it, closes it, in the terminal as in the web UI. A page loading in a tab out of sight goes on loading. See [NomadNet Browsing](https://github.com/zevaryx/rettui/wiki/NomadNet-Browsing). ([#39](https://github.com/zevaryx/rettui/issues/39))
- **Questions in the web UI's own dialogs:** what the web UI asks (before deleting, blocking, banning or switching something, and for an address or a name) is now a dialog in its theme rather than the browser's box, which showed "The page at http://127.0.0.1 says" in the Android app. Buttons say what they do, and are red when they delete; a wrong address or name is explained under the box, which keeps what was typed; Chess promotion is four buttons rather than a letter to type. The page keeps updating while one is open (the browser's boxes stopped it), and it can't be switched off by the browser's "prevent this page from creating additional dialogs", after which every question answered Cancel. See [Web UI](https://github.com/zevaryx/rettui/wiki/Web-UI).
- **How to play:** each game's board says, in a few sentences, how the game is played and won, under **How to play** (in the terminal, when the window is tall enough for it). See [Games](https://github.com/zevaryx/rettui/wiki/Games).
- **The web UI on phones, reworked:** the sections are in a bar at the bottom of the screen (Messages, Channels, Games, Browser, and More for the rest) rather than a drawer; what you read is in the phone's own font; a conversation's header and the Network screen keep their less used buttons behind ⋯; and the message box is one line, with + for files, locations and the way of delivery, and 🎤 that turns into Send. Larger screens are unchanged. See [Web UI](https://github.com/zevaryx/rettui/wiki/Web-UI). Notices go when tapped or swiped aside, and Messages, Games and Channels have a round + button for a new conversation, game or hub.
- **`rettui identity export FILE`** saves a copy of the identity in use, never over another file. **`rettui identity import FILE --name NAME`** keeps the identity in a file (Sideband's, NomadNet's, MeshChat's or another rettui's) as another identity, to switch to in Status > Identities; the one in use stays. See [Running](https://github.com/zevaryx/rettui/wiki/Running#commands).
- **Android app:** rettui, with its whole Reticulum stack, runs on an Android phone, with the web UI as the app's screen. It keeps running in the background and shows messages and mentions as Android notifications, which can be answered or marked read from the notification. Voice recording, location sharing, file uploads and downloads work. Its *Identity and backups* screen saves a copy of the identity, keeps another from a file, and saves and restores full backups, which the web UI can't. TCP and UDP interfaces work, and the app's *RNodes* screen adds RNodes over USB and Bluetooth; AutoInterface doesn't find peers yet. Text, pictures and files shared from other apps go into a message, Reticulum links shared or tapped there open in rettui, and it asks once to keep running in the background. Its back button closes an open dialog or sheet before going back a pane, its notifications show rettui's own icon, and joins and leaves in RRC rooms start hidden on it. Each release now has the APK attached (`rettui-v1.10.0-android-arm64-v8a.apk`, for almost every phone), and it installs over earlier test builds signed with the same key. See [Android](https://github.com/zevaryx/rettui/wiki/Android).
- **Android app uses less battery:** AutoInterface now leaves out the phone's mobile data interfaces (it found no one there, and its messages every few seconds kept the mobile radio awake: on a Pixel, around 90% of what rettui used), the app keeps the processor awake only with the new *Keep the phone awake* setting (`keep_awake`, off), it asks Wi-Fi to pass multicast only while an AutoInterface is on, and the web UI does nothing while it's out of sight. Its service now hears only about notifications, not every peer and announce, and the app no longer checks on rettui every 2 seconds while it's in the background. An RRC hub that can't be reached is tried less and less often (up to every 15 minutes rather than every minute), and a search for a propagation node that finds none comes 10, 20, 40 minutes apart and so on, up to every 6 hours, rather than every 10 minutes. See [Android](https://github.com/zevaryx/rettui/wiki/Android#battery).

- **Your own script pages run in the Browser:** a script page on the node you host (a program, such as a Python `.mu` page like those in SebastianObi's NomadNet-Pages) now runs when you open it in either UI's Browser, as it does for visitors, with what a form sends, rather than showing only "Script page". When visitors would get nothing (the script printed nothing, couldn't start, ran past 10 seconds or printed more than a page may be), the page says why and shows what it wrote to its error output, and points out a first line like `#!/usr/bin/env python3` naming a program that scripts can't find, with where it is on your computer. With "Run page scripts" off, the page says visitors get the script's text instead. See [Hosting a Node](https://github.com/zevaryx/rettui/wiki/Hosting-a-Node#script-pages).
- **Sending Sideband commands:** ask someone where they are, ping them, ask for an echo or a signal report, as Sideband does. In the terminal, `K` in a conversation sends one typed as in Sideband's command box (`ping`, `echo <text>`, `signal`, `location`), and the contact card has *Ask where they are* (`w`) and *Command* (`k`); in the web UI, they're on the contact dialog (and `K`). Sideband answers with a message, or with its location, which goes on the map. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#reactions-locations-commands-and-voice-messages).
- **How a send is going, and cancelling it:** a message being sent says how it's going beside *sending…* (finding a path, working out its stamp, connecting, and how much of a large one has gone, as *42%*), in both UIs, as NomadNet and Sideband show it. `c` on a picked message in the terminal, or **■ Cancel** in the web UI, stops sending it: it shows as *cancelled*, isn't sent again when they announce, and can be sent again by hand. A sync says how many of the messages waiting have come (*3 of 10 downloaded*) in Status. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging).
- **Location on a timer, and tracking someone:** *Send to trusted every* (Status, Location, `telemetry_every_mins`) sends this station's Location, with sensor readings if they're sent, to every contact you trust every so many minutes, as Sideband sends its telemetry on a timer. *Track* on the contact card (`f` in the terminal, *📡 Track…* in the web UI) asks someone where they are every so many minutes while rettui runs, so the map follows them; the conversation's header says so. Each update, and each answer, replaces the one before it in the conversation. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#locations-and-the-map).
- **Telemetry collectors:** a group can see each other on the map through a collector, as with Sideband's. Set *Telemetry collector* to its address (Status, Location), and *Send to collector every* and *Ask collector every* to how often to send it this station's location and ask it for everyone's. The places it passes on go on the map in both UIs, marked *via* the collector, if it's your collector or a contact you trust. *Be a collector* (`collector_serve`) makes rettui one: it answers collector requests with the newest location of everyone who sent one, and this station's. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#locations-and-the-map).
- **Your propagation node passes messages on:** the propagation node you host now peers with other propagation nodes, as `lxmd` does, so a message sent through it reaches its recipient whichever node they collect from (before, it only kept them until collected from it). It peers with those it hears announcing within 4 hops, up to 20 (*Auto-peer*, *Auto-peer depth* and *Most peers* in Settings › Hosting), and always with any *Static peers* you list. Every 24 seconds one of them is offered what it hasn't had. Status says how many nodes it peers with and how many messages it has passed on, in both UIs. See [Hosting a Node](https://github.com/zevaryx/rettui/wiki/Hosting-a-Node#propagation-node).
- **Interface profiles:** keep which Reticulum interfaces are on under a name, and switch between them (at home, say, and out with a radio), as in NomadNet 1.4. `P` in the Reticulum tab, or **⇄ Profiles** in the web UI, lists them, saves the interfaces as they are now, switches to one and deletes one. Switching turns the profile's interfaces on and off, leaves ones added since as they are, keeps the change in the config's history and restarts Reticulum. The web UI can't switch to a profile that turns on a pipe interface. See [Settings and Reticulum Config](https://github.com/zevaryx/rettui/wiki/Settings-and-Reticulum-Config).
- **Reticulum's path table and announce rates:** `t` in the Network tab, or **Tables** in the web UI, shows every path Reticulum knows (where it goes, its hops, the transport node, the interface and how long it's kept) and how often each destination announces, with those held back for announcing too often first, as `rnpath -t` and `rnpath -r` list them. Both are searched by name, address or interface, and read again every 5 seconds while shown. See [Network](https://github.com/zevaryx/rettui/wiki/Network).
- **Trails on the map, and deleting someone's locations:** each place someone sends, or a collector passes on, is kept on their trail, though each update replaces the one before in the conversation. The map draws it as a line to where they are now, in both UIs, and says how many places it has. Each keeps the newest 100 places from the last 30 days. `x` on the map, or **Delete locations** in the web UI, takes someone's place and trail off the map; their messages stay. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#locations-and-the-map).
- **Who may see a page:** a page, picture or file on the node you host can be kept to people you pick, as on NomadNet. `w` in the Node tab, or **Who may see it…** in its ⋯ menu in the web UI, lists your contacts and those you've talked with (once their key has been heard) to tick; visitors not ticked, or who haven't identified themselves, are refused. The list is kept beside it as NomadNet's `.allowed` file, so lists made for NomadNet work too, and it goes along when what it's for is renamed, moved or deleted. A list that's a program is left for you to change on the host. See [Hosting a Node](https://github.com/zevaryx/rettui/wiki/Hosting-a-Node#nomadnet-node).
- **Node statistics:** the Node tab says how many pages, files and pictures the node you host has served, how many requests it had and how many were for something not there, when the last came and since when it has counted, as NomadNet does. The counts are kept across restarts until reset (`R` in the Node tab, **Reset** in the web UI), and Status shows them while the node runs, in both UIs. See [Hosting a Node](https://github.com/zevaryx/rettui/wiki/Hosting-a-Node#nomadnet-node).
- **Sending a file with rncp:** `F` in the Network tab, or **Send a file…** in the web UI, sends a file (up to 32 MB) to someone running Reticulum's `rncp -l`, as `rncp` does and NomadNet 1.4 can, with how it's going as it goes. It says which identity the listener must allow. The web UI sends a file the browser picks, never one from the computer rettui runs on. See [Network](https://github.com/zevaryx/rettui/wiki/Network).
- **Speed test:** `v` in the Network tab, or **Speed test** in the web UI, measures how fast data goes between you and someone using rettui or NomadNet, as NomadNet 1.4's speed test does. Each gives the other their speed test address; either then sends a test of 2 KB to 16 MB, and both see the rate. While it's open, rettui takes tests only from the address entered. See [Network](https://github.com/zevaryx/rettui/wiki/Network).
- **Whose an address is:** `I` in the Network tab, or **Identities…** in the web UI, shows your own addresses (identity, LXMF address, node, propagation node, RRC hub, speed test), and for any other address, the identity behind it, what it is and that identity's other addresses, as NomadNet 1.4's Identity utility does. An address not heard yet is asked of the network. See [Network](https://github.com/zevaryx/rettui/wiki/Network).
- **What an RNode's settings come to:** under an RNode interface's options in the Reticulum tab, in both UIs, rettui works out its data rate, how long a full packet is on air, how faint a signal it hears, its link budget and how far that reaches in free space, again as you change them. See [RNode Radios](https://github.com/zevaryx/rettui/wiki/RNode-Radios#using-it-in-rettui).
- **Voice messages played as they come:** `V` in a conversation, or **▶ Voice** in its ⋯ menu in the web UI, plays their voice messages as soon as they arrive, like Sideband's push-to-talk: in the terminal with the program your desktop plays sound with, in the web UI in the browsers showing it. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#reactions-locations-commands-and-voice-messages).
- **Comments and continuations:** LXMF 1.2's comments on a message, and messages carrying on from an earlier one, show for what they are in both UIs: a comment names the message it's on, and a continuation straight after the message it carries on from shows as part of it. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#replies).
- **Telemetry to MQTT:** *Telemetry to MQTT* (Status, Location, `telemetry_mqtt`) passes every place rettui hears, and this station's, on to an MQTT server, laid out as Sideband publishes them (`lxmf/telemetry/<address>/location/latitude` and so on), for home automation or a dashboard. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#locations-and-the-map).
- **Printing paper messages from the terminal:** `p` on a paper message's QR code in the TUI prints it, with *Print command* (Settings, Messages, `print_command`) or else `lp` or `lpr`. The web UI already prints with the browser. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#paper-messages).
### Fixed
- **A repository called `.git`:** `rns://<node>/<group>/.git` could not be added or opened, because the `.git` was taken off the name and nothing was left. It is asked for by that name now, as `rngit` does. See [Repos](https://github.com/zevaryx/rettui/wiki/Repos).
- **Stopping rettui from the app:** on Android, **Stop rettui** in the web UI left the app waiting for rettui as if it had crashed, until it was force closed. The app now says rettui is stopped, with **Start** and a new **Close the app** button (also shown when rettui fails to start). See [Android](https://github.com/zevaryx/rettui/wiki/Android).
- **Readable text on a phone:** in the web UI's contact dialog, the "Sideband commands:" label was squeezed into a column a few letters wide beside its buttons; on a narrow screen it now has a line to itself. Dimmed text (labels, hints) is also brighter in the dark theme, so it reads on a phone in daylight.
- **Signal reports say how a message was heard:** with *Answer commands* on, a signal report request was answered "No reception info available" even when rettui knew the RSSI, SNR and link quality it came in at. It now sends those, as Sideband does. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#reactions-locations-commands-and-voice-messages).
- **Busy networks write less, and peers don't pile up:** hearing a known peer again was saved every 10 seconds, and new public keys every 5; now a peer heard again is saved every 5 minutes and keys every 30 seconds (new peers and names still within 10 seconds). Peers not heard from in 30 days are forgotten, with their keys, unless you have a conversation with them, they're a contact, a saved page or hub is on them, or they're your propagation node. See [Data and Storage](https://github.com/zevaryx/rettui/wiki/Data-and-Storage).
- **Phones held sideways:** the web UI on a phone in landscape left a third of the screen or less for the list or page, under the top bar, the section's header and the bottom bar. The sections now go in a rail down the left, and the bars and headers are shorter. See [Web UI](https://github.com/zevaryx/rettui/wiki/Web-UI).
- **A connection banner that wouldn't go:** in the Android app, losing Wi-Fi or mobile data showed "Offline: reconnecting when back online…" over the page, and it stayed until the app was closed, though rettui (on the phone itself) was still connected. The banner now shows only when the page loses rettui, and × hides it.
- **The web UI fetches less:** the status it fetches every few seconds no longer carries the whole log (only Status asks for it, and redraws it only when it changed), status updates come at most every 2 seconds, and a page out of sight (a background tab) fetches nothing until it shows again. On a busy network a page fetched the status every 1.4 seconds, hidden or not.

### Security
- **A message's time can no longer lose your messages:** a message whose time wasn't a number (which anyone can send) was saved in a way rettui couldn't read back, so at the next start every conversation was gone, and the next save wrote over them. A time that can't be one (not a number, before 1970 or after 9999) is now taken as when the message came; a saved time like that reads again; and a store that can't be read is moved aside rather than saved over. See [Data and Storage](https://github.com/zevaryx/rettui/wiki/Data-and-Storage).
- **A message's time can no longer stop the terminal UI:** a message dated near the end of the times rettui can hold (the year 262142) stopped rettui whenever it was shown, in time zones ahead of UTC (near the start of them, behind UTC), and again each time its conversation was opened. An RRC chat line could do the same. Such a time now shows none, and a new message's is taken as when it came. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging).
- **Long Micron messages and pages no longer hold rettui up:** some Micron markup (many images, anchors or form fields on a page, or in a message, which can be a megabyte) was read again for each one, so a message of it took minutes to show. The terminal UI read it again each time it drew the conversation, and the web UI each time it fetched it, holding up the web UI for everyone. Micron is now read in one pass, and a formatted message (Micron or Markdown) is kept once it's made. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging).
- **A page of dividers can no longer swamp the web UI:** each Micron divider (two characters of a page) was drawn in the web UI as 1200 of its character, so a page or message of them made gigabytes, which held up the web UI for everyone and could run the computer out of memory. A page's first 50 dividers are drawn as before and the rest as a plain line. See [NomadNet Browsing](https://github.com/zevaryx/rettui/wiki/NomadNet-Browsing).
- **Others in an RRC room can no longer pass for the hub:** a notice another user sent to a room was taken as the hub's, so it showed as `-hub-` and could change the room's topic, the list of rooms or who's there, or open a stretch of "history" whose messages then showed any time their sender chose. Only the hub's own notices do that now; another user's shows as said by them. See [RRC Chat](https://github.com/zevaryx/rettui/wiki/RRC-Chat).
- **A discovered entry point can no longer be this computer:** an entry point anyone announces was connected to unless its address was written exactly as `127.0.0.1`, `::1` or the like, so one given as `127.1.2.3`, `localhost` or `::ffff:127.0.0.1` had rettui connect to whatever listens on this computer. Addresses are now read as addresses, and none of this computer's is used. Entry points elsewhere on your network still are. See [Running](https://github.com/zevaryx/rettui/wiki/Running).
- **One message can no longer have rettui send thousands:** with *Answer commands* on, every command in a message was answered with a message of its own, and an echo was sent back however long it was, so one message could have rettui send thousands, each a megabyte. Only a message's first 8 commands are read now, and an echo's first 500 characters sent back. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging).
- **The web UI can no longer start a pipe interface's program:** it couldn't add or change a pipe interface's command, but it could turn on one that was off, or copy one, which runs its program when Reticulum starts. That's now kept to the terminal too; turning one off still works from the web UI. See [Settings and Reticulum Config](https://github.com/zevaryx/rettui/wiki/Settings-and-Reticulum-Config).
- **Pictures can no longer run the terminal UI out of memory:** a picture of a few kilobytes can say it's millions of pixels across, and was decoded however large it said it was, each on a thread of its own, so a page or conversation of them could take gigabytes. A picture wider or taller than 16384 pixels isn't shown now, one decode takes at most 256 MB, and pictures are decoded two at a time. See [NomadNet Browsing](https://github.com/zevaryx/rettui/wiki/NomadNet-Browsing).

### Docs and development
- **rngit-rs:** the rngit client library is a submodule in `deps/rngit-rs`, built against the same rsReticulum as rettui (a `[patch]` in `Cargo.toml` keeps one copy). See [Development](https://github.com/zevaryx/rettui/wiki/Development).
- **Development prereleases:** *Actions > Prerelease > Run workflow* (on `dev`) publishes a prerelease of `dev`'s newest commit, `v1.10.0-dev.1` and so on, built and attached like a release, with a link to what changed since the last release. `dev` isn't changed: the tag holds a commit with the prerelease's version. Update checks leave prereleases out unless *Include prereleases in update checks* is on. See [Development](https://github.com/zevaryx/rettui/wiki/Development).

## v1.9.0

Changes since v1.8.0: six new games (Checkers, Dots and Boxes, Gomoku, Reversi, Mancala and Hex), claiming a draw in Chess and Checkers, a history of the Reticulum config to compare and put back, every Codec2 mode decoded and voice messages recorded in the terminal, several identities to switch between, Sideband's sensor readings shown and this computer's sent, reactions from Mesh-Client no longer show as a message as well, two small fixes found while writing tests, a LICENSE file, and code coverage measured in CI.

### Added
- **Checkers:** a fourth game, American checkers (English draughts), from lrgp-rs's new `checkers.1`, so it plays against other LRGP clients that have it. In the terminal, `Enter` on one of your pieces and then on each square it lands on (`t` types a move as `a5 c3 e1`); in the web UI, a click on each. Jumps must be taken and finished, and the board says so; where only one way goes on, the rest of a multi-jump is made for you. Black moves first and, like Black in Chess, sees the board turned round. See [Games](https://github.com/zevaryx/rettui/wiki/Games).
- **Dots and Boxes, Gomoku, Reversi, Mancala and Hex:** five more games, from lrgp-rs's `dots_and_boxes.1`, `gomoku.1`, `reversi.1`, `mancala.1` and `hex.1`, so they play against other LRGP clients that have them. Each move is one square, line or pit: in the terminal, the arrows and `Enter`, a click, or `t` to type it (`1`–`6` pick a Mancala pit); in the web UI, a click. Dots and Boxes is 5×5 boxes, and closing a box earns another line; Gomoku is freestyle five in a row on 15×15; Reversi passes for a side with no move ("Your move again: they can't move") and dots where you can play; Mancala is Kalah, six pits of four, with your pits along the bottom and your store on the right; Hex is 11×11 with the swap rule (`s`, or **Swap**). Games won on a count say the score ("You won, 36–28"). See [Games](https://github.com/zevaryx/rettui/wiki/Games).
- **Reticulum config history:** each time rettui saves the Reticulum config it keeps that version, with what was done, and the file as it was before rettui first changed it (or as another program left it). `H` in the Reticulum tab (**🕘 History** in the web UI) lists them beside what each changed, or what putting it back would change, line by line, and puts one back. The newest 50 are kept. See [Settings and Reticulum Config](https://github.com/zevaryx/rettui/wiki/Settings-and-Reticulum-Config).
- **Every Codec2 voice message plays:** 700C, 450 and 450PWB (the lowest-bandwidth modes, which Sideband, MeshChat and Columba can send) are now decoded too, as the rest were, and play in the web UI and in most players. The pure-Rust Codec2 rettui uses didn't have them, so it's now vendored (`vendor/codec2`) with them added from Codec2's own C sources; they're checked against Codec2's reference decoder.
- **Recording voice messages in the terminal:** `v` in a conversation records one (`v` again stops it; `Alt-R` while writing), then `Enter` sends it with anything written. rettui records with a program that's installed (`parecord`, `arecord`, SoX's `rec` or `ffmpeg`) rather than a sound library, so it runs where there's none; *Voice recorder* (`voice_recorder`) names another. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#reactions-locations-commands-and-voice-messages).
- **Send voice at** (`voice_mode`): the Codec2 mode voice messages recorded in either UI go in, from 3200 (the default, the clearest) down to 1200 and 700C (a minute in 5 KB, for LoRa). One that failed goes again in the mode it was recorded in.
- **Several identities:** keep more than one LXMF address, each with its own name, conversations, contacts and settings, and switch between them, as in Columba and MeshChat. `I` in the Status tab (**👤 Identities** in the web UI) lists them: make a new one, keep one from another program's identity file (terminal), rename one, show its address as a QR code, switch (rettui saves everything and starts over as it; the web UI comes back by itself with the same login), and delete one (terminal). The first stays in the data directory; the rest are in `identities/`. See [Data and Storage](https://github.com/zevaryx/rettui/wiki/Data-and-Storage#several-identities).
- **Sensor readings:** the readings Sideband sends in its telemetry (battery, temperature, pressure, humidity, light, motion, the radio link, processor, memory, storage, tanks, fuel, power and its users' own) show under their message in both UIs, one a line, as Sideband says them; until now only a location was read. Like location updates, they're quiet, and newer ones replace the ones before. *Send sensor readings* (`telemetry_sensors`, off unless turned on) sends this computer's battery, processor load, memory and storage with its location, so Sideband shows them on your entry. See [Messaging](https://github.com/zevaryx/rettui/wiki/Messaging#reactions-locations-commands-and-voice-messages).
- **Claim a draw** in Chess and Checkers (`C` in the terminal, a button in the web UI): offered when the same position has come up three times, or after fifty moves (Chess) or forty (Checkers) without a capture or a pawn or man moving. The game ends at once; the other side checks the claim.

### Fixed
- **Reactions from Mesh-Client:** a reaction from [Mesh-Client](https://github.com/Colorado-Mesh/mesh-client) no longer shows as a message as well as on the message it's for. Mesh-Client puts the emoji in the message's text too (for clients without reactions); a reaction whose text is just its emoji is now only the reaction. ([#31](https://github.com/zevaryx/rettui/issues/31))
- **Stopping the web UI right after it starts:** a stop (Ctrl-C or SIGTERM) sent while `rettui --web` was still starting up could end it abruptly instead of shutting it down cleanly; the signals are now listened for before the link is printed.
- **Saving survives a power cut:** settings, messages, games and the rest are now on the disk before they replace the copy saved before. A power cut (or a crash of the computer) while saving could leave a file empty on some file systems, losing what was in it.
- **Saving on Windows:** settings, messages and the Reticulum config are saved by writing a new copy and putting it in place of the old one. On Windows that last step could fail with "access denied" when something else (a virus scanner, the search indexer) had opened the new copy for a moment; it's now tried again for up to a second.
- **The Games tab in a terminal one line high** no longer stops rettui.
- **Drafts stay with their conversation:** opening a conversation from a link or a notification while writing in another no longer carries the half-written message over to the one being opened.

### Security
- **Certificates through emSign's distrusted intermediates** are no longer accepted for outgoing HTTPS (update checks, map tiles): `rustls-webpki` is updated to 0.103.16, which rejects them, and `webpki-roots` to 1.0.10, which drops the emSign ECC root they chain to ([RUSTSEC-2026-0382](https://rustsec.org/advisories/RUSTSEC-2026-0382), [RUSTSEC-2026-0381](https://rustsec.org/advisories/RUSTSEC-2026-0381)). Those intermediates' private keys are known publicly.

### Docs and development
- **License file:** the repository now has a `LICENSE` file with the full text of the AGPL-3.0-or-later, the license rettui (and rsReticulum, rsLXMF and rsNomad, which it links) was already under.
- **Badges** in the README: CI, coverage, release, latest version, Docker image and license.
- **Code coverage:** a *Coverage* workflow measures the tests with `cargo-llvm-cov` on pull requests and pushes to `dev` and `main`, and fails when fewer than 85% of lines run. Desktop notifications, terminal image drawing and the self-update installer are left out of the count, since they need a desktop, a graphics terminal or a release to replace. New tests drive the web API, the terminal UI by key and mouse, the `rettui` binary itself (its commands, the web UI over HTTP and HTTPS, and the TUI in a pseudo-terminal), and rettui instances talking over a loopback Reticulum network; coverage went from 71% to about 94% of lines. Each run's report goes to [Codecov](https://app.codecov.io/gh/zevaryx/rettui), which comments on pull requests with the coverage of their changes, and the README's coverage badge comes from there.

## v1.8.0

Changes since v1.7.2: games with your contacts on a tab of their own (Tic-Tac-Toe, Four in a Row and Chess, over LRGP), faces for everyone without an icon (LXMFace), lists you can resize, runs of dots and colons shown as typed in the web UI, and dependencies updated for security advisories (zip 9, zbus 6).

### Added
- **Games:** Tic-Tac-Toe, Four in a Row and Chess with your contacts, over [LRGP](https://github.com/ratspeak/lrgp-rs) (Ratspeak's Lightweight Reticulum Gaming Protocol), so you can play against other LRGP clients too. Each move goes as a message, sent as messages to them are (direct, or through the propagation node) and resent as they are when it fails. They have a tab of their own, **Games**, between Messages and Channels (`2`; the tabs after it move up one): your games with everyone, newest first, beside the selected one's board, played by keys or clicks, with accepting, declining, draws and resigning; its badge counts the games waiting on you. `n` there (**+ New game** in the web UI) starts one, and `g` in a conversation (the **🎲 Games** button) goes to your games with them. A game's moves aren't in the conversation: the Games tab shows the games, filtered by **All**, **Active**, **Pending** or **Completed** (`f`), says how your last move went while it's being sent, and sends one that failed again (`r`, **↻ Send again**); a move that arrives notifies you as a message does, and opening it shows the game. Only moves whose signature was checked are played, and both sides check the rules. Games are kept in `games.json` and in backups. See [Games](https://github.com/zevaryx/rettui/wiki/Games).
- **Faces:** everyone without an icon of their own gets a face, [LXMFace](https://github.com/ratspeak/LXMFace)'s picture of their LXMF address (the same in every app that draws LXMFace, as Ratspeak does). Icons from other apps (Sideband, Columba, MeshChat) always come first. The web UI shows faces wherever icons go, now beside every conversation, and yours beside your name; the TUI draws them beside each conversation and on the contact card, and shows icons' colours in the list too.

- **Resizing the lists:** the list beside each tab's main pane (Messages, Games, Channels, Browser, Node, Reticulum, Hub) can be made wider or narrower: drag its edge, or press `<` and `>` (`=`, or a double click on the edge, puts it back). Each tab keeps its own width: the terminal UI in `settings.json` (`pane_widths`), the web UI in each browser. The pane beside it always keeps room, and phones (one pane at a time) are as they were. See [Using the TUI](https://github.com/zevaryx/rettui/wiki/Using-the-TUI#resizing-the-lists) and [Web UI](https://github.com/zevaryx/rettui/wiki/Web-UI).

### Fixed
- **Web UI:** a run of dots or colons (`.....`, `:::::`) no longer shows as only its last two in some browsers. The bundled Fira Code font joins `..`, `...`, `::` and the like into ligatures, which some browsers drew wrongly; ligatures are now off everywhere in the web UI, so what's typed is shown as typed (`one...two` no longer gets gaps around its dots either).

### Security
- **zip 9:** the `zip` crate before 9.0 could keep SUID/SGID bits, and follow or create symbolic links pointing outside the folder, when it extracted an archive ([RUSTSEC-2026-0367](https://rustsec.org/advisories/RUSTSEC-2026-0367), [-0368](https://rustsec.org/advisories/RUSTSEC-2026-0368), [-0369](https://rustsec.org/advisories/RUSTSEC-2026-0369)). rettui never extracted archives that way (updating on Windows reads only `rettui.exe` out of the release, by name, into memory), but it now uses zip 9 all the same.
- **zbus 6:** desktop notifications on Linux and the BSDs now use zbus 6, which took in `zvariant`, `zvariant_derive` and `zbus_names`, no longer maintained apart from it ([RUSTSEC-2026-0370](https://rustsec.org/advisories/RUSTSEC-2026-0370), [-0371](https://rustsec.org/advisories/RUSTSEC-2026-0371), [-0372](https://rustsec.org/advisories/RUSTSEC-2026-0372)). notify-rust, which still needs zbus 5, is now only built for Windows and macOS, where it doesn't use D-Bus (rettui speaks D-Bus itself on Linux). That leaves 24 fewer crates in the build.
- A yanked `yoke-derive` (0.8.3) is updated to 0.8.4.

### Docs and development
- A *Games* page in the wiki, and faces in *Messaging*.
- lrgp-rs and LXMFace are submodules in `deps/`. lrgp-rs comes from a fork (`zevaryx/lrgp-rs`, branch `rettui`) whose one change is rusqlite at rettui's version, since only one crate in a build may link SQLite.

## v1.7.2

Changes since v1.7.1: entry points found by interface discovery are connected to by rettui, with *Bootstrap only* interfaces handled as Python Reticulum does, and the Docker image is ready to run a transport node. There are also some dependency bumps to mitigate security advisories

### Added
- **Discovered entry points** (a setting, in System): rettui connects to that many entry points interface discovery finds, as Python Reticulum does. One that isn't up within 12 seconds is dropped and another tried (it's tried again after 5 minutes); interfaces marked *Bootstrap only* are disconnected once that many are up, and connected again as soon as none is. rsReticulum's own auto-connect (`autoconnect_discovered_interfaces`) drops them before the others are up, never brings them back, keeps one that never comes up, and after a restart drops them at once; it's still used where the config sets it, with the warning as before.

### Changed
- **First setup:** with discovery ticked, the getting-started guide adds the entry points as *Bootstrap only* and sets *Discovered entry points* to 2, so the public entry points are used until two found ones are up, not for good. Configs it set up before (with Reticulum's own auto-connect) are left as they are.
- **Docker:** the image exposes port 4242 too, and `compose.yaml` publishes it, for running a transport node others connect to; the [Docker page](https://github.com/zevaryx/rettui/wiki/Docker) shows the interface to add.
- **`rustls`:** Bumped to the latest 0.23 release to mitigate multiple security advisories

### Docs and development
- Interface discovery: since rsReticulum 1.3.0 (in v1.7.1), entry points that publish no location are found, and your own interfaces marked *Discoverable* are announced; the wiki no longer lists those as limits.

## v1.7.1

Changes since v1.7.0: links in chat, copying chat text in the terminal UI, addresses copied with their key, and rsReticulum 1.3.0 as released.

### Added
- **Links in chat**, in both UIs, in LXMF conversations (plain, Markdown and Micron messages) and RRC rooms: NomadNet pages and files (`<node>:/page/…`, also after `nomadnetwork://`) open in the Browser, LXMF addresses (`lxmf@`, `lxmf://`, and `lxma://` links with a key) their conversation, and RRC hubs (`rrc://hub/room`, `rrc@hub`) in Channels, asking first for a hub not added yet. In the terminal UI, a link too long for the view, broken over rows, is still one link.
- **Copying chat text in the terminal UI:** drag across a conversation or a room to select text; it's copied when you let go.
- **Copy address gives their key:** copying someone's address (the contact card, the conversation, an RRC user's menu, in both UIs) copies an `lxma://<address>:<public key>` link once their announce has been heard, so whoever you pass it to can add them and write straight away. Before that, the address alone, as before. Copying your own address in Status gives your `lxma://` link too.

### Changed
- Updated to the [rsReticulum 1.3.0](https://github.com/ratspeak/rsReticulum/releases/tag/v1.3.0) release (rettui had a pre-release). Among its fixes: a peer could crash a Link by reporting an impossible round-trip time; interface discovery publishes and keeps announcements more reliably; Bluetooth and Windows socket fixes. rsReticulum still follows the `rettui` branch of zevaryx/rsReticulum, now 1.3.0 with [ratspeak/rsReticulum#26](https://github.com/ratspeak/rsReticulum/pull/26) merged in, until that pull request is merged upstream. Building rettui needs Rust 1.89 or newer.

### Fixed
- `lxmf@` and `lxmf://` links (on a NomadNet page) didn't open a conversation from the web UI.

## v1.7.0

Changes since v1.6.1: rettui can host an RRC hub, built on [rsRRCD](https://github.com/reticulum-spb/rsRRCD)'s. Checked with RRC clients on Python Reticulum joining it over TCP, and with rettui's own. Pictures and files can be added to your NomadNet node from its page editor, and moved between folders. RRC chat lines show their date, and rooms keep as many messages as you choose, however busy they are.

### Added
- **Host an RRC hub** (a setting, or `e` in the new Hub tab): rsRRCD's hub, on rettui's own Reticulum instance, that NomadNet, MeshChatX, Ratspeak, rettui and the other RRC clients join as they join rrcd. It has an identity of its own, so people who join learn its address and not your LXMF one, and it announces itself with its name (yours, or "Hub name") and greets people with "Hub greeting". You're an operator of it, and your rettui joins it as soon as it's up. Changing its settings applies as it runs, without disconnecting anyone.
- **The Hub tab** (and the web UI's Hub section): who's connected, the rooms and the bans. Kick someone from a room, ban them from one or from the whole hub, make them an operator or voiced, or disconnect them; set a room's topic, modes and key, invite people, register it, rename or delete it, or make a registered room; lift bans. In the web UI, a room or a person opens a page of settings, made for a phone too. A console runs hub commands as typed in a room (`/stats`, `/kline list`, `/reload`...), with the hub's answers beside it.
- "Anyone makes rooms": off, only you make rooms on your hub, and others join the ones there are.
- The Status tab says how the hub is doing, and backups keep its settings, its rooms and (with your identity) its identity.
- The Network tab's announces name RRC hubs that announce their name as rrcd and rsRRCD do.
- RRC chat lines show their date with their time ("Dates on chat lines", on by default; `D` in Channels toggles it).
- "Chat messages kept" (Status › Messages): how many messages each RRC room and whisper conversation keeps, also across restarts (500 by default, 50 to 100000; lowering it drops the oldest at once).
- **Upload in the page editor** (`Alt+P`, in both UIs; in the web UI also by dropping files on the page): a picture is added to the node's `pages/images/`, made smaller for the mesh and without its metadata, and shown in the page; any other file goes to `files/`, linked to download. Nothing already there is replaced.
- **Folders on your node:** the Node tab lists `pages/` and `files/` as trees of their folders. Move anything to another folder of its root (`m`, or *Move…* and dragging in the web UI), and rename or delete pictures and files as well as pages. Links to what's moved or renamed change in every page.

### Changed
- Eight tabs: `1`–`8` switch them.
- Settings are in groups (Profile, Messages, Location, Browsing, Hosting, Display, Notifications, System), one shown at a time, in both UIs: `Tab` or `←` `→` in the terminal, tabs above them in the web UI (a group with changes not saved yet is marked).

### Fixed
- RRC rooms kept 500 lines in all, and people joining and leaving (shown or not) counted: in a busy room, messages went after not many. Now each room and whisper conversation keeps its newest 500 messages ("Chat messages kept" changes how many), with joins, leaves and notices kept apart.
- A hub's greeting that changed but starts as the old one did is shown whole when you reconnect, not without its first lines.

### Docs and development
- *Hosting a Node* has a section on hosting an RRC hub, and says how pictures, files and folders work on a NomadNet node.
- rsRRCD and rsRRC are submodules in `deps/`.
- CI checks dependencies with cargo-deny: their licences, sources and bans, and security advisories (which warn rather than fail).
- A propagation node test failed about one run in a thousand: its message with no stamp work sometimes had enough by chance. It's now always too little.

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
