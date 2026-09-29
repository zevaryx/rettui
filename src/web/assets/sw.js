// rettui's service worker. Phone browsers only let a service worker show
// notifications, so the page shows them through this one; tapping one
// brings the page up at what it's about. It doesn't touch requests.

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const target = event.notification.data?.target || null;
  event.waitUntil((async () => {
    const pages = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    const page = pages.find((p) => p.focused) || pages[0];
    if (page) {
      await page.focus();
      page.postMessage({ open: target });
      return;
    }
    // No page open: start one there.
    const tab = target?.kind === 'room' ? 'channels' : 'messages';
    const opened = await self.clients.openWindow('/#' + tab);
    opened?.postMessage({ open: target });
  })());
});
