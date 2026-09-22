# Benchmark Lab plan

## Goal

Turn routing, memory, code-map and model choices into decisions backed by repeatable, blinded evidence rather than one-off runs. The lab compares an Orteca route with the same provider's plain CLI under the same repository state and task brief.

## Success criteria

- A versioned corpus has at least 20 tasks across small edits, standard work, cross-file refactors, schema work and guarded/security work.
- Each task has public acceptance checks plus hidden behavioral or security checks where appropriate.
- Each comparison records pass/fail, hidden-check recall, wall time, uncached tokens, calls, local verification result and review outcome.
- A routing or context-policy change needs repeated comparable cells and a documented confidence interval before it becomes a default.

## Delivery slices

### 1. Lock the harness contract

Define a manifest for each task: repository revision, setup command, prompt, visible checks, hidden checks, task class, allowed providers and a deterministic cleanup rule. Record CLI version, model and reasoning effort for every cell. Keep test data and credentials out of the repository fixtures.

### 2. Build the corpus

Start with the existing LiftMe and RigInspect benchmark fixtures, then add small disposable repositories for typo, bug, feature, refactor and path-containment tasks. Include a clean control for every security family. Review each hidden check for fairness before admitting it to the corpus.

### 3. Run fair baselines

For every task, run plain Claude/Codex and Orteca with the same provider/model, prompt, clean revision and allowance. Run at least three repetitions per cell; randomize arm order and retain raw JSONL, command output and patches.

### 4. Grade and publish a local report

Grade acceptance and hidden checks without exposing them to agents. Report task-level evidence and aggregate medians with intervals, never a single blended score. Separate correctness, review discoveries, latency, uncached-token use and costs that providers actually report.

### 5. Make one decision at a time

Use the first corpus pass to validate code-map briefs and Memory on related follow-up tasks. Only then test one routing or effort change. Keep a change only when it improves the predefined objective without a material correctness regression; otherwise revert it and retain the result as negative evidence.

## Initial experiment order

1. Repeat the existing typo, bug and feature cells to measure the unverified code-map/context savings.
2. Add repeated Standard-route cells to test whether Review earns its call outside the current LiftMe result.
3. Run paired follow-up tasks with and without approved Memory items, measuring both provenance and prompt overhead.
4. Test guarded Review tier/effort choices only on seeded, all-tests-pass security cases.

## Non-goals

This does not automatically tune routes, spend usage in the background, claim provider costs that are unavailable, or replace local verification with model grading.
