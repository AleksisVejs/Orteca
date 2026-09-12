# Review brief: Orteca milestone 3

You are reviewing one slice of an early-stage Windows desktop app: provider
detection, event normalisation, and a mock that replays JSONL fixtures. Be
adversarial about correctness. Do not expand scope.

## Objective

Verify that milestone 3 does what it claims, fix what is broken, and leave the
repository green and commit-ready.

## Constraints

- Windows only. Do not add cross-platform code paths.
- Do not add dependencies. `async_trait` in particular was deliberately not
  added; do not add it to satisfy the interface sketched in section 6 of
  `docs/architecture.md`.
- Do not implement milestone 4 or later. No spawning a real provider, no
  routing, no orchestrator, no task execution, no new tables.
- Do not invoke the `claude` or `codex` CLIs from any test. Neither is installed
  on the dev machine and a real run would spend the user's subscription.
- Keep the diff minimal. A smaller correct change beats a larger tidy one.
- Match the surrounding style: comments explain *why*, never *what*.

## What the code claims

- `src-tauri/src/providers/mod.rs` — `ProviderId::{Claude, Codex}` with
  `detect()` and `parse_line()`. `detect()` resolves the program against
  PATH x PATHEXT itself (`which`), runs `--version`, and infers an auth mode
  from an environment variable or the existence of a credential file. It never
  reads a credential's contents. "Not installed" is a `Detected` row, never an
  error.
- `src-tauri/src/providers/claude.rs` — parses `--output-format stream-json`.
  Usage is taken *only* from the final `result` event, on the claim that
  per-message usage is cumulative and summing it would double count.
- `src-tauri/src/providers/codex.rs` — parses `codex exec --json`. Every usage
  event it emits is `cost_quality: unavailable` with `cost_usd: None`, because
  Codex reports tokens and no cost.
- `src-tauri/src/providers/mock.rs` — reads a fixture as JSONL and pushes it
  through the same `parse_line` a live run would use. Only the process is faked.
- `src-tauri/src/main.rs` — `detect_providers` returns a row per provider. It is
  the only new Tauri command and it returns `Vec<Detected>`, not a `Result`.
- `src/views/Project.vue` — shows each provider's version, auth mode, and
  "tokens only, no cost" where that applies; "not installed" where it applies.

## Start here

```
src-tauri/src/providers/mod.rs
src-tauri/src/providers/claude.rs
src-tauri/src/providers/codex.rs
src-tauri/src/providers/mock.rs
src-tauri/fixtures/claude-run.jsonl
src-tauri/fixtures/codex-run.jsonl
src-tauri/src/main.rs
src/types.ts  src/api.ts  src/views/Project.vue
docs/architecture.md        (sections 2, 3, 6 and 14 are the contract)
```

## Known weakness, stated up front

**The fixtures are not recordings.** `src-tauri/fixtures/*.jsonl` were written
by hand to the event shapes documented in sections 2 and 3 of
`docs/architecture.md`, because neither CLI is installed here. Every parser test
therefore proves the parser agrees with those documented shapes — not that the
shapes are right. Treat the fixtures as the primary suspect, not as evidence.
If you can establish from the CLIs' own documentation that a field name or an
event name is wrong, fix the fixture *and* the parser together.

## Specific things to attack

1. **`which` versus `CreateProcess`.** The claim is that PATH x PATHEXT
   resolution is required because `CreateProcess` only appends `.exe`, so an npm
   `claude.cmd` shim is invisible to a bare program name. Confirm that claim,
   then attack the implementation: empty and quoted PATH entries, relative PATH
   entries, a *directory* named `claude.exe`, a PATHEXT that is empty or has no
   leading dots, extension precedence against what the shell actually picks, and
   PATH unset entirely. Confirm resolution never falls back to the current
   directory — a repository that ships its own `claude.exe` must not win.
2. **Executing a `.cmd`.** Rust's `Command` runs a batch shim through
   `cmd.exe`. `Cargo.toml` now floors `rust-version` at 1.77.2 for
   CVE-2024-24576. Confirm that floor is the right one and that nothing else in
   the crate re-introduces unescaped batch arguments. Today only `--version` is
   passed; milestone 4 passes a user-written prompt, so say plainly if the
   escaping is not something to rely on.
