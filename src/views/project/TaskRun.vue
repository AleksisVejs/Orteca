<script setup lang="ts">
import { inject } from "vue";
import ActivityLog from "./ActivityLog.vue";
import FileLink from "./FileLink.vue";
import { PROJECT } from "./state";

// The page while a run is going. Stop and steering are the only other things worth doing.
const {
  task, attachments, currentActivity, fileError, taskId, stopping, stopRun,
  instruction, sending, instructionError, steering, instruct, checking,
} = inject(PROJECT)!;
</script>

<template>
  <section class="card head">
    <p class="prompt">{{ task }}</p>
    <p v-if="attachments.length" class="note">
      {{ attachments.length }} attached {{ attachments.length === 1 ? "item" : "items" }}
    </p>
    <div class="now" role="status" aria-live="polite">
      <span class="dot live" aria-hidden="true"></span>
      <strong class="grow">
        {{ currentActivity.text }}
        <FileLink v-if="currentActivity.file" :file="currentActivity.file" />
      </strong>
      <button class="btn" :disabled="taskId === null || stopping" @click="stopRun">
        {{ stopping ? "Stopping…" : "Stop" }}
      </button>
    </div>
    <p v-if="fileError" class="missing">{{ fileError }}</p>
    <!-- Blue means in flight, and only that. -->
    <div class="bar"><span></span></div>
  </section>

  <!-- Result first, proof after: readable now, never labelled done. -->
  <section v-if="checking" class="block" aria-live="polite">
    <h2 class="label">The change · still checking</h2>
    <div class="card early">
      <p class="note">Its focused tests passed. The full test suite is still running, and this is not done until it passes.</p>
      <ul class="files">
        <li v-for="f in checking.diff.filter((f) => f.origin !== 'beforeRun')" :key="f.path">
          <span class="mono grow">{{ f.path }}</span>
          <span v-if="f.added !== null" class="note">+{{ f.added }} &minus;{{ f.deleted }}</span>
          <span v-else class="note">new or binary</span>
        </li>
      </ul>
      <details v-if="checking.patchText" class="code-view">
        <summary>View patch</summary>
        <pre>{{ checking.patchText }}</pre>
      </details>
    </div>
  </section>

  <section class="block">
    <h2 class="label">Activity</h2>
    <ActivityLog />
  </section>

  <section class="block">
    <h2 class="label">Want to add something?</h2>
    <div class="card steer">
      <input
        v-model="instruction"
        type="text"
        aria-label="Additional instructions for this task"
        spellcheck="false"
        :placeholder="steering === 'live' ? 'Say something to it…' : 'Something for the next step…'"
        :disabled="taskId === null || sending"
        @keyup.enter="instruct(false)"
      />
      <div class="steer-row">
        <button class="btn" :disabled="taskId === null || sending || !instruction.trim()" @click="instruct(false)">
          Send
        </button>
        <button
          v-if="steering === 'checkpoint'"
          class="btn"
          :disabled="taskId === null || sending || !instruction.trim()"
          @click="instruct(true)"
        >
          Apply now
        </button>
        <span class="note">
          <template v-if="steering === 'live'">I’ll use this for the next step.</template>
          <template v-else>Used on the next step. “Apply now” restarts that step and keeps anything already changed.</template>
        </span>
      </div>
      <p v-if="instructionError" class="missing err-line">{{ instructionError }}</p>
    </div>
  </section>
</template>

<style scoped>
.head {
  position: relative;
  overflow: hidden;
  padding: 0 0 20px;
  background: none;
  border: none;
  border-radius: 0;
}
.prompt {
  margin: 0 0 4px;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
  white-space: pre-line;
  overflow-wrap: anywhere;
}
.head .note {
  margin: 0;
}
.now {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 16px;
}
.now strong {
  font-weight: 500;
}

.bar {
  position: absolute;
  inset: auto 0 0;
  height: 2px;
  background: var(--border);
}
.bar span {
  display: block;
  width: 35%;
  height: 100%;
  background: var(--info);
  animation: slide 1.4s ease-in-out infinite;
}
@keyframes slide {
  from {
    transform: translateX(-100%);
  }
  to {
    transform: translateX(300%);
  }
}
@media (prefers-reduced-motion: reduce) {
  .bar span {
    width: 100%;
    opacity: 0.5;
    animation: none;
  }
}

.block {
  margin-top: 32px;
}

.early {
  padding: 16px 18px;
}
.early > .note {
  margin: 0 0 12px;
}
.files {
  list-style: none;
  margin: 0 0 12px;
  padding: 0;
}
.files li {
  display: flex;
  align-items: baseline;
  gap: 12px;
  padding: 8px 0;
}
.files li + li {
  border-top: 1px solid var(--border);
}
.files .mono {
  font-size: 12px;
  overflow-wrap: anywhere;
}
.files .note {
  font-variant-numeric: tabular-nums;
}

.steer:focus-within {
  border-color: var(--focus);
}
.steer input {
  display: block;
  width: 100%;
  padding: 14px 18px 4px;
  background: none;
  color: var(--text);
  border: none;
  font: inherit;
}
.steer input::placeholder {
  color: var(--text-faint);
}
.steer input:focus {
  outline: none;
}
.steer-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  padding: 10px 12px 12px;
}
.steer-row .btn {
  padding: 5px 14px;
  font-size: 12px;
}
.err-line {
  margin: 0;
  padding: 0 18px 12px;
}
</style>
