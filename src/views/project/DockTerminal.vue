<script setup lang="ts">
import { inject, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { isAppError, onPtyData, onPtyExit, ptyClose, ptyOpen, ptyResize, ptyWrite } from "../../api";
import { DOCK } from "./dock";
import type { Tab } from "./dock";

// One real shell in a ConPTY. The tab stays mounted while another is on top,
// so a build carries on running when you look at a file and come back.
const props = defineProps<{ tab: Tab; visible: boolean }>();
const dock = inject(DOCK)!;
const { prefs } = dock;

const host = ref<HTMLDivElement | null>(null);
const error = ref<string | null>(null);

let term: Terminal | null = null;
let fit: FitAddon | null = null;
let id = 0;
let stop: (() => void)[] = [];
let observer: ResizeObserver | null = null;

/** Terminal chrome comes from the workspace; the colours inside the grid are
 *  whatever the programs print, so xterm's own ANSI palette is left alone. */
function chrome() {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(name).trim();
  return {
    background: v("--bg"),
    foreground: v("--text-dim"),
    cursor: v("--text"),
    cursorAccent: v("--bg"),
    selectionBackground: v("--surface-2"),
  };
}

const copy = () => term?.hasSelection() && navigator.clipboard.writeText(term.getSelection());
const paste = async () => id && ptyWrite(id, await navigator.clipboard.readText()).catch(() => {});

/** Fit to the pane, then tell the shell the new size so its prompt wraps right. */
function resize() {
  if (!term || !props.visible || !host.value?.clientWidth) return;
  fit?.fit();
  if (id) ptyResize(id, term.cols, term.rows).catch(() => {});
}

async function start() {
  const shell = new Terminal({
    fontSize: prefs.fontSize,
    fontFamily: prefs.fontFamily,
    cursorStyle: prefs.cursorStyle,
    cursorBlink: prefs.cursorBlink,
    scrollback: prefs.scrollback,
    theme: chrome(),
    allowProposedApi: true,
  });
  term = shell;
  fit = new FitAddon();
  shell.loadAddon(fit);
  shell.open(host.value!);
  fit.fit();

  // The workspace keeps Ctrl+` and the clipboard; everything else is the shell's.
  shell.attachCustomKeyEventHandler((e) => {
    if (e.type !== "keydown" || !e.ctrlKey) return true;
    if (e.key === "`") return false;
    const key = e.key.toUpperCase();
    if (key === "V") {
      paste();
      return false;
    }
    if (key === "C" && (e.shiftKey || (prefs.smartCopy && shell.hasSelection()))) {
      copy();
      return false;
    }
    return true;
  });
  shell.onSelectionChange(() => prefs.copyOnSelect && copy());

  try {
    id = await ptyOpen(dock.path, props.tab.cwd ?? "", prefs.shell, shell.cols, shell.rows);
  } catch (e) {
    error.value = isAppError(e) ? e.message : String(e);
    return;
  }
  const typed = shell.onData((data) => ptyWrite(id, data).catch(() => {}));
  stop.push(() => typed.dispose());
  stop.push(await onPtyData(id, (chunk) => {
    shell.write(chunk);
    dock.noteOutput(chunk);
  }));
  stop.push(await onPtyExit(id, () => {
    props.tab.exited = true;
    shell.write("\r\n\x1b[2m[the shell exited]\x1b[0m\r\n");
  }));
  resize();
}

/** Close the shell and open another in the same tab, keeping its place. */
async function restart() {
  for (const off of stop.splice(0)) off();
  if (id) await ptyClose(id).catch(() => {});
  term?.dispose();
  id = 0;
  error.value = null;
  props.tab.exited = false;
  await start();
}

onMounted(async () => {
  await start();
  observer = new ResizeObserver(resize);
  observer.observe(host.value!);
});

onBeforeUnmount(() => {
  observer?.disconnect();
  for (const off of stop.splice(0)) off();
  if (id) ptyClose(id).catch(() => {});
  term?.dispose();
});

watch(() => props.visible, (on) => on && requestAnimationFrame(resize));
watch(
  () => [prefs.fontSize, prefs.fontFamily, prefs.cursorStyle, prefs.cursorBlink, prefs.scrollback],
  () => {
    if (!term) return;
    term.options.fontSize = prefs.fontSize;
    term.options.fontFamily = prefs.fontFamily;
    term.options.cursorStyle = prefs.cursorStyle;
    term.options.cursorBlink = prefs.cursorBlink;
    term.options.scrollback = prefs.scrollback;
    resize();
  },
);
</script>

<template>
  <div class="terminal-tab">
    <p v-if="error" class="note terminal-error" role="alert">{{ error }}</p>
    <div ref="host" class="grid"></div>
    <footer v-if="tab.exited || error" class="terminal-foot">
      <span class="note">{{ error ? "No shell is running." : "The shell exited." }}</span>
      <button class="btn" @click="restart">Start another</button>
    </footer>
  </div>
</template>

<style scoped>
.terminal-tab {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  height: 100%;
  background: var(--bg);
}
.grid {
  flex: 1;
  min-height: 0;
  padding: 6px 4px 6px 10px;
}
.terminal-error {
  margin: 0;
  padding: 10px 12px;
  color: var(--err);
  border-bottom: 1px solid var(--border);
}
.terminal-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--gap);
  padding: 6px 10px;
  border-top: 1px solid var(--border);
  background: var(--surface);
}
.terminal-foot .note {
  margin: 0;
}
.terminal-foot .btn {
  padding: 4px 10px;
  font-size: 12px;
}
</style>
