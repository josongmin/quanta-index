# Lexical capability and proof inventory

Status: `ACTIVE INVENTORY`.

Accepted query/runtime and Sourcegraph compatibility decisions live in
[JUN-02-001](../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md)
and [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md).
The completed closeout/change chronology is recoverable from
`1419f3087f4f09a6ecab4ef39c30a2bf32544d5d`.

- Human-readable capability inventory:
  [lexical-capability-matrix.md](lexical-capability-matrix.md).
- Machine-readable proof inventory: [dsl-proof-ledger.toml](dsl-proof-ledger.toml).
- Runtime fixture authority:
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
  and `tests/e2e_full_corpus.rs`.
- Companion public/transport/restart, fidelity/lowering and bridge rails are
  selected through the current test-authority catalog and Just profiles.

Keep active-runtime, active-owner-local, typed-fail-closed, parser-only and blocked
states distinct. Every claimed surface must have its appropriate owner/fixture;
old execution counts do not transfer to current source. The current executable
inventory and selected terminal results determine implementation/proof state.
Qualification gaps remain in their active execution owners, not this historical
closeout. Do not introduce a second capability registry.
