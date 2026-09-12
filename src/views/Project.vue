<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import {
  detectProviders,
  installProvider,
  isAppError,
  onInstallEvent,
  onSignInEvent,
  signInProvider,
  startTask,
} from "../api";
import type {
  Auth,
  Detected,
  OpenedProject,
  ProviderEvent,
  ProviderId,
  TaskResult,
} from "../types";

const props = defineProps<{ opened: OpenedProject }>();
defineEmits<{ close: [] }>();

const task = ref("");

// Detected live on every open: a CLI can be installed or signed in behind us.
// A missing CLI is shown, not thrown - the app is useful with neither present.
const providers = ref<Detected[]>([]);
const providerError = ref(false);
const provider = ref<ProviderId>("codex");

const installed = computed(() => providers.value.filter((p) => p.path));
const missing = computed(() => providers.value.filter((p) => !p.path));

// npm writes to one global folder, so two installs at once fight over it.
// One at a time, in order, and the button says which one is going.
const installing = ref<ProviderId | null>(null);
const installLine = ref("");
const installError = ref<string | null>(null);

async function install(ids: ProviderId[]) {
  if (installing.value !== null || running.value) return;
  installError.value = null;
  for (const id of ids) {
    installing.value = id;
    installLine.value = "asking npm…";
    try {
      const fresh = await installProvider(id);
      providers.value = providers.value.map((p) => (p.id === id ? fresh : p));
      provider.value = id;
    } catch (e) {
      installError.value = isAppError(e) ? e.message : String(e);
      break;
    }
  }
  installing.value = null;
  installLine.value = "";
}
// Sign-in is the CLI's own browser flow. Orteca starts it and shows its output;
// it never renders a login form and never handles a credential.
const signingIn = ref<ProviderId | null>(null);
const signInLine = ref("");
const signInError = ref<string | null>(null);

async function signIn(id: ProviderId) {
  if (signingIn.value !== null || installing.value !== null || running.value) return;
  signInError.value = null;
  signingIn.value = id;
  signInLine.value = "starting sign-in…";
  try {
    const fresh = await signInProvider(id);
    providers.value = providers.value.map((p) => (p.id === id ? fresh : p));
  } catch (e) {
    signInError.value = isAppError(e) ? e.message : String(e);
  } finally {
    signingIn.value = null;
    signInLine.value = "";
  }
}

const selected = computed(() => providers.value.find((p) => p.id === provider.value));

/// `signedOut` is a hard block, `unknown` is not: the CLI could not be asked,
/// and refusing to run on a guess would be the same mistake in the other
/// direction. The run itself reports an auth failure honestly either way.
const canRun = computed(
  () =>
    !running.value &&
    installing.value === null &&
    signingIn.value === null &&
    task.value.trim().length > 0 &&
    !!selected.value?.path &&
    selected.value.auth !== "signedOut",
);

// One run at a time. The stream and the result are the whole screen while it
// is going; nothing about a second run makes sense until cancel exists.
const running = ref(false);
const stream = ref<Array<{ kind: string; text: string }>>([]);
const result = ref<TaskResult | null>(null);
const runError = ref<string | null>(null);

let stop: Array<() => void> = [];

onMounted(async () => {
  try {
    providers.value = await detectProviders();
    // Prefer whatever is actually installed over the default.
    const first = installed.value[0];
    if (first && !installed.value.some((p) => p.id === provider.value)) {
      provider.value = first.id;
    }
  } catch {
    providerError.value = true;
  }

  stop = await Promise.all([
    onInstallEvent((id, line) => {
      if (installing.value === id) installLine.value = line;
    }),
    onSignInEvent((id, line) => {
      if (signingIn.value === id) signInLine.value = line;
    }),
  ]);
});

onUnmounted(() => stop.forEach((off) => off()));

async function run() {
  if (!canRun.value) return;
  stream.value = [];
  result.value = null;
  runError.value = null;
  running.value = true;
  try {
    result.value = await startTask(
      props.opened.project.path,
      task.value,
      provider.value,
      (event) => {
        const text = describe(event);
        if (text === null) return;
        stream.value.push({ kind: event.kind, text: text.length > 4000 ? text.slice(0, 4000) + "…" : text });
        if (stream.value.length > 500) stream.value.shift();
      },
    );
  } catch (e) {
    running.value = false;
    runError.value = isAppError(e) ? e.message : String(e);
  } finally {
    running.value = false;
  }
}

/** One line per event. Usage and the final result have their own panel. */
function describe(event: ProviderEvent): string | null {
  switch (event.kind) {
    case "started":
      return `session ${event.data.sessionId}`;
    case "text":
      return event.data;
    case "toolUse":
      return `${event.data.name} ${event.data.summary}`.trim();
    case "failed":
      return event.data.message;
    default:
      return null;
  }
}

const lines = computed(() => stream.value);

