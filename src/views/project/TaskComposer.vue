<script setup lang="ts">
import { inject } from "vue";
import { PROJECT } from "./state";

// The idle page: one prompt, one clear action. Options stay one click away.
const {
  task, attachments, attachError, dragging, addAttachments, pasteImages, fileName,
  schedulePreview, canRun, run, optionsOpen, MODES, mode, ISOLATIONS, isolation,
  installed, provider, providerPicked, pickedFor, previewing, preview, previewError,
  limitWarning, alternative, switchTo, runError, providerError, helpersPending, view,
  MODELS, modelChoices, chooseProvider, chooseModel, domId, anyRunning, runningCount,
} = inject(PROJECT)!;
</script>

<template>
  <section class="composer" aria-labelledby="composer-title">
    <h2 id="composer-title" class="hero">Start a new task</h2>
    <p class="lede">Ask a question, fix a bug, or build something new.</p>

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

      <div class="controls">
        <button class="icon" title="Attach files or images. You can also paste or drop them." aria-label="Attach files or images" @click="addAttachments(false)">
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M10.5 4.5 5.8 9.2a1.4 1.4 0 0 0 2 2l5-5a2.8 2.8 0 0 0-4-4l-5 5a4.2 4.2 0 0 0 6 6l4.2-4.2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
          Attach
        </button>
        <button class="icon" title="Attach a folder" aria-label="Add folder" @click="addAttachments(true)">
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
        </button>
        <span class="shortcut note">Ctrl + Enter</span>
        <button class="btn primary run" :disabled="!canRun" title="Run task (Ctrl+Enter)" @click="run()">
          Run task
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M3 8h10M8 3l5 5-5 5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </button>
      </div>

      <button class="options" :aria-expanded="optionsOpen" aria-controls="task-options" @click="optionsOpen = !optionsOpen">
        <span class="options-label">Task settings</span>
        <span class="settings-summary">
          <template v-if="installed.length">{{ providerPicked ? (provider === 'claude' ? 'Claude' : 'Codex') : "Auto" }} · </template>
          {{ MODES.find((m) => m.id === mode)?.label }} · {{ ISOLATIONS.find((w) => w.id === isolation)?.label }}
        </span>
        <svg :class="{ expanded: optionsOpen }" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m4 6 4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
      </button>

      <div v-show="optionsOpen" id="task-options" class="option-panel">
        <div v-if="installed.length" class="helper-options" role="group" aria-labelledby="helper-label">
          <span id="helper-label" class="option-label">AI helper</span>
          <div class="segments provider-options">
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
              {{ p.id === 'claude' ? 'Claude' : 'Codex' }}
            </button>
          </div>
          <div v-if="providerPicked" class="model-options">
            <label>
              <span class="option-label">Model</span>
              <span class="select-field">
                <select :value="modelChoices[provider].model" @change="chooseModel(($event.target as HTMLSelectElement).value)">
                  <option v-for="item in MODELS[provider]" :key="item.id" :value="item.id">{{ item.label }}</option>
                </select>
                <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m5 6 3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
              </span>
            </label>
            <label class="reasoning-field">
              <span class="option-label">Reasoning</span>
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
          </div>
        </div>
        <div>
          <span class="option-label">Approach</span>
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
        <div>
          <span class="option-label">Working location</span>
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
      </div>

      <div v-if="previewing" class="preview note" aria-live="polite">Figuring out the best way to do it…</div>
      <div v-else-if="preview" class="preview" aria-live="polite">
        <span class="note">Questions get an answer. Code changes are planned, made, and checked.</span>
        <span v-if="isolation === 'worktree'" class="note">
          I’ll work in a separate copy on a new branch and leave this folder alone.
          <template v-if="preview.git.dirty">
            Your {{ preview.git.dirtyCount }} uncommitted change{{ preview.git.dirtyCount === 1 ? "" : "s" }} won’t be in it.
          </template>
          Files Git ignores, like node_modules, aren’t copied.
        </span>
        <span v-else-if="preview.git.dirty" class="note">
          I’ll keep your {{ preview.git.dirtyCount }} existing change{{ preview.git.dirtyCount === 1 ? "" : "s" }} safe.
        </span>
        <!-- Only reported figures reach the words; the per-call threshold is a guess and is not shown as a number. -->
        <span v-if="limitWarning" class="missing" role="status">
          {{ preview.provider }}’s {{ limitWarning.window.label }} limit is {{ Math.round(limitWarning.window.usedPercent) }}% used<template v-if="limitWarning.resets">, resets {{ limitWarning.resets }}</template>.
          This takes at least {{ limitWarning.calls }} AI {{ limitWarning.calls === 1 ? "call" : "calls" }}, so it might not finish.
          <button v-if="alternative" class="link" @click="switchTo(alternative)">use {{ alternative }} instead</button>
        </span>
        <span v-if="pickedFor && !providerPicked" class="note">{{ pickedFor }}</span>
        <!-- Two agents editing one folder is allowed, but what each one changed
             stops being separable, so the composer says so before the click. -->
        <span v-if="anyRunning && isolation === 'currentTree'" class="missing" role="status">
          {{ runningCount === 1 ? "Another task is" : `${runningCount} other tasks are` }} already changing this folder.
          Their changes and this one will be mixed together in every diff.
        </span>
      </div>
      <p v-else-if="previewError" class="preview missing" role="status">Preview unavailable: {{ previewError }}</p>
    </section>

    <p v-if="runError" class="missing">{{ runError }}</p>
    <p v-else-if="!installed.length && !providerError && !helpersPending" class="missing">
      No AI helper is set up yet, so there is nothing to run.
      <button class="link" @click="view = 'helpers'">Set one up</button>
    </p>
  </section>
