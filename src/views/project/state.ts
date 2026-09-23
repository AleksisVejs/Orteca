// Everything the Project screen knows and does. The pages in this folder only
// render it, and share one instance through provide/inject.
import { computed, nextTick, onMounted, onUnmounted, reactive, ref } from "vue";
import type { InjectionKey, Ref } from "vue";
import { reference, tidy, forRouting } from "./picks";
import { verificationSummary } from "./taskPresentation";
import type { Pick } from "./picks";
import {
  addMemory,
  pickAttachments,
  savePastedImage,
  cancelProviderOperation,
  cancelTask,
  detectProviders,
  getTaskDetail,
  gitAction,
  draftCommitMessage,
  gitStatus,
  installProvider,
  isAppError,
  onFileDrop,
  onInstallEvent,
  onSignInEvent,
  openFile,
  previewTask,
  providerLimits,
  recentTasks,
  removeWorktree,
  renameTask,
  deleteTask,
  sendInstruction,
  answerWait,
  signInProvider,
  startTask,
} from "../../api";
import type {
  Auth,
  Detected,
  FileEdit,
  GitAction,
  GitState,
  Isolation,
  LimitWindow,
  Limits,
  ModelOverride,
  Mode,
  OpenedProject,
  ProviderEvent,
  ProviderId,
  Preflight,
  Resume,
  FileStat,
  Route,
  TaskDetail,
  TaskEvent,
  TaskResult,
  TaskSummary,
} from "../../types";

/** One finished exchange: what the user said, and what came back. One that
 *  finished on screen keeps its whole result and stream, so its proof stays its
 *  own after a reply; one read back from the database has only the words.
 *  `files` counts only what the run itself changed. */
export type Turn = {
  said: string;
  summary: string | null;
  failure: string | null;
  status?: TaskSummary["status"];
  files?: number;
  result?: TaskResult;
  stream?: ActivityLine[];
};

/** Everything one exchange's answer and its proof show, live or from history. */
export type Exchange = {
  status: string;
  failure: string | null;
  summary: string | null;
  unknownEvents: number;
  changed: { byRun: FileStat[]; beforeRun: FileStat[]; unknown: boolean };
  patchText: string | null;
  verification: string;
  route: Route | null;
  routeSteps: Array<{ stage: string; ran: boolean; asked?: string | null }>;
  durationMs: number | null;
  metrics: {
    callsUsed: number | null;
    turns: number | null;
    ran: string[];
    tokens: { total: number; uncached: number; cached: number; cacheHit: number | null } | null;
    cost: number | null;
    costQuality: string | null;
    model: string | null;
    effort: string | null;
  };
  /** What was said on the way: the agent's messages and the user's steers. */
  messages: ActivityLine[];
};

/** Share of input read from the provider's cache, as a whole percent. A low
 *  figure on a resumed or long run means the context is being billed again.
 *  null when there was no input to share out. */
export function cacheHit(uncachedInput: number, cachedInput: number): number | null {
  const input = uncachedInput + cachedInput;
  return input > 0 ? Math.round((cachedInput / input) * 100) : null;
}

/** Never a number without a label, and never a zero standing in for unknown. */
function tokensOf(r: TaskResult | null) {
  const usage = r?.usage;
  if (!usage) return null;
  return {
    total: usage.inputTokens + usage.cachedInputTokens + usage.outputTokens,
    uncached: usage.inputTokens + usage.outputTokens,
    cached: usage.cachedInputTokens,
    cacheHit: cacheHit(usage.inputTokens, usage.cachedInputTokens),
    output: usage.outputTokens,
    cost: usage.costUsd,
    quality: usage.costQuality,
    model: usage.model,
    // The last stage a model ran; the reported model is the last one too.
    effort: [...(r?.stages ?? [])].reverse().find((s) => s.effort)?.effort ?? null,
  };
}

/** Every stage that ran, then the route's stages that did not. Stages run in
 *  order, so the ones that ran are the front of the route - unless a Fix ran,
 *  after which the run's own order is the whole story. */
function routeStepsOf(r: TaskResult | null) {
  if (!r) return [];
  const ran = r.stages.map((s) => ({
    stage: s.stage,
    ran: true,
    asked: s.model && s.effort ? `${s.model}, ${s.effort}` : null,
  }));
  if (ran.some((s) => s.stage === "fix")) return ran;
  return [...ran, ...r.route.stages.slice(ran.length).map((stage) => ({ stage, ran: false, asked: null }))];
}

/** The diff split by whose change it is. A file untouched since before the
 *  run is the user's, and is neither listed nor counted as this run's work. */
function changedOf(r: TaskResult | null) {
  const diff = r?.diff ?? [];
  return {
    byRun: diff.filter((f) => f.origin !== "beforeRun"),
    beforeRun: diff.filter((f) => f.origin === "beforeRun"),
    unknown: !!r?.dirtyAtStart && diff.some((f) => f.origin === null),
  };
}

/** A saved task's log cut into its exchanges: the first request, then one per
 *  reply, each with its own events and the result the run logged when it
 *  ended. One logged before results were kept per reply has `result: null`. */
export function splitExchanges(d: TaskDetail) {
  const parts: Array<{ said: string; events: TaskEvent[]; result: TaskResult | null }> = [
    { said: tidy(d.prompt), events: [], result: null },
  ];
  for (const e of d.events) {
    const last = parts[parts.length - 1]!;
    if (e.kind === "turn") {
      const data = (e.payload as { data?: { said?: string | null; prompt?: string } } | null)?.data;
      // An older turn logged only the recap sent to the CLI; the user's words are its "My reply".
      const recap = data?.prompt ?? "";
      const said = data?.said ?? recap.match(/My reply:\n([\s\S]*?)(?:\n\nFiles changed so far: [^\n]*)?$/)?.[1] ?? recap;
      parts.push({ said: tidy(said), events: [], result: null });
    } else if (e.kind === "exchange") last.result = e.payload as TaskResult;
    else last.events.push(e);
  }
  return parts;
}

/** One live result, and the stream it came with, as the chat shows it. */
export function exchangeOf(r: TaskResult, stream: ActivityLine[]): Exchange {
  const t = tokensOf(r);
  return {
    status: r.status,
    failure: r.failure,
    summary: r.summary,
    unknownEvents: r.unknownEvents,
    changed: changedOf(r),
    patchText: r.patchText,
    verification: verificationSummary(r.stages),
    route: r.route,
    routeSteps: routeStepsOf(r),
    durationMs: r.durationMs,
    metrics: {
      callsUsed: r.callsUsed,
      turns: r.turnsUsed ?? null,
      ran: r.stages.map((s) => s.stage),
      tokens: t,
      cost: t?.cost ?? null,
      costQuality: t?.quality ?? null,
      model: t?.model ?? null,
      effort: t?.effort ?? null,
    },
    messages: storyOf(stream),
  };
}

/** The lines that tell how the agent got there, as the chat shows them. */
export const storyOf = (lines: ActivityLine[]) => lines.filter((l) => ["text", "instruction", "thinking", "toolUse"].includes(l.kind));

/** A live update; `file` is the full path it is about, shown by name and openable. */
export type Activity = { text: string; file: string | null; id?: string; changes?: FileEdit[]; failed?: boolean };
export type ActivityLine = Omit<Activity, "file"> & { kind: string; file?: string | null; delivery?: string };

const EDIT_RE = /edit|write|patch|create|delete|move|rename|file_change|set-content|out-file|new-item|remove-item/;
const READ_RE = /read|get-content|cat|head|tail|grep|glob|rg|find|list|search|inspect/;
const WEB_RE = /^web(search|fetch)$/i;

// Several projects are open at once, each with its own copy of the workspace in
// the document. Element ids have to be unique per project: a popover is targeted
// by id, and two panels called "project-git" would mean every button opened the
// first one.
let instances = 0;

/** `active` is whether this project is the one on screen. A hidden project keeps
 *  its runs, its terminals and its state; it just stops answering the window. */
