<script setup lang="ts">
import { computed, inject, ref } from "vue";
import FileLink from "./FileLink.vue";
import Orb from "../../components/Orb.vue";
import { PROJECT } from "./state";

// The chat box while a run goes: what the agent is doing now, Stop, and steering,
// as one pinned unit. The reply box takes its place when the run ends.
const {
  activeRun, currentActivity, fileError, taskId, stopping, stopRun,
  instruction, sending, instructionError, steering, instruct, ranOn, stageLabel,
  waitAsk, answerWaitFor, attachments, attachError, addAttachments, pasteImages, fileName,
} = inject(PROJECT)!;
// Orteca runs a command the agent handed over; a message now is a question the
// paused session answers at once, whichever provider it is.
const waiting = computed(() => currentActivity.value.text.startsWith("Waiting on "));
const stage = computed(() => activeRun.value?.progress?.current.map((i) => stageLabel(activeRun.value!.progress!.stages[i] ?? "Working")).join(" + "));
// The route's steps, told apart only by what the runner reported: running, ran, or not reached yet.
const steps = computed(() => {
  const p = activeRun.value?.progress;
  if (!p) return [];
  const first = Math.min(...p.current);
  return p.stages.map((name, i) => ({ name: stageLabel(name), state: p.current.includes(i) ? "running" : i < first ? "ran" : "upcoming" }));
});
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
const steerInput = ref<HTMLTextAreaElement | null>(null);
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

// Enter sends, Shift+Enter breaks the line: the same as the reply box after.
function onEnter(e: KeyboardEvent) {
  if (e.isComposing) return;
  e.preventDefault();
  if (taskId.value !== null && !sending.value && instruction.value.trim()) void send(false);
}

// Where the orb stands, for the page to land it in the heading's dot when the run ends.
defineExpose({ orbRect: () => orb.value?.canvas?.getBoundingClientRect() ?? null });
</script>

<template>
  <p v-if="fileError" class="missing">{{ fileError }}</p>
  <div class="composer">
    <div class="now" role="status" aria-live="polite">
      <Orb ref="orb" class="orb" :size="36" :mood="mood" />
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
    </div>
    <!-- Running it is consent, so neither button is the primary one. -->
    <div v-if="waitAsk" class="composer-row steer-row" role="group" aria-label="Command the agent asks Orteca to run">
      <span class="grow">Run this for the agent? <span class="mono">{{ waitAsk }}</span></span>
      <button class="btn" @click="answerWaitFor(false)">Don’t run</button>
      <button class="btn" @click="answerWaitFor(true)">Run it</button>
    </div>
    <textarea
      ref="steerInput"
      v-model="instruction"
      rows="1"
      aria-label="Additional instructions for this task"
      spellcheck="false"
      :placeholder="waiting ? 'Ask anything while it runs…' : 'Steer: add context or change direction…'"
      :disabled="taskId === null || sending"
      @paste="pasteImages"
      @keydown.enter.exact="onEnter"
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
    <div class="composer-row steer-row tools">
      <button class="icon" title="Attach files or images. You can also paste or drop them." aria-label="Attach files or images" @click="addAttachments(false)">
        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M10.5 4.5 5.8 9.2a1.4 1.4 0 0 0 2 2l5-5a2.8 2.8 0 0 0-4-4l-5 5a4.2 4.2 0 0 0 6 6l4.2-4.2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
      </button>
      <button class="icon" title="Attach a folder" aria-label="Add folder" @click="addAttachments(true)">
        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
      </button>
      <span class="note grow">
        <template v-if="waiting">The agent answers now and changes nothing while the command runs.</template>
        <template v-else-if="steering === 'live' && attachments.length">Anything outside this project restarts the step so Claude can open it.</template>
        <template v-else-if="steering === 'live'">Delivered to the running agent.</template>
        <template v-else>Used on the next step. “Apply now” restarts that step and keeps anything already changed.</template>
      </span>
      <button
        v-if="steering === 'checkpoint' && !waiting"
        class="btn"
        :disabled="taskId === null || sending || !instruction.trim()"
        @click="send(true)"
      >
        Apply now
      </button>
      <button class="btn primary" :disabled="taskId === null || sending || !instruction.trim()" @click="send(false)">
        {{ waiting ? "Ask" : steering === "live" ? "Send to agent" : "Send for next step" }}
      </button>
    </div>
    <p v-if="instructionError" class="missing err-line">{{ instructionError }}</p>
  </div>
  <span ref="flyer" class="flyer" aria-hidden="true"></span>
</template>

<style scoped src="./result.css"></style>
<style scoped src="./chat.css"></style>
<style scoped>
/* The reply box with a status line on top: same edge, same spacing, no divider. */
.now {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 12px 0 12px;
}
.now strong {
  font-weight: 500;
  color: var(--text-dim);
  min-width: 0;
  overflow-wrap: anywhere;
}
.now .btn {
  flex-shrink: 0;
  padding: 5px 14px;
  font-size: 12px;
}
/* The run starting: the status line fades in and the orb grows into place. */
@media (prefers-reduced-motion: no-preference) {
  .now { animation: now-in 240ms ease both; }
  .now .orb { animation: orb-in 420ms cubic-bezier(0.2, 0, 0, 1) both; }
}
@keyframes now-in { from { opacity: 0; } }
@keyframes orb-in { from { opacity: 0; transform: scale(0.3); } }
.current { min-width: 0; }
.current p { margin: 2px 0 0; font-size: 12px; color: var(--text-faint); overflow-wrap: anywhere; }

/* Icons first, lined up as in the reply box. */
.tools {
  padding-left: 10px;
}

.err-line {
  margin: 0;
  padding: 0 18px 12px;
}

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
  .now { gap: 8px; padding: 10px 12px 0; }
  .steer-row { flex-wrap: wrap; }
  .steer-row .note { flex-basis: 100%; white-space: normal; }
}
</style>
