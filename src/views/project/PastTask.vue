<script setup lang="ts">
import { computed, inject, ref, watch } from "vue";
import Markdown from "../../components/Markdown.vue";
import ActivityLog from "./ActivityLog.vue";
import { PROJECT } from "./state";
import type { ProviderEvent, Route } from "../../types";

// One finished task from the sidebar, laid out like the page after a run.
const {
  historyDetail, historyDetailLoading, historyDetailError, HISTORY_STATUS, TONE, TABS,
  formatTokens, formatCost, formatDuration, formatPayload, removedCopies, confirmRemove, removeCopy,
  removeError, running, historyRow, describeVerdict, describe, toolActivity, stageLabel,
  task, result, view, focusTask, newTask,
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
const lines = computed(() =>
  (d.value?.events ?? []).flatMap((e) => {
    const ev = e.payload as ProviderEvent;
    if (typeof ev !== "object" || ev === null || !("kind" in ev)) return [];
    const text = describe(ev);
    if (text === null) return [];
    const act = ev.kind === "toolUse" ? toolActivity(ev.data.name, ev.data.summary) : null;
    return [act?.file ? { kind: ev.kind, ...act } : { kind: ev.kind, text, file: null }];
  }),
);

function editAgain() {
  if (!d.value) return;
  task.value = d.value.prompt;
  result.value = null;
  view.value = "task";
  focusTask();
}
</script>

<template>
  <p v-if="historyDetailLoading" class="note" aria-live="polite">Loading task details…</p>
  <p v-else-if="historyDetailError" class="missing" role="alert">Task details unavailable.</p>
  <p v-else-if="!d" class="note">Pick a task on the left.</p>
  <section v-else class="card outcome">
    <div class="head">
      <span class="dot" :class="TONE[d.status]" aria-hidden="true"></span>
      <h2 class="status">
        {{ route?.kind === "answer" && d.status === "done" ? "Answered" : HISTORY_STATUS[d.status] ?? d.status }}
      </h2>
      <span class="note">{{ formatDuration(d.durationMs) }} · {{ d.startedAt.slice(0, 16) }} UTC</span>
    </div>
    <div v-if="historyRow?.title" class="name" :title="historyRow.title">{{ historyRow.title }}</div>
    <p class="prompt">{{ d.prompt }}</p>

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
        <p v-if="d.status === 'cancelled'" class="note caveat">
          Stopped part-way. Anything the agent had already written is still on disk — Orteca reverts nothing.
        </p>
        <div v-if="d.worktreePath" class="copy" role="status">
          <p class="note">Worked in a separate copy on branch <span class="mono">{{ d.branch }}</span>.</p>
          <button v-if="d.branch && git.branches.includes(d.branch)" class="btn" popovertarget="project-git" popovertargetaction="show" @click="openGit('merge', d.branch)">Merge into {{ git.branch ?? 'current checkout' }}</button>
          <template v-if="!removedCopies.includes(d.id)">
            <p class="note mono">{{ d.worktreePath }}</p>
            <button
              class="btn"
              :class="{ confirming: confirmRemove === d.id }"
              :disabled="running"
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
        <ul class="diff">
          <li v-for="f in changed.byRun" :key="f.path">
            <span class="mono grow">{{ f.path }}</span>
            <span v-if="f.origin === 'both'" class="note">also changed before this run; counts include both</span>
            <span v-if="f.added !== null" class="note">+{{ f.added }} &minus;{{ f.deleted }}</span>
            <span v-else class="note">new or binary</span>
          </li>
          <li v-if="!changed.byRun.length" class="note">No files changed.</li>
        </ul>
        <p class="note caveat">Only files Git can see are listed.</p>
        <p v-if="changed.unknown" class="note caveat">
          This repository already had uncommitted changes, so some of the above may not have been made by this run.
        </p>
        <p v-if="changed.beforeRun.length" class="note caveat">
          Already changed before this run, and left as they were:
          <span class="mono">{{ changed.beforeRun.map((f) => f.path).join(", ") }}</span>
        </p>
        <details v-if="d.patchText" class="code-view">
          <summary>View patch</summary>
          <pre>{{ d.patchText }}</pre>
        </details>
      </template>

      <template v-else-if="tab === 'details'">
        <ol v-if="routeSteps.length" class="route">
          <template v-for="(step, i) in routeSteps" :key="i">
            <li v-if="i" class="arrow" aria-hidden="true">→</li>
            <li :class="{ ran: step.ran }">
              {{ stageLabel(step.stage) }}<span class="hidden-label"> — {{ step.ran ? "ran" : "not started" }}</span>
            </li>
          </template>
        </ol>
        <p v-if="route" class="note reason">
          {{ route.reason }}
          <template v-if="route.candidatePaths.length">
            · brief named <span class="mono">{{ route.candidatePaths.join(", ") }}</span>
          </template>
        </p>

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
          <div>
            <dt class="note">Git-visible files changed</dt>
            <dd>{{ changed.byRun.length }}</dd>
          </div>
        </dl>
      </template>

      <template v-else>
        <ActivityLog :items="lines" />
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

    <div class="actions">
      <button class="btn" :disabled="running" @click="editAgain">Edit and run again</button>
      <button class="btn primary" @click="newTask">New task</button>
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
