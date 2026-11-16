// Runs the banner dock's height block (between `// Dock height:` and `// End of the dock block.`) and the
// setup-checklist open-state helpers against a fake page.
//
//   node --test app/wwwroot/admin/tests/

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../admin.js', import.meta.url), 'utf8');
const slice = (from, to) => {
  const start = source.indexOf(from);
  const end = source.indexOf(to, start);
  assert.ok(start > 0 && end > start, `${from} is in admin.js`);
  return source.slice(start, end);
};

function loadDock({ visible = true, height = 96 } = {}) {
  const state = { visible, height, writes: [] };
  const dock = {
    querySelector: () => (state.visible ? {} : null),
    getBoundingClientRect: () => ({ height: state.height }),
  };
  const observers = [];
  const sandbox = {
    document: {
      querySelector: sel => (sel === '.banner-dock' ? dock : null),
      documentElement: { style: { setProperty: (name, value) => state.writes.push([name, value]) } },
    },
    ResizeObserver: class { constructor(cb) { this.cb = cb; observers.push(this); } observe() {} },
    MutationObserver: class { constructor(cb) { this.cb = cb; observers.push(this); } observe() {} },
  };
  vm.runInNewContext(slice('// Dock height:', '// End of the dock block.'), sandbox);
  return { state, observers };
}

test('--dock-h follows the dock and is 0 when no notice shows', () => {
  const { state, observers } = loadDock({ visible: true, height: 96 });
  assert.deepEqual(state.writes, [['--dock-h', '96px']]);
  state.visible = false;
  observers[0].cb();
  assert.deepEqual(state.writes.at(-1), ['--dock-h', '0px']);
});

test('--dock-h is written only when the number changes', () => {
  const { state, observers } = loadDock({ height: 96 });
  observers[0].cb();
  observers[1].cb();
  assert.equal(state.writes.length, 1);
  state.height = 120.4;
  observers[0].cb();
  assert.deepEqual(state.writes.at(-1), ['--dock-h', '120px']);
  assert.equal(state.writes.length, 2);
});

function loadSetup(store) {
  const sandbox = {
    localStorage: { getItem: k => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) },
    document: { getElementById: () => sandbox.wrap },
    wrap: { open: true },
  };
  vm.runInNewContext(`${slice('function setupDone()', '// End of the setup open-state block.')}; this.api = { setupDone, setSetupDone };`, sandbox);
  return sandbox;
}

test('the setup checklist opens until a finished setup was remembered, then starts collapsed', () => {
  const store = new Map();
  assert.equal(loadSetup(store).wrap.open, true);
  loadSetup(store).api.setSetupDone();
  assert.equal(store.get('flakyfy.setupDone'), '1');
  assert.equal(loadSetup(store).wrap.open, false);
});

test('a blocked localStorage leaves the checklist open and never throws', () => {
  const sandbox = {
    localStorage: { getItem() { throw new Error('blocked'); }, setItem() { throw new Error('blocked'); } },
    document: { getElementById: () => sandbox.wrap },
    wrap: { open: false },
  };
  vm.runInNewContext(`${slice('function setupDone()', '// End of the setup open-state block.')}; this.api = { setSetupDone };`, sandbox);
  assert.equal(sandbox.wrap.open, true);
  assert.doesNotThrow(() => sandbox.api.setSetupDone());
});
