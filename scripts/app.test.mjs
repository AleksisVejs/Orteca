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

test('run history loads on open, refreshes after a run, and never shows a zero for unknown tokens', async () => {
  const past = { id: 1, prompt: 'old', status: 'failed', startedAt: '2026-09-12 10:00:00', summary: null, routeKind: 'implementOnce', callsUsed: 1, provider: 'codex', tokens: null, cachedTokens: null, costUsd: null, costQuality: 'unavailable' };
  let asked = 0;
  const { state } = await projectView({
    recentTasks: async () => (++asked === 1 ? [past] : [{ ...past, id: 2, status: 'done', tokens: 1200, costUsd: 0.01, costQuality: 'estimated' }, past]),
    startTask: async () => ({ ...finished, status: 'done', failure: null }),
  });
  await new Promise(r => setTimeout(r));
  assert.equal(state.historyLine(state.history.value[0]), 'implementOnce · 1 call · tokens unavailable · 2026-09-12 10:00 UTC');
  await state.run();
  assert.equal(state.history.value.length, 2);
  assert.match(state.historyLine(state.history.value[0]), /1,200 tokens · \$0\.0100 estimated/);

  const broken = await projectView({ recentTasks: async () => { throw new Error('db'); } });
  await new Promise(r => setTimeout(r));
  assert.equal(broken.state.historyError.value, true);
});

async function projectView(api = {}) {
  const source = readFileSync(new URL('../src/views/Project.vue', import.meta.url), 'utf8')
    .match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1]
    .replace(/^import[\s\S]*?from ["'][^"']+["'];/gm, '');
  let mounted;
  const listeners = {};
  const state = vm.runInNewContext(`(() => { ${ts.transpile(source, { target: ts.ScriptTarget.ES2022 })}; return { run, stopRun, stopping, taskId, instruct, instruction, sending, instructionError, steering, task, running, result, runError, tokens, lines, currentActivity, activityFor, friendlyToolUse, providerError, install, installing, installError, signIn, signingIn, signInError, canRun, providers, mode, calls, changed, routeSteps, comparison, OUTCOME, history, historyError, historyLine, provider, providerPicked, pickedFor, limits, limitLine, limitWarning, preview, pickByHeadroom, alternative, fallback, continueWith, switchTo, isolation, removeCopy, confirmRemove, removedCopies, removeError, formatCost: typeof formatCost === 'function' ? formatCost : n => '$' + n.toFixed(4) }; })()`, {
    ref, computed, setTimeout, clearTimeout,
    defineProps: () => ({ opened: project }), defineEmits: () => () => {},
    onMounted: fn => { mounted = fn; }, onUnmounted: () => {},
    detectProviders: async () => [{ id: 'codex', path: 'fake.exe' }],
    onTaskEvent: async fn => { listeners.event = fn; return () => {}; },
    onTaskDone: async fn => { listeners.done = fn; return () => {}; },
    onInstallEvent: async () => () => {},
    onSignInEvent: async () => () => {},
    cancelTask: async () => {},
    recentTasks: async () => [],
    sendInstruction: async () => ({ disposition: 'live' }),
    isAppError: e => !!e?.message,
    ...api,
  });
  await mounted();
  state.task.value = 'test';
  return { state, listeners };
}

const oneCall = {
  kind: 'implementOnce', mode: 'balanced', stages: ['implement'], reason: 'small, low-risk and narrow',
  budget: { preferredTier: 'cheapest' },
  signals: { complexity: 0, risk: 0, blastRadius: 1 }, candidatePaths: [], preferredProviders: ['codex'],
};

const finished = { taskId: 1, status: 'failed', failure: 'spawn failed', summary: '', usage: null, diff: [], dirtyAtStart: false, route: oneCall, stages: [], callsUsed: 1, turnsUsed: 1, budgetStop: null, baseline: null };

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

