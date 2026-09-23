<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, ref, watch } from "vue";
import ActivityLog from "./ActivityLog.vue";
import FileLink from "./FileLink.vue";
import Markdown from "../../components/Markdown.vue";
import Orb from "../../components/Orb.vue";
import { PROJECT } from "./state";

// The page while a run is going, laid out as a chat: what was said scrolls above,
// and the orb, Stop and steering sit together in the box pinned at the bottom.
const {
  activeRun, said, describeVerdict, currentActivity, fileError, taskId, stopping, stopRun,
  instruction, sending, instructionError, steering, instruct, checking, lines, ranOn, stageLabel,
} = inject(PROJECT)!;
// The conversation in order: the agent's messages and every steer the user sent.
const messages = computed(() => lines.value.filter((line) => line.kind === "text" || line.kind === "instruction"));
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

// Like any chat, new messages keep the pane at the bottom, unless the reader scrolled up.
const chat = ref<HTMLDivElement | null>(null);
const pane = () => chat.value?.parentElement;
let pinned = true;
function onPaneScroll() {
  const p = pane();
  if (p) pinned = p.scrollHeight - p.scrollTop - p.clientHeight < 48;
}
function toBottom() {
  const p = pane();
  if (p) p.scrollTop = p.scrollHeight;
}
onMounted(() => {
  pane()?.addEventListener("scroll", onPaneScroll, { passive: true });
  toBottom();
});
onBeforeUnmount(() => pane()?.removeEventListener("scroll", onPaneScroll));
watch(() => `${lines.value.length}:${lines.value.at(-1)?.text.length}:${!!checking.value}`, () => { if (pinned) toBottom(); }, { flush: "post" });
watch(() => activeRun.value?.key, () => { pinned = true; toBottom(); }, { flush: "post" });
</script>

<template>
  <div ref="chat" class="chat">
    <div class="thread">
      <!-- Everything already said in this task, so a follow-up reads as one conversation. -->
      <template v-for="(t, i) in activeRun?.turns ?? []" :key="i">
        <p class="bubble">{{ t.said }}</p>
        <Markdown v-if="t.summary" class="summary back" :text="describeVerdict(t.summary) ?? t.summary" />
        <p v-else class="note back">{{ t.failure ?? "No summary was reported." }}</p>
      </template>

      <div class="mine">
        <h1 class="bubble" :class="{ long: said.length > 600 }">{{ said }}</h1>
        <p v-if="activeRun?.attachments.length" class="note">
          {{ activeRun.attachments.length }} attached {{ activeRun.attachments.length === 1 ? "item" : "items" }}
        </p>
      </div>

      <!-- The tool calls behind the answer, folded away the way a chat folds its "worked for…" line. -->
      <details class="activity" @toggle="showActivity = ($event.target as HTMLDetailsElement).open">
        <summary>Activity · {{ lines.length }} events</summary>
        <ActivityLog v-if="showActivity" />
      </details>

      <template v-for="(m, i) in messages" :key="i">
        <div v-if="m.kind === 'instruction'" class="mine">
          <p class="bubble">{{ m.text }}</p>
          <p v-if="m.delivery" class="note" role="status">{{ m.delivery }}</p>
        </div>
        <Markdown v-else class="summary back" :text="m.text" />
      </template>

      <!-- Result first, proof after: readable now, never labelled done. -->
      <section v-if="checking" class="back" aria-live="polite">
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
    </div>

    <!-- The chat box: what the agent is doing now, Stop, and steering, as one pinned unit. -->
    <div class="composer-bar">
      <p v-if="fileError" class="missing">{{ fileError }}</p>
      <div class="composer">
        <div class="now" role="status" aria-live="polite">
          <Orb ref="orb" :size="40" :mood="mood" />
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
          <span class="note grow">
            <template v-if="steering === 'live'">Delivered to the running agent.</template>
            <template v-else>Used on the next step. “Apply now” restarts that step and keeps anything already changed.</template>
          </span>
          <button
            v-if="steering === 'checkpoint'"
            class="btn"
            :disabled="taskId === null || sending || !instruction.trim()"
            @click="send(true)"
          >
            Apply now
          </button>
          <button class="btn primary" :disabled="taskId === null || sending || !instruction.trim()" @click="send(false)">
            {{ steering === "live" ? "Send to agent" : "Send for next step" }}
          </button>
        </div>
        <p v-if="instructionError" class="missing err-line">{{ instructionError }}</p>
      </div>
    </div>
    <span ref="flyer" class="flyer" aria-hidden="true"></span>
  </div>
</template>

<style scoped src="./chat.css"></style>
<style scoped>
.activity {
  color: var(--text-faint);
}
.activity > summary {
  width: fit-content;
  font-size: 12px;
  cursor: pointer;
}
.activity[open] > summary {
  margin-bottom: 12px;
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

.composer {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r);
}
.composer:focus-within {
  border-color: var(--focus);
}
.now {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 14px;
  border-bottom: 1px solid var(--border);
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
.current p { margin: 2px 0 0; font-size: 12px; color: var(--text-faint); overflow-wrap: anywhere; }

.steps {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 16px;
  margin: 8px 0 0;
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

.composer input {
  display: block;
  width: 100%;
  padding: 14px 18px 4px;
  background: none;
  color: var(--text);
  border: none;
  font: inherit;
}
.composer input::placeholder {
  color: var(--text-faint);
}
.composer input:focus {
  outline: none;
}
.steer-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 12px 12px 18px;
}
.steer-row .btn {
  flex-shrink: 0;
  padding: 5px 14px;
  font-size: 12px;
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
  .now { gap: 8px; padding: 10px 12px; }
  .steer-row { flex-wrap: wrap; }
  .steer-row .note { flex-basis: 100%; white-space: normal; }
}
</style>
