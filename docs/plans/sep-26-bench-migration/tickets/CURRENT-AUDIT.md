# Benchmark migration — current source audit, 2026-09-26

**PARTIAL, not all-ticket closure.** This audit supersedes the completion
language in `CLOSEOUT.md` and `BM-07-MIGRATION-MATRIX.md`. A registered producer
and a valid typed fixture are not an implemented profile execution adapter.

## Latest common lexical capture checkpoint

Main base remains `577d60b518344163145ad2f1afe1f3e7c656762e` with concurrent
dirty work. The lexical adapter was frozen independently at
`421a2527bf0ecf310f8407c17d01daf16f026a74` in
`/tmp/quanta-micro-audit.EwaPQA/proof-lexical-capture` (clean checkout).
This does not qualify mutable main or the live products.

**VERIFIED — common lexical recorded-input execution contract:**

- `benchctl run lexical-diagnostic --lexical-spec <external-spec>
  --evidence-root <external-root>` freezes all nine input roles, invokes the
  existing lexical scorer as a subprocess and retains its report and logs.
- Five product cases are promoted only after the whole capture succeeds.
  Failed replacement does not displace the prior complete profile. Validation
  binds registry, source, runtime and frozen inputs; replay re-scores raw bytes
  without following the original mutable input paths.
- Timeout/unavailable remain non-scored typed states, not zero recall.
  Tampered payloads/inputs, registration drift and incompatible CLI controls
  refuse. Full owner report retains hit rate, file recall and distinct latency
  layers; the typed retrieval payload does not invent parity or qualification.
- Frozen selected suites: lexical capture/scorer, retrieval capture, manifest,
  policy, CLI and source closure: **182 passed in 178.58 s** using
  `uv run --frozen --extra dev python -m pytest <seven owner test files> -q
  -p no:cacheprovider --basetemp=/tmp/quanta-micro-audit.EwaPQA/lexical-capture-frozen-fixtures
  --junitxml=/tmp/quanta-micro-audit.EwaPQA/lexical-capture-frozen.xml`.
  JUnit SHA-256: `aa6f53f78d4730291acfe251a49c447fb34847d9937bf42c9bf2d549fb96fcf1`.
- Actual fresh CLI capture, validate, summary and separate replay for each
  of the five run IDs completed on fixed **test-generated inputs** at
  `/tmp/quanta-micro-audit.EwaPQA/lexical-cli-contract`. Capture/validate output
  SHA-256: `c18f341aeb535e61a42d85217265bc3e44bf87ca060dd63e09e15aa3e5784d62`.
  Sourcegraph replay: `fe9bc7dc265a0c6539791a81fc636c071ce74de3fcd86f89be70201de6bb22f7`;
  OpenGrok: `41df62685d63f0f6a08461e2a76cefbf10ea0186b7f0c081616d3138768de6c3`;
  cs: `ff94edcb8d54568adc2276856d45b50cfaa2368d83da8c74db09e11cea123615`;
  Quanta: `d5eb808215b206cc4b4248039cce64cc9aef36fe1c4ffe1ab95791f225cde543`;
  Semble: `d36dd98576385cf9686cfa8e7c0aaee0fd213dd5c2777e1b5ba7bf7ac08cd7cd`.

**NOT_RUN — fresh live-product search/corpus pilot, paired common CLI adapter,
quality/performance qualification and hosted CI.** Fixture commands prove the
execution/evidence contract, not comparator availability, independent gold,
live index freshness or product quality/speed. Real native micro and retrieval
contract processes still have no accepted terminal complete-profile receipt.

The latest local/CI wiring correction adds one
`benchmark-control-contract-local` recipe, selected by locked uv from both
PREP and the CI `rust-policy` job. Seven capture/scorer owner targets are
registered in `tools/ci/test-authority.toml`. The authority guard checks that
the exact workflow command executes an unfiltered pytest selection; it accepts
only the exact locked uv prefix, not arbitrary wrappers or collect-only runs.
Current dirty-main authority validation and Ruff passed. The full new recipe
and hosted execution are not qualified by those structural checks.

Runtime wiring follow-up: launching `uv run ... just` alone did not preserve
the Python executable through Just's `zsh -lc` shell: the initial recipe selected
system Python 3.9 rather than uv Python 3.13.9. That source (`e11efc58`) is not
accepted as locked-runtime proof. The recipe now invokes
`uv run --frozen --extra dev python -m pytest` internally. Frozen corrected
source is `816dac2aa5c4b0f0d4de741f5d595e4cd0146e12` under
`/tmp/quanta-micro-audit.EwaPQA/proof-ci-runtime`; its terminal recipe result is
recorded below.

