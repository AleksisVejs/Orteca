<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import {
  cancelProviderOperation,
  cancelTask,
  detectProviders,
  getTaskDetail,
  installProvider,
  isAppError,
  onInstallEvent,
  onSignInEvent,
  previewTask,
  providerLimits,
  recentTasks,
  sendInstruction,
  signInProvider,
  startTask,
} from "../api";
import type {
  Auth,
  Detected,
  LimitWindow,
  Limits,
  Mode,
  OpenedProject,
  ProviderEvent,
  ProviderId,
  Preflight,
  TaskDetail,
  TaskResult,
  TaskSummary,
} from "../types";

const props = defineProps<{ opened: OpenedProject }>();
defineEmits<{ close: [] }>();

const task = ref("");

// Detected live on every open: a CLI can be installed or signed in behind us.
// A missing CLI is shown, not thrown - the app is useful with neither present.
const providers = ref<Detected[]>([]);
const providerError = ref(false);
const provider = ref<ProviderId>("codex");

// How readily the classifier takes the shorter route. Two modes, not three.
// The route itself is decided in Rust before any CLI starts and costs nothing.
const mode = ref<Mode>("balanced");
const MODES: Array<{ id: Mode; label: string; hint: string }> = [
  { id: "balanced", label: "Careful", hint: "Let Orteca plan, build, and check the work" },
  { id: "efficient", label: "Quick", hint: "Take the shortest safe path" },
];

const installed = computed(() => providers.value.filter((p) => p.path));
const missing = computed(() => providers.value.filter((p) => !p.path));

// Asking a CLI what it is costs a process start each, so the answers take
// seconds and arrive out of order. The card lists every provider from the
// first frame and fills each row in as it replies, rather than showing an
// empty box until the slowest one is done. `providers` still holds only
// answers, so nothing downstream can mistake a pending row for a verdict.
const ORDER: ProviderId[] = ["claude", "codex"];
type Row = Detected & { pending?: true };
const rows = computed<Row[]>(() =>
  ORDER.map(
    (id) =>
      providers.value.find((p) => p.id === id) ?? {
        id,
        program: id,
        path: null,
        version: null,
        auth: "unknown",
        costQuality: "unavailable",
        // Nothing reads this on a pending row - `selected` only ever finds a
        // real answer - but the cautious value is the one to stand in with.
        steering: "checkpoint",
        pending: true,
      },
  ),
);

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

