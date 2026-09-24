<script setup lang="ts">
import { computed, inject, onMounted, ref, watch, nextTick } from "vue";
import FileLink from "./FileLink.vue";
import { PROJECT } from "./state";
import type { ActivityLine } from "./state";
import { activityPatch, parseHistoryPatch } from "./historyPatch";
import { visiblePath } from "../../path";

// The live, plain-English view of a run. The full technical log is saved with the task.
// A past task passes its own lines, rebuilt from the saved log.
const props = defineProps<{ items?: ActivityLine[]; patchText?: string | null; root?: string; finished?: boolean; dirtyAtStart?: boolean }>();
const project = inject(PROJECT)!;
const finished = computed(() => props.finished ?? !!project.result.value);
const dirtyAtStart = computed(() => props.dirtyAtStart ?? project.result.value?.dirtyAtStart);
const taskPatch = computed(() => parseHistoryPatch(props.items ? props.patchText ?? "" : project.result.value?.patchText ?? ""));
const allLines = computed(() => (props.items ?? project.lines.value).map((line) => ({
  ...line,
  edits: (line.failed ? [] : line.changes ?? (line.text === "Editing" && line.file ? [{ path: line.file, patch: null }] : []))
    .map((edit) => ({ path: edit.path, ...activityPatch(visiblePath(edit.path), edit.patch, taskPatch.value.files,
      visiblePath(props.root ?? (props.items ? null : project.result.value?.worktree?.path) ?? project.opened.project.path)) })),
})));
const filter = ref("all");
const filters = [{ id: "all", label: "All" }, { id: "messages", label: "Messages" }, { id: "edits", label: "Edits" }, { id: "checks", label: "Checks" }];
const lines = computed(() => allLines.value.filter((line) => filter.value === "all"
  || (filter.value === "messages" && ["text", "thinking", "instruction", "failed"].includes(line.kind))
  || (filter.value === "edits" && (line.edits.length > 0 || line.failed))
  || (filter.value === "checks" && /test|check|lint|build|compil|verif|passed|failed/i.test(line.text))));
// Three or more reads in a row fold into one "Read 12 files" line.
type Line = (typeof lines.value)[number];
const isRead = (l: Line) => l.kind === "toolUse" && !l.failed && !l.edits.length && l.text.startsWith("Reading");
const rows = computed(() => {
  const out: ({ group: Line[]; line?: never } | { line: Line; group?: never })[] = [];
  let run: Line[] = [];
  const flush = () => {
    if (run.length >= 3) out.push({ group: run });
    else out.push(...run.map((line) => ({ line })));
    run = [];
  };
  for (const line of lines.value) {
    if (isRead(line)) run.push(line);
    else {
      flush();
      out.push({ line });
    }
  }
  flush();
  return out;
});
const log = ref<HTMLOListElement | null>(null);
const following = ref(true);
const unread = ref(false);
function onScroll() {
  if (!log.value) return;
  following.value = log.value.scrollHeight - log.value.scrollTop - log.value.clientHeight < 32;
  if (following.value) unread.value = false;
}
async function follow() {
  following.value = true;
  unread.value = false;
  await nextTick();
  if (log.value) log.value.scrollTop = log.value.scrollHeight;
}
watch(() => (props.items ?? project.lines.value).at(-1), (next, previous) => {
  if (finished.value || !next || next === previous) return;
  if (following.value && log.value) log.value.scrollTop = log.value.scrollHeight;
  else unread.value = true;
}, { flush: "post" });
watch(() => props.items ?? project.activeRun.value?.key ?? project.historyDetail.value?.id, () => { following.value = true; unread.value = false; filter.value = "all"; });
// A live log opened part-way starts at its latest line, like one that was open all along.
onMounted(() => { if (!finished.value) follow(); });
</script>

