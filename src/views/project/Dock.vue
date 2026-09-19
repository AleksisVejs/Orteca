<script setup lang="ts">
import { inject } from "vue";
import DockCode from "./DockCode.vue";
import DockFiles from "./DockFiles.vue";
import DockHistory from "./DockHistory.vue";
import DockPreview from "./DockPreview.vue";
import DockTerminal from "./DockTerminal.vue";
import { DOCK, FONTS, SHELLS } from "./dock";

// The dock's own chrome: the tab strip, the settings and whichever tab is on
// top. Every tab stays mounted underneath, so a build keeps running while you
// read a file.
const dock = inject(DOCK)!;
const { prefs, tabs, active } = dock;

const KIND_LABEL = { terminal: "Terminal", files: "File tree", code: "File", preview: "Preview", history: "Commit history" } as const;
</script>

<template>
  <section class="dock" aria-label="Workspace dock">
    <header class="tabstrip">
      <div class="tabs" role="tablist" aria-label="Dock tabs">
        <div v-for="tab in tabs" :key="tab.id" class="tab" :class="{ on: tab.id === active }">
          <button
            role="tab"
            :aria-selected="tab.id === active"
            :title="`${KIND_LABEL[tab.kind]}${tab.file ? ` · ${tab.file}` : ''}`"
            @click="active = tab.id"
          >
            <svg v-if="tab.kind === 'terminal'" viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="m3 4 3 3-3 3M8.5 11H13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
            <svg v-else-if="tab.kind === 'files'" viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
            <svg v-else-if="tab.kind === 'code'" viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M4 2h5l3 3v9H4V2Zm5 0v3h3M6 8h4M6 11h3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
            <svg v-else-if="tab.kind === 'preview'" viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><rect x="2" y="3" width="12" height="10" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M2 6h12" stroke="currentColor" stroke-width="1.3" /></svg>
            <svg v-else viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><circle cx="4" cy="4" r="1.5" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4" cy="12" r="1.5" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4 5.5v5M7 4h6M7 12h6" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
            <span class="tab-name">{{ tab.title }}</span>
            <span v-if="tab.dirty" class="tab-mark" aria-label="Unsaved changes">•</span>
            <span v-else-if="tab.exited" class="tab-mark" aria-label="The shell exited">□</span>
          </button>
          <button class="tab-close" :title="`Close ${tab.title}`" :aria-label="`Close ${tab.title}`" @click="dock.closeTab(tab.id)">
            <svg viewBox="0 0 16 16" width="10" height="10" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
          </button>
        </div>
      </div>

      <button class="strip-button" title="New terminal (Ctrl+Shift+`)" aria-label="New terminal" @click="dock.openTerminal()">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
      </button>
      <button class="strip-button" title="File tree" aria-label="File tree" @click="dock.openFiles()">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
      </button>
      <button class="strip-button" title="Commit history" aria-label="Commit history" @click="dock.openHistory()">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><circle cx="4" cy="4" r="1.5" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4" cy="12" r="1.5" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4 5.5v5M7 4h6M7 12h6" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
      </button>
      <button class="strip-button" title="Dev server preview" aria-label="Dev server preview" @click="dock.openPreview()">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="2" y="3" width="12" height="10" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M2 6h12" stroke="currentColor" stroke-width="1.3" /></svg>
      </button>
      <button class="strip-button" popovertarget="dock-settings" title="Dock settings" aria-label="Dock settings">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><circle cx="8" cy="8" r="2.2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M8 1.6v1.6M8 12.8v1.6M14.4 8h-1.6M3.2 8H1.6m10.9-4.5-1.1 1.1M4.6 11.4l-1.1 1.1m9 0-1.1-1.1M4.6 4.6 3.5 3.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
      </button>
      <button
        class="strip-button"
        :title="`Move the dock ${prefs.side === 'right' ? 'below the workspace' : 'beside the workspace'}`"
        :aria-label="`Move the dock ${prefs.side === 'right' ? 'below the workspace' : 'beside the workspace'}`"
        @click="prefs.side = prefs.side === 'right' ? 'bottom' : 'right'"
      >
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="2" y="2.5" width="12" height="11" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.3" /><path :d="prefs.side === 'right' ? 'M10 2.5v11' : 'M2 9.5h12'" stroke="currentColor" stroke-width="1.3" /></svg>
      </button>
      <button class="strip-button" title="Hide the dock (Ctrl+`)" aria-label="Hide the dock" @click="prefs.open = false">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
      </button>

      <section id="dock-settings" class="settings" popover role="dialog" aria-labelledby="dock-settings-title">
        <header class="settings-head">
          <h2 id="dock-settings-title">Dock settings</h2>
          <button class="link" @click="dock.resetPrefs()">Reset</button>
        </header>

        <h3 class="label">Terminal</h3>
        <label class="field">
          <span>Shell</span>
          <select v-model="prefs.shell">
            <option v-for="s in SHELLS" :key="s.label" :value="s.value">{{ s.label }}</option>
            <option v-if="!SHELLS.some(s => s.value === prefs.shell)" :value="prefs.shell">Custom</option>
          </select>
        </label>
        <label class="field">
          <span>Or a command</span>
          <input v-model="prefs.shell" class="mono" placeholder="Leave blank for PowerShell" />
        </label>
        <p class="note">New terminals use this. Open ones keep the shell they started with.</p>
        <label class="field">
          <span>Scrollback</span>
          <input v-model.number="prefs.scrollback" type="number" min="200" max="200000" step="500" />
        </label>
        <label class="check"><input v-model="prefs.copyOnSelect" type="checkbox" /> Copy as soon as text is selected</label>
        <label class="check"><input v-model="prefs.smartCopy" type="checkbox" /> Ctrl+C copies a selection, otherwise interrupts</label>

        <h3 class="label">Type</h3>
        <label class="field">
          <span>Font</span>
          <select v-model="prefs.fontFamily">
            <option v-for="f in FONTS" :key="f.label" :value="f.value">{{ f.label }}</option>
          </select>
        </label>
        <label class="field">
          <span>Size</span>
          <input v-model.number="prefs.fontSize" type="number" min="8" max="28" />
        </label>
        <label class="field">
          <span>Cursor</span>
          <select v-model="prefs.cursorStyle">
            <option value="bar">Bar</option>
            <option value="block">Block</option>
            <option value="underline">Underline</option>
          </select>
        </label>
        <label class="check"><input v-model="prefs.cursorBlink" type="checkbox" /> Blink the cursor</label>

        <h3 class="label">Layout</h3>
        <label class="field">
          <span>Position</span>
          <select v-model="prefs.side">
            <option value="right">Beside the workspace</option>
            <option value="bottom">Below the workspace</option>
          </select>
        </label>
        <label class="check"><input v-model="prefs.followDevServer" type="checkbox" /> Point the preview at the last address a terminal printed</label>
      </section>
    </header>

    <div class="bodies">
      <p v-if="!tabs.length" class="note empty">
        Nothing open. Start a terminal, browse the files, read the history, or point a preview at your dev server.
      </p>
      <div v-for="tab in tabs" v-show="tab.id === active" :key="tab.id" class="body" role="tabpanel">
        <DockTerminal v-if="tab.kind === 'terminal'" :tab="tab" :visible="tab.id === active" />
        <DockFiles v-else-if="tab.kind === 'files'" />
        <DockCode v-else-if="tab.kind === 'code'" :tab="tab" />
        <DockHistory v-else-if="tab.kind === 'history'" />
        <DockPreview v-else :tab="tab" />
      </div>
    </div>
  </section>
