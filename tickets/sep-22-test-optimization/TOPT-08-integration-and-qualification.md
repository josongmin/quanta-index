# TOPT-08 — Same-Source Integration and Qualification

Status: `blocked — exact-source full Rust rails, 10,001-row timeout investigation, uncontended performance evidence`

Depends on: TOPT-01 through TOPT-07

Aligned S21 owner: S21-13

## Goal

Integrate the owner fixes without patch-on-patch helpers, then produce honest
correctness and performance evidence from one final source.

## Integration audit

1. Freeze final `HEAD`, dirty digest, toolchain, and target inventory. Verify
   the target root is checkout-scoped and no explicit preservation override
   reconnects it to another worktree's Cargo artifacts.
2. Re-run the 18-row crosswalk against current source; no `OPEN`, stale path,
   duplicate owner, or compatibility shadow path may remain.
3. Confirm production behavior did not change unintentionally:
   - persisted Unix lease semantics unchanged;
   - provider jitter/retry classification unchanged;
   - cancellation remains fail-closed;
   - runtime isolation is not replaced by global fixture sharing;
   - lower-layer resource-envelope authority remains.
4. Inspect diff ownership: clock/env, wakeup, fixture, provider, oracle, helper,
   and coverage changes must each land in their named owner.

## Verification order

1. Each ticket's focused rails.
2. `just fmt-check`
3. `just rust-clippy`
4. `just rust-profile test-fast`
5. `just rust-profile test-integration`
6. `just rust-profile test-daemon`
7. `just rust-profile test-daemon-all`
8. `just rust-profile verify-rust`

Escalate with `just rust-public-api`, `just rust-fuzz-smoke`,
`just rust-hexagonal`, or `just rust-cargo-modules` only when their governed
surface actually changes, as required by `AGENT_PLAYBOOK.md`.

## Performance closeout

- rerun only on a quiet host under TOPT-00 protocol;
- compare exact selectors, source metadata, execution counts, cache class, and
  raw logs;
- report R1-R4 and TH-4 independently; do not hide regressions in one aggregate;
- R5 closes by conserved coverage plus one fewer daemon scenario, not by timing
  alone.

## Verdicts

- `IMPLEMENTED`: owner diffs and focused rails complete;
- `CODE_QUALIFIED`: full required local Rust rails pass on exact source;
- `PERF_EVIDENCE_CLEAN`: uncontended before/after protocol passes;
- `PRODUCT_QUALIFIED`: reserved for the existing S21 aggregate proof graph;
- `BLOCKED`: missing host, stale source, foreign dirty overlap, or failed rail.

This packet cannot emit `PRODUCT_QUALIFIED` by itself and must not modify the
proof registry to manufacture closure.

## Done

All 18 rows are implemented and evidenced, no required rail is failed/skipped,
timing evidence is source-bound and uncontended, and remaining S21 product
qualification gaps are reported separately.

## Evidence — Sep 23 Clippy sweep and broader rails (dirty tree)

`HEAD` at capture: `81fcec7`. Dirty-tree receipts, not frozen-source
qualification: peer lanes were editing `searchd-runtime`, `searchd-harness`,
`benchmarks/retrieval`, and `tools/benchmark` while these rails ran (89 dirty
paths at closeout). Re-freeze and re-run before promoting any verdict.

Clippy sweep (this lane, behavior-preserving lint hygiene, 33 `.rs` files):
`catalog` src + tests, `embed` lib/model2vec/openai/retry/pool, `lq-norm`,
`lq-regex`, `repomap` materializer/store/candidate-activation-test, `sdk`
binding/repomap/tests/sdk-binding-test, `search-plane` single-flight,
control/ingest/query dispatchers + tests, `searchctl`, `searchd`
config/runtime/searchd. Production semantics unchanged; three fail-closed
error messages gained a cause suffix (`cursor_key` id/material length,
`materializer` manifest custody digest, `config` HOME lookup) with no
dependents on the old text.

- `just fmt-check`: exit 0, full workspace.
- `just rust-clippy` (CI recipe, `--workspace --all-targets --all-features
  --locked -- -D warnings`): exit 101 with errors ONLY in
  `benchmarks/retrieval` (152 across 8 files: batch, chunking
  fixed-window/mod/syntax/whole-file, corpus, record, sdk). Every other
  workspace target is clean. `benchmarks/retrieval` is the retrieval
  benchmark lane's active work area (commits `1955700`, `de78a31`,
  `ad35484`, … plus uncommitted `tools/benchmark/retrieval/*`); left to
  its owner per shared-tree discipline, mostly arithmetic/indexing/doc
  debt in chunking math where saturating-vs-wrapping is evidence-critical.
