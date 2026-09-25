// Mirrors the serde shapes in src-tauri. Keep in step with src-tauri/src.

/** One past run. Token and cost fields are null where nothing was reported. */
export interface TaskSummary {
  id: number;
  /** Named by the classifier call, or by the user. Empty means none yet. */
  title: string;
  prompt: string;
  status: TaskResult["status"] | "running" | "clarifying";
  /** UTC, to the second. */
  startedAt: string;
  summary: string | null;
  routeKind: RouteKind | null;
  callsUsed: number | null;
  provider: ProviderId | null;
  model: string | null;
  tokens: number | null;
  uncachedTokens: number | null;
  cachedTokens: number | null;
  costUsd: number | null;
  costQuality: CostQuality | null;
  unknownEvents: number;
  durationMs: number | null;
  patchAvailable: boolean;
}

/** A task in the shared sidebar, including the project that owns it. */
export interface GlobalTaskSummary extends TaskSummary {
  projectPath: string;
  projectName: string;
}

export type ErrorKind =
  | "notFound"
  | "notAGitRepo"
  | "cliMissing"
  | "notTrusted"
  /** Not a failure: the message is one question to answer before the run. */
  | "clarify"
  | "invalid"
  | "io"
  | "db";

export interface AppError {
  kind: ErrorKind;
  message: string;
  taskId?: number;
}

export interface Project {
  id: number;
  path: string;
  name: string;
  trusted: boolean;
  lastOpenedAt: string;
}

export interface GitState {
  isRepo: boolean;
  root: string | null;
  branch: string | null;
  head: string | null;
  dirty: boolean;
  dirtyCount: number;
  changes: GitChange[];
  /** The tracked remote branch and the distance at the last fetch; null when none. */
  upstream: string | null;
  ahead: number | null;
  behind: number | null;
  /** Local branches other than the current one. */
  branches: string[];
}

export interface GitChange {
  path: string;
  /** Two-character Git porcelain status, for example ` M` or `??`. */
  status: string;
}

export type GitAction = "fetch" | "pull" | "commit" | "push" | "merge" | "discard" | "switch" | "branch";

export interface TrustFinding {
  path: string;
  reason: string;
}

export interface OpenedProject {
  project: Project;
  git: GitState;
  trustFindings: TrustFinding[];
}
export interface Preflight {
  provider: ProviderId;
  git: GitState;
  route: Route;
  /** What implementation asks this provider for, including route overrides. */
  model: ModelChoice;
  /** What Plan runs on when Efficient mode lowers its reasoning effort. */
  plan: ModelChoice | null;
  /** What the Review runs on when it is not the route's tier; null otherwise. */
  review: ModelChoice | null;
}

export type ProviderId = "claude" | "codex";

/** A chat the user had with a CLI outside Orteca, read from its own transcript. */
export interface CliChatSummary {
  provider: ProviderId;
  id: string;
  title: string;
  /** How many times the user spoke. */
  turns: number;
  updatedMs: number;
}

export interface CliChat {
  title: string;
  said: string[];
  answer: string;
}

/** How the CLI authenticates, as reported by the CLI itself. Orteca never reads
 *  a credential and never renders a login form. */
export type Auth = "subscription" | "apiKey" | "signedOut" | "unknown";

export type CostQuality = "exact" | "estimated" | "unavailable";

/** How a mid-task instruction reaches a CLI that is already running. */
export type Steering = "live" | "checkpoint";

/** Confirmed by the backend after the run loop handles an instruction. */
export type InstructionDisposition = "live" | "held" | "resumed" | "tooLate";

export interface InstructionReceipt {
  disposition: InstructionDisposition;
}

/** A live detection. `path: null` means not installed - a state, not an error. */
export interface Detected {
  id: ProviderId;
  program: string;
  path: string | null;
  version: string | null;
  auth: Auth;
  costQuality: CostQuality;
  /** `live` takes an instruction mid-turn; `checkpoint` has to be restarted. */
  steering: Steering;
}

