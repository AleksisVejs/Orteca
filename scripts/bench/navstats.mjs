// Navigation stats from the Claude session logs the LiftMe bench left behind
// (~/.claude/projects/*<bench>-bench-wt-*). Free: reads logs, runs nothing.
//
//   node scripts/bench/navstats.mjs
//   node scripts/bench/navstats.mjs --recall   (NOMAP=1 for the old ranking, VERBOSE=1 for lists)
//
// Cost uses Sonnet price ratios: fresh input 1, cache read 0.1, cache write
// 1.25, output 5. A session's first turn is left out, because it pays the
// one-time system prompt whatever tool it calls. "Start here" recall counts
// edited files that were on the brief's list, over every edited file.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const ROOT = path.join(os.homedir(), '.claude', 'projects');
const BENCH = process.env.BENCH ?? 'liftme';
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

export function sessions(root = ROOT, bench = BENCH) {
  const out = [];
  for (const d of fs.readdirSync(root).filter((d) => d.includes(`${bench}-bench-wt-`))) {
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

// The prompt a brief was built from.
export function taskOf(prompt) {
  const m = /^Task:\n([\s\S]*?)\n\nStart here\./.exec(prompt);
  return m ? m[1].trim() : null;
}

// Offline recall: rank each recorded prompt again on a throwaway worktree of
// the selected benchmark repository at HEAD, and count the edited files that
// existed there which the new list holds. NOMAP=1 ranks without the map.
function recall() {
  const repo = process.env.NAV_REPO ?? (BENCH === 'riginspect'
    ? 'C:/Users/User/Projects/RigInspectBE'
    : 'C:/Users/User/Projects/LiftMe');
  const git = (...a) => execFileSync('git', ['-C', repo, ...a], { encoding: 'utf8' });
  const cases = sessions()
    .filter((s) => s.arm === 'orteca' && taskOf(s.prompt))
    .map((s) => ({ task: taskOf(s.prompt), edited: [...s.edited] }));
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'orteca-nav-'));
  const wt = path.join(tmp, 'wt');
  git('worktree', 'add', '--detach', wt, 'HEAD');
  try {
    const existed = new Set(execFileSync('git', ['-C', wt, 'ls-files'], { encoding: 'utf8' }).split('\n'));
    const prompts = [...new Set(cases.map((c) => c.task))];
    fs.writeFileSync(path.join(tmp, 'cases.json'), JSON.stringify(prompts));
    const out = execFileSync('cargo', ['test', '-q', 'nav_recall', '--', '--ignored', '--nocapture'], {
      cwd: new URL('../../src-tauri', import.meta.url),
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'inherit'],
      env: {
        ...process.env,
        NAV_REPO: wt,
        NAV_CASES: path.join(tmp, 'cases.json'),
        ...(process.env.NOMAP ? { NAV_NOMAP: '1' } : {}),
      },
    });
    const lists = out.split('\n').filter((l) => l.startsWith('NAV ')).map((l) => JSON.parse(l.slice(4)));
    const notes = out.split('\n').filter((l) => l.startsWith('NOTES ')).map((l) => JSON.parse(l.slice(6)));
    const listOf = new Map(prompts.map((p, i) => [p, new Set(lists[i])]));
    let hit = 0;
    let total = 0;
    const missed = {};
    for (const c of cases) {
      for (const f of c.edited.filter((f) => existed.has(f))) {
        total++;
        if (listOf.get(c.task).has(f)) hit++;
        else missed[f] = (missed[f] ?? 0) + 1;
      }
    }
    console.log(`recall: ${hit} of ${total} edited files that existed at base (${Math.round((100 * hit) / total)}%)`);
    const top = Object.entries(missed).sort((a, b) => b[1] - a[1]).slice(0, 12);
    console.log('most missed:', top.map(([f, n]) => `${f} x${n}`).join(', '));
    if (process.env.VERBOSE) {
      for (const [i, p] of prompts.entries()) {
        const edited = new Set(cases.filter((c) => c.task === p).flatMap((c) => c.edited).filter((f) => existed.has(f)));
        const miss = [...edited].filter((f) => !listOf.get(p).has(f));
        const line = (f, j) => `${edited.has(f) ? '* ' : '  '}${f}${notes[i][j] ? `: ${notes[i][j]}` : ''}`;
        console.log(`\n${p.slice(0, 100)}\n  ${lists[i].map(line).join('\n  ')}\n  missed: ${miss.join(', ')}`);
      }
    }
  } finally {
    git('worktree', 'remove', '--force', wt);
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

if (process.argv[1]?.endsWith('navstats.mjs') && process.argv.includes('--recall')) {
  recall();
} else if (process.argv[1]?.endsWith('navstats.mjs')) {
  const { rows, recall } = stats(sessions());
  const pct = (a, b) => (b ? `${Math.round((100 * a) / b)}%` : '-');
  console.log('arm     sessions  turns  search  search cost  nav before first edit');
  for (const [arm, r] of Object.entries(rows)) {
    console.log(`${arm.padEnd(8)}${String(r.sessions).padStart(8)}${String(r.turns).padStart(7)}`
      + `${String(r.search).padStart(8)}${pct(r.searchCost, r.cost).padStart(13)}${pct(r.navCost, r.cost).padStart(23)}`);
  }
  console.log(`Start here recall: ${recall.hit} of ${recall.edited} edited files (${pct(recall.hit, recall.edited)})`);
}
