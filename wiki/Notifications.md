rettui tells you about new messages even while you're somewhere else, in
both UIs.

- **What notifies:** a new LXMF message, and in RRC a mention of your nick or
  a whisper. Nothing notifies for the conversation or room you're looking at
  while rettui's window has the focus. Many at once (a sync bringing in a
  backlog) become one notification, and a newer one about the same
  conversation or room replaces the older one.
- **What counts as read:** a conversation or room is read only while it's in
  front of you: on screen in a window that has the focus (in the TUI, where
  the terminal reports focus), or in a browser tab that's showing (on a phone,
  with the conversation open, not just its list). A background tab, or a
  phone with rettui in the background, leaves new messages unread, on every
  device.
- **Terminal UI:** notifications are the desktop's own: D-Bus notifications
  on Linux and the BSDs, Notification Center on macOS, toasts on Windows.
  Terminals that report focus (most do; in tmux, turn on `focus-events`) let
  rettui tell when you're looking. Where there's no desktop to show them (over
  SSH, for example), the log says so once. Clicking one:
  - **Linux and the BSDs:** opens its conversation or room in rettui;
    whether the terminal's window comes forward is up to the desktop.
  - **Windows:** opens its conversation or room, and brings the console
    window forward where Windows allows it (Windows Terminal may not pass
    that on). Notifications show as Windows PowerShell's, since a program
    that isn't installed can't have its own, with the instant-message
    sound. If none show, check that Windows PowerShell's notifications are
    on (Settings > System > Notifications) and Do not disturb is off.
  - **WSL:** with no Linux notification service, Windows shows them,
    through Windows PowerShell. A click can't come back to rettui there.
  - **macOS:** they come from the terminal rettui runs in (with its icon),
    so a click brings that terminal forward; rettui can't tell which one
    was clicked (that needs a Cocoa run loop, which a terminal program
    doesn't run). If none show, allow your terminal's notifications in
    System Settings > Notifications.
- **Web UI:** notifications are the browser's own. Allow them under Status
  (the **Notifications** row). Clicking one opens its conversation or room,
  and the tab's title counts what's unread. Browsers only allow notifications
  on HTTPS (see **HTTPS** under [Web UI](Web-UI)) or on the same computer
  (`localhost`). Elsewhere, and until you
  allow them, a note shows in the page while you're looking at it. On an
  iPhone (iOS 16.4 or later), add rettui to the Home Screen first (see
  **Installing** under [Web UI](Web-UI)).
- **In the background (web UI):** a phone soon stops a page it isn't
  showing, and with it rettui's live connection. **Turn on** beside "In the
  background" (Status, **Notifications**) in each browser that should still
  be notified then. rettui sends those browsers their notifications by Web
  Push, through the browser's push service (Google's, Apple's, Mozilla's or
  Microsoft's), encrypted for that browser: the service sees when one is
  sent, not what it says. rettui needs to reach the internet for this, and
  doesn't push to a browser while it shows rettui.
- **Several devices:** every browser gets each notification (unless it's
  showing what it's about). Reading a conversation or room on one closes
  its notification on the others that are connected. A browser whose
  connection dropped for a while gets the notifications it missed when it's
  back, for what's still unread.
- **Muting a conversation:** `N` (TUI), or the 🔔 bell in its header (web
  UI), turns a conversation's notifications off or back on. Muted
  conversations show *muted* (TUI) or 🔕 (web UI) in the list.
- **Hubs and rooms:**
  - **Hub default:** a hub notifies of mentions and whispers. It can notify
    of every message in its rooms instead, or of nothing.
  - **Rooms:** each room and whisper conversation follows its hub unless it
    is set on its own: all messages, mentions only, or off.
  - **Changing them:** `N` (TUI) steps through the choices for the hub, room
    or whisper conversation on screen. In the web UI, the bell in its header
    opens a menu.
  - **Showing them:** rooms and hubs that are off show *muted* (TUI) or 🔕
    (web UI).
- **Settings:** "Notify on messages" and "Notify from RRC" (under Status)
  turn each kind off everywhere.
