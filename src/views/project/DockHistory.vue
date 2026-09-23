<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue";
import { commitPatch, gitLog, isAppError, workingPatch } from "../../api";
import { DOCK } from "./dock";
import { parseHistoryPatch } from "./historyPatch";
import type { Commit } from "../../types";

// The repository's own history. One thing on screen at a time: the list, or
// the commit you picked. Git does the wording for "3 hours ago" - computing it
// here would drift from what every other git tool says. What the working tree
// holds above the newest commit sits at the top of the list: it is the part of
// the history still being written.
const dock = inject(DOCK)!;

const PAGE = 30;

const commits = ref<Commit[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);
const more = ref(false);

// The commit on screen, or "working" for the changes no commit holds yet.
const picked = ref<Commit | "working" | null>(null);
const working = ref("");
const patch = ref("");
const patchError = ref<string | null>(null);
const patchLoading = ref(false);

const expanded = ref(new Set<number>([0]));
const wrapLines = ref(true);
let patchRequest = 0;

const parsed = computed(() => parseHistoryPatch(patch.value));
const workingFiles = computed(() => parseHistoryPatch(working.value).files.length);
const totals = computed(() => parsed.value.files.reduce((sum, file) => ({
  added: sum.added + file.added, removed: sum.removed + file.removed,
}), { added: 0, removed: 0 }));
const fileCount = (count: number) => `${count} file${count === 1 ? "" : "s"}`;
const directory = (path: string) => path.slice(0, path.lastIndexOf("/") + 1);

function toggleFile(index: number, event: Event) {
  if ((event.target as HTMLDetailsElement).open) expanded.value.add(index);
  else expanded.value.delete(index);
}

function back() {
  patchRequest++;
  picked.value = null;
  patchLoading.value = false;
}

const fail = (e: unknown) => (isAppError(e) ? e.message : String(e));

async function load(append = false) {
  loading.value = true;
  try {
    const [page, tree] = await Promise.all([
      gitLog(dock.path, append ? commits.value.length : 0, PAGE),
      append ? working.value : workingPatch(dock.path),
    ]);
    working.value = tree;
    commits.value = append ? [...commits.value, ...page] : page;
    // A short page is the end of the history; a full one may not be.
    more.value = page.length === PAGE;
    error.value = null;
  } catch (e) {
    error.value = fail(e);
  }
  loading.value = false;
}

async function show(commit: Commit | "working") {
  const request = ++patchRequest;
  picked.value = commit;
  patchError.value = null;
  patch.value = "";
  expanded.value = new Set([0]);
  patchLoading.value = true;
  try {
    const result = commit === "working"
      ? await workingPatch(dock.path)
      : await commitPatch(dock.path, commit.hash);
    if (request !== patchRequest) return;
    patch.value = result;
    if (commit === "working") working.value = result;
  } catch (e) {
    if (request === patchRequest) patchError.value = fail(e);
  } finally {
    if (request === patchRequest) patchLoading.value = false;
  }
}

const exact = (iso: string) => {
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? iso : at.toLocaleString();
};

onMounted(() => load());
</script>

