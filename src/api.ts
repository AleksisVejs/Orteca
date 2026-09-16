import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AppError,
  Detected,
  GitAction,
  GitState,
  Limits,
  Mode,
  OpenedProject,
  Preflight,
  Project,
  ProviderEvent,
  ProviderId,
  InstructionReceipt,
  Isolation,
  Resume,
  TaskResult,
  TaskSummary,
  TaskDetail,
} from "./types";

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/** Native folder picker. Returns null if the user cancelled. */
export async function pickFolder(): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false });
  return typeof picked === "string" ? picked : null;
}

/** Native file or folder picker for attachments. Empty if the user cancelled. */
export async function pickAttachments(directory: boolean): Promise<string[]> {
  const picked = await open({ directory, multiple: true });
  return picked === null ? [] : [picked].flat();
}

/** A pasted image has no file behind it; this writes one and returns its path. */
export const savePastedImage = async (image: Blob, extension: string) =>
  invoke<string>("save_pasted_image", {
    // ponytail: bytes travel as a JSON array; fine for screenshots, raw IPC if big pastes lag.
    bytes: Array.from(new Uint8Array(await image.arrayBuffer())),
    extension,
  });

/** Files dragged over the window: `dragging` while held, `paths` once dropped. */
export const onFileDrop = (fn: (dragging: boolean, paths: string[]) => void) =>
  getCurrentWebview().onDragDropEvent(({ payload }) => {
    const over = payload.type === "enter" || payload.type === "over";
    fn(over, payload.type === "drop" ? payload.paths : []);
  });

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

/** This project's past runs, newest first. */
export const recentTasks = (path: string) =>
  invoke<TaskSummary[]>("recent_tasks", { path });
export const getTaskDetail = (path: string, taskId: number) =>
  invoke<TaskDetail>("task_detail", { path, taskId });
/** `headroom` is the room left in the provider's tightest plan window, or null
 *  when unread. The router may run a checked route a tier down when it is low. */
export const previewTask = (path: string, prompt: string, provider: ProviderId, mode: Mode, headroom: number | null, isolation: Isolation) =>
  invoke<Preflight>("preview_task", { path, prompt, provider, mode, headroom, isolation });

/** Deletes a finished run's copy folder. Git refuses while it holds uncommitted
 *  work; the branch always stays. */
/** Opens a file a run touched with its default app, or shows it in Explorer. */
export const openFile = (path: string, file: string, reveal: boolean) =>
  invoke<void>("open_file", { path, file, reveal });

export const removeWorktree =(path: string, taskId: number) =>
  invoke<void>("remove_worktree", { path, taskId });

/** Runs one git command the user confirmed. `input` is the commit message or
 *  the branch to merge. Returns the git state after it. */
export const gitAction = (path: string, action: GitAction, input = "") =>
  invoke<GitState>("git_action", { path, action, input });

/** Plan limits from each CLI's own answer. Costs no tokens; takes seconds. */
export const providerLimits = () => invoke<Limits[]>("provider_limits");

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
  headroom: number | null,
  isolation: Isolation,
  attachments: string[],
  onEvent: (event: ProviderEvent) => void,
  onTask: (taskId: number) => void,
  resume: (Resume & { reply: string }) | null = null,
  // A reply that goes on in this task instead of opening a new one.
  continueTask: number | null = null,
) => {
  const events = new Channel<ProviderEvent>();
  events.onmessage = onEvent;
  // The id lands as soon as the task row exists, long before the run resolves.
  // Without it there is nothing for Stop to name.
  const task = new Channel<number>();
  task.onmessage = onTask;
  return invoke<TaskResult>("start_task", { path, prompt, provider, mode, headroom, isolation, attachments, resume, continueTask, events, task });
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
export const cancelProviderOperation = (provider: ProviderId) =>
  invoke<void>("cancel_provider_operation", { provider });

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
