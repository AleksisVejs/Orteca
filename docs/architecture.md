# Orteca — MVP Architecture

Integration surfaces verified against Claude Code and Codex docs, Sept 2026.

---

## 1. Current state

Milestones 1 to 7 are implemented; the deferred file cache shipped as the code
map (§12). The
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
- Routing with one small model call: the provider's smallest model reads the
  prompt as chat, a question, or easy/medium/hard work (`intent.rs`, any language),
  and the keyword classifier picks one of six routes,
  each with its stages and tiers declared before a provider starts; a trivial
  task is one Implement call, which verifies itself unless Orteca can run the
  repository's tests after it (§4.3.7)
- Plan, Review and Verify return schema-checked artifacts and cannot edit. A
  failed Verify gets one Fix on the same tier, resuming the session that wrote
  the change, then runs again; a Review runs once and its Fix is judged by
  Verify. A check that still fails ends `verifyFailed` or `reviewRejected` with
  the work kept (§8, §4.3.10)
- Orteca runs the declared tests once before the agent starts. A suite that
  already fails is shown to the agent and buys no Fix when it fails again
  (§4.3.10)
- A diff that tells the run's changes from files already dirty before it
- Briefs name ranked candidate paths, never contents; a finished run is
  compared to the median of comparable runs once five exist; the result shows
  the route
- The project screen lists recent runs with their outcome, route, calls and
  labelled usage; each run opens its patch and append-only event log
- A preflight preview shows the selected route, models and existing working
  tree changes before a provider starts; provider install and sign-in can be
  cancelled and are bounded to five minutes
- Codex is refused up front on a repository whose folder ACL the user cannot
  change, instead of "finishing" with no tools (see §16)
- CI on `windows-latest`: `npm test`, `npm run build`, `cargo test`
- A route's tier picks a real model and effort on both CLIs, and moves up a tier
  when that tier has stalled in the project's own history (§4.3.4)
- Each plan's rolling limits are read from its CLI. Codex answers for free.
  Claude since 2.1.273 answers `/usage` only with a model call, so Orteca
  pays for one (haiku) only when the user presses Refresh; otherwise the
  reading is the `rate_limit_event` its own Claude calls (runs, classify,
  commit drafts) carry, kept 15 minutes; the
  provider with the most left is picked until the user picks one, and the
  preview warns when a route may not fit in what is left (§4.3.5)
- A run can work in a separate copy: a git worktree beside the repository on
  its own branch, committed when the run ends, removable afterwards (§11)

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
| Cost | `estimated` (Claude's own total; Codex from published API rates) |
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
native edit tools are denied — and not even Implement when Orteca runs the
tests after it (§4.3.7).

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
| codex | `-c project_doc_max_bytes=0` | yes | does **not** drop the global `AGENTS.md`; drops the repo's, which is the point: the repo's file reaches a run only as imported Orteca memory. Used. Checked 2026-09-21: a repo `AGENTS.md` token showed in a reply without it, not with it. |
| claude | env `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` | yes | drops the repo's `CLAUDE.md` (user's is already gone with `--setting-sources`). Used, for the same reason. Checked 2026-09-21 the same way. |

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
| `cheapest` | `sonnet` (Sonnet 5), `low` | 2 · 10 | `gpt-6-luna`, `low` | 0.10 · 0.50 |
| `standard` | `sonnet`, `high` | 2 · 10 | `gpt-6-sol`, `medium` | 2 · 10 |
| `deep` | `opus` (Opus 5.5), `high` | 4 · 20 | `gpt-6-sol`, `high` | 2 · 10 |

