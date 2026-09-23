# Sep 23 gate follow-up — shared checkout

Status: `BLOCKED — 1e9f9475 code rails passed; newer main source and TOPT-00 timing remain`

This is a current shared-worktree repair record, **not** a clean-HEAD or
performance qualification. At initial capture, `main` was
`384cecb5825d8ebf38ff6c07bc6bf64ffc43fbe0`, 16 commits ahead of
`origin/main`. The repairs in this note were committed as
`e42cb827b39197bae83752bec0cfb69aa572daae`; no clean checkout of that
commit was qualified. A clean checkout of its doc-only successor
`d9b39c392d0e617ac0656a908bd0a85d755fc525` passed
`just rust-public-api` but failed `just rust-clippy` at the first five
uncommitted lint fixes in `lq-regex` and `lq-norm` tests; this is not a
complete Clippy error inventory. Other writers had uncommitted changes in catalog, embed,
repomap, SDK, search-plane, searchd, benchmark Python/schema, and the
TOPT-08/RCA tickets. The 33 Rust owner changes were later reviewed and
integrated in `f478f69e5e0006afb7ea36360be9b4dd7558d252`. The separate
Sep 23 retrieval benchmark edits remain excluded.

## Gate root causes repaired locally

- `quanta-index-retrieval-bench` introduced strict-Clippy failures in its
  newly added chunking, record, SDK, CLI, and integration-test surfaces.
  The owner fixes use checked spans/counts and typed errors; result-record
  construction is fixed-shape JSON instead of panic-bearing repeated map
  insertions. `chunking/mod.rs` is a facade over `core.rs` to satisfy the
  module-discipline gate without changing its public re-exports.
- The searchd harness had strict-Clippy failures in its open-loop accounting,
  report writers, readiness path, and test target. Counters and artifact
  assembly now fail closed on overflow/missing route budgets; the readiness
  transport semantics remain unchanged. Runtime integration test cleanups
  remove lint-only indexing, redundant borrows, and unfulfilled expectations.
- The public-API baseline lacked the two exported views of the already
  committed `FileOwnerProjectionErrorV1: Error` implementation. The canonical
  baseline update adds those two lines; it does not add another API change.
- `rust-doc` found two broken links: the migration fault-port comment named
  a nonexistent function, and the signal documentation linked an unimported
  `CancelRoot`. The links now target the actual types.
- Three ignored `artifacts/search-quality/*/latest` bundles claimed an older
  Git HEAD. They were moved intact to
  `artifacts/search-quality/_stale/2026-09-23-head-384cecb/` rather than
  deleted. The policy gate then passed with **zero** artifacts attributed to
  current HEAD; this is not benchmark proof.

## Current receipts and exclusions

| Rail | Result | Qualification limit |
|---|---|---|
| `just fmt-check` | pass | shared dirty tree |
| `just rust-clippy` | pass on shared dirty tree; fail on clean `d9b39c3` | clean checkout first reports 3 `lq-regex` and 2 `lq-norm` test lint errors; no clean-HEAD green |
| `just rust-policy`; `just rust-machete` | pass; pass | policy sees zero current-HEAD benchmark artifacts |
| `just rust-public-api` | pass | baseline update is in `e42cb82`, but no clean-commit rerun |
| `just rust-doc` | pass after link repairs | shared dirty tree |
| `just rust-bench-build` | pass; optimized workspace benches compiled, not executed | shared dirty tree; no timing evidence |
| `test-fast` selector via `./scripts/cargow --lane test-fast-lane test --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime --quiet` | pass, exit 0 | exact recipe first ended with SIGTERM/143 mid-run; same selector's quiet-output rerun passed; runtime crate excluded by profile |
| benchmark library / chunking contract / real SDK roundtrip | 17/17, 16/16, 6/6 pass | rerun after the chunking facade move; focused, dirty-tree tests |
| harness library / open-loop binary tests | 93/93, 8/8 pass | focused tests |
| runtime fast+risk+extended selectors | 257 pass, 1 fail, 1 skip out of 258 run | not green; see below |

