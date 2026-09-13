# Orteca — MVP Architecture

Integration surfaces verified against Claude Code and Codex docs, Sept 2026.

---

## 1. Current state

Milestones 1 to 6 are implemented, and 7 is in progress (see §14). The
acceptance checks for 1 and 2 are documented in `docs/m1-m2-verification.md`.
Milestone 8 is in progress: the NSIS installer builds unsigned (see §16).

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
- Routing with no model call: a keyword classifier picks one of five routes,
  each with call, turn and token ceilings declared before a provider starts; a
  trivial task is one Implement call that verifies itself
- Plan, Review and Verify return schema-checked artifacts and cannot edit; a
  Review that is missing, invalid, or asks for changes ends the route with
  `reviewRejected`; a Verify that is missing, invalid, ran no check, or lists a
  failing one ends it with `verifyFailed` — unless the route declared its one
  Fix call a tier up, which runs first; a budget stop ends `budgetReached` and
  never escalates on its own
- A diff that tells the run's changes from files already dirty before it
- Briefs name ranked candidate paths, never contents; the token ceiling is
  checked on cumulative usage after every turn; a finished run is compared to
  the median of comparable runs once five exist; the result shows the route
- The project screen lists recent runs with their outcome, route, calls and
  labelled usage; each run opens its patch and append-only event log
- A preflight preview shows the selected route, ceilings and existing working
  tree changes before a provider starts; provider install and sign-in can be
  cancelled and are bounded to five minutes
- Codex is refused up front on a repository whose folder ACL the user cannot
  change, instead of "finishing" with no tools (see §16)
- CI on `windows-latest`: `npm test`, `npm run build`, `cargo test`
- A route's tier picks a real model and effort on both CLIs, and moves up a tier
  when that tier has stalled in the project's own history (§4.3.4)
- Each plan's rolling limits are read from its CLI at no token cost; the
  provider with the most left is picked until the user picks one, and the
  preview warns when a route may not fit in what is left (§4.3.5)

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
Windows 11; on Windows 10 the installer fetches it with Tauri's default
download bootstrapper, so a first install there needs a network connection.

