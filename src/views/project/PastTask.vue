<script setup lang="ts">
import { computed, inject, ref, watch } from "vue";
import { parseHistoryPatch } from "./historyPatch";
import { verificationSummary, unfinishedSummary, savedArtifacts } from "./taskPresentation";
import Markdown from "../../components/Markdown.vue";
import ActivityLog from "./ActivityLog.vue";
import { PROJECT } from "./state";
import type { ActivityLine } from "./state";
import { split, tidy } from "./picks";
import type { ProviderEvent, Route } from "../../types";
import { visiblePath } from "../../path";

// One finished task from the sidebar, laid out like the page after a run.
const {
  historyDetail, historyDetailLoading, historyDetailError, HISTORY_STATUS, TONE, TABS,
  formatTokens, formatCost, formatDuration, formatPayload, removedCopies, confirmRemove, removeCopy,
  removeError, historyRow, describeVerdict, appendActivity, stageLabel,
  task, picks, selectRun, anyRunning, focusTask, newTask, domId,
  reply, replyToPast, running, attachments, attachError, addAttachments, pasteImages, fileName,
  git, openGit,
} = inject(PROJECT)!;

const tab = ref<(typeof TABS)[number]["id"]>("summary");
watch(() => historyDetail.value?.id, () => (tab.value = "summary"));

const d = computed(() => historyDetail.value);
const route = computed(() => (d.value?.route ?? null) as Route | null);

const changed = computed(() => {
  const diff = d.value?.diff ?? [];
  return {
    byRun: diff.filter((f) => f.origin !== "beforeRun"),
    beforeRun: diff.filter((f) => f.origin === "beforeRun"),
    unknown: !!d.value?.dirtyAtStart && diff.some((f) => f.origin === null),
  };
});

// Stages are not saved as such; the event log says which ran, in order.
const ran = computed(() => {
  const out: string[] = [];
  for (const e of d.value?.events ?? []) if (e.stage && out.at(-1) !== e.stage) out.push(e.stage);
  return out;
});
const routeSteps = computed(() => {
  const steps = ran.value.map((stage) => ({ stage, ran: true }));
  if (!route.value || ran.value.includes("fix")) return steps;
  return [...steps, ...route.value.stages.slice(steps.length).map((stage) => ({ stage, ran: false }))];
});

// The same plain-English lines the live log showed, rebuilt from the saved events.
const lines = computed(() => {
  const items: ActivityLine[] = [];
  for (const e of d.value?.events ?? []) {
    const ev = e.payload as ProviderEvent;
    if (typeof ev === "object" && ev !== null && "kind" in ev) appendActivity(items, ev);
  }
  return items;
});

function editAgain() {
  if (!d.value) return;
  const { picks: again, rest } = split(d.value.prompt);
  task.value = rest;
  picks.value = again;
  selectRun(null);
  focusTask();
}
const selectedFile = ref<string | null>(null);
const parsedPatch = computed(() => parseHistoryPatch(d.value?.patchText ?? ""));
const selectedPath = computed(() => changed.value.byRun.some((f) => f.path === selectedFile.value) ? selectedFile.value : changed.value.byRun[0]?.path);
const selectedPatch = computed(() => parsedPatch.value.files.find((f) => f.path === selectedPath.value));
const verification = computed(() => verificationSummary(savedArtifacts(d.value?.events ?? [])));
watch(() => d.value?.id, () => { selectedFile.value = null; });
</script>

