# BM-06 — Recorded agent outcomes and experiments

Status: `IMPLEMENTED / VERIFIED_CONTRACT`, diagnostic unauthenticated imports only. Priority: P2. Depends on: BM-02/BM-03. Common gates: [TEST-PLAN](TEST-PLAN.md).

Current source follow-up: `tools/benchmark/recorded_capture.py` supplies explicit
external JSONL/native-JSON imports, complete-profile custody, existing-owner
metric recomputation and raw-derived replay. Authenticated claims are explicitly
refused; no authenticator or real agent-capture qualification is claimed.
See [CURRENT-AUDIT.md](CURRENT-AUDIT.md) for exact snapshot/receipt boundaries;
the historical closeout below does not qualify these newer changes.

Implementation/verification/qualification verdicts for this ticket are recorded in `git show eff53181:docs/plans/sep-26-bench-migration/tickets/CLOSEOUT.md` and, where relevant, [BM-00-INVENTORY.md](BM-00-INVENTORY.md), [BM-03-DECISION.md](BM-03-DECISION.md) and [BM-07-MIGRATION-MATRIX.md](BM-07-MIGRATION-MATRIX.md). The original `PLAN / NOT_RUN` status above is the plan-time state, not the closeout state.

## Purpose

Make recorded-only evidence visible and replayable in the same registry without pretending the common CLI generated agents, judged trajectories, or authenticated third-party recordings.

## Work

1. Register `tools/benchmark/agent_outcome` as a recorded evaluator with its frozen A/B/C task, arm, model, budget and test-result contract. Bind the exact input JSONL bytes and underlying trajectory/test receipts; keep the scorer's current domain metrics.
2. Register `scan-vs-index` and any other recorded experiments found by BM-00 as diagnostic-only unless a producer, independent oracle, baseline, and host policy are explicitly supplied.
3. Specify agent-outcome capture prerequisites: external task source, checkout commit, scaffold/model revisions, arm config, complete per-arm trajectories, raw baseline/post test executions and cost source. The evaluator must distinguish validated record shape from authenticated capture and qualified outcome.
4. Expose conditional time-to-first-useful-evidence alongside evidence coverage. Missing useful evidence is not zero milliseconds. A/B/C incomplete or changed test inventory refuses the pair.

## DoD

- `benchctl validate/replay` rejects duplicate/missing arm, mixed task/model/budget, forged test aggregate, incomplete trajectory, changed raw input or absent underlying receipt when authenticity is claimed. A fixture-only replay is labeled contract proof, not real agent-outcome proof. Existing evaluator totals match fresh recomputation on the same valid fixture.
- Summaries retain numerator/denominator, per-task pair and excluded/unknown count; a small self-selected sample yields no benchmark-wide significance claim.
- Registry labels recorded-only families as non-producing. `benchctl run` cannot manufacture a capture or upgrade a submitted summary to qualified truth.

## Verification / exclusions

Use existing agent-outcome fixtures plus adversarial mutation tests and one real recorded input when available. Running coding agents, producing new tasks, or approving human evidence is outside this migration ticket.