Installer: `npm run tauri build` writes a per-user NSIS setup to
`src-tauri/target/release/bundle/nsis/`. No MSI: WiX adds a second toolchain
for the same result, and per-user NSIS needs no admin prompt.

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
max_reported_tokens cumulative uncached usage (input + output, not cache reads); checked after each completed turn
preferred_tier      cheapest capable provider/model tier for this task class
```

The token value is an **inter-turn guard**, not a dishonest promise that Orteca
can interrupt an unknown number of tokens midway through a provider turn. Once a
completed turn crosses it, Orteca starts no later stage or resume automatically.
It returns `budget reached` with the work, diff, usage, and a deliberate user
choice to continue. A trivial task must not silently become a long-running
session.

**Which of these the CLIs actually enforce — checked 2026-09-12, built in M6.**
Asked for without a value, a real flag answers "argument missing" and an
invented one answers "unknown option"; `--help` short-circuits before either, so
it proves nothing on its own.

| Ceiling | claude 2.1.269 | codex-cli 0.154.0 |
|---|---|---|
| `max_turns` | `--max-turns <turns>` — **exists**, though it is absent from `--help` | **none at all** |
| artifact schema | `--json-schema <schema>`, inline JSON | `--output-schema <FILE>`, a path |
| `max_reported_tokens` | none (`--max-budget-usd` is dollars, and only on API-key billing) | none |
| `preferred_tier` | `--model <model>` + `--effort <level>` (§4.3.4) | `--model <MODEL>` + `-c model_reasoning_effort=` (§4.3.4) |

So: `max_turns` is passed straight to Claude, and for Codex **Orteca counts
completed turns itself and ends the process at the ceiling.** The number is real
either way; only who enforces it differs, and the UI is not told otherwise.
`max_reported_tokens` is Orteca's own guard for both, checked as usage is reported:
per turn for Codex, but only per `result` for Claude, which can cover several turns
(`num_turns`, counted in full towards the turn ceiling).

`preferred_tier` names a model and an effort on both command lines since
2026-09-13. What each tier maps to, why, and how it adapts is §4.3.4.

A stage that has no business editing is **stopped** from editing rather than
asked not to: `codex exec --sandbox read-only`, and for Claude — which has no
sandbox flag — a stage-specific `--allowedTools` list grants only `Read`,
`Grep`, `Glob`, read-only `git` inspection, and the allowlisted verification
commands. Claude's native edit tools are also denied. Only Implement receives
the broad shell grant, because a bare shell can write the repository even when
native edit tools are denied.

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

### 4.3.2 Re-measured through the router — 2026-09-13

Same exercise, driven through the real `start_task` (routing, ceilings, event
log) in a fresh `make-sandbox.mjs` repo, `balanced` mode, prompt: "Fix slug() in
src/slug.js so repeated separators collapse to a single dash and leading or
trailing separators are dropped. Make the tests in src/slug.test.js pass."

| Provider | Route | Calls / turns | Outcome | Reported tokens | Cached | Uncached (in + out) | Reported cost |
|---|---|---|---|---:|---:|---:|---:|
| Claude 2.1.269 | implementOnce | 1 / 5 | fix correct, 3/3 tests pass; status `budgetReached` | 215,650 | 198,572 | 17,078 | $0.1116 estimated |
| Codex 0.154.0 | implementOnce | 1 / 0 | `failed`: "Unable to verify model access" | — | — | — | unavailable |

**The target is not met.** The router does its part: a trivial task takes one
call, the brief names `src/slug.js` and `src/slug.test.js`, and the fix verifies
itself. But the tokens are not lower. The Claude total sits inside the earlier
164k-230k range, and its uncached share (17k) is above both earlier Claude runs
(7k and 13k). Almost all of it is the CLI's own cached context, which routing
does not touch. Codex could not be measured: its account did not grant model
access.

Found by the measurement: the token guard counts cache reads, so this ~190k
cached context crosses the 150k `implementOnce` ceiling on its own. The guard
also fires after the final stage's result with nothing left to stop, so a
finished, correct run reads `budgetReached`. The baseline excludes cache reads,
so the two disagree. Fixed 2026-09-13: the guard now counts uncached input and
output only, the same tokens the baseline compares. It still fires after a final
result, but only once a run has genuinely spent past the ceiling.

### 4.3.3 Runs load only what Orteca and the repo supply — 2026-09-13

A run used to start inside the user's own setup: Claude re-read ~16k of plugins,
skills, tools and a proxy MCP server every turn, and Codex opened a global skill
file and ran `rtk` before reading any code. Flags were checked the §4.2 way (a
real value-taking flag answers "argument missing"; a boolean one gets past the
parser to "Input must be provided"; an invented one answers "unknown option"),
then each was tried in a one-turn run whose `init` event or rollout was read.

| CLI | Flag | Exists | What a real run showed |
|---|---|---|---|
| claude 2.1.269 | `--setting-sources project,local` | yes | drops user settings: 3 plugins → 0, user hooks, `~/.claude/CLAUDE.md`, and its `env` (`ENABLE_TOOL_SEARCH`, the proxy `ANTHROPIC_BASE_URL`). **Leaves** claude.ai MCP connectors and 18 skills. Repo `.claude/settings.json` hooks still run. |
| claude | `--strict-mcp-config` (no `--mcp-config`) | yes | MCP servers → 0: user, claude.ai connectors **and the repo's `.mcp.json`** |
| claude | `--mcp-config` | yes | not used |
| claude | `--disable-slash-commands` | yes | skills → 0 |
| claude | `--tools Bash,PowerShell,Read,Edit,Write,Glob,Grep` | yes | built-in tools 28 → 7. Needed: with user settings gone tool search is off, so every schema loads in full — the three flags above alone gave a 46.2k first turn, with `--tools` 21.4k |
| claude | `--no-plugins` | **no** | "unknown option" |
| codex 0.154.0 | `CODEX_HOME=<empty dir>` | — | **breaks sign-in** ("Not logged in"). Not used. |
| codex | `--ignore-user-config` | yes | skips `config.toml`: plugins, MCP servers, `notify`, the proxy provider, and the user's `model` / `windows.sandbox`. Auth still from `CODEX_HOME`. Still loaded host skills and recommended plugins. |
| codex | `-c features.recommended_plugins=false` | yes | recommended-plugins message gone |
| codex | `-c skills.include_instructions=false` | yes | skills block (`~/.agents/skills`, `~/.codex/skills/.system`) gone |
| codex | `-c features.plugins=false`, `-c features.apps=false` | yes | passed for certainty; nothing further visible in the rollout |
| codex | `-c windows.sandbox="elevated"` | yes (`elevated` \| `unelevated`) | **required.** Without it `--ignore-user-config` leaves no Windows sandbox mode, and Codex silently runs `--sandbox workspace-write` as `read-only`: the first measured run could not read or edit anything yet exited 0 as `done`. Both values wrote in a probe. |
| codex | `-c project_doc_max_bytes=0` | yes | does **not** drop the global `AGENTS.md`, and would drop the repo's. Not used. |

Claude's `--bare` stays forbidden; nothing here needed it. Codex's global
`~/.codex/AGENTS.md` has no switch short of `CODEX_HOME`, so it still loads: here
that is one line, `@…\RTK.md`, which Codex does not expand but may choose to open.
Repo hooks still load for Claude, so consent stays unconditional.

One-turn "reply ok" context, same machine: Claude 54.1k → 21.4k; Codex 16.4k
(`--ignore-user-config` alone) → 12.6k.

Measured through `start_task` over CDP (open → trust → start), `balanced`, fresh
clones of `orteca-sandbox` @ `081c1ff` (typo) and `orteca-measure` @ `fb4de85`
(slug); Claude runs one after the other, Codex alongside. "Before" is the
post-guard-fix re-run from the same day.

| Run | Before | Tokens | Cached | Uncached (in + out) | Turns | First-turn context | Status | Tests |
|---|---|---:|---:|---:|---:|---:|---|---|
| Claude typo ("titel") | 163.3k / 3 turns | 65,152 | 57,948 | 7,204 | 3 | 21.4k | `done` | pass |
| Claude slug | 220.7k / 5 turns | 90,126 | 81,610 | 8,516 | 5 | 21.8k | `done` | 3/3 pass |
| Codex slug | 98.7k | 40,971 | 37,632 | 3,339 | 1 | — | `done` | 3/3 pass |

Codex fell 58%, but not like for like: skipping `config.toml` also drops the
user's `model = "gpt-5.6-luna"` at `xhigh` and the headroom proxy, so this ran
Codex's default model (`gpt-6-astra`) direct. The first attempt, before
`windows.sandbox` was set, ran read-only and was reported `done` with no diff;
that row is not counted. Codex's first command still read the global
`~/.codex/RTK.md` that `~/.codex/AGENTS.md` points at; it opened no plugin or
skill file and ran no `rtk`.

Claude: total tokens fell 60% (typo) and 59% (slug) at the same turn count; every
turn's context stayed 21-23k. Its transcripts mention no plugin, skill, `rtk`,
headroom or `.codex` path.

### 4.3.4 Which model a tier runs on — researched 2026-09-13

Flags checked the §4.2 way, with no value: `claude --model <model>`, `claude
--effort <level>` (`low`…`max`), `codex exec --model <MODEL>` and `codex exec
resume --model` all answer "argument missing". Codex has no effort flag; it
takes `-c model_reasoning_effort="…"`, the key its own `config.toml` uses. A bogus
value there is not rejected before the prompt is read, so the levels come from
the account's `~/.codex/models_cache.json` (`supported_reasoning_levels`), not
from the CLI refusing a wrong one.

| Tier | Claude | $/M in · out | Codex | $/M in · out |
|---|---|---:|---|---:|
| `cheapest` | `sonnet` (Sonnet 5), `low` | 2 · 10 | `gpt-5.6-luna`, `medium` | 0.20 · 1.20 |
| `standard` | `sonnet`, `high` | 2 · 10 | `gpt-5.6-terra`, `medium` | 2 · 12 |
| `deep` | `opus` (Opus 5), `high` | 5 · 25 | `gpt-5.6-sol`, `high` | 5 · 30 |

Considered and not chosen: Haiku 4.5 ($1 · $5, 200k, no effort control), Fable
5.1 ($10 · $50), GPT-6 Astra ($10 · $50, out 2026-09-03, Codex's default and the
model this account could not verify access to in §4.3.2), GPT-5.5 (retired for
ChatGPT sign-in on 2026-08-31).

How it was chosen:

1. **Cost per finished task, not per token.** A retry pays for the whole
   context again. The cheapest tier is used only where failure is cheap to see
   (`implementOnce`: narrow brief, one focused check).
2. **Public benchmarks only separate tiers coarsely.** SWE-bench Verified is
   saturated (Opus 5 96%, GPT-5.6 Sol 96.2%). SWE-bench Pro spreads further
   (Fable 5.1 81.2%, Sol 64.6%) but has no score for Astra, Terra, Luna or
   Sonnet 5, and vendor harnesses differ. That is enough to order models inside
   one family, and not enough to promise a success rate here.
3. **Context doesn't separate these models.** Every candidate has at least 200k,
   and §4.3.3 measured Orteca's turns at 21-23k (Claude) and a 12.6k first turn
   (Codex).
4. **Lower effort before a smaller model.** The stronger model at lower effort
   usually matches the weaker one at higher effort, so Claude's cheapest tier is
   Sonnet at `low`, not Haiku. The whole route runs on one tier, because caches
   are per model.
5. **A subscription isn't billed per token, but its allowance scales with
   price**, so the ordering holds either way. None of these prices is turned
   into a cost for Codex: it still reports tokens only, `unavailable`.
6. **Claude aliases follow the account**, and the id that ran comes back in
   `modelUsage`. Codex has no aliases, so its slugs are pinned in
   `routing::Tier::model` and must be updated when one is retired. Each `stage`
   event records the model and effort Orteca asked for, which is the only record
   of it for Codex.

**Adaptation: up on evidence, never down.**

- The same prompt already failed once (`prior_failures >= 1`): one tier up.
  After two failures the route itself escalates (§8).
- A route kind and tier with at least five finished runs (`done`,
  `budgetReached`, `reviewRejected`, `verifyFailed`) among the project's last
  50 on this provider and mode, two in five of them stalled: one tier up, again
  if that tier has stalled too, never past `deep`. `failed` is left out, because
  a rate limit or sign-in failure says nothing about the model. Modes are kept
  apart because Efficient's tighter ceilings stop runs Balanced would finish.
  A run that needed its Fix call counts as a stall of the tier it started on,
  however it ended.
- Orteca never tries a cheaper tier to see what happens: that is spending on its
  own initiative on a worse chance of success. Evidence ages out with the
  project's last 50 runs, and only then is a skipped tier tried again.
- The reason is in `route.tierReason`. The preview shows the model, effort and
  reason before anything starts.

**Not measured yet.** No real run has used these flags. The next measurement
repeats the §4.3.3 exercises on the `cheapest` tier with both CLIs and compares
them to the rows above.

Sources: Claude Code model config (code.claude.com/docs/en/model-config),
Anthropic API prices (claude-api reference, cached 2026-06-24), OpenAI prices
(cloudzero.com/blog/openai-pricing), Codex models
(learn.chatgpt.com/docs/models), SWE-bench Verified (benchlm.ai) and Pro
(codingfleet.com) leaderboards as of 2026-09-10.

### 4.3.5 How much of a plan is left — verified 2026-09-13

Both CLIs answer this without spending anything, and Orteca asks them rather
than reading a credential or a session log (`providers::limits`).

| CLI | Asked with | Answer | Cost |
|---|---|---|---|
| claude 2.1.269 | `claude -p /usage --output-format stream-json --verbose --setting-sources project,local --strict-mcp-config --max-turns 1`, from temp | a synthetic assistant message (`model: "<synthetic>"`), text lines `Current session: 66% used · resets Sep 13, 3:50pm (Europe/Kyiv)` and `Current week (all models): …` | 0 tokens, `total_cost_usd: 0` |
| codex-cli 0.154.0 | `codex app-server` on stdio: `initialize`, `initialized`, `account/rateLimits/read`, sent back to back | `rateLimits.primary` / `secondary`: `usedPercent`, `windowDurationMins` (300, 10080), `resetsAt` (Unix seconds) | no thread started |

`/usage` is the one `claude` call without `--disable-slash-commands`, because it
is a slash command. Its reset time is the CLI's own words and is shown
verbatim. On an API key it prints no windows, and the reading says so. Claude's
run stream also carries `rate_limit_event` with the same utilizations, but only
once a run has started; the fixtures keep two.

What Orteca does with it, in `Project.vue`:

- **Every window is shown on the helper row** with the CLI's label; an unread
  one says `limits unavailable:` and why. It is never shown as 0%.
- **The provider is picked by headroom**, the room left in its tightest window,
  among installed, signed-in CLIs, and only when every one of them has a
  reading. A tie keeps the current choice. Once the user picks, headroom stops
  choosing. Limits are read again after every run.
- **The preview warns** when the fullest window has less room left than 5% per
  call the route may make, its Fix call included. The warning names only
  reported figures. **The 5% is a guess, not a measurement:** replace it with
  each route's measured draw once runs read limits before and after.
- Nothing is blocked: a warning is not a refusal, and a run that does hit a
  limit already ends `failed` with `usageLimit`. The tier is not stepped down to
  save allowance; §4.3.4 moves up on evidence and never down.

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
  migrations/0001_init.sql  0002_tasks.sql  0003_calls.sql  0004_task_details.sql
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
    run.rs         route runner: stages, argv, stream, budgets, event log, diff, result
    routing.rs     deterministic classifier + route builder + stage briefs
    orchestrator.rs *  folded into run.rs — see below
src/
  main.ts  App.vue  api.ts  types.ts
  views/      Launch.vue  Project.vue
  components/ VeloMark.vue  TrustPrompt.vue
  styles/     tokens.css
scripts/make-icon.mjs
```

