<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue";
import Memory from "./Memory.vue";
import ProviderMark from "../../components/ProviderMark.vue";
import { PROJECT } from "./state";
import { cliChat, cliChats, isAppError, memory } from "../../api";
import { reference } from "./picks";
import type { CliChatSummary, MemoryState } from "../../types";

// The idle page: one prompt, one clear action. Options stay one click away.
const {
  task, picks, attachments, attachError, dragging, addAttachments, pasteImages, fileName,
  schedulePreview, canRun, run, optionsOpen, MODES, mode, ISOLATIONS, isolation, autoWait, chooseAutoWait, clarify, chooseClarify,
  installed, provider, providerPicked, previewing, preview, previewError,
  limitWarning, alternative, switchTo, waiting, waitForReset, cancelWait, formatWhen, runError, providerError, agentsPending, view,
  remembering, rememberText, rememberError, remember,
  MODELS, modelChoices, chooseProvider, chooseModel, domId, anyRunning, runningCount, opened, git, history,
  taskName, referTask, TONE, HISTORY_STATUS, stageLabel,
} = inject(PROJECT)!;

// Any task that has ended; a failed one is often the context worth handing on.
const pastTasks = computed(() => history.value.filter((t) => t.status !== "running"));
const taskFilter = ref("");
const shownTasks = computed(() => {
  const q = taskFilter.value.trim().toLowerCase();
  return q ? pastTasks.value.filter((t) => `#${t.id} ${taskName(t)}`.toLowerCase().includes(q)) : pastTasks.value;
});
function pickTask(t: (typeof pastTasks.value)[number]) {
  document.getElementById(domId("task-refs"))?.hidePopover();
  taskFilter.value = "";
  void referTask(t);
}

// Chats the user had with a CLI here before Orteca, handed on the same way.
const chats = ref<CliChatSummary[]>([]);
const shownChats = computed(() => {
  const q = taskFilter.value.trim().toLowerCase();
  return q ? chats.value.filter((c) => c.title.toLowerCase().includes(q)) : chats.value;
});
const cliName = (c: CliChatSummary) => (c.provider === "claude" ? "Claude" : "Codex");
async function pickChat(c: CliChatSummary) {
  document.getElementById(domId("task-refs"))?.hidePopover();
  taskFilter.value = "";
  attachError.value = null;
  const what = `${cliName(c)} chat`;
  try {
    const d = await cliChat(opened.project.path, c.provider, c.id);
    const pick = reference(what, d.title, d.said, d.answer || "(No answer was recorded.)", []);
    if (!picks.value.some((p) => p.label === pick.label)) picks.value.push(pick);
  } catch (e) {
    attachError.value = `That ${what} could not be read: ${isAppError(e) ? e.message : String(e)}`;
  }
}
// A folder with no transcripts is the common case, not an error worth a line.
onMounted(() => cliChats(opened.project.path).then((found) => (chats.value = found), () => {}));

// A run inherits only these explicit rules: global first, then this project.
// Keep the receipt beside Run so the user can check the actual payload before
// spending an agent call.
const memories = ref<MemoryState>({ items: [], limit: 1000, tokens: 0 });
const memoryError = ref("");
async function loadMemory() {
  try {
    memories.value = await memory(opened.project.path);
    memoryError.value = "";
  } catch (err) {
    memoryError.value = isAppError(err) ? err.message : String(err);
  }
}
onMounted(loadMemory);

// A compact local receipt, not a benchmark claim: it groups only by route and
// provider, while task difficulty and mode can still vary within a row.
const routeHistory = computed(() => {
  const rows = history.value.filter((run) => run.status === "done" && run.routeKind && run.provider && run.uncachedTokens !== null);
  const groups = new Map<string, typeof rows>();
  for (const row of rows) {
    const key = `${row.routeKind}:${row.provider}`;
    groups.set(key, [...(groups.get(key) ?? []), row]);
  }
  return [...groups.entries()].map(([key, runs]) => {
    const median = (values: number[]) => values.sort((a, b) => a - b)[Math.floor(values.length / 2)] ?? 0;
    const [route, provider] = key.split(":");
    return {
      key,
      route,
      provider,
      runs: runs.length,
      tokens: median(runs.map((run) => run.uncachedTokens!)),
      duration: median(runs.map((run) => run.durationMs ?? 0).filter(Boolean)),
    };
  }).sort((a, b) => b.runs - a.runs || a.key.localeCompare(b.key));
});

