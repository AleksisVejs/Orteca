<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, provide, ref, watch } from "vue";
import VeloMark from "../components/VeloMark.vue";
import AiHelpers from "./project/AiHelpers.vue";
import Dock from "./project/Dock.vue";
import PastTask from "./project/PastTask.vue";
import TaskComposer from "./project/TaskComposer.vue";
import TaskResult from "./project/TaskResult.vue";
import TaskRun from "./project/TaskRun.vue";
import { DOCK, useDock } from "./project/dock";
import { PROJECT, useProject } from "./project/state";
import type { GitAction, OpenedProject } from "../types";
import { visiblePath } from "../path";

// The shell: sidebar, top bar and whichever page the sidebar picked.
// All state lives in `useProject`; the pages inject it.
const props = defineProps<{ opened: OpenedProject; active: boolean }>();
const emit = defineEmits<{ close: []; busy: [count: number] }>();

const state = useProject(props.opened, computed(() => props.active));
provide(PROJECT, state);
const {
  view, running, result, runs, runLabel, selectRun, selectedRun, activeRun, anyRunning, runningCount,
  history, historyError, historyDetail, historyLine, showHistory, newTask, taskName,
  taskMenu, renameId, renaming, deleteAsk, askDelete, startRename, saveRename, removeTask, taskError,
  rows, helpersPending, helpersReady, TONE, HISTORY_STATUS, OUTCOME,
  usageCounters, limitsLoading, limitsCheckedAt, loadLimits,
  git, gitOpen, gitAsk, gitBusy, gitLoading, gitRefreshError, gitError, gitNotice,
  commitMessage, mergeBranch, gitQuestion, gitDisabledReason, openGit, refreshGit, runGit, domId,
} = state;

// The launch screen lists every open project and says which are busy, so a run
// left going in another one is never invisible.
watch(runningCount, (count) => emit("busy", count), { immediate: true });

const gitTrigger = ref<HTMLButtonElement | null>(null);
const gitPanel = ref<HTMLElement | null>(null);
const gitPosition = ref({ top: "56px" });
const gitActions: Array<{ id: GitAction; label: string; busy: string }> = [
  { id: "commit", label: "Commit", busy: "Committing…" },
  { id: "pull", label: "Pull", busy: "Pulling…" },
  { id: "push", label: "Push", busy: "Pushing…" },
  { id: "merge", label: "Merge", busy: "Merging…" },
];
const gitActionLabel = (action: GitAction) => action === "push" && !git.value.upstream
  ? "Publish branch" : gitActions.find((a) => a.id === action)?.label ?? "Fetch";
const gitProgress = computed(() => gitBusy.value === "fetch" ? "Fetching…"
  : gitActions.find((a) => a.id === gitBusy.value)?.busy);

function positionGit() {
  if (gitTrigger.value) gitPosition.value = { top: `${gitTrigger.value.getBoundingClientRect().bottom + 8}px` };
}
function toggleGit(event: Event) {
  gitOpen.value = (event as ToggleEvent).newState === "open";
  if (gitOpen.value) {
    positionGit();
    void refreshGit();
  } else if (!gitBusy.value) gitAsk.value = null;
}
watch(gitOpen, (open) => open ? gitPanel.value?.showPopover() : gitPanel.value?.hidePopover());
watch(gitAsk, async (action, previous) => {
  const restoreFocus = previous && gitPanel.value?.contains(document.activeElement);
  await nextTick();
  if (!gitOpen.value) return;
  const target = action ? ".git-confirm input, .git-confirm select, .git-confirm button:not(:disabled)"
    : restoreFocus ? ".git-actions button:not(:disabled)" : null;
  if (target) (gitPanel.value?.querySelector<HTMLElement>(target) ?? gitPanel.value?.querySelector<HTMLElement>(".git-close"))?.focus();
});
onMounted(() => window.addEventListener("resize", positionGit));
onUnmounted(() => window.removeEventListener("resize", positionGit));

const dock = useDock(props.opened.project.path);
provide(DOCK, dock);
const dockPrefs = dock.prefs;
// Flex sizes along whichever axis the split runs, so one number serves both.
const dockSize = computed(() => ({ flex: `0 0 ${dockPrefs.size}%` }));

const split = ref<HTMLElement | null>(null);
const dragging = ref(false);

/** Drag the divider. The size is a percentage so it survives a window resize. */
function drag(e: PointerEvent) {
  const box = split.value?.getBoundingClientRect();
  if (!box) return;
  const share = dockPrefs.side === "right"
    ? ((box.right - e.clientX) / box.width) * 100
    : ((box.bottom - e.clientY) / box.height) * 100;
  dockPrefs.size = Math.min(85, Math.max(15, Math.round(share)));
}

