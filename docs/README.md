# Documentation index

Current source and executable contracts own behavior. Accepted ADRs own
implemented decisions; proposed ADRs remain proposals. Active ledgers own unmet
acceptance. Historical tests, counts and status do not qualify current source.

## Start here

| Question | Owner |
| --- | --- |
| Remaining OCT-04 work, code/input/verification distinction | [Residual ledger](plans/oct-4-parallel-closure/tickets/INDEX.md) |
| Owners, handoffs and execution waves | [Owner map](plans/oct-4-parallel-closure/README.md), [waves](plans/oct-4-parallel-closure/WAVES.md) |
| Implemented architecture / open proposals | [ADR registry](adr/README.md) |
| Current source entry points and typed limits | [Engine source map](ssot/engine-status-v1.md), [crate ownership](ssot/crate-ownership.md) |
| Code-search engine/integration acceptance | [Remediation owners](plans/sep-27-code-search-remediation/readme.md) |
| Benchmark B01–B09 acceptance | [Benchmark index](plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md) |
| Shared execution/CI/measurement obligations | [MISC](plans/sep-27-misc/tickets/INDEX.md) |
| Installed, paired, operational and release scopes | [SEP-21 residuals](plans/sep-21-search-plane-sota-hardening/tickets/CURRENT-RESIDUAL-2026-09-26.md) |
| Semantic ownership / relevance / scale / tail | [Semantic residuals](plans/may-25-search-owned-semantic-derivation/README.md), [J7Q acceptance](plans/jun-7-search-product-quality/tickets-wave2/INDEX.md) |
| Test hardening and actual CI provider coverage | [QIT board](plans/jul-15-sota-test-hardening/tickets/00-ticket-status-board.md), [QIT-09](plans/jul-15-sota-test-hardening/tickets/QIT-09-circleci-provider-coverage.md) |

Plans above retain unfinished implementation, input/decision and qualification
conditions. Completed histories and duplicate handoff summaries are retired.
Accepted contracts, current usage, generated capabilities and the reusable
[purpose audit inventory](analysis/quanta-index-purpose-validation-checklist.md)
are references, not another uncompleted feature queue. Proposed ADRs remain
conditional designs; their age or presence does not authorize implementation.

## Usage and references

- [SDK](../crates/quanta-index-sdk/README.md), [CLI](../crates/quanta-index-searchctl/README.md), [build/verification](../README.md#build-and-verification).
- [Benchmark commands](../tools/benchmark/README.md), [code-search report/runbook](../tools/benchmark/CODE_SEARCH_RUNBOOK.md), [retrieval guide](../tools/benchmark/retrieval/README.md).
- [DSL capabilities](reference/dsl-capabilities.md), [Sourcegraph filter coverage](reference/sourcegraph-filter-parity.md).
- [State backup/restore/rebuild](operator/state-cutover-runbook.md), [embedding setup](potion-code-embedder.md).

## History

[Documentation archive](ARCHIVE-INDEX.md) and [plan archive](plans/ARCHIVE-INDEX.md)
record exact pre-edit/deletion revisions and recovery commands. Historical bodies
remain recoverable; they are not another live queue. See
[documentation custody](adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).
