# Review brief: Orteca milestone 4

You are reviewing one slice of an early-stage Windows desktop app: the first
real run. A prompt goes to one provider CLI, its JSONL is normalised and
streamed to the UI, every event is logged, and the run ends with a diff and a
labelled cost. Be adversarial about correctness. Do not expand scope.

## Objective

Verify that milestone 4 does what it claims, fix what is broken, and leave the
repository green and commit-ready.

## Constraints

- Windows only. Do not add cross-platform code paths.
- Do not add dependencies. No `async_trait`, no state library, no ORM.
- Do not implement milestone 5 or later. No steering, no cancel button, no
  routing, no multi-stage orchestrator, no baseline or savings percentage.
- Do not invoke the `claude` or `codex` CLIs from any test, and do not run
  `npm install --global` from a test. Neither CLI is installed on the dev
  machine and a real run would spend the user's subscription.
- Keep the diff minimal. A smaller correct change beats a larger tidy one.
- Match the surrounding style: comments explain *why*, never *what*.

## What is in the slice

Everything uncommitted in the working tree. The run path is the substance:

- `src-tauri/src/run.rs` (new) — `args()` builds the argv per provider,
  `stream()` spawns, drains, records and emits, `Outcome` folds the event
  stream into one answer, `clean_prompt()` rejects a blank prompt.
- `src-tauri/src/main.rs` — `start_task` and `install_provider` commands.
- `src-tauri/migrations/0002_tasks.sql` (new) — `tasks`, `task_events`, `usage`.
- `src-tauri/src/store.rs` — `create_task`, `append_event`, `record_usage`,
  `finish_task`, and `read_project` now returning `NotFound` instead of a raw
  rusqlite error.
- `src-tauri/src/project.rs` — `FileStat` and `diff_since`.
- `src-tauri/src/providers/mod.rs` — `ProviderEvent` is now `Serialize`
  (adjacently tagged), plus `kind()` and `package()`.
- `src-tauri/src/error.rs` — `CliMissing`, `NotTrusted`, `Invalid`.
- `src/api.ts`, `src/types.ts`, `src/views/Project.vue` — start a run, listen to
  `task-event` / `task-done` / `install-event`, render the stream, the tiles and
  the diff.

A visual redesign rode along in the same working tree (`src/styles/tokens.css`,
`src/views/Launch.vue`, `src/components/`, `scripts/make-icon.mjs`, the icons,
`docs/ui.md`). It is in scope only for correctness — a rule in `docs/ui.md` the
code breaks, a token that does not exist, an icon the build cannot find. No
taste opinions on it.

## Known weaknesses, stated up front

**The fixtures are still not recordings.** `src-tauri/fixtures/*.jsonl` were
written by hand to the shapes documented in `docs/architecture.md`. Milestone 4
now *spawns a CLI with those flags*, so a wrong flag is no longer a paper cut —
it is a run that does nothing, or worse, one that runs without the denylist.
Treat the argv in `run::args` and the fixtures as the primary suspects.

**The four new tests in `scripts/app.test.mjs` grep source files for regexes.**
They assert that a string appears in `main.rs` or `Project.vue`. They prove
nothing about behaviour and they will pass a refactor that breaks the feature.
Say plainly which of them are worth keeping and which are theatre.

## Specific things to attack

1. **The argv is the whole security boundary.** Confirm from each CLI's own
   documentation that `claude -p <prompt> --output-format stream-json --verbose
   --permission-mode acceptEdits --disallowedTools <list>` and
   `codex exec <prompt> --json --sandbox workspace-write` are real, current, and
   accepted in that order with the prompt positional. Attack
   `--disallowedTools` hardest: the value is one comma-separated string,
   `Bash(git push:*),Bash(git reset:*),Bash(git clean:*)`. If the flag wants a
   space-separated list, or a different name, or a different pattern syntax,
   the denylist silently matches nothing and the agent can rewrite the user's
   history. A silently-ignored denylist is the worst defect available here.
2. **The prompt reaches a `.cmd` shim.** `run::stream` spawns the resolved
   `claude.cmd` / `codex.cmd` with a user-written prompt as an argument.
   `rust-version` is floored at 1.77.2 for CVE-2024-24576, which makes Rust
   *refuse* arguments it cannot safely escape rather than mis-escape them.
   Establish what a prompt containing `"`, `%`, `&`, `^`, a newline or a lone
   backslash actually does: spawn error, mangled prompt, or something reaching
   `cmd.exe`. Whatever it is, the user must see it. Today a spawn failure
   becomes the string `could not start claude: ...` and nothing else.
3. **The task-id race.** `start_task` spawns `run::stream` and *then* returns
   the id; the UI sets `taskId.value` only when the invoke promise resolves.
   Every `task-event` and `task-done` handler drops payloads whose id does not
   match. Determine whether a fast provider — or a spawn failure, which emits
   `task-done` almost immediately — can finish before that promise resolves. If
   it can, the screen sits on "Running…" forever with an empty stream and no
   error. Fix it in the one place that cannot race, not with a timeout.
4. **A non-zero exit after a `result`.** `run::stream` only synthesises a
   failure `if outcome.failure.is_none() && !outcome.done`. A provider that
   emits a final result and then exits 1 is recorded as `done`. Decide whether
   that is deliberate tolerance or a swallowed failure, and say which.