<template>
  <div v-if="allLines.length" class="activity-controls" role="group" aria-label="Filter activity">
    <button v-for="option in filters" :key="option.id" class="btn" :aria-pressed="filter === option.id" @click="filter = option.id">{{ option.label }}</button>
    <button v-if="unread && !finished" class="btn new-activity" @click="follow">New activity ↓</button>
  </div>
  <p v-if="!lines.length" class="note">{{ allLines.length ? 'No activity matches this filter.' : 'Nothing to show yet.' }}</p>
  <ol v-else ref="log" class="card stream" @scroll="onScroll" role="log" aria-live="polite" aria-relevant="additions">
    <template v-for="(entry, i) in rows" :key="i">
    <li v-if="entry.group" class="toolUse group">
      <details>
        <summary>{{ entry.group.every((l) => l.file) ? `Read ${entry.group.length} files` : `Read and searched ${entry.group.length} times` }}</summary>
        <ul><li v-for="(l, j) in entry.group" :key="j">{{ l.text }} <FileLink v-if="l.file" :file="l.file" /></li></ul>
      </details>
    </li>
    <li v-else :class="[entry.line.kind, { failed: entry.line.failed }]">
      <span v-if="entry.line.kind === 'instruction'" class="said">you</span><span v-if="entry.line.delivery" class="delivery">{{ entry.line.delivery }}</span>
      <template v-if="entry.line.edits.length">
        <details v-for="edit in entry.line.edits" :key="edit.path" class="edit">
          <summary :title="`Show changes in ${edit.path}`">
            <span class="edit-label">{{ entry.line.text }} <span class="edit-name">{{ edit.path.split(/[\\/]/).pop() }}</span></span>
            <span class="edit-counts" :aria-label="edit.file && !edit.file.binary ? `${edit.file.added} lines added, ${edit.file.removed} lines removed` : 'Show diff'">
              <template v-if="edit.file && !edit.file.binary"><span class="added">+{{ edit.file.added }}</span> <span class="removed">−{{ edit.file.removed }}</span></template>
              <template v-else>+ / −</template>
            </span>
            <span v-if="edit.file && !edit.exact" class="scope">task</span>
          </summary>
          <div class="edit-body">
            <div class="edit-heading"><FileLink :file="edit.path" /></div>
            <p v-if="edit.file" class="note">{{ edit.exact ? 'This edit' : 'All changes to this file in the task' }}<template v-if="!edit.exact && dirtyAtStart"> · Includes changes already present when the task started.</template></p>
            <template v-if="edit.file">
              <p v-for="note in edit.file.notes" :key="note" class="note">{{ note }}</p>
              <template v-if="!edit.exact"><p v-for="notice in taskPatch.notices" :key="notice" class="note">{{ notice }}</p></template>
              <div v-if="edit.file.lines.length" class="edit-code" tabindex="0" :aria-label="`Changes in ${edit.path}. Minus means removed; plus means added.`">
                <div v-for="(row, rowIndex) in edit.file.lines" :key="rowIndex" class="edit-row" :class="row.kind">
                  <template v-if="row.kind === 'hunk'"><span class="hunk-label">@@ −{{ row.before }} +{{ row.after }} @@ {{ row.text }}</span></template>
                  <template v-else>
                    <span class="line-number" aria-hidden="true">{{ row.before }}</span><span class="line-number" aria-hidden="true">{{ row.after }}</span>
                    <span class="line-sign">{{ row.kind === 'added' ? '+' : row.kind === 'removed' ? '−' : ' ' }}</span><code>{{ row.text }}</code>
                  </template>
                </div>
              </div>
              <p v-else-if="!edit.file.notes.length" class="note">No text lines changed.</p>
            </template>
            <p v-else class="note">{{ finished ? 'No recorded diff is available for this edit.' : 'Waiting for edit details. If the provider only reports file names, the task diff appears when the run finishes.' }}</p>
          </div>
        </details>
      </template>
      <template v-else>{{ entry.line.text }} <FileLink v-if="entry.line.file" :file="entry.line.file" /></template>
    </li>
    </template>
  </ol>
</template>

<style scoped>
.stream {
  margin: 0;
  padding: 12px 0;
  list-style: none;
  max-height: 360px;
  overflow-y: auto;
  background: none;
  border-width: 1px 0;
  border-radius: 0;
}
.stream li {
  padding: 8px 0;
  border-bottom: 1px solid var(--border);
  color: var(--text-dim);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
/* What the agent says is the story; its tool calls are the footnotes. */
.stream li.text {
  padding: 10px 0 6px;
  color: var(--text);
}
.stream li.thinking {
  color: var(--text-faint);
}
.stream li.toolUse {
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text-faint);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.stream li.group {
  white-space: normal;
}
.stream li.group summary {
  cursor: pointer;
}
.stream li.group ul {
  margin: 6px 0 0;
  padding-left: 16px;
}
.stream li.group li {
  padding: 2px 0;
  border: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.stream li.failed {
  color: var(--err);
}
.edit summary {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 3px 0;
  cursor: pointer;
  list-style: none;
}
.edit summary::-webkit-details-marker {
  display: none;
}
.edit summary:hover, .edit[open] summary {
  color: var(--text);
}
.edit-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.edit-name {
  text-decoration: underline;
  text-underline-offset: 3px;
}
.edit-counts {
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
}
.scope {
  color: var(--text-faint);
  font-family: var(--font);
  font-size: 11px;
}
.edit-body {
  margin: 8px 0 12px;
  white-space: normal;
}
.edit-heading {
  overflow-wrap: anywhere;
  color: var(--text-dim);
}
.edit-body .note {
  margin: 8px 0;
  font-family: var(--font);
}
.edit-code {
  max-height: 320px;
  overflow: auto;
  color: var(--text-dim);
  border-block: 1px solid var(--border);
  line-height: 1.65;
  tab-size: 4;
}
.edit-row {
  display: grid;
  grid-template-columns: 5ch 5ch 2ch minmax(0, 1fr);
  min-width: 100%;
  width: max-content;
}
.edit-row code {
  padding-right: 12px;
  font: inherit;
  white-space: pre;
}
.line-number {
  text-align: right;
  padding-right: 1ch;
  color: var(--text-faint);
  user-select: none;
}
.added {
  color: var(--syntax-string);
}
.removed {
  color: var(--syntax-number);
}
.edit-row.added {
  background: color-mix(in srgb, var(--syntax-string) 9%, var(--bg));
}
.edit-row.removed {
  background: color-mix(in srgb, var(--syntax-number) 9%, var(--bg));
}
.hunk-label {
  grid-column: 1 / -1;
  padding: 4px 12px;
  color: var(--text-faint);
  background: var(--surface);
  white-space: pre;
}
/* The user's own words, marked as theirs rather than as something said back. */
.stream li.instruction {
  color: var(--text);
}
.said {
  margin-right: 8px;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text-faint);
}
.activity-controls { display: flex; flex-wrap: wrap; gap: 8px; margin-bottom: 12px; }
.activity-controls .btn { padding: 4px 10px; font-size: 12px; }
.activity-controls [aria-pressed="true"] { background: var(--surface-2); color: var(--text); border-color: var(--border-strong); }
.new-activity { margin-left: auto; }
.delivery { display: block; color: var(--text-faint); font-size: 11px; }
</style>
