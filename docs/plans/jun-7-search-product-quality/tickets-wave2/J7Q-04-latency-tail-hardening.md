# J7Q-04 — Admitted route-tail evidence

Status: `ACTIVE_RESIDUAL`
Parent: [quality index](INDEX.md)
Owner: benchmark harness, comparator and canonical performance host

DSL p50 and p95 blocking rules are already implemented;
[JUN-08-001](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md)
owns them. `tail.rs` emits route budgets and correctness-gated representative
queries; its local latency budgets remain advisory. p99 is advisory in the DSL
comparator. These policies must not be collapsed into one tail verdict.

## Remaining acceptance

- Obtain admitted warm/cold artifacts and reviewed baselines on the quiet
  canonical Linux host; preserve complete sample, source, host and scenario
  bindings. Performance is not inferred from a successful local correctness rail.
- Measure lexical/symbol/structural/history/runtime-catalog families separately.
  Justify any additional p95/p99 blocker from representative route budgets and
  variance; keep advisory measurements labeled.
- Retain enough native diagnostics for candidate-count, regex-complexity,
  repo-fanout and accept-loop tail cliffs; a p50 improvement cannot hide a tail
  regression or substitute for a missing route.

Output owner: registered `tail_matrix`, with `summary.json` and
`route_budgets.json`. Fresh measurement and host admission remain required;
existing command/schema/threshold implementation is not an open coding task.
