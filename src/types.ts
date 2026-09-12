// Mirrors the serde shapes in src-tauri. Keep in step with src-tauri/src.

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

/** A stage of a route. A trivial task's route is `["implement"]` and nothing else. */
export type Stage = "plan" | "implement" | "review" | "verify";

export type RouteKind =
  | "implementOnce"
  | "standard"
  | "planned"
  | "guarded"
  | "escalated";

/** Cheapest capable tier for this task class. Policy data: Orteca records it
 *  and passes no model flag, because naming a tier is not knowing which model
 *  id is cheapest-capable on this account today. */
export type Tier = "cheapest" | "standard" | "deep";

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
}

export interface Route {
  kind: RouteKind;
  mode: Mode;
  stages: Stage[];
  budget: ExecutionBudget;
  signals: Signals;
  /** Why this route, in the words of the rule that chose it. */
  reason: string;
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
  limit: "calls" | "turns" | "tokens" | "review";
  allowed: number;
  observed: number;
  /** Stages the route still had. Nothing starts them automatically. */
  remaining: Stage[];
  message: string;
}

/** Token counts as the provider reported them. Codex never reports a cost. */
export interface Usage {
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

export interface TaskResult {
  taskId: number;
  /** `cancelled` is the user stopping the run, `budgetReached` a ceiling the
   *  route declared: neither is a win, neither is a fault. */
  status: "done" | "cancelled" | "failed" | "budgetReached";
  summary: string;
  failure: string | null;
  /** null when the run ended before the provider reported any numbers. */
  usage: Usage | null;
  diff: FileStat[];
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
  /** Set when a ceiling stopped the run; `status` is then `budgetReached`. */
  budgetStop: BudgetStop | null;
}