/** One rolling allowance window, as the CLI reported it. */
export interface LimitWindow {
  /** The CLI's own name: "session", "week (all models)", "5-hour", "week". */
  label: string;
  usedPercent: number;
  /** Unix seconds (Codex). */
  resetsAt: number | null;
  /** The CLI's own words, in the user's time zone (Claude). */
  resetsText: string | null;
}

/** How much of a plan is used. No windows means no reading, and `unavailable` says why. */
export interface RepoProfile {
  /** What runs are sent: the user's own text, or the detected one. */
  text: string;
  detected: string;
  custom: boolean;
}

export interface Limits {
  id: ProviderId;
  windows: LimitWindow[];
  unavailable: string | null;
  /** `warning` at 80% of a window; `limited` when one is spent or a run hit the plan. */
  status: "ok" | "warning" | "limited" | "unknown";
  /** Unix seconds, when a limited provider can run again, if known. */
  limitedUntil: number | null;
}

export type FailureKind =
  | "authExpired"
  | "usageLimit"
  | "rateLimit"
  | "timeout"
  /** The CLI hit the turn ceiling Orteca gave it. Not a fault of the run. */
  | "budgetReached"
  | "crashed";

/** Two modes, as decided in the architecture. `efficient` shifts every routing
 *  threshold up by two, so more work takes the shorter route. */
export type Mode = "efficient" | "balanced";

/** Where a run works: the user's folder, or a git worktree beside the
 *  repository on a branch of its own. */
export type Isolation = "currentTree" | "worktree";

/** The separate copy a run worked in. */
export interface Worktree {
  path: string;
  branch: string;
  /** The commit holding the run's changes; null when nothing changed or git refused. */
  commit: string | null;
  commitError: string | null;
}

/** A stage of a route. A trivial task's route is `["implement"]` and nothing else. */
export type Stage = "plan" | "implement" | "review" | "verify" | "fix" | "answer";

export type RouteKind =
  | "answer"
  | "implementOnce"
  | "standard"
  | "planned"
  | "guarded"
  | "escalated";

/** Cheapest capable tier for this task class. It names a model and an effort
 *  on the provider's command line; see architecture §4.3.4. */
export type Tier = "cheapest" | "standard" | "deep";

export type TaskType = "chat" | "question" | "code_change" | "debug" | "plan";

/** One task from the task log (migration 0013), for the Stats page. */
export interface TaskLogRow {
  id: number;
  /** SQLite `datetime('now')`, UTC. */
  startedAt: string;
  status: TaskSummary["status"];
  taskType: TaskType | null;
  provider: ProviderId | null;
  /** The model ID the CLI reported. */
  model: string | null;
  tier: Tier | null;
  gate: "pass" | "fail" | "none" | null;
  verdict: "accepted" | "rejected" | "retried" | null;
  tokens: number | null;
  costUsd: number | null;
  costQuality: CostQuality | null;
  toolsBeforeEdit: number | null;
  /** From the `classified` event: `command` means the user picked the type. */
  classifiedBy: "classifier" | "command" | "keywords" | null;
  /** From the `classified` event: whether a question was asked, and answered or skipped. */
  clarify: "none" | "answered" | "skipped" | null;
  /** Of the files the run edited, the share the pre-run ranking named. Null when it edited none. */
  fileRecall: number | null;
}

/** What a tier asks one provider for. */
export interface ModelChoice {
  model: string;
  effort: string;
}

/** A provider/model combination explicitly chosen by the user. `null` lets
 * Orteca choose a model and effort for each stage. */
export interface ModelOverride {
  model: string;
  effort: string;
}

/** What the classifier read out of the prompt and the repo. Only `intent`
 *  comes from a model: the provider's smallest, asked once when a run starts. */
export interface Signals {
  complexity: number;
  risk: number;
  architecture: boolean;
  security: boolean;
  authz: boolean;
  schemaChange: boolean;
  bug: boolean;
  refactor: boolean;
  frontend: boolean;
  blastRadius: number;
  priorFailures: number;
  intent?: "chat" | "question" | "easy" | "medium" | "hard" | null;
}