Considered and not chosen: Haiku 4.5 ($1 · $5, 200k, no effort control), Fable
5.1 ($10 · $50), GPT-6 Astra ($10 · $50, out 2026-09-03, Codex's default and the
model this account could not verify access to in §4.3.2), GPT-5.5 (retired for
ChatGPT sign-in on 2026-08-31). Rechecked 2026-09-22: Opus 5.5 replaced Opus 5
behind the `opus` alias at a lower price, so nothing in code moved. GPT-6 Sol
and Luna (openai.com/index/introducing-gpt-6-sol-and-luna, same day) replace
their 5.6 namesakes at half the price; there is no GPT-6 Terra, so `standard`
is Sol at `medium`, the price 5.6 Terra was. Astra still has no SWE-bench Pro
score, so `deep` stays Sol. The §4.3.6 measurements were taken on 5.6 models.

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
   price**, so the ordering holds either way. Codex reports tokens only. At
   startup Orteca fetches `https://models.dev/api.json` (its OpenAI rates
   matched LiteLLM's on 2026-09-14; OpenRouter had Sol at half) into
   `model_prices`, and prices each Codex turn at the rate of the model the
   stage asked for, labelled `estimated`: on a ChatGPT sign-in it is what the
   same tokens would cost on the API, not a bill. Offline keeps the last list.
   One turn on an unpriced model leaves the whole run's cost `unavailable`.
6. **Claude aliases follow the account**, and the id that ran comes back in
   `modelUsage`. Codex has no aliases, so its slugs are pinned in
   `routing::Tier::model`. A slug missing from `~/.codex/models_cache.json`
   gives way to that list's first `list` model at its default effort, and
   the stage records and prices that model; update the pin when it happens.
   The first run on a model a provider has not run before logs "New model
   detected" in its task log. Each `stage`
   event records the model and effort Orteca asked for, which is the only record
   of it for Codex.

**Adaptation: up on evidence, never down** — except the one step down a low
plan limit allows on a checked route, which evidence always outranks (§4.3.5).

- The same prompt already failed once (`prior_failures >= 1`): one tier up.
  After two failures the route itself escalates (§8).
- A route kind and tier with at least five finished runs (`done`,
  `budgetReached`, `reviewRejected`, `verifyFailed`) among the project's last
  50 on this provider and mode, two in five of them stalled: one tier up, again
  if that tier has stalled too, never past `deep`. `failed` is left out, because
  a rate limit or sign-in failure says nothing about the model. Modes are kept
  apart because Efficient's tighter ceilings stop runs Balanced would finish.
  A run that needed its Fix call counts as a stall of the tier it started on,
  however it ended. Only runs on the model the route kind and tier last ran
  on count, so a model that moves in behind an alias starts with a clean
  record.
- Orteca never tries a cheaper tier to see what happens: that is spending on its
  own initiative on a worse chance of success. Evidence ages out with the
  project's last 50 runs, and only then is a skipped tier tried again.
- The reason is in `route.tierReason`. The preview shows the model, effort and
  reason before anything starts.

Efficient mode has two measured stage overrides. Plan keeps the route's model
and uses `low`; Implement returns to the tier default, so the provider can keep
the same model's cache. A guarded Codex Review uses Terra `high`; its failed
contract still buys the declared Sol `high` Fix. Balanced keeps the table's
defaults. The measurements are in §4.3.6.

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
- Nothing is blocked: a warning is not a refusal. It offers "use codex
  instead" when the other CLI is known to have more room.
- **A low limit steps the tier down once, where the run still finishes**
  (`routing::route`). The frontend passes its headroom reading to
  `preview_task` and `start_task`, the same number for both. The tier drops one
  step when headroom is under 10% per call the route may make, Fix call
  included, and only on a `standard` or `planned` route: its Verify catches a
  miss, and the Fix call it buys runs on the tier the route would have used. It
  never drops a tier that evidence raised (a prior failure or a stall), never on
  `guarded` or `escalated` work, never onto a tier that stalled here, and never
  below `cheapest`. An unread limit is not a low one. The 10% is a guess too.
- **A run that spends the plan offers the other CLI.** Claude's stream sends
  `rate_limit_event` with `status: "rejected"` when a window is used up (not yet
  seen in a recording) and the parser reports it as `usageLimit`, a kind that
  outlives a vaguer result after it. `TaskResult.failureKind` carries it. When it
  is `usageLimit` and the other CLI is signed in and not known to be empty, the
  result offers "Continue with codex": a fresh run of the same request, started
  by the user, on a tree that keeps whatever the stopped run changed.
- A run that failed on `usageLimit`, `rateLimit` or `authExpired` is not a prior
  failure of its prompt, so continuing it does not send it a tier up (§8).

### 4.3.6 Orteca against the plain CLIs — 2026-09-13

Five tasks in a fresh no-dependency Node repo, one run each, four arms: Orteca
(`balanced`, through `prepare_run` + `run::stream`) and the plain CLI on the
user's own setup (`claude -p --permission-mode acceptEdits`, `codex exec
--sandbox workspace-write`, Codex on the user's `gpt-6-astra` at `low`). Graded
by acceptance checks the agents never saw. All 20 runs passed every check.

| Task | Orteca route | Orteca+Claude | Claude | Orteca+Codex | Codex |
|---|---|---:|---:|---:|---:|
| typo | implementOnce | 113k · $0.12 | 164k · $0.25 | 45k | 96k |
| bug + regression test | implementOnce | 237k · $0.10 | 618k · $0.22 | 71k | 149k |
| feature + tests | implementOnce | 240k · $0.11 | 511k · $0.20 | 77k | 123k |
| refactor across 3 files | standard | 283k · $0.35 | 353k · $0.20 | 264k | 152k |
| path-traversal fix | guarded (deep) | 249k · $1.06 | 339k · $0.17 | 256k | 177k |
| **uncached in + out, all five** | | 195k | 147k | 175k | 100k |

Totals are reported tokens, cache reads included; costs are Claude's estimates.
**One-call routes beat the plain CLI at the same quality. Multi-stage routes do
not:** each stage re-reads the repo, and `guarded` runs on Opus, so the same
passing fix cost 6x. One run per cell on a toy repo: a direction, not a rate.

Found by the benchmark and fixed:

- Claude's `bug` and `feature` runs ended `error_max_turns` at the 10-turn
  `implementOnce` ceiling with correct code, reading `budgetReached`. The plain
  CLI needed 10-12 `num_turns` for the same tasks. Per-route turn ceilings
  (10 / 10 / 8 / 8) also gave each stage of a big route fewer turns than a
  small route's one call. Every stage now gets 50 in both modes: a runaway
  guard, since Claude reports tokens only when a call ends and nothing else
  stops a looping call mid-run. The token ceilings do the budgeting.
- That stop reported `turnsUsed: 0` and `observed: 0`: `error_max_turns` is a
  failed result, and only `Done` counted turns. It now counts the ceiling the
  CLI stopped at. Claude's `num_turns` is not the `--max-turns` unit (11 at a
  ceiling of 10; a successful stage under 10 reported 13).

What the multi-stage rows cost: the Claude refactor's Verify was a $0.14 call
that re-read the diff to run `npm test`; the Codex refactor's Verify ran `npm
test` through PowerShell, hit the execution policy, reported a fail and bought a
Fix call; the security row was four Opus calls at ~$0.25 each for a one-function
fix. Orteca now runs a declared test command itself (§8), and small guarded work
skips Plan and runs on `standard` with only its Review on `deep`. Re-measured,
same tasks, one run each (new = uncached in + out):

| Task | Arm | Before | Now | Plain CLI |
|---|---|---|---|---|
| refactor | Claude | 2 calls · 65k new · $0.35 · 107s | 1 call · 39k · $0.24 · 64s | 26k · $0.20 · 77s |
| refactor | Codex | 3 calls · 55k new · 203s | 1 call · 17k · 104s | 17k · 170s |
| security | Claude | 4 calls · 81k new · $1.06 · 222s | 2 calls · 40k · $0.41 · 110s | 22k · $0.16 · 71s |
| security | Codex | 4 calls · 75k new · 234s | 3 calls · 78k · 255s | 33k · 94s |

All passed every check. Every local Verify ran and passed, the Codex refactor's
inside `codex sandbox`. The Codex security run is the exception: its `deep`
Review found a real regression in the implement stage (`//css/site.css`
refused, a normal path the acceptance checks did not cover) and bought the Fix,
which repaired it. That run cost more than before and bought a caught bug; no
plain-CLI result had the regression. Claude still costs more per guarded task
than the plain CLI: an Opus review of a finished diff is ~$0.25 of it.

Next, cheaper without a weaker review. When the repository declares a test
command, guarded work runs it *before* the Review: a failure buys the Fix and
the Review then reads the fix; passing work is reviewed with the brief saying
the tests pass, so the reviewer spends on what they miss. Efficient mode uses
the measured provider-specific choice in `routing::Route::review_model`;
Balanced stays on `deep` at `high`.

That effort was chosen by a review-only benchmark: Orteca's exact Review argv
and brief, on five seeded changes to a Node file server whose tests all pass,
two runs per arm (2026-09-14). Graded by reading every finding.