`git.rs` never happened: baseline snapshot and diff capture are four functions
next to `git_state`, and `project.rs` already owns every git call.

`orchestrator.rs` never happened either. Milestone 6 turned `run.rs` from a
single-stage runner into a route runner, and that cost a loop: the process
lifetime, the event log, the instruction queue and the cancel path are all
already there, and a second module would have had to borrow every one of them to
own a `for` over four stages. The one-call route is still exactly the path
Milestone 4 shipped, now with a budget attached.

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
  -- status: 'running' | 'done' | 'cancelled' | 'failed' |
  -- 'budgetReached' | 'reviewRejected' | 'verifyFailed'. A stop is its own outcome: the work
  -- so far is real and nothing was reverted.

task_events(id, task_id, ts, stage, kind, provider, payload_json)
  -- append-only. absorbs: agent_calls, commands, artifacts,
  -- verification_results, routing_decisions, instructions, logs.

usage(id, task_id, event_id, provider, model, input_tokens, cached_input_tokens,
      output_tokens, reasoning_tokens, cost_usd, cost_quality)
  -- cost_quality: 'exact' | 'estimated' | 'unavailable'. Every token column is
  -- nullable: a run that dies before its provider reports usage has no honest
  -- number, and a zero would read as "this was free".

-- 0003, shipped
tasks.calls_used   -- exact provider processes started; NULL on older rows