<template>
  <div class="history-tab">
    <header class="history-head">
      <template v-if="picked">
        <button class="link back" @click="back">← History</button>
        <button v-if="picked === 'working'" class="link" :disabled="patchLoading" @click="show('working')">{{ patchLoading ? "Reading…" : "Refresh" }}</button>
        <span v-else class="mono hash">{{ picked.hash }}</span>
      </template>
      <template v-else>
        <h2 class="label">History</h2>
        <button class="link" :disabled="loading" @click="load()">{{ loading ? "Reading…" : "Refresh" }}</button>
      </template>
    </header>

    <template v-if="picked">
      <div class="commit-head">
        <template v-if="picked === 'working'">
          <h2 class="subject">Current changes</h2>
          <p class="note">Changes since the last commit, including new files. Not committed yet.</p>
        </template>
        <template v-else>
          <h2 class="subject">{{ picked.subject }}</h2>
          <p class="note">{{ picked.author }} · <span :title="exact(picked.when)">{{ picked.relative }}</span></p>
        </template>
      </div>
      <p v-if="patchError" class="note tab-error" role="alert">{{ patchError }}</p>
      <p v-else-if="patchLoading" class="note tab-error" role="status">Reading changes…</p>
      <p v-else-if="!patch.trim()" class="note tab-error">
        {{ picked === "working" ? "Nothing has changed since the last commit." : "This commit changed no files." }}
      </p>
      <template v-else>
        <div v-if="parsed.files.length" class="changes-toolbar">
          <div class="change-counts">
            <span>{{ fileCount(parsed.files.length) }} changed</span>
            <span v-if="totals.added" class="added">+{{ totals.added }} added</span>
            <span v-if="totals.removed" class="removed">−{{ totals.removed }} removed</span>
          </div>
          <label class="wrap-option"><input v-model="wrapLines" type="checkbox"> Wrap lines</label>
        </div>
        <div :key="patchRequest" class="changes" :class="{ 'wrap-lines': wrapLines }">
          <p v-for="notice in parsed.notices" :key="notice" class="note patch-notice" role="status">{{ notice }}</p>
          <details v-for="(file, index) in parsed.files" :key="index" class="file-change" :open="expanded.has(index)" @toggle="toggleFile(index, $event)">
            <summary :title="file.path">
              <svg class="chevron" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m6 3 5 5-5 5" /></svg>
              <span class="file-identity">
                <span class="file-name">{{ file.path.split('/').pop() }}</span>
                <span v-if="directory(file.path)" class="file-directory">{{ directory(file.path) }}</span>
              </span>
              <span class="file-status">{{ file.status }}</span>
              <span class="file-counts">
                <span v-if="file.binary">Binary</span>
                <template v-else-if="file.added || file.removed">
                  <span class="added">+{{ file.added }} added</span>
                  <span class="removed">−{{ file.removed }} removed</span>
                </template>
                <span v-else>{{ file.notes.length ? 'Details available' : 'No line changes' }}</span>
              </span>
            </summary>
            <template v-if="expanded.has(index)">
              <p v-if="file.previousPath" class="note file-note">{{ file.status === 'Copied' ? 'Copied from' : 'Renamed from' }} <span class="file-path">{{ file.previousPath }}</span></p>
              <p v-for="note in file.notes" :key="note" class="note file-note">{{ note }}</p>
              <div v-if="file.lines.length" class="file-diff" tabindex="0" :aria-label="`Changes in ${file.path}. Old and new line numbers; minus means removed, plus means added.`" :style="{ fontSize: `${dock.prefs.fontSize}px`, fontFamily: dock.prefs.fontFamily }">
                <div class="diff-columns" aria-hidden="true"><span>Old</span><span>New</span><span></span><span>Changes</span></div>
                <div v-for="(line, lineIndex) in file.lines" :key="lineIndex" class="diff-line" :class="line.kind">
                  <template v-if="line.kind === 'hunk'">
                    <span class="hunk-label">Old line {{ line.before }} · New line {{ line.after }}<span v-if="line.text"> · {{ line.text }}</span></span>
                  </template>
                  <span v-else-if="line.kind === 'note'" class="hunk-label">{{ line.text }}</span>
                  <template v-else>
                    <span class="line-number">{{ line.before }}</span>
                    <span class="line-number">{{ line.after }}</span>
                    <span class="line-sign">{{ line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : ' ' }}</span>
                    <code class="line-text">{{ line.text || ' ' }}</code>
                  </template>
                </div>
              </div>
              <p v-else-if="!file.notes.length && !file.previousPath" class="note file-note">{{ file.status === 'Added' ? 'Empty file added.' : file.status === 'Deleted' ? 'Empty file deleted.' : 'File metadata changed; no text lines changed.' }}</p>
            </template>
          </details>
          <template v-if="!parsed.files.length">
            <p class="note file-note">This comparison is shown in Git’s original format.</p>
            <pre class="raw-patch" :style="{ fontSize: `${dock.prefs.fontSize}px`, fontFamily: dock.prefs.fontFamily }"><code>{{ patch }}</code></pre>
          </template>
        </div>
      </template>
    </template>

    <template v-else>
      <p v-if="error" class="note tab-error" role="alert">{{ error }}</p>
      <p v-else-if="loading && !commits.length" class="note tab-error">Reading the history…</p>
      <p v-else-if="!commits.length && !working.trim()" class="note tab-error">No commits yet.</p>
      <div v-else class="history-list">
        <div v-if="working.trim()" class="current-changes">
          <button class="commit" @click="show('working')">
            <span class="subject">Current changes <span class="view-changes">View changes →</span></span>
            <span class="caption">{{ fileCount(workingFiles) }} changed · not committed yet</span>
          </button>
        </div>
        <h3 v-if="commits.length" class="section-label">Commits</h3>
        <ul class="commits">
          <li v-for="commit in commits" :key="commit.hash">
            <button class="commit" :title="`${commit.subject}\n${commit.hash} · ${exact(commit.when)}`" @click="show(commit)">
              <span class="subject">{{ commit.subject }}</span>
              <span class="caption">
                <span class="mono">{{ commit.hash }}</span>
                · {{ commit.author }} · {{ commit.relative }}
              </span>
            </button>
          </li>
          <li v-if="more" class="older">
            <button class="link" :disabled="loading" @click="load(true)">{{ loading ? "Reading…" : "Load older" }}</button>
          </li>
        </ul>
      </div>
    </template>
  </div>
</template>

<style scoped>
.history-tab {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background: var(--bg);
}
.history-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--gap);
  flex-shrink: 0;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
}
.history-head .label {
  margin: 0;
  color: var(--text-faint);
}
.back {
  font-size: 12px;
}
.hash {
  font-size: 11px;
  color: var(--text-faint);
}
.tab-error {
  margin: 0;
  padding: 12px;
}
[role="alert"].tab-error {
  color: var(--err);
}

