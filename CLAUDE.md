# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Orteca is a Windows desktop app (Tauri 2 + Vue 3 + strict TypeScript, Rust backend)
that orchestrates the `claude` and `codex` CLIs over a local git repo: it classifies a
prompt, routes it through agent stages, streams their JSONL events, and reports honest
token/cost metrics.

`docs/architecture.md` is the source of truth — verified CLI flags, the routing model,
the full SQLite schema, and the 8-milestone plan. Read it before designing anything.
Milestones 1-4 are done; 5 (cancel and mid-task instruction) is next.

## Commands

Rust lives in `src-tauri/`; run `cargo` from there. Everything else from the repo root.

```bash
npm install
node scripts/make-icon.mjs          # rebuild src-tauri/icons/ from src/assets/velo.png
npm run build                       # vue-tsc --noEmit + vite build
npm test                            # node --test scripts/*.test.mjs
node --test scripts/app.test.mjs    # single JS test file
cd src-tauri && cargo test          # Rust tests
cd src-tauri && cargo test trust_scan -- --nocapture   # single Rust test
npm run tauri dev                   # full app (root, not src-tauri)
```

Toolchain: Node 22, Rust 1.98 MSVC, VS 2022 Build Tools (`VC.Tools.x86.x64`).

## Architecture

Rust (`src-tauri/src/`) owns processes, git and storage; Vue owns two screens and a modal.

- `main.rs` — Tauri commands (`open_project`, `recent_projects`, `trust_project`,
  `forget_project`) and app setup. The `Store` is Tauri managed state.
- `proc/` — spawns a child inside a **Win32 Job Object** (`KILL_ON_JOB_CLOSE`) and reads
  its stdout as JSONL while keeping stdin open for steering. Rust's `Child::kill()` only
  kills the direct child; `claude` spawns node → bash → npm, so cancel means closing the
  job handle. Never replace this with a plain kill.
- `project.rs` — git state, path validation, and the **trust scan**.
- `store.rs` — SQLite via `rusqlite` (bundled), plain numbered `.sql` migrations in
  `src-tauri/migrations/`, no ORM.
- `providers/` — `ProviderId::detect()` (PATH x PATHEXT resolution, `--version`,
  auth mode) and `parse_line()`, which normalises each CLI's JSONL into
  `ProviderEvent`. `mock.rs` replays `src-tauri/fixtures/*.jsonl` through those
  same parsers. No trait yet — one enum, two parsers, nothing to dispatch on.
- `error.rs` — `AppError { kind, message }`, the only error shape the frontend sees;
  `src/api.ts` mirrors it with `isAppError`.

Frontend: `src/api.ts` is the single `invoke` wrapper — add commands there, mirror types
in `src/types.ts`. No Pinia. Colors and spacing come from `src/styles/tokens.css`, and
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
  Orteca reads git state and captures diffs; it does not rewrite the user's repo.
- **Every metric carries a `cost_quality`** of `exact` | `estimated` | `unavailable`.
  Nothing reaches the UI unlabelled, and there is no savings percentage until a project
  has a real baseline. Codex reports tokens but no cost — say so, don't infer one.
- Permissions are enforced by the provider CLIs (`--permission-mode acceptEdits`,
  `--sandbox workspace-write`). Orteca configures them and adds a denylist on top; it
  does not reimplement sandboxing. Never `--sandbox danger-full-access`.

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
