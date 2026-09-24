<script lang="ts">
export type OrbState =
  | "idle" | "user_typing" | "classifying" | "thinking" | "planning" | "searching" | "reading" | "editing" | "running"
  | "testing" | "reviewing" | "needs_input" | "success" | "error" | "rate_limited" | "rollback" | "handoff" | "paused";

export const ORB_LABEL: Record<OrbState, string> = {
  idle: "Idle", user_typing: "You're typing", classifying: "Sorting the request", thinking: "Thinking",
  planning: "Planning", searching: "Searching", reading: "Reading", editing: "Editing", running: "Running a command",
  testing: "Testing", reviewing: "Reviewing", needs_input: "Needs your input", success: "Done", error: "Something went wrong",
  rate_limited: "Limit reached, waiting for reset", rollback: "Undoing", handoff: "Handing over", paused: "Paused",
};
</script>

<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { ProviderId, Tier } from "../types";

// A sphere of dots whose motion says what the agent is doing: rotation thinks,
// flow reads, a local change edits, an outward wave needs the user. Provider is
// the colour, tier the dot density. Canvas, one frame loop, paused when unseen.
// A state shows for at least 500 ms; changes inside that window show the latest.
const props = withDefaults(
  defineProps<{ size?: number; state?: OrbState; provider?: ProviderId | null; tier?: Tier | null; progress?: number }>(),
  { size: 32, state: "idle", provider: null, tier: null, progress: 0 },
);

const ZERO = {
  spin: 0, breathe: 0, drift: 0, lean: 0, cluster: 0, twin: 0, rings: 0, sweep: 0, flow: 0, patch: 0, beat: 0,
  fill: 0, layers: 0, wave: 0, settle: 0, rest: 0, bloom: 0, shake: 0, rewind: 0, rebuild: 0, alpha: 1,
};
type Look = typeof ZERO;
const look = (l: Partial<Look>): Look => ({ ...ZERO, ...l });
const IDLE = { spin: 0.35, breathe: 1, drift: 1 };
const LOOKS: Record<OrbState, Look> = {
  idle: look(IDLE),
  user_typing: look({ spin: 0.2, breathe: 0.5, lean: 1 }),
  classifying: look({ spin: 0.6, cluster: 1 }),
  thinking: look({ spin: 0.45, twin: 1 }),
  planning: look({ spin: 0.3, rings: 1 }),
  searching: look({ spin: 0.4, sweep: 1 }),
  reading: look({ spin: 0.1, flow: 1 }),
  editing: look({ spin: 0.3, patch: 1 }),
  running: look({ spin: 0.3, beat: 1 }),
  testing: look({ spin: 0.25, fill: 1 }),
  reviewing: look({ spin: 0.5, layers: 1 }),
  needs_input: look({ wave: 1 }),
  success: look({ ...IDLE, bloom: 1 }),
  error: look({ spin: 0.06, shake: 1, alpha: 0.9 }),
  rate_limited: look({ settle: 1, alpha: 0.85 }),
  rollback: look({ ...IDLE, rewind: 1 }),
  handoff: look({ ...IDLE, rebuild: 1 }),
  paused: look({ rest: 1, alpha: 0.4 }),
};
const KEYS = Object.keys(ZERO) as (keyof Look)[];

const DENSITY: Record<Tier, number> = { cheapest: 56, standard: 88, deep: 120 };
const TETRA = [[0, 1, 0], [0.94, -0.33, 0], [-0.47, -0.33, 0.82], [-0.47, -0.33, -0.82]] as const;
function sphere(n: number) {
  return Array.from({ length: n }, (_, i) => {
    const y = 1 - (2 * (i + 0.5)) / n;
    const r = Math.sqrt(1 - y * y);
    const a = i * Math.PI * (3 - Math.sqrt(5));
    return [Math.cos(a) * r, y, Math.sin(a) * r, (i * 0.618034) % 1] as const;
  });
}
let pts = sphere(DENSITY[props.tier ?? "standard"]);
watch(() => props.tier, (t) => (pts = sphere(DENSITY[t ?? "standard"])));

const shown = ref<OrbState>(props.state);
let shownAt = performance.now();
let queued = 0;
watch(() => props.state, () => {
  clearTimeout(queued);
  const show = () => {
    if (shown.value === props.state) return;
    shown.value = props.state;
    shownAt = performance.now();
  };
  const wait = 500 - (performance.now() - shownAt);
  if (wait <= 0) show();
  else queued = window.setTimeout(show, wait);
});

