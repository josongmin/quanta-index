# Sep 23 gate follow-up — shared checkout

Status: `BLOCKED — dirty-source ownership, one runtime timeout, and TOPT-00 timing evidence`

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
TOPT-08/RCA tickets. Do not stage those paths from this follow-up.

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