The runtime failure was
`e2e_top_k_truth_table::the_public_maximum_reports_the_continuation_over_ten_thousand_and_one_rows`:
fixture ingest exceeded its existing 600-second IPC read deadline. Other
Cargo/nextest/rustc work was running on the same Mac. The prior isolated
tracked tree passed the complete daemon selector, but that is **not** a pass
for this dirty source. Do not increase the timeout, skip the test, or infer
performance from this contended execution. Run the exact selector again on a
quiet host, then investigate producer cost if it still fails.

## Required closeout

1. Reconcile the named foreign dirty owners and freeze one clean tracked
   source. The adjacent TOPT-08 and RCA documents are peer-edited; merge
   their claims only after their writer has released them.
2. Re-run the failing 10,001-row selector on an uncontended host. Preserve the 10,001-row
   real-daemon proof until an equivalent independent oracle exists.
3. Complete the exact-source `verify-rust` gate. A Clippy pass in this shared
   checkout is not a committed-tree pass.
4. Keep TOPT-00 open: the pre-implementation admission record cannot be
   reconstructed. Collect an explicitly **retrospective**, quiet-host paired
   before/after comparison for R1–R4 and TH-4, with selector/count/cache
   metadata and five warm samples where specified; record the historical
   admission gap rather than silently waiving it.
5. Do not emit `CODE_QUALIFIED`, `PERF_EVIDENCE_CLEAN`, or
   `PRODUCT_QUALIFIED` from these partial receipts.

## Attempted integrated-source gate — `f478f69` (withdrawn)

**Do not use the following Cargo-dependent passes as qualification.** The
checkout itself was clean, but `scripts/quanta-index-env.sh` keyed its Cargo
target directory only by lane, so concurrent `main`, linked-worktree, and
temporary-worktree builds wrote the same target files. A repeat Clippy run in
the `f478f69` checkout compiled its `search-plane` against a `core` source
path in the shared `main` checkout and failed with mismatched `reserve` and
`ProviderAuditEventV1` signatures. This proves that checkout cleanliness
alone did not bind the compiler inputs to that source. The prior test-fast,
integration, rustdoc, and public-API outputs are observations, not admitted
same-source receipts. The daemon run cannot be promoted either.

The existing isolated checkout was moved from `d9b39c3` to clean commit
`f478f69e5e0006afb7ea36360be9b4dd7558d252`. On that source,
`just fmt-check`, `just rust-policy`, `just rust-machete`,
`just rust-public-api`, and `just rust-doc` passed.
`just rust-profile test-fast` exited 0.
`just rust-profile test-integration` exited 0:
204/204 fast, 6/6 storage, 58/58 semantic (nextest runs
`cfb14792-2786-487a-996c-9863e2b010ea`,
`c90dc8e5-85e1-4892-8aed-eb570f32f5cb`, and
`76c805c5-8721-4944-911d-8fe813c5163e`). The
policy gate found zero benchmark artifacts attributed to that HEAD. These
results are not a full `verify-rust` or runtime qualification receipt and the
Cargo-dependent subset is invalidated by the artifact collision above.

At capture, another repository had active Cargo compilation and a generated
mutation run on the same host. TOPT-00's quiet-host condition was not met.
The 10,001-row runtime failure therefore remains an investigation target,
not an established load or producer-cost RCA. Keep the exact 10,001-row
semantic oracle and its timeout unchanged until a source-bound, uncontended
rerun distinguishes host contention from implementation cost.

## Target-cache owner repair

Commit `b7efbb3` scopes each Cargo lane under a SHA-256 identifier of the
canonical checkout path. Direct public-API and cargo-modules scripts use the
same namespace. The regression test proves two distinct checkout paths with
the same cache root and lane receive different target directories; all 10
`test_cargow_lane_env.py` cases and Ruff checks passed. Metadata from the
real `main` and isolated qualification worktree showed distinct target paths.
This prevents cross-worktree target-file races but intentionally causes a
one-time cold rebuild per checkout. A fresh qualification must use this
repair and a source-bound checkout; the previous shared-cache receipts are
not reusable.

