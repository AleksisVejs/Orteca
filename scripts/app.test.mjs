import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';
import { ref } from 'vue';

function app(api = {}) {
  const source = readFileSync(new URL('../src/App.vue', import.meta.url), 'utf8')
    .match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1]
    .replace(/^import .*;$/gm, '');
  const js = ts.transpile(source, { target: ts.ScriptTarget.ES2022 });
  return vm.runInNewContext(`(async () => { ${js}; return { open, confirmTrust, close, opened, pendingTrust, openError }; })()`, {
    ref,
    isAppError: e => !!e?.message,
    ...api,
  });
}

const project = { project: { path: 'C:/repo', trusted: false }, trustFindings: [{}] };

test('untrusted findings require successful consent', async () => {
  const state = await app({ openProject: async () => structuredClone(project), trustProject: async () => {} });
  await state.open('C:/repo');
  assert.equal(state.opened.value, null);
  await state.confirmTrust();
  assert.equal(state.opened.value.project.trusted, true);
});

test('a new repository requires consent even without recognized configuration', async () => {
  const result = structuredClone(project);
  result.trustFindings = [];
  const state = await app({ openProject: async () => result });
  await state.open('C:/repo');
  assert.equal(state.opened.value, null);
  assert.deepEqual(state.pendingTrust.value, result);
});

test('remembered consent opens the project', async () => {
  const result = structuredClone(project);
  result.project.trusted = true;
  const state = await app({ openProject: async () => result });
  await state.open('C:/repo');
  assert.equal(state.pendingTrust.value, null);
  assert.equal(state.opened.value.project.path, 'C:/repo');
});

test('a late open cannot replace a newer pending trust decision', async () => {
  let finish;
  const state = await app({ openProject: path => path === 'old'
    ? new Promise(resolve => { finish = resolve; })
    : Promise.resolve(structuredClone(project)) });
  const old = state.open('old');
  await state.open('new');
  finish({ project: { path: 'old', trusted: true }, trustFindings: [] });
  await old;
  assert.equal(state.opened.value, null);
  assert.equal(state.pendingTrust.value.project.path, 'C:/repo');
});

test('failed consent stays closed and exposes the error', async () => {
  const state = await app({ openProject: async () => structuredClone(project), trustProject: async () => { throw new Error('database unavailable'); } });
  await state.open('C:/repo');
  await state.confirmTrust();
  assert.equal(state.opened.value, null);
  assert.equal(state.openError.value, 'database unavailable');
});

test('cancelling pending consent does not reopen on late completion', async () => {
  let finish;
  const state = await app({ openProject: async () => structuredClone(project), trustProject: () => new Promise(resolve => { finish = resolve; }) });
  await state.open('C:/repo');
  const confirmation = state.confirmTrust();
  state.pendingTrust.value = null;
  finish();
  await confirmation;
  assert.equal(state.opened.value, null);
});

test('provider detection runs off the synchronous invoke handler', () => {
  const source = readFileSync(new URL('../src-tauri/src/main.rs', import.meta.url), 'utf8');
  assert.match(source, /#\[tauri::command\(async\)\]\s+fn detect_providers/);
});

test('provider detection failure is visible instead of looking like no providers', () => {
  const source = readFileSync(new URL('../src/views/Project.vue', import.meta.url), 'utf8');
  assert.doesNotMatch(source, /detectProviders\(\)\.catch\(\(\) => \[\]\)/);
  assert.match(source, /providerError/);
  assert.match(source, /detection unavailable/);
});
