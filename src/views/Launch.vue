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
    <header class="brand">
      <VeloMark :size="20" />
      <h1>Orteca</h1>
    </header>

    <p class="tagline">The efficient way to run coding agents.</p>

    <button class="btn" @click="choose">Open Project</button>

    <p v-if="error" class="error">{{ error }}</p>

    <section v-if="recents.length" class="recents">
      <h2>Recent projects</h2>
      <ul>
        <li v-for="p in recents" :key="p.path">
          <button class="entry" @click="emit('open', p.path)">
            <span class="name">{{ p.name }}</span>
            <span class="mono">{{ p.path }}</span>
          </button>
          <button class="forget" title="Remove from list" @click="forget(p.path)">
            &times;
          </button>
        </li>
      </ul>
    </section>
  </main>
</template>

<style scoped>
.launch {
  height: 100%;
  display: flex;
  flex-direction: column;
  justify-content: center;
  max-width: 520px;
  margin: 0 auto;
  padding: 40px var(--pad);
}

.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--accent);
}
.brand h1 {
  margin: 0;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
  color: var(--text);
}

.tagline {
  margin: 10px 0 36px;
  color: var(--text-dim);
}

.btn {
  align-self: flex-start;
}

.error {
  margin-top: 18px;
  color: var(--err);
}

.recents {
  margin-top: 56px;
}
.recents h2 {
  margin: 0 0 6px;
  font-size: 11px;
  font-weight: 500;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--text-faint);
}
.recents ul {
  list-style: none;
  margin: 0;
  padding: 0;
}
.recents li {
  display: flex;
  align-items: center;
  border-top: 1px solid var(--border);
}
.recents li:hover .forget {
  opacity: 1;
}

.entry {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  padding: 11px 8px 11px 0;
  text-align: left;
}
.entry:hover .name {
  color: var(--accent);
}
.name {
  transition: color 90ms ease;
}

.forget {
  opacity: 0;
  padding: 4px 8px;
  color: var(--text-faint);
  transition: opacity 90ms ease, color 90ms ease;
}
.forget:hover {
  color: var(--text);
}
</style>
