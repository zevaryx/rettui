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
  if (!view.loading.has(key)) {
    const request = api.get(url)
      .then((data) => {
        view.cache.set(key, data);
        return data;
      })
      .finally(() => view.loading.delete(key));
    view.loading.set(key, request);
  }
  return view.loading.get(key);
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
  tabs.replaceChildren(...TABS.map((tab) => {
    let badge = null;
    if (tab.id === 'messages' && unread.messages) badge = el('span', { class: 'badge', text: unread.messages });
    if (tab.id === 'channels' && unread.channels) {
      badge = el('span', { class: 'badge' + (unread.mention ? ' mention' : ''), text: unread.channels });
    }
    return el('div', {
      class: 'tab' + (tab.id === app.tab ? ' active' : ''),
      title: tab.title,
      onclick: () => switchTab(tab.id),
    }, el('span', { class: 'icon', text: tab.icon }), el('span', { class: 'label', text: tab.title }), badge);
  }));
  if (app.status) {
    $('#who-name').textContent = app.status.display_name;
    const total = app.status.interfaces.length;
    const net = app.status.net.state === 'online'
      ? `● ${app.status.interfaces_online}/${total} interfaces`
      : app.status.net.state === 'failed' ? '● network failed' : '◌ starting…';
    $('#who-net').textContent = net;
    $('#who-net').className = app.status.net.state === 'online' && app.status.interfaces_online ? 'state-ok' : 'dim';
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
  view.root = el('div', { class: 'view' });
  view.mount(view.root, options);
  view.root.querySelectorAll('[data-stick="bottom"]').forEach(followBottom);
  return true;
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
function loadNow() {
  clearTimeout(refreshTimer);
  refreshTimer = null;
  refreshStatus();
  app.views[app.tab]?.update();
}

// Live updates: the first change schedules one refetch and later ones join
// it, so a steady stream of events can't keep postponing it.
let refreshTimer = null;
function refresh() {
  if (refreshTimer) return;
  refreshTimer = setTimeout(loadNow, 150);
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

function listen() {
  const events = new EventSource('/api/events');
  events.onmessage = refresh;
  events.onerror = () => {
    // The browser reconnects by itself; refresh once it is back.
    events.onopen = () => {
      events.onopen = null;
      refresh();
    };
  };
}

// Cross-tab links (from pages and the network list).
function openConversation(address) {
  attempt(async () => {
    const { key } = await api.post('/conversations', { address });
    app.views.messages.selected = key;
    switchTab('messages', { focus: true });
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
  selected: null,
  pending: [],
  // Conversations loaded so far, by key: a click draws one at once and
  // refreshes it behind.
  cache: new Map(),
  // How many of the most recent conversations are loaded ahead.
  WARM: 10,
  mode: localStorage.getItem('rettui.mode') || 'auto',

  mount(root, options = {}) {
    this.list = el('div', { class: 'scroll' });
    this.header = el('header');
    this.history = el('div', { class: 'scroll history', dataset: { stick: 'bottom' } });
    this.chips = el('div', { class: 'chips' });
    this.text = el('textarea', {
      placeholder: 'Write a message… (Enter sends, Shift+Enter for a new line)',
      rows: 2,
      onkeydown: (e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
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
      el('section', { class: 'panel grow' }, this.header, this.history, this.compose));
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
    // The list and the open conversation, fetched together when known.
    const known = this.selected;
    const [conversations, early] = await Promise.all([
      api.get('/conversations'),
      known ? this.load(known).catch(() => null) : null,
    ]);
    if (!this.selected && conversations.length) this.selected = conversations[0].key;
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
      el('div', { class: 'name', text: c.name }),
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
    const conversation = key === known && early ? early : await this.load(key);
    if (key !== this.selected) return;
    if (conversation.unread && app.tab === 'messages') api.post(`/conversations/${key}/read`).catch(() => {});
    this.renderConversation(key, conversation);
  },

  load(key) {
    return loadInto(this, key, '/conversations/' + key);
  },

  // Open a conversation: at once from what is loaded (or its name while it
  // loads), then fresh.
  select(key) {
    this.selected = key;
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
      el('button', { text: 'Copy address', onclick: () => copy(key, 'LXMF address') }));
    // The newest messages only, unless asked for all.
    const LIMIT = 100;
    const hidden = this.showAll === key ? 0 : Math.max(0, conversation.messages.length - LIMIT);
    const earlier = hidden ? el('div', { class: 'show-more' }, el('button', { text: `Show ${hidden} earlier messages`, onclick: () => {
      this.showAll = key;
      this.update();
    } })) : null;
    const render = () => this.history.replaceChildren(...(conversation.messages.length
      ? [earlier, ...conversation.messages.slice(hidden).map((m) => this.message(m, conversation))].filter(Boolean)
      : [el('div', { class: 'empty', text: 'No messages yet. Say hello!' })]));
    if (this.lastKey !== key) {
      render();
      this.history.scrollTop = this.history.scrollHeight;
      this.history.pinned = true;
      this.lastKey = key;
    } else {
      stickToBottom(this.history, render);
    }
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
    return el('div', { class: 'message' },
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

  async send() {
    const content = this.text.value;
    if (!content.trim() && !this.pending.length) return;
    const files = [];
    for (const file of this.pending) files.push({ name: file.name, data: await readFile(file) });
    const total = this.pending.reduce((n, f) => n + f.size, 0);
    if (total > 1_000_000) toast(`Sending ${humanBytes(total)} of attachments; many clients reject direct transfers over 1 MB`);
    const sent = await attempt(() => api.post(`/conversations/${this.selected}/send`, { content, mode: this.mode, files }));
    if (sent) {
      this.text.value = '';
      this.pending = [];
      this.renderChips();
      this.history.scrollTop = this.history.scrollHeight;
      this.history.pinned = true;
    }
  },
};

// ---- Channels ---------------------------------------------------------------

app.views.channels = {
  // Rooms, whispers and hub pages loaded so far, by "hub/room": a click
  // draws one at once and refreshes it behind.
  cache: new Map(),
  selected: null, // { hub, room } — room '' is the hub itself

  mount(root) {
    this.list = el('div', { class: 'scroll' });
    this.header = el('header');
    this.body = el('div', { class: 'scroll', dataset: { stick: 'bottom' } });
    this.input = el('input', {
      type: 'text',
      onkeydown: (e) => {
        if (e.key === 'Enter') this.send();
      },
    });
    this.inputBar = el('div', { class: 'chat-input' }, this.input,
      el('button', { class: 'primary', text: 'Send', onclick: () => this.send() }));
    this.members = el('div', { class: 'scroll' });
    this.membersPanel = el('section', { class: 'panel members' }, el('header', { text: 'Members' }), this.members);
    root.append(
      el('section', { class: 'panel side' },
        el('header', {}, el('span', { class: 'title grow', text: 'Channels' }),
          el('button', { text: '+ Add hub', onclick: () => this.addHub() })),
        this.list),
      el('section', { class: 'panel grow' }, this.header, this.body, this.inputBar),
      this.membersPanel);
    this.lastView = null;
  },

  async addHub() {
    const address = prompt('RRC hub address (32 hex characters, or rrc://address/room)');
    if (!address) return;
    const result = await attempt(() => api.post('/channels', { address }));
    if (result) {
      this.selected = { hub: result.hub, room: result.room || '' };
      this.update();
    }
  },

  hubAction(hub, action, body = {}) {
    return attempt(() => api.post(`/channels/${hub}/${action}`, body));
  },

  async update() {
    // The hub list and the open room, fetched together when known.
    const known = this.selected && { ...this.selected };
    const [hubs, early] = await Promise.all([
      api.get('/channels'),
      known ? this.load(known.hub, known.room).catch(() => null) : null,
    ]);
    this.hubs = hubs;
    this.warmRooms(hubs);
    if (this.selected && !hubs.some((h) => h.hash === this.selected.hub)) this.selected = null;
    if (!this.selected && hubs.length) {
      const first = hubs[0];
      this.selected = { hub: first.hash, room: first.rooms.find((r) => r.joined)?.name || '' };
    }
    const items = [];
    for (const hub of hubs) {
      const isSelected = (room) => this.selected && this.selected.hub === hub.hash && this.selected.room === room;
      const select = (room) => () => this.select(hub.hash, room);
      // Start loading as the button goes down; the click shows it.
      const press = (room) => () => this.load(hub.hash, room).catch(() => {});
      items.push(el('div', { class: 'list-item' + (isSelected('') ? ' selected' : ''), dataset: { key: hub.hash + '/' }, onpointerdown: press(''), onclick: select('') },
        el('span', { class: 'dot ' + hub.status.kind, text: hub.status.kind === 'disconnected' ? '○' : hub.status.kind === 'connecting' ? '◌' : '●' }),
        el('span', { class: 'main name', text: hub.name }),
        hub.unread ? el('span', { class: 'badge' + (hub.mention ? ' mention' : ''), text: hub.unread }) : null));
      for (const room of hub.rooms) {
        items.push(el('div', { class: 'list-item room-item' + (room.joined ? '' : ' parted') + (isSelected(room.name) ? ' selected' : ''), dataset: { key: hub.hash + '/' + room.name }, onpointerdown: press(room.name), onclick: select(room.name) },
          el('span', { class: 'main name', text: '# ' + room.name }),
          room.unread ? el('span', { class: 'badge' + (room.mention ? ' mention' : ''), text: room.unread }) : null));
      }
      // Whisper conversations: "@" where rooms have "#".
      for (const whisper of hub.whispers) {
        items.push(el('div', { class: 'list-item room-item whisper-item' + (isSelected(whisper.key) ? ' selected' : ''), dataset: { key: hub.hash + '/' + whisper.key }, onpointerdown: press(whisper.key), onclick: select(whisper.key), title: 'Whisper conversation' },
          el('span', { class: 'main name' }, el('span', { class: 'whisper-icon', text: '@ ' }), whisper.name),
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
    const view = same && early ? early : await this.load(hash, room);
    if (!this.selected || this.selected.hub !== hash || this.selected.room !== room) return;
    const current = this.hubs.find((h) => h.hash === hash);
    const whisperEntry = view.whisper_with && current?.whispers.find((w) => w.key === room);
    const unreadNow = view.whisper_with ? whisperEntry?.unread : room ? current?.rooms.find((r) => r.name === room)?.unread : current?.unread;
    if (current && unreadNow && app.tab === 'channels') api.post(`/channels/${hash}/read`, { room }).catch(() => {});
    this.renderRoom(hash, room, view);
  },

  load(hub, room) {
    return loadInto(this, hub + '/' + room, `/channels/${hub}/room?name=${encodeURIComponent(room)}`);
  },

  // Open a room, whisper or hub page: at once from what is loaded (or its
  // name while it loads), then fresh.
  select(hash, room) {
    this.selected = { hub: hash, room };
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
    // The newest lines only, unless asked for all.
    const LIMIT = 200;
    const hidden = this.showAll === viewKey ? 0 : Math.max(0, view.lines.length - LIMIT);
    const earlier = hidden ? el('div', { class: 'show-more' }, el('button', { text: `Show ${hidden} earlier lines`, onclick: () => {
      this.showAll = viewKey;
      this.update();
    } })) : null;
    const render = () => this.body.replaceChildren(...(room || hidden ? [] : this.hubInfo(hub)), ...(earlier ? [earlier] : []), this.chat(view.lines.slice(hidden)));
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
        el('button', { class: 'danger', text: 'Close', title: 'Close the conversation and delete its messages', onclick: async () => {
          if (!confirm(`Close the whisper conversation with ${whisper.name} and delete its messages?`)) return;
          await this.hubAction(hash, 'forget', { room });
          this.selected = { hub: hash, room: '' };
          this.update();
        } }));
    } else if (room) {
      const entry = hub.rooms.find((r) => r.name === room);
      this.header.replaceChildren(
        el('span', { class: 'title grow' }, '#' + room, view.topic ? el('span', { class: 'dim', style: 'font-weight:400', text: ' — ' + view.topic }) : null),
        view.joined
          ? el('button', { text: 'Leave', onclick: () => this.hubAction(hash, 'leave', { room }) })
          : el('button', { text: 'Join', onclick: () => this.hubAction(hash, 'join', { room }) }),
        el('button', { text: 'Copy link', onclick: () => copy(entry?.link, 'link') }),
        el('button', { class: 'danger', text: 'Forget', title: 'Leave and delete the messages', onclick: async () => {
          if (!confirm(`Leave #${room} and delete its messages?`)) return;
          await this.hubAction(hash, 'forget', { room });
          this.selected = { hub: hash, room: '' };
          this.update();
        } }));
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
        el('button', { text: 'Copy link', onclick: () => copy(hub.link, 'link') }),
        el('button', { class: 'danger', text: 'Remove hub', onclick: async () => {
          if (!confirm(`Remove hub ${hub.name} and its history?`)) return;
          await this.hubAction(hash, 'remove');
          this.selected = null;
          this.update();
        } }));
    }
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
      el('span', { class: 'label', text: 'Limits' }), el('span', { text: `${hub.limits.message_bytes} bytes per message, ${hub.limits.rooms} rooms` }))];
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
          el('span', { class: 'text' + (line.pending ? ' pending' : '') }, ...this.marked(line.text, line.highlights || [])),
          line.pending ? el('span', { class: 'pending', text: ' …' }) : null));
    }));
  },

  // Text with only the mentions of us highlighted.
  marked(text, ranges) {
    const out = [];
    let at = 0;
    for (const [start, end] of ranges) {
      if (start < at || end <= start) continue;
      if (start > at) out.push(text.slice(at, start));
      out.push(el('span', { class: 'mention', text: text.slice(start, end) }));
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
    const menu = el('div', { class: 'user-menu', role: 'menu' },
      el('div', { class: 'menu-title', text: user.name }),
      item(this.view.whisper ? 'Whisper through the hub' : 'Whisper (this hub does not pass them)', 'opens your conversation', async () => {
        const result = await this.hubAction(this.selected.hub, 'whisper', { src });
        if (!result) return;
        this.selected = { hub: this.selected.hub, room: result.room };
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
      if (e.type === 'keydown' ? e.key === 'Escape' : !menu.contains(e.target)) close();
    };
    setTimeout(() => {
      document.addEventListener('click', this.menuCloser);
      document.addEventListener('keydown', this.menuCloser);
    });
  },

  closeMenu() {
    this.menu?.remove();
    this.menu = null;
    if (this.menuCloser) {
      document.removeEventListener('click', this.menuCloser);
      document.removeEventListener('keydown', this.menuCloser);
      this.menuCloser = null;
    }
  },

  async send() {
    const text = this.input.value;
    if (!text.trim() || !this.selected) return;
    const { hub, room } = this.selected;
    const result = await this.hubAction(hub, 'send', { room, text });
    if (!result) return;
    this.input.value = '';
    if (result.split) {
      const limit = this.hubs.find((h) => h.hash === hub)?.limits.message_bytes;
      if (confirm(`That is over this hub's ${limit}-byte limit. Send it as ${result.split.length} messages?`)) {
        await this.hubAction(hub, 'split', { room, parts: result.split });
      } else {
        this.input.value = text;
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
  pane: 'saved',
  page: null,
  history: [],
  viewSource: false,
  loading: null,

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
      el('section', { class: 'panel side' }, el('header', {}, this.paneTabs), this.paneList),
      el('section', { class: 'panel grow' },
        el('div', { class: 'toolbar' }, this.buttons.back, this.address, el('button', { class: 'primary', text: 'Go', onclick: () => this.go(this.address.value) }),
          this.buttons.reload, this.buttons.save, this.buttons.identify, this.buttons.source, this.buttons.clear),
        this.status, this.content));
    this.renderPane();
    this.renderPage();
  },

  async update() {
    const [saved, peers] = await Promise.all([api.get('/saved'), api.get('/peers?kind=nomad')]);
    this.saved = saved;
    this.nodes = peers.peers;
    this.renderPane();
    if (this.page) {
      const url = this.page.url;
      this.page.saved = saved.some((s) => s.url === url);
      this.renderButtons();
    }
  },

  renderPane() {
    this.paneTabs.replaceChildren(
      el('button', { class: this.pane === 'saved' ? 'active' : '', text: `Saved ${this.saved?.length ?? ''}`, onclick: () => {
        this.pane = 'saved';
        this.renderPane();
      } }),
      el('button', { class: this.pane === 'nodes' ? 'active' : '', text: `Nodes ${this.nodes?.length ?? ''}`, onclick: () => {
        this.pane = 'nodes';
        this.renderPane();
      } }));
    const current = this.page?.url;
    const currentNode = this.page?.node;
    if (this.pane === 'saved') {
      this.paneList.replaceChildren(...((this.saved || []).length ? this.saved.map((s) => el('div', {
        class: 'list-item' + (s.url === current ? ' selected' : ''),
        onclick: () => this.go(s.url),
      }, el('span', { class: 'main name', text: s.name }),
      el('button', { text: '×', title: 'Remove', class: 'danger', onclick: (e) => {
        e.stopPropagation();
        attempt(() => api.post('/saved/remove', { url: s.url }), `Removed ${s.name}`);
      } }))) : [el('div', { class: 'empty', text: 'Nothing saved yet. Open a page and press ☆ Save.' })]));
    } else {
      this.paneList.replaceChildren(...((this.nodes || []).length ? this.nodes.map((n) => el('div', {
        class: 'list-item' + (n.hash === currentNode ? ' selected' : ''),
        onclick: () => this.go(n.hash),
      }, el('span', { class: 'main' },
        el('div', { class: 'name', text: n.name || `<${n.hash.slice(0, 12)}>` }),
        el('div', { class: 'sub', text: `${n.hash}  ·  ${ago(n.last_seen)} ago` })))) :
        [el('div', { class: 'empty', text: 'No NomadNet nodes heard yet.' })]));
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
      el('section', { class: 'panel grow' },
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
    if (this.open?.path === path) return;
    if (this.dirty() && !confirm(`Discard unsaved changes to ${this.open.path}?`)) return;
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
      el('button', { text: 'Open in Browser', onclick: () => browse(open.url) }),
      el('button', { text: 'Rename', disabled: open.executable, onclick: () => this.rename() }),
      el('button', { class: 'danger', text: 'Delete', onclick: () => this.remove() }));
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
          el('button', { text: 'Restart Reticulum', onclick: () => restartReticulum() })), this.info),
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
        el('div', {}, input, el('div', { class: 'help', text: field.help })));
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

  update() {
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
      label('Known'), el('span', { text: `${s.known} destinations` }));
    this.interfaces.replaceChildren(...s.interfaces.map((i) => el('div', { class: 'iface' },
      el('span', { class: i.online ? 'online' : 'offline', text: i.online ? '● ' : '○ ' }), i.name,
      el('span', { class: 'dim', text: `  ↓${humanBytes(i.rx)} ↑${humanBytes(i.tx)}` }))));
    stickToBottom(this.log, () => this.log.replaceChildren(...s.log.map((line) => el('div', { text: line }))));
    // Pick up changes made elsewhere (the TUI's editor, or by hand).
    this.loadSettings();
  },
};

// ---- Reticulum --------------------------------------------------------------

// The Reticulum config file, option by option or as text. Changes are saved
// to the file and apply when Reticulum restarts. Pipe interface commands
// (programs Reticulum runs) can only be changed from the terminal.
app.views.reticulum = {
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
      el('section', { class: 'panel grow' },
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
        el('button', { text: 'Rename', onclick: () => this.rename(section.title) }),
        el('button', { class: 'danger', text: 'Delete', onclick: () => this.remove(section.title) }),
      ] : []),
      this.revertButton, this.saveButton);
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
    if (this.textMode) return;
    if (this.dirty() && !confirm('Discard unsaved changes?')) return;
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
  const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName);
  // As in the terminal UI, Esc stops writing (the shortcuts work again).
  if (typing && e.key === 'Escape') return document.activeElement.blur();
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
  setTimeout(prefetch, 300);
});
