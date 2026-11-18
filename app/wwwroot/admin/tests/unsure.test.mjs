// Runs the pure logic of the Unsure songs page (between `// Unsure songs: pure logic` and
// `// End of the unsure pure logic.` in admin.js): the closeness words, the file/video time
// mapping, the two-step state, which videos remain, and the open/answered split.
//
//   node --test app/wwwroot/admin/tests/unsure.test.mjs

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../admin.js', import.meta.url), 'utf8');
const start = source.indexOf('// Unsure songs: pure logic');
const end = source.indexOf('// End of the unsure pure logic.');
assert.ok(start > 0 && end > start, 'the unsure pure logic is in admin.js');
const names = ['unsureCloseness', 'unsureRemaining', 'unsureIsOpen', 'unsureSplit', 'unsureStart', 'unsureVideoTime',
  'unsureFileTime', 'unsureStepNext', 'unsureShiftVideo', 'unsureClock', 'unsureSpoken'];
const context = vm.createContext({});
vm.runInContext(`${source.slice(start, end)}\nthis.api = { ${names.join(', ')} };`, context);
const u = context.api;
const plain = value => JSON.parse(JSON.stringify(value));

test('closeness is a word, never a verdict, with the edges where the old page had them', () => {
  assert.equal(u.unsureCloseness(0), 'sounds alike');
  assert.equal(u.unsureCloseness(0.25), 'sounds alike');
  assert.equal(u.unsureCloseness(0.2501), 'sounds somewhat alike');
  assert.equal(u.unsureCloseness(0.3799), 'sounds somewhat alike');
  assert.equal(u.unsureCloseness(0.38), 'sounds different');
  assert.equal(u.unsureCloseness(undefined), '');
  assert.equal(u.unsureCloseness(NaN), '');
});

test('the video plays at the file time plus the offset, clamped to the video', () => {
  assert.deepEqual(plain(u.unsureVideoTime(60, 2.5, 200)), { time: 62.5, clamped: false });
  assert.deepEqual(plain(u.unsureVideoTime(60, -10, 200)), { time: 50, clamped: false });
  assert.deepEqual(plain(u.unsureVideoTime(5, -10, 200)), { time: 0, clamped: true });
  assert.deepEqual(plain(u.unsureVideoTime(195, 10, 200)), { time: 200, clamped: true });
  assert.deepEqual(plain(u.unsureVideoTime(200, 0, 200)), { time: 200, clamped: false });
  // A video of unknown length cannot be clamped at its end.
  assert.deepEqual(plain(u.unsureVideoTime(500, 1, null)), { time: 501, clamped: false });
  assert.deepEqual(plain(u.unsureVideoTime(undefined, undefined, 100)), { time: 0, clamped: false });
});

test('the way back from the video to the file never goes below the start', () => {
  assert.equal(u.unsureFileTime(62.5, 2.5), 60);
  assert.equal(u.unsureFileTime(1, 10), 0);
});

test('a long song starts 45 s in, a short one at the start', () => {
  assert.equal(u.unsureStart(240), 45);
  assert.equal(u.unsureStart(60), 45);
  assert.equal(u.unsureStart(59), 0);
  assert.equal(u.unsureStart(null), 0);
});

test('step state: yes goes to the file question, back returns, another video starts over', () => {
  const first = { step: 1, videoId: 'a' };
  const second = u.unsureStepNext(first, { type: 'yes' });
  assert.deepEqual(plain(second), { step: 2, videoId: 'a' });
  assert.deepEqual(plain(u.unsureStepNext(second, { type: 'back' })), { step: 1, videoId: 'a' });
  assert.deepEqual(plain(u.unsureStepNext(second, { type: 'video', videoId: 'b' })), { step: 1, videoId: 'b' });
  assert.deepEqual(plain(u.unsureStepNext(second, { type: 'nonsense' })), { step: 2, videoId: 'a' });
});

test('which videos remain: turned-down ones are skipped and previous/next stop at the ends', () => {
  const song = { videos: [{ videoId: 'a', rejected: true }, { videoId: 'b' }, { videoId: 'c', rejected: false }, { videoId: 'd' }] };
  const left = u.unsureRemaining(song);
  assert.deepEqual(plain(left.map(v => v.videoId)), ['b', 'c', 'd']);
  assert.equal(u.unsureShiftVideo(left, 'b', 1), 'c');
  assert.equal(u.unsureShiftVideo(left, 'd', 1), 'd');
  assert.equal(u.unsureShiftVideo(left, 'b', -1), 'b');
  assert.equal(u.unsureShiftVideo(left, 'zzz', 1), 'zzz');
  assert.deepEqual(plain(u.unsureRemaining({ videos: [{ videoId: 'a', rejected: true }] })), []);
  assert.deepEqual(plain(u.unsureRemaining({})), []);
});

test('open and answered: can\'t tell stays open and goes last, closest first', () => {
  const song = (id, bitError, decision) => ({ id, videos: [{ videoId: 'v' + id, bitError }], decision });
  const songs = [
    song(1, 0.5, null),
    song(2, 0.1, { label: 'cant_tell' }),
    song(3, 0.3, { label: 'right', by: 'admin' }),
    song(4, 0.2, null),
    song(5, 0.4, { label: 'wrong', by: 'x', rule: 'r1' }),
    { id: 6, videos: [{ videoId: 'x', bitError: 0.1, rejected: true }], decision: null },
  ];
  const { open, answered } = u.unsureSplit(songs);
  assert.deepEqual(plain(open.map(s => s.id)), [4, 1, 6, 2]);
  assert.deepEqual(plain(answered.map(s => s.id)), [3, 5]);
  assert.equal(songs[0].id, 1, 'the input is not reordered');
  assert.deepEqual(plain(u.unsureSplit([])), { open: [], answered: [] });
});

test('the clock and the spoken time', () => {
  assert.equal(u.unsureClock(72), '1:12');
  assert.equal(u.unsureClock(3725), '1:02:05');
  assert.equal(u.unsureClock(-3), '0:00');
  assert.equal(u.unsureSpoken(72), '1 minute 12 seconds');
  assert.equal(u.unsureSpoken(1), '1 second');
  assert.equal(u.unsureSpoken(3600), '1 hour');
  assert.equal(u.unsureSpoken(0), '0 seconds');
});
