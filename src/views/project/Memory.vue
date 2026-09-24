<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { addMemory, deleteMemory, editMemory, isAppError, memory, proposeMemoryFromRuns, proposeMemoryImport, readText, repoProfile, setMemoryLimit, setRepoProfile } from "../../api";
import type { ImportFile, MemoryProposal, MemoryState, RepoProfile } from "../../types";

// Short standing instructions the user writes; every run is told them. One
// component for both screens: with a path it lists this project and all
// projects, without one (the Launch screen) only all projects. With a
// `popId` it is a button and popover (Launch); without, a workspace page.
const props = defineProps<{ path?: string; popId?: string; buttonClass?: string }>();
const emit = defineEmits<{ changed: [] }>();

const LIMITS = [500, 1000, 2000, 0];
const state = ref<MemoryState>({ items: [], limit: 1000, tokens: 0 });
const text = ref("");
const target = ref<"project" | "global">(props.path ? "project" : "global");
const editing = ref<number | null>(null);
const draft = ref("");
const error = ref("");

const lists = computed(() => [
  { name: "This project", global: false, show: !!props.path },
  { name: "All projects", global: true, show: true },
].filter((l) => l.show).map((l) => ({ ...l, items: state.value.items.filter((i) => i.global === l.global) })));

const meter = computed(() => {
  const size = `~${state.value.tokens.toLocaleString()}`;
  const limit = state.value.limit ? ` / ${state.value.limit.toLocaleString()}` : "";
  return `${size}${limit} tokens · estimated · ${state.value.limit ? "" : "no limit · "}sent with every stage`;
});

// The repo's own instruction files reach a run only as memory. Which ones
// exist decides which import buttons show.
const REPO_FILES = ["CLAUDE.md", "AGENTS.md"] as const;
const repoFiles = ref<ImportFile[]>([]);

async function loadRepoFiles() {
  const path = props.path;
  if (!path) return;
  const found = await Promise.all(REPO_FILES.map((name) => readText(path, name).then(() => name, () => null)));
  repoFiles.value = found.filter((n) => n !== null);
}

// How to build and test this repository and what not to read, sent with
// every run's rules. Detected until the user edits it.
const profile = ref<RepoProfile | null>(null);
const profileDraft = ref("");

async function loadProfile() {
  if (!props.path) return;
  try {
    profile.value = await repoProfile(props.path);
    profileDraft.value = profile.value.text;
  } catch (e) {
    error.value = isAppError(e) ? e.message : String(e);
  }
}

// Saving the detected text as-is keeps it detected, so it follows the repo.
const saveProfile = (text: string | null) =>
  change(() => setRepoProfile(props.path!, text === null || text.trim() === profile.value?.detected ? null : text));

/** A rough size for a long file: long rules are paid for on every turn. */
const longFile = computed(() => {
  const chars = proposal.value?.original.length ?? 0;
  return chars > 4000 && source.value !== "runs" ? Math.round(chars / 4) : 0;
});

async function load() {
  void loadRepoFiles();
  void loadProfile();
  try {
    state.value = await memory(props.path);
  } catch (e) {
    error.value = isAppError(e) ? e.message : String(e);
  }
}

/** Run a change, then reload; a refusal (the limit) shows as the error. */
onMounted(() => props.popId || load());

async function change(work: () => Promise<void>) {
  error.value = "";
  try {
    await work();
  } catch (e) {
    error.value = isAppError(e) ? e.message : String(e);
    return false;
  }
  await load();
  emit("changed");
  return true;
}

// Import: the original beside what a small model proposed. Nothing is saved
// until the user keeps some rules and confirms. `runs` is this project's
// recent runs rather than a file.
type Source = ImportFile | "runs";
const proposal = ref<MemoryProposal | null>(null);
const picked = ref<{ text: string; keep: boolean }[]>([]);
const reading = ref(false);
const source = ref<Source>("user");
const scopeName = computed(() => (source.value === "user" ? "All projects" : "This project"));
const sourceName = computed(() =>
  source.value === "user" ? "~/.claude/CLAUDE.md" : source.value === "runs" ? "recent runs" : source.value,
);
const importFiles = (global: boolean): Source[] => (global ? ["user"] : [...repoFiles.value, "runs"]);
const importLabel = (f: Source) =>
  f === "runs" ? "Suggest from recent runs" : `Import ${f === "user" ? "~/.claude/CLAUDE.md" : f}`;
