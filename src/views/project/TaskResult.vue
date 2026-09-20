<script setup lang="ts">
import { computed, inject, ref } from "vue";
import Markdown from "../../components/Markdown.vue";
import ActivityLog from "./ActivityLog.vue";
import { PROJECT } from "./state";
import { visiblePath } from "../../path";

// The page after a run: what happened first, the rest one tab away.
const {
  activeRun, said, result, running, ranOn, domId, switchedFrom, describeVerdict, stageLabel,
  attachments, attachError, addAttachments, pasteImages, fileName,
  removedCopies, confirmRemove, removeCopy, removeError, routeSteps, calls, tokens,
  comparison, changed, OUTCOME, TONE, TABS, resultTab, formatTokens, formatCost,
  formatDuration, newTask, editAgain, reply, sendReply, warmLeft,
  git, openGit,
} = inject(PROJECT)!;

const selectedFile = ref<string | null>(null);

const selectedPatch = computed(() => {
  if (!selectedFile.value || !result.value?.patchText) return null;
  const patch = result.value.patchText;
  const escaped = selectedFile.value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const start = new RegExp(`(?:^|\\n)(diff --git a/${escaped} b/${escaped}\\n[\\s\\S]*?)(?=\\ndiff --git |$)`);
  return patch.match(start)?.[1] ?? null;
});

function showFilePatch(path: string) {
  selectedFile.value = path;
}

function closeFilePatch() {
  selectedFile.value = null;
}
</script>

