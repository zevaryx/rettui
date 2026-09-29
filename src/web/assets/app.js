// rettui web UI. Talks to the JSON API under /api; the server pushes a
// Server-Sent Event whenever something changes and the visible tab refetches.
// Everything received from the network is inserted as text, except NomadNet
// pages, which the server renders to escaped, script-free HTML.
'use strict';

// ---- helpers ----------------------------------------------------------------

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (key === 'class') node.className = value;
    else if (key === 'text') node.textContent = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (key === 'dataset') Object.assign(node.dataset, value);
    else if (key in node && typeof value !== 'string') node[key] = value;
    else node.setAttribute(key, value === true ? '' : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

const $ = (selector, root = document) => root.querySelector(selector);

async function request(method, path, body) {
  const options = { method, headers: {} };
  if (body !== undefined) {
    options.headers['Content-Type'] = 'application/json';
    options.body = JSON.stringify(body);
  }
  const response = await fetch('/api' + path, options);
  if (response.status === 401) {
    location.reload();
    throw new Error('not logged in');
  }
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error || response.statusText);
  return data;
}
const api = {
  get: (path) => request('GET', path),
  post: (path, body = {}) => request('POST', path, body),
};

function toast(text, error = false) {
  const node = el('div', { class: 'toast' + (error ? ' error' : ''), text });
  $('#toasts').append(node);
  setTimeout(() => node.remove(), error ? 6000 : 3000);
}

// Run an action, reporting failures instead of throwing.
async function attempt(action, done) {
  try {
    const result = await action();
    if (done) toast(done);
    return result;
  } catch (e) {
    toast(e.message, true);
    return undefined;
  }
}

async function copy(text, what) {
  if (!text) return toast(`Nothing to copy (${what} is empty)`, true);
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = el('textarea', { value: text });
    document.body.append(area);
    area.select();
    document.execCommand('copy');
    area.remove();
  }
  toast(`Copied ${what}`);
}

function timeLabel(unixSeconds) {
  const date = new Date(unixSeconds * 1000);
  const now = new Date();
  const hm = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
  if (date.toDateString() === now.toDateString()) return hm;
  return date.toLocaleDateString([], { month: 'short', day: '2-digit' }) + ' ' + hm;
}

