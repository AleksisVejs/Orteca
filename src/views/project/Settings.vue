<script setup lang="ts">
import { computed, inject } from "vue";
import { PROJECT } from "./state";
import { DOCK, FONTS, SHELLS } from "./dock";
import { nextPing, sessionReset } from "./anchor";
import type { Anchor } from "./anchor";
import type { ProviderId } from "../../types";

// Every standing choice in one place. The state stays where it lives
// (`state.ts`, the store, the dock's prefs); this page only edits it.
const {
  domId, opened, clarify, chooseClarify, autoWait, chooseAutoWait,
  anchor, saveAnchor, limits, formatWhen, rows,
} = inject(PROJECT)!;
const dock = inject(DOCK)!;
const { prefs } = dock;

const set = (change: Partial<Anchor>) => saveAnchor({ ...anchor.value, ...change });
function toggle(id: ProviderId, on: boolean) {
  set({ providers: on ? [...new Set([...anchor.value.providers, id])] : anchor.value.providers.filter((p) => p !== id) });
}
// What the schedule will do next, per chosen provider. Only as good as the last reading.
const plan = computed(() => anchor.value.mode === "off" ? [] : anchor.value.providers.map((id) => {
  const reading = limits.value.find((l) => l.id === id);
  const ping = nextPing(anchor.value, Date.now(), reading);
  const reset = sessionReset(reading);
  return {
    id,
    ping: ping === null ? "after the next reset it knows of" : formatWhen(ping),
    reset: reset === null ? "not read yet" : formatWhen(reset),
  };
}));
</script>

