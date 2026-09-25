<script setup lang="ts">
import Markdown from "../../components/Markdown.vue";
import FileLink from "./FileLink.vue";
import type { ActivityLine } from "./state";
import { activityPatch } from "./historyPatch";

// How the agent got there, in order: what it said, thought and did, and every
// steer the user sent. Streams open while it runs; folds away under the answer.
defineProps<{ lines: ActivityLine[] }>();
// Only edits that carry their own hunks; a name-only edit waits for the task diff.
const edits = (m: ActivityLine) => m.failed ? [] : (m.changes ?? []).flatMap((c) => {
  const file = c.patch === null ? undefined : activityPatch(c.path, c.patch, [], "").file;
  return file ? [file] : [];
});
</script>

<template>
  <template v-for="(m, i) in lines" :key="i">
    <div v-if="m.kind === 'instruction'" class="mine">
      <p class="bubble">{{ m.text }}</p>
      <p v-if="m.delivery" class="note" role="status">{{ m.delivery }}</p>
    </div>
    <Markdown v-else-if="m.kind === 'thinking'" class="summary back thought" :text="m.text" />
    <details v-else-if="m.kind === 'toolUse' && edits(m).length" class="back">
      <summary class="step">{{ m.text }} <FileLink v-if="m.file" :file="m.file" />
        <span class="added">+{{ edits(m).reduce((n, f) => n + f.added, 0) }}</span> <span class="removed">−{{ edits(m).reduce((n, f) => n + f.removed, 0) }}</span></summary>
      <div class="edit-code">
        <div v-for="(line, j) in edits(m).flatMap((f) => f.lines)" :key="j" class="patch-line" :class="line.kind"><code>{{ line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : line.kind === 'hunk' ? '@@ ' : ' ' }}{{ line.text }}</code></div>
      </div>
    </details>
    <p v-else-if="m.kind === 'toolUse'" class="back step" :class="{ failed: m.failed }">{{ m.text }} <FileLink v-if="m.file" :file="m.file" /> {{ m.lines }}</p>
    <Markdown v-else class="summary back" :text="m.text" />
  </template>
</template>

<style scoped src="./result.css"></style>
<style scoped src="./chat.css"></style>
<style scoped>
.thought {
  color: var(--text-faint);
  font-size: 12px;
}
.step {
  margin: 0;
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text-faint);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.step.failed {
  color: var(--err);
}
summary.step {
  cursor: pointer;
}
.step .added {
  color: var(--syntax-string);
}
.step .removed {
  color: var(--syntax-number);
}
.edit-code {
  max-height: 320px;
  overflow: auto;
  margin: 4px 0 8px;
  border: 1px solid var(--border);
}
</style>
