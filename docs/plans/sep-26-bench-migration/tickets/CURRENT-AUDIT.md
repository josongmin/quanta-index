# Benchmark migration — current source audit, 2026-09-26

**PARTIAL, not all-ticket closure.** This audit supersedes the completion
language in `CLOSEOUT.md` and `BM-07-MIGRATION-MATRIX.md`. A registered producer
and a valid typed fixture are not an implemented profile execution adapter.

## Source and evidence boundary

- Work began at `604149ed3f6033e24a834ebaa86a596f7b8ed82d`, dirty. A concurrent
  owner committed `7cefac4a10a06ed56b6f5b9f42b3726468b1f198` during verification.
  It advanced again to `8eac12c5b45fedfa4aa7cb27da82979ecdbfbb10` at final
  inspection. The bound file hashes, not a moving branch label, identify this
  focused receipt.
  This work did not commit, reset, stash or delete another owner's edits.
- Environment: macOS/aarch64; Python 3.13.9, pytest 9.1.1, uv 0.11.8,
  rustc 1.92.0. Dependencies resolved with `uv --frozen` and `cargow --locked`.
  Executed Python binary SHA-256:
  `37f942b14e1e5a6c0d8fd96471ae445242c46a34c54356c3273e240f4912aae5`.
- `Cargo.lock`: `ff40c97922eb0c81ec1c46ca6bcc3e40e4b9d2d51ea497ec1a288cbb2b314837`.
  `uv.lock`: `c9064e8ead8593a6054d50c9b8e7523026d07d9ae7c95ddbf1268d273d6dad73`.
- Registry digest remains
  `sha256:f2066046b23aad113491157ce777f5fa25ef669a9326b939f3bfbfc7eae9291f`.
- Raw JUnit receipts are external under `/tmp/quanta-bm-closeout.ZsnVLr/`.
  They are local focused/contract evidence, not clean-source measurement or
  hosted CI qualification. Concurrent Cargo writers make timings advisory.
  Final contract JUnit SHA-256:
  `414357fae7346b3e68050c35e76cbe7f8b9d333ce23dd1b0090aa502b585f821`.

Tested implementation file SHA-256 identities:

| File | SHA-256 |
| --- | --- |
| `tools/benchmark/benchctl.py` | `72f18310331b1f241d41a53f89aae4aeb7cee2d379bc9d720b68502033783033` |
| `tools/benchmark/evidence_bridge.py` | `f87c450343504ba31e9aff0f4fdad9f2aa8d0fb4aebed2d5189ccb27ff8d036f` |
| `tools/benchmark/evidence.py` | `ad972ae3ae576ec8d176859328dcfc63ddd89ebb9c9a959dd8194fb18eec09d8` |
| `tools/benchmark/manifest.py` | `bfbac2990fb3c6d23b674dd256d31bfbce51ab4fdc51b644f49c067d6f1cbf55` |
| `tools/ci/tests/test_benchctl.py` | `419b6e0e7663c992ac1a04b0bca3943c5de0d4a8116f5c82a56fbef1f57bb449` |
| `tools/ci/tests/test_benchmark_evidence_bridge.py` | `c5b94c53bef4844bb041b54040daf395186e1bb23292bdaa6ea69b222da380a7` |
| `tools/ci/tests/test_benchmark_manifest.py` | `76cdc94d6e47868efb3c41aaa6593b3a0d50c72fee3d0a144b1c6022eecc3be9` |
| `tools/ci/tests/test_compare_dsl_bench.py` | `52f876212b6258e66f7a0249369e27e0f1fbebbf05a2de00c3cc62f531649326` |
| `tools/ci/tests/test_bench_protocol_conformance.py` | `c9b04df414acd02cd5c0481243b095eb138ac302cc8a46e5524cdab6660e189b` |
| `crates/quanta-index-searchd-harness/src/freshness.rs` | `880b3527c5c839706029525bd84b4c32bd2d2ab165e90e429183dedec1f9fd52` |

## Defects repaired

1. **Systems capture and replay were latency-only.** The bridge now derives
   `freshness` and `load` payloads, and replay re-derives them from captured raw
   artifacts after the independent native artifact checker passes.
2. **Concurrency fan-out was refused.** Promotion retains all three raw files;
   replay requires the complete fast-client 1/8/32 inventory. Envelope totals
   are 1/9/33 when a slow client is present. Client identity comes from native
   scenario IDs and is reconciled with `detail.measurements` and total clients.
   Fast/slow aggregates are counted once, not once per copied report or route.
