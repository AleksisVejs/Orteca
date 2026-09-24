<script setup lang="ts">
import { computed, h } from "vue";
import { openUrl } from "../api";
import { parseMarkdown, type Span } from "../markdown";

// An agent's reply, formatted. Built from parsed data, never v-html.
const props = defineProps<{ text: string }>();
const blocks = computed(() => parseMarkdown(props.text));

const tags = { bold: "strong", italic: "em", strike: "s", code: "code" } as const;
const Spans = (p: { spans: Span[] }) =>
  p.spans.map((s) =>
    s.kind === "link"
      ? h("a", { href: s.href, title: s.href, onClick: (e: Event) => (e.preventDefault(), openUrl(s.href)) }, s.text)
      : s.kind === "text"
        ? s.text
        : h(tags[s.kind], s.text),
  );
Spans.props = ["spans"];
</script>

<template>
  <div class="md">
    <template v-for="(block, b) in blocks" :key="b">
      <pre v-if="block.kind === 'code'">{{ block.text }}</pre>
      <hr v-else-if="block.kind === 'hr'" />
      <p v-else-if="block.kind === 'h'" class="h"><Spans :spans="block.spans" /></p>
      <ul v-else-if="block.kind === 'list'">
        <li v-for="(item, n) in block.items" :key="n" :style="{ marginLeft: `${item.depth * 18}px` }">
          <span class="marker" aria-hidden="true">{{ item.marker }}</span>
          <span><Spans :spans="item.spans" /></span>
        </li>
      </ul>
      <div v-else-if="block.kind === 'table'" class="table">
        <table>
          <thead>
            <tr>
              <th v-for="(cell, c) in block.head" :key="c"><Spans :spans="cell" /></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(row, r) in block.rows" :key="r">
              <td v-for="(cell, c) in row" :key="c"><Spans :spans="cell" /></td>
            </tr>
          </tbody>
        </table>
      </div>
      <component :is="block.kind === 'quote' ? 'blockquote' : 'p'" v-else>
        <template v-for="(line, l) in block.lines" :key="l">
          <br v-if="l" />
          <Spans :spans="line" />
        </template>
      </component>
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
/* Spans renders these, so they carry no scope id of this file. */
.md :deep(strong) {
  font-weight: 600;
}
.md :deep(a) {
  color: var(--text);
  text-decoration: underline;
  text-underline-offset: 2px;
  cursor: pointer;
}
.md :deep(code) {
  padding: 1px 4px;
  background: var(--surface-2);
  border-radius: var(--r-sm);
  font-family: var(--mono);
  font-size: 12px;
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
blockquote {
  padding-left: 10px;
  border-left: 2px solid var(--border);
  color: var(--text-dim);
}
hr {
  border: 0;
  border-top: 1px solid var(--border);
}
.table {
  overflow-x: auto;
}
table {
  border-collapse: collapse;
  overflow-wrap: normal;
}
th,
td {
  padding: 6px 10px;
  border: 1px solid var(--border);
  text-align: left;
  vertical-align: top;
}
th {
  background: var(--surface-2);
  font-weight: 600;
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
