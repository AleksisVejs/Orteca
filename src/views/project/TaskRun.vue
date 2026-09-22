<script setup lang="ts">
import { computed, inject, ref } from "vue";
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
const stage = computed(() => activeRun.value?.progress?.current.map((i) => stageLabel(activeRun.value!.progress!.stages[i] ?? "Working")).join(" + "));
// The route's steps, told apart only by what the runner reported: running, ran, or not reached yet.
const steps = computed(() => {
  const p = activeRun.value?.progress;
  if (!p) return [];
  const first = Math.min(...p.current);
  return p.stages.map((name, i) => ({ name: stageLabel(name), state: p.current.includes(i) ? "running" : i < first ? "ran" : "upcoming" }));
});
const showActivity = ref(false);
// What the orb acts out: the current activity's first word decides it.
const mood = computed(() => {
  const { text, failed } = currentActivity.value;
  if (stopping.value) return "stop";
  if (failed || /^(Edit failed|Couldn)/.test(text)) return "error";
  if (text.startsWith("Editing")) return "edit";
  if (text.startsWith("Reading")) return "read";
  if (/^(Testing|Checking)/.test(text)) return "check";
  if (text.startsWith("Thinking")) return "think";
  return "idle";
});
// A delivered steer flies into the orb, which swallows it. Nothing flies if it wasn't delivered.
const orb = ref<InstanceType<typeof Orb> | null>(null);
const steerInput = ref<HTMLInputElement | null>(null);
const flyer = ref<HTMLSpanElement | null>(null);
async function send(now: boolean) {
  const text = instruction.value.trim();
  const from = steerInput.value?.getBoundingClientRect();
  await instruct(now);
  const el = flyer.value;
  const to = orb.value?.canvas?.getBoundingClientRect();
  if (!text || instruction.value || !from || !el || !to || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  el.textContent = text;
  const x = to.left + to.width / 2 - el.offsetWidth / 2;
  const y = to.top + to.height / 2 - el.offsetHeight / 2;
  el.animate([
    { transform: `translate(${from.left + 18}px, ${from.top + 14}px)`, opacity: 1 },
    { transform: `translate(${x}px, ${y}px) scale(0.05)`, opacity: 0, filter: "blur(2px)" },
  ], { duration: 650, easing: "cubic-bezier(0.55, 0, 0.75, 0.3)" }).finished.then(() => orb.value?.absorb(), () => {});
}
const receipt = computed(() => [...lines.value].reverse().find((line) => line.kind === "instruction")?.delivery);
</script>

<template>
  <!-- Everything already said in this task, so a follow-up reads as one thread. -->
  <article v-for="(t, i) in activeRun?.turns ?? []" :key="i" class="turn">
    <p class="who">You</p>
    <p class="said">{{ t.said }}</p>
    <Markdown v-if="t.summary" class="summary" :text="describeVerdict(t.summary) ?? t.summary" />
    <p v-else class="note">{{ t.failure ?? "No summary was reported." }}</p>
  </article>

  <p class="who">You</p>
  <h1 class="prompt" :class="{ long: said.length > 140 }">{{ said }}</h1>
  <p v-if="activeRun?.attachments.length" class="note attached">
    {{ activeRun.attachments.length }} attached {{ activeRun.attachments.length === 1 ? "item" : "items" }}
  </p>

  <!-- One card for "what is it doing": who, which step, what right now, and Stop. -->
  <section class="card now" role="status" aria-live="polite">
    <Orb ref="orb" :size="64" :mood="mood" />
    <div class="grow current">
      <strong>{{ ranOn === "codex" ? "Codex" : "Claude" }} · {{ stopping ? "Stopping" : stage ?? "Running" }}</strong>
      <p>
        {{ currentActivity.text }}
        <FileLink v-if="currentActivity.file" :file="currentActivity.file" />
      </p>
      <ol v-if="steps.length > 1" class="steps" aria-label="Steps">
        <li v-for="(s, i) in steps" :key="i" :class="s.state" :aria-current="s.state === 'running' ? 'step' : undefined">
          {{ s.name }}<span class="hidden-label"> · {{ s.state === "running" ? "Running" : s.state === "ran" ? "Ran" : "Upcoming" }}</span>
        </li>
      </ol>
    </div>
    <button class="btn" :disabled="taskId === null || stopping" @click="stopRun">
      {{ stopping ? "Stopping…" : "Stop" }}
    </button>
  </section>
  <p v-if="fileError" class="missing">{{ fileError }}</p>

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

  <!-- What it has said so far, oldest first, straight under the card: part of the turn, not a panel. -->
  <section v-if="response" class="block" aria-label="Latest AI response">
    <details v-if="responses.length > 1" class="previous-updates">
      <summary>Earlier updates · {{ responses.length - 1 }}</summary>
      <Markdown v-for="(line, i) in responses.slice(0, -1)" :key="i" class="summary" :text="line.text" />
    </details>
    <Markdown class="summary" :text="response" />
  </section>

  <!-- Closed by default; mounted on open so the log starts at its latest line. -->
  <details class="block activity" @toggle="showActivity = ($event.target as HTMLDetailsElement).open">
    <summary><span class="label">Activity</span><span class="note">{{ lines.length }} events</span></summary>
    <ActivityLog v-if="showActivity" />
  </details>

  <!-- Pinned to the bottom of the pane, like a chat box: always in reach while the thread scrolls. -->
  <div class="steer-bar">
    <div class="steer">
      <input
        ref="steerInput"
        v-model="instruction"
        type="text"
        aria-label="Additional instructions for this task"
        spellcheck="false"
        placeholder="Steer: add context or change direction…"
        :disabled="taskId === null || sending"
        @keyup.enter="send(false)"
      />
      <div class="steer-row">
        <button class="btn primary" :disabled="taskId === null || sending || !instruction.trim()" @click="send(false)">
          {{ steering === "live" ? "Send to agent" : "Send for next step" }}
        </button>
        <button
          v-if="steering === 'checkpoint'"
          class="btn"
          :disabled="taskId === null || sending || !instruction.trim()"
          @click="send(true)"
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
  </div>
  <span ref="flyer" class="flyer" aria-hidden="true"></span>
</template>

<style scoped>
.prompt {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  white-space: pre-line;
  overflow-wrap: anywhere;
}
.prompt.long {
  max-height: 7.5em;
  overflow-y: auto;
}
.attached {
  margin: 4px 0 0;
}

.now {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 16px;
  padding: 16px 18px;
}
.now strong {
  font-weight: 500;
  color: var(--text-dim);
  min-width: 0;
  overflow-wrap: anywhere;
}
.now .btn {
  flex-shrink: 0;
  border-radius: 999px;
}
.current { min-width: 0; }
.current p { margin: 6px 0 0; color: var(--text-dim); overflow-wrap: anywhere; }

.steps {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 16px;
  margin: 10px 0 0;
  padding: 0;
  list-style: none;
  font-size: 12px;
  color: var(--text-faint);
}
.steps li {
  display: flex;
  align-items: center;
  gap: 6px;
}
.steps li::before {
  content: "";
  width: 6px;
  height: 6px;
  border: 1px solid var(--border-strong);
  border-radius: 999px;
}
.steps .ran { color: var(--text-dim); }
.steps .ran::before { background: var(--text-dim); border-color: var(--text-dim); }
.steps .running { color: var(--text); }
.steps .running::before { background: var(--info); border-color: var(--info); }

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

.summary { color: var(--text-dim); }
.previous-updates { margin-bottom: 16px; color: var(--text-dim); }
.previous-updates summary { cursor: pointer; font-size: 12px; }
.previous-updates .summary { padding-bottom: 12px; border-bottom: 1px solid var(--border); }

.activity > summary {
  display: flex;
  align-items: center;
  gap: 12px;
  cursor: pointer;
}
.activity > summary .label { margin: 0; }
.activity[open] > summary { margin-bottom: 12px; }

/* Sticks to the bottom of the scrolling pane; its own background hides the thread passing under it. */
.steer-bar {
  position: sticky;
  bottom: 0;
  margin-top: 32px;
  padding: 12px 0 16px;
  background: var(--bg);
}
.steer {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r);
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
.receipt { padding: 0 18px 12px; margin: 0; }

.flyer {
  position: fixed;
  top: 0;
  left: 0;
  z-index: 10;
  max-width: 40ch;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text);
  opacity: 0;
  pointer-events: none;
}

@media (max-width: 600px) {
  .now { gap: 8px; padding: 12px; }
  .steer-row .note { flex-basis: 100%; }
}
</style>
