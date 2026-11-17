// Settings controls get their accessible names pane by pane, the first time a pane is shown.
//
//   node --test app/wwwroot/admin/tests/

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../admin.js', import.meta.url), 'utf8');
const start = source.indexOf('let labelCount = 0;');
const end = source.indexOf('// ────', source.indexOf('function labelSettingControls'));
assert.ok(start > 0 && end > start, 'the labelling block is in admin.js');

function pane(titles) {
  const rows = titles.map(text => {
    const title = { id: '', querySelector: () => null };
    const ctrl = { attrs: {}, labels: [], hasAttribute: k => k in ctrl.attrs, setAttribute: (k, v) => { ctrl.attrs[k] = v; }, closest: () => null };
    const row = { title, ctrl, querySelector: sel => (sel === ':scope > .set-info .set-info-t' ? title : null), querySelectorAll: () => [ctrl] };
    return row;
  });
  return { dataset: {}, rows, querySelectorAll: () => rows };
}

test('a pane is labelled once, on first show, with ids that never repeat across panes', () => {
  const sandbox = {};
  vm.runInNewContext(`${source.slice(start, end)}; this.labelPane = labelPane;`, sandbox);
  const a = pane(['x', 'y']);
  const b = pane(['z']);
  sandbox.labelPane(a);
  assert.deepEqual(a.rows.map(r => r.title.id), ['set-t-1', 'set-t-2']);
  assert.equal(a.rows[0].ctrl.attrs['aria-labelledby'], 'set-t-1');
  sandbox.labelPane(a);
  assert.deepEqual(a.rows.map(r => r.title.id), ['set-t-1', 'set-t-2'], 'a second show does nothing');
  sandbox.labelPane(b);
  assert.equal(b.rows[0].title.id, 'set-t-3');
  assert.doesNotThrow(() => sandbox.labelPane(null));
});
