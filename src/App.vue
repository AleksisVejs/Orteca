<script setup lang="ts">
import { ref } from "vue";
import Launch from "./views/Launch.vue";
import ProjectView from "./views/Project.vue";
import TrustPrompt from "./components/TrustPrompt.vue";
import { isAppError, openProject, trustProject } from "./api";
import type { OpenedProject } from "./types";

// Two screens plus one modal. A router would be more moving parts than routes.
const opened = ref<OpenedProject | null>(null);
const pendingTrust = ref<OpenedProject | null>(null);
const openError = ref<string | null>(null);
let openRequest = 0;

async function open(path: string) {
  const request = ++openRequest;
  opened.value = null;
  pendingTrust.value = null;
  openError.value = null;
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
  opened.value = result;
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
  opened.value = result;
}

function close() {
  ++openRequest;
  pendingTrust.value = null;
  opened.value = null;
  openError.value = null;
}
</script>

<template>
  <ProjectView v-if="opened" :key="opened.project.path" :opened="opened" @close="close" />
  <Launch v-else :open-error="openError" @open="open" />

  <TrustPrompt
    v-if="pendingTrust"
    :name="pendingTrust.project.name"
    :findings="pendingTrust.trustFindings"
    :error="openError"
    @trust="confirmTrust"
    @cancel="pendingTrust = null"
  />
</template>
