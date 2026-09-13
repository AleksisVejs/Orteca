// Mirrors the serde shapes in src-tauri. Keep in step with src-tauri/src.

/** One past run. Token and cost fields are null where nothing was reported. */
export interface TaskSummary {
  id: number;
  prompt: string;
  status: TaskResult["status"] | "running";
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

export type ErrorKind =
  | "notFound"
  | "notAGitRepo"
  | "cliMissing"
  | "notTrusted"
  | "invalid"
  | "io"
  | "db";

export interface AppError {
  kind: ErrorKind;
  message: string;
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
}

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
  model: ModelChoice;
  /** What the route's one Fix call would run on; null when it declares none. */
  escalation: ModelChoice | null;
}

export type ProviderId = "claude" | "codex";

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

/** A live detection. `path: null` means not installed — a state, not an error. */
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
export interface Limits {
  id: ProviderId;
  windows: LimitWindow[];
  unavailable: string | null;
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
export type Stage = "plan" | "implement" | "review" | "verify" | "fix";

export type RouteKind =
  | "implementOnce"
  | "standard"
  | "planned"
  | "guarded"
  | "escalated";

/** Cheapest capable tier for this task class. It names a model and an effort
 *  on the provider's command line; see architecture §4.3.4. */
export type Tier = "cheapest" | "standard" | "deep";

/** What a tier asks one provider for. */
export interface ModelChoice {
  model: string;
  effort: string;
}

/** What the deterministic classifier read out of the prompt and the repo. No
 *  model was called to produce any of it. */
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
}

/** The ceilings a route declares before any provider is started. */
export interface ExecutionBudget {
  maxAgentCalls: number;
  /** Passed to Claude, which has the flag; counted by Orteca for Codex, which
   *  does not. Either way the number is real. */
  maxTurns: number | null;
  /** An inter-turn guard. It cannot stop a turn already running, so what it
   *  stops is the next stage. */
  maxReportedTokens: number | null;
  preferredTier: Tier;
  /** The tier of the one Fix call a failed Review or Verify may buy. Null
   *  when the route has no such stage or is already on `deep`. */
  escalation: Tier | null;
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
  /** Paths named in the brief. Names only — never file contents. */
  candidatePaths: string[];
  /** What each stage would have preferred to run on. Recorded, not acted on. */
  preferredProviders: ProviderId[];
}

/** One finished stage. `artifact` is null unless structured output came back
 *  and matched the shape the stage contracted for; prose is never read in its
 *  place. */
export interface StageNote {
  stage: Stage;
  summary: string;
  artifact: unknown | null;
}

/** Why a run stopped short of its route. Neither a win nor a fault: the work,
 *  the diff and the usage are all kept, and going further is the user's call. */
export interface BudgetStop {
  limit: "calls" | "turns" | "tokens" | "review" | "verify";
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

/** Adjacently tagged in Rust, so every variant carries its payload in `data`. */
export type ProviderEvent =
  | { kind: "started"; data: { sessionId: string } }
  | { kind: "text"; data: string }
  | { kind: "toolUse"; data: { name: string; summary: string } }
  | { kind: "usage"; data: Usage }
  | { kind: "done"; data: { result: string; structured: unknown } }
  | { kind: "failed"; data: { kind: FailureKind; message: string } };

/** `run`: clean before the run. `beforeRun`: already changed, untouched by
 *  the run. `both`: already changed, and changed again. */
export type Origin = "run" | "beforeRun" | "both";

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

export interface TaskResult {
  taskId: number;
  /** `cancelled` is the user stopping the run, `budgetReached` a ceiling the
   *  route declared, `reviewRejected` an explicit review stop and
   *  `verifyFailed` checks that did not report a pass: none is a win, and none
   *  is a provider fault. */
  status: "done" | "cancelled" | "failed" | "budgetReached" | "reviewRejected" | "verifyFailed";
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
  /** Provider processes started. With `route.budget.maxAgentCalls` this is the
   *  calls-avoided figure, and it is exact. */
  callsUsed: number;
  turnsUsed: number;
  /** Set when a ceiling or review outcome stopped the run. */
  budgetStop: BudgetStop | null;
  /** null until the project has enough comparable runs to compare against. */
  baseline: Baseline | null;
  /** Set when the run worked in a separate copy. */
  worktree: Worktree | null;
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
  costUsd: number | null;
  costQuality: CostQuality | null;
  unknownEvents: number;
  durationMs: number | null;
  branch: string | null;
  /** The separate copy the run worked in, until it is removed. */
  worktreePath: string | null;
  events: TaskEvent[];
}
