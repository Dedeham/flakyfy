// Malformed or thin server answers draw an error or empty state and never throw, and clipped
// text keeps its full form in a title. Pieces of admin.js and listening.js run against fakes.
//
//   node --test app/wwwroot/admin/tests/

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const admin = readFileSync(new URL('../admin.js', import.meta.url), 'utf8');
const listening = readFileSync(new URL('../listening.js', import.meta.url), 'utf8');

// A top-level function of admin.js, by name.
function fn(name) {
  const at = admin.search(new RegExp(`^(async )?function ${name}\\(`, 'm'));
  assert.ok(at >= 0, `${name} is in admin.js`);
  return admin.slice(at, admin.indexOf('\n}\n', at) + 3);
}

// An element that takes any property, call or assignment.
function loose() {
  const store = {};
  return new Proxy(function () {}, {
    get: (_, k) => (k in store ? store[k] : k === 'classList' || k === 'dataset' || k === 'style' ? loose() : k === Symbol.toPrimitive ? () => '' : loose()),
    set: (_, k, v) => { store[k] = v; return true; },
    apply: () => loose(),
  });
}

function run(names, extra = {}, replies = {}) {
  const els = {};
  const sandbox = {
    document: { getElementById: id => (els[id] ??= { hidden: false, textContent: '', innerHTML: '', value: '', checked: false, classList: { toggle() {} }, replaceChildren(...c) { this.children = c; }, addEventListener() {} }),
      createElement: () => ({ setAttribute() {}, append() {}, addEventListener() {} }) },
    api: async path => { const body = replies[path.split('?')[0]]; return { ok: body !== undefined, status: body === undefined ? 500 : 200, json: async () => body }; },
    escapeHtml: s => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;'),
    relTime: () => 'now', fmtSize: () => '', findArt: () => '', fetchedFindButton: () => '',
    window: {}, console, ...extra,
  };
  vm.createContext(sandbox);
  vm.runInContext(names.map(fn).join('\n') + (extra.__tail || ''), sandbox);
  return { sandbox, els };
}

test('normalizeUpdate: no latest release means no update is out, and junk is refused', () => {
  const { sandbox } = run(['normalizeUpdate']);
  const info = sandbox.normalizeUpdate({ updateAvailable: true, latest: null });
  assert.equal(info.updateAvailable, false);
  assert.deepEqual([...info.newer], []);
  assert.throws(() => sandbox.normalizeUpdate(null));
  assert.throws(() => sandbox.normalizeUpdate('x'));
  const ok = { updateAvailable: true, latest: { tag: 'v1' }, newer: [{ tag: 'v1' }] };
  assert.equal(sandbox.normalizeUpdate(ok).updateAvailable, true);
});

test('lfmAgo: a missing or bad date says nothing, not Invalid Date', () => {
  const { sandbox } = run(['lfmAgo']);
  for (const bad of [undefined, null, '', 'nope']) assert.equal(sandbox.lfmAgo(bad), '');
  assert.equal(sandbox.lfmAgo(new Date().toISOString()), 'just now');
});

test('lfmRow: a last play without a date has no stray comma or Invalid Date', () => {
  const { sandbox } = run(['lfmAgo', 'lfmRow']);
  const html = sandbox.lfmRow({ user: 'a', connected: true, lastSent: { title: 'T', artist: 'A' } }, true);
  assert.match(html, /Last sent: T by A\./);
  assert.doesNotMatch(html, /Invalid|undefined/);
});

test('radio stations: a missing track count is left out, not "undefined tracks"', async () => {
  const { sandbox, els } = run(['loadRadioStatus'], {}, {
    '/api/admin/lastfm/radio': { enabled: true, hasApiKey: true, users: [], learning: { plays: 5, needed: 0 }, stations: [{ name: 'Mix', kind: 'artist' }, { name: 'B', kind: 'x', trackCount: 12 }] },
  });
  await sandbox.loadRadioStatus();
  const html = els['radio-status'].innerHTML;
  assert.match(html, /<span>artist<\/span>/);
  assert.match(html, /x · 12 tracks/);
  assert.doesNotMatch(html, /undefined|NaN/);
});

