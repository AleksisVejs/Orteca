import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';
import { computed, nextTick, reactive, ref, watch } from 'vue';

// The dock's own logic, with the same trick app.test.mjs uses: run the module
// in a context where its imports are supplied instead of resolved.
function dock(store = new Map()) {
  const source = readFileSync(new URL('../src/views/project/dock.ts', import.meta.url), 'utf8')
    .replace(/^import[\s\S]*?from ["'][^"']+["'];/gm, '')
    .replace(/^export /gm, '');
  return vm.runInNewContext(`(() => { ${ts.transpile(source, { target: ts.ScriptTarget.ES2022 })}; return useDock('C:/repo'); })()`, {
    ref, computed, reactive, watch, JSON, Symbol,
    localStorage: {
      getItem: k => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, v),
    },
  });
}

test('a dev server address a terminal printed reaches the preview, once', () => {
  const d = dock();
  d.openPreview('http://localhost:5173');
  d.noteOutput('  VITE v6.4.3  ready\n  ➜  Local:   http://localhost:4321/\n');
  assert.equal(d.devUrls.value[0], 'http://localhost:4321/');
  assert.equal(d.tabs.value.find(t => t.kind === 'preview').url, 'http://localhost:4321/');

  d.noteOutput('still http://localhost:4321/ here');
  assert.equal(d.devUrls.value.length, 1, 'the same address must not stack up');

  d.prefs.followDevServer = false;
  d.noteOutput('now on http://127.0.0.1:8000/');
  assert.equal(d.devUrls.value[0], 'http://127.0.0.1:8000/');
  assert.equal(d.tabs.value.find(t => t.kind === 'preview').url, 'http://localhost:4321/', 'the preview only follows when asked to');
});

test('opening a file twice focuses the tab that already has it', () => {
  const d = dock();
  const first = d.openCode('src/main.ts');
  d.openTerminal();
  assert.notEqual(d.active.value, first.id);
  assert.equal(d.openCode('src/main.ts').id, first.id);
  assert.equal(d.active.value, first.id);
  assert.equal(d.tabs.value.filter(t => t.kind === 'code').length, 1);
});

test('closing a tab hands the dock to its neighbour, and the last one leaves it empty', () => {
  const d = dock();
  const one = d.openTerminal();
  const two = d.openTerminal();
  d.closeTab(two.id);
  assert.equal(d.active.value, one.id);
  d.closeTab(one.id);
  assert.equal(d.tabs.value.length, 0);
  assert.equal(d.active.value, 0);
});

test('tabs come back for the project they belong to, and terminals come back closed', async () => {
  const store = new Map();
  const first = dock(store);
  first.openCode('src/api.ts');
  first.openTerminal('scripts');
  first.tabs.value[1].exited = true;
  // The save rides Vue's own flush, so it lands on the next tick, not this one.
  await nextTick();

  const again = dock(store);
  assert.deepEqual(again.tabs.value.map(t => t.kind), ['code', 'terminal']);
  assert.equal(again.tabs.value[1].cwd, 'scripts');
  assert.equal(again.tabs.value[1].exited, false, 'a restored terminal opens a fresh shell');
  assert.equal(again.openTerminal().id > 2, true, 'a new tab must not reuse a restored id');
});

test('a storage that refuses to answer is defaults, not a broken dock', () => {
  const source = readFileSync(new URL('../src/views/project/dock.ts', import.meta.url), 'utf8')
    .replace(/^import[\s\S]*?from ["'][^"']+["'];/gm, '')
    .replace(/^export /gm, '');
  const d = vm.runInNewContext(`(() => { ${ts.transpile(source, { target: ts.ScriptTarget.ES2022 })}; return useDock('C:/repo'); })()`, {
    ref, computed, reactive, watch, JSON, Symbol,
    localStorage: {
      getItem: () => { throw new Error('blocked'); },
      setItem: () => { throw new Error('blocked'); },
    },
  });
  assert.equal(d.tabs.value.length, 0);
  assert.equal(d.prefs.side, 'right');
  d.openTerminal();
  assert.equal(d.tabs.value.length, 1);
});
