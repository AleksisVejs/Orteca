# Repository Guidelines

## Project Structure & Module Organization

Orteca is a Vue 3/TypeScript desktop application with a Tauri 2 backend written in Rust.
- `src/views/` contains screens; `src/components/` contains reusable Vue components.
- `src/api.ts` wraps Tauri commands; `src/types.ts` defines frontend data contracts; `src/styles/tokens.css` holds shared styling tokens.
- `src-tauri/src/` contains project discovery, SQLite storage, errors, and the process runner in `proc/`.
- `src-tauri/migrations/` contains SQL migrations; `src-tauri/icons/` contains application icons.
- `scripts/app.test.mjs` covers frontend behavior. `docs/architecture.md` describes the architecture; `docs/m1-m2-verification.md` records acceptance checks.

## Efficient Workflow

- Make the smallest complete change that satisfies the request. Reuse existing code and dependencies; avoid unrelated refactors or speculative abstractions.
- Inspect Git status once before editing and preserve unrelated changes. Do not commit or push unless requested.
- Search with `rg` or `rg --files` before reading. Read only relevant sections of large files; do not reread unchanged content already available in the conversation.
- Batch independent reads and searches. Bound noisy output with `Select-Object -First` or `-Last` in PowerShell; retain failure details and check command results.
- Use direct commands. RTK has been removed; do not prefix commands with it or reinstall it.
- Do not read images unless the user supplies or explicitly asks you to inspect them. Do not read generated bundles, build output files, or lockfiles unless essential to the requested diagnosis.
- Do not start dev servers or use previews unless explicitly requested.
- Keep updates and final responses concise. Report changes, checks, and material limitations without recounting routine commands.

## Validation

Run only checks relevant to the changed behavior, once after the edits. Broaden or repeat checks only for failures, further changes, or unresolved risks.
- Documentation-only changes: inspect the diff; no build or tests.
- Frontend changes: run the affected tests (`node --test scripts/app.test.mjs` for behavior, `node --test scripts/ui.test.mjs` for UI conventions) and `npm.cmd run build` for Vue/TypeScript changes.
- Rust changes: from `src-tauri/`, run focused `cargo test <filter>` checks and `cargo clippy --all-targets -- -D warnings`. Run the full Rust suite only for broad backend changes or when requested.
- Cross-layer changes: validate the affected frontend and Rust paths. Windows is required to exercise Windows Job Object behavior.
- Use `npm.cmd` on Windows. Install dependencies only when missing or changed; do not install them as a routine first step.
- Add regression coverage for changed trust, asynchronous state, storage, or process-lifetime behavior. Do not add tests that merely duplicate trivial implementation details.
- Report blocked checks accurately. A passing build does not prove visual behavior, CI, or deployment.

## Coding Style & Naming Conventions

Match surrounding code: two-space indentation in TypeScript/Vue and four spaces in Rust. Use PascalCase Vue filenames, camelCase TypeScript functions, and snake_case Rust functions/modules. Preserve strict TypeScript checking and typed IPC contracts. Rust command names use snake_case; serialized frontend fields use camelCase. Use Clippy for Rust linting; frontend builds enforce type correctness.

## Commit & Pull Request Guidelines

Follow the history's short, imperative subjects, such as `Fix M1 process events and M2 migration and trust flow defects`. Keep commits focused. PRs should explain the problem, resulting behavior, validation, and relevant issue or milestone. Include screenshots when UI changes have been visually verified.

## Security & Agent Instructions

Preserve explicit repository trust consent, typed IPC boundaries, and process-tree cleanup. Never commit credentials. Do not weaken security or error handling to shorten the implementation.
