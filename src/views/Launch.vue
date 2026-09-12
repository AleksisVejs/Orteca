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
    <div class="hero">
      <div class="tile"><VeloMark :size="64" /></div>
      <h1>Orteca</h1>
      <p class="tagline">The efficient way to run coding agents.</p>

      <button class="btn primary open" @click="choose">Open project</button>
      <p v-if="error" class="error">{{ error }}</p>
    </div>

    <section v-if="recents.length" class="recents">
      <h2 class="label">Recent</h2>
      <ul class="card">
        <li v-for="p in recents" :key="p.path">
          <button class="entry" @click="emit('open', p.path)">
            <span class="name">{{ p.name }}</span>
            <span class="mono path">{{ p.path }}</span>
          </button>
          <button class="forget" title="Remove from list" @click="forget(p.path)">
            &times;
          </button>
        </li>
      </ul>
    </section>

    <p class="foot">Better results. Less wasted context.</p>
  </main>
</template>

<style scoped>
.launch {
  height: 100%;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  justify-content: center;
  max-width: 560px;
  margin: 0 auto;
  padding: 48px var(--pad);
}

.hero {
  text-align: center;
}

/* The one place a glow is allowed: it is the product mark. */
.tile {
  display: inline-grid;
  place-items: center;
  width: 76px;
  height: 76px;
  border-radius: 20px;
  background: var(--surface);
  border: 1px solid var(--border-strong);
  box-shadow: 0 0 0 1px var(--mark-rim), 0 18px 40px var(--mark-shadow);
}

h1 {
  margin: 20px 0 0;
  font-size: 30px;
  font-weight: 600;
  letter-spacing: -0.03em;
}

.tagline {
  margin: 6px 0 28px;
  color: var(--text-dim);
}

.open {
  min-width: 180px;
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
  overflow: hidden;
}
.recents li {
  display: flex;
  align-items: center;
  padding-right: 8px;
  transition: background 120ms ease;
}
.recents li + li {
  border-top: 1px solid var(--border);
}
.recents li:hover {
  background: var(--surface-2);
}
.recents li:hover .forget {
  opacity: 1;
}

.entry {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 1px;
  padding: 12px 16px;
  text-align: left;
}
.path {
  max-width: 100%;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.forget {
  opacity: 0;
  padding: 4px 8px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  transition: opacity 120ms ease, color 120ms ease;
}
.forget:hover {
  color: var(--text);
}

.foot {
  margin: 40px 0 0;
  text-align: center;
  font-size: 12px;
  color: var(--text-faint);
}
</style>
