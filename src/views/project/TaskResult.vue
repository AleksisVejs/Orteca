<script setup lang="ts">
import { computed, inject, ref, watch } from "vue";
import { parseHistoryPatch } from "./historyPatch";
import { verificationSummary, unfinishedSummary } from "./taskPresentation";
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
  git, openGit, retryAt, waiting, waitForReset, cancelWait, formatWhen,
} = inject(PROJECT)!;

const selectedFile = ref<string | null>(null);
const parsedPatch = computed(() => parseHistoryPatch(result.value?.patchText ?? ""));
const selectedPath = computed(() => changed.value.byRun.some((f) => f.path === selectedFile.value) ? selectedFile.value : changed.value.byRun[0]?.path);
const selectedPatch = computed(() => parsedPatch.value.files.find((f) => f.path === selectedPath.value));
const verification = computed(() => verificationSummary(result.value?.stages ?? []));
watch(() => result.value?.taskId, () => { selectedFile.value = null; });
</script>

<template>
  <section v-if="result" class="card outcome">
    <!-- Everything already said in this task, so a follow-up reads as one thread. -->
    <article v-for="(t, i) in activeRun?.turns ?? []" :key="i" class="turn">
      <p class="who">You</p>
      <p class="said">{{ t.said }}</p>
      <Markdown v-if="t.summary" class="summary" :text="describeVerdict(t.summary) ?? t.summary" />
      <p v-else class="note">{{ t.failure ?? "No summary was reported." }}</p>
    </article>

    <header class="result-header">
    <div class="result-heading">
    <div class="head">
      <span class="dot" :class="TONE[result.status]" aria-hidden="true"></span>
      <h2 class="status">
        {{ result.route.kind === "answer" && result.status === "done" ? "Answered" : OUTCOME[result.status] }}
      </h2>
      <span class="note">{{ ranOn === "codex" ? "Codex" : "Claude" }} · {{ formatDuration(result.durationMs) }}</span>
    </div>
    <h1 class="prompt" :class="{ long: said.length > 140 }">{{ said }}</h1>
    <div class="outcome-summary"><span>{{ changed.byRun.length }} files changed</span><span>{{ verification }}</span><span>{{ unfinishedSummary(result.status) }}</span></div>

    </div>
    <div class="actions">
      <button class="btn" @click="editAgain">Edit and run again</button>
      <button class="btn primary" @click="newTask">New task</button>
    </div>
    </header>

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
        <p v-if="waiting && waiting.prompt === activeRun?.prompt" class="note" role="status">
          Carries on {{ formatWhen(waiting.at) }}, after {{ waiting.provider }}’s limit resets. Keep Orteca open.
          <button class="link" @click="cancelWait">Cancel</button>
        </p>
        <p v-else-if="retryAt && activeRun" class="note">
          {{ activeRun.provider }}’s limit resets {{ formatWhen(retryAt) }}.
          <button class="link" @click="waitForReset(retryAt, activeRun)">Carry on then</button>
        </p>
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
        <div v-if="changed.byRun.length" class="file-review">
          <ul class="review-files" aria-label="Changed files">
            <li v-for="f in changed.byRun" :key="f.path">
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
            <p v-if="changed.byRun.find((f) => f.path === selectedPath)?.origin === 'both'" class="note">This patch includes changes already present before the task.</p>
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
        <p v-if="changed.unknown" class="note caveat">Some changes could not be attributed to this task.</p>
        <p v-if="changed.beforeRun.length" class="note caveat">Already changed before this task and left untouched: <span class="mono">{{ changed.beforeRun.map((f) => f.path).join(', ') }}</span></p>
        <p v-for="notice in parsedPatch.notices" :key="notice" class="note">{{ notice }}</p>
        <details v-if="result.patchText" class="code-view"><summary>View full patch</summary><pre>{{ result.patchText }}</pre></details>
      </template>

      <template v-else-if="resultTab === 'details'">
        <!-- The route as decided before anything ran: what ran, what did not. -->
        <section class="execution" aria-label="Execution">
          <div class="section-heading"><h2 class="label">Execution</h2><span class="note">Steps taken for this task</span></div>
        <ol class="route">
          <template v-for="(step, i) in routeSteps" :key="i">
            <li v-if="i" class="arrow" aria-hidden="true">→</li>
            <li :class="{ ran: step.ran }"><span class="step-number" aria-hidden="true">{{ i + 1 }}</span><div>
              {{ stageLabel(step.stage) }}<span v-if="step.asked" class="mono"> · {{ step.asked }}</span><small>{{ step.ran ? "Ran" : "Not started" }}</small></div>
            </li>
          </template>
        </ol>
        <p class="note reason">
          {{ result.route.reason }}
          <template v-if="result.route.candidatePaths.length">
            · brief named <span class="mono">{{ result.route.candidatePaths.join(", ") }}</span>
          </template>
        </p>
        <details class="route-context">
          <summary>Route context</summary>
          <p class="note">{{ result.route.tierReason }}</p>
          <ul v-if="result.route.candidatePaths.length">
            <li v-for="(path, i) in result.route.candidatePaths" :key="path"><span class="mono">{{ path }}</span><template v-if="result.route.candidateNotes?.[i]"> — {{ result.route.candidateNotes[i] }}</template></li>
          </ul>
          <p v-else class="note">No repository paths were supplied as route context.</p>
        </details>

        <!-- Every tile labelled, none faked when unknown. -->
        </section>

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
            <dd v-if="tokens && tokens.cacheHit !== null" class="note" title="Share of input read from the provider's cache. Low means context was billed again.">{{ tokens.cacheHit }}% cache hit</dd>
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

        </dl>
        <div class="detail-links">
          <section class="detail-section"><h2 class="label">Changes</h2><p><strong>{{ changed.byRun.length }}</strong> Git-visible files changed</p><p class="note">Review recorded changes and their patches.</p><button class="btn" @click="resultTab = 'files'">View changed files</button></section>
          <section class="detail-section"><h2 class="label">Execution activity</h2><p>Follow the work step by step.</p><p class="note">Agent messages, tool activity and recorded edits.</p><button class="btn" @click="resultTab = 'activity'">View activity</button></section>
        </div>


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


  </section>
</template>

<style scoped src="./result.css"></style>
