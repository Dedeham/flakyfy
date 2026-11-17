// Runs the dashboard's Actions page (the part of admin.js between `// Actions: every job in one
// list` and `// Toast helper`) against a fake page and a scripted API.
//
//   node --test app/wwwroot/admin/tests/

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../admin.js', import.meta.url), 'utf8');
const start = source.indexOf('// Actions: every job in one list');
const end = source.indexOf('// Toast helper');
assert.ok(start > 0 && end > start, 'the Actions block is in admin.js');
const block = source.slice(start, end);

function element() {
  const handlers = {};
  const classes = new Set();
  return {
    innerHTML: '',
    textContent: '',
    hidden: false,
    dataset: {},
    handlers,
    classes,
    classList: { toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)) },
    addEventListener(type, handler) { handlers[type] = handler; },
    contains: () => false,
    querySelector: () => null,
  };
}

const escape = text => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

// The page with the Actions pane open, an API that answers from `replies` (path -> body or
// [status, body]) and a confirm dialog that says `confirmed`.
function load({ replies = {}, confirmed = true } = {}) {
  const ids = {
    'actions-running': element(),
    'actions-finished': element(),
    'actions-clear': element(),
    'actions-title': element(),
  };
  const pane = element();
  const badge = element();
  const calls = [];
  const notes = [];
  const sandbox = {
    document: {
      activeElement: null,
      createElement: () => element(),
      getElementById: id => ids[id] ?? null,
      querySelector: selector => {
        if (selector === 'section[data-pane="actions"]') return pane;
        if (selector === '[data-actions-badge]') return badge;
        return /^section\[data-pane="(tags|fetched|lossy)"\]$/.test(selector) ? element() : null;
      },
    },
    CSS: { escape: text => text },
    api: async (path, options = {}) => {
      calls.push({ path, method: options.method || 'GET', body: options.body ? JSON.parse(options.body) : undefined });
      const reply = replies[path] ?? {};
      const [status, body] = Array.isArray(reply) ? reply : [200, reply];
      return { ok: status < 400, status, json: async () => body };
    },
    browseAuthenticate: async () => false,
    askConfirm: async () => confirmed,
    note: (anchor, message, kind) => notes.push({ message, kind }),
    openTab: page => calls.push({ opened: page }),
    activityTick: () => {},
    activityKey: item => item.kind,
    activityFraction: () => null,
    activityIcon: () => '',
    activityLine: item => item.detail || '',
    activityPageOpen: page => page === 'actions',
    relativeTime: () => 'just now',
    icon: () => '',
    escapeHtml: escape,
    stateBlock: (kind, message) => `<div class="state">${message}</div>`,
  };
  vm.createContext(sandbox);
  vm.runInContext(block, sandbox);
  const stop = id => pane.handlers.click({
    target: { closest: selector => (selector === '[data-action-stop]' ? { closest: () => ({ dataset: { actionId: id } }) } : null) },
  });
  return { sandbox, ids, pane, badge, calls, notes, stop };
}

const job = (kind, page, state, extra = {}) => ({ id: kind, kind, page, title: `The ${kind} job`, state, ...extra });

test('a job its page can stop gets a Stop button, and any other only opens its page', () => {
  const { sandbox, ids, badge } = load();
  sandbox.renderActions([job('tidy', 'tags', 'running'), job('downloads', 'fetched', 'running')]);
  const rows = ids['actions-running'].innerHTML.split('<div class="action-row"').slice(1);
  assert.equal(rows.length, 2);
  assert.match(rows[0], /data-action-stop/);
  assert.match(rows[0], /data-action-open="tags"/);
  assert.doesNotMatch(rows[1], /data-action-stop/);
  assert.match(rows[1], /data-action-open="fetched"/);
  assert.equal(badge.textContent, '2');
  assert.ok(badge.classes.has('good'));
});

test('Stop, once confirmed, makes the call the job\'s own page makes', async () => {
  const { sandbox, calls, notes, stop } = load();
  sandbox.renderActions([job('tidy', 'tags', 'running')]);
  await stop('tidy');
  assert.deepEqual(calls, [{ path: '/api/admin/tidy/cancel', method: 'POST', body: undefined }]);
  assert.deepEqual(notes, [{ message: 'The tidy job: Stopping after the current file.', kind: 'info' }]);
});

test('Stop that is not confirmed calls nothing', async () => {
  const { sandbox, calls, notes, stop } = load({ confirmed: false });
  sandbox.renderActions([job('tidy', 'tags', 'running')]);
  await stop('tidy');
  assert.deepEqual(calls, []);
  assert.deepEqual(notes, []);
});

test('a stop the server refuses is said as an error', async () => {
  const { sandbox, notes, stop } = load({ replies: { '/api/admin/tidy/cancel': [409, { error: 'No tidy is running.' }] } });
  sandbox.renderActions([job('tidy', 'tags', 'running')]);
  await stop('tidy');
  assert.deepEqual(notes, [{ message: 'The tidy job: No tidy is running.', kind: 'error' }]);
});

test('stopping better copies takes back only the songs still waiting', async () => {
  const { sandbox, calls, notes, stop } = load({
    replies: {
      '/api/admin/upgrades': { jobs: [{ id: 'a', state: 'working' }, { id: 'b', state: 'queued' }, { id: 'c', state: 'waiting' }, { id: 'd', state: 'upgraded' }] },
      '/api/admin/upgrades/cancel': { cancelled: 2 },
    },
  });
  sandbox.renderActions([job('upgrades', 'lossy', 'running')]);
  await stop('upgrades');
  assert.deepEqual(calls[1], { path: '/api/admin/upgrades/cancel', method: 'POST', body: { ids: ['b', 'c'] } });
  assert.equal(notes[0].message, 'The upgrades job: Took back 2 songs. The song being fetched finishes.');
});

test('a finished job stays listed after the server drops it, a grown batch is one row, and Clear empties the list', () => {
  const { sandbox, ids } = load();
  const count = () => ids['actions-finished'].innerHTML.split('<div class="action-row"').length - 1;
  sandbox.renderActions([{ ...job('downloads', 'fetched', 'done'), id: 'downloads:1', title: 'Added 1 song' }]);
  sandbox.renderActions([{ ...job('downloads', 'fetched', 'done'), id: 'downloads:2', title: 'Added 2 songs' }]);
  assert.equal(count(), 1);
  assert.match(ids['actions-finished'].innerHTML, /Added 2 songs/);
  sandbox.renderActions([]);
  assert.equal(count(), 1, 'still listed once the server no longer lists it');
  assert.equal(ids['actions-clear'].hidden, false);

  sandbox.renderActions([{ ...job('tidy', 'tags', 'stopped'), id: 'tidy:9' }]);
  assert.equal(count(), 2);
  ids['actions-clear'].handlers.click();
  sandbox.renderActions([{ ...job('tidy', 'tags', 'stopped'), id: 'tidy:9' }]);
  assert.equal(count(), 0, 'a cleared job the server still lists does not come back');
  assert.equal(ids['actions-clear'].hidden, true);
});
