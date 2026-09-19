<script setup lang="ts">
import { computed, inject, nextTick, onMounted, ref, watch } from "vue";
import hljs from "highlight.js/lib/common";
import { isAppError, readText, writeText } from "../../api";
import { DOCK } from "./dock";
import type { Tab } from "./dock";

// One file, editable. The textarea is the only thing that scrolls; the
// highlighted copy and the line numbers are moved to follow it, so there is
// one scroll position and nothing to keep in step.
const props = defineProps<{ tab: Tab }>();
const dock = inject(DOCK)!;

const text = ref("");
const saved = ref("");
const error = ref<string | null>(null);
const loading = ref(true);
const area = ref<HTMLTextAreaElement | null>(null);
const code = ref<HTMLPreElement | null>(null);
const gutter = ref<HTMLDivElement | null>(null);

const LANGUAGES: Record<string, string> = {
  ts: "typescript", tsx: "typescript", mts: "typescript", cts: "typescript",
  js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
  rs: "rust", php: "php", py: "python", rb: "ruby", go: "go", java: "java",
  json: "json", css: "css", scss: "scss", less: "less", sql: "sql",
  html: "xml", vue: "xml", xml: "xml", svg: "xml", md: "markdown",
  yml: "yaml", yaml: "yaml", sh: "bash", bash: "bash", toml: "ini", ini: "ini",
  lock: "yaml", diff: "diff", patch: "diff",
};

/** ponytail: whole-file highlight on every keystroke; chunk it if a big file drags. */
const HIGHLIGHT_LIMIT = 200_000;

const language = computed(() => {
  const named = LANGUAGES[props.tab.file?.split(".").pop()?.toLowerCase() ?? ""];
  // The common bundle is a subset; anything it does not know renders plainly.
  return named && hljs.getLanguage(named) ? named : "plaintext";
});

const highlighted = computed(() => {
  const source = text.value.endsWith("\n") ? `${text.value} ` : text.value;
  if (source.length > HIGHLIGHT_LIMIT) return hljs.highlight(source, { language: "plaintext" }).value;
  return hljs.highlight(source, { language: language.value, ignoreIllegals: true }).value;
});

const lines = computed(() => text.value.split("\n").length);
const dirty = computed(() => text.value !== saved.value);
watch(dirty, (on) => (props.tab.dirty = on), { immediate: true });

async function load() {
  loading.value = true;
  try {
    saved.value = await readText(dock.path, props.tab.file!);
    text.value = saved.value;
    error.value = null;
  } catch (e) {
    error.value = isAppError(e) ? e.message : String(e);
  }
  loading.value = false;
}

async function save() {
  if (!dirty.value) return;
  const wrote = text.value;
  try {
    await writeText(dock.path, props.tab.file!, wrote);
    saved.value = wrote;
    error.value = null;
  } catch (e) {
    error.value = isAppError(e) ? e.message : String(e);
  }
}

/** Move the two passive layers instead of giving them scrollbars of their own. */
function sync() {
  const box = area.value;
  if (!box) return;
  if (code.value) code.value.style.transform = `translate(${-box.scrollLeft}px, ${-box.scrollTop}px)`;
  if (gutter.value) gutter.value.style.transform = `translateY(${-box.scrollTop}px)`;
}

/** Tab indents in a code editor; it does not leave the box. */
async function indent(e: KeyboardEvent) {
  const box = area.value!;
  const { selectionStart: from, selectionEnd: to } = box;
  e.preventDefault();
  text.value = `${text.value.slice(0, from)}  ${text.value.slice(to)}`;
  // v-model writes the box back on the next flush; the caret moves after that.
  await nextTick();
  box.setSelectionRange(from + 2, from + 2);
}

onMounted(load);
watch(() => props.tab.file, load);
</script>

