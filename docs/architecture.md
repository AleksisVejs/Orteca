# Orteca — MVP Architecture

Integration surfaces verified against Claude Code and Codex docs, Sept 2026.

---

## 1. Current state

Milestones 1 to 5 are implemented. The acceptance checks for 1 and 2 are
documented in `docs/m1-m2-verification.md`. Milestones 6-8 are not started.

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
- Stopping a live run: the Job Object takes the whole tree, the stop is written
  to the event log, and the task ends `cancelled` rather than `failed`
- Mid-task instructions: Claude takes one live on stdin mid-turn, Codex holds
  one until asked to apply it and then resumes its own session with it
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

**4.2 The context engine must cut exploration, and it must enforce a budget.**

Sending curated file *contents* fights the agent's own retrieval and often pays for
the same bytes twice. The leverage is cheaper: a tight brief that **names paths** so
the agent stops hunting, plus a `--max-turns` ceiling.

MVP context engine = ripgrep + path heuristics + `git log` recency → a ranked list of
~10 likely paths, pasted into the prompt as "start here". Not file contents. Not
embeddings. Not an AST index. It also gives every route an `ExecutionBudget`:

```text
max_agent_calls     maximum provider processes/stages
max_turns           provider turn ceiling where the CLI supports one
max_reported_tokens cumulative reported usage; checked after each completed turn
preferred_tier      cheapest capable provider/model tier for this task class
```

The token value is an **inter-turn guard**, not a dishonest promise that Orteca
can interrupt an unknown number of tokens midway through a provider turn. Once a
completed turn crosses it, Orteca starts no later stage or resume automatically.
It returns `budget reached` with the work, diff, usage, and a deliberate user
choice to continue. A trivial task must not silently become a long-running
session.

**4.3 `project_symbols` / full repo indexing is premature.** Deferred. ripgrep is fast
enough on a solo dev's repo and is always current.

### 4.3.1 Observed efficiency baseline — 2026-09-12

These are the values shown by the desktop provider-run summaries for the same
throwaway slug-normalisation exercise. They are retained as an observed product
baseline, not as a billing record: provider-reported token semantics and account
allowance consumption are related but are not interchangeable.

| Provider | Run | Outcome | Reported tokens | Cached tokens | Cached share | Reported cost |
|---|---|---|---:|---:|---:|---:|
| Codex | collapse repeated separators / trim boundaries | 6 tests passed | 89,751 | 81,920 | 91.3% | unavailable |
| Codex | leading-punctuation regression | 7 tests passed | 109,115 | 104,064 | 95.4% | unavailable |
| Codex | mixed spaces-and-punctuation regression | 7 tests passed | 257,144 | 238,592 | 92.8% | unavailable |
| Claude | live-steered punctuation regression | 7 tests passed | 230,296 | 217,236 | 94.3% | $0.1033 estimated |
| Claude | M5 completion confirmation | 7 tests passed | 164,616 | 157,473 | 95.7% | $0.0664 estimated |
| **Total** | **five tiny-task runs** | **all functional** | **850,922** | **799,285** | **93.9%** | **$0.1697 estimated (Claude only)** |

The code change was a two-file slug fix plus one focused test command. These
numbers are therefore **not an acceptable efficient-task baseline**. High cache
share explains repeated context reuse, not proportional work. M6/M7 are only
complete when a comparable trivial task takes the one-call route and the new
baseline is materially lower without reducing verification quality.

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
  -- status: 'running' | 'done' | 'cancelled' | 'failed'. A stop is its own
  -- outcome: the work so far is real and nothing was reverted.

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
| complexity <= 3, risk <= 3, blast <= 5 | **Implement once** — inspect named paths, edit, run one focused verification inside that call; no Plan or Review |
| architecture OR complexity >= 7 | Understand → **Plan** → Implement → Verify |
| security OR authz OR schema_change | Understand → **Plan** → Implement → **Review** → Verify |
| implement failed twice | escalate: Plan → Implement → Review |

`Efficient` shifts thresholds up by 2 (fewer stages) and chooses the cheapest
capable configured tier. `Balanced` uses the route as written. A route declares
its call and turn ceilings before any provider is started; tiny tasks have a
one-call ceiling. Escalation is never automatic after a budget stop: the user
must choose to spend more.

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
| Claude (stdin open) | write `{"type":"user","message":{"role":"user","content":"..."}}` to its stdin. Live. |
| Codex | hold. Apply when the current process completes, or at the next stage boundary. UI says "will apply at next step". |
| Between stages | merge into the next stage's brief. |
| User picks "apply now" on Codex | Job Object close → `codex exec resume <id>` with the instruction prepended. Diff so far is preserved; nothing is reverted. |

Every later stage brief includes the full accumulated constraint list. An instruction
never silently expires.

### Verified against the CLIs, not assumed

The first draft of this section guessed `{"type":"user","text":"..."}`, which the
CLI does not accept. These facts came out of live runs and are retained in
fixtures and regression tests so they do not need paying for again.

1. **The message shape is the Messages-API one**, nested under `message`.
   `--replay-user-messages` echoes an accepted message back, which is the cheap
   way to check this without reading the answer.
2. **A live instruction is part of the active turn.** A message written while
   Claude is working is incorporated into that turn and does not necessarily
   produce another `result`; the sandbox steering run completed both requests
   with one result. A message written after a result starts another turn, as the
   two-result fixture demonstrates. Orteca therefore closes stdin at the active
   turn's result, and an instruction racing after that point is reported
   `tooLate` rather than leaving the task waiting for a result that will not come.
