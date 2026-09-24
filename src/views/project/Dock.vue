<script setup lang="ts">
import { inject } from "vue";
import DockCode from "./DockCode.vue";
import DockFiles from "./DockFiles.vue";
import DockHistory from "./DockHistory.vue";
import DockPreview from "./DockPreview.vue";
import DockTerminal from "./DockTerminal.vue";
import { DOCK } from "./dock";
import { PROJECT } from "./state";

// The dock's own chrome: the tab strip and whichever tab is on
// top. Every tab stays mounted underneath, so a build keeps running while you
// read a file.
const dock = inject(DOCK)!;
const { view } = inject(PROJECT)!;
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
      <button class="strip-button" title="Dock settings" aria-label="Dock settings" @click="view = 'settings'">
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