function startDrag(e: PointerEvent) {
  dragging.value = true;
  (e.target as HTMLElement).setPointerCapture(e.pointerId);
}

function endDrag(e: PointerEvent) {
  dragging.value = false;
  (e.target as HTMLElement).releasePointerCapture(e.pointerId);
}

/** The divider is a real separator, so the keyboard moves it too. */
function nudge(by: number) {
  dockPrefs.size = Math.min(85, Math.max(15, dockPrefs.size + by));
}

// Ctrl+` shows and hides the dock; with Shift it opens another terminal. The
// physical key is what matters - Ctrl+Shift+` is a tilde on some layouts.
function shortcut(e: KeyboardEvent) {
  // One window: the project on screen owns the keyboard.
  if (!props.active || !e.ctrlKey || e.altKey || e.code !== "Backquote") return;
  e.preventDefault();
  if (e.shiftKey) dock.openTerminal();
  else if (dockPrefs.open) dockPrefs.open = false;
  else if (dock.tabs.value.length) dockPrefs.open = true;
  else dock.openTerminal();
}
onMounted(() => window.addEventListener("keydown", shortcut));
onUnmounted(() => window.removeEventListener("keydown", shortcut));

const vSelect = { mounted: (el: HTMLInputElement) => el.select() };

// A click anywhere else closes the row menu.
const closeMenu = () => (taskMenu.value = null);
onMounted(() => document.addEventListener("click", closeMenu));
onUnmounted(() => document.removeEventListener("click", closeMenu));

// The native dialog brings focus trapping, Escape and a backdrop for free.
const deleteDialog = ref<HTMLDialogElement | null>(null);
watch(deleteAsk, (t) => (t ? deleteDialog.value?.showModal() : deleteDialog.value?.close()));
</script>