function formatDuration(milliseconds: number) {
  const seconds = Math.round(milliseconds / 1000);
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}
</script>

<template>
  <section class="composer" aria-labelledby="composer-title">
    <h2 id="composer-title" class="hero">Start a new task</h2>

    <section class="ask card" :class="{ dragging }">
      <label class="hidden-label" :for="domId('task')">Describe your task</label>
      <textarea
        :id="domId('task')"
        v-model="task"
        rows="4"
        spellcheck="false"
        placeholder="Describe the task and what a good result looks like…"
        @input="schedulePreview"
        @paste="pasteImages"
        @keydown.ctrl.enter.prevent="run()"
      ></textarea>

      <ul v-if="picks.length" class="attachments" aria-label="Elements and tasks picked">
        <li v-for="(pick, i) in picks" :key="pick.block" class="chip">
          <span class="mono" :title="pick.block">{{ pick.block.startsWith("On ") ? "⌖" : "↩" }} {{ pick.label }}</span>
          <button class="unattach" :title="`Remove ${pick.label}`" :aria-label="`Remove ${pick.label}`" @click="picks.splice(i, 1)">×</button>
        </li>
      </ul>
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
      <p v-if="memoryError" class="attach-error" role="status">Memory could not be read: {{ memoryError }}</p>
      <div v-if="rememberText !== null" class="remember" role="group" aria-label="Save to memory">
        <span>Save “{{ rememberText }}” to memory for:</span>
        <button class="btn" @click="remember(false)">This project</button>
        <button class="btn" @click="remember(true)">All projects</button>
        <button class="link" @click="rememberText = null">Cancel</button>
        <p v-if="rememberError" class="attach-error" role="alert">{{ rememberError }}</p>
      </div>

      <div class="controls">
        <button class="icon" title="Attach files or images. You can also paste or drop them." aria-label="Attach files or images" @click="addAttachments(false)">
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M10.5 4.5 5.8 9.2a1.4 1.4 0 0 0 2 2l5-5a2.8 2.8 0 0 0-4-4l-5 5a4.2 4.2 0 0 0 6 6l4.2-4.2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
          Attach
        </button>
        <button class="icon" title="Attach a folder" @click="addAttachments(true)">
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
          Folder
        </button>
        <button v-if="pastTasks.length || chats.length" class="icon" :popovertarget="domId('task-refs')" title="Build on an earlier task: its chat and final answer go with this one">
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M6 4 2.5 7.5 6 11M3 7.5h6.5a4 4 0 0 1 4 4v1" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
          Task
        </button>
        <section :id="domId('task-refs')" class="task-refs" popover role="dialog" :aria-labelledby="domId('task-refs-title')">
          <header class="refs-head">
            <h2 :id="domId('task-refs-title')">Build on an earlier task</h2>
            <p class="note">Its chat and final answer go with this task. The tool work does not.</p>
          </header>
          <input v-model="taskFilter" class="refs-filter" type="search" placeholder="Filter tasks…" aria-label="Filter tasks" />
          <ul class="refs-list">
            <li v-for="t in shownTasks" :key="t.id">
              <button @click="pickTask(t)">
                <span class="dot" :class="TONE[t.status]" aria-hidden="true"></span>
                <span class="refs-title">{{ taskName(t) }}</span>
                <span class="refs-meta">#{{ t.id }} · {{ HISTORY_STATUS[t.status] ?? t.status }}</span>
              </button>
            </li>
            <li v-if="!shownTasks.length" class="note refs-empty">No task matches.</li>
            <template v-if="shownChats.length">
              <li class="refs-group" role="presentation">Chats here outside Orteca</li>
              <li v-for="c in shownChats" :key="`${c.provider}:${c.id}`">
                <button @click="pickChat(c)">
                  <ProviderMark :id="c.provider" />
                  <span class="refs-title">{{ c.title }}</span>
                  <span class="refs-meta">{{ cliName(c) }} · {{ new Date(c.updatedMs).toLocaleDateString() }}</span>
                </button>
              </li>
            </template>
          </ul>
        </section>
        <Memory :path="opened.project.path" :pop-id="domId('memory')" button-class="icon" @changed="loadMemory">
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M4 2.5h8v11L8 10.8 4 13.5Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
          Memory
          <span v-if="memories.items.length" class="count" :title="`${memories.items.length} rule${memories.items.length === 1 ? '' : 's'} · ~${memories.tokens.toLocaleString()} tokens, sent with every stage`">{{ memories.items.length }}</span>
        </Memory>
        <span class="shortcut note">Ctrl + Enter</span>
        <button class="btn primary run" :disabled="!canRun" title="Run task (Ctrl+Enter)" @click="run()">
          {{ remembering ? "Remember" : "Run task" }}
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M3 8h10M8 3l5 5-5 5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </button>
      </div>

      <button class="options" :aria-expanded="optionsOpen" :aria-controls="domId('task-options')" @click="optionsOpen = !optionsOpen">
        <span class="options-label">Task settings</span>
        <span class="settings-summary">
          <span :title="opened.project.path">{{ opened.project.name }}</span> ·
          <span class="mono">{{ isolation === 'worktree' ? 'from ' : '' }}{{ git.branch ?? 'no branch' }}</span> ·
          <template v-if="installed.length">{{ providerPicked ? (provider === 'claude' ? 'Claude' : 'Codex') : "Auto" }} · </template>
          {{ MODES.find((m) => m.id === mode)?.label }} · {{ ISOLATIONS.find((w) => w.id === isolation)?.label }}
        </span>
        <svg :class="{ expanded: optionsOpen }" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m4 6 4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
      </button>

      <div v-show="optionsOpen" :id="domId('task-options')" class="option-panel">
        <div v-if="installed.length" class="setting" role="group" :aria-labelledby="domId('helper-label')">
          <div class="setting-text">
            <span :id="domId('helper-label')" class="option-label">AI helper</span>
            <span class="setting-hint">{{ providerPicked ? "You pick the helper; Orteca still picks each step's model unless you choose one" : "Orteca picks the helper, model and reasoning" }}</span>
          </div>
          <div class="segments">
            <button
              class="seg"
              :class="{ on: !providerPicked }"
              :aria-pressed="!providerPicked"
              title="Let Orteca choose the best provider, model, and reasoning for the task"
              @click="chooseProvider('auto')"
            >
              Auto
            </button>
            <button
              v-for="p in installed"
              :key="p.id"
              class="seg"
              :class="{ on: providerPicked && provider === p.id }"
              :aria-pressed="providerPicked && provider === p.id"
              @click="chooseProvider(p.id)"
            >
              <ProviderMark :id="p.id" />{{ p.id === 'claude' ? 'Claude' : 'Codex' }}
            </button>
          </div>
        </div>
        <template v-if="installed.length && providerPicked">
          <label class="setting sub">
            <span class="setting-text"><span class="option-label">Model</span></span>
            <span class="select-field">
              <select :value="modelChoices[provider].model" @change="chooseModel(($event.target as HTMLSelectElement).value)">
                <option v-for="item in MODELS[provider]" :key="item.id" :value="item.id">{{ item.label }}</option>
              </select>
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m5 6 3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
            </span>
          </label>
          <label v-if="modelChoices[provider].model !== 'auto'" class="setting sub">
            <span class="setting-text"><span class="option-label">Reasoning</span></span>
            <span class="select-field">
              <select v-model="modelChoices[provider].effort" @change="schedulePreview">
                <option
                  v-for="effort in MODELS[provider].find((item) => item.id === modelChoices[provider].model)?.efforts"
                  :key="effort"
                  :value="effort"
                >
                  {{ effort === 'xhigh' ? 'Extra high' : effort === 'max' ? 'Maximum' : effort.charAt(0).toUpperCase() + effort.slice(1) }}
                </option>
              </select>
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m5 6 3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
            </span>
          </label>
        </template>
        <div class="setting" role="group" :aria-labelledby="domId('mode-label')">
          <div class="setting-text">
            <span :id="domId('mode-label')" class="option-label">Approach</span>
            <span class="setting-hint">{{ MODES.find((m) => m.id === mode)?.hint }}</span>
          </div>
          <div class="segments">
            <button
              v-for="m in MODES"
              :key="m.id"
              class="seg"
              :class="{ on: mode === m.id }"
              :aria-pressed="mode === m.id"
              :title="m.hint"
              @click="mode = m.id; schedulePreview()"
            >
              {{ m.label }}
            </button>
          </div>
        </div>
        <div class="setting" role="group" :aria-labelledby="domId('isolation-label')">
          <div class="setting-text">
            <span :id="domId('isolation-label')" class="option-label">Working location</span>
            <span class="setting-hint">{{ ISOLATIONS.find((w) => w.id === isolation)?.hint }}</span>
          </div>
          <div class="segments">
            <button
              v-for="w in ISOLATIONS"
              :key="w.id"
              class="seg"
              :class="{ on: isolation === w.id }"
              :aria-pressed="isolation === w.id"
              :title="w.hint"
              @click="isolation = w.id; schedulePreview()"
            >
              {{ w.label }}
            </button>
          </div>
        </div>
        <div class="setting" role="group" :aria-labelledby="domId('wait-label')">
          <div class="setting-text">
            <span :id="domId('wait-label')" class="option-label">Slow commands</span>
            <span class="setting-hint">{{ autoWait ? "Runs them at once, as you, outside the agent's sandbox. Only a short list, like git push, is refused" : "Asks before it runs a command the agent hands over" }}</span>
          </div>
          <div class="segments">
            <button
              class="seg"
              :class="{ on: !autoWait }"
              :aria-pressed="!autoWait"
              title="When the agent hands Orteca a slow command, ask before running it"
              @click="chooseAutoWait(false)"
            >
              Ask me
            </button>
            <button
              class="seg"
              :class="{ on: autoWait }"
              :aria-pressed="autoWait"
              title="Run it at once, in this project only. It runs as you, with network access, outside the agent's sandbox; only a short list like git push or rm is refused"
              @click="chooseAutoWait(true)"
            >
              Run them
            </button>
          </div>
        </div>
        <div class="setting" role="group" :aria-labelledby="domId('clarify-label')">
          <div class="setting-text">
            <span :id="domId('clarify-label')" class="option-label">Unclear requests</span>
            <span class="setting-hint">{{ clarify ? "Asks one question first when a wrong guess would waste the run" : "The agent guesses and says how it read the request" }}</span>
          </div>
          <div class="segments">
            <button class="seg" :class="{ on: clarify }" :aria-pressed="clarify" title="Ask one short question before a change, fix or plan whose target is unclear" @click="chooseClarify(true)">Ask me</button>
            <button class="seg" :class="{ on: !clarify }" :aria-pressed="!clarify" title="Never ask; the agent states its reading at the top of its reply" @click="chooseClarify(false)">Let it guess</button>
          </div>
        </div>
      </div>

      <div v-if="previewing" class="preview note" aria-live="polite">Figuring out the best way to do it…</div>
      <div v-else-if="preview" class="preview" aria-live="polite">
        <!-- Routed without the model reading of the prompt, which costs a call; the run may still adjust it. -->
        <span class="note">Likely steps: {{ preview.route.stages.map(stageLabel).join(" → ") }}. This can change once Orteca reads your request.</span>
        <span v-if="preview.route.candidatePaths.length" class="note">
          Likely files: <span class="mono">{{ preview.route.candidatePaths.slice(0, 4).map((p) => p.split(/[\\/]/).pop()).join(", ") }}</span><template v-if="preview.route.candidatePaths.length > 4"> and {{ preview.route.candidatePaths.length - 4 }} more</template>.
        </span>
        <span v-if="isolation === 'worktree'" class="note">
          I’ll work in a separate copy on a new branch and leave this folder alone.
          <template v-if="preview.git.dirty">
            Your {{ preview.git.dirtyCount }} uncommitted change{{ preview.git.dirtyCount === 1 ? "" : "s" }} won’t be in it.
          </template>
          Files Git ignores, like node_modules, aren’t copied.
        </span>
        <span v-else-if="preview.git.dirty" class="note">
          Your {{ preview.git.dirtyCount }} existing change{{ preview.git.dirtyCount === 1 ? " is" : "s are" }} saved first. If the run undoes any, you can put them back.
        </span>
        <!-- Only reported figures reach the words; the per-call threshold is a guess and is not shown as a number. -->
        <span v-if="limitWarning" class="missing" role="status">
          {{ preview.provider }}’s {{ limitWarning.window.label }} limit is {{ Math.round(limitWarning.window.usedPercent) }}% used<template v-if="limitWarning.resets">, resets {{ limitWarning.resets }}</template>.
          This takes at least {{ limitWarning.calls }} AI {{ limitWarning.calls === 1 ? "call" : "calls" }}, so it might not finish.
          <button v-if="alternative" class="link" @click="switchTo(alternative)">use {{ alternative }} instead</button>
          <button v-if="limitWarning.startsAt && canRun" class="link" @click="waitForReset(limitWarning.startsAt)">start it after the reset</button>
        </span>
        <!-- Two agents editing one folder is allowed, but what each one changed
             stops being separable, so the composer says so before the click. -->
        <span v-if="anyRunning && isolation === 'currentTree'" class="missing" role="status">
          {{ runningCount === 1 ? "Another task is" : `${runningCount} other tasks are` }} already changing this folder.
          Their changes and this one will be mixed together in every diff.
        </span>
      </div>
      <p v-else-if="previewError" class="preview missing" role="status">Preview unavailable: {{ previewError }}</p>
    </section>

    <details v-if="routeHistory.length" class="route-history">
      <summary>
        Past runs in this project
        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m4 6 4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
      </summary>
      <table>
        <thead><tr><th>Route</th><th>AI</th><th class="num">Runs</th><th class="num">Median tokens</th><th class="num">Median time</th></tr></thead>
        <tbody>
          <tr v-for="row in routeHistory" :key="row.key">
            <td class="cap">{{ row.route }}</td>
            <td class="cap">{{ row.provider }}</td>
            <td class="num">{{ row.runs }}</td>
            <td class="num">{{ row.tokens.toLocaleString() }}</td>
            <td class="num">{{ row.duration ? formatDuration(row.duration) : "not recorded" }}</td>
          </tr>
        </tbody>
      </table>
      <p class="note">Finished runs only. Tokens leave out cached ones. A description of what happened, not a benchmark.</p>
    </details>

    <p v-if="waiting" class="note" role="status">
      Waiting for {{ waiting.provider }}’s limit to reset: “{{ waiting.prompt.slice(0, 80) }}” starts {{ formatWhen(waiting.at) }}.
      Keep Orteca open.
      <button class="link" @click="cancelWait">Cancel</button>
    </p>
    <p v-if="runError" class="missing">{{ runError }}</p>
    <p v-else-if="!installed.length && !providerError && !agentsPending" class="missing">
      No AI helper is set up yet, so there is nothing to run.
      <button class="link" @click="view = 'agents'">Set one up</button>
    </p>
  </section>
