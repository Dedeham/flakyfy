// Runs the dashboard's shell block (the seven sidebar groups, the phone menu sheet and the group
// flags: the part of admin.js between `// Shell: the seven groups` and `// End of the shell
// block.`) against a fake page.
//
//   node --test app/wwwroot/admin/tests/

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../admin.js', import.meta.url), 'utf8');
const start = source.indexOf('// Shell: the seven groups');
const end = source.indexOf('// End of the shell block.');
assert.ok(start > 0 && end > start, 'the shell block is in admin.js');
const block = source.slice(start, end);

function element(extra = {}) {
  const classes = new Set();
  const attrs = {};
  const handlers = {};
  return {
    dataset: {},
    textContent: '',
    classes,
    attrs,
    handlers,
    focused: 0,
    classList: {
      toggle: (name, on) => { if (on === undefined ? !classes.has(name) : on) classes.add(name); else classes.delete(name); },
      contains: name => classes.has(name),
    },
    setAttribute: (name, value) => { attrs[name] = value; },
    removeAttribute: name => { delete attrs[name]; },
    addEventListener(type, handler) { handlers[type] = handler; },
    focus() { this.focused += 1; },
    contains(node) { return node === this; },
    querySelector: () => null,
    ...extra,
  };
}

function group(key, panes, text = element()) {
  return element({
    dataset: { group: key, panes: panes.join(' '), open: panes[0] },
    title: key[0].toUpperCase() + key.slice(1),
    querySelector: sel => (sel === '.group-flag-text' ? text : null),
    text,
  });
}

// The page: three groups, each with a tab strip whose flagged state a test sets.
function load() {
  const groups = [group('overview', ['status', 'actions']), group('downloads', ['downloads', 'soulseek']), group('server', ['subsonic', 'family'])];
  const flagged = new Set();
  const strips = Object.fromEntries(groups.map(g => [g.dataset.group, { querySelector: () => (flagged.has(g.dataset.group) ? {} : null) }]));
  const sidebar = element();
  const toggle = element();
  const opened = [];
  const documentHandlers = {};
  let frames = [];
  const observers = [];
  const sandbox = {
    document: {
      querySelectorAll: sel => (sel === '.nav-group-item' ? groups : sel === '.subnav-tabs' ? Object.values(strips).map(() => element()) : []),
      querySelector: sel => {
        if (sel === '.sidebar') return sidebar;
        if (sel === '.nav-toggle') return toggle;
        const m = sel.match(/data-group="(\w+)"\] \.subnav-tabs/);
        return m ? strips[m[1]] : null;
      },
      addEventListener: (type, handler) => { documentHandlers[type] = handler; },
    },
    window: { MutationObserver: class { constructor(cb) { this.cb = cb; observers.push(this); } observe() {} } },
    MutationObserver: class { constructor(cb) { this.cb = cb; observers.push(this); } observe() {} },
    requestAnimationFrame: cb => { frames.push(cb); return frames.length; },
    openTab: name => opened.push(name),
  };
  sandbox.window.MutationObserver = sandbox.MutationObserver;
  vm.runInNewContext(`${block}; this.api = { syncNavGroups, syncGroupFlags, setNavOpen };`, sandbox);
  const runFrames = () => { const f = frames; frames = []; f.forEach(cb => cb()); };
  return { ...sandbox.api, groups, flagged, sidebar, toggle, opened, documentHandlers, observers, runFrames, frames: () => frames.length };
}

test('syncNavGroups lights the group that holds the pane and only that one', () => {
  const page = load();
  page.syncNavGroups('soulseek');
  assert.deepEqual(page.groups.map(g => g.classes.has('active')), [false, true, false]);
  assert.equal(page.groups[1].attrs['aria-current'], 'page');
  assert.equal(page.groups[0].attrs['aria-current'], undefined);
  assert.equal(page.sidebar.dataset.current, 'Downloads', 'the phone top bar names the group');
  page.syncNavGroups('status');
  assert.deepEqual(page.groups.map(g => g.classes.has('active')), [true, false, false]);
  assert.equal(page.groups[1].attrs['aria-current'], undefined);
  assert.equal(page.sidebar.dataset.current, 'Overview');
});

test('a group click opens its first tab', () => {
  const page = load();
  page.groups[2].handlers.click();
  assert.deepEqual(page.opened, ['subsonic']);
});

test('syncNavGroups closes the phone sheet', () => {
  const page = load();
  page.setNavOpen(true);
  assert.ok(page.sidebar.classes.has('nav-open'));
  assert.equal(page.toggle.attrs['aria-expanded'], 'true');
  page.syncNavGroups('family');
  assert.ok(!page.sidebar.classes.has('nav-open'));
  assert.equal(page.toggle.attrs['aria-expanded'], 'false');
});

test('the menu button toggles the sheet', () => {
  const page = load();
  page.toggle.handlers.click();
  assert.ok(page.sidebar.classes.has('nav-open'));
  page.toggle.handlers.click();
  assert.ok(!page.sidebar.classes.has('nav-open'));
});

test('Escape closes the sheet and returns focus to the button, but only when open', () => {
  const page = load();
  page.documentHandlers.keydown({ key: 'Escape' });
  assert.equal(page.toggle.focused, 0);
  page.setNavOpen(true);
  page.documentHandlers.keydown({ key: 'Enter' });
  assert.ok(page.sidebar.classes.has('nav-open'));
  page.documentHandlers.keydown({ key: 'Escape' });
  assert.ok(!page.sidebar.classes.has('nav-open'));
  assert.equal(page.toggle.focused, 1);
});

test('a click outside the sidebar closes the sheet, a click inside keeps it open', () => {
  const page = load();
  page.setNavOpen(true);
  page.documentHandlers.click({ target: page.sidebar });
  assert.ok(page.sidebar.classes.has('nav-open'));
  page.documentHandlers.click({ target: {} });
  assert.ok(!page.sidebar.classes.has('nav-open'));
});

test('a flagged tab flags its group, with a word for screen readers, and clears again', () => {
  const page = load();
  page.flagged.add('downloads');
  page.syncGroupFlags();
  assert.deepEqual(page.groups.map(g => g.classes.has('has-flag')), [false, true, false]);
  assert.equal(page.groups[1].text.textContent, 'needs attention');
  assert.equal(page.groups[0].text.textContent, '');
  page.flagged.clear();
  page.syncGroupFlags();
  assert.ok(!page.groups[1].classes.has('has-flag'));
  assert.equal(page.groups[1].text.textContent, '');
});

test('syncGroupFlags leaves the page alone when nothing changed', () => {
  const page = load();
  page.flagged.add('server');
  page.syncGroupFlags();
  let writes = 0;
  const text = page.groups[2].text;
  let value = text.textContent;
  Object.defineProperty(text, 'textContent', { get: () => value, set: v => { writes += 1; value = v; } });
  page.syncGroupFlags();
  page.syncGroupFlags();
  assert.equal(writes, 0);
});

test('a burst of tab changes is one flag pass, in one frame', () => {
  const page = load();
  assert.ok(page.observers.length >= 1);
  const watcher = page.observers[0];
  watcher.cb(); watcher.cb(); watcher.cb();
  assert.equal(page.frames(), 1);
  page.runFrames();
  assert.equal(page.frames(), 0);
  watcher.cb();
  assert.equal(page.frames(), 1);
});
