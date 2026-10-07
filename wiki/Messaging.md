The Messages tab: LXMF messages with other Reticulum users (Sideband,
MeshChat, NomadNet and other LXMF clients).

- **Conversations:** a conversation per peer, with unread counts in the
  list and the sidebar. Start one by address (`n`), from the Network tab, or
  from an `lxmf@` link on a page.
- **Pinning:** a pinned conversation stays at the top of the list, newest
  pinned first. `*` pins or unpins the open one in the TUI; in the web UI,
  it's the 📌 button above the conversation.
- **Marking all read:** `R` in the TUI's Messages tab, or **✓ All read**
  above the web UI's conversations (shown while anything is unread).
- **Drafts:** what you write (and attach) stays with its conversation.
  Opening another one doesn't carry it over, and coming back brings it back.
  A message that fails to send goes back to the conversation it was for.
  In the web UI, drafts also outlive reloading the page (which phones do to
  pages left in the background) and restarting rettui: their text, and
  what they reply to, are kept in that browser. Files attached aren't.
- **Message states:** your messages show *sending…*, ✓ (delivered), *✓ via
  propagation node*, or *failed* with the reason.
- **Delivery modes:**
  - **Auto** (default): tries direct delivery first. If that fails, it hands
    the message to your propagation node.
  - **Direct:** delivers over a Link and waits for the recipient's proof.
  - **Propagated:** always goes through the propagation node.
  - **Paper:** isn't sent. It's written as a [paper message](#paper-messages)
    to pass on some other way.
  - Each conversation keeps its own mode: `d` (or `Ctrl-P` while writing)
    in the TUI, the menu beside **Send** in the web UI. Paper is for one
    message; the next goes their usual way again. The contact card shows a
    mode other than auto.
- **Stamps and ratchets:** if the recipient's announce asks for a stamp,
  rettui generates it (or uses a ticket they gave you). It also generates
  the propagation node's stamp, and encrypts to the recipient's ratchet
  when one is known. Your own announces carry a ratchet too, so messages
  to you have forward secrecy (see [Data and Storage](Data-and-Storage)).
  To ask for stamps yourself, see
  [Blocking and spam](#blocking-and-spam).
- **Propagation nodes:** the ones you hear are listed in the Network tab.
  Pick one with `p`. You can also [host one](Hosting-a-Node#propagation-node),
  or let rettui pick one (below). rettui syncs every `sync_interval_mins`
  (120, two hours, by default; 0 for never; *Sync every* in Status)
  or whenever you press `S`. Settings that still had the old default of
  30 minutes move to two hours; an interval of your own stays. A sync identifies you to the node, downloads your
  messages, then tells the node to delete them. It downloads them 512 KB
  at a time, as many times as it takes, each lot cleared from the node as
  it arrives, so a slow link isn't held for long by one transfer. A
  message bigger than that (a node can be set to take them), within your
  *Largest message* limit, comes in a transfer of its own.
- **Attachments:**
  - The first image goes in the LXMF image field, which Sideband and MeshChat
    show inline. Other files are sent as file attachments.
  - Pictures are made smaller before they're sent, as Sideband and MeshChat
    do. A photo straight from a phone is megabytes, which takes minutes over
    a radio link, if it gets there at all: propagation nodes usually take
    256 KB at most. *Send pictures at* (Status, `picture_size`) picks how
    small:
    - **small:** at most 480 pixels on the longest side, about 20–40 KB. For
      LoRa.
    - **medium** (the default): 1024 pixels, about 100–200 KB.
    - **large:** 2048 pixels, often over 500 KB, too big for most
      propagation nodes.
    - **original:** as they are.

    A picture is turned the way its camera noted and sent as a JPEG (a PNG
    if it's partly see-through). That leaves out its metadata, such as where
    a photo was taken. GIFs go as they are, since they may move, and so does
    a picture that wouldn't get smaller. The picture on your computer isn't
    changed: the smaller copy is saved in `uploads/` (see
    [Data and Storage](Data-and-Storage)), and goes when the message is
    deleted. While you write, a picture to be shrunk shows *(smaller)* in
    the TUI, and *sent smaller* in the web UI.
  - Received attachments are saved automatically, and images get an inline
    preview. `o` opens the newest attachment with your desktop's default app.
  - Many clients refuse direct transfers over about 1 MB, and rettui warns
    before sending that much.
- **Signatures:** incoming messages are checked against the sender's key. If
  the key isn't known, rettui looks it up first (up to 10 s). Messages that
  still can't be checked are marked *unverified*. A message whose signature
  doesn't match its sender's known key was written by someone else: it's
  dropped, and the log says so (as Python LXMF does).
- **Peers who are offline:** public keys from announces are kept, so you can
  write to a peer you heard earlier even while they're away.
- **Announces:** rettui announces your LXMF address when it starts (unless
  you turn that off), on its own every so often, and whenever you press
  `A`. *Announce on its own* (Status, `announce_schedule`) is:
  - **random** (the default): at a random time between *Random: from* and
    *Random: to* (`announce_random_min_mins` and `announce_random_max_mins`,
    60 and 360 minutes), picked again after each announce, as Sideband
    does, so announces don't fall in step with everyone else's;
  - **fixed:** every *Fixed: every* minutes (`announce_interval_mins`, 360);
  - **off:** only at start and when you announce.

  All of them stay between one hour and six (more often, and public
  gateways hold your announces back).
  Settings from before this keep their choice: an interval of 0 is off,
  and one of your own (other than 360, the old default) is fixed.
  An announce only reaches those
  connected when it goes out, so when an interface comes online later (an
  entry point that connects late, or one interface discovery connects
  to), rettui announces again, as Sideband does. One that comes back
  within half an hour of an announce going out on it doesn't bring
  another: public gateways hold back destinations that announce too
  often. With announcing at start and on its own both off, it doesn't.

## Picking a propagation node automatically

*Pick propagation node automatically* (ticked in the getting-started
guide the first time it opens, with a warning under it; otherwise off
unless you turn it on) picks one
for you, as Sideband does when none is set:

- **Which:** of the propagation nodes heard announcing in the last day that
  are serving and ask a stamp cost of 20 or less, the four nearest by hops
  (and the one picked before) are probed: a Link is set up to each and
  timed, like a [ping](#contacts). Of those that answer about as fast as
  the fastest, the nearest is picked. Your own node, if you host one, is
  left out: it doesn't pass messages on to other nodes.
- **Hops are only a hint.** They travel outside the announce's signature, so
  any transport node on the way can make a node look closer than it is.
  Nothing can make a node answer faster than it really does, so the
  measured time decides.
- **Keeping it:** the node picked is checked again every 6 hours, and soon
  after syncing with it or sending through it fails. It's replaced only if
  it stops answering or another answers in under half its time. The Status
  tab shows which node is in use and how it was picked; picking one by hand
  (`p` in the Network tab, or the propagation node setting in either UI)
  turns automatic picking off.
- **Warning:** anyone can run a propagation node near you, for instance on
  the same public entry point. The node picked can't read or change your
  messages (they're encrypted for their recipients and signed), but it sees
  who your propagated messages are for and when you collect yours, and it
  could lose them. Where you can, pick a node you trust instead.

## Searching

Find messages by what they say: every word typed must be found, in any
order, in a message's text, title, attachments' names or what rettui noted
of it. It looks through the messages each conversation keeps; older ones
moved to the archive open from their conversation (see
[Data and Storage](Data-and-Storage#message-history)). Results are newest first (the newest 200 at most), each with who
it's with, when, and the part that matched.

- **In the TUI:** `/` in the Messages tab opens the search over all
  conversations. `Tab` keeps it to the open conversation (or goes back to
  all), `↑↓` pick a result, and `Enter` opens its conversation scrolled to
  the message, picked. `Esc` closes it; a click on a result opens it.
- **In the web UI:** type in **Search messages** above the conversations
  (or press `/`). **In <name>** keeps it to the open conversation, and a
  click on a result opens it there, loading the older messages first if
  it's further back. `Esc` (or emptying the box) shows the conversations
  again.

## Message actions

- **Retry:** a message of yours that failed can be sent again, as the same
  message (the same hash), so a copy that got through anyway isn't shown
  twice. It goes by the delivery mode chosen now. Its files must still be
  where they were.
- **Sent again when they announce:** as in MeshChat, when someone
  announces, your messages to them that failed go again on their own (by
  how messages to them go), since the announce shows they can be reached.
  Text ones only, as a file can be large; not a paper message that
  couldn't be written (it would go over the network instead); and a
  message isn't sent again so for ten minutes after the last time, however
  often they announce. Turn it off with *Resend when they announce*
  (Status, `resend_on_announce`).
- **Forward a message:** its text and its files go to another conversation
  (or to an address typed in full) as a new message of yours, by how
  messages to them go. The files are copied, so deleting either message
  leaves the other's. In the TUI, pick it and press `f`, then type part of
  a name or address and press `Enter`; in the web UI, it's *Forward…* in
  the **⋯** beside it.
- **Export a conversation:** all of it, archived messages too, as a text
  file: who wrote what and when, with the names of files, locations and
  reactions. `E` in the TUI's Messages tab writes it to `exports/` in the
  downloads folder (and says where); **Export as text** in the web UI's
  **Contact** dialog downloads it.
- **Delete a message:** it goes from rettui, with any files rettui saved
  for it (received files, and ones uploaded in the web UI). A file you sent
  from elsewhere on your computer stays where it is. It isn't deleted from
  the other side.
- **Delete a conversation:** all its messages go, including those moved to
  the archive, and the files rettui saved for them. Your name for the
  contact and your notes stay (they're about the person, not the
  conversation).

In the TUI, pick a message (`m`, or click its name line) and press `t` to
retry or `x` to delete; `X` deletes the open conversation. In the web UI,
**↻ Retry** shows on a message that failed, and the **⋯** beside a message's
buttons has *Delete*. *Delete conversation* is in the **Contact** dialog.
Both UIs ask before deleting.

## Contacts

Each person you message can have a name of your own for them, shown instead
of the one they announce (in the conversation list, notifications and
everywhere else), and notes for you alone.

- **In the TUI:** `c` opens the open conversation's contact card: who they
  are, the name they announce, their address, when they were last heard
  and how far away, and your notes. `r` renames them (empty goes back to
  their own name), `e` edits the notes (line breaks show as ↵ while
  editing), `y` copies their address, `p` pings them, `X` deletes the
  conversation, and `Esc` closes the card. Its buttons can be clicked too.
- **In the web UI:** the **Contact** button above the conversation.
- **Ping:** rettui finds a path to them and times setting up a Link to
  their LXMF address (closed straight away; nothing is sent over it), then
  shows how long it took and how many hops away they are, and how well
  their answer was heard (RSSI, SNR, link quality) when it came in over a
  radio that reports it, such as an RNode. Any LXMF client
  answers, while it's running. The card keeps the last answer; `rettui
  ping <address>` does it from the command line.
- **Sharing your address:** `c` in the Status tab (TUI), or **QR code** next
  to your address (web UI), shows it as a QR code: an `lxma://` link with
  your address and public key, the way Columba shares contacts. Someone who
  scans it can write to you before hearing your announce. `rettui address
  --link` prints the link.
- **Icons:** Sideband, Columba and MeshChat let people pick an icon (a
  [Material Design Icon](https://pictogrammers.com/library/mdi/) in a colour
  on a colour), which comes with their messages. rettui keeps the newest
  one each person sent. The web UI shows it beside their name in the
  conversation list and above the conversation; once anyone has one, the
  rest get their first letter. The TUI's contact card names it, in its
  colours.
  - **Your own:** *Icon* in the settings (Status), with *Icon colour* and
    *Icon background*. Type part of a name in the web UI and pick from the
    icons that match; in the TUI, type the name. Once set, it goes with
    every message you send (not paper ones), as Sideband sends its own, and
    shows beside your name in the web UI. Empty, the default, sends none.
- **Adding someone from theirs:** paste their `lxma://` link where you'd type
  an address (`n` in the TUI, **+ New** in the web UI), or read its QR code
  like a [paper message](#paper-messages) (a picture of it, or the camera in
  the web UI). rettui checks the key belongs to the address, keeps it, and
  opens the conversation.

## Blocking and spam

- **Unknown senders:** someone who isn't a contact (you haven't trusted
  them, and you've never written to them) is marked with a **?** in the
  conversation list, and their conversation asks what to do with them, as
  NomadNet's Untrusted list does:
  - **Trust** them: they're spared the stamp cost (below), and given
    tickets.
  - **Leave as is:** their messages are taken, but they aren't trusted.
    The question goes away.
  - **Block** them (below).

  *Unknown senders* (Status, `unknown_senders`) decides what becomes of
  their messages:
  - **show** (the default): as above, like anyone's, marked `?`.
  - **requests:** kept aside as *message requests*, as Signal does: listed
    last, under **Message requests** in the web UI and marked *request* in
    the TUI, with no notifications and not counted as unread. Trusting
    them, leaving them as they are, or replying makes it a conversation
    like any other; blocking them, or deleting it (the web UI's request
    has **Delete** beside **Block**), gets rid of it.
  - **ignore:** dropped altogether, as Sideband can. A settings file that
    had *Ignore unknown senders* on reads as this.

  Anyone you've written to, trusted, or left as is always gets through.
- **Blocking** someone drops their messages, deletes the conversation with
  them, and blocks their identity in Reticulum (its blackhole list), so
  their announces and traffic are dropped too, as NomadNet does. If rettui
  uses a shared instance (rnsd, NomadNet), that instance blocks them, for
  every program using it. Blocked contacts are listed under the Network
  tab's *Blocked* filter, where they can be unblocked; an unblocked contact
  is an unknown sender again. Blocking needs their identity: if it isn't
  known (no announce heard), rettui still drops their messages and says
  so in the log, and blocks them in Reticulum the next time it starts and
  knows it.
- **Stamp cost** (`stamp_cost`, 0 by default): proof of work asked of
  anyone who isn't a contact, announced with your address as Python LXMF
  does. Their client works out the stamp before sending (a cost of 8 takes
  a moment, 16 a lot longer); messages without a valid one are dropped and
  the log says so. Contacts (trusted, or written to) are spared it.
  Trusted contacts are also given a **ticket** with your messages (LXMF's
  ticket field, at most once a day): their client stamps later messages
  with it instead of doing the work, which matters to those who aren't in
  your contacts on their own side. Tickets they give you are used for your
  messages to them in the same way. rettui keeps tickets in
  `tickets.json`, using rsLXMF's ticket store.
- **Largest message** (`max_message_kb`, 1000 by default, as LXMF's own
  default; NomadNet uses 500): bigger direct transfers are refused before
  they're downloaded, and bigger messages from a propagation node are
  dropped. 0 takes any size.

In the TUI, the contact card (`c`) has *Trust*, *Leave as is* and *Block*
(`t`, `l`, `b`), and `b` on an LXMF peer in the Network tab blocks or
unblocks them. In the web UI, the question shows above an unknown sender's
messages, the **Contact** dialog has *Trust* and *Block*, and the Network
tab has *Block* and *Unblock* on LXMF peers.

## Reactions, locations, commands and voice messages

Other LXMF clients send some messages with no text, only an LXMF field.
rettui shows what each one is, rather than an empty message:

- **Reactions** (LXMF field `0x40`, as Columba and MeshChatX send them) show
  under the message they react to, with who reacted. A reaction that
  arrives before its message waits for it. A reaction to one of your
  messages gets a notification; it isn't counted as unread.
- **Locations** (Sideband and Columba telemetry, field `0x02`) show as a
  line with the coordinates. Click it (or open it, below) to see it on
  OpenStreetMap, or on [the map](#locations-and-the-map). Location updates
  don't notify you or count as unread, and a newer one replaces the one
  before it while nothing else was said in between, so a shared location
  doesn't fill the conversation.
- **Commands** (Sideband's, field `0x09`), such as asking for your location
  or a ping, are shown in words, and don't notify you.
  - *Answer commands* (Status, `answer_commands`) answers pings, echoes and
    signal report requests as Sideband does: "Ping reply", "Echo reply:"
    and the text, and, for a signal report, "No reception info available"
    (rettui isn't told how well a message was heard). It's off by default;
    *trusted* answers only contacts you trust, *contacts* any contact. The
    answers go as messages, shown in the conversation, at most once a
    minute to each sender; the command's line says whether, or why not.
  - Requests for your location are answered only if *Location requests*
    says so (see [below](#locations-and-the-map)). Plugin commands aren't
    run.
- **Voice messages** (field `0x07`) are saved with the attachments, and
  play in the web UI and in most players (`o` in the TUI opens them):
  - Opus recordings are saved as `.ogg` files, as they came.
  - Codec2 recordings (the low-bandwidth modes MeshChat, Columba and
    Sideband can send) are decoded when they arrive and saved as `.wav`
    files (8 kHz mono). The 3200, 2400, 1600, 1400, 1300 and 1200 modes are
    decoded, up to ten minutes; 700C, 450 and 450PWB aren't yet, so those
    are saved as they came (`.codec2`) and marked as not playable.
  - **Recording one** (web UI): the 🎤 button beside **Attach** records
    from the microphone; press it again (⏹ shows how long so far) to stop,
    listen to it, then **Send** it, with text if you like. It goes as
    Codec2 at 3200 bit/s (about 400 bytes a second, so a minute is 24 KB:
    light enough for LoRa), which Sideband, MeshChat and Columba play; you
    hear what they will. Up to five minutes. Browsers only let a secure
    page use the microphone: open the web UI at `localhost`, or over
    [HTTPS](Web-UI). The TUI can't record, but sends audio files as
    attachments.
  - **Voice calls** (Sideband's, over LXST) aren't something rettui can
    make or answer. LXST, the library they use, is published under
    CC BY-NC-ND 4.0, which doesn't allow adaptations of it to be shared,
    and there's no separate description of its protocol to build one
    from. Voice messages work with the same clients.
- A message with nothing rettui can show says so, naming the fields it
  carried, without a notification.

## Locations and the map

You can share a location as Sideband does (LXMF's telemetry field, with
the time, latitude, longitude and, if known, height and accuracy), so
Sideband and Columba show it on their maps:

- **In the TUI:** `L` in a conversation asks for a location (latitude,
  longitude, such as `51.5074, -0.1278`, or a `geo:` link), filled in with
  this station's *Location* (Status, `location`) if it's set; `Enter`
  shares it.
- **In the web UI:** the 📍 button beside the microphone (or `L`) offers
  where the device is (the browser asks you first; browsers only say on a
  secure page, so open the web UI at `localhost` or over [HTTPS](Web-UI)),
  this station's *Location*, or one typed.

A location goes as its own message, the way the conversation's messages go
(not on paper: a paper message carries text only), and shows in the
conversation like anyone's. It's sent once, unless you share it live:

- **Sharing live** (as Columba does): updates for 15 minutes, an hour,
  eight hours or until you stop, then a message saying you've stopped,
  which Columba shows (and rettui does too). Each update replaces the one
  before it in the conversation, so a live share is one message there,
  and its header says *📍 live until …* with **Stop** (web UI).
  - **In the web UI:** **📍 Share live** in the share dialog shares where
    the device is, as it moves: rettui sends it at most once a minute, and
    only while a page of rettui is open on that device (with no page open
    for five minutes, updates stop until one is). On a page that can't say
    where the device is (not HTTPS or localhost), it shares this station's
    *Location* instead.
  - **In the TUI:** `L`, then `live 15m`, `live 1h`, `live 8h` or `live on`
    (until stopped), shares this station's *Location* every few minutes.
    `L` again stops it.
  - Live shares last while rettui runs: stopping rettui stops them (without
    the message saying so).

*Location requests* (Status, `location_requests`) answers Sideband's
requests for your location with this station's *Location*: *off* (the
default), *trusted* (only contacts you trust) or *contacts* (any contact).
As with *Answer commands*, the answer is a message in the conversation, at
most one a minute to each sender, and the request's line says whether it
was answered, or why not. With no *Location* set, nothing is sent.

The **map** shows everyone's newest location, the newest first, and this
station's if it's set. Someone who said they'd stopped sharing (as Columba
does) isn't on it, nor is anyone blocked.

- **In the TUI:** `M` in Messages opens it, on the open conversation's
  place if they shared one. It draws the coastlines when it's zoomed out
  far enough to show them, with each place on it, and lists them with how
  long ago and how far from this station. `↑↓` pick a place, `+`/`-` (or
  the mouse wheel) zoom in on it and out, `0` shows them all again,
  `Enter` opens the conversation, `o` opens the place on OpenStreetMap,
  `Esc` closes it.
- **In the web UI:** 🗺 beside **+ New** (or `M`), or **Map** beside a
  location in a conversation. Drag it, zoom with the wheel, a double click,
  two fingers or `+`/`−`, and `⤢` shows them all. A click on a place or in
  the list shows it; the list also opens the conversation, or the place on
  OpenStreetMap. How far each may be off shows as a circle around it.

The web map's pictures (tiles) come from *Map tiles* (Status, `map_tiles`):
OpenStreetMap's by default, or any other address with `{z}`, `{x}` and
`{y}` in it. rettui fetches them itself, so a phone with no internet of its
own still gets them as long as the computer running rettui has it, and the
page loads nothing from elsewhere. They're kept a month in `map-tiles/`
(up to 200 MB, the oldest going first), so places looked at before still
show without the internet. The tile server learns which parts of the map
are looked at, as with any online map; with *Map tiles* empty, nothing is
fetched, and the places are drawn on a plain grid.

To react, pick a message and choose an emoji:

- **In the TUI:** `m` picks the newest message, and `↑↓` pick another (or
  click a message's name line). The picked message shows its buttons: `r`
  reply, `e` react, `y` copy its text, `o` open its file or location. `e`
  opens the emoji picker; `Enter` sends the reaction. `Esc` puts the
  message down.
- **In the web UI:** hover over a message (on a phone the buttons always
  show) and click **🙂 React**.

A reaction is sent as an LXMF message with no text and the reaction field,
the way Columba does, so clients that don't know reactions (Sideband,
NomadNet) may show it as an empty message. A reaction that failed to send
shows as *failed*: react with the same emoji again (or click it, in the web
UI) to send it again.

## Paper messages

A paper message is an LXMF message that travels outside Reticulum: as an
`lxm://` link or its QR code. You can print it, show it on a screen, or send
the link any way you like. It's signed and encrypted for the recipient like
any other message, so only they can read it. The format is Python LXMF's, so
Sideband and NomadNet can read what rettui writes, and rettui can read theirs.

- **Writing one:** pick the *paper* delivery mode (`d` or `Ctrl-P` in the
  TUI, *Paper (QR code)* in the web UI), then write the message.
  - rettui only needs the recipient's key, from an announce heard at some
    point. They don't have to be online.
  - Paper messages carry text only, up to about 1,800 characters.
  - When the message is ready, its QR code opens:
    - **TUI:** `y` copies the link, and `s` saves the QR code as an SVG
      image in the downloads folder, ready to print. If the window is too
      small for the code, rettui says so, and you can still copy or save it.
    - **Web UI:** *Copy link*, *Share* (on phones), and *Print*, which
      prints the code alone.
  - The message then shows *✓ paper message*. `P` in the TUI, or its *QR
    code* button in the web UI, opens the code again.
- **Reading one:** rettui checks the message is for you, decrypts it, and
  checks the signature like any other message. It then goes into the
  sender's conversation, which opens.
  - **TUI:** press `p`, then paste the link or give the path to a picture of
    the QR code. You can also paste an `lxm://` link straight into the
    Messages tab.
  - **Web UI:** *Read paper*, beside *+ New*. Paste the link, or scan the
    code:
    - *Scan* uses the camera, in browsers that can read QR codes themselves
      (Chrome on Android and macOS). The page has to be secure (https or
      localhost).
    - *Scan (take a picture)*, or *From a picture*, works in every browser:
      take a photo or pick a picture, and rettui finds the code in it.
  - A message you've already read in is only reported, not added twice.

## Formatting (Markdown)

LXMF's renderer field says how a message's text is written. Sideband and
NomadNet compose in Markdown; rettui does too.

- **What you write** is marked as Markdown, so clients that format it show
  `**bold**`, `*italic*`, `~~struck~~`, `` `code` ``, lists, quotes (`>`),
  headings (`#`), code blocks and links formatted. Turn *Write in Markdown*
  off in the settings to send plain text. `rettui send` sends plain text.
- **What you get** marked as Markdown shows formatted in both UIs, and so
  does a message marked as Micron (NomadNet's page markup). Line breaks are
  kept, as in a chat. Previews in the conversation list and notifications
  leave the markup out.
- **Safety:** HTML in a message is shown as text, never run, and in the web
  UI only web and mail links can be clicked (others show their address).

## Replies

A reply names the message it answers, and the start of that message's text
goes with it, so the other side sees what it answers even without that
message. It's LXMF's own reply format, as Columba and MeshChatX send it:
replies between them and rettui show as replies both ways. Clients that don't
support replies yet (Sideband, NomadNet, MeshChat) show an ordinary message.

- **In the TUI:** `r` replies to the newest message they sent, and so does
  `Ctrl-R` while writing. `↑↓` then pick another (the one chosen is marked
  in the history), and `Esc` stops replying. To reply to a message further
  back, pick it (`m`, or click its name line) and press `r` or click
  **↩ Reply**.
- **In the web UI:** hover over a message (on a phone it's always shown) and
  click **↩ Reply**. The box shows what you're replying to; `Esc` or **×**
  stops replying.
- **Showing them:** a reply shows the first line of what it answers, under
  who wrote it. That's taken from the message itself when it's here, and
  from what the reply quoted when it isn't (it's older than the messages
  kept, or was never received). Click the quote to scroll to the message.

A message you send can be replied to once it's sent: replies name it by its
hash, which it has from then on. What you're replying to stays with the
conversation's draft.

## Emoji

Both UIs have the same two ways to add an emoji to a message, and to what you
write in a channel:

- **The picker:** `Ctrl-E` (or the 🙂 button in the web UI) opens it, at the
  emoji you used lately (both UIs share the list). Type to search by name or
  shortcode, or go through the groups (`Tab` in the TUI, the tabs in the web
  UI). `Enter` or a click puts the chosen one in, and in the web UI
  Shift-click keeps the picker open for another.
- **By name:** type `:` and the start of a name, such as `:dra`, and a list of
  the emoji it could be opens above the box. `↑↓` choose, `Tab` or `Enter`
  puts one in, and `Esc` closes the list. Typing the whole name, such as
  `:dragon:`, turns it into 🐉 right away. Names are GitHub's shortcodes
  (`:+1:`, `:tada:`); words of an emoji's Unicode name find it too.

The emoji are Unicode's, up to Unicode 15.0 (2022): many systems still draw
newer ones as boxes. In the web UI, `Ctrl-E` isn't used on a Mac or an
iPhone, where it moves to the end of the line and `Ctrl-Cmd-Space` opens the
system's own picker.
