<script setup lang="ts">
import type { TrustFinding } from "../types";

defineProps<{ name: string; findings: TrustFinding[]; error?: string | null }>();
defineEmits<{ trust: []; cancel: [] }>();
</script>

<template>
  <div class="scrim">
    <section class="panel">
      <h2>Trust {{ name }}?</h2>
      <p>
        Agent CLIs can load instructions and run configuration from this project
        and its parent folders. Open it only if you trust where the code came from.
      </p>

      <ul>
        <li v-for="f in findings" :key="f.path">
          <code>{{ f.path }}</code>
          <span>{{ f.reason }}</span>
        </li>
      </ul>

      <p v-if="error" role="alert">{{ error }}</p>
      <footer>
        <button class="btn" @click="$emit('cancel')">Cancel</button>
        <button class="btn primary" @click="$emit('trust')">Trust and open</button>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.scrim {
  position: fixed;
  inset: 0;
  display: grid;
  place-items: center;
  background: rgb(0 0 0 / 0.55);
  padding: var(--pad);
}

.panel {
  max-height: calc(100dvh - 2 * var(--pad));
  overflow-y: auto;
  width: 100%;
  max-width: 520px;
  background: var(--surface);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
  padding: 24px;
}

h2 {
  margin: 0 0 8px;
  font-size: 15px;
  font-weight: 600;
  letter-spacing: -0.01em;
}

p {
  margin: 0 0 18px;
  color: var(--text-dim);
}

ul {
  list-style: none;
  margin: 0 0 22px;
  padding: 0;
}
li {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 9px 0;
  border-top: 1px solid var(--border);
}
code {
  font-family: var(--mono);
  font-size: 12px;
  color: var(--accent);
}
li span {
  color: var(--text-faint);
  font-size: 12px;
}

footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--gap);
}
.primary {
  border-color: var(--accent-dim);
  color: var(--accent);
}
</style>
