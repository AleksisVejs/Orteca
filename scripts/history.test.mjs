import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';
import { computed, ref } from 'vue';

const parser = readFileSync(new URL('../src/views/project/historyPatch.ts', import.meta.url), 'utf8')
  .replace(/^export /gm, '');
const parseHistoryPatch = vm.runInNewContext(`(() => {
  ${ts.transpile(parser, { target: ts.ScriptTarget.ES2022 })}; return parseHistoryPatch;
})()`, { TextEncoder, TextDecoder });

test('file comparisons keep exact code, counts, and old/new line numbers across hunks', () => {
  const { files } = parseHistoryPatch(`diff --git a/src/main.ts b/src/main.ts
index abc..def 100644
--- a/src/main.ts
+++ b/src/main.ts
@@ -8,3 +8,4 @@ function render()
 unchanged
---literal old header
+++literal new header
+<script>alert("text only")</script>
 last
@@ -40 +41 @@
-before
+after
\\ No newline at end of file
`);
  assert.equal(files.length, 1);
  const file = files[0];
  assert.equal(file.path, 'src/main.ts');
  assert.equal(file.status, 'Modified');
  assert.equal(file.added, 3);
  assert.equal(file.removed, 2);
  assert.deepEqual(Array.from(file.lines.filter(l => l.kind === 'removed'), l => [l.text, l.before, l.after]), [
    ['--literal old header', 9, undefined], ['before', 40, undefined],
  ]);
  assert.deepEqual(Array.from(file.lines.filter(l => l.kind === 'added'), l => [l.text, l.before, l.after]), [
    ['++literal new header', undefined, 9], ['<script>alert("text only")</script>', undefined, 10], ['after', undefined, 41],
  ]);
  assert.equal(file.lines.at(-1).kind, 'note');
});

test('new, deleted, renamed, quoted Unicode and spaced file paths stay identifiable', () => {
  const { files } = parseHistoryPatch(String.raw`diff --git a/new file.txt b/new file.txt
new file mode 100644
--- /dev/null
+++ b/new file.txt
@@ -0,0 +1 @@
+new
diff --git a/old.txt b/old.txt
deleted file mode 100644
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-old
diff --git "a/caf\303\251.txt" "b/caf\303\251.txt"
--- "a/caf\303\251.txt"
+++ "b/caf\303\251.txt"
@@ -1 +1 @@
-a
+b
diff --git a/old name.ts b/new name.ts
similarity index 100%
rename from old name.ts
rename to new name.ts
diff --git a/source.ts b/copy.ts
similarity index 100%
copy from source.ts
copy to copy.ts
`);
  assert.deepEqual(Array.from(files, f => [f.path, f.status]), [
    ['new file.txt', 'Added'], ['old.txt', 'Deleted'], ['café.txt', 'Modified'], ['new name.ts', 'Renamed'], ['copy.ts', 'Copied'],
  ]);
  assert.equal(files[3].previousPath, 'old name.ts');
  assert.equal(files[4].previousPath, 'source.ts');
});

test('binary, metadata-only, and unavailable untracked files do not invent line changes', () => {
  const { files } = parseHistoryPatch(`diff --git a/icon.png b/icon.png
GIT binary patch
literal 12
abcdef
diff --git a/empty.txt b/empty.txt
new file mode 100644
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
# binary untracked file: new/icon.png
# untracked file too large to display: data/big.txt
# untracked file could not be read: gone.txt
`);
  assert.equal(files.length, 6);
  assert.equal(files[0].binary, true);
  assert.equal(files[1].status, 'Added');
  assert.equal(files[1].path, 'empty.txt');
  assert.deepEqual(Array.from(files[2].notes), ['Previous file mode: 100644', 'New file mode: 100755']);
  assert.equal(files[3].path, 'new/icon.png');
  assert.equal(files[3].status, 'Added');
  assert.equal(files[3].binary, true);
  assert.match(files[4].notes[0], /too large/);
  assert.match(files[5].notes[0], /could not be read/);
  assert.ok(files.every(f => f.added === 0 && f.removed === 0));
});

test('truncated patches warn about partial counts; combined merge patches keep the raw fallback', () => {
  const patch = 'diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n';
  for (const ending of ['… patch truncated', '# patch truncated at 512 KiB']) {
    const parsed = parseHistoryPatch(`${patch}${ending}\n`);
    assert.equal(parsed.notices.length, 1);
    assert.match(parsed.notices[0], /incomplete/);
    assert.equal(parsed.files[0].added, 1);
  }
  assert.equal(parseHistoryPatch('diff --cc merge.txt\n@@@ -1,1 -1,1 +1,1 @@@\n++merged').files.length, 0);
  assert.equal(parseHistoryPatch('').files.length, 0);
  const cutOff = parseHistoryPatch('diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n first\n+part');
  assert.equal(cutOff.notices.length, 1, 'a backend cutoff without a truncation marker is still incomplete');
  const spaced = parseHistoryPatch('diff --git a/folder b/icon.png b/folder b/icon.png\nGIT binary patch\n');
  assert.equal(spaced.files[0].path, 'folder b/icon.png');
});

function history(api = {}) {
  const source = readFileSync(new URL('../src/views/project/DockHistory.vue', import.meta.url), 'utf8')
    .match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1]
    .replace(/^import .*;$/gm, '');
  return vm.runInNewContext(`(() => {
    ${ts.transpile(source, { target: ts.ScriptTarget.ES2022 })};
    return { show, back, load, picked, patch, patchLoading, patchError, working, parsed };
  })()`, {
    computed, ref, parseHistoryPatch,
    DOCK: Symbol(), inject: () => ({ path: 'C:/repo' }), onMounted: () => {},
    isAppError: e => !!e?.message,
    gitLog: async () => [], workingPatch: async () => '', commitPatch: async () => '',
    ...api,
  });
}

test('Current changes refreshes on entry and ignores a late commit response', async () => {
  let finish;
  let version = 0;
  const view = history({
    commitPatch: () => new Promise(resolve => { finish = resolve; }),
    workingPatch: async () => `working ${++version}`,
  });
  const stale = view.show({ hash: 'abc123' });
  view.back();
  await view.show('working');
  assert.equal(view.patch.value, 'working 1');
  finish('old commit');
  await stale;
  assert.equal(view.picked.value, 'working');
  assert.equal(view.patch.value, 'working 1');
  assert.equal(view.patchLoading.value, false);
  await view.show('working');
  assert.equal(view.patch.value, 'working 2');
  assert.equal(view.working.value, 'working 2');
});

test('late request failures cannot replace the error or loading state of a newer selection', async () => {
  let rejectOld;
  let resolveNew;
  const view = history({ commitPatch: (_path, hash) => hash === 'old'
    ? new Promise((_resolve, reject) => { rejectOld = reject; })
    : new Promise(resolve => { resolveNew = resolve; }),
  });
  const old = view.show({ hash: 'old' });
  const current = view.show({ hash: 'new' });
  rejectOld(new Error('stale failure'));
  await old;
  assert.equal(view.patchError.value, null);
  assert.equal(view.patchLoading.value, true);
  resolveNew('new patch');
  await current;
  assert.equal(view.patch.value, 'new patch');
  assert.equal(view.patchLoading.value, false);

  const broken = history({ workingPatch: async () => { throw new Error('Refresh failed'); } });
  await broken.show('working');
  assert.equal(broken.patchError.value, 'Refresh failed');
  assert.equal(broken.patchLoading.value, false);
});