<template>
  <h2 class="title">Settings</h2>
  <p class="lede">Changes save as you make them.</p>

  <section :aria-labelledby="domId('set-tasks')">
    <h3 :id="domId('set-tasks')" class="label">Tasks</h3>
    <div class="card rows">
      <div class="row">
        <div class="text">
          <span class="name">Unclear requests</span>
          <span class="note">{{ clarify ? "Asks one question first when a wrong guess would waste the run" : "The agent guesses and says how it read the request" }}. Every project.</span>
        </div>
        <div class="segments" role="group" aria-label="Unclear requests">
          <button class="seg" :class="{ on: clarify }" :aria-pressed="clarify" @click="chooseClarify(true)">Ask me</button>
          <button class="seg" :class="{ on: !clarify }" :aria-pressed="!clarify" @click="chooseClarify(false)">Let it guess</button>
        </div>
      </div>
      <div class="row">
        <div class="text">
          <span class="name">Slow commands</span>
          <span class="note">{{ autoWait ? "Runs them at once, as you, outside the agent's sandbox. Only a short list, like git push, is refused" : "Asks before it runs a command the agent hands over" }}. Only <span class="mono">{{ opened.project.name }}</span>.</span>
        </div>
        <div class="segments" role="group" aria-label="Slow commands">
          <button class="seg" :class="{ on: !autoWait }" :aria-pressed="!autoWait" @click="chooseAutoWait(false)">Ask me</button>
          <button class="seg" :class="{ on: autoWait }" :aria-pressed="autoWait" @click="chooseAutoWait(true)">Run them</button>
        </div>
      </div>
    </div>
  </section>

  <section :aria-labelledby="domId('set-anchor')">
    <h3 :id="domId('set-anchor')" class="label">Start the 5-hour window on purpose</h3>
    <div class="card rows">
      <div class="row">
        <div class="text">
          <span class="note">One tiny call on the cheapest model starts a plan's 5-hour window, so it resets when you need it. It is skipped while a task runs or just ran, and when this computer was asleep at the time. Keep Orteca open.</span>
        </div>
        <div class="segments" role="group" aria-label="Window start">
          <button v-for="m in (['off', 'morning', 'continuous'] as const)" :key="m" class="seg" :class="{ on: anchor.mode === m }" :aria-pressed="anchor.mode === m" @click="set({ mode: m })">
            {{ m === "off" ? "Off" : m === "morning" ? "Before work" : "After each reset" }}
          </button>
        </div>
      </div>
      <template v-if="anchor.mode !== 'off'">
        <label class="row">
          <span class="name">Work starts</span>
          <input type="time" :value="anchor.start" @change="set({ start: ($event.target as HTMLInputElement).value })" />
        </label>
        <label v-if="anchor.mode === 'morning'" class="row">
          <span class="name">Limit usually hit after (hours)</span>
          <input type="number" min="0" max="5" step="0.5" :value="anchor.hours" @change="set({ hours: Number(($event.target as HTMLInputElement).value) })" />
        </label>
        <label v-else class="row">
          <span class="name">Work ends</span>
          <input type="time" :value="anchor.end" @change="set({ end: ($event.target as HTMLInputElement).value })" />
        </label>
        <div class="row">
          <span class="name">Agents</span>
          <span class="checks">
            <label v-for="p in rows.filter((r) => r.path)" :key="p.id" class="check">
              <input type="checkbox" :checked="anchor.providers.includes(p.id)" @change="toggle(p.id, ($event.target as HTMLInputElement).checked)" /> {{ p.program }}
            </label>
            <span v-if="!rows.some((r) => r.path)" class="note">None installed</span>
          </span>
        </div>
        <ul v-if="plan.length" class="plan">
          <li v-for="row in plan" :key="row.id" class="note"><span class="mono">{{ row.id }}</span> next ping {{ row.ping }} · window resets {{ row.reset }}</li>
        </ul>
      </template>
    </div>
  </section>

  <section :aria-labelledby="domId('set-dock')">
    <div class="section-head">
      <h3 :id="domId('set-dock')" class="label">Dock</h3>
      <button class="link" @click="dock.resetPrefs()">Reset dock settings</button>
    </div>
    <div class="card rows">
      <label class="row">
        <span class="name">Shell</span>
        <select v-model="prefs.shell">
          <option v-for="s in SHELLS" :key="s.label" :value="s.value">{{ s.label }}</option>
          <option v-if="!SHELLS.some(s => s.value === prefs.shell)" :value="prefs.shell">Custom</option>
        </select>
      </label>
      <label class="row">
        <span class="text">
          <span class="name">Or a command</span>
          <span class="note">New terminals use this. Open ones keep the shell they started with.</span>
        </span>
        <input v-model="prefs.shell" class="mono" placeholder="Blank for PowerShell" />
      </label>
      <label class="row">
        <span class="name">Scrollback lines</span>
        <input v-model.number="prefs.scrollback" type="number" min="200" max="200000" step="500" />
      </label>
      <label class="row">
        <span class="name">Copy as soon as text is selected</span>
        <input v-model="prefs.copyOnSelect" type="checkbox" />
      </label>
      <label class="row">
        <span class="name">Ctrl+C copies a selection, otherwise interrupts</span>
        <input v-model="prefs.smartCopy" type="checkbox" />
      </label>
      <label class="row">
        <span class="name">Font</span>
        <select v-model="prefs.fontFamily">
          <option v-for="f in FONTS" :key="f.label" :value="f.value">{{ f.label }}</option>
        </select>
      </label>
      <label class="row">
        <span class="name">Font size</span>
        <input v-model.number="prefs.fontSize" type="number" min="8" max="28" />
      </label>
      <label class="row">
        <span class="name">Cursor</span>
        <select v-model="prefs.cursorStyle">
          <option value="bar">Bar</option>
          <option value="block">Block</option>
          <option value="underline">Underline</option>
        </select>
      </label>
      <label class="row">
        <span class="name">Blink the cursor</span>
        <input v-model="prefs.cursorBlink" type="checkbox" />
      </label>
      <label class="row">
        <span class="name">Position</span>
        <select v-model="prefs.side">
          <option value="right">Beside the workspace</option>
          <option value="bottom">Below the workspace</option>
        </select>
      </label>
      <label class="row">
        <span class="name">Point the preview at the last address a terminal printed</span>
        <input v-model="prefs.followDevServer" type="checkbox" />
      </label>
    </div>
  </section>
</template>

<style scoped>
.title {
  margin: 0;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
}
.lede {
  margin: 6px 0 0;
  color: var(--text-dim);
}
section {
  margin-top: 32px;
}
.label {
  margin: 0 0 8px;
}
.section-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
}
.rows {
  margin: 0;
  padding: 4px 18px;
}
.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 12px 0;
  color: var(--text);
}
.row + .row {
  border-top: 1px solid var(--border);
}
.text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.name {
  font-size: 14px;
}
.row input:not([type="checkbox"]),
.row select {
  flex: 0 0 auto;
  width: 220px;
  max-width: 50%;
  padding: 5px 8px;
  background: var(--bg);
  color: var(--text);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  font: inherit;
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}
.checks {
  display: flex;
  gap: 16px;
}
.check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-dim);
}
.plan {
  margin: 0;
  padding: 0 0 12px;
  list-style: none;
}
.segments {
  display: inline-flex;
  flex: 0 0 auto;
  gap: 2px;
  padding: 2px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.seg {
  min-height: 26px;
  padding: 2px 10px;
  border-radius: var(--r-sm);
  color: var(--text-faint);
  font-size: 12px;
  transition: color 120ms ease, background 120ms ease;
}
.seg:hover {
  color: var(--text);
}
.seg.on {
  background: var(--surface-2);
  color: var(--text);
}
@media (max-width: 600px) {
  .row { flex-wrap: wrap; }
}
</style>