function ago(unixSeconds) {
  const secs = Math.max(0, Math.floor(Date.now() / 1000 - unixSeconds));
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h`;
  return `${Math.floor(secs / 86400)}d`;
}

// Bytes per second, short (as the terminal UI shows it).
function rate(bytesPerSec) {
  const units = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
  let value = Math.max(0, bytesPerSec);
  let unit = 0;
  while (value >= 999.5 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  return `${unit === 0 || value >= 9.95 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

function humanBytes(bytes) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(1)} ${units[unit]}`;
}

// Put new server-rendered HTML into `container`, replacing only the lines
// that differ: typing changes a line or two, and redrawing the rest would
// reload its images (they flash and push the page about) for nothing.
function patchHtml(container, html) {
  const next = document.createElement('template');
  next.innerHTML = html;
  const fresh = next.content.firstElementChild;
  const current = container.firstElementChild;
  const sameShell = current && fresh && container.childElementCount === 1 && next.content.childElementCount === 1
    && current.tagName === fresh.tagName && current.className === fresh.className
    && current.getAttribute('style') === fresh.getAttribute('style');
  if (!sameShell) {
    container.replaceChildren(next.content);
    return;
  }
  const old = [...current.children];
  const neu = [...fresh.children];
  let head = 0;
  while (head < old.length && head < neu.length && old[head].isEqualNode(neu[head])) head++;
  let tail = 0;
  while (tail < old.length - head && tail < neu.length - head
    && old[old.length - 1 - tail].isEqualNode(neu[neu.length - 1 - tail])) tail++;
  const anchor = tail ? old[old.length - tail] : null;
  for (const node of old.slice(head, old.length - tail)) node.remove();
  for (const node of neu.slice(head, neu.length - tail)) current.insertBefore(node, anchor);
}

// Fetch into `cache` (a Map) under `key`, sharing a request already under
// way, so a click can use what a prefetch started.
function loadInto(view, key, url) {
  view.loading ||= new Map();
  view.loaded ||= new Map();
  const inFlight = view.loading.get(key);
  // One started before the latest change may not have it: load again.
  if (inFlight && inFlight.epoch === loadEpoch) return inFlight.request;
  const epoch = loadEpoch;
  const request = api.get(url)
    .then((data) => {
      // An older load finishing late must not replace newer data.
      if ((view.loaded.get(key) ?? -1) <= epoch) {
        view.loaded.set(key, epoch);
        view.cache.set(key, data);
      }
      return view.cache.get(key);
    })
    .finally(() => {
      if (view.loading.get(key)?.request === request) view.loading.delete(key);
    });
  view.loading.set(key, { request, epoch });
  return request;
}
// Counts refetches for live updates (see `loadNow`).
let loadEpoch = 0;

// Something just sent, drawn at once as sending (`node`, added to `parent`
// when `key` is what `view` shows), before the server's copy arrives. Redraws
// keep it (see `echoesFor`) until the send is answered; then it goes, or is
// replaced by the real one on a redraw that isn't skipped as unchanged.
function echoSent(view, { key, node, parent, scroller, slot }) {
  view.echoes ||= [];
  const echo = {
    key,
    node,
    done(failed) {
      view.echoes = view.echoes.filter((e) => e !== echo);
      if (failed) node.remove();
      else {
        (view.snapshots ||= {})[slot] = null;
        refresh();
      }
    },
  };
  view.echoes.push(echo);
  node.classList.add('echo');
  if (parent) {
    parent.append(node);
    scroller.scrollTop = scroller.scrollHeight;
    scroller.pinned = true;
  }
  return echo;
}

// Unsent writing for each conversation or room, kept where it was written:
// switching puts away what's in the box (a view's `takeDraft`) and brings
// back what was left in the one opened (`putDraft`), rather than carrying
// it over to be sent to someone else.
function draftSwitch(view, key) {
  if (key == null || view.draftKey === key) return;
  view.drafts ||= new Map();
  if (view.draftKey != null) {
    const draft = view.takeDraft();
    if (draft) view.drafts.set(view.draftKey, draft);
    else view.drafts.delete(view.draftKey);
  }
  view.draftKey = key;
  view.putDraft(view.drafts.get(key) || null);
  view.drafts.delete(key);
}

// Put back what failed to send, before anything written since, in the
// conversation or room it was sent to.
function draftRestore(view, key, draft) {
  view.drafts ||= new Map();
  if (view.draftKey === key) view.putDraft(view.joinDrafts(draft, view.takeDraft()));
  else view.drafts.set(key, view.joinDrafts(draft, view.drafts.get(key) || null));
}

function echoesFor(view, key) {
  return (view.echoes || []).filter((e) => e.key === key).map((e) => e.node);
}

// Load a few things at a time in the background, skipping failures.
async function warm(loads, parallel = 4) {
  const queue = [...loads];
  const worker = async () => {
    while (queue.length) await queue.shift()().catch(() => {});
  };
  await Promise.all(Array.from({ length: parallel }, worker));
}

// Keep a scrolled list at the bottom if it was there before an update.
function stickToBottom(node, update) {
  const atBottom = node.pinned ?? node.scrollHeight - node.scrollTop - node.clientHeight < 40;
  update();
  if (atBottom) node.scrollTop = node.scrollHeight;
}

function readFile(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result.split(',', 2)[1] || '');
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

const NICK_COLORS = ['#f14c4c', '#23d18b', '#f5f543', '#6cb6ff', '#d670d6', '#29b8db', '#ffa55a', '#b48eff', '#7fdbca', '#ff8fb3'];
function nickColor(src) {
  const seed = src ? parseInt(src.slice(0, 2), 16) || 0 : 0;
  return NICK_COLORS[seed % NICK_COLORS.length];
}

// ---- app shell --------------------------------------------------------------

const TABS = [
  { id: 'messages', icon: '✉', title: 'Messages' },
  { id: 'channels', icon: '#', title: 'Channels' },
  { id: 'network', icon: '◎', title: 'Network' },
  { id: 'browser', icon: '◈', title: 'Browser' },
  { id: 'node', icon: '⌂', title: 'Node' },
  { id: 'status', icon: 'ⓘ', title: 'Status' },
  { id: 'reticulum', icon: '⛭', title: 'Reticulum' },
];

const app = {
  tab: null,
  status: null,
  views: {},
};

function renderSidebar() {
  const unread = app.status?.unread || {};
  const tabs = $('#tabs');
  // Only when something on them changed: a tab redrawn under a finger
  // loses its tap.
  const drawn = JSON.stringify([app.tab, unread.messages, unread.channels, unread.mention]);
  if (tabs.dataset.drawn !== drawn) tabs.replaceChildren(...TABS.map((tab) => {
    let badge = null;
    if (tab.id === 'messages' && unread.messages) badge = el('span', { class: 'badge', text: unread.messages });
    if (tab.id === 'channels' && unread.channels) {
      badge = el('span', { class: 'badge' + (unread.mention ? ' mention' : ''), text: unread.channels });
    }
    return el('div', {
      class: 'tab' + (tab.id === app.tab ? ' active' : ''),
      title: tab.title,
      onclick: () => {
        // On a phone, the section on screen goes back to its list.
        if (tab.id === app.tab && phone.matches) setPane(app.views[tab.id], 'list', { record: false });
        switchTab(tab.id);
        setDrawer(false);
      },
    }, el('span', { class: 'icon', text: tab.icon }), el('span', { class: 'label', text: tab.title }), badge);
  }));
  tabs.dataset.drawn = drawn;
  // Unread in the tab's title, for when the page is in the background.
  const count = (unread.messages || 0) + (unread.channels || 0);
  document.title = count ? `(${count}) rettui` : 'rettui';
  renderAppbar();
  if (app.status) {
    $('#who-name').textContent = app.status.display_name;
    const total = app.status.interfaces.length;
    const net = app.status.net.state === 'online'
      ? `● ${app.status.interfaces_online}/${total} interfaces`
      : app.status.net.state === 'failed' ? '● network failed' : '◌ starting…';
    $('#who-net').textContent = net;
    $('#who-net').className = app.status.net.state === 'online' && app.status.interfaces_online ? 'state-ok' : 'dim';
    // Traffic over all interfaces, once there is a rate to show.
    const traffic = app.status.traffic;
    const flow = $('#who-flow');
    const known = app.status.net.state === 'online' && traffic?.rx_rate != null;
    flow.classList.toggle('hidden', !known);
    if (known) {
      const drawn = `${traffic.rx_rate} ${traffic.tx_rate}`;
      if (flow.dataset.drawn !== drawn) {
        flow.dataset.drawn = drawn;
        flow.replaceChildren(el('span', { class: 'state-ok', text: '↓ ' }), rate(traffic.rx_rate),
          el('span', { class: 'flow-out', text: '  ↑ ' }), rate(traffic.tx_rate));
      }
      flow.title = `Received ${humanBytes(traffic.rx)}, sent ${humanBytes(traffic.tx)} (all interfaces)`;
    }
  }
}

// Sections are built once and kept: switching shows one at once, as it
// was left (like the terminal UI, which has everything in memory), and its
// data is refreshed behind it. Only the section on screen is in the page;
// the others wait, detached, with their content and state.
function switchTab(id, options = {}) {
  const view = app.views[id];
  if (!view) return;
  const previous = app.views[app.tab];
  app.tab = id;
  if (location.hash !== '#' + id) history.replaceState(null, '', '#' + id);
  renderSidebar();
  app.views.channels.closeMenu();
  closeSheet();
  if (previous && previous !== view && previous.root?.isConnected) {
    saveScroll(previous.root);
    previous.root.remove();
  }
  const fresh = mountView(id, options);
  if (!view.root.isConnected) $('#main').append(view.root);
  view.root.querySelectorAll('textarea.page-editor').forEach(applyWrap);
  restoreScroll(view.root);
  if (!fresh) view.shown?.(options);
  loadNow();
}

// Build a section (off the page until shown); true if it was new.
function mountView(id, options = {}) {
  const view = app.views[id];
  if (view.root) return false;
  view.root = el('div', { class: 'view' + (view.panes ? ' panes' : ''), dataset: { view: id, pane: view.pane || 'list' } });
  view.mount(view.root, options);
  view.root.querySelectorAll('[data-stick="bottom"]').forEach(followBottom);
  return true;
}

// ---- phones -----------------------------------------------------------------
//
// On a small screen a section shows one pane at a time: its list, or what
// was picked from it, with a back button in the top bar (the phone's own
// back button works too). The sidebar is a drawer. Larger screens show
// everything side by side, and none of this changes them.

// Keep in step with the phone `@media` in style.css.
const phone = matchMedia('(max-width: 760px), (pointer: coarse) and (max-height: 500px)');

// Back goes up one pane.
const PANE_UP = { members: 'detail', detail: 'list', list: 'list' };
const PANE_DEPTH = { list: 0, detail: 1, members: 2 };

// Show a pane of a section with a list (`view.panes`): 'list', 'detail'
// (what was picked) or 'members' (a room's). Going deeper on a phone adds
// a history entry, so the phone's back button comes back.
function setPane(view, pane, { record = true } = {}) {
  const from = view.pane || 'list';
  if (!view.panes || from === pane) return;
  view.pane = pane;
  if (view.root) swapPanes(view.root, () => { view.root.dataset.pane = pane; });
  if (view === app.views[app.tab]) {
    if (record && phone.matches) {
      if (PANE_DEPTH[pane] > PANE_DEPTH[from]) history.pushState({ tab: app.tab, pane }, '', location.hash);
      // Back up by an action (not the back button): drop the entry for the
      // pane left, so back doesn't return to it.
      else if (history.state?.tab === app.tab && history.state.pane === from) history.back();
    }
    renderAppbar();
  }
}

// A pane that is hidden loses its scroll position: keep what is on screen
// and put it back when it shows again (a chat goes to its newest line).
function swapPanes(root, change) {
  const shown = (node) => node.offsetParent !== null;
  for (const node of root.querySelectorAll('.scroll')) {
    if (shown(node)) node.paneScroll = { top: node.scrollTop, atBottom: node.pinned ?? node.scrollHeight - node.scrollTop - node.clientHeight < 40 };
  }
  change();
  for (const node of root.querySelectorAll('.scroll')) {
    if (!shown(node)) continue;
    const saved = node.paneScroll;
    node.paneScroll = null;
    if (node.dataset.stick === 'bottom' && (!saved || saved.atBottom)) {
      node.scrollTop = node.scrollHeight;
      node.pinned = true;
    } else if (saved) node.scrollTop = saved.top;
  }
}

function paneBack() {
  const view = app.views[app.tab];
  if (!view) return;
  if (history.state?.tab === app.tab && history.state.pane) history.back();
  else setPane(view, PANE_UP[view.pane || 'list'], { record: false });
}

window.addEventListener('popstate', (e) => {
  const view = app.views[app.tab];
  if (!view?.panes) return;
  setPane(view, e.state?.tab === app.tab ? e.state.pane : 'list', { record: false });
});

// The top bar: the section's name, and the menu (in a list) or back (deeper).
function renderAppbar() {
  const view = app.views[app.tab];
  const deeper = !!view?.panes && (view.pane || 'list') !== 'list';
  $('#nav-menu').classList.toggle('hidden', deeper);
  $('#nav-back').classList.toggle('hidden', !deeper);
  $('#appbar-title').textContent = TABS.find((t) => t.id === app.tab)?.title || '';
  // Unread in another section shows on the menu button.
  const unread = app.status?.unread || {};
  const elsewhere = (app.tab !== 'messages' && unread.messages) || (app.tab !== 'channels' && unread.channels);
  $('#nav-menu').classList.toggle('unread', !!elsewhere);
  $('#nav-menu').classList.toggle('mention', app.tab !== 'channels' && !!unread.mention);
}

function setDrawer(open) {
  document.body.classList.toggle('drawer-open', open);
  $('#nav-menu').setAttribute('aria-expanded', String(open));
}

// A header's less used buttons (class "more") go behind this on a phone,
// where there's no room for them all: it lists them in a sheet from the
// bottom of the screen, and picking one clicks the real button.
function moreButton() {
  return el('button', { class: 'phone-only more-button', text: '⋯', title: 'More', 'aria-label': 'More actions', onclick: (e) => {
    e.stopPropagation();
    const header = e.currentTarget.closest('header');
    const buttons = [...header.querySelectorAll('button.more')];
    openSheet(header.querySelector('.title')?.textContent || '', buttons.map((button) => ({
      text: button.textContent, danger: button.classList.contains('danger'), disabled: button.disabled, action: () => button.click(),
    })));
  } });
}

// A menu of `items` ({ text, danger, disabled, checked, action }): a sheet
// from the bottom on a phone, else under `anchor` (the button that opened
// it).
let sheet = null;
function openSheet(title, items, anchor = null) {
  closeSheet();
  const node = el('div', { class: 'user-menu sheet', role: 'menu' },
    title ? el('div', { class: 'menu-title', text: title }) : null,
    items.map(({ text, danger, disabled, checked, action }) => el('button', {
      class: 'menu-item' + (danger ? ' danger' : ''), disabled, role: checked === undefined ? 'menuitem' : 'menuitemradio',
      'aria-checked': checked === undefined ? null : String(checked), onclick: () => {
        closeSheet();
        action();
      },
    }, checked === undefined ? el('span', { text })
      : el('span', {}, el('span', { class: 'check', text: checked ? '✓' : '' }), text))));
  document.body.append(node);
  if (anchor && !phone.matches) {
    const rect = anchor.getBoundingClientRect();
    node.style.left = Math.max(8, Math.min(rect.left, window.innerWidth - node.offsetWidth - 8)) + 'px';
    node.style.top = Math.max(8, rect.bottom + node.offsetHeight + 8 > window.innerHeight ? rect.top - node.offsetHeight - 4 : rect.bottom + 4) + 'px';
  }
  const closer = (e) => {
    if (e.type === 'keydown') {
      if (e.key === 'Escape') closeSheet();
    } else if (!node.contains(e.target)) {
      // A tap outside only closes it, not also presses what's under it.
      e.preventDefault();
      e.stopPropagation();
      closeSheet();
    }
  };
  sheet = { node, closer };
  setTimeout(() => {
    document.addEventListener('click', closer, true);
    document.addEventListener('keydown', closer);
  });
}

function closeSheet() {
  if (!sheet) return;
  sheet.node.remove();
  document.removeEventListener('click', sheet.closer, true);
  document.removeEventListener('keydown', sheet.closer);
  sheet = null;
}

$('#nav-menu').addEventListener('click', () => setDrawer(true));
$('#nav-back').addEventListener('click', paneBack);
$('#scrim').addEventListener('click', () => setDrawer(false));
// ---- swipes (phones) --------------------------------------------------------
//
// A swipe follows the finger, and finishes or springs back when it lifts:
// right from a list opens the drawer, and left closes it; right from what
// was picked goes back to the list (left from a room shows its members);
// down on a sheet closes it. A swipe that starts where something scrolls
// sideways, or in a text field, is left to that, and so is the screen's
// very edge, where phones have a back gesture of their own.

const SWIPE_EDGE = 20; // px from the screen's sides left to the phone
const SWIPE_SLOP = 10; // px moved before a touch is a swipe (or a scroll)
const SWIPE_FLICK = 0.4; // px/ms: a quick flick finishes a short swipe
const SWIPE_TIME = 200; // ms to finish (or spring back) after lifting

let swipe = null; // the touch being followed
let swiped = 0; // when a swipe last ended (the click after it is dropped)

document.addEventListener('touchstart', (e) => {
  // A second finger ends a swipe; one still finishing ignores new touches.
  if (swipe?.gesture) return swipeDone(false);
  if (swipe?.busy && performance.now() - swipe.busy < 1000) return;
  swipe?.unfollow?.();
  swipe = null;
  if (!phone.matches || e.touches.length !== 1) return;
  if (e.target.closest('input, textarea, select, [contenteditable]')) return;
  const { clientX: x, clientY: y } = e.touches[0];
  swipe = { x, y, target: e.target, edge: x < SWIPE_EDGE || x > window.innerWidth - SWIPE_EDGE, samples: [[performance.now(), 0]] };
  swipe.unfollow = followTouch(e.target);
}, { passive: true, capture: true });

// A touch's moves and lift go to where it started, even once that's taken
// off the page (redrawn under the finger), when they no longer reach the
// document: so they're followed there.
function followTouch(target) {
  const end = (e) => swipeDone(e.type === 'touchend');
  target.addEventListener('touchmove', swipeMove, { passive: false });
  target.addEventListener('touchend', end);
  target.addEventListener('touchcancel', end);
  return () => {
    target.removeEventListener('touchmove', swipeMove);
    target.removeEventListener('touchend', end);
    target.removeEventListener('touchcancel', end);
  };
}

function swipeMove(e) {
  if (!swipe || swipe.busy || swipe.dropped) return;
  if (e.touches.length !== 1) return swipeDone(false);
  const dx = e.touches[0].clientX - swipe.x;
  const dy = e.touches[0].clientY - swipe.y;
  if (!swipe.gesture) {
    if (Math.abs(dx) < SWIPE_SLOP && Math.abs(dy) < SWIPE_SLOP) return;
    swipe.gesture = pickSwipe(dx, dy);
    if (!swipe.gesture) {
      swipe.dropped = true;
      return;
    }
  }
  if (e.cancelable) e.preventDefault();
  const { axis, dir } = swipe.gesture;
  const along = (axis === 'x' ? dx : dy) * dir;
  swipe.samples.push([performance.now(), along]);
  if (swipe.samples.length > 6) swipe.samples.shift();
  swipe.gesture.move(Math.max(0, along));
}

// A swipe ends in a tap-sized click on some browsers: not a tap.
document.addEventListener('click', (e) => {
  if (performance.now() - swiped < 400) {
    e.preventDefault();
    e.stopImmediatePropagation();
  }
}, true);

// The finger lifted (or the phone took the touch): finish if it went a
// third of the way (and isn't flicking back), or was flicked on; spring
// back if not.
function swipeDone(lifted) {
  if (!swipe || swipe.busy) return;
  const current = swipe;
  swipe = null;
  current.unfollow?.();
  if (!current.gesture) return;
  const gesture = current.gesture;
  // How fast it was going as it lifted: over its last tenth of a second
  // of moving (none if it rested before lifting).
  const now = performance.now();
  const [t1, p1] = current.samples.at(-1);
  const [t0, p0] = current.samples.find(([t]) => t1 - t < 100);
  const speed = now - t1 < 100 && t1 > t0 ? (p1 - p0) / (t1 - t0) : 0;
  const done = lifted && (p1 > gesture.size / 3 ? speed > -SWIPE_FLICK : p1 > SWIPE_SLOP && speed > SWIPE_FLICK);
  swiped = now;
  swipe = { busy: now };
  gesture.end(done, () => { swipe = null; });
}

// Which swipe a touch that has started moving is, if any: its axis, the
// direction that finishes it, how far is all the way, and what it moves
// (`move` with how far it has gone, `end` when it lifts).
function pickSwipe(dx, dy) {
  const sideways = Math.abs(dx) > Math.abs(dy);
  const target = swipe.target;
  const menu = app.views.channels.menu;
  const open = sheet?.node || menu;
  if (!sideways) {
    // Down on a sheet (unless it's scrolled down) closes it.
    if (dy < 0 || !open?.contains(target) || scrollsFrom(target, 'y', 1, open)) return null;
    return { axis: 'y', dir: 1, size: open.offsetHeight, ...sheetSwipe(open, open === menu ? () => app.views.channels.closeMenu() : closeSheet) };
  }
  if (open || swipe.edge || (window.visualViewport?.scale ?? 1) > 1.01) return null;
  if (scrollsFrom(target, 'x', Math.sign(dx), document.body)) return null;
  const drawer = document.body.classList.contains('drawer-open');
  if (drawer) return dx < 0 ? { axis: 'x', dir: -1, size: $('#sidebar').offsetWidth, ...drawerSwipe(false) } : null;
  const view = app.views[app.tab];
  const pane = view?.panes ? view.pane || 'list' : 'list';
  if (dx > 0) {
    if (pane === 'list') return { axis: 'x', dir: 1, size: $('#sidebar').offsetWidth, ...drawerSwipe(true) };
    return { axis: 'x', dir: 1, size: view.root.clientWidth, ...paneSwipe(view, PANE_UP[pane], false) };
  }
  if (pane === 'detail' && view.root.querySelector(':scope > .members:not(.hidden)')) {
    return { axis: 'x', dir: -1, size: view.root.clientWidth, ...paneSwipe(view, 'members', true) };
  }
  return null;
}

// Whether something from `target` up to `stop` would scroll along `axis`
// for a finger moving in direction `dir` (content moves the other way).
function scrollsFrom(target, axis, dir, stop) {
  for (let node = target; node && node !== stop.parentElement; node = node.parentElement) {
    const [pos, size, view, overflow] = axis === 'x'
      ? [node.scrollLeft, node.scrollWidth, node.clientWidth, getComputedStyle(node).overflowX]
      : [node.scrollTop, node.scrollHeight, node.clientHeight, getComputedStyle(node).overflowY];
    if (size <= view + 1 || !/auto|scroll/.test(overflow)) continue;
    if (dir > 0 ? pos > 0 : pos < size - view - 1) return true;
  }
  return false;
}

// Run `then` once `node` has finished moving.
function afterMove(node, then) {
  let ran = false;
  const run = (e) => {
    if (ran || (e && e.target !== node)) return;
    ran = true;
    node.removeEventListener('transitionend', run);
    then();
  };
  node.addEventListener('transitionend', run);
  setTimeout(run, SWIPE_TIME + 60);
}

// The drawer, sliding out with the finger (or back in).
function drawerSwipe(opening) {
  const sidebar = $('#sidebar');
  const scrim = $('#scrim');
  const width = sidebar.offsetWidth;
  for (const node of [sidebar, scrim]) node.style.transition = 'none';
  sidebar.style.visibility = 'visible';
  sidebar.style.boxShadow = '8px 0 32px #000c';
  return {
    move(along) {
      const shown = Math.min(1, opening ? along / width : 1 - along / width);
      sidebar.style.transform = `translateX(${(shown - 1) * 102}%)`;
      scrim.style.opacity = shown;
    },
    // The stylesheet's own transitions take it from where the finger left it.
    end(done, finished) {
      for (const node of [sidebar, scrim]) node.style.transition = '';
      sidebar.style.transform = sidebar.style.visibility = sidebar.style.boxShadow = scrim.style.opacity = '';
      setDrawer(opening === done);
      setTimeout(finished, SWIPE_TIME);
    },
  };
}

// One pane sliding over another: going back, the pane on screen slides
// off to the right over the one it came from; to a room's members, they
// slide in from the right over the chat.
function paneSwipe(view, to, forward) {
  const root = view.root;
  const from = view.pane || 'list';
  const node = (pane) => root.querySelector(`:scope > ${{ list: '.side', detail: '.pane-main', members: '.members' }[pane]}`);
  root.dataset.peek = to;
  // Where the pane showing up was scrolled to (as swapPanes puts it back).
  for (const scroll of node(to).querySelectorAll('.scroll')) {
    const saved = scroll.paneScroll;
    if (scroll.dataset.stick === 'bottom' && (!saved || saved.atBottom)) scroll.scrollTop = scroll.scrollHeight;
    else if (saved) scroll.scrollTop = saved.top;
  }
  const moving = node(forward ? to : from);
  const width = root.clientWidth;
  moving.classList.add('swiping');
  moving.style.transition = 'none';
  const place = (along) => { moving.style.transform = `translateX(${forward ? width - along : along}px)`; };
  place(0);
  return {
    move: (along) => place(Math.min(width, along)),
    end(done, finished) {
      moving.style.transition = `transform ${SWIPE_TIME}ms ease-out`;
      place(done ? width : 0);
      afterMove(moving, () => {
        moving.classList.remove('swiping');
        moving.style.transition = moving.style.transform = '';
        delete root.dataset.peek;
        if (done && view.pane === from) setPane(view, to);
        finished();
      });
    },
  };
}

// A sheet going down with the finger.
function sheetSwipe(node, close) {
  node.style.transition = 'none';
  return {
    move(along) { node.style.transform = `translateY(${along}px)`; },
    end(done, finished) {
      node.style.transition = `transform ${SWIPE_TIME}ms ease-out`;
      node.style.transform = done ? 'translateY(100%)' : '';
      afterMove(node, () => {
        if (done) close();
        else node.style.transition = '';
        finished();
      });
    },
  };
}

phone.addEventListener('change', () => {
  setDrawer(false);
  closeSheet();
  renderAppbar();
  if (app.views.messages.text) app.views.messages.text.placeholder = composePlaceholder();
});

// Keys a phone's keyboard doesn't have aren't worth mentioning there.
function composePlaceholder() {
  return phone.matches ? 'Write a message…' : 'Write a message… (Enter sends, Shift+Enter for a new line)';
}

// A chat-style area stays at the newest line while it is there, even as
// images finish loading and push the content down. Only the reader moves
// it off (wheel, touch, keys or the scrollbar): scroll events alone can't
// tell that apart from content growing under an earlier jump to the end.
function followBottom(node) {
  node.pinned = true;
  let touched = 0;
  const reader = () => { touched = performance.now(); };
  for (const type of ['wheel', 'touchmove', 'keydown', 'pointerdown']) node.addEventListener(type, reader, { passive: true });
  node.addEventListener('scroll', () => {
    if (!node.clientHeight) return;
    if (node.scrollHeight - node.scrollTop - node.clientHeight < 40) node.pinned = true;
    else if (performance.now() - touched < 1000) node.pinned = false;
  });
  node.addEventListener('load', () => {
    if (node.pinned && node.clientHeight) node.scrollTop = node.scrollHeight;
  }, true);
}

// Elements off the page lose their scroll position and can't be scrolled,
// so it is kept when a section is hidden and put back when it is shown. Areas
// that follow the newest line (data-stick="bottom") go back to the bottom
// if they were there, or if they were drawn while hidden.
function saveScroll(root) {
  for (const node of root.querySelectorAll('.scroll')) {
    node.savedScroll = { top: node.scrollTop, atBottom: node.pinned ?? node.scrollHeight - node.scrollTop - node.clientHeight < 40 };
  }
}

function restoreScroll(root) {
  for (const node of root.querySelectorAll('.scroll')) {
    const saved = node.savedScroll;
    node.savedScroll = null;
    if (node.dataset.stick === 'bottom' && (!saved || saved.atBottom)) {
      node.scrollTop = node.scrollHeight;
      node.pinned = true;
    } else if (saved) node.scrollTop = saved.top;
  }
}

// Load every other section in the background once the first is up, so
// even a first visit shows data straight away.
function prefetch() {
  for (const [id, view] of Object.entries(app.views)) {
    if (id === app.tab || view.root) continue;
    mountView(id);
    Promise.resolve().then(() => view.update()).catch((e) => console.warn(e));
  }
}

// Wrap long lines in the text editors, per the "Wrap editor lines" setting.
function applyWrap(textarea) {
  const wrap = !!app.status?.wrap_lines;
  textarea.classList.toggle('wrap', wrap);
  textarea.wrap = wrap ? 'soft' : 'off';
}

async function refreshStatus() {
  try {
    app.status = await api.get('/state');
    renderSidebar();
    document.querySelectorAll('textarea.page-editor').forEach(applyWrap);
  } catch (e) {
    console.warn(e);
  }
}

// Refetch what is on screen: the status and the section's data, together.
// Only one refetch runs at a time; asking during one runs another after it.
function loadNow() {
  if (refreshing) {
    refreshAgain = true;
    return;
  }
  // Loads started before now may miss the change being fetched for.
  loadEpoch++;
  refreshing = Promise.allSettled([
    refreshStatus(),
    Promise.resolve().then(() => app.views[app.tab]?.update()).catch((e) => console.warn(e)),
  ]).finally(() => {
    refreshing = null;
    if (refreshAgain) {
      refreshAgain = false;
      refresh();
    }
  });
}

// Live updates: a change refetches at once, and changes during a refetch
// are fetched together once it is done: over a slow link, a refetch for
// each would queue up in the browser faster than they finish (it opens only
// a few connections to one server), and what was just sent would wait
// behind them.
let refreshing = null;
let refreshAgain = false;
function refresh() {
  if (refreshing) {
    refreshAgain = true;
    return;
  }
  loadNow();
}

// Peers heard: the Network list and the Browser's nodes show them, and the
// sidebar counts them. A busy network announces several a second, so they
// refetch at most once every PEERS_EVERY ms (the latest included).
const PEERS_EVERY = 2000;
let peersTimer = null;
let peersFetched = 0;
function refreshPeers() {
  if (peersTimer) return;
  peersTimer = setTimeout(() => {
    peersTimer = null;
    peersFetched = Date.now();
    if (app.tab === 'network' || app.tab === 'browser') refresh();
    else refreshSidebar();
  }, Math.max(0, peersFetched + PEERS_EVERY - Date.now()));
}

// Just the status (counters and the log, every few seconds): the sidebar,
// and Status if it's on screen. A full refetch under way has it already.
let sidebarLoading = null;
function refreshSidebar() {
  if (refreshing || sidebarLoading) return;
  sidebarLoading = refreshStatus()
    .then(() => {
      if (app.tab === 'status') app.views.status.update({ settings: false });
    })
    .finally(() => {
      sidebarLoading = null;
    });
}

// Whether a view's data changed since it was last drawn (live updates
// refetch often; unchanged data needs no redraw).
function changed(view, slot, data) {
  const snapshot = JSON.stringify(data);
  view.snapshots ||= {};
  if (view.snapshots[slot] === snapshot) return false;
  view.snapshots[slot] = snapshot;
  return true;
}

// ---- notifications ----------------------------------------------------------
//
// rettui sends a notification for each new message, mention or whisper
// (as the settings, and each conversation's, hub's and room's own, say).
// This browser shows it with its own notifications once they're allowed,
// unless it's showing that conversation or room right now. Browsers only
// allow them on HTTPS or this computer (localhost): elsewhere, and until
// they're allowed, a note in the page stands in while it's being looked at.

const notifications = {
  // The service worker (phone browsers only show notifications through
  // one), once registered.
  worker: null,
  hinted: false,
  // Background notifications (Web Push) in this browser: 'on', 'off', or
  // null where they can't be had; and its subscription's address.
  push: null,
  endpoint: null,
  // Notifications shown without the service worker, by tag (to close them).
  open: new Map(),

  // 'granted', 'default', 'denied', 'insecure' (plain HTTP from elsewhere)
  // or 'unsupported'.
  state() {
    if (!window.isSecureContext) return 'insecure';
    if (!('Notification' in window)) return 'unsupported';
    return Notification.permission;
  },

  async start() {
    if (this.state() === 'insecure' || !('serviceWorker' in navigator)) return;
    try {
      this.worker = await navigator.serviceWorker.register('/sw.js');
      navigator.serviceWorker.addEventListener('message', (e) => {
        if (e.data && 'open' in e.data) openTarget(e.data.open);
      });
    } catch (e) {
      console.warn('No service worker:', e);
      return;
    }
    await this.checkPush().catch((e) => console.warn('Web Push:', e));
    // Say whether this browser shows rettui (it's pushed to only when it
    // doesn't), when that changes and every minute while it does.
    document.addEventListener('visibilitychange', () => this.showing());
    setInterval(() => document.visibilityState === 'visible' && this.showing(), 60_000);
    this.showing();
    app.views.status.update?.();
  },

  // Whether this browser has background notifications, and that rettui has
  // its subscription (made with rettui's current key).
  async checkPush() {
    const registration = await navigator.serviceWorker.ready;
    if (!registration.pushManager || this.state() !== 'granted') {
      this.push = registration.pushManager && this.state() !== 'denied' ? 'off' : null;
      return;
    }
    let subscription = await registration.pushManager.getSubscription();
    if (subscription) {
      const { key } = await api.get('/push');
      const current = subscription.options?.applicationServerKey;
      if (current && base64url(current) !== key) {
        // rettui's key changed (its data was reset): subscribe again.
        await subscription.unsubscribe();
        subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: fromBase64url(key) });
      }
      await api.post('/push/subscribe', subscription.toJSON());
    }
    this.endpoint = subscription?.endpoint || null;
    this.push = subscription ? 'on' : 'off';
  },

  async setPush(on) {
    const registration = await navigator.serviceWorker.ready;
    if (on) {
      if (this.state() !== 'granted') await Notification.requestPermission();
      if (this.state() !== 'granted') return toast('Notifications are blocked: allow them in this site\'s settings', true);
      const { key } = await api.get('/push');
      const subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: fromBase64url(key) });
      await api.post('/push/subscribe', subscription.toJSON());
      this.endpoint = subscription.endpoint;
      this.push = 'on';
      this.showing();
      toast('Background notifications are on in this browser');
    } else {
      const subscription = await registration.pushManager.getSubscription();
      if (subscription) {
        await api.post('/push/unsubscribe', { endpoint: subscription.endpoint }).catch(() => {});
        await subscription.unsubscribe();
      }
      this.endpoint = null;
      this.push = 'off';
      toast('Background notifications are off in this browser');
    }
    app.views.status.update?.();
  },

  // Tell rettui whether this browser shows it now (kept alive as the page
  // hides, which is when it matters).
  showing() {
    if (this.push !== 'on' || !this.endpoint) return;
    fetch('/api/push/showing', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ endpoint: this.endpoint, showing: document.visibilityState === 'visible' }),
      keepalive: true,
    }).catch(() => {});
  },

  // What a notification was about was read (here or elsewhere): close it.
  async close(tag) {
    this.open.get(tag)?.close();
    this.open.delete(tag);
    const registration = await navigator.serviceWorker?.getRegistration?.();
    for (const shown of (await registration?.getNotifications({ tag })) || []) shown.close();
  },

  async allow() {
    await Notification.requestPermission();
    if (this.state() === 'granted') toast('Notifications are on in this browser');
    else if (this.state() === 'denied') toast('Notifications are blocked: allow them in this site\'s settings', true);
    app.views.status.update?.();
  },

  // Whether what it's about is on screen, in a window that has the focus.
  onScreen(target) {
    if (!document.hasFocus()) return false;
    if (target.kind === 'conversation') {
      return showing('messages') && app.views.messages.selected === target.key;
    }
    if (target.kind === 'room') {
      const selected = app.views.channels.selected;
      return showing('channels') && selected?.hub === target.hub && selected.room === target.room;
    }
    return false;
  },

  async show(notification) {
    const { title, body, target, tag } = notification;
    if (this.onScreen(target)) return;
    // In the background with Web Push on, the push shows it (a page in the
    // background may be stopped at any moment).
    if (this.push === 'on' && document.visibilityState !== 'visible') return;
    if (this.state() === 'granted') {
      // A newer one about the same conversation or room replaces it.
      const options = { body, tag, renotify: true, icon: '/brand/icon.png', data: { target } };
      try {
        if (this.worker) return await (await navigator.serviceWorker.ready).showNotification(title, options);
        const shown = new Notification(title, options);
        this.open.set(tag, shown);
        shown.onclick = () => {
          window.focus();
          openTarget(target);
          shown.close();
        };
        return;
      } catch (e) {
        console.warn('Could not show a notification:', e);
      }
    }
    if (document.visibilityState !== 'visible') return;
    toast(`${title}: ${body}`);
    if (!this.hinted && this.state() === 'default') {
      this.hinted = true;
      toast('To be notified while rettui is in the background, allow notifications under Status');
    }
  },

  // For the Status page: how things stand here, and a button to allow them;
  // then background notifications, where the browser can have them.
  describe() {
    const state = this.state();
    const text = {
      granted: 'on in this browser',
      default: 'not allowed in this browser yet',
      denied: 'blocked in this browser (allow them in this site\'s settings)',
      insecure: 'not available: browsers only allow them on HTTPS or on this computer (localhost)',
      unsupported: 'not supported by this browser (on an iPhone, add rettui to the Home Screen first)',
    }[state];
    const allowed = el('div', { class: 'row' },
      el('span', { class: state === 'granted' ? 'state-ok' : 'dim', text }),
      state === 'default' ? el('button', { text: 'Allow', onclick: () => this.allow() }) : null);
    if (!this.push || state === 'denied') return allowed;
    const on = this.push === 'on';
    return el('div', {}, allowed,
      el('div', { class: 'row', style: 'margin-top:6px' },
        el('span', { class: on ? 'state-ok' : 'dim', text: on ? 'In the background: on' : 'In the background: off' }),
        el('button', { text: on ? 'Turn off' : 'Turn on', onclick: () => attempt(() => this.setPush(!on)) })),
      el('div', { class: 'dim', style: 'font-size:12.5px;margin-top:4px;max-width:34em', text:
        'Notifications while rettui isn\'t open here (a phone with it in the background, say) go through this '
        + 'browser\'s push service (Google, Apple, Mozilla or Microsoft), encrypted: it sees when one is sent, not what '
        + 'it says. rettui needs to reach the internet for them.' }));
  },
};

// Whether a section's open conversation or room is in front of the user: the
// section is open in a page that shows (not a background tab, or a phone's
// app in the background), and on a phone it shows the conversation, not the
// list. Only then does what arrives there count as read.
function showing(tab) {
  if (app.tab !== tab || document.visibilityState !== 'visible') return false;
  return !phone.matches || app.views[tab].pane === 'detail';
}

// Web Push keys are base64url text on the wire, and bytes to the browser.
function fromBase64url(text) {
  const plain = atob(text.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - text.length % 4) % 4));
  return Uint8Array.from(plain, (c) => c.charCodeAt(0));
}

function base64url(buffer) {
  return btoa(String.fromCharCode(...new Uint8Array(buffer))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

// A muted conversation, room or hub, in its list.
function mutedMark() {
  return el('span', { class: 'muted-mark', title: 'Notifications off', 'aria-label': 'notifications off', text: ' 🔕' });
}

// The header button for a conversation's, room's or hub's notifications,
// showing where they stand ('on', 'off', 'all' or 'mentions'): a bell,
// crossed out when they're off, and "All" beside it when every message
// notifies (not on a phone, which has no room for it).
function bellButton(level, title, onclick) {
  const labels = { on: 'on', off: 'off', all: 'all messages', mentions: 'mentions and whispers' };
  const said = `${title}: ${labels[level]}`;
  return el('button', { class: 'bell' + (level === 'off' ? ' off' : ''), title: said, 'aria-label': said, onclick },
    el('span', { text: level === 'off' ? '🔕' : '🔔' }), level === 'all' ? el('span', { class: 'bell-label', text: ' All' }) : null);
}

// Go to what a notification is about.
function openTarget(target) {
  if (target?.kind === 'conversation') {
    switchTab('messages');
    app.views.messages.select(target.key);
  } else if (target?.kind === 'room') {
    switchTab('channels');
    app.views.channels.select(target.hub, target.room);
  } else {
    switchTab('messages');
  }
}

let events = null;
// The last notification this browser had: reconnecting catches up on the
// ones missed meanwhile (those still unread).
let lastNotice = (() => {
  try {
    return localStorage.getItem('rettui.notice') || '';
  } catch {
    return '';
  }
})();
function listen() {
  events?.close();
  events = new EventSource('/api/events' + (lastNotice ? `?since=${lastNotice}` : ''));
  // Each change says what it touched: "all", or parts such as
  // "status,peers": the counters and the log ("status"), the hosted node's
  // counters ("node"), peers heard ("peers").
  events.onmessage = (e) => {
    const parts = new Set((e.data.split(' ')[1] || 'all').split(','));
    if (parts.has('all') || (parts.has('node') && app.tab === 'node')) refresh();
    else if (parts.has('peers')) refreshPeers();
    else refreshSidebar();
  };
  events.addEventListener('notify', (e) => {
    if (e.lastEventId) {
      lastNotice = e.lastEventId;
      try {
        localStorage.setItem('rettui.notice', lastNotice);
      } catch {}
    }
    try {
      const notification = JSON.parse(e.data);
      // Missed while away, and the page shows now: its unread counts say so.
      if (notification.replay && document.visibilityState === 'visible') return;
      notifications.show(notification);
    } catch (error) {
      console.warn(error);
    }
  });
  // Read, here or on another device: its notification goes.
  events.addEventListener('read', (e) => {
    try {
      notifications.close(JSON.parse(e.data).tag);
    } catch (error) {
      console.warn(error);
    }
  });
  // The browser reconnects by itself (unless the server refused, as it does
  // once the login token has changed); until then, the page may be out of
  // date, and says so.
  events.onerror = () => connectionLost(events.readyState === EventSource.CLOSED);
  events.onopen = () => {
    if (connectionBack()) refresh();
  };
}

// The banner for a lost connection, shown once it has been down a moment (a
// restart needn't flash it).
let connectionTimer = null;
let connectionDown = false;
function connectionLost(closed) {
  connectionDown = true;
  clearTimeout(connectionTimer);
  connectionTimer = setTimeout(() => {
    $('#connection').replaceChildren(...[
      el('span', { text: closed ? 'Disconnected from rettui' : navigator.onLine ? 'Connection lost: reconnecting…' : 'Offline: reconnecting when back online…' }),
      closed ? el('button', { text: 'Reload', onclick: () => location.reload() }) : null,
    ].filter(Boolean));
    $('#connection').classList.remove('hidden');
  }, closed ? 0 : 2000);
}

// Connected again: true if it had been lost (what changed meanwhile needs
// fetching).
function connectionBack() {
  clearTimeout(connectionTimer);
  $('#connection').classList.add('hidden');
  const was = connectionDown;
  connectionDown = false;
  return was;
}

window.addEventListener('offline', () => connectionLost(false));
// Back to the page (a phone waking, a tab brought forward): catch up, and
// start again if the connection gave up meanwhile.
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState !== 'visible' || !events) return;
  if (events.readyState === EventSource.CLOSED) listen();
  refresh();
});

// Cross-tab links (from pages and the network list).
function openConversation(address) {
  attempt(async () => {
    const { key } = await api.post('/conversations', { address });
    app.views.messages.selected = key;
    switchTab('messages', { focus: true });
    setPane(app.views.messages, 'detail');
  });
}

async function openHubLink(link) {
  // As in the TUI, only a hub not added yet asks first.
  const hash = (link.match(/[0-9a-f]{32}/i) || [''])[0].toLowerCase();
  const hubs = await api.get('/channels').catch(() => []);
  const known = hubs.some((h) => h.hash === hash);
  if (!known && !confirm(`Open RRC hub ${link}?\nConnecting tells the hub who you are.`)) return;
  const result = await attempt(() => api.post('/channels', { address: link }));
  if (result) {
    app.views.channels.selected = { hub: result.hub, room: result.room || '' };
    switchTab('channels');
  }
}

function browse(url) {
  switchTab('browser');
  app.views.browser.go(url);
}

// ---- Messages ---------------------------------------------------------------

app.views.messages = {
  panes: true,
  selected: null,
  pending: [],
  // Conversations loaded so far, by key: a click draws one at once and
  // refreshes it behind.
  cache: new Map(),
  // How many of the most recent conversations are loaded ahead.
  WARM: 10,
  // Messages shown (and loaded) until "Show earlier".
  LIMIT: 100,
  mode: localStorage.getItem('rettui.mode') || 'auto',

  mount(root, options = {}) {
    this.list = el('div', { class: 'scroll' });
    this.header = el('header');
    this.history = el('div', { class: 'scroll history', dataset: { stick: 'bottom' } });
    this.chips = el('div', { class: 'chips' });
    this.text = el('textarea', {
      placeholder: composePlaceholder(),
      enterkeyhint: 'send',
      rows: 2,
      onkeydown: (e) => {
        // Enter while an input method is composing confirms the text.
        if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
          e.preventDefault();
          this.send();
        }
      },
    });
    this.fileInput = el('input', {
      type: 'file',
      multiple: true,
      class: 'hidden',
      onchange: () => {
        this.pending.push(...this.fileInput.files);
        this.fileInput.value = '';
        this.renderChips();
      },
    });
    const mode = el('select', { title: 'Delivery mode', onchange: (e) => {
      this.mode = e.target.value;
      localStorage.setItem('rettui.mode', this.mode);
    } },
    ['auto', 'direct', 'propagated'].map((m) => el('option', { value: m, text: m[0].toUpperCase() + m.slice(1), selected: m === this.mode })));
    this.compose = el('div', { class: 'compose' },
      this.chips,
      el('div', { class: 'row' }, this.text),
      el('div', { class: 'row' },
        el('button', { text: '📎 Attach', onclick: () => this.fileInput.click() }),
        mode,
        el('span', { class: 'grow' }),
        el('button', { class: 'primary', text: 'Send', onclick: () => this.send() })),
      this.fileInput);
    root.append(
      el('section', { class: 'panel side' },
        el('header', {}, el('span', { class: 'title grow', text: 'Conversations' }),
          el('button', { text: '+ New', onclick: () => this.newConversation() })),
        this.list),
      el('section', { class: 'panel grow pane-main' }, this.header, this.history, this.compose));
    this.renderChips();
    this.lastKey = null;
    this.shown(options);
  },

  shown(options = {}) {
    if (options.focus) setTimeout(() => this.text.focus(), 50);
  },

  newConversation() {
    const address = prompt('LXMF address (32 hex characters)');
    if (address) openConversation(address);
  },

  renderChips() {
    this.chips.replaceChildren(...this.pending.map((file, i) => el('span', { class: 'chip' },
      `${file.name} (${humanBytes(file.size)})`,
      el('button', { text: '×', title: 'Remove', onclick: () => {
        this.pending.splice(i, 1);
        this.renderChips();
      } }))));
  },

  async update() {
    // The list and the open conversation, fetched together when known. The
    // conversation is drawn as soon as it arrives (what was just sent shows
    // without waiting for the list).
    draftSwitch(this, this.selected);
    const known = this.selected;
    const early = known ? this.load(known).catch(() => null) : null;
    early?.then((conversation) => {
      if (conversation && this.selected === known) this.renderConversation(known, conversation);
    });
    const conversations = await api.get('/conversations');
    if (!this.selected && conversations.length) this.selected = conversations[0].key;
    draftSwitch(this, this.selected);
    this.names = new Map(conversations.map((c) => [c.key, c.name]));
    this.warmRecent(conversations);
    if (changed(this, 'list:' + this.selected, conversations)) this.list.replaceChildren(...(conversations.length ? conversations.map((c) => el('div', {
      class: 'list-item' + (c.key === this.selected ? ' selected' : ''),
      dataset: { key: c.key },
      // Start loading as the button goes down; the click shows it.
      onpointerdown: () => this.load(c.key).catch(() => {}),
      onclick: () => this.select(c.key),
    },
    el('div', { class: 'main' },
      el('div', { class: 'name' }, c.name, c.muted ? mutedMark() : null),
      el('div', { class: 'sub', text: c.last ? `${c.last.incoming ? '' : 'You: '}${c.last.text}` : 'No messages yet' })),
    el('div', { class: 'dim', style: 'font-size:12px;text-align:right' },
      c.last ? timeLabel(c.last.timestamp) : '',
      c.unread ? el('div', {}, el('span', { class: 'badge', text: c.unread })) : null))) :
      [el('div', { class: 'empty', text: 'No conversations yet. Press + New, or message a peer from the Network tab.' })]));
    this.compose.classList.toggle('hidden', !this.selected);
    if (!this.selected) {
      this.header.replaceChildren(el('span', { class: 'title', text: 'Messages' }));
      this.history.replaceChildren();
      return;
    }
    const key = this.selected;
    const conversation = (key === known && await early) || await this.load(key);
    if (key !== this.selected) return;
    if (conversation.unread && showing('messages')) api.post(`/conversations/${key}/read`).catch(() => {});
    this.renderConversation(key, conversation);
  },

  // The newest messages, which is what shows (all of them once asked for).
  load(key) {
    return loadInto(this, key, '/conversations/' + key + (this.showAll === key ? '' : `?last=${this.LIMIT}`));
  },

  echo(key, node) {
    const shown = this.lastKey === key;
    return echoSent(this, { key, node, parent: shown && this.history, scroller: this.history, slot: 'conversation' });
  },

  // Open a conversation: at once from what is loaded (or its name while it
  // loads), then fresh.
  select(key) {
    this.selected = key;
    draftSwitch(this, key);
    setPane(this, 'detail');
    for (const item of this.list.children) item.classList.toggle('selected', item.dataset.key === key);
    const cached = this.cache.get(key);
    if (cached) this.renderConversation(key, cached);
    else {
      (this.snapshots ||= {}).conversation = null;
      this.lastKey = null;
      this.header.replaceChildren(el('span', { class: 'title', text: this.names?.get(key) || key }),
        el('span', { class: 'dim mono grow', style: 'font-weight:400;font-size:12.5px', text: key }));
      this.history.replaceChildren(el('div', { class: 'empty', text: 'Loading…' }));
    }
    this.update();
  },

  // Load the most recent conversations ahead, and again when they change.
  warmRecent(conversations) {
    const stale = conversations.slice(0, this.WARM).filter((c) => {
      const cached = this.cache.get(c.key);
      return !cached || cached.messages.at(-1)?.timestamp !== c.last?.timestamp;
    });
    if (stale.length) warm(stale.map((c) => () => this.load(c.key)));
  },

  renderConversation(key, conversation) {
    if (!changed(this, 'conversation', { key, conversation, all: this.showAll === key })) return;
    this.header.replaceChildren(
      el('span', { class: 'title', text: conversation.name }),
      el('span', { class: 'dim mono grow', style: 'font-weight:400;font-size:12.5px', text: key }),
      bellButton(conversation.muted ? 'off' : 'on', 'Notifications from this conversation', () => this.setMuted(key, !conversation.muted)),
      el('button', { text: 'Copy address', onclick: () => copy(key, 'LXMF address') }));
    // The newest messages only, unless asked for all: those not loaded, and
    // any loaded but not shown.
    const from = this.showAll === key ? 0 : Math.max(0, conversation.messages.length - this.LIMIT);
    const hidden = from + (conversation.total ?? conversation.messages.length) - conversation.messages.length;
    // Older still are in the archive, which rettui doesn't show.
    const archived = conversation.archived
      ? el('div', { class: 'show-more' }, el('span', { class: 'dim', style: 'overflow-wrap:anywhere', text: `${conversation.archived} older ${conversation.archived === 1 ? 'message was' : 'messages were'} moved to the archive (${conversation.archive})` }))
      : null;
    const earlier = hidden ? el('div', { class: 'show-more' }, el('button', { text: `Show ${hidden} earlier messages`, onclick: () => {
      this.showAll = key;
      this.update();
    } })) : archived;
    const echoes = echoesFor(this, key);
    const render = () => this.history.replaceChildren(...(conversation.messages.length || echoes.length
      ? [earlier, ...conversation.messages.slice(from).map((m) => this.message(m, conversation))].filter(Boolean)
      : [el('div', { class: 'empty', text: 'No messages yet. Say hello!' })]), ...echoes);
    if (this.lastKey !== key) {
      render();
      this.history.scrollTop = this.history.scrollHeight;
      this.history.pinned = true;
      this.lastKey = key;
    } else {
      stickToBottom(this.history, render);
    }
  },

  async setMuted(key, muted) {
    const done = await attempt(() => api.post(`/conversations/${key}/notify`, { muted }),
      muted ? 'No notifications from this conversation' : 'Notifications from this conversation are on');
    if (done) this.update();
  },

  message(m, conversation) {
    const state = {
      received: m.state.verified ? null : el('span', { class: 'state-warn', text: ' unverified' }),
      sending: el('span', { class: 'dim', text: ' sending…' }),
      delivered: el('span', { class: 'state-ok', text: ' ✓' }),
      propagated: el('span', { class: 'state-ok', text: ' ✓ via propagation node' }),
      failed: el('span', { class: 'state-bad', text: ` failed: ${m.state.error}` }),
    }[m.state.kind];
    const base = `/api/conversations/${conversation.key}/attachments/${encodeURIComponent(m.id)}/`;
    return el('div', { class: 'message ' + (m.incoming ? 'in' : 'out') },
      el('div', { class: 'meta' },
        el('span', { class: 'author ' + (m.incoming ? 'in' : 'out'), text: m.incoming ? conversation.name : 'You' }),
        el('span', { class: 'dim', text: '  ' + timeLabel(m.timestamp) }),
        state),
      m.title ? el('div', { class: 'title', text: m.title }) : null,
      m.content ? el('div', { class: 'content', text: m.content }) : null,
      m.attachments.map((a) => {
        const url = base + a.index;
        if (!a.exists) return el('div', { class: 'attachment dim', text: `📎 ${a.name} (file missing)` });
        if (a.image) {
          return el('div', { class: 'attachment' },
            el('a', { href: url, target: '_blank', rel: 'noopener' }, el('img', { src: url, alt: a.name, loading: 'lazy' })));
        }
        return el('div', { class: 'attachment' },
          el('a', { href: url, download: a.name, text: `📎 ${a.name}` }),
          el('span', { class: 'dim', text: ` ${humanBytes(a.size)}` }));
      }));
  },

  // What's written (and attached) in the box, if anything (see draftSwitch).
  takeDraft() {
    return this.text.value || this.pending.length ? { text: this.text.value, files: this.pending } : null;
  },

  putDraft(draft) {
    this.text.value = draft?.text || '';
    this.pending = draft?.files || [];
    this.renderChips();
  },

  joinDrafts(first, second) {
    if (!second) return first;
    return { text: [first.text, second.text].filter(Boolean).join('\n'), files: [...first.files, ...second.files] };
  },

  async send() {
    const content = this.text.value;
    const pending = this.pending;
    if (!content.trim() && !pending.length) return;
    // Take the message out of the box at once, so a second Enter (or a
    // double click) while this one is on its way has nothing to send, and
    // anything typed meanwhile is kept.
    this.text.value = '';
    this.pending = [];
    this.renderChips();
    // Show it straight away as sending; the next update draws the real one.
    const echo = this.echo(this.selected, el('div', { class: 'message out echo' },
      el('div', { class: 'meta' },
        el('span', { class: 'author out', text: 'You' }),
        el('span', { class: 'dim', text: '  ' + timeLabel(Date.now() / 1000) }),
        el('span', { class: 'dim', text: ' sending…' })),
      content ? el('div', { class: 'content', text: content }) : null,
      pending.map((f) => el('div', { class: 'attachment dim', text: `📎 ${f.name}` }))));
    const files = [];
    for (const file of pending) files.push({ name: file.name, data: await readFile(file) });
    const total = pending.reduce((n, f) => n + f.size, 0);
    if (total > 1_000_000) toast(`Sending ${humanBytes(total)} of attachments; many clients reject direct transfers over 1 MB`);
    const sent = await attempt(() => api.post(`/conversations/${echo.key}/send`, { content, mode: this.mode, files }));
    echo.done(!sent);
    if (!sent) {
      // Put it back to try again, in the conversation it was for.
      draftRestore(this, echo.key, { text: content, files: pending });
      return;
    }
    this.history.scrollTop = this.history.scrollHeight;
    this.history.pinned = true;
  },
};

// ---- Channels ---------------------------------------------------------------

app.views.channels = {
  panes: true,
  // Rooms, whispers and hub pages loaded so far, by "hub/room": a click
  // draws one at once and refreshes it behind.
  cache: new Map(),
  selected: null, // { hub, room } — room '' is the hub itself
  // Lines shown (and loaded) until "Show earlier".
  LIMIT: 200,

  mount(root) {
    this.list = el('div', { class: 'scroll' });
    this.header = el('header');
    this.body = el('div', { class: 'scroll', dataset: { stick: 'bottom' } });
    this.input = el('input', {
      type: 'text',
      enterkeyhint: 'send',
      onkeydown: (e) => {
        if (this.mentionKey(e)) return;
        if (e.key === 'Enter' && !e.isComposing) this.send();
      },
      oninput: () => this.updateMentions(),
      // The cursor moved: the name being typed may have changed.
      onkeyup: (e) => ['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key) && this.updateMentions(),
      onclick: () => this.updateMentions(),
      onblur: () => this.hideMentions(),
    });
    // Who `@` can mention, narrowed as the name is typed.
    this.mentionList = el('div', { class: 'mention-list hidden', role: 'listbox' });
    this.inputBar = el('div', { class: 'chat-input' }, this.mentionList, this.input,
      el('button', { class: 'primary', text: 'Send', onclick: () => this.send() }));
    this.members = el('div', { class: 'scroll' });
    this.membersPanel = el('section', { class: 'panel members' }, el('header', { text: 'Members' }), this.members);
    root.append(
      el('section', { class: 'panel side' },
        el('header', {}, el('span', { class: 'title grow', text: 'Channels' }),
          el('button', { text: '+ Add hub', onclick: () => this.addHub() })),
        this.list),
      el('section', { class: 'panel grow pane-main' }, this.header, this.body, this.inputBar),
      this.membersPanel);
    this.lastView = null;
  },

  async addHub() {
    const address = prompt('RRC hub address (32 hex characters, or rrc://address/room)');
    if (!address) return;
    const result = await attempt(() => api.post('/channels', { address }));
    if (result) {
      this.selected = { hub: result.hub, room: result.room || '' };
      setPane(this, 'detail');
      this.update();
    }
  },

  hubAction(hub, action, body = {}) {
    return attempt(() => api.post(`/channels/${hub}/${action}`, body));
  },

  async update() {
    // The hub list and the open room, fetched together when known.
    // The room is drawn as soon as it arrives, with the hubs from last time
    // (what was just sent shows without waiting for the hub list).
    const known = this.selected && { ...this.selected };
    if (known) draftSwitch(this, known.hub + '/' + known.room);
    const early = known ? this.load(known.hub, known.room).catch(() => null) : null;
    early?.then((view) => {
      const { hub, room } = this.selected || {};
      if (view && this.hubs && hub === known.hub && room === known.room) this.renderRoom(hub, room, view);
    });
    const hubs = await api.get('/channels');
    this.hubs = hubs;
    this.warmRooms(hubs);
    if (this.selected && !hubs.some((h) => h.hash === this.selected.hub)) this.selected = null;
    if (!this.selected && hubs.length) {
      const first = hubs[0];
      this.selected = { hub: first.hash, room: first.rooms.find((r) => r.joined)?.name || '' };
    }
    if (this.selected) draftSwitch(this, this.selected.hub + '/' + this.selected.room);
    const items = [];
    for (const hub of hubs) {
      const isSelected = (room) => this.selected && this.selected.hub === hub.hash && this.selected.room === room;
      const select = (room) => () => this.select(hub.hash, room);
      // Start loading as the button goes down; the click shows it.
      const press = (room) => () => this.load(hub.hash, room).catch(() => {});
      items.push(el('div', { class: 'list-item' + (isSelected('') ? ' selected' : ''), dataset: { key: hub.hash + '/' }, onpointerdown: press(''), onclick: select('') },
        el('span', { class: 'dot ' + hub.status.kind, text: hub.status.kind === 'disconnected' ? '○' : hub.status.kind === 'connecting' ? '◌' : '●' }),
        el('span', { class: 'main name' }, hub.name, hub.notify === 'off' ? mutedMark() : null),
        hub.unread ? el('span', { class: 'badge' + (hub.mention ? ' mention' : ''), text: hub.unread }) : null));
      for (const room of hub.rooms) {
        items.push(el('div', { class: 'list-item room-item' + (room.joined ? '' : ' parted') + (isSelected(room.name) ? ' selected' : ''), dataset: { key: hub.hash + '/' + room.name }, onpointerdown: press(room.name), onclick: select(room.name) },
          el('span', { class: 'main name' }, '# ' + room.name, room.notify === 'off' ? mutedMark() : null),
          room.unread ? el('span', { class: 'badge' + (room.mention ? ' mention' : ''), text: room.unread }) : null));
      }
      // Whisper conversations: "@" where rooms have "#".
      for (const whisper of hub.whispers) {
        items.push(el('div', { class: 'list-item room-item whisper-item' + (isSelected(whisper.key) ? ' selected' : ''), dataset: { key: hub.hash + '/' + whisper.key }, onpointerdown: press(whisper.key), onclick: select(whisper.key), title: 'Whisper conversation' },
          el('span', { class: 'main name' }, el('span', { class: 'whisper-icon', text: '@ ' }), whisper.name, whisper.notify === 'off' ? mutedMark() : null),
          whisper.unread ? el('span', { class: 'badge mention', text: whisper.unread }) : null));
      }
    }
    if (changed(this, 'list', { hubs, selected: this.selected })) {
      this.list.replaceChildren(...(items.length ? items : [el('div', { class: 'empty', text: 'No hubs yet. Add one by address or rrc:// link; NomadNet pages can also link to hubs.' })]));
    }

    if (!this.selected) {
      this.header.replaceChildren(el('span', { class: 'title', text: 'Channels' }));
      this.body.replaceChildren();
      this.inputBar.classList.add('hidden');
      this.membersPanel.classList.add('hidden');
      return;
    }
    const { hub: hash, room } = this.selected;
    const same = known && known.hub === hash && known.room === room;
    const view = (same && await early) || await this.load(hash, room);
    if (!this.selected || this.selected.hub !== hash || this.selected.room !== room) return;
    const current = this.hubs.find((h) => h.hash === hash);
    const whisperEntry = view.whisper_with && current?.whispers.find((w) => w.key === room);
    const unreadNow = view.whisper_with ? whisperEntry?.unread : room ? current?.rooms.find((r) => r.name === room)?.unread : current?.unread;
    if (current && unreadNow && showing('channels')) api.post(`/channels/${hash}/read`, { room }).catch(() => {});
    this.renderRoom(hash, room, view);
  },

  // The newest lines, which is what shows (all of them once asked for).
  load(hub, room) {
    const all = this.showAll === hub + '/' + room;
    return loadInto(this, hub + '/' + room, `/channels/${hub}/room?name=${encodeURIComponent(room)}` + (all ? '' : `&last=${this.LIMIT}`));
  },

  // Open a room, whisper or hub page: at once from what is loaded (or its
  // name while it loads), then fresh.
  select(hash, room) {
    this.selected = { hub: hash, room };
    draftSwitch(this, hash + '/' + room);
    setPane(this, 'detail');
    this.hideMentions();
    for (const item of this.list.children) item.classList.toggle('selected', item.dataset.key === hash + '/' + room);
    const cached = this.cache.get(hash + '/' + room);
    if (cached && this.hubs?.some((h) => h.hash === hash)) this.renderRoom(hash, room, cached);
    else {
      (this.snapshots ||= {}).room = null;
      this.lastView = null;
      const hub = this.hubs?.find((h) => h.hash === hash);
      const whisper = hub?.whispers.find((w) => w.key === room);
      this.header.replaceChildren(el('span', { class: 'title grow', text: whisper ? '@ ' + whisper.name : room ? '#' + room : hub?.name || '' }));
      this.membersPanel.classList.add('hidden');
      this.body.replaceChildren(el('div', { class: 'empty', text: 'Loading…' }));
    }
    this.update();
  },

  // Load every room, whisper and hub page ahead (hubs have few), and again
  // when one has new lines (its unread count changed).
  warmRooms(hubs) {
    this.seenUnread ||= new Map();
    const loads = [];
    for (const hub of hubs) {
      const entries = [{ key: '', unread: hub.unread }, ...hub.rooms.map((r) => ({ key: r.name, unread: r.unread })), ...hub.whispers.map((w) => ({ key: w.key, unread: w.unread }))];
      for (const { key, unread } of entries) {
        const cacheKey = hub.hash + '/' + key;
        const selected = this.selected && this.selected.hub === hub.hash && this.selected.room === key;
        const moved = (this.seenUnread.get(cacheKey) || 0) !== (unread || 0);
        this.seenUnread.set(cacheKey, unread || 0);
        if (!selected && (!this.cache.has(cacheKey) || moved)) loads.push(() => this.load(hub.hash, key));
      }
    }
    if (loads.length) warm(loads);
  },

  renderRoom(hash, room, view) {
    const hub = this.hubs.find((h) => h.hash === hash);
    if (!hub) return;
    if (!changed(this, 'room', { hash, room, view, hub, all: this.showAll === hash + '/' + room })) return;
    this.inputBar.classList.remove('hidden');
    const whisper = view.whisper_with;
    this.input.placeholder = whisper ? `Whisper to ${whisper.name}` + (view.whisper ? '' : '  (this hub doesn\'t pass whispers)')
      : room ? `Message #${room} as ${view.nick}  (/help for commands)` : `Commands for ${hub.name}  (/join, /nick, /list, /help)`;
    const viewKey = hash + '/' + room;
    this.view = view;
    // Someone joined or left while choosing whom to mention.
    if (this.mentionMatches) this.updateMentions();
    // The newest lines only, unless asked for all: those not loaded, and any
    // loaded but not shown.
    const from = this.showAll === viewKey ? 0 : Math.max(0, view.lines.length - this.LIMIT);
    const hidden = from + (view.total_lines ?? view.lines.length) - view.lines.length;
    const earlier = hidden ? el('div', { class: 'show-more' }, el('button', { text: `Show ${hidden} earlier lines`, onclick: () => {
      this.showAll = viewKey;
      this.update();
    } })) : null;
    const render = () => {
      const chat = this.chat(view.lines.slice(from));
      chat.append(...echoesFor(this, viewKey));
      this.body.replaceChildren(...(room || hidden ? [] : this.hubInfo(hub)), ...(earlier ? [earlier] : []), chat);
    };
    if (this.lastView !== viewKey) {
      render();
      this.body.scrollTop = this.body.scrollHeight;
      this.body.pinned = true;
      this.lastView = viewKey;
    } else {
      stickToBottom(this.body, render);
    }

    if (whisper) {
      this.membersPanel.classList.add('hidden');
      this.header.replaceChildren(
        el('span', { class: 'title grow' }, el('span', { class: 'whisper-icon', text: '@ ' }), whisper.name,
          el('span', { class: 'dim', style: 'font-weight:400', text: ' · whisper' + (view.whisper ? '' : ' (this hub doesn\'t pass whispers)') })),
        el('button', { text: 'Actions', onclick: (e) => this.userMenu(e, whisper.src) }),
        bellButton(view.notify_level === 'off' ? 'off' : 'on', `Notifications from ${whisper.name}`, (e) => this.notifyMenu(e, hub, room, view)),
        el('button', { class: 'danger more', text: 'Close', title: 'Close the conversation and delete its messages', onclick: async () => {
          if (!confirm(`Close the whisper conversation with ${whisper.name} and delete its messages?`)) return;
          await this.hubAction(hash, 'forget', { room });
          this.selected = { hub: hash, room: '' };
          this.update();
        } }), moreButton());
    } else if (room) {
      const entry = hub.rooms.find((r) => r.name === room);
      this.header.replaceChildren(
        el('span', { class: 'title grow' }, '#' + room, view.topic ? el('span', { class: 'dim', style: 'font-weight:400', text: ' — ' + view.topic }) : null),
        view.joined
          ? el('button', { text: 'Leave', onclick: () => this.hubAction(hash, 'leave', { room }) })
          : el('button', { text: 'Join', onclick: () => this.hubAction(hash, 'join', { room }) }),
        el('button', { class: 'phone-only', text: `Members ${view.members.length}`, onclick: () => setPane(this, 'members') }),
        bellButton(view.notify_level, `Notifications in #${room}`, (e) => this.notifyMenu(e, hub, room, view)),
        // A setting ("Show joins and leaves"), here where it matters.
        el('label', { class: 'toggle', title: 'Show people joining and leaving the room in the chat' },
          el('input', { type: 'checkbox', checked: view.show_joins, onchange: (e) => attempt(
            () => api.post('/settings', { values: { show_joins: String(e.target.checked) } }),
            e.target.checked ? 'Showing people joining and leaving' : 'Hiding people joining and leaving') }),
          ' Show joins'),
        el('button', { class: 'more', text: 'Copy link', onclick: () => copy(entry?.link, 'link') }),
        el('button', { class: 'danger more', text: 'Forget', title: 'Leave and delete the messages', onclick: async () => {
          if (!confirm(`Leave #${room} and delete its messages?`)) return;
          await this.hubAction(hash, 'forget', { room });
          this.selected = { hub: hash, room: '' };
          this.update();
        } }), moreButton());
      this.membersPanel.classList.remove('hidden');
      $('header', this.membersPanel).textContent = `Members ${view.members.length}`;
      this.members.replaceChildren(...view.members.map((member) => el('div', {
        class: 'member' + (member.own ? ' own' : ' clickable'),
        text: member.name,
        title: member.own ? 'You' : 'Message this user',
        onclick: member.own ? null : (e) => this.userMenu(e, member.src),
      })));
    } else {
      this.membersPanel.classList.add('hidden');
      const busy = hub.status.kind === 'connected' || hub.status.kind === 'connecting';
      this.header.replaceChildren(
        el('span', { class: 'title grow', text: hub.name }),
        el('button', { text: busy ? 'Disconnect' : 'Connect', onclick: () => this.hubAction(hash, 'connect') }),
        bellButton(hub.notify, `Notifications from ${hub.name}`, (e) => this.notifyMenu(e, hub, '', view)),
        el('button', { class: 'more', text: 'Copy link', onclick: () => copy(hub.link, 'link') }),
        el('button', { class: 'danger more', text: 'Remove hub', onclick: async () => {
          if (!confirm(`Remove hub ${hub.name} and its history?`)) return;
          await this.hubAction(hash, 'remove');
          this.selected = null;
          setPane(this, 'list');
          this.update();
        } }), moreButton());
    }
  },

  // What gets a notification: in a room or whisper conversation, or (with
  // `room` empty) in a hub's rooms unless they're set otherwise.
  notifyMenu(event, hub, room, view) {
    event.stopPropagation();
    const labels = { all: 'all messages', mentions: 'mentions and whispers', off: 'off' };
    const whisper = !!view?.whisper_with;
    const hubLevel = whisper ? (hub.notify === 'off' ? 'off' : 'on') : labels[hub.notify];
    const choices = !room ? [['all', 'All messages in its rooms'], ['mentions', 'Mentions and whispers'], ['off', 'Off']]
      : whisper ? [['default', `As the hub (${hubLevel})`], ['mentions', 'On'], ['off', 'Off']]
        : [['default', `As the hub (${hubLevel})`], ['all', 'All messages'], ['mentions', 'Mentions and whispers'], ['off', 'Off']];
    // Whispers notify at either level but off.
    const current = !room ? hub.notify : whisper && view.notify === 'all' ? 'mentions' : view.notify;
    const where = !room ? `from ${hub.name}` : whisper ? `from ${view.whisper_with.name}` : `in #${room}`;
    const title = `Notifications ${where}` + (room ? '' : ' (each room can differ)');
    openSheet(title, choices.map(([level, text]) => ({
      text,
      checked: level === current,
      action: async () => {
        if (!await this.hubAction(hub.hash, 'notify', { room, level })) return;
        const now = level === 'default' ? `as the hub (${hubLevel})` : whisper ? (level === 'off' ? 'off' : 'on') : labels[level];
        toast(`Notifications ${where}: ${now}`);
        this.update();
      },
    })), event.currentTarget);
  },

  hubInfo(hub) {
    const status = {
      connected: el('span', { class: 'state-ok', text: 'connected' }),
      connecting: el('span', { class: 'state-warn', text: `connecting: ${hub.status.text || ''}` }),
      failed: el('span', { class: 'state-bad', text: `failed: ${hub.status.text || ''}` }),
      disconnected: el('span', { class: 'dim', text: 'disconnected' }),
    }[hub.status.kind];
    const nodes = [el('div', { class: 'hub-info' },
      el('span', { class: 'label', text: 'Status' }), status,
      el('span', { class: 'label', text: 'Address' }), el('span', { class: 'mono' }, hub.hash, el('span', { class: 'dim', text: '  ' + hub.aspect })),
      el('span', { class: 'label', text: 'Nick' }), el('span', { text: hub.nick || `${hub.display_name} (display name)` }),
      el('span', { class: 'label', text: 'Auto' }), el('span', {},
        el('label', {}, el('input', { type: 'checkbox', checked: hub.auto_connect, onchange: () => this.hubAction(hub.hash, 'auto') }), ' reconnect automatically')),
      el('span', { class: 'label', text: 'Limits' }), el('span', { text: `${hub.limits.message_bytes} bytes per message, ${hub.limits.rooms} rooms` }),
      el('span', { class: 'label', text: 'Notify' }), el('span', { text: `${{ all: 'all messages', mentions: 'mentions and whispers', off: 'off' }[hub.notify]} (the bell above; each room can differ)` }))];
    if (hub.motd) nodes.push(el('div', { class: 'motd', text: hub.motd }));
    if (hub.available) {
      nodes.push(el('div', { class: 'public-rooms' },
        el('div', { style: 'font-weight:700', text: hub.available.length ? 'Public rooms' : 'No public rooms on this hub; /join <name> to create one' }),
        hub.available.map((r) => el('div', { class: 'public-room' },
          r.joined
            ? el('span', { class: 'state-ok', text: '# ' + r.name })
            : el('a', { href: '#', text: '# ' + r.name, onclick: async (e) => {
              e.preventDefault();
              const result = await this.hubAction(hub.hash, 'join', { room: r.name });
              if (result) {
                this.selected = { hub: hub.hash, room: result.room };
                this.update();
              }
            } }),
          r.topic ? el('span', { class: 'dim', text: r.topic }) : null))));
    }
    return nodes;
  },

  chat(lines) {
    return el('div', { class: 'chat' }, lines.map((line) => {
      const name = line.nick || '';
      const color = line.own ? 'var(--accent)' : nickColor(line.src);
      const time = new Date(line.ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
      // Other people's names open the user menu.
      const user = !line.own && line.src && this.view?.users[line.src] && ['msg', 'action', 'private'].includes(line.kind);
      // In a whisper conversation, whispers read like ordinary chat.
      const kind = this.view?.whisper_with && line.kind === 'private' ? 'msg' : line.kind;
      const attrs = (text, style) => ({
        class: 'prefix' + (user ? ' clickable' : ''), style, text,
        title: user ? 'Message this user' : null,
        onclick: user ? (e) => this.userMenu(e, line.src) : null,
      });
      let prefix;
      switch (kind) {
        case 'msg': prefix = el('span', attrs(`<${name}> `, `color:${color}`)); break;
        case 'action': prefix = el('span', attrs(`* ${name} `, `color:${color}`)); break;
        case 'private': prefix = el('span', attrs(line.own ? `» to ${name}: ` : `» ${name} (private): `)); break;
        case 'notice': prefix = el('span', { class: 'prefix', text: '-hub- ' }); break;
        case 'error': prefix = el('span', { class: 'prefix', text: '! ' }); break;
        default: prefix = el('span', { class: 'prefix', text: '— ' });
      }
      return el('div', { class: 'chat-line ' + kind },
        el('span', { class: 'time', text: time }),
        el('span', { class: 'body' }, prefix,
          el('span', { class: 'text' + (line.pending ? ' pending' : '') }, ...this.marked(line.text, line.highlights || [], line.mentions || [])),
          line.pending ? el('span', { class: 'pending', text: ' …' }) : null));
    }));
  },

  // The `@name` being typed before the cursor: where its `@` is and what
  // follows (as the server's `mention_prefix`). The `@` starts a word (not
  // an address like a@b). Names can have spaces (`@ann b`), so what follows
  // may too, but not at its end: after a space it's up to the next letter
  // whether a longer name is meant.
  mentionQuery() {
    const input = this.input;
    const caret = input.selectionStart ?? input.value.length;
    if (input.selectionEnd !== caret) return null;
    const before = input.value.slice(0, caret);
    // The last `@` that starts a word: an `@` inside a name (`@a@b`) doesn't.
    let at = before.lastIndexOf('@');
    while (at >= 0 && /[\p{Alphabetic}\p{N}_]$/u.test(before.slice(0, at))) at = before.lastIndexOf('@', at - 1);
    if (at < 0) return null;
    const partial = before.slice(at + 1);
    if (/\s$/u.test(partial)) return null;
    return { start: at, end: caret, partial };
  },

  // Show who matches what is typed after `@`: names starting with it first,
  // then names containing it.
  updateMentions() {
    const query = this.mentionQuery();
    const people = this.selected?.room ? (this.view?.mentionable || []).filter((u) => !u.own) : [];
    if (!query || query.start === this.mentionDismissed) {
      if (!query) this.mentionDismissed = null;
      return this.hideMentions();
    }
    // Once it has a space it's a name being finished, not a search.
    const partial = query.partial.toLowerCase();
    const search = !/\s/u.test(partial);
    const starts = people.filter((u) => u.name.toLowerCase().startsWith(partial));
    const contains = search ? people.filter((u) => !u.name.toLowerCase().startsWith(partial) && u.name.toLowerCase().includes(partial)) : [];
    const matches = [...starts, ...contains].slice(0, 8);
    if (!matches.length) return this.hideMentions();
    const same = this.mentionMatches?.map((u) => u.src).join() === matches.map((u) => u.src).join();
    this.mentionMatches = matches;
    if (!same) this.mentionIndex = 0;
    this.renderMentions();
  },

  renderMentions() {
    this.mentionList.replaceChildren(...this.mentionMatches.map((user, i) => el('div', {
      class: 'mention-item' + (i === this.mentionIndex ? ' selected' : ''),
      role: 'option',
      // Keep the focus (and the cursor) in the input.
      onmousedown: (e) => e.preventDefault(),
      onclick: () => this.pickMention(user),
    }, el('span', { class: 'at', text: '@' }), el('span', { style: `color:${nickColor(user.src)}`, text: user.name }))));
    this.mentionList.classList.remove('hidden');
  },

  hideMentions() {
    this.mentionMatches = null;
    this.mentionList.classList.add('hidden');
  },

  // Up and Down choose, Tab or Enter picks, Esc closes (until the next @).
  mentionKey(e) {
    if (!this.mentionMatches || e.isComposing) return false;
    const count = this.mentionMatches.length;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      this.mentionIndex = (this.mentionIndex + (e.key === 'ArrowDown' ? 1 : count - 1)) % count;
      this.renderMentions();
    } else if (e.key === 'Tab' || e.key === 'Enter') {
      this.pickMention(this.mentionMatches[this.mentionIndex]);
    } else if (e.key === 'Escape') {
      this.mentionDismissed = this.mentionQuery()?.start ?? null;
      this.hideMentions();
    } else {
      return false;
    }
    e.preventDefault();
    return true;
  },

  // Put `@name ` in place of what was typed after the `@`.
  pickMention(user) {
    const query = this.mentionQuery();
    if (!query) return this.hideMentions();
    const input = this.input;
    const inserted = `@${user.name} `;
    input.value = input.value.slice(0, query.start) + inserted + input.value.slice(query.end).replace(/^ /, '');
    const caret = query.start + inserted.length;
    input.setSelectionRange(caret, caret);
    input.focus();
    this.hideMentions();
  },

  // Text with the mentions of us highlighted, and `@name` mentions of
  // others in their colour (both as UTF-16 ranges from the server).
  marked(text, highlights, mentions = []) {
    const ranges = [
      ...highlights.map(([start, end]) => [start, end, { class: 'mention' }]),
      ...mentions.map(([start, end, src]) => [start, end, {
        class: 'user-mention',
        style: `color:${src === this.view?.own_src ? 'var(--accent)' : nickColor(src)}`,
      }]),
    ].sort((a, b) => a[0] - b[0]);
    const out = [];
    let at = 0;
    for (const [start, end, attrs] of ranges) {
      if (start < at || end <= start) continue;
      if (start > at) out.push(text.slice(at, start));
      out.push(el('span', { ...attrs, text: text.slice(start, end) }));
      at = end;
    }
    if (at < text.length) out.push(text.slice(at));
    return out;
  },

  // Actions for another user: whisper through the hub, or LXMF.
  userMenu(event, src) {
    event.stopPropagation();
    const user = this.view?.users[src];
    if (!user) return;
    this.closeMenu();
    const close = () => this.closeMenu();
    const item = (text, sub, action, disabled = false) => el('button', {
      class: 'menu-item', disabled, onclick: () => { close(); action(); },
    }, el('span', { text }), sub ? el('span', { class: 'dim mono', text: sub }) : null);
    const inRoom = !!this.selected?.room && !this.view.whisper_with;
    const menu = el('div', { class: 'user-menu', role: 'menu' },
      el('div', { class: 'menu-title', text: user.name }),
      item(inRoom ? 'Mention' : 'Mention (in rooms only)', inRoom ? `@${user.name}` : null, () => this.mention(user.name), !inRoom),
      item(this.view.whisper ? 'Whisper through the hub' : 'Whisper (this hub does not pass them)', 'opens your conversation', async () => {
        const result = await this.hubAction(this.selected.hub, 'whisper', { src });
        if (!result) return;
        this.selected = { hub: this.selected.hub, room: result.room };
        setPane(this, 'detail');
        await this.update();
        this.input.focus();
      }, !this.view.whisper),
      item('LXMF message', user.lxmf_known ? user.lxmf : 'no announce seen yet', () => openConversation(user.lxmf)),
      item('Copy LXMF address', null, () => copy(user.lxmf, `${user.name}'s LXMF address`)),
      item('Copy identity hash', null, () => copy(src, `${user.name}'s identity hash`)));
    document.body.append(menu);
    const rect = event.currentTarget.getBoundingClientRect();
    const left = Math.min(rect.left, window.innerWidth - menu.offsetWidth - 8);
    const top = rect.bottom + menu.offsetHeight + 8 > window.innerHeight ? rect.top - menu.offsetHeight - 4 : rect.bottom + 4;
    menu.style.left = Math.max(8, left) + 'px';
    menu.style.top = Math.max(8, top) + 'px';
    this.menu = menu;
    this.menuCloser = (e) => {
      if (e.type === 'keydown') {
        if (e.key === 'Escape') close();
      } else if (!menu.contains(e.target)) {
        // On a phone it's a sheet: a tap outside only closes it.
        if (phone.matches) {
          e.preventDefault();
          e.stopPropagation();
        }
        close();
      }
    };
    setTimeout(() => {
      document.addEventListener('click', this.menuCloser, true);
      document.addEventListener('keydown', this.menuCloser);
    });
  },

  // Add `@name ` to what is being written, at the cursor, and carry on
  // writing.
  mention(name) {
    setPane(this, 'detail');
    const input = this.input;
    const caret = input.selectionStart ?? input.value.length;
    const before = input.value.slice(0, caret);
    const after = input.value.slice(input.selectionEnd ?? caret);
    const inserted = `${before && !/\s$/u.test(before) ? ' ' : ''}@${name}${after.startsWith(' ') ? '' : ' '}`;
    input.value = before + inserted + after;
    const at = before.length + inserted.length;
    input.focus();
    input.setSelectionRange(at, at);
    this.mentionDismissed = null;
    this.hideMentions();
  },

  closeMenu() {
    this.menu?.remove();
    this.menu = null;
    if (this.menuCloser) {
      document.removeEventListener('click', this.menuCloser, true);
      document.removeEventListener('keydown', this.menuCloser);
      this.menuCloser = null;
    }
  },

  // What's written in the line, if anything (see draftSwitch).
  takeDraft() {
    return this.input.value ? { text: this.input.value } : null;
  },

  putDraft(draft) {
    this.input.value = draft?.text || '';
    this.hideMentions();
  },

  joinDrafts(first, second) {
    return second ? { text: `${first.text} ${second.text}` } : first;
  },

  async send() {
    const text = this.input.value;
    if (!text.trim() || !this.selected) return;
    const { hub, room } = this.selected;
    // Take the line out of the box at once, so a second Enter (or a double
    // click) while this one is on its way has nothing to send, and anything
    // typed meanwhile is kept.
    this.input.value = '';
    this.hideMentions();
    // Put it back to try again or edit, in the room it was for.
    const restore = () => draftRestore(this, hub + '/' + room, { text });
    // A chat line (not a command) shows straight away as sending; the next
    // update draws the real one.
    const key = hub + '/' + room;
    const echo = !room || text.trim().startsWith('/') ? null : echoSent(this, {
      key,
      node: this.chat([{ kind: 'msg', nick: this.view?.nick, own: true, text: text.trim(), ts: Date.now(), pending: true }]).firstChild,
      parent: this.lastView === key && this.body.querySelector('.chat'),
      scroller: this.body,
      slot: 'room',
    });
    const result = await this.hubAction(hub, 'send', { room, text });
    // Over the hub's limit, nothing was sent (yet).
    echo?.done(!result || !!result.split);
    if (!result) return restore();
    if (result.split) {
      const limit = this.hubs.find((h) => h.hash === hub)?.limits.message_bytes;
      if (confirm(`That is over this hub's ${limit}-byte limit. Send it as ${result.split.length} messages?`)) {
        if (!await this.hubAction(hub, 'split', { room, parts: result.split })) restore();
      } else {
        restore();
      }
    }
    this.body.scrollTop = this.body.scrollHeight;
  },
};