The rejected system-Python run has a terminal result: **FAILED, 17 failed and
469 passed in 332.16 s**, receipt
`/tmp/quanta-micro-audit.EwaPQA/benchmark-control-frozen.xml`, SHA-256
`ab8a5f8df444ac450b82496d8b720fdb200e3e53b723736c447526a66621793f`.
Its failures include
Python 3.9 rejecting `zip(..., strict=True)` in the frozen tests/owners. No
failed run is discarded or promoted as a success; the internally uv-bound
source is verified separately.

The frozen authority guard suite at `816dac2a` completed with 166 passed in
146.54 s; the actual authority command also returned OK. JUnit
`/tmp/quanta-micro-audit.EwaPQA/benchmark-authority-runtime-frozen.xml`, SHA-256
`12151c701164a7206104be476e081b6ee88321461257b1b9cc1a86e9887ea447`.
The new locked recipe executes `.venv/bin/python3` (uv CPython 3.13.9), verified
from its live process, rather than the former system interpreter. Its terminal
20-file result is **VERIFIED: 486 passed in 269.77 s**, receipt
`/tmp/quanta-micro-audit.EwaPQA/benchmark-control-runtime-frozen.xml`, SHA-256
`46f1b32c859290e858c78f9949624689fd374ffa15aa795fff8dfbd124b5aafc`.
Main additionally selects agent-outcome and
concurrency sample contracts: 37 selected tests passed in 17.84 s on dirty main,
receipt `/tmp/quanta-micro-audit.EwaPQA/benchmark-full-owner-selection-current.xml`,
SHA-256 `f368ab22de09ea4988e752f59317815f674fd2821d0a202d9b6912aa4b4fd565`.
The guard now checks that every normative control-plane test path is selected,
not just the seven new owners. These different-source results are not composable
into qualification of the current 22-file recipe.

Shared main advanced externally to `bc7946309a65a0dd70ee0e90f768c7386e9390b5`.
The snapshot receipt identities above remain unchanged. The CI/Justfile/catalog/
closure/guard write set is frozen for coordinated final integration; whole-main
and hosted CI results are still NOT_RUN in this owner checkpoint.

Minimum-runtime contract correction (newer source/lock, separate from the 486
receipt): the package previously declared Python >=3.9 although benchmark
owners use `zip(strict=True)` (the rejected 3.9 run demonstrates the mismatch).
`pyproject.toml` now requires >=3.10; `uv lock --offline` pruned the unsupported
3.9 dependency forks. Comparing sorted package/version tuples against the
816dac2a lock introduced no new version tuple. `uv lock --check --offline`
passed, and explicitly requesting `/usr/bin/python3` refused Python 3.9.6 with
exit 2 before executing user code. Selected lexical capture/scorer/manifest
tests passed: 37 in 9.79 s under the new lock on dirty main, receipt
`/tmp/quanta-micro-audit.EwaPQA/benchmark-runtime-floor-current.xml`.
This is not a Python 3.10 full-suite compatibility receipt. The conservative
Ruff syntax target remains unchanged; broad legacy lint-rule migration is not
silently coupled to the runtime floor fix.
`uv run --frozen --extra dev ruff check . --output-format json --output-file
/tmp/quanta-micro-audit.EwaPQA/ruff-runtime-floor-current.json` returned exit 0
with zero diagnostics on dirty main at `349090ca1ff5db875814dda848f01baf71131a10`.
This read-only lint check does not qualify the unexecuted product/native rails.

The earlier frozen lexical owner regression at `febcb612` has now terminated:
449 passed in 1623.68 s. Receipt
`/tmp/quanta-micro-audit.EwaPQA/lexical-owner-frozen-final.xml`, SHA-256
`fd42ad100042b59c32ababbc3dd4d7a51d712bf3964145bd97e3508fb5d237e1`.
Its seven suites include the full native retrieval Python owner contract but
exclude the later common lexical adapter and CI/runtime wiring changes. It
does not supersede the source-bound 182-test adapter receipt above.

## Latest lexical owner / registration correction

Main reached `577d60b518344163145ad2f1afe1f3e7c656762e` with concurrent
dirty changes. This section is newer than the contract checkpoint below.

Source-backed corrections:

- `lexical-file-comparison` was incorrectly registered against the pair
  producer and manifest schema. It now names `lexical_file_comparison.py --spec`
  and `lexical-file-diagnostic:v1`; the five-product profile is
  `lexical-diagnostic`, separate from `retrieval-diagnostic` pair execution.
