# Shared modules

Build recipes for libraries the GNOME runtime lacks, copied unchanged from
[Flathub's shared modules](https://github.com/flathub/shared-modules)
(with their folder layout, which the recipes' relative paths need):

- `libayatana-appindicator/`: the tray icon (with libdbusmenu, ayatana-ido
  and libayatana-indicator).
- `intltool/`: a build tool libdbusmenu needs.

To update one, copy the folder again from that repository.