5. **Trust is checked against a string, the run happens in a path.**
   `start_task` reads the row with `store.project(&path)` using the raw string
   from the frontend, but spawns in `project::validate_dir(&path)`. Try to make
   those two disagree — trailing slash, case difference, `.` or `..` segments, a
   subdirectory of an already-trusted repo, a short (8.3) path, a symlink or
   junction. A run inside a directory whose consent was never given loads that
   repository's `.claude/settings.json` hooks and `.mcp.json`. This is the rule
   the whole trust flow exists for.
6. **`detect()` runs again inside `start_task`.** Every run spawns a `--version`
   child before it spawns the real one, on a command with no timeout, and the
   result can disagree with the list the UI is showing. Decide whether the
   second detection buys anything, and whether it blocks. Related:
   `#[tauri::command(async)]` on a *synchronous* body — establish which thread
   Tauri 2 actually runs that on and whether a hung shim freezes the window.
7. **The totals in the UI.** `tokens.total` is
   `inputTokens + cachedInputTokens + outputTokens`. Reasoning tokens are
   dropped from the total entirely, and cached reads are added to input —
   correct only if each provider reports them disjointly. Verify per provider.
   Then look at `cost.toFixed(4)`: a run costing $0.00004 renders as `$0.0000`,
   which reads as free. Under this project's own honesty rule, decide whether
   that is a lie, and fix it if it is.
8. **`mode` does nothing.** `efficient` / `balanced` is validated, stored in a
   column and offered as a segmented control the user can press. Nothing reads
   it until routing in milestone 6. A control that visibly changes nothing is a
   claim the app cannot cash. Say whether it should be removed until it means
   something, or whether the column alone is enough. Do not build routing.
9. **Usage rows can multiply.** `record()` inserts a `usage` row for every
   `Usage` event, and `stream()` inserts an `unavailable` row when none arrived.
   Establish whether a provider can emit `Usage` more than once in a single
   `exec` run, and what a later `SUM()` over that table would report. Also:
   `usage.model` is in the schema and nothing ever writes it.
10. **Nothing ever leaves `running`.** There is no cancel and no reconciliation.
    Close the app mid-run — the Job Object kills the child, and the `tasks` row
    stays `running` with a NULL `ended_at` forever. Decide whether milestone 4
    owes a startup sweep of stale rows, or whether that genuinely waits for the
    cancel work in milestone 5. Say which; do not leave it ambiguous.
11. **What the diff actually shows.** `diff_since` runs
    `git diff --numstat <base>` plus `git ls-files --others --exclude-standard`.
    Attack it: a path containing a space, a non-ASCII path under the default
    `core.quotePath` (git C-quotes it), a rename (numstat writes `old => new` in
    one field), a binary file (`-\t-`), a file the agent edited that is
    `.gitignore`d, and untracked files that were already there before the run.
    Confirm the numbers and paths the user sees are the ones on disk.
12. **The installer runs `npm install --global` on the user's machine.** It
    resolves `npm` through the same `which`, runs from `std::env::temp_dir()`
    so the opened repository's `.npmrc` cannot redirect the registry, and
    streams output as `install-event`. Attack the rest: nothing serialises two
    installs on the backend — the guard is a `:disabled` in the template, so a
    second window or a fast double-click races npm's global folder. Confirm the
    package names `@anthropic-ai/claude-code` and `@openai/codex` are the real
    published ones. Confirm a failed install reports why rather than leaving a
    button that appears to do nothing.
13. **Serde shape drift, again.** `ProviderEvent` is now adjacently tagged with
    `rename_all_fields = "camelCase"`, and `src/types.ts` mirrors it by hand.
    Confirm the `serde` version in `Cargo.toml` actually supports
    `rename_all_fields`, that every variant and field matches the TypeScript
    union exactly, that `ProviderEvent::kind()` returns the same spelling as the
    serialised tag for all six variants, and that `vue-tsc` would catch a
    mismatch rather than infer `any`.
14. **Events are logged before they are shown, and both can fail silently.**
    `record()` discards the result of `append_event`, `record_usage`, `emit`
    and `finish_task` with `let _ =`. Establish which of those failing is
    survivable and which leaves the user looking at a run whose record does not
    exist. The event log is described as append-only and replayable; confirm the
    payload written is genuinely enough to replay without re-parsing a provider.
15. **The stream has no ceiling.** `stream.value.push(event)` for the life of a
    run, one `<li>` each, with `describe()` returning the full text of every
    message. Decide whether a long run degrades the window, and whether a cap
    belongs here or waits.

## Verify

Run all of these. All must pass.

```
cd src-tauri && cargo test
cd src-tauri && cargo clippy --all-targets -- -D warnings
npm run build
npm test
```

If `src-tauri/icons/icon.ico` is missing, regenerate the set with
`node scripts/make-icon.mjs` rather than committing a binary by hand.

## Deliver

1. Apply the fixes.
2. Add a test for every defect you fix. A fix with no failing-test-first is not
   accepted. Prefer a Rust test over a regex against a source file; if the only
   way to test something is to grep a file, say so and explain why.
3. Leave the working tree clean and all four commands green.
4. Report, in this order and nothing else:
   - **Defects fixed** — one line each: file, what was wrong, what breaks
     without the fix.
   - **CLI flag corrections** — any argv or fixture change, and the source in
     the provider's own documentation that proves the new form.
   - **Suspected but not fixed** — anything you could not confirm, and why.
   - **Verified sound** — the claims above you actively tried to break and could
     not. Name the attack, not just the claim.
   - **Test results** — the final line of each of the four commands.

Do not report style opinions. Do not report anything you did not test.
