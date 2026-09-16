<script setup lang="ts">
import { inject } from "vue";
import Markdown from "../../components/Markdown.vue";
import ActivityLog from "./ActivityLog.vue";
import { PROJECT } from "./state";

// The page after a run: what happened first, the rest one tab away.
const {
  task, result, running, provider, fallback, continueWith, describeVerdict, stageLabel,
  removedCopies, confirmRemove, removeCopy, removeError, routeSteps, calls, tokens,
  comparison, changed, OUTCOME, TONE, TABS, resultTab, formatTokens, formatCost,
  formatDuration, newTask, editAgain, reply, sendReply, warmLeft,
} = inject(PROJECT)!;
</script>

<template>
  <section v-if="result" class="card outcome">
    <div class="head">
      <span class="dot" :class="TONE[result.status]" aria-hidden="true"></span>
      <h2 class="status">
        {{ result.route.kind === "answer" && result.status === "done" ? "Answered" : OUTCOME[result.status] }}
      </h2>
      <span class="note">{{ formatDuration(result.durationMs) }}</span>
    </div>
    <p class="prompt">{{ task }}</p>

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
        <div v-if="fallback" class="fallback" role="status">
          <p class="note">
            {{ provider }} is out of plan usage. {{ fallback.id }} can carry on with the same
            request<template v-if="fallback.room !== null">, with {{ Math.round(fallback.room) }}% of its tightest limit left</template>.
            Anything already changed stays on disk.
          </p>
          <button class="btn" :disabled="running" @click="continueWith(fallback.id)">Continue with {{ fallback.id }}</button>
        </div>
        <Markdown v-else-if="result.summary" class="summary" :text="describeVerdict(result.summary) ?? result.summary" />
        <p v-else-if="!result.failure" class="note">No summary was reported.</p>
        <!-- A copy's committed work is on its branch; a fresh run would start without it. -->
        <div v-if="result.summary && !result.worktree?.commit" class="reply">
          <input
            v-model="reply"
            type="text"
            spellcheck="false"
            aria-label="Reply"
            placeholder="Reply, or ask for something more…"
            :disabled="running"
            @keyup.enter="sendReply"
          />
          <button class="btn" :disabled="running || !reply.trim()" @click="sendReply">Reply</button>
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
              Merge the branch when you’re happy with it.
            </template>
            <template v-else-if="result.worktree.commitError">
              The changes are in the copy but not committed: {{ result.worktree.commitError }}
            </template>
            <template v-else>Nothing changed, so there is nothing to merge.</template>
          </p>
          <template v-if="!removedCopies.includes(result.taskId)">
            <p class="note mono">{{ result.worktree.path }}</p>
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
        <ul class="diff">
          <li v-for="f in changed.byRun" :key="f.path">
            <span class="mono grow">{{ f.path }}</span>
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

<style scoped>
.outcome {
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
  margin: 8px 0 16px;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
  white-space: pre-line;
  overflow-wrap: anywhere;
}

.tabs {
  display: flex;
  gap: 4px;
  border-bottom: 1px solid var(--border);
}
.tabs button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: -1px;
  padding: 8px 10px;
  border-bottom: 1px solid transparent;
  color: var(--text-faint);
  transition: color 120ms ease, border-color 120ms ease;
}
.tabs button:hover {
  color: var(--text);
}
.tabs button.on {
  color: var(--text);
  border-bottom-color: var(--text);
}
.count {
  padding: 0 6px;
  border-radius: 999px;
  background: var(--surface-2);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}

.panel {
  min-height: 120px;
  padding: 18px 0;
}
.panel > :first-child {
  margin-top: 0;
}

.fallback {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
  margin: 0 0 16px;
}
.reply {
  display: flex;
  gap: 8px;
  margin: 12px 0 0;
}
.reply input {
  flex: 1;
  min-width: 0;
  padding: 8px 12px;
  background: none;
  color: var(--text);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  font: inherit;
}
.reply input:focus {
  outline: none;
  border-color: var(--info);
}
.reply-cost {
  margin: 6px 0 0;
  font-variant-numeric: tabular-nums;
}
.reply input::placeholder {
  color: var(--text-faint);
}
.fallback .note {
  flex: 1;
  margin: 0;
}

.route {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  margin: 0;
  padding: 0;
  list-style: none;
  font-size: 12px;
}
.route li {
  padding: 1px 9px;
  border: 1px solid var(--border);
  border-radius: 999px;
  color: var(--text-faint);
}
.route li.ran {
  border-color: var(--border-strong);
  color: var(--text);
}
.route li.arrow {
  padding: 0;
  border: none;
}
.reason {
  margin: 8px 0 16px;
}

.tiles {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
  gap: 1px;
  margin: 0 0 16px;
  background: var(--border);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  overflow: hidden;
}
.tiles > div {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  padding: 12px 14px;
  background: var(--surface-2);
}
.tiles dt {
  order: 2;
}
.tiles dd {
  margin: 0;
  order: 1;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
  font-variant-numeric: tabular-nums;
}
.tiles dd.note {
  order: 3;
  font-size: 12px;
  font-weight: 400;
  letter-spacing: normal;
}
.tiles dd.model {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--mono);
  font-size: 12px;
  line-height: 30px;
}
.good {
  color: var(--accent);
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding-top: 16px;
  border-top: 1px solid var(--border);
}
</style>
