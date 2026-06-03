# ADV-04 Generated Proof Truth and Benchmark Bar

Parent packet: [../README.md](../README.md)

Status: `landed`

Current status:

- checker (not generator) is landed:
  `tools/ci/lint/check-dsl-capability-truth.py`
- checker unit rail is landed:
  `tools/ci/tests/test_check_dsl_capability_truth.py`
- benchmark/shadow template is frozen in [../README.md](../README.md) §10
- warm benchmark artifact and committed baseline are refreshed on the adopted
  command path:
  `artifacts/dsl-bench/warm-matrix.json`
  `tools/benchmark/baselines/warm-matrix.json`
- cold benchmark artifact and committed baseline are refreshed on the adopted
  command path:
  `artifacts/dsl-bench/cold-matrix.json`
  `tools/benchmark/baselines/cold-matrix.json`
- the current cold baseline is tied to the corrected harness measurement
  contract: readiness polling now reuses the first successful IPC response
  instead of discarding it and timing a second query
- cold benchmark gating now fail-closes measured rows with fewer than `20`
  samples, and the orchestrator prebuilds `dsl_cold_matrix` once instead of
  shelling through `cargo run` per sample

## Objective

Prevent widening from outrunning proof or docs by generating capability truth
from code-owned data where possible, and set the measurement bar required before
calling the widened surface “advanced”.

## Current Source Truth

- code-owned dump bins now exist for predicate subset truth and SG legality:
  `crates/quanta-index-lexical/src/bin/dump_predicate_capabilities.rs`
  `crates/quanta-index-search-plane/src/bin/dump_sg_structural_legality.rs`
- checker path now exists:
  `tools/ci/lint/check-dsl-capability-truth.py`
- structural improvement alone is not enough to justify an “advanced” claim
- warm benchmark path must remain runnable; the owning manifest/profile seams are
  `crates/quanta-index-searchd-runtime/Cargo.toml` and workspace `[profile.bench]`
- baseline refresh now tracks the current widened tree at git rev `c7e995f`
- older cold baselines that predate the first-success-response fix are not
  comparable as-if they measured the same first-query path; they undercount
  route-local initialization on the first successful response

## Current Code Pointers

- current proof inventory:
  `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
  `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`
- predicate capability source:
  `crates/quanta-index-lexical/src/predicate_registry.rs`
- SG legality source:
  `crates/quanta-index-search-plane/src/lowering/structural_matrix.rs`
- checker precedents:
  `tools/ci/lint/check-public-api.py`
  and `tools/ci/lint/check-cargo-modules-snapshot.py`
- cost/shadow proof rails:
  `crates/quanta-index-lq-norm/benches/pipeline.rs`,
  `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`,
  `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`,
  `crates/quanta-index-searchd-runtime/benches/dsl_query_matrix.rs`,
  `crates/quanta-index-searchd-harness/src/bin/dsl_cold_matrix.rs`,
  `tools/benchmark/run_dsl_cold_matrix.py`

## 핵심 로직

- code owns legality and capability
- docs consume generated or mechanically checked truth
- correctness proof and cost evidence are separate gates

## 건드릴 파일

- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`
- new generator/check tooling under `tools/` or another code-owned location
- widening tickets’ owner rails where benchmark/shadow selectors are defined

## 생성 가능 파일

- checker first:
  `tools/ci/lint/check-dsl-capability-truth.py`
- checker tests:
  `tools/ci/tests/test_check_dsl_capability_truth.py`
- optional generator if the checker proves too weak:
  `tools/ci/lint/generate-dsl-capability-truth.py`
- optional generated artifact only if the team commits machine output:
  `docs/plans/may-25-lexical-enhancement/generated/dsl-capability-truth.json`

## 건드리지 말 것

- runtime corpus schema unless widening truly needs it
- bridge carrier category split
- benchmark wording ahead of actual measured evidence

## TODO

- [x] choose generated vs mechanically checked truth for capability inventory
- [x] derive predicate subset and SG legality subset from code-owned metadata
- [x] refresh warm benchmark artifact on the adopted bench command path
- [x] make docs fail closed when widening code and capability truth diverge

## Concrete First Increment

The first PR for this ticket should do only this:

1. build the checker, not the generator
2. make it fail on predicate-subset drift and SG-legality drift
3. make it fail when a widening claim lacks a benchmark/shadow section

Do not start by generating docs if the repository still lacks a strong checker.

## Concrete Deliverables

The ticket is not done until it produces all of these:

1. one code-owned capability source for predicate subset truth
2. one code-owned capability source for SG legality truth
3. one deterministic checker at `tools/ci/lint/check-dsl-capability-truth.py`
4. one test file at `tools/ci/tests/test_check_dsl_capability_truth.py`
5. one optional generator only if the checker is not strong enough by itself
6. one benchmark/shadow checklist template that future widening tickets must fill
7. one reproducible warm benchmark artifact on the adopted command path

## Implementation Steps

1. red: write checker tests first for docs-vs-code drift and for missing
   benchmark/shadow sections on widening claims
2. refactor: extract the minimal code-owned metadata needed from `ADV-01` and
   `ADV-02` without making docs the source of truth
3. refactor: build a deterministic checker first; generation stays optional
4. proof/doc sync: add a widening claim template:
   - correctness proof
   - parity proof
   - chaos/restart proof if route-sensitive
   - shadow or benchmark evidence
5. proof/doc sync: wire the checker into the normal docs/lint workflow
6. benchmark: keep the warm lane runnable by excluding non-bench runtime bins
   from bench builds and pinning a dedicated workspace `[profile.bench]`

## Dependency / Import Constraints

- do not make docs the source of truth for widening capability
- do not require runtime test execution inside the generator itself
- keep benchmark/shadow recipes as verifiable commands, not prose-only promises
- keep tooling under `tools/ci/lint/` and `tools/ci/tests/`; do not hide the
  checker inside a docs folder
- do not let the generator silently heal drift that the checker should fail
- do not let bench command surfaces silently pull unrelated runtime bins into
  the warm lane

## Red Rails First

- checker rail:
  `python3 tools/ci/lint/check-dsl-capability-truth.py`
- checker unit rail:
  `python3 -m pytest tools/ci/tests/test_check_dsl_capability_truth.py -q`
- widening correctness rail that must stay green while the checker is added:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`
- benchmark/shadow gate:
  `./scripts/cargow --lane bench-lane bench -p quanta-index-searchd-runtime --bench dsl_query_matrix --all-features --locked`
  and
  `python3 tools/benchmark/run_dsl_cold_matrix.py`

## NOT TODO

- no cosmetic doc regeneration without semantic checks
- no “SOTA++” label without benchmark/shadow evidence
- no hidden manual override for widened capability truth

## Test Plan

- generator/check tool deterministic output test
- targeted widening rails from `ADV-01` through `ADV-03`
- benchmark/shadow recipe verification docs and command checks

## DoD

- capability truth is generated or mechanically checked from code-owned data
- widening docs cannot silently drift from the executable subset
- benchmark/shadow bar is explicit and repeatable
- advanced claim requires both correctness proof and cost evidence
- warm benchmark artifact is reproducible from the adopted bench command path
- warm/cold committed baselines are refreshed from the adopted command paths

## Failure Modes

- generated docs omit a widened surface
- docs continue to require manual sync and drift again
- benchmark bar is vague enough to become non-falsifiable
- checker is so weak that it passes while capability truth still drifts
