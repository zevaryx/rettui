`rettui` starts the terminal UI: seven tabs (Messages, Channels, Network,
Browser, Node, Status, Reticulum) with keyboard and mouse control. Terminals
narrower than 110 columns get a sidebar of icons and narrower side lists, so
80×24 works well.

## Keys

| Tab | Keys |
| --- | ---- |
| All tabs | `1`–`7` switch tab, `A` announce, `S` sync with the propagation node, `Ctrl-L` redraw the screen, `q` / `Ctrl-C` quit |
| Messages | `↑↓` pick a conversation, `Enter` write, `n` new conversation by address, `y` copy the peer's address, `a` attach a file, `o` open the newest attachment, `d` cycle delivery mode, `N` turn the conversation's notifications off or on, `PgUp/PgDn` scroll a page, `Home`/`End` the oldest or newest |
| Writing a message | `Enter` send, `Esc` stop writing, `Ctrl-V` paste, `Ctrl-O` attach, `Ctrl-X` clear attachments, `Ctrl-P` cycle delivery mode |
| Channels | `↑↓` pick a hub, room or whisper conversation, `Enter` write (text or `/commands`), `n` add a hub, `c` connect or disconnect, `a` toggle auto-connect, `J` show or hide joins and leaves, `N` next notification choice for the hub, room or whisper conversation, `x` leave a room, close a whisper conversation, or remove a hub, `y` copy an `rrc://` link, `m` message a user in the room, `PgUp/PgDn` scroll a page, `Home`/`End` the oldest or newest |
| User menu (click a name, or `m`) | `w` open your whisper conversation, `l` LXMF message, `Enter` pick (also copy their LXMF address or identity), `Esc` close |
| Network | `↑↓` select, `y` copy the selected address, `Enter` message a peer, browse a node, or pick a propagation node; `p` use the selected propagation node; `f` filter; `/` search by name or address (`Enter` done, `Esc` clear) |
| Browser (both panes) | `←`/`→` move between the node pane and the page, `t` switch Saved/Nodes, `g` go to an address, `y` copy the current address, `s` save the current page, `b` back, `r` refresh from the network, `R` clear the whole cache, `I` identify to this node (toggle), `H` home, `u` view the page's Micron source (toggle), `Esc` cancel loading |
| Browser, node pane | `↑↓` select, `Enter` open beside the list, `x` remove a saved page |
| Browser, page | `Tab`/`Shift-Tab` move between links and fields, `Enter` follow a link or edit a field, `L` copy the selected link, `Y` copy the whole page (the raw source when viewing source), `Ctrl-V` paste into the selected field, `Esc` clear the selection or leave the source view, `↑↓` / `PgUp`/`PgDn` scroll |
| Node | `Enter` edit the selected page, `n` new page, `r` rename, `x` delete, `h` start or stop hosting, `a` announce the node, `b` open it in the Browser, `y` copy its address, `p` switch view (editor and preview, editor only, preview only) |
| Editing a page | `Ctrl-S` save (live on the node), `Esc` back to the pages, `Alt` + the underlined letter formats (see the ribbon), `Shift`+arrows select, `Ctrl-A` select all, `Ctrl-Z` / `Ctrl-Y` undo and redo, `Ctrl-P` switch view, `Ctrl-V` paste; with the preview alone, `↑↓` / `PgUp` / `PgDn` scroll it |
| Status | `↑↓` select a setting, `Enter` edit it (or toggle), `e` edit display name, `y` copy your LXMF address, `Ctrl-R` restart Reticulum |
| Reticulum | `Tab` sections / options, `↑↓` select, `Enter` edit (toggles flip, choices open a list), `d` back to the default, `a` add an interface, `Space` enable or disable it, `r` rename, `x` delete, `t` edit the file as text, `R` reload the file, `Ctrl-R` restart Reticulum |
| Reticulum as text | `Ctrl-S` save (refused while the file can't load), `Esc` close, `Ctrl-Z` / `Ctrl-Y` undo and redo, `Ctrl-V` paste |

The footer confirms what keys did: ✓ (green) when something is done, ! (yellow)
when it can't be done right now, and ✗ (red) when it failed. Problems stay up
a little longer.

## Mouse

- Click a section in the left sidebar to switch to it.
- Click a conversation to open it, the compose box to start writing, and an
  attachment to open it with your desktop's default app.
- In Channels, click a hub or room to open it, the input box to write, a
  public room in a hub's view to join it, and a name to message that user.
- In the Network tab, click a row to select it and double-click to open it.
  Click the search bar to start searching.
- In the browser, click a saved page or node on the left to open it beside
  the list. Click the Saved and Nodes headers to switch lists.
- On a page, click links and form fields, click the address bar to go to an
  address, and right-click to go back.
- Click **view source** in the page's title bar to see its Micron source, and
  **back to page** to return.
- Drag across page text to select it. It's copied when you release the
  button, and dragging past the top or bottom edge scrolls the page.
- In the Status and Reticulum tabs, click a row to select it and double-click
  to edit it. In the editors, click to place the cursor and drag to select;
  click a ribbon button to format.
- The scroll wheel scrolls whatever is under the pointer.
- Click the version beside the name (top left) to open rettui's project page
  in your browser. In the web UI it opens in a new tab.

## Copy and paste

- Copied text goes to the system clipboard (X11 or Wayland). It's also sent
  through the terminal's OSC 52 clipboard escape, so copying works over SSH
  in terminals that support it. Set `RETTUI_CLIPBOARD=osc52` to use only
  OSC 52.
- Your terminal's paste shortcut (for example `Ctrl-Shift-V`) and `Ctrl-V`
  both paste into whatever is being edited: a message, a prompt, the selected
  form field on a page, or an editor. Line breaks become spaces in the
  single-line inputs, and are kept in the editors.
- rettui captures the mouse, so the terminal's own selection usually needs a
  modifier. Most terminals use `Shift`+drag.