3. **Different concurrency configs were lost or incorrectly rejected.** Corpus
   identity must agree; config digests remain case-specific. Native corpus and
   config identities replace the misleading unavailable-fixture placeholder.
4. **Missing counters could become fabricated measurements.** Freshness emits
   an explicit observed `stale_hits` count after exact visibility assertions.
   Every phase from each native sample is retained. Open-loop accounts for every
   offered request, including transport/typed/invalid errors, timeouts and all
   three drop kinds. Missing/bool/string counts and duplicated load labels fail.
5. **A partial profile could execute before adapter failure.** Unsupported
   micro/retrieval/recorded profiles now refuse before any producer executes.
   Criterion raw output is no longer projected as a BenchArtifact JSON family.
   Read commands report explicit unmeasured state; unsupported preflight does
   not crash or write a false receipt.
6. **Profile capture metadata claimed an unmeasured zero duration/bench build.**
   Recipe execution retains the exact CLI argument vector (including sample
   overrides), has a measured monotonic elapsed duration and a per-recipe
   timeout. Build profile is `producer-recipe`, not an invented `bench` flag set.
   Evidence-root location is checked before execution. Baseline admission and
   immutable capture are separate actions.
7. **Partial validation could mutate the run store.** All native family inputs
   are prepared and checked before promotion begins. Store-level I/O failure
   still may leave individually valid family runs; profile validation requires
   one consistent capture and never treats that partial set as complete.
   Microsecond capture timestamps and nanosecond run IDs prevent same-second
   captures from sharing the old capture identity or colliding.
8. **Comparator tests depended on moving shared main.** Their real CLI now runs
   with a separate Git checkout pinned to the collected revision. Production
   HEAD/stale-artifact checks are unchanged; no source check is mocked away.
9. **Python payload validation accepted non-finite numbers and oversized
   counters before canonical serialization.** `NaN`, infinities, numeric
   overflow and counters outside Rust's `u64` range now fail at validation.
   The micro bridge no longer coerces missing/bool/string values into numbers.
   Finite/range negative tests and the existing Rust canonical vectors pass.

## Verification receipts

| Command/surface | Status | Result/scope |
| --- | --- | --- |
| 14-file Python control-plane suite, listed below | VERIFIED | 293 passed in 38.19 s; `qualified-contract.xml`; focused/fixture/raw-replay scope, not performance or repository qualification |
| `pytest tools/ci/tests/test_compare_dsl_bench.py -q -p no:cacheprovider --junitxml=/tmp/quanta-bm-closeout.ZsnVLr/comparator-fixed.xml` | VERIFIED | 35 passed; frozen Git fixture, real comparator CLI |
| `./scripts/cargow --lane bench-lane test -p quanta-index-bench-protocol --locked` | VERIFIED | 46 tests: 37 adversarial, 2 conformance, 7 round-trip |
| `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-harness --bin freshness_matrix --all-features --locked -- --nocapture` | VERIFIED | 1 real workspace update/delete/rename and historical-generation test |
| Same harness command, `--lib concurrency::` | VERIFIED | 5 tally/row/artifact tests |
| Same harness command, `--bin open_loop_matrix` | VERIFIED | 9 tests, including bounded real-runtime smoke and request-accounting negatives |
| `pytest tools/ci/tests/test_retrieval_benchmark.py -k 'verdict_cli_smoke or successful_promotion_replays_identically_in_new_process or run_pair_promotes_complete_stage_and_public_verdict_replays' -q -p no:cacheprovider --junitxml=/tmp/quanta-bm-closeout.ZsnVLr/retrieval-replay-rerun.xml` | VERIFIED | 3 passed / 273 deselected in 15.80 s; only the three source-moving failures, not the entire retrieval suite |
| `python3 tools/ci/lint/check-benchmark-policy.py --print-registry-digest` | VERIFIED | Registry/dependency/CI policy accepted |
| `ruff check` / `ruff format`, changed Python surfaces; `cargow fmt -p quanta-index-searchd-harness -- --check`; scoped `git diff --check` | VERIFIED | Formatting/static gates, not execution or E2E |
| Clean-source `benchctl run systems`, native load capacity and hosted CI | NOT_RUN | Dirty, concurrently edited source and contended host; no performance claim |

