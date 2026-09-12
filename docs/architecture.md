# Orteca — MVP Architecture

Integration surfaces verified against Claude Code and Codex docs, Sept 2026.

---

## 1. Current state

Milestones 1 to 4 are implemented. The acceptance checks for 1 and 2 are
documented in `docs/m1-m2-verification.md`. Milestones 5-8 are not started.

What exists and works:

- Tauri 2 + Vue 3 + strict TS shell, Orteca design tokens
- Win32 Job Object process runner, proven to kill a detached grandchild
- JSONL event reader over stdout, stdin held open for steering
- SQLite with migrations, `projects` table
- Open a project: git state, trust scan, recents
- Provider detection (PATH resolution, version, auth mode), shown on the project
  screen; `mock` replays JSONL fixtures through the real event parsers
- A single-stage run: prompt to one CLI, live stream, append-only event log,
  working-tree diff, and a result labelled with its cost quality
- Installing a missing CLI from the project screen, via the user's own npm

Build (Rust lives in `src-tauri/`, run from there for cargo):

```
npm install
node scripts/make-icon.mjs      # only if src-tauri/icons/icon.ico is missing
npm run build                   # vue-tsc + vite
cd src-tauri && cargo test
npm run tauri dev               # from the repo root
```

Toolchain: Node 22, Rust 1.98 (MSVC), VS 2022 Build Tools with
`Microsoft.VisualStudio.Component.VC.Tools.x86.x64`. WebView2 ships with
Windows 11.

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

**Mitigation:** every untrusted project requires consent, including projects with
no recognized configuration. The scan explains known sources in the repository,
nested folders and ancestors. It groups `.claude`, `.codex` and `.agents` resources
and reports unreadable paths, links and scan limits. Custom filenames therefore
cannot bypass consent. Git discovery disables `core.fsmonitor` before consent.
The decision is stored for that repository.

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

Install: `npm install --global @openai/codex`. Orteca shells out to the user's
own npm and never fetches a binary itself, and it runs the install from a
temporary directory so the opened repository's `.npmrc` cannot redirect the
registry before that repository has been consented to. Claude Code installs the
same way from `@anthropic-ai/claude-code`.

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
**Decided: two modes.**

### Found while building (not from docs)

- `CreateJobObjectW` is gated behind the `windows` crate's `Win32_Security`
  feature, because it takes `SECURITY_ATTRIBUTES`. Easy to miss; the compiler
  reports it as a plain missing import.
- `tauri-build` requires `icons/icon.ico` on Windows even for `cargo test`, not
  just for packaging. Generated by `scripts/make-icon.mjs` so no opaque binary
  is checked in.
- SQLite `datetime('now')` resolves only to the second, so ordering recents by
  it ties. `projects.opened_seq` is the real ordering key; the timestamp is for
  display only.
- A user may open a subdirectory of a repo. `open_project` resolves to the git
  root via `rev-parse --show-toplevel` and stores that, so a project is one row
  however it was opened.

## 5. Modules — 7, not 13

A module is a folder only once it outgrows one file. Actual layout today,
`*` marks what is not written yet:

```
src-tauri/
  migrations/0001_init.sql
  src/
    main.rs        Tauri commands + app setup
    error.rs       AppError { kind, message }, serialized to the frontend
    proc/          spawn, Job Object, cancel, JSONL reader
      mod.rs
      job.rs
    project.rs     git state, trust scan, path validation
    store.rs       SQLite, migrations, projects
    providers/     detect + event normalisation + mock fixture replay
      mod.rs       ProviderId, Detected, ProviderEvent, `which`
      claude.rs    stream-json parser
      codex.rs     exec --json parser
      mock.rs      replays fixtures/*.jsonl through the real parsers
    run.rs         one stage: argv, stream, event log, diff, result
    routing.rs *   deterministic classifier + route builder
    orchestrator.rs *  runs the route, owns the instruction queue
src/
  main.ts  App.vue  api.ts  types.ts
  views/      Launch.vue  Project.vue
  components/ VeloMark.vue  TrustPrompt.vue
  styles/     tokens.css
scripts/make-icon.mjs
```

