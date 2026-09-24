<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue";
import { isAppError, taskLog } from "../../api";
import type { TaskLogRow, TaskType, Tier } from "../../types";
import { PROJECT } from "./state";

// How this project's real tasks went, read from the task log. No eval runs:
// every figure is from work the user asked for, with its sample size beside it.
const { opened, formatTokens, formatCost } = inject(PROJECT)!;
const MIN = 10;
const TYPE: Record<TaskType, string> = { chat: "Chat", question: "Question", code_change: "Code change", debug: "Debug", plan: "Plan" };
const TIERS: Tier[] = ["cheapest", "standard", "deep"];
const TIER: Record<Tier, string> = { cheapest: "cheap", standard: "mid", deep: "top" };

const log = ref<TaskLogRow[] | null>(null);
const error = ref<string | null>(null);
onMounted(async () => {
  try {
    log.value = await taskLog(opened.project.path);
  } catch (e) {
    error.value = isAppError(e) ? e.message : String(e);
  }
});

const done = computed(() => (log.value ?? []).filter((r): r is TaskLogRow & { taskType: TaskType } => r.status !== "running" && !!r.taskType));
// A success ended done, passed its gates, and was neither undone nor asked again.
const won = (r: TaskLogRow) => r.status === "done" && r.gate !== "fail" && r.verdict !== "rejected" && r.verdict !== "retried";
const rate = (rows: TaskLogRow[]) => ({ n: rows.length, wins: rows.filter(won).length });
const rateLabel = ({ n, wins }: { n: number; wins: number }) =>
  n < MIN ? `${wins} of ${n} · too few to tell` : `${Math.round((100 * wins) / n)}% of ${n}`;
const median = (xs: number[]) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.floor(s.length / 2)]!;
};
function groupBy<T>(xs: T[], key: (x: T) => string) {
  const out = new Map<string, T[]>();
  for (const x of xs) out.set(key(x), [...(out.get(key(x)) ?? []), x]);
  return out;
}
/** The Monday that starts the row's UTC week, as YYYY-MM-DD. */
function week(startedAt: string) {
  const d = new Date(startedAt.replace(" ", "T") + "Z");
  d.setUTCDate(d.getUTCDate() - ((d.getUTCDay() + 6) % 7));
  return d.toISOString().slice(0, 10);
}

const success = computed(() => [...groupBy(done.value, (r) => `${r.taskType}|${r.model ?? "unknown model"}`)]
  .map(([key, rows]) => ({ type: TYPE[rows[0]!.taskType], model: key.split("|")[1]!, ...rate(rows) }))
  .sort((a, b) => a.type.localeCompare(b.type) || b.n - a.n));

const weeks = computed(() => [...new Set(done.value.map((r) => week(r.startedAt)))].sort().slice(-6));
const spend = computed(() => [...groupBy(done.value, (r) => r.taskType)].map(([type, rows]) => {
  const costs = rows.map((r) => r.costQuality);
  const quality = costs.some((q) => q !== "exact" && q !== "estimated") ? "partly unavailable" : costs.includes("estimated") ? "estimated" : "exact";
  return {
    type: TYPE[type as TaskType],
    cells: weeks.value.map((w) => {
      const tokens = rows.filter((r) => week(r.startedAt) === w && r.tokens !== null).map((r) => r.tokens!);
      return tokens.length ? `${formatTokens(median(tokens)!)} (${tokens.length})` : "—";
    }),
    cost: rows.some((r) => r.costUsd !== null) ? `${formatCost(rows.reduce((sum, r) => sum + (r.costUsd ?? 0), 0))} ${quality}` : "— cost unavailable",
  };
}));

// Advice only: Orteca picks the tier from each request's difficulty.
const tierAdvice = computed(() => [...groupBy(done.value.filter((r) => r.tier), (r) => r.taskType)].flatMap(([type, rows]) => {
  const by = TIERS.map((tier) => ({ tier, ...rate(rows.filter((r) => r.tier === tier)) }));
  const main = by.reduce((a, b) => (b.n > a.n ? b : a));
  const pct = (t: { n: number; wins: number }) => t.wins / t.n;
  const label = TYPE[type as TaskType];
  const out: string[] = [];
  const cheaper = by.find((t) => TIERS.indexOf(t.tier) < TIERS.indexOf(main.tier) && t.n >= MIN && main.n >= MIN && pct(t) >= pct(main) - 0.05);
  if (cheaper) out.push(`${label}: the ${TIER[cheaper.tier]} tier did as well (${rateLabel(cheaper)}) as ${TIER[main.tier]} (${rateLabel(main)}). A cheaper tier would do.`);
  if (main.n >= MIN && pct(main) < 0.6 && main.tier !== "deep") out.push(`${label}: the ${TIER[main.tier]} tier succeeded ${rateLabel(main)}. A stronger tier may help.`);
  return out;
}));

