<script setup lang="ts">
import { ref } from "vue";
import type { Mode, OpenedProject } from "../types";

defineProps<{ opened: OpenedProject }>();
defineEmits<{ close: [] }>();

// Task submission lands in Milestone 4.
const task = ref("");
const mode = ref<Mode>("balanced");
const modes: Mode[] = ["efficient", "balanced"];
</script>

<template>
  <main class="project">
    <header>
      <button class="back" @click="$emit('close')">&larr;</button>
      <h1>{{ opened.project.name }}</h1>
      <span class="mono git">
        {{ opened.git.branch ?? "detached" }}
        <template v-if="opened.git.dirty">
          &middot; {{ opened.git.dirtyCount }} uncommitted
        </template>
      </span>
    </header>

    <label class="ask" for="task">What do you want to build?</label>
    <textarea id="task" v-model="task" rows="4" spellcheck="false"></textarea>

    <div class="modes">
      <button
        v-for="m in modes"
        :key="m"
        class="mode"
        :class="{ on: mode === m }"
        @click="mode = m"
      >
        {{ m }}
      </button>
    </div>
  </main>
</template>

<style scoped>
.project {
  max-width: 640px;
  margin: 0 auto;
  padding: 72px var(--pad);
}

header {
  display: flex;
  align-items: baseline;
  gap: 12px;
  margin-bottom: 56px;
}
.back {
  color: var(--text-faint);
  padding: 0 2px;
}
.back:hover {
  color: var(--text);
}
h1 {
  margin: 0;
  font-size: 17px;
  font-weight: 600;
  letter-spacing: -0.01em;
}
.git {
  color: var(--text-faint);
}

.ask {
  display: block;
  margin-bottom: 12px;
  color: var(--text-dim);
}

textarea {
  width: 100%;
  background: var(--surface);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
  padding: 12px 14px;
  font: inherit;
  resize: vertical;
}
textarea:focus {
  outline: none;
  border-color: var(--accent-dim);
}

.modes {
  display: flex;
  gap: 4px;
  margin-top: 16px;
}
.mode {
  padding: 5px 12px;
  border-radius: var(--r);
  color: var(--text-faint);
  text-transform: capitalize;
  transition: color 90ms ease, background 90ms ease;
}
.mode:hover {
  color: var(--text-dim);
}
.mode.on {
  background: var(--surface);
  color: var(--accent);
}
</style>
