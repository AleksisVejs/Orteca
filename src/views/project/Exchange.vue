<script setup lang="ts">
import { computed, inject, ref } from "vue";
import { parseHistoryPatch } from "./historyPatch";
import { unfinishedSummary } from "./taskPresentation";
import Markdown from "../../components/Markdown.vue";
import Story from "./Story.vue";
import { PROJECT } from "./state";
import type { Exchange } from "./state";

// One exchange in a task's chat: what the user said, what the agent said on the
// way, its answer, and a footer line whose proof opens over the chat. Every
// answer keeps its own, so a reply never takes an earlier answer's proof away.
const props = defineProps<{
  idPrefix: string;
  prompt: string;
  data: Exchange;
  /** An exchange above the current one: its outcome is in its footer, not the heading. */
  earlier?: boolean;
  /** Offer "Edit" under the user's words; only the current exchange does. */
  editable?: boolean;
  editDisabled?: boolean;
}>();
const emit = defineEmits<{ editAgain: [] }>();

const { domId, describeVerdict, stageLabel, formatTokens, formatCost, formatDuration, HISTORY_STATUS, TONE } = inject(PROJECT)!;

const id = (name: string) => domId(`${props.idPrefix}-${name}`);
const d = computed(() => props.data);

const PROOF = [
  { id: "files", label: "Files" },
  { id: "details", label: "Details" },
  { id: "activity", label: "Activity" },
] as const;
// A plain answer that touched nothing has no files or checks worth a line.
const quiet = computed(() => d.value.route?.kind === "answer" && d.value.status === "done" && !d.value.changed.byRun.length);
const proof = computed(() => (quiet.value ? PROOF.filter((t) => t.id !== "files") : PROOF));
const tab = ref<(typeof PROOF)[number]["id"]>("details");

// The proof opens over the chat, so reading it never moves the conversation.
const dialog = ref<HTMLDialogElement | null>(null);
function openProof(t: typeof tab.value) {
  tab.value = t;
  dialog.value?.showModal();
}
defineExpose({ close: () => dialog.value?.close() });

// The final answer is also the agent's last message; say it once, as the answer.
const talk = computed(() => {
  const out = [...d.value.messages];
  const answer = d.value.summary?.trim() ?? "";
  while (answer && out.at(-1)?.kind === "text" && answer.includes(out.at(-1)!.text.trim())) out.pop();
  return out;
});

const selectedFile = ref<string | null>(null);
const parsedPatch = computed(() => parseHistoryPatch(d.value.patchText ?? ""));
const selectedPath = computed(() => d.value.changed.byRun.some((f) => f.path === selectedFile.value) ? selectedFile.value : d.value.changed.byRun[0]?.path);
const selectedPatch = computed(() => parsedPatch.value.files.find((f) => f.path === selectedPath.value));
</script>

