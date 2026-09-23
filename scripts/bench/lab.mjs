// Local benchmark-lab utilities. They never invoke a provider CLI.
import { readFileSync, writeFileSync } from 'node:fs';

const CLASSES = new Set(['small-edit', 'standard', 'cross-file-refactor', 'schema', 'guarded-security']);
const REQUIRED = ['id', 'repository', 'revision', 'taskClass', 'prompt', 'setup', 'cleanup', 'providers', 'publicChecks', 'hiddenCheckId'];

export function validateCorpus(corpus) {
  const errors = [];
  const ids = new Set();
  const classes = new Set();
  for (const task of corpus.tasks ?? []) {
    for (const key of REQUIRED) if (!task[key] || (Array.isArray(task[key]) && !task[key].length)) errors.push(`${task.id ?? '<unnamed>'}: missing ${key}`);
    if (ids.has(task.id)) errors.push(`${task.id}: duplicate id`);
    ids.add(task.id);
    if (!CLASSES.has(task.taskClass)) errors.push(`${task.id}: unknown taskClass`);
    classes.add(task.taskClass);
    if (task.taskClass === 'guarded-security' && !task.controlId) errors.push(`${task.id}: guarded task needs clean control`);
    if (task.publicChecks?.some(check => typeof check !== 'string' || !check.trim())) errors.push(`${task.id}: invalid public check`);
  }
  if ((corpus.tasks?.length ?? 0) < 20) errors.push('corpus needs at least 20 tasks');
  for (const taskClass of CLASSES) if (!classes.has(taskClass)) errors.push(`corpus lacks ${taskClass}`);
  return errors;
}

function random(seed) {
  let value = seed >>> 0;
  return () => ((value = (value * 1664525 + 1013904223) >>> 0) / 2 ** 32);
}

export function makePlan(tasks, arms, repetitions, seed = 1) {
  const next = random(seed);
  const rows = [];
  for (const task of tasks) for (let repetition = 1; repetition <= repetitions; repetition += 1) {
    const order = arms.filter(arm => task.providers.includes(arm)).sort(() => next() - 0.5);
    order.forEach((arm, orderIndex) => rows.push({ taskId: task.id, arm, repetition, order: orderIndex + 1 }));
  }
  return rows;
}

function median(values) {
  const sorted = values.filter(Number.isFinite).sort((a, b) => a - b);
  if (!sorted.length) return null;
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function interval(values, seed = 1, samples = 2000) {
  if (values.length < 2) return null;
  const next = random(seed);
  const medians = Array.from({ length: samples }, () => median(Array.from({ length: values.length }, () => values[Math.floor(next() * values.length)]))).sort((a, b) => a - b);
  return [medians[Math.floor(samples * 0.025)], medians[Math.floor(samples * 0.975)]];
}

export function summarise(rows) {
  const groups = new Map();
  for (const row of rows) {
    if (!row.taskId || !row.arm) throw Error('Each result needs taskId and arm');
    const key = `${row.taskId}\0${row.arm}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(row);
  }
  return [...groups].map(([key, records]) => {
    const [taskId, arm] = key.split('\0');
    const complete = records.filter(row => row.publicPass === true && row.hiddenPass === true).length;
    const hidden = records.filter(row => row.hiddenPass === true).length;
    const timings = records.map(row => row.wallMs).filter(Number.isFinite);
    const tokens = records.map(row => row.uncachedTokens).filter(Number.isFinite);
    return { taskId, arm, runs: records.length, complete, hidden, medianWallMs: median(timings), wall95: interval(timings), medianUncachedTokens: median(tokens), token95: interval(tokens, 2), calls: median(records.map(row => row.calls).filter(Number.isFinite)), localVerifyPass: records.filter(row => row.localVerify === 'pass').length, reviewFound: records.filter(row => row.reviewOutcome === 'found').length };
  });
}

export function markdownReport(rows) {
  const report = summarise(rows);
  const cell = value => value == null ? '-' : value;
  const time = value => value == null ? '-' : `${Math.round(value / 1000)}s`;
  const ci = value => value == null ? '-' : `${Math.round(value[0] / 1000)}–${Math.round(value[1] / 1000)}s`;
  return ['# Benchmark Lab report', '', 'Task-level cells; do not blend correctness, latency, and cost into one score.', '', '| Task | Arm | Complete | Hidden | Median wall | 95% CI | Uncached tokens | Calls | Verify | Review findings |', '| --- | --- | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: |', ...report.map(row => `| ${row.taskId} | ${row.arm} | ${row.complete}/${row.runs} | ${row.hidden}/${row.runs} | ${time(row.medianWallMs)} | ${ci(row.wall95)} | ${cell(row.medianUncachedTokens)} | ${cell(row.calls)} | ${row.localVerifyPass}/${row.runs} | ${row.reviewFound}/${row.runs} |`), ''].join('\n');
}

function parseArgs(args) {
  return Object.fromEntries(args.filter(arg => arg.startsWith('--')).map(arg => {
    const [key, value = 'true'] = arg.slice(2).split('=');
    return [key, value];
  }));
}

if (process.argv[1]?.endsWith('/lab.mjs') || process.argv[1]?.endsWith('\\lab.mjs')) {
  const [command = 'validate'] = process.argv.slice(2);
  const options = parseArgs(process.argv.slice(3));
  const corpus = JSON.parse(readFileSync(options.corpus ?? 'benchmarks/corpus.json', 'utf8'));
  if (command === 'validate') {
    const errors = validateCorpus(corpus);
    if (errors.length) throw Error(errors.join('\n'));
    console.log(`valid corpus: ${corpus.tasks.length} tasks`);
  } else if (command === 'plan') {
    const arms = (options.arms ?? 'claude,codex').split(',');
    const plan = makePlan(corpus.tasks, arms, Number(options.repetitions ?? 3), Number(options.seed ?? 1));
    if (options.out) writeFileSync(options.out, `${JSON.stringify(plan, null, 2)}\n`);
    else console.log(JSON.stringify(plan, null, 2));
  } else if (command === 'report') {
    if (!options.results || !options.out) throw Error('report needs --results=raw.jsonl and --out=report.md');
    const rows = readFileSync(options.results, 'utf8').trim().split(/\r?\n/).filter(Boolean).map(JSON.parse);
    writeFileSync(options.out, markdownReport(rows));
  } else throw Error(`unknown command: ${command}`);
}
