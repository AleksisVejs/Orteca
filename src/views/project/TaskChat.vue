<script setup lang="ts">
import { inject, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import ExchangeView from "./Exchange.vue";
import { PROJECT } from "./state";
import type { Exchange } from "./state";

// One task as a chat, live or from history: earlier exchanges, the current one
// with its answer where the run left it, then the box to reply. The pane
// heading carries the status, so the answer is only what was said. A run going
// fills `current` and `composer` itself, and the page stays as it finishes.
const props = defineProps<{
  taskKey: PropertyKey;
  idPrefix: string;
  prompt: string;
  data?: Exchange;
  canReply: boolean;
  /** What a reply costs, short enough for the box; `replyHint` says it in full. */
  replyNote: string;
  replyHint: string;
  editDisabled?: boolean;
  /** What "Rewind" on the current message puts back; null when it cannot. */
  rewindFiles?: string[] | null;
  /** Start at the bottom, where a running task's newest message is. */
  follow?: boolean;
}>();
const emit = defineEmits<{ send: []; editAgain: [text: string]; rewind: [text: string] }>();

const {
  domId, newTask, reply, running, rewindError, attachments, attachError, addAttachments, pasteImages, fileName,
} = inject(PROJECT)!;

const id = (name: string) => domId(`${props.idPrefix}-${name}`);
const current = ref<InstanceType<typeof ExchangeView> | null>(null);

// A running task keeps to the bottom. Anything opened fresh starts at the top
// of the answer when it is too long to fit. Like any chat, new messages keep
// the pane at the bottom, unless the reader scrolled up.
const chat = ref<HTMLDivElement | null>(null);
const pane = () => chat.value?.parentElement;
let pinned = true;
const atBottom = (p: HTMLElement) => p.scrollHeight - p.scrollTop - p.clientHeight < 48;
function onPaneScroll() {
  const p = pane();
  if (p) pinned = atBottom(p);
}
function land(follow: boolean) {
  const p = pane();
  if (!p) return;
  p.scrollTop = p.scrollHeight;
  const answers = chat.value?.querySelectorAll(".answer");
  const a = answers?.[answers.length - 1];
  if (!follow && a && a.getBoundingClientRect().top < p.getBoundingClientRect().top) a.scrollIntoView({ block: "start" });
  pinned = atBottom(p);
}
function stick() {
  const p = pane();
  if (p && pinned) p.scrollTop = p.scrollHeight;
}
defineExpose({ land, stick });
onMounted(() => {
  pane()?.addEventListener("scroll", onPaneScroll, { passive: true });
  land(!!props.follow);
});
onBeforeUnmount(() => pane()?.removeEventListener("scroll", onPaneScroll));
watch(() => props.taskKey, () => {
  current.value?.close();
  void nextTick(() => land(!!props.follow));
});

// Enter sends, Shift+Enter breaks the line: the same as the steer box while it ran.
function onEnter(e: KeyboardEvent) {
  if (e.isComposing) return;
  e.preventDefault();
  if (!running.value && reply.value.trim()) emit("send");
}
</script>

<template>
  <div ref="chat" class="chat">
    <div class="thread">
      <slot name="earlier" />
      <slot name="current">
        <ExchangeView
          v-if="data"
          ref="current"
          :key="taskKey"
          :id-prefix="idPrefix"
          :prompt="prompt"
          :data="data"
          editable
          :edit-disabled="editDisabled"
          :rewind-files="rewindFiles"
          @edit-again="(text: string) => emit('editAgain', text)"
          @rewind="(text: string) => emit('rewind', text)"
        >
          <template #notes><slot name="notes" /></template>
          <template #after><slot name="after" /></template>
          <template #files-top><slot name="files-top" /></template>
          <template #details-end><slot name="details-end" /></template>
          <template #activity><slot name="activity" /></template>
        </ExchangeView>
      </slot>
      <p v-if="rewindError" class="missing" role="alert">{{ rewindError }}</p>
    </div>

    <!-- The same box the run was steered from, now taking a reply. -->
    <div :id="id('composer')" class="composer-bar">
      <slot name="composer">
        <div v-if="canReply" class="composer">
          <label class="hidden-label" :for="id('reply')">Reply</label>
          <textarea
            :id="id('reply')"
            v-model="reply"
            rows="1"
            spellcheck="false"
            placeholder="Reply, or ask for something more…"
            :disabled="running"
            @paste="pasteImages"
            @keydown.enter.exact="onEnter"
          ></textarea>
          <ul v-if="attachments.length" class="attachments" aria-label="Attached">
            <li v-for="path in attachments" :key="path" class="chip">
              <span class="mono" :title="path">{{ fileName(path) }}</span>
              <button
                class="unattach"
                :title="`Remove ${fileName(path)}`"
                :aria-label="`Remove ${fileName(path)}`"
                @click="attachments = attachments.filter((p) => p !== path)"
              >
                ×
              </button>
            </li>
          </ul>
          <p v-if="attachError" class="attach-error">{{ attachError }}</p>
          <div class="composer-row reply-row">
            <button class="icon" title="Attach files or images. You can also paste or drop them." aria-label="Attach files or images" @click="addAttachments(false)">
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M10.5 4.5 5.8 9.2a1.4 1.4 0 0 0 2 2l5-5a2.8 2.8 0 0 0-4-4l-5 5a4.2 4.2 0 0 0 6 6l4.2-4.2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
            </button>
            <button class="icon" title="Attach a folder" aria-label="Add folder" @click="addAttachments(true)">
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
            </button>
            <span class="note grow reply-cost" :title="replyHint">{{ replyNote }}</span>
            <button class="btn primary" :disabled="running || !reply.trim()" @click="emit('send')">Reply</button>
          </div>
        </div>
        <div v-else class="composer composer-row">
          <span class="note grow"><slot name="no-reply">This task worked in its own copy, so a reply here would start without its changes.</slot></span>
          <button class="btn primary" @click="newTask">New task</button>
        </div>
      </slot>
    </div>
  </div>
</template>

<style scoped src="./result.css"></style>
<style scoped src="./chat.css"></style>
<style scoped>
.reply-row {
  padding-left: 10px;
}
.reply-cost {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
</style>