3. **Usage is per turn but `total_cost_usd` is a session running total.** In the
   recording the cost goes 0.0302 → 0.0405 while turn two's own output is six
   tokens. So `Usage::absorb` adds the tokens and replaces the cost. Treating
   both the same way in either direction reports a wrong number.
4. **`codex exec resume` has no `--sandbox` flag** — only
   `--dangerously-bypass-approvals-and-sandbox`, which this project forbids. The
   sandbox travels as `-c sandbox_mode="workspace-write"`; a bogus value is
   rejected with the three valid variants named, which is how the spelling was
   confirmed without spending a run. Losing this would leave a resumed agent
   read-only while still exiting 0 — the failure already documented in §16.

**How it is built.** `run::Live` holds one control sender per live task, and
`stream` selects over that channel alongside the CLI's output. `Control` is
`Cancel` or `Instruct { text, apply_now, reply }`. The reply is completed by the
run loop, not when the control is merely enqueued, so the UI only clears words
the provider can still take. Every instruction is written to `task_events` with
its disposition — `live`, `held`, `resumed` or `tooLate`. A resume re-enters the
same `stream` with new argv, keeping one event log, one token total and one diff
baseline across both processes; the kill that hands over is not reported as a
crash.

A held Codex instruction is applied by resuming the session when its current
process completes. "Apply now" ends that process early; if it arrives before
Codex reports a session ID, the request remains pending and restarts as soon as
the ID arrives. The UI says plainly that Send waits for the natural boundary.

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

The brief also carries its route budget and a concise completion contract:
target paths, one focused test command when known, and the instruction to stop
after success. It must not paste file contents, whole repository status, or a
generic multi-stage checklist into a small task.

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
| 5 | Cancel + mid-task instruction (both paths) | can steer and stop safely | done |
| 6 | Classifier + budgeted routes + structured artifacts + verify | classifier adds no model call; a trivial task has a one-call route; every route has explicit call/turn ceilings and never auto-escalates after a budget stop | |
| 7 | file_cache, path ranking, usage + baselines, route visual | brief names ranked paths without file contents; cumulative provider usage enforces the inter-turn token guard; comparable-task baselines make savings estimates honest | |
| 8 | MSI/NSIS installer, signing, first-run | installable Windows app | |

Spec's 17 collapsed: detection folds into one slice, metrics into one, route visual
rides along with metrics. Each slice ends commit-ready with tests.

## 15. Decisions taken

1. **Two modes**, `Efficient` and `Balanced`. No `Maximum` in MVP.
2. **No savings percentage until a baseline exists.** Once a project has >= 5
   comparable tasks, show a rolling-median comparison labelled `estimated`.
   Before that, the result screen shows absolute tokens and calls avoided.
3. **Efficiency is a control, not a slogan.** Do not claim a task was efficient
   merely because most input was cached. The run must have a declared budget,
   a small-task single-call path, and an honest budget-reached outcome before
   Orteca can make that claim.

## 16. Notes for whoever picks this up

- Both CLIs are installed on the dev machine (`@anthropic-ai/claude-code`
  2.1.269, `@openai/codex` 0.154.0). `claude` is signed in on a claude.ai
  subscription and can complete a real run; `codex` reports a login but its
  credit state is unknown. "CLI missing" stays a first-class state, and `mock`
  is still what tests run against — no test ever invokes a real CLI.
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
- **A child's TEMP is part of the sandbox contract.** Codex's Windows sandbox
  grants a write ACE on every write root, and TEMP is one of them. If TEMP is a
  directory the user cannot re-ACL - `E:\Temp` owned by `BUILTIN\Administrators`
  grants Modify, which excludes WRITE_DAC - `SetNamedSecurityInfoW` fails with
  ERROR_ACCESS_DENIED, the sandbox never starts, and `codex exec` still **exits 0**
  with a polite final message while the agent had no tools at all. `proc::spawn`
  therefore gives every child a TEMP under `%LOCALAPPDATA%pp.orteca	mp`. The
  workspace is also a write root, so a repository under such a directory fails the
  same way and no environment variable can fix it - that one needs a real UI state.
- **An event the parser cannot read is still written to the log.** Both parsers
  match a subset of their CLI's event types and drop the rest, which is how the
  run above recorded a clean `done` with no trace of why it did nothing. Unparsed
  JSON lines are appended verbatim under `kind = 'unknown'` and are not emitted,
  so the log stays complete without putting shapes the UI cannot render on screen.
- **npm cannot run inside Codex's Windows sandbox, and must not be made to.**
  npm resolves its own path with a realpath that walks every ancestor, and it is
  installed under the user profile, so it dies on `EPERM: lstat 'C:\Users\<user>'`
  before it starts. PowerShell separately refuses to load `npm.ps1` under a
  Restricted execution policy. `node` itself runs fine. The obvious fix -
  `codex exec --add-dir <npm prefix>` - is **forbidden**: that prefix is
  `%APPDATA%\npm`, where `claude` and `codex` themselves live, so granting it
  would let an agent overwrite the provider CLIs. There is no headless way to add
  a *read* root; `/sandbox-add-read-dir` is an interactive TUI command. Until
  Codex offers one, an agent that reaches for npm burns several turns finding a
  way around it, and the cheapest mitigation is to tell it not to.
- Never invoke a real provider CLI from a test. Fixtures are recorded JSONL
  replayed by `providers::mock`.
- The Rust crate root is `src-tauri/`; run `cargo` from there. `npm run tauri
  dev` runs from the repo root.
- Every metric written to the `usage` table carries a `cost_quality` of
  `exact`, `estimated` or `unavailable`. Nothing reaches the UI without one.
