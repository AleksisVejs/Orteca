// Mirrors the serde shapes in src-tauri. Keep in step with src-tauri/src.

export type ErrorKind = "notFound" | "notAGitRepo" | "io" | "db" | "gitFailed";

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

export type Mode = "efficient" | "balanced";
