<script setup lang="ts">
import { computed, inject, nextTick, ref, watch } from "vue";
import Markdown from "../../components/Markdown.vue";
import ActivityLog from "./ActivityLog.vue";
import TaskChat from "./TaskChat.vue";
import SteerBox from "./SteerBox.vue";
import ExchangeView from "./Exchange.vue";
import Story from "./Story.vue";
import { PROJECT, exchangeOf, rewindCode, storyOf } from "./state";
import { visiblePath } from "../../path";

// The task on screen, as one chat from the first message to the reply after it.
// The page stays while the run goes and finishes; only the current answer and
// the box at the bottom change with it.
const {
  activeRun, said, result, running, checking, ranOn, domId, switchedFrom, describeVerdict, stageLabel,
  removedCopies, confirmRemove, removeCopy, removeError, tokens,
  comparison, HISTORY_STATUS, formatTokens, sendReply, warmLeft,
  git, openGit, retryAt, waiting, waitForReset, cancelWait, formatWhen, lines, carryOn, rewindTo,
} = inject(PROJECT)!;

// A copy's committed work is on its branch; a fresh run would start without it.
// A failed or stopped run takes a reply like any other: "try again, but…".
const canReply = computed(() => !!result.value && !result.value.worktree?.commit);
const replyNote = computed(() => warmLeft.value > 0
  ? `Resumes for ${Math.floor(warmLeft.value / 60000)}:${String(Math.floor(warmLeft.value / 1000) % 60).padStart(2, "0")}`
  : "Rereads the files");
const replyHint = computed(() => warmLeft.value > 0
  ? "A reply now picks up where it left off. After 5 minutes it starts over and reads the files again, which costs more."
  : "A reply now starts over and reads the files again, so it costs more than one sent within 5 minutes of the run.");
// Each earlier answer keeps its own proof; one read back from history has only its words.
const earlier = computed(() => (activeRun.value?.turns ?? []).map((t) => ({ ...t, data: t.result ? exchangeOf(t.result, t.stream ?? []) : null })));

// Back to before message `i`, sending `text` in its place; with `code` its files go back as
// they were. The first request keeps its picked files, so it goes back raw too. Work in a copy is not in this
// folder, so any of its messages goes to the composer as a new task.
const results = computed(() => [...(activeRun.value?.turns ?? []).map((t) => t.result), result.value]);
const inTree = computed(() => !!result.value && !result.value.worktree);
function rewind(i: number, text: string, code: boolean) {
  const live = activeRun.value;
  if (!live || !result.value) return;
  const raw = i === 0 ? live.asked[0] ?? live.prompt : "";
  void rewindTo(result.value.taskId, inTree.value ? i : 0, text, raw, code && inTree.value ? rewindCode(results.value, i) : null);
}
// While it runs, everything in order: what the agent says, thinks and does, and every steer.
const messages = computed(() => storyOf(lines.value));

const chat = ref<InstanceType<typeof TaskChat> | null>(null);
const steer = ref<InstanceType<typeof SteerBox> | null>(null);
watch(() => `${lines.value.length}:${lines.value.at(-1)?.text.length}:${!!checking.value}:${running.value}`, () => chat.value?.stick(), { flush: "post" });

