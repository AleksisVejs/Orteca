<script setup lang="ts">
import { inject } from "vue";
import Markdown from "../../components/Markdown.vue";
import { PROJECT } from "./state";

// One finished task from the sidebar, as it was recorded.
const {
  historyDetail, historyDetailLoading, historyDetailError, HISTORY_STATUS, TONE,
  formatTokens, formatDuration, formatPayload, removedCopies, confirmRemove, removeCopy,
  removeError, running,
} = inject(PROJECT)!;
</script>

<template>
  <p v-if="historyDetailLoading" class="note" aria-live="polite">Loading task details…</p>
  <p v-else-if="historyDetailError" class="missing" role="alert">Task details unavailable.</p>
  <p v-else-if="!historyDetail" class="note">Pick a task on the left.</p>
  <section v-else class="card detail">
    <div class="head">
      <span class="dot" :class="TONE[historyDetail.status]" aria-hidden="true"></span>
      <h2 class="status">{{ HISTORY_STATUS[historyDetail.status] ?? historyDetail.status }}</h2>
      <span class="note">{{ historyDetail.startedAt.slice(0, 16) }} UTC</span>
    </div>
    <p class="prompt">{{ historyDetail.prompt }}</p>

    <div class="metrics">
      <span>{{ historyDetail.uncachedTokens === null ? "uncached tokens unavailable" : formatTokens(historyDetail.uncachedTokens) + " uncached tokens" }}</span>
      <span>{{ historyDetail.model ?? "model unavailable" }}</span>
      <span>{{ formatDuration(historyDetail.durationMs) }}</span>
      <span v-if="historyDetail.callsUsed !== null">{{ historyDetail.callsUsed }} calls</span>
      <span v-if="historyDetail.unknownEvents">{{ historyDetail.unknownEvents }} unknown events</span>
    </div>

    <Markdown v-if="historyDetail.summary" class="summary" :text="historyDetail.summary" />

    <div v-if="historyDetail.worktreePath && !removedCopies.includes(historyDetail.id)" class="copy">
      <p class="note">
        Worked in a separate copy on branch <span class="mono">{{ historyDetail.branch }}</span> at
        <span class="mono">{{ historyDetail.worktreePath }}</span>.
      </p>
      <button
        class="btn"
        :class="{ confirming: confirmRemove === historyDetail.id }"
        :disabled="running"
        @click="removeCopy(historyDetail.id)"
      >
        {{ confirmRemove === historyDetail.id ? "Yes, delete the copy folder" : "Remove copy" }}
      </button>
      <button v-if="confirmRemove === historyDetail.id" class="link" @click="confirmRemove = null">Keep it</button>
      <p v-if="removeError" class="missing">{{ removeError }}</p>
    </div>

    <h3 class="label files">Files</h3>
    <ul class="diff">
      <li v-for="f in historyDetail.diff" :key="f.path">
        <span class="mono grow">{{ f.path }}</span>
        <span v-if="f.added !== null" class="note">+{{ f.added }} −{{ f.deleted }}</span>
        <span v-else class="note">new or binary</span>
      </li>
      <li v-if="!historyDetail.diff.length" class="note">No Git-visible changes.</li>
    </ul>
    <details v-if="historyDetail.patchText" class="code-view">
      <summary>View patch</summary>
      <pre>{{ historyDetail.patchText }}</pre>
    </details>
    <details class="code-view log">
      <summary>View task log · {{ historyDetail.events.length }} events</summary>
      <ol>
        <li v-for="event in historyDetail.events" :key="event.id">
          <span class="mono">{{ event.stage ?? "run" }} · {{ event.kind }}</span>
          <pre>{{ formatPayload(event.payload) }}</pre>
        </li>
      </ol>
    </details>
  </section>
</template>

<style scoped>
.detail {
  padding: 20px 24px;
}
.head {
  display: flex;
  align-items: center;
  gap: 10px;
}
.status {
  flex: 1;
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}
.prompt {
  margin: 8px 0 12px;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
  white-space: pre-line;
  overflow-wrap: anywhere;
}
.metrics {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 16px;
  margin: 0 0 16px;
  font-size: 12px;
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
}
.files {
  margin-top: 24px;
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
