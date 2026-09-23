<script setup lang="ts">
import Markdown from "../../components/Markdown.vue";
import FileLink from "./FileLink.vue";
import type { ActivityLine } from "./state";

// How the agent got there, in order: what it said, thought and did, and every
// steer the user sent. Streams open while it runs; folds away under the answer.
defineProps<{ lines: ActivityLine[] }>();
</script>

<template>
  <template v-for="(m, i) in lines" :key="i">
    <div v-if="m.kind === 'instruction'" class="mine">
      <p class="bubble">{{ m.text }}</p>
      <p v-if="m.delivery" class="note" role="status">{{ m.delivery }}</p>
    </div>
    <Markdown v-else-if="m.kind === 'thinking'" class="summary back thought" :text="m.text" />
    <p v-else-if="m.kind === 'toolUse'" class="back step" :class="{ failed: m.failed }">{{ m.text }} <FileLink v-if="m.file" :file="m.file" /></p>
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
</style>
