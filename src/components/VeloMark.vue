<script setup lang="ts">
// Velo. Flat fox mark, no personality, no animation.
defineProps<{ size?: number }>();
</script>

<template>
  <svg
    :width="size ?? 18"
    :height="size ?? 18"
    viewBox="0 0 24 24"
    fill="none"
    aria-hidden="true"
  >
    <path
      d="M3 4l4 3.2h10L21 4l-1.1 6.4A8 8 0 0 1 12 21a8 8 0 0 1-7.9-10.6L3 4Z"
      stroke="currentColor"
      stroke-width="1.4"
      stroke-linejoin="round"
    />
    <path d="M9.2 12h.01M14.8 12h.01" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
  </svg>
</template>