/** Never a number without a label, and never a zero standing in for unknown. */
const tokens = computed(() => {
  const usage = result.value?.usage;
  if (!usage) return null;
  return {
    total: usage.inputTokens + usage.cachedInputTokens + usage.outputTokens,
    cached: usage.cachedInputTokens,
    output: usage.outputTokens,
    cost: usage.costUsd,
    quality: usage.costQuality,
  };
});

function formatCost(cost: number): string {
  return cost > 0 && cost < 0.0001 ? "<$0.0001" : "$" + cost.toFixed(4);
}

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
      <button class="back" title="Back to projects" @click="$emit('close')">
        &larr;
      </button>
      <h1>{{ opened.project.name }}</h1>
      <span class="chip">{{ opened.git.branch ?? "detached" }}</span>
      <span v-if="opened.git.dirty" class="chip warn">
        {{ opened.git.dirtyCount }} uncommitted
      </span>
    </header>

    <!-- Prompt. The card is the input; the controls sit on its floor. -->
    <section class="ask card">
      <textarea
        id="task"
        v-model="task"
        rows="4"
        spellcheck="false"
        placeholder="What do you want to build?"
        :disabled="running"
      ></textarea>

      <div class="controls">
        <div v-if="installed.length" class="segments">
          <button
            v-for="p in installed"
            :key="p.id"
            class="seg"
            :class="{ on: provider === p.id }"
            :disabled="running"
            @click="provider = p.id"
          >
            {{ p.program }}
          </button>
        </div>
        <button class="btn primary run" :disabled="!canRun" @click="run">
          {{ running ? "Running…" : "Run" }}
        </button>
      </div>

      <div v-if="running" class="bar"><span></span></div>
    </section>

    <p v-if="runError" class="missing">{{ runError }}</p>
    <p v-else-if="!installed.length && !providerError" class="missing">
      Neither CLI is installed, so there is nothing to run yet.
    </p>

    <section v-if="lines.length" class="block">
      <h2 class="label">Activity</h2>
      <p class="note">Latest 500 entries; long messages shortened. Full events are saved in the task log.</p>
      <ol class="card stream">
        <li v-for="(line, i) in lines" :key="i" :class="line.kind">
          {{ line.text }}
        </li>
      </ol>
    </section>

    <section v-if="result" class="block">
      <h2 class="label">
        {{ result.status === "done" ? "Finished" : "Failed" }}
      </h2>
      <div class="card outcome">
        <p v-if="result.failure" class="missing">{{ result.failure }}</p>
        <p v-else-if="result.summary" class="summary">{{ result.summary }}</p>

        <!-- Three tiles. Every one labelled, none faked when unknown. -->
        <div class="tiles">
          <div class="tile">
            <span class="figure">{{ tokens ? tokens.total : "—" }}</span>
            <span class="note">
              {{ tokens ? tokens.cached + " cached" : "tokens unavailable" }}
            </span>
          </div>
          <div class="tile">
            <span class="figure" :class="{ good: tokens && tokens.cost !== null }">
              {{ tokens && tokens.cost !== null ? formatCost(tokens.cost) : "—" }}
            </span>
            <span class="note">
              {{
                tokens && tokens.cost !== null
                  ? "cost, " + tokens.quality
                  : "cost unavailable"
              }}
            </span>
          </div>
          <div class="tile">
            <span class="figure">{{ result.diff.length }}</span>
            <span class="note">Git-visible files changed</span>
          </div>
        </div>

        <ul class="diff">
          <li v-for="f in result.diff" :key="f.path">
            <span class="mono path">{{ f.path }}</span>
            <span v-if="f.added !== null" class="note">
              +{{ f.added }} &minus;{{ f.deleted }}
            </span>
            <span v-else class="note">new or binary</span>
          </li>
          <li v-if="!result.diff.length && !result.failure" class="note">no Git-visible changes</li>
        </ul>

        <p class="note caveat">Git-ignored files are excluded from this diff.</p>
        <p v-if="result.dirtyAtStart" class="note caveat">
          This repository already had uncommitted changes, so some of the above
          were not made by this run.
        </p>
      </div>
    </section>

    <section class="block">
      <h2 class="label">Providers</h2>
      <p v-if="providerError" class="missing">detection unavailable</p>
      <ul v-else class="card providers">
        <li v-for="p in providers" :key="p.id">
          <span class="who">{{ p.program }}</span>
          <template v-if="p.path && signingIn === p.id">
            <span class="note grow">{{ signInLine }}</span>
          </template>
          <template v-else-if="p.path">
            <span class="mono">{{ p.version ?? "version unknown" }}</span>
            <span :class="p.auth === 'signedOut' ? 'missing' : 'note'">
              {{ AUTH[p.auth] }}
            </span>
            <button
              v-if="p.auth === 'signedOut'"
              class="link"
              :disabled="signingIn !== null || installing !== null || running"
              @click="signIn(p.id)"
            >
              sign in
            </button>
            <span v-if="p.costQuality === 'unavailable'" class="note">
              tokens only, no cost
            </span>
          </template>
          <template v-else-if="installing === p.id">
            <span class="note grow">{{ installLine }}</span>
          </template>
          <template v-else>
            <span class="missing">not installed</span>
            <button
              class="link"
              :disabled="installing !== null || running"
              @click="install([p.id])"
            >
              install
            </button>
          </template>
        </li>
      </ul>
      <p v-if="missing.length > 1 && !providerError" class="foot">
        <button
          class="btn"
          :disabled="installing !== null || running"
          @click="install(missing.map((p) => p.id))"
        >
          {{ installing ? "Installing…" : "Install both" }}
        </button>
        <span class="note">
          runs <span class="mono">npm install --global</span> for
          {{ missing.map((p) => p.program).join(" and ") }}
        </span>
      </p>
      <p v-if="installError" class="missing">{{ installError }}</p>
      <p v-if="signInError" class="missing">{{ signInError }}</p>
      <p v-if="signingIn" class="note">
        Approve the sign-in in your browser. Orteca never sees your password.
      </p>
    </section>
  </main>
