# J7-01 — Scenario SSOT Normalization

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
