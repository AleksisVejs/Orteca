import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

// The schedule alone; the ticker and its API calls are not loaded.
const source = readFileSync(new URL('../src/views/project/anchor.ts', import.meta.url), 'utf8')
  .replace(/^import[\s\S]*?from ["'][^"']+["'];/gm, '')
  .replace(/^export /gm, '');
const { morningPing, continuousPing, due } = vm.runInNewContext(
  `(() => { ${ts.transpile(source, { target: ts.ScriptTarget.ES2022 })}; return { morningPing, continuousPing, due }; })()`,
  { Date, Math, JSON, Number, setInterval: () => 0 },
);

const day = (h, m = 0) => new Date(2026, 8, 24, h, m).getTime();
const a = { mode: 'morning', start: '09:00', end: '18:00', hours: 3, providers: ['claude'] };

test('before work, the ping goes (5 - hours) ahead of the start, and a slept-through one waits a day', () => {
  assert.equal(morningPing(a, day(6)), day(7), 'window resets at 12:00, three hours in');
  assert.equal(morningPing(a, day(7, 5)), day(7), 'five minutes late still goes');
  assert.equal(morningPing(a, day(8)), day(7) + 24 * 3600_000, 'an hour late waits for tomorrow');
});

test('after each reset, only inside work hours and not late', () => {
  assert.equal(continuousPing(a, day(12), day(12)), day(12, 1));
  assert.equal(continuousPing(a, day(20), day(20)), null, 'after work');
  assert.equal(continuousPing(a, day(13), day(12)), null, 'the machine slept through it');
});

test('a ping waits for runs and goes once', () => {
  const idle = { running: 0, lastEnd: 0 };
  assert.ok(due(day(7), day(7, 1), 0, idle));
  assert.ok(!due(day(7), day(7, 1), day(7), idle), 'already sent');
  assert.ok(!due(day(7), day(7, 1), 0, { running: 1, lastEnd: 0 }), 'a task is running');
  assert.ok(!due(day(7), day(7, 1), 0, { running: 0, lastEnd: day(6, 50) }), 'a task just ran');
  assert.ok(!due(day(7), day(6, 59), 0, idle), 'not yet');
});