test('provider actions become plain-English live updates', async () => {
  const { state } = await projectView();
  assert.equal(state.friendlyToolUse('Read', 'src/App.vue'), 'Reading src/App.vue');
  assert.equal(state.friendlyToolUse('Edit', 'src/App.vue'), 'Editing src/App.vue');
  assert.equal(state.friendlyToolUse('Shell', 'npm test'), 'Checking that it works');
  assert.equal(state.activityFor({ kind: 'text', data: 'I found the issue.' }), 'Thinking through the request');
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

test('an instruction is shown as the user’s own words and clears the box', async () => {
  let sent = null;
  let release;
  const { state } = await projectView({
    detectProviders: async () => [{ id: 'claude', path: 'fake.exe', steering: 'live' }],
    sendInstruction: async (taskId, text, applyNow) => {
      sent = { taskId, text, applyNow };
      return { disposition: 'live' };
    },
    startTask: async (...args) => {
      const [onEvent, onTask] = args.filter(arg => typeof arg === 'function');
      onTask(4);
      onEvent({ kind: 'text', data: 'working on it' });
      return new Promise(resolve => { release = resolve; });
    },
  });

  const running = state.run();
  assert.equal(state.steering.value, 'live', 'the card must say how this CLI takes it');
  state.instruction.value = '  use tabs  ';
  await state.instruct(false);

  assert.deepEqual(sent, { taskId: 4, text: 'use tabs', applyNow: false });
  assert.equal(state.instruction.value, '', 'a sent instruction must not sit in the box');
  const mine = state.lines.value.filter(l => l.kind === 'instruction');
  assert.equal(mine.length, 1);
  assert.equal(mine[0].text, 'use tabs');

  release({ ...finished, status: 'done', failure: null, summary: 'done' });
  await running;
});

test('a too-late instruction stays in the box and is not shown as delivered', async () => {
  let release;
  const { state } = await projectView({
    detectProviders: async () => [{ id: 'claude', path: 'fake.exe', steering: 'live' }],
    sendInstruction: async () => ({ disposition: 'tooLate' }),
    startTask: async (...args) => {
      args.filter(arg => typeof arg === 'function')[1](6);
      return new Promise(resolve => { release = resolve; });
    },
  });

  const running = state.run();
  state.instruction.value = 'keep this request';
  await state.instruct(false);

  assert.match(state.instructionError.value, /finished before/);
  assert.equal(state.instruction.value, 'keep this request');
  assert.equal(state.lines.value.filter(l => l.kind === 'instruction').length, 0);

  release({ ...finished, status: 'done', failure: null });
  await running;
});

test('a checkpoint provider can apply now, and a refused instruction is not shown as sent', async () => {
  let applyNow = null;
  let release;
  const { state } = await projectView({
    detectProviders: async () => [{ id: 'codex', path: 'fake.exe', steering: 'checkpoint' }],
    sendInstruction: async (_id, _text, now) => {
      applyNow = now;
      throw Error('That run has already finished.');
    },
    startTask: async (...args) => {
      args.filter(arg => typeof arg === 'function')[1](5);
      return new Promise(resolve => { release = resolve; });
    },
  });

  const running = state.run();
  assert.equal(state.steering.value, 'checkpoint');
  state.instruction.value = 'make it faster';
  await state.instruct(true);

  assert.equal(applyNow, true);
  assert.match(state.instructionError.value, /already finished/);
  assert.equal(state.lines.value.filter(l => l.kind === 'instruction').length, 0,
    'a refused instruction must not appear as if it landed');
  assert.equal(state.instruction.value, 'make it faster', 'the words must not be thrown away');
  assert.equal(state.sending.value, false);

  release({ ...finished, status: 'done', failure: null });
  await running;
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

test('the chosen mode reaches the backend, which is what picks the route', async () => {
  let sent = null;
  const { state } = await projectView({
    startTask: async (path, prompt, provider, mode) => {
      sent = { provider, mode };
      return { ...finished, status: 'done', failure: null, summary: 'done' };
    },
  });

  // Balanced by default: the mode that routes as the architecture wrote it.
  assert.equal(state.mode.value, 'balanced');
  await state.run();
  assert.equal(sent.mode, 'balanced');

  state.mode.value = 'efficient';
  await state.run();
  assert.equal(sent.mode, 'efficient', 'the mode the user picked never reached the router');
});

test('a budget stop reads as its own outcome and never as a failure', async () => {
  const stopped = {
    ...finished,
    status: 'budgetReached',
    failure: null,
    summary: 'got as far as the edit',
    diff: [{ path: 'src/a.rs', added: 3, deleted: 1 }],
    usage: { inputTokens: 10, cachedInputTokens: 0, outputTokens: 2, reasoningTokens: 0, costUsd: null, costQuality: 'unavailable' },
    route: { ...oneCall, kind: 'planned', stages: ['plan', 'implement', 'verify'] },
    stages: [{ stage: 'plan', summary: 'planned', artifact: null }, { stage: 'implement', summary: 'got as far as the edit', artifact: null }],
    callsUsed: 2,
    budgetStop: { limit: 'calls', allowed: 2, observed: 2, remaining: ['verify'], message: 'This route was given 2 agent calls, and they are used up.' },
  };
  const { state } = await projectView({ startTask: async () => stopped });
  await state.run();

  assert.equal(state.result.value.status, 'budgetReached');
  assert.equal(state.OUTCOME.budgetReached, 'Stopped safely', 'a budget stop must not be labelled a failure');
  assert.equal(state.result.value.failure, null);
  // The work survives the stop, and is what the screen has to show.
  assert.equal(state.result.value.summary, 'got as far as the edit');
  assert.equal(state.result.value.diff.length, 1);
  assert.equal(state.tokens.value.total, 12);
  // And the spend is shown, exact.
  // Through JSON: a computed hands back reactive proxies, which compare by
  // reference rather than by what they hold.
  assert.deepEqual(JSON.parse(JSON.stringify(state.calls.value)), { used: 2, stages: ['plan', 'implement', 'verify'], ran: ['plan', 'implement'] });
  assert.deepEqual(state.result.value.budgetStop.remaining, ['verify'], 'the user is not told what was left undone');
});

test('a trivial task reports the one call it spent', async () => {
  const { state } = await projectView({
    startTask: async () => ({ ...finished, status: 'done', failure: null, summary: 'fixed it', stages: [{ stage: 'implement', summary: 'fixed it', artifact: null }] }),
  });
  await state.run();
  assert.deepEqual(JSON.parse(JSON.stringify(state.calls.value)), { used: 1, stages: ['implement'], ran: ['implement'] });
  assert.equal(state.result.value.budgetStop, null);
});

test('a file that was already dirty is never counted as the run\'s work', async () => {
  const { state } = await projectView({
    startTask: async () => ({
      ...finished, status: 'done', failure: null, summary: 'fixed it', dirtyAtStart: true,
      diff: [
        { path: 'src/slug.js', added: 1, deleted: 1, origin: 'run' },
        { path: 'notes.md', added: 3, deleted: 0, origin: 'both' },
        { path: 'stray.txt', added: null, deleted: null, origin: 'beforeRun' },
      ],
    }),
  });
  await state.run();
  assert.deepEqual(state.changed.value.byRun.map(f => f.path), ['src/slug.js', 'notes.md']);
  assert.deepEqual(state.changed.value.beforeRun.map(f => f.path), ['stray.txt']);
  assert.equal(state.changed.value.unknown, false, 'a snapshotted diff must not fall back to the vague caveat');
});

test('with no snapshot the screen says it cannot tell, and claims nothing', async () => {
  const { state } = await projectView({
    startTask: async () => ({
      ...finished, status: 'done', failure: null, summary: 'fixed it', dirtyAtStart: true,
      diff: [{ path: 'stray.txt', added: null, deleted: null, origin: null }],
    }),
  });
  await state.run();
  assert.equal(state.changed.value.unknown, true);
  assert.equal(state.changed.value.beforeRun.length, 0);
});

test('the route shows what ran, and a saving is only claimed against a baseline', async () => {
  const usage = { inputTokens: 60, cachedInputTokens: 500, outputTokens: 15, reasoningTokens: 0, costUsd: null, costQuality: 'unavailable' };
  const done = {
    ...finished, status: 'done', failure: null, summary: 'ok', usage,
    route: { ...oneCall, kind: 'planned', stages: ['plan', 'implement', 'verify'] },
    stages: [{ stage: 'plan', summary: '', artifact: null }, { stage: 'implement', summary: 'ok', artifact: null }],
  };
  const { state } = await projectView({ startTask: async () => done });
  await state.run();
  assert.deepEqual(JSON.parse(JSON.stringify(state.routeSteps.value)), [
    { stage: 'plan', ran: true }, { stage: 'implement', ran: true }, { stage: 'verify', ran: false },
  ]);
  assert.equal(state.comparison.value, null, 'no baseline, no comparison');

  const measured = await projectView({ startTask: async () => ({ ...done, baseline: { runs: 5, medianTokens: 100, medianCalls: 3 } }) });
  await measured.state.run();
  assert.equal(measured.state.comparison.value.change, 25, 'cache reads are not counted against the baseline');

  const stopped = await projectView({ startTask: async () => ({ ...done, status: 'budgetReached', baseline: { runs: 5, medianTokens: 100, medianCalls: 3 } }) });
  await stopped.state.run();
  assert.equal(stopped.state.comparison.value, null, 'an unfinished run is not a saving');
});

const bothInstalled = async () => [{ id: 'claude', path: 'c.exe', auth: 'subscription' }, { id: 'codex', path: 'x.exe', auth: 'subscription' }];
const reading = (claude, codex) => async () => [
  { id: 'claude', windows: [{ label: 'session', usedPercent: claude, resetsAt: null, resetsText: 'Sep 13, 3:50pm' }], unavailable: null },
  codex === null
    ? { id: 'codex', windows: [], unavailable: 'codex app-server exited' }
    : { id: 'codex', windows: [{ label: '5-hour', usedPercent: codex, resetsAt: 1789304323, resetsText: null }], unavailable: null },
];
const settle = () => new Promise(r => setTimeout(r));

test('the provider with the most limit left is picked, until the user picks one', async () => {
  const { state } = await projectView({ detectProviders: bothInstalled, providerLimits: reading(35, 90) });
  await settle();
  assert.equal(state.provider.value, 'claude', 'codex has 10% left, claude 65%');
  assert.match(state.pickedFor.value, /claude 65%, codex 10%/);

  state.providerPicked.value = true;
  state.provider.value = 'codex';
  state.limits.value = await reading(95, 10)();
  state.pickByHeadroom();
  assert.equal(state.provider.value, 'codex', 'a choice the user made is never overridden');
});

test('an unread limit is not an empty one, and says why', async () => {
  const { state } = await projectView({ detectProviders: bothInstalled, providerLimits: reading(95, null) });
  await settle();
  assert.equal(state.provider.value, 'codex', 'no switch on a missing reading');
  assert.equal(state.limitLine('codex'), 'limits unavailable: codex app-server exited');
  assert.equal(state.limitLine('claude'), 'session 95% used');
});

test('a route that may not fit in the fullest window warns before it starts', async () => {
  const { state } = await projectView({ detectProviders: bothInstalled, providerLimits: reading(20, 88) });
  await settle();
  const threeStages = { ...oneCall, stages: ['implement', 'verify', 'review'] };
  state.preview.value = { provider: 'codex', route: threeStages };
  assert.equal(state.limitWarning.value.calls, 3, 'every stage is a call');
  assert.equal(state.limitWarning.value.window.label, '5-hour');
  state.preview.value = { provider: 'claude', route: threeStages };
  assert.equal(state.limitWarning.value, null, '80% left covers three calls');
  state.preview.value = { provider: 'codex', route: oneCall };
  assert.equal(state.limitWarning.value, null, '12% left covers one call');

  state.preview.value = { provider: 'codex', route: threeStages };
  assert.equal(state.alternative.value, 'claude', 'claude has 80% left against codex 12%');
  state.switchTo('claude');
  assert.equal(state.provider.value, 'claude');
  assert.equal(state.providerPicked.value, true, 'switching is the user choosing');
});

test('a run that ran out of plan usage offers the other CLI, and continuing is a fresh run there', async () => {
  const sent = [];
  const { state } = await projectView({
    detectProviders: bothInstalled,
    providerLimits: reading(100, 30),
    startTask: async (path, prompt, provider, mode, headroom) => {
      sent.push({ provider, headroom });
      return sent.length === 1
        ? { ...finished, status: 'failed', failure: "Claude's session usage limit is used up.", failureKind: 'usageLimit' }
        : { ...finished, status: 'done', failure: null, failureKind: null };
    },
  });
  await settle();
  state.providerPicked.value = true;
  state.provider.value = 'claude';
  await state.run();
  assert.deepEqual(JSON.parse(JSON.stringify(state.fallback.value)), { id: 'codex', room: 70 });
  assert.equal(sent.length, 1, 'offered, never taken on the user’s behalf');

  await state.continueWith('codex');
  assert.deepEqual(sent, [{ provider: 'claude', headroom: 0 }, { provider: 'codex', headroom: 70 }]);
  assert.equal(state.fallback.value, null);

  state.result.value = { ...finished, status: 'failed', failureKind: 'crashed' };
  assert.equal(state.fallback.value, null, 'a crash is not a reason to switch');
});

test('a run in a separate copy asks for one, and removing the copy takes a second click', async () => {
  const sent = [];
  const removed = [];
  const worktree = { path: 'C:/.orteca-worktrees/repo-1', branch: 'orteca/task-1', commit: 'abcdef123', commitError: null };
  const { state } = await projectView({
    detectProviders: bothInstalled,
    providerLimits: reading(100, 30),
    startTask: async (path, prompt, provider, mode, headroom, isolation) => {
      sent.push(isolation);
      return { ...finished, status: 'failed', failure: 'used up', failureKind: 'usageLimit', worktree };
    },
    removeWorktree: async (path, taskId) => { removed.push(taskId); },
  });
  await settle();
  state.providerPicked.value = true;
  state.provider.value = 'claude';
  state.isolation.value = 'worktree';
  await state.run();
  assert.deepEqual(sent, ['worktree']);
  assert.equal(state.fallback.value, null, 'a fresh run would start from the commit without the copy’s work');

  await state.removeCopy(1);
  assert.deepEqual(removed, [], 'the first click only asks');
  await state.removeCopy(1);
  assert.deepEqual(removed, [1]);
  assert.deepEqual([...state.removedCopies.value], [1]);
  assert.equal(state.confirmRemove.value, null);
});
