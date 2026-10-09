# Documentation index

Current source and executable contracts own behavior. Accepted ADRs own
implemented decisions; proposed ADRs remain proposals. Active ledgers own unmet
acceptance. Historical tests, counts and status do not qualify current source.

## Start here

| Question | Owner |
| --- | --- |
| Remaining Index code | [Index closeout plan](plans/oct-10-index-closeout/README.md) |
| Owners and execution order | [Owner map](plans/oct-10-index-closeout/README.md#ownership), [execution order](plans/oct-10-index-closeout/README.md#execution-order) |
| Implemented architecture / open proposals | [ADR registry](adr/README.md) |
| Current source entry points and typed limits | [Engine source map](ssot/engine-status-v1.md), [crate ownership](ssot/crate-ownership.md) |
| Historical RFC/tickets and accepted contracts | [Recovery index](ARCHIVE-INDEX.md#historical-record-recovery) and [ADRs](adr/README.md) |
| Test/CI/platform, semantic, installed/pair/actions | [Test/platform](plans/oct-10-index-closeout/VALIDATION.md#affected-checks), [semantic](plans/oct-10-index-closeout/VALIDATION.md#semantic-compatibility), [release/proof](plans/oct-10-index-closeout/VALIDATION.md#release-and-consumer) |

The closeout plan retains three Index implementation workstreams; its validation
document separates required inputs and execution from code work. Old plan/ticket
bodies and duplicate handoff summaries are retired; necessary background is in
the new plan and stable contracts remain in existing ADRs.
Accepted contracts, current usage, generated capabilities and the reusable
[purpose audit inventory](reference/purpose-audit-inventory.md)
are references, not another uncompleted feature queue. Proposed ADRs remain
conditional designs; their age or presence does not authorize implementation.

## Usage and references

- [SDK](../crates/quanta-index-sdk/README.md), [CLI](../crates/quanta-index-searchctl/README.md), [build/verification](../README.md#build-and-verification).
- [Benchmark commands](../tools/benchmark/README.md), [code-search report/runbook](../tools/benchmark/CODE_SEARCH_RUNBOOK.md), [retrieval guide](../tools/benchmark/retrieval/README.md).
- [DSL capabilities](reference/dsl-capabilities.md), [proof inventory](reference/dsl-proof-inventory.md), [Sourcegraph filter coverage](reference/sourcegraph-filter-parity.md).
- [State backup/restore/rebuild](operator/state-cutover-runbook.md), [embedding setup](potion-code-embedder.md).

## History

[One recovery index](ARCHIVE-INDEX.md#historical-record-recovery) records pre-edit/deletion revisions and
recovery commands for documents and tickets. Historical bodies
remain recoverable; they are not another live queue. See
[documentation custody](adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).
