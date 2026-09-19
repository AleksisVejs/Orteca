# Code map plan: find the right files without searching

Goal: the agent starts on the files it will edit instead of searching for
them. A small, local version of what graphify.net does, built into the brief.
Implements the deferred `file_cache` of `architecture.md` §12.

## What the measurement says (2026-09-19, free, no new runs)

Source: the 46 Claude session logs the LiftMe bench left in
`~/.claude/projects/*liftme-bench-wt-*` (37 Orteca, 9 plain CLI). Cost units
use Sonnet price ratios (fresh input 1, cache read 0.1, cache write 1.25,
output 5). Each session's first turn is left out, because it pays the
one-time system prompt whatever tool it calls.

| arm    | turns | search turns | search share of cost | search + reads of never-edited files, before the first edit |
|--------|-------|--------------|----------------------|--------------------------------------------------------------|
| Orteca | 603   | 188          | 26%                  | 19%                                                          |
| plain  | 137   | 31           | 23%                  | 30%                                                          |

The "Start here" list (`routing::candidates`, 10 paths) held **38 of the
154** files the Implement and Fix sessions edited (25%). That count includes
files the agent created, which no list can hold. It missed files that already
existed, again and again: `routes/api.php`, `Api/QuoteController.php`,
`Api/OrderController.php`, `app/Models/Conversation.php`. The list matches
prompt words against *paths*. "Let a customer edit a sent quote" never names
a controller or the routes file.

So navigation is about a fifth to a quarter of cost. The list is too weak to
cut it. A map that knows `routes/api.php` points at `QuoteController`, and
that `QuoteController` uses `Quote`, is what fills the gap. A realistic goal
is to halve navigation: **~10% of cost**, plus fewer turns and less time.

Risk already paid for: speed plan step 6 (a sharper brief) was reverted
because it raised cost. Anything this plan adds to the brief is measured
and reverted the same way.

## Rules for this work

- One step per commit, `cargo test` and `npm test` after each.
- `routing` stays pure. `main.rs` hands it the map, as it hands it
  `git ls-files` today.
- Offline recall (step 5) is free. Spend on a real bench only in step 6.
- No LLM calls, no embeddings, no MCP server, no graph UI.

## Steps

0. **Keep the measurement.** Add `scripts/bench/navstats.mjs`: the two
   scripts above (cost share by tool kind, and the list recall). Free, reads
   `~/.claude/projects`. It is the before/after for every later step.

1. **Parse.** New `src-tauri/src/codemap.rs`, pure:
   `parse(lang, source) -> FileFacts { defines: Vec<Symbol>, uses: Vec<String> }`.
   Crates: `tree-sitter` plus `tree-sitter-php`, `-typescript`, `-javascript`
   and `-rust`. A `.vue` file parses its `<script>` block as TypeScript.
   - defines: classes, interfaces, traits, functions, methods (name, kind, line).
   - uses: PHP `use` imports, `X::class`, `new X`, `X::method`,
     `extends`/`implements`; JS/TS import specifiers; Rust `use` paths.
   - Laravel strings that point at files: `'Controller@method'`,
     `view('mail.x')` → `resources/views/mail/x.blade.php`,
     `__('shop.key')` → `lang/*/shop.php`. A few string rules, no framework model.
   - Tests: small source snippets per language, asserting what comes out.
   - Check that the build time and `.exe` size stay acceptable. Four grammars
     add C code to compile.

2. **Store.** Migration `00NN_code_map.sql`:
   `map_files(project_id, path, sha256, mtime, size, lang)`,
   `map_symbols(file_id, name, kind, line)`, `map_uses(file_id, name)`.
   Names resolve to files at query time (name → the files that define it), so
   a changed file only rewrites its own rows. Rescan on `open_project` and
   before `start_task`: walk `git ls-files`, re-parse only files whose mtime
   or size changed, drop rows for files that are gone. Cap the file size
   (skip minified and vendored code). Tests on a temp repo: first scan,
   an unchanged rescan does no parsing, and edit / delete / rename.

3. **Rank.** `routing::candidates` gains two signals from the map:
   - A prompt word that matches a symbol name (after splitting camelCase and
     snake_case; "quote" matches `QuoteController`, `Quote`) scores like a
     file-name match.
   - One hop: files linked to the top seeds, in either direction, get a
     small score. `routes/api.php` uses `QuoteController`, so a controller
     seed pulls it in. `Quote` is used by it, so it comes in too.
   - Still the top 10, ties as today. The existing ranking tests keep passing.

4. **Brief.** For the top ~5 paths, add one line each with what the file
   defines and what it uses:
   `app/Http/Controllers/Api/QuoteController.php: class QuoteController (index, show, store); uses Quote, StoreQuoteRequest`.
   Hard cap ~1.5k tokens. The brief still never pastes file contents.

5. **Offline recall (free).** For each recorded bench prompt, rebuild the
   map at the task's base commit in a throwaway worktree. Rank, then compare
   with the files that session edited (only files that existed at base).
   Tune the step 3 scores against this. Target: **≥ 70% recall**, from 25%
   today. Put it in `navstats.mjs` so it reruns in seconds.

6. **Paid check.** `BENCH=liftme ARMS=orteca-claude`, easy + medium +
   blind-edit, two runs each, from a detached worktree. Compare with
   `results-after.json` and `results-blind.json`. Keep it only if the search
   share drops, grades hold, and cost is not higher. Otherwise revert step 4
   first (the brief lines), then step 3.

7. **Docs.** Update `architecture.md` §12 "As built" and the `CLAUDE.md`
   module list (`codemap.rs`).

## Size

About 800-1,200 lines of Rust with tests, and one migration. Steps 1-2 are one
session. Steps 3-5 are another, mostly tuning against free recall. Step 6 is
one bench pass, about 20% of a Claude 5-hour window.

## Not in this plan

Community clustering, LLM reading of docs and images, an interactive graph,
an MCP server. Add them only if step 6 shows the map pays and the remaining
search is about *understanding* code, not *finding* it.