/** Whether the open review belongs to this list. */
const reviewing = (global: boolean) => !!proposal.value && (source.value === "user") === global;

async function proposeImport(file: Source) {
  source.value = file;
  error.value = "";
  reading.value = true;
  try {
    proposal.value = file === "runs" ? await proposeMemoryFromRuns(props.path!) : await proposeMemoryImport(props.path, file);
    picked.value = proposal.value.bullets.map((text) => ({ text, keep: true }));
    if (!picked.value.length) {
      error.value = file === "runs" ? "Nothing in the recent runs looked worth a standing rule." : "The model proposed no rules from that file.";
    }
  } catch (e) {
    error.value = isAppError(e) ? e.message : String(e);
  }
  reading.value = false;
}

async function saveImport() {
  for (const b of picked.value.filter((b) => b.keep)) {
    // The first refusal (the limit) stops the rest, and what was saved stays.
    if (!(await change(() => addMemory(props.path, source.value === "user", b.text)))) return;
    b.keep = false;
  }
  proposal.value = null;
}

async function add() {
  if (await change(() => addMemory(props.path, target.value === "global", text.value))) text.value = "";
}

function edit(id: number, current: string) {
  editing.value = id;
  draft.value = current;
}

async function save(id: number) {
  if (await change(() => editMemory(id, draft.value))) editing.value = null;
}
</script>

