import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';
import { computed, ref, watch, nextTick, effectScope } from 'vue';

const source = readFileSync(new URL('../src/views/project/taskPresentation.ts', import.meta.url), 'utf8').replace(/^import .*;$/gm, '').replace(/^export /gm, '');
const { verificationSummary, savedArtifacts } = vm.runInNewContext('(() => {' + ts.transpile(source, { target: ts.ScriptTarget.ES2022 }) + '; return { verificationSummary, savedArtifacts }; })()');
test('completion proof uses the final structured check, never prose or an earlier pass', () => {
  const pass = { stage: 'verify', artifact: { verdict: 'pass', checks: [{ passed: true }] } };
  assert.equal(verificationSummary([]), 'No verification recorded');
  assert.equal(verificationSummary([pass]), '1 recorded check passed');
  assert.equal(verificationSummary([pass, { stage: 'fix', artifact: null }]), 'Verification result unavailable');
  assert.equal(verificationSummary([{ stage: 'verify', artifact: { verdict: 'pass', checks: [null] } }]), '0 of 1 recorded checks passed');
  assert.equal(verificationSummary([pass, { stage: 'verify', artifact: { verdict: 'fail', checks: [{ passed: false }] } }]), '0 of 1 recorded checks passed');
  assert.equal(verificationSummary(savedArtifacts([{ kind: 'artifact', stage: 'verify', payload: { data: { stage: 'verify', valid: false, artifact: pass.artifact } } }])), 'Verification result unavailable');
  assert.equal(verificationSummary(savedArtifacts([{ kind: 'artifact', stage: 'verify', payload: { data: { stage: 'verify', valid: true, artifact: pass.artifact } } }])), '1 recorded check passed');
});

function activity() {
  const source = readFileSync(new URL('../src/views/project/ActivityLog.vue', import.meta.url), 'utf8').match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1].replace(/^import .*;$/gm, '');
  const project = { result: ref(null), lines: ref([]), activeRun: ref({ key: Symbol() }), historyDetail: ref(null), opened: { project: { path: 'C:/repo' } } };
  const scope = effectScope();
  const state = scope.run(() => vm.runInNewContext('(() => {' + ts.transpile(source, { target: ts.ScriptTarget.ES2022 }) + '; return { filter, lines, log, following, unread, onScroll, follow }; })()', {
    computed, ref, watch, nextTick, defineProps: () => ({}), inject: () => project, PROJECT: Symbol(),
    parseHistoryPatch: () => ({ files: [] }), visiblePath: value => value, activityPatch: () => ({}),
  }));
  return { state, project, stop: () => scope.stop() };
}
test('activity preserves a reader position and resumes following only on request', async () => {
  const { state, project, stop } = activity();
  try {
    state.log.value = { scrollHeight: 1000, scrollTop: 100, clientHeight: 200 };
    state.onScroll();
    project.lines.value.push({ kind: 'text', text: 'first' });
    await nextTick(); await nextTick();
    assert.equal(state.log.value.scrollTop, 100);
    assert.equal(state.unread.value, true);
    await state.follow();
    assert.equal(state.log.value.scrollTop, 1000);
    assert.equal(state.unread.value, false);
    state.log.value.scrollHeight = 1200;
    project.lines.value.push({ kind: 'text', text: 'first' });
    await nextTick(); await nextTick();
    assert.equal(state.log.value.scrollTop, 1200, 'identical new messages still count as new activity');
    project.lines.value.push({ kind: 'toolUse', text: 'Editing', file: 'a.ts', changes: [{ path: 'a.ts', patch: null }] });
    project.lines.value.push({ kind: 'text', text: 'Tests passed' });
    state.filter.value = 'edits'; assert.equal(state.lines.value.length, 1);
    state.filter.value = 'checks'; assert.equal(state.lines.value.length, 1);
    state.filter.value = 'messages'; assert.equal(state.lines.value.length, 3);
  } finally { stop(); }
});

