<script setup lang="ts">
import VeloMark from "./components/VeloMark.vue";

// Milestone 2 replaces this with the real recents store.
const recent: { name: string; path: string }[] = [];
</script>

<template>
  <main class="launch">
    <header class="brand">
      <VeloMark :size="20" />
      <h1>Orteca</h1>
    </header>

    <p class="tagline">The efficient way to run coding agents.</p>

    <button class="btn open">Open Project</button>

    <section v-if="recent.length" class="recents">
      <h2>Recent projects</h2>
      <ul>
        <li v-for="p in recent" :key="p.path">
          <span class="name">{{ p.name }}</span>
          <span class="mono">{{ p.path }}</span>
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
  padding: 0 var(--pad);
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

.open {
  align-self: flex-start;
}

.recents {
  margin-top: 56px;
}
.recents h2 {
  margin: 0 0 14px;
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
  flex-direction: column;
  padding: 10px 0;
  border-top: 1px solid var(--border);
}
.name {
  color: var(--text);
}
</style>