</template>

<style scoped>
.remember {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  padding: 8px 0;
  font-size: 12px;
}
.composer {
  container-type: inline-size;
  width: 100%;
  max-width: 680px;
  margin: auto;
}
/* Secondary, outside the card: it describes past runs, not this one. */
.route-history { margin-top: 32px; font-size: 12px; }
.route-history summary {
  display: flex;
  align-items: center;
  gap: 6px;
  width: fit-content;
  padding: 4px 0;
  color: var(--text-dim);
  font-weight: 500;
  list-style: none;
  cursor: pointer;
  transition: color 120ms ease;
}
.route-history summary::-webkit-details-marker { display: none; }
.route-history summary:hover { color: var(--text); }
.route-history[open] summary svg { transform: rotate(180deg); }
.route-history table { width: 100%; margin-top: 8px; border-collapse: collapse; text-align: left; font-variant-numeric: tabular-nums; }
.route-history th, .route-history td { padding: 8px 0; border-bottom: 1px solid var(--border); }
.route-history th + th, .route-history td + td { padding-left: 12px; }
.route-history th { color: var(--text-faint); font-weight: 400; }
.route-history td { color: var(--text-dim); }
.route-history .cap { text-transform: capitalize; }
.route-history .num { text-align: right; }
.route-history > .note { margin: 10px 0 0; }
.hero {
  margin: 0 0 24px;
  font-size: 20px;
  line-height: 1.25;
  font-weight: 600;
  letter-spacing: -0.03em;
  text-wrap: balance;
  text-align: center;
}

