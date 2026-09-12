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

/** A live detection. `path: null` means not installed — a state, not an error. */
export interface Detected {
  id: ProviderId;
  program: string;
  path: string | null;
  version: string | null;
  auth: Auth;
  costQuality: CostQuality;
}

export type FailureKind =
  | "authExpired"
  | "usageLimit"
  | "rateLimit"
  | "timeout"
  | "crashed";

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

/** `added`/`deleted` are null for a binary or untracked file, never zero. */
export interface FileStat {
  path: string;
  added: number | null;
  deleted: number | null;
}

export interface TaskResult {
  taskId: number;
  /** `cancelled` is the user stopping the run: neither a win nor a fault. */
  status: "done" | "cancelled" | "failed";
  summary: string;
  failure: string | null;
  /** null when the run ended before the provider reported any numbers. */
  usage: Usage | null;
  diff: FileStat[];
  /** The diff also contains edits that were already there when the run began. */
  dirtyAtStart: boolean;
}