export function useProject(opened: OpenedProject, active: Ref<boolean> = ref(true)) {
  const uid = `p${++instances}`;
  const domId = (name: string) => `${name}-${uid}`;
  const task = ref("");

  // Absolute paths the run is told about. Kept after a run, like the prompt.
  const attachments = ref<string[]>([]);
  const attachError = ref<string | null>(null);
  const dragging = ref(false);
  // Elements pointed at in the preview tab. They ride in front of `task` when a
  // run starts, and show as chips instead of the block of text they are.
  const picks = ref<Pick[]>([]);
  const promptText = () => [...picks.value.map((p) => p.block), task.value.trim()].filter(Boolean).join("\n\n");

  function attach(paths: string[]) {
    attachments.value = [...new Set([...attachments.value, ...paths])];
  }

  async function addAttachments(directory: boolean) {
    attach(await pickAttachments(directory));
  }

  async function pasteImages(e: ClipboardEvent) {
    const images = Array.from(e.clipboardData?.files ?? []).filter((f) => f.type.startsWith("image/"));
    if (images.length === 0) return;
    e.preventDefault();
    attachError.value = null;
    try {
      for (const image of images) {
        attach([await savePastedImage(image, image.type.split("/")[1] ?? "png")]);
      }
    } catch (err) {
      attachError.value = isAppError(err) ? err.message : String(err);
    }
  }

  function fileName(path: string): string {
    return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
  }

  // Detected live on every open: a CLI can be installed or signed in behind us.
  // A missing CLI is shown, not thrown - the app is useful with neither present.
  const providers = ref<Detected[]>([]);
  const providerError = ref(false);
  const provider = ref<ProviderId>("codex");

  const MODELS: Record<ProviderId, Array<{ id: string; label: string; efforts: string[] }>> = {
    claude: [
      { id: "sonnet", label: "Sonnet", efforts: ["low", "medium", "high", "xhigh", "max"] },
      { id: "opus", label: "Opus", efforts: ["low", "medium", "high", "xhigh", "max"] },
      { id: "fable", label: "Fable", efforts: ["low", "medium", "high", "xhigh", "max"] },
      { id: "haiku", label: "Haiku", efforts: ["low", "medium", "high"] },
    ],
    codex: [
      { id: "gpt-6-astra", label: "GPT-6 Astra", efforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
      { id: "gpt-6-sol", label: "GPT-6 Sol", efforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
      { id: "gpt-6-luna", label: "GPT-6 Luna", efforts: ["low", "medium", "high", "xhigh", "max"] },
      { id: "gpt-5.6-sol", label: "GPT-5.6 Sol", efforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
      { id: "gpt-5.6-terra", label: "GPT-5.6 Terra", efforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
      { id: "gpt-5.6-luna", label: "GPT-5.6 Luna", efforts: ["low", "medium", "high", "xhigh", "max"] },
      { id: "gpt-5.5", label: "GPT-5.5", efforts: ["low", "medium", "high", "xhigh"] },
    ],
  };
  const modelChoices = ref<Record<ProviderId, ModelOverride>>({
    claude: { model: "sonnet", effort: "high" },
    codex: { model: "gpt-6-sol", effort: "medium" },
  });

  // How readily the classifier takes the shorter route. Two modes, not three.
  // The route itself is decided in Rust before any CLI starts and costs nothing.
  const mode = ref<Mode>("balanced");
  const MODES: Array<{ id: Mode; label: string; hint: string }> = [
    { id: "balanced", label: "Careful", hint: "Let Orteca plan, build, and check the work" },
    { id: "efficient", label: "Quick", hint: "Take the shortest safe path" },
  ];

  // Where the agent works. A copy is a git worktree beside the repository on a
  // branch of its own; this folder is not touched until the user merges it.
  const isolation = ref<Isolation>("currentTree");
  const ISOLATIONS: Array<{ id: Isolation; label: string; hint: string }> = [
    { id: "currentTree", label: "This folder", hint: "Change the files you have open" },
    { id: "worktree", label: "Separate copy", hint: "Work in a copy on a new branch and leave this folder alone" },
  ];

  // A slow command the agent hands over (`ORTECA-WAIT:`): run it at once, or
  // ask first. Remembered, since it is a standing choice about trust.
  const WAIT_KEY = "orteca.autoWait";
  const autoWait = ref(false);
  try { autoWait.value = localStorage.getItem(WAIT_KEY) === "1"; } catch { /* no storage: ask first */ }
  function chooseAutoWait(on: boolean) {
    autoWait.value = on;
    try { localStorage.setItem(WAIT_KEY, on ? "1" : "0"); } catch { /* kept for this session */ }
  }

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
    if (installing.value !== null || anyRunning.value) return;
    installError.value = null;
    for (const id of ids) {
      installing.value = id;
      installLine.value = "asking npm…";
      try {
        const fresh = await installProvider(id);
        providers.value = providers.value.map((p) => (p.id === id ? fresh : p));
        provider.value = id;
        void loadLimits(true);
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
    if (signingIn.value !== null || installing.value !== null || anyRunning.value) return;
    signInError.value = null;
    signingIn.value = id;
    signInLine.value = "starting sign-in…";
    try {
      const fresh = await signInProvider(id);
      providers.value = providers.value.map((p) => (p.id === id ? fresh : p));
      // A signed-out CLI reports no limits, so the reading taken at open is stale
      // the moment a sign-in succeeds. Not awaited: it takes seconds.
      void loadLimits(true);
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
  const limitsLoading = ref(false);
  const limitsError = ref<string | null>(null);
  const limitsCheckedAt = ref<number | null>(null);
  let limitsRequest = 0;
  let limitsTimer: ReturnType<typeof setInterval> | null = null;
  // Once the user picks a provider, headroom stops choosing for them.
  const providerPicked = ref(false);
  const pickedFor = ref<string | null>(null);

  async function loadLimits(fresh = false) {
    const request = ++limitsRequest;
    limitsLoading.value = true;
    try {
      const reading = await providerLimits(fresh);
      if (request !== limitsRequest) return;
      limits.value = reading;
      limitsError.value = null;
    } catch (e) {
      if (request !== limitsRequest) return;
      limits.value = [];
      limitsError.value = isAppError(e) ? e.message : String(e);
    } finally {
      if (request === limitsRequest) {
        limitsLoading.value = false;
        limitsCheckedAt.value = Date.now();
      }
    }
    pickByHeadroom();
  }

  /** The room left in this provider's tightest window, or null with no reading. */
  function headroom(id: ProviderId): number | null {
    const windows = limits.value.find((l) => l.id === id)?.windows ?? [];
    return windows.length && windows.every((w) => Number.isFinite(w.usedPercent))
      ? Math.min(...windows.map((w) => Math.max(0, Math.min(100, 100 - w.usedPercent))))
      : null;
  }

  /** Move to the runnable provider with the most room left. Only when every
   *  runnable one has a reading: an unread limit is not an empty one. */
  function pickByHeadroom() {
    if (providerPicked.value || anyRunning.value) return;
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

  function formatWhen(ms: number): string | null {
    const date = new Date(ms);
    return Number.isFinite(date.getTime())
      ? date.toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })
      : null;
  }

  function resetWhen(w: LimitWindow): string | null {
    if (w.resetsText) return w.resetsText;
    return w.resetsAt === null ? null : formatWhen(w.resetsAt * 1000);
  }

  const usageCounters = computed(() => rows.value.map((p) => {
    const reading = limits.value.find((l) => l.id === p.id);
    const status = p.pending
      ? providerError.value ? "Unavailable" : "Checking helper…"
      : !p.path ? "Not installed"
      : p.auth === "signedOut" ? "Sign in required"
      : !reading && limitsLoading.value ? "Checking usage…"
      : !reading?.windows.length ? "Unavailable" : null;
    const windows = status ? [] : reading!.windows.map((w) => {
      const left = Number.isFinite(w.usedPercent) ? Math.max(0, Math.min(100, 100 - w.usedPercent)) : null;
      return {
        label: w.label,
        shortLabel: w.label === "5-hour" ? "5h" : w.label === "week (all models)" ? "week" : w.label,
        left,
        leftLabel: left === null ? "-" : left > 0 && left < 1 ? "<1%" : `${Math.floor(left)}%`,
        resets: resetWhen(w),
      };
    });
    return {
      id: p.id,
      name: p.id === "codex" ? "Codex" : "Claude",
      status,
      windows,
      reason: limitsError.value ?? reading?.unavailable ?? "No plan usage reading is available yet.",
    };
  }));

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

  /** The fullest window that may not cover this route's stages. A check that
   *  fails adds a fix and the check again on top. */
  const limitWarning = computed(() => {
    const p = preview.value;
    if (!p) return null;
    const calls = p.route.stages.length;
    const all = tightWindows(p.provider, calls);
    const tight = [...all].sort((a, b) => b.usedPercent - a.usedPercent)[0];
    return tight ? { window: tight, calls, resets: resetWhen(tight), startsAt: resetsAfter(all) } : null;
  });

  function tightWindows(id: ProviderId, calls: number): LimitWindow[] {
    return (limits.value.find((l) => l.id === id)?.windows ?? [])
      .filter((w) => 100 - w.usedPercent < calls * PERCENT_PER_CALL);
  }

  /** When every one of these windows has reset, in Unix ms. Null when one
   *  gives no time: Claude's older text reading is words, not a clock. */
  function resetsAfter(windows: LimitWindow[]): number | null {
    return windows.length && windows.every((w) => w.resetsAt !== null)
      ? Math.max(...windows.map((w) => w.resetsAt!)) * 1000
      : null;
  }

  // One task waiting for a plan to reset. It starts on its own, with the
  // composer's mode, isolation and model at that moment.
  // ponytail: one slot, in memory only; closing the app drops it. Persist it
  // in the store if people start queueing overnight work.
  type Waiting = { prompt: string; attachments: string[]; asked: string[]; provider: ProviderId; at: number };
  const waiting = ref<Waiting | null>(null);
  let waitTimer: ReturnType<typeof setTimeout> | null = null;
  // The minute after a reset can still read as spent.
  const RESET_SLACK_MS = 60_000;

  /** Queue the composer's request, or a run the plan stopped, for `at`. */
  function waitForReset(at: number, from: LiveRun | null = null) {
    cancelWait();
    waiting.value = from
      ? { prompt: from.prompt, attachments: from.attachments, asked: from.asked, provider: from.provider, at }
      : { prompt: promptText(), attachments: attachments.value, asked: [], provider: provider.value, at };
    if (!from) {
      task.value = "";
      picks.value = [];
      attachments.value = [];
    }
    waitTimer = setTimeout(startWaiting, Math.max(0, at - Date.now()) + RESET_SLACK_MS);
  }

  function startWaiting() {
    const w = waiting.value;
    if (!w) return;
    // Git or an install busy: try again in a minute rather than drop it.
    if (!ready.value) {
      waitTimer = setTimeout(startWaiting, RESET_SLACK_MS);
      return;
    }
    waiting.value = null;
    waitTimer = null;
    provider.value = w.provider;
    providerPicked.value = true;
    pickedFor.value = null;
    void run({ prompt: w.prompt, attachments: w.attachments, asked: w.asked });
  }

  function cancelWait() {
    if (waitTimer !== null) clearTimeout(waitTimer);
    waitTimer = null;
    waiting.value = null;
  }

  /** A run its plan stopped: when it could go again on the same CLI. */
  const retryAt = computed(() => {
    const r = activeRun.value;
    // A copy's work is on its branch; a fresh run would start without it.
    if (!r || r.active || r.result?.failureKind !== "usageLimit" || r.result.worktree) return null;
    return resetsAfter(tightWindows(r.provider, r.result.route.stages.length));
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

  /** After a run that stopped because its plan ran out: the CLI that takes over.
   *  `run` continues there on its own, once. Asked of the run itself, because
   *  the composer may already be pointed at a different CLI by then. */
  function fallbackFor(r: {
    result: TaskResult | null;
    provider: ProviderId;
  }): { id: ProviderId; room: number | null } | null {
    const done = r.result;
    // A copy's work is on its branch; a fresh run would start from the commit without it.
    if (done?.status !== "failed" || done.failureKind !== "usageLimit" || done.worktree) return null;
    const other = otherThan(r.provider);
    return other && (other.room === null || other.room > 0) ? other : null;
  }

  const fallback = computed(() =>
    activeRun.value ? fallbackFor(activeRun.value) : null,
  );

  function switchTo(id: ProviderId) {
    provider.value = id;
    providerPicked.value = true;
    pickedFor.value = null;
    schedulePreview();
  }

  function chooseProvider(id: ProviderId | "auto") {
    providerPicked.value = id !== "auto";
    pickedFor.value = null;
    if (id === "auto") pickByHeadroom();
    else provider.value = id;
    schedulePreview();
  }

  function chooseModel(model: string) {
    const choice = modelChoices.value[provider.value];
    choice.model = model;
    const efforts = MODELS[provider.value].find((item) => item.id === model)?.efforts ?? [];
    if (!efforts.includes(choice.effort)) choice.effort = efforts[0] ?? "medium";
    schedulePreview();
  }

  const modelOverride = computed<ModelOverride | null>(() =>
    providerPicked.value ? { ...modelChoices.value[provider.value] } : null,
  );

  /// `signedOut` is a hard block, `unknown` is not: the CLI could not be asked,
  /// and refusing to run on a guess would be the same mistake in the other
  /// direction. The run itself reports an auth failure honestly either way.
  // "Remember: ..." saves a memory item instead of running a task. The text is
  // held here until the user says which list it goes to.
  const rememberText = ref<string | null>(null);
  const rememberError = ref<string | null>(null);
  const remembering = computed(() => /^remember:\s*\S/i.test(task.value.trim()));
  async function remember(global: boolean) {
    rememberError.value = null;
    try {
      await addMemory(opened.project.path, global, rememberText.value ?? "");
      rememberText.value = null;
      task.value = "";
    } catch (err) {
      rememberError.value = isAppError(err) ? err.message : String(err);
    }
  }

  /** Nothing in the way of starting a run, whatever the composer holds. */
  const ready = computed(
    () =>
      !gitBusy.value &&
      installing.value === null &&
      signingIn.value === null &&
      !!selected.value?.path &&
      selected.value.auth !== "signedOut",
  );
  const canRun = computed(() => ready.value && (task.value.trim().length > 0 || picks.value.length > 0));

  // Runs are independent, so each owns the whole of its screen - its stream,
  // its result, its steering box - instead of sharing one set of refs. The
  // backend already keys everything by task id; nothing here serialises them.
  type LiveRun = {
    key: symbol;
    /** The task id, from the moment the row exists until the run ends. */
    id: number | null;
    prompt: string;
    /** The paths the run was handed. */
    attachments: string[];
    provider: ProviderId;
    /** The CLI whose spent plan this run took over from, if it did. */
    switchedFrom: ProviderId | null;
    /** The user's own words across a chain of follow-ups, oldest first. */
    asked: string[];
    /** The exchanges already finished in this task, oldest first. A follow-up
     *  pushes the one it answers here rather than opening a run of its own.
     *  Only what an exchange said, so one read back from the database seeds it
     *  as easily as one that just finished on screen. */
    turns: Turn[];
    active: boolean;
    stream: ActivityLine[];
    progress?: { stages: string[]; current: number[] };
    result: TaskResult | null;
    checking: TaskResult | null;
    error: string | null;
    activity: Activity;
    stopping: boolean;
    instruction: string;
    sending: boolean;
    instructionError: string | null;
    /** The command the agent handed over, while it waits for the user's OK. */
    waitAsk?: string | null;
  };
  // Newest first. Every run still going is kept; finished ones are capped,
  // since the database holds them and the sidebar lists them from there.
  const runs = ref<LiveRun[]>([]);
  const KEPT_FINISHED = 20;
  const selectedRun = ref<symbol | null>(null);
  const activeRun = computed(() => runs.value.find((r) => r.key === selectedRun.value) ?? null);
  /** Anything going in this project. Git and installs wait on this, not on the run on screen. */
  const anyRunning = computed(() => runs.value.some((r) => r.active));
  const runningCount = computed(() => runs.value.filter((r) => r.active).length);
  const running = computed(() => !!activeRun.value?.active);
  const stream = computed(() => activeRun.value?.stream ?? []);
  const result = computed<TaskResult | null>({ get: () => activeRun.value?.result ?? null, set: (value) => { if (activeRun.value) activeRun.value.result = value; } });
  /** The change while its full suite still runs. Never a finished result. */
  const checking = computed<TaskResult | null>(() => activeRun.value?.checking ?? null);
  const runError = computed<string | null>(() => activeRun.value?.error ?? null);
  const currentActivity = computed<Activity>(() => activeRun.value?.activity ?? { text: "Getting ready", file: null });
  /** The CLI whose spent plan the run on screen took over from, if it did. */
  const switchedFrom = computed(() => activeRun.value?.switchedFrom ?? null);
  const fileError = ref<string | null>(null);

  /** A run's sidebar label: the first line of what was asked. A follow-up keeps
   *  the name of the request it continues, not the recap sent to the CLI. */
  function runLabel(r: LiveRun): string {
    const source = r.asked[0] ?? r.prompt;
    const first = source.split("\n").find((line) => line.trim())?.trim() ?? "Task";
    return first.length > 40 ? `${first.slice(0, 39).trimEnd()}…` : first;
  }

  /** What the user typed for one exchange. `prompt` is what the CLI gets, and
   *  for a follow-up that is the whole chain recapped. */
  const saidIn = (r: LiveRun) => tidy(r.asked.at(-1) ?? r.prompt);
  const said = computed(() => (activeRun.value ? saidIn(activeRun.value) : ""));

  /** Put a run on screen, or null for the composer. The warm-session clock
   *  follows the selection, since it is about the run being looked at. */
  function selectRun(key: symbol | null) {
    selectedRun.value = key;
    view.value = "task";
    if (tick !== null) clearTimeout(tick);
    tickWhileWarm();
  }

  async function openActivityFile(file: string, reveal: boolean) {
    fileError.value = null;
    try {
      await openFile(opened.project.path, file, reveal);
    } catch (e) {
      fileError.value = isAppError(e) ? e.message : String(e);
    }
  }

  // The backend sends this the moment the task row exists, which is what Stop
  // names. Until it arrives there is a run on screen that cannot yet be stopped,
  // so the button is disabled rather than lying about what it would do.
  const taskId = computed(() => activeRun.value?.id ?? null);
  const stopping = computed(() => !!activeRun.value?.stopping);

  // A mid-task instruction. Where it lands is the provider's business, and the
  // card says which before the user types rather than after they have sent it.
  // The run on screen decides that, not the composer's current pick.
  const instruction = computed<string>({ get: () => activeRun.value?.instruction ?? "", set: (value) => { if (activeRun.value) activeRun.value.instruction = value; } });
  const sending = computed(() => !!activeRun.value?.sending);
  const instructionError = computed(() => activeRun.value?.instructionError ?? null);
  const steering = computed(
    () => providers.value.find((p) => p.id === (activeRun.value?.provider ?? provider.value))?.steering ?? "checkpoint",
  );

  async function instruct(applyNow: boolean) {
    const text = instruction.value.trim();
    if (!running.value || taskId.value === null || sending.value || !text) return;
    const live = activeRun.value!;
    live.sending = true;
    live.instructionError = null;
    try {
      // The steer box shares the reply box's attachments: only one of them is ever on screen.
      // ponytail: shared across runs too, so an unsent one shows on another run's steer box; per-run if that confuses.
      const attached = attachments.value;
      const receipt = await sendInstruction(taskId.value, text, applyNow, attached);
      if (receipt.disposition === "tooLate") {
        live.instructionError = "The run finished before it could take that instruction.";
        return;
      }
      // Shown as the user's own words. Never pushed through `describe`, which
      // would file them among the things the agent said.
      const delivery = receipt.disposition === "held" ? "Queued for next step"
        : receipt.disposition === "resumed" ? "Applied · step restarted" : "Delivered to the running agent";
      const count = attached.length ? ` · ${attached.length} attached` : "";
      live.stream.push({ kind: "instruction", text, file: null, delivery: delivery + count });
      live.instruction = "";
      attachments.value = attachments.value.filter((p) => !attached.includes(p));
    } catch (e) {
      live.instructionError = isAppError(e) ? e.message : String(e);
    } finally {
      live.sending = false;
    }
  }

  /** The command the run on screen is waiting on the user's OK for. */
  const waitAsk = computed(() => activeRun.value?.waitAsk ?? null);
  async function answerWaitFor(run: boolean) {
    const live = activeRun.value;
    if (!live?.waitAsk || live.id === null) return;
    live.waitAsk = null;
    try {
      await answerWait(live.id, run);
    } catch (e) {
      live.instructionError = isAppError(e) ? e.message : String(e);
    }
  }

  async function stopRun() {
    if (!running.value || taskId.value === null || stopping.value) return;
    activeRun.value!.stopping = true;
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
      history.value = await recentTasks(opened.project.path);
      historyError.value = false;
    } catch {
      historyError.value = true;
    }
  }

  async function openHistory(run: TaskSummary) {
    const request = ++historyRequest;
    historyDetailLoading.value = true;
    historyDetailError.value = false;
    try {
      const detail = await getTaskDetail(opened.project.path, run.id);
      if (request === historyRequest) historyDetail.value = detail;
    } catch {
      if (request === historyRequest) historyDetailError.value = true;
    } finally {
      if (request === historyRequest) historyDetailLoading.value = false;
    }
  }

  // Removing a copy deletes a folder, so it takes a second click. The branch stays.
  const confirmRemove = ref<number | null>(null);
  const removedCopies = ref<number[]>([]);
  const removeError = ref<string | null>(null);

  async function removeCopy(id: number) {
    if (confirmRemove.value !== id) {
      confirmRemove.value = id;
      removeError.value = null;
      return;
    }
    try {
      await removeWorktree(opened.project.path, id);
      removedCopies.value = [...removedCopies.value, id];
    } catch (e) {
      removeError.value = isAppError(e) ? e.message : String(e);
    } finally {
      confirmRemove.value = null;
    }
  }

  // A task goes by its title, or its prompt cut to the title limit until it has one.
  const taskName = (t: TaskSummary) =>
    t.title || (t.prompt.length > 60 ? `${t.prompt.slice(0, 59).trimEnd()}…` : t.prompt);
  const historyRow = computed(() => history.value.find((t) => t.id === historyDetail.value?.id) ?? null);

  // The sidebar row whose menu is open, the row being renamed, the task asked about.
  const taskMenu = ref<number | null>(null);
  const renameId = ref<number | null>(null);
  const renaming = ref("");
  const deleteAsk = ref<TaskSummary | null>(null);
  const taskError = ref<string | null>(null);

  function startRename(t: TaskSummary) {
    taskMenu.value = null;
    renameId.value = t.id;
    renaming.value = t.title || t.prompt.slice(0, 60);
    taskError.value = null;
  }

  async function saveRename() {
    const row = history.value.find((t) => t.id === renameId.value);
    const title = renaming.value.trim();
    renameId.value = null;
    if (!row || !title || title === row.title) return;
    try {
      await renameTask(opened.project.path, row.id, title);
      row.title = title;
    } catch (e) {
      taskError.value = isAppError(e) ? e.message : String(e);
    }
  }

  function askDelete(t: TaskSummary) {
    taskMenu.value = null;
    deleteAsk.value = t;
    taskError.value = null;
  }

  // Deleting drops the task's log and numbers for good, so it asks first.
  async function removeTask() {
    const id = deleteAsk.value?.id;
    deleteAsk.value = null;
    if (id === undefined) return;
    try {
      await deleteTask(opened.project.path, id);
      history.value = history.value.filter((t) => t.id !== id);
      if (historyDetail.value?.id === id) {
        historyDetail.value = null;
        newTask();
      }
    } catch (e) {
      taskError.value = isAppError(e) ? e.message : String(e);
    }
  }

  // Fetch is immediate. Actions that change files or publish work are reviewed
  // in the same panel before submission; opening a form never executes it.
  const git = ref<GitState>(opened.git);
  const gitOpen = ref(false);
  const gitAsk = ref<GitAction | null>(null);
  const gitBusy = ref<GitAction | null>(null);
  const gitLoading = ref(false);
  const gitRefreshError = ref<string | null>(null);
  const gitError = ref<string | null>(null);
  const gitNotice = ref<string | null>(null);
  const commitMessage = ref("");
  const drafting = ref(false);
  /** The existing branch a merge or switch names. */
  const targetBranch = ref("");
  const newBranch = ref("");
  const discardPath = ref("");
  let gitRequest = 0;

  async function refreshGit() {
    if (anyRunning.value || gitBusy.value) return;
    const request = ++gitRequest;
    gitLoading.value = true;
    gitRefreshError.value = null;
    try {
      const current = await gitStatus(opened.project.path);
      if (request === gitRequest) git.value = current;
    } catch (e) {
      if (request === gitRequest) gitRefreshError.value = isAppError(e) ? e.message : String(e);
    } finally {
      if (request === gitRequest) {
        ++gitRequest;
        gitLoading.value = false;
      }
    }
  }

  function gitDisabledReason(action: GitAction): string {
    const g = git.value;
    if (anyRunning.value) return runningCount.value > 1 ? "Available when the tasks finish." : "Available when the task finishes.";
    if (gitBusy.value) return "Wait for the current Git action to finish.";
    if (gitLoading.value) return "Refreshing Git status…";
    if (!g.isRepo) return "This folder is no longer a Git repository.";
    if (action === "commit" && !g.dirty) return "No changes to commit.";
    if (action === "discard" && !g.dirty) return "No changes to revert.";
    if (action === "pull" && !g.upstream) return "Publish this branch before pulling.";
    if (action === "pull" && g.behind === 0) return "No incoming commits at the last fetch.";
    if (action === "push" && !g.branch) return "Check out a branch before pushing.";
    if (action === "push" && g.ahead === 0) return "No outgoing commits.";
    if (action === "merge" && g.dirty) return "Commit your changes before merging.";
    if (action === "merge" && !g.branches.length) return "No other local branches to merge.";
    return "";
  }

  function openGit(action: GitAction | null = null, branch = "") {
    gitOpen.value = true;
    if (gitBusy.value || anyRunning.value) return;
    gitAsk.value = action;
    gitError.value = null;
    gitNotice.value = null;
    if (action === "merge" || action === "switch") targetBranch.value = branch || git.value.branches[0] || "";
    if (action === "discard") discardPath.value = branch;
  }

  function gitQuestion(action: GitAction): string {
    const g = git.value;
    const remote = g.upstream ?? "the remote";
    const n = (count: number | null, word: string) => count === null ? `${word}s` : `${count} ${word}${count === 1 ? "" : "s"}`;
    switch (action) {
      case "fetch":
        return `Download what's new on ${remote}? Your files stay as they are.`;
      case "pull":
        return `Bring ${n(g.behind, "commit")} from ${remote} into your files? Git stops if that needs a merge.`;
      case "commit":
        return `Commit all ${n(g.dirtyCount, "changed file")} on ${g.branch ?? "this detached HEAD"}?`;
      case "push":
        return g.upstream
          ? `Send ${n(g.ahead, "commit")} to ${remote}? Others will see them.`
          : `Publish ${g.branch ?? "this branch"} to the remote for the first time? Others will see it.`;
      case "merge":
        return `Bring ${targetBranch.value || "a branch"} into ${g.branch ?? "this detached HEAD"}? If they change the same lines, Orteca stops and changes nothing.`;
      case "switch":
        return `Check out ${targetBranch.value || "a branch"}? ${g.dirty ? "Uncommitted changes come along; Git stops if they would be overwritten." : "Your files change to match it."}`;
      case "branch":
        return `Start a new branch from ${g.branch ?? "this detached HEAD"} and switch to it?${g.dirty ? " Uncommitted changes come along." : ""}`;
      case "discard":
        return discardPath.value === "" ? `Revert all ${n(g.dirtyCount, "changed file")}? This permanently removes uncommitted changes.` : `Revert ${discardPath.value}? This permanently removes its uncommitted changes.`;
    }
  }

  async function draftCommit() {
    if (drafting.value || gitBusy.value) return;
    drafting.value = true;
    gitError.value = null;
    try {
      commitMessage.value = await draftCommitMessage(opened.project.path);
    } catch (e) {
      gitError.value = isAppError(e) ? e.message : String(e);
    } finally {
      drafting.value = false;
    }
  }

  async function runGit(action: GitAction) {
    if (gitDisabledReason(action) || (action !== "fetch" && gitAsk.value !== action)) return;
    const input = action === "merge" || action === "switch" ? targetBranch.value
      : action === "commit" ? commitMessage.value.trim()
      : action === "branch" ? newBranch.value.trim()
      : action === "discard" ? discardPath.value : "";
    if ((action === "commit" || action === "branch") && !input) return;
    if ((action === "merge" || action === "switch") && !git.value.branches.includes(input)) return;
    ++gitRequest;
    gitBusy.value = action;
    gitError.value = null;
    gitNotice.value = null;
    try {
      git.value = await gitAction(opened.project.path, action, input);
      if (action === "commit") commitMessage.value = "";
      if (action === "branch") newBranch.value = "";
      if (action !== "fetch") gitAsk.value = null;
      gitNotice.value = {
        fetch: "Fetched. Remote status is up to date.",
        pull: "Pulled. Your files are up to date.",
        commit: "Changes committed.",
        push: "Pushed to the remote.",
        merge: `Merged ${input}.`,
        discard: input ? `Reverted ${input}.` : "Reverted all uncommitted changes.",
        switch: `Switched to ${input}.`,
        branch: `Created and switched to ${input}.`,
      }[action];
    } catch (e) {
      gitError.value = isAppError(e) ? e.message : String(e);
    } finally {
      ++gitRequest;
      gitBusy.value = null;
      // A rejected hook or pull can still have changed repository state.
      if (gitError.value) await refreshGit();
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
    if (!task.value.trim() || !selected.value?.path) {
      preview.value = null;
      previewError.value = null;
      return;
    }
    previewTimer = setTimeout(refreshPreview, 600);
  }

  async function refreshPreview() {
    const request = ++previewRequest;
    const gitVersion = gitRequest;
    const chosen = selected.value;
    if (!chosen?.path || !task.value.trim()) return;
    previewing.value = true;
    previewError.value = null;
    try {
      const planned = await previewTask(opened.project.path, task.value, chosen.id, mode.value, headroom(chosen.id), isolation.value, modelOverride.value);
      if (request === previewRequest) {
        preview.value = planned;
        if (gitVersion === gitRequest && !gitBusy.value && !gitLoading.value && !anyRunning.value) git.value = planned.git;
      }
    } catch (e) {
      if (request === previewRequest) previewError.value = isAppError(e) ? e.message : String(e);
    } finally {
      if (request === previewRequest) previewing.value = false;
    }
  }

  onMounted(async () => {
    void loadHistory();
    void loadLimits();
    limitsTimer = setInterval(() => {
      // A reading costs a CLI start each. Only the project on screen asks.
      if (!limitsLoading.value && active.value) void loadLimits();
    }, 60_000);
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
      onFileDrop((over, paths) => {
        // One window, one drop target: whichever project is on screen takes it.
        if (!active.value) return;
        dragging.value = over;
        attach(paths);
      }),
      onInstallEvent((id, line) => {
        if (installing.value === id) installLine.value = line;
      }),
      onSignInEvent((id, line) => {
        if (signingIn.value === id) signInLine.value = line;
      }),
    ]);
  });

  onUnmounted(() => {
    ++gitRequest;
    ++limitsRequest;
    if (limitsTimer !== null) clearInterval(limitsTimer);
    if (waitTimer !== null) clearTimeout(waitTimer);
    if (previewTimer !== null) clearTimeout(previewTimer);
    stop.forEach((off) => off());
  });

  /** Everything one `run` carries over from the call that asked for it.
   *  Passed rather than stashed, so two runs started at once cannot mix. */
  type RunOpts = {
    resume?: (Resume & { reply: string }) | null;
    /** The task a follow-up goes on in, when it goes on in one at all. */
    continueTask?: number | null;
    /** The words already asked in this chain; empty for a fresh request. */
    asked?: string[];
    /** The spent plan this run is taking over from, if it is. */
    switchedFrom?: ProviderId | null;
    /** The run this one carries on in, keeping its row and its exchanges. */
    continueRun?: LiveRun | null;
    /** The paths already taken off the composer, for a handoff that re-sends them. */
    attachments?: string[];
    /** Exchanges to open the new run with, when it carries on a task whose own
     *  run is no longer on screen. */
    turns?: Turn[];
    /** The request itself, when it is not the composer's: a handoff or a run
     *  that waited for a reset. The composer is left as the user has it. */
    prompt?: string;
  };

  async function run(opts: RunOpts = {}) {
    if (!(opts.prompt === undefined ? canRun.value : ready.value)) return;
    if (!opts.continueRun && opts.prompt === undefined && remembering.value) {
      rememberText.value = task.value.trim().replace(/^remember:\s*/i, "");
      return;
    }
    // The composer empties on send, the way the chain of replies below it does.
    // A handoff re-sends what the first attempt was given rather than nothing.
    const attached = opts.attachments ?? attachments.value;
    if (!opts.attachments) attachments.value = [];
    const sent = opts.prompt ?? promptText();
    if (opts.prompt === undefined) picks.value = [];
    const carry = opts.continueRun ?? null;
    // Reactive up front: the callbacks below write through this object for as
    // long as the run lasts, and a plain one would leave the screen frozen on
    // whatever it happened to show first.
    const live: LiveRun = carry ?? reactive({
      key: Symbol("run"),
      id: null,
      prompt: sent,
      attachments: attached,
      provider: provider.value,
      switchedFrom: opts.switchedFrom ?? null,
      asked: opts.asked ?? [],
      turns: opts.turns ?? [],
      active: true,
      stream: [],
      result: null,
      checking: null,
      error: null,
      activity: { text: "Getting ready", file: null },
      stopping: false,
      instruction: "",
      sending: false,
      instructionError: null,
    });
    if (carry) {
      // The exchange it answers stays on the page, above the new one.
      if (carry.result) {
        const r = carry.result;
        carry.turns.push({
          said: saidIn(carry), summary: r.summary, failure: r.failure, status: r.status,
          files: changedOf(r).byRun.length, result: r, stream: carry.stream,
        });
      }
      Object.assign(carry, {
        id: null,
        prompt: sent,
        attachments: attached,
        provider: provider.value,
        switchedFrom: opts.switchedFrom ?? null,
        asked: opts.asked ?? [],
        active: true,
        stream: [],
        progress: undefined,
        result: null,
        checking: null,
        error: null,
        activity: { text: "Getting ready", file: null },
        stopping: false,
        waitAsk: null,
      });
    } else {
      runs.value = [live, ...runs.value].filter((r, i) => r.active || i < KEPT_FINISHED);
    }
    selectedRun.value = live.key;
    ++gitRequest;
    gitLoading.value = false;
    gitAsk.value = null;
    view.value = "task";
    try {
      live.result = await startTask(
        opened.project.path,
        live.prompt,
        live.provider,
        mode.value,
        // The same reading the preview was routed on, so the run matches it.
        headroom(live.provider),
        isolation.value,
        attached,
        modelOverride.value,
        (event) => {
          if (event.kind === "stageProgress") {
            live.progress = event.data;
            live.activity = { text: event.data.current.map((i) => stageLabel(event.data.stages[i] ?? "Working")).join(" + "), file: null };
            return;
          }
          if (event.kind === "wait") live.waitAsk = event.data.asking ? event.data.command : null;
          const activity = activityFor(event);
          if (activity !== null) live.activity = activity;
          appendActivity(live.stream, event);
          if (event.kind === "toolResult" && live.activity.id === event.data.id) {
            const updated = [...live.stream].reverse().find((line) => line.id === event.data.id);
            if (updated) live.activity = { ...updated, file: updated.file ?? null };
          }
          if (live.stream.length > 500) live.stream.shift();
        },
        (id) => {
          live.id = id;
          // The sidebar lists runs from the database, so the new row shows now, not at the end.
          void loadHistory();
        },
        (early) => {
          live.checking = early;
        },
        autoWait.value,
        opts.resume ?? null,
        opts.continueTask ?? null,
        // A follow-up's prompt carries the whole exchange so the agent has the
        // context, but the route must be chosen from what the user just said:
        // read the blob, a classifier answers about the task at the top instead
        // of the reply at the bottom. An earlier task handed in goes as its title alone.
        opts.asked?.at(-1) ?? (forRouting(live.prompt) === live.prompt ? null : forRouting(live.prompt)),
      );
    } catch (e) {
      live.error = isAppError(e) ? e.message : String(e);
    } finally {
      live.active = false;
      live.stopping = false;
      live.waitAsk = null;
      live.checking = null;
      live.id = null;
      if (selectedRun.value === live.key) {
        if (tick !== null) clearTimeout(tick);
        tickWhileWarm();
        // A steer still being typed when the run ended becomes the reply, not lost.
        if (live.instruction.trim() && !reply.value.trim()) reply.value = live.instruction;
      }
      live.instruction = "";
      await loadHistory();
      void refreshGit();
      // The run just spent some of a limit; the next pick should know.
      void loadLimits();
    }
    // Out of plan usage: the other CLI carries on, once, so two spent plans cannot bounce.
    const next = opts.switchedFrom ? null : fallbackFor(live);
    if (next) {
      // The same request on the other CLI. What the stopped run changed is still on disk.
      provider.value = next.id;
      providerPicked.value = true;
      pickedFor.value = null;
      // The handoff is the same request carrying on: it keeps the reply's task.
      await run({ ...opts, prompt: live.prompt, attachments: attached, switchedFrom: live.provider });
    }
  }

  /** Short, human words for the things a provider does behind the scenes. */
  function actionTarget(summary: string): string {
    const target = summary.replace(/\s+/g, " ").trim();
    return target.length > 64 ? target.slice(0, 61) + "…" : target;
  }

  /** Codex wraps every command as `"...\powershell.exe" -Command "..."`; show the inner command. */
  function unwrapShell(summary: string): string {
    const m = summary.match(/^\s*"?[^"]*?\b(?:powershell|pwsh|cmd|bash|sh)(?:\.exe)?"?((?:\s+[-/]\w+)*)\s+([\s\S]*)$/i);
    if (!m?.[1] || m[2] === undefined) return summary;
    return m[2].trim().replace(/^(["'])([\s\S]*)\1$/, "$2");
  }

  /** The last thing in a summary that looks like one file (has an extension, no wildcard). */
  function fileIn(name: string, summary: string): string | null {
    // A Read/Edit tool's summary is the path itself, spaces and all.
    const whole = summary.trim();
    if (!/^(shell|bash|powershell)$/i.test(name) && /^[^*?<>|,]*[\w-]\.[A-Za-z0-9]{1,8}$/.test(whole)) return whole;
    const tokens = summary.split(/[\s,]+/).map((t) => t.replace(/^["']|["']$/g, ""));
    return tokens.reverse().find((t) => /^[^*?<>|]*[\w-]\.[A-Za-z0-9]{1,8}$/.test(t) && !/^-/.test(t)) ?? null;
  }

  /** What the live line and the log show: words, plus the file they are about when there is one. */
  function toolActivity(name: string, rawSummary: string): Activity {
    if (WEB_RE.test(name)) return { text: friendlyToolUse(name, rawSummary), file: null };
    const summary = unwrapShell(rawSummary);
    const head = `${name} ${summary.trim().split(/\s+/)[0] ?? ""}`.toLowerCase();
    const kind = `${name} ${summary}`.toLowerCase();
    const verb = EDIT_RE.test(head) ? "Editing" : READ_RE.test(kind) ? "Reading" : null;
    const file = verb ? fileIn(name, summary) : null;
    if (verb && file) return { text: verb, file };
    return { text: friendlyToolUse(name, rawSummary), file: null };
  }

  function friendlyToolUse(name: string, rawSummary: string): string {
    const summary = unwrapShell(rawSummary);
    const kind = `${name} ${summary}`.toLowerCase();
    const target = actionTarget(summary);
    // Only the tool name and the command's first word decide "editing": a search
    // that mentions "write" somewhere is still a read.
    const head = `${name} ${summary.trim().split(/\s+/)[0] ?? ""}`.toLowerCase();
    if (/^websearch$/i.test(name)) return `Searched the web: ${target}`;
    if (/^webfetch$/i.test(name)) return `Opened ${target}`;
    if (EDIT_RE.test(head)) {
      return target ? `Editing ${target}` : "Editing files";
    }
    if (READ_RE.test(kind)) {
      return target ? `Reading ${target}` : "Reading the project";
    }
    if (/test|check|lint|build|compile|typecheck|cargo|npm|phpunit|artisan/.test(kind)) {
      return target ? `Testing: ${target}` : "Checking that it works";
    }
    if (/git diff|git status/.test(kind)) return "Checking what changed";
    if (/shell|command|execute|bash|powershell/.test(kind)) return "Running a command";
    return target ? `Working on ${target}` : "Working on it";
  }

  function activityFor(event: ProviderEvent): Activity | null {
    const say = (text: string) => ({ text, file: null });
    switch (event.kind) {
      case "started":
        return say("Getting ready");
      case "text":
        return say("Thinking through the request");
      case "thinking":
        return say("Thinking");
      case "toolUse":
        return {
          ...toolActivity(event.data.name, event.data.summary),
          ...(event.data.changes?.length ? { text: "Editing", file: event.data.changes[0]!.path } : {}),
          ...(event.data.name === "Edit failed" ? { text: "Edit failed", failed: true } : {}),
          id: event.data.id, changes: event.data.changes,
        };
      case "done":
        return say("Putting on the finishing touches");
      case "failed":
        return say(`Couldn’t finish: ${event.data.message}`);
      case "wait":
        return say(event.data.asking ? "Waiting for your OK to run a command" : `Waiting on ${actionTarget(event.data.command)}`);
      default:
        return null;
    }
  }

  /** Replay and live delivery match a result to its own tool call. */
  function appendActivity(items: ActivityLine[], event: ProviderEvent) {
    if (event.kind === "toolResult") {
      const line = [...items].reverse().find((item) => item.id === event.data.id);
      if (line) {
        line.failed = event.data.failed;
        if (event.data.failed) {
          line.text = "Edit failed";
          line.changes = [];
        } else if (event.data.changes.length) {
          line.changes = event.data.changes;
        }
      }
      return;
    }
    const text = describe(event);
    if (text === null) return;
    const activity = event.kind === "toolUse" ? activityFor(event)! : { text, file: null };
    items.push({ kind: event.kind, ...activity, text: activity.text.slice(0, 4000) + (activity.text.length > 4000 ? "…" : "") });
  }

  /** A stage's JSON (a checker's verdict or a plan), in words. Null for anything that isn't one. */
  function describeVerdict(text: string): string | null {
    let v: {
      verdict?: string;
      checks?: Array<{ command: string; passed: boolean }>;
      findings?: Array<{ issue: string }>;
      objective?: string;
      implementation_steps?: unknown[];
    };
    try {
      v = JSON.parse(text);
    } catch {
      return null;
    }
    if (typeof v !== "object" || v === null) return null;
    if (typeof v.objective === "string") {
      const steps = (v.implementation_steps ?? []).filter((s): s is string => typeof s === "string");
      return [`Plan: ${v.objective}`, ...steps.map((s, i) => `${i + 1}. ${s}`)].join("\n");
    }
    if (typeof v.verdict !== "string") return null;
    const ok = v.verdict === "pass";
    if (v.findings) {
      const issues = v.findings.map((f) => f.issue).join("; ");
      if (ok) return issues ? `Review passed, with notes: ${issues}` : "Review passed";
      return `Review asked for changes: ${issues}`;
    }
    const checks = v.checks ?? [];
    if (!checks.length) return ok ? "Checks passed" : "Checks didn’t pass: nothing was run";
    return checks.map((c) => `${c.command} ${c.passed ? "passed" : "didn’t pass"}`).join("\n");
  }

  /** One line per event. Usage and the final result have their own panel. */
  function describe(event: ProviderEvent): string | null {
    switch (event.kind) {
      // The live activity line already says it; one per stage is just noise.
      case "started":
        return null;
      case "text":
        return describeVerdict(event.data) ?? event.data;
      case "thinking":
        return event.data;
      case "toolUse":
        // Orteca's own checks report their result as the next line.
        if (event.data.name === "orteca") return null;
        return friendlyToolUse(event.data.name, event.data.summary);
      case "failed":
        return event.data.message;
      case "wait":
        return event.data.asking ? `Asks to run: ${event.data.command}` : `Running ${event.data.command}. Ask anything meanwhile.`;
      default:
        return null;
    }
  }

  const lines = computed(() => stream.value);

  /** The same plain-English lines the live log showed, rebuilt from saved events. */
  function linesOf(events: TaskEvent[]): ActivityLine[] {
    const items: ActivityLine[] = [];
    for (const e of events) {
      // The classify call's reply is Orteca's own bookkeeping; the live run never showed it.
      if (e.stage === "classify") continue;
      const ev = e.payload as ProviderEvent;
      if (typeof ev === "object" && ev !== null && "kind" in ev) appendActivity(items, ev);
    }
    return items;
  }

  const tokens = computed(() => tokensOf(result.value));

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
    checking: "Still checking",
  };

  const STAGE_LABELS: Record<string, string> = {
    plan: "Making a plan",
    implement: "Making changes",
    review: "Reviewing the work",
    verify: "Checking that it works",
    fix: "Fixing what didn’t pass",
    answer: "Answering your question",
  };

  function stageLabel(stage: string): string {
    return STAGE_LABELS[stage] ?? stage;
  }

  /** What the route spent. Exact: Orteca started every process itself. */
  const calls = computed(() => {
    const r = result.value;
    if (!r) return null;
    return {
      used: r.callsUsed,
      stages: r.route.stages,
      ran: r.stages.map((s) => s.stage),
    };
  });

  const routeSteps = computed(() => routeStepsOf(result.value));

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

  const changed = computed(() => changedOf(result.value));

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

  // The main pane shows one thing at a time; the sidebar picks which.
  const view = ref<"task" | "history" | "agents">("task");
  const optionsOpen = ref(false);

  /** Dot colour per status: green only for a finished run, blue only while one is going. */
  const TONE: Partial<Record<TaskSummary["status"], string>> = {
    done: "ok",
    failed: "bad",
    verifyFailed: "warn",
    reviewRejected: "warn",
    running: "live",
    checking: "live",
  };

  const agentsPending = computed(() => rows.value.some((r) => r.pending));
  const agentsReady = computed(() => installed.value.filter((p) => p.auth !== "signedOut").length);

  function focusTask() {
    void nextTick(() => document.getElementById(domId("task"))?.focus());
  }

  // An empty composer. Whatever is running keeps running; it is still in the
  // sidebar, one click away.
  function newTask() {
    selectRun(null);
    task.value = "";
    attachments.value = [];
    picks.value = [];
    focusTask();
  }

  /** Back to the prompt with the same words, to tweak and run again. */
  function editAgain() {
    selectRun(null);
    focusTask();
  }

  // A follow-up. While the provider's prompt cache is warm it resumes the
  // session, which already holds the files it read; after that it starts over.
  // ponytail: one fixed window for both CLIs, since neither says when its cache cools.
  const WARM_MS = 5 * 60_000;
  const now = ref(Date.now());
  // Ticks only while a session is warm, so a closed window leaves nothing running.
  let tick: ReturnType<typeof setTimeout> | null = null;
  function tickWhileWarm() {
    now.value = Date.now();
    const r = result.value?.resume;
    tick = r && now.value < r.endedAt + WARM_MS ? setTimeout(tickWhileWarm, 1000) : null;
  }
  onUnmounted(() => {
    if (tick !== null) clearTimeout(tick);
  });

  const reply = ref("");
  /** The CLI the run on screen actually ran on. */
  const ranOn = computed(() => activeRun.value?.provider ?? null);

  /** Milliseconds a reply can still resume the session; 0 means it starts over. */
  const warmLeft = computed(() => {
    const r = result.value?.resume;
    if (!r || ranOn.value !== provider.value || isolation.value !== "currentTree") return 0;
    return Math.max(0, r.endedAt + WARM_MS - now.value);
  });

  /** One exchange a reply can go on from, whether it is still on screen or was
   *  read back out of the database. */
  type Answered = {
    taskId: number;
    /** It worked in a separate copy, so this folder is not where its work is. */
    inCopy: boolean;
    summary: string;
    files: string[];
    /** The session to pick back up, when one is still warm. */
    resume: Resume | null;
    /** The user's own words so far, oldest first. */
    asked: string[];
    /** The run to carry on in, or null to open one. */
    carry: LiveRun | null;
    /** What to open that new run with, when there is no run to carry on in. */
    turns: Turn[];
  };

  async function followUp(from: Answered) {
    const answer = reply.value.trim();
    if (!answer) return;
    const asked = [...from.asked, answer];
    // What a fresh run needs, and no more: the requests, the last reply, and where the work is.
    task.value = [
      asked.slice(0, -1).join("\n\n"),
      `Your answer:\n${from.summary}`,
      `My reply:\n${answer}`,
      ...(from.files.length ? [`Files changed so far: ${from.files.join(", ")}`] : []),
    ].join("\n\n");
    reply.value = "";
    // A copy folder is not where the task's work is; a reply there starts its own.
    const continueTask = isolation.value === "currentTree" && !from.inCopy ? from.taskId : null;
    await run({
      asked,
      // The resumed session already holds everything but the answer.
      resume: from.resume && warmLeft.value > 0 ? { ...from.resume, reply: answer } : null,
      continueTask,
      // The page stays on the conversation exactly when the task does. A reply
      // that had to open its own task gets its own row, because that is true.
      continueRun: continueTask === null ? null : from.carry,
      turns: from.turns,
    });
  }

  // A run that failed or was stopped takes a reply too; the agent is told it gave no answer.
  const noAnswer = (status: string, failure?: string | null) =>
    `(No answer: the run ended as ${HISTORY_STATUS[status as TaskSummary["status"]] ?? status}${failure ? `. ${failure}` : ""}.)`;

  async function sendReply() {
    const live = activeRun.value;
    const r = live?.result;
    if (!live || live.active || !r) return;
    await followUp({
      taskId: r.taskId,
      inCopy: !!r.worktree,
      summary: r.summary ?? noAnswer(r.status, r.failure),
      files: changedOf(r).byRun.map((f) => f.path),
      resume: r.resume,
      asked: live.asked.length ? live.asked : [live.prompt],
      carry: live,
      turns: [],
    });
  }

  /** A reply to a task from the sidebar. Its run is long over and its provider
   *  session long cold, so this always reads the files again - but it goes on in
   *  the same task row, and opens with the exchange it is answering. */
  async function replyToPast() {
    const d = historyDetail.value;
    if (!d || d.status === "running") return;
    // Every exchange so far comes along, each with the proof it logged.
    const parts = splitExchanges(d);
    const turns: Turn[] = parts.map((p, i) => {
      const r = p.result ?? undefined;
      const latest = i === parts.length - 1;
      return {
        said: p.said,
        summary: r ? r.summary : latest ? d.summary : null,
        failure: r?.failure ?? null,
        status: r?.status ?? (latest ? d.status : undefined),
        files: changedOf(r ?? (latest ? ({ diff: d.diff } as TaskResult) : null)).byRun.length,
        result: r,
        stream: linesOf(p.events),
      };
    });
    await followUp({
      taskId: d.id,
      inCopy: !!d.worktreePath,
      summary: d.summary ?? noAnswer(d.status),
      files: changedOf({ diff: d.diff } as TaskResult).byRun.map((f) => f.path),
      resume: null,
      asked: [d.prompt, ...parts.slice(1).map((p) => p.said)],
      carry: null,
      turns,
    });
  }

  /** Hand an earlier task to the next one: the user's words and the final
   *  answer, not the tool calls in between. */
  async function referTask(t: TaskSummary) {
    attachError.value = null;
    if (picks.value.some((p) => p.label.startsWith(`#${t.id} `))) return;
    try {
      const d = await getTaskDetail(opened.project.path, t.id);
      const files = changedOf({ diff: d.diff } as TaskResult).byRun.map((f) => f.path);
      const said = splitExchanges(d).map((p) => p.said);
      picks.value.push(reference(t.id, taskName(t), said, d.summary ?? noAnswer(d.status), files));
    } catch (e) {
      attachError.value = `Task #${t.id} could not be read: ${isAppError(e) ? e.message : String(e)}`;
    }
  }

  /** From a task's menu: the composer, with that task already handed in and
   *  whatever the user was writing left as it was. */
  function buildOn(t: TaskSummary) {
    selectRun(null);
    focusTask();
    void referTask(t);
  }

  function showHistory(t: TaskSummary) {
    // A task still on screen this session opens live, with its stream and steering box.
    const live = runs.value.find((r) => (r.result?.taskId ?? r.id) === t.id);
    if (live) return selectRun(live.key);
    view.value = "history";
    if (historyDetail.value?.id !== t.id) void openHistory(t);
  }

  return {
    opened,
    domId,
    task,
    attachments,
    picks,
    attachError,
    dragging,
    attach,
    addAttachments,
    pasteImages,
    fileName,
    providers,
    providerError,
    provider,
    mode,
    MODES,
    isolation,
    ISOLATIONS,
    autoWait,
    chooseAutoWait,
    installed,
    missing,
    rows,
    installing,
    installLine,
    installError,
    install,
    cancelProvider,
    signingIn,
    signInLine,
    signInError,
    signIn,
    selected,
    limits,
    limitsLoading,
    limitsError,
    limitsCheckedAt,
    usageCounters,
    providerPicked,
    MODELS,
    modelChoices,
    modelOverride,
    chooseProvider,
    chooseModel,
    pickedFor,
    loadLimits,
    headroom,
    pickByHeadroom,
    resetWhen,
    formatWhen,
    limitLine,
    limitWarning,
    waiting,
    waitForReset,
    cancelWait,
    retryAt,
    otherThan,
    alternative,
    fallback,
    switchedFrom,
    switchTo,
    canRun,
    remembering,
    rememberText,
    rememberError,
    remember,
    runs,
    runLabel,
    linesOf,
    said,
    selectRun,
    selectedRun,
    activeRun,
    anyRunning,
    runningCount,
    running,
    stream,
    result,
    checking,
    runError,
    currentActivity,
    fileError,
    openActivityFile,
    taskId,
    stopping,
    instruction,
    sending,
    instructionError,
    steering,
    instruct,
    stopRun,
    waitAsk,
    answerWaitFor,
    history,
    historyError,
    historyDetail,
    historyDetailLoading,
    historyDetailError,
    loadHistory,
    openHistory,
    confirmRemove,
    removedCopies,
    removeError,
    removeCopy,
    taskName,
    historyRow,
    taskMenu,
    renameId,
    renaming,
    deleteAsk,
    askDelete,
    taskError,
    startRename,
    saveRename,
    removeTask,
    git,
    gitOpen,
    gitAsk,
    gitBusy,
    gitLoading,
    gitRefreshError,
    gitError,
    gitNotice,
    commitMessage,
    drafting,
    draftCommit,
    targetBranch,
    newBranch,
    discardPath,
    gitQuestion,
    gitDisabledReason,
    openGit,
    refreshGit,
    runGit,
    preview,
    previewing,
    previewError,
    schedulePreview,
    refreshPreview,
    run,
    actionTarget,
    unwrapShell,
    friendlyToolUse,
    toolActivity,
    activityFor,
    appendActivity,
    describeVerdict,
    describe,
    lines,
    tokens,
    formatCost,
    cacheHit,
    formatTokens,
    formatDuration,
    formatPayload,
    OUTCOME,
    stageLabel,
    calls,
    routeSteps,
    comparison,
    changed,
    HISTORY_STATUS,
    historyLine,
    AUTH,
    view,
    optionsOpen,
    TONE,
    agentsPending,
    agentsReady,
    focusTask,
    newTask,
    editAgain,
    reply,
    ranOn,
    sendReply,
    replyToPast,
    referTask,
    buildOn,
    warmLeft,
    showHistory,
  };
}

export type ProjectState = ReturnType<typeof useProject>;
export const PROJECT: InjectionKey<ProjectState> = Symbol("project");
