# Orteca — MVP Architecture

Status: pre-code. Verified against Claude Code and Codex docs, Sept 2026.

---

## 1. Repository state

Empty directory, not a git repo. Greenfield.

## 2. Claude Code — verified integration surface

Official CLI, non-interactive. No SDK dependency needed; Rust drives the process directly.

```
claude -p "<prompt>" \
  --output-format stream-json --verbose \
  --input-format stream-json \
  --permission-mode acceptEdits \
  --allowedTools "Read,Edit,Bash(git diff *)" \
  --max-turns 20 \
  --model sonnet \
  --json-schema <schema>
```

| Need | Mechanism | Confidence |
|---|---|---|
| Run headless | `-p` | exact |
| Event stream | `--output-format stream-json --verbose`, JSONL | exact |
| **Mid-run instruction** | `--input-format stream-json`, write `{"type":"user","text":"..."}` to stdin | exact |
| Structured artifact | `--json-schema` → `structured_output` field | exact |
| Token + cost | final `result` event: `usage`, `total_cost_usd` (client-side estimate) | estimated |
| Resume | `--resume <session_id>`, `--fork-session` | exact |
| End turn | SIGINT | exact |
| Kill run | SIGTERM → exit 143, terminates its Bash process tree | exact |
| Deny-all prompts | `--permission-mode dontAsk --permission-prompts none` | exact |

### Auth finding — important

`--bare` **disables OAuth and keychain reads** and requires `ANTHROPIC_API_KEY`.
To reuse the user's Claude subscription, Orteca must **not** pass `--bare`.

Consequence: a non-bare `-p` run loads the *target repo's* `.claude/settings.json`
hooks and `.mcp.json` **with no trust prompt**. Orteca opens arbitrary local repos,
so this is a real code-execution path.

**Mitigation (must ship in MVP):** on first open of a project, scan for
`.claude/settings.json`, `.claude/hooks`, `.mcp.json`, `CLAUDE.md`. If present,
show them once and require the user to trust the project. Store the decision.

## 3. Codex — verified integration surface

```
codex exec "<prompt>" --json --sandbox workspace-write --output-schema <file>
codex exec resume <SESSION_ID> "<follow-up>"
```

| Need | Mechanism | Confidence |
|---|---|---|
| Run headless | `codex exec` | exact |
| Event stream | `--json` JSONL: `thread.started`, `turn.started/completed/failed`, `item.started/completed` | exact |
| Tokens | `turn.completed.usage`: `input_tokens`, `cached_input_tokens`, `output_tokens`, `reasoning_output_tokens` | exact |
| Cost | **not reported** — tokens only | unavailable |
| Structured artifact | `--output-schema <path>` | exact |
| Final message | `-o/--output-last-message <path>` | exact |
| Write access | `--sandbox workspace-write` (default is read-only) | exact |
| Resume | `codex exec resume <id>` / `--last` | exact |
| Reproducible run | `--ignore-user-config`, `--ignore-rules` | exact |
| **Mid-run instruction** | **not supported.** stdin is the prompt, consumed at start | exact |

Auth: `codex login` writes `~/.codex/auth.json` (ChatGPT subscription). Orteca never
touches it, never asks for passwords — the CLI owns auth. `CODEX_API_KEY` is the
alternative. Orteca detects which is present and reports it.

## 4. Assumptions in the spec that are wrong

**4.1 "Context avoided 62%" is not measurable. Do not ship it as the headline.**

Both CLIs are *agents*. They read files themselves inside their own loop. Orteca
controls the prompt it sends, not what Claude Code decides to `Read`. There is no
API that reports "context this agent assembled." A savings percentage with no
baseline is invented precision, and the spec forbids that.

What is honestly available:

| Metric | Label |
|---|---|
| Tokens used per call | `exact` (both CLIs report) |
| Cost | `estimated` (Claude only), `unavailable` (Codex) |
| Agent calls made | `exact` |
| **Agent calls avoided** | `exact` — we chose the route, max route is known |
| Turns used vs. `--max-turns` ceiling | `exact` |
| Files named in brief vs. files in repo | `exact` |
| Context saved % | `unavailable` until a baseline exists |

**Decision:** headline metric is `calls avoided` + `tokens used`. A savings % appears
only after >= 5 comparable tasks in that project give a rolling median baseline, and is
labelled `estimated`. `baselines` table exists from day one to collect this.

**4.2 The "context engine" as specified duplicates work the agents already do.**

Sending curated file *contents* fights the agent's own retrieval and often pays for
the same bytes twice. The leverage is cheaper: a tight brief that **names paths** so
the agent stops hunting, plus a `--max-turns` ceiling.

