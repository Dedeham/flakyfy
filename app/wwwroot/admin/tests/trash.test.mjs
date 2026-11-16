// loadTrash asks nothing while library actions are off (that answer is a 403 the dashboard
// should never provoke) and asks as before when they are on or the settings are not loaded yet.
//
//   node --test app/wwwroot/admin/tests/

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../admin.js', import.meta.url), 'utf8');
const start = source.indexOf('async function loadTrash()');
const end = source.indexOf('\n}\n', start) + 3;
assert.ok(start > 0 && end > start, 'loadTrash is in admin.js');

function load(currentSettings) {
  const list = { innerHTML: '' };
  const summary = { textContent: '' };
  const asked = [];
  const sandbox = {
    currentSettings,
    findEl: id => (id === 'trash-list' ? list : id === 'trash-summary' ? summary : null),
    stateBlock: (kind, text) => `[${kind}] ${text}`,
    parityFetch: async path => { asked.push(path); return { ok: true, json: async () => ({ songs: [] }) }; },
    parityError: async () => 'error',
    escapeHtml: s => s, relTime: () => '', icon: () => '',
  };
  vm.runInNewContext(`${source.slice(start, end)}; this.run = loadTrash;`, sandbox);
  return { run: sandbox.run, list, summary, asked };
}

test('library actions off: no request, an empty state instead', async () => {
  const page = load({ LibraryActions: { Enabled: false } });
  await page.run();
  assert.deepEqual(page.asked, []);
  assert.match(page.list.innerHTML, /^\[empty\]/);
  assert.match(page.summary.textContent, /off/);
});

test('library actions on, or settings not loaded: the trash is asked for', async () => {
  for (const settings of [{ LibraryActions: { Enabled: true } }, null]) {
    const page = load(settings);
    await page.run();
    assert.deepEqual(page.asked, ['/api/admin/trash']);
  }
});
