# Code-search remaining work

Status: `ACTIVE — implementation and qualification remain separate`.

Completed L1–L5 contracts are consolidated in
[SEP-27-003](../../adr/SEP-27-003-code-search-source-and-preview-contract.md).
Completed capture/process/I/O decisions are consolidated in
[SEP-27-004](../../adr/SEP-27-004-benchmark-capture-and-resource-custody.md).
Historical tickets, handoffs, RCA and terminal snapshots have been removed;
exact bodies remain recoverable through the [plan archive](../ARCHIVE-INDEX.md).

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

[CS-ENG-04](rfcs/CS-ENG-04-match-anchored-snippets.md) is conditional P3 and
deferred: current regex guards remain, but no exact request-wide allocation cap
or worker containment contract is selected. It is not an active implementation
or release blocker without a numerical requirement or measured regex-driven breach.

[Research references](references.md) are non-normative inputs to the open
benchmark design. Accepted product/qualification contracts remain in the ADRs.
