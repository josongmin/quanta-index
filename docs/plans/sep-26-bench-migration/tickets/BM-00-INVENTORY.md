# BM-00 — Inventory and authority map (implementation receipt)

Status of this artifact: **implementation inventory, VERIFIED against the live
registry at implementation time**. It is not a benchmark result.

- Source state when frozen: `HEAD = 51d3d34253d5d8ec0a72e1550a3c5e71ca38d8ab`,
  shared checkout **dirty** (concurrent retrieval/RBR edits plus this packet).
  A concurrent worker committed to `main` during this session, so no clean-source
  claim is made anywhere in this packet.
- Machine authority: [`tools/benchmark/registry.toml`](../../../../tools/benchmark/registry.toml),
  validated by `tools/benchmark/registry.py` and mirrored by
  `tools/ci/lint/check-benchmark-policy.py`.
- Registry canonical digest at freeze:
  `sha256:f2066046b23aad113491157ce777f5fa25ef669a9326b939f3bfbfc7eae9291f`
  (`python3 tools/ci/lint/check-benchmark-policy.py --print-registry-digest`).

The table is a human-readable inventory, not an admission authority. Source
registration and policy tests validate `registry.toml`; they do not independently
verify this Markdown table. Current corrections below supersede the original
freeze digest. Use `benchctl list/plan` for machine-derived current registration.

## 1. Registered families

| family | purpose | payload | producer | validator | scorer | host policy | gate tier | floor | baseline | closure | authority |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `agent-outcome` | agent-outcome | agent_outcome | `recorded-input:agent-outcome` | `agent-outcome` | `agent-outcome` | any | contract | 0 | none | benchmark-control-plane | registry |
| `ambiguity` | search-quality | latency | `just-recipe:rust-verify-quality-ambiguity` | `bench-artifacts` | `quality-integration` | local-diagnostic | authority | 0 | none | benchmark-control-plane | registry |
| `ann` | systems | latency | `just-recipe:rust-verify-quality-ann` | `bench-artifacts` | `quality-integration` | local-diagnostic | authority | 64 | none | benchmark-control-plane | registry |
| `concurrency` | systems | load | `just-recipe:rust-verify-quality-concurrency` | `bench-artifacts` | `quality-integration` | local-diagnostic | authority | 16 | none | benchmark-control-plane | registry |
| `dsl-cold` | dsl-latency | latency | `just-recipe:rust-bench-dsl-cold` | `bench-artifacts` | `dsl-latency` | canonical-linux | authority | 20 | `tools/benchmark/baselines/cold-matrix.json` | benchmark-control-plane | registry |
| `dsl-warm` | dsl-latency | latency | `just-recipe:rust-bench-dsl-warm` | `bench-artifacts` | `dsl-latency` | canonical-linux | authority | 200 | `tools/benchmark/baselines/warm-matrix.json` | benchmark-control-plane | registry |
| `freshness` | systems | freshness | `just-recipe:rust-verify-quality-freshness` | `bench-artifacts` | `quality-integration` | local-diagnostic | authority | 20 | none | benchmark-control-plane | registry |
| `lexical-file-comparison` | retrieval | retrieval | `python-module:tools/benchmark/retrieval/lexical_file_comparison.py` | `lexical-file-diagnostic` | `lexical-file-diagnostic` | any | diagnostic | 0 | none | retrieval | registry |
| `micro-lq-norm-pipeline` | micro | micro | `cargo-bench:quanta-index-lq-norm:pipeline` | `evidence-protocol` | `none` | local-diagnostic | diagnostic | 0 | none | micro | registry |
| `micro-searchd-runtime-dsl-query-matrix` | micro | micro | `cargo-bench:quanta-index-searchd-runtime:dsl_query_matrix` | `evidence-protocol` | `none` | local-diagnostic | diagnostic | 0 | none | micro | registry |
| `open-loop` | systems | load | `just-recipe:rust-verify-quality-open-loop` | `bench-artifacts` | `quality-integration` | local-diagnostic | authority | 0 | none | benchmark-control-plane | registry |
| `ops` | systems | latency | `just-recipe:rust-verify-quality-ops` | `bench-artifacts` | `quality-integration` | local-diagnostic | diagnostic | 0 | none | benchmark-control-plane | registry |
| `relevance` | search-quality | latency | `just-recipe:rust-verify-quality-relevance` | `bench-artifacts` | `quality-integration` | any | authority | 0 | none | benchmark-control-plane | registry |
| `relevance-openai-ab` | search-quality | latency | `just-recipe:rust-capture-quality-relevance-openai-ab` | `bench-artifacts` | `quality-integration` | any | advisory | 0 | none | benchmark-control-plane | registry |
| `retrieval-contract` | retrieval | proof | `just-recipe:retrieval-contract-proof` | `retrieval-proof` | `none` | any | contract | 0 | none | retrieval | registry |
| `retrieval-pair` | retrieval | retrieval | `python-module:tools/benchmark/retrieval/run.py` | `retrieval-pair` | `retrieval-relevance` | any | diagnostic | 0 | none | retrieval | registry |
| `retrieval-sdk` | retrieval | proof | `just-recipe:retrieval-sdk-proof` | `retrieval-proof` | `none` | any | contract | 0 | none | retrieval | registry |
| `scale` | systems | latency | `just-recipe:rust-verify-quality-scale` | `bench-artifacts` | `quality-integration` | local-diagnostic | authority | 32 | none | benchmark-control-plane | registry |
| `scan-vs-index` | recorded-experiment | recorded_experiment | `python-module:tools/benchmark/run_scan_vs_index.py` | `evidence-protocol` | `none` | local-diagnostic | diagnostic | 0 | none | benchmark-control-plane | registry |
| `snippet` | search-quality | latency | `just-recipe:rust-verify-quality-snippet` | `bench-artifacts` | `quality-integration` | local-diagnostic | authority | 0 | none | benchmark-control-plane | registry |
| `tail` | systems | latency | `just-recipe:rust-verify-quality-tail` | `bench-artifacts` | `quality-integration` | local-diagnostic | authority | 64 | none | benchmark-control-plane | registry |
| `ui` | systems | latency | `just-recipe:rust-verify-quality-ui` | `bench-artifacts` | `quality-integration` | local-diagnostic | diagnostic | 0 | none | benchmark-control-plane | registry |

