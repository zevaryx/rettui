Things that may look odd, and why.

## Announces on a shared instance

rsReticulum sends one announce when a destination registers with a shared
instance, so the local daemon learns the path. This happens even when
`announce_at_start` is false. The announce is a path response, so the Python
daemon doesn't rebroadcast it to the network, but apps using the same shared
instance will see it.

## Characters left on screen

Terminals differ on how wide they draw some emoji (such as ❤️ or joined
family emoji), and the TUI only sends the cells that change. Where a terminal
draws one wider or narrower than expected, stray characters could stay behind,
so rettui repaints the whole screen when you switch tabs or panes, and
`Ctrl-L` repaints it at any time. Its own icons avoid symbols that terminals
may draw as emoji.

## Older LXMF clients

Some clients announce their name in LXMF's original format (the name itself,
not msgpack). rsLXMF only reads the newer format, so rettui reads the older
one itself, as Python LXMF does.
