<script setup lang="ts">
import { inject } from "vue";
import { PROJECT } from "./state";

// The coding CLIs Orteca drives: installed, signed in, and how much plan is left.
const {
  rows, missing, providerError, signingIn, signInLine, signInError, signIn, installing,
  installLine, installError, install, cancelProvider, limitLine, running, AUTH,
} = inject(PROJECT)!;
</script>

<template>
  <h2 class="title">AI helpers</h2>
  <p class="lede">The coding tools Orteca runs for you. It needs at least one that is set up and signed in.</p>

  <p v-if="providerError" class="missing">Could not check the AI helpers.</p>
  <ul v-else class="card providers">
    <li v-for="p in rows" :key="p.id">
      <span class="dot" :class="{ ok: p.path && p.auth !== 'signedOut' }" aria-hidden="true"></span>
      <span class="who">{{ p.program }}</span>
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
          <span v-if="AUTH[p.auth]" :class="p.auth === 'signedOut' ? 'missing' : 'note'">{{ AUTH[p.auth] }}</span>
          <span v-if="p.costQuality === 'unavailable'" class="note">tokens only, no cost</span>
          <span v-if="limitLine(p.id)" class="note">{{ limitLine(p.id) }}</span>
        </span>
        <button
          v-if="p.auth === 'signedOut'"
          class="btn"
          :disabled="signingIn !== null || installing !== null || running"
          @click="signIn(p.id)"
        >
          Sign in
        </button>
      </template>
      <template v-else-if="installing === p.id">
        <span class="note grow">{{ installLine }}</span>
        <button class="link" @click="cancelProvider(p.id)">cancel</button>
      </template>
      <template v-else>
        <span class="note grow">not set up</span>
        <button class="btn" :disabled="installing !== null || running" @click="install([p.id])">Set up</button>
      </template>
    </li>
  </ul>

  <p v-if="missing.length > 1 && !providerError" class="both">
    <button class="btn" :disabled="installing !== null || running" @click="install(missing.map((p) => p.id))">
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
</style>
