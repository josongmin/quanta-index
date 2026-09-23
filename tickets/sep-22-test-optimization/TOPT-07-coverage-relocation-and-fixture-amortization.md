# TOPT-07 — Coverage Relocation and Immutable Fixture Amortization

Status: `code-landed; qualification pending`

Depends on: TOPT-05, TOPT-06

Findings: R5, TH-4

Aligned S21 owners: S21-03, S21-13

## Goal

Remove duplicated heavy execution without reducing authoritative coverage or
introducing cross-test shared mutable state.

## Resource-envelope relocation

Keep:

- core at-bound/one-over record, text, and vector policy matrix;
- search-plane preflight/publish zero-mutation and accounting proof;
- config field-to-policy mapping proof;
- one daemon E2E proving typed refusal leaves no generation and an admitted
  batch seals/serves.

Delete only `the_record_ceiling_counts_every_carried_row` after the four owners
above pass on the same source. Do not delete lower-layer boundaries or weaken
the retained daemon wiring oracle.

## Filter fixture amortization

- group read-only cases by immutable fixture family;
- one test boots/ingests/seals each family, then executes table rows with a
  named case, query, and exact expected result/error;
- mutation, activation, restart, and isolation-sensitive cases remain separate;
- no `OnceLock<E2eRuntime>`, process-global state, or cross-test ordering;
- a row failure reports its case identity and does not skip later cleanup.

## Measurement

For the focused filter target, record before/after:

- selected/executed case count;
- runtime boot/ingest/seal count by fixture family;
- warm median/p95 under the TOPT-00 protocol;
- identical semantic assertion inventory.

Success requires fewer heavy fixture constructions. A faster contended sample
without equal assertions is not evidence.

## Verification

- `./scripts/cargow test -p quanta-index-core --test ingest_resource_policy`
- `./scripts/cargow test -p quanta-index-search-plane --lib ingest_dispatcher::tests::search_corpus`
- `./scripts/cargow nextest run -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -E 'test(/^e2e_ingest_resource_envelope::/)'`
- `./scripts/cargow nextest run -p quanta-index-searchd-runtime --test runtime_risk_suite --all-features --locked -E 'test(/^e2e_filter_execution::/)'`
- `just rust-profile test-daemon`
- `just rust-profile test-daemon-all`

## Done

The redundant daemon case is removed only after coverage conservation is
proved, filter fixtures are amortized inside test boundaries, and before/after
receipts show equal assertions with fewer heavy setups.
