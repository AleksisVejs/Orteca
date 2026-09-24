<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, provide, ref, watch } from "vue";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import ProviderMark from "../components/ProviderMark.vue";
import VeloMark from "../components/VeloMark.vue";
import Agents from "./project/Agents.vue";
import Stats from "./project/Stats.vue";
import Settings from "./project/Settings.vue";
import Memory from "./project/Memory.vue";
import Dock from "./project/Dock.vue";
import CurrentTask from "./project/CurrentTask.vue";
import PastTask from "./project/PastTask.vue";
import TaskComposer from "./project/TaskComposer.vue";
import { DOCK, useDock } from "./project/dock";
import { PROJECT, useProject } from "./project/state";
import { globalTasks, workingPatch } from "../api";
import type { GitAction, GlobalTaskSummary, OpenedProject, TaskType, Tier } from "../types";
import { nextPing } from "./project/anchor";
import { parseHistoryPatch } from "./project/historyPatch";

// The shell: sidebar, top bar and whichever page the sidebar picked.
// All state lives in `useProject`; the pages inject it.

/** Every project currently open, with how many runs each has going right now. */
type OpenProject = { path: string; name: string; running: number };
const props = defineProps<{ opened: OpenedProject; active: boolean; openProjects?: OpenProject[]; taskFocus?: number | null; newTaskFocus?: boolean; closedTaskProjects?: Set<string>; expandedTaskProjects?: Set<string>; sidebarSearch?: string; sidebarStatus?: string }>();
const emit = defineEmits<{ close: []; switch: [path: string]; open: [path: string]; task: [path: string, id: number]; newTask: [path: string]; focused: []; newTaskFocused: []; toggleProject: [path: string]; toggleExpandedProject: [path: string]; updateSearch: [value: string]; updateStatus: [value: string]; busy: [count: number] }>();

// The sidebar is a task inbox. Its contents do not change when the active
// workspace changes, so work from another project remains immediately visible.
const allTasks = ref<GlobalTaskSummary[]>([]);
const globalHistoryError = ref(false);
async function loadGlobalTasks() {
  try {
    allTasks.value = await globalTasks();
    globalHistoryError.value = false;
  } catch {
    globalHistoryError.value = true;
  }
}

const state = useProject(props.opened, computed(() => props.active));
provide(PROJECT, state);
const {
  view, running, result, runLabel, activeRun, anyRunning, runningCount,
  history, historyDetail, historyLine, showHistory, newTask, taskName, buildOn,
  taskMenu, renameId, renaming, deleteAsk, askDelete, startRename, saveRename, removeTask, taskError,
  rows, agentsPending, agentsReady, TONE, HISTORY_STATUS, OUTCOME,
  usageCounters, limitsLoading, limitsCheckedAt, loadLimits,
  git, gitOpen, gitAsk, gitBusy, gitLoading, gitRefreshError, gitError, gitNotice,
  commitMessage, drafting, draftCommit, targetBranch, newBranch, gitQuestion, gitDisabledReason, openGit, refreshGit, runGit, domId,
  formatDuration, anchor, limits, formatWhen,
} = state;

// The finished task on screen, as it actually ran: type, agent, model, tier, effort.
const TYPE: Record<TaskType, string> = { chat: "Chat", question: "Question", code_change: "Code change", debug: "Debug", plan: "Plan" };
const TIER: Record<Tier, string> = { cheapest: "cheap tier", standard: "mid tier", deep: "top tier" };
const taskBadge = computed(() => {
  const r = result.value;
  if (!r || !activeRun.value) return null;
  const main = r.stages.find((s) => s.stage === "implement" || s.stage === "answer") ?? r.stages.find((s) => s.model);
  return [
    r.route.taskType && TYPE[r.route.taskType],
    activeRun.value.provider === "codex" ? "Codex" : "Claude",
    r.usage?.model ?? main?.model,
    TIER[r.route.budget.preferredTier],
    main?.effort && `${main.effort} effort`,
  ].filter(Boolean).join(" · ");
});
// The next window-anchor ping, when anchoring is on and one is scheduled.
const nextAnchor = computed(() => {
  const a = anchor.value;
  if (a.mode === "off") return null;
  const times = a.providers.map((id) => nextPing(a, Date.now(), limits.value.find((l) => l.id === id))).filter((t): t is number => t !== null);
  return times.length ? formatWhen(Math.min(...times)) : null;
});

// The pane heading names the task on screen; its dot carries the outcome, the tooltip the rest.
const heading = computed(() => {
  const d = historyDetail.value;
  if (view.value === "history" && d) {
    const row = history.value.find((t) => t.id === d.id);
    const answered = (d.route as { kind?: string } | null)?.kind === "answer" && d.status === "done";
    return {
      name: row ? taskName(row) : d.prompt.split("\n")[0],
      tone: TONE[d.status] ?? "",
      status: answered ? "Answered" : HISTORY_STATUS[d.status] ?? d.status,
      meta: `${formatDuration(d.durationMs)} · ${d.startedAt.slice(0, 16)} UTC`,
    };
  }
  const r = activeRun.value;
  if (view.value !== "task" || !r || !(r.active || r.result)) return null;
  const id = r.result?.taskId ?? r.id;
  const row = history.value.find((t) => t.id === id);
  const who = r.provider === "codex" ? "Codex" : "Claude";
  if (r.active || !r.result) return { name: row ? taskName(row) : runLabel(r), tone: "live", status: "Running", meta: who };
  const answered = r.result.route.kind === "answer" && r.result.status === "done";
  return {
    name: row ? taskName(row) : runLabel(r),
    tone: TONE[r.result.status] ?? "",
    status: answered ? "Answered" : OUTCOME[r.result.status],
    meta: `${who} · ${formatDuration(r.result.durationMs)}`,
  };
});

