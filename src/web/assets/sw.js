// rettui's service worker. Phone browsers only let a service worker show
// notifications, so the page shows them through this one; tapping one
// brings the page up at what it's about. With background notifications on,
// it shows those rettui pushes (Web Push) while no page shows rettui. It
// doesn't touch requests.

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const target = event.notification.data?.target || null;
  event.waitUntil((async () => {
    const pages = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    const page = pages.find((p) => p.focused) || pages[0];
    if (page) {
      // What to open first: the browser may refuse to bring the page
      // forward (it then shows it when next looked at).
      page.postMessage({ open: target });
      await page.focus().catch(() => {});
      return;
    }
    // No page open: start one there. What to open goes in its address too,
    // since the browser may not hand back the window it opened (an
    // installed app's, say) to be told.
    const tab = target?.kind === 'room' ? 'channels' : 'messages';
    const open = target ? '?open=' + encodeURIComponent(JSON.stringify(target)) : '';
    // Where rettui is: the worker's scope, behind a proxy at a sub-path too.
    const opened = await self.clients.openWindow(new URL(open + '#' + tab, self.registration.scope).href);
    opened?.postMessage({ open: target });
  })());
});

// A push: shown, always (browsers require it, and some stop pushes to a
// site that doesn't). If a page in the background showed the same one from
// its own connection, it's shown again quietly, so the phone doesn't buzz
// twice.
self.addEventListener('push', (event) => {
  let notification = null;
  try {
    notification = event.data?.json();
  } catch {}
  if (!notification?.title) return;
  const { title, body, tag, target } = notification;
  event.waitUntil((async () => {
    const shown = await self.registration.getNotifications({ tag });
    const again = shown.some((n) => n.body === body);
    await self.registration.showNotification(title, { body, tag, renotify: !again, icon: 'brand/icon.png', data: { target } });
  })());
});