The 14-file suite command is:

```sh
uv run --frozen --extra dev python -m pytest \
  tools/ci/tests/test_benchmark_manifest.py \
  tools/ci/tests/test_benchmark_policy.py \
  tools/ci/tests/test_bench_protocol_conformance.py \
  tools/ci/tests/test_benchmark_evidence_bridge.py \
  tools/ci/tests/test_benchmark_source_closure.py \
  tools/ci/tests/test_benchctl.py \
  tools/ci/tests/test_check_bench_artifacts.py \
  tools/ci/tests/test_check_host_contention.py \
  tools/ci/tests/test_compare_dsl_bench.py \
  tools/ci/tests/test_quality_integration_summary.py \
  tools/ci/tests/test_retrieval_contract_proof.py \
  tools/ci/tests/test_retrieval_sdk_proof.py \
  tools/ci/tests/test_agent_outcome_benchmark.py \
  tools/ci/tests/test_validate_agent_output.py \
  -q -p no:cacheprovider \
  --junitxml=/tmp/quanta-bm-closeout.ZsnVLr/qualified-contract.xml
```

`final-control-plane.xml` records the earlier **FAILED** attempt (23 stale-HEAD
comparator assertions / 261 passed), not a green receipt. The shared checkout
advanced mid-run. Those failures led to the frozen comparator fixture; neither
the failures nor the real source-attribution guard were hidden.

The subsequent `current-control-plane.xml` passed 284 tests before the final
micro-value/argv changes (SHA-256
`625753ee7b3c1c18c08d6c3927649aba5bc93bc2754511a9de1be10ce4c855dd`).
`comparator-fixed.xml` SHA-256 is
`f31521a2408bff1d86c4b3dd8d201ae4aeb47c143a5ea7973d12a79a70ca474f`.
The final receipt, not the earlier one, governs the file identities above.
`sealed-control-plane.xml` is a negative intermediate receipt (1 NaN refusal
test failed / 287 passed), repaired by the stricter common numeric validator.

The expanded 15-file run, additionally including `test_retrieval_benchmark.py`,
completed with **3 failed / 552 passed / 32 subtests passed** in 541.90 s
(`python.xml`). The three failures compare in-process and fresh-process
`PAIR_VALID` verdicts while the shared retrieval source/test files were being
edited; the in-process assertion uses older loaded source. This is a failed,
source-moving diagnostic run, not an accepted current-source retrieval receipt.
The three exact tests are re-run separately before assigning a current defect.
That fresh current-source rerun passed all three; no retrieval product fix was
inferred from the mixed-source failure. Full retrieval qualification remains
outside this focused receipt.
Rerun JUnit SHA-256:
`95ecdce29a8bf6db97610918dab4a27137bca4286c613ca87e06dd01d8abbfe5`.

## Remaining implementation — not manual/external-input blockers

| Owner | Required code work | DoD |
| --- | --- | --- |
| BM-04 | Criterion capture/estimate adapter for `micro` and `dsl-diagnostic` | Registered owner command executes through CLI; retain raw estimates/samples and every benchmark case; typed ns/instruction units preserved; interrupted/partial results refused; fresh-process raw-derived replay |
| BM-05 | Retrieval CLI parameter and evidence adapters | Explicit external corpus/query/gold/capture paths; reuse existing scorer/proof owners; one immutable complete profile capture; file/span and judged/unjudged spaces preserved; raw-derived replay and source/input mismatch negatives |
| BM-06 | Recorded agent/scan import adapters | Explicit input paths and authenticity; no arbitrary shell producer; recorded-only diagnostic scope; all rows/cases retained; no fabricated gold; native/JSONL-derived replay |
| BM-04/07 | Native build/binary inventory and capture-time host lease | Exact executed binary/toolchain/flags bound before execution; monitored lease loss and generator saturation exclude speed/capacity qualification; clean-source representative capture and hosted CI receipts |

BM-00/01/02 infrastructure remains in place. BM-03 is a partial execution
surface, BM-04 has native wiring/fixture coverage but not complete micro support,
BM-05/06 are registration/contracts rather than completed capture adapters, and
BM-07 cannot claim global cutover while those adapters are absent.

External license/gold/model/quiet-host admission belongs to qualification, not
this implementation list. No fabricated `PAIR_VALID`, `QUALITY_DELTA`, speedup,
capacity or product-win result is issued here.
