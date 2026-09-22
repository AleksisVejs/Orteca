# Benchmark Lab

The lab is local tooling for fair, repeatable routing and context experiments. It does not start provider CLIs, consume allowance, change a route, or claim a provider cost. `scripts/bench/riginspect.mjs` remains the runner for the existing LiftMe and RigInspectBE cases; the lab gives every runner one contract and one reporting format.

## Corpus and private graders

`benchmarks/corpus.json` is the public, versioned task contract. It currently contains 20 tasks spanning small edits, standard work, cross-file refactors, schema work, and guarded/security work. Each task fixes the repository revision, setup and cleanup commands, prompt, public checks, allowed providers, and an opaque `hiddenCheckId`. Guarded tasks also name a clean control.

Keep the command or test behind `hiddenCheckId` outside the target checkout. Before admitting a hidden check, verify that HEAD fails it and a minimal correct patch passes it; run its named clean control too. Never put credentials, private test data, or hidden assertions in an agent-visible fixture.

## Fair cell protocol

1. Check out the manifest revision into a disposable worktree, run setup, and record the CLI version, provider, model, reasoning effort, and route/context policy.
2. Create a three-repetition schedule before starting with `node scripts/bench/lab.mjs plan --repetitions=3 --seed=41 --out=bench-plan.json`. It randomizes plain and Orteca arm order for each paired task.
3. Give both arms the exact manifest prompt and clean revision. Save raw provider JSONL, terminal output, and the patch outside the source repository.
4. Run public checks, then attach the private grader after the patch is captured. Record both results; do not reveal private failures to the agent during the cell.
5. Add one normalized JSON object per completed cell to a local `raw.jsonl`. Use `null` for unavailable provider costs or tokens, never zero.

Required result fields are `taskId`, `arm`, `repetition`, `publicPass`, `hiddenPass`, `wallMs`, `uncachedTokens`, `calls`, `localVerify`, and `reviewOutcome`. Include the run metadata and paths to raw evidence beside these fields. `reviewOutcome` is `found`, `none`, or `unavailable`.

```json
{"taskId":"liftme-cancel-order","arm":"orteca-claude","repetition":1,"publicPass":true,"hiddenPass":true,"wallMs":184000,"uncachedTokens":23110,"calls":3,"localVerify":"pass","reviewOutcome":"found"}
```

## Local operations

```powershell
node scripts/bench/lab.mjs validate
node scripts/bench/lab.mjs plan --repetitions=3 --seed=41 --out=bench-plan.json
node scripts/bench/lab.mjs report --results=raw.jsonl --out=benchmark-report.md
node --test scripts/benchmark-lab.test.mjs
```

The existing LiftMe/RigInspect runner also guards rolling allowance before it starts an arm. Its default requires 15% weekly allowance to remain after reserving 5% for the next arm; override `WEEKLY_FLOOR` and `WEEKLY_ARM_RESERVE` only when an experiment has an explicitly different approved budget.

The report presents each task/arm cell separately: complete pass rate, hidden-check pass rate, median wall time, a 95% bootstrap interval, uncached tokens, calls, local verification, and review discoveries. It deliberately has no blended score. A default route or context-policy change needs repeated comparable cells and its interval to support the predefined objective without a material correctness regression. Keep negative results with the same evidence.
