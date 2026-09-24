<script setup lang="ts">
import { computed, inject } from "vue";
import ProviderMark from "../../components/ProviderMark.vue";
import { PROJECT } from "./state";
import { nextPing, sessionReset } from "./anchor";
import type { Anchor } from "./anchor";
import type { ProviderId } from "../../types";

// The coding CLIs Orteca drives: installed, signed in, and how much plan is left.
const {
  rows, missing, providerError, signingIn, signInLine, signInError, signIn, installing,
  installLine, installError, install, cancelProvider, usageCounters, limitsLoading, limitsCheckedAt, loadLimits, anyRunning, AUTH, newTask,
  anchor, saveAnchor, limits, formatWhen,
} = inject(PROJECT)!;

const set = (change: Partial<Anchor>) => saveAnchor({ ...anchor.value, ...change });
function toggle(id: ProviderId, on: boolean) {
  set({ providers: on ? [...new Set([...anchor.value.providers, id])] : anchor.value.providers.filter((p) => p !== id) });
}
// What the schedule will do next, per chosen provider. Only as good as the last reading.
const plan = computed(() => anchor.value.mode === "off" ? [] : anchor.value.providers.map((id) => {
  const reading = limits.value.find((l) => l.id === id);
  const ping = nextPing(anchor.value, Date.now(), reading);
  const reset = sessionReset(reading);
  return {
    id,
    ping: ping === null ? "after the next reset it knows of" : formatWhen(ping),
    reset: reset === null ? "not read yet" : formatWhen(reset),
  };
}));
</script>

<template>
  <h2 class="title">Agents</h2>
  <p class="lede">The coding tools Orteca runs for you. It needs at least one that is set up and signed in.</p>

  <div class="agents-refresh"><span class="note">{{ limitsCheckedAt ? 'Usage checked ' + new Date(limitsCheckedAt).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }) : 'Usage has not been checked yet' }}</span><button class="btn" :disabled="limitsLoading" @click="loadLimits(true)">{{ limitsLoading ? 'Refreshing…' : 'Refresh allowance' }}</button></div>
  <p v-if="providerError" class="missing">Could not check the agents.</p>
  <ul v-else class="card providers">
    <li v-for="p in rows" :key="p.id">
      <span class="dot" :class="{ ok: p.path && (p.auth === 'subscription' || p.auth === 'apiKey') }" aria-hidden="true"></span>
      <span class="who"><ProviderMark :id="p.id" :size="16" />{{ p.program }}</span>
      <template v-if="p.pending">
        <span class="note grow">checking…</span>
      </template>
      <template v-else-if="p.path && signingIn === p.id">
        <span class="note grow">{{ signInLine }}</span>
        <button class="link" @click="cancelProvider(p.id)">cancel</button>
      </template>
      <template v-else-if="p.path">
        <span class="facts grow">
          <span class="mono">{{ p.version ?? "version unknown" }}</span>
          <span :class="p.auth === 'signedOut' || p.auth === 'unknown' ? 'missing' : 'note'">{{ AUTH[p.auth] || "Sign-in status unknown" }}</span>
          <span v-if="p.costQuality === 'unavailable'" class="note">tokens only, no cost</span>
          <span v-for="usage in usageCounters.filter((u) => u.id === p.id)" :key="usage.id" class="allowance">
            <span v-if="usage.status" class="note" :title="usage.reason">Allowance: {{ usage.status }}</span>
            <span v-for="window in usage.windows" :key="window.label" class="allowance-window"><span>{{ window.label }} · <strong>{{ window.leftLabel }} left</strong></span><meter v-if="window.left !== null" :value="window.left" min="0" max="100" :aria-label="window.label + ' allowance remaining'"></meter><span v-if="window.resets" class="note">Resets {{ window.resets }}</span></span>
          </span>
        </span>
        <button
          v-if="p.auth === 'signedOut' || p.auth === 'unknown'"
          class="btn"
          :disabled="signingIn !== null || installing !== null || anyRunning"
          @click="signIn(p.id)"
        >
          Sign in
        </button>
        <button v-else class="btn" @click="newTask">New task</button>
      </template>
      <template v-else-if="installing === p.id">
        <span class="note grow">{{ installLine }}</span>
        <button class="link" @click="cancelProvider(p.id)">cancel</button>
      </template>
      <template v-else>
        <span class="note grow">not set up</span>
        <button class="btn" :disabled="installing !== null || anyRunning" @click="install([p.id])">Set up</button>
      </template>
    </li>
  </ul>

  <p v-if="missing.length > 1 && !providerError" class="both">
    <button class="btn" :disabled="installing !== null || anyRunning" @click="install(missing.map((p) => p.id))">
      {{ installing ? "Setting up…" : "Set up both" }}
    </button>
    <span class="note">
      runs <span class="mono">npm install --global</span> for {{ missing.map((p) => p.program).join(" and ") }}
    </span>
  </p>
  <p v-if="installError" class="missing">{{ installError }}</p>
  <p v-if="signInError" class="missing">{{ signInError }}</p>
  <p v-if="signingIn" class="note">Approve the sign-in in your browser. Orteca never sees your password.</p>

  <section class="anchor" aria-labelledby="anchor-title">
    <h3 id="anchor-title" class="label">Start the 5-hour window on purpose</h3>
    <p class="note">One tiny call on the cheapest model starts a plan's 5-hour window, so it resets when you need it. It is skipped while a task runs or just ran, and when this computer was asleep at the time. Keep Orteca open.</p>
    <div class="segments" role="group" aria-label="Window start">
      <button v-for="m in (['off', 'morning', 'continuous'] as const)" :key="m" class="seg" :class="{ on: anchor.mode === m }" :aria-pressed="anchor.mode === m" @click="set({ mode: m })">
        {{ m === "off" ? "Off" : m === "morning" ? "Before work" : "After each reset" }}
      </button>
    </div>
    <div v-if="anchor.mode !== 'off'" class="anchor-fields">
      <label class="note">Work starts <input type="time" :value="anchor.start" @change="set({ start: ($event.target as HTMLInputElement).value })" /></label>
      <label v-if="anchor.mode === 'morning'" class="note">Limit usually hit after
        <input type="number" min="0" max="5" step="0.5" :value="anchor.hours" @change="set({ hours: Number(($event.target as HTMLInputElement).value) })" /> hours</label>
      <label v-else class="note">Work ends <input type="time" :value="anchor.end" @change="set({ end: ($event.target as HTMLInputElement).value })" /></label>
      <label v-for="p in rows.filter((r) => r.path)" :key="p.id" class="note">
        <input type="checkbox" :checked="anchor.providers.includes(p.id)" @change="toggle(p.id, ($event.target as HTMLInputElement).checked)" /> {{ p.program }}
      </label>
    </div>
    <ul v-if="plan.length" class="anchor-plan">
      <li v-for="row in plan" :key="row.id" class="note"><span class="mono">{{ row.id }}</span> next ping {{ row.ping }} · window resets {{ row.reset }}</li>
    </ul>
  </section>