- Validators now name their actual owners: `portable_proof.py verify` for
  SDK/contract receipts, `run.py verdict` for pair captures, the lexical scorer
  for five-product rows. A proof adapter refuses changed validator, scorer,
  schema, unit, host policy or gate tier rather than silently using its old
  hardcoded owner under new registration.
- The lexical product scorer formerly labelled any-hit rate as file recall.
  One retrieved file out of two gold files now yields Hit@10=1 and Recall@10=0.5.
  Both metrics and per-query observations are retained. Pair hit counts are
  derived from hit observations, not rounded from macro recall. The evaluator
  emits actual per-query file recall from native candidates; the lexical scorer
  cross-checks those observations against the aggregate. Missing historical
  fields require original-record re-scoring, never defaulted score upgrades.
- Duplicate JSON keys, non-finite constants, non-object rows, symlinked inputs,
  malformed IDs/paths/count types and files changed during reading refuse.
  Score and digest use the same bytes. Output is exclusive-create and mixed
  explicit/spec input controls refuse before output.

Scoped dirty-main tests passed: 86 tests over lexical owner, registry and CLI
in 24.86 s; raw `/tmp/quanta-micro-audit.EwaPQA/lexical-owner-current.xml`,
SHA-256 `7170078ecbcf7e99ed9813f5570ac5e70f665b7da58d8ddeae1935b1a1b37d02`.
Owner-registration and proof focused rerun: 29 passed in 10.33 s; raw
`registry-owner-guard-fixed.xml`, SHA-256
`0fa5cec438ef3d7d06b97c37ff83b29b5ca969e23a472fbca659d88fe37c6ee6`.
The preceding run failed six tests because a new test was inserted before
the prior test's final context assertion; that fixture scope error was fixed,
not suppressed. These moving-main checks do not qualify clean product execution.

Private clean `febcb612c76c580dacbe8c2919cdfab31bb5d88e` at
`/tmp/quanta-micro-audit.EwaPQA/proof-lexical` freezes the lexical metric change,
profile split and initial negative tests. A seven-file suite including all
`test_retrieval_benchmark.py` tests is live; no terminal receipt is accepted yet.
Later validator registration/owner guards and exclusive-output CLI fixes on
main are not in this older snapshot and need their own final freeze.

Actual historical product rows were re-scored in that clean snapshot using
`_tasks` and `product_result`, with suites/packs from external
`qi-rb-coverage-set-2026-09-25/bare-symbol-lexical-inputs-03/{gin,ripgrep}`
and raw Sourcegraph/OpenGrok/cs recordings. Result raw:
`/tmp/quanta-micro-audit.EwaPQA/actual-lexical-products-recomputed.json`, SHA-256
`01d088ff2e413556eb72fdea79df8d65a9d4320ff57241d7f14a1afd545a0447`.
The result records exact corpus revisions and suite/pack/raw digests.

| Corpus | Sourcegraph Hit/Recall@10 | OpenGrok Hit/Recall@10 | cs Hit/Recall@10 |
| --- | --- | --- | --- |
| gin, 20 recorded bare-symbol queries | 20/20, 1.0 | 20/20, 1.0 | 20/20, 1.0 |
| ripgrep, 20 recorded bare-symbol queries | 20/20, 1.0 | 19/20, 0.95 | 19/20, 0.95 |

This is **VERIFIED historical-recording diagnostic recomputation**, not fresh
service execution, independent gold, search-universe proof or a qualified
comparison. Sourcegraph stream HTTP, OpenGrok REST HTTP and cs spawn+search
latencies are retained separately; no cross-product speed ratio is valid.
Old pair lockfiles lack current execution profiles and reports lack per-query
file recall: the current five-product scorer refuses them rather than silently
upgrading the old pair. New common paired/lexical capture adapters, current
paired raw re-scoring and actual fresh pilot remain open.

Current registry digest after owner corrections:
`sha256:8725e8077ab03768846c0249189a1d1d3a07a377107275d06ff67bafe756c2c7`.
`check-benchmark-policy.py --print-registry-digest` exited 0. This is policy
registration proof, not measured-result proof.

