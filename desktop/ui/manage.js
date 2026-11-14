'use strict';
// The management window's page: the app's own, with nothing from the network.
const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);

function say(text, kind) {
  const n = $('notice');
  n.textContent = text;
  n.className = kind || '';
  n.hidden = !text;
}
const fail = (e) => say(typeof e === 'string' ? e : (e && e.message) || 'That failed', 'error');

// A page-made confirmation (window.confirm is not always there).
function confirmed(text, ok) {
  return new Promise((resolve) => {
    const d = $('confirm');
    $('confirm-text').textContent = text;
    $('confirm-ok').textContent = ok;
    d.addEventListener('close', () => resolve(d.returnValue === 'ok'), { once: true });
    d.returnValue = 'cancel';
    d.showModal();
  });
}

function button(label, onclick, cls) {
  const b = document.createElement('button');
  b.type = 'button';
  b.textContent = label;
  if (cls) b.className = cls;
  b.addEventListener('click', onclick);
  return b;
}

function el(tag, text, cls) {
  const e = document.createElement(tag);
  if (text != null) e.textContent = text;
  if (cls) e.className = cls;
  return e;
}

// Run an action: say what failed, or what was done.
async function act(work, done) {
  say('');
  try {
    const result = await work();
    if (done) say(typeof done === 'function' ? done(result) : done, 'done');
    return result;
  } catch (e) {
    fail(e);
  }
}

let defaults = {};

// ---- Tabs ----
function show(tab) {
  for (const b of document.querySelectorAll('nav button')) b.setAttribute('aria-selected', String(b.dataset.tab === tab));
  for (const s of document.querySelectorAll('main > section')) s.hidden = s.id !== tab;
  say('');
  if (tab === 'identities') loadIdentities();
  if (tab === 'pipes') loadPipes();
}
for (const b of document.querySelectorAll('nav button')) b.addEventListener('click', () => show(b.dataset.tab));

// ---- Identities ----
async function loadIdentities() {
  let data;
  try {
    data = await invoke('manage_identities');
  } catch (e) {
    return fail(e);
  }
  const list = $('identity-list');
  list.replaceChildren();
  for (const p of data.identities) {
    const li = el('li');
    const title = el('div', null, 'row');
    title.append(el('span', p.name || '(no name)', 'title'));
    if (p.active) title.append(el('span', 'in use', 'badge'));
    li.append(title, el('code', p.address));
    const row = el('div', null, 'row');
    if (!p.active) row.append(button('Switch to it', () => switchTo(p)));
    row.append(button('Rename', () => rename(p)));
    if (!p.active && !p.first) row.append(button('Delete', () => remove(p), 'danger'));
    li.append(row);
    list.append(li);
  }
  const first = defaults.identity || '';
  const to = document.querySelector('#export [name=to]');
  if (!to.value && first) to.value = first;
}

async function switchTo(p) {
  if (!(await confirmed(`Switch to ${p.name}? Everything is saved, and rettui starts again as it. The main window loads again.`, 'Switch'))) return;
  await act(() => invoke('manage_switch_identity', { id: p.id }), (r) => `Switching to ${r.name}…`);
  setTimeout(loadIdentities, 2500);
}

async function rename(p) {
  const name = window.prompt ? window.prompt('The name it announces', p.name) : null;
  if (name == null || !name.trim()) return;
  await act(() => invoke('manage_rename_identity', { id: p.id, name }), 'Renamed.');
  loadIdentities();
}

async function remove(p) {
  if (!(await confirmed(`Delete ${p.name} (${p.address}) and its conversations, contacts and settings? Without a copy of its key it can't be brought back.`, 'Delete'))) return;
  await act(() => invoke('manage_delete_identity', { id: p.id }), (n) => `Deleted ${n}.`);
  loadIdentities();
}

$('keep').addEventListener('submit', async (e) => {
  e.preventDefault();
  const f = e.target;
  const r = await act(() => invoke('manage_keep_identity', { name: f.name.value, from: f.from.value || null }), (r) => `Kept ${r.name} (${r.address}). Switch to it from the list.`);
  if (r) f.reset();
  loadIdentities();
});

$('export').addEventListener('submit', async (e) => {
  e.preventDefault();
  const to = e.target.to.value;
  if (!(await confirmed('The copy holds your key, the file that makes it you. Keep it somewhere private.', 'Save the copy'))) return;
  await act(() => invoke('manage_export_identity', { to }), (path) => `Saved a copy to ${path}.`);
});