onMounted(loadGlobalTasks);
watch(history, () => void loadGlobalTasks());
watch([() => props.active, () => props.taskFocus, history], () => {
  if (!props.active || props.taskFocus === null || props.taskFocus === undefined) return;
  const task = history.value.find((row) => row.id === props.taskFocus);
  if (task) {
    void showHistory(task);
    emit("focused");
  }
}, { immediate: true });
watch([() => props.active, () => props.newTaskFocus], () => {
  if (!props.active || !props.newTaskFocus) return;
  newTask();
  emit("newTaskFocused");
}, { immediate: true });

const historySearch = computed({ get: () => props.sidebarSearch ?? "", set: (value: string) => emit("updateSearch", value) });
const historyStatus = computed({ get: () => props.sidebarStatus ?? "all", set: (value: string) => emit("updateStatus", value) });
const filteredHistory = computed(() => allTasks.value.filter((task) => {
  const query = historySearch.value.trim().toLocaleLowerCase();
  return (!query || [task.title, task.prompt, task.summary, task.provider, task.projectName].some((text) => text?.toLocaleLowerCase().includes(query)))
    && (historyStatus.value === "all" || (historyStatus.value === "attention" ? ["failed", "verifyFailed", "reviewRejected", "budgetReached"].includes(task.status) : task.status === historyStatus.value));
}));
const expandedProjects = computed(() => props.expandedTaskProjects ?? new Set<string>());
const closedProjects = computed(() => props.closedTaskProjects ?? new Set<string>());
const taskGroups = computed(() => {
  const groups = new Map<string, { path: string; name: string; tasks: GlobalTaskSummary[] }>();
  for (const task of filteredHistory.value) {
    const group = groups.get(task.projectPath) ?? { path: task.projectPath, name: task.projectName, tasks: [] };
    group.tasks.push(task);
    groups.set(task.projectPath, group);
  }
  return [...groups.values()].sort((a, b) => Number(b.path === props.opened.project.path) - Number(a.path === props.opened.project.path));
});
const visibleTasks = (group: { path: string; tasks: GlobalTaskSummary[] }) =>
  expandedProjects.value.has(group.path) ? group.tasks : group.tasks.slice(0, 5);
function toggleExpandedProject(path: string) {
  emit("toggleExpandedProject", path);
}
function toggleProjectSection(path: string) {
  emit("toggleProject", path);
}

/** The row for the task on screen, whether it opened from history or is still live. */
const isOpen = (t: GlobalTaskSummary) => t.projectPath === props.opened.project.path && (
  view.value === "history" ? historyDetail.value?.id === t.id
    : view.value === "task" && !!activeRun.value && (activeRun.value.result?.taskId ?? activeRun.value.id) === t.id);

function openTask(task: GlobalTaskSummary) {
  if (task.projectPath === props.opened.project.path) {
    void showHistory(task);
  } else {
    emit("task", task.projectPath, task.id);
  }
}

