`rettui --web` runs the same client in a browser instead of the terminal. All
seven sections work as they do in the TUI and update live as messages and
announces arrive. It prints a link to open:

```text
rettui web UI: http://127.0.0.1:8740/?token=…
```

- **Logging in:** the link logs that browser in with a cookie, and stays valid
  across restarts. Keep it private. Anyone with the link can read and send your
  messages as you. The login page also takes the token (the part after
  `token=`) pasted in.
- **Installing:** phones and desktop browsers can install the web UI as an
  app of its own (on an iPhone, Share → Add to Home Screen), which opens full
  screen. An iPhone's Home Screen app keeps its own login: paste the token
  there once.
- **Connection:** if the live connection to rettui drops (a phone asleep, a
  network change, a restart), a banner says so until it's back, and the page
  catches up.
- **Scripts and proxies:** send the token in a header instead of using the
  link: `Authorization: Bearer <token>`, or `X-Rettui-Token: <token>` if
  `Authorization` is already taken (for example by a proxy's own login). The
  token is in `web_token` in the data directory, and the JSON API is under
  `/api/`:

  ```sh
  curl -H "Authorization: Bearer $(cat ~/.local/share/rettui/web_token)" \
       http://127.0.0.1:8740/api/state
  ```

- **Address:** it listens on 127.0.0.1 by default. Pass an address such as
  `--web 0.0.0.0:8740` to reach it from other devices, and prefer a VPN or SSH
  tunnel over exposing it. It uses plain HTTP unless started with `--https`.
- **HTTPS:** browsers keep some things for secure pages: notifications,
  background notifications, installing the app, and the camera and
  microphone. A phone on the same network reaching rettui at
  `http://192.168.…` gets none of them. Either put a proxy that serves HTTPS
  in front of rettui (below), or start it with HTTPS of its own (it stays
  plain HTTP otherwise):
  - **`--https`:** with rettui's own certificate. rettui makes a small
    certificate authority of its own (once, kept in `web-tls/` in the data
    directory) and a certificate it signs for this computer's names and
    addresses (made again when they change, and each year). It prints
    them, the link, and the authority's fingerprint. Browsers warn about the
    page until the device trusts that authority: download it from
    `/rettui-ca.crt` (or **Download** beside *Certificate* under Status) and
    install it:
    - **Android:** Settings → Security → Encryption & credentials → Install
      a certificate → CA certificate.
    - **iPhone:** open the download, install the profile under Settings,
      then turn it on under General → About → Certificate Trust Settings.
    - **A computer:** in the browser's or the system's certificate settings.

    A device that installs it trusts certificates rettui signs, for any
    address, so only install it on your own devices, and keep `web-tls/`
    private like the rest of the data directory.
  - **`--tls-cert FILE --tls-key FILE`:** with a certificate you have, in
    PEM files (the certificate with its chain, and its key), such as one
    from `tailscale cert`, mkcert or Let's Encrypt.

  Either way, a plain `http://` request to the port is sent on to its
  `https://` address, and the login cookie is only sent over HTTPS.
- **Behind a reverse proxy:** it works behind one with nothing to set,
  at a site's root or under a path such as `https://example.com/rettui/`
  (all its addresses are relative). Keep rettui on 127.0.0.1, and open the
  proxy's address with the same `?token=…`. Through a proxy that serves
  HTTPS, a phone gets what browsers keep for secure pages: notifications,
  background notifications, installing the app, and the camera and
  microphone. For example:
  - **Caddy:** `reverse_proxy 127.0.0.1:8740`, or under a path,
    `handle_path /rettui/* { reverse_proxy 127.0.0.1:8740 }`.
  - **nginx:** `location /rettui/ { proxy_pass http://127.0.0.1:8740/; }`.
    The live connection isn't buffered (rettui asks nginx not to). nginx
    refuses uploads over 1 MB unless `client_max_body_size` is raised (to
    `64m`, rettui's own limit); the web UI says so when it happens.
  - **Tailscale:** `tailscale serve --bg 8740` serves it at your machine's
    `https://…ts.net` address, with a certificate browsers trust, to your
    tailnet only.
- **From a phone:** listening beyond 127.0.0.1, rettui also prints the
  login link at this computer's network address, and its QR code (in a
  terminal wide enough), so a phone on the same network can scan it
  rather than type the token. Like the link, the QR code logs in whoever
  scans it. In Docker it's left out: the container's address isn't the
  one other devices reach (use the host's).
- **Browsing:** NomadNet pages are rendered to HTML by rettui, with links and
  forms handled by the web UI and no scripts allowed. `/file/` links download
  through the browser.
- **Slow links:** live updates stay light, so the web UI is usable over a
  slow or distant connection:
  - **Only what changed:** the interface counters, which change every few
    seconds, refresh only the sidebar and Status, not the rest of the page.
    Announces, several a second on a busy network, refresh the Network list
    and the Browser's nodes at most every 2 seconds, and nothing else.
  - **Only what shows:** a conversation loads its newest 100 messages, a
    room its newest 200 lines, and the Browser's node list the 200 nodes
    heard most recently, until you ask for more (its search finds any).
  - **Compressed:** answers are gzip-compressed.
  - **Cached:** the script and stylesheet are fetched again only when they've
    changed. The font's text (about 90 KB for each weight) loads on the first
    visit, and Nerd Font's icons (about 1 MB) only for a page that shows
    one.
  - **For scripts:** `?last=N` on `/api/conversations/<address>` and on
    `/api/channels/<hub>/room` asks for just the newest N, and `total` or
    `total_lines` says how many there are in all.
- **Keys:** `1`–`7` switch sections, `/` searches the Network list (in Messages,
  [the messages](Messaging#searching); in the Browser, its nodes and saved pages), and `Esc` leaves a text box. In the message box and the channel input, `Ctrl-E`
  opens the [emoji](Messaging#emoji) picker (not on a Mac or an iPhone).
- **Phones:** on a phone (or any window up to 760px wide, and phones held
  sideways) the web UI changes layout. Larger screens keep the side-by-side
  layout:
  - **Drawer:** the sections are in a drawer, opened with ☰ in the top bar. A
    dot on ☰ means something is unread in another section. Picking a section,
    tapping outside the drawer, swiping it left or `Esc` closes it.
  - **One pane at a time:** a section shows its list (conversations, rooms,
    saved pages, node pages, config sections), then what you open from it,
    full screen. ← in the top bar, or the phone's back button, goes back. A
    room's members are a pane of their own (**Members**).
  - **Larger text and targets:** text is bigger, and rows and buttons are
    easier to tap. Messages show as bubbles, yours on the right, and Network
    rows are cards.
  - **Fewer buttons:** less used buttons (such as Copy link, Forget, Rename,
    Delete) wait behind ⋯. It and a user's actions open as a sheet from the
    bottom of the screen.
  - **Swipes:** swipe right from a list to open the drawer, and left to close
    it. Swipe right from what you opened to go back to the list, and left in
    a room to see its members. Swipe a sheet down to close it. A swipe
    follows your finger, finishes once it is a third of the way (or
    flicked), and springs back otherwise. Swipes are left alone in text
    boxes, in rows that scroll sideways and at the very edge of the screen,
    where the phone has its own back gesture.
- **Font:** all text uses Fira Code Nerd Font, which rettui serves itself
  (browsers never fetch fonts from a third party).
- **Stopping:** Ctrl-C or SIGTERM stops it cleanly, leaving hubs and saving
  everything.
- The TUI and the web UI can't use the same data directory at the same time.