| Seeded change | Opus high | Opus medium | Sonnet high |
|---|---|---|---|
| `startsWith(ROOT)` with no separator (sibling bypass) | 2/2 high | 2/2 high | 2/2 (1 medium) |
| `..` checked before decoding (`..%2f` bypass) | 2/2 high | 2/2 high | 2/2 high |
| 403 with no `return`, delete still runs | 2/2 high | 2/2 high | 2/2 high |
| `//css/site.css` refused (the Codex-caught regression) | 2/2 low | 2/2 medium | **0/2** |
| correct fix (control) | 0/2 pass | 0/2 pass | 2/2 pass |
| **avg per review** | $0.150 · 53s | **$0.121 · 38s** | $0.156 · 104s |

Opus `medium` found everything `high` found, rated the regression higher, and
cost 19% less and ran 28% faster, so Efficient uses it. Sonnet missed the
regression both times and was not cheaper - it took more turns - so it is not a
reviewer. Both Opus efforts asked for changes on the correct fix because its
regression test cannot fail (the URL parser strips `..` first). That is true,
and it buys a Fix call; it does not depend on effort. Ten runs per arm on a toy
repo: a direction, not a rate.

The Codex stage matrix used five more short calls on the same no-dependency
repository, with user config, plugins, apps and skills excluded. It stopped at
five instead of the planned seven when the five-hour allowance moved from 60%
to 76% used (the weekly window moved 56% to 58%). Acceptance checks stayed
outside each run's working root.

| Stage | Model / effort | Result | Reported tokens (total · cached · output) |
|---|---|---|---:|
| Plan | Luna `low` | valid artifact; all refactor constraints, API compatibility, exact errors and checks covered | 21,595 · 8,960 · 947 |
| Plan | Luna `medium` | same quality; no rubric gain | 33,228 · 18,944 · 1,250 |
| Implement | Luna `low` | correct duration fix; 7/7 hidden checks | 53,894 · 38,912 · 701 |
| security Review | Terra `high` | found the prefix-sharing sibling escape, `high` severity | 24,441 · 11,008 · 903 |
| security Review | Sol `low` | found the same defect, `high` severity | 36,095 · 22,656 · 802 |

Luna `low` therefore replaces `medium` on `cheapest`, and Efficient Plan uses
`low`. Terra `high` becomes the Efficient Codex guarded reviewer: it matched
Sol `low` on the seeded defect with 32% fewer total reported tokens, finished
first, and has the lower published token price. This is one implementation case
and one review case, so project history still moves a stalled tier up; Balanced
remains the conservative choice.

Stage handoffs now carry only information the next process cannot recover from
the working tree: Plan to Implement, and the last failed Review or Verify to
Fix. Review gets the already-tested signal without the full Verify artifact;
Verify and Review inspect the task and tree directly. User instructions still
repeat in every stage.

### 4.3.7 Orteca does the looking and the checking — 2026-09-14

Every agent turn re-sends the CLI's own prompt and tool schemas, 21-23k on
Claude (§4.3.3), so a run costs roughly its turn count times that. The recorded
isolated Claude run went Glob → Read → Edit → reply: four turns for a file the
brief had already named. Fewer turns and a smaller per-turn context are the
levers. Skills are the opposite, more text on every turn, which is why they stay
off.

- **An `implementOnce` in a repository whose tests Orteca can run is Implement →
  Verify**, that Verify Orteca's own (§8). The call is told not to run tests or
  builds, and a failure buys the one Fix call. Before, the call ran a check
  inside itself. Every Implement with a Verify after it gets the same line.
- **An Implement that such a Verify follows gets no shell on Claude:** `--tools
  Read,Edit,Write,Glob,Grep`, the same five granted, the denylist kept as a
  backstop. Bash and PowerShell are the two biggest of the seven schemas. A
  prompt that names a command-shaped job (`routing::needs_shell`: install,
  dependency, bump, rename, delete, …) keeps the shell. Codex's only tool is its
  shell, so Codex is unchanged. Each `stage` event records `shell`.
- **Codex's Implement brief carries the first three candidate files** under 8 KB
  each and 16 KB in all, symlinks skipped, so it opens them with no shell call.
  Claude's does not: its Edit tool refuses a file it has not Read in the same
  session, so pasting would pay for those bytes twice. Every brief now says to
  open the named paths directly, with no search first.

**Not measured yet.** The next measurement repeats the §4.3.6 typo, bug and
feature tasks on both CLIs and compares turns and uncached tokens.

### 4.3.8 No ceilings: fix until it passes — 2026-09-14

Measured on a real task in RigInspectBE (Laravel, 664 tests): configurable
reminder lead days, which needs a migration and schema dump, API validation, a
cron rewrite and feature tests. One run each, Codex on both arms.

| | Plain `codex exec` (user config, Sol high) | Orteca `guarded`, before |
|---|---|---|
| Wall time | 6m 44s | 17m 44s |
| Tokens in (cached) · out | 916k (870k) · 7.0k | 1,476k (1,325k) · 23.3k |
| Estimated cost | $0.68 | $1.36 |
| `composer test` afterwards | 669 pass | 1 failing |
| Outcome | done | `reviewRejected`, 3 open findings |

Orteca's local Verify caught a schema dump the Terra implement broke, and its
Review found three real defects. Then the call ceiling ended the run with them
unfixed. Every stage was a fresh process: the Sol Fix re-read the repository
(620k cached input), and nothing checked the Fix again before the Review.

What changed:

- **No call, turn or token ceiling.** `ExecutionBudget` is tiers only. A Review
  or Verify that does not pass is followed by a Fix and the same check again (a
  failed Review also re-runs a Verify that comes before it), for as many rounds
  as it takes. The one stop is no progress: a check that still fails after a Fix
  that left `patch_since` unchanged ends `reviewRejected` or `verifyFailed`.
  The user's Stop ends any run.
- **No escalation.** A Fix runs on the route's own tier, and **a Codex Fix
  resumes the Implement session** (`codex exec resume <id>`, model and schema
  kept), so the task and the files it read are already in its cached context.
  A tier that keeps failing is still stepped over by `stalled_tiers` on the next
  run; a run that needed fixes and finished no longer counts as a stall.
- **A Review whose findings are all `low` passes**, and a second Review is
  handed the first one's findings to check first. Without both, a picky
  reviewer turns "until it passes" into a loop.
