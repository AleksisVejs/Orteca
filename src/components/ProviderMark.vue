<script setup lang="ts">
// A provider's logo beside its name. Claude keeps its brand orange; the OpenAI
// knot is black on transparent, so it is a mask filled with the text colour
// around it and dims with its label.
import claude from "../assets/claude.png";
import codex from "../assets/codex.png";
import type { ProviderId } from "../types";
defineProps<{ id: ProviderId; size?: number }>();
</script>

<template>
  <img v-if="id === 'claude'" :src="claude" :width="size ?? 14" :height="size ?? 14" alt="" aria-hidden="true" draggable="false" />
  <span v-else class="knot" :style="{ width: `${size ?? 14}px`, height: `${size ?? 14}px`, maskImage: `url(${codex})` }" aria-hidden="true"></span>
</template>

<style scoped>
img,
.knot {
  display: block;
  flex: none;
  user-select: none;
}
.knot {
  background: currentColor;
  mask-size: contain;
  mask-repeat: no-repeat;
}
</style>
