# Repository Guidelines

## Project Structure & Module Organization

Orteca is a Vue 3/TypeScript desktop application with a Tauri 2 backend written in Rust.
- `src/views/` contains screens; `src/components/` contains reusable Vue components.
- `src/api.ts` wraps Tauri commands; `src/types.ts` defines frontend data contracts; `src/styles/tokens.css` holds shared styling tokens.
- `src-tauri/src/` contains project discovery, SQLite storage, errors, and the process runner in `proc/`.
- `src-tauri/migrations/` contains SQL migrations; `src-tauri/icons/` contains application icons.
- `scripts/app.test.mjs` covers frontend behavior. `docs/architecture.md` describes the architecture; `docs/m1-m2-verification.md` records acceptance checks.

## Build, Test, and Development Commands

Run frontend commands from the repository root:
- `npm install`: install JavaScript dependencies.
- `npm test`: run Node's built-in test runner over `scripts/*.test.mjs`.
- `npm run build`: type-check Vue/TypeScript and build with Vite.
- `npm run dev`: start Vite for local development.
- `npm run tauri dev`: run the desktop application in development mode.

From `src-tauri/`, run `cargo test` for Rust tests and `cargo clippy --all-targets -- -D warnings` for lint checks. Windows is required to exercise Windows Job Object behavior.

## Coding Style & Naming Conventions

Match surrounding code: two-space indentation in TypeScript/Vue and four spaces in Rust. Use PascalCase Vue filenames, camelCase TypeScript functions, and snake_case Rust functions/modules. Preserve strict TypeScript checking and typed IPC contracts. Rust command names use snake_case; serialized frontend fields use camelCase. Use Clippy for Rust linting; frontend builds enforce type correctness.

## Testing Guidelines

Use descriptive behavior names in `*.test.mjs` and Rust tests alongside their modules. Add regression coverage for changed trust, asynchronous state, storage, or process-lifetime behavior. No numeric coverage threshold is documented. Run the relevant tests, production build, and Clippy before submitting code changes; report checks that could not run.

## Commit & Pull Request Guidelines

Follow the history's short, imperative subjects, such as `Fix M1 process events and M2 migration and trust flow defects`. Keep commits focused. PRs should explain the problem, resulting behavior, validation, and relevant issue or milestone. Include screenshots when UI changes have been visually verified.

## Security & Agent Instructions

Preserve explicit repository trust consent and process-tree cleanup. Never commit credentials. Prefix shell commands with `rtk`. Search before reading files, bound noisy output, and avoid reading images or generated bundles. Do not start dev servers or use previews for verification unless explicitly requested.
