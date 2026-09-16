<script setup lang="ts">
import { provide } from "vue";
import VeloMark from "../components/VeloMark.vue";
import AiHelpers from "./project/AiHelpers.vue";
import PastTask from "./project/PastTask.vue";
import TaskComposer from "./project/TaskComposer.vue";
import TaskResult from "./project/TaskResult.vue";
import TaskRun from "./project/TaskRun.vue";
import { PROJECT, useProject } from "./project/state";
import type { OpenedProject } from "../types";

// The shell: sidebar, top bar and whichever page the sidebar picked.
// All state lives in `useProject`; the pages inject it.
const props = defineProps<{ opened: OpenedProject }>();
defineEmits<{ close: [] }>();

const state = useProject(props.opened);
provide(PROJECT, state);
const {
  view, running, result, history, historyError, historyDetail, historyLine, showHistory, newTask,
  rows, helpersPending, helpersReady, TONE, HISTORY_STATUS,
  git, gitAsk, gitBusy, gitError, commitMessage, mergeBranch, gitQuestion, runGit,
} = state;
</script>

<template>
  <div class="workspace">
    <aside class="sidebar" aria-label="Workspace">
      <div class="brand"><VeloMark :size="26" /> Orteca</div>

      <button class="btn new-task" @click="newTask">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
        New task
      </button>

      <nav class="nav" aria-label="Project">
        <button
          v-if="running || result"
          :class="{ on: view === 'task' }"
          :aria-current="view === 'task' ? 'page' : undefined"
          @click="view = 'task'"
        >
          <span class="dot" :class="running ? 'live' : result && TONE[result.status]" aria-hidden="true"></span>
          <span class="grow">{{ running ? "Running now" : "Latest result" }}</span>
        </button>
        <button
          :class="{ on: view === 'helpers' }"
          :aria-current="view === 'helpers' ? 'page' : undefined"
          @click="view = 'helpers'"
        >
          <span class="dot" :class="{ ok: !helpersPending && helpersReady > 0 }" aria-hidden="true"></span>
          <span class="grow">AI helpers</span>
          <span class="count" :class="{ warn: !helpersPending && helpersReady < rows.length }">
            {{ helpersPending ? "checking" : `${helpersReady} of ${rows.length} ready` }}
          </span>
        </button>
      </nav>

      <h2 class="label side-label">Recent</h2>
      <p v-if="historyError" class="note side-note">History unavailable.</p>
      <p v-else-if="!history.length" class="note side-note">Finished tasks show up here.</p>
      <ul v-else class="recent">
        <li v-for="t in history" :key="t.id">
          <button
            :class="{ on: view === 'history' && historyDetail?.id === t.id }"
            :title="`${t.prompt}\n${historyLine(t)}`"
            @click="showHistory(t)"
          >
            <span class="dot" :class="TONE[t.status]" aria-hidden="true"></span>
            <span class="grow">{{ t.prompt }}</span>
            <span class="hidden-label">{{ HISTORY_STATUS[t.status] ?? t.status }}</span>
          </button>
        </li>
      </ul>

      <div class="side-project">
        <span class="grow" :title="opened.project.path">{{ opened.project.name }}</span>
        <button class="link" @click="$emit('close')">Switch project</button>
      </div>
    </aside>

    <div class="main">
      <header class="topbar">
        <h1 class="grow">{{ opened.project.name }}</h1>
        <span class="chip mono" title="Branch">{{ git.branch ?? "detached" }}</span>
        <span v-if="git.dirty" class="chip warn">{{ git.dirtyCount }} uncommitted</span>
        <span v-if="git.upstream" class="chip" :title="git.upstream">
          {{ git.behind ?? "—" }} behind · {{ git.ahead ?? "—" }} ahead
        </span>
        <div v-if="git.isRepo" class="git-buttons" role="group" aria-label="Git">
          <button class="btn" :disabled="running || gitBusy || !git.upstream" @click="runGit('fetch')">Fetch</button>
          <button class="btn" :disabled="running || gitBusy || !git.behind" @click="runGit('pull')">Pull</button>
          <button class="btn" :disabled="running || gitBusy || !git.dirty" @click="runGit('commit')">Commit</button>
          <button class="btn" :disabled="running || gitBusy || git.ahead === 0" @click="runGit('push')">Push</button>
          <button
            class="btn"
            :disabled="running || gitBusy || git.dirty || !git.branches.length"
            :title="git.dirty ? 'Commit your changes first' : undefined"
            @click="runGit('merge')"
          >
            Merge
          </button>
        </div>
      </header>

      <div v-if="gitAsk || gitError" class="git-confirm">
        <template v-if="gitAsk">
          <p class="note">{{ gitQuestion(gitAsk) }}</p>
          <input
            v-if="gitAsk === 'commit'"
            v-model="commitMessage"
            class="git-input"
            placeholder="Commit message"
            aria-label="Commit message"
            @keydown.enter="runGit('commit')"
          />
          <select v-if="gitAsk === 'merge'" v-model="mergeBranch" class="git-input mono" aria-label="Branch to merge">
            <option v-for="b in git.branches" :key="b" :value="b">{{ b }}</option>
          </select>
          <div class="git-buttons">
            <button
              class="btn git-yes"
              :disabled="gitBusy || (gitAsk === 'commit' && !commitMessage.trim()) || (gitAsk === 'merge' && !mergeBranch)"
              @click="runGit(gitAsk)"
            >
              {{ gitBusy ? "Working…" : "Yes, " + gitAsk }}
            </button>
            <button class="btn" :disabled="gitBusy" @click="gitAsk = null">Cancel</button>
          </div>
        </template>
        <p v-if="gitError" class="git-error">{{ gitError }}</p>
      </div>

      <main class="content">
        <template v-if="view === 'task'">
          <TaskRun v-if="running" />
          <TaskResult v-else-if="result" />
          <TaskComposer v-else />
        </template>
        <PastTask v-else-if="view === 'history'" />
        <AiHelpers v-else />
      </main>
    </div>
  </div>
