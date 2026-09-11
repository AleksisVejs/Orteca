# Milestones 1 and 2 acceptance

## Process runner

The Windows child starts with `CREATE_SUSPENDED`. Orteca assigns it to a
kill-on-close Job Object before resuming its primary thread. Assignment or resume
failure returns an error and drops the child. Job configuration errors also close
the allocated handle.

Dropping a run terminates its job immediately. Normal parent exit terminates any
remaining descendants, drains stdout and stderr, then emits `Exit`. This prevents
detached descendants holding pipe handles from hiding the parent's exit. Only
newline-terminated stdout records are parsed as JSON. Stdin stays open for steering.

Output is buffered in memory so a paused consumer cannot deadlock the process.
Consumers must drain output during normal operation; this slice does not promise
bounded memory for arbitrarily large unread streams.

## Project trust

Every new repository requires explicit consent before the project view opens.
The finding list is explanatory, not a proof that a repository without findings
is safe. This is necessary because providers support configurable instruction
filenames, imports, plugins and additional directories.

The scan checks known instruction names and groups `.claude`, `.codex`, `.agents`
resources. It checks ancestors and nested repository directories, skips Git's
internal `.git` directory, and reports linked/unreadable paths without following
junctions. It reports a partial scan after 10,000 entries instead of blocking
indefinitely. All of these outcomes still require consent for an untrusted project.

Git discovery disables `core.fsmonitor`, which can otherwise execute a repository
command even during `git status`. Saved trust is repository-level consent, not a
hash-based approval of a particular revision. Milestone 3 must enforce this stored
decision at its future provider-invocation boundary; no provider invocation exists
in milestones 1 or 2.

Official sources checked on 2026-09-11:

- [Claude memory](https://code.claude.com/docs/en/memory): `CLAUDE.md`,
  `.claude/CLAUDE.md`, `CLAUDE.local.md`, rules, ancestors and nested instructions.
- [Claude settings](https://code.claude.com/docs/en/settings): project/local
  settings, hooks, MCP configuration and agents.
- [Claude skills](https://code.claude.com/docs/en/skills): skills, legacy commands
  and nested discovery.
- [Codex instructions](https://developers.openai.com/codex/guides/agents-md):
  `AGENTS.override.md`, `AGENTS.md` and configurable fallback filenames.
- [Codex skills](https://developers.openai.com/codex/skills): `.agents/skills`
  discovery from the working directory to the repository root.
- [Codex configuration](https://developers.openai.com/codex/config-basic):
  project `.codex/config.toml` configuration and trust.

## Repeatable checks

Run `cargo test` and `cargo clippy --all-targets -- -D warnings` from `src-tauri`.
Run `npm test` and `npm run build` from the repository root.

Regression coverage includes suspended creation, process-tree cancellation,
inherited pipes, Job Object error cleanup, JSONL truncation, event ordering,
paused consumers, steering, transactional migrations, repeated recents ordering,
path aliases, empty/detached repositories, missing Git, invalid paths, fsmonitor
execution, trust discovery and asynchronous frontend consent handling.

No test invokes Claude or Codex. No dev server or visual preview is used.

Final results: 25 Rust tests passed; 6 frontend tests passed; Clippy passed with
warnings denied; the production build passed. Milestones 1 and 2 have no remaining
acceptance blockers within the scope above.
