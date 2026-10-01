Words you'll meet in rettui and around Reticulum, each in a line or two,
with where to read more. Reticulum's own
[manual](https://reticulum.network/manual/) explains them properly; where
it and this page differ, the manual is right.

## The network

- **Reticulum:** a networking stack that works without servers or
  central addresses, over whatever links there are: a local network, the
  internet, radios. ([Manual](https://reticulum.network/manual/understanding.html#introduction-basic-functionality))
- **Identity:** the key pair behind everything you are on the network.
  Keep it safe: lose it and your addresses are gone. Every Reticulum
  program keeps it in the same format, so it can move between them (see
  [Data and Storage](Data-and-Storage)).
  ([Manual](https://reticulum.network/manual/understanding.html#understanding-identities))
- **Address (destination):** what others reach you at, 32 hex characters.
  It comes from your identity and the program's purpose, so one identity
  has several: your LXMF address for messages, your node's address for
  pages, and so on. ([Manual](https://reticulum.network/manual/understanding.html#destinations))
- **Announce:** a signed note telling the network an address exists,
  with its public key (and, for messages, your display name). Others learn
  how to reach you from it; until you announce, they can't find you.
  ([Manual](https://reticulum.network/manual/understanding.html#public-key-announcements))
- **Hops:** how many steps away something is; each transport node on the
  way adds one. Shown beside everything in the Network tab.
  ([Manual](https://reticulum.network/manual/understanding.html#reaching-the-destination))
- **Link:** an encrypted connection between two programs, set up when
  needed: for pages, files, syncing with a propagation node and RRC.
  ([Manual](https://reticulum.network/manual/understanding.html#link-establishment-in-detail))

## Getting connected

- **Interface:** one way Reticulum reaches others: the local network
  (the Auto interface, over IPv6), a TCP connection to an entry point, a
  LoRa radio, a serial line, I2P and more. Set in the Reticulum config
  (the Reticulum tab). ([Manual](https://reticulum.network/manual/interfaces.html))
- **Transport node:** a node that passes others' traffic and announces
  on. Entry points are transport nodes. rettui isn't one unless you turn
  on `enable_transport`, which is meant for machines that stay on.
  ([Manual](https://reticulum.network/manual/understanding.html#node-types))
- **Entry point:** a transport node anyone can connect to over the
  internet, to reach the wider network. RMAP World (`rmap.world:4242`) and
  Ratspeak (`rns.ratspeak.org:4242`) are two; [directory.rns.recipes](https://directory.rns.recipes) lists more.
  ([Manual](https://reticulum.network/manual/gettingstartedfast.html#bootstrapping-connectivity))
- **Interface discovery:** entry points can announce themselves as
  discoverable; with discovery on, Reticulum hears of them through the
  connections it has, and connects to some by itself (see [Settings and Reticulum Config](Settings-and-Reticulum-Config)
  for what works so far). ([Manual](https://reticulum.network/manual/interfaces.html#discoverable-interfaces))
- **Shared instance:** the first program to start Reticulum on a computer
  opens the interfaces, and programs started after it share them. If rnsd
  or another program got there first, its config's interfaces are the ones
  in use. ([Manual](https://reticulum.network/manual/using.html#the-rnsd-utility))
- **Network name and passphrase (IFAC):** set on an interface, they
  keep out anyone who doesn't know them: a private network over a shared
  link.
  ([Manual](https://reticulum.network/manual/understanding.html#interface-access-codes))
- **RNode:** a LoRa radio running the RNode firmware, which Reticulum
  uses as an interface: no internet needed (see [RNode Radios](RNode-Radios)).

## Messages, pages and chat

- **LXMF:** the message format Reticulum messaging programs share
  (Sideband, NomadNet, MeshChat, rettui), end-to-end encrypted and
  signed. ([LXMF](https://github.com/markqvist/LXMF))
- **Propagation node:** keeps messages for people who are offline, until
  their program syncs and collects them. Propagation nodes share messages
  with each other. It can't read them, but sees who they're for (see
  [Messaging](Messaging)). ([LXMF](https://github.com/markqvist/LXMF#propagation-nodes))
- **Stamp:** a little proof of work a recipient can ask of each message,
  so sending spam costs time; contacts can be spared it with tickets (see
  [Blocking and spam](Messaging#blocking-and-spam)).
- **Paper message:** a message written as a QR code or `lxm://` link,
  to carry by hand (see [Paper messages](Messaging#paper-messages)).
- **NomadNet:** a terminal program for Reticulum whose nodes host pages
  (written in its Micron markup) and files. rettui's Browser tab opens
  them (see [NomadNet Browsing](NomadNet-Browsing)), and rettui can host a
  node too (see [Hosting a Node](Hosting-a-Node)).
  ([Manual](https://reticulum.network/manual/software.html#nomad-network))
- **RRC (Reticulum Relay Chat):** live chat rooms on hubs, closer to IRC
  than to messaging: what's said while you're away doesn't wait for you
  (see [RRC Chat](RRC-Chat)). ([Manual](https://reticulum.network/manual/software.html#reticulum-relay-chat))