const trend = computed(() => weeks.value.map((w) => {
  const rows = done.value.filter((r) => week(r.startedAt) === w);
  const recall = rows.filter((r) => r.fileRecall !== null).map((r) => r.fileRecall!);
  const before = rows.filter((r) => r.toolsBeforeEdit !== null).map((r) => r.toolsBeforeEdit!);
  return {
    week: w,
    recall: recall.length ? `${Math.round((100 * recall.reduce((a, b) => a + b, 0)) / recall.length)}% of ${recall.length}` : "—",
    before: before.length ? `${median(before)} (${before.length})` : "—",
  };
}));

const classifier = computed(() => {
  const rows = done.value;
  const asked = rows.filter((r) => r.clarify === "answered" || r.clarify === "skipped");
  const skipped = asked.filter((r) => r.clarify === "skipped").length;
  return {
    total: rows.length,
    picked: rows.filter((r) => r.classifiedBy === "command").length,
    fallback: rows.filter((r) => r.classifiedBy === "keywords").length,
    retried: rows.filter((r) => r.verdict === "retried").length,
    asked: asked.length,
    skipped: asked.length ? `${skipped} of ${asked.length} skipped` : "none asked",
  };
});
</script>

<template>
  <h2 class="title">Stats</h2>
  <p class="lede">How this project's real tasks went. A figure needs at least {{ MIN }} tasks before it means much; fewer say so.</p>

  <p v-if="error" class="missing">Stats could not be read: {{ error }}</p>
  <p v-else-if="!log" class="note" role="status">Reading the task log…</p>
  <p v-else-if="!done.length" class="note">No finished tasks with a recorded type yet.</p>
  <template v-else>
    <section aria-labelledby="stats-success">
      <h3 id="stats-success" class="label">Success by model and task type</h3>
      <p class="note">A success finished, passed its checks, and was not undone or asked again.</p>
      <table class="card">
        <thead><tr><th scope="col">Task type</th><th scope="col">Model</th><th scope="col">Succeeded</th></tr></thead>
        <tbody>
          <tr v-for="s in success" :key="s.type + s.model"><td>{{ s.type }}</td><td class="mono">{{ s.model }}</td><td>{{ rateLabel(s) }}</td></tr>
        </tbody>
      </table>
    </section>

    <section aria-labelledby="stats-spend">
      <h3 id="stats-spend" class="label">Tokens per task, by week</h3>
      <p class="note">Median tokens per task, with the number of tasks. Cost is the total for all of them.</p>
      <div class="scroll">
        <table class="card">
          <thead><tr><th scope="col">Task type</th><th v-for="w in weeks" :key="w" scope="col">Week of {{ w }}</th><th scope="col">Cost</th></tr></thead>
          <tbody>
            <tr v-for="s in spend" :key="s.type"><td>{{ s.type }}</td><td v-for="(c, i) in s.cells" :key="i">{{ c }}</td><td>{{ s.cost }}</td></tr>
          </tbody>
        </table>
      </div>
    </section>

    <section aria-labelledby="stats-tiers">
      <h3 id="stats-tiers" class="label">Tiers</h3>
      <ul v-if="tierAdvice.length" class="advice"><li v-for="a in tierAdvice" :key="a">{{ a }}</li></ul>
      <p v-else class="note">Nothing to suggest yet.</p>
      <p class="note">Advice only. Orteca picks the tier from how hard each request looks, so nothing here changes it.</p>
    </section>

    <section aria-labelledby="stats-files">
      <h3 id="stats-files" class="label">Finding the right files</h3>
      <p class="note">Files found: of the files a run edited, the share Orteca named before it started. Steps before the first edit: the median number of tool calls.</p>
      <table class="card">
        <thead><tr><th scope="col">Week of</th><th scope="col">Files found</th><th scope="col">Steps before the first edit</th></tr></thead>
        <tbody>
          <tr v-for="t in trend" :key="t.week"><td>{{ t.week }}</td><td>{{ t.recall }}</td><td>{{ t.before }}</td></tr>
        </tbody>
      </table>
    </section>

    <section aria-labelledby="stats-classifier">
      <h3 id="stats-classifier" class="label">Reading requests</h3>
      <ul class="advice">
        <li>You picked the type yourself with a /command on {{ classifier.picked }} of {{ classifier.total }} tasks.</li>
        <li>The classifier could not answer, so keywords decided, on {{ classifier.fallback }}.</li>
        <li>The same request was sent again {{ classifier.retried }} times, a sign it was misread or failed.</li>
        <li>A question was asked first on {{ classifier.asked }}: {{ classifier.skipped }}.</li>
      </ul>
    </section>
  </template>
</template>

<style scoped>
.title {
  margin: 0;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.02em;
}
.lede {
  margin: 6px 0 24px;
  color: var(--text-dim);
}
section {
  margin-bottom: 32px;
}
section .note {
  margin: 4px 0 12px;
}
.scroll {
  overflow-x: auto;
}
table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}
th,
td {
  padding: 8px 12px;
  text-align: left;
  white-space: nowrap;
}
th {
  color: var(--text-faint);
  font-weight: 500;
}
td {
  color: var(--text-dim);
  border-top: 1px solid var(--border);
}
.advice {
  margin: 0;
  padding-left: 18px;
  color: var(--text-dim);
}
.advice li + li {
  margin-top: 4px;
}
</style>