<template>
  <div class="workspace">
    <aside class="sidebar" aria-label="Workspace">
      <div class="brand"><VeloMark :size="26" /> Orteca</div>

      <button class="btn new-task" :class="{ on: view === 'task' && !activeRun }" :aria-current="view === 'task' && !activeRun ? 'page' : undefined" @click="newTask">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
        New task
      </button>

      <nav class="nav" aria-label="Project">
        <button
          v-for="r in runs"
          :key="r.key"
          :class="{ on: view === 'task' && selectedRun === r.key }"
          :aria-current="view === 'task' && selectedRun === r.key ? 'page' : undefined"
          :title="r.prompt"
          @click="selectRun(r.key)"
        >
          <span class="dot" :class="r.active ? 'live' : r.result ? TONE[r.result.status] : r.error ? 'bad' : ''" aria-hidden="true"></span>
          <span class="task-copy">
            <span class="task-title">{{ runLabel(r) }}</span>
            <span class="task-caption">{{ r.active ? "Running now" : r.result ? OUTCOME[r.result.status] : "Couldn’t start" }} · {{ r.provider }}</span>
          </span>
        </button>
        <button
          :class="{ on: view === 'helpers' }"
          :aria-current="view === 'helpers' ? 'page' : undefined"
          @click="view = 'helpers'"
        >
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="3" y="4" width="10" height="9" rx="2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M8 1v3M1 7v3m14-3v3M6 8h.01M10 8h.01M6 10.5h4" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
          <span class="grow">AI helpers</span>
          <span class="count" :class="{ warn: !helpersPending && helpersReady < rows.length }" :aria-label="helpersPending ? 'Checking availability' : `${helpersReady} of ${rows.length} ready`">
            {{ helpersPending ? "…" : `${helpersReady}/${rows.length}` }}
          </span>
        </button>
      </nav>

      <h2 class="label side-label">Project</h2>
      <div class="project-root">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
        <span class="grow" :title="visiblePath(opened.project.path)">{{ opened.project.name }}</span>
        <span v-if="!historyError && history.length" class="count" :title="`${history.length} recent tasks`">{{ history.length }}</span>
      </div>
      <h3 class="hidden-label">Recent tasks</h3>
      <p v-if="historyError" class="note side-note">History unavailable.</p>
      <p v-else-if="!history.length" class="note side-note">Finished tasks show up here.</p>
      <ul v-else class="recent">
        <li v-for="t in history" :key="t.id">
          <input
            v-if="renameId === t.id"
            v-model="renaming"
            v-select
            class="rename-input"
            maxlength="60"
            aria-label="Task name"
            @keydown.enter="saveRename"
            @keydown.esc="renameId = null"
            @blur="saveRename"
          />
          <button
            v-else
            :class="{ on: view === 'history' && historyDetail?.id === t.id }"
            :aria-current="view === 'history' && historyDetail?.id === t.id ? 'page' : undefined"
            :title="`${t.title || t.prompt}\n${historyLine(t)}`"
            @click="showHistory(t)"
          >
            <span class="dot" :class="TONE[t.status]" aria-hidden="true"></span>
            <span class="task-copy">
              <span class="task-title">{{ taskName(t) }}</span>
              <span class="task-caption">{{ HISTORY_STATUS[t.status] ?? t.status }} · {{ t.provider }}</span>
            </span>
          </button>
          <button
            v-if="renameId !== t.id && t.status !== 'running'"
            class="row-menu"
            :class="{ open: taskMenu === t.id }"
            title="Task options"
            :aria-label="`Options for ${taskName(t)}`"
            aria-haspopup="menu"
            :aria-expanded="taskMenu === t.id"
            @click.stop="taskMenu = taskMenu === t.id ? null : t.id"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
              <path d="M3 4.5l3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
          <div v-if="taskMenu === t.id" class="menu" role="menu" @click.stop>
            <button role="menuitem" @click="startRename(t)">Rename</button>
            <button role="menuitem" class="danger" @click="askDelete(t)">Delete</button>
          </div>
        </li>
      </ul>
      <p v-if="taskError" class="note side-note" role="alert">{{ taskError }}</p>

      <dialog ref="deleteDialog" class="confirm" aria-labelledby="delete-title" @close="deleteAsk = null">
        <h2 id="delete-title">Delete this task?</h2>
        <p>{{ deleteAsk && taskName(deleteAsk) }}</p>
        <p class="note">Its log and numbers are removed for good. Your files are not touched.</p>
        <footer>
          <button class="btn" autofocus @click="deleteAsk = null">Cancel</button>
          <button class="btn danger" @click="removeTask">Delete</button>
        </footer>
      </dialog>

      <div class="side-project">
        <button class="switch-project" @click="$emit('close')">
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M13 5H3m3-3L3 5l3 3M3 11h10m-3-3 3 3-3 3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
          Switch project
        </button>
      </div>
    </aside>

    <div class="main">
      <header class="topbar">
        <h1 class="workspace-tab">
          <span v-if="view === 'task' && (running || result)" class="dot" :class="running ? 'live' : result && TONE[result.status]" aria-hidden="true"></span>
          <svg v-else viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M4 2h5l3 3v9H4V2Zm5 0v3h3M6 8h4M6 11h3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
          {{ view === 'helpers' ? 'AI helpers' : view === 'history' ? 'Task history' : running ? 'Running task' : result ? 'Task result' : 'New task' }}
        </h1>
        <button
          v-if="git.isRepo"
          ref="gitTrigger"
          class="git-trigger"
          :class="{ on: gitOpen }"
          :popovertarget="domId('project-git')"
          :title="`Git · ${git.branch ?? 'detached HEAD'} · ${git.dirtyCount} uncommitted changes`"
          :aria-label="`Git status and actions: ${git.branch ?? 'detached HEAD'}, ${git.dirtyCount} changed files`"
          aria-haspopup="dialog"
        >
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><circle cx="4" cy="3" r="1.5" fill="none" stroke="currentColor" /><circle cx="12" cy="4" r="1.5" fill="none" stroke="currentColor" /><circle cx="4" cy="13" r="1.5" fill="none" stroke="currentColor" /><path d="M4 4.5v7M12 5.5C12 9 4 7 4 11" fill="none" stroke="currentColor" stroke-width="1.3" /></svg>
          <span class="branch mono">{{ git.branch ?? "detached HEAD" }}</span>
          <span v-if="gitBusy" class="note">{{ gitProgress }}</span>
          <span v-else-if="gitError || gitRefreshError" class="git-error">Needs attention</span>
          <template v-else>
            <span v-if="git.dirty" class="git-dirty">{{ git.dirtyCount }} changed</span>
            <span v-if="git.behind" class="note">{{ git.behind }} to pull</span>
            <span v-if="git.ahead" class="note">{{ git.ahead }} to push</span>
          </template>
          <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="m3 4.5 3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </button>
        <button
          class="dock-toggle"
          :class="{ on: dockPrefs.open }"
          :title="dockPrefs.open ? 'Hide the dock (Ctrl+`)' : 'Show the dock (Ctrl+`)'"
          :aria-label="dockPrefs.open ? 'Hide the dock' : 'Show the dock'"
          :aria-pressed="dockPrefs.open"
          @click="dockPrefs.open ? (dockPrefs.open = false) : dock.tabs.value.length ? (dockPrefs.open = true) : dock.openTerminal()"
        >
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="2" y="2.5" width="12" height="11" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M10 2.5v11" stroke="currentColor" stroke-width="1.3" /></svg>
          Dock
        </button>
      </header>

      <section :id="domId('project-git')" ref="gitPanel" class="git-panel" :style="gitPosition" popover role="dialog" aria-labelledby="git-title" @toggle="toggleGit">
        <div class="git-heading">
          <h2 id="git-title">Git</h2>
          <button class="git-close" :popovertarget="domId('project-git')" popovertargetaction="hide" aria-label="Close Git actions" title="Close Git actions">
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
          </button>
        </div>
        <p class="git-current mono">{{ git.branch ?? "detached HEAD" }}</p>
        <p class="note" :class="{ 'git-dirty': git.dirty }">{{ git.dirty ? `${git.dirtyCount} changed file${git.dirtyCount === 1 ? '' : 's'}` : 'Working tree clean' }}</p>
        <div class="git-remote">
          <div>
            <span v-if="git.upstream" class="mono">{{ git.upstream }}</span>
            <span v-else class="note">No tracking branch</span>
            <p v-if="git.upstream" class="note">{{ git.behind ?? '—' }} incoming · {{ git.ahead ?? '—' }} outgoing · at last fetch</p>
          </div>
          <button class="btn" :disabled="!!gitDisabledReason('fetch')" :title="gitDisabledReason('fetch') || 'Check the remote for new commits; your files stay as they are'" @click="runGit('fetch')">{{ gitBusy === 'fetch' ? 'Fetching…' : 'Fetch' }}</button>
        </div>
        <div v-if="!gitAsk" class="git-actions" role="group" aria-label="Git actions">
          <div v-for="action in gitActions" :key="action.id" :title="gitDisabledReason(action.id)">
            <button class="btn" :disabled="!!gitDisabledReason(action.id)" @click="openGit(action.id)">{{ gitActionLabel(action.id) }}</button>
            <span v-if="gitDisabledReason(action.id) && !running && !gitBusy && !gitLoading" class="note">{{ gitDisabledReason(action.id) }}</span>
          </div>
        </div>
        <form v-else class="git-confirm" @submit.prevent="runGit(gitAsk!)">
          <h3>{{ gitActionLabel(gitAsk) }}</h3>
          <p id="git-scope" class="note">{{ gitQuestion(gitAsk) }}</p>
          <label v-if="gitAsk === 'commit'" for="commit-message" class="label">Commit message</label>
          <input
            v-if="gitAsk === 'commit'"
            id="commit-message"
            v-model="commitMessage"
            class="git-input"
            placeholder="Describe these changes"
            :disabled="!!gitBusy"
            aria-describedby="git-scope"
            required
          />
          <label v-if="gitAsk === 'merge'" for="merge-branch" class="label">Branch to merge</label>
          <select v-if="gitAsk === 'merge'" id="merge-branch" v-model="mergeBranch" class="git-input mono" :disabled="!!gitBusy" aria-describedby="git-scope" required>
            <option disabled value="">Choose a branch</option>
            <option v-for="b in git.branches" :key="b" :value="b">{{ b }}</option>
          </select>
          <p v-if="gitDisabledReason(gitAsk) && !gitBusy && !gitLoading" class="note">{{ gitDisabledReason(gitAsk) }}</p>
          <div class="git-buttons">
            <button
              type="submit"
              class="btn git-yes"
              :disabled="!!gitDisabledReason(gitAsk) || (gitAsk === 'commit' && !commitMessage.trim()) || (gitAsk === 'merge' && !git.branches.includes(mergeBranch))"
            >
              {{ gitBusy ? gitProgress : gitAsk === 'commit' ? 'Commit all changes' : gitActionLabel(gitAsk) }}
            </button>
            <button type="button" class="btn" :disabled="!!gitBusy" @click="openGit()">Back</button>
          </div>
        </form>
        <p v-if="running" class="note" role="status">Git actions are available when the task finishes.</p>
        <p v-if="gitError" class="git-error" role="alert">{{ gitError }}</p>
        <p v-if="gitRefreshError" class="git-error" role="alert">Could not refresh Git status: {{ gitRefreshError }}</p>
        <p v-if="gitBusy || gitNotice" class="note git-notice" role="status">{{ gitProgress || gitNotice }}</p>
        <footer class="git-footer">
          <span class="note">{{ gitLoading ? 'Refreshing status…' : 'Local repository status' }}</span>
          <button class="link" :disabled="anyRunning || !!gitBusy || gitLoading" @click="refreshGit">Refresh</button>
        </footer>
      </section>

      <div ref="split" class="split" :class="[dockPrefs.side, { dragging }]">
        <main class="content" :class="{ composing: view === 'task' && !running && !result }">
          <template v-if="view === 'task'">
            <TaskRun v-if="running" />
            <TaskResult v-else-if="result" />
            <TaskComposer v-else />
          </template>
          <PastTask v-else-if="view === 'history'" />
          <AiHelpers v-else />
        </main>
        <template v-if="dockPrefs.open">
          <div
            class="divider"
            role="separator"
            tabindex="0"
            :aria-orientation="dockPrefs.side === 'right' ? 'vertical' : 'horizontal'"
            aria-label="Resize the dock"
            :aria-valuenow="dockPrefs.size"
            aria-valuemin="15"
            aria-valuemax="85"
            @pointerdown="startDrag"
            @pointermove="dragging && drag($event)"
            @pointerup="endDrag"
            @keydown.left.prevent="nudge(dockPrefs.side === 'right' ? 2 : 0)"
            @keydown.right.prevent="nudge(dockPrefs.side === 'right' ? -2 : 0)"
            @keydown.up.prevent="nudge(dockPrefs.side === 'bottom' ? 2 : 0)"
            @keydown.down.prevent="nudge(dockPrefs.side === 'bottom' ? -2 : 0)"
          ></div>
          <Dock :style="dockSize" />
        </template>
      </div>
    </div>

    <footer class="statusbar" aria-label="Workspace status">
      <span class="mono grow" :title="visiblePath(opened.project.path)">{{ visiblePath(opened.project.path) }}</span>
      <span v-if="anyRunning" class="status-item"><span class="dot live" aria-hidden="true"></span>{{ runningCount === 1 ? "Task running" : `${runningCount} tasks running` }}</span>
      <div class="usage-counters" role="group" aria-label="Plan usage remaining">
        <template v-for="usage in usageCounters" :key="usage.id">
          <button class="usage-counter" :popovertarget="domId(`usage-${usage.id}`)" :title="`${usage.name} usage details and reset times`">
            <span class="usage-name">{{ usage.name }}</span>
            <span v-if="usage.status" class="usage-unavailable">— {{ usage.status }}</span>
            <template v-else>
              <span v-for="(w, i) in usage.windows.slice(0, 2)" :key="i" class="usage-window" :class="{ low: w.left !== null && w.left <= 20 }" :title="w.left === null ? `${w.label}: usage unavailable` : w.label">
                <span class="window-label">{{ w.shortLabel }}</span>
                <strong>{{ w.leftLabel }}</strong>
                <span v-if="w.left === null" class="hidden-label">usage unavailable</span>
                <span v-if="w.left !== null" class="usage-mini" aria-hidden="true"><span :style="{ width: `${w.left}%` }"></span></span>
              </span>
              <span>left</span>
              <span v-if="usage.windows.length > 2">+{{ usage.windows.length - 2 }} windows</span>
            </template>
          </button>

          <section :id="domId(`usage-${usage.id}`)" class="usage-popover" popover role="dialog" :aria-labelledby="`usage-title-${usage.id}`">
            <header class="usage-heading">
              <h2 :id="`usage-title-${usage.id}`">{{ usage.name }} usage</h2>
              <button class="usage-close" :popovertarget="domId(`usage-${usage.id}`)" popovertargetaction="hide" :title="`Close ${usage.name} usage`" :aria-label="`Close ${usage.name} usage`">
                <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
              </button>
            </header>
            <p class="note usage-description">Remaining allowance in each reported plan window.</p>
            <template v-if="usage.status">
              <p class="usage-empty" role="status">{{ usage.status }}</p>
              <p v-if="usage.status === 'Unavailable'" class="note usage-reason">{{ usage.reason }}</p>
              <button v-if="usage.status === 'Not installed' || usage.status === 'Sign in required'" class="btn" :popovertarget="domId(`usage-${usage.id}`)" popovertargetaction="hide" @click="view = 'helpers'">Open AI helpers</button>
            </template>
            <div v-for="(w, i) in usage.windows" :key="i" class="usage-detail" :class="{ low: w.left !== null && w.left <= 20 }">
              <div class="usage-detail-label"><span>{{ w.label }}</span><strong>{{ w.leftLabel }} {{ w.left === null ? 'unavailable' : 'left' }}</strong></div>
              <meter v-if="w.left !== null" :value="w.left" min="0" max="100" :aria-label="`${usage.name} ${w.label} allowance remaining`">{{ w.leftLabel }} left</meter>
              <p class="note">{{ w.resets ? `Resets ${w.resets}` : 'Reset time unavailable' }}</p>
            </div>
            <footer class="usage-footer">
              <span class="note" role="status">{{ limitsLoading ? 'Refreshing…' : limitsCheckedAt ? `Checked ${new Date(limitsCheckedAt).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}` : 'Not checked yet' }}</span>
              <button class="link" :disabled="limitsLoading" @click="loadLimits">Refresh usage</button>
            </footer>
            <p class="note usage-schedule">Updates when you open a project and after each task.</p>
          </section>
        </template>
        <button class="usage-refresh" :disabled="limitsLoading" :title="limitsLoading ? 'Refreshing usage…' : 'Refresh plan usage'" :aria-label="limitsLoading ? 'Refreshing plan usage' : 'Refresh plan usage'" @click="loadLimits">
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M13 6a5 5 0 0 0-8.5-2L2 6m0-4v4h4M3 10a5 5 0 0 0 8.5 2L14 10m0 4v-4h-4" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </button>
      </div>
    </footer>
  </div>