let gateFrom = 0;
let gateTo = 0;
let gateAt = -1e9;
watch(() => props.progress, (now, was) => {
  if (now > was) [gateFrom, gateTo, gateAt] = [was, now, performance.now()];
});

const canvas = ref<HTMLCanvasElement | null>(null);
let gulp = 0;
let rippleAt = -1e9;
defineExpose({ canvas, absorb: () => (gulp = 1), ripple: () => (rippleAt = performance.now()) });

const clamp = (v: number, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));
const ease = (v: number) => { const x = clamp(v); return x * x * (3 - 2 * x); };
const TAU = Math.PI * 2;

// Radius over time for the one-shot states, 1 when done.
function bloomAt(s: number) {
  if (s < 0.35) return 1 - 0.6 * ease(s / 0.35);
  if (s < 0.8) return 0.4 + 0.85 * ease((s - 0.35) / 0.45);
  return 1.25 - 0.25 * ease((s - 0.8) / 0.8);
}

type Frame = { L: Look; t: number; s: number; aOut: number; aIn: number; rgb: string; motion: boolean; typedS: number; gateS: number };

function draw(ctx: CanvasRenderingContext2D, px: number, f: Frame) {
  const { L, t, s, aOut, aIn, motion } = f;
  ctx.clearRect(0, 0, px, px);
  ctx.fillStyle = f.rgb;
  const c = px / 2;
  const R = c * 0.74;
  const dot = (x: number, y: number, size: number, alpha: number) => {
    ctx.globalAlpha = clamp(alpha);
    ctx.beginPath();
    ctx.arc(x, y, size, 0, TAU);
    ctx.fill();
  };
  const tilt = 0.35 + L.twin * 0.45 * Math.sin(t * 0.5);
  const [ct, st] = [Math.cos(tilt), Math.sin(tilt)];
  const pulse = Math.max(0, Math.sin(t * TAU)) ** 18;
  const shake = motion && s < 1 ? L.shake * 0.1 * R * Math.sin(s * 45) * Math.exp(-s * 5) : 0;
  const bloom = 1 + L.bloom * (bloomAt(s) - 1);
  const ringT = (t % 3) / 3;
  const patchN = Math.floor(t / 1.1);
  const patchP = (t / 1.1) % 1;
  const ph = [Math.sin(patchN * 12.9), Math.sin(patchN * 78.2) * 0.8, 0.6 + 0.4 * Math.abs(Math.sin(patchN * 37.7))];
  const pl = Math.hypot(ph[0]!, ph[1]!, ph[2]!);
  const band = ((t * 2.4) % TAU) - Math.PI;
  const rest = 1 + L.rest * 0.6 * Math.max(0, Math.sin((t * TAU) / (motion ? 7 : 3))) ** 10;
  const rippleFront = f.typedS * 3.5;
  const rippleFade = Math.exp(-f.typedS * 2.5);
  const n = pts.length;

  pts.forEach(([bx, by, bz, seed], i) => {
    let [x, y, z] = [bx, by, bz];
    // Split into clusters and merge back.
    const m = L.cluster * (0.5 - 0.5 * Math.cos((t * TAU) / 1.4)) * 0.75;
    if (m > 0) {
      const g = TETRA[i % 4]!;
      [x, y, z] = [x + (g[0] - x) * m, y + (g[1] - y) * m, z + (g[2] - z) * m];
      const l = Math.hypot(x, y, z) || 1;
      [x, y, z] = [x / l, y / l, z / l];
    }
    // Horizontal rings, appearing top to bottom.
    const ring = i % 5;
    let alpha = 1;
    if (L.rings > 0) {
      const ry = -0.8 + 0.4 * ring;
      const rr = Math.sqrt(1 - ry * ry);
      const th = (Math.floor(i / 5) / Math.ceil(n / 5)) * TAU;
      [x, y, z] = [x + (Math.cos(th) * rr - x) * L.rings, y + (ry - y) * L.rings, z + (Math.sin(th) * rr - z) * L.rings];
      const vis = clamp(ringT * 7 - ring) * (ringT > 0.85 ? 1 - (ringT - 0.85) / 0.15 : 1);
      alpha *= 1 + (vis - 1) * L.rings;
    }
    const inner = i % 2 === 1;
    const a = inner ? aIn : aOut;
    const rx = x * Math.cos(a) + z * Math.sin(a);
    const rz = -x * Math.sin(a) + z * Math.cos(a);
    let sx = rx;
    let sy = y * ct - rz * st;
    let sz = y * st + rz * ct;
    // Lines of dots flowing left to right.
    if (L.flow > 0) {
      const u = (seed + t * (0.3 + 0.08 * (ring % 2))) % 1;
      const ly = -0.6 + 0.3 * ring;
      const w = Math.sqrt(1 - ly * ly);
      sx += (-w + 2 * w * u - sx) * L.flow;
      sy += (ly - sy) * L.flow;
      sz += (0.5 - sz) * L.flow;
      alpha *= 1 + (Math.sin(Math.PI * u) - 1) * L.flow;
    }
    // Settled at the bottom; refills as the reset nears.
    const down = L.settle * clamp((seed - props.progress) * 10 + 0.5);
    if (down > 0) {
      sx += (bx * 0.8 - sx) * down;
      sy += (0.95 - seed * 0.35 * (1 - bx * bx) - sy) * down;
      sz += (0.3 - sz) * down;
    }
    const depth = (sz + 1) / 2;
    let size = 1;
    let r = R * bloom * (1 + 0.02 * L.breathe * Math.sin((t * TAU) / 4.5) + 0.09 * L.beat * pulse + 0.14 * gulp);
    const layer = Math.max(L.twin, L.layers);
    if (inner) r *= 1 - 0.3 * layer * (i % 6 === 3 && L.layers > 0 ? 0.5 + 0.5 * Math.sin(t * 1.3 + i) : 1);
    if (L.rebuild > 0) {
      const own = s < 0.5 ? 1 - 0.75 * ease(s / 0.5) : 0.25 + 0.75 * ease((s - 0.5 - seed * 0.4) / 0.5);
      r *= 1 + (own - 1) * L.rebuild;
    }
    // A keystroke ripples up from the input below.
    const bump = rippleFade * Math.exp(-((Math.hypot(sx, sy - 1) - rippleFront) ** 2) / 0.04);
    r *= 1 + 0.08 * bump;
    // One area fades out, then pops back in.
    if (L.patch > 0) {
      const near = ease(((sx * ph[0]! + sy * ph[1]! + sz * ph[2]!) / pl - 0.82) / 0.12);
      const out = patchP < 0.55 ? patchP / 0.55 : 0;
      alpha *= 1 - L.patch * near * out;
      if (patchP >= 0.55) size *= 1 + 0.8 * L.patch * near * (1 - (patchP - 0.55) / 0.45);
    }
    // A bright band sweeping around.
    let hit = 0;
    if (L.sweep > 0) {
      const d = Math.atan2(sx, sz) - band;
      const w = Math.atan2(Math.sin(d), Math.cos(d));
      hit = L.sweep * Math.exp(-(w * w) / 0.06);
      alpha *= 1 - 0.4 * L.sweep;
    }
    const drift = L.drift * (i % 11 === 0 ? 0.05 : 0);
    const X = c + shake + sx * r + drift * R * Math.sin(t * 0.6 + i);
    const Y = c + sy * r + drift * R * Math.cos(t * 0.45 + i * 1.7) + L.lean * 0.1 * R * (0.5 + 0.5 * sy);
    const glow = 0.8 * hit + 0.4 * L.beat * pulse + 0.5 * bump + 0.5 * gulp;
    dot(X, Y, px * (0.012 + 0.02 * depth) * size * (1 + 0.8 * hit), L.alpha * rest * (alpha * (0.15 + 0.85 * depth) + glow));
  });

  // Testing: a ring filling clockwise; a passed gate flashes its segment.
  if (L.fill > 0.01) {
    const G = 28;
    const frac = props.progress > 0 ? props.progress : (t / 2.6) % 1;
    const flash = Math.exp(-f.gateS * 3);
    for (let j = 0; j < G; j++) {
      const q = j / G;
      const ang = -Math.PI / 2 + q * TAU;
      const lit = q < frac;
      const gate = lit && q >= gateFrom && q < gateTo ? flash : 0;
      dot(c + Math.cos(ang) * R * 1.16, c + Math.sin(ang) * R * 1.16, px * 0.014 * (1 + gate), L.fill * ((lit ? 0.85 : 0.15) + gate));
    }
  }
  // Needs input: the only outward wave.
  if (L.wave > 0.01 && motion) {
    const p = (t / 2.4) % 1;
    for (let j = 0; j < 40; j++) {
      const ang = (j / 40) * TAU;
      dot(c + Math.cos(ang) * R * (1.02 + 0.3 * p), c + Math.sin(ang) * R * (1.02 + 0.3 * p), px * 0.011, L.wave * (1 - p) * 0.8);
    }
  }
}

