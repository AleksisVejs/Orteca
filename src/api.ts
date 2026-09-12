import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AppError,
  Detected,
  OpenedProject,
  Project,
  ProviderEvent,
  ProviderId,
  TaskResult,
} from "./types";

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

/** The channel exists before invocation; completion uses the invoke response. */
export const startTask = (
  path: string,
  prompt: string,
  provider: ProviderId,
  onEvent: (event: ProviderEvent) => void,
) => {
  const events = new Channel<ProviderEvent>();
  events.onmessage = onEvent;
  return invoke<TaskResult>("start_task", { path, prompt, provider, events });
};

/** Installs the CLI with npm. Resolves with the fresh detection, or throws. */
export const installProvider = (provider: ProviderId) =>
  invoke<Detected>("install_provider", { provider });

export const onInstallEvent = (fn: (provider: ProviderId, line: string) => void) =>
  listen<[ProviderId, string]>("install-event", (e) => fn(e.payload[0], e.payload[1]));

/**
 * Runs the CLI's own sign-in. Orteca shows no login form and never sees a
 * password: the CLI prints a URL, the user approves it in their browser.
 * Resolves with the fresh detection, or throws if it is still signed out.
 */
export const signInProvider = (provider: ProviderId) =>
  invoke<Detected>("sign_in_provider", { provider });

export const onSignInEvent = (fn: (provider: ProviderId, line: string) => void) =>
  listen<[ProviderId, string]>("sign-in-event", (e) => fn(e.payload[0], e.payload[1]));
