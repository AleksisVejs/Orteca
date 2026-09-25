<script setup lang="ts">
import { ref, watch } from "vue";
import Orb from "../../components/Orb.vue";
import type { ProviderId } from "../../types";

// The chat box while a request waits on one question: the question beside the
// orb, the answer where a reply would go. Skip lets the agent guess and say how.
const props = defineProps<{ question: string; provider: ProviderId | null }>();
const emit = defineEmits<{ answer: [text: string] }>();

const answer = ref("");
watch(() => props.question, () => (answer.value = ""));
const orb = ref<InstanceType<typeof Orb> | null>(null);

function send() {
  if (answer.value.trim()) emit("answer", answer.value.trim());
}
function onEnter(e: KeyboardEvent) {
  if (e.isComposing) return;
  e.preventDefault();
  send();
}
</script>

<template>
  <div class="composer">
    <div class="ask" role="status" aria-live="polite">
      <Orb ref="orb" class="orb" :size="42" state="asking" :provider="provider" />
      <div class="grow">
        <strong>One question before it starts</strong>
        <p>{{ question }}</p>
      </div>
    </div>
    <textarea
      v-model="answer"
      rows="1"
      aria-label="Your answer"
      spellcheck="false"
      placeholder="Your answer…"
      autofocus
      @input="orb?.ripple()"
      @keydown.enter.exact="onEnter"
    ></textarea>
    <div class="composer-row">
      <span class="note grow">Skip, and the agent says how it read the request, then does it.</span>
      <button class="btn" @click="emit('answer', '')">Skip</button>
      <button class="btn primary" :disabled="!answer.trim()" @click="send">Answer and run</button>
    </div>
  </div>
</template>

<style scoped src="./chat.css"></style>
<style scoped>
/* Laid out like the steer box's status line, so the box keeps its shape. */
.ask {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 12px 0 12px;
}
.ask strong {
  font-weight: 500;
  color: var(--text-dim);
}
.ask p {
  margin: 2px 0 0;
  color: var(--text);
  overflow-wrap: anywhere;
}
.ask .grow {
  min-width: 0;
}
@media (prefers-reduced-motion: no-preference) {
  .ask { animation: ask-in 240ms ease both; }
  .ask .orb { animation: orb-in 420ms cubic-bezier(0.2, 0, 0, 1) both; }
}
@keyframes ask-in { from { opacity: 0; } }
@keyframes orb-in { from { opacity: 0; transform: scale(0.3); } }
@media (max-width: 600px) {
  .composer-row { flex-wrap: wrap; }
  .composer-row .note { flex-basis: 100%; }
}
</style>
