<script setup lang="ts">
import { onMounted, ref } from "vue";
import { detectProviders } from "../api";
import type { Auth, Detected, Mode, OpenedProject } from "../types";

defineProps<{ opened: OpenedProject }>();
defineEmits<{ close: [] }>();

// Task submission lands in Milestone 4.
const task = ref("");
const mode = ref<Mode>("balanced");
const modes: Mode[] = ["efficient", "balanced"];

// Detected live on every open: a CLI can be installed or signed in behind us.
// A missing CLI is shown, not thrown - the app is useful with neither present.
const providers = ref<Detected[]>([]);
const providerError = ref(false);
onMounted(async () => {
  try {
    providers.value = await detectProviders();
  } catch {
    providerError.value = true;
  }
});

const AUTH: Record<Auth, string> = {
  subscription: "saved login",
  apiKey: "API key",
  signedOut: "not signed in",
  unknown: "",
};
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

    <section class="providers">
      <h2>Providers</h2>
      <p v-if="providerError" class="missing">detection unavailable</p>
      <ul v-else>
        <li v-for="p in providers" :key="p.id">
          <span class="who">{{ p.program }}</span>
          <template v-if="p.path">
            <span class="mono">{{ p.version ?? "version unknown" }}</span>
            <span class="note">{{ AUTH[p.auth] }}</span>
            <span v-if="p.costQuality === 'unavailable'" class="note">
              tokens only, no cost
            </span>
          </template>
          <span v-else class="missing">not installed</span>
        </li>
      </ul>
    </section>
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

.providers {
  margin-top: 56px;
  border-top: 1px solid var(--border);
  padding-top: 16px;
}
.providers h2 {
  margin: 0 0 8px;
  font-size: 14px;
  font-weight: 400;
  color: var(--text-dim);
}
.providers ul {
  margin: 0;
  padding: 0;
  list-style: none;
}
.providers li {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 3px 0;
}
.who {
  min-width: 64px;
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text);
}
.note {
  font-size: 12px;
  color: var(--text-faint);
}
.missing {
  font-size: 12px;
  color: var(--warn);
}
</style>
