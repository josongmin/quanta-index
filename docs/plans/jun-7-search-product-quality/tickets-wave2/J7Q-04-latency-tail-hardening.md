# J7Q-04 — Admitted route-tail and offered-load evidence

Status: `ACTIVE_RESIDUAL`. Parent: [quality index](INDEX.md).
Owner: existing `tail_matrix`/`open_loop_matrix`, comparator and admitted host.
Implemented gates are in
[JUN-08-001](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md)
and [OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

## Remaining acceptance

- Obtain actual admitted cold/warm artifacts and reviewed baselines on the quiet
  canonical Linux host, bound to complete samples/source/config/scenarios.
  Execute current release offered-load tiers through the existing seeded Poisson
  scheduler independently of completion; zero offered arrivals cannot qualify.
- Measure lexical/symbol/structural/history/runtime-catalog separately. Preserve
  arrival-to-completion, service time, queue delay, served/errors/timeouts/drops
  and finite complete accounting. Validate every response after its timer against
  independent scenario/source truth, including the declared parse-failure case.
- Keep ordered candidate/source-repository/path identities and load-point source
  digest/arrival settings. Failed runs retain typed stage/refusal and missing
  latency. Query timeout does not extend seal transport timeout; open-loop keeps
  eight history generations and 16 MiB unless an explicit bounded profile says so.
- Justify new blocking budgets from representative route variance. Existing DSL
  p50/p95 blockers, advisory p99 and advisory local tail budgets remain distinct.
  Preserve candidate-count, regex-complexity, repo-fanout and accept-loop tail
  diagnostics; a p50 improvement cannot hide route/p95 regression.

Outputs: registered `tail_matrix` summary/route budgets and `open_loop_matrix`
offered-load results/refusals. [J7Q-03](J7Q-03-large-corpus-scale-tiers.md) retains
source tier/resource/lifecycle acceptance; [B07](../../sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md)
and [OCT-04 E4-05/06](../../oct-4-parallel-closure/tickets/INDEX.md#e4) own actual
measurement and host admission. Existing functional tests and release SDK proof
do not close these runs. Exact older commands/results are recoverable through
[the history index](../../../ARCHIVE-INDEX.md#historical-record-recovery).
