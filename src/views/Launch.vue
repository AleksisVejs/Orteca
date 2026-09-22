<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import VeloMark from "../components/VeloMark.vue";
import { detectProviders, forgetProject, gitStatus, isAppError, onFileDrop, pickFolder, recentProjects } from "../api";
import Memory from "./project/Memory.vue";
import { visiblePath } from "../path";
import type { Detected, GitState, Project } from "../types";

/** Projects already open, with how many runs each has going right now. */
type OpenProject = { path: string; name: string; running: number };
const props = defineProps<{ openError?: string | null; openProjects?: OpenProject[] }>();
const emit = defineEmits<{ open: [path: string]; closeProject: [path: string] }>();

const RECENT_SHOWN = 5;
const AUTH: Record<string, string> = { subscription: "signed in", apiKey: "API key", signedOut: "signed out", unknown: "" };

const open = computed(() => props.openProjects ?? []);

const recents = ref<Project[]>([]);
const git = ref<Record<string, GitState>>({});
const providers = ref<Detected[]>([]);
const dragging = ref(false);
const list = ref<HTMLElement | null>(null);
const loadError = ref<string | null>(null);
const error = computed(() => props.openError ?? loadError.value);

/** The newest recent project, offered as one click. Without one, the plain button leads. */
const resume = computed(() => recents.value[0] ?? null);

type Row = { path: string; name: string; running: number; isOpen: boolean; at?: string };
/** Open projects first, then the other recents: one list, one look. */
const rows = computed<Row[]>(() => {
  const isOpen = new Set(open.value.map((p) => p.path));
  const others = recents.value.slice(1).filter((p) => !isOpen.has(p.path)).slice(0, RECENT_SHOWN);
  return [
    ...open.value.map((p) => ({ ...p, isOpen: true })),
    ...others.map((p) => ({ path: p.path, name: p.name, running: 0, isOpen: false, at: p.lastOpenedAt })),
  ];
});

const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
/** SQLite stamps are UTC "YYYY-MM-DD HH:MM:SS". */
function ago(stamp?: string) {
  if (!stamp) return "";
  const secs = (new Date(stamp.replace(" ", "T") + "Z").getTime() - Date.now()) / 1000;
  if (Number.isNaN(secs)) return "";
  for (const [unit, size] of [["day", 86400], ["hour", 3600], ["minute", 60]] as const) {
    if (Math.abs(secs) >= size) return relative.format(Math.round(secs / size), unit);
  }
  return "just now";
}

function branchOf(path: string) {
  const g = git.value[path];
  return g?.branch ? `${g.branch}${g.dirty ? " · uncommitted changes" : ""}` : "";
}

async function refresh() {
  try {
    recents.value = await recentProjects();
  } catch (e) {
    loadError.value = isAppError(e) ? e.message : String(e);
    return;
  }
  // An untrusted project refuses a status read; it just shows no branch.
  for (const p of recents.value) {
    gitStatus(p.path).then((g) => (git.value[p.path] = g)).catch(() => {});
  }
}

function onKey(e: KeyboardEvent) {
  if (e.ctrlKey && !e.altKey && e.key.toLowerCase() === "o") {
    e.preventDefault();
    choose();
  }
}

/** Arrow keys walk the project list. */
function walk(e: KeyboardEvent) {
  const step = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
  if (!step) return;
  const items = Array.from(list.value?.querySelectorAll<HTMLElement>(".entry") ?? []);
  const at = items.indexOf(document.activeElement as HTMLElement);
  items[Math.min(items.length - 1, Math.max(0, at + step))]?.focus();
  e.preventDefault();
}

let unlisten: (() => void) | undefined;
let gone = false;
onMounted(async () => {
  refresh();
  detectProviders((d) => (providers.value = [...providers.value.filter((x) => x.id !== d.id), d])).catch(() => {});
  window.addEventListener("keydown", onKey);
  const off = await onFileDrop((over, paths) => {
    dragging.value = over;
    if (paths[0]) emit("open", paths[0]);
  });
  if (gone) off();
  else unlisten = off;
});
onUnmounted(() => {
  gone = true;
  window.removeEventListener("keydown", onKey);
  unlisten?.();
});

