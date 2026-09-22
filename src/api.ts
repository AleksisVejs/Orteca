import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AppError,
  Commit,
  Detected,
  DirEntry,
  GitAction,
  GitState,
  Limits,
  ImportFile,
  MemoryProposal,
  MemoryState,
  Mode,
  ModelOverride,
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
  GlobalTaskSummary,
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

/** Memory items, the limit and the estimated size. No path: global only. */
export const memory = (path?: string) => invoke<MemoryState>("memory", { path });
export const addMemory = (path: string | undefined, global: boolean, text: string) =>
  invoke<void>("add_memory", { path, global, text });
export const editMemory = (id: number, text: string) => invoke<void>("edit_memory", { id, text });
export const deleteMemory = (id: number) => invoke<void>("delete_memory", { id });
/** Rules a small model proposes from an instructions file. Saves nothing. */
export const proposeMemoryImport = (path: string | undefined, file: ImportFile) =>
  invoke<MemoryProposal>("propose_memory_import", { path, file });
/** 0 means no limit. */
/** Rules proposed from this project's recent runs; `original` is what the model read. */
export const proposeMemoryFromRuns = (path: string) =>
  invoke<MemoryProposal>("propose_memory_from_runs", { path });
export const setMemoryLimit = (limit: number) => invoke<void>("set_memory_limit", { limit });

/** This project's past runs, newest first. */
export const recentTasks = (path: string) =>
  invoke<TaskSummary[]>("recent_tasks", { path });
/** Recent tasks across every remembered project, newest first. */
export const globalTasks = () => invoke<GlobalTaskSummary[]>("global_tasks");
export const getTaskDetail = (path: string, taskId: number) =>
  invoke<TaskDetail>("task_detail", { path, taskId });
/** `headroom` is the room left in the provider's tightest plan window, or null
 *  when unread. The router may run a checked route a tier down when it is low.
 *  `asked` is only what the user just said, when `prompt` also carries the
 *  exchange before it; the route is chosen from it. */
export const previewTask = (path: string, prompt: string, provider: ProviderId, mode: Mode, headroom: number | null, isolation: Isolation, model: ModelOverride | null, asked: string | null = null) =>
  invoke<Preflight>("preview_task", { path, prompt, asked, provider, mode, headroom, isolation, model });

/** Deletes a finished run's copy folder. Git refuses while it holds uncommitted
 *  work; the branch always stays. */
/** Opens a file a run touched with its default app, or shows it in Explorer. */
export const openFile = (path: string, file: string, reveal: boolean) =>
  invoke<void>("open_file", { path, file, reveal });

export const renameTask = (path: string, taskId: number, title: string) =>
  invoke<void>("rename_task", { path, taskId, title });

export const deleteTask = (path: string, taskId: number) =>
  invoke<void>("delete_task", { path, taskId });

export const removeWorktree =(path: string, taskId: number) =>
  invoke<void>("remove_worktree", { path, taskId });

/** Runs one git command the user confirmed. `input` is the commit message or
 *  the branch to merge. Returns the git state after it. */
export const gitAction = (path: string, action: GitAction, input = "") =>
  invoke<GitState>("git_action", { path, action, input });

/** Refresh local Git state without fetching or planning an AI task. */
export const gitStatus = (path: string) => invoke<GitState>("git_status", { path });

/** Plan limits from each CLI's own answer. Costs no tokens; takes seconds. */
/** `fresh` pays for a new Claude reading instead of the one kept for 15 minutes. */
export const providerLimits = (fresh = false) => invoke<Limits[]>("provider_limits", { fresh });

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
  model: ModelOverride | null,
  onEvent: (event: ProviderEvent) => void,
  onTask: (taskId: number) => void,
  // The change, once its focused tests pass and while the full suite runs.
  onChecking: (result: TaskResult) => void,
  resume: (Resume & { reply: string }) | null = null,
  // A reply that goes on in this task instead of opening a new one.
  continueTask: number | null = null,
  // Only what the user just said, when `prompt` also carries the exchange
  // before it. The route and the classifier read this, so a question asked
  // after a finished change is routed as a question and not as more of it.
  asked: string | null = null,
) => {
  const events = new Channel<ProviderEvent>();
  events.onmessage = onEvent;
  // The id lands as soon as the task row exists, long before the run resolves.
  // Without it there is nothing for Stop to name.
  const task = new Channel<number>();
  task.onmessage = onTask;
  const checking = new Channel<TaskResult>();
  checking.onmessage = onChecking;
  return invoke<TaskResult>("start_task", { path, prompt, asked, provider, mode, headroom, isolation, attachments, resume, continueTask, model, events, task, checking });
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

/* --- The dock: terminals, the file tree and one file in the code tab. --- */

/** Opens a shell in `sub` (relative to the project) and returns its id. */
export const ptyOpen = (path: string, sub: string, shell: string, cols: number, rows: number) =>
  invoke<number>("pty_open", { path, sub, shell, cols, rows });

export const ptyWrite = (id: number, data: string) =>
  invoke<void>("pty_write", { id, data });

export const ptyResize = (id: number, cols: number, rows: number) =>
  invoke<void>("pty_resize", { id, cols, rows });

/** Closes a terminal and everything it started. Never throws for a dead one. */
export const ptyClose = (id: number) => invoke<void>("pty_close", { id });

/** Output from one terminal. The returned promise resolves to an unlisten fn. */
export const onPtyData = (id: number, fn: (chunk: string) => void) =>
  listen<string>(`pty:${id}`, (e) => fn(e.payload));

/** The shell ended on its own — the tab stays, holding what it printed. */
export const onPtyExit = (id: number, fn: () => void) =>
  listen<null>(`pty-exit:${id}`, () => fn());

export const listDir = (path: string, sub: string) =>
  invoke<DirEntry[]>("list_dir", { path, sub });

export const readText = (path: string, file: string) =>
  invoke<string>("read_text", { path, file });

export const writeText = (path: string, file: string, text: string) =>
  invoke<void>("write_text", { path, file, text });

/** The repository's own commits, newest first, from `skip` back. */
export const findLines = (path: string, needles: string[]) =>
  invoke<string[]>("find_lines", { path, needles });
export const gitLog =(path: string, skip: number, count: number) =>
  invoke<Commit[]>("git_log", { path, skip, count });

/** What one commit changed, as a patch. */
export const commitPatch = (path: string, hash: string) =>
  invoke<string>("commit_patch", { path, hash });

/** What the working tree has that no commit holds yet, as a patch. */
export const workingPatch = (path: string) =>
  invoke<string>("working_patch", { path });
