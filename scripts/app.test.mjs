import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';
import { ref, computed, reactive } from 'vue';

function app(api = {}) {
  const source = readFileSync(new URL('../src/App.vue', import.meta.url), 'utf8')
    .match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1]
    .replace(/^import .*;$/gm, '');
  const js = ts.transpile(source, { target: ts.ScriptTarget.ES2022 });
  return vm.runInNewContext(`(async () => { ${js}; return { open, confirmTrust, close, closeProject, projects, activePath, busy, openProjects, pendingTrust, openError }; })()`, {
    ref,
    computed,
    isAppError: e => !!e?.message,
    ...api,
  });
}

const gitState = { isRepo: true, root: 'C:/repo', branch: 'main', head: 'abc123', dirty: false, dirtyCount: 0, upstream: 'origin/main', ahead: 0, behind: 0, branches: ['feature'] };
const project = { project: { path: 'C:/repo', trusted: false }, git: gitState, trustFindings: [{}] };

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

test('task titles can be renamed and deletion takes confirmation', async () => {
  const past = { id: 1, title: 'Generated title', prompt: 'long original prompt', status: 'done' };
  let renamed;
  let deleted = 0;
  const { state } = await projectView({
    recentTasks: async () => [past],
    renameTask: async (_path, id, title) => { renamed = { id, title }; },
    deleteTask: async () => { deleted += 1; },
  });
  await new Promise(r => setTimeout(r));
  state.historyDetail.value = { id: past.id, status: 'done' };

  assert.equal(state.taskName(past), 'Generated title');
  state.startRename(past);
  state.renaming.value = '  Better title  ';
  await state.saveRename();
  assert.deepEqual(renamed, { id: 1, title: 'Better title' });
  assert.equal(past.title, 'Better title');

  await state.removeTask();
  assert.equal(deleted, 0, 'nothing is deleted before the user asks');
  state.askDelete(past);
  assert.equal(state.deleteAsk.value.id, past.id);
  await state.removeTask();
  assert.equal(deleted, 1);
  assert.equal(state.history.value.length, 0);
  assert.equal(state.historyDetail.value, null);
});

