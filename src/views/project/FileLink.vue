<script setup lang="ts">
import { computed, inject } from "vue";
import { PROJECT } from "./state";

// A file a run touched, shown by name. Clicking it offers to open it or show it in Explorer.
const props = defineProps<{ file: string }>();
const { openActivityFile } = inject(PROJECT)!;
const name = computed(() => props.file.split(/[\\/]/).pop() || props.file);

function pick(event: Event, reveal: boolean) {
  (event.currentTarget as HTMLElement).closest("details")?.removeAttribute("open");
  void openActivityFile(props.file, reveal);
}
function closeOnLeave(event: FocusEvent) {
  const details = event.currentTarget as HTMLDetailsElement;
  if (!details.contains(event.relatedTarget as Node | null)) details.open = false;
}
</script>

<template>
  <details class="file" @focusout="closeOnLeave">
    <summary :title="file">{{ name }}</summary>
    <span class="choices">
      <button class="link" @click="pick($event, false)">Open</button>
      <button class="link" @click="pick($event, true)">Show in folder</button>
    </span>
  </details>
</template>

<style scoped>
.file {
  display: inline;
}
.file summary {
  display: inline;
  list-style: none;
  cursor: pointer;
  text-decoration: underline;
  text-decoration-color: var(--border-strong);
  text-underline-offset: 3px;
}
.file summary::-webkit-details-marker {
  display: none;
}
.file summary:hover,
.file[open] summary {
  text-decoration-color: currentColor;
}
.choices {
  display: inline-flex;
  gap: 12px;
  margin-left: 12px;
}
</style>
