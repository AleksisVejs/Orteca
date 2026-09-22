# Codex pilot: LiftMe easy

## Status

Two paired cells completed. They measure the actual product choice: the easy plain arm used Terra medium while Orteca routed it to Luna; both medium arms used Terra. Since model selection is part of Orteca's value proposition, these are product comparisons; they are not, however, evidence that route context or staging alone caused the easy result. No further provider calls were queued.

## Evidence

| Field | Plain Codex | Orteca / Codex |
| --- | --- | --- |
| Task | LiftMe `easy` — blog `per_page` default/cap | Same prompt and clean revision |
| Model / route | `gpt-5.6-terra`, medium | `gpt-5.6-luna`, `implementOnce` |
| Result | completed, but 1/2 grade | completed, 2/2 grade |
| Public check | passed | passed |
| Hidden check | failed: capped response returned 12 rows where the grader expected 5 | passed |
| Wall time | 216.5s | 126.0s |
| Calls / turns | 1 / unavailable | 2 / 2 |
| Uncached input / output | 27,894 / 3,001 tokens | 38,859 / 1,291 tokens |
| Cached input | 380,416 tokens | 24,832 tokens |
| Estimated API-equivalent cost | $0.1679 | $0.0098 |

The plain patch changed two files (25 additions, 1 deletion); Orteca changed two (29 additions, 1 deletion). Orteca's result appeared at 74.4s and its full local verification finished at 134.1s. The weekly allowance moved from 22% before the plain arm to 20% after the pair, remaining above the agreed 15% floor.

The current product route won this cell on both hidden correctness and wall time (90.4s faster, 42%). That is a single data point, not aggregate evidence.

## Medium cell

| Field | Plain Codex | Orteca / Codex |
| --- | --- | --- |
| Task | LiftMe `medium` — efficient newest-message conversation list | Same prompt and clean revision |
| Model / route | `gpt-5.6-terra`, medium | `gpt-5.6-terra`, `standard` |
| Grade | 4/4 | 4/4 |
| Wall time | 145.9s | 217.3s |
| Calls / turns | 1 / unavailable | 3 / 3 (`implement > verify > review`) |
| Uncached input / output | 24,191 / 3,423 tokens | 57,874 / 4,897 tokens |
| Cached input | 276,224 tokens | 146,944 tokens |
| Estimated API-equivalent cost | $0.1447 | $0.1015 |

Here the model matched and both patches passed every check. Plain Codex was 71.3s faster (49%), while Orteca used fewer total tokens (209,715 versus 303,838) and a lower estimated API-equivalent cost. The Standard route's Verify and Review stages are the clear latency cost to investigate; this cell does not justify removing them, because it contains no seeded defect that exercises Review's value.

## Free evidence pass

- The versioned lab corpus validates: 20 tasks cover every planned task class, and the focused lab tests pass.
- Recorded code-map replay found 94 of 129 base-existing edited files (73% recall). The most frequent misses are conversation resources, localization files, and `routes/api.php`; these are the best candidates for targeted map/ranking diagnostics rather than another paid run.
- The hidden medium control is discriminative: unpatched HEAD failed `BenchMediumConversationListTest` (2 failures), while both saved plain-Codex and Orteca-Codex patches passed on regrade.
- `riginspect.mjs` now defaults to a 15% weekly floor plus a 5% conservative arm reserve. It stops before an arm starts when the selected provider does not have 20% remaining; `WEEKLY_FLOOR` and `WEEKLY_ARM_RESERVE` make that policy explicit for a future experiment.

## Improvements before the next paired cell

1. **Make the weekly floor enforceable before a call.** The harness records a limit before and after each arm, but an arm has no per-turn/token ceiling and can cross a floor before its post-arm check. Add a `MIN_WEEKLY_REMAINING` preflight threshold and do not start a paired cell unless the remaining headroom covers a conservative arm budget plus the configured floor.
2. **Keep model matching as a diagnostic, not the main score.** Plain used Terra medium; Orteca correctly exercised its configured Luna selection. If this product-level advantage holds across repeated cells, add an optional model-matched pair to separate the value of model selection from route context and staging.
3. **Preserve the hidden distinction.** Keep the public `BlogApiTest` and the hidden count/cap check separate in every report. A visible pass must never be reported as a completed task.
4. **Keep product and mechanism evidence separate.** This pair validates the current product configuration, including its model selection. A Terra-vs-Terra diagnostic is useful only when estimating the value of route context, local verification, and staging alone.
5. **Repeat only a decision-worthy cell.** Run two further model-matched repetitions only after the reset and compare correctness first, then median wall time and uncached tokens. If either arm fails hidden grading, check grader fairness before spending additional Codex usage.
6. **Measure Standard Review on a defect it can catch.** The medium task passed cleanly on both arms, so its 71.3s Orteca overhead cannot distinguish safety value from unnecessary latency. Use a blinded Standard task with a seeded, all-public-tests-pass defect before retaining, changing, or removing that Review stage.
