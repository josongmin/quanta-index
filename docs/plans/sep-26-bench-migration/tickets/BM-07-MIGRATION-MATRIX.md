# BM-07 — Migration matrix and cutover state

One row per registered family. "Cutover" means **the registry CLI is the live
authority for that family**; the old control plane is no longer current for it.
Nothing here is a benchmark result — the right-hand columns are implementation
and contract receipts only.

Common receipt for all rows (dirty-checkout implementation evidence, `HEAD =
51d3d34253d5d8ec0a72e1550a3c5e71ca38d8ab`):

- `python3 tools/ci/lint/check-benchmark-policy.py --print-registry-digest`
  → `benchmark control-plane policy ok`, registry digest
  `sha256:f2066046b23aad113491157ce777f5fa25ef669a9326b939f3bfbfc7eae9291f`
- `uv run --frozen --extra dev python -m pytest <15-file benchmark control-plane suite>`
  → **271 passed, 0 failed** (`test_benchmark_manifest`, `test_benchmark_policy`,
  `test_bench_protocol_conformance`, `test_benchmark_evidence_bridge`,
  `test_benchmark_source_closure`, `test_benchctl`, `test_check_bench_artifacts`,
  `test_check_host_contention`, `test_compare_dsl_bench`,
  `test_quality_integration_summary`, `test_retrieval_contract_proof`,
  `test_retrieval_sdk_proof`, `test_write_verification_receipt`,
  `test_agent_outcome_benchmark`, `test_validate_agent_output`)
- `./scripts/cargow --lane bench-lane test -p quanta-index-bench-protocol` → 46 passed
- `python3 tools/benchmark/benchctl.py list` → all 10 registered profiles plus the registry digest

## 1. Family cutover

| family | old current path | new registration | legacy reader | raw parity | cutover state |
| --- | --- | --- | --- | --- | --- |
| `dsl-warm` | `manifest.json` family + direct `just rust-bench-dsl-warm` in CI + direct `compare_dsl_bench.py` | `benchctl run dsl-authority` → `just rust-bench-dsl-warm`; comparator `dsl-latency` (`compare_dsl_bench.py`) | native `BenchArtifactV1` schema-2 reader retained; `manifest.json` reader removed | native rows copied verbatim; payload ms/sample/error counts asserted equal (`test_latency_payload_preserves_rows_and_sums_failures`) | **cut over** (CI drives `benchctl`; policy refuses a direct step) |
| `dsl-cold` | as above | `benchctl run dsl-authority --cold-samples 20` | as above | as above | **cut over** |
| `freshness` | `manifest.json` family + direct `just rust-verify-quality-freshness` | `benchctl run systems`; validator `bench-artifacts` | `BenchArtifactV1` reader retained | native rows copied verbatim; freshness payload keeps phases distinct | **cut over** |
| `open-loop` | `manifest.json` family + direct `just rust-verify-quality-open-loop` | `benchctl run systems` | `BenchArtifactV1` reader retained | open-loop points keep `offered_rate`; closed-loop points may not claim one | **cut over** |
| `relevance`, `ambiguity`, `snippet`, `scale`, `tail`, `ann`, `ops`, `ui` | `manifest.json` families + `just rust-verify-quality-*`; `rust-verify-quality-all` already delegated to `benchctl` | `benchctl run quality-core` / `quality-full`; scorer `quality-integration` | `BenchArtifactV1` reader retained | native rows copied verbatim | **registry authority** (aggregate rail delegated before this packet; registry replaces the parallel table) |
| `concurrency` | `manifest.json` family | `benchctl run quality-full`; payload `load` (closed loop) | `BenchArtifactV1` reader retained | closed-loop throughput cannot be reported as offered capacity | **registry authority** |
| `relevance-openai-ab` | `manifest.json` family (advisory) | `benchctl run semantic-ab` (needs `OPENAI_API_KEY`) | `BenchArtifactV1` reader retained | n/a (advisory capture) | **registered, advisory** |
| `dsl-warm-criterion` | undeclared exploratory recipe | family `dsl-warm-criterion`, profile `dsl-diagnostic`, payload `micro`, no scorer | raw Criterion output | none — diagnostic only, never a latency gate | **newly registered** |
| `micro-lq-norm-pipeline` | crate-local Criterion target, unregistered as evidence | family + profile `micro`, producer `cargo-bench`, validator `evidence-protocol` | raw Criterion output | `micro_payload_from_criterion` keeps `ns`/`wall` vs `instructions` separate | **registered**; `ci.yml: rust-bench-build` remains compile-only and is labelled non-measurement |
| `micro-searchd-runtime-dsl-query-matrix` | as above | as above | as above | as above | **registered** |
| `retrieval-sdk` | `just retrieval-sdk-proof` + `portable_proof.py` | family `retrieval-sdk`, profile `retrieval-contract`; validator `retrieval-sdk`; scorer `retrieval-relevance` | retrieval runner v3/v4/v5 readers retained read-only | existing scorer unchanged; `RetrievalPayload` requires an explicit lane and metric space | **registered, rail unchanged** |
| `retrieval-contract` | `just retrieval-contract-proof` | family `retrieval-contract`; validator `retrieval-contract` | as above | n/a (contract receipt) | **registered, rail unchanged** |
| `retrieval-pair`, `lexical-file-comparison` | `run.py pair` + `lexical_file_comparison.py` | profile `retrieval-diagnostic`; scorers `retrieval-relevance` / `lexical-file-diagnostic` | as above | file rank metrics stay file-space; span judgments require `judged`/`pooled` and can never be `mechanically_labeled` | **registered, diagnostic** |
| `scan-vs-index` | `run_scan_vs_index.py`, `manifest.json` family | profile `recorded`, payload `recorded_experiment`, `diagnostic_only` enforced | `BenchArtifactV1` reader retained | recorded points only; promotion of a non-diagnostic recorded payload is refused | **registered, diagnostic** |
| `agent-outcome` | `tools/benchmark/agent_outcome/__main__.py validate/summarize` (separate entrypoint) | family `agent-outcome`, producer `none` (recorded-only), validator/scorer `agent-outcome` | JSONL row contract v1 retained | payload requires exactly arms `[A,B,C]`, an input digest and an explicit `capture` authenticity label | **registered, non-producing** |

## 2. Removed / retired

| Item | State |
| --- | --- |
| `tools/benchmark/manifest.json` | deleted; `manifest.py` is a projection of `registry.toml` |
| Direct CI `just` producer steps and `compare_dsl_bench.py` steps in `correctness.yml` | removed; policy now refuses them |
| `latest/` directory semantics as authority | never admitted as a baseline; `benchmarks/bench-protocol` treats `latest` as an advisory pointer only |
| Parallel family tables in `manifest.py` | removed (registry is the only data source) |

Historical artifacts, committed baselines and user-owned external corpora were
not deleted or rewritten.

## 3. External inputs still required (not synthesized)

| Input | Owner | Status |
| --- | --- | --- |
| Quiet canonical Linux bench host + admitted DSL warm/cold baselines | platform + repo owner | **absent** → DSL performance qualification `BLOCKED` |
| External corpus release / gold (RBR-12) | RBR-12 | **absent** → retrieval quality qualification `BLOCKED` |
| Recorded agent A/B/C trajectories + test receipts | BM-06 owner | **absent** → agent-outcome qualification `NOT_RUN` |
| Open-loop capacity threshold on the pinned host | repo owner | **absent** → capacity verdict `NOT_RUN` |