MVP context engine = ripgrep + path heuristics + `git log` recency → a ranked list of
~10 likely paths, pasted into the prompt as "start here". Not file contents. Not
embeddings. Not an AST index.

**4.3 `project_symbols` / full repo indexing is premature.** Deferred. ripgrep is fast
enough on a solo dev's repo and is always current.

**4.4 Mid-task steering is asymmetric and the UI must say so.**

- Claude stage running → instruction is injected live via stdin.
- Codex stage running → queued; applied at the next stage boundary, or the user can
  choose "stop and re-run with this". No faking.

**4.5 Windows process-tree kill is the highest-risk detail.**

`claude` spawns node → bash → npm. Rust `Child::kill()` kills only the direct child
and orphans the rest. Must create a **Win32 Job Object** with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` per task, assign the child to it, and close the
handle to cancel. Build this in Milestone 1, not Milestone 14.

**4.6 Three modes is one too many for MVP.** Ship `Efficient` / `Balanced`. `Maximum`
adds a third routing table with no evidence anyone wants it. Add after real use.

## 5. Modules — 7, not 13

```
src-tauri/src/
  proc/          process spawn, Job Object, cancel, JSONL reader
  providers/     trait Provider + claude.rs + codex.rs + mock.rs
  project/       open, git state, trust check, file ranking (context)
  routing/       deterministic classifier + route builder
  orchestrator/  runs the route, owns instruction queue, emits events
  store/         SQLite + migrations + metrics writes
  git/           baseline snapshot, diff capture
src/
  views/  components/  stores/  types/
```

Merged away vs. spec: `optimization`, `execution`, `verification`, `metrics`,
`safety`, `context` fold into the above. One store, not many.

## 6. Provider interface

```rust
pub struct Invocation {
    pub cwd: PathBuf,
    pub prompt: String,
    pub capability: Capability,      // FAST | BALANCED | DEEP | IMPLEMENT | REVIEW
    pub schema: Option<serde_json::Value>,
    pub max_turns: Option<u32>,
    pub write: bool,
    pub resume: Option<String>,
}

pub enum ProviderEvent {
    Started { session_id: String },
    Text(String),
    ToolUse { name: String, summary: String },
    Usage(Usage),
    Done { result: String, structured: Option<serde_json::Value> },
    Failed { kind: FailureKind, message: String },
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    async fn detect(&self) -> Option<Detected>;      // version, auth mode
    async fn start(&self, inv: Invocation) -> Result<Handle>;
}

pub struct Handle {
    pub events: Receiver<ProviderEvent>,
    pub steering: Steering,          // Live(stdin) | Checkpoint
    pub cancel: CancelToken,
}

pub enum FailureKind {
    CliMissing, AuthExpired, UsageLimit, RateLimit,
    Timeout, Crashed, MalformedOutput, Cancelled,
}
```

`mock.rs` replays recorded JSONL fixtures. CI never spends a subscription.

## 7. SQLite schema — 6 tables

```sql
projects(id, path, name, trusted, trust_scanned_at, last_opened_at)

tasks(id, project_id, prompt, mode, route_json, status, branch, base_commit,
      dirty_at_start, started_at, ended_at, summary, diff_stat_json)

task_events(id, task_id, ts, stage, kind, provider, payload_json)
  -- append-only. absorbs: agent_calls, commands, artifacts,
  -- verification_results, routing_decisions, instructions, logs.

usage(id, task_id, event_id, provider, model, input_tokens, cached_input_tokens,
      output_tokens, reasoning_tokens, cost_usd, cost_quality)
  -- cost_quality: 'exact' | 'estimated' | 'unavailable'

file_cache(project_id, path, sha256, size, lang, indexed_at)

baselines(project_id, task_class, n, median_tokens, median_calls, updated_at)
```

Deferred: `project_symbols`, `provider_status` (live-detected, not stored),
separate `commands` / `artifacts` tables (they are `task_events` rows).

Migrations: plain numbered `.sql` files run in order. No ORM.

## 8. Routing model — deterministic, zero tokens

Classifier scores the prompt plus cheap repo signals. Pure Rust, no LLM call.

```
signals: complexity 0-10, risk 0-10,
         architecture, security, authz, schema_change, bug, refactor, frontend
         + blast_radius (candidate file count from ripgrep)