Profiles: `dsl-authority`(2), `dsl-diagnostic`(1), `micro`(2), `quality-core`(5),
`quality-full`(11), `recorded`(2), `retrieval-contract`(2), `retrieval-diagnostic`(1), `lexical-diagnostic`(1),
`semantic-ab`(1), `systems`(2).

## 2. Cargo bench targets

Discovered from `./scripts/cargow --lane metadata-lane metadata` (read-only) and
compared against the registry by `check-benchmark-policy.py`:

| package | target | family | registered |
| --- | --- | --- | --- |
| `quanta-index-lq-norm` | `pipeline` | `micro-lq-norm-pipeline` | yes |
| `quanta-index-searchd-runtime` | `dsl_query_matrix` | `micro-searchd-runtime-dsl-query-matrix` | yes |

There are **no unregistered bench targets and no phantom ones** — the policy
check enforces both directions (`test_every_cargo_bench_target_is_registered`).

Crate-local Criterion targets stay in their owning product crates. No file was
moved for the migration (see BM-01; `crates/quanta-index-searchd-harness/`
stays put and remains a runtime **dev-dependency** of
`quanta-index-searchd-runtime`).

## 3. Correctness-only owners, deliberately not counted as benchmark evidence

| Path | Role | Why it is not a timed result |
| --- | --- | --- |
| `tools/benchmark/sourcegraph_parity.py` | Generated Sourcegraph execution-coverage matrix | Correctness/parity inventory; no timing |
| `just rust-bench-dsl-truth` | Golden-truth smoke over the bench scenario table | Behaviour oracle, no timing assertions |
| `tools/benchmark/retrieval/portable_proof.py` | Source-bound JUnit/nextest receipts | Contract proof, not a metric |
| `tools/benchmark/retrieval/proof_inventory.py` | nextest inventory verification | Coverage proof |

## 4. CI invocation inventory

Before the cutover, `.github/workflows/correctness.yml` drove the DSL/system
families with direct `just <producer>` steps and two direct
`compare_dsl_bench.py` invocations, while `ci.yml` ran the artifact checker and
the Sourcegraph parity generator. That is the divergence BM-07 removes.

After the cutover:

| Workflow job | Step | Path |
| --- | --- | --- |
| `ci.yml: rust-policy` | `check-bench-artifacts.py` | artifact attribution (unchanged) |
| `ci.yml: rust-policy` | `check-benchmark-policy.py` | registry, dependency direction, no direct bypass |
| `ci.yml: rust-policy` | `sourcegraph_parity.py --check --write` | correctness parity (unchanged) |
| `ci.yml: rust-bench-build` | `cargow bench --no-run` | compile-only, explicitly not a measurement |
| `correctness.yml: dsl-bench-latency` | `benchctl run dsl-authority --cold-samples 20 --evidence-root ...` | one authority path |
| `correctness.yml: dsl-bench-latency` | `benchctl run systems --evidence-root ...` | one authority path |
| `correctness.yml: dsl-bench-latency` | `benchctl validate <profile> --evidence-root ...` | requires the promoted immutable runs |
| `correctness.yml: dsl-bench-latency` | `benchctl replay dsl-warm --evidence-root ...` | fresh-process replay of captured raw |

`tools/ci/lint/check-benchmark-policy.py` refuses any direct
`just <registered-producer>` or `compare_dsl_bench.py` step introduced later.

## 5. Baselines

`tools/benchmark/baselines/` does not exist in this checkout: **no DSL baseline
is admitted**. `dsl-warm` and `dsl-cold` therefore declare baseline paths that
must be produced by `benchctl run dsl-authority --admit-baseline` on the quiet
canonical Linux host. Until then the scheduled CI job fails typed, which is the
existing and intended behaviour. No `latest` file or historical receipt is
accepted as a baseline by filename: baselines are immutable run ids plus
digests (`benchmarks/bench-protocol` `BaselineRecord`).

## 6. Source closure: normative vs excluded

New profile `benchmark-control-plane` in `tools/ci/source_closure.py` binds:

- **Cargo package**: `quanta-index-bench-protocol` (the typed evidence contract
  and its tests).
- **Config/toolchain**: `.cargo/config.toml`, `Cargo.toml`, `Cargo.lock`,
  `Justfile`, `pyproject.toml`, `rust-toolchain.toml`, `scripts/cargow`.
- **Register/CLI/evidence**: `benchmarks/bench-protocol`, `tools/benchmark/registry.toml`,
  `registry.py`, `manifest.py`, `evidence.py`, `evidence.schema.json`, `evidence_bridge.py`,
  `benchctl.py`, `compare_dsl_bench.py`, `quality_integration_summary.py`.
- **Policy/verification owners**: `tools/ci/lint/check-bench-artifacts.py`,
  `tools/ci/lint/check-benchmark-policy.py`, `tools/ci/source_closure.py`,
  `tools/ci/timing/check_host_contention.py`.
- **Owning tests**: `test_bench_protocol_conformance.py`,
  `test_benchmark_evidence_bridge.py`, `test_benchmark_manifest.py`,
  `test_benchmark_policy.py`, `test_benchmark_source_closure.py`,
  `test_benchctl.py`.

**Explicit exclusions** (with reason):

- `docs/plans/sep-26-bench-migration/**` — planning/history. A status edit is
  not a contract change and must not invalidate a product proof. Enforced by
  `test_planning_history_is_excluded_from_the_normative_closure`.
- Product `crates/**` source — bound through the envelope's Git revision and the
  measured binary digests, and through the existing `retrieval` profile for the
  SDK rail, rather than by pulling the whole product tree into every benchmark
  proof.
- External corpus, gold, model assets and raw captures — outside Git, bound by
  digest at replay time (`InputReference`).

**Known consequence**: the `retrieval` closure already binds
`tools/ci/source_closure.py` itself, so adding a profile changes its digest and
invalidates previously issued retrieval closure receipts. That is the intended
fail-closed behaviour of a source-bound proof, not a relaxation; those receipts
must be re-issued from a clean exact-source snapshot.

## 7. Duplicate and retired paths

| Item | Disposition |
| --- | --- |
| `tools/benchmark/manifest.json` | **removed**; `manifest.py` is now a read-only projection of `registry.toml` and keeps a single data authority |
| `tools/benchmark/manifest.py` parallel tables | replaced by the registry projection (`test_artifact_projection_matches_the_registry`) |
| `BenchArtifactV1` universal latency-row envelope | retained as the **native** raw artifact format; it is no longer the common envelope |
| Retrieval / agent-outcome entrypoints | registered; their existing scorers keep metric ownership |
| `dsl-warm-criterion` exploratory view | registered as `diagnostic` with no scorer and no baseline |

## 8. Authority rule

One current authority per family, `authority = "registry"` for all 23 families:
the family is discovered, planned, run, validated, promoted and replayed through
`python3 tools/benchmark/benchctl.py`. Producers, validators and scorers stay
the named owners; the CLI never reimplements their metric mathematics. No family
is left in a `legacy` state — `check-benchmark-policy.py` refuses one.