// ---- Network ----------------------------------------------------------------

// Search words must each appear in the name or the address; addresses may be
// pasted as <hash>, lxmf@hash or hash:/page/index.mu.
function searchTerms(query) {
  return query.split(/\s+/).filter(Boolean).map((word) => {
    word = word.replace(/^<+/, '').replace(/>+$/, '');
    for (const prefix of ['lxmf@', 'lxmf://', 'nomadnetwork://']) {
      if (word.startsWith(prefix)) word = word.slice(prefix.length);
    }
    const colon = word.indexOf(':');
    if (colon > 0 && /^[0-9a-fA-F]+$/.test(word.slice(0, colon))) word = word.slice(0, colon);
    return word.toLowerCase();
  }).filter(Boolean);
}

function highlighted(text, terms) {
  const lower = text.toLowerCase();
  const mask = new Array(text.length).fill(false);
  for (const term of terms) {
    for (let i = lower.indexOf(term); i !== -1; i = lower.indexOf(term, i + 1)) mask.fill(true, i, i + term.length);
  }
  const out = [];
  let run = '';
  let on = false;
  for (let i = 0; i < text.length; i++) {
    if (mask[i] !== on && run) {
      out.push(on ? el('mark', { text: run }) : run);
      run = '';
    }
    on = mask[i];
    run += text[i];
  }
  if (run) out.push(on ? el('mark', { text: run }) : run);
  return out;
}