<template>
  <p v-if="historyDetailLoading" class="note" aria-live="polite">Loading task details…</p>
  <p v-else-if="historyDetailError" class="missing" role="alert">Task details unavailable.</p>
  <p v-else-if="!d" class="note">Pick a task on the left.</p>
  <section v-else class="card outcome">
    <header class="result-header">
    <div class="result-heading">
    <div class="head">
      <span class="dot" :class="TONE[d.status]" aria-hidden="true"></span>
      <h2 class="status">
        {{ route?.kind === "answer" && d.status === "done" ? "Answered" : HISTORY_STATUS[d.status] ?? d.status }}
      </h2>
      <span class="note">{{ formatDuration(d.durationMs) }} · {{ d.startedAt.slice(0, 16) }} UTC</span>
    </div>
    <div v-if="historyRow?.title" class="name" :title="historyRow.title">{{ historyRow.title }}</div>
    <h1 class="prompt" :class="{ long: tidy(d.prompt).length > 140 }">{{ tidy(d.prompt) }}</h1>
    <div class="outcome-summary"><span>{{ changed.byRun.length }} files changed</span><span>{{ verification }}</span><span>{{ unfinishedSummary(d.status) }}</span></div>

    </div>
    <div class="actions">
      <button class="btn" :disabled="anyRunning" @click="editAgain">Edit and run again</button>
      <button class="btn primary" @click="newTask">New task</button>
    </div>
    </header>

    <div class="tabs" role="tablist" aria-label="Past task">
      <button
        v-for="t in TABS"
        :id="`past-tab-${t.id}`"
        :key="t.id"
        role="tab"
        :aria-selected="tab === t.id"
        aria-controls="past-panel"
        :class="{ on: tab === t.id }"
        @click="tab = t.id"
      >
        {{ t.label }}<span v-if="t.id === 'files'" class="count">{{ changed.byRun.length }}</span>
      </button>
    </div>

    <div id="past-panel" class="panel" role="tabpanel" :aria-labelledby="`past-tab-${tab}`">
      <template v-if="tab === 'summary'">
        <Markdown v-if="d.summary" class="summary" :text="describeVerdict(d.summary) ?? d.summary" />
        <p v-else class="note">No summary was reported.</p>
        <!-- A copy's work is on its branch; a follow-up would start without it. -->
        <template v-if="d.summary && !d.worktreePath">
          <div class="reply">
            <label class="hidden-label" :for="domId('past-reply')">Reply</label>
            <textarea
              :id="domId('past-reply')"
              v-model="reply"
              rows="2"
              spellcheck="false"
              placeholder="Reply, or ask for something more…"
              :disabled="running"
              @paste="pasteImages"
              @keydown.ctrl.enter.prevent="replyToPast"
            ></textarea>
            <ul v-if="attachments.length" class="attachments" aria-label="Attached">
              <li v-for="path in attachments" :key="path" class="chip">
                <span class="mono" :title="path">{{ fileName(path) }}</span>
                <button
                  class="unattach"
                  :title="`Remove ${fileName(path)}`"
                  :aria-label="`Remove ${fileName(path)}`"
                  @click="attachments = attachments.filter((p) => p !== path)"
                >
                  ×
                </button>
              </li>
            </ul>
            <p v-if="attachError" class="attach-error">{{ attachError }}</p>
            <div class="reply-controls">
              <button class="icon" title="Attach files or images. You can also paste or drop them." aria-label="Attach files or images" @click="addAttachments(false)">
                <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M10.5 4.5 5.8 9.2a1.4 1.4 0 0 0 2 2l5-5a2.8 2.8 0 0 0-4-4l-5 5a4.2 4.2 0 0 0 6 6l4.2-4.2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
                Attach
              </button>
              <button class="icon" title="Attach a folder" aria-label="Add folder" @click="addAttachments(true)">
                <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
              </button>
              <span class="shortcut note">Ctrl + Enter</span>
              <button class="btn" :disabled="running || !reply.trim()" @click="replyToPast">Reply</button>
            </div>
          </div>
          <p class="note reply-cost">
            This task is over, so a reply reads the files again rather than picking up
            where it left off. It goes on in this same task.
          </p>
        </template>
        <p v-if="d.status === 'cancelled'" class="note caveat">
          Stopped part-way. Anything the agent had already written is still on disk — Orteca reverts nothing.
        </p>
        <div v-if="d.worktreePath" class="copy" role="status">
          <p class="note">Worked in a separate copy on branch <span class="mono">{{ d.branch }}</span>.</p>
          <button v-if="d.branch && git.branches.includes(d.branch)" class="btn" :popovertarget="domId('project-git')" popovertargetaction="show" @click="openGit('merge', d.branch)">Merge into {{ git.branch ?? 'current checkout' }}</button>
          <template v-if="!removedCopies.includes(d.id)">
            <p class="note mono">{{ visiblePath(d.worktreePath) }}</p>
            <button
              class="btn"
              :class="{ confirming: confirmRemove === d.id }"
              :disabled="anyRunning"
              @click="removeCopy(d.id)"
            >
              {{ confirmRemove === d.id ? "Yes, delete the copy folder" : "Remove copy" }}
            </button>
            <button v-if="confirmRemove === d.id" class="link" @click="confirmRemove = null">Keep it</button>
          </template>
          <p v-else class="note">Copy removed. The branch is still there.</p>
          <p v-if="removeError" class="missing">{{ removeError }}</p>
        </div>
        <p v-if="d.unknownEvents" class="note caveat" role="status">
          {{ d.unknownEvents }} provider event{{ d.unknownEvents === 1 ? "" : "s" }} were not recognized and remain in the saved task log.
        </p>
      </template>

      <template v-else-if="tab === 'files'">
        <div v-if="changed.byRun.length" class="file-review">
          <ul class="review-files" aria-label="Changed files">
            <li v-for="f in changed.byRun" :key="f.path">
              <button :aria-pressed="selectedPath === f.path" @click="selectedFile = f.path">
                <span class="mono file-path">{{ f.path }}</span>
                <span class="note">{{ parsedPatch.files.find((p) => p.path === f.path)?.status ?? 'Status unavailable' }} · <template v-if="f.added !== null && f.deleted !== null">+{{ f.added }} −{{ f.deleted }}</template><template v-else>Counts unavailable</template></span>
                <span v-if="f.origin === 'both'" class="note">Includes pre-existing changes</span>
                <span v-else-if="f.origin === null" class="note">Change origin unknown</span>
              </button>
            </li>
          </ul>
          <section class="review-patch" aria-label="Selected file changes" tabindex="0">
            <h2 class="label mono">{{ selectedPath }}</h2>
            <p v-if="changed.byRun.find((f) => f.path === selectedPath)?.origin === 'both'" class="note">This patch includes changes already present before the task.</p>
            <template v-if="selectedPatch">
              <p v-for="note in selectedPatch.notes" :key="note" class="note">{{ note }}</p>
              <div v-for="(line, i) in selectedPatch.lines" :key="i" class="patch-line" :class="line.kind"><span class="line-number">{{ line.before ?? '' }}</span><span class="line-number">{{ line.after ?? '' }}</span><code>{{ line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : line.kind === 'hunk' ? '@@ ' : ' ' }}{{ line.text }}</code></div>
              <p v-if="!selectedPatch.lines.length && !selectedPatch.notes.length" class="note">No text changes recorded.</p>
            </template>
            <p v-else class="note">This file’s patch is unavailable or was truncated.</p>
          </section>
        </div>
        <p v-else class="note">No files changed.</p>
        <p class="note caveat">Only files Git can see are listed. Line counts may include pre-existing changes.</p>
        <p v-if="changed.unknown" class="note caveat">Some changes could not be attributed to this task.</p>
        <p v-if="changed.beforeRun.length" class="note caveat">Already changed before this task and left untouched: <span class="mono">{{ changed.beforeRun.map((f) => f.path).join(', ') }}</span></p>
        <p v-for="notice in parsedPatch.notices" :key="notice" class="note">{{ notice }}</p>
        <details v-if="d.patchText" class="code-view"><summary>View full patch</summary><pre>{{ d.patchText }}</pre></details>
      </template>

      <template v-else-if="tab === 'details'">
        <section class="execution" aria-label="Execution">
          <div class="section-heading"><h2 class="label">Execution</h2><span class="note">Steps taken for this task</span></div>
        <ol v-if="routeSteps.length" class="route">
          <template v-for="(step, i) in routeSteps" :key="i">
            <li v-if="i" class="arrow" aria-hidden="true">→</li>
            <li :class="{ ran: step.ran }"><span class="step-number" aria-hidden="true">{{ i + 1 }}</span><div>
              {{ stageLabel(step.stage) }}<small>{{ step.ran ? "Ran" : "Not started" }}</small></div>
            </li>
          </template>
        </ol>
        <p v-if="route" class="note reason">
          {{ route.reason }}
          <template v-if="route.candidatePaths.length">
            · brief named <span class="mono">{{ route.candidatePaths.join(", ") }}</span>
          </template>
        </p>

        </section>

        <dl class="tiles">
          <div>
            <dt class="note">{{ d.callsUsed === null ? "agent calls unavailable" : d.callsUsed === 1 ? "agent call" : "agent calls" }}</dt>
            <dd>{{ d.callsUsed ?? "—" }}</dd>
            <dd class="note">{{ ran.join(" → ") || "none" }}</dd>
          </div>
          <div>
            <dt class="note">{{ d.tokens !== null ? "tokens" : "tokens unavailable" }}</dt>
            <dd>{{ d.tokens !== null ? formatTokens(d.tokens) : "—" }}</dd>
            <dd v-if="d.tokens !== null" class="note">
              {{ formatTokens(d.uncachedTokens ?? 0) }} uncached · {{ formatTokens(d.cachedTokens ?? 0) }} cached
            </dd>
          </div>
          <div>
            <dt class="note">{{ d.costUsd !== null ? "cost, " + d.costQuality : "cost unavailable" }}</dt>
            <dd :class="{ good: d.costUsd !== null }">{{ d.costUsd !== null ? formatCost(d.costUsd) : "—" }}</dd>
          </div>
          <div>
            <dt class="note">elapsed</dt>
            <dd>{{ formatDuration(d.durationMs) }}</dd>
          </div>
          <div>
            <dt class="note">model reported by provider</dt>
            <dd class="model">{{ d.model ?? "—" }}</dd>
          </div>

        </dl>
        <div class="detail-links">
          <section class="detail-section"><h2 class="label">Changes</h2><p><strong>{{ changed.byRun.length }}</strong> Git-visible files changed</p><p class="note">Review recorded changes and their patches.</p><button class="btn" @click="tab = 'files'">View changed files</button></section>
          <section class="detail-section"><h2 class="label">Execution activity</h2><p>Follow the work step by step.</p><p class="note">Agent messages, tool activity and recorded edits.</p><button class="btn" @click="tab = 'activity'">View activity</button></section>
        </div>

      </template>

      <template v-else>
        <ActivityLog :items="lines" :patch-text="d.patchText" :root="d.worktreePath ?? undefined" :finished="true" :dirty-at-start="d.dirtyAtStart" />
        <details class="code-view log">
          <summary>View full task log · {{ d.events.length }} events</summary>
          <ol>
            <li v-for="event in d.events" :key="event.id">
              <span class="mono">{{ event.stage ?? "run" }} · {{ event.kind }}</span>
              <pre>{{ formatPayload(event.payload) }}</pre>
            </li>
          </ol>
        </details>
      </template>
    </div>


  </section>
</template>

<style scoped src="./result.css"></style>
<style scoped>
.name {
  margin-top: 10px;
  overflow: hidden;
  font-size: 12px;
  color: var(--text-dim);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.log {
  margin-top: 12px;
}
.log ol {
  margin: 10px 0 0;
  padding: 0;
  list-style: none;
}
.log li + li {
  margin-top: 10px;
}
</style>
