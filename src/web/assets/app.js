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
  const response = await fetch('api' + path, options);
  if (response.status === 401) {
    location.reload();
    throw new Error('not logged in');
  }
  if (response.status === 413) {
    throw new Error('Too large to send: over 64 MB, or over the limit of a proxy in front of rettui (nginx takes 1 MB unless its client_max_body_size is raised)');
  }
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error || response.statusText);
  return data;
}
const api = {
  get: (path) => request('GET', path),
  post: (path, body = {}) => request('POST', path, body),
  delete: (path) => request('DELETE', path),
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
    // No clipboard API (a plain http page): the old way. A textarea's text
    // is its content, not a value attribute.
    const area = el('textarea', {}, text);
    document.body.append(area);
    area.select();
    document.execCommand('copy');
    area.remove();
  }
  toast(`Copied ${what}`);
}

// A time of day as the Clock setting has it: 14:05, or 2:05 PM.
function clockTime(date, seconds = false) {
  const twelve = app.status?.clock === '12-hour';
  return date.toLocaleTimeString(twelve ? 'en-US' : [], {
    hour: twelve ? 'numeric' : '2-digit', minute: '2-digit', second: seconds ? '2-digit' : undefined, hour12: twelve,
  });
}

// A day as the Dates setting has it: Oct 07, 07 Oct, or 10-07 (with the
// year, for another year's).
function clockDay(date, withYear = false) {
  const pad = (n) => String(n).padStart(2, '0');
  const month = date.toLocaleDateString('en-US', { month: 'short' });
  const [d, m, y] = [pad(date.getDate()), pad(date.getMonth() + 1), date.getFullYear()];
  switch (app.status?.date_style) {
    case 'day-month': return withYear ? `${d} ${month} ${y}` : `${d} ${month}`;
    case 'year-month-day': return withYear ? `${y}-${m}-${d}` : `${m}-${d}`;
    default: return withYear ? `${month} ${d} ${y}` : `${month} ${d}`;
  }
}

function timeLabel(unixSeconds) {
  const date = new Date(unixSeconds * 1000);
  const now = new Date();
  const hm = clockTime(date);
  if (date.toDateString() === now.toDateString()) return hm;
  return clockDay(date, date.getFullYear() !== now.getFullYear()) + ' ' + hm;
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
  const drafts = draftsOf(view);
  if (view.draftKey != null) {
    const draft = view.takeDraft();
    if (draft) drafts.set(view.draftKey, draft);
    else drafts.delete(view.draftKey);
  }
  view.draftKey = key;
  view.putDraft(drafts.get(key) || null);
  drafts.delete(key);
  keepDrafts(view);
}

// Put back what failed to send, before anything written since, in the
// conversation or room it was sent to.
function draftRestore(view, key, draft) {
  const drafts = draftsOf(view);
  if (view.draftKey === key) view.putDraft(view.joinDrafts(draft, view.takeDraft()));
  else drafts.set(key, view.joinDrafts(draft, drafts.get(key) || null));
  keepDrafts(view);
}

// Drafts outlive a reload (phones reload pages they've put away, and a
// restart of rettui reloads them all): their text, and what they reply to,
// are kept in this browser under the view's `draftStore`. Attached files
// aren't: a page can't keep those.
const MAX_KEPT_DRAFT = 100_000;

function draftsOf(view) {
  if (!view.drafts) {
    view.drafts = new Map();
    try {
      const kept = JSON.parse(localStorage.getItem(view.draftStore) || '{}');
      for (const [key, draft] of Object.entries(kept)) {
        if (typeof draft?.text !== 'string') continue;
        view.drafts.set(key, { text: draft.text, files: [], reply: draft.reply || null });
      }
    } catch { /* kept in this page only */ }
  }
  return view.drafts;
}

// Keep the drafts, the open one's too: soon (as it's typed), or `now`.
function keepDrafts(view, now = false) {
  clearTimeout(view.draftTimer);
  if (!now) {
    view.draftTimer = setTimeout(() => keepDrafts(view, true), 400);
    return;
  }
  const kept = {};
  const keep = (key, draft) => {
    if (draft?.text || draft?.reply) kept[key] = { text: (draft.text || '').slice(0, MAX_KEPT_DRAFT), reply: draft.reply || undefined };
  };
  for (const [key, draft] of draftsOf(view)) keep(key, draft);
  if (view.draftKey != null) keep(view.draftKey, view.takeDraft());
  try {
    if (Object.keys(kept).length) localStorage.setItem(view.draftStore, JSON.stringify(kept));
    else localStorage.removeItem(view.draftStore);
  } catch { /* full, or not allowed: kept in this page only */ }
}

