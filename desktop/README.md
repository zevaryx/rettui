# rettui desktop

The web UI in a window of its own, without a browser. It runs rettui in
this process, as the Android app does: the web UI listens on 127.0.0.1 (on
port 8741, or the one it had last time, kept in `desktop_port` in the data
folder), and the window opens its login link. Links that aren't rettui's
open in the system's browser; downloads go to the Downloads folder;
notifications are the system's own, since the webview has none, and
clicking one opens what it's about (on Windows, while the toast is still
on screen, not from the notification centre).

Closing the window leaves rettui running in the background, so messages
keep arriving and notifications keep coming, with an icon in the system
tray (the menu bar on a Mac): a click on it opens the window again, and
its menu has **Open rettui** and **Quit**. Starting the app again opens
the window too. *Keep running when closed* (Settings › System, on) turns
that off, so closing the window stops rettui, which saves everything
first; *Minimize to tray* (off) hides the window to the tray when it's
minimized. On Linux the tray needs libayatana-appindicator (the
Flatpak carries its own); without it the window closes and minimizes as
usual.

## Identities, backups and pipe interfaces

The web UI leaves these out on purpose: anyone with its login link controls
it, so it may not copy, import or delete identity keys, make backups that
hold the identity, or set up programs for Reticulum to run. The desktop app
has them in a window of its own, opened from **Identities, backups and
pipes…** in the tray menu (the **rettui** menu on a Mac, Cmd+Shift+M), the
launcher's action of that name, or by starting `rettui-desktop --manage`.

What keeps the rule's intent: nothing is added to the web UI or its HTTP
API. The window is a page bundled in the app (`ui/`), and its commands
(`src/manage.rs`) have permissions that only that window's capability
(`capabilities/manage.json`) grants; the web UI's page is a remote origin
with permission only for notifications. Each command also checks that it's
asked from that window. They reach rettui through `rettui::manage`, which
works only inside this app. Anything that runs a program (a pipe
interface's command) asks first, and a new one starts turned off.

It's a separate package so that `rettui` itself never links the system
webview: `cargo build` at the top builds rettui alone.

## Building and running

Linux needs WebKitGTK and its friends (Debian and Ubuntu):

    sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev

macOS and Windows use their own webviews (WKWebView, WebView2) and need
nothing more.

    cargo run -p rettui-desktop                      # the usual data folder
    cargo run -p rettui-desktop -- --data-dir DIR    # another one

Installers (`.deb`, AppImage, `.dmg`, NSIS `.exe`) come from the Tauri CLI:

    cargo install tauri-cli --version '^2'
    cd desktop && cargo tauri build

## Signing

The Desktop workflow signs the Windows and macOS builds when the
repository has the certificates as secrets, and builds them unsigned when
it doesn't. An unsigned build runs, but Windows SmartScreen and macOS
Gatekeeper warn about it. See [Development](../wiki/Development.md).

## Not done yet

- Restoring a backup from the window (use `rettui restore`, with rettui
  stopped).