Latest mutable-source owner suite after exclusive-create CLI and registry
guards: `uv run --frozen --extra dev python -m pytest
tools/ci/tests/test_lexical_file_comparison.py
tools/ci/tests/test_benchmark_manifest.py tools/ci/tests/test_retrieval_capture.py
-q -p no:cacheprovider --junitxml=/tmp/quanta-micro-audit.EwaPQA/lexical-registration-owner-final-current.xml`
exited 0: **53 passed in 17.10 s**. Raw JUnit SHA-256
`5d90b4cfb33184737ea54e2b730913232c2504dc969b33b3177eb195cf7609dd`.
Scope: owner math, typed refusal, schema/registration and CLI output contracts;
excludes fresh product queries, common diagnostic profile capture and whole-repo
qualification. Ruff and scoped `git diff --check` exited 0.

Audited current implementation SHA-256 (shared main, not a clean snapshot):

| File | SHA-256 |
| --- | --- |
| `tools/benchmark/registry.toml` | `1cae8c8f3afd355a3c0077de95da26d1ce8a000e79168e2a1f80db257022941e` |
| `tools/benchmark/retrieval/evaluator.py` | `d7a426088c3b096a900692037ce7947fdf71d2f8a6593a2f1e908855a1e67ebc` |
| `tools/benchmark/retrieval/lexical_file_comparison.py` | `18ffc8c994e2a684a47e2b55b0ece2122b4a9758cf2b2066ae65758884495b7e` |
| `tools/benchmark/retrieval_capture.py` | `7635fd89aa44615d0d236496bc8e86e12d89ba9888e2e0cf38743dbb6e3957cf` |

## Latest BM-05 contract adapter checkpoint

Shared main advanced to `2c08dccff4a9c1c23a3885dc74f10a5e9f4bd5f8`
with concurrent dirty work. This checkpoint does not qualify mutable main.
Verification uses private clean snapshot
`9f8892ae9ca6e87eba7ecf4be986f09689857fc7` at
`/tmp/quanta-micro-audit.EwaPQA/proof-retrieval`. The snapshot includes the
matching current portable proof owners and their dependency/test authorities;
no other owner's source was reset or committed on main.

Implemented:

- Common `run/validate/replay/summarize retrieval-contract` executes both
  registered SDK and contract owners. `retrieval_capture.py` reuses their
  portable validator and publishes only a complete two-family capture.
- Rust, Python and the registry distinguish terminal test `proof` counts from
  retrieval relevance. Selected/executed/pass/fail counts, source and execution
  context digests are bound. Wrong-source, partial execution, failed tests
  presented as passing and quality/performance scope are refused.
- Native raw receipts retain original command/path provenance after immutable
  relocation. SDK runner/searchd binaries are frozen independently of live
  caches. Build target comes from recorded rustc host/target identity, not a
  guessed OS/architecture string. Replay re-derives owner counts and checks
  toolchain, binary, target and input inventories.
- Shared producer execution retains owned process-group cancellation/deadline
  behavior and explicitly refuses non-POSIX hosts. External retrieval Just
  parameters are exported and quoted, not interpolated into shell syntax.

Frozen verification commands:

```sh
uv run --frozen --extra dev python -m pytest \
  tools/ci/tests/test_retrieval_capture.py \
  tools/ci/tests/test_portable_proof.py \
  tools/ci/tests/test_bench_protocol_conformance.py \
  tools/ci/tests/test_benchctl.py \
  tools/ci/tests/test_criterion_capture.py \
  tools/ci/tests/test_benchmark_source_closure.py \
  -q -p no:cacheprovider \
  --junitxml=/tmp/quanta-micro-audit.EwaPQA/retrieval-contract-frozen-final.xml
./scripts/cargow --lane test-light-lane test \
  -p quanta-index-bench-protocol --locked
```

Environment: macOS/aarch64, locked Python 3.13.9 and rustc 1.92.0.
**VERIFIED** scoped contract tests: Python 199 passed in 131.49 s;
Rust 38 adversarial + 2 conformance + 8 round-trip passed.
JUnit SHA-256:
`0190aca93e871eb9da3f111e143f90e835468ea351fa6ae299f0b9dc3d845d9c`.
Rust raw log `/tmp/quanta-micro-audit.EwaPQA/retrieval-proof-protocol-rust.log`,
SHA-256 `e2b3c08f8d49dcdaed239e7c387caaa8596d4af61ac91a59712afdfbae357d94`.
Frozen `cargow --lane test-light-lane fmt -p quanta-index-bench-protocol -- --check`
and `clippy -p quanta-index-bench-protocol --all-targets --locked` also exited 0.
Clippy raw `/tmp/quanta-micro-audit.EwaPQA/retrieval-proof-protocol-clippy.log`,
SHA-256 `b3d0e4d5d94585b12bef561b191170a88588baac858323133a5ea03f490c1d39`.
Composite source closure has 903 files and digest
`494c88a057c1827ac9486c9e8427a232df8a7b69ae37e57a7e8fa3717224a598`;
raw `/tmp/quanta-micro-audit.EwaPQA/retrieval-common-source.json`, SHA-256
`a2eac9db450cdb038f10f59ad0b4e78d9f1c488a35b5ab8d05b14d8e2a667aaa`.