app.views.network = {
  filter: 'all',
  query: '',
  selected: null,

  mount(root) {
    this.search = el('input', {
      type: 'search',
      class: 'grow',
      placeholder: 'Find by name or address  ( / )',
      value: this.query,
      oninput: () => {
        this.query = this.search.value;
        this.limit = 200;
        // Searched by rettui; wait for a pause in typing.
        clearTimeout(this.typing);
        this.typing = setTimeout(() => this.update(), 120);
      },
      onkeydown: (e) => {
        if (e.key === 'Escape') {
          this.search.value = '';
          this.query = '';
          this.limit = 200;
          this.update();
        }
      },
    });
    const filter = el('select', { onchange: (e) => {
      this.filter = e.target.value;
      this.limit = 200;
      this.update();
    } }, [['all', 'All'], ['lxmf', 'LXMF peers'], ['nomad', 'NomadNet nodes'], ['propagation', 'Propagation nodes']]
      .map(([value, text]) => el('option', { value, text, selected: value === this.filter })));
    this.title = el('span', { class: 'title grow' });
    this.table = el('div', { class: 'scroll' });
    root.append(el('div', { class: 'column grow' },
      el('section', { class: 'panel' }, el('header', {}, this.search, filter,
        el('button', { text: 'Announce', onclick: () => attempt(() => api.post('/announce'), 'Announcing') }),
        el('button', { text: 'Sync', onclick: () => attempt(() => api.post('/sync'), 'Syncing with the propagation node') }))),
      el('section', { class: 'panel grow' }, el('header', {}, this.title), this.table)));
  },

  limit: 200,
  request: 0,

  // Only the rows shown are fetched (there can be thousands of peers);
  // rettui filters, searches and counts the rest.
  async update() {
    const request = ++this.request;
    const params = new URLSearchParams({ q: this.query, limit: this.limit });
    if (this.filter !== 'all') params.set('kind', this.filter);
    const data = await api.get('/peers?' + params);
    // A newer search or filter was asked for while this one loaded.
    if (request !== this.request) return;
    data.query = this.query;
    if (!changed(this, 'peers', data)) return;
    this.data = data;
    this.render();
  },

  render() {
    if (!this.data) return;
    // Highlight what the rows were found by, not what has been typed since.
    const terms = searchTerms(this.data.query);
    const filterName = { all: 'all', lxmf: 'LXMF peers', nomad: 'NomadNet nodes', propagation: 'propagation nodes' }[this.filter];
    const rows = this.data.peers;
    const total = this.data.total;
    this.title.textContent = `Heard announces · ${filterName} · ${total}${terms.length ? ' matching' : ''}`;
    if (!rows.length) {
      this.table.replaceChildren(el('div', { class: 'empty', text: terms.length
        ? `Nothing heard matches “${this.data.query.trim()}”. Esc clears the search.`
        : 'Listening for announces… peers, NomadNet nodes and propagation nodes appear here as they are heard.' }));
      return;
    }
    const tag = { lxmf: 'PEER', nomad: 'NODE', propagation: 'PROP' };
    const outbound = this.data.propagation_node;
    // The newest rows only; more on request.
    const more = total - rows.length;
    this.table.replaceChildren(el('table', { class: 'net-table' },
      el('thead', {}, el('tr', {}, ['', 'Name', 'Address', 'Hops', 'Heard', ''].map((h) => el('th', { text: h })))),
      el('tbody', {}, rows.map((p) => el('tr', {
        class: p.hash === this.selected ? 'selected' : '',
        onclick: (e) => {
          this.selected = p.hash;
          for (const row of e.currentTarget.parentNode.children) row.classList.remove('selected');
          e.currentTarget.classList.add('selected');
        },
        ondblclick: () => this.open(p),
      },
      el('td', {}, el('span', { class: 'tag ' + p.kind, text: tag[p.kind] })),
      el('td', { class: 'name' }, p.name ? highlighted(p.name, terms) : el('span', { class: 'dim', text: '(unnamed)' }),
        p.hash === outbound ? el('span', { class: 'star', text: '  ★ outbound' }) : null),
      el('td', { class: 'mono dim' }, highlighted(p.hash, terms)),
      el('td', { class: 'dim', text: `${p.hops} hop${p.hops === 1 ? '' : 's'}` }),
      el('td', { class: 'dim', text: ago(p.last_seen) + ' ago' }),
      el('td', {}, el('div', { class: 'actions' },
        p.kind === 'lxmf' ? el('button', { text: 'Message', onclick: () => this.open(p) }) : null,
        p.kind === 'nomad' ? el('button', { text: 'Browse', onclick: () => this.open(p) }) : null,
        p.kind === 'propagation' ? el('button', { text: p.hash === outbound ? 'In use' : 'Use for sync', disabled: p.hash === outbound, onclick: () => this.open(p) }) : null,
        el('button', { text: 'Copy', onclick: (e) => {
          e.stopPropagation();
          copy(p.hash, 'address');
        } })))))),
      more > 0 ? el('div', { class: 'show-more' },
        el('span', { class: 'dim', text: `Showing the ${rows.length} most recently heard of ${total}. Search or filter to find others, or ` }),
        el('button', { text: `show ${Math.min(more, 200)} more`, onclick: () => {
          this.limit += 200;
          this.update();
        } })) : null));
  },

  open(peer) {
    if (peer.kind === 'lxmf') openConversation(peer.hash);
    else if (peer.kind === 'nomad') browse(peer.hash);
    else attempt(() => api.post('/propagation', { node: peer.hash }), `Propagation node set to ${peer.name || peer.hash}`);
  },
};