<template>
  <section v-if="result" class="card outcome">
    <!-- Everything already said in this task, so a follow-up reads as one thread. -->
    <article v-for="(t, i) in activeRun?.turns ?? []" :key="i" class="turn">
      <p class="said">{{ t.said }}</p>
      <Markdown v-if="t.summary" class="summary" :text="describeVerdict(t.summary) ?? t.summary" />
      <p v-else class="note">{{ t.failure ?? "No summary was reported." }}</p>
    </article>

    <div class="head">
      <span class="dot" :class="TONE[result.status]" aria-hidden="true"></span>
      <h2 class="status">
        {{ result.route.kind === "answer" && result.status === "done" ? "Answered" : OUTCOME[result.status] }}
      </h2>
      <span class="note">{{ formatDuration(result.durationMs) }}</span>
    </div>
    <p class="prompt">{{ said }}</p>

    <div class="tabs" role="tablist" aria-label="Result">
      <button
        v-for="t in TABS"
        :id="`tab-${t.id}`"
        :key="t.id"
        role="tab"
        :aria-selected="resultTab === t.id"
        aria-controls="result-panel"
        :class="{ on: resultTab === t.id }"
        @click="resultTab = t.id"
      >
        {{ t.label }}<span v-if="t.id === 'files'" class="count">{{ changed.byRun.length }}</span>
      </button>
    </div>

    <div id="result-panel" class="panel" role="tabpanel" :aria-labelledby="`tab-${resultTab}`">
      <template v-if="resultTab === 'summary'">
        <p v-if="result.failure" class="missing">{{ result.failure }}</p>
        <p v-if="switchedFrom" class="note switched" role="status">
          {{ switchedFrom }} ran out of plan usage, so {{ ranOn }} carried on with the same request.
        </p>
        <Markdown v-if="result.summary" class="summary" :text="describeVerdict(result.summary) ?? result.summary" />
        <p v-else-if="!result.failure" class="note">No summary was reported.</p>
        <!-- A copy's committed work is on its branch; a fresh run would start without it. -->
        <div v-if="result.summary && !result.worktree?.commit" class="reply">
          <label class="hidden-label" :for="domId('reply')">Reply</label>
          <textarea
            :id="domId('reply')"
            v-model="reply"
            rows="2"
            spellcheck="false"
            placeholder="Reply, or ask for something more…"
            :disabled="running"
            @paste="pasteImages"
            @keydown.ctrl.enter.prevent="sendReply"
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
          <div class="reply-controls">
            <button class="icon" title="Attach files or images. You can also paste or drop them." aria-label="Attach files or images" @click="addAttachments(false)">
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M10.5 4.5 5.8 9.2a1.4 1.4 0 0 0 2 2l5-5a2.8 2.8 0 0 0-4-4l-5 5a4.2 4.2 0 0 0 6 6l4.2-4.2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
              Attach
            </button>
            <button class="icon" title="Attach a folder" aria-label="Add folder" @click="addAttachments(true)">
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
            </button>
            <span class="shortcut note">Ctrl + Enter</span>
            <button class="btn" :disabled="running || !reply.trim()" @click="sendReply">Reply</button>
          </div>
        </div>
        <p v-if="result.summary && !result.worktree?.commit" class="note reply-cost">
          <template v-if="warmLeft > 0">
            Reply in the next {{ Math.floor(warmLeft / 60000) }}:{{ String(Math.floor(warmLeft / 1000) % 60).padStart(2, "0") }}
            and it picks up where it left off. After that it starts over and reads the files again, which costs more.
          </template>
          <template v-else>
            A reply now starts over and reads the files again, so it costs more than one sent within 5 minutes of the run.
          </template>
        </p>
        <p v-if="result.status === 'cancelled'" class="note caveat">
          Stopped part-way. Anything the agent had already written is still on disk — Orteca reverts nothing.
        </p>
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
        <p v-if="result.unknownEvents" class="note caveat" role="status">
          {{ result.unknownEvents }} provider event{{ result.unknownEvents === 1 ? "" : "s" }} were not recognized and remain in the saved task log.
        </p>
      </template>

      <template v-else-if="resultTab === 'files'">
        <button v-if="!result.worktree && git.dirty" class="btn" :popovertarget="domId('project-git')" popovertargetaction="show" @click="openGit('commit')">Commit changes…</button>
        <ul class="diff">
          <li v-for="f in changed.byRun" :key="f.path">
            <button class="file-diff-link mono grow" @click="showFilePatch(f.path)">{{ f.path }}</button>
            <span v-if="f.origin === 'both'" class="note">also changed before this run; counts include both</span>
            <span v-if="f.added !== null" class="note">+{{ f.added }} &minus;{{ f.deleted }}</span>
            <span v-else class="note">new or binary</span>
          </li>
          <li v-if="!changed.byRun.length" class="note">No files changed.</li>
        </ul>
        <p class="note caveat">Only files Git can see are listed.</p>
        <!-- Only when git could not snapshot the tree first. -->
        <p v-if="changed.unknown" class="note caveat">
          This repository already had uncommitted changes, so some of the above may not have been made by this run.
        </p>
        <p v-if="changed.beforeRun.length" class="note caveat">
          Already changed before this run, and left as they were:
          <span class="mono">{{ changed.beforeRun.map((f) => f.path).join(", ") }}</span>
        </p>
        <details v-if="result.patchText" class="code-view">
          <summary>View patch</summary>
          <pre>{{ result.patchText }}</pre>
        </details>

        <div v-if="selectedFile" class="file-diff" role="dialog" aria-modal="true" aria-labelledby="file-diff-title">
          <div class="file-diff-head">
            <h3 id="file-diff-title">{{ selectedFile }}</h3>
            <button class="link" @click="closeFilePatch">Close</button>
          </div>
          <pre v-if="selectedPatch" class="code-view">{{ selectedPatch }}</pre>
          <p v-else class="note">This file’s patch is unavailable or was truncated.</p>
        </div>
      </template>

      <template v-else-if="resultTab === 'details'">
        <!-- The route as decided before anything ran: what ran, what did not. -->
        <ol class="route">
          <template v-for="(step, i) in routeSteps" :key="step.stage">
            <li v-if="i" class="arrow" aria-hidden="true">→</li>
            <li :class="{ ran: step.ran }">
              {{ stageLabel(step.stage) }}<span v-if="step.asked" class="mono"> · {{ step.asked }}</span><span class="hidden-label"> — {{ step.ran ? "ran" : "not started" }}</span>
            </li>
          </template>
        </ol>
        <p class="note reason">
          {{ result.route.reason }}
          <template v-if="result.route.candidatePaths.length">
            · brief named <span class="mono">{{ result.route.candidatePaths.join(", ") }}</span>
          </template>
        </p>

        <!-- Every tile labelled, none faked when unknown. -->
        <dl class="tiles">
          <div v-if="calls">
            <dt class="note">agent {{ calls.used === 1 ? "call" : "calls" }} · {{ result.turnsUsed }} turns</dt>
            <dd>{{ calls.used }}</dd>
            <dd class="note">{{ calls.ran.join(" → ") || "none" }}</dd>
          </div>
          <div>
            <dt class="note">{{ tokens ? "tokens" : "tokens unavailable" }}</dt>
            <dd>{{ tokens ? formatTokens(tokens.total) : "—" }}</dd>
            <dd v-if="tokens" class="note">{{ formatTokens(tokens.uncached) }} uncached · {{ formatTokens(tokens.cached) }} cached</dd>
          </div>
          <div>
            <dt class="note">{{ tokens && tokens.cost !== null ? "cost, " + tokens.quality : "cost unavailable" }}</dt>
            <dd :class="{ good: tokens && tokens.cost !== null }">
              {{ tokens && tokens.cost !== null ? formatCost(tokens.cost) : "—" }}
            </dd>
          </div>
          <div>
            <dt class="note">elapsed</dt>
            <dd>{{ formatDuration(result.durationMs) }}</dd>
          </div>
          <div>
            <dt class="note">model reported by provider{{ tokens?.effort ? ` · ${tokens.effort} effort` : "" }}</dt>
            <dd class="model">{{ tokens?.model ?? "—" }}</dd>
          </div>
          <div>
            <dt class="note">Git-visible files changed</dt>
            <dd>{{ changed.byRun.length }}</dd>
          </div>
        </dl>

        <!-- No savings claim without a measured baseline, and never unlabelled. -->
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

      <ActivityLog v-else />
    </div>

    <div class="actions">
      <button class="btn" @click="editAgain">Edit and run again</button>
      <button class="btn primary" @click="newTask">New task</button>
    </div>
  </section>
</template>

<style scoped src="./result.css"></style>