3. **`FailureKind` has six variants nothing produces.** `CliMissing`,
   `AuthExpired`, `UsageLimit`, `RateLimit`, `MalformedOutput` and `Cancelled`
   are never emitted by either parser. Decide which it is: a real gap, because
   the CLIs *do* report expired auth or a usage limit in a shape the parser is
   dropping, or dead flexibility that should be deleted. Do not leave it
   ambiguous, and do not invent a mapping you cannot point at an event for.
4. **Silently dropped events.** `parse_line` returns an empty `Vec` for any
   event it does not recognise. Establish whether a real failure — auth expired,
   rate limited, sandbox denial, a Codex `turn.failed` shape other than the one
   handled — can arrive as an unrecognised line and vanish, leaving a run that
   looks like it simply stopped.
5. **Cumulative-usage claim.** `claude.rs` ignores per-message usage and reads
   only the final `result`. Verify that is correct rather than lossy: a run that
   is cancelled, or that fails before emitting `result`, reports no usage at all
   under this design. Decide whether that is honest or a hole.
6. **Cache accounting.** `cached_input_tokens` maps only
   `cache_read_input_tokens`; `cache_creation_input_tokens` is dropped entirely
   rather than folded into `input_tokens`. Under the project's own rule that
   nothing reaches the UI unlabelled, decide whether dropping it understates the
   run and, if so, fix it in the one place both parsers agree on.
7. **Cost labelling.** Claude's `total_cost_usd` is labelled `estimated`, never
   `exact`; Codex is always `unavailable` with `cost_usd: None`. Try to find any
   path that produces `exact`, or that produces a zero cost where the truthful
   answer is "unknown". A zero that reads as free is worse than no number.
8. **Auth inference.** `Auth::Subscription` is inferred purely from the
   existence of `%USERPROFILE%\.claude\.credentials.json` or
   `%USERPROFILE%\.codex\auth.json`. Confirm those are the paths each CLI
   actually writes **on Windows**. If Claude Code stores its credential
   elsewhere on this platform, the UI currently tells a signed-in user they are
   not signed in, or the reverse — both are lies the honesty rule forbids. An
   empty or malformed credential file is also worth a thought.
9. **Blocking the UI thread.** `detect_providers` is a synchronous
   `#[tauri::command]` that spawns two child processes, and `Project.vue` calls
   it from `onMounted`. Determine which thread Tauri 2 runs a non-async command
   on, and whether a slow or hung shim freezes the window. `version_of` has no
   timeout and says so in a `ponytail:` comment. Decide whether the comment is
   sufficient or the freeze is real.
10. **Swallowed errors in the UI.** `detectProviders().catch(() => [])` in
    `src/views/Project.vue` turns any failure of the command into an empty list,
    which renders as nothing at all. Confirm that is distinguishable from "both
    CLIs are missing", which must render as two explicit rows.
11. **Serde shape drift.** `src/types.ts` mirrors `Detected`, `ProviderId`,
    `Auth` and `CostQuality` by hand. Confirm every `#[serde(rename_all)]` and
    every variant name matches the TypeScript union exactly, and that `vue-tsc`
    would actually catch a mismatch rather than infer `any`.
12. **The missing trait, on purpose.** Section 6 of `docs/architecture.md`
    specifies `trait Provider` with `async_trait`; the code ships an enum and
    free functions instead, on the argument that there is nothing to dispatch on
    until milestone 4. Say whether that argument holds, or whether milestone 4
    will have to rewrite these call sites. Do not add the trait; just say.

## Verify

Run all of these. All must pass.

```
cd src-tauri && cargo test
cd src-tauri && cargo clippy --all-targets -- -D warnings
npm run build
npm test
```

If `src-tauri/icons/icon.ico` is missing, regenerate it with
`node scripts/make-icon.mjs` rather than committing a binary by hand.

## Deliver

1. Apply the fixes.
2. Add a test for every defect you fix. A fix with no failing-test-first is not
   accepted. Parser defects get a fixture line or an inline JSON case, never a
   live CLI call.
3. Leave the working tree clean and all four commands green.
4. Report, in this order and nothing else:
   - **Defects fixed** — one line each: file, what was wrong, what breaks
     without the fix.
   - **Fixture corrections** — any event or field name you changed, and the
     source that proves the new shape.
   - **Suspected but not fixed** — anything you could not confirm, and why.
   - **Verified sound** — the claims above you actively tried to break and could
     not. Name the attack, not just the claim.
   - **Test results** — the final line of each of the four commands.

Do not report style opinions. Do not report anything you did not test.