</template>

<style scoped>
.composer {
  container-type: inline-size;
  width: 100%;
  max-width: 680px;
  margin: auto;
}
.hero {
  margin: 0;
  font-size: 20px;
  line-height: 1.25;
  font-weight: 600;
  letter-spacing: -0.03em;
  text-wrap: balance;
  text-align: center;
}
.lede {
  margin: 8px 0 24px;
  color: var(--text-dim);
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
  padding: 18px 18px 8px;
  background: none;
  color: var(--text);
  border: none;
  font: inherit;
  resize: vertical;
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
.icon,
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
.icon {
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
.icon:hover,
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
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 18px 24px;
  padding: 18px;
  border-top: 1px solid var(--border);
}
.option-label {
  display: block;
  margin-bottom: 5px;
  font-size: 12px;
  color: var(--text-dim);
}
.segments {
  display: flex;
  flex-wrap: wrap;
  gap: 2px;
  padding: 2px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.seg {
  flex: 1;
  padding: 4px 11px;
  min-height: 32px;
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
.helper-options {
  grid-column: 1 / -1;
  min-width: 0;
  padding-bottom: 18px;
  border-bottom: 1px solid var(--border);
}
.provider-options {
  flex-wrap: nowrap;
}
.model-options {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  margin-top: 8px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.model-options label {
  min-width: 0;
  padding: 8px;
}
.reasoning-field {
  border-left: 1px solid var(--border);
}
.model-options .option-label {
  margin: 0 6px 2px;
  color: var(--text-faint);
  font-size: 11px;
}
.select-field {
  display: block;
  position: relative;
}
.select-field svg {
  position: absolute;
  top: 50%;
  right: 6px;
  transform: translateY(-50%);
  pointer-events: none;
  color: var(--text-faint);
}
.model-options select {
  appearance: none;
  width: 100%;
  min-height: 32px;
  padding: 4px 28px 4px 6px;
  color: var(--text);
  background: var(--bg);
  border: none;
  border-radius: var(--r-sm);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  text-overflow: ellipsis;
  transition: background 120ms ease;
}
.model-options select:hover {
  background: var(--surface-2);
}
.model-options select:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}

@container (max-width: 440px) {
  .option-panel {
    grid-template-columns: minmax(0, 1fr);
    gap: 16px;
  }
}
@container (max-width: 340px) {
  .model-options {
    grid-template-columns: minmax(0, 1fr);
  }
  .reasoning-field {
    border-left: none;
    border-top: 1px solid var(--border);
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
