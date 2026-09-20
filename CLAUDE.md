# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Orteca is a Windows desktop app (Tauri 2 + Vue 3 + strict TypeScript, Rust backend)
that orchestrates the `claude` and `codex` CLIs over a local git repo: it classifies a
prompt, routes it through agent stages, streams their JSONL events, and reports honest
token/cost metrics.

`docs/architecture.md` is the source of truth — verified CLI flags, the routing model,
the full SQLite schema, and the 8-milestone plan. Read it before designing anything.
Milestones 1-7 are done — the deferred file cache shipped as the code map;
8 has an unsigned NSIS installer and waits on a signing certificate. CI
(`.github/workflows/ci.yml`) runs `npm test`, `npm run build` and `cargo test`
on Windows.

## Commands

Rust lives in `src-tauri/`; run `cargo` from there. Everything else from the repo root.

```bash
npm install
node scripts/make-icon.mjs          # rebuild src-tauri/icons/ from src/assets/velo.png
npm run build                       # vue-tsc --noEmit + vite build
npm test                            # node --test scripts/*.test.mjs
node --test scripts/app.test.mjs    # single JS test file
node scripts/bench/riginspect.mjs easy   # spends real usage; see its header
cd src-tauri && cargo test          # Rust tests
cd src-tauri && cargo test trust_scan -- --nocapture   # single Rust test
npm run tauri dev                   # full app (root, not src-tauri)
```

Toolchain: Node 22, Rust 1.98 MSVC, VS 2022 Build Tools (`VC.Tools.x86.x64`).

## Architecture

