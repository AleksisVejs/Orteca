<script setup lang="ts">
import { computed, inject } from "vue";
import { verificationSummary, savedArtifacts } from "./taskPresentation";
import ActivityLog from "./ActivityLog.vue";
import TaskChat from "./TaskChat.vue";
import ExchangeView from "./Exchange.vue";
import Markdown from "../../components/Markdown.vue";
import { PROJECT, exchangeOf, splitExchanges, storyOf } from "./state";
import type { Exchange } from "./state";
import { split } from "./picks";
import type { Route } from "../../types";
import { visiblePath } from "../../path";

// One finished task from the sidebar, laid out like the page after a run: the same chat.
const {
  historyDetail, historyDetailLoading, historyDetailError,
  cacheHit, formatPayload, removedCopies, confirmRemove, removeCopy,
  removeError, linesOf, task, picks, selectRun, anyRunning, focusTask, domId,
  replyToPast, git, openGit, describeVerdict,
} = inject(PROJECT)!;

const d = computed(() => historyDetail.value);
// One chat per task: every reply is its own exchange, each with the proof it logged.
const parts = computed(() => (d.value ? splitExchanges(d.value) : []));
const earlier = computed(() => parts.value.slice(0, -1).map((p) => {
  const lines = linesOf(p.events);
  return { ...p, lines, data: p.result ? exchangeOf(p.result, lines) : null };
}));
const current = computed(() => parts.value.at(-1) ?? null);
const route = computed(() => (d.value?.route ?? null) as Route | null);
// A copy's work is on its branch; a follow-up would start without it.
const canReply = computed(() => !!d.value && !d.value.worktreePath && d.value.status !== "running");

const changed = computed(() => {
  const diff = d.value?.diff ?? [];
  return {
    byRun: diff.filter((f) => f.origin !== "beforeRun"),
    beforeRun: diff.filter((f) => f.origin === "beforeRun"),
    unknown: !!d.value?.dirtyAtStart && diff.some((f) => f.origin === null),
  };
});

// Stages are not saved as such; the event log says which ran, in order. The
// classify reply and the pre-run notes (stamped with the first stage before it
// starts) are not a stage running.
const PRE_RUN = new Set(["routing", "turn"]);
const ran = computed(() => {
  const out: string[] = [];
  for (const e of current.value?.events ?? []) {
    if (!e.stage || e.stage === "classify" || PRE_RUN.has(e.kind)) continue;
    if (out.at(-1) !== e.stage) out.push(e.stage);
  }
  return out;
});
const routeSteps = computed(() => {
  const steps = ran.value.map((stage) => ({ stage, ran: true }));
  if (!route.value || ran.value.includes("fix")) return steps;
  return [...steps, ...route.value.stages.slice(steps.length).map((stage) => ({ stage, ran: false }))];
});

const metrics = computed(() => {
  const t = d.value;
  return {
    callsUsed: t?.callsUsed ?? null,
    turns: null,
    ran: ran.value,
    tokens: t?.tokens == null ? null : {
      total: t.tokens,
      uncached: t.uncachedTokens ?? 0,
      cached: t.cachedTokens ?? 0,
      cacheHit: t.inputTokens != null ? cacheHit(t.inputTokens, t.cachedTokens ?? 0) : null,
    },
    cost: t?.costUsd ?? null,
    costQuality: t?.costQuality ?? null,
    model: t?.model ?? null,
    effort: null,
  };
});

// The same plain-English lines the live log showed, rebuilt from the saved events.
const lines = computed(() => linesOf(current.value?.events ?? []));

function editAgain() {
  if (!d.value) return;
  // A reply is edited as what was typed; the first request keeps its picked files.
  const { picks: again, rest } = split(parts.value.length > 1 ? current.value!.said : d.value.prompt);
  task.value = rest;
  picks.value = again;
  selectRun(null);
  focusTask();
}
// A task logged before each reply kept its result falls back to the task row.
const data = computed<Exchange>(() => current.value?.result ? exchangeOf(current.value.result, lines.value) : ({
  status: d.value?.status ?? "failed",
  failure: null,
  summary: d.value?.summary ?? null,
  unknownEvents: d.value?.unknownEvents ?? 0,
  changed: changed.value,
  patchText: d.value?.patchText ?? null,
  verification: verificationSummary(savedArtifacts(current.value?.events ?? [])),
  route: route.value,
  routeSteps: routeSteps.value,
  durationMs: d.value?.durationMs ?? null,
  metrics: metrics.value,
  messages: storyOf(lines.value),
}));
</script>

<template>
  <p v-if="historyDetailLoading" class="note" aria-live="polite">Loading task details…</p>
  <p v-else-if="historyDetailError" class="missing" role="alert">Task details unavailable.</p>
  <p v-else-if="!d" class="note">Pick a task on the left.</p>
  <TaskChat
    v-else
    :task-key="d.id"
    id-prefix="past"
    :prompt="current?.said ?? ''"
    :data="data"
    :can-reply="canReply"
    reply-note="Rereads the files"
    reply-hint="This task is over, so a reply reads the files again rather than picking up where it left off. It goes on in this same task."
    :edit-disabled="anyRunning"
    @send="replyToPast"
    @edit-again="editAgain"
  >
    <template #earlier>
      <template v-for="(t, i) in earlier" :key="i">
        <ExchangeView v-if="t.data" :id-prefix="`past-turn-${i}`" :prompt="t.said" :data="t.data" earlier>
          <template #activity>
            <ActivityLog :items="t.lines" :patch-text="t.result?.patchText" :root="d.worktreePath ?? undefined" :finished="true" :dirty-at-start="t.result?.dirtyAtStart" />
          </template>
        </ExchangeView>
        <!-- Logged before each reply kept its result: the words survive, the proof was not saved. -->
        <template v-else>
          <p class="bubble">{{ t.said }}</p>
          <Markdown v-for="(m, j) in t.lines.filter((l) => l.kind === 'text')" :key="j" class="summary back" :text="describeVerdict(m.text) ?? m.text" />
          <p class="note turn-end">Details for this reply were not saved.</p>
        </template>
      </template>
    </template>

    <template #after>
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
    </template>

    <template #activity>
      <ActivityLog :items="lines" :patch-text="data.patchText" :root="d.worktreePath ?? undefined" :finished="true" :dirty-at-start="d.dirtyAtStart" />
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

    <template v-if="d.status === 'running'" #no-reply>Still running. It takes a reply once it finishes.</template>
  </TaskChat>
</template>

<!-- Slot content is styled here, in the scope it was written in. -->
<style scoped src="./chat.css"></style>
<style scoped>
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
