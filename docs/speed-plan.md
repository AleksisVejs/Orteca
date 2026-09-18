# Speed plan: faster than the plain CLI without weaker checks

Start from branch `verify-fixes-liftme-bench`. Read `docs/architecture.md` §4.3.6 and
§4.3.9 first. Benchmark: `BENCH=liftme node scripts/bench/riginspect.mjs`
(`ARMS=orteca-claude`, `RESULTS=<new file>`); baseline numbers are in
`C:\Users\User\Projects\liftme-bench\results.json` and `results-after.json`.

## Where time goes today (LiftMe, Sonnet, 2026-09-17)

| task   | before agent | implement | verify | Orteca | plain claude |
|--------|--------------|-----------|--------|--------|--------------|
| easy   | ~42s         | 38s       | 183s   | 263s   | 34s          |
| medium | ~61s         | 137s      | 149s   | 347s   | 61s          |
| tough  | ~96s         | 246s      | 182s   | 524s   | 190s         |

"Before agent" is inferred (total minus stages), not measured. The full
`composer test` is ~140s of every Verify.

## Rules for this work

- One step per commit. Run `cargo test` and the free bench modes (`DRY`, `SUITE`)
  after each step; spend on a real benchmark only after steps 1-4.
- Never lose a check, only move it. Every run still ends with the full suite
  having run; nothing may mark a run `done` on a partial result.
- Don't touch `trust_scan`, the Job Object kill path, or the no-`--bare` rule.
- A speedup that is one benchmark sample is not a result. Run each task twice
  and report both.

## Steps

0. **Measure.** Record durations for the classify call, the check before the
   change, and each check command (not just each stage) in `TaskResult`. Rerun
   nothing; the next benchmark reads them.

1. **Check before the change only on failure.** Drop the pre-run quick check from
   the critical path. When the post-change Verify fails, find out if the base
   commit already failed by running the same failing test file in a temporary
   `git worktree` at `base_commit` (read-only for the user's tree; `worktree
   remove` after). Keep "failed before → no automatic Fix". Test: a fixture whose
   test fails on HEAD still buys no Fix.

2. **Classify in parallel.** Start the intent call and the repo scan together;
   skip the intent call when the keyword route is unambiguous (define that
   narrowly, with routing tests). Never start the agent before the route exists.

3. **Parallel suite shards, opt-in by evidence.** Only when `phpunit.xml` sets
   `DB_CONNECTION=sqlite` and `DB_DATABASE=:memory:`, and no paratest is
   installed: split test files into `min(cores, 8)` shards, run
   `php vendor/bin/phpunit <files...>` per shard in its own Job Object, merge the
   results. Any shard failure is a suite failure. Output must still name the
   failing test. Check with `SUITE` that sharded and serial agree on LiftMe
   HEAD. MySQL projects (RigInspectBE) stay serial.

4. **Result first, proof after.** When implement finishes and the focused tests
   pass, show the diff with status `checking` (new status, mirrored in
   `src/types.ts`, no success label). The full suite runs on; `done` only when
   it passes, `verifyFailed` + Fix if it fails. Cancel stops the suite too. UI
   per `docs/ui.md`. The bench measures both "time to result" and "time to
   done".

5. **Affected tests (careful).** Focus on tests that name a changed class
   basename or a route URI that `routes/*.php` maps to a changed controller.
   Plain text search, capped at 8 files. This only picks what runs *first*;
   the full suite still runs (step 4). Test with a controller change where the
   test names only the URI.

6. **Sharper implement brief.** Name the candidate paths, the paired test file
   and the exact focused test command, so the agent searches less. Compare
   turn counts before and after; revert if turns or cost rise.

## Done when

- Easy: time to result ≤ plain claude (34s) on 2 runs; medium and tough
  below plain.
- Grades unchanged (2/2, 4/4, 7/7), cost not above `results-after.json`.
- Every run still reaches `done` or `verifyFailed` after the full suite.