async function projectView(api = {}) {
  const source = readFileSync(new URL('../src/views/project/state.ts', import.meta.url), 'utf8')
    .replace(/^import[\s\S]*?from ["'][^"']+["'];/gm, '')
    .replace(/^export /gm, '');
  let mounted;
  const listeners = {};
  const state = vm.runInNewContext(`(() => { ${ts.transpile(source, { target: ts.ScriptTarget.ES2022 })}; return useProject(opened); })()`, {
    ref, computed, reactive, nextTick: async () => {}, setTimeout, clearTimeout, setInterval: () => 0, clearInterval: () => {},
    opened: project,
    onMounted: fn => { mounted = fn; }, onUnmounted: () => {},
    detectProviders: async () => [{ id: 'codex', path: 'fake.exe' }],
    onTaskEvent: async fn => { listeners.event = fn; return () => {}; },
    onTaskDone: async fn => { listeners.done = fn; return () => {}; },
    onInstallEvent: async () => () => {},
    onSignInEvent: async () => () => {},
    onFileDrop: async () => () => {},
    cancelTask: async () => {},
    tidy: s => s,
    recentTasks: async () => [],
    gitStatus: async () => ({ ...gitState }),
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

test('Git forms require submission, preserve drafts, and reject invalid input', async () => {
  const sent = [];
  const { state } = await projectView({ gitAction: async (...args) => { sent.push(args); return { ...gitState }; } });
  state.git.value = { ...gitState, dirty: true, dirtyCount: 2 };
  state.commitMessage.value = '  Save my changes  ';
  await state.runGit('commit');
  assert.equal(sent.length, 0, 'an action cannot execute without its form');
  state.openGit('commit');
  state.openGit();
  state.openGit('commit');
  assert.equal(state.commitMessage.value, '  Save my changes  ');
  assert.equal(sent.length, 0, 'reopening the form never confirms it');
  state.commitMessage.value = '  ';
  await state.runGit('commit');
  assert.equal(sent.length, 0);
  state.commitMessage.value = '  Save my changes  ';
  await state.runGit('commit');
  assert.deepEqual(sent, [['C:/repo', 'commit', 'Save my changes']]);
  assert.equal(state.commitMessage.value, '');
  assert.equal(state.gitAsk.value, null);
  assert.equal(state.gitNotice.value, 'Changes committed.');

  state.openGit('merge', 'missing-branch');
  await state.runGit('merge');
  assert.equal(sent.length, 1, 'a missing source branch cannot be merged');
  state.openGit('merge', 'feature');
  state.openGit('merge', 'feature');
  assert.equal(sent.length, 1);
  await state.runGit('merge');
  assert.deepEqual(sent[1], ['C:/repo', 'merge', 'feature']);
});

test('Fetch runs immediately without tracking and keeps an open commit draft', async () => {
  const sent = [];
  const { state } = await projectView({ gitAction: async (...args) => { sent.push(args); return { ...gitState, behind: 2 }; } });
  state.git.value = { ...gitState, upstream: null, ahead: null, behind: null, dirty: true, dirtyCount: 1 };
  state.openGit('commit');
  state.commitMessage.value = 'Still writing';
  await state.runGit('fetch');
  assert.deepEqual(sent, [['C:/repo', 'fetch', '']]);
  assert.equal(state.git.value.behind, 2);
  assert.equal(state.gitAsk.value, 'commit');
  assert.equal(state.commitMessage.value, 'Still writing');
  assert.match(state.gitNotice.value, /Fetched/);
});

test('Git revert requires confirmation and scopes to one selected file or all files', async () => {
  const sent = [];
  const { state } = await projectView({ gitAction: async (...args) => { sent.push(args); return { ...gitState, dirty: true, dirtyCount: 2 }; } });
  state.git.value = { ...gitState, dirty: true, dirtyCount: 2 };
  await state.runGit('discard');
  assert.equal(sent.length, 0, 'revert cannot run without the confirmation form');
  state.openGit('discard', 'src/App.vue');
  assert.match(state.gitQuestion('discard'), /src\/App\.vue/);
  await state.runGit('discard');
  state.openGit('discard');
  await state.runGit('discard');
  assert.deepEqual(sent, [['C:/repo', 'discard', 'src/App.vue'], ['C:/repo', 'discard', '']]);
});

test('Git submissions cannot overlap each other or an AI task', async () => {
  let finish;
  let endTask;
  let calls = 0;
  let tasks = 0;
  const { state } = await projectView({
    gitAction: () => { calls++; return new Promise(resolve => { finish = resolve; }); },
    startTask: () => { tasks++; return new Promise(resolve => { endTask = resolve; }); },
  });
  state.git.value = { ...gitState, dirty: true, dirtyCount: 1 };
  state.openGit('commit');
  state.commitMessage.value = 'Save';
  const pending = state.runGit('commit');
  assert.equal(state.gitBusy.value, 'commit');
  assert.equal(state.canRun.value, false);
  await state.runGit('commit');
  await state.runGit('fetch');
  await state.run();
  assert.equal(calls, 1);
  assert.equal(tasks, 0);
  finish({ ...gitState, ahead: 1 });
  await pending;
  assert.equal(state.gitBusy.value, null);
  assert.equal(state.git.value.ahead, 1);
  const running = state.run();
  assert.equal(tasks, 1);
  await state.runGit('fetch');
  assert.equal(calls, 1, 'Git waits on the AI tasks, not the other way round');
  endTask(finished);
  await running;
});

test('a failed Git command refreshes status, keeps its error and draft, and can be retried', async () => {
  let attempts = 0;
  let refreshed = 0;
  const dirty = { ...gitState, dirty: true, dirtyCount: 1 };
  const { state } = await projectView({
    gitAction: async () => { if (++attempts === 1) throw new Error('Commit hook failed'); return { ...gitState, ahead: 1 }; },
    gitStatus: async () => { refreshed++; return dirty; },
  });
  state.git.value = dirty;
  state.openGit('commit');
  state.commitMessage.value = 'Keep this message';
  await state.runGit('commit');
  assert.equal(state.gitError.value, 'Commit hook failed');
  assert.equal(state.gitAsk.value, 'commit');
  assert.equal(state.commitMessage.value, 'Keep this message');
  assert.equal(refreshed, 1);
  await state.runGit('commit');
  assert.equal(state.gitError.value, null);
  assert.equal(state.git.value.ahead, 1);
});

test('new Git status wins over older refreshes and task previews', async () => {
  const pending = [];
  let finishPreview;
  let finishFetch;
  const { state } = await projectView({
    gitStatus: () => new Promise((resolve, reject) => pending.push({ resolve, reject })),
    gitAction: () => new Promise(resolve => { finishFetch = resolve; }),
    previewTask: () => new Promise(resolve => { finishPreview = resolve; }),
  });
  const older = state.refreshGit();
  const newer = state.refreshGit();
  pending[1].resolve({ ...gitState, ahead: 2 });
  await newer;
  pending[0].reject(new Error('Old request failed'));
  await older;
  assert.equal(state.git.value.ahead, 2);
  assert.equal(state.gitRefreshError.value, null);
  assert.equal(state.gitLoading.value, false);

  const fetch = state.runGit('fetch');
  const preview = state.refreshPreview();
  finishFetch({ ...gitState, behind: 3 });
  await fetch;
  finishPreview({ git: gitState });
  await preview;
  assert.equal(state.git.value.behind, 3, 'a preview started during Fetch cannot overwrite its result');

  const refresh = state.refreshGit();
  const stalePreview = state.refreshPreview();
  pending[2].resolve({ ...gitState, dirty: true, dirtyCount: 3 });
  await refresh;
  finishPreview({ git: gitState });
  await stalePreview;
  assert.equal(state.git.value.dirtyCount, 3);
});

test('task completion refreshes Git and invalidates status requested before the run', async () => {
  let oldStatus;
  let requests = 0;
  const { state } = await projectView({
    gitStatus: () => ++requests === 1 ? new Promise(resolve => { oldStatus = resolve; }) : Promise.resolve({ ...gitState, branches: ['orteca/task-1'] }),
    startTask: async () => ({ ...finished, status: 'done' }),
  });
  const pending = state.refreshGit();
  await state.run();
  oldStatus({ ...gitState, dirty: true, dirtyCount: 9 });
  await pending;
  assert.equal(requests, 2);
  assert.deepEqual([...state.git.value.branches], ['orteca/task-1']);
  assert.equal(state.git.value.dirty, false);
});

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

test('a change still being checked is shown while running and never becomes the result', async () => {
  let seen;
  const { state } = await projectView({ startTask: async (...args) => {
    const onChecking = args.filter(arg => typeof arg === 'function')[2];
    onChecking({ ...finished, status: 'checking', failure: null, diff: [{ path: 'a.php', added: 1, deleted: 0, origin: 'run' }] });
    seen = { running: state.running.value, checking: state.checking.value?.status, result: state.result.value };
    return { ...finished, status: 'verifyFailed', failure: null };
  } });
  await state.run();
  assert.deepEqual(seen, { running: true, checking: 'checking', result: null });
  assert.equal(state.checking.value, null, 'cleared once the run ends');
  assert.equal(state.result.value.status, 'verifyFailed', 'the suite decides, not the early result');
});

test('two runs go at once in one project, each with its own stream and result', async () => {
  const ends = [];
  const events = [];
  const { state } = await projectView({
    startTask: (...args) => {
      const fns = args.filter(arg => typeof arg === 'function');
      events.push(fns[0]);
      return new Promise(resolve => { ends.push(resolve); });
    },
  });
  state.task.value = 'first thing';
  const first = state.run();
  state.task.value = 'second thing';
  const second = state.run();

  assert.equal(state.runs.value.length, 2, 'a second run never replaces the first');
  assert.equal(state.runningCount.value, 2);
  assert.equal(state.anyRunning.value, true);
  assert.deepEqual(Array.from(state.runs.value, r => state.runLabel(r)), ['second thing', 'first thing'],
    'newest first, each labelled by what it asked');

  events[0]({ kind: 'text', data: 'from the first' });
  events[1]({ kind: 'text', data: 'from the second' });
  assert.deepEqual(Array.from(state.stream.value, l => l.text), ['from the second'],
    'the screen shows only the run it is on');

  const firstKey = state.runs.value[1].key;
  state.selectRun(firstKey);
  assert.deepEqual(Array.from(state.stream.value, l => l.text), ['from the first']);
  assert.equal(state.running.value, true);

  ends[0]({ ...finished, taskId: 11, status: 'done', failure: null });
  await first;
  assert.equal(state.result.value.taskId, 11, 'the finished run is the one on screen');
  assert.equal(state.runningCount.value, 1, 'the other one is still going');
  assert.equal(state.anyRunning.value, true);

  ends[1]({ ...finished, taskId: 12, status: 'done', failure: null });
  await second;
  assert.equal(state.result.value.taskId, 11, 'a run finishing elsewhere never takes over the screen');
  assert.equal(state.anyRunning.value, false);
});

test('a reply goes on in the same task, unless that task ran in a copy', async () => {
  const calls = [];
  const { state } = await projectView({ startTask: async (...args) => {
    calls.push(args);
    return { ...finished, taskId: 7, status: 'done', failure: null, summary: 'did it', worktree: calls.length === 2 ? {} : null };
  } });
  await state.run();
  assert.equal(calls[0].at(-2), null, 'a first run opens its own task');
  assert.equal(calls[0].at(-1), null, 'a first ask is routed on the whole prompt');
  state.reply.value = 'also this';
  await state.sendReply();
  assert.equal(calls[1].at(-2), 7);
  assert.equal(calls[1].at(-1), 'also this', 'the route reads the reply, not the exchange above it');
  assert.equal(calls[1][1], [
    'test',
    'Your answer:\ndid it',
    'My reply:\nalso this',
  ].join('\n\n'), 'the follow-up keeps user and assistant text under the correct labels');
  state.reply.value = 'and that';
  await state.sendReply();
  assert.equal(calls[2].at(-2), null, 'a copy folder is not where the task is');
});

test('a reply carries on in the run on screen, with its own attachments', async () => {
  const calls = [];
  const { state } = await projectView({
    pickAttachments: async () => ['C:/repo/notes.md'],
    startTask: async (...args) => {
      calls.push(args);
      return { ...finished, taskId: 7, status: 'done', failure: null, summary: `answer ${calls.length}`, worktree: null };
    },
  });
  await state.run();
  const row = state.runs.value[0];
  assert.equal(state.runLabel(row), 'test');

  await state.addAttachments(false);
  state.reply.value = 'also this';
  await state.sendReply();
  assert.equal(state.runs.value.length, 1, 'a follow-up stays in the run it answers');
  assert.equal(state.activeRun.value, row, 'and on the same page');
  assert.equal(state.runLabel(row), 'test', 'the row keeps the name of what was first asked');
  assert.equal(row.turns.length, 1);
  assert.equal(row.turns[0].said, 'test');
  assert.equal(row.turns[0].summary, 'answer 1');
  assert.equal(state.said.value, 'also this', 'the page shows what was typed, not the recap sent to the CLI');
  assert.equal(calls[1][6].join(), 'C:/repo/notes.md', 'the reply sends what was attached to it');
  assert.equal(state.attachments.value.length, 0, 'and the box empties, so the next reply does not resend them');

  state.reply.value = 'and that';
  await state.sendReply();
  assert.equal(state.runs.value.length, 1);
  assert.equal(row.turns.length, 2, 'every exchange stays on the page');
  assert.equal(calls[2][6].length, 0, 'a reply with nothing attached sends nothing');
});

test('a reply that had to open its own task gets its own run row', async () => {
  const { state } = await projectView({ startTask: async () => ({
    ...finished, taskId: 7, status: 'done', failure: null, summary: 'did it', worktree: {},
  }) });
  await state.run();
  state.reply.value = 'also this';
  await state.sendReply();
  assert.equal(state.runs.value.length, 2, 'a copy is a different task, and the sidebar says so');
});

test('an old task from the sidebar can be replied to, and goes on in that task', async () => {
  const past = {
    id: 42, prompt: 'the original ask', status: 'done', summary: 'what it did',
    diff: [{ path: 'src/a.ts', origin: null, added: 1, deleted: 0 }], events: [], worktreePath: null,
  };
  const calls = [];
  const { state } = await projectView({
    recentTasks: async () => [past],
    getTaskDetail: async () => past,
    startTask: async (...args) => {
      calls.push(args);
      return { ...finished, taskId: 42, status: 'done', failure: null, summary: 'and again', worktree: null };
    },
  });
  await state.openHistory(past);
  assert.equal(state.runs.value.length, 0, 'nothing is running yet');

  state.reply.value = 'now do the other half';
  await state.replyToPast();

  assert.equal(calls.length, 1);
  assert.equal(calls[0].at(-2), 42, 'it goes on in the task it was replying to');
  assert.equal(calls[0].at(-1), 'now do the other half', 'the route reads the reply alone');
  assert.equal(calls[0][1], [
    'the original ask',
    'Your answer:' + String.fromCharCode(10) + 'what it did',
    'My reply:' + String.fromCharCode(10) + 'now do the other half',
    'Files changed so far: src/a.ts',
  ].join(String.fromCharCode(10, 10)), 'the recap carries the old exchange, since the session is cold');
  assert.equal(calls[0].at(-3), null, 'a task this old has no session left to resume');

  const row = state.runs.value[0];
  assert.equal(state.runs.value.length, 1, 'the reply opens one run for the task');
  assert.equal(state.runLabel(row), 'the original ask', 'named after what was first asked');
  assert.equal(row.turns.length, 1, 'the exchange it answers opens the page');
  assert.equal(row.turns[0].said, 'the original ask');
  assert.equal(row.turns[0].summary, 'what it did');
  assert.equal(state.view.value, 'task', 'and the page moves from history to the run');
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
  assert.equal(state.friendlyToolUse('Shell', 'npm test'), 'Testing: npm test');
  const ps = '"C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe" -Command "rg -n Write-Output app"';
  assert.equal(state.friendlyToolUse('Shell', ps), 'Reading rg -n Write-Output app');
  assert.equal(state.describe({ kind: 'started', data: {} }), null);
  assert.equal(state.describeVerdict('{"checks":[],"verdict":"fail"}'), 'Checks didn’t pass: nothing was run');
  assert.equal(state.describeVerdict('{"checks":[{"command":"npm test","passed":true,"output":"ok"}],"verdict":"pass"}'), 'npm test passed');
  assert.equal(state.describeVerdict('plain words'), null);
  assert.equal(state.describeVerdict('{"objective":"Add a queue","constraints":[],"implementation_steps":["Read state.ts","Add the timer"],"risks":[]}'), 'Plan: Add a queue\n1. Read state.ts\n2. Add the timer');
  assert.equal(state.activityFor({ kind: 'text', data: 'I found the issue.' }).text, 'Thinking through the request');
  const file = 'C:\\Users\\me\\My App\\app\\Models\\User.php';
  assert.deepEqual({ ...state.toolActivity('Read', file) }, { text: 'Reading', file });
  assert.deepEqual({ ...state.toolActivity('Edit', 'src/App.vue') }, { text: 'Editing', file: 'src/App.vue' });
  assert.deepEqual({ ...state.toolActivity('Shell', 'Get-Content -Raw "app/User.php"') }, { text: 'Reading', file: 'app/User.php' });
  assert.deepEqual({ ...state.toolActivity('Shell', ps) }, { text: 'Reading rg -n Write-Output app', file: null });
  assert.deepEqual({ ...state.toolActivity('Shell', 'npm test') }, { text: 'Testing: npm test', file: null });
});

test('edit results update their own activity entry and replay without replacing another edit', async () => {
  const { state } = await projectView();
  const lines = [];
  const first = { kind: 'toolUse', data: { name: 'Edit', summary: 'state.ts', id: 'first', changes: [{ path: 'C:/repo/src/state.ts', patch: null }] } };
  const second = { kind: 'toolUse', data: { ...first.data, id: 'second' } };
  const patch = '@@ -8,1 +8,2 @@\n-old\n+new\n+extra\n';
  const applied = { kind: 'toolResult', data: { id: 'first', failed: false, changes: [{ path: first.data.changes[0].path, patch }] } };
  for (const event of [first, second, applied]) state.appendActivity(lines, event);
  assert.equal(lines.length, 2);
  assert.equal(lines[0].changes[0].patch, patch);
  assert.equal(lines[1].changes[0].patch, null);
  assert.equal(lines[0].file, 'C:/repo/src/state.ts', 'the full path survives a shortened summary');
  state.appendActivity(lines, { kind: 'toolResult', data: { id: 'missing', failed: false, changes: applied.data.changes } });
  assert.equal(lines.length, 2, 'an unmatched result cannot attach to a different call');
  state.appendActivity(lines, { kind: 'toolResult', data: { id: 'second', failed: true, changes: [] } });
  assert.equal(lines[1].text, 'Edit failed');
  assert.equal(lines[1].changes.length, 0, 'failed tools cannot claim a successful diff');
  assert.equal(lines[0].changes[0].patch, patch);
});

test('live edit completion retains its diff when subsequent activity arrives', async () => {
  const patch = '@@ -1 +1 @@\n-before\n+after\n';
  const { state } = await projectView({ startTask: async (...args) => {
    const emit = args[8];
    emit({ kind: 'toolUse', data: { id: 'edit-1', name: 'Edit', summary: 'state.ts', changes: [{ path: 'state.ts', patch: null }] } });
    assert.equal(state.currentActivity.value.changes[0].patch, null, 'render the pending edit before it completes');
    const rendered = computed(() => state.lines.value[0].changes[0].patch);
    assert.equal(rendered.value, null);
    emit({ kind: 'toolResult', data: { id: 'edit-1', failed: false, changes: [{ path: 'state.ts', patch }] } });
    assert.equal(state.currentActivity.value.changes[0].patch, patch);
    assert.equal(rendered.value, patch, 'an already-rendered log updates when its result arrives');
    emit({ kind: 'toolUse', data: { name: 'Read', summary: 'other.ts' } });
    return finished;
  } });
  await state.run();
  assert.equal(state.lines.value[0].changes[0].patch, patch);
  assert.equal(state.lines.value[1].file, 'other.ts');
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
  assert.equal(state.activePath.value, null);
  await state.confirmTrust();
  assert.equal(state.projects.value[0].project.trusted, true);
  assert.equal(state.activePath.value, 'C:/repo');
});

test('a new repository requires consent even without recognized configuration', async () => {
  const result = structuredClone(project);
  result.trustFindings = [];
  const state = await app({ openProject: async () => result });
  await state.open('C:/repo');
  assert.equal(state.activePath.value, null);
  assert.deepEqual(state.pendingTrust.value, result);
});

test('remembered consent opens the project', async () => {
  const result = structuredClone(project);
  result.project.trusted = true;
  const state = await app({ openProject: async () => result });
  await state.open('C:/repo');
  assert.equal(state.pendingTrust.value, null);
  assert.equal(state.activePath.value, 'C:/repo');
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
  assert.equal(state.activePath.value, null);
  assert.equal(state.pendingTrust.value.project.path, 'C:/repo');
});

test('failed consent stays closed and exposes the error', async () => {
  const state = await app({ openProject: async () => structuredClone(project), trustProject: async () => { throw new Error('database unavailable'); } });
  await state.open('C:/repo');
  await state.confirmTrust();
  assert.equal(state.activePath.value, null);
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
  assert.equal(state.activePath.value, null);
});

test('projects stay open while another is on screen, and a busy one cannot be closed', async () => {
  const opened = path => ({ project: { path, name: path, trusted: true }, git: gitState, trustFindings: [] });
  const state = await app({ openProject: async path => opened(path) });
  await state.open('C:/one');
  await state.open('C:/two');
  assert.deepEqual(Array.from(state.projects.value, p => p.project.path), ['C:/one', 'C:/two'],
    'the first project is still open behind the second');
  assert.equal(state.activePath.value, 'C:/two');

  await state.open('C:/one');
  assert.equal(state.projects.value.length, 2, 'reopening one already open only shows it again');
  assert.equal(state.activePath.value, 'C:/one');

  state.close();
  assert.equal(state.activePath.value, null, 'switching away closes nothing');
  assert.equal(state.projects.value.length, 2);

  state.busy.value['C:/one'] = 1;
  assert.deepEqual(Array.from(state.openProjects.value, p => p.running), [1, 0]);
  state.closeProject('C:/one');
  assert.equal(state.projects.value.length, 2, 'a project with a task running is not closed');
  state.busy.value['C:/one'] = 0;
  state.closeProject('C:/one');
  assert.deepEqual(Array.from(state.projects.value, p => p.project.path), ['C:/two']);
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
    stages: [
      { stage: 'plan', summary: '', artifact: null, model: 'opus', effort: 'high' },
      { stage: 'implement', summary: 'ok', artifact: null, model: 'sonnet', effort: 'medium' },
    ],
  };
  const { state } = await projectView({ startTask: async () => done });
  await state.run();
  assert.deepEqual(JSON.parse(JSON.stringify(state.routeSteps.value)), [
    { stage: 'plan', ran: true, asked: 'opus, high' },
    { stage: 'implement', ran: true, asked: 'sonnet, medium' },
    { stage: 'verify', ran: false, asked: null },
  ]);
  assert.equal(state.tokens.value.effort, 'medium', 'the effort beside the model is from the last stage');
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
  const { state } = await projectView({
    detectProviders: bothInstalled,
    providerLimits: reading(35, 90),
    startTask: () => new Promise(() => {}),
  });
  await settle();
  assert.equal(state.provider.value, 'claude', 'codex has 10% left, claude 65%');
  assert.match(state.pickedFor.value, /claude 65%, codex 10%/);

  state.providerPicked.value = true;
  state.provider.value = 'codex';
  state.limits.value = await reading(95, 10)();
  state.pickByHeadroom();
  assert.equal(state.provider.value, 'codex', 'a choice the user made is never overridden');

  state.providerPicked.value = false;
  void state.run();
  await state.loadLimits();
  assert.equal(state.provider.value, 'codex', 'refreshing usage cannot change the provider during a run');
});

test('auto routes the model while a provider choice sends its model and reasoning', async () => {
  let selected;
  const { state } = await projectView({
    detectProviders: bothInstalled,
    providerLimits: reading(35, 90),
    startTask: async (...args) => {
      selected = args[7];
      return { ...finished, status: 'done', failure: null };
    },
  });
  await settle();
  assert.equal(state.modelOverride.value, null);

  state.chooseProvider('claude');
  state.chooseModel('opus');
  state.modelChoices.value.claude.effort = 'max';
  await state.run();
  assert.equal(selected.model, 'opus');
  assert.equal(selected.effort, 'max');

  state.chooseProvider('auto');
  assert.equal(state.modelOverride.value, null);
});

test('an unread limit is not an empty one, and says why', async () => {
  const { state } = await projectView({ detectProviders: bothInstalled, providerLimits: reading(95, null) });
  await settle();
  assert.equal(state.provider.value, 'codex', 'no switch on a missing reading');
  assert.equal(state.limitLine('codex'), 'limits unavailable: codex app-server exited');
  assert.equal(state.limitLine('claude'), 'session 95% used');
});

test('usage counters show remaining allowance for every window and preserve reset information', async () => {
  const { state } = await projectView({ detectProviders: bothInstalled, providerLimits: async () => [
    { id: 'claude', windows: [{ label: 'session', usedPercent: 99.5, resetsAt: null, resetsText: 'Today at 3:50pm' }], unavailable: null },
    { id: 'codex', windows: [
      { label: '5-hour', usedPercent: 26, resetsAt: 1789304323, resetsText: null },
      { label: 'week', usedPercent: 100, resetsAt: null, resetsText: null },
      { label: 'other', usedPercent: NaN, resetsAt: NaN, resetsText: null },
    ], unavailable: null },
  ] });
  await settle();
  const [claude, codex] = state.usageCounters.value;
  assert.equal(claude.windows[0].left, 0.5);
  assert.equal(claude.windows[0].leftLabel, '<1%', 'a fraction left is not an exhausted plan');
  assert.equal(claude.windows[0].resets, 'Today at 3:50pm');
  assert.equal(codex.windows[0].leftLabel, '74%');
  assert.equal(codex.windows[0].shortLabel, '5h');
  assert.ok(codex.windows[0].resets);
  assert.equal(codex.windows[1].left, 0, 'a reported zero is distinct from unknown');
  assert.equal(codex.windows[2].left, null);
  assert.equal(codex.windows[2].leftLabel, '—');
  assert.equal(codex.windows[2].resets, null);
  assert.equal(state.headroom('codex'), null, 'an invalid window cannot guide provider selection');
});

test('usage counters distinguish missing installation, sign-in, and unavailable readings', async () => {
  const { state } = await projectView({ detectProviders: bothInstalled, providerLimits: reading(95, null) });
  await settle();
  assert.equal(state.usageCounters.value[1].status, 'Unavailable');
  assert.equal(state.usageCounters.value[1].reason, 'codex app-server exited');
  assert.equal(state.usageCounters.value[1].windows.length, 0);
  state.providers.value[0].auth = 'signedOut';
  assert.equal(state.usageCounters.value[0].status, 'Sign in required');
  assert.equal(state.usageCounters.value[0].windows.length, 0, 'old readings stay hidden after sign-out');
  state.providers.value[1].path = null;
  assert.equal(state.usageCounters.value[1].status, 'Not installed');
});

test('usage refresh keeps the latest response and exposes loading and failed checks', async () => {
  const pending = [];
  const { state } = await projectView({ detectProviders: bothInstalled, providerLimits: () => new Promise((resolve, reject) => pending.push({ resolve, reject })) });
  assert.equal(state.limitsLoading.value, true);
  assert.equal(state.usageCounters.value[0].status, 'Checking usage…');
  const latest = state.loadLimits();
  pending[1].resolve(await reading(25, 50)());
  await latest;
  const checked = state.limitsCheckedAt.value;
  assert.equal(state.limitsLoading.value, false);
  assert.ok(checked);
  pending[0].resolve(await reading(99, 100)());
  await settle();
  assert.equal(state.usageCounters.value[0].windows[0].left, 75, 'late initial response cannot overwrite the manual refresh');
  assert.equal(state.limitsCheckedAt.value, checked);

  const failed = state.loadLimits();
  assert.equal(state.usageCounters.value[0].windows[0].left, 75, 'the previous reading remains visible while refreshing');
  pending[2].reject(new Error('Could not reach the CLI'));
  await failed;
  assert.equal(state.limitsLoading.value, false);
  assert.equal(state.usageCounters.value[0].status, 'Unavailable');
  assert.equal(state.usageCounters.value[0].reason, 'Could not reach the CLI');
  assert.equal(state.headroom('claude'), null, 'failed checks do not leave stale allowance available to routing');
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

test('a run that ran out of plan usage carries on with the other CLI by itself, once', async () => {
  const sent = [];
  const spent = { ...finished, status: 'failed', failure: 'usage limit is used up.', failureKind: 'usageLimit' };
  let outcomes = [spent, { ...finished, status: 'done', failure: null, failureKind: null }];
  const { state } = await projectView({
    detectProviders: bothInstalled,
    providerLimits: reading(100, 30),
    startTask: async (path, prompt, provider, mode, headroom) => {
      sent.push({ provider, headroom });
      return outcomes.shift();
    },
  });
  await settle();
  state.providerPicked.value = true;
  state.provider.value = 'claude';
  await state.run();
  assert.deepEqual(sent, [{ provider: 'claude', headroom: 0 }, { provider: 'codex', headroom: 70 }]);
  assert.equal(state.switchedFrom.value, 'claude');
  assert.equal(state.result.value.status, 'done');

  sent.length = 0;
  outcomes = [spent, spent];
  await state.run();
  assert.deepEqual(sent.map((s) => s.provider), ['codex'], 'a plan read as spent is not tried');

  state.result.value = { ...finished, status: 'failed', failureKind: 'crashed' };
  assert.equal(state.fallback.value, null, 'a crash is not a reason to switch');
});

test('a request can wait for its plan to reset, and a spent run can carry on then', async () => {
  const timers = [];
  const sent = [];
  const resets = 2000000000;
  const spent = { ...finished, status: 'failed', failure: 'usage limit is used up.', failureKind: 'usageLimit', worktree: null };
  const { state } = await projectView({
    providerLimits: async () => [{ id: 'codex', windows: [{ label: '5-hour', usedPercent: 100, resetsAt: resets, resetsText: null }], unavailable: null }],
    startTask: async (path, prompt) => { sent.push(prompt); return spent; },
    setTimeout: (fn, ms) => { timers.push({ fn, ms }); return timers.length; },
    clearTimeout: () => {},
  });
  await settle();
  state.preview.value = { provider: 'codex', route: oneCall };
  assert.equal(state.limitWarning.value.startsAt, resets * 1000);
  state.task.value = 'rename it';
  state.waitForReset(state.limitWarning.value.startsAt);
  const first = timers.at(-1);
  assert.ok(first.ms > resets * 1000 - Date.now(), 'not before the reset');
  assert.equal(state.task.value, '', 'the composer is free for the next thing');
  assert.equal(state.waiting.value.prompt, 'rename it');
  first.fn();
  await settle();
  assert.deepEqual(sent, ['rename it']);
  assert.equal(state.waiting.value, null);

  assert.equal(state.retryAt.value, resets * 1000, 'the spent run can wait for the same reset');
  state.task.value = 'a draft';
  state.waitForReset(state.retryAt.value, state.activeRun.value);
  timers.at(-1).fn();
  await settle();
  assert.deepEqual(sent, ['rename it', 'rename it']);
  assert.equal(state.task.value, 'a draft', 'carrying on leaves the composer alone');
});

test('a reply that hands off to the other CLI stays in its task', async () => {
  const sent = [];
  const done = { ...finished, taskId: 7, status: 'done', failure: null, failureKind: null, summary: 'did it', worktree: null };
  const spent = { ...done, status: 'failed', failure: 'usage limit is used up.', failureKind: 'usageLimit' };
  const outcomes = [done, spent, done];
  const { state } = await projectView({
    detectProviders: bothInstalled,
    providerLimits: reading(100, 30),
    startTask: async (...args) => {
      sent.push({ provider: args[2], task: args.at(-2) });
      return outcomes.shift();
    },
  });
  await settle();
  state.providerPicked.value = true;
  state.provider.value = 'claude';
  await state.run();
  state.reply.value = 'also this';
  await state.sendReply();
  assert.deepEqual(sent.slice(1), [
    { provider: 'claude', task: 7 },
    { provider: 'codex', task: 7 },
  ], 'the handoff continues the reply’s task rather than opening another');
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


test('live stage progress stays with its run and accepts inserted and concurrent steps', async () => {
  const pending = [];
  const { state } = await projectView({ startTask: (...args) => new Promise(resolve => {
    pending.push({ emit: args[8], resolve }); args[9](pending.length);
  }) });
  const first = state.run();
  const firstRun = state.activeRun.value;
  state.newTask(); state.task.value = 'second';
  const second = state.run();
  pending[0].emit({ kind: 'stageProgress', data: { stages: ['implement', 'verify', 'review'], current: [1, 2] } });
  assert.equal(state.activeRun.value.progress, undefined);
  assert.deepEqual([...firstRun.progress.current], [1, 2]);
  pending[0].emit({ kind: 'stageProgress', data: { stages: ['implement', 'verify', 'fix', 'verify'], current: [2] } });
  assert.equal(firstRun.progress.stages[2], 'fix');
  assert.equal(firstRun.stream.length, 0, 'stage metadata must not become an AI message');
  for (const run of pending) run.resolve({ ...finished, status: 'done' });
  await Promise.all([first, second]);
});

for (const [disposition, expected] of [['held', 'Queued for next step'], ['resumed', 'Applied · step restarted'], ['live', 'Delivered to the running agent']]) {
  test('steering displays the backend receipt: ' + disposition, async () => {
    let release;
    const { state } = await projectView({
      sendInstruction: async () => ({ disposition }),
      startTask: (...args) => { args[9](1); return new Promise(resolve => { release = resolve; }); },
    });
    const run = state.run();
    state.instruction.value = 'keep the public API';
    await state.instruct(disposition === 'resumed');
    assert.equal(state.lines.value.at(-1).delivery, expected);
    release(finished); await run;
  });
}
