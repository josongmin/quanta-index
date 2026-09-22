# RB-06 — Registered Entry Point and Source-Bound Closeout

Status: `planned`

Depends on: RB-05

Owner: Just/CI/test authority and final evidence inventory

## Goal

Make the benchmark easy to run without turning a short fixture test into a search-quality claim or adding a mandatory Semble/model download to every CI job.

## Work

1. Add named `Justfile` commands for cheap preparation/contracts and explicit real-repo capture. Reuse the `./scripts/cargow` build front door. Keep `benchctl`; register a new retrieval profile in `tools/benchmark/manifest.json` only when the profile has a frozen suite/query-pack digest, a pinned runner command with artifact/host/source controls actually wired and tested, and a declared claim scope (pilot exploratory-only vs qualified). Do not overload `quality-full` with an external real-repo comparison, and do not register a profile whose Semble/model/host inputs are still unpinned.
2. Register owner-local Rust/Python tests in `tools/ci/test-authority.toml` where required, and update the benchmark prep recipe to include the new contract tests. Keep full opponent measurements opt-in or on a dedicated controlled host; CI should check schemas, strategies and a tiny SDK roundtrip without downloading Semble/model assets.
3. Run a pinned exploratory-only pilot (20-query floor proves machinery, not superiority), inspect every excluded query/file, and rerun after correcting any contract bug. Then scale across supported categories/languages with a held-out suite. Refuse a single global comparison if support differs materially; report strata and coverage.
4. Validate final clean source, quiet-host profile with check-record entries, exact binary/model revisions, both raw records, path-mapping proof artifact, suite hashes, mandatory manifest fields (tokenizer/budget version, Semble lockfile digest, path+SHA diff digest), and report. Distinguish implementation/test pass from qualified quality/speed evidence and from production activation.
5. Maintain [TEST-PLAN.md](TEST-PLAN.md) T00–T16 as the required coverage matrix. Map every blocking ID to a test target, command and artifact; stop expensive E2E work after a cheap blocking schema/source/model failure. Do not emit a success summary with `NOT_RUN` IDs. T15 is mapped only to the same-model control claim; T16 only to an incremental claim — otherwise record them as not-applicable in `verdict.json`, not as failures.

## Planned files

- `Justfile`
- `tools/ci/test-authority.toml`
- `tools/benchmark/manifest.json` only if registration semantics fit the external inputs
- `tools/benchmark/retrieval/README.md`
- `tools/ci/tests/test_retrieval_benchmark.py`
- final report outside the source tree, with an immutable receipt/reference

## Acceptance / verification

- A documented command runs Quanta-only chunking A/B; another optional mode runs the same suite with Semble. Neither requires Semantica.
- Focused Python/Rust tests and one real SDK roundtrip pass at the final source; production API/boundary edits trigger the repository's mandatory escalations.
- Pilot head-to-head reports query count, eligible/excluded corpus and reasons, paired quality, index phases, warm-query latency, resources, raw records and exact revisions.
- No result is described as “Quanta > Semble” unless the same-host paired run and independent evaluator substantiate the scoped claim. An incomplete run is reported as incomplete.
- Final handoff verifies `verdict.json` per [TEST-PLAN.md](TEST-PLAN.md) §8: `CONTRACT_GREEN`, `SDK_PATH_GREEN`, `PAIR_VALID`, `PERF_QUALIFIED` and `QUALITY_DELTA` independently, with `blinding`/`isolation_method`/`access_block_log`, missing/not-applicable T-IDs, failure class, provenance digests cross-checked against the run manifest, and selected/executed counts.
