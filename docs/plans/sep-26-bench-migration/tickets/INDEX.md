# SEP-26 Benchmark Migration

Status: `PARTIAL`. An accepted control-plane decision is not complete profile
execution or benchmark qualification.

## Authority

- [SEP-27-002](../../../adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md)
  owns the completed single-registry, Python CLI and typed-evidence decision.
- [SEP-26-003](../../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md)
  owns retrieval proof and qualification boundaries.
- [`registry.toml`](../../../../tools/benchmark/registry.toml) is the live
  benchmark family/profile/producer registry; `benchctl.py` is the sole current
  orchestration CLI. Registration does not prove a family can execute.
- [Current gaps](CURRENT-AUDIT.md) own unfinished implementation and execution;
  [TEST-PLAN](TEST-PLAN.md) owns required oracles and commands. Neither is a
  source-bound passing receipt.

## Implemented boundary

The root Cargo workspace contains benchmark-only packages; product crates do
not depend on the evidence protocol. Crate-local Criterion benches remain with
their owning product. The common evidence envelope retains native raw data and
separates diagnostic capture, domain verdict, quality and performance. Recorded
agent imports accept explicitly unauthenticated input only. Retrieval contract,
pair and lexical diagnostic adapters share corpus/evidence identity while their
scorers and metric denominators remain distinct. External corpora, gold, model
assets and run artifacts are outside the checkout.

These are implementation boundaries, not a claim that every registered profile
has a current complete run. The exact remaining work is in the current gaps.

## Ticket map

| Ticket | Current owner boundary |
|---|---|
| [BM-00](BM-00-inventory-and-authority.md) | Inventory and single-authority decision adopted; re-enumerate against source before cutover. |
| [BM-01](BM-01-workspace-boundaries.md) | Benchmark-only workspace boundary adopted; no layout move without measured need. |
| [BM-02](BM-02-evidence-protocol.md) | Typed protocol and immutable-run design adopted; current-source full custody/scale proof remains in the gaps. |
| [BM-03](BM-03-registry-and-cli.md) | Python CLI selected; Rust CLI replacement is `NO-GO` under SEP-27-002. Profile execution coverage remains partial. |
| [BM-04](BM-04-system-and-micro.md) | Actual complete micro/system capture and replay remain open. |
| [BM-05](BM-05-retrieval-bridge.md) | Recorded scorers and pair/contract bridges exist; live view-bound product pilot remains open. |
| [BM-06](BM-06-recorded-and-experiments.md) | Unauthenticated recorded import contract implemented; authenticated real-agent outcome is not claimed. |
| [BM-07](BM-07-ci-baselines-cutover.md) | Full cutover, fresh representative profiles and hosted qualification remain open. |

The old BM-03 decision and chronological audit are historical. Recover their
exact pre-compression bodies with
`git show 84c9331f:docs/plans/sep-26-bench-migration/tickets/<file>`.
The [plan history index](../../ARCHIVE-INDEX.md) records the retired decision.
Historical receipts apply only to their original source and inputs.