<template>
  <div class="code-tab">
    <header class="code-head">
      <span class="mono grow" :title="tab.file">{{ tab.file }}</span>
      <span v-if="dirty" class="note">Unsaved</span>
      <button class="btn" :disabled="!dirty" @click="save">Save</button>
    </header>
    <p v-if="error" class="note code-error" role="alert">{{ error }}</p>
    <p v-else-if="loading" class="note code-error">Opening…</p>
    <div v-else class="editor" :style="{ fontSize: `${dock.prefs.fontSize}px`, fontFamily: dock.prefs.fontFamily }">
      <div class="numbers" aria-hidden="true">
        <div ref="gutter">
          <span v-for="n in lines" :key="n">{{ n }}</span>
        </div>
      </div>
      <div class="layers">
        <pre ref="code" class="paint" aria-hidden="true"><code v-html="highlighted"></code></pre>
        <textarea
          ref="area"
          v-model="text"
          class="input"
          spellcheck="false"
          autocomplete="off"
          :aria-label="tab.file"
          @scroll="sync"
          @keydown.tab="indent"
          @keydown.ctrl.s.prevent="save"
        ></textarea>
      </div>
    </div>
  </div>
</template>

<style scoped>
.code-tab {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background: var(--bg);
}
.code-head {
  display: flex;
  align-items: center;
  gap: var(--gap);
  flex-shrink: 0;
  min-width: 0;
  padding: 6px 10px;
  border-bottom: 1px solid var(--border);
}
.code-head .mono {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: 11px;
  color: var(--text-faint);
}
.code-head .note {
  margin: 0;
  color: var(--warn);
}
.code-head .btn {
  padding: 4px 10px;
  font-size: 12px;
}
.code-error {
  margin: 0;
  padding: 12px;
  color: var(--err);
}
.editor {
  display: flex;
  flex: 1;
  min-height: 0;
  line-height: 1.5;
  tab-size: 2;
}
.numbers {
  flex-shrink: 0;
  overflow: hidden;
  padding: 10px 8px 10px 10px;
  border-right: 1px solid var(--border);
  color: var(--text-faint);
  text-align: right;
}
.numbers span {
  display: block;
}
.layers {
  position: relative;
  flex: 1;
  min-width: 0;
}
.paint,
.input {
  margin: 0;
  padding: 10px 12px;
  border: 0;
  font: inherit;
  line-height: inherit;
  tab-size: inherit;
  white-space: pre;
  overflow-wrap: normal;
}
.paint {
  position: absolute;
  inset: 0 auto auto 0;
  min-width: 100%;
  pointer-events: none;
  color: var(--text-dim);
}
.input {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  overflow: auto;
  resize: none;
  background: none;
  /* The painted copy underneath carries the colour; this layer only shows the
     caret and the selection, so the text itself must not print twice. */
  color: transparent;
  caret-color: var(--text);
}
.input::selection {
  background: var(--surface-2);
}

/* Syntax hues. The workspace palette says nothing about code, so these are
   their own tokens rather than green and blue borrowed from outcomes. */
.paint :deep(.hljs-comment),
.paint :deep(.hljs-quote) {
  color: var(--syntax-comment);
  font-style: italic;
}
.paint :deep(.hljs-keyword),
.paint :deep(.hljs-built_in),
.paint :deep(.hljs-literal),
.paint :deep(.hljs-type),
.paint :deep(.hljs-meta) {
  color: var(--syntax-keyword);
}
.paint :deep(.hljs-string),
.paint :deep(.hljs-regexp),
.paint :deep(.hljs-char.escape_),
.paint :deep(.hljs-addition) {
  color: var(--syntax-string);
}
.paint :deep(.hljs-number),
.paint :deep(.hljs-symbol),
.paint :deep(.hljs-variable),
.paint :deep(.hljs-template-variable),
.paint :deep(.hljs-deletion) {
  color: var(--syntax-number);
}
.paint :deep(.hljs-title),
.paint :deep(.hljs-title.function_),
.paint :deep(.hljs-title.class_),
.paint :deep(.hljs-section),
.paint :deep(.hljs-name),
.paint :deep(.hljs-selector-tag) {
  color: var(--syntax-name);
}
.paint :deep(.hljs-attr),
.paint :deep(.hljs-attribute),
.paint :deep(.hljs-property),
.paint :deep(.hljs-selector-class),
.paint :deep(.hljs-selector-id),
.paint :deep(.hljs-params) {
  color: var(--syntax-attr);
}
</style>