// ---- Backup ----
$('backup-form').passphrase.addEventListener('input', async (e) => {
  const weak = $('weak');
  const why = e.target.value ? await invoke('manage_passphrase_weakness', { passphrase: e.target.value }).catch(() => null) : null;
  weak.textContent = why || '';
  weak.hidden = !why;
});

$('backup-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const f = e.target;
  if (f.passphrase.value !== f.again.value) return say('The passphrases differ: nothing was backed up.', 'error');
  const sealed = !!f.passphrase.value;
  if (!sealed && !(await confirmed('Without a passphrase the backup holds your key in the clear: whoever has the file can pose as you. Back up anyway?', 'Back up'))) return;
  const button = f.querySelector('button');
  button.disabled = true;
  say('Backing up…');
  const r = await act(
    () => invoke('manage_back_up', { to: f.to.value, passphrase: sealed ? f.passphrase.value : null }),
    (n) => `Backed up ${n} files to ${f.to.value}${sealed ? ', sealed: it can’t be restored without the passphrase' : ''}.`,
  );
  button.disabled = false;
  if (r != null) {
    f.passphrase.value = f.again.value = '';
    f.to.value = (await invoke('manage_defaults')).backup;
  }
});

// ---- Pipe interfaces ----
async function loadPipes() {
  let data;
  try {
    data = await invoke('manage_pipes');
  } catch (e) {
    return fail(e);
  }
  const list = $('pipe-list');
  list.replaceChildren();
  if (!data.pipes.length) list.append(el('li', 'No pipe interfaces.'));
  for (const p of data.pipes) {
    const li = el('li');
    const title = el('div', null, 'row');
    title.append(el('span', p.name, 'title'), el('span', p.enabled ? 'on' : 'off', p.enabled ? 'badge' : 'badge off'));
    li.append(title, el('code', p.command || '(no command)'));
    const row = el('div', null, 'row');
    row.append(button('Change command', () => changeCommand(p)));
    row.append(button(p.enabled ? 'Turn off' : 'Turn on', () => toggle(p)));
    row.append(button('Remove', () => removePipe(p), 'danger'));
    li.append(row);
    list.append(li);
  }
}

const restartNeeded = () => { $('restart-row').hidden = false; };

async function changeCommand(p) {
  const command = window.prompt ? window.prompt(`The program ${p.name} runs`, p.command) : null;
  if (command == null || !command.trim() || command === p.command) return;
  if (!(await confirmed(`${p.name} will run “${command.trim()}” with your account's rights when it's on. Only use a program you trust.`, 'Use it'))) return;
  if (await act(() => invoke('manage_set_pipe', { name: p.name, command, enabled: p.enabled }), 'Changed.')) restartNeeded();
  loadPipes();
}

async function toggle(p) {
  if (!p.enabled && !(await confirmed(`Turn on ${p.name}? It runs “${p.command}” with your account's rights.`, 'Turn on'))) return;
  if (await act(() => invoke('manage_set_pipe', { name: p.name, command: p.command, enabled: !p.enabled }), p.enabled ? 'Turned off.' : 'Turned on.')) restartNeeded();
  loadPipes();
}

async function removePipe(p) {
  if (!(await confirmed(`Remove ${p.name} from Reticulum's config?`, 'Remove'))) return;
  if (await act(() => invoke('manage_remove_pipe', { name: p.name }), 'Removed.')) restartNeeded();
  loadPipes();
}

$('add-pipe').addEventListener('submit', async (e) => {
  e.preventDefault();
  const f = e.target;
  if (!(await confirmed(`Add ${f.name.value}, which will run “${f.command.value}” with your account's rights once you turn it on. Only use a program you trust.`, 'Add'))) return;
  if (await act(() => invoke('manage_add_pipe', { name: f.name.value, command: f.command.value }), 'Added, turned off.')) {
    f.reset();
    restartNeeded();
  }
  loadPipes();
});

$('restart').addEventListener('click', async () => {
  if (await act(() => invoke('manage_restart_reticulum'), 'Reticulum is starting again.')) $('restart-row').hidden = true;
});

(async () => {
  try {
    defaults = await invoke('manage_defaults');
    document.querySelector('#backup-form [name=to]').value = defaults.backup;
  } catch (e) {
    fail(e);
  }
  loadIdentities();
})();