.history-list, .changes {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}
.commits {
  margin: 0;
  padding: 0 0 12px;
  list-style: none;
}
.commit {
  display: grid;
  gap: 5px;
  width: 100%;
  padding: 12px;
  text-align: left;
  border-bottom: 1px solid var(--border);
  transition: background 120ms ease;
}
.commit:hover {
  background: var(--surface);
}
.subject {
  margin: 0;
  overflow-wrap: anywhere;
  color: var(--text);
  font-size: 14px;
  font-weight: 500;
}
.caption {
  overflow-wrap: anywhere;
  color: var(--text-faint);
  font-size: 12px;
}
.caption .mono {
  font-size: 11px;
}
.older {
  padding: 10px 12px;
}
.current-changes {
  background: var(--surface);
}
.current-changes .subject {
  display: flex;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 4px 12px;
}
.view-changes, .section-label {
  color: var(--text-dim);
  font-size: 12px;
  font-weight: 400;
}
.section-label {
  margin: 0;
  padding: 16px 12px 4px;
}

.commit-head {
  flex-shrink: 0;
  padding: 10px 12px;
  border-bottom: 1px solid var(--border);
}
.commit-head .subject {
  white-space: normal;
  overflow-wrap: anywhere;
}
.commit-head .note {
  margin: 4px 0 0;
}
.changes-toolbar, .change-counts, .file-counts {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px 12px;
}
.changes-toolbar {
  flex-shrink: 0;
  justify-content: space-between;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  color: var(--text-dim);
  font-size: 12px;
}
.wrap-option {
  display: flex;
  align-items: center;
  gap: 6px;
  white-space: nowrap;
  cursor: pointer;
}
.wrap-option input {
  margin: 0;
  accent-color: var(--text-dim);
}
.patch-notice {
  padding: 10px 12px;
  margin: 0;
  color: var(--warn);
}
.file-change {
  border-bottom: 1px solid var(--border);
}
.file-change summary {
  display: grid;
  grid-template-columns: 16px minmax(0, 1fr) auto;
  align-items: start;
  gap: 4px 8px;
  padding: 10px 12px;
  background: var(--surface);
  cursor: pointer;
  list-style: none;
}
.file-change summary::-webkit-details-marker {
  display: none;
}
.file-change summary:hover {
  background: var(--surface-2);
}
.file-change summary:focus-visible {
  outline-offset: -2px;
}
.chevron {
  width: 16px;
  height: 16px;
  margin-top: 2px;
  stroke: var(--text-faint);
  stroke-width: 1.5;
}
.file-change[open] .chevron {
  transform: rotate(90deg);
}
.file-identity {
  display: grid;
  min-width: 0;
  overflow-wrap: anywhere;
  font-family: var(--mono);
}
.file-name {
  color: var(--text);
  font-size: 12px;
  font-weight: 600;
}
.file-directory, .file-status, .file-counts {
  color: var(--text-faint);
  font-size: 12px;
}
.file-counts {
  grid-column: 2 / -1;
}
.file-path {
  font-family: var(--mono);
}
.file-note {
  margin: 0;
  padding: 8px 12px;
  overflow-wrap: anywhere;
}
.file-diff {
  max-height: 420px;
  overflow: auto;
  color: var(--text-dim);
  line-height: 1.65;
  tab-size: 4;
}
.diff-line, .diff-columns {
  display: grid;
  grid-template-columns: 5ch 5ch 2ch minmax(0, 1fr);
  min-width: 100%;
  width: max-content;
}
.wrap-lines .diff-line, .wrap-lines .diff-columns {
  width: 100%;
}
.diff-columns {
  color: var(--text-faint);
  background: var(--surface);
  font-size: 11px;
}
.diff-columns > span:nth-child(-n+2), .line-number {
  padding-right: 1ch;
  text-align: right;
}
.line-number, .line-sign {
  color: var(--text-faint);
  user-select: none;
}
.line-text {
  padding-right: 12px;
  font: inherit;
  white-space: pre;
}
.wrap-lines .line-text {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.added, .added .line-sign {
  color: var(--syntax-string);
}
.removed, .removed .line-sign {
  color: var(--syntax-number);
}
.diff-line.added {
  background: color-mix(in srgb, var(--syntax-string) 9%, var(--bg));
}
.diff-line.removed {
  background: color-mix(in srgb, var(--syntax-number) 9%, var(--bg));
}
.hunk-label {
  grid-column: 1 / -1;
  padding: 6px 12px;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  color: var(--text-faint);
  background: var(--surface);
  font-family: var(--font);
  font-size: 11px;
}
.raw-patch {
  overflow: auto;
  margin: 0;
  padding: 12px;
  color: var(--text-dim);
}
.wrap-lines .raw-patch {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