Failures retained: mutable-source expanded run had 197 passes and one stale
message assertion after the source-closure owner hardened committed-byte
checking. The source mutation was correctly refused; the test now asserts
dirty-source refusal and the exact affected path. Raw failed JUnit
`retrieval-contract-final-current.xml`, SHA-256
`69aae6551ddd3cd0b7050339ff9d22ffe0e461c947fa7543ff26e4cf64c6a6e2`.
Four earlier argv shim tests invoked real tools because the configured login
shell reset PATH. Tests now override only the test shell to preserve the shim
PATH and exercise the real quoted recipes; no product proof is inferred from
these shim tests.

An additional dirty-main CLI admission regression (added after the frozen
receipt) confirms dirty-source `RuntimeError` becomes exit 2 before any producer
or evidence directory is created. Focused dirty-main suite: 23 passed in
56.38 s; raw `/tmp/quanta-micro-audit.EwaPQA/retrieval-dirty-refusal-current.xml`,
SHA-256 `c41cbf8cc8206e89cb17f8c0e0b91b10fca1fc3aaf3cac833e31059f36b5b419`.
This does not extend the frozen 199-test source receipt to newer main edits.

Actual common CLI run is in progress in the same clean snapshot:

```sh
CARGO_BUILD_JOBS=2 uv run --frozen --extra dev python tools/benchmark/benchctl.py \
  run retrieval-contract \
  --evidence-root /tmp/quanta-micro-audit.EwaPQA/retrieval-proof-actual \
  --producer-timeout 7200
```

Raw log: `/tmp/quanta-micro-audit.EwaPQA/retrieval-proof-actual.log`.
No complete profile is accepted yet: actual SDK/contract producer completion,
fresh-process validation and both replays remain **NOT_RUN** until terminal
results. This receipt does not cover paired/lexical comparison, independent
gold, quiet-host performance, all Rust packages or hosted CI.

Remaining implementation: `retrieval-diagnostic` paired/lexical common adapter,
external corpus/capture custody and scorer cross-check; native measured binary
inventory/monitored host lease (including contract Rust test-executable custody,
not just the SDK runner/searchd pair); global cutover/CI. The in-progress micro capture
still binds the older clean `9e0f9371` snapshot, not this newer closure.

## BM-04 follow-up: Criterion adapter and complete-profile custody

This section is newer than the historical source/receipt block below. Work
started at `f9c3b4dc487a1b54a260e4ab2d3dd199310d3a5e`, with unrelated retrieval
edits already dirty. Implementation remains on shared main; verification uses
a private, clean snapshot `e6650669aac48be4009058e9d2b893d985201c5f` under
`/tmp/quanta-micro-audit.EwaPQA/checkout`. That private commit is not a main
commit, merge, or release qualification. No other owner's edits were reset,
stashed, deleted or committed on main.

Implemented:

- `benchctl run/validate micro|dsl-diagnostic --evidence-root ...` now has a
  Criterion owner adapter, not a pre-execution missing-adapter refusal.
  All targets still live at their owning product crates.
- The redundant `dsl-warm-criterion` registration was retired. The diagnostic
  DSL profile reuses `micro-searchd-runtime-dsl-query-matrix`; its Just recipe
  delegates to the common CLI and takes an explicit external evidence root.
  The auxiliary manual DSL JSON is not treated as Criterion estimates.
- Fresh Criterion output directories; exact Cargo-reported executable hashes
  before/after execution; rustc verbose identity, feature list, arguments and
  non-secret build environment; binary `--list`, correctness `--test`, then
  timed capture. Timeout/cancellation kills the owned producer process group.
- Exact metadata/sample/estimate schema and case inventory; raw means are
  independently re-derived from `times / iters`. No missing/NaN/coerced sample,
  partial case set or changed binary/source becomes a passing capture.
- LQ used to repeat short terms until parser fan-out limits were exceeded and
  silently skip stages. It now uses sixteen distinct, bounded-fan-out terms at
  exactly 1/4/16 KiB, validates tokenize/parse/normalize before registering any
  timing and always requires all twelve stage/size cases.
- A dedicated `benchmark-micro` source closure includes both owning crates
  and their local dependency closures, plus control-plane code/tests.