<template>
  <section class="exchange" :class="{ earlier }">
    <div class="mine">
      <component :is="earlier ? 'p' : 'h1'" class="bubble" :class="{ long: prompt.length > 600 }">{{ prompt }}</component>
      <button v-if="editable" class="link edit" :disabled="editDisabled" title="Back to the composer with these words, to change them and run again" @click="emit('editAgain')">Edit</button>
    </div>

    <!-- The way there, as it streamed; folded so the answer reads first. -->
    <details v-if="talk.length" class="story back">
      <summary class="note">How it got there · {{ talk.length }} {{ talk.length === 1 ? "step" : "steps" }}</summary>
      <div class="story-body"><Story :lines="talk" /></div>
    </details>

    <!-- The answer, in the place the last message streamed into; the facts and proof under it. -->
    <article class="back answer">
      <p v-if="d.failure" class="missing">{{ d.failure }}</p>
      <slot name="notes" />
      <Markdown v-if="d.summary" class="summary" :text="describeVerdict(d.summary) ?? d.summary" />
      <p v-else-if="!d.failure" class="note">No summary was reported.</p>
      <p v-if="d.status === 'cancelled'" class="note caveat">
        Stopped part-way. Anything the agent had already written is still on disk - Orteca reverts nothing.
      </p>
      <slot name="after" />
      <p v-if="d.unknownEvents" class="note caveat" role="status">
        {{ d.unknownEvents }} provider event{{ d.unknownEvents === 1 ? "" : "s" }} were not recognized and remain in the saved task log.
      </p>
      <div class="outcome-summary">
        <span v-if="earlier" class="outcome"><span class="dot" :class="TONE[d.status as keyof typeof TONE]" aria-hidden="true"></span>{{ HISTORY_STATUS[d.status as keyof typeof HISTORY_STATUS] ?? d.status }}</span>
        <template v-if="!quiet">
          <span>{{ d.changed.byRun.length }} files changed</span><span>{{ d.verification }}</span><span>{{ unfinishedSummary(d.status) }}</span>
        </template>
        <div class="proof">
          <button v-for="t in proof" :key="t.id" class="proof-toggle" aria-haspopup="dialog" @click="openProof(t.id)">
            {{ t.label }}<span v-if="t.id === 'files'" class="count">{{ d.changed.byRun.length }}</span>
          </button>
        </div>
      </div>
    </article>

    <dialog ref="dialog" class="proof-modal" :aria-labelledby="id('proof-title')" @click="$event.target === dialog && dialog?.close()">
      <header>
        <h2 :id="id('proof-title')" class="hidden-label">How this answer was reached</h2>
        <div class="proof">
          <button v-for="t in proof" :key="t.id" class="proof-toggle" :aria-pressed="tab === t.id" @click="tab = t.id">
            {{ t.label }}<span v-if="t.id === 'files'" class="count">{{ d.changed.byRun.length }}</span>
          </button>
        </div>
        <button class="close" title="Close" aria-label="Close" @click="dialog?.close()">×</button>
      </header>
      <div class="panel">
        <template v-if="tab === 'files'">
          <slot name="files-top" />
          <div v-if="d.changed.byRun.length" class="file-review">
            <ul class="review-files" aria-label="Changed files">
              <li v-for="f in d.changed.byRun" :key="f.path">
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
              <p v-if="d.changed.byRun.find((f) => f.path === selectedPath)?.origin === 'both'" class="note">This patch includes changes already present before the task.</p>
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
          <p v-if="d.changed.unknown" class="note caveat">Some changes could not be attributed to this task.</p>
          <p v-if="d.changed.beforeRun.length" class="note caveat">Already changed before this task and left untouched: <span class="mono">{{ d.changed.beforeRun.map((f) => f.path).join(', ') }}</span></p>
          <p v-for="notice in parsedPatch.notices" :key="notice" class="note">{{ notice }}</p>
          <details v-if="d.patchText" class="code-view"><summary>View full patch</summary><pre>{{ d.patchText }}</pre></details>
        </template>

        <template v-else-if="tab === 'details'">
          <!-- The route as decided before anything ran: what ran, what did not. -->
          <section class="execution" aria-label="Execution">
            <div class="section-heading"><h2 class="label">Execution</h2><span class="note">Steps taken for this answer</span></div>
            <ol v-if="d.routeSteps.length" class="route">
              <template v-for="(step, i) in d.routeSteps" :key="i">
                <li v-if="i" class="arrow" aria-hidden="true">→</li>
                <li :class="{ ran: step.ran }"><span class="step-number" aria-hidden="true">{{ i + 1 }}</span><div>
                  {{ stageLabel(step.stage) }}<span v-if="step.asked" class="mono"> · {{ step.asked }}</span><small>{{ step.ran ? "Ran" : "Not started" }}</small></div>
                </li>
              </template>
            </ol>
            <p v-if="d.route" class="note reason">
              {{ d.route.reason }}
              <template v-if="d.route.candidatePaths.length">
                · brief named <span class="mono">{{ d.route.candidatePaths.join(", ") }}</span>
              </template>
            </p>
            <details v-if="d.route" class="route-context">
              <summary>Route context</summary>
              <p class="note">{{ d.route.tierReason }}</p>
              <ul v-if="d.route.candidatePaths.length">
                <li v-for="(path, i) in d.route.candidatePaths" :key="path"><span class="mono">{{ path }}</span><template v-if="d.route.candidateNotes?.[i]"> - {{ d.route.candidateNotes[i] }}</template></li>
              </ul>
              <p v-else class="note">No repository paths were supplied as route context.</p>
            </details>
          </section>

          <!-- Every tile labelled, none faked when unknown. -->
          <dl class="tiles">
            <div>
              <dt class="note">{{ d.metrics.callsUsed === null ? "agent calls unavailable" : d.metrics.callsUsed === 1 ? "agent call" : "agent calls" }}<template v-if="d.metrics.turns !== null"> · {{ d.metrics.turns }} turns</template></dt>
              <dd>{{ d.metrics.callsUsed ?? "-" }}</dd>
              <dd class="note">{{ d.metrics.ran.join(" → ") || "none" }}</dd>
            </div>
            <div>
              <dt class="note">{{ d.metrics.tokens ? "tokens" : "tokens unavailable" }}</dt>
              <dd>{{ d.metrics.tokens ? formatTokens(d.metrics.tokens.total) : "-" }}</dd>
              <dd v-if="d.metrics.tokens" class="note">{{ formatTokens(d.metrics.tokens.uncached) }} uncached · {{ formatTokens(d.metrics.tokens.cached) }} cached</dd>
              <dd v-if="d.metrics.tokens && d.metrics.tokens.cacheHit !== null" class="note" title="Share of input read from the provider's cache. Low means context was billed again.">{{ d.metrics.tokens.cacheHit }}% cache hit</dd>
            </div>
            <div>
              <dt class="note">{{ d.metrics.cost !== null ? "cost, " + d.metrics.costQuality : "cost unavailable" }}</dt>
              <dd :class="{ good: d.metrics.cost !== null }">{{ d.metrics.cost !== null ? formatCost(d.metrics.cost) : "-" }}</dd>
            </div>
            <div>
              <dt class="note">elapsed</dt>
              <dd>{{ formatDuration(d.durationMs) }}</dd>
            </div>
            <div>
              <dt class="note">model reported by provider{{ d.metrics.effort ? ` · ${d.metrics.effort} effort` : "" }}</dt>
              <dd class="model">{{ d.metrics.model ?? "-" }}</dd>
            </div>
          </dl>
          <slot name="details-end" />
        </template>

        <slot v-else name="activity" />
      </div>
    </dialog>
  </section>