/** The tiers a route runs on. There is no call, turn or token ceiling: a
 *  failed check is fixed and run again until it passes. */
export interface ExecutionBudget {
  preferredTier: Tier;
  /** Guarded work is reviewed on this tier rather than `preferredTier`. */
  reviewTier: Tier | null;
  /** Hard work is planned on this tier and built on `preferredTier`. Absent on older rows. */
  planTier?: Tier | null;
}

export interface Route {
  kind: RouteKind;
  mode: Mode;
  stages: Stage[];
  budget: ExecutionBudget;
  signals: Signals;
  /** Why this route, in the words of the rule that chose it. */
  reason: string;
  /** Why `budget.preferredTier`, the same way. */
  tierReason: string;
  /** Paths named in the brief. Names only - never file contents. */
  candidatePaths: string[];
  /** What the first few of those define and use, same order. Absent on runs from before the code map. */
  candidateNotes?: string[];
  /** What each stage would have preferred to run on. Recorded, not acted on. */
  preferredProviders: ProviderId[];
  /** Picks the ruleset and the tools. Missing on routes saved before task types. */
  taskType?: TaskType;
}

/** One finished stage. `artifact` is null unless structured output came back
 *  and matched the shape the stage contracted for; prose is never read in its
 *  place. */
export interface StageNote {
  stage: Stage;
  summary: string;
  artifact: unknown | null;
  /** What Orteca asked the CLI for; null for checks Orteca ran itself. */
  model?: string | null;
  effort?: string | null;
  /** Wall time of the stage, Orteca's own checks included. */
  durationMs?: number | null;
}

/** Why a run stopped short of its route. Neither a win nor a fault: the work,
 *  the diff and the usage are all kept, and going further is the user's call. */
export interface BudgetStop {
  /** `loop`: the agent repeated a failing command or an edit. */
  limit: "calls" | "turns" | "tokens" | "review" | "verify" | "loop";
  allowed: number;
  observed: number;
  /** Stages the route still had. Nothing starts them automatically. */
  remaining: Stage[];
  message: string;
}

/** Token counts as the provider reported them. Codex never reports a cost. */
export interface Usage {
  model: string | null;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  costUsd: number | null;
  costQuality: CostQuality;
}

export interface FileEdit {
  path: string;
  /** Unified hunks recorded for this edit; null when the CLI only reports a path. */
  patch: string | null;
}

/** Adjacently tagged in Rust, so every variant carries its payload in `data`. */
export type ProviderEvent =
  | { kind: "stageProgress"; data: { stages: string[]; current: number[] } }
  | { kind: "started"; data: { sessionId: string } }
  | { kind: "text"; data: string }
  | { kind: "thinking"; data: string }
  | { kind: "toolUse"; data: { name: string; summary: string; id?: string; changes?: FileEdit[] } }
  | { kind: "toolResult"; data: { id: string; changes: FileEdit[]; failed: boolean } }
  | { kind: "usage"; data: Usage }
  | { kind: "done"; data: { result: string; structured: unknown } }
  | { kind: "failed"; data: { kind: FailureKind; message: string } }
  /** A command the agent handed Orteca to run; `asking` while it waits for the user's OK. */
  | { kind: "wait"; data: { command: string; asking: boolean } }
  | { kind: "waitOutput"; data: string[] };

/** `run`: clean before the run. `beforeRun`: already changed, untouched by
 *  the run. `both`: already changed, and changed again. `reverted`: already
 *  changed, and put back to the commit by the run - the user's change undone. */
export type Origin = "run" | "beforeRun" | "both" | "reverted";

/** `added`/`deleted` are null for a binary or untracked file, never zero. */
export interface FileStat {
  path: string;
  added: number | null;
  deleted: number | null;
  /** null when there was no snapshot to compare against: unknown, not the run's. */
  origin: Origin | null;
}

/** Medians of the last finished runs in this project on the same route kind
 *  and provider. Only sent once there are at least five, and always an
 *  estimate: the same route is not the same work. */
