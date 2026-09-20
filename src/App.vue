<script setup lang="ts">
import { computed, ref } from "vue";
import Launch from "./views/Launch.vue";
import ProjectView from "./views/Project.vue";
import TrustPrompt from "./components/TrustPrompt.vue";
import { isAppError, openProject, trustProject } from "./api";
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
  activePath.value = null;
  let result: OpenedProject;
  try {
    result = await openProject(path);
  } catch (e) {
    if (request !== openRequest) return;
    openError.value = isAppError(e) ? e.message : String(e);
    return;
  }
  if (request !== openRequest) return;

  // Custom instruction filenames and future config formats cannot bypass consent.
  if (!result.project.trusted) {
    pendingTrust.value = result;
    return;
  }
  show(result);
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
    @close="close"
    @switch="(path) => (activePath = path)"
    @open="open"
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