// The orb settles into the heading's status dot, so the outcome reads as where the work landed.
function settle(from: DOMRect) {
  const el = document.getElementById(domId("task-dot"));
  if (!el || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  const to = el.getBoundingClientRect();
  if (!to.width) return;
  const dx = from.left + from.width / 2 - (to.left + to.width / 2);
  const dy = from.top + from.height / 2 - (to.top + to.height / 2);
  // Above the pane it crosses, only while it moves.
  el.style.position = "relative";
  el.style.zIndex = "1";
  el.animate(
    [{ transform: `translate(${dx}px, ${dy}px) scale(${from.width / to.width})`, opacity: 0.6 }, { transform: "none", opacity: 1 }],
    { duration: 600, easing: "cubic-bezier(0.2, 0, 0, 1)" },
  ).finished.then(() => el.removeAttribute("style"), () => el.removeAttribute("style"));
}
// The box swapping between reply and steer grows or shrinks to its new height
// instead of jumping there.
const composerBox = () => document.getElementById(domId("task-composer"))?.querySelector<HTMLElement>(".composer");
function resize(from: number) {
  const box = composerBox();
  if (!box || !from || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  const to = box.offsetHeight;
  if (to === from) return;
  box.animate(
    [{ height: `${from}px`, overflow: "hidden" }, { height: `${to}px`, overflow: "hidden" }],
    { duration: 240, easing: "cubic-bezier(0.2, 0, 0, 1)" },
  );
}
// The same task finishing lands its orb; one starting (a reply) goes to the bottom,
// where it streams. Switching to another task does neither. Runs before the
// page changes, while the steer box and its orb are still there.
watch([() => activeRun.value?.key, running], ([key, now], [was, before]) => {
  if (key !== was || !!before === !!now) return;
  const height = composerBox()?.offsetHeight ?? 0;
  if (before) {
    const from = steer.value?.orbRect();
    void nextTick(() => {
      resize(height);
      if (from?.width) settle(from);
    });
  } else {
    void nextTick(() => {
      resize(height);
      chat.value?.land(true);
    });
  }
});
</script>

<template>
  <TaskChat
    v-if="activeRun"
    ref="chat"
    :task-key="activeRun.key"
    id-prefix="task"
    :prompt="said"
    :data="result ? exchangeOf(result, lines) : undefined"
    :can-reply="canReply"
    :reply-note="replyNote"
    :reply-hint="replyHint"
    :follow="running"
    :edit-disabled="running"
    :rewind-files="inTree ? rewindCode(results, earlier.length)?.files : null"
    @send="sendReply"
    @edit-again="(text: string) => rewind(earlier.length, text, false)"
    @rewind="(text: string) => rewind(earlier.length, text, true)"
  >
    <!-- Everything already said in this task, so a follow-up reads as one conversation. -->
    <template #earlier>
      <template v-for="(t, i) in earlier" :key="i">
        <ExchangeView
          v-if="t.data"
          :id-prefix="`turn-${i}`"
          :prompt="t.said"
          :data="t.data"
          earlier
          :editable="!!result"
          :edit-disabled="running"
          :rewind-files="inTree ? rewindCode(results, i)?.files : null"
          @edit-again="(text: string) => rewind(i, text, false)"
          @rewind="(text: string) => rewind(i, text, true)"
        >
          <template #activity>
            <ActivityLog :items="t.stream ?? []" :patch-text="t.result?.patchText" :root="t.result?.worktree?.path" :finished="true" :dirty-at-start="t.result?.dirtyAtStart" />
          </template>
        </ExchangeView>
        <template v-else>
          <p class="bubble">{{ t.said }}</p>
          <Markdown v-if="t.summary" class="summary back" :text="describeVerdict(t.summary) ?? t.summary" />
          <p v-else class="note back">{{ t.failure ?? "No summary was reported." }}</p>
          <p class="note turn-end"><template v-if="t.status">{{ HISTORY_STATUS[t.status] }}<template v-if="t.files"> · {{ t.files }} files changed</template></template></p>
        </template>
      </template>
    </template>

    <!-- While it runs: the words so far, streaming where the answer will land. -->
    <template v-if="running" #current>
      <div class="mine">
        <h1 class="bubble" :class="{ long: said.length > 600 }">{{ said }}</h1>
        <p v-if="activeRun.attachments.length" class="note">
          {{ activeRun.attachments.length }} attached {{ activeRun.attachments.length === 1 ? "item" : "items" }}
        </p>
      </div>

      <Story :lines="messages" />

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
    </template>
    <template v-if="running" #composer><SteerBox ref="steer" /></template>

    <template #notes>
      <p v-if="waiting && waiting.prompt === activeRun.prompt" class="note" role="status">
        Carries on {{ formatWhen(waiting.at) }}, after {{ waiting.provider }}’s limit resets. Keep Orteca open.
        <button class="link" @click="cancelWait">Cancel</button>
      </p>
      <p v-else-if="retryAt" class="note">
        {{ activeRun.provider }}’s limit resets {{ formatWhen(retryAt) }}.
        <button class="link" @click="waitForReset(retryAt, activeRun)">Carry on then</button>
      </p>
      <p v-if="activeRun.handoff && !running" class="note" role="status">
        {{ activeRun.provider }} ran out of plan usage. {{ activeRun.handoff.to }} can carry on with the same request, on its own plan.
        <button class="link" @click="carryOn">Carry on with {{ activeRun.handoff.to }}</button>
      </p>
      <p v-if="switchedFrom" class="note switched" role="status">
        {{ switchedFrom }} ran out of plan usage, so {{ ranOn }} carried on with the same request.
      </p>
    </template>

    <template v-if="result" #after>
      <!-- A budget stop hands the decision back rather than spending more. -->
      <p v-if="result.budgetStop" class="note caveat">
        Orteca stopped safely after using the planned limit.
        <template v-if="result.budgetStop.remaining.length">
          Still to do: {{ result.budgetStop.remaining.map(stageLabel).join(" → ") }}.
        </template>
        Run again if you want it to keep going.
      </p>
      <div v-if="result.worktree" class="copy" role="status">
        <p class="note">
          Worked in a separate copy on branch <span class="mono">{{ result.worktree.branch }}</span>.
          <template v-if="result.worktree.commit">
            Its changes are committed there as <span class="mono">{{ result.worktree.commit.slice(0, 7) }}</span>.
          </template>
          <template v-else-if="result.worktree.commitError">
            The changes are in the copy but not committed: {{ result.worktree.commitError }}
          </template>
          <template v-else>Nothing changed, so there is nothing to merge.</template>
        </p>
        <button v-if="result.worktree.commit" class="btn" :popovertarget="domId('project-git')" popovertargetaction="show" @click="openGit('merge', result.worktree.branch)">Merge into {{ git.branch ?? 'current checkout' }}</button>
        <template v-if="!removedCopies.includes(result.taskId)">
          <p class="note mono">{{ visiblePath(result.worktree.path) }}</p>
          <button
            class="btn"
            :class="{ confirming: confirmRemove === result.taskId }"
            :disabled="running"
            @click="removeCopy(result.taskId)"
          >
            {{ confirmRemove === result.taskId ? "Yes, delete the copy folder" : "Remove copy" }}
          </button>
          <button v-if="confirmRemove === result.taskId" class="link" @click="confirmRemove = null">Keep it</button>
        </template>
        <p v-else class="note">Copy removed. The branch is still there.</p>
        <p v-if="removeError" class="missing">{{ removeError }}</p>
      </div>
    </template>

    <template v-if="result" #files-top>
      <button v-if="!result.worktree && git.dirty" class="btn" :popovertarget="domId('project-git')" popovertargetaction="show" @click="openGit('commit')">Commit changes…</button>
    </template>

    <!-- No savings claim without a measured baseline, and never unlabelled. -->
    <template v-if="result" #details-end>
      <p v-if="comparison" class="note">
        <span :class="{ good: comparison.change > 0 }">
          {{ comparison.size }}% {{ comparison.change >= 0 ? "fewer" : "more" }} uncached tokens
        </span>
        than the median of the last {{ comparison.runs }} finished runs of this route here
        ({{ formatTokens(comparison.medianTokens) }}), estimated.
      </p>
      <p v-else-if="result.status === 'done' && tokens" class="note">
        No savings figure yet: that needs five finished runs of this route here to compare against.
      </p>
    </template>

    <template #activity><ActivityLog /></template>
  </TaskChat>
</template>

<!-- Slot content is styled here, in the scope it was written in. -->
<style scoped src="./result.css"></style>
<style scoped src="./chat.css"></style>
<style scoped>
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
</style>
