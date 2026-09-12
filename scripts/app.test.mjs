import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';
import { ref, computed } from 'vue';

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

async function projectView(api = {}) {
  const source = readFileSync(new URL('../src/views/Project.vue', import.meta.url), 'utf8')
    .match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1]
    .replace(/^import[\s\S]*?from ["'][^"']+["'];/gm, '');
  let mounted;
  const listeners = {};
  const state = vm.runInNewContext(`(() => { ${ts.transpile(source, { target: ts.ScriptTarget.ES2022 })}; return { run, stopRun, stopping, taskId, task, running, result, runError, tokens, lines, providerError, install, installing, installError, signIn, signingIn, signInError, canRun, providers, formatCost: typeof formatCost === 'function' ? formatCost : n => '$' + n.toFixed(4) }; })()`, {
    ref, computed,
    defineProps: () => ({ opened: project }), defineEmits: () => () => {},
    onMounted: fn => { mounted = fn; }, onUnmounted: () => {},
    detectProviders: async () => [{ id: 'codex', path: 'fake.exe' }],
    onTaskEvent: async fn => { listeners.event = fn; return () => {}; },
    onTaskDone: async fn => { listeners.done = fn; return () => {}; },
    onInstallEvent: async () => () => {},
    onSignInEvent: async () => () => {},
    cancelTask: async () => {},
    isAppError: e => !!e?.message,
    ...api,
  });
  await mounted();
  state.task.value = 'test';
  return { state, listeners };
}

const finished = { taskId: 1, status: 'failed', failure: 'spawn failed', summary: '', usage: null, diff: [], dirtyAtStart: false };

test('completion before invoke resolves never leaves the screen running', async () => {
  const { state, listeners } = await projectView({ startTask: async (...args) => {
    const progress = args.find(arg => typeof arg === 'function');
    if (progress) { progress({ kind: 'text', data: 'early' }); return finished; }
    listeners.event(1, { kind: 'text', data: 'early' });
    listeners.done(finished);
    return 1;
  } });
  await state.run();
  assert.equal(state.running.value, false);
  assert.equal(state.result.value.failure, 'spawn failed');
  assert.equal(state.lines.value[0].text, 'early');
  assert.equal(state.tokens.value, null);
});

test('activity is bounded even when a provider emits long messages', async () => {
  const { state, listeners } = await projectView({ startTask: async (...args) => {
    const progress = args.find(arg => typeof arg === 'function');
    for (let i = 0; i < 1200; i++) (progress ?? (event => listeners.event(1, event)))({ kind: 'text', data: 'x'.repeat(20000) });
    return progress ? finished : 1;
  } });
  await state.run();
  assert.ok(state.lines.value.length > 0 && state.lines.value.length <= 500);
  assert.ok(state.lines.value.every(line => line.text.length <= 4001));
});

test('detection errors and invoke failures are exposed', async () => {
  const { state } = await projectView({ detectProviders: async () => { throw Error('detection failed'); } });
  assert.equal(state.providerError.value, true);
  const { state: ready } = await projectView({ startTask: async () => { throw Error('database unavailable'); } });
  await ready.run();
  assert.equal(ready.running.value, false);
  assert.equal(ready.runError.value, 'database unavailable');
});

test('a positive sub-cent cost never renders as zero', async () => {
  const { state } = await projectView();
  assert.equal(state.formatCost(0.00004), '<$0.0001');
  assert.equal(state.formatCost(0), '$0.0000');
  assert.equal(state.formatCost(0.01234), '$0.0123');
});

test('double clicks cannot enter two install loops and failures stay visible', async () => {
  let reject;
  let calls = 0;
  const { state } = await projectView({ installProvider: () => { calls++; return new Promise((_, no) => { reject = no; }); } });
  const first = state.install(['codex']);
  const second = state.install(['codex']);
  assert.equal(calls, 1);
  reject(Error('npm denied access'));
  await Promise.all([first, second]);
  assert.equal(state.installError.value, 'npm denied access');
  assert.equal(state.installing.value, null);
});

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


test('a run becomes stoppable as soon as it has an id, and a stop is not a failure', async () => {
  let stopped = null;
  let release;
  const { state } = await projectView({
    cancelTask: async id => {
      stopped = id;
      release({ ...finished, taskId: 7, status: 'cancelled', failure: null, summary: 'half an answer' });
    },
    startTask: async (...args) => {
      const [onEvent, onTask] = args.filter(arg => typeof arg === 'function');
      onEvent({ kind: 'text', data: 'half an answer' });
      onTask(7);
      return new Promise(resolve => { release = resolve; });
    },
  });

  const running = state.run();
  assert.equal(state.taskId.value, 7, 'Stop has nothing to name until the id arrives');
  assert.equal(state.running.value, true);

  await state.stopRun();
  assert.equal(stopped, 7);
  await running;

  assert.equal(state.result.value.status, 'cancelled');
  assert.equal(state.result.value.summary, 'half an answer', 'what the agent said is kept');
  assert.equal(state.running.value, false);
  assert.equal(state.taskId.value, null, 'a finished run must not stay stoppable');
});

test('a stop that lands after the run ended never becomes a run error', async () => {
  let release;
  const { state } = await projectView({
    cancelTask: async () => { throw Error('That run has already finished.'); },
    startTask: async (...args) => {
      args.filter(arg => typeof arg === 'function')[1](9);
      return new Promise(resolve => { release = resolve; });
    },
  });

  const running = state.run();
  await state.stopRun();
  assert.equal(state.runError.value, null, 'a stop that arrived too late is not an error to report');

  release({ ...finished, status: 'done', failure: null, summary: 'finished anyway' });
  await running;
  assert.equal(state.result.value.status, 'done', 'a late stop must not rewrite the real outcome');
});

test('a signed-out CLI blocks the run and offers sign-in instead', async () => {
  // The bug this guards: auth used to be guessed from a credential file that
  // exists for users who have never signed in, so Run was enabled for a run
  // that could only fail. Detection now reports the CLI's own answer.
  const { state } = await projectView({
    detectProviders: async () => [{ id: 'codex', path: 'fake.exe', auth: 'signedOut' }],
  });
  assert.equal(state.canRun.value, false, 'signed out must not be runnable');

  // "unknown" means the CLI could not be asked, which is not a reason to block.
  const asked = await projectView({
    detectProviders: async () => [{ id: 'codex', path: 'fake.exe', auth: 'unknown' }],
  });
  assert.equal(asked.state.canRun.value, true);
});

test('sign-in failure stays visible and never claims a login', async () => {
  const { state } = await projectView({
    detectProviders: async () => [{ id: 'codex', path: 'fake.exe', auth: 'signedOut' }],
    signInProvider: async () => { throw Error('codex is still signed out'); },
  });
  await state.signIn('codex');
  assert.equal(state.signingIn.value, null);
  assert.match(state.signInError.value, /still signed out/);
  assert.equal(state.providers.value[0].auth, 'signedOut', 'a failed sign-in must not upgrade auth');
  assert.equal(state.canRun.value, false);
});