-- deferred
file_cache(project_id, path, sha256, size, lang, indexed_at)
  -- nothing reads a hash yet: ranking uses path text and git history, and
  -- git's own index already knows which tracked files changed.

-- not a table: baselines are a query over tasks + usage (see §12), because a
-- stored median is a second copy of rows that already exist.
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
| complexity <= 3, risk <= 3, blast <= 5, and blast >= 1 or a small-edit word | **Implement once** — inspect named paths, edit, run one focused verification inside that call; no Plan or Review |
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

**As built (Milestone 6).** `routing.rs` is pure and makes no model call of any
kind: keyword tables over the prompt, plus one `git ls-files` for blast radius
and one `COUNT(*)` for prior failures. Same inputs, same route, every time.

Rule order is not the table's order. The two escalating rules are tested first,
because a prompt that scores trivially but touches authorisation is not a
trivial task, and one that has already failed twice is not a candidate for the
route that just failed it. The table also had no default row; there is now a
`Standard` route — Implement → Verify — for everything in the middle.

| Route | Stages | Calls |
|---|---|---|
| `ImplementOnce` | Implement | 1 |
| `Standard` | Implement → Verify | 2 |
| `Planned` | Plan → Implement → Verify | 3 |
| `Escalated` | Plan → Implement → Review | 3 |
| `Guarded` | Plan → Implement → Review → Verify | 4 |