- `just rust-profile test-fast`: exit 0, 44 suites, 1859 passed / 0 failed.
- `just rust-profile test-integration`: exit 0, 204 + 6 + 58 passed /
  0 failed across fast/storage/semantic slices.
- `just rust-profile test-daemon`: exit 0, 203 passed / 1 skipped.
- `just rust-profile test-daemon-all`: exit 0, 304 passed / 1 skipped.
- `just rust-policy`, `just rust-machete`: exit 0.
- `just rust-doc`: exit 101 on peer-active files only
  (`searchd-runtime` signal `CancelRoot` link, `searchd`
  `state_format` fault-port link); the owning lane is fixing these in-tree
  (uncommitted fixes observed); untouched here.
- `just verify-rust`: not green — blocked on the two foreign items above.
- Timing evidence: still blocked on a quiet host (pre-existing TOPT-00).

No `CODE_QUALIFIED`: the workspace gate is red on foreign-owned debt and no
frozen-source re-run exists yet.

## Sep 23 integrated-source update

The preceding Clippy and rustdoc failures describe the historical `81fcec7`
snapshot, not the current gate state. The benchmark/harness, rustdoc, and
public-API repairs landed in `e42cb82`; the 33-file workspace Clippy owner
sweep landed in `f478f69e5e0006afb7ea36360be9b4dd7558d252`. The latter
commit excludes the concurrently edited Sep 23 retrieval benchmark lane.
An attempted clean checkout of `f478f69` exposed a shared Cargo target-dir
collision with newer worktrees. Its Cargo-dependent test and lint outputs
are withdrawn as same-source qualification, even when the command exited 0.
The target owner was repaired in `b7efbb3`; rerun the full gate with the
checkout-scoped target directory. No earlier dirty-tree or shared-target
pass is promoted to a clean-source receipt.

Owner-local recheck on `820cf8e`: isolated-target search-plane Clippy passed
for all targets, and the exact provider audit-correlation test passed 1/1.
The full workspace rails and uncontended 10,001-row E2E/timing protocol are
still outstanding.

On later isolated source `2e8dce9`, the retrieval benchmark's four current
lint errors and a `batch -> record -> sdk -> batch` module cycle were repaired
at their owners. Benchmark all-target Clippy, library tests (28/28), real
SDK roundtrip (8/8 with a pinned same-source daemon), `fmt-check`, policy,
and dependency hygiene passed. This remains owner-local qualification; full
workspace Clippy, daemon-all, and uncontended performance evidence have not
yet passed on that source.

The remaining runtime investigation is the 10,001-row top-k E2E: a shared,
contended-tree execution timed out during fixture ingest at its existing
600-second IPC read limit. This is a failed run, not proof of a producer
defect or a reason to increase the deadline or remove the oracle. TOPT-00
still lacks the retrospective quiet-host paired timing comparison and its
pre-implementation admission gap remains recorded.

On clean isolated `5b24c34`, `fmt-check`, policy, SDK/search-plane strict
all-target Clippy, and their library tests (102/102, 401/401) passed.
The earlier clean `2e8dce9` also passed `test-fast` with exit 0, but later
code changes require a fresh final-source run. Full-workspace Clippy has
not passed on `5b24c34`; prior attempts found and repaired owner errors,
then stopped or became source-stale. Daemon-profile rails, the 10,001-row
quiet-host recheck, and retrospective paired performance evidence remain
open. No `CODE_QUALIFIED` or `PERF_EVIDENCE_CLEAN` verdict is emitted.

## Current code receipt and remaining qualification — `87f4e797`

Clean checkout-scoped, sccache-disabled `test-fast` and `test-integration`
passed on `87f4e797` (search-plane library 402/402; integration 210 + 6 +
58). Strict rustdoc passed. The full-workspace static gates passed on
`a6ec03fe`, one test-oracle edit earlier. Focused active-generation drift
unit/E2E tests passed after the server began returning typed `NOT_READY` for
stale `Active` pins while retaining `INVALID_REQUEST` for fixed-pin mismatch.
The activation E2E still requires an observed complete generation two within
its original five-second bound.

This does **not** emit `CODE_QUALIFIED`: final-source `test-daemon-all`,
`verify-rust`, and any surface-triggered fuzz rail are not complete. Earlier
daemon-all runs failed before all cases executed: four contended 30-second
IPC read timeouts at four threads; then the now-fixed activation drift at one
thread. The 10,001-row ingest timeout must be retried on a quiet host without
changing its limit. `PERF_EVIDENCE_CLEAN` is blocked independently on TOPT-00's
retrospective paired protocol. The missing historical admission record cannot
be retroactively manufactured.
