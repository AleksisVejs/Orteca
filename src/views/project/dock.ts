// The dock beside the workspace: terminals, a file tree, a file being edited
// and the dev server's own page. Tabs live per project; settings are yours and
// follow you between projects. Dock.vue provides one instance to its tabs.
import { computed, reactive, ref, watch } from "vue";
import type { InjectionKey } from "vue";

export type TabKind = "terminal" | "files" | "code" | "preview" | "history";

export type Tab = {
  id: number;
  kind: TabKind;
  title: string;
  /** code: the file being edited, relative to the project root. */
  file?: string;
  /** preview: the address the frame is showing. */
  url?: string;
  /** terminal: the folder its shell opened in, relative to the project root. */
  cwd?: string;
  /** terminal: the shell ended on its own; its output is still on screen. */
  exited?: boolean;
  /** code: edited since it was last saved. */
  dirty?: boolean;
};

/** Every knob the dock has. Saved as one object, reset as one object. */
export type DockPrefs = {
  open: boolean;
  /** Beside the workspace or under it. */
  side: "right" | "bottom";
  /** How much of the pane the dock takes, as a percentage. */
  size: number;
  fontSize: number;
  fontFamily: string;
  cursorStyle: "block" | "bar" | "underline";
  cursorBlink: boolean;
  scrollback: number;
  /** Blank means whichever PowerShell this machine has. */
  shell: string;
  copyOnSelect: boolean;
  /** Ctrl+C with nothing selected still interrupts; with a selection it copies. */
  smartCopy: boolean;
  /** Follow a dev server's address into the preview tab as soon as it prints one. */
  followDevServer: boolean;
};

export const DEFAULTS: DockPrefs = {
  open: false,
  side: "right",
  size: 44,
  fontSize: 12,
  fontFamily: 'ui-monospace, "Cascadia Code", "Consolas", monospace',
  cursorStyle: "bar",
  cursorBlink: true,
  scrollback: 5000,
  shell: "",
  copyOnSelect: false,
  smartCopy: true,
  followDevServer: true,
};

/** The mono stacks offered in settings. All of them ship with Windows or the
 *  app — the workspace downloads no font. */
export const FONTS = [
  { label: "System mono", value: 'ui-monospace, "Cascadia Code", "Consolas", monospace' },
  { label: "Cascadia Code", value: '"Cascadia Code", "Cascadia Mono", monospace' },
  { label: "Consolas", value: "Consolas, monospace" },
  { label: "Lucida Console", value: '"Lucida Console", monospace' },
];

/** Shells worth one click. Anything else is typed into the box beside them. */
export const SHELLS = [
  { label: "PowerShell", value: "" },
  { label: "Command Prompt", value: "cmd.exe" },
  { label: "Git Bash", value: String.raw`"C:\Program Files\Git\bin\bash.exe" --login -i` },
  { label: "WSL", value: "wsl.exe" },
];

const PREFS_KEY = "orteca.dock.prefs";
const tabsKey = (path: string) => `orteca.dock.tabs:${path}`;

/** Storage is a convenience here, never a source of truth: a blocked or
 *  corrupted store just means the defaults. */
function load<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? { ...fallback, ...JSON.parse(raw) } : fallback;
  } catch {
    return fallback;
  }
}

function save(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* A full or blocked store loses the preference, not the session. */
  }
}

/** A dev server's own address, as printed by vite, next, webpack and the rest. */
const LOCAL_URL = /https?:\/\/(?:localhost|127\.0\.0\.1|\[::1\])(?::\d+)?(?:\/[^\s"'`)\]]*)?/g;

export function useDock(path: string) {
  const prefs = reactive<DockPrefs>(load(PREFS_KEY, DEFAULTS));
  watch(prefs, () => save(PREFS_KEY, { ...prefs }), { deep: true });

  const stored = load<{ tabs: Tab[]; active: number }>(tabsKey(path), { tabs: [], active: 0 });
  // A terminal's shell is gone between sessions; its tab reopens a fresh one.
  const tabs = ref<Tab[]>(stored.tabs.map((t) => ({ ...t, exited: false, dirty: false })));
  const active = ref(stored.active);
  let nextId = Math.max(0, ...tabs.value.map((t) => t.id)) + 1;

  watch([tabs, active], () => save(tabsKey(path), { tabs: tabs.value, active: active.value }), { deep: true });

  /** Addresses a terminal has printed, newest first. The preview offers them. */
  const devUrls = ref<string[]>([]);

  const current = computed(() => tabs.value.find((t) => t.id === active.value) ?? null);

  function add(tab: Omit<Tab, "id">) {
    const created = { ...tab, id: nextId++ };
    tabs.value.push(created);
    active.value = created.id;
    prefs.open = true;
    return created;
  }

  const openTerminal = (cwd = "") =>
    add({ kind: "terminal", title: cwd ? cwd.split(/[\\/]/).pop()! : "Terminal", cwd });

  /** Opening a file twice focuses the tab that already has it. */
  function openCode(file: string) {
    const existing = tabs.value.find((t) => t.kind === "code" && t.file === file);
    if (existing) {
      active.value = existing.id;
      prefs.open = true;
      return existing;
    }
    return add({ kind: "code", title: file.split(/[\\/]/).pop() ?? file, file });
  }

  /** The tabs there is only ever one of: a second file tree or history would
   *  show the same thing twice. */
  function openOnly(kind: "files" | "history", title: string) {
    const existing = tabs.value.find((t) => t.kind === kind);
    if (existing) {
      active.value = existing.id;
      prefs.open = true;
      return existing;
    }
    return add({ kind, title });
  }

  const openFiles = () => openOnly("files", "Files");
  const openHistory = () => openOnly("history", "History");

  function openPreview(url = devUrls.value[0] ?? "http://localhost:5173") {
    const existing = tabs.value.find((t) => t.kind === "preview");
    if (existing) {
      existing.url = url;
      active.value = existing.id;
      prefs.open = true;
      return existing;
    }
    return add({ kind: "preview", title: "Preview", url });
  }

  function closeTab(id: number) {
    const at = tabs.value.findIndex((t) => t.id === id);
    if (at < 0) return;
    tabs.value.splice(at, 1);
    if (active.value === id) active.value = tabs.value[Math.min(at, tabs.value.length - 1)]?.id ?? 0;
  }

  /** Terminal output, scanned for the address a dev server just started on.
   *  A preview already pointed somewhere is left alone unless it is idle. */
  function noteOutput(chunk: string) {
    for (const found of chunk.match(LOCAL_URL) ?? []) {
      const url = found.replace(/[.,]$/, "");
      if (devUrls.value[0] === url) continue;
      devUrls.value = [url, ...devUrls.value.filter((u) => u !== url)].slice(0, 6);
      if (!prefs.followDevServer) continue;
      const preview = tabs.value.find((t) => t.kind === "preview");
      if (preview) preview.url = url;
    }
  }

  function resetPrefs() {
    Object.assign(prefs, DEFAULTS, { open: prefs.open });
  }

  return {
    prefs,
    tabs,
    active,
    current,
    devUrls,
    openTerminal,
    openCode,
    openFiles,
    openHistory,
    openPreview,
    closeTab,
    noteOutput,
    resetPrefs,
    path,
  };
}

export type DockState = ReturnType<typeof useDock>;
export const DOCK: InjectionKey<DockState> = Symbol("dock");