<template>
  <button v-if="popId" :class="buttonClass ?? 'btn'" :popovertarget="popId" title="Standing instructions every run receives"><slot>Memory</slot></button>
  <div
    :id="popId"
    class="memory"
    :popover="popId ? '' : undefined"
    :role="popId ? 'dialog' : undefined"
    :aria-labelledby="popId && `${popId}-title`"
    @toggle="(e) => (e as ToggleEvent).newState === 'open' && load()"
  >
    <header class="head">
      <div>
        <h2 :id="popId && `${popId}-title`" class="title">Memory</h2>
        <p class="lede">Short rules sent with every task, to Claude and Codex alike.</p>
      </div>
      <button v-if="popId" class="close" :popovertarget="popId" popovertargetaction="hide" title="Close" aria-label="Close memory">
        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
      </button>
    </header>

    <!-- Adding a rule is the common job, so it comes first. -->
    <form class="card add" @submit.prevent="add">
      <input v-model="text" placeholder="Add a rule, like “Use pnpm, never npm”" aria-label="New rule" />
      <div v-if="path" class="segments" role="group" aria-label="Save the rule for">
        <button type="button" class="seg" :class="{ on: target === 'project' }" :aria-pressed="target === 'project'" @click="target = 'project'">This project</button>
        <button type="button" class="seg" :class="{ on: target === 'global' }" :aria-pressed="target === 'global'" @click="target = 'global'">All projects</button>
      </div>
      <button class="btn primary" :disabled="!text.trim()">Add rule</button>
    </form>
    <p v-if="error" class="err" role="alert">{{ error }}</p>

    <section v-for="list in lists" :key="list.name" :aria-label="list.name">
      <div class="section-head">
        <h3 class="label">{{ list.name }} <span class="count">{{ list.items.length }}</span></h3>
        <span v-if="!reviewing(list.global)" class="imports">
          <button v-for="f in importFiles(list.global)" :key="f" class="link" :disabled="reading" @click="proposeImport(f)">
            {{ reading && source === f ? "Reading…" : importLabel(f) }}
          </button>
        </span>
      </div>

      <div v-if="reviewing(list.global) && proposal" class="card review">
        <p class="review-title">Rules proposed from <strong>{{ sourceName }}</strong></p>
        <details>
          <summary>{{ source === "runs" ? "Show the runs it read" : "Show the original file" }}</summary>
          <pre aria-label="What the rules came from">{{ proposal.original }}</pre>
        </details>
        <ul aria-label="Proposed rules">
          <li v-for="(b, i) in picked" :key="i">
            <input v-model="b.keep" type="checkbox" class="keep" :aria-label="`Keep: ${b.text}`" />
            <textarea v-model="b.text" rows="2" :aria-label="`Rule ${i + 1}`"></textarea>
          </li>
        </ul>
        <p v-if="longFile" class="note">This file is long, about {{ longFile.toLocaleString() }} tokens, estimated. Long rules are paid for on every turn and can make runs worse, so keep only the few that matter.</p>
        <p class="note">Nothing is saved until you confirm.<template v-if="source !== 'runs'"> These become copies: changing the file later does not change them.</template></p>
        <div class="actions-row">
          <button class="btn" :disabled="!picked.some((b) => b.keep)" @click="saveImport">Save {{ picked.filter((b) => b.keep).length }} to {{ scopeName }}</button>
          <button class="link" @click="proposal = null">Cancel</button>
        </div>
      </div>

      <ul v-if="list.items.length" class="card items">
        <li v-for="item in list.items" :key="item.id">
          <template v-if="editing === item.id">
            <input v-model="draft" :aria-label="`Edit: ${item.text}`" @keydown.enter="save(item.id)" @keydown.esc.stop="editing = null" />
            <button class="btn" @click="save(item.id)">Save</button>
            <button class="link" @click="editing = null">Cancel</button>
          </template>
          <template v-else>
            <span class="text">{{ item.text }}</span>
            <span class="actions">
              <button class="act" title="Edit rule" :aria-label="`Edit: ${item.text}`" @click="edit(item.id, item.text)">
                <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="m10.5 2.5 3 3-8 8H2.5v-3Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
              </button>
              <button class="act" title="Delete rule" :aria-label="`Delete: ${item.text}`" @click="change(() => deleteMemory(item.id))">
                <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
              </button>
            </span>
          </template>
        </li>
      </ul>
      <p v-else-if="!reviewing(list.global)" class="card empty note">No rules yet.</p>
    </section>

    <section v-if="path && profile" aria-label="Repository profile">
      <div class="section-head">
        <h3 class="label">Repository profile <span class="count">{{ profile.custom ? "edited" : "detected" }}</span></h3>
        <button v-if="profile.custom" class="link" @click="saveProfile(null)">Use detected</button>
      </div>
      <div class="card profile">
        <p class="note">Sent with every run's rules: how to build and test, the folders, and what not to read. Keep it to a few lines.</p>
        <textarea v-model="profileDraft" rows="7" aria-label="Repository profile"></textarea>
        <div class="actions-row">
          <button class="btn" :disabled="profileDraft === profile.text" @click="saveProfile(profileDraft)">Save profile</button>
        </div>
      </div>
    </section>

    <section aria-label="Size">
      <h3 class="label">Size</h3>
      <label class="card limit">
        <span class="text">
          <span class="name">Limit</span>
          <span class="note" role="status">{{ meter }}</span>
        </span>
        <select :value="state.limit" @change="change(() => setMemoryLimit(Number(($event.target as HTMLSelectElement).value)))">
          <option v-for="n in LIMITS" :key="n" :value="n">{{ n ? `${n.toLocaleString()} tokens` : "No limit" }}</option>
        </select>
      </label>
      <p class="note hint">Instruction files like <code>CLAUDE.md</code> and <code>AGENTS.md</code> are not sent on their own. Import the rules you want, and you review them before anything is saved.</p>
    </section>
  </div>
</template>

<style scoped>
/* A workspace page by default, laid out like Settings; on Launch the same
   content sits in a popover, one step smaller. */
.memory {
  color: var(--text);
  font-size: 14px;
}
.memory[popover] {
  width: min(560px, calc(100vw - 24px));
  max-width: none;
  max-height: calc(100dvh - 60px);
  overflow-y: auto;
  padding: 20px;
  background: var(--surface);
  border: 1px solid var(--border-strong);
  border-radius: var(--r);
  font-size: 12px;
}
.memory[popover]::backdrop {
  background: var(--overlay);
}
.memory[popover] .title {
  font-size: 14px;
}
.memory[popover] .card {
  background: var(--bg);
}
.memory[popover] section {
  margin-top: 20px;
}

