# Source Truth Map

> Archive status: `Historical program record`. Current architecture: [JUN-08-001](../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


## Scenario Authority

- `crates/quanta-index-searchd-harness/src/scenarios.rs`
- `crates/quanta-index-searchd-harness/src/bench_support.rs`
- `crates/quanta-index-searchd-harness/tests/dsl_scenario_truth.rs`

## Fast Hellgates

- text route:
  - `crates/quanta-index-searchd-runtime/tests/e2e_text_route_hellgate.rs`
- structural route:
  - `crates/quanta-index-searchd-runtime/tests/e2e_structural_hellgate.rs`
- structural direct lexical demotion witness:
  - `crates/quanta-index-search-plane/src/lowering.rs`

## Broad Daemon Rails

- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`

## Guard / Inventory

- `tools/benchmark/sourcegraph_parity.py`
- `tools/ci/lint/check-dsl-capability-truth.py`
- `tools/benchmark/README.md`

## Cross-Repo Ingress

- `semantica-codegraph-v2/.../tests/index_sdk_ingress_publish_contract_test.rs`