</template>

<style scoped>
.dock {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  height: 100%;
  background: var(--bg);
}
.tabstrip {
  display: flex;
  align-items: stretch;
  flex-shrink: 0;
  min-height: 34px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
}
.tabs {
  display: flex;
  flex: 1;
  min-width: 0;
  overflow-x: auto;
}
.tab {
  position: relative;
  display: flex;
  align-items: center;
  flex-shrink: 0;
  max-width: 180px;
  border-right: 1px solid var(--border);
}
.tab > button:first-child {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  padding: 0 24px 0 10px;
  height: 100%;
  color: var(--text-faint);
  font-size: 12px;
  transition: background 120ms ease, color 120ms ease;
}
.tab:hover > button:first-child,
.tab.on > button:first-child {
  color: var(--text);
}
.tab.on {
  background: var(--bg);
}
.tab svg {
  flex-shrink: 0;
}
.tab-name {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.tab-mark {
  flex-shrink: 0;
  color: var(--warn);
}
.tab-close {
  position: absolute;
  right: 4px;
  display: flex;
  padding: 4px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  opacity: 0;
}
.tab:hover .tab-close,
.tab-close:focus-visible {
  opacity: 1;
}
.tab-close:hover {
  background: var(--surface-2);
  color: var(--text);
}
.strip-button {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  padding: 0 8px;
  color: var(--text-faint);
  transition: background 120ms ease, color 120ms ease;
}
.strip-button:hover {
  background: var(--surface-2);
  color: var(--text);
}

.settings {
  inset: auto;
  position: fixed;
  top: 50%;
  left: 50%;
  translate: -50% -50%;
  width: min(420px, calc(100vw - 32px));
  max-height: calc(100dvh - 80px);
  overflow-y: auto;
  margin: 0;
  padding: 18px;
  background: var(--surface);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
  font-size: 12px;
}
.settings::backdrop {
  background: var(--overlay);
}
.settings-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--gap);
  margin-bottom: 4px;
}
.settings-head h2 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}
.settings .label {
  margin: 20px 0 8px;
  color: var(--text-faint);
}
.field {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--gap);
  margin-bottom: 8px;
  color: var(--text-dim);
}
.field input,
.field select {
  width: 220px;
  max-width: 55%;
  padding: 5px 8px;
  background: var(--bg);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  font: inherit;
}
.check {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  color: var(--text-dim);
}
.settings .note {
  margin: 0 0 12px;
}

.bodies {
  position: relative;
  display: flex;
  flex: 1;
  min-height: 0;
}
.body {
  flex: 1;
  min-width: 0;
  min-height: 0;
}
.empty {
  align-self: center;
  width: 100%;
  max-width: 44ch;
  margin: 0 auto;
  padding: 0 20px;
  text-align: center;
}
</style>