// ---- Browser ----------------------------------------------------------------

app.views.browser = {
  panes: true,
  // The list beside the page: saved pages or heard nodes.
  listing: 'saved',
  page: null,
  history: [],
  viewSource: false,
  loading: null,
  // Nodes listed (the most recently heard); more on request. A busy
  // network hears thousands, refetched with every burst of announces.
  nodeLimit: 200,

  mount(root) {
    this.paneList = el('div', { class: 'scroll' });
    this.paneTabs = el('div', { class: 'subtabs' });
    this.address = el('input', {
      type: 'text',
      placeholder: 'NomadNet address (hash:/page/index.mu)',
      onkeydown: (e) => {
        if (e.key === 'Enter') this.go(this.address.value);
      },
    });
    this.buttons = {
      back: el('button', { text: '←', title: 'Back', onclick: () => this.back() }),
      reload: el('button', { text: '⟳', title: 'Refresh from the network', onclick: () => this.page && this.go(this.page.url, { refresh: true, record: false }) }),
      save: el('button', { text: '☆ Save', onclick: () => this.toggleSaved() }),
      identify: el('button', { text: 'Identify', title: 'Identify to this node', onclick: () => this.toggleIdentify() }),
      source: el('button', { text: '</> Source', title: 'View the Micron source', onclick: () => {
        this.viewSource = !this.viewSource;
        this.renderPage();
      } }),
      clear: el('button', { text: 'Clear cache', onclick: async () => {
        const result = await attempt(() => api.post('/cache/clear'));
        if (result) toast(`Cleared ${result.removed} cached page(s) and image(s)`);
      } }),
    };
    this.status = el('div', { class: 'page-status' });
    this.content = el('div', { class: 'scroll page', onclick: (e) => this.click(e) });
    root.append(
      el('section', { class: 'panel side' }, el('header', {}, this.paneTabs,
        el('button', { class: 'phone-only', text: 'Address…', onclick: () => {
          setPane(this, 'detail');
          this.address.focus();
        } })), this.paneList),
      el('section', { class: 'panel grow pane-main' },
        el('div', { class: 'toolbar' }, this.buttons.back, this.address, el('button', { class: 'primary', text: 'Go', onclick: () => this.go(this.address.value) }),
          // Their own row on a phone.
          el('span', { class: 'more-tools' }, this.buttons.reload, this.buttons.save, this.buttons.identify, this.buttons.source, this.buttons.clear)),
        this.status, this.content));
    this.renderPane();
    this.renderPage();
  },

  async update() {
    const [saved, peers] = await Promise.all([api.get('/saved'), api.get(`/peers?kind=nomad&limit=${this.nodeLimit}`)]);
    this.saved = saved;
    this.nodes = peers.peers;
    this.nodeTotal = peers.total;
    this.renderPane();
    if (this.page) {
      const url = this.page.url;
      this.page.saved = saved.some((s) => s.url === url);
      this.renderButtons();
    }
  },

  renderPane() {
    this.paneTabs.replaceChildren(
      el('button', { class: this.listing === 'saved' ? 'active' : '', text: `Saved ${this.saved?.length ?? ''}`, onclick: () => {
        this.listing = 'saved';
        this.renderPane();
      } }),
      el('button', { class: this.listing === 'nodes' ? 'active' : '', text: `Nodes ${this.nodeTotal ?? ''}`, onclick: () => {
        this.listing = 'nodes';
        this.renderPane();
      } }));
    const current = this.page?.url;
    const currentNode = this.page?.node;
    if (this.listing === 'saved') {
      this.paneList.replaceChildren(...((this.saved || []).length ? this.saved.map((s) => el('div', {
        class: 'list-item' + (s.url === current ? ' selected' : ''),
        onclick: () => this.go(s.url),
      }, el('span', { class: 'main name', text: s.name }),
      el('button', { text: '×', title: 'Remove', class: 'danger', onclick: (e) => {
        e.stopPropagation();
        attempt(() => api.post('/saved/remove', { url: s.url }), `Removed ${s.name}`);
      } }))) : [el('div', { class: 'empty', text: 'Nothing saved yet. Open a page and press ☆ Save.' })]));
    } else {
      const nodes = this.nodes || [];
      const rows = nodes.length ? nodes.map((n) => el('div', {
        class: 'list-item' + (n.hash === currentNode ? ' selected' : ''),
        onclick: () => this.go(n.hash),
      }, el('span', { class: 'main' },
        el('div', { class: 'name', text: n.name || `<${n.hash.slice(0, 12)}>` }),
        el('div', { class: 'sub', text: `${n.hash}  ·  ${ago(n.last_seen)} ago` })))) :
        [el('div', { class: 'empty', text: 'No NomadNet nodes heard yet.' })];
      // The newest only; more on request.
      const more = (this.nodeTotal ?? nodes.length) - nodes.length;
      if (more > 0) {
        rows.push(el('div', { class: 'show-more' },
          el('span', { class: 'dim', text: `The ${nodes.length} most recently heard of ${this.nodeTotal}. Search the Network tab for others, or ` }),
          el('button', { text: `show ${Math.min(more, 200)} more`, onclick: () => {
            this.nodeLimit += 200;
            this.update();
          } })));
      }
      this.paneList.replaceChildren(...rows);
    }
  },

  renderButtons() {
    const page = this.page;
    for (const name of ['reload', 'save', 'identify', 'source']) this.buttons[name].disabled = !page;
    this.buttons.back.disabled = !this.history.length;
    this.buttons.save.textContent = page?.saved ? '★ Saved' : '☆ Save';
    this.buttons.save.classList.toggle('on', !!page?.saved);
    this.buttons.identify.classList.toggle('on', !!page?.identified);
    this.buttons.identify.textContent = page?.identified ? 'Identified' : 'Identify';
    this.buttons.source.classList.toggle('on', this.viewSource);
    this.buttons.source.textContent = this.viewSource ? '◂ Page' : '</> Source';
  },

  renderPage() {
    this.renderButtons();
    const status = [];
    if (this.loading) {
      status.push(el('span', { class: 'state-warn', text: `Loading ${this.loading.url}…` }),
        el('a', { href: '#', text: 'cancel', onclick: (e) => {
          e.preventDefault();
          this.loading.cancelled = true;
          this.loading = null;
          this.renderPage();
        } }));
    } else if (this.error) {
      status.push(el('span', { class: 'error', text: '⚠ ' + this.error }));
    }
    if (this.page && !this.loading) {
      status.push(el('span', { text: this.page.node_name }));
      if (this.page.cached_age !== null) status.push(el('span', { text: `cached ${ago(Date.now() / 1000 - this.page.cached_age)} ago · ⟳ refreshes` }));
      if (this.page.identified) status.push(el('span', { class: 'identified', text: 'identified' }));
    }
    this.status.replaceChildren(...status);
    this.status.classList.toggle('hidden', !status.length);
    if (!this.page) {
      this.content.replaceChildren(el('div', { class: 'empty', text: 'Pick a node or saved page on the left, or enter an address above.' }));
      return;
    }
    // Server-rendered: all page text is escaped and the CSP blocks scripts.
    this.content.innerHTML = this.viewSource ? this.page.source_html : this.page.html;
  },

  // Open a NomadNet address. `fields` submits a form (never cached).
  async go(url, { refresh = false, record = true, fields = null } = {}) {
    url = url.trim();
    if (!url) return;
    if (url.startsWith('lxmf@') || url.startsWith('lxmf://')) return openConversation(url);
    if (url.startsWith('rrc://') || url.startsWith('rrc@')) return openHubLink(url);
    if (/:\/file\//.test(url)) {
      window.location.href = '/api/download?url=' + encodeURIComponent(url);
      toast('Downloading…');
      return;
    }
    setPane(this, 'detail');
    const loading = { url, cancelled: false };
    this.loading = loading;
    this.error = null;
    this.address.value = url;
    this.renderPage();
    try {
      const page = fields
        ? await api.post('/page', { url, fields })
        : await api.get(`/page?url=${encodeURIComponent(url)}${refresh ? '&refresh=true' : ''}`);
      if (loading.cancelled || this.loading !== loading) return;
      if (page.download) {
        this.loading = null;
        this.renderPage();
        return this.go(page.download);
      }
      if (record && this.page && this.page.url !== page.url) this.history.push(this.page.url);
      if (!this.page || this.page.url !== page.url) this.viewSource = false;
      this.page = page;
      this.address.value = page.url;
      this.content.scrollTop = 0;
    } catch (e) {
      if (loading.cancelled) return;
      this.error = e.message;
    }
    this.loading = null;
    this.renderPage();
    this.renderPane();
  },

  back() {
    const previous = this.history.pop();
    if (previous) this.go(previous, { record: false });
  },

  // Links carry their target in data-url and the fields they submit in
  // data-fields (names, `*` for all, or `var=value`).
  click(e) {
    const link = e.target.closest('.m-link');
    if (!link) return;
    e.preventDefault();
    const spec = link.dataset.fields ? link.dataset.fields.split('|') : [];
    if (!spec.length) return this.go(link.dataset.url);
    const fields = {};
    const all = spec.includes('*');
    for (const input of this.content.querySelectorAll('input[name]')) {
      const name = input.name;
      if (!all && !spec.includes(name)) continue;
      const key = 'field_' + name;
      if (input.type === 'checkbox') {
        if (input.checked) fields[key] = fields[key] ? fields[key] + ',' + input.value : input.value;
      } else if (input.type === 'radio') {
        if (input.checked) fields[key] = input.value;
      } else {
        fields[key] = input.value;
      }
    }
    for (const item of spec) {
      const eq = item.indexOf('=');
      if (eq > 0) fields['var_' + item.slice(0, eq)] = item.slice(eq + 1);
    }
    this.go(link.dataset.url, { fields });
  },

  toggleSaved() {
    if (!this.page) return;
    const url = this.page.url;
    if (this.page.saved) attempt(() => api.post('/saved/remove', { url }), 'Removed from saved pages');
    else attempt(() => api.post('/saved', { url }), 'Saved');
  },

  async toggleIdentify() {
    if (!this.page) return;
    const on = !this.page.identified;
    const done = await attempt(() => api.post('/identify', { node: this.page.node, on }));
    if (done) this.go(this.page.url, { record: false });
  },
};

