<script setup lang="ts">
import { inject } from "vue";
import FileLink from "./FileLink.vue";
import { PROJECT } from "./state";

// The live, plain-English view of a run. The full technical log is saved with the task.
const { lines } = inject(PROJECT)!;
</script>

<template>
  <p v-if="!lines.length" class="note">Nothing to show yet.</p>
  <ol v-else class="card stream" role="log" aria-live="polite" aria-relevant="additions">
    <li v-for="(line, i) in lines" :key="i" :class="line.kind">
      <span v-if="line.kind === 'instruction'" class="said">you</span>
      {{ line.text }}
      <FileLink v-if="line.file" :file="line.file" />
    </li>
  </ol>
</template>

<style scoped>
.stream {
  margin: 0;
  padding: 12px 18px;
  list-style: none;
  max-height: 360px;
  overflow-y: auto;
}
.stream li {
  padding: 3px 0;
  color: var(--text-dim);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
/* What the agent says is the story; its tool calls are the footnotes. */
.stream li.text {
  padding: 10px 0 6px;
  color: var(--text);
}
.stream li.toolUse {
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text-faint);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.stream li.failed {
  color: var(--err);
}
/* The user's own words, marked as theirs rather than as something said back. */
.stream li.instruction {
  color: var(--text);
}
.said {
  margin-right: 8px;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text-faint);
}
</style>
