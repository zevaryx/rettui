The desktop app, `rettui-desktop`, is rettui in a window of its own: the
[web UI](Web-UI) without a browser. It runs rettui itself, so it needs
nothing else running, and shares the data folder with the terminal rettui
(see [Data and Storage](Data-and-Storage)). Installers (`.deb`, AppImage,
Flatpak, `.dmg` and a Windows installer) come with each release; see
[Installing](Installing). The builds aren't signed yet, so Windows
SmartScreen and macOS Gatekeeper warn when you first open one.

Closing the window leaves rettui running in the system tray, so messages
keep arriving (*Keep running when closed* in Settings > System turns that
off). Notifications are the system's own, and links that aren't rettui's
open in your browser.

## Identities, backups and pipes

The web UI can't copy, import or delete identity keys, make a backup that
holds your identity, or set up programs for Reticulum to run, since anyone
with its login link controls it. The desktop app has these in a window of
its own that only this computer can open. Open it from **Identities,
backups and pipes…** in the tray menu, the **rettui** menu on a Mac
(Cmd+Shift+M), the launcher's action of that name, or with
`rettui-desktop --manage`.

- **Identities:** switch, rename and delete the identities kept here (not
  the first, nor the one in use), keep a new one, or keep one from a file
  (from Sideband, NomadNet, MeshChat or another rettui), and save a copy of
  the identity in use. A copy is the key itself: anyone who has it can pose
  as you.
- **Backup:** settings, contacts, messages and your identity in one file,
  sealed with a passphrase if you give one (see
  [Sealed Backups](Sealed-Backups)). **Restore a backup** puts one in
  place of the data of the identity in use: type the file's path, and a
  sealed backup asks for its passphrase. It looks into the backup first, so
  a wrong passphrase or a file that isn't a backup changes nothing, then
  asks before going on, since a restore replaces your settings, contacts,
  messages and identity. What was there is moved aside into a
  `before-restore-<time>` folder in the data folder, not deleted. rettui
  stops while it restores (so anything being sent or received at that
  moment is lost) and starts again, and the main window reloads. If the
  restore fails, rettui starts again as it was. `rettui restore`, with
  rettui stopped, does the same without the window.
- **Pipe interfaces:** add, change, turn on or off and remove the pipe
  interfaces in Reticulum's config. Each runs a program with your account's
  rights, so it asks first, and a new one starts off. Changes apply when
  Reticulum restarts, which the window can do.