async function cancelProvider(id: ProviderId) {
  try {
    await cancelProviderOperation(id);
  } catch {
    // The operation may have completed between rendering and the click.
  }
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

// What each plan has used, as its CLI reports it. Read on open and after every
// run; a reading costs no tokens but takes a few seconds.
const limits = ref<Limits[]>([]);
// Once the user picks a provider, headroom stops choosing for them.
const providerPicked = ref(false);
const pickedFor = ref<string | null>(null);

async function loadLimits() {
  try {
    limits.value = await providerLimits();
  } catch {
    limits.value = [];
  }
  pickByHeadroom();
}

/** The room left in this provider's tightest window, or null with no reading. */
function headroom(id: ProviderId): number | null {
  const windows = limits.value.find((l) => l.id === id)?.windows ?? [];
  return windows.length ? Math.min(...windows.map((w) => 100 - w.usedPercent)) : null;
}

/** Move to the runnable provider with the most room left. Only when every
 *  runnable one has a reading: an unread limit is not an empty one. */
function pickByHeadroom() {
  if (providerPicked.value) return;
  const runnable = installed.value
    .filter((p) => p.auth !== "signedOut")
    .map((p) => ({ id: p.id, room: headroom(p.id) }));
  if (runnable.length < 2 || runnable.some((r) => r.room === null)) return;
  // A tie keeps the provider already selected.
  const best = runnable.reduce((a, b) =>
    b.room! > a.room! || (b.room === a.room && b.id === provider.value) ? b : a,
  );
  pickedFor.value = `Using ${best.id}: it has the most limit left (${runnable
    .map((r) => `${r.id} ${Math.round(r.room!)}%`)
    .join(", ")}).`;
  if (best.id !== provider.value) {
    provider.value = best.id;
    schedulePreview();
  }
}

function resetWhen(w: LimitWindow): string | null {
  if (w.resetsText) return w.resetsText;
  if (w.resetsAt === null) return null;
  return new Date(w.resetsAt * 1000).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
}

/** A helper row's reading: every window with its label, or why there is none. */
function limitLine(id: ProviderId): string | null {
  const reading = limits.value.find((l) => l.id === id);
  if (!reading) return null;
  if (!reading.windows.length) return `limits unavailable: ${reading.unavailable ?? "no reading"}`;
  return reading.windows.map((w) => `${w.label} ${Math.round(w.usedPercent)}% used`).join(" · ");
}

// ponytail: a flat 5% of a window per agent call is a guess, not a measurement.
// Replace it with each route's measured draw once runs read limits before and after.
const PERCENT_PER_CALL = 5;

/** The fullest window that may not cover every call this route can make. */
const limitWarning = computed(() => {
  const p = preview.value;
  if (!p) return null;
  const calls = p.route.budget.maxAgentCalls + (p.escalation ? 1 : 0);
  const windows = limits.value.find((l) => l.id === p.provider)?.windows ?? [];
  const tight = windows
    .filter((w) => 100 - w.usedPercent < calls * PERCENT_PER_CALL)
    .sort((a, b) => b.usedPercent - a.usedPercent)[0];
  return tight ? { window: tight, calls, resets: resetWhen(tight) } : null;
});

/** The other runnable CLI, and its room left (null when unread). */
function otherThan(id: ProviderId): { id: ProviderId; room: number | null } | null {
  const other = installed.value.find((p) => p.id !== id && p.auth !== "signedOut");
  return other ? { id: other.id, room: headroom(other.id) } : null;
}

/** Before a run: the other CLI, when it is known to have more room than this one. */
const alternative = computed(() => {
  const p = preview.value;
  if (!p || !limitWarning.value) return null;
  const other = otherThan(p.provider);
  const mine = headroom(p.provider);
  return other && other.room !== null && (mine === null || other.room > mine) ? other.id : null;
});

/** After a run that stopped because its plan ran out: the CLI that could take over.
 *  Offered, never taken: continuing is a fresh run the user starts. */
const fallback = computed(() => {
  const r = result.value;
  if (r?.status !== "failed" || r.failureKind !== "usageLimit") return null;
  const other = otherThan(provider.value);
  return other && (other.room === null || other.room > 0) ? other : null;
});

function switchTo(id: ProviderId) {
  provider.value = id;
  providerPicked.value = true;
  pickedFor.value = null;
  schedulePreview();
}

/** The same request on the other CLI. What the stopped run changed is still on disk. */
async function continueWith(id: ProviderId) {
  provider.value = id;
  providerPicked.value = true;
  pickedFor.value = null;
  await run();
}

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
// is going, and Stop is the only other thing worth doing.
const running = ref(false);
const stream = ref<Array<{ kind: string; text: string }>>([]);
const result = ref<TaskResult | null>(null);
const runError = ref<string | null>(null);
const currentActivity = ref("Getting ready");

// The backend sends this the moment the task row exists, which is what Stop
// names. Until it arrives there is a run on screen that cannot yet be stopped,
// so the button is disabled rather than lying about what it would do.
const taskId = ref<number | null>(null);
const stopping = ref(false);

// A mid-task instruction. Where it lands is the provider's business, and the
// card says which before the user types rather than after they have sent it.
const instruction = ref("");
const sending = ref(false);
const instructionError = ref<string | null>(null);
const steering = computed(() => selected.value?.steering ?? "checkpoint");

async function instruct(applyNow: boolean) {
  const text = instruction.value.trim();
  if (!running.value || taskId.value === null || sending.value || !text) return;
  sending.value = true;
  instructionError.value = null;
  try {
    const receipt = await sendInstruction(taskId.value, text, applyNow);
    if (receipt.disposition === "tooLate") {
      instructionError.value = "The run finished before it could take that instruction.";
      return;
    }
    // Shown as the user's own words. Never pushed through `describe`, which
    // would file them among the things the agent said.
    stream.value.push({ kind: "instruction", text });
    instruction.value = "";
  } catch (e) {
    instructionError.value = isAppError(e) ? e.message : String(e);
  } finally {
    sending.value = false;
  }
}

async function stopRun() {
  if (!running.value || taskId.value === null || stopping.value) return;
  stopping.value = true;
  try {
    await cancelTask(taskId.value);
  } catch {
    // The run ended between the click and the call. There is nothing left to
    // stop, and the result about to arrive already says what happened.
  }
}

let stop: Array<() => void> = [];

// Past runs here, reloaded after each one. A history that cannot be read says
// so; it never blocks a run.
const history = ref<TaskSummary[]>([]);
const historyError = ref(false);
const historyDetail = ref<TaskDetail | null>(null);
const historyDetailLoading = ref(false);
const historyDetailError = ref(false);
let historyRequest = 0;

async function loadHistory() {
  try {
    history.value = await recentTasks(props.opened.project.path);
    historyError.value = false;
  } catch {
    historyError.value = true;
  }
}

async function openHistory(run: TaskSummary) {
  if (historyDetail.value?.id === run.id) {
    historyDetail.value = null;
    return;
  }
  const request = ++historyRequest;
  historyDetailLoading.value = true;
  historyDetailError.value = false;
  try {
    const detail = await getTaskDetail(props.opened.project.path, run.id);
    if (request === historyRequest) historyDetail.value = detail;
  } catch {
    if (request === historyRequest) historyDetailError.value = true;
  } finally {
    if (request === historyRequest) historyDetailLoading.value = false;
  }
}

const preview = ref<Preflight | null>(null);
const previewing = ref(false);
const previewError = ref<string | null>(null);
let previewTimer: ReturnType<typeof setTimeout> | null = null;
let previewRequest = 0;

function schedulePreview() {
  previewRequest += 1;
  if (previewTimer !== null) clearTimeout(previewTimer);
  if (!task.value.trim() || !selected.value?.path || running.value) {
    preview.value = null;
    previewError.value = null;
    return;
  }
  previewTimer = setTimeout(refreshPreview, 250);
}

async function refreshPreview() {
  const request = ++previewRequest;
  const chosen = selected.value;
  if (!chosen?.path || !task.value.trim()) return;
  previewing.value = true;
  previewError.value = null;
  try {
    const planned = await previewTask(props.opened.project.path, task.value, chosen.id, mode.value, headroom(chosen.id));
    if (request === previewRequest) preview.value = planned;
  } catch (e) {
    if (request === previewRequest) previewError.value = isAppError(e) ? e.message : String(e);
  } finally {
    if (request === previewRequest) previewing.value = false;
  }
}

onMounted(async () => {
  void loadHistory();
  void loadLimits();
  try {
    providers.value = await detectProviders((one) => {
      providers.value = [...providers.value.filter((p) => p.id !== one.id), one];
    });
    // Prefer whatever is actually installed over the default.
    const first = installed.value[0];
    if (first && !installed.value.some((p) => p.id === provider.value)) {
      provider.value = first.id;
    }
    // Limits may have landed first; both halves are needed to choose.
    pickByHeadroom();
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

onUnmounted(() => {
  if (previewTimer !== null) clearTimeout(previewTimer);
  stop.forEach((off) => off());
});

async function run() {
  if (!canRun.value) return;
  stream.value = [];
  result.value = null;
  runError.value = null;
  taskId.value = null;
  stopping.value = false;
  instruction.value = "";
  instructionError.value = null;
  currentActivity.value = "Getting ready";
  running.value = true;
  try {
    result.value = await startTask(
      props.opened.project.path,
      task.value,
      provider.value,
      mode.value,
      // The same reading the preview was routed on, so the run matches it.
      headroom(provider.value),
      (event) => {
        const activity = activityFor(event);
        if (activity !== null) currentActivity.value = activity;
        const text = describe(event);
        if (text === null) return;
        stream.value.push({ kind: event.kind, text: text.length > 4000 ? text.slice(0, 4000) + "…" : text });
        if (stream.value.length > 500) stream.value.shift();
      },
      (id) => {
        taskId.value = id;
      },
    );
  } catch (e) {
    running.value = false;
    runError.value = isAppError(e) ? e.message : String(e);
  } finally {
    running.value = false;
    stopping.value = false;
    taskId.value = null;
    await loadHistory();
    // The run just spent some of a limit; the next pick should know.
    void loadLimits();
  }
}

/** Short, human words for the things a provider does behind the scenes. */
function actionTarget(summary: string): string {
  const target = summary.replace(/\s+/g, " ").trim();
  return target.length > 64 ? target.slice(0, 61) + "…" : target;
}

function friendlyToolUse(name: string, summary: string): string {
  const kind = `${name} ${summary}`.toLowerCase();
  const target = actionTarget(summary);
  if (/edit|write|patch|create|delete|move|rename|file_change/.test(kind)) {
    return target ? `Editing ${target}` : "Editing files";
  }
  if (/read|cat|head|tail|grep|rg|find|list|search|inspect/.test(kind)) {
    return target ? `Reading ${target}` : "Reading the project";
  }
  if (/test|check|lint|build|compile|typecheck|cargo|npm/.test(kind)) {
    return "Checking that it works";
  }
  if (/git diff|git status/.test(kind)) return "Checking what changed";
  if (/shell|command|execute|bash|powershell/.test(kind)) return "Running a command";
  return target ? `Working on ${target}` : "Working on it";
}

function activityFor(event: ProviderEvent): string | null {
  switch (event.kind) {
    case "started":
      return "Getting ready";
    case "text":
      return "Thinking through the request";
    case "toolUse":
      return friendlyToolUse(event.data.name, event.data.summary);
    case "done":
      return "Putting on the finishing touches";
    case "failed":
      return `Couldn’t finish: ${event.data.message}`;
    default:
      return null;
  }
}

/** One line per event. Usage and the final result have their own panel. */
function describe(event: ProviderEvent): string | null {
  switch (event.kind) {
    case "started":
      return "Getting ready";
    case "text":
      return event.data;
    case "toolUse":
      return friendlyToolUse(event.data.name, event.data.summary);
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
    uncached: usage.inputTokens + usage.outputTokens,
    cached: usage.cachedInputTokens,
    output: usage.outputTokens,
    cost: usage.costUsd,
    quality: usage.costQuality,
    model: usage.model,
  };
});

function formatCost(cost: number): string {
  return cost > 0 && cost < 0.0001 ? "<$0.0001" : "$" + cost.toFixed(4);
}

/** Six-figure token counts are unreadable run together; group them. */
function formatTokens(count: number): string {
  return count.toLocaleString("en-US");
}

function formatDuration(milliseconds: number | null | undefined): string {
  if (milliseconds === null || milliseconds === undefined) return "duration unavailable";
  const seconds = Math.round(milliseconds / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m ${seconds % 60}s`;
}

function formatPayload(payload: unknown): string {
  if (typeof payload === "string") return payload;
  try {
    return JSON.stringify(payload, null, 2);
  } catch {
    return "unreadable event";
  }
}

/** A stopped run is its own outcome, not a quieter kind of failure. Nor is a
 *  run that reached the budget its route declared. */
const OUTCOME: Record<TaskResult["status"], string> = {
  done: "Done",
  cancelled: "Stopped",
  failed: "Couldn’t finish",
  budgetReached: "Stopped safely",
  reviewRejected: "Needs another look",
  verifyFailed: "Checks didn’t pass",
};

const STAGE_LABELS: Record<string, string> = {
  plan: "Making a plan",
  implement: "Making changes",
  review: "Reviewing the work",
  verify: "Checking that it works",
  fix: "Fixing it with a stronger model",
};

function stageLabel(stage: string): string {
  return STAGE_LABELS[stage] ?? stage;
}

/** What the route spent against what it was allowed. Both numbers are exact:
 *  Orteca chose the route, so it knows the ceiling as well as the spend. */
const calls = computed(() => {
  const r = result.value;
  if (!r) return null;
  return {
    used: r.callsUsed,
    // The Fix call, once bought, is the one call the route declared on top.
    allowed: r.route.budget.maxAgentCalls + (r.stages.some((s) => s.stage === "fix") ? 1 : 0),
    stages: r.route.stages,
    ran: r.stages.map((s) => s.stage),
  };
});

/** Every stage that ran, then the route's stages that did not. Stages run in
 *  order, so the ones that ran are the front of the route — unless a Fix call
 *  was bought, which takes the place of whatever was left. */
const routeSteps = computed(() => {
  const r = result.value;
  if (!r) return [];
  const ran = r.stages.map((s) => ({ stage: s.stage, ran: true }));
  if (ran.some((s) => s.stage === "fix")) return ran;
  return [...ran, ...r.route.stages.slice(ran.length).map((stage) => ({ stage, ran: false }))];
});

/** This run against the median of comparable finished runs here. Only for a
 *  run that finished, only once the backend has a baseline, and always
 *  labelled an estimate. `change` is positive when this run used fewer. */
const comparison = computed(() => {
  const r = result.value;
  const t = tokens.value;
  if (!r?.baseline || r.status !== "done" || !t || r.baseline.medianTokens <= 0) return null;
  // Cache reads excluded, as in the baseline: a warm cache is not less work.
  const change = Math.round((1 - (t.total - t.cached) / r.baseline.medianTokens) * 100);
  return { ...r.baseline, change, size: Math.abs(change) };
});

/** The diff split by whose change it is. A file untouched since before the
 *  run is the user's, and is neither listed nor counted as this run's work. */
const changed = computed(() => {
  const diff = result.value?.diff ?? [];
  return {
    byRun: diff.filter((f) => f.origin !== "beforeRun"),
    beforeRun: diff.filter((f) => f.origin === "beforeRun"),
    unknown: !!result.value?.dirtyAtStart && diff.some((f) => f.origin === null),
  };
});

const HISTORY_STATUS: Record<TaskSummary["status"], string> = { ...OUTCOME, running: "Running" };

/** A past run's metrics, each labelled, unknown spelled out rather than zeroed. */
function historyLine(t: TaskSummary): string {
  // Keep old local rows readable while newer rows distinguish uncached work.
  const legacy = t.uncachedTokens === undefined;
  const measured = legacy ? t.tokens : t.uncachedTokens;
  return [
    t.routeKind,
    t.callsUsed === null ? null : `${t.callsUsed} ${t.callsUsed === 1 ? "call" : "calls"}`,
    measured == null
      ? "tokens unavailable"
      : `${formatTokens(measured)} ${legacy ? "tokens" : "uncached"}`,
    t.model,
    t.costUsd === null ? null : `${formatCost(t.costUsd)} ${t.costQuality}`,
    t.durationMs == null ? null : formatDuration(t.durationMs),
    t.unknownEvents ? `${t.unknownEvents} unknown event${t.unknownEvents === 1 ? "" : "s"}` : null,
    `${t.startedAt.slice(0, 16)} UTC`,
  ]
    .filter(Boolean)
    .join(" · ");
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
      <button class="back" title="Back to projects" aria-label="Back to projects" @click="$emit('close')">
        &larr;
      </button>
      <h1>{{ opened.project.name }}</h1>
      <span class="chip">{{ opened.git.branch ?? "detached" }}</span>
      <span v-if="opened.git.dirty" class="chip warn">
        {{ opened.git.dirtyCount }} uncommitted
      </span>
    </header>

    <!-- One prompt, one clear action. Details stay available without crowding the first step. -->
    <section class="ask card">
      <label class="ask-heading" for="task">What do you want done?</label>
      <textarea
        id="task"
        v-model="task"
        rows="4"
        spellcheck="false"
        placeholder="Example: Add dark mode and make sure it works."
        :disabled="running"
        @input="schedulePreview"
      ></textarea>

      <div class="controls">
        <button
          v-if="running"
          class="btn stop"
          :disabled="taskId === null || stopping"
          @click="stopRun"
        >
          {{ stopping ? "Stopping…" : "Stop" }}
        </button>
        <button class="btn primary run" :disabled="!canRun" @click="run">
          {{ running ? "Working…" : "Do it" }}
        </button>
      </div>

      <details class="advanced-controls">
        <summary>More control</summary>
        <div class="advanced-options">
          <div>
            <span class="option-label">How careful should I be?</span>
            <div class="segments">
              <button
                v-for="m in MODES"
                :key="m.id"
                class="seg"
                :class="{ on: mode === m.id }"
                :aria-pressed="mode === m.id"
                :title="m.hint"
                :disabled="running"
                @click="mode = m.id; schedulePreview()"
              >
                {{ m.label }}
              </button>
            </div>
          </div>
          <div v-if="installed.length">
            <span class="option-label">Which AI should help?</span>
            <div class="segments">
              <button
                v-for="p in installed"
                :key="p.id"
                class="seg"
                :class="{ on: provider === p.id }"
                :aria-pressed="provider === p.id"
                :disabled="running"
                @click="provider = p.id; providerPicked = true; pickedFor = null; schedulePreview()"
              >
                {{ p.program }}
              </button>
            </div>
          </div>
        </div>
      </details>

      <div v-if="previewing" class="preview note" aria-live="polite">
        Figuring out the best way to do it…
      </div>
      <div v-else-if="preview" class="preview" aria-live="polite">
        <span class="preview-title">Orteca will handle the rest</span>
        <span class="route compact">{{ preview.route.stages.map(stageLabel).join(" → ") }}</span>
        <span class="note">I’ll plan, make the changes, and check the result.</span>
        <span v-if="preview.git.dirty" class="note caveat">
          I’ll keep your {{ preview.git.dirtyCount }} existing change{{ preview.git.dirtyCount === 1 ? "" : "s" }} safe.
        </span>
        <!-- Only reported figures reach the words; the per-call threshold is a guess and is not shown as a number. -->
        <span v-if="limitWarning" class="missing limit-warning" role="status">
          {{ preview.provider }}’s {{ limitWarning.window.label }} limit is {{ Math.round(limitWarning.window.usedPercent) }}% used<template v-if="limitWarning.resets">, resets {{ limitWarning.resets }}</template>.
          This can take up to {{ limitWarning.calls }} AI {{ limitWarning.calls === 1 ? "call" : "calls" }}, so it might not finish.
          <button v-if="alternative" class="link" @click="switchTo(alternative)">use {{ alternative }} instead</button>
        </span>
        <span v-if="pickedFor && !providerPicked" class="note">{{ pickedFor }}</span>
        <details class="preview-details">
          <summary>Show the plan</summary>
          <p class="note">{{ preview.route.reason }}</p>
          <p class="note budget-line">
            Up to {{ preview.route.budget.maxTurns ?? "the provider’s limit" }} steps ·
            {{ preview.route.budget.maxAgentCalls }} AI {{ preview.route.budget.maxAgentCalls === 1 ? "call" : "calls" }} ·
            {{ preview.model.model }}, {{ preview.model.effort }} effort
            <template v-if="preview.escalation">
              · one more call on {{ preview.escalation.model }}, {{ preview.escalation.effort }} effort, if a check or review doesn’t pass
            </template>
          </p>
          <p class="note">{{ preview.route.tierReason }}</p>
        </details>
      </div>
      <p v-else-if="previewError" class="preview-error missing" role="status">
        Preview unavailable: {{ previewError }}
      </p>

      <div v-if="running" class="bar"><span></span></div>
    </section>

    <p v-if="runError" class="missing">{{ runError }}</p>
    <p v-else-if="!installed.length && !providerError" class="missing">
      Neither CLI is installed, so there is nothing to run yet.
    </p>

    <section v-if="running" class="block">
      <h2 class="label">Right now</h2>
      <div class="card working" role="status" aria-live="polite">
        <span class="activity-dot" aria-hidden="true"></span>
        <div>
          <strong>{{ currentActivity }}</strong>
          <p class="note">Orteca is taking care of the details.</p>
        </div>
      </div>
      <h2 class="label follow-up-label">Want to add something?</h2>
      <div class="card steer">
        <input
          v-model="instruction"
          type="text"
          spellcheck="false"
          :placeholder="
            steering === 'live'
              ? 'Say something to it…'
              : 'Something for the next step…'
          "
          :disabled="taskId === null || sending"
          @keyup.enter="instruct(false)"
        />
        <div class="steer-row">
          <button
            class="btn"
            :disabled="taskId === null || sending || !instruction.trim()"
            @click="instruct(false)"
          >
            Send
          </button>
          <button
            v-if="steering === 'checkpoint'"
            class="btn"
            :disabled="taskId === null || sending || !instruction.trim()"
            @click="instruct(true)"
          >
            Apply now
          </button>
          <span class="note grow">
            <template v-if="steering === 'live'">
              I’ll use this for the next step.
            </template>
            <template v-else>
              Orteca will use this on the next step. “Apply now” restarts that
              step and keeps anything already changed.
            </template>
          </span>
        </div>
        <p v-if="instructionError" class="missing">{{ instructionError }}</p>
      </div>
    </section>

    <section v-if="lines.length" class="block">
      <h2 class="label">What Orteca is doing</h2>
      <p class="note">A simple live view. The full technical log is saved with the task.</p>
      <ol class="card stream" role="log" aria-live="polite" aria-relevant="additions">
        <li v-for="(line, i) in lines" :key="i" :class="line.kind">
          <span v-if="line.kind === 'instruction'" class="said">you</span>
          {{ line.text }}
        </li>
      </ol>
    </section>

    <section v-if="result" class="block">
      <h2 class="label">{{ OUTCOME[result.status] }}</h2>
      <div class="card outcome">
        <p v-if="result.failure" class="missing">{{ result.failure }}</p>
        <div v-if="fallback" class="fallback" role="status">
          <p class="note">
            {{ provider }} is out of plan usage. {{ fallback.id }} can carry on with the same
            request<template v-if="fallback.room !== null">, with {{ Math.round(fallback.room) }}% of its tightest limit left</template>.
            Anything already changed stays on disk.
          </p>
          <button class="btn" :disabled="running" @click="continueWith(fallback.id)">
            Continue with {{ fallback.id }}
          </button>
        </div>
        <p v-else-if="result.summary" class="summary">{{ result.summary }}</p>
        <p v-if="result.status === 'cancelled'" class="note caveat stopped">
          Stopped part-way. Anything the agent had already written is still on
          disk — Orteca reverts nothing.
        </p>
        <!-- A budget stop hands the decision back rather than spending more.
             Nothing here continues the run: that is a fresh Run, deliberately. -->
        <p v-if="result.budgetStop" class="note caveat stopped">
          Orteca stopped safely after using the planned limit. Run again if you
          want it to keep going.
        </p>
        <p v-if="result.budgetStop?.remaining.length" class="note caveat stopped">
          Still to do: {{ result.budgetStop.remaining.map(stageLabel).join(" → ") }}. Run again
          if you want Orteca to keep going.
        </p>
        <p v-if="result.unknownEvents" class="note caveat unknown-events" role="status">
          {{ result.unknownEvents }} provider event{{ result.unknownEvents === 1 ? "" : "s" }} were not recognized and remain available in the task log below.
        </p>

        <details class="result-details">
          <summary>Show details</summary>
          <!-- The route as decided before anything ran: what ran, what did not. -->
        <ol class="route">
          <template v-for="(step, i) in routeSteps" :key="step.stage">
            <li v-if="i" class="arrow" aria-hidden="true">→</li>
            <li :class="{ ran: step.ran }">
              {{ stageLabel(step.stage) }}<span class="hidden-label"> — {{ step.ran ? "ran" : "not started" }}</span>
            </li>
          </template>
        </ol>
        <p class="note reason">
          {{ result.route.reason }}
          <template v-if="result.route.candidatePaths.length">
            · brief named
            <span class="mono">{{ result.route.candidatePaths.join(", ") }}</span>
          </template>
        </p>

        <!-- Four tiles. Every one labelled, none faked when unknown. -->
        <div class="tiles">
          <div v-if="calls" class="tile">
            <span class="figure">{{ calls.used }} / {{ calls.allowed }}</span>
            <span class="note">
              agent {{ calls.allowed === 1 ? "call" : "calls" }} used ·
              {{ result.turnsUsed }} / {{ result.route.budget.maxTurns ?? "provider-defined" }} turns —
              {{ calls.ran.join(" → ") || "none" }}
            </span>
          </div>
          <div class="tile">
            <span class="figure">{{ tokens ? formatTokens(tokens.total) : "—" }}</span>
            <span class="note">
              {{
                tokens
                  ? formatTokens(tokens.uncached) + " uncached · " + formatTokens(tokens.cached) + " cached"
                  : "tokens unavailable"
              }}
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
            <span class="figure">{{ formatDuration(result.durationMs) }}</span>
            <span class="note">elapsed</span>
          </div>
          <div class="tile">
            <span class="figure model">{{ tokens?.model ?? "—" }}</span>
            <span class="note">model reported by provider</span>
          </div>
          <div class="tile">
            <span class="figure">{{ changed.byRun.length }}</span>
            <span class="note">Git-visible files changed by this run</span>
          </div>
        </div>

        <!-- No savings claim without a measured baseline, and never unlabelled. -->
        <p v-if="comparison" class="note compare">
          <span :class="{ good: comparison.change > 0 }">
            {{ comparison.size }}% {{ comparison.change >= 0 ? "fewer" : "more" }} uncached tokens
          </span>
          than the median of the last {{ comparison.runs }} finished runs of this
          route here ({{ formatTokens(comparison.medianTokens) }}), estimated.
        </p>
        <p v-else-if="result.status === 'done' && tokens" class="note compare">
          No savings figure yet: that needs five finished runs of this route here
          to compare against.
        </p>
        </details>

        <ul class="diff">
          <li v-for="f in changed.byRun" :key="f.path">
            <span class="mono path">{{ f.path }}</span>
            <span v-if="f.added !== null" class="note">
              +{{ f.added }} &minus;{{ f.deleted }}
            </span>
            <span v-else class="note">new or binary</span>
            <span v-if="f.origin === 'both'" class="note">
              already changed before this run; counts include both
            </span>
          </li>
          <li v-if="!changed.byRun.length && !result.failure" class="note">no files changed</li>
        </ul>

        <p class="note caveat">Some hidden files are not shown here.</p>
        <!-- Only when git could not snapshot the tree first. Otherwise the list
             above is exact about whose change is whose. -->
        <p v-if="changed.unknown" class="note caveat">
          This repository already had uncommitted changes, so some of the above
          may not have been made by this run.
        </p>
        <p v-if="changed.beforeRun.length" class="note caveat">
          Already changed before this run, and left as they were:
          <span class="mono">{{ changed.beforeRun.map((f) => f.path).join(", ") }}</span>
        </p>
        <details v-if="result.patchText" class="patch-view">
          <summary>View patch</summary>
          <pre>{{ result.patchText }}</pre>
        </details>
      </div>
    </section>

    <section v-if="history.length || historyError" class="block">
      <h2 class="label">Past work</h2>
      <p v-if="historyError" class="missing">history unavailable</p>
      <ul v-else class="card history">
        <li v-for="t in history" :key="t.id">
          <button
            class="history-entry"
            :aria-expanded="historyDetail?.id === t.id"
            :title="`Open details for ${t.prompt}`"
            @click="openHistory(t)"
          >
            <span class="status">{{ HISTORY_STATUS[t.status] ?? t.status }}</span>
            <span class="grow" :title="t.prompt">{{ t.prompt }}</span>
            <span class="note">{{ historyLine(t) }}</span>
            <span class="chevron" aria-hidden="true">{{ historyDetail?.id === t.id ? "−" : "+" }}</span>
          </button>
        </li>
      </ul>
      <p v-if="historyDetailLoading" class="note detail-state" aria-live="polite">Loading task details…</p>
      <p v-if="historyDetailError" class="missing detail-state" role="alert">Task details unavailable.</p>
      <div v-if="historyDetail" class="card history-detail">
        <div class="detail-heading">
          <span class="status">{{ HISTORY_STATUS[historyDetail.status] ?? historyDetail.status }}</span>
          <span class="note">{{ historyDetail.startedAt.slice(0, 16) }} UTC</span>
        </div>
        <p class="detail-prompt">{{ historyDetail.prompt }}</p>
        <p v-if="historyDetail.summary" class="summary">{{ historyDetail.summary }}</p>
        <div class="detail-metrics">
          <span>{{ historyDetail.uncachedTokens === null ? "uncached tokens unavailable" : formatTokens(historyDetail.uncachedTokens) + " uncached tokens" }}</span>
          <span>{{ historyDetail.model ?? "model unavailable" }}</span>
          <span>{{ formatDuration(historyDetail.durationMs) }}</span>
          <span v-if="historyDetail.callsUsed !== null">{{ historyDetail.callsUsed }} calls</span>
          <span v-if="historyDetail.unknownEvents">{{ historyDetail.unknownEvents }} unknown events</span>
        </div>
        <ul class="diff">
          <li v-for="f in historyDetail.diff" :key="f.path">
            <span class="mono path">{{ f.path }}</span>
            <span v-if="f.added !== null" class="note">+{{ f.added }} −{{ f.deleted }}</span>
            <span v-else class="note">new or binary</span>
          </li>
          <li v-if="!historyDetail.diff.length" class="note">no Git-visible changes</li>
        </ul>
        <details v-if="historyDetail.patchText" class="patch-view">
          <summary>View patch</summary>
          <pre>{{ historyDetail.patchText }}</pre>
        </details>
        <details class="event-log">
          <summary>View task log · {{ historyDetail.events.length }} events</summary>
          <ol>
            <li v-for="event in historyDetail.events" :key="event.id">
              <span class="mono">{{ event.stage ?? "run" }} · {{ event.kind }}</span>
              <pre>{{ formatPayload(event.payload) }}</pre>
            </li>
          </ol>
        </details>
      </div>
    </section>

    <section class="block">
      <h2 class="label">AI helpers</h2>
      <p v-if="providerError" class="missing">Could not check the AI helpers.</p>
      <ul v-else class="card providers">
        <li v-for="p in rows" :key="p.id">
          <span class="who">{{ p.program }}</span>
          <template v-if="p.pending">
            <span class="note grow">checking…</span>
          </template>
          <template v-else-if="p.path && signingIn === p.id">
            <span class="note grow">{{ signInLine }}</span>
            <button class="link" @click="cancelProvider(p.id)">cancel</button>
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
            <span v-if="limitLine(p.id)" class="note">{{ limitLine(p.id) }}</span>
          </template>
          <template v-else-if="installing === p.id">
            <span class="note grow">{{ installLine }}</span>
            <button class="link" @click="cancelProvider(p.id)">cancel</button>
          </template>
          <template v-else>
            <span class="missing">not ready</span>
            <button
              class="link"
              :disabled="installing !== null || running"
              @click="install([p.id])"
            >
              set up
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
          {{ installing ? "Setting up…" : "Set up both" }}
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
.hidden-label {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}
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
.ask-heading {
  display: block;
  padding: 16px 18px 0;
  font-weight: 600;
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
.advanced-controls {
  border-top: 1px solid var(--border);
  padding: 0 12px;
}
.advanced-controls summary,
.preview-details summary,
.result-details summary {
  cursor: pointer;
  color: var(--text-faint);
  font-size: 12px;
}
.advanced-controls summary {
  padding: 10px 0;
}
.advanced-options {
  display: flex;
  flex-wrap: wrap;
  gap: 16px 24px;
  padding: 0 0 12px;
}
.option-label {
  display: block;
  margin-bottom: 5px;
  font-size: 12px;
  color: var(--text-dim);
}
.preview {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 10px;
  margin: 0 12px 12px;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--surface-2);
}
.preview-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--text);
}
.preview .route {
  font-size: 12px;
  color: var(--text-dim);
}
.preview .note {
  width: 100%;
}
.preview-details {
  width: 100%;
  margin-top: 4px;
  padding-top: 8px;
  border-top: 1px solid var(--border);
}
.preview-details p {
  margin: 8px 0 0;
}
.preview-error {
  margin: 0 18px 12px;
}
.limit-warning {
  width: 100%;
}
.fallback {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
  margin: 0 0 16px;
}
.fallback .note {
  flex: 1;
  margin: 0;
}
/* Never the primary button: stopping a run is not the obvious next step. */
.stop {
  margin-left: auto;
  padding: 7px 16px;
}
.stop + .run {
  margin-left: 0;
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
  .bar span,
  .activity-dot {
    animation: none;
  }
  .bar span {
    width: 100%;
    opacity: 0.5;
  }
}

.block {
  margin-top: 32px;
}

.working {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 16px 18px;
}
.working strong {
  display: block;
  font-weight: 600;
}
.working p {
  margin: 2px 0 0;
}
.activity-dot {
  flex: 0 0 auto;
  width: 8px;
  height: 8px;
  margin-top: 7px;
  border-radius: 50%;
  background: var(--info);
  animation: activity-pulse 1.6s ease-in-out infinite;
}
@keyframes activity-pulse {
  0%,
  100% {
    opacity: 0.45;
  }
  50% {
    opacity: 1;
  }
}
.follow-up-label {
  margin-top: 24px;
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
/* The user's own words, marked as theirs rather than as something said back. */
.stream li.instruction {
  color: var(--text);
}
.said {
  margin-right: 8px;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text-faint);
}

.steer input {
  display: block;
  width: 100%;
  background: none;
  color: var(--text);
  border: none;
  padding: 14px 18px 4px;
  font: inherit;
}
.steer input::placeholder {
  color: var(--text-faint);
}
.steer input:focus {
  outline: none;
}
.steer-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 12px 12px;
}
.steer-row .btn {
  padding: 6px 14px;
  font-size: 12px;
}
.steer .missing {
  padding: 0 18px 12px;
}

.outcome {
  padding: 18px;
}
.summary {
  margin: 0 0 16px;
}

.tiles {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
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
.figure.model {
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--mono);
  font-size: 12px;
}
.good {
  color: var(--accent);
}

.route {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  margin: 0;
  padding: 0;
  list-style: none;
  font-size: 12px;
}
.route li {
  padding: 1px 9px;
  border: 1px solid var(--border);
  border-radius: 999px;
  color: var(--text-faint);
}
.route li.ran {
  border-color: var(--border-strong);
  color: var(--text);
}
.route li.arrow {
  padding: 0;
  border: none;
}
.reason {
  margin: 6px 0 16px;
}
.result-details {
  margin: 16px 0;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}
.result-details .route {
  margin-top: 12px;
}
.result-details .tiles {
  margin-top: 16px;
}
.compare {
  margin: 0 0 12px;
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
.stopped {
  margin: 0 0 16px;
}

.providers,
.history {
  margin: 0;
  padding: 8px 18px;
  list-style: none;
}
.providers li,
.history li {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 7px 0;
}
.providers li + li,
.history li + li {
  border-top: 1px solid var(--border);
}
.status {
  min-width: 96px;
  font-size: 12px;
  color: var(--text-dim);
}
.history .grow {
  color: var(--text);
}
.history-entry {
  display: flex;
  align-items: baseline;
  gap: 10px;
  width: 100%;
  min-width: 0;
  padding: 0;
  text-align: left;
}
.history-entry:hover .grow,
.history-entry:hover .chevron {
  color: var(--text);
}
.chevron {
  color: var(--text-faint);
}
.detail-state {
  margin: 10px 0 0;
}
.history-detail {
  margin-top: 10px;
  padding: 16px 18px;
}
.detail-heading {
  display: flex;
  justify-content: space-between;
  gap: 10px;
}
.detail-prompt {
  margin: 10px 0 12px;
  color: var(--text);
}
.detail-metrics {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 12px;
  margin: 0 0 14px;
  font-size: 12px;
  color: var(--text-faint);
}
.patch-view,
.event-log {
  margin-top: 16px;
  border-top: 1px solid var(--border);
  padding-top: 12px;
}
.patch-view summary,
.event-log summary {
  cursor: pointer;
  color: var(--text-dim);
  font-size: 12px;
}
.patch-view pre,
.event-log pre {
  max-height: 360px;
  margin: 10px 0 0;
  padding: 12px;
  overflow: auto;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  color: var(--text-dim);
  font-family: var(--mono);
  font-size: 12px;
  line-height: 1.45;
  white-space: pre;
}
.event-log ol {
  margin: 10px 0 0;
  padding: 0;
  list-style: none;
}
.event-log li + li {
  margin-top: 10px;
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

@media (max-width: 600px) {
  .project {
    padding-top: 32px;
  }
  header {
    flex-wrap: wrap;
  }
  .controls,
  .steer-row,
  .history-entry {
    flex-wrap: wrap;
  }
  .controls .segments {
    max-width: 100%;
    overflow-x: auto;
  }
  .run,
  .stop {
    margin-left: 0;
  }
  .run {
    margin-left: auto;
  }
  .tiles {
    grid-template-columns: repeat(2, 1fr);
  }
  .status {
    min-width: 0;
  }
}
</style>
