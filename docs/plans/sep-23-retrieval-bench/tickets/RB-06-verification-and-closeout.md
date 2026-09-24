# RB-06 — Registered Entry Point and Source-Bound Closeout

Status: `proof-inventory-remediation-implemented / source-bound-receipts-pending / closeout-blocked`

Depends on: RB-05

Owner: Just/CI/test authority and final evidence inventory

## Current code status (2026-09-24)

Named prep, SDK, Quanta-only, paired, verdict, host-probe and host-profile commands exist. Both Rust integration targets are registered in `tools/ci/test-authority.toml`; focused Python and live SDK rails execute successfully on the audited retrieval source.

This ticket is not closed. The repository has no W0-B pilot, real paired manifest/verdict, final source-closure-bound contract/SDK receipt set or broader-suite run. The macOS isolated-quality path and the shared cold/warm query protocol are implemented and mutant-tested but have not been exercised on admitted pilot inputs. `retrieval-contract-proof <fresh-out>` derives summaries from raw JUnit and nextest JSONL; `retrieval-sdk-proof <fresh-out>` derives its summary from nextest JSONL plus the actual-runner v3 record. Receipt v2 binds those raw inputs by role/SHA as well as the transitive source closure, and the final verdict reparses the frozen raw evidence instead of trusting a summary JSON. An earlier dirty-checkout audit completed 160 Python contract tests, 31 retrieval-library tests, 20 chunking-contract Rust tests and 22 receipt/proof-helper tests. Those results are historical implementation evidence only: relevant source remained dirty and source closure refused it, so a fresh clean source closure, registered contract/SDK receipts, issued W0-B admission and real quiet-host paired run remain mandatory.

The adversarial baseline accepted a correctly rebound JUnit file with one pass and 159 skips, an announced 99-test nextest suite with one terminal test, and duplicate JUnit identities as contract proof. The remediation freezes exact Python/Rust/SDK required identities in `benchmarks/retrieval/proof-required-tests.json`, checks collection and terminal evidence, and re-reads that authority as a Git blob at the receipt revision. `pair-spec.schema.json` accepts the three collection inventories; `run-manifest.schema.json` binds their paths and digest claims. The verdict compares the blob SHA-256 with the source closure before accepting an inventory. At `64938975`, the authority lists 173 Python contract, 51 Rust contract and 12 SDK identities; these are inventory counts, not executed proof results. Final clean-source contract/SDK receipts and a real paired verdict remain pending.

## Goal

Make the benchmark easy to run without turning a short fixture test into a search-quality claim or adding a mandatory Semble/model download to every CI job.

## Work

1. Add named `Justfile` commands for cheap preparation/contracts and explicit real-repo capture. Reuse the `./scripts/cargow` build front door. Keep `benchctl`; register a new retrieval profile in `tools/benchmark/manifest.json` only when the profile has a frozen suite/query-pack digest, a pinned runner command with artifact/host/source controls actually wired and tested, and a declared claim scope (pilot exploratory-only vs qualified). Do not overload `quality-full` with an external real-repo comparison, and do not register a profile whose Semble/model/host inputs are still unpinned.
2. Register owner-local Rust/Python tests in `tools/ci/test-authority.toml` where required, and update the benchmark prep recipe to include the new contract tests. Keep full opponent measurements opt-in or on a dedicated controlled host; CI should check schemas, strategies and a tiny SDK roundtrip without downloading Semble/model assets.
3. Run a pinned exploratory-only pilot (20-query floor proves machinery, not superiority), inspect every excluded query/file, and rerun after correcting any contract bug. Then scale across supported categories/languages with a held-out suite. Refuse a single global comparison if support differs materially; report strata and coverage.
4. Validate final clean source, quiet-host profile with check-record entries, exact binary/model revisions, both raw records, path-mapping proof artifact, suite hashes, mandatory manifest fields (tokenizer/budget version, Semble lockfile digest, path+SHA diff digest), and report. Distinguish implementation/test pass from qualified quality/speed evidence and from production activation.
   The qualified driver must freeze clean retrieval source before staging, verify the same closure after capture, cross-bind it to receipt closures, and bind stage-local isolated runner-tool hashes in its proof.
5. Maintain [TEST-PLAN.md](TEST-PLAN.md) T00–T17 as the required coverage matrix. Map every blocking ID to a test target, command and artifact; stop expensive E2E work after a cheap blocking schema/source/model/admission failure. Do not emit a success summary with `NOT_RUN` IDs. T15 is mapped only to the same-model control claim; T16 only to an incremental claim — otherwise record them as not-applicable. T17 is mandatory for every qualified quality/performance state.
6. Check successful atomic promotion with a new public verdict process at the final path; require unchanged digest-bound record/report identity. Complete implementation, test and normative-document edits before source freeze and receipt issuance. Any subsequent bound input change requires renewed source closure and receipts.

## Owner / expected files

- `Justfile`
- `tools/ci/test-authority.toml`
- `tools/benchmark/manifest.json` only if registration semantics fit the external inputs
- `tools/benchmark/retrieval/README.md`
- `tools/ci/tests/test_retrieval_benchmark.py`
- final report outside the source tree, with an immutable receipt/reference

## Acceptance / verification

- A documented command runs Quanta-only chunking A/B; another optional mode runs the same suite with Semble. Neither requires Semantica.
- Focused Python/Rust tests and one real SDK roundtrip pass at the final source; production API/boundary edits trigger the repository's mandatory escalations.
- Contract/SDK receipts bind exact collected mandatory identities and raw terminal evidence to the required-test authority committed at the receipt revision. Partial, skipped, duplicated, count-mismatched or wrong-binary evidence fails even when a summary and every SHA-256 are internally consistent. A final receipt must demonstrate this on the frozen source.
- Pilot head-to-head reports query count, eligible/excluded corpus and reasons, paired quality, index phases, warm-query latency, resources, raw records and exact revisions.
- No result is described as “Quanta > Semble” unless the same-host paired run and independent evaluator substantiate the scoped claim. An incomplete run is reported as incomplete.
- Final handoff verifies `verdict.json` per [TEST-PLAN.md](TEST-PLAN.md) §8: `CONTRACT_GREEN`, `SDK_PATH_GREEN`, `PAIR_VALID`, `PERF_QUALIFIED` and `QUALITY_DELTA` independently, with `blinding`/`isolation_method`/`access_block_log`, missing/not-applicable T-IDs, failure class, provenance digests cross-checked against the run manifest, and selected/executed counts.
