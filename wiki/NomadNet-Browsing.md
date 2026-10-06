The Browser tab opens NomadNet pages (written in Micron) from nodes on the
network.

- **Finding nodes:** the Browser tab has a pane with two lists: Saved (pages
  you saved with `s`) and Nodes (every NomadNet node heard, newest first).
  Pages open beside the pane. You can also go to any address (`g`), or to
  your home page (`H`, set in the settings).
- **Searching:** `/` (or a click on the line under the lists' tabs) searches
  both lists by name or address, as the Network tab's search does: every
  word typed must match, and addresses can be pasted as `<hash>` or
  `hash:/page/index.mu`. The lists narrow as you type, matches are
  highlighted, a node or page found by its address shows it, and the tabs
  count the matches ("Nodes 4/120"). `Enter` keeps the search while you
  move through the list, and `Esc` clears it. In the web UI, the search box
  is under the tabs, and finds any node heard, not only the 200 newest
  listed.
- **Saving a node:** with the Nodes list in hand, `s` saves the selected
  node's home page without opening it (with the page in hand, `s` saves
  the page). In the web UI, the ☆ beside a node saves it, and ★ removes it.
  Either way it's the same as saving the home page once open, and a node
  whose home page is saved is starred.
- **Micron:** most of it is supported, including 24-bit colour, alignment,
  dividers, literal blocks, links, forms (text fields, checkboxes and radio
  buttons), inline images and page colours. Long lines wrap between words.
  Pages look as they do in NomadNet: a line's background colour fills its
  row, and headings have NomadNet's colours. Also, as NomadNet's Micron guide
  describes them:
  - **Tables** between `` `t `` lines (`` `tc30 `` centres one and keeps it
    within 30 columns), with columns aligned by a `| --- | :-: | --: |` row,
    which also makes the row above it a header. Columns are as wide as their
    widest cells where they fit; otherwise the widest give way and their
    cells wrap. Links and fields in cells work as anywhere else.
  - **Collapsible sections:** a heading written `` `+>`` starts open, and
    `` `->`` starts folded, marked ▾ and ▸ (or a page's own marks, from
    `#!fold`). `Enter` on the heading, or a click, folds or opens it.
  - **Anchors:** every heading is one (its text in lowercase, with hyphens
    between words), and `` `:name `` adds one anywhere. A link to `#name`
    scrolls to it, `#` alone scrolls to the next heading, and a link to
    another page with `anchor=name` among its variables opens that page there.
  - **Partials:** `` `{address`seconds`fields} `` loads part of a page on its
    own once the page shows (with the fields and variables it names), and
    again every so many seconds if it says so, while the page is on screen.
    A link to `p:id` reloads the partials with that `pid=id` among their
    variables. What you typed in the page's fields stays when a partial
    loads. Partials inside partials aren't loaded.
  - **Text fields of several rows:** `` `<40x5|notes`> `` (40 wide, 5 rows).
    In the TUI the field is edited on one line, with ↵ for each new line; the
    web UI gives it a text box of that size.
- **Links:** page links, `lxmf@` links (open a conversation), `rrc://` links
  (open a hub room), and `/file/` downloads, which are saved to `downloads/`.
- **Identifying:** pages that personalise content need you to identify.
  Toggle it per node with `I`.
- **View source:** `u` (or the button in the page's title bar) shows the
  page's Micron with line numbers and the markup coloured. Refreshing keeps
  the source view, and `Y` copies the source exactly as the node sent it.
- **Copying:** drag across page text to copy it, `Y` copies the whole page,
  and `L` copies the selected link.
- **History:** `b` (or a right-click) goes back.
- **Cache:** pages and images are cached on disk for `cache_hours` (24 by
  default). A page's `#!c=` directive can shorten that, and `#!c=0` pages are
  never cached. Form submissions and file downloads always go to the network.
  - The address bar shows when a page came from the cache.
  - `r` refetches the page and its images. The cached copy is only replaced
    once the new one arrives, so it still works while a node is unreachable.
  - `R` clears the whole cache.
- **Images:**
  - Page images and image attachments are drawn with
    [ratatui-image](https://crates.io/crates/ratatui-image) in the best
    protocol the terminal has: Kitty graphics (Kitty, Ghostty and others),
    Sixel, iTerm2 inline images (iTerm2, WezTerm), or half blocks everywhere
    else. They scroll and clip like text, and are decoded in the background so
    the UI never waits for them.
  - **Windows Terminal** (detected by `WT_SESSION`, which is also set inside
    WSL) always gets Sixel. It may not report its cell size, which Sixel needs
    to size pictures: if they come out too big or small, set
    `RETTUI_CELL_SIZE` to your font's cell size in pixels, for example
    `RETTUI_CELL_SIZE=9x19`.
  - rettui asks the terminal what it supports at startup with a short,
    bounded query (at most 1.5 s, and only if the terminal doesn't answer).
    Konsole's Sixel and Kitty support and WezTerm's Kitty support are skipped,
    as ratatui-image recommends.
  - `RETTUI_GRAPHICS` forces a protocol: `kitty`, `sixel`, `iterm2` or
    `halfblocks`.