Two rules the spec did not write down, both of which exist to stop Orteca
spending on its own initiative:

- **A Review that is missing, invalid, or returns `changes_requested` ends the
  route there with `reviewRejected`; a Verify that does not report a pass
  backed by at least one check, none failing, ends it with `verifyFailed`.** A
  stage that ends cleanly while printing failures is not `done`, and no Verify
  call is spent confirming what a review has already rejected. The findings are
  the result; what to do about them is the user's to choose.
- **Bounded escalation: one Fix call, declared before the run.** A route with a
  Review or Verify stage whose tier is not `deep` carries `budget.escalation`,
  the next tier up. The first Review or Verify that does not pass is followed by
  one `fix` stage on that tier, in place of whatever stages were left: it is
  handed the failing artifact, may edit, and returns the Verify artifact. A
  passing Fix makes the task `done`; anything else is `verifyFailed`. There is
  never a second Fix, `deep` and `implementOnce` routes have none, the call
  ceiling grows by exactly that one call, and a budget stop never escalates.
- **A held instruction is delivered by the next stage's brief, not by resuming
  the stage it arrived in** — unless that stage is the last one, where M5's
  behaviour is unchanged. Resuming as well would pay for a second process to say
  the same thing twice.

"Failed twice" is matched on the exact prompt text in the same project, and
counts a `budgetReached` run as not having finished. Exact text is the only
honest definition available without a model call; a re-worded retry counts as a
fresh task, which errs towards the cheaper route.