</template>

<style scoped>
.workspace {
  display: grid;
  grid-template-columns: 248px minmax(0, 1fr);
  height: 100%;
}

.sidebar {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-height: 0;
  padding: 16px 12px;
  background: var(--surface);
  border-right: 1px solid var(--border);
}
.brand {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 6px 16px;
  font-size: 14px;
  font-weight: 600;
}
.new-task {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 12px;
}

.nav,
.recent {
  display: grid;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.nav button,
.recent button {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 7px 10px;
  border-radius: var(--r-sm);
  color: var(--text-dim);
  text-align: left;
  transition: background 120ms ease, color 120ms ease;
}
.nav button:hover,
.recent button:hover,
.nav button.on,
.recent button.on {
  background: var(--surface-2);
  color: var(--text);
}
.count {
  font-size: 11px;
  color: var(--text-faint);
}
.count.warn {
  color: var(--warn);
}

.side-label {
  margin: 20px 10px 6px;
}
.side-note {
  margin: 0 10px;
}
.recent {
  flex: 1;
  align-content: start;
  min-height: 0;
  overflow-y: auto;
}
.side-project {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: auto;
  padding: 12px 10px 0;
  border-top: 1px solid var(--border);
  font-size: 12px;
}
.recent + .side-project {
  margin-top: 8px;
}

.main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow-y: auto;
}
.topbar {
  position: sticky;
  top: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  padding: 12px 24px;
  background: var(--bg);
  border-bottom: 1px solid var(--border);
}
h1 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}
.git-buttons {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-left: 8px;
}
.git-buttons .btn {
  padding: 3px 10px;
  font-size: 12px;
}
.git-confirm {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
  padding: 12px 24px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
}
.git-confirm .note,
.git-error {
  flex-basis: 100%;
  margin: 0;
}
.git-confirm .git-buttons {
  margin-left: 0;
}
.git-yes {
  color: var(--warn);
}
.git-input {
  min-width: 280px;
  padding: 6px 10px;
  background: var(--bg);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  font: inherit;
}
.git-input::placeholder {
  color: var(--text-faint);
}
.git-error {
  font-size: 12px;
  color: var(--err);
  white-space: pre-wrap;
}

.content {
  width: 100%;
  max-width: 760px;
  margin: 0 auto;
  padding: 40px 24px 64px;
}

@media (max-width: 700px) {
  .workspace {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto minmax(0, 1fr);
  }
  .sidebar {
    border-right: none;
    border-bottom: 1px solid var(--border);
  }
  .recent {
    max-height: 120px;
  }
  .git-buttons {
    margin-left: 0;
  }
}
</style>