.ask {
  border-color: var(--border);
  transition: border-color 120ms ease;
}
.ask:focus-within,
.ask.dragging {
  border-color: var(--focus);
}
textarea {
  display: block;
  width: 100%;
  min-height: 128px;
  max-height: 50vh;
  padding: 18px 18px 8px;
  background: none;
  color: var(--text);
  border: none;
  font: inherit;
  field-sizing: content;
  resize: none;
}
textarea::placeholder {
  color: var(--text-faint);
}
textarea:focus {
  outline: none;
}

.attachments {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin: 0;
  padding: 8px 18px 0;
  list-style: none;
}
.attachments .chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 100%;
}
.attachments .mono {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.unattach {
  padding: 0 2px;
  color: var(--text-faint);
}
.unattach:hover {
  color: var(--text);
}
.attach-error {
  margin: 6px 18px 0;
  font-size: 12px;
  color: var(--err);
}

.controls {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px 12px;
}
.controls :deep(.icon),
.options {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-height: 36px;
  padding: 8px;
  color: var(--text-faint);
  font-size: 12px;
  transition: background 120ms ease, color 120ms ease;
}
.controls :deep(.icon) {
  border-radius: var(--r-sm);
}
.options {
  width: 100%;
  padding: 10px 18px;
  border-top: 1px solid var(--border);
  text-align: left;
}
.options-label {
  color: var(--text-dim);
}
.settings-summary {
  margin-left: auto;
  text-align: right;
}
.options svg {
  flex-shrink: 0;
}
.options svg.expanded {
  transform: rotate(180deg);
}
.shortcut {
  margin-left: auto;
}
.task-refs {
  width: min(480px, calc(100vw - 24px));
  max-height: min(520px, calc(100dvh - 60px));
  padding: 16px;
  background: var(--surface);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
  color: var(--text);
  font-size: 12px;
}
.task-refs:popover-open {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.task-refs::backdrop {
  background: var(--overlay);
}
.refs-head h2 {
  margin: 0 0 4px;
  font-size: 14px;
  font-weight: 600;
}
.refs-head .note {
  margin: 0;
}
.refs-filter {
  min-height: 34px;
  padding: 4px 10px;
  color: var(--text);
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  font: inherit;
}
.refs-filter:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}
.refs-list {
  flex: 1;
  min-height: 0;
  margin: 0 -8px;
  padding: 0;
  overflow-y: auto;
  list-style: none;
}
.refs-list button {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 8px;
  border-radius: var(--r-sm);
  text-align: left;
  transition: background 120ms ease;
}
.refs-list button:hover,
.refs-list button:focus-visible {
  background: var(--surface-2);
}
.refs-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.refs-meta {
  flex-shrink: 0;
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
}
.refs-empty {
  padding: 8px;
}
.refs-group {
  padding: 12px 8px 4px;
  color: var(--text-faint);
}
.count {
  min-width: 18px;
  padding: 0 5px;
  background: var(--surface-2);
  border-radius: var(--r-sm);
  color: var(--text-dim);
  font-size: 11px;
  line-height: 18px;
  text-align: center;
  font-variant-numeric: tabular-nums;
}
.controls :deep(.icon:hover),
.options:hover,
.options[aria-expanded="true"] {
  background: var(--surface-2);
  color: var(--text);
}
.run {
  flex-shrink: 0;
  padding: 6px 14px;
  font-size: 12px;
}

.option-panel {
  padding: 0 18px;
  border-top: 1px solid var(--border);
}
.setting {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 12px 0;
}
.setting + .setting {
  border-top: 1px solid var(--border);
}
.setting.sub {
  padding-left: 14px;
}
.setting-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-width: 0;
}
.option-label {
  font-size: 12px;
  color: var(--text);
}
.sub .option-label {
  color: var(--text-dim);
}
.setting-hint {
  font-size: 11px;
  color: var(--text-faint);
}
.segments {
  display: flex;
  flex-shrink: 0;
  gap: 2px;
  padding: 2px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.seg {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 4px 12px;
  min-height: 28px;
  white-space: nowrap;
  border-radius: var(--r-sm);
  font-size: 12px;
  color: var(--text-faint);
  transition: color 120ms ease, background 120ms ease;
}
.seg:hover {
  background: var(--surface);
  color: var(--text);
}
.seg.on {
  background: var(--surface-2);
  color: var(--text);
}

.preview {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin: 0;
  padding: 12px 18px;
  border-top: 1px solid var(--border);
}
.select-field {
  display: block;
  position: relative;
  flex-shrink: 0;
  width: 200px;
}
.select-field svg {
  position: absolute;
  top: 50%;
  right: 6px;
  transform: translateY(-50%);
  pointer-events: none;
  color: var(--text-faint);
}
.select-field select {
  appearance: none;
  width: 100%;
  min-height: 34px;
  padding: 4px 28px 4px 10px;
  color: var(--text);
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  text-overflow: ellipsis;
  transition: background 120ms ease;
}
.select-field select:hover {
  background: var(--surface-2);
}
.select-field select:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}

@container (max-width: 520px) {
  .setting {
    flex-direction: column;
    align-items: stretch;
    gap: 8px;
  }
  .select-field {
    width: auto;
  }
  .seg {
    flex: 1;
  }
}

@media (max-width: 900px) {
  .shortcut {
    display: none;
  }
  .run {
    margin-left: auto;
  }
  .options {
    flex-wrap: wrap;
    gap: 4px 8px;
  }
  .settings-summary {
    order: 3;
    flex-basis: 100%;
    text-align: left;
  }
  .options svg {
    margin-left: auto;
  }
}
</style>