On clean commit `820cf8e3c6c11a5943e36bf3705859d46f1fbc7c`, metadata
resolved separate main and qualification target paths. With sccache disabled,
the full `quanta-index-search-plane` Clippy target set passed in that isolated
target after repairing a new W10 test's long first doc paragraph and panicable
index assertions. Its exact audit-correlation test passed 1/1 from the same
source. This is owner-local proof, not a workspace Clippy or `verify-rust`
pass. At this point, the shared `main` checkout also had unrelated active
SDK, IPC, query-dispatcher, benchmark, and prompt-control edits; none of
their dirty-tree results can be adopted as this ticket's qualification.

## Retrieval benchmark gate repair — `2e8dce9`

The first isolated full-workspace Clippy attempt exposed four new
retrieval-benchmark lint errors in the committed Sep 23 runner. Commit
`3efddfe` replaced linted string addition in fixture digests and made the
stale-generation outcome match explicit; isolated benchmark all-target
Clippy passed. The next policy run found a real module cycle:
`batch -> record -> sdk -> batch`. `batch` needed canonical JSON for receipt
digests, not the entire runner-record layer. Commit `2e8dce9` moved the
canonical JSON authority and its two semantic tests to a lower `canonical`
module; the module-cycle check then passed without a baseline exception.

On clean `2e8dce9`, `just fmt-check`, `just rust-policy`, `just rust-machete`,
isolated benchmark all-target Clippy, and benchmark library tests (28/28)
passed. The exact `sdk_roundtrip` integration binary passed 8/8 with a
same-checkout searchd executable explicitly pinned at SHA-256
`150c2e6edc4d42a2a9131730fe7d61d9250ba2f606d291757749b598fdb0183f`.
An earlier 6/8 run without `QUANTA_INDEX_SEARCHD_BIN` was a verification
setup error, not a code failure; it is not a green receipt. These are
source-bound owner and policy rails, not the full TOPT-08 verdict.

Still required: isolated full-workspace Clippy and runtime-profile gates,
an uncontended recheck of the 10,001-row ingest timeout, and TOPT-00's
retrospective paired timing protocol. The historical missing
pre-implementation admission record cannot be recreated.

## Sep 23 integrated-source recheck — `5b24c34`

The isolated checkout at `2e8dce9` also completed
`just rust-profile test-fast` with exit 0 using the checkout-scoped target;
this is not a result for later commits. Subsequent clean-source workspace
Clippy attempts exposed newly committed searchd, IPC, SDK, search-plane,
searchctl, and retrieval-benchmark compile/lint failures. The owner repairs
landed through `5b24c348be4ca2f4ac156c0fca4e751dfeabf6d5`. In particular,
the new query response variants are matched explicitly, and the SDK test
request helpers use checked `split_last` custody rather than panicable
slicing. No other writer's dirty files were included in these commits.

On clean isolated `5b24c34`, `just fmt-check` and `just rust-policy` passed;
the policy rail found **zero** benchmark artifacts attributed to this HEAD.
SDK and search-plane all-target, all-feature strict Clippy passed together;
their library tests passed 102/102 and 401/401. Earlier full-workspace
Clippy runs were stopped only after decisive errors or source drift, and
none is a full-workspace green receipt for `5b24c34`. The shared `main`
checkout still has another writer's prompt-manager dirty paths, excluded
from these isolated-source results.

TOPT-08 remains open: complete the exact-source full Rust and daemon rails,
rerun the 10,001-row ingest case without competing host load, and collect
TOPT-00's retrospective paired timing. A contended timeout is not yet a
producer-cost RCA; do not increase its deadline or weaken the oracle.

## Sep 23 source-bound remediation — `87f4e797`

The current code source was exercised in the clean, checkout-scoped detached
worktree `sep23-gate-qualification/quanta-index` with sccache disabled. The
shared `main` checkout still contains another writer's prompt-manager changes;
they were excluded from every commit and source-bound Cargo receipt below.

- Removed redundant crate-local `multiple_crate_versions` expectations. The
  workspace already allows this Clippy lint and cargo-deny owns dependency
  duplication. The explicit expectations forced Clippy's recursive dependency
  graph traversal and caused an 18-minute-plus lint run. A workspace-lint
  guard now prevents reintroducing the redundant expectations.
- Repaired exhaustive IPC response matching in the harness and runtime tests.
  The new active-generation response variants are rejected in routes that do
  not consume them, rather than swallowed by a wildcard.
