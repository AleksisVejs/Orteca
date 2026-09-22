<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";

// A sphere of dots that turns slowly: "in flight" made visible. Canvas, no dependency.
// Each mood is a look (spin, breath, twinkle, scan, ripple, beat); a change eases
// into the next look instead of jumping. Colour is read from a token; under reduced
// motion it draws one still frame per mood. `absorb()` swallows a steering message:
// a swell, a flash and a spin kick that fade out.
type OrbMood = "idle" | "think" | "read" | "edit" | "check" | "error" | "stop";
const props = withDefaults(defineProps<{ size?: number; mood?: OrbMood }>(), { size: 32, mood: "idle" });

// speed, breathe, twinkle, scan, ripple, beat, scale, alpha
const LOOKS: Record<OrbMood, number[]> = {
  idle: [0.5, 1, 0, 0, 0, 0, 1, 1], // slow turn, gentle breath
  think: [0.3, 0, 1, 0, 0, 0, 0.96, 1], // dim, dots flash like sparks
  read: [1.5, 0, 0, 1, 0, 0, 1, 1], // quick turn, a band sweeps down
  edit: [0.8, 0, 0, 0, 1, 0, 1, 1], // waves roll across the surface
  check: [0.6, 0, 0, 0, 0, 1, 1, 1], // a heartbeat
  error: [0.15, 0, 0, 0, 0, 0, 0.9, 1],
  stop: [0.05, 0, 0, 0, 0, 0, 0.92, 0.45],
};

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
let gulp = 0;
defineExpose({ canvas, absorb: () => (gulp = 1) });

function draw(ctx: CanvasRenderingContext2D, px: number, angle: number, t: number, look: number[], color: string, gulp = 0) {
  const [, breathe, twinkle, scan, ripple, beat, scale, alpha] = look as [number, number, number, number, number, number, number, number];
  ctx.clearRect(0, 0, px, px);
  ctx.fillStyle = color;
  const c = px / 2;
  const R = c * 0.86 * scale * (1 + 0.14 * gulp);
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const tilt = 0.35;
  const pulse = Math.max(0, Math.sin(t * 3.5)) ** 12;
  const scanY = ((t * 0.8) % 2.4) - 1.2;
  points.forEach(([x, y, z], i) => {
    const rx = x * cos + z * sin;
    const rz = -x * sin + z * cos;
    const ty = y * Math.cos(tilt) - rz * Math.sin(tilt);
    const tz = y * Math.sin(tilt) + rz * Math.cos(tilt);
    const depth = (tz + 1) / 2; // 0 back, 1 front
    const hit = scan * Math.exp(-((ty - scanY) ** 2) / 0.015);
    const spark = twinkle * Math.max(0, Math.sin(t * 2.2 + i * 2.39)) ** 8;
    const r = R * (1 + 0.04 * breathe * Math.sin(t * 1.6) + 0.07 * ripple * Math.sin(ty * 7 - t * 6) + 0.1 * beat * pulse);
    ctx.globalAlpha = alpha * Math.min(1, (0.15 + 0.85 * depth) * (1 - 0.5 * twinkle) + 0.8 * spark + 0.8 * hit + 0.5 * gulp);
    ctx.beginPath();
    ctx.arc(c + rx * r, c + ty * r, px * (0.012 + 0.02 * depth) * (1 + 0.8 * hit + 0.6 * spark), 0, Math.PI * 2);
    ctx.fill();
  });
}

onMounted(() => {
  const el = canvas.value;
  const ctx = el?.getContext("2d");
  if (!el || !ctx) return;
  const px = props.size * (window.devicePixelRatio || 1);
  el.width = px;
  el.height = px;
  const color = getComputedStyle(el).color;
  const err = getComputedStyle(el).getPropertyValue("--err").trim() || color;
  const colorFor = () => (props.mood === "error" ? err : color);
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
    watch(() => props.mood, (m) => draw(ctx, px, 0.6, 0, LOOKS[m], colorFor()), { immediate: true });
    return;
  }
  const look = [...LOOKS[props.mood]];
  let angle = 0;
  let last = performance.now();
  const t0 = last;
  const tick = (now: number) => {
    const dt = Math.min(0.1, (now - last) / 1000);
    last = now;
    const target = LOOKS[props.mood];
    const k = 1 - Math.exp(-dt * 4);
    look.forEach((v, j) => (look[j] = v + (target[j]! - v) * k));
    gulp *= Math.exp(-dt * 3);
    angle += (look[0]! + 5 * gulp) * dt;
    draw(ctx, px, angle, (now - t0) / 1000, look, colorFor(), gulp);
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
