# Documentation index

Current source and executable contracts own behavior. Accepted ADRs own
implemented decisions; proposed ADRs remain proposals. Active ledgers own unmet
acceptance. Historical tests, counts and status do not qualify current source.

## Start here

| Question | Owner |
| --- | --- |
| Remaining code, owners and execution order | [Oct-22 SSOT](plans/oct-22-finalizing/README.md) |
| Shared files and handoff | [Single-writer rules](plans/oct-22-finalizing/OWNERSHIP.md) |
| Original full-product DoD | [R01-R40 and final exit conditions](plans/oct-22-finalizing/DOD.md) |
| Executed results | [VALIDATION](plans/oct-22-finalizing/VALIDATION.md) |
| Implemented architecture / open proposals | [ADR registry](adr/README.md) |
| Current source entry points and typed limits | [Engine source map](ssot/engine-status-v1.md), [crate ownership](ssot/crate-ownership.md) |
| Historical RFC/tickets and accepted contracts | [Recovery index](ARCHIVE-INDEX.md#historical-record-recovery) and [ADRs](adr/README.md) |
| Test/CI/platform, semantic, installed/pair/actions | [Current acceptance work](plans/oct-22-finalizing/E08-integration-release.md); prior [test/platform](history/oct-22-finalizing/oct-10-index-closeout/VALIDATION.md#affected-checks), [semantic](history/oct-22-finalizing/oct-10-index-closeout/VALIDATION.md#semantic-compatibility), [release/proof](history/oct-22-finalizing/oct-10-index-closeout/VALIDATION.md#release-and-consumer) results |

One Oct-22 epic index owns remaining work. Epic documents hold fixed contracts;
VALIDATION keeps source-scoped results. Historical plans and handoff summaries
do not create another live queue. Superseded dirty/untracked bodies have a
verified recovery entry in ARCHIVE-INDEX.
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
remain recoverable; they are not another live queue. The Native RFC keeps its
[fixed requirement inventory](rfc/oct-10-native-incremental-vnext.md#23-fixed-full-product-implementation-and-acceptance-ledger);
its dated progress tables and execution checkpoints are preserved as
[section 23 history](history/oct-10-index-closeout/RFC-CHECKPOINTS-2026-10-21.md) and
[implementation history](history/oct-10-index-closeout/RFC-IMPLEMENTATION-CHECKPOINTS-2026-10-21.md). See
[documentation custody](adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).
