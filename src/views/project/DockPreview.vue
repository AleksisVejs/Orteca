<script setup lang="ts">
import { inject, ref } from "vue";
import { DOCK } from "./dock";
import type { Tab } from "./dock";

// The dev server's own page, inside the workspace. Orteca does not start it —
// a terminal tab does, and the address it prints lands here.
const props = defineProps<{ tab: Tab }>();
const dock = inject(DOCK)!;
// A nested ref does not unwrap in the template; this one is used there.
const devUrls = dock.devUrls;

const typed = ref(props.tab.url ?? "");
const reloads = ref(0);
/** A narrow frame to check a layout, without leaving the workspace. */
const width = ref("0");

function go() {
  const url = typed.value.trim();
  props.tab.url = /^https?:\/\//i.test(url) ? url : `http://${url}`;
  typed.value = props.tab.url;
  reloads.value++;
}

function use(url: string) {
  typed.value = url;
  go();
}
</script>

<template>
  <div class="preview-tab">
    <header class="preview-head">
      <button class="head-button" title="Reload the page" aria-label="Reload the page" @click="reloads++">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M13 6a5 5 0 0 0-8.5-2L2 6m0-4v4h4M3 10a5 5 0 0 0 8.5 2L14 10m0 4v-4h-4" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
      </button>
      <input v-model="typed" class="address mono" aria-label="Address" placeholder="http://localhost:5173" @keydown.enter="go" />
      <select v-model="width" class="address size" aria-label="Frame width">
        <option value="0">Full width</option>
        <option value="1280">1280</option>
        <option value="768">768</option>
        <option value="375">375</option>
      </select>
    </header>
    <p v-if="devUrls.length" class="found">
      <span class="note">Seen in a terminal:</span>
      <button v-for="url in devUrls" :key="url" class="link mono" @click="use(url)">{{ url }}</button>
    </p>
    <div class="frame-well">
      <iframe
        v-if="tab.url"
        :key="`${tab.url}#${reloads}`"
        :src="tab.url"
        :style="width === '0' ? undefined : { width: `${width}px` }"
        class="frame"
        :title="`Preview of ${tab.url}`"
        sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-modals"
      ></iframe>
      <p v-else class="note empty">Start a dev server in a terminal tab, or type an address above.</p>
    </div>
  </div>
</template>

<style scoped>
.preview-tab {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background: var(--bg);
}
.preview-head {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  padding: 6px 8px;
  border-bottom: 1px solid var(--border);
}
.head-button {
  display: flex;
  align-items: center;
  padding: 5px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  transition: background 120ms ease, color 120ms ease;
}
.head-button:hover {
  background: var(--surface-2);
  color: var(--text);
}
.address {
  flex: 1;
  min-width: 0;
  padding: 5px 8px;
  background: var(--surface);
  color: var(--text);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  font: inherit;
  font-size: 11px;
}
.address::placeholder {
  color: var(--text-faint);
}
.size {
  flex: 0 0 auto;
}
.found {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 10px;
  margin: 0;
  padding: 6px 10px;
  border-bottom: 1px solid var(--border);
}
.found .note {
  margin: 0;
}
.found .link {
  font-size: 11px;
}
.frame-well {
  display: flex;
  justify-content: center;
  flex: 1;
  min-height: 0;
  overflow: auto;
  background: var(--surface);
}
.frame {
  width: 100%;
  height: 100%;
  border: 0;
  background: var(--bg);
}
.empty {
  align-self: center;
  max-width: 40ch;
  text-align: center;
}
</style>
