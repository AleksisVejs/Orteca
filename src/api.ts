import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AppError,
  Detected,
  Mode,
  OpenedProject,
  Project,
  ProviderEvent,
  ProviderId,
  InstructionReceipt,
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

/** Rows arrive one at a time as each CLI answers; the response is the full set. */
export const detectProviders = (onFound: (provider: Detected) => void) => {
  const found = new Channel<Detected>();
  found.onmessage = onFound;
  return invoke<Detected[]>("detect_providers", { found });
};

export const recentProjects = () => invoke<Project[]>("recent_projects");

export const trustProject = (path: string, trusted: boolean) =>
  invoke<void>("trust_project", { path, trusted });

export const forgetProject = (path: string) =>
  invoke<void>("forget_project", { path });

/**
 * The channel exists before invocation; completion uses the invoke response.
 *
 * `mode` picks how readily the classifier takes the shorter route. The route
 * itself is chosen in Rust before any CLI starts, costs nothing, and comes back
 * on the result.
 */
export const startTask = (
  path: string,
  prompt: string,
  provider: ProviderId,
  mode: Mode,
  onEvent: (event: ProviderEvent) => void,
  onTask: (taskId: number) => void,
) => {
  const events = new Channel<ProviderEvent>();
  events.onmessage = onEvent;
  // The id lands as soon as the task row exists, long before the run resolves.
  // Without it there is nothing for Stop to name.
  const task = new Channel<number>();
  task.onmessage = onTask;
  return invoke<TaskResult>("start_task", { path, prompt, provider, mode, events, task });
};

/**
 * Stops a run and the whole process tree under it. Throws if the run has
 * already ended - the backend will not claim to have stopped something that
 * was already over.
 */
export const cancelTask = (taskId: number) =>
  invoke<void>("cancel_task", { taskId });

/**
 * Sends a mid-task instruction. A live provider takes it straight away; a
 * checkpoint one holds it unless `applyNow`, which restarts its session with
 * the instruction rather than waiting for a boundary that may never come.
 */
export const sendInstruction = (taskId: number, text: string, applyNow: boolean) =>
  invoke<InstructionReceipt>("send_instruction", { taskId, text, applyNow });

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
