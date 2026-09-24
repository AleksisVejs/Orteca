<script setup lang="ts">
import { computed, ref } from "vue";
import Launch from "./views/Launch.vue";
import ProjectView from "./views/Project.vue";
import TrustPrompt from "./components/TrustPrompt.vue";
import { gitStatus, isAppError, openProject, trustProject } from "./api";
import type { OpenedProject } from "./types";

// Two screens plus one modal. A router would be more moving parts than routes.
//
// Every opened project stays mounted, hidden rather than torn down, because a
// run is a promise held by its screen: unmounting would leave the agent working
// in the repository with nothing on screen listening. Switching projects is
// changing which one is shown, never closing one.
const projects = ref<OpenedProject[]>([]);
const activePath = ref<string | null>(null);
const pendingTrust = ref<OpenedProject | null>(null);
const openError = ref<string | null>(null);
const taskFocus = ref<{ path: string; id: number } | null>(null);
const newTaskPath = ref<string | null>(null);
const SIDEBAR_STATE_KEY = "orteca.sidebar.task-groups.v1";
function loadSidebarState() {
  try {
    const saved = JSON.parse(localStorage.getItem(SIDEBAR_STATE_KEY) ?? "{}");
    return {
      closed: new Set<string>(Array.isArray(saved.closed) ? saved.closed : []),
      expanded: new Set<string>(Array.isArray(saved.expanded) ? saved.expanded : []),
      search: typeof saved.search === "string" ? saved.search : "",
      status: typeof saved.status === "string" ? saved.status : "all",
    };
  } catch {
    return { closed: new Set<string>(), expanded: new Set<string>(), search: "", status: "all" };
  }
}
const savedSidebarState = loadSidebarState();
// Sidebar state lives above individual project screens, so switching never
// swaps in a separate set of open groups, filters, or expanded task lists.
const closedTaskProjects = ref(savedSidebarState.closed);
const expandedTaskProjects = ref(savedSidebarState.expanded);
const sidebarSearch = ref(savedSidebarState.search);
const sidebarStatus = ref(savedSidebarState.status);
function saveSidebarState() {
  try {
    localStorage.setItem(SIDEBAR_STATE_KEY, JSON.stringify({
      closed: [...closedTaskProjects.value],
      expanded: [...expandedTaskProjects.value],
      search: sidebarSearch.value,
      status: sidebarStatus.value,
    }));
  } catch {
    // Storage can be disabled; the current app session still keeps its state.
  }
}
// Runs per project, reported by each screen. Closing a busy project is refused.
const busy = ref<Record<string, number>>({});
let openRequest = 0;

const openProjects = computed(() =>
  projects.value.map((p) => ({
    path: p.project.path,
    name: p.project.name,
    running: busy.value[p.project.path] ?? 0,
  })),
);

function show(result: OpenedProject) {
  // The backend resolves to the repository root, so the same repo opened by two
  // different subdirectories is one row here as well as one row in the database.
  if (!projects.value.some((p) => p.project.path === result.project.path)) {
    projects.value = [...projects.value, result];
  }
  activePath.value = result.project.path;
}

async function open(path: string) {
  const request = ++openRequest;
  pendingTrust.value = null;
  openError.value = null;
  const already = projects.value.find((p) => p.project.path === path);
  if (already) {
    activePath.value = already.project.path;
    return;
  }
  // Stay on the current screen while it loads; going home first made a flash.
  let result: OpenedProject;
  try {
    result = await openProject(path);
  } catch (e) {
    if (request !== openRequest) return;
    // The error shows on the launch screen, so go there only when it fails.
    activePath.value = null;
    openError.value = isAppError(e) ? e.message : String(e);
    return;
  }
  if (request !== openRequest) return;

  // Custom instruction filenames and future config formats cannot bypass consent.
  if (!result.project.trusted) {
    activePath.value = null;
    pendingTrust.value = result;
    return;
  }
  show(result);
}

async function openTask(path: string, id: number) {
  taskFocus.value = { path, id };
  await open(path);
}

async function newTaskForProject(path: string) {
  newTaskPath.value = path;
  await open(path);
}

function toggleTaskProject(path: string) {
  const next = new Set(closedTaskProjects.value);
  next.has(path) ? next.delete(path) : next.add(path);
  closedTaskProjects.value = next;
  saveSidebarState();
}

function toggleExpandedTaskProject(path: string) {
  const next = new Set(expandedTaskProjects.value);
  next.has(path) ? next.delete(path) : next.add(path);
  expandedTaskProjects.value = next;
  saveSidebarState();
}

function setSidebarSearch(value: string) {
  sidebarSearch.value = value;
  saveSidebarState();
}

function setSidebarStatus(value: string) {
  sidebarStatus.value = value;
  saveSidebarState();
}

async function confirmTrust() {
  const result = pendingTrust.value;
  if (!result) return;
  try {
    await trustProject(result.project.path, true);
  } catch (e) {
    if (pendingTrust.value !== result) return;
    openError.value = isAppError(e) ? e.message : String(e);
    return;
  }
  if (pendingTrust.value !== result) return;
  result.project.trusted = true;
  // Before consent Orteca asked git only for the root; the full state is safe now.
  try {
    result.git = await gitStatus(result.project.path);
  } catch {
    // The project screen refreshes git on its own; a failure here is not a failed open.
  }
  if (pendingTrust.value !== result) return;
  pendingTrust.value = null;
  show(result);
}

/** Back to the launch screen. Nothing is closed and nothing stops running. */
function close() {
  ++openRequest;
  pendingTrust.value = null;
  activePath.value = null;
  openError.value = null;
}

/** Really close one: its screen goes away, so its runs must have ended first. */
function closeProject(path: string) {
  if (busy.value[path]) return;
  projects.value = projects.value.filter((p) => p.project.path !== path);
  delete busy.value[path];
  if (activePath.value === path) activePath.value = null;
}
</script>

<template>
  <ProjectView
    v-for="p in projects"
    v-show="p.project.path === activePath"
    :key="p.project.path"
    :opened="p"
    :active="p.project.path === activePath"
    :open-projects="openProjects"
    :task-focus="taskFocus?.path === p.project.path ? taskFocus.id : null"
    :new-task-focus="newTaskPath === p.project.path"
    :closed-task-projects="closedTaskProjects"
    :expanded-task-projects="expandedTaskProjects"
    :sidebar-search="sidebarSearch"
    :sidebar-status="sidebarStatus"
    @close="close"
    @switch="(path) => (activePath = path)"
    @open="open"
    @task="openTask"
    @new-task="newTaskForProject"
    @toggle-project="toggleTaskProject"
    @toggle-expanded-project="toggleExpandedTaskProject"
    @update-search="setSidebarSearch"
    @update-status="setSidebarStatus"
    @focused="taskFocus = null"
    @new-task-focused="newTaskPath = null"
    @busy="(count) => (busy[p.project.path] = count)"
  />
  <Launch
    v-if="activePath === null"
    :open-error="openError"
    :open-projects="openProjects"
    @open="open"
    @close-project="closeProject"
  />

  <TrustPrompt
    v-if="pendingTrust"
    :name="pendingTrust.project.name"
    :findings="pendingTrust.trustFindings"
    :error="openError"
    @trust="confirmTrust"
    @cancel="pendingTrust = null"
  />
</template>