/** Centres the lit patch of the dot grid on the cursor. Direct style, no re-render per move. */
const glow = ref<HTMLElement | null>(null);
const GLOW = 160; // radius, matches the mask size in CSS
function light(e: PointerEvent) {
  const box = (e.currentTarget as HTMLElement).getBoundingClientRect();
  glow.value?.style.setProperty("mask-position", `${e.clientX - box.left - GLOW}px ${e.clientY - box.top - GLOW}px`);
}

async function choose() {
  loadError.value = null;
  const path = await pickFolder();
  if (path) emit("open", path);
}

async function forget(path: string) {
  await forgetProject(path);
  await refresh();
}
</script>

<template>
  <main class="launch" :class="{ dragging }" @pointermove="light">
    <div ref="glow" class="glow" aria-hidden="true"></div>
    <div class="column">
      <header class="hero">
        <VeloMark :size="48" />
        <h1>Let’s get to work.</h1>
        <p class="tagline">Pick a project, then give your coding agents a task.</p>
      </header>

      <p v-if="error" class="error" role="alert">{{ error }}</p>

      <section v-if="resume">
        <h2 class="label">Last opened</h2>
        <div class="card resume">
          <div class="info">
            <span class="name">{{ resume.name }}</span>
            <span class="mono path" :title="visiblePath(resume.path)">{{ visiblePath(resume.path) }}</span>
            <span v-if="branchOf(resume.path) || resume.lastOpenedAt" class="meta">
              <span v-if="branchOf(resume.path)" class="mono">{{ branchOf(resume.path) }}</span>
              <span>{{ ago(resume.lastOpenedAt) }}</span>
            </span>
          </div>
          <button class="btn primary" @click="emit('open', resume.path)">
            Continue
            <svg viewBox="0 0 20 20" width="16" height="16" aria-hidden="true"><path d="M4 10h12m-5-5 5 5-5 5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
          </button>
        </div>
      </section>

      <section>
        <h2 class="label">{{ resume ? "Projects" : "Get started" }}</h2>
        <ul ref="list" class="card projects" @keydown="walk">
          <li v-for="p in rows" :key="p.path">
            <button class="entry" @click="emit('open', p.path)">
              <span class="info">
                <span class="name">
                  <span v-if="p.isOpen" class="dot" :class="{ live: p.running }" aria-hidden="true"></span>
                  {{ p.name }}
                </span>
                <span class="mono path" :title="visiblePath(p.path)">{{ visiblePath(p.path) }}</span>
              </span>
              <span class="side">
                <span v-if="branchOf(p.path)" class="mono">{{ branchOf(p.path) }}</span>
                <span v-if="p.running" class="note" role="status">{{ p.running === 1 ? "1 task running" : `${p.running} tasks running` }}</span>
                <span v-else class="note">{{ p.isOpen ? "open" : ago(p.at) }}</span>
              </span>
            </button>
            <button
              v-if="p.isOpen"
              class="forget"
              :disabled="p.running > 0"
              :title="p.running ? 'Stop its tasks before closing it' : 'Close this project'"
              :aria-label="`Close ${p.name}`"
              @click="emit('closeProject', p.path)"
            >
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
            </button>
            <button v-else class="forget" title="Remove from list" :aria-label="`Remove ${p.name} from recent projects`" @click="forget(p.path)">
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
            </button>
          </li>
          <li class="add">
            <button class="entry" @click="choose">
              <span class="info">
                <span class="name">
                  <svg viewBox="0 0 20 20" width="16" height="16" aria-hidden="true"><path d="M3 6v10a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8a1 1 0 0 0-1-1h-6L8 4H4a1 1 0 0 0-1 1v1Z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" /></svg>
                  {{ resume ? "Open another folder" : "Open a project folder" }}
                </span>
                <span class="note">{{ dragging ? "Drop the folder to open it." : "or drop a folder anywhere in this window" }}</span>
              </span>
              <kbd>Ctrl+O</kbd>
            </button>
          </li>
        </ul>
      </section>
    </div>

    <footer class="foot">
      <span v-for="d in providers" :key="d.id" class="cli">
        <span class="dot" :class="!d.path ? 'bad' : d.auth === 'signedOut' ? 'warn' : 'ok'" aria-hidden="true"></span>
        {{ d.id }} {{ d.path ? [d.version, AUTH[d.auth]].filter(Boolean).join(" · ") : "not installed" }}
      </span>
      <span class="spacer"></span>
      <Memory pop-id="memory-launch" button-class="foot-btn" />
    </footer>
  </main>