// Leaving (or put in the background, where a phone may close it): keep
// them now.
for (const event of ['pagehide', 'visibilitychange']) {
  addEventListener(event, () => {
    if (event === 'visibilitychange' && document.visibilityState !== 'hidden') return;
    for (const view of Object.values(app.views)) if (view.draftStore && view.draftKey != null) keepDrafts(view, true);
  });
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

// Someone's icon, as Sideband and MeshChat show one: a Material Design Icon
// in a colour on a colour, or with no icon (or one rettui doesn't know),
// the first letter of their name.
function avatar(icon, name) {
  const letter = ([...(name || '').trim()][0] || '?').toUpperCase();
  return el('span', {
    class: 'avatar' + (icon?.glyph ? ' icon' : ''),
    style: icon ? `color:${icon.fg};background:${icon.bg}` : null,
    title: icon ? `Icon: ${icon.name}` : null,
    text: icon?.glyph || letter,
  });
}

// Recording a voice message: the microphone's sound, as it comes, made 8 kHz
// mono 16-bit (what Codec2 takes, which rettui sends it as) when it stops.
// Plain Web Audio, so every browser can, Safari too; browsers only give
// the microphone to a secure page (HTTPS, or localhost).
const recorder = {
  // Longest recording (five minutes, as rettui takes).
  MAX_SECONDS: 300,
  RATE: 8000,

  available() {
    return !!navigator.mediaDevices?.getUserMedia && window.isSecureContext;
  },

  async start() {
    this.stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
    const AudioContextClass = window.AudioContext || window.webkitAudioContext;
    this.context = new AudioContextClass();
    const source = this.context.createMediaStreamSource(this.stream);
    this.processor = this.context.createScriptProcessor(4096, 1, 1);
    this.chunks = [];
    this.processor.onaudioprocess = (e) => this.chunks.push(new Float32Array(e.inputBuffer.getChannelData(0)));
    source.connect(this.processor);
    this.processor.connect(this.context.destination);
    this.started = Date.now();
  },

  seconds() {
    return this.started ? (Date.now() - this.started) / 1000 : 0;
  },

  // Stop: the recording as a WAV file (base64), and how long it is; none if
  // cancelled.
  async stop(keep = true) {
    const rate = this.context?.sampleRate || 48000;
    this.processor?.disconnect();
    for (const track of this.stream?.getTracks() || []) track.stop();
    await this.context?.close().catch(() => {});
    const chunks = this.chunks || [];
    this.stream = this.context = this.processor = this.chunks = null;
    this.started = 0;
    if (!keep) return null;
    const length = chunks.reduce((n, c) => n + c.length, 0);
    const all = new Float32Array(length);
    let at = 0;
    for (const chunk of chunks) {
      all.set(chunk, at);
      at += chunk.length;
    }
    // 8 kHz: each sample the mean of those it stands for (so nothing above
    // 4 kHz folds back down), at most five minutes.
    const step = rate / this.RATE;
    const count = Math.min(Math.floor(length / step), this.MAX_SECONDS * this.RATE);
    const samples = new Int16Array(count);
    for (let i = 0; i < count; i++) {
      const from = Math.floor(i * step);
      const to = Math.max(from + 1, Math.floor((i + 1) * step));
      let sum = 0;
      for (let j = from; j < to; j++) sum += all[j];
      samples[i] = Math.max(-1, Math.min(1, sum / (to - from))) * 0x7fff;
    }
    if (!count) return null;
    const wav = this.wav(samples);
    return { wav: wav.base64, url: URL.createObjectURL(new Blob([wav.bytes], { type: 'audio/wav' })), seconds: count / this.RATE };
  },

  // Samples as a WAV file: its bytes, and base64.
  wav(samples) {
    const buffer = new ArrayBuffer(44 + samples.length * 2);
    const view = new DataView(buffer);
    const text = (at, s) => [...s].forEach((c, i) => view.setUint8(at + i, c.charCodeAt(0)));
    text(0, 'RIFF');
    view.setUint32(4, 36 + samples.length * 2, true);
    text(8, 'WAVEfmt ');
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, this.RATE, true);
    view.setUint32(28, this.RATE * 2, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    text(36, 'data');
    view.setUint32(40, samples.length * 2, true);
    new Int16Array(buffer, 44).set(samples);
    let binary = '';
    const bytes = new Uint8Array(buffer);
    for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return { bytes, base64: btoa(binary) };
  },
};

// Whether a file attached is a picture that goes smaller, as the setting
// has it (not a GIF, which may move): see shrink.rs.
function shrinks(file) {
  const size = app.status?.picture_size;
  return !!size && size !== 'original' && /\.(jpe?g|png|webp|bmp)$/i.test(file.name);
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
// The same, darker, to read on the light theme.
const NICK_COLORS_LIGHT = ['#c42b2b', '#17784a', '#8a6400', '#1a5fb4', '#9a2f99', '#12698e', '#b8520a', '#6a3fd0', '#1c7d6e', '#c2306a'];

function nickColor(src) {
  const seed = src ? parseInt(src.slice(0, 2), 16) || 0 : 0;
  const colors = document.documentElement.dataset.theme === 'light' ? NICK_COLORS_LIGHT : NICK_COLORS;
  return colors[seed % colors.length];
}

// The web UI's colours, chosen in each browser: dark (rettui's own), light,
// or as the device is set (and following it when it changes).
const theme = {
  query: window.matchMedia('(prefers-color-scheme: light)'),
  chosen() {
    try { return localStorage.getItem('rettui.theme') || 'dark'; } catch { return 'dark'; }
  },
  apply() {
    const chosen = this.chosen();
    const light = chosen === 'light' || (chosen === 'auto' && this.query.matches);
    document.documentElement.dataset.theme = light ? 'light' : 'dark';
  },
  set(chosen) {
    try { localStorage.setItem('rettui.theme', chosen); } catch { /* this page only */ }
    this.apply();
    // Colours drawn by script (names in rooms) follow on the next draw.
    app.views[app.tab]?.update?.();
  },
};
theme.apply();
theme.query.addEventListener('change', () => theme.chosen() === 'auto' && theme.set('auto'));

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
  liveWatch.update();
  // The version, marked when a newer release is out (it links to that).
  const version = $('#sidebar .version');
  if (version) {
    if (!version.dataset.current) Object.assign(version.dataset, { current: version.textContent, home: version.href });
    const update = app.status?.update;
    version.classList.toggle('update', !!update);
    version.textContent = update ? `${version.dataset.current} ↑` : version.dataset.current;
    version.href = update ? update.url : version.dataset.home;
    version.title = update ? `rettui ${update.version} is out: what's new` : 'rettui on GitHub';
  }
  // Unread in the tab's title, for when the page is in the background.
  const count = (unread.messages || 0) + (unread.channels || 0);
  document.title = count ? `(${count}) rettui` : 'rettui';
  renderAppbar();
  if (app.status) {
    $('#who-name').replaceChildren(...[app.status.icon ? avatar(app.status.icon, app.status.display_name) : null, app.status.display_name].filter(Boolean));
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
  emojiPicker.close(false);
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
    // A new install: the guide opens by itself, once a page load.
    if (app.status.welcome && !app.guideShown) {
      app.guideShown = true;
      gettingStarted();
    }
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
    // Status draws from the status too: again once it's in, in case its
    // own update finished first (with the one before).
    refreshStatus().then(() => app.tab === 'status' && app.views.status.update({ settings: false })),
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
      this.worker = await navigator.serviceWorker.register('sw.js');
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
    refreshStatusView();
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
    refreshStatusView();
  },

  // Tell rettui whether this browser shows it now (kept alive as the page
  // hides, which is when it matters).
  showing() {
    if (this.push !== 'on' || !this.endpoint) return;
    fetch('api/push/showing', {
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
    refreshStatusView();
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
      const options = { body, tag, renotify: true, icon: 'brand/icon.png', data: { target } };
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

// Redraw the Status section, if it has been drawn (it shows how
// notifications stand in this browser).
function refreshStatusView() {
  if (app.views.status.root) app.views.status.update();
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
  events = new EventSource('api/events' + (lastNotice ? `?since=${lastNotice}` : ''));
  // Each change says what it touched: "all", or parts such as
  // "status,peers": the counters and the log ("status"), the hosted node's
  // counters ("node"), peers heard ("peers"), RRC ("channels": the
  // Channels section, and unread counts in the sidebar).
  events.onmessage = (e) => {
    const parts = new Set((e.data.split(' ')[1] || 'all').split(','));
    if (parts.has('all') || (parts.has('node') && app.tab === 'node') || (parts.has('channels') && app.tab === 'channels')) refresh();
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

// A formatted message's text: HTML the server made from its Markdown or
// Micron (all of the sender's text escaped). Micron links open in the
// Browser (or a conversation, for lxmf@ links).
function formattedContent(html) {
  const node = el('div', { class: 'content formatted', onclick: (e) => {
    const link = e.target.closest('.m-link');
    if (!link) return;
    e.preventDefault();
    browse(link.dataset.url);
  } });
  node.innerHTML = html;
  return node;
}

// ---- Live location ------------------------------------------------------------

// While this page's device shares its location live with anyone, its
// position goes to rettui as it moves (at most every 30 seconds; rettui
// sends at most one update a minute).
const liveWatch = {
  id: null,
  last: 0,
  async post(found) {
    const c = found.coords;
    this.last = Date.now();
    await api.post('/live/position', { latitude: c.latitude, longitude: c.longitude, accuracy: c.accuracy, altitude: c.altitude }).catch(() => {});
  },
  // Watching when a share is the device's, not otherwise.
  update() {
    const wanted = (app.status?.live || []).some((s) => s.source === 'device') && window.isSecureContext && navigator.geolocation;
    if (wanted && this.id === null) {
      this.id = navigator.geolocation.watchPosition((found) => {
        if (Date.now() - this.last >= 30000) this.post(found);
      }, () => {}, { enableHighAccuracy: true, maximumAge: 30000 });
    } else if (!wanted && this.id !== null) {
      navigator.geolocation.clearWatch(this.id);
      this.id = null;
    }
  },
};

// ---- Map ----------------------------------------------------------------------

// A map to drag and zoom: tiles in Web Mercator (as OpenStreetMap's are),
// fetched through rettui (see the Map tiles setting), with `places` on it
// and their accuracy around them. With no tiles, a grid. `onPick` hears
// about a place clicked.
function slippyMap(data, places, onPick) {
  const TILE = 256;
  const [LEAST, MOST] = [1, 19];
  const project = (lat, lon, z) => {
    const n = TILE * 2 ** z;
    const s = Math.sin(Math.max(-85.05, Math.min(85.05, lat)) * Math.PI / 180);
    return [(lon + 180) / 360 * n, (0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)) * n];
  };
  const unproject = (x, y, z) => {
    const n = TILE * 2 ** z;
    return [Math.atan(Math.sinh(Math.PI * (1 - 2 * y / n))) * 180 / Math.PI, x / n * 360 - 180];
  };
  let zoom = 3;
  let center = [20, 0];
  const tiles = el('div', { class: 'map-tiles' + (data.tiles ? '' : ' none') });
  const marks = el('div', { class: 'map-marks' });
  const button = (text, title, onclick) => el('button', { type: 'button', text, title, onclick });
  const node = el('div', { class: 'map' }, tiles, marks,
    el('div', { class: 'map-controls' },
      button('+', 'Closer', () => zoomAt(1)),
      button('−', 'Further', () => zoomAt(-1)),
      button('⤢', 'All of them', () => fit())),
    data.tiles && data.credit?.includes('OpenStreetMap')
      ? el('a', { class: 'map-credit', href: 'https://www.openstreetmap.org/copyright', target: '_blank', rel: 'noopener noreferrer', text: data.credit })
      : el('span', { class: 'map-credit', text: data.tiles ? data.credit || '' : 'No map tiles (the Map tiles setting is empty)' }));
  const shown = new Map();
  for (const p of places) {
    p.circle = el('div', { class: 'map-circle', hidden: true });
    p.marker = el('button', { type: 'button', class: 'map-marker' + (p.key ? '' : ' station'), title: p.label, onclick: () => onPick(p) },
      p.icon ? avatar(p.icon, p.name) : el('span', { class: 'map-pin' }),
      el('span', { class: 'map-label', text: p.name }));
    marks.append(p.circle, p.marker);
  }
  const size = () => [node.clientWidth || 600, node.clientHeight || 400];
  function draw() {
    const [w, h] = size();
    const [cx, cy] = project(center[0], center[1], zoom);
    // Whole pixels, so tiles meet without seams.
    const [left, top] = [Math.round(cx - w / 2), Math.round(cy - h / 2)];
    if (data.tiles) {
      const n = 2 ** zoom;
      const wanted = new Set();
      for (let tx = Math.floor(left / TILE); tx <= Math.floor((left + w) / TILE); tx++) {
        for (let ty = Math.max(0, Math.floor(top / TILE)); ty <= Math.min(n - 1, Math.floor((top + h) / TILE)); ty++) {
          const id = `${zoom}/${tx}/${ty}`;
          wanted.add(id);
          let img = shown.get(id);
          if (!img) {
            // Round the world and back, east and west.
            const x = ((tx % n) + n) % n;
            img = el('img', { class: 'map-tile', alt: '', draggable: false, src: `api/map/tiles/${zoom}/${x}/${ty}` });
            img.addEventListener('error', () => img.classList.add('missing'));
            shown.set(id, img);
            tiles.append(img);
          }
          img.style.transform = `translate(${tx * TILE - left}px, ${ty * TILE - top}px)`;
        }
      }
      for (const [id, img] of shown) {
        if (!wanted.has(id)) {
          img.remove();
          shown.delete(id);
        }
      }
    } else {
      tiles.style.backgroundPosition = `${-left}px ${-top}px`;
    }
    for (const p of places) {
      const [x, y] = project(p.latitude, p.longitude, zoom);
      p.marker.style.transform = `translate(${Math.round(x - left)}px, ${Math.round(y - top)}px)`;
      // How far off it may be, once that shows.
      const perPixel = 156543.03 * Math.cos(p.latitude * Math.PI / 180) / 2 ** zoom;
      const radius = (p.accuracy || 0) / perPixel;
      p.circle.hidden = radius < 8;
      Object.assign(p.circle.style, { width: `${2 * radius}px`, height: `${2 * radius}px`, transform: `translate(${x - left - radius}px, ${y - top - radius}px)` });
    }
  }
  // Closer or further, the spot at (px, py) staying where it is.
  function zoomAt(step, px, py) {
    const next = Math.max(LEAST, Math.min(MOST, zoom + step));
    if (next === zoom) return;
    const [w, h] = size();
    px ??= w / 2;
    py ??= h / 2;
    const [cx, cy] = project(center[0], center[1], zoom);
    const spot = unproject(cx - w / 2 + px, cy - h / 2 + py, zoom);
    zoom = next;
    const [sx, sy] = project(spot[0], spot[1], zoom);
    center = unproject(sx - px + w / 2, sy - py + h / 2, zoom);
    draw();
  }
  // All of them in view, as close as that goes (a street's worth for one).
  function fit(among = places) {
    if (!among.length) return draw();
    const [w, h] = size();
    zoom = among.length === 1 ? 15 : LEAST;
    for (let z = MOST; among.length > 1 && z >= LEAST; z--) {
      const xs = among.map((p) => project(p.latitude, p.longitude, z));
      const across = Math.max(...xs.map(([x]) => x)) - Math.min(...xs.map(([x]) => x));
      const down = Math.max(...xs.map(([, y]) => y)) - Math.min(...xs.map(([, y]) => y));
      if (across <= w - 120 && down <= h - 80) {
        zoom = Math.min(z, 16);
        break;
      }
    }
    const xs = among.map((p) => project(p.latitude, p.longitude, zoom));
    const mid = (values) => (Math.max(...values) + Math.min(...values)) / 2;
    center = unproject(mid(xs.map(([x]) => x)), mid(xs.map(([, y]) => y)), zoom);
    draw();
  }
  // Dragged with a finger or the mouse; two fingers pinch.
  const pointers = new Map();
  let pinch = null;
  node.addEventListener('pointerdown', (e) => {
    if (e.target.closest('button, a')) return;
    node.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, [e.clientX, e.clientY]);
    node.classList.add('dragging');
  });
  node.addEventListener('pointermove', (e) => {
    const was = pointers.get(e.pointerId);
    if (!was) return;
    pointers.set(e.pointerId, [e.clientX, e.clientY]);
    if (pointers.size === 1) {
      const [cx, cy] = project(center[0], center[1], zoom);
      center = unproject(cx - (e.clientX - was[0]), cy - (e.clientY - was[1]), zoom);
      center[0] = Math.max(-85, Math.min(85, center[0]));
      draw();
    } else if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      const apart = Math.hypot(a[0] - b[0], a[1] - b[1]);
      if (pinch === null) pinch = apart;
      else if (apart / pinch > 1.5 || apart / pinch < 0.66) {
        zoomAt(apart > pinch ? 1 : -1);
        pinch = apart;
      }
    }
  });
  const lift = (e) => {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch = null;
    if (!pointers.size) node.classList.remove('dragging');
  };
  node.addEventListener('pointerup', lift);
  node.addEventListener('pointercancel', lift);
  node.addEventListener('wheel', (e) => {
    e.preventDefault();
    const box = node.getBoundingClientRect();
    zoomAt(e.deltaY < 0 ? 1 : -1, e.clientX - box.left, e.clientY - box.top);
  }, { passive: false });
  node.addEventListener('dblclick', (e) => {
    if (e.target.closest('button, a')) return;
    const box = node.getBoundingClientRect();
    zoomAt(1, e.clientX - box.left, e.clientY - box.top);
  });
  const resized = new ResizeObserver(() => draw());
  resized.observe(node);
  return {
    node, fit, draw,
    // One place in the middle, close enough to see where.
    show(p) {
      zoom = Math.max(zoom, 13);
      center = [p.latitude, p.longitude];
      draw();
    },
    stop: () => resized.disconnect(),
  };
}

// Where everyone was, from the newest location each has shared (and this
// station, if its Location is set), on a map and in a list. With `at`, a
// message's location is shown as well (it may not be their newest).
async function mapDialog({ at = null, name = '', key = null } = {}) {
  const data = await attempt(() => api.get('/locations'));
  if (!data) return;
  const places = data.places.slice();
  let focus = key ? places.find((p) => p.key === key) : null;
  if (at && !(focus && focus.latitude === at.latitude && focus.longitude === at.longitude)) {
    focus = { ...at, name: name ? `${name} (this message)` : 'This message', key: null, at: null, icon: null };
    places.push(focus);
  }
  if (!places.length) return toast('No one has shared a location yet (📍 in the message box shares one)');
  const rows = new Map();
  const pick = (p) => {
    for (const q of places) {
      q.marker.classList.toggle('picked', q === p);
      rows.get(q)?.classList.toggle('selected', q === p);
    }
    rows.get(p)?.scrollIntoView({ block: 'nearest' });
    view.show(p);
  };
  const view = slippyMap(data, places, (p) => pick(p));
  const list = el('div', { class: 'map-list' });
  const station = data.here ? data.places[0] : null;
  const close = dialog('Map', (close) => {
    for (const p of places) {
      const about = [p.at ? `${ago(p.at)} ago` : p === station ? 'From the Location setting' : '', p.away].filter(Boolean).join(' · ');
      const row = el('div', { class: 'list-item', onclick: () => pick(p) },
        p.icon ? avatar(p.icon, p.name) : null,
        el('div', { class: 'main' },
          el('div', { class: 'name', text: p.name }),
          el('div', { class: 'sub', text: about || p.label }),
          el('div', { class: 'row map-actions' },
            p.key ? el('button', { class: 'inline', text: 'Messages', title: 'Open your conversation', onclick: (e) => {
              e.stopPropagation();
              close();
              app.views.messages.selected = p.key;
              switchTab('messages', { focus: true });
              setPane(app.views.messages, 'detail');
            } }) : null,
            el('a', { href: p.map, target: '_blank', rel: 'noopener noreferrer', text: 'OpenStreetMap ↗', onclick: (e) => e.stopPropagation() }))));
      rows.set(p, row);
      list.append(row);
    }
    return [el('div', { class: 'map-body' }, view.node, list)];
  }, { className: 'map-dialog', onclose: () => view.stop() });
  // Once it's on the page and has a size.
  requestAnimationFrame(() => {
    view.fit();
    if (focus) pick(focus);
  });
  return close;
}

// ---- Paper messages ---------------------------------------------------------

// A dialog over the page: closed by its buttons, Escape or a click beside
// it. Its close function (which `onclose` hears about).
function dialog(title, body, { className = '', onclose } = {}) {
  const onKey = (e) => {
    if (e.key === 'Escape') close();
  };
  const close = () => {
    overlay.remove();
    document.removeEventListener('keydown', onKey);
    onclose?.();
  };
  const overlay = el('div', { class: 'overlay', onclick: (e) => e.target === overlay && close() },
    el('div', { class: 'dialog ' + className, role: 'dialog', 'aria-label': title },
      el('header', {}, el('span', { class: 'title grow', text: title }),
        el('button', { class: 'close', text: '×', title: 'Close', onclick: () => close() })),
      body(close)));
  document.addEventListener('keydown', onKey);
  document.body.append(overlay);
  return close;
}

// A destination's path, as rnpath shows it: found (asked for if none is
// known, up to three times over a minute), or forgotten when it has gone
// stale.
function pathDialog(hash, name) {
  let open = true;
  const status = el('p', { class: 'path-status' });
  const show = (state) => {
    status.textContent = state.text;
    status.className = 'path-status ' + state.state;
  };
  const find = async () => {
    show({ state: 'waiting', text: 'looking for a path…' });
    // How it's going ("path request 2 of 3"), while the answer is awaited.
    const watching = setInterval(async () => {
      const state = await api.get(`/path/${hash}`).catch(() => null);
      if (open && state?.state === 'waiting') show(state);
    }, 1000);
    try {
      const state = await api.post(`/path/${hash}`);
      if (open) show(state);
    } catch (e) {
      if (open) show({ state: 'failed', text: e.message });
    } finally {
      clearInterval(watching);
    }
  };
  const forget = async () => {
    if (await attempt(() => api.delete(`/path/${hash}`))) show({ state: 'none', text: 'forgotten: the next use asks for a fresh one' });
  };
  // A probe, as rnprobe sends: how quickly it answers.
  const probed = el('p', { class: 'path-status', hidden: true });
  const probe = async () => {
    const showProbe = (state) => {
      probed.hidden = false;
      probed.textContent = 'Probe: ' + state.text;
      probed.className = 'path-status ' + state.state;
    };
    showProbe({ state: 'waiting', text: 'waiting for an answer…' });
    const watching = setInterval(async () => {
      const state = await api.get(`/probe/${hash}`).catch(() => null);
      if (open && state?.state === 'waiting') showProbe(state);
    }, 1000);
    try {
      const state = await api.post(`/probe/${hash}`);
      if (open) showProbe(state);
    } catch (e) {
      if (open) showProbe({ state: 'failed', text: e.message });
    } finally {
      clearInterval(watching);
    }
  };
  dialog(`Path to ${name || hash}`, () => el('div', { class: 'path-dialog' },
    el('p', { class: 'dim mono', text: hash }),
    status,
    probed,
    el('p', { class: 'dim help', text: 'Finding asks for a path if none is known, up to three times over a minute: one request often goes unanswered. Forget a path that has gone stale (the destination moved, or a node on the way went), and the next use asks for a fresh one. A probe times an answer: a probe packet for LXMF addresses (as rnprobe sends), a Link for nodes and hubs, which don\'t answer those.' }),
    el('div', { class: 'buttons' },
      el('button', { class: 'primary', text: 'Find path', onclick: find }),
      el('button', { text: 'Probe', onclick: probe }),
      el('button', { text: 'Forget path', onclick: forget }))),
  { className: 'path', onclose: () => { open = false; } });
  find();
}

// The getting-started guide: an entry point to reach others, finding more
// over time, a propagation node, and where to learn more.
async function gettingStarted() {
  const g = await attempt(() => api.get('/guide'));
  if (!g) return;
  let finished = false;
  const name = el('input', { type: 'text', value: g.name, maxlength: 128 });
  const option = (key, label, help, { on, already } = {}) => {
    const box = el('input', { type: 'checkbox', checked: already || on, disabled: !!already });
    return { key, box, node: el('label', { class: 'guide-option' }, box,
      el('span', {}, el('span', { text: already ? `${label} (${already})` : label }), el('span', { class: 'dim help', text: help }))) };
  };
  // Each entry point, under its region, with whether it answered when
  // tried: one that didn't is greyed out and can't be ticked. Until one is
  // ticked or unticked, the ticks follow the fastest as they answer.
  let byHand = false;
  const entry = (e) => {
    const box = el('input', { type: 'checkbox', checked: e.present || e.on, disabled: e.present, onchange: () => { byHand = true; } });
    const status = el('span', { class: 'reach' });
    const help = el('span', { class: 'dim help' });
    const node = el('label', { class: 'guide-option' }, box,
      el('span', {}, el('span', {}, el('span', { text: e.name }), ' ', el('span', { class: 'dim', text: `${e.host}:${e.port}` }), ' ', status), help));
    const show = (reach) => {
      choice.reach = reach;
      if (e.present) {
        status.textContent = 'in your Reticulum config already';
        status.className = 'reach dim';
        help.hidden = true;
        return;
      }
      status.textContent = reach.label;
      status.className = 'reach ' + reach.state;
      const down = reach.state === 'down';
      if (down) box.checked = false;
      else if (!byHand) box.checked = !!reach.on;
      box.disabled = down;
      node.classList.toggle('down', down);
      help.textContent = down ? reach.help : '';
      help.hidden = !down;
    };
    const choice = { key: 'connect', box, node, show, present: e.present };
    show({ ...e.reach, on: e.on });
    return choice;
  };
  const options = [];
  const entries = [];
  const nodes = [];
  const noneNote = el('p', { class: 'guide-status warn', text: g.help.none_answered });
  const noneAnswered = () => {
    const offered = entries.filter((c) => !c.present);
    return offered.length > 0 && offered.every((c) => c.reach.state === 'down');
  };
  if (!g.external) {
    nodes.push(el('p', { class: 'dim help guide-entries', text: g.help.connect }));
    let region = null;
    for (const e of g.entries) {
      if (e.region !== region) {
        region = e.region;
        nodes.push(el('p', { class: 'dim guide-region', text: 'Entry points · ' + region }));
      }
      const choice = entry(e);
      entries.push(choice);
      options.push(choice);
      nodes.push(choice.node);
    }
    noneNote.hidden = !noneAnswered();
    nodes.push(noneNote);
    const discover = option('discover', 'Also find entry points near you over time (interface discovery)', g.help.discover,
      { on: g.defaults.discover, already: g.has_discovery && 'on already' });
    options.push(discover);
    nodes.push(discover.node);
  }
  const autoPropagation = option('auto_propagation', 'Pick a propagation node automatically', g.help.auto_propagation, { on: g.defaults.auto_propagation });
  options.push(autoPropagation);
  nodes.push(autoPropagation.node);
  // While it's ticked, what picking one means.
  const autoWarning = el('p', { class: 'guide-status warn guide-warning', text: g.help.auto_propagation_warning });
  const showAutoWarning = () => { autoWarning.hidden = !autoPropagation.box.checked; };
  autoPropagation.box.addEventListener('change', showAutoWarning);
  showAutoWarning();
  nodes.push(autoWarning);
  // Recommended, but off unless ticked.
  const updateCheck = option('update_check', 'Check for updates once a day (recommended)', g.help.update_check, { on: g.defaults.update_check });
  options.push(updateCheck);
  nodes.push(updateCheck.node);
  // The entry points' tries finish over a few seconds.
  let open = true;
  (async () => {
    while (open && entries.some((c) => c.reach.state === 'checking')) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      const reaches = open ? await api.get('/guide/reach').catch(() => null) : null;
      if (!open || !Array.isArray(reaches)) continue;
      entries.forEach((c, i) => reaches[i] && c.show(reaches[i]));
      noneNote.hidden = !noneAnswered();
    }
  })();
  const status = g.interfaces_online === 0 ? ['warn', '○ Not connected to anyone yet']
    : g.heard === 0 ? ['warn', `◌ ${g.interfaces_online} interface(s) online, nobody heard yet`]
      : ['ok', `● Connected: ${g.interfaces_online} interface(s) online, ${g.heard} peers and nodes heard`];
  const close = dialog('Getting started', (close) => [
    el('p', { class: 'dim', text: g.help.intro }),
    el('p', { class: 'guide-status ' + status[0], text: status[1] }),
    g.shared_note ? el('p', { class: 'guide-status warn', text: g.shared_note }) : null,
    el('label', { class: 'field' }, el('span', { text: 'Your name' }), name, el('span', { class: 'dim help', text: g.help.name })),
    el('p', { class: 'dim help' }, g.help.identity + ' ', el('code', { text: g.identity_file })),
    g.external ? el('p', { class: 'dim', text: `${g.external_note}: add entry points in that program's Reticulum config.` }) : null,
    ...nodes,
    el('div', { class: 'guide-links' }, el('span', { class: 'dim', text: 'Learn more' }),
      g.links.map((l) => el('span', { class: 'guide-link' },
        el('a', { href: l.url, target: '_blank', rel: 'noopener noreferrer', text: '↗ ' + l.title }),
        l.note ? el('span', { class: 'dim help', text: l.note }) : null))),
    el('div', { class: 'row actions' },
      el('button', { text: 'Not now', onclick: () => close() }),
      el('span', { class: 'grow' }),
      el('button', { class: 'primary', text: 'Apply', title: g.help.apply, onclick: async () => {
        const body = { name: name.value, connect: entries.map((o) => o.box.checked && !o.box.disabled) };
        for (const o of options) if (o.key !== 'connect') body[o.key] = o.box.checked && !o.box.disabled;
        const result = await attempt(() => api.post('/guide', body));
        if (!result) return;
        finished = true;
        close();
        toast(result.done.length ? result.done.join('. ') : 'All set');
        loadNow();
      } })),
  ], { className: 'guide', onclose: () => {
    open = false;
    // Closed without applying: not shown by itself again.
    if (!finished) api.post('/guide/dismiss').catch(() => {});
  } });
}

// Your address and public key as a QR code (an lxma:// link), for others
// to add you as a contact.
function showAddress(link) {
  dialog('Your address', (close) => [
    el('p', { class: 'dim', text: 'Scan it in Columba or rettui to add you as a contact: it carries your public key, so they can write to you before hearing your announce.' }),
    el('img', { class: 'qr', src: 'api/qr?text=' + encodeURIComponent(link), alt: 'QR code of your address' }),
    el('textarea', { class: 'mono paper-link', readonly: true, rows: 3, onfocus: (e) => e.target.select() }, link),
    el('div', { class: 'row actions' },
      el('button', { text: 'Copy link', onclick: () => copy(link, 'your contact link') }),
      navigator.share ? el('button', { text: 'Share', onclick: () => navigator.share({ text: link }).catch(() => {}) }) : null,
      el('span', { class: 'grow' }),
      el('button', { class: 'primary', text: 'Done', onclick: close })),
  ], { className: 'paper' });
}

// A paper message written: its QR code, to scan into the recipient's app,
// print, or pass on as a link.
function showPaper(link) {
  dialog('Paper message', (close) => [
    el('p', { class: 'dim', text: 'Only the recipient can read it. Scan it into their app (Sideband, rettui…), print it, or pass the link on any way you like.' }),
    el('img', { class: 'qr', src: 'api/qr?text=' + encodeURIComponent(link), alt: 'QR code of the paper message' }),
    el('textarea', { class: 'mono paper-link', readonly: true, rows: 3, onfocus: (e) => e.target.select() }, link),
    el('div', { class: 'row actions' },
      el('button', { text: 'Copy link', onclick: () => copy(link, 'paper message link') }),
      navigator.share ? el('button', { text: 'Share', onclick: () => navigator.share({ text: link }).catch(() => {}) }) : null,
      el('button', { text: 'Print', onclick: () => window.print() }),
      el('span', { class: 'grow' }),
      el('button', { class: 'primary', text: 'Done', onclick: close })),
  ], { className: 'paper' });
}

// Scanning with the camera needs a browser that finds QR codes itself
// (Chrome on Android and macOS) and a secure page (https, or localhost).
// Everywhere else a photo of the code does: the server finds it.
const cameraScan = 'BarcodeDetector' in window && window.isSecureContext && !!navigator.mediaDevices?.getUserMedia;

// Read in a paper message: pasted, scanned, or from a picture of its code.
function readPaper() {
  let stopCamera = null;
  let closed = false;
  const text = el('textarea', {
    class: 'mono paper-link', rows: 3, placeholder: 'lxm://… (or a contact’s lxma://…)', autocomplete: 'off', spellcheck: 'false',
    onkeydown: (e) => {
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        submit(text.value);
      }
    },
  });
  const video = el('video', { class: 'scanner hidden', muted: true, playsinline: true });
  const status = el('div', { class: 'dim status' });
  const picture = el('input', {
    type: 'file', accept: 'image/*', class: 'hidden',
    onchange: async () => {
      const file = picture.files[0];
      picture.value = '';
      if (!file) return;
      status.textContent = 'Looking for the QR code…';
      const result = await attempt(async () => api.post('/paper/scan', { image: await shrunk(file) }));
      status.textContent = '';
      if (result) opened(result);
    },
  });
  const opened = (result) => {
    close();
    toast(result.contact ? 'Contact added' : result.known ? 'You have this message already' : 'Reading the paper message…');
    app.views.messages.selected = result.key;
    switchTab('messages');
    setPane(app.views.messages, 'detail');
  };
  const submit = async (link) => {
    if (!link.trim()) return;
    const result = await attempt(() => api.post('/paper/read', { link }));
    if (result) opened(result);
  };
  const scan = async () => {
    if (stopCamera) return;
    stopCamera = () => {};
    try {
      video.classList.remove('hidden');
      status.textContent = 'Point the camera at the QR code';
      const stop = await scanCamera(video, (link) => {
        stopCamera = null;
        status.textContent = '';
        text.value = link;
        submit(link);
      });
      // Closed while the browser asked for the camera: let it go.
      if (closed) stop();
      else if (stopCamera) stopCamera = stop;
    } catch (e) {
      stopCamera = null;
      video.classList.add('hidden');
      status.textContent = '';
      toast(`Can't use the camera: ${e.message}`, true);
    }
  };
  const close = dialog('Read a paper message', () => [
    el('p', { class: 'dim', text: 'A paper message is an lxm:// link, often as a QR code. Paste the link, or scan the code. A contact’s lxma:// link (or code) adds them.' }),
    text, video, status,
    el('div', { class: 'row actions' },
      cameraScan ? el('button', { text: 'Scan', onclick: scan }) : null,
      el('button', { text: cameraScan ? 'From a picture' : 'Scan (take a picture)', onclick: () => picture.click() }),
      el('span', { class: 'grow' }),
      el('button', { class: 'primary', text: 'Read', onclick: () => submit(text.value) })),
    picture,
  ], { className: 'paper', onclose: () => {
    closed = true;
    stopCamera?.();
  } });
  setTimeout(() => text.focus(), 50);
}

// Watch the camera for a paper message's QR code; a function to stop.
async function scanCamera(video, found) {
  const detector = new BarcodeDetector({ formats: ['qr_code'] });
  const stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' } });
  let stopped = false;
  const stop = () => {
    stopped = true;
    for (const track of stream.getTracks()) track.stop();
    video.srcObject = null;
  };
  video.srcObject = stream;
  await video.play();
  const look = async () => {
    if (stopped) return;
    const codes = await detector.detect(video).catch(() => []);
    const code = codes.find((c) => c.rawValue.startsWith('lxm://') || c.rawValue.startsWith('lxma://'));
    if (code) {
      stop();
      video.classList.add('hidden');
      found(code.rawValue);
    } else {
      setTimeout(look, 250);
    }
  };
  look();
  return stop;
}

// A picture, smaller (a phone's photo is megabytes; a QR code needs far
// fewer pixels), as base64 JPEG. As it is if it can't be drawn here.
async function shrunk(file) {
  const MAX = 1600;
  try {
    const bitmap = await createImageBitmap(file);
    const scale = Math.min(1, MAX / Math.max(bitmap.width, bitmap.height));
    const canvas = el('canvas', { width: Math.round(bitmap.width * scale), height: Math.round(bitmap.height * scale) });
    const context = canvas.getContext('2d');
    // White under a transparent screenshot, as it looks.
    context.fillStyle = '#fff';
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    bitmap.close();
    return canvas.toDataURL('image/jpeg', 0.92).split(',')[1];
  } catch {
    return readFile(file);
  }
}

// ---- Emoji ------------------------------------------------------------------

// The emoji the pickers offer (fetched once; the browser keeps the list) and
// the recently used, which the TUI shares. Found as the server's `search`
// finds them: each word typed starts a word of the name or a shortcode.
const emoji = {
  groups: null, // [{ name, icon, emoji: [{ e, name, codes, words }] }]
  recent: [],
  RECENT: 30,

  load() {
    this.loading ||= Promise.all([api.get('/emoji'), api.get('/emoji/recent')]).then(([list, recent]) => {
      const split = (text) => text.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
      this.groups = list.groups.map((g) => ({
        name: g.name,
        icon: g.icon,
        emoji: g.emoji.map(([e, name, codes]) => ({ e, name, codes, words: [...split(name), ...codes, ...codes.flatMap(split)] })),
      }));
      this.all = this.groups.flatMap((g) => g.emoji);
      this.byChar = new Map(this.all.map((x) => [x.e, x]));
      this.byCode = new Map(this.all.flatMap((x) => x.codes.map((c) => [c, x])));
      this.recent = recent;
    }).catch((e) => {
      this.loading = null;
      throw e;
    });
    return this.loading;
  },

  // Best first: the query as a shortcode, then shortcodes starting with it
  // (shorter first), then the rest in Unicode's order.
  search(query) {
    const terms = query.trim().replace(/^:+|:+$/g, '').toLowerCase().split(/[\s_]+/u).filter(Boolean);
    if (!terms.length || !this.all) return [];
    const code = terms.join('_');
    const found = [];
    for (const x of this.all) {
      if (!terms.every((t) => x.words.some((w) => w.startsWith(t)))) continue;
      const lengths = x.codes.filter((c) => c.startsWith(code)).map((c) => c.length);
      const shortest = lengths.length ? Math.min(...lengths) : null;
      found.push([shortest === code.length ? [0, 0] : shortest != null ? [1, shortest] : [2, 0], x]);
    }
    return found.sort((a, b) => a[0][0] - b[0][0] || a[0][1] - b[0][1]).map(([, x]) => x);
  },

  recentList() {
    return this.recent.map((e) => this.byChar?.get(e)).filter(Boolean);
  },

  // The shortcode to show for `x` when `typed` was typed.
  codeFor(x, typed = '') {
    const t = typed.replace(/^:+|:+$/g, '').toLowerCase();
    return x.codes.find((c) => c.startsWith(t)) || x.codes[0] || null;
  },

  picked(e) {
    this.recent = [e, ...this.recent.filter((r) => r !== e)].slice(0, this.RECENT);
    api.post('/emoji/recent', { emoji: e }).then((recent) => { this.recent = recent; }).catch(() => {});
  },
};

// Put `text` in place of the input's selection (or `start`..`end`), the
// caret after it, and tell the input's listeners.
function insertText(input, text, start = input.selectionStart ?? input.value.length, end = input.selectionEnd ?? start) {
  input.setRangeText(text, start, end, 'end');
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

// The `:name` being typed before the caret (as the server's): its `:`
// starts a word, so not `12:30` or `http://`, and what follows is a
// shortcode's characters, at least `min` of them (`:)` and `:D` aren't).
function shortcodeQuery(input, min = 2) {
  const caret = input.selectionStart ?? input.value.length;
  if (input.selectionEnd !== caret) return null;
  const match = /(?:^|\s):([A-Za-z0-9_+-]*)$/u.exec(input.value.slice(0, caret));
  if (!match || match[1].length < min) return null;
  return { start: caret - match[1].length - 1, end: caret, name: match[1] };
}

// `:name` completion for an input: while one is typed, a list of the emoji
// it could be just above the input (Up and Down choose, Tab or Enter picks,
// Esc closes it until the next `:`); a finished `:name:` becomes its emoji.
// Its `key` handler goes before the input's own keys; `list` is its element.
function shortcodes(input) {
  const list = el('div', { class: 'mention-list emoji-list hidden', role: 'listbox' });
  const self = { list, matches: null, index: 0, dismissed: null };
  const hide = () => {
    self.matches = null;
    list.classList.add('hidden');
  };
  const render = () => {
    const typed = shortcodeQuery(input)?.name || '';
    list.replaceChildren(...self.matches.map((x, i) => el('div', {
      class: 'mention-item' + (i === self.index ? ' selected' : ''),
      role: 'option',
      // Keep the focus (and the caret) in the input.
      onmousedown: (e) => e.preventDefault(),
      onclick: () => pick(x),
    }, el('span', { class: 'emoji-char', text: x.e }), el('span', { class: 'at', text: ` :${emoji.codeFor(x, typed) || x.name}:` }))));
    list.classList.remove('hidden');
  };
  const pick = (x) => {
    const query = shortcodeQuery(input);
    if (!query) return hide();
    insertText(input, x.e, query.start, query.end);
    emoji.picked(x.e);
    input.focus();
    hide();
  };
  self.update = (event) => {
    const closing = event?.inputType === 'insertText' && event.data === ':';
    if (!emoji.all) {
      // The list is fetched on the first `:`, and what's typed by then
      // looked at again (a `:name:` finished meanwhile included).
      self.closed ||= closing;
      if (!self.waiting && input.value.includes(':')) {
        self.waiting = true;
        emoji.load().then(() => {
          const closed = self.closed;
          self.closed = false;
          self.update(closed ? { inputType: 'insertText', data: ':' } : undefined);
        }).catch(() => {}).finally(() => { self.waiting = false; });
      }
      return hide();
    }
    // Typed (not pasted): a finished `:name:` is its emoji.
    if (closing) {
      const caret = input.selectionStart;
      const done = /(?:^|\s):([A-Za-z0-9_+-]+):$/u.exec(input.value.slice(0, caret));
      const x = done && emoji.byCode.get(done[1].toLowerCase());
      if (x) {
        insertText(input, x.e, caret - done[1].length - 2, caret);
        emoji.picked(x.e);
        return hide();
      }
    }
    const query = shortcodeQuery(input);
    if (!query || query.start === self.dismissed) {
      if (!query) self.dismissed = null;
      return hide();
    }
    const matches = emoji.search(query.name).slice(0, 8);
    if (!matches.length) return hide();
    const same = self.matches?.map((x) => x.e).join() === matches.map((x) => x.e).join();
    self.matches = matches;
    if (!same) self.index = 0;
    render();
  };
  self.key = (e) => {
    if (!self.matches || e.isComposing) return false;
    const count = self.matches.length;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      self.index = (self.index + (e.key === 'ArrowDown' ? 1 : count - 1)) % count;
      render();
    } else if (e.key === 'Tab' || e.key === 'Enter') {
      pick(self.matches[self.index]);
    } else if (e.key === 'Escape') {
      self.dismissed = shortcodeQuery(input)?.start ?? null;
      hide();
    } else {
      return false;
    }
    e.preventDefault();
    return true;
  };
  input.addEventListener('input', (e) => self.update(e));
  // Fetched while the first words are written, so it's there for a `:`.
  input.addEventListener('focus', () => emoji.load().catch(() => {}), { once: true });
  input.addEventListener('keyup', (e) => ['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key) && self.update());
  input.addEventListener('click', () => self.update());
  input.addEventListener('blur', hide);
  return self;
}

// Ctrl-E opens the picker (not on Apple's systems, where it's the end of
// the line and Ctrl-Cmd-Space opens their own).
const APPLE = /Mac|iPhone|iPad/.test(navigator.userAgent);
function emojiKey(e) {
  return !APPLE && e.ctrlKey && !e.altKey && !e.metaKey && e.key.toLowerCase() === 'e';
}

// The button that opens the picker for `input`.
function emojiButton(input) {
  const button = el('button', { class: 'emoji-button', text: '🙂', title: APPLE ? 'Emoji' : 'Emoji (Ctrl+E)', type: 'button',
    // Keep the input's caret where it is.
    onmousedown: (e) => e.preventDefault(),
    onclick: () => emojiPicker.toggle(input, button) });
  return button;
}

// The picker: a search, a tab for the recently used and one per group, and
// every emoji (the groups one after another; a tab scrolls to its group).
// A click puts one in at the input's caret and closes it (Shift keeps it
// open). Arrows and Enter work from the search, Esc closes. Given
// `onPick` (a reaction), it hands the emoji to that instead, by `button`.
const emojiPicker = {
  node: null,

  build() {
    this.search = el('input', { type: 'search', class: 'emoji-search', placeholder: 'Search emoji',
      oninput: () => this.render(), onkeydown: (e) => this.key(e) });
    this.tabs = el('div', { class: 'emoji-tabs' });
    this.grid = el('div', { class: 'emoji-grid', onscroll: () => this.markTab() });
    this.name = el('div', { class: 'emoji-name' });
    this.node = el('div', { class: 'emoji-picker hidden', role: 'dialog', 'aria-label': 'Emoji' },
      this.search, this.tabs, this.grid, this.name);
    document.body.append(this.node);
    document.addEventListener('pointerdown', (e) => {
      if (this.input && !this.node.contains(e.target) && e.target !== this.button) this.close(false);
    });
    window.addEventListener('resize', () => this.input && this.place());
  },

  async toggle(input, button, onPick = null) {
    if (this.input === input) return this.close();
    if (!this.node) this.build();
    this.input = input;
    this.button = button;
    this.onPick = onPick;
    this.node.classList.remove('hidden');
    this.place();
    this.grid.replaceChildren(el('div', { class: 'empty', text: 'Loading…' }));
    try {
      await emoji.load();
    } catch (e) {
      this.grid.replaceChildren(el('div', { class: 'empty', text: e.message }));
      return;
    }
    if (this.input !== input) return;
    this.search.value = '';
    this.render();
    // A phone's keyboard would cover it; it has its own emoji anyway.
    if (!phone.matches) this.search.focus();
  },

  // Above the input's bar, over its button as far as the bar allows (a
  // reaction's: by its button, below it if there's no room above).
  place() {
    const bar = this.input.closest?.('.compose, .chat-input') || this.input;
    const box = bar.getBoundingClientRect();
    const button = this.button.getBoundingClientRect();
    const width = Math.min(360, innerWidth - 16);
    const left = Math.min(Math.max(button.left + button.width / 2 - width / 2, box.left + 8), box.right - width - 8);
    this.node.style.width = width + 'px';
    this.node.style.left = Math.max(8, Math.min(left, innerWidth - width - 8)) + 'px';
    if (this.onPick && box.top < 260) {
      this.node.style.bottom = '';
      this.node.style.top = (box.bottom + 6) + 'px';
      this.node.style.maxHeight = Math.max(200, Math.min(400, innerHeight - box.bottom - 16)) + 'px';
      return;
    }
    this.node.style.top = '';
    this.node.style.bottom = (innerHeight - box.top + 6) + 'px';
    this.node.style.maxHeight = Math.max(200, Math.min(400, box.top - 16)) + 'px';
  },

  close(focus = true) {
    if (!this.input) return;
    const input = this.input;
    this.input = null;
    this.onPick = null;
    this.node.classList.add('hidden');
    if (focus) input.focus();
  },

  // What's shown, as sections of [title, emoji].
  sections() {
    const query = this.search.value;
    if (query.trim()) {
      const found = emoji.search(query);
      return [[found.length ? `${found.length} found` : 'No emoji with that name', found, -1]];
    }
    const recent = emoji.recentList();
    return [
      ['Used lately', recent, 0, recent.length ? null : 'None yet: the ones you pick show here'],
      ...emoji.groups.map((g, i) => [g.name, g.emoji, i + 1]),
    ];
  },

  render() {
    const searching = !!this.search.value.trim();
    const icons = ['🕘', ...emoji.groups.map((g) => g.icon)];
    const titles = ['Used lately', ...emoji.groups.map((g) => g.name)];
    this.tabs.replaceChildren(...icons.map((icon, i) => el('button', {
      class: 'emoji-tab', type: 'button', text: icon, title: titles[i],
      onclick: () => {
        this.search.value = '';
        this.render();
        this.grid.querySelector(`[data-tab="${i}"]`)?.scrollIntoView({ block: 'start' });
        this.markTab();
      },
    })));
    this.cells = [];
    const parts = [];
    for (const [title, list, tab, note] of this.sections()) {
      parts.push(el('div', { class: 'emoji-group', text: title, dataset: tab >= 0 ? { tab } : {} }));
      if (note) parts.push(el('div', { class: 'dim emoji-note', text: note }));
      parts.push(el('div', { class: 'emoji-cells' }, ...list.map((x) => {
        const cell = el('button', { class: 'emoji-cell', type: 'button', text: x.e, title: x.name,
          onmousedown: (e) => e.preventDefault(),
          onmouseenter: () => this.show(x),
          onclick: (e) => this.pick(x, e.shiftKey) });
        this.cells.push([cell, x]);
        return cell;
      })));
    }
    this.grid.replaceChildren(...parts);
    this.grid.scrollTop = 0;
    this.chosen = 0;
    this.choose(searching ? 0 : -1);
    this.markTab();
  },

  // The tab of the group scrolled to.
  markTab() {
    const searching = !!this.search.value.trim();
    let current = 0;
    for (const header of this.grid.querySelectorAll('[data-tab]')) {
      if (header.offsetTop <= this.grid.scrollTop + 4) current = Number(header.dataset.tab);
    }
    [...this.tabs.children].forEach((tab, i) => tab.classList.toggle('active', !searching && i === current));
  },

  show(x) {
    const code = emoji.codeFor(x, this.search.value);
    this.name.replaceChildren(el('span', { class: 'emoji-char', text: x.e }), ' ', code ? `:${code}:` : '', el('span', { class: 'dim', text: `  ${x.name}` }));
  },

  // Choose the cell at `index` (-1: none) for the arrows and Enter.
  choose(index) {
    this.cells[this.chosen]?.[0].classList.remove('chosen');
    this.chosen = index;
    const cell = this.cells[index];
    if (!cell) return this.name.replaceChildren(el('span', { class: 'dim', text: 'Pick one, or type to search' }));
    cell[0].classList.add('chosen');
    cell[0].scrollIntoView({ block: 'nearest' });
    this.show(cell[1]);
  },

  key(e) {
    const cells = this.cells || [];
    // Cells in a row, as laid out.
    const columns = Math.max(1, Math.round(this.grid.querySelector('.emoji-cells')?.clientWidth / (cells[0]?.[0].offsetWidth || 1)) || 1);
    const at = Math.max(this.chosen, 0);
    const steps = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -columns, ArrowDown: columns };
    if (e.key in steps && cells.length) {
      this.choose(this.chosen < 0 ? 0 : Math.max(0, Math.min(cells.length - 1, at + steps[e.key])));
    } else if (e.key === 'Enter' && cells[this.chosen]) {
      this.pick(cells[this.chosen][1], e.shiftKey);
    } else if (e.key === 'Escape' || emojiKey(e)) {
      this.close();
    } else {
      return;
    }
    e.preventDefault();
  },

  pick(x, keepOpen) {
    const input = this.input;
    if (!input) return;
    if (this.onPick) {
      const onPick = this.onPick;
      this.close(false);
      emoji.picked(x.e);
      return onPick(x.e);
    }
    insertText(input, x.e);
    emoji.picked(x.e);
    if (!keepOpen) this.close();
  },
};

// ---- Messages ---------------------------------------------------------------

app.views.messages = {
  panes: true,
  draftStore: 'rettui.drafts.messages',
  selected: null,
  pending: [],
  // Conversations loaded so far, by key: a click draws one at once and
  // refreshes it behind.
  cache: new Map(),
  // How many of the most recent conversations are loaded ahead.
  WARM: 10,
  // Messages shown (and loaded) until "Show earlier".
  LIMIT: 100,
  // How the message being written goes: the conversation's own mode, or
  // paper for one message. `modeFor` is the conversation it was set for.
  mode: 'auto',
  modeFor: null,
  modeKept: null,

  // The message search: what's typed, and whether only in the open
  // conversation.
  query: '',
  searchHere: false,
  searchRequest: 0,

  mount(root, options = {}) {
    this.list = el('div', { class: 'scroll' });
    this.header = el('header');
    this.history = el('div', { class: 'scroll history', dataset: { stick: 'bottom' } });
    this.search = el('input', {
      type: 'search',
      class: 'pane-search',
      placeholder: 'Search messages  ( / )',
      value: this.query,
      oninput: () => {
        this.query = this.search.value;
        clearTimeout(this.searchTimer);
        this.searchTimer = setTimeout(() => this.query.trim() ? this.runSearch() : this.endSearch(), 150);
      },
      onkeydown: (e) => {
        if (e.key === 'Escape' && this.query) {
          e.preventDefault();
          this.search.value = '';
          this.query = '';
          this.endSearch();
        }
      },
    });
    this.chips = el('div', { class: 'chips' });
    this.text = el('textarea', {
      placeholder: composePlaceholder(),
      enterkeyhint: 'send',
      rows: 2,
      oninput: () => keepDrafts(this),
      onkeydown: (e) => {
        if (this.emojiList.key(e)) return;
        if (emojiKey(e)) {
          e.preventDefault();
          emojiPicker.toggle(this.text, this.emojiButton);
        }
        if (e.key === 'Escape' && this.replyTo) {
          e.preventDefault();
          this.setReply(null);
        }
        // Enter while an input method is composing confirms the text.
        if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
          e.preventDefault();
          this.send();
        }
      },
    });
    // `:name` completion, and the picker's button.
    this.emojiList = shortcodes(this.text);
    this.emojiButton = emojiButton(this.text);
    this.readAll = el('button', { class: 'hidden', text: '✓ All read', title: 'Mark every conversation read', onclick: () => this.markAllRead() });
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
    // Kept for each conversation, except paper (for one message).
    const mode = this.modeSelect = el('select', { title: 'Delivery mode (kept for this conversation, except paper)', onchange: async (e) => {
      this.mode = e.target.value;
      const key = this.selected;
      if (!key || this.mode === 'paper') return;
      await attempt(() => api.post(`/conversations/${key}/delivery`, { mode: this.mode }));
    } },
    [['auto', 'Auto'], ['direct', 'Direct'], ['propagated', 'Propagated'], ['paper', 'Paper (QR code)']].map(([m, label]) =>
      el('option', { value: m, text: label, selected: m === this.mode })));
    // What the message being written answers, if anything.
    this.replyBar = el('div', { class: 'reply-bar hidden' });
    this.compose = el('div', { class: 'compose' },
      this.emojiList.list,
      this.replyBar,
      this.chips,
      el('div', { class: 'row' }, this.text),
      el('div', { class: 'row' },
        el('button', { text: '📎 Attach', onclick: () => this.fileInput.click() }),
        this.micButton = el('button', { text: '🎤', title: 'Record a voice message (sent as Codec2, as Sideband and MeshChat play)', onclick: () => this.toggleRecording() }),
        el('button', { text: '📍', title: 'Share a location ( L )', onclick: () => this.shareLocation() }),
        this.emojiButton,
        mode,
        el('span', { class: 'grow' }),
        el('button', { class: 'primary', text: 'Send', onclick: () => this.send() })),
      this.fileInput);
    root.append(
      el('section', { class: 'panel side' },
        el('header', {}, el('span', { class: 'title grow', text: 'Conversations' }),
          el('button', { text: 'Read paper', title: 'Read in a paper message (an lxm:// link or its QR code)', onclick: () => readPaper() }),
          el('button', { text: '+ New', onclick: () => this.newConversation() }),
          el('button', { text: '🗺', title: 'Map of the locations shared with you ( M )', onclick: () => mapDialog({ key: this.selected }) }),
          this.readAll, this.search),
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
    const address = prompt('LXMF address (32 hex characters), or an lxma:// link');
    if (address) openConversation(address);
  },

  // Record a voice message, or stop and keep it to send.
  async toggleRecording() {
    if (recorder.started) {
      clearInterval(this.recordingTimer);
      const recording = await recorder.stop();
      this.micButton.textContent = '🎤';
      this.micButton.classList.remove('on');
      this.cancelRecording?.remove();
      if (recording) this.voice = recording;
      this.renderChips();
      return;
    }
    if (!recorder.available()) {
      return toast('Browsers only let a secure page use the microphone: open rettui over HTTPS (start it with --https), or at localhost');
    }
    if (this.pending.length) return toast('A voice message goes on its own, or with text: take the files off first');
    try {
      await recorder.start();
    } catch (e) {
      return toast(`Couldn't record: ${e.message || e}`);
    }
    this.micButton.classList.add('on');
    const tick = () => {
      const seconds = Math.floor(recorder.seconds());
      this.micButton.textContent = `⏹ ${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
      if (seconds >= recorder.MAX_SECONDS) this.toggleRecording();
    };
    tick();
    this.recordingTimer = setInterval(tick, 250);
    this.cancelRecording = el('button', { text: '✕', title: 'Throw the recording away', onclick: async () => {
      clearInterval(this.recordingTimer);
      await recorder.stop(false);
      this.micButton.textContent = '🎤';
      this.micButton.classList.remove('on');
      this.cancelRecording.remove();
    } });
    this.micButton.after(this.cancelRecording);
  },

  renderChips() {
    const size = app.status?.picture_size;
    const voice = this.voice ? [el('span', { class: 'chip voice-chip' },
      el('span', { text: `🎤 ${Math.round(this.voice.seconds)} s, as Codec2 ` }),
      el('audio', { controls: true, preload: 'auto', src: this.voice.url }),
      el('button', { text: '×', title: 'Throw it away', onclick: () => {
        this.voice = null;
        this.renderChips();
      } }))] : [];
    this.chips.replaceChildren(...voice, ...this.pending.map((file, i) => el('span', {
      class: 'chip',
      title: shrinks(file) ? `Sent smaller (${size}), as set in Status: Send pictures at` : '',
    },
      `${file.name} (${humanBytes(file.size)}${shrinks(file) ? ', sent smaller' : ''})`,
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
    // Icons beside every name once someone has one (letters for the rest).
    const icons = conversations.some((c) => c.icon);
    // Everything read at once, when anything is unread.
    this.readAll.classList.toggle('hidden', !conversations.some((c) => c.unread));
    // Message requests come last, under a heading of their own.
    const requests = conversations.filter((c) => c.request).length;
    const requestsMark = el('div', { class: 'list-heading', text: `Message requests (${requests})`,
      title: 'From people who aren\'t contacts: no notifications until you trust them, leave them as they are, or reply' });
    if (this.query.trim()) this.runSearch();
    else if (changed(this, 'list:' + this.selected, conversations)) this.list.replaceChildren(...(conversations.length ? conversations.flatMap((c, i) => [c.request && !conversations[i - 1]?.request ? requestsMark : null, el('div', {
      class: 'list-item' + (c.key === this.selected ? ' selected' : ''),
      dataset: { key: c.key },
      // Start loading as the button goes down; the click shows it.
      onpointerdown: () => this.load(c.key).catch(() => {}),
      onclick: () => this.select(c.key),
    },
    icons ? avatar(c.icon, c.name) : null,
    el('div', { class: 'main' },
      el('div', { class: 'name' }, c.name, c.muted ? mutedMark() : null,
        c.pinned ? el('span', { class: 'pin-mark', text: ' 📌', title: 'Pinned to the top' }) : null,
        c.unknown && !c.request ? el('span', { class: 'unknown-mark', text: ' ?', title: 'Not one of your contacts' }) : null),
      el('div', { class: 'sub', text: c.last ? `${c.last.incoming ? '' : 'You: '}${c.last.text}` : 'No messages yet' })),
    el('div', { class: 'dim', style: 'font-size:12px;text-align:right' },
      c.last ? timeLabel(c.last.timestamp) : '',
      c.unread ? el('div', {}, el('span', { class: 'badge' + (c.request ? ' quiet' : ''), text: c.unread })) : null))].filter(Boolean)) :
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

  echo(key, node) {
    const shown = this.lastKey === key;
    return echoSent(this, { key, node, parent: shown && this.history, scroller: this.history, slot: 'conversation' });
  },

  // Messages matching the search, newest first, in the list's place.
  async runSearch() {
    const request = ++this.searchRequest;
    const query = this.query;
    const params = new URLSearchParams({ q: query });
    const here = this.searchHere && this.selected;
    if (here) params.set('in', this.selected);
    const found = await api.get('/search?' + params).catch(() => null);
    if (!found || request !== this.searchRequest || query !== this.query) return;
    const terms = searchTerms(query);
    const scope = this.selected ? el('label', { class: 'search-scope' },
      el('input', { type: 'checkbox', checked: !!here, onchange: (e) => {
        this.searchHere = e.target.checked;
        this.runSearch();
      } }), ` In ${this.names?.get(this.selected) || 'this conversation'}`) : null;
    const count = found.hits.length ? `${found.full ? 'The newest ' : ''}${found.hits.length} found` : 'Nothing found';
    this.list.replaceChildren(
      el('div', { class: 'search-status dim' }, el('span', { class: 'grow', text: count }), scope),
      ...found.hits.map((hit) => el('div', { class: 'list-item' + (hit.key === this.selected ? ' selected' : ''), onclick: () => this.openHit(hit) },
        el('div', { class: 'main' },
          el('div', { class: 'name' }, hit.name),
          el('div', { class: 'sub hit' }, hit.incoming ? '' : 'You: ', ...highlighted(hit.snippet, terms))),
        el('div', { class: 'dim', style: 'font-size:12px;text-align:right', text: timeLabel(hit.timestamp) }))));
  },

  // Back to the conversations.
  endSearch() {
    ++this.searchRequest;
    for (const slot of Object.keys(this.snapshots || {})) if (slot.startsWith('list:')) delete this.snapshots[slot];
    this.update();
  },

  // Open a message found: its conversation, at it (all of it loaded if it's
  // further back than what shows).
  async openHit(hit) {
    if (hit.newer >= this.LIMIT) this.showAll = hit.key;
    await this.select(hit.key);
    this.showMessage(hit.id);
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
    return this.update();
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
    // A paper message this browser wrote: its QR code, once it's ready.
    const waiting = this.paperWaiting?.key === key && conversation.messages.find((m) => m.id === this.paperWaiting.id);
    if (waiting && waiting.state.kind !== 'sending') {
      this.paperWaiting = null;
      if (waiting.paper) showPaper(waiting.paper);
    }
    // Its way of delivery, unless paper was picked for the next message.
    const kept = conversation.contact?.delivery || 'auto';
    if (this.modeFor !== key || (this.modeKept !== kept && this.mode !== 'paper')) {
      this.mode = kept;
      this.modeSelect.value = kept;
      this.modeFor = key;
    }
    this.modeKept = kept;
    if (!changed(this, 'conversation', { key, conversation, all: this.showAll === key, archive: this.archive?.key === key && this.archive.messages.length })) return;
    this.header.replaceChildren(...[
      conversation.icon ? avatar(conversation.icon, conversation.name) : null,
      el('span', { class: 'title', text: conversation.name }),
      el('span', { class: 'dim mono grow', style: 'font-weight:400;font-size:12.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap', text: key }),
      el('button', { class: 'pin-button' + (conversation.pinned ? ' on' : ''), text: '📌',
        title: conversation.pinned ? 'Unpin it from the top of the list' : 'Pin it to the top of the list',
        onclick: () => this.setPinned(key, !conversation.pinned) }),
      conversation.live ? el('span', { class: 'live-chip', title: conversation.live.source === 'device' ? 'Sharing where this device is, while a page of rettui is open on it' : 'Sharing this station\'s Location' },
        conversation.live.until ? `📍 live until ${clockTime(new Date(conversation.live.until * 1000))}` : '📍 live',
        el('button', { class: 'inline', text: 'Stop', onclick: async () => {
          if (await attempt(() => api.post(`/conversations/${key}/live/stop`), 'Stopped sharing your location live')) loadNow();
        } })) : null,
      bellButton(conversation.muted ? 'off' : 'on', 'Notifications from this conversation', () => this.setMuted(key, !conversation.muted)),
      el('button', { text: 'Contact', title: 'Your name for them, notes, and more', onclick: () => this.contactDialog(key, conversation) }),
      el('button', { text: 'Copy address', onclick: () => copy(key, 'LXMF address') })].filter(Boolean));
    // The newest messages only, unless asked for all: those not loaded, and
    // any loaded but not shown.
    const from = this.showAll === key ? 0 : Math.max(0, conversation.messages.length - this.LIMIT);
    const hidden = from + (conversation.total ?? conversation.messages.length) - conversation.messages.length;
    // Older still are in the archive, which rettui doesn't show.
    // Older still are in the archive: shown (read only) when asked for.
    const archive = this.archive?.key === key ? this.archive.messages : null;
    const archived = archive
      ? el('div', { class: 'archive-block' },
        el('div', { class: 'archive-mark dim', text: archive.length
          ? `From the archive: ${archive.length} older ${archive.length === 1 ? 'message' : 'messages'}, only to read`
          : 'Nothing of this conversation is in the archive now' }),
        archive.map((m) => this.message(m, conversation, true)))
      : conversation.archived
        ? el('div', { class: 'show-more' }, el('button', {
          text: `Show ${conversation.archived} archived ${conversation.archived === 1 ? 'message' : 'messages'}`,
          title: `Older messages, moved to the archive (${conversation.archive})`,
          onclick: (e) => this.openArchive(key, e.currentTarget),
        }))
        : null;
    const earlier = hidden ? el('div', { class: 'show-more' }, el('button', { text: `Show ${hidden} earlier messages`, onclick: () => {
      this.showAll = key;
      this.update();
    } })) : archived;
    const echoes = echoesFor(this, key);
    // Someone not a contact: trust them, leave them as they are, or block.
    const contact = conversation.contact || {};
    const banner = conversation.messages.length && !contact.known ? el('div', { class: 'stranger' },
      el('span', { class: 'grow', text: contact.request
        ? `A message request: ${conversation.name} isn't one of your contacts. Nothing from them notifies you until you trust them, leave them as they are, or reply.`
        : `${conversation.name} isn't one of your contacts.` }),
      el('button', { text: 'Trust', title: 'No stamp asked of them, and they get tickets', onclick: () => this.setTrust(key, 'trusted', conversation) }),
      el('button', { text: 'Leave as is', onclick: () => this.setTrust(key, 'untrusted', conversation) }),
      el('button', { class: 'danger', text: 'Block', onclick: () => this.setTrust(key, 'blocked', conversation) }),
      contact.request ? el('button', { class: 'danger', text: 'Delete', title: 'Delete the conversation (they aren\'t blocked)',
        onclick: () => this.deleteConversation(key, conversation) }) : null) : null;
    const render = () => this.history.replaceChildren(...(conversation.messages.length || echoes.length
      ? [banner, earlier, ...conversation.messages.slice(from).map((m) => this.message(m, conversation))].filter(Boolean)
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

  // Delete a conversation (asked first), and `close` what asked for it.
  async deleteConversation(key, conversation, close = () => {}) {
    if (!confirm(`Delete the conversation with ${conversation.name}, its archived messages and the files it brought?`)) return;
    if (await attempt(() => api.post(`/conversations/${key}/delete`), 'Deleted the conversation')) {
      close();
      if (this.selected === key) this.selected = null;
      this.cache.delete(key);
      setPane(this, 'list');
      this.update();
    }
  },

  async setPinned(key, pinned) {
    if (await attempt(() => api.post(`/conversations/${key}/pin`, { pinned }), pinned ? 'Pinned to the top' : 'Unpinned')) this.update();
  },

  async markAllRead() {
    const done = await attempt(() => api.post('/conversations/read-all'));
    if (done) {
      toast(done.marked ? `Marked ${done.marked} ${done.marked === 1 ? 'conversation' : 'conversations'} read` : 'Nothing is unread');
      this.update();
    }
  },

  async setMuted(key, muted) {
    const done = await attempt(() => api.post(`/conversations/${key}/notify`, { muted }),
      muted ? 'No notifications from this conversation' : 'Notifications from this conversation are on');
    if (done) this.update();
  },

  // A key pressed with nothing typed into: true if it was one of the
  // conversation keys (see KEYS).
  key(e) {
    const keys = [...this.list.querySelectorAll('.list-item[data-key]')].map((n) => n.dataset.key);
    const at = keys.indexOf(this.selected);
    if (e.key === 'j' || e.key === 'k') {
      const next = keys[Math.max(0, Math.min(keys.length - 1, at + (e.key === 'j' ? 1 : -1)))];
      if (next && next !== this.selected) this.select(next);
      return true;
    }
    if (e.key === 'i' && this.selected) {
      setPane(this, 'detail');
      this.text.focus();
      return true;
    }
    if (e.key === 'r' && this.selected) {
      const replies = this.history.querySelectorAll('.message.in .reply-button');
      const newest = [...replies].reverse().find((b) => b.textContent.includes('Reply'));
      if (newest) newest.click();
      else toast('Nothing of theirs to reply to here');
      return true;
    }
    if (e.key === 'n') {
      this.newConversation();
      return true;
    }
    if (e.key === '*' && this.selected) {
      const pinned = this.header.querySelector('.pin-button')?.classList.contains('on');
      this.setPinned(this.selected, !pinned);
      return true;
    }
    if (e.key === 'L' && this.selected) {
      this.shareLocation();
      return true;
    }
    if (e.key === 'M') {
      mapDialog({ key: this.selected });
      return true;
    }
    return false;
  },

  // Share a location with the open conversation, as Sideband shares one:
  // where this device is (browsers only say on a secure page), this
  // station's (the Location setting), or one typed.
  shareLocation() {
    const key = this.selected;
    if (!key) return;
    if (this.mode === 'paper') return toast('A paper message carries text only: pick another delivery to share a location');
    const name = this.names?.get(key) || 'them';
    const station = app.status?.location;
    const typed = (p) => `${p.latitude}, ${p.longitude}`;
    // From the device: how accurate, and how high, too.
    let device = null;
    const coords = el('input', { type: 'text', inputmode: 'decimal', placeholder: 'latitude, longitude (e.g. 51.5074, -0.1278)',
      value: station ? typed(station) : '', oninput: () => { device = null; } });
    const status = el('p', { class: 'dim' });
    const here = el('button', { type: 'button', text: '📍 Where this device is', onclick: () => {
      if (!window.isSecureContext || !navigator.geolocation) {
        return toast('Browsers only tell a secure page where the device is: open rettui over HTTPS (start it with --https), or at localhost', true);
      }
      status.textContent = 'Asking the browser…';
      navigator.geolocation.getCurrentPosition((found) => {
        const c = found.coords;
        device = { latitude: Number(c.latitude.toFixed(6)), longitude: Number(c.longitude.toFixed(6)), accuracy: c.accuracy, altitude: c.altitude };
        coords.value = typed(device);
        status.textContent = `Within about ${Math.round(c.accuracy)} m`;
      }, (e) => {
        status.textContent = '';
        toast(e.code === 1 ? 'The browser wasn\'t allowed to say where this device is' : `Couldn't tell where this device is: ${e.message}`, true);
      }, { enableHighAccuracy: true, timeout: 20000, maximumAge: 60000 });
    } });
    const share = async (close) => {
      const match = coords.value.trim().replace(/^geo:/i, '').match(/^(-?\d+(?:\.\d+)?)\s*[,\s]\s*(-?\d+(?:\.\d+)?)/);
      if (!match) return toast('A location is latitude, longitude in degrees, as 51.5074, -0.1278', true);
      const location = device || { latitude: Number(match[1]), longitude: Number(match[2]) };
      const echo = this.echo(key, el('div', { class: 'message out echo' },
        el('div', { class: 'meta' },
          el('span', { class: 'author out', text: 'You' }),
          el('span', { class: 'dim', text: '  ' + timeLabel(Date.now() / 1000) }),
          el('span', { class: 'dim', text: ' sending…' })),
        el('div', { class: 'location', text: `📍 ${location.latitude}, ${location.longitude}` })));
      close();
      const sent = await attempt(() => api.post(`/conversations/${key}/send`, { content: '', mode: this.mode, location }));
      echo.done(!sent);
    };
    // Live: where this device is, as it moves (while a page is open), or
    // else this station's Location; for a while, or until stopped.
    const deviceOk = window.isSecureContext && !!navigator.geolocation;
    const liveSource = deviceOk ? 'device' : station ? 'station' : null;
    const howLong = el('select', { title: 'How long to share it live' },
      [[15, '15 minutes'], [60, '1 hour'], [480, '8 hours'], [0, 'Until I stop']].map(([m, text]) => el('option', { value: m, text })));
    const shareLive = async (close) => {
      if (liveSource === 'device') {
        const ok = await new Promise((resolve) => navigator.geolocation.getCurrentPosition(
          (found) => resolve(found), () => resolve(null), { enableHighAccuracy: true, timeout: 20000, maximumAge: 60000 }));
        if (!ok) return toast('The browser didn\'t say where this device is, so it isn\'t shared', true);
        const started = await attempt(() => api.post(`/conversations/${key}/live`, { minutes: Number(howLong.value), source: 'device' }));
        if (!started) return;
        await liveWatch.post(ok);
        toast(started.doing);
      } else {
        const started = await attempt(() => api.post(`/conversations/${key}/live`, { minutes: Number(howLong.value), source: 'station' }));
        if (!started) return;
        toast(started.doing);
      }
      close();
      loadNow();
    };
    dialog('Share a location', (close) => [
      el('p', {}, `With ${name}, as Sideband shares one: Sideband and Columba show it on their maps.`),
      el('div', { class: 'row' }, here, station ? el('button', { type: 'button', text: 'This station\'s', title: 'The Location setting', onclick: () => {
        device = null;
        coords.value = typed(station);
        status.textContent = '';
      } }) : null),
      coords,
      status,
      el('div', { class: 'row actions' }, el('span', { class: 'grow' }),
        el('button', { type: 'button', text: 'Cancel', onclick: () => close() }),
        el('button', { type: 'button', class: 'primary', text: 'Share', onclick: () => share(close) })),
      el('h4', { class: 'live-heading', text: 'Or share it live' }),
      el('p', { class: 'dim', text: liveSource === 'device'
        ? 'Where this device is, as it moves, at most once a minute, while a page of rettui is open on it; then they\'re told you stopped.'
        : liveSource === 'station' ? 'This station\'s Location, every few minutes (this page can\'t say where this device is: it isn\'t a secure page).'
          : 'Set this station\'s Location in Status, or open rettui over HTTPS (or at localhost) to share where this device is.' }),
      el('div', { class: 'row' }, howLong, el('span', { class: 'grow' }),
        el('button', { type: 'button', text: '📍 Share live', disabled: !liveSource, onclick: () => shareLive(close) })),
    ], { className: 'share-location' });
    setTimeout(() => coords.focus(), 50);
  },

  // The open conversation's archived messages, above the rest (and all
  // those kept, so nothing's missing between).
  async openArchive(key, button) {
    button.disabled = true;
    button.textContent = 'Reading the archive…';
    const read = await attempt(() => api.get(`/conversations/${key}/archive`));
    if (!read) {
      button.disabled = false;
      return;
    }
    this.archive = { key, messages: read.messages };
    this.showAll = key;
    const top = this.history.scrollHeight - this.history.scrollTop;
    await this.update();
    // Where it was: the archive goes in above.
    this.history.scrollTop = this.history.scrollHeight - top;
  },

  // A message as shown; an `archived` one is only to read (it's not kept,
  // so it can't be replied to, reacted to or deleted here).
  message(m, conversation, archived = false) {
    const state = {
      received: m.state.verified ? null : el('span', { class: 'state-warn', text: ' unverified' }),
      sending: el('span', { class: 'dim', text: this.paperWaiting?.id === m.id ? ' writing the paper message…' : ' sending…' }),
      delivered: m.paper
        ? el('span', { class: 'state-ok' }, ' ✓ paper message ',
          el('button', { class: 'inline', text: 'QR code', onclick: () => showPaper(m.paper) }))
        : el('span', { class: 'state-ok', text: ' ✓' }),
      propagated: el('span', { class: 'state-ok', text: ' ✓ via propagation node' }),
      failed: el('span', { class: 'state-bad', text: ` failed: ${m.state.error}` }),
    }[m.state.kind];
    const base = `api/conversations/${conversation.key}/attachments/${encodeURIComponent(m.id)}/`;
    const author = m.incoming ? conversation.name : 'You';
    const actions = !archived;
    const reactButton = actions && m.can_reply ? el('button', { class: 'inline reply-button', text: '🙂 React', title: 'React to this message' }) : null;
    reactButton?.addEventListener('click', () => emojiPicker.toggle(reactButton, reactButton, (e) => this.react(conversation.key, m.id, e)));
    return el('div', { class: 'message ' + (m.incoming ? 'in' : 'out') + (archived ? ' archived' : ''), dataset: { id: m.id } },
      el('div', { class: 'meta' },
        el('span', { class: 'author ' + (m.incoming ? 'in' : 'out'), text: author }),
        el('span', { class: 'dim', text: '  ' + timeLabel(m.timestamp) }),
        state,
        actions && m.can_reply ? el('button', { class: 'inline reply-button', text: '↩ Reply', title: 'Reply to this message',
          onclick: () => this.setReply({ id: m.id, author, text: this.opening(m) }, true) }) : null,
        reactButton,
        actions && m.state.kind === 'failed' ? el('button', { class: 'inline', text: '↻ Retry', title: 'Send it again',
          onclick: () => this.retry(conversation.key, m.id) }) : null,
        actions ? el('button', { class: 'inline reply-button', text: '⋯', title: 'More', onclick: (e) => this.messageMenu(m, conversation, e.currentTarget) }) : null),
      m.reply ? this.quote(m.reply, conversation) : null,
      m.title ? el('div', { class: 'title', text: m.title }) : null,
      m.html ? formattedContent(m.html) : m.content ? el('div', { class: 'content', text: m.content }) : null,
      m.location ? el('div', { class: 'location' }, '📍 ',
        el('a', { href: m.location.map, target: '_blank', rel: 'noopener noreferrer', text: m.location.label, title: 'Show it on OpenStreetMap' }),
        el('button', { class: 'inline', text: 'Map', title: 'Show it on the map, with everyone else\'s', onclick: () => mapDialog({ at: m.location, name: m.incoming ? conversation.name : 'You', key: m.incoming ? conversation.key : null }) })) : null,
      (m.notes || []).map((note) => el('div', { class: 'note dim', text: note })),
      m.attachments.map((a) => {
        const url = base + a.index;
        if (!a.exists) return el('div', { class: 'attachment dim', text: `📎 ${a.name} (file missing)` });
        if (a.voice) {
          return el('div', { class: 'attachment voice' },
            el('span', { text: '🎤 Voice message ' }),
            a.playable ? el('audio', { controls: true, preload: 'none', src: url }) : null,
            el('a', { href: url, download: a.name, class: 'dim', text: a.playable ? ` ${humanBytes(a.size)}` : ` ${a.voice}, which browsers can't play (${humanBytes(a.size)})` }));
        }
        if (a.image) {
          return el('div', { class: 'attachment' },
            el('a', { href: url, target: '_blank', rel: 'noopener' }, el('img', { src: url, alt: a.name, loading: 'lazy' })));
        }
        return el('div', { class: 'attachment' },
          el('a', { href: url, download: a.name, text: `📎 ${a.name}` }),
          el('span', { class: 'dim', text: ` ${humanBytes(a.size)}` }));
      }),
      this.reactions(m, conversation));
  },

  // A message's reactions: each emoji, with who reacted (a failed one of
  // yours goes again when clicked).
  reactions(m, conversation) {
    if (!m.reactions?.length) return null;
    const groups = new Map();
    for (const r of m.reactions) {
      const who = r.incoming ? conversation.name : { sending: 'You (sending…)', failed: 'You (failed)' }[r.state.kind] || 'You';
      groups.set(r.emoji, [...(groups.get(r.emoji) || []), { who, failed: !r.incoming && r.state.kind === 'failed' }]);
    }
    return el('div', { class: 'reactions' }, [...groups].map(([e, people]) => {
      const failed = people.some((p) => p.failed);
      return el('button', { class: 'reaction' + (failed ? ' failed' : ''), type: 'button',
        title: failed ? 'Not sent: click to send it again' : people.map((p) => p.who).join(', '),
        onclick: () => failed && this.react(conversation.key, m.id, e) },
      el('span', { text: e }), el('span', { class: 'dim', text: ' ' + people.map((p) => p.who).join(', ') }));
    }));
  },

  async react(key, id, e) {
    if (await attempt(() => api.post(`/conversations/${key}/react`, { id, emoji: e }))) this.update();
  },

  // Trust someone (`trusted`), leave them as is (`untrusted`), stop
  // trusting them (`unknown`), or block them (`blocked`, after asking).
  async setTrust(key, trust, conversation, after) {
    const name = conversation?.name || key;
    if (trust === 'blocked' && !confirm(`Block ${name}? Their messages are dropped, the conversation is deleted, and their identity is blocked in Reticulum.`)) return false;
    const said = { trusted: `Trusting ${name}`, untrusted: `Leaving ${name} as is`, unknown: `Not trusting ${name}`, blocked: `Blocked ${name}` }[trust];
    if (!await attempt(() => api.post(`/conversations/${key}/trust`, { trust }), said)) return false;
    if (trust === 'blocked' && this.selected === key) {
      this.selected = null;
      this.cache.delete(key);
      setPane(this, 'list');
    }
    after?.();
    this.update();
    return true;
  },

  async retry(key, id) {
    if (await attempt(() => api.post(`/conversations/${key}/retry`, { id, mode: this.mode }), 'Sending it again')) this.update();
  },

  // What else can be done with a message.
  messageMenu(m, conversation, anchor) {
    const items = [{ text: 'Copy text', action: () => copy(m.content || this.opening(m), 'the message') }];
    if (m.content?.trim() || m.attachments.length) items.push({ text: 'Forward…', action: () => this.forwardDialog(conversation.key, m) });
    if (m.state.kind === 'failed') items.push({ text: 'Send again', action: () => this.retry(conversation.key, m.id) });
    items.push({ text: m.attachments.length ? 'Delete (and its files)' : 'Delete', danger: true, action: async () => {
      if (!confirm(m.attachments.length ? 'Delete this message and the files it brought?' : 'Delete this message?')) return;
      if (await attempt(() => api.post(`/conversations/${conversation.key}/messages/delete`, { id: m.id }), 'Deleted the message')) this.update();
    } });
    openSheet(null, items, anchor);
  },

  // Send a message on to another conversation (or an address): its text
  // and copies of its files.
  async forwardDialog(from, m) {
    const conversations = (await attempt(() => api.get('/conversations')) || []).filter((c) => !c.request);
    const filter = el('input', { type: 'text', placeholder: 'A name, or an LXMF address', autocomplete: 'off' });
    const list = el('div', { class: 'forward-list' });
    const send = async (to, close) => {
      if (await attempt(() => api.post(`/conversations/${from}/forward`, { id: m.id, to: to.key }), `Forwarded to ${to.name}`)) {
        close();
        this.update();
      }
    };
    dialog('Forward to', (close) => {
      const render = () => {
        const words = filter.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
        const shown = conversations.filter((c) => words.every((w) => c.name.toLowerCase().includes(w) || c.key.includes(w)));
        // An address typed in full, even with no conversation yet.
        const typed = filter.value.trim().replace(/^lxmf@/, '').toLowerCase();
        if (/^[0-9a-f]{32}$/.test(typed) && !shown.some((c) => c.key === typed)) shown.unshift({ key: typed, name: `<${typed.slice(0, 12)}>`, fresh: true });
        list.replaceChildren(...(shown.length ? shown.map((c) => el('button', { class: 'list-item', onclick: () => send(c, close) },
          el('span', { class: 'name grow', text: c.name }),
          el('span', { class: 'dim mono', text: c.fresh ? 'new conversation' : c.key.slice(0, 12) })))
          : [el('div', { class: 'empty', text: 'No conversation matches: type an LXMF address (32 hex characters)' })]));
      };
      filter.addEventListener('input', render);
      filter.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') list.querySelector('button')?.click();
      });
      render();
      setTimeout(() => filter.focus(), 50);
      return [el('p', { class: 'dim', text: `${this.opening(m)}${m.attachments.length ? ` (with ${m.attachments.length} ${m.attachments.length === 1 ? 'file' : 'files'})` : ''}` }), filter, list];
    }, { className: 'forward-dialog' });
  },

  // Your name for them, notes about them, and deleting the conversation.
  contactDialog(key, conversation) {
    const contact = conversation.contact || {};
    const alias = el('input', { type: 'text', value: contact.alias || '', maxlength: 128,
      placeholder: contact.announced ? `They announce ${contact.announced}` : 'Nothing heard from them yet' });
    const notes = el('textarea', { rows: 5, maxlength: 10000, placeholder: 'Notes about them, for you alone' });
    notes.value = contact.notes || '';
    const trusted = contact.trust === 'trusted';
    const pingText = el('span', { text: contact.ping || 'not yet' });
    dialog('Contact', (close) => [
      el('p', { class: 'dim mono', style: 'overflow-wrap:anywhere', text: key }),
      el('div', { class: 'row trust' },
        el('span', { class: 'grow' }, el('span', { class: 'dim', text: 'Ping: ' }), pingText),
        el('button', { text: 'Ping', title: 'How long a Link to them takes to set up, and how far away they are', onclick: async (e) => {
          e.target.disabled = true;
          pingText.textContent = 'waiting for an answer…';
          const result = await attempt(() => api.post(`/conversations/${key}/ping`));
          pingText.textContent = result ? result.text : 'no answer';
          e.target.disabled = false;
        } })),
      el('div', { class: 'row trust' },
        el('span', { class: 'grow' }, el('span', { class: 'dim', text: 'Trust: ' }), contact.trust_label || ''),
        el('button', { text: trusted ? 'Stop trusting' : 'Trust', onclick: () => this.setTrust(key, trusted ? 'unknown' : 'trusted', conversation, close) }),
        el('button', { class: 'danger', text: 'Block', onclick: () => this.setTrust(key, 'blocked', conversation, close) })),
      el('label', { class: 'field' }, el('span', { text: 'Your name for them' }), alias),
      el('label', { class: 'field' }, el('span', { text: 'Notes' }), notes),
      el('div', { class: 'row actions' },
        el('button', { class: 'danger', text: 'Delete conversation', onclick: () => this.deleteConversation(key, conversation, close) }),
        el('button', { text: 'Export as text', title: 'Save the conversation, archived messages and all, as a text file',
          onclick: () => el('a', { href: `api/conversations/${key}/export`, download: '' }).click() }),
        el('span', { class: 'grow' }),
        el('button', { class: 'primary', text: 'Save', onclick: async () => {
          if (await attempt(() => api.post(`/conversations/${key}/contact`, { alias: alias.value, notes: notes.value }), 'Saved')) {
            close();
            this.update();
          }
        } })),
    ]);
    setTimeout(() => alias.focus(), 50);
  },

  // The start of what a message says, for a reply to it (as the server's
  // `Message::opening`).
  opening(m) {
    const first = (text) => (text || '').split('\n').map((l) => l.trim()).find(Boolean);
    const file = m.attachments[0] && (m.attachments[0].voice ? '🎤 Voice message' : `📎 ${m.attachments[0].name}`);
    return first(m.content) || first(m.title) || file || (m.location ? '📍 Location' : '') || m.notes?.[0] || '';
  },

  // What a reply answers, above its text: a click shows that message.
  quote(reply, conversation) {
    const author = reply.incoming == null ? '' : `${reply.incoming ? conversation.name : 'You'}: `;
    return el('div', { class: 'reply-quote' + (reply.id ? ' clickable' : ''), title: reply.id ? 'Show this message' : null,
      onclick: () => reply.id && this.showMessage(reply.id) },
    author ? el('span', { class: 'quote-author', text: author }) : null, reply.text);
  },

  // Scroll to a message and make it stand out for a moment.
  showMessage(id) {
    const node = [...this.history.querySelectorAll('.message')].find((n) => n.dataset.id === id);
    if (!node) return toast('That message is further back: Show earlier messages to see it');
    node.scrollIntoView({ block: 'center', behavior: 'smooth' });
    node.classList.remove('flash');
    void node.offsetWidth;
    node.classList.add('flash');
  },

  // Reply to `target` ({ id, author, text }), or not (null).
  setReply(target, focus = false) {
    this.replyTo = target;
    if (this.draftKey != null) keepDrafts(this);
    this.replyBar.classList.toggle('hidden', !target);
    this.replyBar.replaceChildren(...(target ? [
      el('span', { class: 'reply-label', text: '↩' }),
      el('span', { class: 'quote-author', text: `${target.author}:` }),
      el('span', { class: 'reply-text', text: target.text }),
      el('button', { class: 'inline', text: '×', title: 'Don\'t reply (Esc)', onclick: () => this.setReply(null, true) }),
    ] : []));
    if (focus) this.text.focus();
  },

  // What's written (and attached) in the box, and what it replies to, if
  // anything (see draftSwitch).
  takeDraft() {
    return this.text.value || this.pending.length || this.replyTo
      ? { text: this.text.value, files: this.pending, reply: this.replyTo }
      : null;
  },

  putDraft(draft) {
    this.text.value = draft?.text || '';
    this.pending = draft?.files || [];
    this.setReply(draft?.reply || null);
    this.renderChips();
  },

  joinDrafts(first, second) {
    if (!second) return first;
    return {
      text: [first.text, second.text].filter(Boolean).join('\n'),
      files: [...first.files, ...second.files],
      reply: first.reply || second.reply,
    };
  },

  async send() {
    const content = this.text.value;
    const pending = this.pending;
    const reply = this.replyTo;
    const voice = this.voice;
    if (!content.trim() && !pending.length && !voice) return;
    if (recorder.started) return toast('Stop recording first (⏹)');
    this.voice = null;
    // Take the message out of the box at once, so a second Enter (or a
    // double click) while this one is on its way has nothing to send, and
    // anything typed meanwhile is kept.
    this.text.value = '';
    this.pending = [];
    this.setReply(null);
    this.renderChips();
    keepDrafts(this, true);
    // Show it straight away as sending; the next update draws the real one.
    const echo = this.echo(this.selected, el('div', { class: 'message out echo' },
      el('div', { class: 'meta' },
        el('span', { class: 'author out', text: 'You' }),
        el('span', { class: 'dim', text: '  ' + timeLabel(Date.now() / 1000) }),
        el('span', { class: 'dim', text: ' sending…' })),
      reply ? el('div', { class: 'reply-quote' }, el('span', { class: 'quote-author', text: `${reply.author}: ` }), reply.text) : null,
      content ? el('div', { class: 'content', text: content }) : null,
      pending.map((f) => el('div', { class: 'attachment dim', text: `📎 ${f.name}` })),
      voice ? el('div', { class: 'attachment dim', text: `🎤 Voice message, ${Math.round(voice.seconds)} s` }) : null));
    const files = [];
    for (const file of pending) files.push({ name: file.name, data: await readFile(file) });
    // Pictures that go smaller don't count: rettui warns of what they come to.
    const total = pending.filter((f) => !shrinks(f)).reduce((n, f) => n + f.size, 0);
    if (total > 1_000_000) toast(`Sending ${humanBytes(total)} of attachments; many clients reject direct transfers over 1 MB`);
    const mode = this.mode;
    const sent = await attempt(() => api.post(`/conversations/${echo.key}/send`, { content, mode, files, reply_to: reply?.id, voice: voice?.wav }));
    echo.done(!sent);
    if (!sent) {
      if (voice && !this.voice) {
        this.voice = voice;
        this.renderChips();
      }
      // Put it back to try again, in the conversation it was for.
      draftRestore(this, echo.key, { text: content, files: pending, reply });
      return;
    }
    // Its QR code shows once written (see renderConversation); the next
    // message goes their usual way.
    if (mode === 'paper') {
      this.paperWaiting = { key: echo.key, id: sent.id };
      this.modeFor = null;
    }
    this.history.scrollTop = this.history.scrollHeight;
    this.history.pinned = true;
  },
};

// ---- Channels ---------------------------------------------------------------

app.views.channels = {
  panes: true,
  draftStore: 'rettui.drafts.channels',
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
        if (this.emojiList.key(e) || this.mentionKey(e)) return;
        if (emojiKey(e)) {
          e.preventDefault();
          emojiPicker.toggle(this.input, this.emojiButton);
        }
        if (e.key === 'Enter' && !e.isComposing) this.send();
      },
      oninput: () => {
        this.updateMentions();
        keepDrafts(this);
      },
      // The cursor moved: the name being typed may have changed.
      onkeyup: (e) => ['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key) && this.updateMentions(),
      onclick: () => this.updateMentions(),
      onblur: () => this.hideMentions(),
    });
    // Who `@` can mention, narrowed as the name is typed; `:name` emoji,
    // and the picker's button.
    this.mentionList = el('div', { class: 'mention-list hidden', role: 'listbox' });
    this.emojiList = shortcodes(this.input);
    this.emojiButton = emojiButton(this.input);
    this.inputBar = el('div', { class: 'chat-input' }, this.mentionList, this.emojiList.list, this.input, this.emojiButton,
      el('button', { class: 'primary', text: 'Send', onclick: () => this.send() }));
    this.members = el('div', { class: 'scroll' });
    this.membersPanel = el('section', { class: 'panel members' }, el('header', { text: 'Members' }), this.members);
    // Searching what was said, as Messages searches messages.
    this.search = el('input', {
      type: 'search',
      class: 'pane-search',
      placeholder: 'Search channels  ( / )',
      value: this.query || '',
      oninput: () => {
        this.query = this.search.value;
        clearTimeout(this.searchTimer);
        this.searchTimer = setTimeout(() => this.query.trim() ? this.runSearch() : this.endSearch(), 150);
      },
      onkeydown: (e) => {
        if (e.key === 'Escape' && this.query) {
          e.preventDefault();
          this.search.value = '';
          this.query = '';
          this.endSearch();
        }
      },
    });
    root.append(
      el('section', { class: 'panel side' },
        el('header', {}, el('span', { class: 'title grow', text: 'Channels' }),
          el('button', { text: '+ Add hub', onclick: () => this.addHub() }), this.search),
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
    if (this.query?.trim()) this.runSearch();
    else if (changed(this, 'list', { hubs, selected: this.selected })) {
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

  // Lines found by the search, newest first, in the list's place: where,
  // who, a part of it, and when.
  async runSearch() {
    const request = (this.searchRequest = (this.searchRequest || 0) + 1);
    const query = this.query;
    const params = new URLSearchParams({ q: query });
    const here = this.searchHere && this.selected;
    if (here) {
      params.set('hub', this.selected.hub);
      params.set('room', this.selected.room);
    }
    const found = await api.get('/channels/search?' + params).catch(() => null);
    if (!found || request !== this.searchRequest || query !== this.query) return;
    const terms = searchTerms(query);
    const where = (hit) => hit.whisper ? `@${hit.whisper} · ${hit.hub_name}` : hit.room ? `#${hit.room} · ${hit.hub_name}` : hit.hub_name;
    const open = this.selected && this.hubs?.find((h) => h.hash === this.selected.hub);
    const openName = open && (this.selected.room ? where({ ...this.selected, hub_name: open.name, whisper: open.whispers.find((w) => w.key === this.selected.room)?.name }) : open.name);
    const scope = open ? el('label', { class: 'search-scope' },
      el('input', { type: 'checkbox', checked: !!here, onchange: (e) => {
        this.searchHere = e.target.checked;
        this.runSearch();
      } }), ` In ${openName}`) : null;
    const count = found.hits.length ? `${found.full ? 'The newest ' : ''}${found.hits.length} found` : 'Nothing found';
    this.list.replaceChildren(
      el('div', { class: 'search-status dim' }, el('span', { class: 'grow', text: count }), scope),
      ...found.hits.map((hit) => el('div', { class: 'list-item', onclick: () => this.openHit(hit) },
        el('div', { class: 'main' },
          el('div', { class: 'name', text: where(hit) }),
          el('div', { class: 'sub hit' }, hit.nick ? `${hit.nick}: ` : '', ...highlighted(hit.snippet, terms))),
        el('div', { class: 'dim', style: 'font-size:12px;text-align:right', text: timeLabel(hit.ts / 1000) }))));
  },

  // Back to the hubs and rooms.
  endSearch() {
    this.searchRequest = (this.searchRequest || 0) + 1;
    delete (this.snapshots || {}).list;
    this.update();
  },

  // Open a line found: its room, all of it, scrolled to the line.
  openHit(hit) {
    this.showAll = hit.hub + '/' + hit.room;
    this.jumpTo = { key: hit.hub + '/' + hit.room, ts: hit.ts };
    this.select(hit.hub, hit.room);
  },

  // The line a search opened, once it's drawn: in view, flashed.
  showFound(viewKey) {
    if (!this.jumpTo || this.jumpTo.key !== viewKey) return;
    const node = this.body.querySelector(`.chat-line[data-ts="${this.jumpTo.ts}"]`);
    if (!node) return;
    this.jumpTo = null;
    this.body.pinned = false;
    node.scrollIntoView({ block: 'center' });
    node.classList.remove('flash');
    void node.offsetWidth;
    node.classList.add('flash');
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
    // A room or whisper conversation while its hub isn't connected (the
    // hub's own page says why), kept in view at the top.
    const state = hub.status.kind;
    const offline = room && state !== 'connected' ? el('div', { class: 'room-offline' },
      el('span', { class: state === 'failed' ? 'state-bad' : 'dim', text: {
        connecting: 'Connecting to the hub…',
        failed: 'The hub failed to connect (its page says why).',
        disconnected: 'Not connected to the hub.',
      }[state] }),
      state === 'connecting' ? null : el('button', { text: 'Connect', onclick: async () => {
        if (await this.hubAction(hash, 'connect')) this.update();
      } })) : null;
    const render = () => {
      const chat = this.chat(view.lines.slice(from));
      chat.append(...echoesFor(this, viewKey));
      this.body.replaceChildren(...(offline ? [offline] : []), ...(room || hidden ? [] : this.hubInfo(hub)), ...(earlier ? [earlier] : []), chat);
    };
    if (this.lastView !== viewKey) {
      render();
      this.body.scrollTop = this.body.scrollHeight;
      this.body.pinned = true;
      this.lastView = viewKey;
    } else {
      stickToBottom(this.body, render);
    }
    this.showFound(viewKey);

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
      const time = clockTime(new Date(line.ts), true);
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
      return el('div', { class: 'chat-line ' + kind, dataset: { ts: line.ts } },
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
    // Compared as the server does: any case, and without the variation
    // selectors emoji keyboards add or leave out (🆎 and 🆎\uFE0F match).
    const compared = (text) => text.replace(/[\uFE0E\uFE0F]/g, '').toLowerCase();
    // Once it has a space it's a name being finished, not a search.
    const partial = compared(query.partial);
    const search = !/\s/u.test(partial);
    const starts = people.filter((u) => compared(u.name).startsWith(partial));
    const contains = search ? people.filter((u) => !compared(u.name).startsWith(partial) && compared(u.name).includes(partial)) : [];
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
    keepDrafts(this, true);
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
  sort: 'heard',
  via: '',
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
    } }, [['all', 'All'], ['lxmf', 'LXMF peers'], ['nomad', 'NomadNet nodes'], ['propagation', 'Propagation nodes'], ['blocked', 'Blocked']]
      .map(([value, text]) => el('option', { value, text, selected: value === this.filter })));
    const sort = el('select', { title: 'Order', onchange: (e) => {
      this.sort = e.target.value;
      this.update();
    } }, [['heard', 'Last heard'], ['name', 'Name'], ['hops', 'Nearest']]
      .map(([value, text]) => el('option', { value, text, selected: value === this.sort })));
    // Kept to an interface (those with paths through them, filled in as
    // they're known).
    this.viaSelect = el('select', { title: 'Through which interface', onchange: (e) => {
      this.via = e.target.value;
      this.limit = 200;
      this.update();
    } });
    this.title = el('span', { class: 'title grow' });
    this.table = el('div', { class: 'scroll' });
    root.append(el('div', { class: 'column grow' },
      el('section', { class: 'panel' }, el('header', {}, this.search, filter, sort, this.viaSelect,
        el('button', { text: 'Announce', onclick: () => attempt(() => api.post('/announce'), 'Announcing') }),
        el('button', { text: 'Sync', onclick: () => attempt(() => api.post('/sync'), 'Syncing with the propagation node') }),
        el('button', { text: 'Find a path…', title: 'Find the path to any address', onclick: () => {
          const address = prompt('Find a path to (address)', this.selected || '');
          if (address?.trim()) pathDialog(address.trim().replace(/^<|>$/g, ''));
        } }))),
      el('section', { class: 'panel grow' }, el('header', {}, this.title), this.table)));
  },

  limit: 200,
  request: 0,

  // Only the rows shown are fetched (there can be thousands of peers);
  // rettui filters, searches and counts the rest.
  async update() {
    const request = ++this.request;
    const params = new URLSearchParams({ q: this.query, limit: this.limit, sort: this.sort });
    if (this.filter !== 'all') params.set('kind', this.filter);
    if (this.via) params.set('via', this.via);
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
    const filterName = { all: 'all', lxmf: 'LXMF peers', nomad: 'NomadNet nodes', propagation: 'propagation nodes', blocked: 'blocked' }[this.filter];
    const rows = this.data.peers;
    const total = this.data.total;
    const interfaces = [...new Set([...(this.data.interfaces || []), ...(this.via ? [this.via] : [])])];
    this.viaSelect.replaceChildren(el('option', { value: '', text: 'Any interface' }),
      ...interfaces.map((name) => el('option', { value: name, text: `Via ${name}`, selected: name === this.via })));
    this.viaSelect.classList.toggle('hidden', !interfaces.length);
    this.title.textContent = `Heard announces · ${filterName}${this.via ? ` · via ${this.via}` : ''} · ${total}${terms.length ? ' matching' : ''}`;
    if (!rows.length) {
      this.table.replaceChildren(el('div', { class: 'empty', text: terms.length
        ? `Nothing heard matches “${this.data.query.trim()}”. Esc clears the search.`
        : this.filter === 'blocked' ? 'Nobody is blocked. Block someone from their conversation\'s Contact dialog, or with Block on an LXMF peer here.'
          : !app.status?.interfaces_online ? 'Not connected to anyone yet, so nothing can be heard. Getting started, on the Status page, adds an entry point to connect through.'
            : 'Listening for announces… peers, NomadNet nodes and propagation nodes appear here as they are heard. Each announces on its own schedule, many only every few hours, so the list fills over the first hours. Announce (above) so others can find you too.' }));
      return;
    }
    const tag = { lxmf: 'PEER', nomad: 'NODE', propagation: 'PROP' };
    const outbound = this.data.propagation_node;
    // The newest rows only; more on request.
    const more = total - rows.length;
    this.table.replaceChildren(el('table', { class: 'net-table' },
      el('thead', {}, el('tr', {}, ['', 'Name', 'Address', 'Hops', 'Heard', 'Via', ''].map((h) => el('th', { text: h })))),
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
        p.hash === outbound ? el('span', { class: 'star', text: '  ★ outbound' }) : null,
        p.blocked ? el('span', { class: 'blocked-mark', text: '  ⛔ blocked' }) : null),
      el('td', { class: 'mono dim' }, highlighted(p.hash, terms)),
      el('td', { class: 'dim', text: p.last_seen ? `${p.hops} hop${p.hops === 1 ? '' : 's'}` : '' }),
      el('td', { class: 'dim', text: p.last_seen ? ago(p.last_seen) + ' ago' : 'not heard' }),
      el('td', { class: 'dim', text: p.via || '' }),
      el('td', {}, el('div', { class: 'actions' },
        p.kind === 'lxmf' && !p.blocked ? el('button', { text: 'Message', onclick: () => this.open(p) }) : null,
        p.kind === 'lxmf' ? el('button', { class: p.blocked ? '' : 'danger', text: p.blocked ? 'Unblock' : 'Block', onclick: async (e) => {
          e.stopPropagation();
          if (await app.views.messages.setTrust(p.hash, p.blocked ? 'unknown' : 'blocked', { name: p.name || p.hash })) this.update();
        } }) : null,
        p.kind === 'nomad' ? el('button', { text: 'Browse', onclick: () => this.open(p) }) : null,
        p.kind === 'propagation' ? el('button', { text: p.hash === outbound ? 'In use' : 'Use for sync', disabled: p.hash === outbound, onclick: () => this.open(p) }) : null,
        el('button', { text: 'Path', title: 'Find the path to it, or forget it', onclick: (e) => {
          e.stopPropagation();
          pathDialog(p.hash, p.name);
        } }),
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
  // What both lists are narrowed to: nodes found by rettui, saved pages
  // here, by name or address.
  query: '',
  request: 0,

  mount(root) {
    this.paneList = el('div', { class: 'scroll' });
    this.paneTabs = el('div', { class: 'subtabs' });
    this.search = el('input', {
      type: 'search',
      class: 'pane-search',
      placeholder: 'Find by name or address  ( / )',
      value: this.query,
      oninput: () => {
        this.query = this.search.value;
        this.nodeLimit = 200;
        // Saved pages narrow at once; nodes are searched by rettui, after
        // a pause in typing.
        this.renderPane();
        clearTimeout(this.typing);
        this.typing = setTimeout(() => this.update(), 120);
      },
      onkeydown: (e) => {
        if (e.key === 'Escape') {
          this.search.value = '';
          this.query = '';
          this.nodeLimit = 200;
          this.update();
        }
      },
    });
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
      find: el('button', { text: 'Find', title: 'Find in the page (f)', onclick: () => this.openFind() }),
      clear: el('button', { text: 'Clear cache', onclick: async () => {
        const result = await attempt(() => api.post('/cache/clear'));
        if (result) toast(`Cleared ${result.removed} cached page(s) and image(s)`);
      } }),
    };
    this.status = el('div', { class: 'page-status' });
    this.content = el('div', { class: 'scroll page', onclick: (e) => this.click(e) });
    // Finding in the page (f, or the Find button): every match marked.
    this.findInput = el('input', { type: 'search', placeholder: 'Find in the page', oninput: () => this.find(0),
      onkeydown: (e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          this.find(e.shiftKey ? -1 : 1);
        } else if (e.key === 'Escape') {
          e.preventDefault();
          this.closeFind();
        }
      } });
    this.findCount = el('span', { class: 'dim find-count' });
    this.findBar = el('div', { class: 'find-bar hidden' }, this.findInput, this.findCount,
      el('button', { text: '↑', title: 'Previous (Shift+Enter)', onclick: () => this.find(-1) }),
      el('button', { text: '↓', title: 'Next (Enter)', onclick: () => this.find(1) }),
      el('button', { text: '×', title: 'Done (Esc)', onclick: () => this.closeFind() }));
    root.append(
      el('section', { class: 'panel side' }, el('header', {}, this.paneTabs,
        el('button', { class: 'phone-only', text: 'Address…', onclick: () => {
          setPane(this, 'detail');
          this.address.focus();
        } }), this.search), this.paneList),
      el('section', { class: 'panel grow pane-main' },
        el('div', { class: 'toolbar' }, this.buttons.back, this.address, el('button', { class: 'primary', text: 'Go', onclick: () => this.go(this.address.value) }),
          // Their own row on a phone.
          el('span', { class: 'more-tools' }, this.buttons.reload, this.buttons.save, this.buttons.identify, this.buttons.source, this.buttons.find, this.buttons.clear)),
        this.status, this.findBar, this.content));
    this.renderPane();
    this.renderPage();
  },

  async update() {
    const request = ++this.request;
    const query = this.query;
    const params = new URLSearchParams({ kind: 'nomad', q: query, limit: this.nodeLimit });
    const [saved, peers] = await Promise.all([api.get('/saved'), api.get('/peers?' + params)]);
    // A newer search was asked for while this one loaded.
    if (request !== this.request) return;
    this.saved = saved;
    this.nodes = peers.peers;
    this.nodeTotal = peers.total;
    this.nodesHeard = peers.heard;
    // Nodes are highlighted by what found them, not what's typed since.
    this.nodesQuery = query;
    this.renderPane();
    if (this.page) {
      const url = this.page.url;
      this.page.saved = saved.some((s) => s.url === url);
      this.renderButtons();
    }
  },

  renderPane() {
    const savedTerms = searchTerms(this.query);
    const nodeTerms = searchTerms(this.nodesQuery || '');
    const lower = (text) => text.toLowerCase();
    const saved = (this.saved || []).filter((s) => savedTerms.every((t) => lower(s.name).includes(t) || lower(s.url).includes(t)));
    const count = (shown, all, searching) => searching ? `${shown}/${all}` : `${all}`;
    this.paneTabs.replaceChildren(
      el('button', { class: this.listing === 'saved' ? 'active' : '', text: `Saved ${this.saved ? count(saved.length, this.saved.length, savedTerms.length) : ''}`, onclick: () => {
        this.listing = 'saved';
        this.renderPane();
      } }),
      el('button', { class: this.listing === 'nodes' ? 'active' : '', text: `Nodes ${this.nodes ? count(this.nodeTotal, this.nodesHeard, nodeTerms.length) : ''}`, onclick: () => {
        this.listing = 'nodes';
        this.renderPane();
      } }));
    const current = this.page?.url;
    const currentNode = this.page?.node;
    const nothing = (query) => [el('div', { class: 'empty', text: `Nothing matches “${query.trim()}”. Esc clears the search.` })];
    if (this.listing === 'saved') {
      // A page found by its address shows it.
      const byAddress = (s) => savedTerms.length && !savedTerms.some((t) => lower(s.name).includes(t));
      this.paneList.replaceChildren(...(saved.length ? saved.map((s) => el('div', {
        class: 'list-item' + (s.url === current ? ' selected' : ''),
        onclick: () => this.go(s.url),
      }, el('span', { class: 'main' },
        el('div', { class: 'name' }, highlighted(s.name, savedTerms)),
        byAddress(s) ? el('div', { class: 'sub mono' }, highlighted(s.url, savedTerms)) : null),
      el('button', { text: '×', title: 'Remove', class: 'danger', onclick: (e) => {
        e.stopPropagation();
        attempt(() => api.post('/saved/remove', { url: s.url }), `Removed ${s.name}`);
      } }))) : savedTerms.length ? nothing(this.query)
        : [el('div', { class: 'empty', text: 'Nothing saved yet. Open a page and press ☆ Save.' })]));
    } else {
      const nodes = this.nodes || [];
      // A node is saved when its home page is.
      const savedUrls = new Set((this.saved || []).map((s) => s.url));
      const rows = nodes.length ? nodes.map((n) => {
        const home = `${n.hash}:/page/index.mu`;
        const saved = savedUrls.has(home);
        const name = n.name || `<${n.hash.slice(0, 12)}>`;
        return el('div', {
          class: 'list-item' + (n.hash === currentNode ? ' selected' : ''),
          onclick: () => this.go(n.hash),
        }, el('span', { class: 'main' },
          el('div', { class: 'name' }, n.name ? highlighted(n.name, nodeTerms) : name),
          el('div', { class: 'sub' }, highlighted(n.hash, nodeTerms), `  ·  ${ago(n.last_seen)} ago`)),
        el('button', { class: 'save-node' + (saved ? ' on' : ''), text: saved ? '★' : '☆', title: saved ? 'Saved; remove it' : 'Save its home page', onclick: (e) => {
          e.stopPropagation();
          if (saved) attempt(() => api.post('/saved/remove', { url: home }), `Removed ${name}`);
          else attempt(() => api.post('/saved', { url: home }), `Saved ${name}`);
        } }));
      }) :
        nodeTerms.length ? nothing(this.nodesQuery) : [el('div', { class: 'empty', text: 'No NomadNet nodes heard yet.' })];
      // The newest only; more on request.
      const more = (this.nodeTotal ?? nodes.length) - nodes.length;
      if (more > 0) {
        rows.push(el('div', { class: 'show-more' },
          el('span', { class: 'dim', text: `The ${nodes.length} most recently heard of ${this.nodeTotal}${nodeTerms.length ? ' matching' : ''}. Search (above) to find others, or ` }),
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
    this.fillPartials();
    if (!this.findBar.classList.contains('hidden')) this.find(0);
  },

  openFind() {
    this.findBar.classList.remove('hidden');
    this.findInput.focus();
    this.findInput.select();
    if (this.findInput.value) this.find(0);
  },

  closeFind() {
    this.clearFind();
    this.findBar.classList.add('hidden');
  },

  clearFind() {
    for (const hit of this.content.querySelectorAll('mark.find-hit')) hit.replaceWith(...hit.childNodes);
    this.content.normalize();
    this.findHits = [];
    this.findCount.textContent = '';
  },

  // Mark what's looked for in the page's text, and show a match: the
  // first in view on (step 0), or the next or previous, round the page.
  find(step) {
    const query = this.findInput.value.toLowerCase();
    const previous = this.findAt || 0;
    this.clearFind();
    if (!query) return;
    const walker = document.createTreeWalker(this.content, NodeFilter.SHOW_TEXT);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    for (const node of nodes) {
      const text = node.nodeValue;
      const lower = text.toLowerCase();
      let at = lower.indexOf(query);
      if (at < 0) continue;
      const pieces = document.createDocumentFragment();
      let from = 0;
      while (at >= 0) {
        pieces.append(text.slice(from, at));
        const hit = el('mark', { class: 'find-hit', text: text.slice(at, at + query.length) });
        pieces.append(hit);
        this.findHits.push(hit);
        from = at + query.length;
        at = lower.indexOf(query, from);
      }
      pieces.append(text.slice(from));
      node.replaceWith(pieces);
    }
    const hits = this.findHits;
    if (!hits.length) {
      this.findCount.textContent = 'not found';
      return;
    }
    const top = this.content.getBoundingClientRect().top;
    this.findAt = step === 0
      ? Math.max(0, hits.findIndex((hit) => hit.getBoundingClientRect().bottom >= top))
      : (previous + step + hits.length) % hits.length;
    const current = hits[this.findAt];
    current.classList.add('current');
    current.scrollIntoView({ block: 'center' });
    this.findCount.textContent = `${this.findAt + 1}/${hits.length}`;
  },

  // The shown page's partials: their HTML once loaded (by index), which
  // are loading, and their reload timers.
  partials: { page: null, html: [], loading: [], timers: [] },

  // A new page: its partials start afresh.
  resetPartials() {
    for (const timer of this.partials.timers) clearInterval(timer);
    this.partials = { page: this.page, html: [], loading: [], timers: [] };
  },

  // Put the partials loaded in their places, load the others, and reload
  // those that ask to be every so often (while the page is on screen).
  fillPartials() {
    if (this.viewSource || !this.page) return;
    if (this.partials.page !== this.page) this.resetPartials();
    [...this.content.querySelectorAll('.m-partial')].forEach((node, i) => {
      node.dataset.index = i;
      if (this.partials.html[i] != null) node.innerHTML = this.partials.html[i];
      else if (!this.partials.loading[i]) this.loadPartial(i);
      const every = Number(node.dataset.refresh);
      if (every > 0 && !this.partials.timers[i]) {
        this.partials.timers[i] = setInterval(() => showing('browser') && !this.viewSource && this.loadPartial(i), every * 1000);
      }
    });
  },

  async loadPartial(i) {
    const page = this.page;
    const node = this.content.querySelector(`.m-partial[data-index="${i}"]`);
    if (!node || this.partials.page !== page || this.partials.loading[i]) return;
    this.partials.loading[i] = true;
    const fields = this.formFields(node.dataset.fields ? node.dataset.fields.split('|') : []);
    let html;
    try {
      html = (await api.post('/partial', { url: node.dataset.url, fields })).html;
    } catch (e) {
      html = el('span', { class: 'error', text: `Could not load this part of the page: ${e.message}` }).outerHTML;
    }
    if (this.partials.page !== page) return;
    this.partials.loading[i] = false;
    this.partials.html[i] = html;
    const target = this.content.querySelector(`.m-partial[data-index="${i}"]`);
    if (target && !this.viewSource) target.innerHTML = html;
  },

  // Scroll to an anchor (`name`), or with none, to the first heading after
  // `from`; sections folded around it open.
  jumpTo(name, from = null) {
    let target = null;
    if (name) {
      target = [...this.content.querySelectorAll('.m-anchor')].find((a) => a.dataset.anchor === name);
    } else if (from) {
      target = [...this.content.querySelectorAll('.m-heading')]
        .find((h) => from.compareDocumentPosition(h) & Node.DOCUMENT_POSITION_FOLLOWING);
    }
    if (!target) return toast(name ? `There's no anchor #${name} on this page` : 'There\'s no heading after this link', true);
    for (let fold = target.closest('.m-fold'); fold; fold = fold.parentElement.closest('.m-fold')) {
      if (fold.classList.contains('hidden')) this.toggleFold(fold.previousElementSibling);
    }
    (target.nextElementSibling || target).scrollIntoView({ block: 'start' });
  },

  // Fold or open a collapsible heading's section.
  toggleFold(head) {
    const fold = head?.nextElementSibling;
    if (!fold || !fold.classList.contains('m-fold')) return;
    const open = fold.classList.toggle('hidden') === false;
    const mark = head.querySelector('.m-fold-mark');
    if (mark) mark.textContent = (open ? head.dataset.open : head.dataset.closed) + ' ';
  },

  // The page's form values (and variables) a link or partial asks for:
  // `spec` is names, `*` for all, or `var=value`.
  formFields(spec) {
    const fields = {};
    const all = spec.includes('*');
    for (const input of this.content.querySelectorAll('input[name], textarea[name]')) {
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
    return fields;
  },

  // Open a NomadNet address. `fields` submits a form (never cached).
  async go(url, { refresh = false, record = true, fields = null } = {}) {
    url = url.trim();
    if (!url) return;
    if (url.startsWith('lxmf@') || url.startsWith('lxmf://')) return openConversation(url);
    if (url.startsWith('rrc://') || url.startsWith('rrc@')) return openHubLink(url);
    if (/:\/file\//.test(url)) {
      window.location.href = 'api/download?url=' + encodeURIComponent(url);
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
    // A link's `anchor=name`: where on the page to show.
    if (fields?.var_anchor && !this.error) this.jumpTo(fields.var_anchor);
  },

  back() {
    const previous = this.history.pop();
    if (previous) this.go(previous, { record: false });
  },

  // Links carry their target in data-url and the fields they submit in
  // data-fields (names, `*` for all, or `var=value`).
  // A collapsible heading folds or opens its section.
  click(e) {
    const link = e.target.closest('.m-link');
    if (!link) {
      const head = e.target.closest('.m-fold-head');
      if (head && !e.target.closest('input, textarea, label')) this.toggleFold(head);
      return;
    }
    e.preventDefault();
    const url = link.dataset.url;
    // An anchor on this page, or partials to reload (`p:id`).
    if (url.startsWith('#')) return this.jumpTo(url.slice(1), link);
    if (url.startsWith('p:')) {
      const ids = url.slice(2).split(/[|,]/).map((id) => id.trim()).filter(Boolean);
      const nodes = [...this.content.querySelectorAll('.m-partial')].filter((p) => ids.includes(p.dataset.pid));
      if (!nodes.length) return toast(`No part of this page has the id ${ids.join(', ')}`, true);
      for (const node of nodes) this.loadPartial(Number(node.dataset.index));
      return;
    }
    const spec = link.dataset.fields ? link.dataset.fields.split('|') : [];
    if (!spec.length) return this.go(url);
    this.go(url, { fields: this.formFields(spec) });
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
    // Scrolls on its own when taller than its share, so Settings keeps room.
    this.info = el('div', { class: 'info scroll' });
    this.form = el('div', { class: 'settings' });
    this.saveButton = el('button', { class: 'primary', text: 'Save', disabled: true, onclick: () => this.save() });
    this.revertButton = el('button', { text: 'Revert', disabled: true, onclick: () => this.loadSettings(true) });
    this.settingsFooter = el('div', { class: 'settings-footer dim' });
    this.interfaces = el('div', { class: 'scroll' });
    this.log = el('div', { class: 'scroll log' });
    root.append(
      el('div', { class: 'column grow' },
        el('section', { class: 'panel', style: 'max-height:55%' }, el('header', {}, el('span', { class: 'title grow', text: 'Identity' }),
          el('button', { text: 'Announce', onclick: () => attempt(() => api.post('/announce'), 'Announcing') }),
          el('button', { text: 'Sync now', onclick: () => attempt(() => api.post('/sync'), 'Syncing with the propagation node') }),
          el('button', { class: 'more', text: 'Getting started', title: 'Connect to others, and where to learn more', onclick: () => gettingStarted() }),
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
      } else if (field.kind === 'choice') {
        input = el('select', {}, ...field.choices.map((choice) => el('option', { value: choice, text: choice, selected: choice === field.value })));
      } else if (field.kind === 'color') {
        input = el('input', { type: 'color', value: field.value });
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
        el('div', {}, input, field.key === 'icon' ? this.iconPicker(input) : null, el('div', { class: 'help', text: field.help }),
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

  // Icons to pick from, by what's typed in the icon's box (shown once it's
  // used: the icons' font is a megabyte), with the one picked in its
  // colours.
  iconPicker(input) {
    const picks = el('div', { class: 'icon-picks hidden' });
    let timer = null;
    const show = async () => {
      const name = input.value.trim().toLowerCase();
      const found = await api.get('/icons?q=' + encodeURIComponent(name)).catch(() => []);
      const colours = () => `color:${this.inputs.icon_color?.value};background:${this.inputs.icon_background?.value}`;
      picks.replaceChildren(...found.map((icon) => el('button', {
        type: 'button',
        class: 'icon-pick' + (icon.name === name ? ' selected' : ''),
        style: icon.name === name ? colours() : null,
        title: icon.name,
        text: icon.glyph,
        onclick: () => {
          input.value = icon.name;
          this.markDirty();
          show();
        },
      })), found.length ? '' : el('span', { class: 'dim', text: 'No icon has that in its name' }));
      picks.classList.remove('hidden');
    };
    input.addEventListener('focus', () => picks.classList.contains('hidden') && show());
    input.addEventListener('input', () => {
      clearTimeout(timer);
      timer = setTimeout(show, 200);
    });
    // The colours, as they're picked.
    for (const key of ['icon_color', 'icon_background']) {
      setTimeout(() => this.inputs[key]?.addEventListener('input', () => picks.classList.contains('hidden') || show()));
    }
    return picks;
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
    const h = s.hosting;
    const hosting = h.state === 'running'
      ? el('span', {}, el('span', { class: 'mono', style: 'color:var(--accent)', text: h.hash }),
        h.stats ? el('span', { class: 'dim', text: '  ' + [
          `${h.stats.messages} kept (${humanBytes(h.stats.bytes)})`, `${h.stats.received} in`, `${h.stats.served} collected`,
          h.stats.delivered_here ? `${h.stats.delivered_here} for you` : null, h.stats.rejected ? `${h.stats.rejected} refused` : null,
          `${h.stats.peers} peer${h.stats.peers === 1 ? '' : 's'}`,
        ].filter(Boolean).join(' · ') }) : null)
      : el('span', { class: h.state === 'off' ? 'dim' : '', text: {
        off: 'no (Host a propagation node, in the settings)', starting: 'starting', failed: `failed: ${h.error}`,
      }[h.state] });
    const label = (text) => el('span', { class: 'label', text });
    this.info.replaceChildren(
      label('Display name'), el('span', { style: 'font-weight:600', text: s.display_name }),
      label('LXMF address'), el('div', { class: 'row' }, el('span', { class: 'mono', style: 'color:var(--accent)', text: s.lxmf_address || '(starting)' }),
        s.lxmf_address ? el('button', { text: 'Copy', onclick: () => copy(s.lxmf_address, 'your LXMF address') }) : null,
        s.identity_link ? el('button', { text: 'QR code', title: 'Your address as a QR code, to be added as a contact', onclick: () => showAddress(s.identity_link) }) : null),
      // Until they're all taken: near the top, for those just starting.
      ...(s.first_steps.length ? [label('First steps'), el('div', { class: 'first-steps' }, ...s.first_steps.map((step) =>
        el('div', { class: step.done ? 'done' : '' },
          el('span', { class: step.done ? 'online' : 'dim', text: step.done ? '✓ ' : '○ ' }), step.label,
          step.done ? null : el('span', { class: 'dim', text: ` · ${step.how}` }),
          !step.done && step.label === 'Back up your identity'
            ? el('button', { text: "I've done it", title: 'You saved a copy of the identity file somewhere safe', onclick: async () => {
              if (await attempt(() => api.post('/identity/backed-up'))) refresh();
            } }) : null)),
        el('div', {}, el('button', { class: 'more', text: 'Hide', title: 'Stop showing the first steps', onclick: async () => {
          if (await attempt(() => api.post('/first-steps/hide'))) refresh();
        } })))] : []),
      label('Network'), el('span', { text: net }),
      label('Propagation node'), el('span', { text: s.propagation_node
        ? `${s.propagation_node.name}  ${s.propagation_node.hash}${s.auto_propagation ? ' · ' + s.auto_propagation : ''}`
        : s.auto_propagation || 'none (pick one in the Network tab)' }),
      label('Last sync'), el('span', { text: sync }),
      label('Hosting messages'), hosting,
      label('RNS config'), el('span', { class: 'mono', text: s.rns_config || 'rsReticulum default' }),
      label('Data'), el('span', { class: 'mono', text: s.data_dir || '' }),
      label('Backup'), el('div', {},
        el('button', { text: 'Download a backup', title: 'Settings, contacts and messages, in one file to restore with rettui restore',
          onclick: () => el('a', { href: 'api/backup', download: '' }).click() }),
        el('div', { class: 'dim', style: 'font-size:12.5px;margin-top:4px;max-width:34em', text:
          'Settings, contacts and messages, without your identity: back that up in the terminal UI (B in Status), or with rettui backup.' })),
      label('Known'), el('span', { text: `${s.known} destinations` }),
      ...(s.update ? [label('Update'), s.update.installed
        ? el('strong', { class: 'online', text: `rettui ${s.update.version} installed: start rettui again to use it` })
        : el('div', { class: 'row' },
          el('strong', { class: 'update-note', text: `rettui ${s.update.version} is out` }),
          el('a', { href: s.update.url, target: '_blank', rel: 'noopener noreferrer', text: 'what\'s new ↗' }),
          s.update.installing ? el('span', { class: 'dim', text: 'installing…' })
            : s.update.installable ? el('button', { text: 'Install', title: 'Download it from GitHub, check it, and put it in place of this one', onclick: async () => {
              if (!confirm(`Install rettui ${s.update.version} in place of this one? rettui carries on as it is until it's started again.`)) return;
              const done = await attempt(() => api.post('/update/install'));
              if (done) toast(done.doing);
              loadNow();
            } })
              : el('span', { class: 'dim', text: s.update.how }))] : []),
      label('Notifications'), notifications.describe(),
      label('Theme'), el('select', { title: 'This browser\'s colours', onchange: (e) => theme.set(e.target.value) },
        [['dark', 'Dark'], ['light', 'Light'], ['auto', 'As the device is set']]
          .map(([value, text]) => el('option', { value, text, selected: value === theme.chosen() }))),
      // A link shared too widely, or a lost phone: everyone else out.
      label('Browsers'), el('div', { class: 'row' },
        el('span', { class: 'dim', text: 'This one stays signed in' }),
        el('button', { text: 'Sign out the others', title: 'A new login link (printed where rettui runs): every other browser, and scripts with the old token, must log in again; push notifications stop until browsers turn them on again',
          onclick: async () => {
            if (!confirm('Sign every other browser out? They (and scripts using the old token) need the new link, printed where rettui runs, to log in again.')) return;
            if (await attempt(() => api.post('/sign-out-others'), 'Every other browser was signed out')) notifications.checkPush().catch(() => {});
          } })),
      // With --https and rettui's own certificate.
      ...(s.own_certificate ? [label('Certificate'), el('div', {},
        el('div', { class: 'row' },
          el('span', { class: 'dim', text: 'rettui\'s own, for HTTPS' }),
          el('button', { text: 'Download', onclick: () => el('a', { href: 'rettui-ca.crt', download: 'rettui-ca.crt' }).click() })),
        el('div', { class: 'dim', style: 'font-size:12.5px;margin-top:4px;max-width:34em', text:
          'Browsers warn about rettui\'s pages until the device trusts the certificate authority that signs them. '
          + 'Download it on each device and install it: on Android, Settings → Security → Encryption & credentials → '
          + 'Install a certificate → CA certificate; on an iPhone, open the download, install the profile in Settings, '
          + 'then turn it on under General → About → Certificate Trust Settings; on a computer, in the browser\'s or '
          + 'system\'s certificate settings. Then reopen rettui.' }))] : []));

    const noInterfaces = s.net.state !== 'online' ? 'Reticulum is starting…'
      : s.external_shared_instance ? 'None here: the program running the shared instance (such as rnsd) has them.'
        : 'None: add one in the Reticulum section to reach other peers.';
    this.interfaces.replaceChildren(...(s.interfaces.length ? s.interfaces.map((i) => el('div', { class: 'iface' },
      el('span', { class: i.online ? 'online' : 'offline', text: i.online ? '● ' : '○ ' }), i.name,
      el('span', { class: 'dim', text: `  ↓${humanBytes(i.rx)} ↑${humanBytes(i.tx)}` }),
      i.details?.length ? el('div', { class: 'iface-details dim', text: i.details.join(' · ') }) : null))
      : [el('div', { class: 'empty', text: noInterfaces })]));
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
          el('button', { text: 'Restart', title: 'Restart Reticulum, to apply saved changes now', onclick: () => restartReticulum() })), this.fileCard),
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

// The keys there are (the ? key shows them), as the terminal UI's popup.
const KEYS = [
  ['Anywhere', [['1–7', 'switch sections'], ['?', 'this list'], ['Esc', 'leave a text box, or close what\'s open']]],
  ['Messages', [['j / k', 'next or previous conversation'], ['i', 'write (the message box)'], ['r', 'reply to their newest message'],
    ['n', 'new conversation'], ['*', 'pin it, or unpin it'], ['/', 'search messages'], ['L', 'share a location'], ['M', 'map of locations shared'],
    ['Ctrl+E', 'emoji, in the message box'],
    ['Enter / Shift+Enter', 'send, or a new line, in the message box']]],
  ['Channels', [['/', 'search what was said in every room']]],
  ['Network', [['/', 'search by name or address']]],
  ['Browser', [['f', 'find in the page (Enter next, Shift+Enter previous)'], ['b', 'back'], ['g', 'go to an address'],
    ['/', 'search the nodes and saved pages']]],
];

function keysDialog() {
  dialog('Keys', () => KEYS.map(([where, keys]) => el('div', { class: 'keys-group' },
    el('h3', { text: where }),
    el('dl', {}, keys.flatMap(([key, what]) => [el('dt', {}, el('kbd', { text: key })), el('dd', { text: what })])))),
  { className: 'keys-dialog' });
}

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && document.body.classList.contains('drawer-open')) return setDrawer(false);
  const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName);
  // As in the terminal UI, Esc stops writing (the shortcuts work again),
  // unless it closed something first (the list @ opens).
  if (typing && e.key === 'Escape' && !e.defaultPrevented) return document.activeElement.blur();
  if (typing || e.ctrlKey || e.metaKey || e.altKey) return;
  if (e.key >= '1' && e.key <= String(TABS.length)) switchTab(TABS[Number(e.key) - 1].id);
  else if (e.key === '?') keysDialog();
  else if (app.tab === 'messages' && app.views.messages.key(e)) e.preventDefault();
  else if (e.key === 'f' && app.tab === 'browser' && app.views.browser.page) {
    e.preventDefault();
    setPane(app.views.browser, 'detail');
    app.views.browser.openFind();
  } else if (e.key === 'b' && app.tab === 'browser') app.views.browser.back();
  else if (e.key === 'g' && app.tab === 'browser') {
    e.preventDefault();
    setPane(app.views.browser, 'detail');
    app.views.browser.address.focus();
    app.views.browser.address.select();
  } else if (e.key === '/' && app.tab === 'network') {
    e.preventDefault();
    app.views.network.search.focus();
  } else if (e.key === '/' && app.tab === 'messages') {
    e.preventDefault();
    setPane(app.views.messages, 'list');
    app.views.messages.search.focus();
  } else if (e.key === '/' && app.tab === 'channels') {
    e.preventDefault();
    setPane(app.views.channels, 'list');
    app.views.channels.search.focus();
  } else if (e.key === '/' && app.tab === 'browser') {
    e.preventDefault();
    setPane(app.views.browser, 'list');
    app.views.browser.search.focus();
  }
});

window.addEventListener('hashchange', () => {
  const id = location.hash.slice(1);
  if (id !== app.tab && app.views[id]) switchTab(id);
});

// A notification tapped: open what it's about. Listened for from the
// start, since a page opened by the tap is told as soon as it has loaded,
// before it's ready (kept until it is).
let openOnStart = (() => {
  // From a page a tap opened: what to open is in its address.
  const params = new URLSearchParams(location.search);
  if (!params.has('open')) return null;
  history.replaceState(history.state, '', location.pathname + location.hash);
  try {
    const target = JSON.parse(params.get('open'));
    return ['conversation', 'room', 'summary'].includes(target?.kind) ? target : null;
  } catch {
    return null;
  }
})();
let started = false;
navigator.serviceWorker?.addEventListener('message', (e) => {
  if (!e.data || !('open' in e.data)) return;
  if (started) openTarget(e.data.open);
  else openOnStart = e.data.open;
});

refreshStatus().then(() => {
  const initial = location.hash.slice(1);
  switchTab(app.views[initial] ? initial : 'messages');
  started = true;
  if (openOnStart) openTarget(openOnStart);
  listen();
  notifications.start();
  setTimeout(prefetch, 300);
});