.head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}
.title {
  margin: 0;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
}
.lede {
  margin: 6px 0 0;
  color: var(--text-dim);
}
section {
  margin-top: 32px;
}
.section-head {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  justify-content: space-between;
  gap: 4px 12px;
}
.count {
  margin-left: 4px;
  color: var(--text-faint);
  font-weight: 400;
  font-variant-numeric: tabular-nums;
}
.imports {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 14px;
  margin-bottom: 10px;
}
.link:disabled {
  color: var(--text-faint);
  cursor: default;
}
code {
  font-family: var(--mono);
  font-size: 11px;
}

.close,
.act {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  transition: background 120ms ease, color 120ms ease;
}
.close {
  width: 28px;
  height: 28px;
  margin: -4px -6px 0 0;
}
.act {
  width: 26px;
  height: 26px;
}
.close:hover,
.act:hover {
  background: var(--surface-2);
  color: var(--text);
}

.add {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  margin-top: 24px;
  padding: 12px;
}
.add input {
  flex: 1 1 240px;
}

.segments {
  display: inline-flex;
  flex: none;
  gap: 2px;
  padding: 2px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.seg {
  min-height: 26px;
  padding: 2px 10px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  font-size: 12px;
  transition: color 120ms ease, background 120ms ease;
}
.seg:hover {
  color: var(--text);
}
.seg.on {
  background: var(--surface-2);
  color: var(--text);
}

ul {
  margin: 0;
  padding: 0;
  list-style: none;
}
.items {
  padding: 4px 8px;
}
.items li {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 44px;
  padding: 6px 4px 6px 10px;
  border-radius: var(--r-sm);
  transition: background 120ms ease;
}
.items li + li {
  border-top: 1px solid var(--border);
}
.items li:hover,
.items li:focus-within {
  background: var(--surface-2);
}
.text {
  flex: 1;
  min-width: 0;
  line-height: 1.45;
  overflow-wrap: anywhere;
}
/* Row actions stay out of the way until the row is pointed at or focused. */
.actions {
  display: flex;
  gap: 2px;
  opacity: 0;
  transition: opacity 120ms ease;
}
.items li:hover .actions,
.items li:focus-within .actions {
  opacity: 1;
}
.empty {
  margin: 0;
  padding: 16px 18px;
}

.review,
.profile {
  padding: 16px 18px;
}
.review + .items {
  margin-top: 12px;
}
.review-title {
  margin: 0 0 8px;
}
.review details {
  margin-bottom: 10px;
  color: var(--text-dim);
}
.review summary {
  cursor: pointer;
}
.review li {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin-bottom: 8px;
}
.review .note,
.profile .note {
  margin: 0 0 10px;
}
.keep {
  flex: none;
  width: 16px;
  height: 16px;
  margin-top: 8px;
  accent-color: var(--button);
}
pre {
  margin: 6px 0 0;
  max-height: 200px;
  overflow: auto;
  padding: 8px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  white-space: pre-wrap;
  font: 11px var(--mono);
}
/* The profile is commands and paths, so it reads as mono. */
.profile textarea {
  display: block;
  width: 100%;
  font-family: var(--mono);
}
.actions-row {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 12px;
}

.limit {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 12px 18px;
}
.limit .text {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.limit select {
  flex: none;
  width: 160px;
}
.hint {
  margin: 12px 0 0;
  line-height: 1.5;
}

input:not([type="checkbox"]),
textarea,
select {
  min-height: 32px;
  padding: 5px 8px;
  color: var(--text);
  background: var(--bg);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  font: inherit;
  font-size: 12px;
}
.items input,
.review textarea {
  flex: 1;
  min-width: 0;
}
textarea {
  resize: vertical;
  line-height: 1.4;
}
input:focus-visible,
textarea:focus-visible,
select:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 1px;
}
.err {
  margin: 8px 0 0;
  color: var(--err);
}
@media (max-width: 600px) {
  .limit { flex-wrap: wrap; }
}
</style>
