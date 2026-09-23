<script setup lang="ts">
import { inject } from "vue";
import ProviderMark from "../../components/ProviderMark.vue";
import { PROJECT } from "./state";

// The coding CLIs Orteca drives: installed, signed in, and how much plan is left.
const {
  rows, missing, providerError, signingIn, signInLine, signInError, signIn, installing,
  installLine, installError, install, cancelProvider, usageCounters, limitsLoading, limitsCheckedAt, loadLimits, anyRunning, AUTH, newTask,
} = inject(PROJECT)!;
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
.agents-refresh { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 16px; }
.facts { flex-direction: column; gap: 8px; }
.allowance { display: flex; flex-wrap: wrap; gap: 16px; }
.allowance-window { display: flex; flex-direction: column; gap: 4px; font-size: 12px; }
.allowance-window meter { width: 140px; height: 6px; }
.providers li { align-items: flex-start; }
@media (max-width: 600px) { .providers li { flex-wrap: wrap; } .facts { flex-basis: 65%; } }
</style>
