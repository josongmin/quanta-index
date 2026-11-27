# Documentation index

Current source and executable contracts own behavior. Accepted ADRs own
implemented decisions; proposed ADRs remain proposals. Active ledgers own unmet
acceptance. Historical tests, counts and status do not qualify current source.

## Start here

| Question | Owner |
| --- | --- |
| Remaining code, owners and execution order | [Oct-22 epics](plans/oct-22-finalizing/README.md) |
| Installed DELTA follow-up and C0 execution | [B05 current C0/P09 acceptance](plans/nov-12-bugbash-c0-p09/tickets/B05-final-verification.md) |
| Native indexing performance RCA and new code packets | [Nov-5 bugbash SSOT](plans/nov-5-indexing-bugbash/README.md) |
| Native-only cutover audit, execution order and acceptance | [Nov-12 cutover plan](plans/nov-12-native-cutover/README.md) |
| Shared files and handoff | [Single-writer rules](plans/oct-22-finalizing/OWNERSHIP.md) |
| Original full-product DoD | [R01-R40 and final exit conditions](plans/oct-22-finalizing/DOD.md) |
| Executed results | [VALIDATION](plans/oct-22-finalizing/VALIDATION.md) |
| Implemented architecture / open proposals | [ADR registry](adr/README.md) |
| Current source entry points and typed limits | [Engine source map](ssot/engine-status-v1.md), [crate ownership](ssot/crate-ownership.md) |
| Historical RFC/tickets and accepted contracts | [Recovery index](ARCHIVE-INDEX.md#historical-record-recovery) and [ADRs](adr/README.md) |
| Test/CI/platform, semantic, installed/pair/actions | [Current acceptance work](plans/oct-22-finalizing/E08-integration-release.md) and [original execution records](ARCHIVE-INDEX.md#oct-22-validation-and-w2-preimages) |

The Oct-22 epic index owns full-product scope. Its Native indexing performance RCA
and new code packets are delegated to the Nov-5 bugbash SSOT. Epic documents hold fixed contracts;
VALIDATION keeps source-scoped results. Historical plans and handoff summaries
do not create another live queue. The Oct-10/13/17 forwarding pages are retired;
current links go directly to the relevant epic or contract, and historical links
go to the preserved source and result records.
The [November closeout](ARCHIVE-INDEX.md#2026-11-07-november-closeout),
[publication/Sidecar closeout](ARCHIVE-INDEX.md#2026-11-12-publication-and-sidecar-closeout)
and [semantic plan consolidation](ARCHIVE-INDEX.md#2026-11-12-semantic-plan-consolidation)
map implemented decisions, retired packets and remaining acceptance.
Accepted contracts, current usage, generated capabilities and the reusable
[purpose audit inventory](reference/purpose-audit-inventory.md)
are references, not another uncompleted feature queue. Proposed ADRs remain
conditional designs; their age or presence does not authorize implementation.

## Usage and references

- [SDK](../crates/quanta-index-sdk/README.md), [CLI](../crates/quanta-index-searchctl/README.md), [build/verification](../README.md#development-and-verification).
- [Benchmark commands](../tools/benchmark/README.md), [code-search report/runbook](../tools/benchmark/CODE_SEARCH_RUNBOOK.md), [retrieval guide](../tools/benchmark/retrieval/README.md).
- [DSL capabilities](reference/dsl-capabilities.md), [proof inventory](reference/dsl-proof-inventory.md), [Sourcegraph filter coverage](reference/sourcegraph-filter-parity.md).
- [State backup/restore/rebuild](operator/state-cutover-runbook.md), [embedding setup](potion-code-embedder.md).

## History

The [recovery index](ARCHIVE-INDEX.md#historical-record-recovery) records exact
preimages and recovery commands, including the [October RFC checkpoints](ARCHIVE-INDEX.md#oct-10-rfc-and-lexical-preimages).
Use the Native RFC's [fixed requirement inventory](rfc/oct-10-native-incremental-vnext.md#23-fixed-full-product-implementation-and-acceptance-ledger)
and the [documentation custody ADR](adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).