</template>

<style scoped>
.project {
  min-height: 100%;
  max-width: 720px;
  margin: 0 auto;
  padding: 56px var(--pad) 72px;
}

header {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 28px;
}
.back {
  padding: 2px 8px 4px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  transition: color 120ms ease, background 120ms ease;
}
.back:hover {
  color: var(--text);
  background: var(--surface-2);
}
h1 {
  margin: 0;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
}

.chip {
  padding: 2px 9px;
  border: 1px solid var(--border-strong);
  border-radius: 999px;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text-dim);
}
.chip.warn {
  color: var(--warn);
  border-color: var(--border);
}

/* Prompt card */
.ask {
  position: relative;
  overflow: hidden;
  transition: border-color 120ms ease;
}
.ask:focus-within {
  border-color: var(--border-strong);
}
textarea {
  display: block;
  width: 100%;
  background: none;
  color: var(--text);
  border: none;
  padding: 16px 18px 4px;
  font: inherit;
  resize: vertical;
}
textarea::placeholder {
  color: var(--text-faint);
}
textarea:focus {
  outline: none;
}
textarea:disabled {
  color: var(--text-dim);
}

.controls {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 12px 12px;
}
.segments {
  display: flex;
  gap: 2px;
  padding: 2px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.seg {
  padding: 4px 11px;
  border-radius: 4px;
  font-size: 12px;
  color: var(--text-faint);
  text-transform: capitalize;
  transition: color 120ms ease, background 120ms ease;
}
.seg:hover:not(:disabled) {
  color: var(--text-dim);
}
.seg.on {
  background: var(--surface-2);
  color: var(--text);
}
.seg:disabled {
  cursor: default;
}
.run {
  margin-left: auto;
  padding: 7px 20px;
}

/* Blue means in flight, and only that. */
.bar {
  position: absolute;
  inset: auto 0 0;
  height: 2px;
  background: var(--border);
}
.bar span {
  display: block;
  width: 35%;
  height: 100%;
  background: var(--info);
  animation: slide 1.4s ease-in-out infinite;
}
@keyframes slide {
  0% {
    transform: translateX(-100%);
  }
  100% {
    transform: translateX(300%);
  }
}
@media (prefers-reduced-motion: reduce) {
  .bar span {
    animation: none;
    width: 100%;
    opacity: 0.5;
  }
}

.block {
  margin-top: 32px;
}

.stream {
  margin: 0;
  padding: 12px 18px;
  list-style: none;
  max-height: 320px;
  overflow-y: auto;
}
.stream li {
  padding: 3px 0;
  color: var(--text-dim);
}
.stream li.toolUse,
.stream li.started {
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text-faint);
}
.stream li.failed {
  color: var(--err);
}

.outcome {
  padding: 18px;
}
.summary {
  margin: 0 0 16px;
}

.tiles {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 1px;
  background: var(--border);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  overflow: hidden;
  margin-bottom: 16px;
}
.tile {
  display: flex;
  flex-direction: column;
  gap: 2px;
  align-items: center;
  padding: 14px 10px;
  background: var(--surface-2);
  text-align: center;
}
.figure {
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
  font-variant-numeric: tabular-nums;
}
.figure.good {
  color: var(--accent);
}

.diff {
  margin: 0;
  padding: 0;
  list-style: none;
}
.diff li {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 3px 0;
}
.path {
  color: var(--text);
}
.caveat {
  margin: 12px 0 0;
}

.providers {
  margin: 0;
  padding: 8px 18px;
  list-style: none;
}
.providers li {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 7px 0;
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
.note {
  font-size: 12px;
  color: var(--text-faint);
}
/* npm prints long lines; the row must not grow a horizontal scrollbar. */
.grow {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.link {
  padding: 0;
  font-size: 12px;
  color: var(--accent);
  text-decoration: underline;
  text-underline-offset: 2px;
}
.link:disabled {
  color: var(--text-faint);
  cursor: default;
}
.foot {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 12px 0 0;
}
.missing {
  font-size: 12px;
  color: var(--warn);
}
</style>
