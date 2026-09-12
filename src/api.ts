import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { AppError, Detected, OpenedProject, Project } from "./types";

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/** Native folder picker. Returns null if the user cancelled. */
export async function pickFolder(): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false });
  return typeof picked === "string" ? picked : null;
}

export const openProject = (path: string) =>
  invoke<OpenedProject>("open_project", { path });

export const detectProviders = () => invoke<Detected[]>("detect_providers");

export const recentProjects = () => invoke<Project[]>("recent_projects");

export const trustProject = (path: string, trusted: boolean) =>
  invoke<void>("trust_project", { path, trusted });

export const forgetProject = (path: string) =>
  invoke<void>("forget_project", { path });
