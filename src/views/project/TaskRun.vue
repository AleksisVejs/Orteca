<script setup lang="ts">
import { computed, inject } from "vue";
import ActivityLog from "./ActivityLog.vue";
import FileLink from "./FileLink.vue";
import Markdown from "../../components/Markdown.vue";
import Orb from "../../components/Orb.vue";
import { PROJECT } from "./state";

// The page while a run is going. Stop and steering are the only other things worth doing.
const {
  activeRun, said, describeVerdict, currentActivity, fileError, taskId, stopping, stopRun,
  instruction, sending, instructionError, steering, instruct, checking, lines, ranOn, stageLabel,
} = inject(PROJECT)!;
const responses = computed(() => lines.value.filter((line) => line.kind === "text"));
const response = computed(() => responses.value.at(-1)?.text);
const stage = computed(() => activeRun.value?.progress?.current.map((i) => stageLabel(activeRun.value!.progress!.stages[i] ?? "Working")).join(" + ") ?? "Preparing");
const receipt = computed(() => [...lines.value].reverse().find((line) => line.kind === "instruction")?.delivery);
</script>

<template>
  <!-- Everything already said in this task, so a follow-up reads as one thread. -->
  <article v-for="(t, i) in activeRun?.turns ?? []" :key="i" class="turn">
    <p class="said">{{ t.said }}</p>
    <Markdown v-if="t.summary" class="summary" :text="describeVerdict(t.summary) ?? t.summary" />
    <p v-else class="note">{{ t.failure ?? "No summary was reported." }}</p>
  </article>

  <section class="card head">
    <div class="run-meta"><span class="dot live" aria-hidden="true"></span><strong>{{ stopping ? "Stopping" : "Running" }}</strong><span class="note">{{ ranOn === "codex" ? "Codex" : "Claude" }}</span></div>
    <h1 class="prompt" :class="{ long: said.length > 140 }">{{ said }}</h1>
    <p v-if="activeRun?.attachmentCount" class="note">
      {{ activeRun.attachmentCount }} attached {{ activeRun.attachmentCount === 1 ? "item" : "items" }}
    </p>
    <div class="now" role="status" aria-live="polite">
      <Orb :size="64" />
      <div class="grow current"><strong>{{ ranOn === "codex" ? "Codex" : "Claude" }} · {{ stopping ? "Stopping" : stage }}</strong><p>
        {{ currentActivity.text }}
        <FileLink v-if="currentActivity.file" :file="currentActivity.file" />
      </p></div>
      <button class="btn" :disabled="taskId === null || stopping" @click="stopRun">
        {{ stopping ? "Stopping…" : "Stop" }}
      </button>
    </div>
    <p v-if="fileError" class="missing">{{ fileError }}</p>
    <!-- Blue means in flight, and only that. -->
    <div class="bar"><span></span></div>
  </section>

  <section v-if="activeRun?.progress" class="execution block" aria-label="Live execution">
    <h2 class="label">Execution</h2>
    <ol class="live-stages">
      <li v-for="(name, i) in activeRun.progress.stages" :key="i" :class="{ current: activeRun.progress.current.includes(i) }" :aria-current="activeRun.progress.current.includes(i) ? 'step' : undefined">
        <span>{{ i + 1 }}</span><div>{{ stageLabel(name) }}<small>{{ activeRun.progress.current.includes(i) ? 'Running' : i < Math.min(...activeRun.progress.current) ? 'Ran' : 'Upcoming' }}</small></div>
      </li>
    </ol>
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

  <section v-if="response" class="response block" aria-label="Latest AI response">
    <div class="response-head"><span class="note">{{ ranOn === "codex" ? "Codex" : "Claude" }}</span><h2 class="label">AI response</h2><span class="note grow">Latest update</span></div>
    <Markdown class="summary" :text="response" />
    <details v-if="responses.length > 1" class="previous-updates">
      <summary>Earlier updates · {{ responses.length - 1 }}</summary>
      <Markdown v-for="(line, i) in responses.slice(0, -1)" :key="i" class="summary" :text="line.text" />
    </details>
  </section>
  <section class="block">
    <div class="section-heading"><h2 class="label">Activity</h2><span class="note">{{ lines.length }} events</span></div>
    <ActivityLog />
  </section>

  <section class="block">
    <h2 class="label">Steer task</h2>
    <div class="card steer">
      <input
        v-model="instruction"
        type="text"
        aria-label="Additional instructions for this task"
        spellcheck="false"
        placeholder="Add context or change direction…"
        :disabled="taskId === null || sending"
        @keyup.enter="instruct(false)"
      />
      <div class="steer-row">
        <button class="btn primary" :disabled="taskId === null || sending || !instruction.trim()" @click="instruct(false)">
          {{ steering === "live" ? "Send to agent" : "Send for next step" }}
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
          <template v-if="steering === 'live'">Delivered to the running agent.</template>
          <template v-else>Used on the next step. “Apply now” restarts that step and keeps anything already changed.</template>
        </span>
      </div>
      <p v-if="receipt" class="note receipt" role="status">{{ receipt }}</p>
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
.prompt.long {
  font-size: 14px;
  font-weight: 500;
  line-height: 1.5;
  letter-spacing: 0;
  max-height: 7.5em;
  overflow-y: auto;
}
.head .note {
  margin: 0;
}
.now {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 16px;
  padding: 16px 18px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r);
}
.now strong {
  font-weight: 500;
  color: var(--text-dim);
  min-width: 0;
  overflow-wrap: anywhere;
}
.now .btn {
  border-radius: 999px;
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
  margin-top: 24px;
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
.run-meta, .section-heading, .response-head { display: flex; align-items: center; gap: 12px; }
.run-meta { margin-bottom: 12px; }
.current { min-width: 0; }
.current p { margin: 6px 0 0; color: var(--text-dim); overflow-wrap: anywhere; }
.section-heading { justify-content: space-between; margin-bottom: 12px; }
.section-heading .label, .response-head .label { margin: 0; }
.response { padding: 18px; border: 1px solid var(--border); border-radius: var(--r); background: var(--surface); }
.response-head { margin-bottom: 16px; }
.response-head .grow { text-align: right; }
.response .summary { max-height: 280px; overflow: auto; }
.now .btn { flex-shrink: 0; }
.previous-updates { margin-top: 16px; color: var(--text-dim); }
.previous-updates summary { cursor: pointer; font-size: 12px; }
.previous-updates .summary { padding-top: 12px; border-top: 1px solid var(--border); }
@media (max-width: 600px) {
  .now { gap: 8px; padding: 12px; }
  .response-head { flex-wrap: wrap; }
  .steer-row .note { flex-basis: 100%; }
}
.receipt { padding: 0 18px 12px; margin: 0; }
.live-stages { display: flex; flex-wrap: wrap; gap: 12px; padding: 0; list-style: none; }
.live-stages li { display: flex; align-items: center; gap: 10px; padding: 12px; flex: 1 0 120px; border: 1px solid var(--border); border-radius: var(--r); color: var(--text-faint); }
.live-stages li.current { border-color: var(--info); color: var(--text); }
.live-stages li > span { font-variant-numeric: tabular-nums; }
.live-stages small { display: block; font-size: 11px; margin-top: 4px; }
</style>
