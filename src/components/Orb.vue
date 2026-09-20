<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";

// A sphere of dots that turns slowly: "in flight" made visible. Canvas, no dependency.
// Colour is read from a token; under reduced motion it draws one still frame.
const props = withDefaults(defineProps<{ size?: number }>(), { size: 32 });

const canvas = ref<HTMLCanvasElement | null>(null);
const DOTS = 120;
// Fibonacci sphere: evenly spread points on a unit sphere.
const points = Array.from({ length: DOTS }, (_, i) => {
  const y = 1 - (2 * (i + 0.5)) / DOTS;
  const r = Math.sqrt(1 - y * y);
  const a = i * Math.PI * (3 - Math.sqrt(5));
  return [Math.cos(a) * r, y, Math.sin(a) * r] as const;
});

let raf = 0;

function draw(ctx: CanvasRenderingContext2D, px: number, angle: number, color: string) {
  ctx.clearRect(0, 0, px, px);
  ctx.fillStyle = color;
  const c = px / 2;
  const R = c * 0.86;
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const tilt = 0.35;
  for (const [x, y, z] of points) {
    const rx = x * cos + z * sin;
    const rz = -x * sin + z * cos;
    const ty = y * Math.cos(tilt) - rz * Math.sin(tilt);
    const tz = y * Math.sin(tilt) + rz * Math.cos(tilt);
    const depth = (tz + 1) / 2; // 0 back, 1 front
    ctx.globalAlpha = 0.15 + 0.85 * depth;
    ctx.beginPath();
    ctx.arc(c + rx * R, c + ty * R, px * (0.012 + 0.02 * depth), 0, Math.PI * 2);
    ctx.fill();
  }
}

onMounted(() => {
  const el = canvas.value;
  const ctx = el?.getContext("2d");
  if (!el || !ctx) return;
  const px = props.size * (window.devicePixelRatio || 1);
  el.width = px;
  el.height = px;
  const color = getComputedStyle(el).color;
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
    draw(ctx, px, 0.6, color);
    return;
  }
  const t0 = performance.now();
  const tick = (t: number) => {
    draw(ctx, px, ((t - t0) / 1000) * 0.9, color);
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
});
onBeforeUnmount(() => cancelAnimationFrame(raf));
</script>

<template>
  <canvas ref="canvas" class="orb" :style="{ width: size + 'px', height: size + 'px' }" aria-hidden="true"></canvas>
</template>

<style scoped>
.orb {
  flex: none;
  color: var(--text-dim);
}
</style>
