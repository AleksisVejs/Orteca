<script setup lang="ts">
import { computed, inject, onMounted, reactive, ref } from "vue";
import { isAppError, listDir, openFile } from "../../api";
import { DOCK } from "./dock";
import type { DirEntry } from "../../types";

// The project's own files. Folders are read when they are first opened and
// kept, so collapsing and expanding again costs nothing; Refresh re-reads.
const dock = inject(DOCK)!;

const children = reactive<Record<string, DirEntry[]>>({});
const open = ref(new Set<string>());
const error = ref<string | null>(null);

type Row = { path: string; name: string; dir: boolean; depth: number };

const rows = computed(() => {
  const out: Row[] = [];
  const walk = (dir: string, depth: number) => {
    for (const entry of children[dir] ?? []) {
      const path = dir ? `${dir}/${entry.name}` : entry.name;
      out.push({ path, name: entry.name, dir: entry.dir, depth });
      if (entry.dir && open.value.has(path)) walk(path, depth + 1);
    }
  };
  walk("", 0);
  return out;
});

async function load(dir: string) {
  try {
    children[dir] = await listDir(dock.path, dir);
    error.value = null;
  } catch (e) {
    error.value = isAppError(e) ? e.message : String(e);
  }
}

async function toggle(path: string) {
  const showing = new Set(open.value);
  if (showing.delete(path)) return void (open.value = showing);
  if (!children[path]) await load(path);
  showing.add(path);
  open.value = showing;
}

/** Re-read every folder that is open, so a run's new files show up. */
const refresh = () => Promise.all(["", ...open.value].map(load));

const reveal = (path: string) =>
  openFile(dock.path, path, true).catch((e) => (error.value = isAppError(e) ? e.message : String(e)));

onMounted(() => load(""));
</script>

<template>
  <div class="files-tab">
    <header class="files-head">
      <h2 class="label">Files</h2>
      <button class="link" @click="refresh">Refresh</button>
    </header>
    <p v-if="error" class="note files-error" role="alert">{{ error }}</p>
    <ul class="tree">
      <li v-for="row in rows" :key="row.path">
        <button
          class="row"
          :style="{ paddingLeft: `${8 + row.depth * 14}px` }"
          :title="row.path"
          @click="row.dir ? toggle(row.path) : dock.openCode(row.path)"
        >
          <svg v-if="row.dir" viewBox="0 0 16 16" width="12" height="12" class="twist" :class="{ open: open.has(row.path) }" aria-hidden="true">
            <path d="m6 4 4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
          <span v-else class="twist" aria-hidden="true"></span>
          <span class="name mono">{{ row.name }}</span>
        </button>
        <button
          v-if="row.dir"
          class="row-action"
          :title="`Open a terminal in ${row.name}`"
          :aria-label="`Open a terminal in ${row.name}`"
          @click="dock.openTerminal(row.path)"
        >
          <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="m3 4 3 3-3 3M8.5 11H13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </button>
        <button
          v-else
          class="row-action"
          :title="`Show ${row.name} in folder`"
          :aria-label="`Show ${row.name} in folder`"
          @click="reveal(row.path)"
        >
          <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
        </button>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.files-tab {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background: var(--bg);
}
.files-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--gap);
  flex-shrink: 0;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
}
.files-head .label {
  margin: 0;
  color: var(--text-faint);
}
.files-error {
  margin: 0;
  padding: 8px 10px;
  color: var(--err);
}
.tree {
  flex: 1;
  min-height: 0;
  overflow: auto;
  margin: 0;
  padding: 4px 0 12px;
  list-style: none;
}
.tree li {
  position: relative;
  display: flex;
  align-items: center;
}
.row {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  min-width: 0;
  padding: 3px 28px 3px 8px;
  color: var(--text-dim);
  text-align: left;
  transition: background 120ms ease, color 120ms ease;
}
.row:hover,
.tree li:hover .row {
  background: var(--surface);
  color: var(--text);
}
.twist {
  flex-shrink: 0;
  width: 12px;
  color: var(--text-faint);
}
.twist.open {
  transform: rotate(90deg);
}
.name {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: 12px;
}
.row-action {
  position: absolute;
  right: 6px;
  display: flex;
  align-items: center;
  padding: 4px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  opacity: 0;
}
.tree li:hover .row-action,
.row-action:focus-visible {
  opacity: 1;
}
.row-action:hover {
  background: var(--surface-2);
  color: var(--text);
}
</style>
