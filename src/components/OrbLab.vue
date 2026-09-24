<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import Orb, { ORB_LABEL, type OrbState } from "./Orb.vue";
import type { ProviderId, Tier } from "../types";

// Dev builds only: Ctrl+Shift+O previews every orb state.
const open = ref(false);
const state = ref<OrbState>("idle");
const provider = ref<ProviderId | "">("claude");
const tier = ref<Tier>("standard");
const progress = ref(0);
const orb = ref<InstanceType<typeof Orb> | null>(null);
const states = Object.keys(ORB_LABEL) as OrbState[];

function onKey(e: KeyboardEvent) {
  if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "o") {
    e.preventDefault();
    open.value = !open.value;
  } else if (open.value && e.key === "Escape") open.value = false;
}
onMounted(() => window.addEventListener("keydown", onKey));
onBeforeUnmount(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <section v-if="open" class="lab card" role="dialog" aria-label="Orb states (dev)">
    <header>
      <strong>Orb states</strong>
      <button class="link" @click="open = false">Close</button>
    </header>
    <div class="stage">
      <Orb :key="`${provider}-${tier}`" ref="orb" :size="160" :state="state" :provider="provider || null" :tier="tier" :progress="progress" />
    </div>
    <div class="states" role="group" aria-label="State">
      <button v-for="s in states" :key="s" class="btn" :aria-pressed="state === s" @click="state = s">{{ s }}</button>
    </div>
    <div class="row">
      <label>Provider
        <select v-model="provider"><option value="claude">Claude</option><option value="codex">Codex</option><option value="">None</option></select>
      </label>
      <label>Tier
        <select v-model="tier"><option value="cheapest">Cheap</option><option value="standard">Mid</option><option value="deep">Top</option></select>
      </label>
      <label>Progress <input v-model.number="progress" type="range" min="0" max="1" step="0.25" /></label>
    </div>
    <div class="row">
      <button class="btn" @click="orb?.ripple()">Keystroke</button>
      <button class="btn" @click="orb?.absorb()">Absorb steer</button>
    </div>
  </section>
</template>

<style scoped>
.lab {
  position: fixed;
  right: 16px;
  bottom: 44px;
  z-index: 50;
  width: 360px;
  padding: 12px;
  display: grid;
  gap: 10px;
  background: var(--surface);
  border: 1px solid var(--border-strong);
}
header, .row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
header { justify-content: space-between; }
.stage {
  display: grid;
  place-items: center;
  padding: 8px 0;
}
.states {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}
.states .btn {
  padding: 3px 8px;
  font-size: 12px;
}
.states .btn[aria-pressed="true"] {
  background: var(--surface-2);
  border-color: var(--border-strong);
}
label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-faint);
}
</style>
