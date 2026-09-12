<script setup lang="ts">
// Velo, the product mark. The PNG is the source of truth; it is white on
// transparent, so it only ever sits on a dark surface.
import velo from "../assets/velo.png";
defineProps<{ size?: number }>();
</script>

<template>
  <img
    :src="velo"
    :width="size ?? 18"
    :height="size ?? 18"
    alt=""
    aria-hidden="true"
    draggable="false"
  />
</template>

<style scoped>
img {
  display: block;
  object-fit: contain;
  user-select: none;
}
</style>