</template>

<style scoped>
.workspace {
  display: grid;
  grid-template-columns: 240px minmax(0, 1fr);
  grid-template-rows: minmax(0, 1fr) auto;
  height: 100%;
}

.sidebar {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-height: 0;
  padding: 0 8px 8px;
  background: var(--surface);
  border-right: 1px solid var(--border);
}
.brand {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  min-height: 44px;
  margin: 0 -8px 12px;
  padding: 0 14px;
  border-bottom: 1px solid var(--border);
  font-size: 14px;
  font-weight: 600;
}
.new-task {
  display: flex;
  align-items: center;
  gap: 8px;
  justify-content: flex-start;
  margin-bottom: 4px;
  border-color: transparent;
  font-size: 12px;
}
.new-task.on {
  background: var(--surface-2);
  border-color: transparent;
}

.nav,
.recent {
  display: grid;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.nav button,
.recent button {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  min-height: 32px;
  padding: 6px 10px;
  border-radius: var(--r-sm);
  color: var(--text-dim);
  text-align: left;
  font-size: 12px;
  transition: background 120ms ease, color 120ms ease;
}
.nav button:hover,
.recent button:hover,
.nav button.on,
.recent button.on {
  background: var(--surface-2);
  color: var(--text);
}
.task-copy {
  display: grid;
  gap: 2px;
  flex: 1;
  min-width: 0;
}
.task-title,
.task-caption {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.task-caption {
  color: var(--text-faint);
  font-size: 11px;
}
.count {
  font-size: 12px;
  color: var(--text-faint);
}
.count.warn {
  color: var(--warn);
}

.side-label {
  margin: 24px 10px 8px;
  color: var(--text-faint);
}
.project-root {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  font-size: 12px;
  font-weight: 500;
}
.project-root svg,
.nav svg {
  flex-shrink: 0;
  color: var(--text-faint);
}
.side-note {
  margin: 6px 10px 6px 32px;
}
.recent {
  flex: 1;
  grid-template-columns: minmax(0, 1fr);
  overflow-x: hidden;
  align-content: start;
  min-height: 0;
  overflow-y: auto;
}
.recent li {
  position: relative;
}
.recent > li > button:first-child {
  align-items: flex-start;
  padding: 9px 10px 9px 20px;
}
.recent > li > button:first-child > .dot {
  margin-top: 6px;
  width: 6px;
  height: 6px;
}
.recent li:has(.row-menu) > button:first-child {
  padding-right: 30px;
}
.recent .row-menu {
  position: absolute;
  top: 50%;
  right: 4px;
  width: 24px;
  min-height: 24px;
  padding: 4px;
  transform: translateY(-50%);
  color: var(--text-faint);
  opacity: 0;
}
.recent li:hover .row-menu,
.recent .row-menu:focus-visible,
.recent .row-menu.open {
  opacity: 1;
}
.recent .row-menu:hover,
.recent .row-menu.open {
  background: var(--surface);
  color: var(--text);
}
.menu {
  position: absolute;
  top: calc(100% - 2px);
  right: 4px;
  z-index: 1;
  display: grid;
  min-width: 120px;
  padding: 4px;
  background: var(--surface-2);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
}
.recent .menu button {
  padding: 6px 10px;
}
.recent .menu button:hover {
  background: var(--surface);
}
.recent .menu .danger,
.recent .menu .danger:hover {
  color: var(--err);
}
.rename-input {
  width: 100%;
  padding: 6px 10px;
  background: var(--bg);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  font: inherit;
}
.confirm {
  width: min(420px, calc(100vw - 32px));
  padding: 24px;
  background: var(--surface);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
}
.confirm::backdrop {
  background: var(--overlay);
}
.confirm h2 {
  margin: 0 0 8px;
  font-size: 20px;
  font-weight: 600;
}
.confirm p {
  margin: 0 0 12px;
  overflow-wrap: anywhere;
}
.confirm footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--gap);
  margin-top: 20px;
}
.confirm .danger {
  color: var(--err);
}
.confirm .danger:hover {
  border-color: var(--err);
}
.side-project {
  margin-top: auto;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}