</template>

<style scoped>
.title {
  margin: 0;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
}
.lede {
  margin: 6px 0 24px;
  color: var(--text-dim);
}
.providers {
  margin: 0;
  padding: 4px 18px;
  list-style: none;
}
.providers li {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 0;
}
.providers li + li {
  border-top: 1px solid var(--border);
}
.who {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-width: 64px;
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text);
}
.facts {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 12px;
  white-space: normal;
}
.providers .btn {
  padding: 4px 12px;
  font-size: 12px;
}
.both {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 12px 0 0;
}
.anchor {
  margin-top: 32px;
}
.anchor .note {
  margin: 4px 0 12px;
}
.segments {
  display: inline-flex;
  gap: 2px;
  padding: 2px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.seg {
  min-height: 26px;
  padding: 2px 10px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  font-size: 12px;
  transition: color 120ms ease, background 120ms ease;
}
.seg:hover {
  color: var(--text);
}
.seg.on {
  background: var(--surface-2);
  color: var(--text);
}
.anchor-fields {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 16px;
  margin-top: 12px;
}
.anchor-fields input[type="time"],
.anchor-fields input[type="number"] {
  padding: 3px 6px;
  color: var(--text);
  background: var(--bg);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  font: inherit;
  font-variant-numeric: tabular-nums;
}
.anchor-fields input[type="number"] {
  width: 56px;
}
.anchor-plan {
  margin: 12px 0 0;
  padding: 0;
  list-style: none;
}
.agents-refresh { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 16px; }
.facts { flex-direction: column; gap: 8px; }
.allowance { display: flex; flex-wrap: wrap; gap: 16px; }
.allowance-window { display: flex; flex-direction: column; gap: 4px; font-size: 12px; }
.allowance-window meter { width: 140px; height: 6px; }
.providers li { align-items: flex-start; }
@media (max-width: 600px) { .providers li { flex-wrap: wrap; } .facts { flex-basis: 65%; } }
</style>
