# Speed plan 2: the agent's own time, and the route it is given

Start from branch `verify-fixes-liftme-bench` at `2e4c655` or later. Read
`docs/speed-plan.md` first: its steps 0-5 are in, step 6 (sharper Implement
brief) was reverted because medium and tough cost more with it.

Benchmark: `BENCH=liftme ARMS=orteca-claude RESULTS=<new file> node
scripts/bench/riginspect.mjs`. Run it from a detached worktree of the commit
under test, never the working tree, so an edit cannot leak into a run. One pass
used ~19% of the Claude 5-hour window (2026-09-18), so a pass of two runs per
task needs a fresh window.

## Where time goes now (LiftMe, 2026-09-18, pass C, one run each)

| task   | route         | Implement            | checks (Orteca)            | done  | cost  | plain claude |
|--------|---------------|----------------------|----------------------------|-------|-------|--------------|
| easy   | implementOnce | sonnet/low 33s       | 120s (focused 49s cold)    | 196s  | $0.10 | 34s, $0.30   |
| medium | standard      | sonnet/low 91s + Fix 124s | 183s (cold copy + base check 96s) | 524s | $0.39 | 61s, $0.35 |
| tough  | guarded       | sonnet/high 1002s, 123 turns + Fix 162s | 96s (shards 35s) | 1325s | $3.00 | 190s, $1.02 |

"done" includes classify (12-15s) and the bench start-up (~25s, fixed in
`2e4c655` after this pass). Grades held: 2/2, 4/4, 7/7.

What this says:

- **Checks are no longer the problem.** The sharded suite is 35-70s. What is
  left in the check column is cold starts on a fresh copy (the first `php
  artisan test` took 49-95s; the same files took 10s warm) and one base-commit
  check.
- **Guarded Implement is the problem.** Sonnet at `high` took 105-123 turns
  and 440-1000s where the plain CLI finished the whole task in 190s. The Opus
  Review was 33-38s and each time bought a 162s Fix.
- **The route itself changed.** The classify call reads "only the owning
  customer may call them" as authorization work, so tough runs `guarded`.
  The old $0.74 baseline never made that call (the bench skipped it).

## Rules for this work

- One step per commit, `cargo test` and `npm test` after each. Spend on a real
  benchmark only where a step says so; two runs per task, report both.
- Never lose a check: the full suite still ends every run, and security or
  authorization work still gets an independent Review.
- Don't touch `trust_scan`, the Job Object kill path, or the no-`--bare` rule.
- A step whose measured turns or cost rise is reverted, as step 6 was.

## Steps

0. **Warm the bench copy.** In `makeWorktree`, run one `php artisan test` on
   the smallest feature test after copying `vendor/`, so a run starts as warm
   as a real checkout. Free; rerun `SUITE=shards` and confirm the first
   focused test drops from ~50s to ~10s. Every later number depends on this.

1. **Guarded Implement effort, by measurement.** Record the tough task with
   guarded Implement at `sonnet/medium` and `sonnet/low` (two runs each). Keep
   the lowest effort whose grade stays 7/7 and whose Review still passes or
   asks only for what the hidden tests also need. Change `routing` tier
   effort only for Implement and Fix; the Review stays on Opus.

2. **Review beside Verify.** On a guarded route both are read-only: start the
   Opus Review while the local suite runs, then give one Fix both findings and
   run Verify once more. Cancel must stop both. Test: a fixture where Verify
   fails and Review asks for changes buys exactly one Fix whose brief carries
   both.

3. **Base-commit check off the critical path.** When Verify fails, start the
   base-copy check and the Fix together. If the base check says the test
   already failed, cancel the Fix (its partial edits stay in the diff, as a
   stopped run's do) and end `verifyFailed` as today. Only for a Fix Orteca
   would otherwise start; count the cancelled call's usage. Test with the
   existing failed-before fixture.

4. **Classify route check.** Collect every classify reading on the LiftMe and
   RigInspect prompts (free once the calls are recorded) and list the ones
   the keyword route and the classifier disagree on. Decide per case, with the
   user, whether "only the owner may" is authorization work that needs
   `guarded`. No code change until that list is agreed.

5. **The parallel-only shard failure.** Pass C medium had one shard fail and
   pass alone. Rerun `SUITE=shards` five times, name the test, and isolate
   what it shares (a fake disk, a temp file, a queue). Fix it with one more
   per-shard variable if one exists; otherwise the one-process rerun stays.

## Done when

- Easy: done at or under 120s warm, two runs; time to result under 60s.
- Tough: done under 600s, cost under $1.50, 7/7, on the route step 4 agreed.
- Grades unchanged, and every run still ends after the full suite.
