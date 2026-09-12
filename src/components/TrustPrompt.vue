<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import type { TrustFinding } from "../types";

defineProps<{ name: string; findings: TrustFinding[]; error?: string | null }>();
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
        Orteca may read setup instructions from this project and its parent
        folders. Only open it if you trust where the code came from.
      </p>

      <ul>
        <li v-for="f in findings" :key="f.path">
          <code>{{ f.path }}</code>
          <span>{{ f.reason }}</span>
        </li>
      </ul>

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

ul {
  list-style: none;
  margin: 0 0 22px;
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
code {
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text);
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