Every stage runs on **the provider the user selected**. The capability map is
recorded per stage and not acted on: routing a stage to a CLI the user has not
signed into fails the run for a reason the screen never mentioned, and the
router cannot see per-stage auth state yet. The tier does act: it picks the
model on the selected provider (§4.3.4).

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

**As built.** A tree that is dirty at start is snapshotted before the first
provider starts: every already-changed path, with a hash of its contents. After
the run each diff entry is labelled `run` (clean before), `beforeRun` (changed
before, byte-identical after — the user's, not counted as the run's work) or
`both` (changed before and again; line counts are against the commit, so they
include both). If git cannot produce the snapshot the origin is `null` and the
screen falls back to saying it cannot tell. A file the run restored to the
commit drops out of the diff and is not reported.

## 12. Project intelligence

`file_cache` stores path + sha256 + language. Rescan on open: walk (respecting
`.gitignore`), hash only files whose mtime or size changed. Unchanged → reuse.

Ranking for a task brief: ripgrep the prompt's nouns → score by path match, name
match, `git log -n 50` recency, test-file adjacency. Top ~10 paths go in the brief.

**As built (Milestone 7).** No file cache yet — see §7. Ranking is pure
(`routing::candidates`) over `git ls-files` and `git log -n 50 --name-only`:
a file named for a prompt word scores 3, a path containing one scores 1, a path
in recent history gets +2, and a test named like a matched file (`test_slug.py`
beside `slug.rs`) is listed with it without counting towards blast radius.
Ties go shallowest and shortest first. The top 10 go in the brief, ranked.

The token ceiling is checked on cumulative reported usage after every turn,
where the turn ceiling is checked, as well as before each stage. A single-stage
route therefore stops at the turn that crossed it rather than never.

Baselines: the median tokens and calls of the last 20 runs in the project that
finished `done` on the same route kind and provider and reported usage. Fewer
than five → no baseline, and the result screen says there is no savings figure
yet. With one, a finished run shows its difference from the median, labelled
estimated. The same route is not the same work, so it is never shown as exact.

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

`verdict: pass` → done, no fix call. A missing, malformed, or
`changes_requested` review → `reviewRejected`, no Verify call. This is where
"calls avoided" is earned.