let raf = 0;
let stopLoop = () => {};
onMounted(() => {
  const el = canvas.value;
  const ctx = el?.getContext("2d");
  if (!el || !ctx) return;
  const px = Math.round(props.size * (window.devicePixelRatio || 1));
  el.width = px;
  el.height = px;
  const style = getComputedStyle(el);
  const rgbOf = (v: string, fallback: number[]) => {
    ctx.fillStyle = "#000";
    ctx.fillStyle = v || "#000";
    const h = String(ctx.fillStyle);
    const out = h.startsWith("#") ? h.slice(1).match(/../g)!.map((x) => parseInt(x, 16)) : (h.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
    return out.length === 3 && v ? out : fallback;
  };
  const dim = rgbOf(style.color, [188, 188, 188]);
  const token = (name: string) => rgbOf(style.getPropertyValue(name).trim(), dim);
  const tone = { claude: token("--orb-claude"), codex: token("--orb-codex"), err: token("--err"), warn: token("--warn"), ok: token("--accent") };
  const base = () => (props.provider ? tone[props.provider] : dim);
  const reduced = matchMedia("(prefers-reduced-motion: reduce)");

  const L = look(LOOKS[shown.value]);
  const rgb = [...base()];
  let aOut = 0.6;
  let aIn = 0.6;
  let last = performance.now();
  const t0 = last;
  const tick = (now: number) => {
    const dt = Math.min(0.1, (now - last) / 1000);
    last = now;
    const motion = !reduced.matches;
    const state = shown.value;
    const s = (now - shownAt) / 1000;
    const k = 1 - Math.exp(-dt * 9);
    const target = LOOKS[state];
    for (const key of KEYS) L[key] += (target[key] - L[key]) * k;
    const want = state === "error" ? tone.err : state === "rate_limited" ? tone.warn : base();
    rgb.forEach((v, j) => (rgb[j] = v + (want[j]! - v) * k));
    gulp *= Math.exp(-dt * 3);
    if (motion) {
      const spin = L.spin + L.rewind * (s < 1 ? -8 * (1 - s) ** 2 : 0) + 5 * gulp;
      aOut += spin * dt;
      aIn += (spin - 2 * L.spin * Math.max(L.twin, L.layers)) * dt;
    }
    let color = rgb;
    const green = L.bloom * clamp(1 - Math.abs(s - 0.8) / 0.8);
    if (green > 0) color = color.map((v, j) => v + (tone.ok[j]! - v) * green);
    if (motion && state === "error" && s < 0.5 && Math.sin(s * 60) < 0) color = base();
    const Lm = motion ? L : look({ alpha: L.alpha * (1 + 0.3 * L.bloom * green), rest: Math.max(L.rest, L.wave, L.beat) });
    draw(ctx, px, {
      L: Lm, t: (now - t0) / 1000, s, aOut, aIn, motion,
      rgb: `rgb(${color.map(Math.round).join(" ")})`,
      typedS: motion ? (now - rippleAt) / 1000 : 99, gateS: (now - gateAt) / 1000,
    });
    raf = requestAnimationFrame(tick);
  };

  // Only draw while the orb can be seen: window shown and the orb on screen.
  let onScreen = true;
  const sync = () => {
    cancelAnimationFrame(raf);
    raf = 0;
    if (onScreen && !document.hidden) {
      last = performance.now();
      raf = requestAnimationFrame(tick);
    }
  };
  const seen = new IntersectionObserver(([e]) => { onScreen = !!e?.isIntersecting; sync(); });
  seen.observe(el);
  document.addEventListener("visibilitychange", sync);
  sync();
  stopLoop = () => {
    seen.disconnect();
    document.removeEventListener("visibilitychange", sync);
    cancelAnimationFrame(raf);
  };
});
onBeforeUnmount(() => {
  clearTimeout(queued);
  stopLoop();
});
</script>

<template>
  <span class="orb" :style="{ width: size + 'px', height: size + 'px' }">
    <canvas ref="canvas" :style="{ width: size + 'px', height: size + 'px' }" aria-hidden="true"></canvas>
    <span class="hidden-label" role="status">{{ ORB_LABEL[shown] }}</span>
  </span>
</template>

<style scoped>
.orb {
  flex: none;
  display: inline-block;
  position: relative;
  color: var(--text-dim);
}
canvas {
  display: block;
}
</style>