- The peer-watch test exposed a real ordering race: a disconnect event could
  be observed before cancellation. The owner now cancels first and publishes
  the event second; the callback test synchronizes on callback entry.
- Five daemon E2E queries and the SDK frontdoor ingest fixture issued request
  ID zero, which the current IPC contract rejects. They now allocate nonzero
  IDs at the request producer. The SDK regression helper asserts that invariant.
- A concurrent activation test originally hid query errors behind a polling
  atomic. An event channel surfaced the actual `INVALID_REQUEST`: the SDK
  resolved an `Active` generation before activation, then sent its stale
  explicit pin after activation. The server still refuses the inconsistent
  query, but now classifies this *retryable active-selector drift* as typed
  `NOT_READY`. A mismatched fixed `Pinned` selector remains `INVALID_REQUEST`.
  Unit tests cover active lexical, semantic, and hybrid drift and pinned
  mismatch. The E2E keeps its original five-second deadline, requires a
  complete generation-two result, and tolerates only typed `NOT_READY` while
  activation is in flight.

On clean `87f4e797`, `just rust-profile test-fast` passed; the search-plane
library passed 402/402. `just rust-profile test-integration` passed with
210/210 fast, 6/6 storage, and 58/58 semantic cases. Strict-warnings
`just rust-doc` passed. Full-workspace `just rust-clippy`,
`just fmt-check`, `just rust-policy`, `just rust-machete`, and
`just rust-public-api` passed on its predecessor `a6ec03fe`;
the sole subsequent code edit changed a search-plane test assertion from an
obsolete `INVALID_REQUEST` expectation to the exact `NOT_READY` drift code
and message. Do not call those static receipts exact-HEAD results until rerun.

`test-daemon-all` is not green on this source. The default four-thread run on
`a1d6c0fb` failed four 30-second IPC reads while other Rust workloads were
active; each failed case passed independently with one test thread. A
one-thread full run then exposed the activation drift above and failed at
170/322; that root cause was fixed and its focused owner/E2E cases passed,
but the complete selector has not been rerun after the fix. Neither earlier
fail-fast run is a full-suite receipt. A prior 10,001-row fixture ingest
timeout under host contention likewise remains unresolved; do not call it a
producer regression or delete/relax the oracle. TOPT-00 quiet-host paired
performance evidence and the irrecoverable historical pre-implementation
admission record remain separate open items.

## Frozen-source qualification result — `1e9f9475`

In the clean checkout-scoped detached worktree, with sccache disabled:

- `just fmt-check`, full-workspace `just rust-clippy`, `just rust-policy`,
  `just rust-machete`, `just rust-public-api`, and
  `just rust-cargo-modules` passed.
- `QUANTA_INDEX_TEST_THREADS=1 just rust-profile test-daemon` passed:
  203/203 executed tests, one policy skip.
- The same one-thread `test-daemon-all` passed: 322/322 executed tests,
  one policy skip. The 10,001-row top-k real-daemon oracle passed in both
  selectors with its existing 600-second IPC deadline unchanged.
- `just rust-fuzz-smoke` passed all four 60-second nightly targets:
  IPC request, IPC response, search-corpus ingest, and parse pipeline.
- `RUST_TEST_THREADS=1 just rust-profile verify-rust` exited 0, including
  format, strict Clippy, policy, dependency hygiene, bench `--no-run`, full
  workspace tests, and strict rustdoc. Its runtime risk binary passed
  137/137, including the activation-concurrency and 10,001-row cases.

This is a `CODE_QUALIFIED` receipt for the frozen `1e9f9475` tree only. While
that isolated rail ran, peer commits advanced shared `main` to at least
`7d4994b9`, changing search-plane readiness, searchd runtime/state migration,
and the retrieval SDK. The current shared checkout also has peer-owned dirty
benchmark and receipt-generator edits. Do not promote the `1e9f9475` full
rail to a later HEAD or dirty overlay. Re-freeze and rerun the impacted rails
after those owners finish. The passing contended 10,001-row runs retire the
old *correctness* timeout concern on `1e9f9475`, not TOPT-00's quiet-host
performance requirement or its irrecoverable pre-implementation admission gap.