- Immutable complete-profile capture records publish a separate profile
  pointer only after every run and case validates. GC pins historical captures
  and refuses malformed custody; POSIX process/thread locking excludes GC
  during publication. Rust GC refuses orchestration-owned capture roots.

Current receipts (macOS/aarch64; locked Python 3.13.9; rustc 1.92.0):

The latest coherent control-plane snapshot is private clean commit
`83f2487953f737b52989bca2c3b8609b07fd024a` at
`/tmp/quanta-micro-audit.EwaPQA/proof-current`. It includes the concurrently
updated native checker, its pure contract, producer concurrency sources, and
matching fixtures. The earlier `4b96e745` bundle failed 12 tests because it
omitted checker dependencies; `55828607` failed three tests because its old
bridge fixture mixed the previous and current concurrency contract. Neither
failure is suppressed or counted as qualification. The matching fixture was
copied from current source; no validator fallback was introduced.

Command for the latest focused receipt:

```sh
uv run --frozen --extra dev python -m pytest \
  tools/ci/tests/test_benchmark_manifest.py \
  tools/ci/tests/test_benchmark_policy.py \
  tools/ci/tests/test_bench_protocol_conformance.py \
  tools/ci/tests/test_benchmark_evidence_bridge.py \
  tools/ci/tests/test_benchmark_source_closure.py \
  tools/ci/tests/test_benchctl.py \
  tools/ci/tests/test_criterion_capture.py \
  tools/ci/tests/test_benchmark_profile_capture.py \
  tools/ci/tests/test_check_bench_artifacts.py \
  tools/ci/tests/test_concurrency_sample_contract.py \
  -q -p no:cacheprovider \
  --junitxml=/tmp/quanta-micro-audit.EwaPQA/current-frozen-coherent-v2.xml
```

Result: **VERIFIED**, 236 passed in 51.25 s. JUnit SHA-256:
`551ac78e6616a38811f50b1242fa957d3fa18ccad22a91f4e613559893b7c017`.
Scope: profile custody, Criterion parsing/orchestration contract, fresh-process
replay, native checker/fixture agreement, source closure and CLI. Excludes
complete product micro capture, performance qualification, retrieval/recorded
adapters, repository-wide Rust verification and hosted CI.

| Command/scope | Status | Evidence |
| --- | --- | --- |
| Frozen snapshot, eight-file Python suite: manifest, policy, conformance, bridge, source closure, CLI, Criterion and profile capture tests | VERIFIED | 181 passed in 56.85 s; `/tmp/quanta-micro-audit.EwaPQA/frozen-contract.xml`; SHA-256 `2d3af61ee33449ca386fd5031838255d12c0c212607868c41ee441855396a0f1` |
| New adapter/custody focused suite on dirty main | VERIFIED | 32 passed in 33.27 s; `capture-unit-custody.xml`; SHA-256 `a4f7ef02c4f388ba824b506c40b6816e4f51419c88e3d80c9f9cb68fea6733e9`; synthetic owner orchestration is contract proof, not product timing |
| `./scripts/cargow --lane test-light-lane test -p quanta-index-bench-protocol --locked` | VERIFIED | 38 adversarial + 2 conformance + 7 round-trip tests; includes Rust GC refusal without deletion |
| `./scripts/cargow --lane bench-lane bench -p quanta-index-lq-norm --bench pipeline --all-features --locked --no-run --message-format=json`, actual binary `--list --format terse`, `--test`, then 10-sample diagnostic measurement | VERIFIED | 12 registered / 12 smoke successes / 12 native sample sets; raw under `/tmp/quanta-micro-audit.EwaPQA/lq-raw`; all mean estimates match raw sample recomputation; dirty-source fixture/adapter proof only |
| Frozen full `micro` profile capture and fresh-process validation/replay | NOT_RUN | Capture has been launched but no complete profile receipt is accepted yet; wait for producer completion before upgrading this row |
| Quiet-host micro/system performance, peak RSS/capacity, hosted CI, complete migration | NOT_RUN | No performance, repository-wide, merge or global-ticket closure claim |

The first full-profile attempt was interrupted to freeze a Python-version
compatibility fix; it produced no complete capture. Earlier mutable-source
Python attempts are not substituted for the frozen 181-test receipt.

The `e6650669` full-profile attempt **FAILED**: the registered runtime bench
build exceeded its 1,800 s producer deadline; the owned process group was
terminated. LQ completed, but no complete profile was promoted. This is a
build-time failure, not a measured slow search result. The latest snapshot
`83f24879` has a separate in-progress diagnostic capture at
`/tmp/quanta-micro-audit.EwaPQA/evidence-current`, with producer deadline 7,200 s
and `CARGO_BUILD_JOBS=2` to bound concurrent compilation on this contended host.
The exact capture command is:

