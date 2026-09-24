<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import type { TrustFinding } from "../types";

const props = defineProps<{ name: string; findings: TrustFinding[]; error?: string | null }>();

// Riskiest first. Findings sharing a reason share a row, so three skipped
// build folders read as one line instead of three identical ones.
const GROUPS = ["Can run code", "Adds to the agent's instructions", "In a parent folder", "Not fully checked"];
const groups = computed(() => {
  const kind = (path: string) => {
    if (/^(\/\/|[a-z]:)/i.test(path)) return 2;
    const base = path.split("/").pop()!.toLowerCase();
    if (base.endsWith(".md")) return 1;
    return [".claude", ".mcp.json", ".codex", ".agents"].includes(base) ? 0 : 3;
  };
  const rows = GROUPS.map((label) => ({ label, rows: new Map<string, string[]>() }));
  for (const f of props.findings) {
    const bucket = rows[kind(f.path)]!.rows;
    bucket.set(f.reason, [...(bucket.get(f.reason) ?? []), f.path.replace(/^\/\/\?\//, "")]);
  }
  return rows.filter((g) => g.rows.size);
});
const emit = defineEmits<{ trust: []; cancel: [] }>();
const panel = ref<HTMLElement | null>(null);
let previouslyFocused: HTMLElement | null = null;

function keydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    emit("cancel");
    return;
  }
  if (event.key !== "Tab") return;
  const buttons = panel.value?.querySelectorAll<HTMLElement>("button");
  if (!buttons?.length) return;
  const first = buttons.item(0);
  const last = buttons.item(buttons.length - 1);
  if (!first || !last) return;
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

onMounted(() => {
  previouslyFocused = document.activeElement as HTMLElement | null;
  void nextTick(() => panel.value?.querySelector<HTMLElement>("button")?.focus());
});
onUnmounted(() => previouslyFocused?.focus());
</script>

<template>
  <div class="scrim" @keydown="keydown">
    <section
      ref="panel"
      class="panel"
      role="dialog"
      aria-modal="true"
      aria-labelledby="trust-title"
      tabindex="-1"
    >
      <h2 id="trust-title">Trust {{ name }}?</h2>
      <p>
        Trusting lets this project run code on your computer: its agent
        settings and hooks load with every run, and Orteca runs its tests and
        Git hooks as you. Only trust it if you trust where the code came from.
        If those settings change later, Orteca asks again.
      </p>

      <section v-for="g in groups" :key="g.label">
        <h3 class="label">{{ g.label }}</h3>
        <ul>
          <li v-for="[reason, paths] in g.rows" :key="reason">
            <span class="paths"><code v-for="p in paths" :key="p">{{ p }}</code></span>
            <span>{{ reason }}</span>
          </li>
        </ul>
      </section>
      <p v-if="!findings.length" class="none">
        No agent settings found. Its tests and Git hooks still run as you.
      </p>

      <p v-if="error" role="alert" aria-live="assertive">{{ error }}</p>
      <footer>
        <button class="btn" @click="emit('cancel')">Cancel</button>
        <button class="btn consent" @click="emit('trust')">Trust and open</button>
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
  background: var(--overlay);
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
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
}

p {
  margin: 0 0 18px;
  color: var(--text-dim);
}

.none {
  color: var(--text-faint);
  font-size: 12px;
}

h3.label {
  margin-bottom: 6px;
}

ul {
  list-style: none;
  margin: 0 0 18px;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
li {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 10px 14px;
}
li + li {
  border-top: 1px solid var(--border);
}
.paths {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 10px;
}
code {
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text);
  overflow-wrap: anywhere;
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
/* Consent is never the light primary button: opening is the risky move. */
.consent {
  color: var(--warn);
  border-color: var(--border-strong);
}
.consent:hover {
  border-color: var(--warn);
  background: var(--surface-2);
}
</style>