- **The Review brief carries the patch** (up to 48 KB), so the reviewer does
  not spend turns finding the change.
- **Codex usage is per process again.** `turn.completed` carries the thread's
  running totals, resumes included (the rollout's `total_token_usage`), so a
  resumed Fix re-billed the Implement. `run::added_since` subtracts the thread's
  previous report.

Re-measured, same task, fresh clone, one run:

| | Plain Codex | Orteca, no ceilings |
|---|---|---|
| Wall time | 6m 44s | 27m 29s |
| Estimated cost | $0.68 | $2.28 (Terra thread $0.93, three Sol reviews $1.35) |
| Stages | 1 call | Implement → Verify → 3× (Fix → Verify → Review), 7 calls |
| `composer test` afterwards | 669 pass | 670 pass |
| Outcome | done | done; the last Review passed with one `low` finding |

It now finishes, and every medium finding the Reviews raised was fixed
(`integer` accepting `"14"` against a strict compare, `whereDate` defeating the
`next_checkup` index, `array` accepting an object). It is not cheaper or faster
than the plain CLI on this task. Where the time went: the first Verify ran 6
minutes because the broken draft made ParaTest error slowly until Composer's
300-second timeout; every Verify also ran `npm ci` and the Vite suite for a
PHP-only change; three Sol reviews took 10 minutes. Plain Codex on Sol wrote
it right in one warm session that tested itself, which a Terra draft plus
separate reviews cannot undercut.

### 4.3.9 One strong schema session and change-aware Verify — 2026-09-15

The next route removes the costs the comparison exposed. Schema-only work uses
one Codex implementation session on Sol `medium`, with the implementation brief
requiring a final boundary, migration/schema and index-use review. When the
repository declares tests, Orteca runs Verify afterwards; otherwise that same
session runs one focused check. Astra is deliberately excluded because its
allowance draw is too high for this account.

Local Verify now selects ecosystems from files changed by this run. A PHP-only
change no longer installs or tests an adjacent Vite application. Changed
Laravel tests run first as a cheap tripwire; the broad PHP suite runs only after
they pass. Security and authorisation work still receives one independent
Review. A rejected Review buys one Fix and Verify, never another Review. A
failed Verify likewise buys at most one automatic Fix; another failure stops
with its output instead of spending through the user's allowance. A test that
cannot start a required executable is an environment failure and buys no Fix.

The first live run of this route is not a valid comparison result. It used a
clean RigInspectBE copy at `0aea720d`, but that copy did not include the
benchmark checkout's untracked MySQL test environment. The initial Sol
implementation took about 11 minutes; Verify then reached 13 test failures
with zero assertions because `mysql.exe` was unavailable. Orteca treated that
environment failure as a code defect and began Fix work. The run was stopped
around 18 minutes, when a second Fix started, to preserve the remaining account
allowance. It produced no result JSON, so no cost or correctness result is
claimed. It also did not beat plain Codex's 6m 44s time. This invalid run is
what prompted the hard one-Fix ceiling and missing-executable stop above.

### 4.3.10 Check before the change, and resume Claude's Fix — 2026-09-16

The invalid run in §4.3.9 failed because the checks could not pass on that
machine at all, and a failing Verify cannot say whether the change or the
setup is at fault. Orteca now asks before anything changes:

- **A quick check runs once before the first stage** of any route with a
  Verify Orteca can run itself: for a Laravel suite, the smallest test file
  under `tests/Feature` (`php artisan test <file>`), which boots the app and
  its database. The suites are picked from the route's candidate paths, the
  same way Verify picks them from the changed paths; ecosystems with no cheap
  slice skip the check. A pass is remembered for this launch by folder,
  commit, dirty-file fingerprints and suites.
- **A suite that already fails is shown to the Implement stage** (it may be the
  task), and **when the same suite fails after the change, no Fix is started**:
  the run ends `verifyFailed` with the output kept and a message saying the
  checks failed before the run. A different suite failing still buys its Fix.
- **Claude's Fix resumes the Implement session** (`claude -p ... --resume
  <id>`), as Codex's already did. The task and every file it read are in its
  context, and Claude's Edit tool refuses a file it has not Read in the same
  session, so a fresh Fix paid to read the files again.

Installs were already once per tree: a suite's install runs only while
`node_modules` or `vendor` is missing, so the §4.3.8 `npm ci` on every Verify
is gone since change-aware Verify.

Measured the same day on the RigInspectBE `easy` task (one run each, after
the benchmark stopped junctioning `vendor/`, which broke ParaTest): both
providers finished `done` in one call, 3/3 hidden checks, Claude $0.31 in
4.2 min, Codex (Luna) $0.017 in 3.9 min. The full suite ran before and after
(~60-75 s each), so the first design cost a whole suite run per task.

Why a quick check, not the full suite, and not in parallel:

- The full suite before the change is ~60 s on RigInspectBE and caught one
  flaky parallel failure (`PersonalDataExportTest`, a temp-file clash) on an
  unchanged tree. One flake marked the suite as already failing, which would
  have denied a real failure its Fix. One file, run alone, does not race.
- Running it beside Implement is unsafe in the default mode: the agent edits
  the same tree the suite is reading, and every run of a Laravel suite shares
  one MySQL test database.
- The suite already runs on every core (`--parallel --processes=8` on 8
  logical cores); `composer` itself adds ~1 s.

**Not measured yet.** Whether the resumed Claude Fix is cheaper once its
prompt cache has expired during a long Verify.

### 4.3.11 Task types, rulesets and the task log — 2026-09-24

Checked against claude 2.1.280 and codex-cli 0.155.1. `--system-prompt-file`,
`--append-system-prompt-file` and `--max-turns` are hidden from `claude --help`
but present; `developer_instructions`, `service_tier` and `rate_limits` are in
the Codex binary; `codex exec resume` takes `--output-schema`.

- **Task type beside difficulty.** `intent::TaskType` (chat, question,
  code_change, debug, plan) picks the ruleset and tools; `Intent` still picks
  route and tier. A plan runs as a read-only Answer. `/chat`, `/ask`, `/code`,
  `/debug`, `/plan` pick it and skip the classifier.
- **Classifier is JSON** (`--json-schema` / `--output-schema`): type,
  difficulty, confidence, clarify, title, job, run, reply. Talk is answered in
  that call and no agent starts. Unsure talk (< 0.6) is a question. A follow-up
  is told the type it continues. Live, 2026-09-24: haiku ~2.2k in / 400-1500
  out, $0.004-0.010, 8-22 s; Luna ~6.2k in / ~50 out, ~10 s. The instruction
  says it runs outside the repository: told nothing, haiku saw its empty temp
  folder and asked which project the request was about.
- **Clarify.** A code_change, debug or plan the classifier cannot read asks one
  question (`ErrorKind::Clarify`) before any task exists; the answer, or a
  skip that has the agent state its reading, rides on the prompt. The reading
  is kept, so the second try is not classified again. Setting `clarify=off`.
- **Rulesets** are `src-tauri/rulesets/*.md`, base + type, with the repository
  profile under them: Claude `--append-system-prompt-file` (chat:
  `--system-prompt-file` and `--tools ""`), Codex `-c developer_instructions`
  (never `model_instructions_file`, which the classifier alone keeps). Never
  sent on a Codex resume; Claude's system-prompt snapshot ignores it on one.
- **Repository profile** (`project::profile`, editable in Memory, stored in
  `projects.profile`): commands, top folders, what not to read. Junk folders
  also become Claude Read deny rules through `--settings`, a hint Grep and
  Glob follow and a shell does not. No startup self-test: it would spend a call.
- **Stripping still drops the user's own settings**: user deny rules, env and
  model defaults (`--setting-sources project,local`), and all of Codex's
  `config.toml`, profiles and sandbox defaults included (`--ignore-user-config`).
- **Billing.** `ANTHROPIC_API_KEY` in the environment reads as `apiKey`.
  `system/api_retry` with `billing_error` or an auth error fails the run at
  once, `rate_limit` over a minute too. A spent plan marks the provider
  `limited` until its window resets (`limits::block`); the next task's pick
  skips it. `--fallback-model` is set per alias (`routing::claude_fallback`).
- **Loops.** The same failing command three times with no edit between, or the
  same edit three times, stops the stage as `budgetReached` (limit `loop`),
  which counts toward `stalled_tiers`. No turn ceiling came back (§4.3.8).
- **Gates.** Code changes and fixes that touch JS also run the repository's
  lint, type-check and build scripts (on a PHP-only change `npm run build`
  cost 26-75 s a Verify and could fail nothing); one counts only when its output names a file
  the run changed, so a failure that was already there buys no Fix. A Fix is
  handed only the lines that say what failed. Existing tests changed without
  being asked for are flagged in the summary.
- **Files.** Git co-changes rank siblings of the top matches, off under 150
  commits: `navstats --recall` LiftMe (119 commits) 73% unchanged, RigInspect
  61% -> 68%; on for LiftMe they cost 5 points. Files earlier tasks read or
  changed, when their prompt shares this one's rarer words, rank too
  (`task_files`); the offline recall has no history, so that part is unmeasured.
- **Second opinion.** A Codex code change over 100 lines is reviewed by Claude
  (opus medium, fresh session) when Claude is signed in to its plan. Advisory:
  findings go in the summary, a `crossReview` event logs lines, findings and
  cost. Claude's work is never sent to Codex.
- **Checkpoints.** Every run in the current tree saves `refs/orteca/before/`,
  a clean one too, so a moved branch cannot take the undo point with it.
- **Window anchoring** (`views/project/anchor.ts`, Settings page, off by
  default): one tiny cheapest-model call before work (start − (5 − N) h) or
  right after each reset inside work hours; skipped while a task runs or ran in
  the last 30 minutes, or more than 10 minutes late. `pings` logs each cost.
- **Task log** (migration 0013): `task_type`, `ruleset` (`name@hash`), `turns`,
  `tools_before_edit`, `gate`, `verdict` (commit on the run's base or a merge
  = accepted, a rewind with files = rejected, the same prompt again = retried),
  plus a `classified` event (type, confidence, source, clarify answered).

Measured the same day, RigInspectBE `easy`, one run per arm, 0280b56 against
this: Claude 3/3 both, $0.41 / 159 s -> $0.25 / 143 s; Codex 3/3 both,
$0.17 / 336 s -> $0.01 / 268 s. Most of Codex's drop is the route: the JSON
classifier read the task as easy, so it ran without the Sol Review the old
run bought. One sample each; not yet a baseline.

### Parked: replace the CLI's own system prompt

Most of each turn's ~21k is the CLI's built-in system prompt, not Orteca's
brief. Both CLIs can replace it, which would shrink every turn of every stage:

- `claude --system-prompt-file <file>` replaces the default prompt entirely
  (`--append-system-prompt` only adds to it, and
  `--exclude-dynamic-system-prompt-sections` is ignored alongside it). Whether
  tool schemas and the repo's `CLAUDE.md` still load needs checking in the
  `init` event. `--system-prompt-snapshot` is on by default, so a resume sends
  the prompt recorded at the start.
- `codex exec -c model_instructions_file=<file>` replaces Codex's built-in base
  instructions rather than adding to them.

Parked because the default prompts carry the tool-use guidance these models are
tuned on, and only real runs show what a short prompt costs in quality. To pick
it up: write one short prompt per CLI (use the tools to edit, stay in scope,
never rewrite git history, stop when done), check both flags the §4.2 way, keep
the file in Orteca's temp directory and never in the user's repo, then run the
§4.3.6 tasks against the §4.3.7 rows. Keep it only if every acceptance check
still passes.

Sources: code.claude.com/docs/en/cli-reference,
developers.openai.com/codex/config-reference.

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
  migrations/0001_init.sql … 0006_model_prices.sql
  src/
    main.rs        Tauri commands + app setup
    error.rs       AppError { kind, message }, serialized to the frontend
    proc/          spawn, Job Object, cancel, JSONL reader
      mod.rs
      job.rs
    project.rs     git state, trust scan, path validation, diff, worktrees
    store.rs       SQLite, migrations, projects
    providers/     detect + event normalisation + mock fixture replay
      mod.rs       ProviderId, Detected, ProviderEvent, `which`
      claude.rs    stream-json parser
      codex.rs     exec --json parser
      limits.rs    plan limits read from each CLI (§4.3.5)
      mock.rs      replays fixtures/*.jsonl through the real parsers
    run.rs         route runner: stages, argv, stream, fix rounds, event log, diff, result
    intent.rs      one small-model call that reads what the prompt wants
    routing.rs     deterministic classifier + route builder + stage briefs
    dock.rs        ConPTY terminals, directory listing, one text file in and out
    orchestrator.rs *  folded into run.rs — see below
src/
  main.ts  App.vue  api.ts  types.ts
  views/      Launch.vue  Project.vue
  views/project/  the workspace pages, state.ts, and the dock (dock.ts + Dock*.vue)
  components/ VeloMark.vue  TrustPrompt.vue
  styles/     tokens.css
scripts/make-icon.mjs  make-sandbox.mjs  app.test.mjs  ui.test.mjs  dock.test.mjs
```

`git.rs` never happened: baseline snapshot and diff capture are four functions
next to `git_state`, and `project.rs` already owns every git call.

### 5.1 The dock

`dock.rs` is the workspace's other half: terminals, the file tree, a file being
edited and the dev server's own page, beside or below whatever screen the
sidebar picked. It is deliberately outside the run path — nothing here routes,
classifies or spends tokens.

- Each terminal is a real ConPTY through `portable-pty`, spawned inside the
  same `proc::job::Job` the agent runner uses. A shell spawns `npm -> node ->
  vite`; closing a tab has to take that whole tree, and `Child::kill()` does
  not. `pump` reads the master and emits `pty:<id>`, holding back the trailing
  bytes of a character a read stopped inside.
- Every command goes through `inside()`: `trusted_dir` first, then
  `canonicalize` and a `starts_with` on the project root, so `..` and a symlink
  are both refused. The dock reaches nothing outside a consented repository.
- The shell is the user's preference, parsed with `shell-words` so a quoted
  path keeps its spaces. Blank means `pwsh` if `providers::which` finds it,
  else `powershell.exe`.
- `read_text` refuses anything over 4 MB or anything that is not UTF-8;
  `write_text` refuses a path that is not already a file. The code tab edits,
  it does not create, and it never renders a binary as noise.
- The preview is an `<iframe>`, which is why the CSP names `frame-src` for
  loopback only. Orteca starts no dev server: a terminal does, and the address
  it printed is picked out of the stream by `noteOutput` in `dock.ts`.
- The history tab is `project::git_log`, `project::commit_patch` and
  `project::working_patch`, which is `patch_since` against HEAD (or the empty
  tree, before the first commit) so the uncommitted work reads as one more
  entry — git logic
  belongs beside every other git call, and `dock.rs` only holds the commands.
  The log format uses `%x1f`/`%x1e` separators so a subject carrying a tab or a
  newline still parses, and `commit_patch` refuses any name that is not
  hexadecimal so nothing from the frontend can arrive at git as an option.
- Tabs and settings live in `localStorage`, not SQLite. They are window state,
  the store is for what a run measured, and a blocked store costs a preference
  rather than a session.

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

The first fixtures in `src-tauri/fixtures/*.jsonl` were written to the event
shapes verified above before either CLI was installed. Both are installed now
(§16), and a real run's JSONL is kept under the app data `recordings` folder;
swap a capture in for a hand-written fixture and nothing else changes.

Detection resolves the program against PATH x PATHEXT itself. `CreateProcess`
only ever appends `.exe`, so `claude.cmd` — how both CLIs install on Windows —
is invisible to a bare program name. The resolved path is what `run.rs` spawns.

## 7. SQLite schema — 9 tables

Migrations 0001 to 0010 are shipped; `file_cache` shipped as the code map (§12).

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

-- 0004, shipped
tasks.patch_text, tasks.unknown_events, tasks.duration_ms

-- 0005, shipped
tasks.worktree_path  -- the separate copy a run worked in; NULL once removed.
                     -- tasks.branch then holds the copy's branch.

-- 0006, shipped
model_prices(model, input, output, cache_read, fetched_at)
  -- USD per million tokens from models.dev, replaced whole on each fetch;
  -- an empty or failed fetch keeps the last list (§4.3.4).

-- 0007 to 0009, shipped
tasks.title                          -- 0007, the sidebar's label for a run
check_passes(key, passed_at)         -- 0008, dropped again by 0009: a failed
  -- Verify asks the base commit instead, so there is no pass to remember.

-- 0010, shipped: the code map that replaced the deferred file_cache (§12)
map_files(id, project_id, path, mtime, size, lang)   -- UNIQUE (project_id, path)
map_symbols(file_id, name, kind, line)
map_uses(file_id, name)
  -- No hash: mtime and size decide what to reparse, and a use is a bare name
  -- resolved through map_symbols when read, so a changed file rewrites only
  -- its own rows.

-- not a table: baselines are a query over tasks + usage (see §12), because a
-- stored median is a second copy of rows that already exist.
```

Deferred: `project_symbols`, `provider_status` (live-detected, not stored),
separate `commands` / `artifacts` tables (they are `task_events` rows).

Migrations: plain numbered `.sql` files run in order. No ORM.

## 8. Routing model — one cheap semantic classification

Each run first asks the provider's smallest model (`haiku`, `gpt-5.6-luna`)
what the prompt wants, then pure Rust combines that reading with cheap repo
signals. See the as-built notes below.

```
signals: complexity 0-10, risk 0-10,
         architecture, security, authz, schema_change, bug, refactor, frontend
         + blast_radius (candidate file count from ripgrep)
```

| Condition | Route |
|---|---|
| complexity <= 3, risk <= 3, blast <= 5, and blast >= 1 or a small-edit word | **Implement once** — inspect named paths, edit, run one focused verification inside that call; no Plan or Review |
| architecture OR complexity >= 7 | Understand → **Plan** → Implement → Verify |
| security OR authz | Implement → **Review** → Verify; **Plan** first when also architectural or complexity >= 7 |
| schema_change | one strong Implement/self-review → Verify when local checks exist |
| implement failed twice | escalate: Plan → Implement → Review |

`Efficient` shifts thresholds up by 2 (fewer stages) and chooses the cheapest
capable configured tier. `Balanced` uses the route as written. A route declares
its stages and tiers before any provider is started; tiny tasks are one call.
Verify gets at most one automatic Fix, and an independent Review runs at most
once (§4.3.9).

Capability → provider map lives in one table, not in the router:

```
DEEP/REVIEW → claude    IMPLEMENT → codex    fallback: whichever is detected
```

Every decision writes a `routing_decision` event with the signal values, so future
adaptive routing has training data without a schema change.

**As built (Milestone 6).** `routing.rs` is pure and makes no model call itself:
the semantic reading is an input beside one `git ls-files` for blast radius and
one `COUNT(*)` for prior failures. Same inputs, same route, every time.

**Semantic job reading (2026-09-16).** Keywords misrouted questions ("should I
email a user who cancelled?") and harmless names such as `auth.rememberMe`.
`start_task` now asks the selected provider's smallest model, with no tools and
outside the project, for difficulty, title, actual job boundaries (security,
authentication/authorisation, schema or general), and explicitly requested
build/test/lint actions. These arrive in `RepoSignals`, so `route` stays pure.
The semantic job reading controls the guarded gates; keyword gates are only the
fallback when the call fails or times out after 45 seconds. Twice-failed work
still escalates independently. Classification usage counts toward the run as
one call, logged as stage `classify`. The preview does not ask: it refreshes on
every pause in typing.

Rule order is not the table's order. The two escalating rules are tested first,
because a prompt that scores trivially but touches authorisation is not a
trivial task, and one that has already failed twice is not a candidate for the
route that just failed it. The table also had no default row; there is now a
`Standard` route — Implement → Review → Verify — for everything in the middle.

| Route | Stages | Calls |
|---|---|---|
| `ImplementOnce` | Implement, then Verify when Orteca can run it | 1 |
| `Standard` | Implement → Review → Verify, checks Orteca runs before the Review, the Review on `deep` (2026-09-18: it was the Review that beat the plain CLI on LiftMe, and a Standard run without one lost); schema work is one Implement, then Verify when Orteca can run it | 1 to 3 |
| `Planned` | Plan → Implement → Verify | 3 |
| `Escalated` | Plan → Implement → Review | 3 |
| `Guarded` | Implement → Review → Verify, Plan first when large | 3 or 4 |

Calls are the number of stages, plus a Fix and Verify when a check fails.
Review is never repeated. **Orteca runs a Verify itself when
the repository declares its test suites** (`project::check_commands`, at the
root and one folder down: a real `scripts.test` in `package.json` run with the
lockfile's package manager, `composer.json`'s test script or `phpunit.xml`,
`Cargo.toml`, `go.mod`, pytest config). After an edit, only ecosystems positively
identified by this run's changed paths execute; unknown-only changes keep the
full set. Changed Laravel tests run first, and a failure skips the broad suite.
On most repositories Verify is therefore no agent call.
A root suite claims its ecosystem, so a workspace's packages are not run twice.
A trusted repository's explicitly requested `build` and `lint` package scripts
also run here, directly as the user through Orteca rather than inside the model
sandbox. The classifier selects only the action; the command must exist in
`package.json`, so a model cannot invent an executable or command line.
A suite whose dependencies are not installed - a fresh clone, or every run's
separate copy - gets its install (`npm ci`, `pnpm install --frozen-lockfile`,
`composer install`, ...) first; a suite whose tools are not on PATH is reported
as not run, never passed. A pass or a fail comes from the exit code, and the
last 60 lines of output become the Verify artifact, so a failure is fixed with
the output in the Fix brief. **Suites run as the user on both
providers**, as the project's trust consent covers, not in `codex sandbox`: the
sandbox account cannot `lstat` the folders above a repository under the user
profile, Vite, Jest and npm realpath through them, so every such suite failed
there and bought a Fix no edit could win; an install also needs the network the
sandbox denies. With no declared suite, or none that can start, the agent
verifies as before. Guarded work runs on
`standard` with its Review on `deep` (`budget.reviewTier`), shown in the preview.

Rules the spec did not write down:

- **A Review that asks for changes on a high or medium finding is followed by
  one Fix and Verify; it is not repeated. A Verify that does not report a pass
  backed by at least one check, none failing, gets at most one Fix and Verify**
  (§4.3.9). A stage that ends cleanly while printing failures is not `done`.
  The run ends `reviewRejected` or `verifyFailed` when a Fix changed nothing;
  the findings are kept.
- **A held instruction is delivered by the next stage's brief, not by resuming
  the stage it arrived in** — unless that stage is the last one, where M5's
  behaviour is unchanged. Resuming as well would pay for a second process to say
  the same thing twice.

"Failed twice" is matched on the exact prompt text in the same project, and
counts a `budgetReached` run as not having finished. A run that failed on a
spent plan, a rate limit or an expired sign-in does not count: that says
nothing about the prompt. Exact text is the only
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
   both the same way in either direction reports a wrong number. A session is
   one process, though: every stage and every resume starts a new one that
   counts from zero, so `run::Outcome` banks each process's last cost before the
   next starts and adds them up when the run ends. Until 2026-09-13 it did not,
   and a three-call run reported one call's cost as the whole run's.
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

Orteca adds one layer on top: Claude's `--disallowedTools` denylist
(`run::CLAUDE_DENY_COMMANDS`: git push/reset/clean/rebase/restore, rm/del,
publishing, curl/wget/ssh, registry and scheduled-task edits). It matches
command spelling, so it is a guardrail against an agent going wrong, not a
sandbox against a hostile one. A denied command is refused inside the CLI
(`--permission-prompts none`) and the agent carries on; nothing pauses or asks.

**As built - what runs outside the provider's sandbox.** Three things run as
the user, with network, whatever the provider: the repository's own test
suites in a local Verify (the agent may have edited them), a command the agent
hands over with `ORTECA-WAIT:` (asked first unless the user picked "Run them"
for that project; `run::refusal` blocks shells, inline code such as `node -e`,
and the denylist), and the repo's Git hooks on a git-bar commit. Trust consent
covers all three, and is asked again when the agent configuration it covered
changes (`project::trust_fingerprint`).

Process: one Job Object per task. Cancel = close handle = whole tree dies. Then record
`cancelled`, keep the diff, show it.

## 11. Git safety

Before any stage that writes:

```
record branch, HEAD sha, `git status --porcelain`, `git stash list`
```

Isolation choice at task start: `Current tree` (default) | `New branch` | `Worktree`.
Orteca never runs a destructive git command. Diff captured with
`git diff <base_commit>` plus `git status` for untracked files. The project
screen's git bar runs fetch, `pull --ff-only`, `add -A` + commit, a plain
push (`push -u <origin or first remote> HEAD` when nothing is tracked yet) and
`merge --no-edit <local branch>` on a clean tree only, aborted on any clash so
it lands whole or not at all. Fetch runs on request; the other actions require
submission of an inline confirmation that says what they will do. All require a
trusted project (a commit runs the repo's hooks). A separate trusted `git_status`
command refreshes local state without a task preview or a network fetch. The header shows the
upstream and ahead/behind from `rev-list --left-right --count HEAD...@{u}`.

**As built.** A tree that is dirty at start is snapshotted before the first
provider starts: every already-changed path, with a hash of its contents. After
the run each diff entry is labelled `run` (clean before), `beforeRun` (changed
before, byte-identical after — the user's, not counted as the run's work) or
`both` (changed before and again; line counts are against the commit, so they
include both). If git cannot produce the snapshot the origin is `null` and the
screen falls back to saying it cannot tell. A file the run restored to the
commit drops out of the diff and is not reported.

**Separate copy — built 2026-09-13.** Two choices, not three: `This folder`
(default) and `Separate copy`. `New branch` in the user's own folder was
dropped: switching branches under uncommitted edits is the risky part, and a
worktree gives a branch without it.

- A copy is `git worktree add -b orteca/task-<id>
  <parent>/.orteca-worktrees/<repo>-<id> HEAD`. It sits **beside the
  repository, not in app data**, so the folders above it are the ones the
  trust scan already read, plus one Orteca made.
- It starts from HEAD. The user's uncommitted changes and ignored files
  (`node_modules`) are not in it, and the preview says so. The diff needs no
  snapshot: everything in it is the run's.
- When the run ends, whatever the outcome, its changes are committed to that
  branch as `Orteca`, with `core.hooksPath` pointed at a folder that does not
  exist and signing off. `--no-verify` alone still runs post-checkout and
  post-commit hooks the user never consented to. The user merges the branch.
- **Remove copy** runs `git worktree remove` without `--force` and keeps the
  branch: deleting a branch is the user's call. A copy deleted by hand is
  cleared with `git worktree prune`. A running task's copy is never offered.
- Refused before any CLI starts: a repository with no commits, one at the top
  of a drive, and Codex on a copy whose ACL cannot be changed.
- "Continue with the other CLI" is not offered after a copy's run: a fresh copy
  would start from HEAD without the stopped run's work.

## 12. Project intelligence

`file_cache` stores path + sha256 + language. Rescan on open: walk (respecting
`.gitignore`), hash only files whose mtime or size changed. Unchanged → reuse.

Ranking for a task brief: ripgrep the prompt's nouns → score by path match, name
match, `git log -n 50` recency, test-file adjacency. Top ~10 paths go in the brief.

**As built (Milestone 7).** Ranking is pure (`routing::candidates`) over
`git ls-files` and `git log -n 50 --name-only`: a file named for a prompt word
scores 3, a path containing one scores 1, a path in recent history gets +2, and
a test named like a matched file (`test_slug.py` beside `slug.rs`) is listed
with it without counting towards blast radius. Ties go shallowest and shortest
first. The top 10 go in the brief, ranked.

**The code map (2026-09-19).** The deferred `file_cache` shipped as
`codemap.rs` plus the `map_files` / `map_symbols` / `map_uses` tables, because
path matching alone held only a quarter of the files a run went on to edit:
"Let a customer edit a sent quote" never names a controller or the routes file.
`codemap::parse` reads each tracked source file with tree-sitter (PHP,
TypeScript, JavaScript, Rust; a `.vue` file's `<script>` block as TypeScript)
into `FileFacts { defines, uses }`, plus the Laravel strings that point at a
file (`'Controller@method'`, `view()`, `__()`). `store.rs` rescans on
`open_project` and before `start_task`, reparsing only files whose mtime or
size moved. Names resolve to files at query time, so a changed file rewrites
only its own rows.

`routing::candidates` then scores a prompt word that matches a symbol name
(camelCase and snake_case split) like a file-name match, and gives one hop from
the top seeds in either direction a small score — `routes/api.php` uses
`QuoteController`, so a controller seed pulls the routes file in. The brief
adds one capped line per top-five path saying what it defines and uses; it
still never pastes file contents. Recall of the files a run edited that existed
at its base commit went **29% → 73%** offline, and on the LiftMe bench the
navigation share of cost went 27% → 18% with grades held; on the one task where
route and model matched both sides, cost fell 20% and wall time 32%. Method and
caveats in `docs/code-map-plan.md`. `NAV_NOMAP=1` withholds the map from a run,
which is how the before arm is measured.

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
| 7 | file_cache, path ranking, usage + baselines, route visual | brief names ranked paths without file contents; cumulative provider usage enforces the inter-turn token guard; comparable-task baselines make savings estimates honest | all built; file_cache shipped as the code map (§12) |
| 8 | MSI/NSIS installer, signing, first-run | installable Windows app | NSIS installer, single instance and missing-git first-run message built; signing waits on a certificate |

Spec's 17 collapsed: detection folds into one slice, metrics into one, route visual
rides along with metrics. Each slice ends commit-ready with tests.

## 15. Decisions taken

1. **Two modes**, `Efficient` and `Balanced`. No `Maximum` in MVP.
2. **No savings percentage until a baseline exists.** Once a project has >= 5
   comparable tasks, show a rolling-median comparison labelled `estimated`.
   Before that, the result screen shows absolute tokens and calls avoided.
3. **Efficiency is a control, not a slogan.** Do not claim a task was efficient
   merely because most input was cached. The run must have a small-task
   single-call path and honest outcomes before Orteca can make that claim.
4. **`budgetReached` is a fifth task status, and `reviewRejected` and
   `verifyFailed` are explicit stop outcomes beside it.** Neither is a failure when the provider completed
   normally; neither is a success because the route did not finish. The work,
   diff, findings, and reported usage are kept exactly as they are, and the
   remaining stages are named so the user knows what they are being asked to
   decide about. Since §4.3.8 a stop means a Fix changed nothing, and
   `budgetReached` survives only on older rows.
5. **Bounded recovery (2026-09-15).** Verify gets one automatic Fix. Review runs
   once, and its one Fix is judged by deterministic Verify. Another attempt is
   the user's decision (§4.3.9).

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
- Codex has no turn ceiling at 0.154.0 and no way to add one. Orteca no longer
  sets one on either CLI (§4.3.8); the user's Stop is what ends a runaway call.
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