</template>

<style scoped src="./result.css"></style>
<style scoped src="./chat.css"></style>
<style scoped>
.exchange {
  display: flex;
  flex-direction: column;
  gap: 16px;
}
/* A finished exchange above the current one ends in a hairline. */
.exchange.earlier {
  padding-bottom: 16px;
  border-bottom: 1px solid var(--border);
}
.answer .outcome-summary { align-items: center; margin: 12px 0 0; }
.outcome { display: inline-flex; align-items: center; gap: 6px; }
/* The proof sits beside the facts it backs. */
.proof { display: flex; gap: 14px; margin-left: auto; }
.answer { scroll-margin-top: 16px; }
.story > summary { cursor: pointer; width: fit-content; }
.story > summary:hover { color: var(--text-dim); }
.story-body { display: flex; flex-direction: column; gap: 16px; margin-top: 12px; }

.mine .edit {
  font-size: 11px;
  color: var(--text-faint);
}
.mine .edit:hover:not(:disabled) {
  color: var(--text);
}

.proof-modal {
  width: min(1100px, calc(100vw - 32px));
  height: min(820px, calc(100vh - 64px));
  padding: 0;
  background: var(--bg);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
}
.proof-modal[open] {
  display: flex;
  flex-direction: column;
}
.proof-modal::backdrop {
  background: var(--overlay);
}
.proof-modal header {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 12px 18px;
  border-bottom: 1px solid var(--border);
}
.proof-modal header .proof {
  margin-left: 0;
}
.close {
  margin-left: auto;
  padding: 0 6px;
  font-size: 20px;
  line-height: 1;
  color: var(--text-faint);
  transition: color 120ms ease;
}
.close:hover {
  color: var(--text);
}
.proof-modal .panel {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 18px;
}
</style>