// ---- Node -------------------------------------------------------------------

// Hosting this client's NomadNet node, and editing its pages. Saving writes
// the page the node serves, so visitors get it straight away. Scripts
// (executable pages) are read-only here.
const PAGE_VIEWS = [['editor', 'Editor'], ['split', 'Both'], ['preview', 'Preview']];

// ---- Micron formatting (the page editor's ribbon) ---------------------------
//
// The same actions and Alt keys as the terminal UI's editor: markup put
// around the selection, on the selected lines, or at the cursor.

const MICRON_RIBBON = [
  [['bold', 'Bold', 'b', 'Bold (Alt+B, Ctrl+B)'], ['italic', 'Italic', 'i', 'Italic (Alt+I, Ctrl+I)'],
    ['underline', 'Underline', 'u', 'Underline (Alt+U, Ctrl+U)'], ['normal', 'Normal', 'n', 'Remove formatting from the selection, or reset it here (Alt+N)']],
  [['fg', 'Fg', 'f', 'Text colour (Alt+F)'], ['bg', 'Bg', 'g', 'Background colour (Alt+G)']],
  [['left', 'Left', 'l', 'Align left (Alt+L)'], ['center', 'Centre', 'c', 'Centre (Alt+C)'], ['right', 'Right', 'r', 'Align right (Alt+R)']],
  [['h1', 'H1', '1', 'Heading (Alt+1)'], ['h2', 'H2', '2', 'Subheading (Alt+2)'], ['h3', 'H3', '3', 'Third-level heading (Alt+3)']],
  [['divider', 'Divider', 'v', 'Divider line (Alt+V)'], ['literal', 'Literal', 't', 'Literal block, shown as written (Alt+T)'],
    ['comment', 'Comment', 'o', 'Comment lines out, or back in (Alt+O)']],
  [['link', 'Link', 'k', 'Link (Alt+K, Ctrl+K)'], ['image', 'Image', 'm', 'Image (Alt+M)'], ['field', 'Field', 'd', 'Text field (Alt+D)'],
    ['checkbox', 'Check', 'h', 'Checkbox (Alt+H)'], ['radio', 'Radio', 'a', 'Radio button (Alt+A)']],
];
const MICRON_KEYS = Object.fromEntries(MICRON_RIBBON.flat().map(([id, , key]) => [key, id]));
const MICRON_COLOURS = ['f00', 'f80', 'ff0', '8f0', '0f0', '0fc', '0af', '00f', '80f', 'f0f', 'fff', 'aaa', '555', '000'];