</template>

<style scoped>
/* A faint dot grid; a brighter copy of it shows only in a circle under the cursor. */
.launch {
  position: relative;
  height: 100%;
  display: flex;
  flex-direction: column;
  background: radial-gradient(circle, var(--dots) 1px, transparent 1.5px) center / 22px 22px;
}
.glow {
  position: absolute;
  inset: 0;
  pointer-events: none;
  background: radial-gradient(circle, var(--dots-lit) 1px, transparent 1.5px) center / 22px 22px;
  mask: radial-gradient(closest-side, black, transparent) no-repeat -999px -999px / 320px 320px;
  opacity: 0;
  transition: opacity 120ms ease;
}
.launch:hover .glow {
  opacity: 1;
}
/* A folder held over the window lights the whole grid. */
.launch.dragging .glow {
  mask: none;
  opacity: 0.5;
}

.column {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 32px;
  padding: 48px max(var(--pad), calc((100% - 560px) / 2));
}
/* margin:auto centres the column when it fits and still lets it scroll when it doesn't */
.column > :first-child {
  margin-top: auto;
}
.column > :last-child {
  margin-bottom: auto;
}

.hero {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
}
h1 {
  margin: 16px 0 0;
  font-size: 30px;
  font-weight: 600;
  letter-spacing: -0.03em;
}
.tagline {
  margin: 8px 0 0;
  color: var(--text-dim);
}
.error {
  margin: -16px 0 0;
  text-align: center;
  color: var(--err);
  font-size: 12px;
}

.info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.name {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-weight: 600;
}
.path {
  max-width: 100%;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.meta {
  display: flex;
  gap: 12px;
  margin-top: 4px;
  font-size: 12px;
  color: var(--text-faint);
}

.resume {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 18px;
}
.resume .btn {
  flex-shrink: 0;
}

.projects {
  list-style: none;
  margin: 0;
  padding: 0;
  overflow: hidden;
}
.projects li {
  display: flex;
  align-items: center;
  padding-right: 8px;
  transition: background 120ms ease;
}
.projects li + li {
  border-top: 1px solid var(--border);
}
.projects li:hover,
.dragging .add {
  background: var(--surface-2);
}
.entry {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 12px 8px 12px 18px;
  text-align: left;
}
.side {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 2px;
  flex-shrink: 0;
}

.add .name {
  font-weight: 500;
  color: var(--text-dim);
  transition: color 120ms ease;
}
.add:hover .name {
  color: var(--text);
}
kbd {
  padding: 0 5px;
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  font: inherit;
  font-size: 11px;
  color: var(--text-faint);
}

.forget {
  display: grid;
  place-items: center;
  width: 28px;
  height: 28px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  transition: background 120ms ease, color 120ms ease;
}
.forget:hover:not(:disabled) {
  background: var(--border);
  color: var(--text);
}
.forget:disabled {
  opacity: 0.4;
}

/* Slim status line, like the workspace's: CLI state left, Memory right. */
.foot {
  position: relative;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 16px;
  min-height: 28px;
  padding: 2px var(--pad);
  border-top: 1px solid var(--border);
  background: var(--bg);
  font-size: 11px;
  color: var(--text-faint);
}
.cli {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.foot .dot {
  width: 6px;
  height: 6px;
}
.spacer {
  flex: 1;
}
.foot :deep(.foot-btn) {
  padding: 2px 8px;
  border-radius: var(--r-sm);
  color: var(--text-dim);
  transition: background 120ms ease, color 120ms ease;
}
.foot :deep(.foot-btn:hover) {
  background: var(--surface-2);
  color: var(--text);
}
</style>