**As built (Milestone 6).** Both flags exist and both are used: Claude takes the
schema inline, Codex takes a file, written into the temp directory Orteca owns
and never into the user's repository. Plan, Review and Verify contract for an
artifact. Verify's is `{checks: [{command, passed, output}], verdict: pass |
fail}`, and a pass needs at least one check with none failing. Implement is
judged by the diff, and wrapping a code change in a JSON envelope buys nothing.

What an artifact is worth is decided *after* the stage, by a shallow check of
required keys against the shape that stage contracted for. It is not a second
JSON Schema validator — the provider's flag is that — it is there so a missing
or half-built artifact is recorded as **invalid** rather than passed on as if it
were a plan. Either way an `artifact` row is written, valid or not.

The runner captures the provider's schema-constrained machine value from the
provider-specific event that carries it; Codex's current CLI emits the value as
the completed agent message text, while Claude emits `structured_output` on its
result. Ordinary assistant prose is never mined for fields. When no artifact
comes back, the stage's own closing words are forwarded to the next brief
**verbatim and labelled unvalidated**.

## 14. Milestones — 8 slices, each runnable

| # | Slice | Done when | State |
|---|---|---|---|
| 1 | Tauri 2 + Vue 3 shell, design tokens, **Job Object process runner + JSONL reader** | app opens, can spawn and kill a process tree | done |
| 2 | Launch screen, open project, recents, git state, **trust scan** | can open a real repo | done |
| 3 | Provider detect (version + auth mode) + `mock` provider + fixtures | detection shown in UI, CI green | done |
| 4 | Single-stage run: prompt → Codex → stream → diff → result screen | one real task end to end | done |
| 5 | Cancel + mid-task instruction (both paths) | can steer and stop safely | done |
| 6 | Classifier + budgeted routes + structured artifacts + verify | classifier adds no model call; a trivial task has a one-call route; every route has explicit call/turn ceilings and never auto-escalates after a budget stop | done |
| 7 | file_cache, path ranking, usage + baselines, route visual | brief names ranked paths without file contents; cumulative provider usage enforces the inter-turn token guard; comparable-task baselines make savings estimates honest | ranking, token guard, baselines, route visual built; file_cache deferred |
| 8 | MSI/NSIS installer, signing, first-run | installable Windows app | NSIS installer, single instance and missing-git first-run message built; signing waits on a certificate |

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
4. **`budgetReached` is a fifth task status, and `reviewRejected` and
   `verifyFailed` are explicit stop outcomes beside it.** Neither is a failure when the provider completed
   normally; neither is a success because the route did not finish. The work,
   diff, findings, and reported usage are kept exactly as they are, and the
   remaining stages are named so the user knows what they are being asked to
   decide about. Continuing is a fresh Run — a deliberate act — and Orteca never
   takes it on their behalf. The single exception is the one Fix call a route
   declares in its budget before it starts.
5. **The one exemption from the call ceiling is an instruction the user gave.**
   A resume that carries a mid-task instruction is allowed past `max_agent_calls`
   and is still counted and logged. The rule the budget enforces is that Orteca
   never spends more *on its own initiative*; refusing here would lose an
   instruction the user was promised would arrive.

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
  same way and no environment variable can fix it. `proc::can_change_acl` asks
  for a WRITE_DAC handle on the repository before a Codex run and `prepare_run`
  refuses with a message saying to move the repo or use claude.
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
- **A flag missing from `--help` is not a missing flag.** `claude --max-turns`
  is real and accepted at 2.1.269 but undocumented in the help output, and the
  budget in §4.2 would have been built around a ceiling Orteca could not set if
  the help text had been taken as the answer. Ask the CLI for the flag with no
  value: a real one says `option '--max-turns <turns>' argument missing`, an
  invented one says `unknown option`. `--help` short-circuits before either
  check and exits 0 whatever you put in front of it, so it proves nothing.
- Codex has no turn ceiling at 0.154.0 and no way to add one. Orteca counts
  `turn.completed` and closes the Job Object at the ceiling. This cannot stop a
  turn that is already running, and neither the doc nor the UI says it can.
- Never invoke a real provider CLI from a test. Fixtures are recorded JSONL
  replayed by `providers::mock`.
- The Rust crate root is `src-tauri/`; run `cargo` from there. `npm run tauri
  dev` runs from the repo root.
- Every metric written to the `usage` table carries a `cost_quality` of
  `exact`, `estimated` or `unavailable`. Nothing reaches the UI without one.
- **Signing needs a certificate the repo does not have.** Nothing is committed
  for it: a thumbprint is machine-specific and a key never belongs in git. With
  a code-signing certificate in the user store, sign at build time with
  `npm run tauri build -- --config '{"bundle":{"windows":{"certificateThumbprint":"<sha1>","digestAlgorithm":"sha256","timestampUrl":"http://timestamp.digicert.com"}}}'`.
  Tauri then signs the app exe and the installer with `signtool`. Unsigned
  builds work but SmartScreen warns on first launch.
- **First run on a clean machine.** The data directory is created by
  `Store::open`. No git on PATH used to read as "not a git repository" for
  every folder; `open_project` now says Git is missing. Missing CLIs were
  already a normal state on the project screen. A second launch used to exit
  with no window, because the running instance holds the database; the
  single-instance plugin now focuses the existing window instead.