```sh
env CARGO_TARGET_DIR=/Users/songmin/Library/Caches/quanta-index/target/e385f4e6b4fe8e9b/bench-lane \
  CARGO_BUILD_JOBS=2 uv run --frozen --extra dev python \
  tools/benchmark/benchctl.py run micro \
  --evidence-root /tmp/quanta-micro-audit.EwaPQA/evidence-current \
  --criterion-samples 10 --criterion-warmup 0.01 \
  --criterion-measurement 0.01 --criterion-resamples 1000 \
  --producer-timeout 7200
```

Raw process log: `/tmp/quanta-micro-audit.EwaPQA/current-micro-capture.log`.
An active process is not a success receipt; full capture remains **NOT_RUN**
until terminal success and fresh-process validation/replay are available.

Registry digest: `sha256:d501c218b0c4792d866c104b724ca49f292b32d91314d9379ab257a1ca36a3f0`.
`Cargo.lock` and `uv.lock` digests remain the values in the historical block.
Frozen `e6650669` implementation SHA-256 identities (later native-build replay,
summary, POSIX refusal and lint fixes require a new frozen receipt):

| File | SHA-256 |
| --- | --- |
| `criterion_capture.py` | `eac84e50a80dc27350b82fbc4e0f98336a764a5ef8dbea6df51e2b6a3149b007` |
| `profile_capture.py` | `4dd6593d2ba429f706de13d019ec1c4484b3a72c013322b13e5b8189255b4e8c` |
| `custody.py` | `4312fa30334523fd0b3ca60c2e0ecffc9d5f0c1d6375e9a221c985bb2095103d` |
| `benchctl.py` | `cd25cd26f00eed9a95a10ec8b2da43fbca822fc784562af3b805dc9134beac85` |
| `evidence.py` | `70fed09e42a464e0d73bde23d126d57099a63ed1d6177239dbfc132c0fc3c0be` |

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

### BM-06 follow-up — recorded import contract proof

Implementation is on shared dirty main based on
`f9c3b4dc487a1b54a260e4ab2d3dd199310d3a5e`. Its clean private proof snapshot is
`9e0f9371b8b238f6db9f775154afe908efad9fcb` at
`/tmp/quanta-micro-audit.EwaPQA/proof-recorded` (not a main commit).

- `recorded_capture.py` and the common CLI now import explicit external
  A/B/C JSONL plus a JSON collection of scan-native artifacts. They retain raw
  inputs and the complete recomputed agent summary (including cost, pair
  identity, paired wins/losses/ties and conditional evidence metrics).
- All families are validated before publication; immutable complete-profile
  records use the existing custody/GC contract. Scan row percentiles and
  error/timeout counters are retained. Missing evidence is not zero timing.
- No arbitrary producer or coding agent is launched. Source/build/host in the
  envelope identify the importer; original producer claims stay in raw input.
  Submitted rows are explicitly unauthenticated diagnostics, not a trusted
  benchmark result. Authenticated imports are refused without downgrading.
- A real clean CLI run found the source-closure boundary passing a bare hash
  into the typed `sha256:` vocabulary. The bridge now strictly validates and
  prefixes the engine's bare hash; a real Git/closure-engine regression crosses
  the typed envelope boundary. A second fresh-process check found scan
  collections being validated twice as both recorded collections and a single
  native envelope; recorded runs now use their recorded replay owner only.

Latest focused frozen suite: **VERIFIED**, 272 passed in 59.38 s, using the
ten-file suite above plus `test_recorded_capture.py` and
`test_agent_outcome_benchmark.py`; receipt
`/tmp/quanta-micro-audit.EwaPQA/recorded-authority-final-frozen.xml`, SHA-256
`b8a178dcb9e59160cafd41c2fe4c7bfa9a1da7aafd7323fbd226716923df4521`.

Actual CLI contract sequence at that clean snapshot:

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py run recorded \
  --evidence-root /tmp/quanta-micro-audit.EwaPQA/recorded-cli-final \
  --agent-recording /tmp/quanta-micro-audit.EwaPQA/recorded-fixtures-v1/test_complete_recorded_capture0/agent.jsonl \
  --scan-recording /tmp/quanta-micro-audit.EwaPQA/recorded-fixtures-v1/test_complete_recorded_capture0/scan.json \
  --recorded-authenticity recorded_unauthenticated
