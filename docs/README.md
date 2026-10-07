# Documentation index

Current source and executable contracts own behavior. Accepted ADRs own
implemented decisions; proposed ADRs remain proposals. Active ledgers own unmet
acceptance. Historical tests, counts and status do not qualify current source.

## Start here

| Question | Owner |
| --- | --- |
| Remaining work, code/input/verification distinction | [Single residual ledger](plans/oct-4-parallel-closure/tickets/INDEX.md) |
| Owners and execution order | [Owner map](plans/oct-4-parallel-closure/tickets/INDEX.md#owners), [execution order](plans/oct-4-parallel-closure/tickets/INDEX.md#실행-순서) |
| Implemented architecture / open proposals | [ADR registry](adr/README.md) |
| Current source entry points and typed limits | [Engine source map](ssot/engine-status-v1.md), [crate ownership](ssot/crate-ownership.md) |
| Previous RFC/ticket IDs and acceptance | [Scope routes](plans/oct-4-parallel-closure/tickets/INDEX.md#legacy-scope-routes) and linked ADRs |
| Test/CI/platform, semantic, installed/pair/actions | [Test/platform](plans/oct-4-parallel-closure/tickets/INDEX.md#test-and-platform), [semantic](plans/oct-4-parallel-closure/tickets/INDEX.md#semantic), [release/proof](plans/oct-4-parallel-closure/tickets/INDEX.md#release-and-proof) |

The single ledger retains unfinished implementation, input/decision and
qualification conditions. Dated RFC/plan/ticket packets and duplicate handoff
summaries are retired; stable contracts are consolidated in existing ADRs.
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