Rust (`src-tauri/src/`) owns processes, git and storage; Vue owns two screens and a modal.
The Project screen is a shell plus pages in `src/views/project/`, sharing `state.ts`.
Runs are concurrent in both directions: `state.ts` keeps a `runs` list, one reactive
`LiveRun` per task with its own stream, result and steering box, and `App.vue` keeps
every opened project mounted (hidden, never torn down) so a run keeps streaming while
another project is on screen. That is why each project namespaces its element ids
through `domId` - popovers are targeted by id - and why window-wide listeners (file
drop, Ctrl+`) check the `active` flag before answering.

- `main.rs` — Tauri commands (projects: `open_project`, `recent_projects`,
  `trust_project`, `forget_project`; providers: `detect_providers`,
  `install_provider`, `sign_in_provider`, `provider_limits`; runs: `start_task`, `cancel_task`,
  `send_instruction`, `recent_tasks`, `remove_worktree`) and app setup. `start_task` routes the
  prompt, snapshots a dirty tree, and refuses Codex on a folder whose ACL the user
  cannot change, all before any CLI starts. The `Store` is Tauri managed state.
- `routing.rs` — pure keyword classifier, route and tiers per route, stage
  briefs, artifact checks. No model call and no I/O beyond what `main.rs` hands it.
- `codemap.rs` — pure tree-sitter parse of one source file (PHP, TS, JS, Rust; a
  `.vue` file's `<script>` block as TypeScript) into `FileFacts { defines, uses }`,
  plus the Laravel strings that name a file. `store.rs` keeps a map per project,
  rescanned on open and before a run, reparsing only what moved; `routing` ranks
  with it. `NAV_NOMAP=1` withholds it from a run, for the benchmark's before arm.
- `intent.rs` — before a run, asks the provider's smallest model (no tools, temp dir)
  whether the prompt is a question or easy/medium/hard work. Feeds `routing` as
  `RepoSignals::intent`; a failed read falls back to keywords.
- `run.rs` — runs a route stage by stage: argv, stream, event log, steering,
  fix rounds, diff, `TaskResult`. No call, turn or token ceiling. A failed
  Verify gets one Fix (resuming the session that wrote the change) and runs
  again, a Review runs once; on a guarded route it runs beside the local
  suite and one Fix answers both. Beside that Fix, the failing Laravel test
  files run again in a throwaway worktree at `base_commit`; if they already
  failed there the Fix is stopped and the run ends `verifyFailed`. A PHPUnit suite on in-memory SQLite (no ParaTest) runs as
  up to 8 parallel shards, each with its own Laravel manifests and `storage/`
  tree; a failing shard's tests rerun in one process and that is the verdict.
- `proc/` — spawns a child inside a **Win32 Job Object** (`KILL_ON_JOB_CLOSE`) and reads
  its stdout as JSONL while keeping stdin open for steering. Rust's `Child::kill()` only
  kills the direct child; `claude` spawns node → bash → npm, so cancel means closing the
  job handle. Never replace this with a plain kill.
- `project.rs` — git state, path validation, the **trust scan**, and the diff,
  whose entries are labelled `run` / `beforeRun` / `both` against a pre-run snapshot.
- `store.rs` — SQLite via `rusqlite` (bundled), plain numbered `.sql` migrations in
  `src-tauri/migrations/`, no ORM.
- `providers/` — `ProviderId::detect()` (PATH x PATHEXT resolution, `--version`,
  auth mode) and `parse_line()`, which normalises each CLI's JSONL into
  `ProviderEvent`. `mock.rs` replays `src-tauri/fixtures/*.jsonl` through those
  same parsers. No trait yet — one enum, two parsers, nothing to dispatch on.
- `error.rs` — `AppError { kind, message }`, the only error shape the frontend sees;
  `src/api.ts` mirrors it with `isAppError`.
- `dock.rs` — the workspace's other half, outside the run path: real ConPTY
  terminals (`portable-pty`, each in a `proc::job::Job` so a tab takes its whole
  tree with it), a directory listing, and one text file in and out for the code
  tab. Every command goes through `inside()` — `trusted_dir`, then canonicalize
  and `starts_with` the root — so nothing reaches outside a consented repo. Its
  history tab calls `project::git_log` / `project::commit_patch` /
  `project::working_patch`; git logic stays
  in `project.rs`, which owns every git call.

Frontend: `src/api.ts` is the single `invoke` wrapper — add commands there, mirror types
in `src/types.ts`. No Pinia. The dock (`views/project/dock.ts` plus `Dock*.vue`) is its
own `provide`d state beside `state.ts`; its tabs and settings live in `localStorage`,
never the store. Colors and spacing come from `src/styles/tokens.css`, and
`docs/ui.md` is the binding rulebook for anything visual — read it before touching UI.

## Rules this codebase runs on

- **Never pass `--bare` to `claude`** — it disables OAuth/keychain and forces an API key.
  The consequence is that a run loads the *target repo's* `.claude/settings.json` hooks
  and `.mcp.json` with no prompt, which is why every untrusted project must be consented
  to, even one with zero findings. Don't weaken `trust_scan` or make consent conditional.
- **Never invoke a real provider CLI from a test.** Tests replay recorded JSONL through
  the mock provider. Both CLIs are installed but neither can complete a run, and
  "CLI missing" is a first-class UI state, not an error.
- **Never run a destructive git command** (`push --force`, `reset --hard`, `clean -fd`).
  Orteca changes the user's repo only when the user asks and confirms: the git bar's
  fetch, `pull --ff-only`, commit, plain push and a clean-tree merge that aborts on
  a clash (`project::git_action`). Nothing there may force, reset or discard work.
- **Every metric carries a `cost_quality`** of `exact` | `estimated` | `unavailable`.
  Nothing reaches the UI unlabelled, and there is no savings percentage until a project
  has a real baseline. Codex reports tokens but no cost — Orteca prices them from
  models.dev's published rates as `estimated`, and an unpriced model stays `unavailable`.
- Permissions are enforced by the provider CLIs (`--permission-mode acceptEdits`,
  `--sandbox workspace-write`). Orteca configures them and adds a denylist on top; it
  does not reimplement sandboxing. Never `--sandbox danger-full-access`.

## Orteca on Orteca

This repo is a normal project for Orteca: open it, consent once, run in **Current tree**.
Two things make it different from any other target:

- **Run the built app, not `npm run tauri dev`.** The dev watcher rebuilds and restarts
  the app the moment a run edits `src-tauri/`, and the run dies with it - its children
  are in the app's Job Object. Vite's HMR does the same to a live run's UI on a `src/`
  edit. `src-tauri/target/release/orteca.exe` (or the NSIS installer beside it) has
  neither watcher; `npm run tauri dev -- --no-watch` covers only the Rust half.
- Verify is this repo's own suites, no model: `npm test` (~4s) and `src-tauri`'s
  `cargo test` (~19s warm). A worktree copy starts cold - `npm ci` plus a full build,
  about two minutes - which fits the 10-minute per-command limit with room to spare.

## Gotchas already paid for

- `tauri-build` needs `src-tauri/icons/icon.ico` even for `cargo test`. The whole icon
  set is generated from `src/assets/velo.png` — regenerate with
  `node scripts/make-icon.mjs`, never edit the files in `src-tauri/icons/` by hand.
- `CreateJobObjectW` requires the `windows` crate's `Win32_Security` feature; the error
  looks like a plain missing import.
- SQLite `datetime('now')` is second-resolution, so recents tie. Order by
  `projects.opened_seq`; the timestamp is display only.
- A user can open a subdirectory. `open_project` resolves to the git root via
  `rev-parse --show-toplevel` and stores that, so one repo is one row.