uv run --frozen --extra dev python tools/benchmark/benchctl.py validate recorded \
  --evidence-root /tmp/quanta-micro-audit.EwaPQA/recorded-cli-final
uv run --frozen --extra dev python tools/benchmark/benchctl.py replay \
  --family agent-outcome --evidence-root /tmp/quanta-micro-audit.EwaPQA/recorded-cli-final
uv run --frozen --extra dev python tools/benchmark/benchctl.py replay \
  --family scan-vs-index --evidence-root /tmp/quanta-micro-audit.EwaPQA/recorded-cli-final
```

All four commands exited 0; both replay receipts say `re_derived` and
`recorded_unauthenticated`. The inputs are deliberately test-generated
fixtures: **contract proof only**, not real recorded-agent outcome or scan
performance qualification. External receipts:

| Artifact | SHA-256 |
| --- | --- |
| `recorded-final-import.json` and identical `recorded-final-validate.json` | `7ca36c5c38d30a85d90e007691deb63243b73f6089ad149dde265b5eb3b6c1dd` |
| `recorded-final-replay-agent.json` | `03d387d2a8563191021637e88f7fe4dfb0d770f11819522b79516b878681a2d0` |
| `recorded-final-replay-scan.json` | `10cc3b52a05611d1f8fe7e4a1c674ec5d55ba2bb12281e1f3dbe7ba4aebe094f` |
| `recorded-final-summary.json` (present, unvalidated inventory only) | `9b3b97861dad36987005ff90f9d4750c7a7aa80b3690843b6962c30555146651` |

All paths are under `/tmp/quanta-micro-audit.EwaPQA/`. A fresh CLI request with
`--recorded-authenticity authenticated` exited 2 with empty stdout and left the
existing profile pointer byte-for-byte unchanged; raw refusal is
`recorded-final-auth-refusal.stderr`.

Micro follow-up: the `83f24879` run was deliberately interrupted after the
real CLI uncovered its shared source-digest promotion bug. No complete capture
was promoted. The corrected `9e0f9371` run is active with external root
`/tmp/quanta-micro-audit.EwaPQA/evidence-authority-final` and process log
`/tmp/quanta-micro-audit.EwaPQA/micro-authority-final.log`. It uses the same
10-sample/0.01 s diagnostic knobs, `CARGO_BUILD_JOBS=2`, and 7,200 s producer
deadline. Successful capture will be followed by fresh-process `validate` and
both family replays in that same clean snapshot. This supersedes the older
in-progress root above; it is **NOT_RUN** until that sequence terminates.

| Owner | Required code work | DoD |
| --- | --- | --- |
| BM-04 | Complete the actual full-profile Criterion capture/validate/replay receipt | Adapter and focused custody tests exist; require both registered targets and every case through the clean frozen CLI, then fresh-process raw-derived replay. Diagnostic ns only; instruction and performance rails are not implemented/qualified. |
| BM-05 | Paired CLI execution/evidence adapter, fresh live lexical pilot and actual contract-profile receipt | Contract and lexical recorded-input adapters exist; lexical frozen contract tests and CLI replay are VERIFIED above. Require terminal actual SDK/contract run plus fresh validation/replay, paired common execution with external corpus/query/gold paths, and a fresh real-product/corpus lexical pilot. Preserve file/span and judged/unjudged spaces and reuse scorer owners. |
| BM-06 | No remaining unauthenticated import implementation gap in this audited scope | Clean-source import/validate/two-family fresh-process replay is VERIFIED on fixed fixtures. Authentic recording verification and real-agent outcomes remain NOT_RUN; the CLI rejects authenticated claims rather than fabricating that proof. |
| BM-04/07 | Native build/binary inventory and capture-time host lease | Exact executed binary/toolchain/flags bound before execution; monitored lease loss and generator saturation exclude speed/capacity qualification; clean-source representative capture and hosted CI receipts |

BM-00/01/02 infrastructure remains in place. BM-03 is a partial execution
surface, BM-04 has native wiring/fixture coverage but not complete micro support,
BM-05 has contract-proof and lexical recorded-input adapters but no completed
paired common adapter or fresh live lexical pilot;
BM-06 has a verified unauthenticated diagnostic import contract, not real-agent
qualification. BM-07 cannot claim global cutover while the other gaps remain.

External license/gold/model/quiet-host admission belongs to qualification, not
this implementation list. No fabricated `PAIR_VALID`, `QUALITY_DELTA`, speedup,
capacity or product-win result is issued here.
