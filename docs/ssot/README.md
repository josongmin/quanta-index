# quanta-index documentation index

This directory is the living status index. Completed and superseded plans are
recoverable from Git history through the archive indexes. Accepted ADRs own
decisions; proposed ADRs are not implementation authority. Qualification
residuals do not establish missing engine implementations. Inspect
[engine status](engine-status-v1.md) and current source for code behavior.

| Question | Owner |
| --- | --- |
| Live search engine path | [engine-status-v1.md](engine-status-v1.md), `search-plane` / `searchd` / `sdk` source |
| Workspace crate ownership and entry points | [crate-ownership.md](crate-ownership.md) |
| `expect` production reachability audit | [expect-reachability.md](expect-reachability.md) |
| Host/release evidence still open | [CURRENT-RESIDUAL](../plans/sep-21-search-plane-sota-hardening/tickets/CURRENT-RESIDUAL-2026-09-26.md) |
| CI provider and release coverage | [QIT-09](../plans/jul-15-sota-test-hardening/tickets/QIT-09-circleci-provider-coverage.md) and [release evidence](../plans/sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md) |
| DSL and Sourcegraph support | [DSL capabilities](../reference/dsl-capabilities.md) and [filter parity](../reference/sourcegraph-filter-parity.md) |
| Benchmark acceptance | [code-search trust tickets](../plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md) |
| Open architecture proposals | [ADR index](../adr/README.md) (Proposed section) |
| Operator backup/restore | [state-cutover-runbook](../operator/state-cutover-runbook.md) |
| CLI | [searchctl README](../../crates/quanta-index-searchctl/README.md) |

Accepted decisions remain in the [ADR index](../adr/README.md). Verification
claims require the current source and the appropriate local or hosted gate;
this index is a route to owners, not an evidence receipt.