export interface Baseline {
  runs: number;
  medianTokens: number;
  medianCalls: number;
}

export interface Resume {
  session: string;
  model: string;
  /** Unix milliseconds. */
  endedAt: number;
}

export interface TaskResult {
  taskId: number;
  /** `cancelled` is the user stopping the run, `budgetReached` a ceiling the
   *  route declared, `reviewRejected` an explicit review stop and
   *  `verifyFailed` checks that did not report a pass: none is a win, and none
   *  is a provider fault. `unchanged` is the agent looking and deciding
   *  nothing needed changing: not a win and not a failure. `checking` is only
   *  the early result a run sends while its full suite still runs; no run ends on it. */
  status: "done" | "cancelled" | "failed" | "budgetReached" | "reviewRejected" | "verifyFailed" | "unchanged" | "checking";
  summary: string;
  failure: string | null;
  /** Why it failed, when it did. `usageLimit` is what offers the other CLI. */
  failureKind: FailureKind | null;
  /** null when the run ended before the provider reported any numbers. */
  usage: Usage | null;
  diff: FileStat[];
  patchText: string | null;
  unknownEvents: number;
  durationMs: number;
  /** The diff also contains edits that were already there when the run began. */
  dirtyAtStart: boolean;
  /** Decided before any provider started, ceilings included. */
  route: Route;
  /** The stages that actually ran, in order. */
  stages: StageNote[];
  /** Provider processes started, stages and resumes together. */
  callsUsed: number;
  turnsUsed: number;
  /** Set when a fix changed nothing and a check still did not pass. */
  budgetStop: BudgetStop | null;
  /** null until the project has enough comparable runs to compare against. */
  baseline: Baseline | null;
  /** Set when the run worked in a separate copy. */
  worktree: Worktree | null;
  /** The session a follow-up can pick back up, and when it last spoke. */
  resume: Resume | null;
  /** Steps outside the agent (classify, each check command), in order. */
  timings: { label: string; ms: number }[];
  /** The commit holding the user's uncommitted work from before the run; a
   *  `reverted` file is restored from it. Absent on older results. */
  savedState?: string | null;
  /** The reply Claude's second opinion recommends for its serious findings,
   *  offered as one click and never sent on its own. Absent on older results. */
  recommendedFix?: string | null;
}
export interface TaskEvent {
  id: number;
  ts: string;
  stage: string | null;
  kind: string;
  provider: string;
  payload: unknown;
}
export interface TaskDetail {
  id: number;
  prompt: string;
  status: TaskSummary["status"];
  startedAt: string;
  endedAt: string | null;
  summary: string | null;
  route: unknown | null;
  diff: FileStat[];
  patchText: string | null;
  dirtyAtStart: boolean;
  callsUsed: number | null;
  provider: ProviderId | null;
  model: string | null;
  tokens: number | null;
  uncachedTokens: number | null;
  cachedTokens: number | null;
  /** Uncached input alone; `uncachedTokens` also counts output. */
  inputTokens: number | null;
  costUsd: number | null;
  costQuality: CostQuality | null;
  unknownEvents: number;
  durationMs: number | null;
  branch: string | null;
  /** The separate copy the run worked in, until it is removed. */
  worktreePath: string | null;
  events: TaskEvent[];
}

/** One child of a directory in the dock's file tree. */
export type DirEntry = { name: string; dir: boolean };

/** One commit in the dock's history tab. `relative` is git's own wording. */
export type Commit = {
  hash: string;
  subject: string;
  author: string;
  relative: string;
  when: string;
};

export interface MemoryItem {
  id: number;
  global: boolean;
  text: string;
}

/** `tokens` is characters / 4: an estimate, always shown as one. 0 limit = none. */
export interface MemoryState {
  items: MemoryItem[];
  limit: number;
  tokens: number;
}

export interface MemoryProposal {
  original: string;
  bullets: string[];
}

/** `user` is ~/.claude/CLAUDE.md; the others are the project root's files. */
export type ImportFile = "user" | "CLAUDE.md" | "AGENTS.md";
