# J7-01 — Scenario SSOT Normalization

> Archive status: `Historical program record`. Current architecture: [JUN-08-001](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `landed`

Goal:

- keep `SCENARIOS` as query authority
- annotate it with fast-hellgate ownership metadata

Owner seam:

- `crates/quanta-index-searchd-harness/src/scenarios.rs`
- `crates/quanta-index-searchd-harness/tests/dsl_scenario_truth.rs`

Delivered:

- `HellgateLane`
- per-row verification metadata
- helper to iterate text vs structural fast subsets
