# Review brief: Orteca milestones 1 and 2

You are reviewing an early-stage Windows desktop app. Be adversarial about
correctness. Do not expand scope.

## Objective

Verify that milestones 1 and 2 actually do what they claim, fix what is broken,
and leave the repository green and commit-ready.

## Constraints

- Windows only. Do not add cross-platform code paths.
- Do not add dependencies. Do not introduce abstractions with one implementation.
- Do not implement milestones 3 or later. No provider integration, no routing,
  no orchestrator, no task execution.
- Do not invoke the `claude` or `codex` CLIs from any test.
- Keep the diff minimal. A smaller correct change beats a larger tidy one.
- Match the surrounding style: comments explain *why*, never *what*.

## What the code claims

Milestone 1 — process runner:
- `src-tauri/src/proc/job.rs` wraps a Win32 Job Object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Dropping it must kill the entire process
  tree, including grandchildren the direct child spawned.
- `src-tauri/src/proc/mod.rs` spawns a CLI inside that job, reads stdout as
  newline-delimited JSON, merges stderr as text, keeps stdin open so a running
  agent can be given further instructions, and reports process exit.

Milestone 2 — open a project:
- `src-tauri/src/project.rs` reads git state (branch, HEAD, dirty count) by
  shelling out to `git`, and scans for repository files that cause a provider
  CLI to execute repo-controlled configuration.
- `src-tauri/src/store.rs` is SQLite with numbered migrations tracked by
  `PRAGMA user_version`. `projects.opened_seq` orders recents because
  `datetime('now')` only resolves to the second.
- `src-tauri/src/main.rs` exposes `open_project`, `recent_projects`,
  `trust_project`, `forget_project`. `open_project` rejects non-git folders and
  anchors on the git root so opening a subdirectory does not create a second row.
- The frontend must not let an agent run in an untrusted project:
  `src/App.vue` shows `src/components/TrustPrompt.vue` when
  `trustFindings` is non-empty and the project is not yet trusted.

## Start here

```
src-tauri/src/proc/job.rs
src-tauri/src/proc/mod.rs
src-tauri/src/store.rs
src-tauri/src/project.rs
src-tauri/src/main.rs
src-tauri/migrations/0001_init.sql
src/App.vue
src/api.ts
docs/architecture.md        (sections 1, 5, 7, 10, 11 are the contract)
```

## Specific things to attack

1. **Job Object lifetime.** `Run` holds `_job` alongside the stdin handle and
   the channel. Confirm drop order actually kills the tree, and that
   `kill_on_drop(true)` on the tokio `Child` does not fight the job or mask a
   failure. Confirm a failure to assign the process to the job is not silently
   ignored.
2. **Channel back-pressure.** The event channel has a fixed capacity. Determine
   what happens when a consumer stops reading while a chatty process keeps
   writing: can the reader tasks block such that the `Exit` event is never
   delivered, or such that drop hangs? If so, fix it.
3. **Cancel mid-write.** A process killed while emitting a partial JSON line
   must not produce a panic or a bogus `Json` event.
4. **`opened_seq` correctness.** The `ON CONFLICT DO UPDATE` recomputes
   `MAX(opened_seq) + 1` with a subquery over the table being written. Prove the
   ordering holds with at least three projects and repeated re-opens, not two.
5. **Migration safety.** `PRAGMA user_version` starts at 0. Confirm a partially
   applied batch cannot leave the version ahead of the schema, and that opening
   an existing database twice is a no-op.
6. **Path identity.** Confirm the same repository opened by different routes
   (folder picker, recents entry, a subdirectory, a trailing separator) always
   resolves to exactly one `projects` row. Note that `git rev-parse
   --show-toplevel` returns forward slashes while the OS picker returns
   backslashes.
7. **Git edge cases.** Repository with no commits, detached HEAD, a path
   containing spaces or non-ASCII characters, and a folder that is not a
   repository at all. `git` missing from PATH entirely must not panic.
8. **Trust scan completeness.** The list in `TRUST_TARGETS` is the security
   boundary: a repository file that makes a provider CLI execute
   repo-controlled configuration and is *not* in that list is a real hole.
   Check the list against what Claude Code and Codex actually load, and say
   plainly if something is missing rather than assuming it is complete.
9. **Trust bypass.** Confirm there is no path through `src/App.vue` that reaches
   the project screen with pending trust findings unacknowledged.
10. **Error surface.** Every `#[tauri::command]` returns `Result<_, AppError>`.
    Confirm nothing panics across the Tauri boundary on bad input (missing path,
    a file instead of a folder, a path the process cannot read).

## Verify

Run all of these. All must pass.

```
cd src-tauri && cargo test
cd src-tauri && cargo clippy --all-targets -- -D warnings
npm run build
```

If `src-tauri/icons/icon.ico` is missing, regenerate it with
`node scripts/make-icon.mjs` rather than committing a binary by hand.

## Deliver

1. Apply the fixes.
2. Add a test for every defect you fix. A fix with no failing-test-first is not
   accepted.
3. Leave the working tree clean and all three commands green.
4. Report, in this order and nothing else:
   - **Defects fixed** — one line each: file, what was wrong, what breaks
     without the fix.
   - **Suspected but not fixed** — anything you could not confirm, and why.
   - **Verified sound** — the claims above you actively tried to break and could
     not. Name the attack, not just the claim.
   - **Test results** — the final line of each of the three commands.

Do not report style opinions. Do not report anything you did not test.
