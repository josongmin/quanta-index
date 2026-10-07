# Code-search remaining work

Status: `ACTIVE — implementation and qualification remain separate`.

## Implementation constraint

Evolve the existing `LqQuery` and sealed-generation contracts in place. Do not
introduce a parallel CodeSearch IR, another full-file source authority, or
compatibility fallbacks for old generation formats. `CodeSearchPlan` is an
internal compiled executor derived from `LqQuery`, not another public query
model. Breaking wire or storage changes require a producer rebuild and refusal
of stale cursors. Keep historical benchmark captures immutable for audit under
their pinned producers; old schemas do not become live execution paths. Reuse the
existing admission, budget, source-verification, result-window and evaluator
owners. Remove superseded branches when their replacement is verified.

Completed L1–L5 contracts are consolidated in
[SEP-27-003](../../adr/SEP-27-003-code-search-source-and-preview-contract.md).
Completed capture/process/I/O decisions are consolidated in
[SEP-27-004](../../adr/SEP-27-004-benchmark-capture-and-resource-custody.md).
Historical tickets, handoffs, RCA and terminal snapshots have been removed;
exact bodies remain recoverable through the [plan archive](../../ARCHIVE-INDEX.md#historical-record-recovery).

| Active owner | Remaining scope |
| --- | --- |
| [CS-ENG-02](rfcs/CS-ENG-02-capability-publication-and-freshness.md) | Total coverage pipeline cost, uncached large-root decode, full verification scans and physical heap qualification |
| [CS-BENCH-01](rfcs/CS-BENCH-01-corpus-gold-and-holdout.md) | Independent source/gold releases and sealed holdout |
| [CS-BENCH-02](rfcs/CS-BENCH-02-native-response-validation.md) | Remaining native entrypoint/format refusal coverage and real captures; local cs/Sourcegraph/OpenGrok path/hit refusal is implemented |
| [CS-BENCH-03](rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md) | Task/metric/statistical admission, ranking/context experiments |
| [CS-BENCH-04](rfcs/CS-BENCH-04-comparators-performance-and-incremental.md) | Real comparator, update-cost and equal-work measurement |
| [CS-INT-01](rfcs/CS-INT-01-integration-and-qualification.md) | Combined-source, external producer/consumer and release qualification |

Common execution, process/custody, resource matrix, CI enrollment and measurement
acceptance have one owner in [MISC](../sep-27-misc/tickets/INDEX.md). The active
benchmark RFCs remain acceptance/design work; their presence does not establish
an admitted corpus, quality gain, speedup or default change. Source/tooling/test
identities, rather than old counts, determine the required execution.

Retain the implemented legal-plan/window, canonical file coverage/lineage,
exact lookup/federated grouping, semantic preview/provenance and parser fixes.
Reopen a completed code change only for a current reproduced failure. Resolve
structural gaps and enrollment before one serial integration boundary; admit
independent inputs and a supported host before qualified measurements.

[CS-ENG-04](../../adr/SEP-27-003-code-search-source-and-preview-contract.md#deferred-regex-allocation-cap-cs-eng-04) is ADR-owned conditional P3 and
deferred: current regex guards remain, but no exact request-wide allocation cap
or worker containment contract is selected. It is not an active implementation
or release blocker without a numerical requirement or measured regex-driven breach.

The dated research survey and completed/deferred proposal body are retired to
[history](../../ARCHIVE-INDEX.md#historical-record-recovery).
The active table above contains remaining work; permanent contracts are ADR-owned.
