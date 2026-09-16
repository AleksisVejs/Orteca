<script setup lang="ts">
import { inject } from "vue";
import { PROJECT } from "./state";

// The idle page: one prompt, one clear action. Options stay one click away.
const {
  task, attachments, attachError, dragging, addAttachments, pasteImages, fileName,
  schedulePreview, canRun, run, optionsOpen, MODES, mode, ISOLATIONS, isolation,
  installed, provider, providerPicked, pickedFor, previewing, preview, previewError,
  limitWarning, alternative, switchTo, runError, providerError, helpersPending, view,
} = inject(PROJECT)!;
</script>

<template>
  <h2 class="hero">What do you want to build?</h2>
  <p class="lede">Describe it in plain words. Orteca picks the steps.</p>

  <section class="ask card" :class="{ dragging }">
    <label class="hidden-label" for="task">What do you want to build?</label>
    <textarea
      id="task"
      v-model="task"
      rows="4"
      spellcheck="false"
      placeholder="Example: Add dark mode and make sure it works."
      @input="schedulePreview"
      @paste="pasteImages"
      @keydown.ctrl.enter.prevent="run"
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
      <button class="icon" title="Attach files or images. You can also paste or drop them." aria-label="Add files" @click="addAttachments(false)">
        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M10.5 4.5 5.8 9.2a1.4 1.4 0 0 0 2 2l5-5a2.8 2.8 0 0 0-4-4l-5 5a4.2 4.2 0 0 0 6 6l4.2-4.2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
      </button>
      <button class="icon" title="Attach a folder" aria-label="Add folder" @click="addAttachments(true)">
        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
      </button>
      <button class="options" :aria-expanded="optionsOpen" @click="optionsOpen = !optionsOpen">
        <template v-if="installed.length">{{ provider }} · </template>
        {{ MODES.find((m) => m.id === mode)?.label }} · {{ ISOLATIONS.find((w) => w.id === isolation)?.label }}
        <span aria-hidden="true">{{ optionsOpen ? "▴" : "▾" }}</span>
      </button>
      <button class="btn primary run" :disabled="!canRun" title="Ctrl+Enter" @click="run">Do it</button>
    </div>

    <div v-if="optionsOpen" class="option-panel">
      <div>
        <span class="option-label">How careful?</span>
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
        <span class="option-label">Where?</span>
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
      <div v-if="installed.length">
        <span class="option-label">Which AI?</span>
        <div class="segments">
          <button
            v-for="p in installed"
            :key="p.id"
            class="seg"
            :class="{ on: provider === p.id }"
            :aria-pressed="provider === p.id"
            @click="provider = p.id; providerPicked = true; pickedFor = null; schedulePreview()"
          >
            {{ p.program }}
          </button>
        </div>
      </div>
    </div>

    <div v-if="previewing" class="preview note" aria-live="polite">Figuring out the best way to do it…</div>
    <div v-else-if="preview" class="preview" aria-live="polite">
      <span class="note">A small model reads your request first: a question just gets an answer, and a change gets planned, made and checked.</span>
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
    </div>
    <p v-else-if="previewError" class="preview missing" role="status">Preview unavailable: {{ previewError }}</p>
  </section>

  <p v-if="runError" class="missing">{{ runError }}</p>
  <p v-else-if="!installed.length && !providerError && !helpersPending" class="missing">
    No AI helper is set up yet, so there is nothing to run.
    <button class="link" @click="view = 'helpers'">Set one up</button>
  </p>
</template>

<style scoped>
.hero {
  margin: 24px 0 0;
  font-size: 30px;
  line-height: 1.25;
  font-weight: 600;
  letter-spacing: -0.03em;
}
.lede {
  margin: 6px 0 24px;
  color: var(--text-dim);
}

.ask {
  border-color: var(--border-strong);
  transition: border-color 120ms ease;
}
.ask:focus-within,
.ask.dragging {
  border-color: var(--info);
}
textarea {
  display: block;
  width: 100%;
  min-height: 110px;
  padding: 16px 18px 4px;
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
  gap: 4px;
  padding: 10px 12px 12px;
}
.icon,
.options {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  font-size: 12px;
  transition: background 120ms ease, color 120ms ease;
}
.icon:hover,
.options:hover,
.options[aria-expanded="true"] {
  background: var(--surface-2);
  color: var(--text);
}
.run {
  margin-left: auto;
  padding: 7px 20px;
}

.option-panel {
  display: flex;
  flex-wrap: wrap;
  gap: 16px 24px;
  padding: 12px 18px 16px;
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
  gap: 2px;
  padding: 2px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.seg {
  padding: 4px 11px;
  border-radius: 4px;
  font-size: 12px;
  color: var(--text-faint);
  transition: color 120ms ease, background 120ms ease;
}
.seg:hover {
  color: var(--text-dim);
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
  padding: 10px 18px 12px;
  border-top: 1px solid var(--border);
}
</style>