```

| Condition | Route |
|---|---|
| complexity <= 3, risk <= 3, blast <= 5 | Understand → **Implement** → Verify |
| architecture OR complexity >= 7 | Understand → **Plan** → Implement → Verify |
| security OR authz OR schema_change | Understand → **Plan** → Implement → **Review** → Verify |
| implement failed twice | escalate: Plan → Implement → Review |

`Efficient` shifts thresholds up by 2 (fewer stages). `Balanced` as written.

Capability → provider map lives in one table, not in the router:

```
DEEP/REVIEW → claude    IMPLEMENT → codex    fallback: whichever is detected
```

Every decision writes a `routing_decision` event with the signal values, so future
adaptive routing has training data without a schema change.

## 9. Mid-task instructions — exact mechanics

User types an instruction while a task runs:

1. Append to `task_events` immediately (`kind=instruction`). Never lost.
2. Append to the task's live constraint list.
3. Dispatch by what is running:

| Running | Action |
|---|---|
| Claude (stdin open) | write `{"type":"user","text":"..."}` to its stdin. Live. |
| Codex | hold. Apply at next stage boundary. UI says "will apply at next step". |
| Between stages | merge into the next stage's brief. |
| User picks "apply now" on Codex | SIGTERM (Job Object close) → `codex exec resume <id>` with the instruction prepended. Diff so far is preserved; nothing is reverted. |

Every later stage brief includes the full accumulated constraint list. An instruction
never silently expires.

## 10. Command and process safety

Permissions are enforced by the provider CLIs — Orteca configures them, it does not
reimplement them.

- Claude: `--permission-mode acceptEdits` plus explicit `--allowedTools` prefix rules.
- Codex: `--sandbox workspace-write`. **Never** `danger-full-access`.

Orteca adds one layer on top: a denylist checked against every command event.
Hard-blocked regardless of the "Automatic" setting:

```
git push --force | git reset --hard | git clean -fd | rm -rf /
DROP DATABASE | TRUNCATE | deploy | publish | npm publish
credential/keychain writes | shutdown
```

Hitting one pauses the task and asks. The setting
`Risky command confirmation: Automatic | Ask me` controls the CAUTION tier only
(installs, migrations, deletes).

Process: one Job Object per task. Cancel = close handle = whole tree dies. Then record
`cancelled`, keep the diff, show it.

## 11. Git safety

Before any stage that writes:

```
record branch, HEAD sha, `git status --porcelain`, `git stash list`
```

Isolation choice at task start: `Current tree` (default) | `New branch` | `Worktree`.
Orteca never runs a destructive git command. Diff captured with
`git diff <base_commit>` plus `git status` for untracked files.

## 12. Project intelligence

`file_cache` stores path + sha256 + language. Rescan on open: walk (respecting
`.gitignore`), hash only files whose mtime or size changed. Unchanged → reuse.

Ranking for a task brief: ripgrep the prompt's nouns → score by path match, name
match, `git log -n 50` recency, test-file adjacency. Top ~10 paths go in the brief.

## 13. Inter-stage artifacts

Enforced by `--json-schema` (Claude) and `--output-schema` (Codex). Not prose parsing.
No agent-to-agent chat.

```json
{ "objective": "", "constraints": [], "affected_areas": [],
  "implementation_steps": [], "risks": [], "tests_required": [] }
```

Review artifact:

```json
{ "findings": [{ "severity": "", "file": "", "line": 0, "issue": "", "fix": "" }],
  "verdict": "pass" }
```

`verdict: pass` → done, no fix call. This is where "calls avoided" is earned.

## 14. Milestones — 8 slices, each runnable

| # | Slice | Done when |
|---|---|---|
| 1 | Tauri 2 + Vue 3 shell, design tokens, **Job Object process runner + JSONL reader** | app opens, can spawn and kill a process tree |
| 2 | Launch screen, open project, recents, git state, **trust scan** | can open a real repo |
| 3 | Provider detect (version + auth mode) + `mock` provider + fixtures | detection shown in UI, CI green |
| 4 | Single-stage run: prompt → Codex → stream → diff → result screen | one real task end to end |
| 5 | Cancel + mid-task instruction (both paths) | can steer and stop safely |
| 6 | Classifier + multi-stage routes + structured artifacts + verify | Plan → Build → Review works |
| 7 | file_cache, path ranking, usage + baselines, route visual | metrics are honest and labelled |
| 8 | MSI/NSIS installer, signing, first-run | installable Windows app |

Spec's 17 collapsed: detection folds into one slice, metrics into one, route visual
rides along with metrics. Each slice ends commit-ready with tests.

## 15. Open decisions

1. Ship `Maximum` mode in MVP, or `Efficient` / `Balanced` only? (recommend: two)
2. Show a savings % only once a baseline exists, or show no % at all in MVP?
   (recommend: after baseline, labelled `estimated`)
