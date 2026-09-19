<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import VeloMark from "../components/VeloMark.vue";
import { forgetProject, isAppError, pickFolder, recentProjects } from "../api";
import type { Project } from "../types";

const props = defineProps<{ openError?: string | null }>();
const emit = defineEmits<{ open: [path: string] }>();

const recents = ref<Project[]>([]);
const loadError = ref<string | null>(null);
const error = computed(() => props.openError ?? loadError.value);

async function refresh() {
  try {
    recents.value = await recentProjects();
  } catch (e) {
    loadError.value = isAppError(e) ? e.message : String(e);
  }
}

onMounted(refresh);

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
  <main class="launch">
    <div class="brand"><VeloMark :size="32" /><span>Orteca</span></div>
    <div class="hero">
      <h1>Let’s get to work.</h1>
      <p class="tagline">Open a project. Give your coding agents a task.</p>

      <button class="btn primary open" @click="choose">
        <svg viewBox="0 0 20 20" width="18" height="18" aria-hidden="true"><path d="M3 6v10a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8a1 1 0 0 0-1-1h-6L8 4H4a1 1 0 0 0-1 1v1Z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" /></svg>
        Open a project
        <svg class="open-arrow" viewBox="0 0 20 20" width="18" height="18" aria-hidden="true"><path d="M4 10h12m-5-5 5 5-5 5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
      </button>
      <p v-if="error" class="error" role="alert">{{ error }}</p>
    </div>

    <section v-if="recents.length" class="recents">
      <h2 class="label">Recent projects</h2>
      <ul>
        <li v-for="p in recents" :key="p.path">
          <button class="entry" @click="emit('open', p.path)">
            <span class="name">{{ p.name }}</span>
            <span class="mono path" :title="p.path">{{ p.path }}</span>
          </button>
          <button class="forget" title="Remove from list" :aria-label="`Remove ${p.name} from recent projects`" @click="forget(p.path)">
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
          </button>
        </li>
      </ul>
    </section>

    <p class="foot">Choose a folder on your computer to get started.</p>
  </main>
</template>

<style scoped>
.launch {
  min-height: 100%;
  display: flex;
  flex-direction: column;
  justify-content: center;
  max-width: 560px;
  margin: 0 auto;
  padding: 48px var(--pad);
}

.brand {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  margin-bottom: 32px;
  font-weight: 600;
}
.hero {
  text-align: center;
}

h1 {
  margin: 0;
  font-size: 30px;
  font-weight: 600;
  letter-spacing: -0.03em;
}

.tagline {
  margin: 10px 0 32px;
  color: var(--text-dim);
}

.open {
  display: flex;
  align-items: center;
  justify-content: flex-start;
  min-width: 240px;
  max-width: 100%;
  margin: 0 auto;
  padding: 10px 16px;
}
.open-arrow {
  margin-left: 16px;
}

.error {
  margin: 16px 0 0;
  color: var(--err);
  font-size: 12px;
}

.recents {
  margin-top: 48px;
}
.recents ul {
  list-style: none;
  margin: 0;
  padding: 0;
  border-top: 1px solid var(--border);
}
.recents li {
  display: flex;
  align-items: center;
  padding-right: 4px;
  border-radius: var(--r-sm);
  transition: background 120ms ease;
}
.recents li + li {
  border-top: 1px solid var(--border);
}
.recents li:hover {
  background: var(--surface-2);
}
.entry {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 1px;
  padding: 16px 12px;
  text-align: left;
}
.path {
  max-width: 100%;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.forget {
  display: grid;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  transition: background 120ms ease, color 120ms ease;
}
.forget:hover {
  background: var(--surface-2);
  color: var(--text);
}

.foot {
  margin: 24px 0 0;
  text-align: center;
  font-size: 12px;
  color: var(--text-faint);
}
</style>