`git.rs` never happened: baseline snapshot and diff capture are four functions
next to `git_state`, and `project.rs` already owns every git call. `run.rs` is
the single-stage runner; the orchestrator that owns a multi-stage route and an
instruction queue arrives with routing in Milestone 6.

Merged away vs. spec: `optimization`, `execution`, `verification`, `metrics`,
`safety`, `context` fold into the above. One store, not many. No Pinia — two
screens and a modal do not need a state library.

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
    AuthExpired, UsageLimit, RateLimit, Timeout, Crashed,
}
```

`mock.rs` replays recorded JSONL fixtures. CI never spends a subscription.

**As built (Milestone 3).** No `async_trait` and no `Provider` trait yet: a
`ProviderId` enum with `detect()` and `parse_line()` covers detection and
normalisation, and dynamic dispatch has nothing to dispatch on until the
orchestrator picks a provider at runtime. `Invocation`, `Handle`, `Steering`
and `CancelToken` did not land with the first real run either — see the verdict
below.

`parse_line` takes one raw JSONL line and returns zero or more events: a Claude
`result` is both a `Usage` and an outcome, and one assistant message can hold
text and a tool call together. `Done.result` is empty for Codex, which reports
no final-answer field — the caller keeps the last `Text`.

`FailureKind` lists only what a parser produces. `CliMissing`, `Cancelled` and
`MalformedOutput` were dropped, because they are the process runner's to report.
Milestone 4 answered which of them the runner really needs: `CliMissing` became
an `AppError` raised before a task row is even opened, and a process that exits
without a result becomes a `failed` task carrying the exit code and the tail of
its non-JSON output. Neither is a `FailureKind`, so the enum did not grow. The other three are live: neither CLI reports a
machine-readable error code, so `classify_failure` matches the phrases both
providers share in their free-text message and leaves anything unrecognised as
`Crashed`. A wrong bucket sends the user to fix the wrong thing.

**Usage is only ever read from Claude's final `result`, and that is a real
limit, not an oversight.** Per-message `usage` reports the output count the API
had at `message_start`, and one API response can produce several assistant
messages that all repeat that same placeholder — so summing them and keeping
the last are both wrong. A run cancelled or crashed before `result` therefore
has no honest token count at all. That is recorded as `cost_quality:
unavailable` with every token column NULL; nothing writes a zero, which would
read as "this was free". The result screen says "token count unavailable".

**Verdict on the trait, asked and answered — again, after building the run.**
The enum still stands, and `Invocation`/`Handle`/`Steering` were not needed
either. A run is `run::args(id, prompt)` returning a `Vec<String>` plus the one
`proc::spawn` every provider shares; the only per-provider difference is the
argv, which is one `match`. A trait would dispatch a single method that returns
data. Revisit when steering makes `start()` genuinely asymmetric in Milestone 5.

Fixtures in `src-tauri/fixtures/*.jsonl` are written to the event shapes
verified above, not captured from a live session, because neither CLI is
installed. Swap in a real capture when one exists; nothing else changes.

Detection resolves the program against PATH x PATHEXT itself. `CreateProcess`
only ever appends `.exe`, so `claude.cmd` — how both CLIs install on Windows —
is invisible to a bare program name. The resolved path is what `run.rs` spawns.

## 7. SQLite schema — 6 tables

Migration 0001 exists; the rest land with the milestone that needs them.

```sql
-- 0001 and 0002, shipped
projects(id, path, name, trusted, trust_scanned_at, last_opened_at, opened_seq)
  -- path is the git root and is UNIQUE. opened_seq orders recents.

tasks(id, project_id, prompt, mode, route_json, status, branch, base_commit,
      dirty_at_start, started_at, ended_at, summary, diff_stat_json)

task_events(id, task_id, ts, stage, kind, provider, payload_json)
  -- append-only. absorbs: agent_calls, commands, artifacts,
  -- verification_results, routing_decisions, instructions, logs.

usage(id, task_id, event_id, provider, model, input_tokens, cached_input_tokens,
      output_tokens, reasoning_tokens, cost_usd, cost_quality)
  -- cost_quality: 'exact' | 'estimated' | 'unavailable'. Every token column is
  -- nullable: a run that dies before its provider reports usage has no honest
  -- number, and a zero would read as "this was free".

-- not written yet
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

| # | Slice | Done when | State |
|---|---|---|---|
| 1 | Tauri 2 + Vue 3 shell, design tokens, **Job Object process runner + JSONL reader** | app opens, can spawn and kill a process tree | done |
| 2 | Launch screen, open project, recents, git state, **trust scan** | can open a real repo | done |
| 3 | Provider detect (version + auth mode) + `mock` provider + fixtures | detection shown in UI, CI green | done |
| 4 | Single-stage run: prompt → Codex → stream → diff → result screen | one real task end to end | done |
| 5 | Cancel + mid-task instruction (both paths) | can steer and stop safely | next |
| 6 | Classifier + multi-stage routes + structured artifacts + verify | Plan → Build → Review works | |
| 7 | file_cache, path ranking, usage + baselines, route visual | metrics are honest and labelled | |
| 8 | MSI/NSIS installer, signing, first-run | installable Windows app | |

Spec's 17 collapsed: detection folds into one slice, metrics into one, route visual
rides along with metrics. Each slice ends commit-ready with tests.

## 15. Decisions taken

1. **Two modes**, `Efficient` and `Balanced`. No `Maximum` in MVP.
2. **No savings percentage until a baseline exists.** Once a project has >= 5
   comparable tasks, show a rolling-median comparison labelled `estimated`.
   Before that, the result screen shows absolute tokens and calls avoided.

## 16. Notes for whoever picks this up

- Both CLIs are now installed on the dev machine (`@anthropic-ai/claude-code`
  2.1.269, `@openai/codex` 0.154.0), but neither is usable for a full run:
  `claude` is logged out and `codex` is out of credits. "CLI missing" stays a
  first-class state, and `mock` is still what tests run against.
- The argv in `run::args` was checked against both CLIs' `--help` on 2026-09-12.
  Every flag exists, and on the same day the first real runs went end to end
  against a throwaway repo: a question answered with no edits, and a build that
  edited a file and produced a diff.
- **A headless run must never wait on a permission prompt.** `claude --print`
  defaults to `--permission-prompts host`, and Orteca is not an SDK host, so the
  first build run stalled: the agent asked four times to run a test and nothing
  could answer. Orteca passes `--permission-prompts none`, which denies instead
  of hanging. `acceptEdits` also only auto-approves *edits*, so `--allowedTools
  Bash PowerShell` is what lets an agent verify its own work; `CLAUDE_DENY_COMMANDS`
  narrows that back down, because deny beats allow.
- `bypassPermissions` was considered and rejected. Whether a denylist still
  applies under it is undocumented and could not be tested, and unlike
  `codex --sandbox workspace-write` it is not an OS-level fence — Claude exposes
  no sandbox flag at all. Allow-plus-deny gets the same power with documented
  precedence. Do not swap it for the mode without evidence.
- **Auth comes from the CLI, never from a credential file.** `claude auth status
  --json` and `codex login status` are the only sources. The old check - does
  `~/.claude/.credentials.json` exist - reported a saved login for a user who had
  never signed in, because on Windows that file also holds MCP server tokens.
  A signed-out provider now disables Run and offers `sign_in_provider`, which
  spawns the CLI's own browser flow. Orteca renders no login form.
- Never invoke a real provider CLI from a test. Fixtures are recorded JSONL
  replayed by `providers::mock`.
- The Rust crate root is `src-tauri/`; run `cargo` from there. `npm run tauri
  dev` runs from the repo root.
- Every metric written to the `usage` table carries a `cost_quality` of
  `exact`, `estimated` or `unavailable`. Nothing reaches the UI without one.
