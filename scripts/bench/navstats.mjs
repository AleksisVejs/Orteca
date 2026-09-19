// Navigation stats from the Claude session logs the LiftMe bench left behind
// (~/.claude/projects/*liftme-bench-wt-*). Free: reads logs, runs nothing.
//
//   node scripts/bench/navstats.mjs
//
// Cost uses Sonnet price ratios: fresh input 1, cache read 0.1, cache write
// 1.25, output 5. A session's first turn is left out, because it pays the
// one-time system prompt whatever tool it calls. "Start here" recall counts
// edited files that were on the brief's list, over every edited file.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const ROOT = path.join(os.homedir(), '.claude', 'projects');
const EDIT = new Set(['Edit', 'Write', 'MultiEdit', 'NotebookEdit']);
const SEARCH_CMD = /^(grep|rg|find|ls|tree|dirname|git (grep|ls-files))\b/;
const READ_CMD = /^(cat|sed -n|head|tail|wc)\b/;

export function turnCost(u) {
  return (u.input_tokens ?? 0) + 0.1 * (u.cache_read_input_tokens ?? 0)
    + 1.25 * (u.cache_creation_input_tokens ?? 0) + 5 * (u.output_tokens ?? 0);
}

// The shell segments of a command, with any leading `cd x` dropped.
function segments(cmd) {
  return cmd.split(/&&|\|\||;|\|/).map((s) => s.trim()).filter((s) => s && !/^cd\b/.test(s));
}

// 'search' | 'read' | 'edit' | 'other' for one tool call, plus the file it reads.
export function toolKind(b) {
  if (EDIT.has(b.name)) return { kind: 'edit', file: b.input.file_path };
  if (b.name === 'Grep' || b.name === 'Glob' || b.name === 'LS') return { kind: 'search' };
  if (b.name === 'Read') return { kind: 'read', file: b.input.file_path };
  if (b.name === 'Bash' || b.name === 'PowerShell') {
    const first = segments(b.input.command ?? '')[0] ?? '';
    if (SEARCH_CMD.test(first)) return { kind: 'search' };
    if (READ_CMD.test(first)) return { kind: 'read' };
  }
  return { kind: 'other' };
}

export function rel(file, cwd) {
  const f = file.replaceAll('\\', '/');
  const c = cwd.replaceAll('\\', '/').replace(/\/$/, '') + '/';
  return f.toLowerCase().startsWith(c.toLowerCase()) ? f.slice(c.length) : f;
}

// One session log -> { prompt, cwd, turns: [{ cost, kinds, reads }], edited }.
export function readSession(text) {
  const turns = new Map();
  let prompt = null;
  let cwd = null;
  for (const line of text.split('\n')) {
    if (!line) continue;
    const e = JSON.parse(line);
    cwd ??= e.cwd ?? null;
    if (e.type === 'user' && prompt === null && typeof e.message?.content === 'string') {
      prompt = e.message.content;
    }
    if (e.type !== 'assistant') continue;
    const id = e.message.id;
    const t = turns.get(id) ?? { cost: 0, kinds: [], reads: [], edits: [] };
    t.cost = turnCost(e.message.usage ?? {});
    for (const b of e.message.content) {
      if (b.type !== 'tool_use') continue;
      const k = toolKind(b);
      t.kinds.push(k.kind);
      if (k.kind === 'read' && k.file) t.reads.push(rel(k.file, cwd ?? ''));
      if (k.kind === 'edit' && k.file) t.edits.push(rel(k.file, cwd ?? ''));
    }
    turns.set(id, t);
  }
  const list = [...turns.values()];
  return { prompt: prompt ?? '', cwd, turns: list, edited: new Set(list.flatMap((t) => t.edits)) };
}

// The "Start here" paths a brief listed.
export function startHere(prompt) {
  const at = prompt.indexOf('Start here.');
  if (at < 0) return [];
  const out = [];
  for (const line of prompt.slice(at).split('\n').slice(1)) {
    const m = /^- (.+)$/.exec(line.trim());
    if (!m) break;
    out.push(m[1].trim());
  }
  return out;
}

export function sessions(root = ROOT) {
  const out = [];
  for (const d of fs.readdirSync(root).filter((d) => d.includes('liftme-bench-wt-'))) {
    const arm = d.endsWith('-orteca-claude') ? 'orteca' : 'plain';
    for (const f of fs.readdirSync(path.join(root, d)).filter((f) => f.endsWith('.jsonl'))) {
      out.push({ arm, dir: d, ...readSession(fs.readFileSync(path.join(root, d, f), 'utf8')) });
    }
  }
  return out;
}

export function stats(all) {
  const rows = {};
  for (const s of all) {
    const r = (rows[s.arm] ??= { sessions: 0, turns: 0, search: 0, cost: 0, searchCost: 0, navCost: 0 });
    r.sessions++;
    let editedYet = false;
    for (const t of s.turns.slice(1)) {
      r.turns++;
      r.cost += t.cost;
      const search = t.kinds.includes('search');
      if (search) { r.search++; r.searchCost += t.cost; }
      if (t.kinds.includes('edit')) editedYet = true;
      const wasted = t.reads.some((f) => !s.edited.has(f));
      if (!editedYet && (search || wasted)) r.navCost += t.cost;
    }
  }
  let hit = 0;
  let edited = 0;
  for (const s of all) {
    const list = new Set(startHere(s.prompt));
    if (!list.size) continue;
    edited += s.edited.size;
    for (const f of s.edited) if (list.has(f)) hit++;
  }
  return { rows, recall: { hit, edited } };
}

if (process.argv[1]?.endsWith('navstats.mjs')) {
  const { rows, recall } = stats(sessions());
  const pct = (a, b) => (b ? `${Math.round((100 * a) / b)}%` : '-');
  console.log('arm     sessions  turns  search  search cost  nav before first edit');
  for (const [arm, r] of Object.entries(rows)) {
    console.log(`${arm.padEnd(8)}${String(r.sessions).padStart(8)}${String(r.turns).padStart(7)}`
      + `${String(r.search).padStart(8)}${pct(r.searchCost, r.cost).padStart(13)}${pct(r.navCost, r.cost).padStart(23)}`);
  }
  console.log(`Start here recall: ${recall.hit} of ${recall.edited} edited files (${pct(recall.hit, recall.edited)})`);
}