.switch-project {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  min-height: 32px;
  padding: 6px 10px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  font-size: 12px;
  text-align: left;
  transition: background 120ms ease, color 120ms ease;
}
.switch-project:hover {
  color: var(--text);
  background: var(--surface-2);
}
.recent + .side-project {
  margin-top: 8px;
}

.main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
.topbar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 0 12px;
  flex-shrink: 0;
  min-height: 44px;
  padding: 0 12px 0 0;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
}
h1 {
  margin: 0;
  font-size: 12px;
  font-weight: 500;
}
.workspace-tab {
  display: flex;
  align-items: center;
  align-self: stretch;
  gap: 8px;
  min-width: 160px;
  min-height: 44px;
  padding: 0 16px;
  background: var(--bg);
  border-right: 1px solid var(--border);
}
.workspace-tab svg {
  color: var(--text-faint);
}
.git-trigger {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  max-width: 100%;
  min-height: 32px;
  margin-left: auto;
  padding: 4px 8px;
  border-radius: var(--r-sm);
  font-size: 12px;
}
.git-trigger svg {
  flex-shrink: 0;
}
.branch {
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
}
.git-trigger > span {
  white-space: nowrap;
}
.git-trigger:hover,
.git-trigger.on,
.git-close:hover {
  background: var(--surface-2);
}
.git-panel {
  inset: 56px 12px auto auto;
  width: min(380px, calc(100vw - 24px));
  max-height: calc(100dvh - 72px);
  max-height: calc(100dvh - v-bind('gitPosition.top') - 16px);
  margin: 0;
  overflow-y: auto;
  padding: 16px;
  background: var(--surface);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
  font-size: 12px;
}
.git-heading,
.git-remote,
.git-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.git-heading h2,
.git-confirm h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}
.git-close {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 32px;
  min-height: 32px;
  border-radius: var(--r-sm);
}
.git-panel p {
  margin: 6px 0 0;
  overflow-wrap: anywhere;
}
.git-panel .git-current {
  color: var(--text);
}
.git-dirty {
  color: var(--warn);
}
.git-remote {
  margin: 16px 0;
  padding-bottom: 16px;
  border-bottom: 1px solid var(--border);
}
.git-remote > div {
  min-width: 0;
  overflow-wrap: anywhere;
}
.git-remote .btn {
  flex-shrink: 0;
}
.git-actions {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
}
.git-actions .btn {
  width: 100%;
}
.git-actions .note {
  display: block;
  margin-top: 4px;
}
.git-buttons {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 12px;
}
.git-panel .btn {
  font-size: 12px;
}
.git-confirm .label {
  display: block;
  margin: 12px 0 6px;
}
.git-yes {
  color: var(--warn);
}
.git-input {
  width: 100%;
  min-width: 0;
  padding: 6px 10px;
  background: var(--bg);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  font: inherit;
}
.git-input::placeholder {
  color: var(--text-faint);
}
.git-error {
  font-size: 12px;
  color: var(--err);
  white-space: pre-wrap;
}
.git-panel .git-error,
.git-panel .git-notice {
  margin-top: 16px;
}
.git-footer {
  margin-top: 16px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}