const micron = {
  // Replace a range of the text box, keeping the browser's undo.
  replace(t, start, end, text) {
    t.focus();
    t.setSelectionRange(start, end);
    if (!document.execCommand('insertText', false, text)) {
      t.setRangeText(text, start, end, 'end');
      t.dispatchEvent(new Event('input'));
    }
  },

  // Put open and close around the selection (kept selected), or both at the
  // cursor with the cursor between.
  wrap(t, open, close) {
    const { selectionStart: start, selectionEnd: end, value } = t;
    const text = value.slice(start, end);
    this.replace(t, start, end, open + text + close);
    if (text) t.setSelectionRange(start + open.length, start + open.length + text.length);
    else t.setSelectionRange(start + open.length, start + open.length);
  },

  // Bold, italic and underline switch with the same tag: wrap, or unwrap.
  toggle(t, tag) {
    const { selectionStart: start, selectionEnd: end, value } = t;
    const text = value.slice(start, end);
    const n = tag.length;
    if (text.length >= 2 * n && text.startsWith(tag) && text.endsWith(tag)) {
      const inner = text.slice(n, -n);
      this.replace(t, start, end, inner);
      t.setSelectionRange(start, start + inner.length);
    } else if (text && value.slice(start - n, start) === tag && value.slice(end, end + n) === tag) {
      this.replace(t, start - n, end + n, text);
      t.setSelectionRange(start - n, start - n + text.length);
    } else {
      this.wrap(t, tag, tag);
    }
  },

  strip(text) {
    return text.replace(/`(?:[!*_fb`]|[FB]T[0-9a-fA-F]{6}|[FB][0-9a-fA-F]{3})/g, '');
  },

  colour(text) {
    const hex = text.trim().replace(/^#/, '').toLowerCase();
    if (/^[0-9a-f]{3}$/.test(hex)) return hex;
    if (/^[0-9a-f]{6}$/.test(hex)) return 'T' + hex;
    return null;
  },

  // The lines an action on lines applies to (not the last when the
  // selection ends at its very start), as offsets and text.
  lines(t) {
    const { value } = t;
    let { selectionStart: start, selectionEnd: end } = t;
    if (end > start && value[end - 1] === '\n') end -= 1;
    const from = value.lastIndexOf('\n', start - 1) + 1;
    let to = value.indexOf('\n', end);
    if (to === -1) to = value.length;
    return { from, to, lines: value.slice(from, to).split('\n') };
  },

  setLines(t, from, to, lines, cursorLine = 0, cursorEnd = true) {
    const text = lines.join('\n');
    this.replace(t, from, to, text);
    let at = from;
    for (let i = 0; i < cursorLine; i++) at += lines[i].length + 1;
    at += cursorEnd ? lines[cursorLine].length : 0;
    t.setSelectionRange(at, at);
  },

  heading: (line) => line.match(/^>*/)[0],
  unaligned: (text) => text.replace(/^(`[clra])+/, ''),

  align(t, tag) {
    const { from, to, lines } = this.lines(t);
    const out = lines.map((line, i) => {
      const marks = this.heading(line);
      return marks + (i === 0 ? '`' + tag : '') + this.unaligned(line.slice(marks.length));
    });
    let end = to;
    // Alignment carries on in Micron: the next line goes back to the left.
    if (tag !== 'a' && to < t.value.length) {
      let nextEnd = t.value.indexOf('\n', to + 1);
      if (nextEnd === -1) nextEnd = t.value.length;
      const next = t.value.slice(to + 1, nextEnd);
      const marks = this.heading(next);
      const rest = next.slice(marks.length);
      if (this.unaligned(rest) === rest) {
        out.push(marks + '`a' + rest);
        end = nextEnd;
      }
    }
    this.setLines(t, from, end, out);
  },

  toggleHeading(t, level) {
    const { from, to, lines } = this.lines(t);
    const already = lines.every((line) => this.heading(line).length === level);
    const out = lines.map((line) => {
      const rest = line.slice(this.heading(line).length);
      return already ? rest : '>'.repeat(level) + rest;
    });
    this.setLines(t, from, to, out, out.length - 1);
  },

  clean: (text) => text.replace(/[`[\]()<>|]/g, ''),

  // Apply an action; asks for what it needs (colours come from the palette).
  run(t, action, answer) {
    const { selectionStart: start, selectionEnd: end, value } = t;
    const selected = value.slice(start, end);
    const oneLine = selected && !selected.includes('\n') ? this.clean(selected) : '';
    const ask = (question) => {
      const reply = prompt(question);
      return reply === null ? null : reply.trim();
    };
    const name = (question) => {
      const reply = ask(question);
      if (reply === null) return null;
      if (!/^[\p{L}\p{N}_-]+$/u.test(reply)) {
        toast('Field names use letters, digits, - and _', true);
        return null;
      }
      return reply;
    };
    switch (action) {
      case 'bold': return this.toggle(t, '`!');
      case 'italic': return this.toggle(t, '`*');
      case 'underline': return this.toggle(t, '`_');
      case 'normal':
        if (!selected) return this.replace(t, start, end, '``');
        this.replace(t, start, end, this.strip(selected));
        return t.setSelectionRange(start, start + this.strip(selected).length);
      case 'fg':
      case 'bg': {
        const code = this.colour(answer || '');
        if (!code) return toast(`"${answer}" is not a colour: use 3 or 6 hex digits, like f80`, true);
        return action === 'fg' ? this.wrap(t, '`F' + code, '`f') : this.wrap(t, '`B' + code, '`b');
      }
      case 'left': return this.align(t, 'a');
      case 'center': return this.align(t, 'c');
      case 'right': return this.align(t, 'r');
      case 'h1': return this.toggleHeading(t, 1);
      case 'h2': return this.toggleHeading(t, 2);
      case 'h3': return this.toggleHeading(t, 3);
      case 'divider': {
        const { from, to, lines } = this.lines(t);
        const line = lines[lines.length - 1];
        const start = from + lines.slice(0, -1).reduce((n, l) => n + l.length + 1, 0);
        if (!line.trim()) return this.setLines(t, start, to, ['-']);
        return this.setLines(t, start, to, [line, '-'], 1);
      }
      case 'literal': {
        const { from, to, lines } = this.lines(t);
        if (!selected && lines.length === 1 && !lines[0].trim()) return this.setLines(t, from, to, ['`=', '', '`='], 1);
        return this.setLines(t, from, to, ['`=', ...lines, '`='], lines.length);
      }
      case 'comment': {
        const { from, to, lines } = this.lines(t);
        const commented = (l) => l.startsWith('#') && !l.startsWith('#!');
        const all = lines.every(commented);
        const out = lines.map((l) => (all ? l.replace(/^# ?/, '') : '# ' + l));
        return this.setLines(t, from, to, out, out.length - 1);
      }
      case 'link': {
        const url = ask('Link to (a page like :/page/about.mu, node:/page/…, lxmf@…, rrc://…)');
        if (!url) return;
        return this.replace(t, start, end, `\`[${oneLine || url}\`${url}]`);
      }
      case 'image': {
        const url = ask('Image address (like :/media/logo.png)');
        if (!url) return;
        return this.replace(t, start, end, `\`(${oneLine || 'image'}\`${url})`);
      }
      case 'field': {
        const field = name('Field name');
        if (field) this.replace(t, start, end, `\`<24|${field}\`${oneLine}>`);
        return;
      }
      case 'checkbox': {
        const field = name('Checkbox name');
        if (field) this.replace(t, start, end, `\`<?|${field}|yes\`${oneLine || field}>`);
        return;
      }
      case 'radio': {
        const field = name('Radio group name');
        const label = oneLine || 'Option';
        if (field) this.replace(t, start, end, `\`<^|${field}|${label}\`${label}>`);
        return;
      }
    }
  },
};

app.views.node = {
  panes: true,
  open: null, // { path, saved, executable, url }
  view: (() => {
    try { return localStorage.getItem('rettui.pageView') || 'split'; } catch { return 'split'; }
  })(),

  mount(root) {
    this.card = el('div', { class: 'hub-info node-card' });
    this.hostButton = el('button', { onclick: () => this.toggleHosting() });
    this.list = el('div', { class: 'scroll' });
    this.title = el('span', { class: 'title grow' });
    this.buttons = el('div', { class: 'row' });
    this.banner = el('div', { class: 'banner hidden' });
    this.text = el('textarea', {
      class: 'page-editor',
      spellcheck: false,
      placeholder: 'Pick a page on the left, or create one with + New',
      oninput: () => this.changed(),
      onkeydown: (e) => {
        // Formatting: Alt and the key underlined in the ribbon (by the key's
        // place, so it works with any layout's Alt characters), and the
        // usual Ctrl+B / I / U / K.
        const code = e.code.startsWith('Key') ? e.code.slice(3).toLowerCase() : e.code.startsWith('Digit') ? e.code.slice(5) : '';
        const common = { b: 'bold', i: 'italic', u: 'underline', k: 'link' };
        const action = e.altKey && !e.ctrlKey && !e.metaKey ? MICRON_KEYS[code]
          : (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey ? common[code] : null;
        if (action && !this.text.readOnly) {
          e.preventDefault();
          this.format(action, e.currentTarget);
        } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
          e.preventDefault();
          this.save();
        } else if (e.key === 'Tab' && !e.shiftKey && !this.text.readOnly) {
          e.preventDefault();
          this.text.setRangeText('    ', this.text.selectionStart, this.text.selectionEnd, 'end');
          this.changed();
        }
      },
    });
    this.preview = el('div', { class: 'scroll page preview', onclick: (e) => {
      const link = e.target.closest('.m-link');
      if (link) browse(link.dataset.url);
    } });
    this.viewButtons = el('div', { class: 'subtabs', title: 'Show the editor, the preview, or both' });
    this.split = el('div', { class: 'editor-split' }, this.text, this.preview);
    this.ribbon = el('div', { class: 'ribbon', role: 'toolbar', 'aria-label': 'Formatting' },
      MICRON_RIBBON.map((group) => el('div', { class: 'group' }, group.map(([action, label, key, title]) => {
        const at = label.toLowerCase().indexOf(key);
        return el('button', {
          class: 'fmt fmt-' + action, title,
          // Keep the selection in the text box while clicking.
          onmousedown: (e) => e.preventDefault(),
          onclick: (e) => this.format(action, e.currentTarget),
        }, label.slice(0, at), el('span', { class: 'key', text: label[at] }), label.slice(at + 1));
      }))));
    applyWrap(this.text);
    root.append(
      el('div', { class: 'column side' },
        el('section', { class: 'panel' }, el('header', {}, el('span', { class: 'title grow', text: 'Hosting' })), this.card,
          el('div', { class: 'row card-actions' }, this.hostButton,
            el('button', { text: 'Announce', onclick: () => attempt(() => api.post('/node/announce'), 'Announcing the node') }))),
        el('section', { class: 'panel grow' }, el('header', {}, el('span', { class: 'title grow', text: 'Pages' }),
          el('button', { text: '+ New', onclick: () => this.create() })), this.list)),
      el('section', { class: 'panel grow pane-main' },
        el('header', {}, this.title, this.buttons),
        this.banner,
        this.ribbon,
        this.split));
    this.renderEditor();
    if (!this.unloadGuard) {
      this.unloadGuard = true;
      window.addEventListener('beforeunload', (e) => {
        if (this.dirty()) e.preventDefault();
      });
    }
  },

  dirty() {
    return !!this.open && this.text && this.text.value !== this.open.saved;
  },

  // A ribbon action: colours pick from a palette first.
  format(action, from) {
    if (!this.open || this.text.readOnly) return;
    if (action === 'fg' || action === 'bg') return this.palette(action, from);
    micron.run(this.text, action);
  },

  palette(action, from) {
    this.closePalette();
    const pick = (code) => {
      this.closePalette();
      micron.run(this.text, action, code);
    };
    const hex = el('input', {
      type: 'text', placeholder: 'or hex: f80, 00aaff', maxlength: 7, class: 'mono',
      onkeydown: (e) => {
        if (e.key === 'Enter') {
          // Not also typed into the editor, which has the focus next.
          e.preventDefault();
          pick(hex.value);
        }
        if (e.key === 'Escape') {
          this.closePalette();
          this.text.focus();
        }
      },
    });
    const menu = el('div', { class: 'user-menu palette', role: 'dialog' },
      el('div', { class: 'menu-title', text: action === 'fg' ? 'Text colour' : 'Background colour' }),
      el('div', { class: 'swatches' }, MICRON_COLOURS.map((code) => el('button', {
        class: 'swatch', title: code, style: `background:#${code}`, onclick: () => pick(code),
      }))),
      el('div', { class: 'row' }, hex, el('button', { text: 'Apply', onclick: () => pick(hex.value) })));
    document.body.append(menu);
    const rect = (from || this.ribbon).getBoundingClientRect();
    menu.style.left = Math.max(8, Math.min(rect.left, window.innerWidth - menu.offsetWidth - 8)) + 'px';
    menu.style.top = rect.bottom + 4 + 'px';
    this.paletteMenu = menu;
    this.paletteCloser = (e) => {
      if (!menu.contains(e.target)) this.closePalette();
    };
    setTimeout(() => document.addEventListener('mousedown', this.paletteCloser));
    hex.focus();
  },

  closePalette() {
    this.paletteMenu?.remove();
    this.paletteMenu = null;
    if (this.paletteCloser) {
      document.removeEventListener('mousedown', this.paletteCloser);
      this.paletteCloser = null;
    }
  },

  async update() {
    const node = await api.get('/node');
    this.node = node;
    const state = {
      running: el('span', { class: 'state-ok', text: '● hosting' }),
      starting: el('span', { class: 'state-warn', text: '◌ starting' }),
      off: el('span', { class: 'dim', text: '○ off' }),
      failed: el('span', { class: 'state-bad', text: `✗ ${node.error}` }),
    }[node.status];
    this.hostButton.textContent = node.status === 'off' || node.status === 'failed' ? 'Start hosting' : 'Stop hosting';
    this.hostButton.className = node.status === 'off' || node.status === 'failed' ? 'primary' : '';
    const label = (text) => el('span', { class: 'label', text });
    this.card.replaceChildren(
      label('Node'), state,
      label('Address'), el('span', { class: 'row' }, el('span', { class: 'mono', text: node.address }),
        el('button', { text: 'Copy', onclick: () => copy(node.address, 'node address') })),
      label('Name'), el('span', { text: node.name }),
      label('Requests'), el('span', { text: node.stats ? `${node.stats.requests} (${node.stats.pages} pages, ${node.stats.files} files)` : '–' }),
      label('Folder'), el('span', { class: 'mono dim', text: node.dir }));
    if (node.list_error) {
      this.list.replaceChildren(el('div', { class: 'empty state-bad', text: node.list_error }));
      return;
    }
    this.list.replaceChildren(...(node.pages.length ? node.pages.map((page) => el('div', {
      class: 'list-item' + (this.open?.path === page.path ? ' selected' : ''),
      onclick: () => page.text ? this.load(page.path) : toast(`${page.path} is not a text file`, true),
    },
    el('span', { class: 'main' },
      el('div', { class: 'name' + (page.text ? '' : ' dim'), text: page.path }),
      el('div', { class: 'sub' }, humanBytes(page.size), page.executable ? el('span', { class: 'script-tag', text: ' script' }) : null)),
    this.open?.path === page.path && this.dirty() ? el('span', { class: 'state-warn', text: '●', title: 'Unsaved changes' }) : null)) :
      [el('div', { class: 'empty', text: 'No pages yet. Create one with + New.' })]));
  },

  toggleHosting() {
    const on = this.node && (this.node.status === 'off' || this.node.status === 'failed');
    attempt(() => api.post('/settings', { values: { node_enabled: String(on) } }), on ? 'Starting the node' : 'Stopped hosting');
  },

  async load(path) {
    if (this.open?.path === path) return setPane(this, 'detail');
    if (this.dirty() && !confirm(`Discard unsaved changes to ${this.open.path}?`)) return;
    setPane(this, 'detail');
    const page = await attempt(() => api.get('/node/page?path=' + encodeURIComponent(path)));
    if (!page) return;
    this.open = { path: page.path, saved: page.content, executable: page.executable, url: page.url };
    this.text.value = page.content;
    this.renderEditor();
    this.renderPreview();
    this.update();
  },

  setView(view) {
    this.view = view;
    try { localStorage.setItem('rettui.pageView', view); } catch { /* per-browser convenience only */ }
    this.renderEditor();
    this.renderPreview();
    if (view !== 'preview') this.text.focus();
  },

  changed() {
    // The title and buttons only change when the page becomes (un)modified.
    if (this.dirty() !== this.shownDirty) this.renderEditor();
    this.renderPreview();
  },

  // One preview request at a time: a change is sent at once, and changes
  // made while it is out go together when it returns (no fixed delay, and
  // an older reply can never land after a newer one).
  async renderPreview() {
    this.previewWanted = true;
    if (this.previewBusy) return;
    this.previewBusy = true;
    try {
      while (this.previewWanted) {
        this.previewWanted = false;
        if (!this.open) {
          this.preview.replaceChildren();
          this.previewed = null;
          continue;
        }
        // Nothing to draw while only the editor shows; setView catches up.
        if (this.view === 'editor') continue;
        const { path } = this.open;
        const content = this.text.value;
        if (this.previewed === path + '\0' + content) continue;
        const result = await api.post('/node/preview', { content }).catch(() => null);
        if (!result || this.open?.path !== path) continue;
        // Server-rendered like browsed pages: escaped, and scripts are blocked.
        patchHtml(this.preview, result.html);
        this.previewed = path + '\0' + content;
      }
    } finally {
      this.previewBusy = false;
    }
  },

  renderEditor() {
    const open = this.open;
    this.text.disabled = !open;
    this.text.readOnly = !!open?.executable;
    this.banner.classList.toggle('hidden', !open?.executable);
    this.banner.textContent = 'This page is a script. Scripts can only be changed on the host, not from the web UI.';
    this.split.className = 'editor-split view-' + this.view;
    this.ribbon.classList.toggle('hidden', !open || !!open.executable || this.view === 'preview');
    this.viewButtons.replaceChildren(...PAGE_VIEWS.map(([view, label]) => el('button', {
      text: label, class: view === this.view ? 'active' : '', onclick: () => this.setView(view),
    })));
    if (!open) {
      this.title.textContent = 'Editor';
      this.buttons.replaceChildren();
      this.text.value = '';
      return;
    }
    const dirty = this.dirty();
    this.shownDirty = dirty;
    this.title.replaceChildren(el('span', { class: 'mono', text: open.path }),
      ...(dirty ? [el('span', { class: 'state-warn', style: 'font-weight:400', text: '  ● modified' })] : []));
    this.buttons.replaceChildren(
      this.viewButtons,
      el('button', { class: 'primary', text: 'Save', title: 'Ctrl+S', disabled: !dirty || open.executable, onclick: () => this.save() }),
      el('button', { class: 'more', text: 'Open in Browser', onclick: () => browse(open.url) }),
      el('button', { class: 'more', text: 'Rename', disabled: open.executable, onclick: () => this.rename() }),
      el('button', { class: 'danger more', text: 'Delete', onclick: () => this.remove() }),
      moreButton());
  },

  async save() {
    const open = this.open;
    if (!open || open.executable || !this.dirty()) return;
    const content = this.text.value;
    const saved = await attempt(() => api.post('/node/page', { path: open.path, content }));
    if (saved === undefined) return;
    open.saved = content;
    this.renderEditor();
    toast(this.node?.status === 'running' ? `Saved ${open.path} (live)` : `Saved ${open.path}`);
  },

  async create() {
    const name = prompt('New page (e.g. about, or blog/first)');
    if (!name) return;
    const result = await attempt(() => api.post('/node/pages', { name }));
    if (result) this.load(result.path);
  },

  async rename() {
    const open = this.open;
    const to = prompt(`Rename ${open.path} to`, open.path);
    if (!to || to === open.path) return;
    const result = await attempt(() => api.post('/node/rename', { from: open.path, to }));
    if (result) {
      open.path = result.path;
      open.url = open.url.replace(/:\/page\/.*$/, ':/page/' + result.path);
      this.renderEditor();
    }
  },

  async remove() {
    const open = this.open;
    if (!confirm(`Delete ${open.path}?`)) return;
    const done = await attempt(() => api.post('/node/delete', { path: open.path }), `Deleted ${open.path}`);
    if (done) {
      this.open = null;
      this.renderEditor();
      this.renderPreview();
    }
  },
};

// ---- Status -----------------------------------------------------------------

app.views.status = {
  mount(root) {
    this.info = el('div', { class: 'info' });
    this.form = el('div', { class: 'settings' });
    this.saveButton = el('button', { class: 'primary', text: 'Save', disabled: true, onclick: () => this.save() });
    this.revertButton = el('button', { text: 'Revert', disabled: true, onclick: () => this.loadSettings(true) });
    this.settingsFooter = el('div', { class: 'settings-footer dim' });
    this.interfaces = el('div', { class: 'scroll' });
    this.log = el('div', { class: 'scroll log' });
    root.append(
      el('div', { class: 'column grow' },
        el('section', { class: 'panel' }, el('header', {}, el('span', { class: 'title grow', text: 'Identity' }),
          el('button', { text: 'Announce', onclick: () => attempt(() => api.post('/announce'), 'Announcing') }),
          el('button', { text: 'Sync now', onclick: () => attempt(() => api.post('/sync'), 'Syncing with the propagation node') }),
          el('button', { class: 'more', text: 'Restart Reticulum', onclick: () => restartReticulum() }), moreButton()), this.info),
        el('section', { class: 'panel grow' }, el('header', {}, el('span', { class: 'title grow', text: 'Settings' }),
          this.revertButton, this.saveButton), el('div', { class: 'scroll' }, this.form, this.settingsFooter))),
      el('div', { class: 'column', style: 'width:42%' },
        el('section', { class: 'panel', style: 'max-height:40%' }, el('header', { text: 'Interfaces' }), this.interfaces),
        el('section', { class: 'panel grow' }, el('header', { text: 'Log' }), this.log)));
    // A fresh form: nothing from the last visit to compare against.
    this.inputs = {};
    this.fields = null;
    this.snapshot = null;
    this.loadSettings(true);
  },

  // Values in the form that differ from settings.json.
  changes() {
    const changes = {};
    for (const field of this.fields || []) {
      const input = this.inputs[field.key];
      const value = field.kind === 'toggle' ? String(input.checked) : input.value;
      if (value !== field.value) changes[field.key] = value;
    }
    return changes;
  },

  markDirty() {
    const dirty = Object.keys(this.changes()).length > 0;
    this.saveButton.disabled = !dirty;
    this.revertButton.disabled = !dirty;
    for (const field of this.fields || []) {
      const row = this.inputs[field.key].closest('.setting');
      row.classList.toggle('changed', field.key in this.changes());
    }
  },

  // Reload settings.json; unsaved edits are kept unless `force`.
  async loadSettings(force = false) {
    const editing = this.form.contains(document.activeElement) || Object.keys(this.changes()).length;
    if (!force && this.fields && editing) return;
    const data = await attempt(() => api.get('/settings'));
    if (!data) return;
    const snapshot = JSON.stringify(data);
    if (!force && snapshot === this.snapshot) return;
    this.snapshot = snapshot;
    this.fields = data.fields;
    this.inputs = {};
    this.form.replaceChildren(...data.fields.map((field) => {
      let input;
      if (field.kind === 'toggle') {
        input = el('input', { type: 'checkbox', checked: field.value === 'true' });
      } else {
        input = el('input', {
          type: field.kind === 'number' ? 'number' : 'text',
          min: field.kind === 'number' ? '0' : null,
          value: field.value,
          placeholder: field.kind === 'optional' ? '(none)' : null,
          class: field.key === 'display_name' ? '' : 'mono',
        });
      }
      // Settings that decide what runs on this computer are changed in the
      // terminal UI, not here (a page script can still be turned off).
      const locked = field.web === 'terminal_only' || (field.web === 'turn_off_only' && field.value !== 'true');
      input.disabled = locked;
      input.addEventListener('input', () => this.markDirty());
      input.addEventListener('change', () => this.markDirty());
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') this.save();
      });
      this.inputs[field.key] = input;
      const id = 'setting-' + field.key;
      input.id = id;
      return el('div', { class: 'setting' },
        el('label', { for: id, class: 'label' }, field.label,
          field.next_start ? el('span', { class: 'next-start', text: 'next start', title: 'Takes effect the next time rettui starts' }) : null),
        el('div', {}, input, el('div', { class: 'help', text: field.help }),
          field.web === 'change' ? null : el('div', { class: 'help locked', text: field.web === 'terminal_only'
            ? 'Changed in the terminal UI (or settings.json), not here: it decides what runs on this computer.'
            : 'Turned on in the terminal UI, not here: scripts run programs on this computer.'
              + (field.value === 'true' ? ' It can be turned off here.' : '') })));
    }));
    const notes = [`Saved in ${data.path}`];
    const file = data.fields.find((f) => f.key === 'rns_config').value;
    if (data.session_rns_config && data.session_rns_config !== file) {
      notes.push(`This session uses the Reticulum config in ${data.session_rns_config}.`);
    }
    this.settingsFooter.replaceChildren(...notes.map((n) => el('div', { text: n })));
    this.markDirty();
  },

  async save() {
    const values = this.changes();
    if (!Object.keys(values).length) return;
    const result = await attempt(() => api.post('/settings', { values }));
    if (!result) return;
    toast(result.notes.length ? 'Saved. ' + result.notes.join('. ') : 'Settings saved');
    this.loadSettings(true);
  },

  // `settings: false` when only the status changed (the settings can't have).
  update({ settings = true } = {}) {
    const s = app.status;
    if (!s) return;
    const net = { starting: 'starting', online: 'online', failed: `failed: ${s.net.error}` }[s.net.state];
    const sync = {
      idle: 'never',
      running: `running (${s.sync.secs}s)`,
      done: `${s.sync.at} · ${s.sync.count} new`,
      failed: `${s.sync.at} · failed: ${s.sync.error}`,
    }[s.sync.state];
    const label = (text) => el('span', { class: 'label', text });
    this.info.replaceChildren(
      label('Display name'), el('span', { style: 'font-weight:600', text: s.display_name }),
      label('LXMF address'), el('div', { class: 'row' }, el('span', { class: 'mono', style: 'color:var(--accent)', text: s.lxmf_address || '(starting)' }),
        s.lxmf_address ? el('button', { text: 'Copy', onclick: () => copy(s.lxmf_address, 'your LXMF address') }) : null),
      label('Network'), el('span', { text: net }),
      label('Propagation node'), el('span', { text: s.propagation_node ? `${s.propagation_node.name}  ${s.propagation_node.hash}` : 'none (pick one in the Network tab)' }),
      label('Last sync'), el('span', { text: sync }),
      label('RNS config'), el('span', { class: 'mono', text: s.rns_config || 'rsReticulum default' }),
      label('Data'), el('span', { class: 'mono', text: s.data_dir || '' }),
      label('Known'), el('span', { text: `${s.known} destinations` }),
      label('Notifications'), notifications.describe());
    this.interfaces.replaceChildren(...s.interfaces.map((i) => el('div', { class: 'iface' },
      el('span', { class: i.online ? 'online' : 'offline', text: i.online ? '● ' : '○ ' }), i.name,
      el('span', { class: 'dim', text: `  ↓${humanBytes(i.rx)} ↑${humanBytes(i.tx)}` }))));
    stickToBottom(this.log, () => this.log.replaceChildren(...s.log.map((line) => el('div', { text: line }))));
    // Pick up changes made elsewhere (the TUI's editor, or by hand).
    if (settings) this.loadSettings();
  },
};