test('Family: members null still draws the card', async () => {
  const { sandbox, els } = run(['loadFamily'], { familyRoles: {}, familyEl: (tag, props, ...kids) => ({ tag, kids: kids.flat() }), refreshStatus() {}, confirm: () => true },
    { '/api/admin/family': { enabled: true, members: null, devices: 0, pendingRequests: 0, liveStreams: 0 } });
  els['family-card'] = { replaceChildren(...c) { this.children = c; } };
  await sandbox.loadFamily();
  assert.ok(els['family-card'].children.length > 0);
  assert.doesNotMatch(JSON.stringify(els['family-card'].children), /could not read/);
});

test('Family: a non-object answer shows the error line', async () => {
  const { sandbox, els } = run(['loadFamily'], { familyEl: (tag, props, ...kids) => ({ tag, kids: kids.flat() }) }, { '/api/admin/family': null });
  els['family-card'] = { replaceChildren(...c) { this.children = c; } };
  await sandbox.loadFamily();
  assert.match(JSON.stringify(els['family-card'].children), /could not read the family/);
});

test('animated art wall: an answer without albums changes nothing and does not throw', async () => {
  const { sandbox, els } = run(['loadArtWall'], { artShown: 0, ART_PAGE: 24, artTile: () => '' }, { '/api/admin/animated-art/albums': {} });
  await sandbox.loadArtWall(true);
  assert.equal(els['art-wall'].innerHTML, '');
});

test('Fetched row: title and by line carry the full text in a title', () => {
  const { sandbox } = run(['fetchedRow'], { findArt: () => '', fetchedLog: () => '', fetchedLogOpen: () => false });
  const html = sandbox.fetchedRow({ title: 'A very long title', artist: 'Some Artist', album: 'Album', path: '/x', source: 'soulseek' });
  assert.match(html, /class="dl-title" title="A very long title"/);
  assert.match(html, /class="dl-by" title="Some Artist · Album"/);
});

// ── Listening ────────────────────────────────────────────────────────────────────────
function mountListening() {
  const html = [];
  const target = { clientWidth: 900, get innerHTML() { return html.join('|'); }, set innerHTML(v) { html.push(String(v)); } };
  const holder = new Proxy(target, { get: (t, k) => (k in t ? t[k] : loose()), set: (t, k, v) => { t[k] = v; return true; } });
  const page = loose();
  page.querySelector = sel => (sel === '.ls-body' ? holder : loose());
  const sandbox = {
    document: { createElement: () => page, querySelector: () => null, querySelectorAll: () => [], documentElement: loose(), fonts: null, body: loose(), getElementById: () => loose() },
    window: { innerWidth: 1200, addEventListener() {}, removeEventListener() {}, matchMedia: () => ({ matches: true }) },
    requestAnimationFrame: f => f(), setTimeout: f => f(), clearTimeout() {}, localStorage: { getItem: () => null, setItem() {} },
    console, Intl, Date, Math,
  };
  vm.createContext(sandbox);
  vm.runInContext(listening, sandbox);
  return { sandbox, root: { replaceChildren() {} }, html };
}

test('Listening: a rollup without change shows the error state, not "NaN"', async () => {
  const { sandbox, root, html } = mountListening();
  const rollup = { days: 30, hours: 1, plays: 3, uniqueSongs: 1, artists: 1, libraryShare: 1, topArtists: [], topSongs: [], genres: [], apps: [], hearted: [], grid: [], perDay: [] };
  let threw = null;
  try {
    sandbox.window.FlakyfyListening.mount(root, { fetch: async () => ({ ok: true, status: 200, data: { rollup, ranges: [7, 30, 90] } }), cover: () => '', canSeeEveryone: false });
    await new Promise(r => setImmediate(r));
  } catch (e) { threw = e; }
  assert.equal(threw, null);
  const out = html.at(-1);
  assert.match(out, /Listening didn&#39;t load|Listening didn't load/);
  assert.doesNotMatch(out, /NaN|Last undefined/);
});