.split {
  display: flex;
  flex: 1;
  min-width: 0;
  min-height: 0;
}
.split.right {
  flex-direction: row;
}
.split.bottom {
  flex-direction: column;
}
/* While the divider is being dragged the iframe and the terminal must not
   swallow the pointer, or the drag stops the moment it crosses one. */
.split.dragging :deep(iframe),
.split.dragging :deep(.xterm) {
  pointer-events: none;
}
.divider {
  flex: 0 0 5px;
  background: var(--border);
  transition: background 120ms ease;
}
.divider:hover,
.divider:focus-visible {
  background: var(--border-strong);
}
.split.right > .divider {
  cursor: col-resize;
}
.split.bottom > .divider {
  cursor: row-resize;
}
.dock-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  border-radius: var(--r-sm);
  color: var(--text-dim);
  font-size: 12px;
  transition: background 120ms ease, color 120ms ease;
}
.dock-toggle:hover,
.dock-toggle.on {
  background: var(--surface-2);
  color: var(--text);
}
.content {
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow-y: auto;
  padding: 28px 32px 48px;
}
.content.composing {
  display: flex;
}
.statusbar {
  grid-column: 1 / -1;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 16px;
  min-width: 0;
  min-height: 28px;
  padding: 4px 12px;
  border-top: 1px solid var(--border);
  background: var(--surface);
  color: var(--text-faint);
  font-size: 11px;
}
.statusbar .mono {
  font-size: 11px;
}
.status-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  white-space: nowrap;
}
.usage-counters {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px;
  min-width: 0;
}
.usage-counter,
.usage-refresh,
.usage-close {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  min-height: 32px;
  padding: 4px 8px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  transition: background 120ms ease, color 120ms ease;
}
.usage-counter:hover,
.usage-close:hover,
.usage-refresh:hover:not(:disabled) {
  background: var(--surface-2);
  color: var(--text);
}
.usage-refresh:disabled {
  opacity: 0.5;
  cursor: default;
}
.usage-name,
.usage-window strong {
  color: var(--text);
  font-weight: 500;
}
.usage-counter:not(:first-child) {
  border-left: 1px solid var(--border);
}
.usage-unavailable {
  font-size: 11px;
}
.usage-window {
  display: grid;
  grid-template-columns: auto auto;
  gap: 2px 4px;
  color: var(--text-dim);
}
.window-label {
  max-width: 64px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.usage-mini {
  grid-column: 1 / -1;
  height: 2px;
  background: var(--border-strong);
}
.usage-mini > span {
  display: block;
  height: 100%;
  background: currentColor;
}
.usage-popover {
  inset: auto 12px 52px auto;
  width: min(360px, calc(100vw - 24px));
  max-height: calc(100dvh - 100px);
  margin: 0;
  overflow-y: auto;
  padding: 18px;
  background: var(--surface);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
  color: var(--text);
  font-size: 12px;
}
.usage-heading,
.usage-detail-label,
.usage-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.usage-heading h2 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}
.usage-close {
  padding: 4px;
  min-width: 32px;
}
.usage-description {
  margin: 4px 0 20px;
}
.usage-empty {
  margin: 0 0 8px;
}
.usage-reason {
  overflow-wrap: anywhere;
}
.usage-detail + .usage-detail {
  margin-top: 18px;
}
.usage-detail-label {
  margin-bottom: 6px;
  color: var(--text-dim);
}
.usage-detail-label strong {
  flex-shrink: 0;
  color: var(--text);
  font-weight: 500;
}
.usage-detail meter {
  display: block;
  width: 100%;
  height: 6px;
  appearance: none;
  background: var(--surface-2);
  border: none;
  border-radius: 999px;
  overflow: hidden;
}
.usage-detail meter::-webkit-meter-bar {
  height: 6px;
  background: var(--surface-2);
  border: none;
}
.usage-detail meter::-webkit-meter-optimum-value {
  background: var(--text-dim);
}
.usage-detail meter::-moz-meter-bar {
  background: var(--text-dim);
}
.usage-detail.low meter::-webkit-meter-optimum-value {
  background: var(--warn);
}
.usage-detail.low meter::-moz-meter-bar {
  background: var(--warn);
}
.usage-detail p {
  margin: 6px 0 0;
}
.usage-footer {
  margin-top: 20px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}
.usage-schedule {
  margin: 10px 0 0;
}
.low,
.low strong {
  color: var(--warn);
}

@media (min-width: 701px) and (max-width: 1000px) {
  .workspace {
    grid-template-columns: 208px minmax(0, 1fr);
  }
}

@media (max-width: 700px) {
  .workspace {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto minmax(0, 1fr) auto;
  }
  .sidebar {
    max-height: 40dvh;
    overflow-y: auto;
    padding: 0 8px 8px;
    border-right: none;
    border-bottom: 1px solid var(--border);
  }
  .recent {
    flex: 0 0 auto;
    max-height: 120px;
  }
  .brand {
    margin-bottom: 8px;
  }
  .new-task {
    margin-bottom: 4px;
  }
  .side-label {
    margin-top: 16px;
  }
  .topbar {
    gap: 0 8px;
  }
  .workspace-tab {
    min-width: 120px;
  }
  .statusbar {
    flex-wrap: wrap;
    gap: 4px 12px;
  }
  .statusbar > .grow {
    flex-basis: 100%;
  }
  .content {
    padding: 24px 20px 48px;
  }
}
</style>