// ---- Reticulum --------------------------------------------------------------

// The Reticulum config file, option by option or as text. Changes are saved
// to the file and apply when Reticulum restarts. Pipe interface commands
// (programs Reticulum runs) can only be changed from the terminal.
app.views.reticulum = {
  panes: true,
  section: 'reticulum',
  textMode: false,

  mount(root) {
    this.sections = el('div', { class: 'scroll' });
    this.fileCard = el('div', { class: 'rns-file' });
    this.addForm = el('div', { class: 'rns-add hidden' });
    this.title = el('span', { class: 'title grow' });
    this.buttons = el('div', { class: 'row' });
    this.form = el('div', { class: 'settings' });
    this.formScroll = el('div', { class: 'scroll' }, this.form);
    this.text = el('textarea', {
      class: 'page-editor rns-text hidden',
      spellcheck: false,
      oninput: () => this.textChanged(),
      onkeydown: (e) => {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
          e.preventDefault();
          this.saveText();
        } else if (e.key === 'Tab' && !e.shiftKey) {
          e.preventDefault();
          this.text.setRangeText('    ', this.text.selectionStart, this.text.selectionEnd, 'end');
          this.textChanged();
        }
      },
    });
    this.textStatus = el('div', { class: 'rns-check hidden' });
    applyWrap(this.text);
    root.append(
      el('div', { class: 'column side' },
        el('section', { class: 'panel' }, el('header', {}, el('span', { class: 'title grow', text: 'Config file' }),
          el('button', { text: 'Restart Reticulum', title: 'Apply saved changes now', onclick: () => restartReticulum() })), this.fileCard),
        el('section', { class: 'panel grow' },
          el('header', {}, el('span', { class: 'title grow', text: 'Sections' }),
            el('button', { text: '+ Interface', onclick: () => this.showAdd() })),
          this.addForm, this.sections)),
      el('section', { class: 'panel grow pane-main' },
        el('header', {}, this.title, this.buttons),
        this.formScroll, this.text, this.textStatus));
    this.inputs = {};
    this.snapshot = null;
    this.load(true);
    if (!this.unloadGuard) {
      this.unloadGuard = true;
      window.addEventListener('beforeunload', (e) => {
        if (app.tab === 'reticulum' && this.dirty()) e.preventDefault();
      });
    }
  },

  update() {
    this.load(false);
  },

  dirty() {
    if (!this.data) return false;
    return this.textMode ? this.text.value !== this.data.text : Object.keys(this.changes()).length > 0;
  },

  // Reload from the server; unsaved edits are kept unless `force`.
  async load(force) {
    if (!force && this.dirty()) return;
    const data = await attempt(() => api.get('/reticulum?section=' + encodeURIComponent(this.section)));
    if (!data) return;
    const snapshot = JSON.stringify(data);
    if (!force && snapshot === this.snapshot) return;
    this.snapshot = snapshot;
    this.data = data;
    this.section = data.section;
    this.render();
  },

  render() {
    const data = this.data;
    this.fileCard.replaceChildren(...[
      el('div', { class: 'mono', text: data.path }),
      data.exists ? null : el('div', { class: 'state-warn', text: 'Not created yet: showing rsReticulum\'s defaults. Saving creates it.' }),
      data.error ? el('div', { class: 'state-bad', text: '✗ Reticulum cannot load this file: ' + data.error })
        : el('div', { class: 'state-ok', text: '✓ The file loads' }),
      ...data.warnings.map((w) => el('div', { class: 'state-warn', text: '! ' + w })),
      el('div', { class: 'dim', text: data.note }),
      data.external_note ? el('div', { class: 'state-warn', text: data.external_note }) : null,
    ].filter(Boolean));
    const item = (s) => el('div', {
      class: 'list-item' + (s.id === this.section && !this.textMode ? ' selected' : '') + (s.interface ? ' rns-iface' : ''),
      onclick: () => this.select(s.id),
    }, el('span', { class: 'main' },
      el('div', { class: 'name' },
        s.interface ? el('span', { class: s.enabled ? 'online' : 'dim', text: s.enabled ? '● ' : '○ ' }) : null, s.title),
      s.interface ? el('div', { class: 'sub', text: s.type || 'no type' }) : null));
    const general = data.sections.filter((s) => !s.interface);
    const interfaces = data.sections.filter((s) => s.interface);
    this.sections.replaceChildren(
      ...general.map(item),
      el('div', { class: 'list-heading', text: `Interfaces · ${interfaces.length}` }),
      ...(interfaces.length ? interfaces.map(item) : [el('div', { class: 'empty', text: 'No interfaces.' })]),
      el('div', { class: 'list-item' + (this.textMode ? ' selected' : ''), onclick: () => this.openText() },
        el('span', { class: 'main' }, el('div', { class: 'name', text: '✎ Edit as text' }),
          el('div', { class: 'sub', text: 'The whole file, comments included' }))));
    if (this.textMode) return this.renderText();
    this.formScroll.classList.remove('hidden');
    this.text.classList.add('hidden');
    this.textStatus.classList.add('hidden');
    this.renderForm();
  },

  current() {
    return this.data.sections.find((s) => s.id === this.section);
  },

  async select(id) {
    if (this.dirty() && !confirm('Discard unsaved changes?')) return;
    setPane(this, 'detail');
    this.textMode = false;
    this.section = id;
    this.snapshot = null;
    await this.load(true);
  },

  renderForm() {
    const section = this.current();
    this.title.textContent = section.interface ? `Interface · ${section.title}` : section.title;
    this.saveButton = el('button', { class: 'primary', text: 'Save', disabled: true, onclick: () => this.save() });
    this.revertButton = el('button', { text: 'Revert', disabled: true, onclick: () => this.load(true) });
    this.buttons.replaceChildren(
      ...(section.interface ? [
        el('button', { class: 'more', text: 'Rename', onclick: () => this.rename(section.title) }),
        el('button', { class: 'danger more', text: 'Delete', onclick: () => this.remove(section.title) }),
      ] : []),
      this.revertButton, this.saveButton, ...(section.interface ? [moreButton()] : []));
    this.inputs = {};
    let group = null;
    const rows = [];
    for (const option of this.data.options) {
      if (option.group !== group) {
        group = option.group;
        rows.push(el('div', { class: 'settings-group', text: group }));
      }
      rows.push(this.optionRow(option));
    }
    this.form.replaceChildren(...rows);
    this.markDirty();
  },

  // The text an option's input starts with ('' when unset).
  initial(option) {
    if (option.value === null || option.value === undefined) return '';
    if (option.kind === 'bool') return /^(yes|true|on|1)$/i.test(option.value.trim()) ? 'yes' : 'no';
    return option.value;
  },

  optionRow(option) {
    const initial = this.initial(option);
    const fallback = option.default ? `default: ${option.default}` : 'not set';
    let input;
    if (option.kind === 'bool' || option.kind === 'choice' || option.key === 'type') {
      const choices = option.kind === 'bool' ? [['yes', 'Yes'], ['no', 'No']]
        : option.key === 'type' ? this.data.types.map((t) => [t.name, `${t.label} · ${t.name}`])
          : option.choices.map((c) => [c, c]);
      if (initial && !choices.some(([v]) => v.toLowerCase() === initial.toLowerCase())) choices.push([initial, initial]);
      input = el('select', {},
        option.key === 'type' ? null : el('option', { value: '', text: `(${fallback})` }),
        ...choices.map(([value, label]) => el('option', { value, text: label, selected: value.toLowerCase() === initial.toLowerCase() })));
    } else {
      input = el('input', {
        type: option.kind === 'secret' ? 'password' : 'text',
        inputmode: option.kind === 'int' || option.kind === 'float' ? 'decimal' : null,
        value: initial,
        placeholder: option.kind === 'list' ? `${fallback} (comma-separated)` : fallback,
        class: 'mono',
        autocomplete: 'off',
      });
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') this.save();
      });
    }
    input.addEventListener('input', () => this.markDirty());
    input.addEventListener('change', () => this.markDirty());
    const id = 'rns-' + option.key;
    input.id = id;
    this.inputs[option.key] = { input, initial: input.value };
    return el('div', { class: 'setting' },
      el('label', { for: id, class: 'label', text: option.label }),
      el('div', {}, input, el('div', { class: 'help' }, el('span', { class: 'mono', text: option.key }), ' · ', option.help)));
  },

  changes() {
    const changes = {};
    for (const [key, { input, initial }] of Object.entries(this.inputs)) {
      if (input.value !== initial) changes[key] = input.value;
    }
    return changes;
  },

  markDirty() {
    if (this.textMode || !this.saveButton) return;
    const changes = this.changes();
    const dirty = Object.keys(changes).length > 0;
    this.saveButton.disabled = !dirty;
    this.revertButton.disabled = !dirty;
    for (const [key, { input }] of Object.entries(this.inputs)) {
      input.closest('.setting').classList.toggle('changed', key in changes);
    }
  },

  report(result, done) {
    if (!result) return;
    toast(result.warnings.length ? `${done}, but: ${result.warnings.join('; ')}` : `${done}. Restart Reticulum to apply.`, result.warnings.length > 0);
  },

  async save() {
    const values = this.changes();
    if (!Object.keys(values).length) return;
    const result = await attempt(() => api.post('/reticulum/options', { section: this.section, values }));
    this.report(result, 'Saved');
    if (result) this.load(true);
  },

  showAdd() {
    const name = el('input', { type: 'text', placeholder: 'Interface name' });
    const type = el('select', {}, ...this.data.types.map((t) => el('option', { value: t.name, text: t.label })));
    const add = async () => {
      if (!name.value.trim()) return name.focus();
      const result = await attempt(() => api.post('/reticulum/interfaces', { action: 'add', name: name.value.trim(), type: type.value }));
      if (!result) return;
      this.report(result, `Added ${name.value.trim()}`);
      this.addForm.classList.add('hidden');
      this.textMode = false;
      this.section = 'interface:' + name.value.trim();
      this.load(true);
    };
    name.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') add();
    });
    this.addForm.replaceChildren(name, type, el('div', { class: 'row' },
      el('button', { class: 'primary', text: 'Add', onclick: add }),
      el('button', { text: 'Cancel', onclick: () => this.addForm.classList.add('hidden') })));
    this.addForm.classList.remove('hidden');
    name.focus();
  },

  async rename(name) {
    const to = prompt(`Rename ${name} to`, name);
    if (!to || to.trim() === name) return;
    const result = await attempt(() => api.post('/reticulum/interfaces', { action: 'rename', name, to: to.trim() }));
    this.report(result, `Renamed ${name}`);
    if (result) {
      this.section = 'interface:' + to.trim();
      this.load(true);
    }
  },

  async remove(name) {
    if (!confirm(`Delete the interface ${name} from the Reticulum config?`)) return;
    const result = await attempt(() => api.post('/reticulum/interfaces', { action: 'delete', name }));
    this.report(result, `Deleted ${name}`);
    if (result) {
      this.section = 'reticulum';
      this.load(true);
    }
  },

  // ---- the file as text ----

  async openText() {
    if (this.textMode) return setPane(this, 'detail');
    if (this.dirty() && !confirm('Discard unsaved changes?')) return;
    setPane(this, 'detail');
    this.textMode = true;
    await this.load(true);
    this.text.value = this.data.text;
    this.renderText();
    this.text.focus();
  },

  renderText() {
    this.formScroll.classList.add('hidden');
    this.text.classList.remove('hidden');
    this.textStatus.classList.remove('hidden');
    if (document.activeElement !== this.text && !this.dirty()) this.text.value = this.data.text;
    const dirty = this.dirty();
    this.title.replaceChildren(el('span', { class: 'mono', text: this.data.path }),
      ...(dirty ? [el('span', { class: 'state-warn', style: 'font-weight:400', text: '  ● modified' })] : []));
    this.buttons.replaceChildren(
      el('button', { text: 'Revert', disabled: !dirty, onclick: () => { this.text.value = this.data.text; this.textChanged(); } }),
      el('button', { class: 'primary', text: 'Save', title: 'Ctrl+S', disabled: !dirty, onclick: () => this.saveText() }));
    this.checkText();
  },

  textChanged() {
    this.renderText();
  },

  // Check the text as it is typed, like the terminal editor does.
  checkText() {
    clearTimeout(this.checkTimer);
    this.checkTimer = setTimeout(async () => {
      const result = await api.post('/reticulum/check', { text: this.text.value }).catch(() => null);
      if (!result) return;
      this.textStatus.className = 'rns-check ' + (result.error ? 'state-bad' : result.warnings.length ? 'state-warn' : 'state-ok');
      this.textStatus.textContent = result.error ? `✗ ${result.error} (cannot be saved like this)`
        : result.warnings.length ? '! ' + result.warnings.join(' · ') : '✓ Loads';
    }, 300);
  },

  async saveText() {
    if (!this.dirty()) return;
    const result = await attempt(() => api.post('/reticulum/text', { text: this.text.value }));
    this.report(result, 'Saved the Reticulum config');
    if (result) {
      await this.load(true);
      this.text.value = this.data.text;
      this.renderText();
    }
  },
};

// Restart the Reticulum stack (from the Status and Reticulum sections).
async function restartReticulum() {
  const question = app.status?.external_shared_instance
    ? 'Reconnect to the shared instance?\nIts own program (such as rnsd) applies interface changes when it restarts.'
    : 'Restart Reticulum?\nLinks and transfers in progress stop; hubs reconnect and your node starts again.';
  if (!confirm(question)) return;
  attempt(() => api.post('/reticulum/restart'), 'Restarting Reticulum');
}

// ---- start ------------------------------------------------------------------

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && document.body.classList.contains('drawer-open')) return setDrawer(false);
  const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName);
  // As in the terminal UI, Esc stops writing (the shortcuts work again),
  // unless it closed something first (the list @ opens).
  if (typing && e.key === 'Escape' && !e.defaultPrevented) return document.activeElement.blur();
  if (typing || e.ctrlKey || e.metaKey || e.altKey) return;
  if (e.key >= '1' && e.key <= String(TABS.length)) switchTab(TABS[Number(e.key) - 1].id);
  else if (e.key === '/' && app.tab === 'network') {
    e.preventDefault();
    app.views.network.search.focus();
  }
});

window.addEventListener('hashchange', () => {
  const id = location.hash.slice(1);
  if (id !== app.tab && app.views[id]) switchTab(id);
});

refreshStatus().then(() => {
  const initial = location.hash.slice(1);
  switchTab(app.views[initial] ? initial : 'messages');
  listen();
  notifications.start();
  setTimeout(prefetch, 300);
});
