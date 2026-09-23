<script setup lang="ts">
import { computed } from "vue";
import { openUrl } from "../api";
import { parseMarkdown } from "../markdown";

// An agent's reply, formatted. Built from parsed data, never v-html.
const props = defineProps<{ text: string }>();
const blocks = computed(() => parseMarkdown(props.text));
</script>

<template>
  <div class="md">
    <template v-for="(block, b) in blocks" :key="b">
      <pre v-if="block.kind === 'code'">{{ block.text }}</pre>
      <p v-else-if="block.kind === 'h'" class="h">
        <template v-for="(s, i) in block.spans" :key="i">
          <code v-if="s.kind === 'code'">{{ s.text }}</code><a v-else-if="s.kind === 'link'" :href="s.href" :title="s.href" @click.prevent="openUrl(s.href)">{{ s.text }}</a><template v-else>{{ s.text }}</template>
        </template>
      </p>
      <ul v-else-if="block.kind === 'list'">
        <li v-for="(item, n) in block.items" :key="n" :style="{ marginLeft: `${item.depth * 18}px` }">
          <span class="marker" aria-hidden="true">{{ item.marker }}</span>
          <span>
            <template v-for="(s, i) in item.spans" :key="i">
              <strong v-if="s.kind === 'bold'">{{ s.text }}</strong><code v-else-if="s.kind === 'code'">{{ s.text }}</code><a v-else-if="s.kind === 'link'" :href="s.href" :title="s.href" @click.prevent="openUrl(s.href)">{{ s.text }}</a><template v-else>{{ s.text }}</template>
            </template>
          </span>
        </li>
      </ul>
      <p v-else>
        <template v-for="(line, l) in block.lines" :key="l">
          <br v-if="l" />
          <template v-for="(s, i) in line" :key="i">
            <strong v-if="s.kind === 'bold'">{{ s.text }}</strong><code v-else-if="s.kind === 'code'">{{ s.text }}</code><a v-else-if="s.kind === 'link'" :href="s.href" :title="s.href" @click.prevent="openUrl(s.href)">{{ s.text }}</a><template v-else>{{ s.text }}</template>
          </template>
        </template>
      </p>
    </template>
  </div>
</template>

<style scoped>
.md {
  line-height: 1.55;
  overflow-wrap: anywhere;
}
.md > * {
  margin: 0 0 10px;
}
.md > :last-child {
  margin-bottom: 0;
}
.h {
  margin-top: 16px;
  font-weight: 600;
}
.md > .h:first-child {
  margin-top: 0;
}
strong {
  font-weight: 600;
}
a {
  color: var(--text);
  text-decoration: underline;
  text-underline-offset: 2px;
  cursor: pointer;
}
ul {
  padding: 0;
  list-style: none;
}
li {
  display: flex;
  gap: 8px;
}
li + li {
  margin-top: 4px;
}
.marker {
  flex: none;
  min-width: 10px;
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
}
code {
  padding: 1px 4px;
  background: var(--surface-2);
  border-radius: var(--r-sm);
  font-family: var(--mono);
  font-size: 12px;
}
pre {
  padding: 12px;
  overflow: auto;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  color: var(--text-dim);
  font-family: var(--mono);
  font-size: 12px;
  white-space: pre;
}
</style>