// The launch screen lists every open project and says which are busy, so a run
// left going in another one is never invisible.
watch(runningCount, (count) => emit("busy", count), { immediate: true });
// A run that ends while Orteca is in the background flashes the taskbar until it is looked at.
watch(runningCount, (count, before) => {
  if (count < before && !document.hasFocus()) void getCurrentWindow().requestUserAttention(UserAttentionType.Informational);
});
// A run waiting on the user's OK goes nowhere until they give it, so it asks
// loudly wherever they are: another app, another project, another task.
watch(() => state.runs.value.filter((r) => r.waitAsk).length, (asking, before) => {
  if (asking > before && (!document.hasFocus() || !props.active || !activeRun.value?.waitAsk)) {
    void getCurrentWindow().requestUserAttention(UserAttentionType.Critical);
  }
});
// A new task starting: the box it was typed in glides down to where it steers,
// rather than the page swapping under it. Runs before the swap, while that box is there.
watch(() => view.value === "task" && !!(running.value || result.value), (chat, was) => {
  const from = document.getElementById(domId("task"))?.closest(".ask")?.getBoundingClientRect();
  if (!chat || was || !from || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  void nextTick(() => {
    const box = document.getElementById(domId("task-composer"))?.querySelector<HTMLElement>(".composer");
    const to = box?.getBoundingClientRect();
    if (!box || !to?.width) return;
    box.animate(
      [
        { transform: `translate(${from.left - to.left}px, ${from.top - to.top}px)`, width: `${from.width}px`, height: `${from.height}px`, overflow: "hidden" },
        { transform: "none", width: `${to.width}px`, height: `${to.height}px`, overflow: "hidden" },
      ],
      { duration: 360, easing: "cubic-bezier(0.2, 0, 0, 1)" },
    );
  });
});

const gitTrigger = ref<HTMLButtonElement | null>(null);
const gitPanel = ref<HTMLElement | null>(null);
const gitPosition = ref({ top: "56px" });
const selectedGitPath = ref<string | null>(null);
const gitPatch = ref("");
const gitPatchError = ref<string | null>(null);
const selectedGitPatch = computed(() => parseHistoryPatch(gitPatch.value).files.find((file) => file.path === selectedGitPath.value));
watch(() => git.value.changes, (changes) => {
  if (!changes.some((change) => change.path === selectedGitPath.value)) selectedGitPath.value = changes[0]?.path ?? null;
}, { immediate: true });
const GIT_LABELS: Record<GitAction, [label: string, busy: string]> = {
  fetch: ["Fetch", "Fetching…"],
  pull: ["Pull", "Pulling…"],
  push: ["Push", "Pushing…"],
  commit: ["Commit", "Committing…"],
  merge: ["Merge a branch", "Merging…"],
  discard: ["Revert", "Reverting…"],
  switch: ["Switch branch", "Switching…"],
  branch: ["New branch", "Creating branch…"],
};
const gitStatusLabel = (status: string) => {
  if (status === "??") return "Untracked";
  if (status.includes("A")) return "Added";
  if (status.includes("D")) return "Deleted";
  if (status.includes("R")) return "Renamed";
  if (status.includes("C")) return "Copied";
  return status[0] !== " " ? "Staged" : "Modified";
};
const splitPath = (path: string) => {
  const cut = path.lastIndexOf("/") + 1;
  return { dir: path.slice(0, cut), name: path.slice(cut) };
};
const gitActionLabel = (action: GitAction) => action === "push" && !git.value.upstream ? "Publish" : GIT_LABELS[action][0];
const gitProgress = computed(() => gitBusy.value ? GIT_LABELS[gitBusy.value][1] : "");
const gitSubmitLabel = (action: GitAction) => ({
  commit: `Commit ${git.value.dirtyCount} file${git.value.dirtyCount === 1 ? "" : "s"}`,
  discard: "Revert changes",
  merge: `Merge ${targetBranch.value || "branch"}`,
  switch: `Switch to ${targetBranch.value || "branch"}`,
  branch: "Create and switch",
} as Partial<Record<GitAction, string>>)[action] ?? gitActionLabel(action);

function positionGit() {
  if (gitTrigger.value) gitPosition.value = { top: `${gitTrigger.value.getBoundingClientRect().bottom + 8}px` };
}
function toggleGit(event: Event) {
  gitOpen.value = (event as ToggleEvent).newState === "open";
  if (gitOpen.value) {
    positionGit();
    void refreshGit();
    void loadGitPatch();
  } else if (!gitBusy.value) gitAsk.value = null;
}
function closeGit() {
  gitPanel.value?.hidePopover();
  gitOpen.value = false;
}
async function loadGitPatch() {
  try {
    gitPatch.value = await workingPatch(props.opened.project.path);
    gitPatchError.value = null;
  } catch (error) {
    gitPatchError.value = error instanceof Error ? error.message : String(error);
  }
}
watch(gitOpen, (open) => open ? gitPanel.value?.showPopover() : gitPanel.value?.hidePopover());
watch(gitAsk, async (action, previous) => {
  const restoreFocus = previous && gitPanel.value?.contains(document.activeElement);
  await nextTick();
  if (!gitOpen.value) return;
  const target = action ? ".git-confirm input, .git-confirm select, .git-confirm button:not(:disabled)"
    : restoreFocus ? ".git-actions button:not(:disabled)" : null;
  if (target) (gitPanel.value?.querySelector<HTMLElement>(target) ?? gitPanel.value?.querySelector<HTMLElement>(".git-head .git-icon"))?.focus();
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
          :class="{ on: view === 'agents' }"
          :aria-current="view === 'agents' ? 'page' : undefined"
          @click="view = 'agents'"
        >
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="3" y="4" width="10" height="9" rx="2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M8 1v3M1 7v3m14-3v3M6 8h.01M10 8h.01M6 10.5h4" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
          <span class="grow">Agents</span>
          <span class="count" :class="{ warn: !agentsPending && agentsReady < rows.length }" :aria-label="agentsPending ? 'Checking availability' : `${agentsReady} of ${rows.length} ready`">
            {{ agentsPending ? "…" : `${agentsReady}/${rows.length}` }}
          </span>
        </button>
        <button :class="{ on: view === 'stats' }" :aria-current="view === 'stats' ? 'page' : undefined" @click="view = 'stats'">
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M2.5 13.5h11M4.5 11V8m3.5 3V4.5m3.5 6.5V6.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
          <span class="grow">Stats</span>
        </button>
        <button :class="{ on: view === 'memory' }" :aria-current="view === 'memory' ? 'page' : undefined" @click="view = 'memory'">
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M4 2.5h8v11L8 10.8 4 13.5Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
          <span class="grow">Memory</span>
        </button>
        <button :class="{ on: view === 'settings' }" :aria-current="view === 'settings' ? 'page' : undefined" @click="view = 'settings'">
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><circle cx="8" cy="8" r="2.2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M8 1.6v1.6M8 12.8v1.6M14.4 8h-1.6M3.2 8H1.6m10.9-4.5-1.1 1.1M4.6 11.4l-1.1 1.1m9 0-1.1-1.1M4.6 4.6 3.5 3.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
          <span class="grow">Settings</span>
        </button>
      </nav>

      <div class="task-inbox">
        <input v-model="historySearch" type="search" aria-label="Search tasks across projects" placeholder="Search all tasks…" />
        <select v-model="historyStatus" aria-label="Filter tasks by status"><option value="all">All</option><option value="done">Done</option><option value="attention">Needs attention</option><option value="cancelled">Stopped</option><option value="running">Running</option></select>
      </div>
      <p v-if="globalHistoryError" class="note side-note">Tasks are unavailable.</p>
      <p v-else-if="!allTasks.length" class="note side-note">Tasks from every project appear here.</p>
      <p v-else-if="!filteredHistory.length" class="note side-note">No matching tasks.</p>
      <div v-else class="project-groups">
        <section v-for="group in taskGroups" :key="group.path" class="project-group">
          <h2 class="project-heading">
            <button class="project-toggle" :aria-expanded="!closedProjects.has(group.path)" :aria-label="`${closedProjects.has(group.path) ? 'Open' : 'Close'} ${group.name} tasks`" @click="toggleProjectSection(group.path)">
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
              <span>{{ group.name }}</span>
              <svg class="project-chevron" :class="{ closed: closedProjects.has(group.path) }" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="m3 4.5 3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
            </button>
            <button class="project-new" :title="`New task in ${group.name}`" :aria-label="`New task in ${group.name}`" @click="emit('newTask', group.path)">
              <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
            </button>
          </h2>
          <ul v-if="!closedProjects.has(group.path)" class="recent">
            <li v-for="t in visibleTasks(group)" :key="t.id">
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
            :class="{ on: isOpen(t) }"
            :aria-current="isOpen(t) ? 'page' : undefined"
            :title="`${t.projectName}\n${t.title || t.prompt}\n${historyLine(t)}`"
            @click="openTask(t)"
          >
            <span class="dot" :class="TONE[t.status]" aria-hidden="true"></span>
            <span class="task-copy">
              <span class="task-title">{{ taskName(t) }}</span>
              <span class="task-caption">{{ t.projectName }} · {{ HISTORY_STATUS[t.status] ?? t.status }}</span>
            </span>
          </button>
          <button
            v-if="t.projectPath === opened.project.path && renameId !== t.id && t.status !== 'running'"
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
            <button role="menuitem" @click="closeMenu(); buildOn(t)">Use in a new task</button>
            <button role="menuitem" @click="startRename(t)">Rename</button>
            <button role="menuitem" class="danger" @click="askDelete(t)">Delete</button>
          </div>
        </li>
          </ul>
          <button v-if="!closedProjects.has(group.path) && group.tasks.length > 5" class="show-more" @click="toggleExpandedProject(group.path)">
            {{ expandedProjects.has(group.path) ? "Show less" : "Show more" }}
          </button>
        </section>
      </div>
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

    </aside>

    <div class="main">
      <header class="topbar">
        <h1 v-if="heading" class="workspace-tab" :title="`${heading.name}\n${heading.status} · ${heading.meta}`">
          <span :id="domId('task-dot')" class="dot" :class="heading.tone" aria-hidden="true"></span>
          <span class="tab-name">{{ heading.name }}</span>
          <span class="hidden-label"> · {{ heading.status }}</span>
        </h1>
        <h1 v-else class="workspace-tab">
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M4 2h5l3 3v9H4V2Zm5 0v3h3M6 8h4M6 11h3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
          {{ view === 'agents' ? 'Agents' : view === 'stats' ? 'Stats' : view === 'memory' ? 'Memory' : view === 'settings' ? 'Settings' : view === 'history' ? 'Task history' : 'New task' }}
        </h1>
        <span class="project-name">{{ opened.project.name }}</span>
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

      <section :id="domId('project-git')" ref="gitPanel" class="git-panel" :style="gitPosition" popover role="dialog" :aria-labelledby="domId('git-title')" @toggle="toggleGit" @keydown.esc.stop="closeGit">
        <h2 :id="domId('git-title')" class="git-sr">Git</h2>
        <header class="git-head">
          <button
            type="button"
            class="git-branch"
            :class="{ on: gitAsk === 'switch' || gitAsk === 'branch' }"
            :disabled="!!gitDisabledReason('switch')"
            :title="gitDisabledReason('switch') || 'Switch or create a branch'"
            :aria-expanded="gitAsk === 'switch' || gitAsk === 'branch'"
            @click="openGit(gitAsk === 'switch' || gitAsk === 'branch' ? null : 'switch')"
          >
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><circle cx="4" cy="3" r="1.5" fill="none" stroke="currentColor" /><circle cx="12" cy="4" r="1.5" fill="none" stroke="currentColor" /><circle cx="4" cy="13" r="1.5" fill="none" stroke="currentColor" /><path d="M4 4.5v7M12 5.5C12 9 4 7 4 11" fill="none" stroke="currentColor" stroke-width="1.3" /></svg>
            <span class="mono">{{ git.branch ?? "detached HEAD" }}</span>
            <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="m3 4.5 3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
          </button>
          <span v-if="git.upstream" class="git-remote note" :title="`${git.upstream}: ${git.behind ?? 'unknown'} incoming, ${git.ahead ?? 'unknown'} outgoing at the last fetch`">
            <span class="mono">{{ git.upstream }}</span>
            <span>↓ {{ git.behind ?? '-' }}</span>
            <span>↑ {{ git.ahead ?? '-' }}</span>
            <span>at last fetch</span>
          </span>
          <span v-else class="git-remote note">Not published yet</span>
          <div class="git-sync" role="group" aria-label="Sync with the remote">
            <button class="btn" :disabled="!!gitDisabledReason('fetch')" :title="gitDisabledReason('fetch') || 'Check the remote for new commits; your files stay as they are'" @click="runGit('fetch')">Fetch</button>
            <button class="btn" :disabled="!!gitDisabledReason('pull')" :title="gitDisabledReason('pull') || gitQuestion('pull')" @click="openGit('pull')">Pull</button>
            <button class="btn" :disabled="!!gitDisabledReason('push')" :title="gitDisabledReason('push') || gitQuestion('push')" @click="openGit('push')">{{ gitActionLabel('push') }}</button>
          </div>
          <button type="button" class="git-icon" aria-label="Close Git" title="Close Git" @click="closeGit">
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
          </button>
        </header>

        <div class="git-body">
          <aside class="git-side">
            <div class="git-side-head">
              <span class="label">Changes</span>
              <span v-if="git.dirty" class="git-count">{{ git.dirtyCount }}</span>
              <button v-if="git.dirty" class="link danger-link" :disabled="!!gitDisabledReason('discard')" :title="gitDisabledReason('discard')" @click="openGit('discard')">Revert all</button>
            </div>
            <ul v-if="git.changes.length" class="git-changes" aria-label="Changed files">
              <li v-for="change in git.changes" :key="change.path" :class="{ selected: selectedGitPath === change.path }">
                <button class="git-file" :title="change.path" :aria-pressed="selectedGitPath === change.path" @click="selectedGitPath = change.path">
                  <span class="git-status mono" :class="gitStatusLabel(change.status).toLowerCase()" :title="gitStatusLabel(change.status)">{{ gitStatusLabel(change.status)[0] }}</span>
                  <span class="git-name">{{ splitPath(change.path).name }}</span>
                  <span class="git-dir mono">{{ splitPath(change.path).dir }}</span>
                </button>
                <button class="git-icon git-revert" :disabled="!!gitDisabledReason('discard')" :aria-label="`Revert ${change.path}`" :title="gitDisabledReason('discard') || `Revert ${change.path}`" @click="openGit('discard', change.path)">
                  <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><path d="M3.5 6.5h6a3.5 3.5 0 0 1 0 7H6M3.5 6.5 6 4M3.5 6.5 6 9" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
                </button>
              </li>
            </ul>
            <p v-else class="git-clean note">Working tree clean</p>

            <div class="git-compose">
              <div v-if="!gitAsk" class="git-actions" role="group" aria-label="Git actions">
                <button class="btn" :disabled="!!gitDisabledReason('commit')" :title="gitDisabledReason('commit')" @click="openGit('commit')">{{ git.dirty ? `Commit ${git.dirtyCount} file${git.dirtyCount === 1 ? '' : 's'}…` : 'Nothing to commit' }}</button>
                <button class="btn" :disabled="!!gitDisabledReason('merge')" :title="gitDisabledReason('merge')" @click="openGit('merge')">Merge…</button>
              </div>
              <form v-else class="git-confirm" @submit.prevent="runGit(gitAsk!)">
                <h3>{{ gitActionLabel(gitAsk) }}</h3>
                <p :id="domId('git-scope')" class="note">{{ gitQuestion(gitAsk) }}</p>
                <template v-if="gitAsk === 'commit'">
                  <label :for="domId('commit-message')" class="label">Commit message</label>
                  <input :id="domId('commit-message')" v-model="commitMessage" class="git-input" placeholder="Describe these changes" :disabled="!!gitBusy || drafting":aria-describedby="domId('git-scope')" required />
                </template>
                <template v-if="gitAsk === 'branch'">
                  <label :for="domId('new-branch')" class="label">Branch name</label>
                  <input :id="domId('new-branch')" v-model="newBranch" class="git-input mono" placeholder="feature/my-change" spellcheck="false" :disabled="!!gitBusy" :aria-describedby="domId('git-scope')" required />
                </template>
                <fieldset v-if="gitAsk === 'merge' || gitAsk === 'switch'" class="git-branches" :disabled="!!gitBusy">
                  <legend class="label">{{ gitAsk === 'merge' ? 'Branch to merge' : 'Local branches' }}</legend>
                  <label v-for="b in git.branches" :key="b" class="mono" :class="{ selected: targetBranch === b }">
                    <input v-model="targetBranch" type="radio" :name="domId('git-target')" :value="b" required />
                    <span>{{ b }}</span>
                  </label>
                  <p v-if="!git.branches.length" class="note">No other local branches.</p>
                </fieldset>
                <p v-if="gitDisabledReason(gitAsk) && !gitBusy && !gitLoading" class="note">{{ gitDisabledReason(gitAsk) }}</p>
                <div class="git-buttons">
                  <button
                    type="submit"
                    class="btn"
                    :class="gitAsk === 'discard' ? 'danger' : 'git-yes'"
                    :disabled="!!gitDisabledReason(gitAsk) || (gitAsk === 'commit' && !commitMessage.trim()) || (gitAsk === 'branch' && !newBranch.trim()) || ((gitAsk === 'merge' || gitAsk === 'switch') && !git.branches.includes(targetBranch))"
                  >
                    {{ gitBusy ? gitProgress : gitSubmitLabel(gitAsk) }}
                  </button>
                  <button type="button" class="btn" :disabled="!!gitBusy" @click="openGit()">Cancel</button>
                  <button v-if="gitAsk === 'commit'" type="button" class="link git-new" :disabled="!!gitBusy || drafting" title="Drafted by the cheapest model from your uncommitted changes" @click="draftCommit">{{ drafting ? 'Writing…' : 'Write it for me' }}</button>
                  <button v-if="gitAsk === 'switch'" type="button" class="link git-new" :disabled="!!gitBusy" @click="openGit('branch')">New branch…</button>
                </div>
              </form>
              <p v-if="running" class="note" role="status">Git actions are available when the task finishes.</p>
              <p v-if="gitError" class="git-error" role="alert">{{ gitError }}</p>
              <p v-if="gitRefreshError" class="git-error" role="alert">Could not refresh Git status: {{ gitRefreshError }}</p>
              <p v-if="gitBusy || gitNotice" class="note" role="status">{{ gitProgress || gitNotice }}</p>
            </div>
          </aside>

          <section class="git-diff" aria-label="Selected file changes">
            <template v-if="selectedGitPath">
              <h3><span class="mono">{{ selectedGitPath }}</span></h3>
              <template v-if="selectedGitPatch">
                <p v-for="note in selectedGitPatch.notes" :key="note" class="note">{{ note }}</p>
                <div v-for="(line, index) in selectedGitPatch.lines" :key="index" class="patch-line" :class="line.kind"><span class="line-number">{{ line.before ?? '' }}</span><span class="line-number">{{ line.after ?? '' }}</span><code>{{ line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : line.kind === 'hunk' ? '@@ ' : ' ' }}{{ line.text }}</code></div>
              </template>
              <p v-else class="note">No text comparison is available for this file.</p>
            </template>
            <p v-else-if="gitPatchError" class="git-error" role="alert">Could not load changes: {{ gitPatchError }}</p>
            <p v-else class="git-diff-empty">{{ git.dirty ? 'Select a changed file to review its diff.' : 'Nothing to review.' }}</p>
          </section>
        </div>

        <footer class="git-footer">
          <span class="note">{{ gitLoading ? 'Refreshing status…' : 'Local repository status' }}</span>
          <button class="link" :disabled="anyRunning || !!gitBusy || gitLoading" @click="refreshGit">Refresh</button>
        </footer>
      </section>

      <div ref="split" class="split" :class="[dockPrefs.side, { dragging }]">
        <main class="content" :class="{ page: ['stats', 'memory', 'settings', 'agents'].includes(view), composing: view === 'task' && !running && !result, chatting: (view === 'task' && (running || !!result)) || view === 'history' }">
          <template v-if="view === 'task'">
            <CurrentTask v-if="running || result" />
            <TaskComposer v-else />
          </template>
          <PastTask v-else-if="view === 'history'" />
          <Stats v-else-if="view === 'stats'" />
          <Memory v-else-if="view === 'memory'" :path="opened.project.path" />
          <Settings v-else-if="view === 'settings'" />
          <Agents v-else />
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
      <button class="switch-project" title="Back to the start screen" @click="$emit('close')">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M2 7.2 8 2l6 5.2" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /><path d="M3.6 6.4v6.1c0 .5.4.9.9.9h7c.5 0 .9-.4.9-.9V6.4M6.6 13.4V9.6h2.8v3.8" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
        Start screen
      </button>
      <span v-if="anyRunning" class="status-item"><span class="dot live" aria-hidden="true"></span>{{ runningCount === 1 ? "Task running" : `${runningCount} tasks running` }}</span>
      <span v-if="taskBadge" class="status-item" title="How the task on screen ran">{{ taskBadge }}</span>
      <span v-if="nextAnchor" class="status-item" title="The next call that starts a plan window on purpose. Set in Settings.">Window ping {{ nextAnchor }}</span>
      <div class="usage-counters" role="group" aria-label="Plan usage remaining">
        <template v-for="usage in usageCounters" :key="usage.id">
          <button class="usage-counter" :popovertarget="domId(`usage-${usage.id}`)" :title="`${usage.name} usage details and reset times`">
            <span class="usage-name"><ProviderMark :id="usage.id" :size="12" />{{ usage.name }}</span>
            <span v-if="usage.level" class="dot warn" aria-hidden="true"></span>
            <span v-if="usage.level" class="hidden-label">{{ usage.level === "limited" ? `limited${usage.until ? ` until ${usage.until}` : ""}` : "running low" }}</span>
            <span v-if="usage.status" class="usage-unavailable">- {{ usage.status }}</span>
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
            <p v-if="usage.level === 'limited'" class="note low" role="status">Limited{{ usage.until ? ` until ${usage.until}` : "" }}. New tasks go to the other agent until then.</p>
            <p v-else-if="usage.level === 'warning'" class="note low" role="status">Over 80% of a window is used.</p>
            <template v-if="usage.status">
              <p class="usage-empty" role="status">{{ usage.status }}</p>
              <p v-if="usage.status === 'Unavailable'" class="note usage-reason">{{ usage.reason }}</p>
              <button v-if="usage.status === 'Not installed' || usage.status === 'Sign in required'" class="btn" :popovertarget="domId(`usage-${usage.id}`)" popovertargetaction="hide" @click="view = 'agents'">Open Agents</button>
            </template>
            <div v-for="(w, i) in usage.windows" :key="i" class="usage-detail" :class="{ low: w.left !== null && w.left <= 20 }">
              <div class="usage-detail-label"><span>{{ w.label }}</span><strong>{{ w.leftLabel }} {{ w.left === null ? 'unavailable' : 'left' }}</strong></div>
              <meter v-if="w.left !== null" :value="w.left" min="0" max="100" :aria-label="`${usage.name} ${w.label} allowance remaining`">{{ w.leftLabel }} left</meter>
              <p class="note">{{ w.resets ? `Resets ${w.resets}` : 'Reset time unavailable' }}</p>
            </div>
            <footer class="usage-footer">
              <span class="note" role="status">{{ limitsLoading ? 'Refreshing…' : limitsCheckedAt ? `Checked ${new Date(limitsCheckedAt).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}` : 'Not checked yet' }}</span>
              <button class="link" :disabled="limitsLoading" @click="loadLimits(true)">Refresh usage</button>
            </footer>
            <p class="note usage-schedule">Updates when you open a project and after each task.</p>
          </section>
        </template>
        <button class="usage-refresh" :disabled="limitsLoading" :title="limitsLoading ? 'Refreshing usage…' : 'Refresh plan usage'" :aria-label="limitsLoading ? 'Refreshing plan usage' : 'Refresh plan usage'" @click="loadLimits(true)">
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
  overflow-y: auto;
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
.recent button,
.switch-list button {
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
.switch-list button:hover,
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
.project-groups {
  display: grid;
  align-content: start;
  gap: 16px;
  flex: 1;
  min-height: 0;
  padding: 6px 0 12px;
  overflow-y: auto;
}
.project-group {
  min-width: 0;
}
.project-heading {
  display: flex;
  align-items: center;
  gap: 4px;
  margin: 0 10px 5px;
  font-size: 14px;
  font-weight: 500;
}
.project-heading .project-toggle {
  display: flex;
  align-items: center;
  gap: 9px;
  flex: 1;
  min-width: 0;
  min-height: 30px;
  padding: 4px 0;
  color: var(--text-dim);
  text-align: left;
}
.project-heading .project-toggle:hover { color: var(--text); }
.project-heading span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.project-heading svg {
  flex-shrink: 0;
  color: var(--text-faint);
}
.project-heading .project-chevron { margin-left: auto; transition: transform 120ms ease; }
.project-heading .project-chevron.closed { transform: rotate(-90deg); }
.project-heading .project-new {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 26px;
  min-height: 26px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
}
.project-heading .project-new:hover { color: var(--text); background: var(--surface-2); }
.recent {
  grid-template-columns: minmax(0, 1fr);
  overflow-x: hidden;
  align-content: start;
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
  white-space: nowrap;
}
.recent .menu button:hover {
  background: var(--surface);
}
.recent .menu .danger,
.recent .menu .danger:hover {
  color: var(--err);
}
.show-more {
  margin: 4px 10px 0 20px;
  padding: 4px 0;
  color: var(--text-faint);
  font-size: 12px;
  text-align: left;
}
.show-more:hover {
  color: var(--text);
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
.switch-list {
  display: grid;
  gap: 2px;
  flex-shrink: 0;
  max-height: 30%;
  margin: 0;
  padding: 0;
  overflow-y: auto;
  list-style: none;
}
.switch-list svg {
  flex-shrink: 0;
  color: var(--text-faint);
}
.switch-list .dot {
  flex-shrink: 0;
}

.switch-project {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  min-height: 22px;
  padding: 2px 10px 2px 8px;
  border: 1px solid var(--border);
  border-radius: 999px;
  color: var(--text-dim, var(--text-faint));
  font-size: 11px;
  white-space: nowrap;
  transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
}
.switch-project:hover {
  color: var(--text);
  background: var(--surface-2);
  border-color: var(--text-faint);
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
.workspace-tab .dot {
  flex-shrink: 0;
}
.tab-name {
  max-width: 40ch;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.project-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  font-weight: 500;
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
.git-trigger.on {
  background: var(--surface-2);
}
.git-panel {
  inset: 56px 12px auto auto;
  width: min(900px, calc(100vw - 24px));
  height: min(600px, calc(100dvh - v-bind('gitPosition.top') - 16px));
  margin: 0;
  overflow: hidden;
  padding: 0;
  background: var(--surface);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
  font-size: 12px;
}
.git-panel:popover-open {
  display: flex;
  flex-direction: column;
}
.git-sr {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: 0;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}
.git-head {
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 48px;
  padding: 8px 8px 8px 10px;
  border-bottom: 1px solid var(--border);
}
.git-branch {
  display: inline-flex;
  flex-shrink: 1;
  align-items: center;
  gap: 6px;
  min-width: 0;
  min-height: 32px;
  padding: 4px 8px;
  border-radius: var(--r-sm);
  color: var(--text);
}
.git-branch svg {
  flex-shrink: 0;
  color: var(--text-faint);
}
.git-branch .mono {
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.git-branch:hover:not(:disabled),
.git-branch.on,
.git-icon:hover:not(:disabled) {
  background: var(--surface-2);
  color: var(--text);
}
.git-remote {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
}
.git-remote .mono {
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--text-dim);
}
.git-sync {
  display: flex;
  flex-shrink: 0;
  margin-left: auto;
}
.git-sync .btn,
.git-sync .btn:disabled {
  border-color: var(--border-strong);
  border-radius: 0;
}
.git-sync .btn + .btn {
  margin-left: -1px;
}
.git-sync .btn:first-child {
  border-radius: var(--r-sm) 0 0 var(--r-sm);
}
.git-sync .btn:last-child {
  border-radius: 0 var(--r-sm) var(--r-sm) 0;
}
.git-sync .btn:hover:not(:disabled) {
  position: relative;
}
.git-icon {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  min-width: 32px;
  min-height: 32px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
}
.git-body {
  display: grid;
  grid-template-columns: 300px minmax(0, 1fr);
  flex: 1;
  min-height: 0;
}
.git-side {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  border-right: 1px solid var(--border);
}
.git-side-head {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 40px;
  padding: 8px 12px 4px;
}
.git-side-head .label {
  margin: 0;
}
.git-side-head .link {
  margin-left: auto;
}
.git-count {
  padding: 0 7px;
  background: var(--surface-2);
  color: var(--text-dim);
  border-radius: 999px;
  font-size: 12px;
  line-height: 18px;
}
.git-changes {
  flex: 1;
  min-height: 0;
  margin: 0;
  padding: 0 6px 8px;
  overflow-y: auto;
  list-style: none;
}
.git-changes li {
  display: flex;
  align-items: center;
  border-radius: var(--r-sm);
}
.git-changes li:hover,
.git-changes li.selected {
  background: var(--surface-2);
}
.git-changes li:not(:hover, :focus-within, .selected) .git-revert {
  opacity: 0;
}
.git-file {
  display: flex;
  flex: 1;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
  padding: 5px 6px;
  color: var(--text);
  text-align: left;
}
.git-status {
  flex-shrink: 0;
  width: 1ch;
  color: var(--text-faint);
  font-weight: 600;
}
.git-status.added,
.git-status.untracked {
  color: var(--syntax-string);
}
.git-status.deleted {
  color: var(--syntax-number);
}
.git-status.modified,
.git-status.staged {
  color: var(--syntax-attr);
}
.git-status.renamed,
.git-status.copied {
  color: var(--syntax-name);
}
.git-name,
.git-dir {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.git-name {
  flex: 0 1 auto;
  min-width: 4ch;
}
.git-dir {
  flex: 1 1 0;
  min-width: 0;
  color: var(--text-faint);
}
.git-revert {
  min-width: 28px;
  min-height: 28px;
}
.git-revert:hover:not(:disabled) {
  color: var(--err);
}
.git-clean {
  flex: 1;
  margin: 0;
  padding: 4px 12px;
}
.git-compose {
  display: grid;
  gap: 8px;
  max-height: 65%;
  padding: 12px;
  overflow-y: auto;
  border-top: 1px solid var(--border);
}
.git-compose p {
  margin: 0;
  overflow-wrap: anywhere;
}
.git-actions {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 8px;
}
.git-confirm {
  display: grid;
  gap: 8px;
}
.git-confirm h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}
.git-confirm .label {
  margin: 4px 0 0;
}
.git-branches {
  display: grid;
  gap: 2px;
  min-width: 0;
  max-height: 180px;
  margin: 0;
  padding: 0;
  overflow-y: auto;
  border: 0;
}
.git-branches legend {
  padding: 0;
  margin-bottom: 6px;
}
.git-branches label {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  padding: 5px 8px;
  border-radius: var(--r-sm);
  cursor: pointer;
}
.git-branches label span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.git-branches label:hover,
.git-branches label.selected {
  background: var(--surface-2);
}
.git-branches input {
  margin: 0;
  accent-color: var(--text);
}
.git-buttons {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  margin-top: 4px;
}
.git-new {
  margin-left: auto;
}
.git-panel .btn {
  font-size: 12px;
}
.git-yes {
  color: var(--warn);
}
.git-panel .btn.danger {
  color: var(--err);
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
.git-input.mono {
  font-family: var(--mono);
}
.git-input::placeholder {
  color: var(--text-faint);
}
.git-dirty {
  color: var(--warn);
}
.danger-link {
  color: var(--err);
}
.git-error {
  font-size: 12px;
  color: var(--err);
  white-space: pre-wrap;
}
.git-diff {
  min-width: 0;
  min-height: 0;
  overflow: auto;
  background: var(--bg);
}
.git-diff h3 {
  position: sticky;
  top: 0;
  z-index: 1;
  margin: 0;
  padding: 8px 12px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
  font-size: 12px;
  font-weight: 400;
}
.git-diff > .note,
.git-diff > .git-error {
  margin: 0;
  padding: 8px 12px;
}
.git-diff .patch-line {
  display: grid;
  grid-template-columns: 6ch 6ch minmax(max-content, 1fr);
  width: max-content;
  min-width: 100%;
  font: 12px/1.7 var(--mono);
  white-space: pre;
}
.git-diff .line-number {
  min-width: 0;
  padding-right: 1ch;
  color: var(--text-faint);
  text-align: right;
  user-select: none;
}
.git-diff .patch-line code { padding-right: 12px; font: inherit; }
.git-diff .patch-line.added {
  color: var(--syntax-string);
  background: color-mix(in srgb, var(--syntax-string) 16%, var(--bg));
}
.git-diff .patch-line.removed {
  color: var(--syntax-number);
  background: color-mix(in srgb, var(--syntax-number) 16%, var(--bg));
}
.git-diff .patch-line.hunk {
  margin: 8px 0 2px;
  color: var(--text-faint);
  background: var(--surface-2);
}
.git-diff-empty {
  display: grid;
  height: 100%;
  margin: 0;
  place-items: center;
  color: var(--text-faint);
}
.git-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 6px 12px;
  border-top: 1px solid var(--border);
}
@media (max-width: 720px) {
  .git-panel { height: auto; max-height: calc(100dvh - v-bind('gitPosition.top') - 16px); overflow-y: auto; }
  .git-head { flex-wrap: wrap; }
  .git-body { grid-template-columns: 1fr; }
  .git-side { border-right: 0; border-bottom: 1px solid var(--border); }
  .git-changes { max-height: 180px; }
  .git-diff { min-height: 220px; max-height: 320px; }
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
/* Stats, Memory, Settings and Agents render loose blocks; centre each in the chat's column. */
.content.page > :deep(*) {
  max-width: 820px;
  margin-inline: auto;
}
/* A live run, its result and a past task are a chat: the box sits on the pane's bottom edge, so no bottom padding. */
.content.chatting {
  display: flex;
  flex-direction: column;
  padding-bottom: 0;
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
  margin-left: auto;
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
.usage-name {
  display: inline-flex;
  align-items: center;
  gap: 6px;
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
  .project-groups { max-height: 240px; }
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
  .content {
    padding: 24px 20px 48px;
  }
}
.task-inbox { display: flex; gap: 6px; margin: 8px 12px 6px; }
.task-inbox input, .task-inbox select { min-width: 0; background: var(--surface); color: var(--text); border: 1px solid var(--border); border-radius: var(--r-sm); padding: 7px 8px; font: inherit; font-size: 12px; }
.task-inbox input { flex: 1; }
.task-inbox select { flex: 0 0 58px; padding-right: 2px; }
.task-inbox input::placeholder { color: var(--text-faint); }
</style>
