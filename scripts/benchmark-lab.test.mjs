import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { makePlan, markdownReport, validateCorpus } from './bench/lab.mjs';
import { weeklyHeadroomStop } from './bench/weekly-limit.mjs';

const corpus = JSON.parse(readFileSync(new URL('../benchmarks/corpus.json', import.meta.url), 'utf8'));

test('the corpus supplies every planned class and a clean security control', () => {
  assert.deepEqual(validateCorpus(corpus), []);
  assert.ok(corpus.tasks.length >= 20);
});

test('the run plan is deterministic and randomizes a fair paired arm order', () => {
  const tasks = corpus.tasks.filter(task => task.id === 'liftme-cancel-order');
  assert.deepEqual(makePlan(tasks, ['claude', 'codex'], 3, 9), makePlan(tasks, ['claude', 'codex'], 3, 9));
  assert.equal(makePlan(tasks, ['claude', 'codex'], 3, 9).length, 6);
});

test('reports preserve separate correctness and timing evidence', () => {
  const report = markdownReport([
    { taskId: 'sample', arm: 'claude', publicPass: true, hiddenPass: true, wallMs: 10_000, uncachedTokens: 20, calls: 1, localVerify: 'pass', reviewOutcome: 'found' },
    { taskId: 'sample', arm: 'claude', publicPass: true, hiddenPass: false, wallMs: 14_000, uncachedTokens: 24, calls: 2, localVerify: 'fail', reviewOutcome: 'none' },
  ]);
  assert.match(report, /1\/2/);
  assert.match(report, /12s/);
  assert.match(report, /95% CI/);
});

test('weekly preflight preserves a floor plus one conservative arm reserve', () => {
  assert.equal(weeklyHeadroomStop({ 'codex week': 80 }, new Set(['codex'])), null);
  assert.match(weeklyHeadroomStop({ 'codex week': 81 }, new Set(['codex'])), /19% remaining; need 20%/);
  assert.equal(weeklyHeadroomStop({ 'codex 5-hour': 99 }, new Set(['codex'])), null);
});
