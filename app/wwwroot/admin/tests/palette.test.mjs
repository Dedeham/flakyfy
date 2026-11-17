// Runs the Ctrl/Cmd+K jump palette (between `// Jump palette:` and `// End of the jump block.`) against a fake
// page, and checks that every pane of index.html is reachable from the tab strips it reads.
//
//   node --test app/wwwroot/admin/tests/

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../admin.js', import.meta.url), 'utf8');
const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
const start = source.indexOf('// Jump palette:');
const end = source.indexOf('// End of the jump block.');
assert.ok(start > 0 && end > start, 'the jump block is in admin.js');
const block = source.slice(start, end);

const span = text => ({ textContent: text });
const tabEl = (tab, label) => ({ dataset: { tab }, textContent: label, querySelector: () => span(label) });
const groupEl = (open, title) => ({ dataset: { open }, title, textContent: title, querySelector: () => span(title) });
const paneGroup = (title, tabs) => ({ querySelector: sel => (sel === '.subnav-title' ? span(title) : null), querySelectorAll: () => tabs });

function load() {
  const handlers = {};
  const opened = [];
  const state = { open: false, closed: 0 };
  const dialog = { get open() { return state.open; }, showModal() { state.open = true; }, close() { state.open = false; state.closed++; } };
  const input = { value: '', attrs: {}, handlers: {}, setAttribute(k, v) { this.attrs[k] = v; }, focus() {}, addEventListener(t, f) { this.handlers[t] = f; } };
  const list = { innerHTML: '', handlers: {}, addEventListener(t, f) { this.handlers[t] = f; }, querySelector: () => null };
  const groups = [groupEl('status', 'Overview'), groupEl('soulseek', 'Downloads')];
  const strips = [paneGroup('Overview', [tabEl('status', 'Status'), tabEl('actions', 'Actions')]), paneGroup('Downloads', [tabEl('soulseek', 'Soulseek'), tabEl('lidarr', 'Lidarr')])];
  const sandbox = {
    document: {
      getElementById: id => ({ jump: dialog, 'jump-q': input, 'jump-list': list }[id] ?? null),
      querySelectorAll: sel => (sel === '.nav-group-item' ? groups : sel === '.pane-group' ? strips : []),
      addEventListener: (t, f) => { handlers[t] = f; },
    },
    esc: s => String(s),
    openTab: tab => opened.push(tab),
  };
  vm.runInNewContext(`${block}; this.api = { jumpBuild, jumpFilter };`, sandbox);
  const key = (k, mods = {}) => { let prevented = false; handlers.keydown({ key: k, preventDefault() { prevented = true; }, ...mods }); return prevented; };
  const press = k => { let prevented = false; input.handlers.keydown({ key: k, preventDefault() { prevented = true; } }); return prevented; };
  return { ...sandbox.api, handlers, opened, state, input, list, key, press };
}

test('Ctrl+K and Cmd+K open the palette, even in a field, and stop the browser shortcut', () => {
  for (const mods of [{ ctrlKey: true }, { metaKey: true }]) {
    const page = load();
    assert.equal(page.key('k', mods), true);
    assert.equal(page.state.open, true);
  }
  const page = load();
  assert.equal(page.key('k'), false, 'a bare k is left alone');
  assert.equal(page.state.open, false);
});

test('the list is groups first, then every page with its group, and filters on name, group and tab', () => {
  const page = load();
  const items = page.jumpBuild();
  assert.deepEqual(Array.from(items, i => i.tab), ['status', 'soulseek', 'status', 'actions', 'soulseek', 'lidarr']);
  assert.deepEqual(Array.from(page.jumpFilter(items, 'soul'), i => i.tab), ['soulseek', 'soulseek']);
  assert.deepEqual(page.jumpFilter(items, 'DOWNLOADS').length, 3);
  assert.equal(page.jumpFilter(items, 'zzz').length, 0);
});

test('Down twice then Enter opens the third page and closes the dialog', () => {
  const page = load();
  page.key('k', { ctrlKey: true });
  page.press('ArrowDown');
  page.press('ArrowDown');
  page.press('Enter');
  assert.deepEqual(page.opened, ['status']);
  assert.equal(page.state.open, false);
});

test('a group row opens the group first tab, typing filters, and no match does nothing on Enter', () => {
  const page = load();
  page.key('k', { ctrlKey: true });
  page.press('ArrowDown');
  page.press('Enter');
  assert.deepEqual(page.opened, ['soulseek']);
  page.key('k', { ctrlKey: true });
  page.input.value = 'zzz';
  page.input.handlers.input();
  assert.match(page.list.innerHTML, /No page matches/);
  page.press('Enter');
  assert.deepEqual(page.opened, ['soulseek']);
  assert.equal(page.state.open, true, 'the palette stays open with nothing to open');
});

test('a click on an option opens it', () => {
  const page = load();
  page.key('k', { ctrlKey: true });
  page.list.handlers.click({ target: { closest: () => ({ dataset: { i: '5' } }) } });
  assert.deepEqual(page.opened, ['lidarr']);
});

test('every pane of the page has a tab in a strip, so the palette reaches all of them', () => {
  const panes = [...html.matchAll(/<section[^>]*data-pane="(\w+)"/g)].map(m => m[1]);
  const tabs = new Set([...html.matchAll(/class="[^"]*subnav-item[^"]*"[^>]*data-tab="(\w+)"|data-tab="(\w+)"[^>]*class="[^"]*subnav-item/g)].map(m => m[1] || m[2]));
  assert.ok(panes.length >= 25, `found ${panes.length} panes`);
  assert.deepEqual(panes.filter(p => !tabs.has(p)), []);
});
