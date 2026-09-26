# L3 RCA and error-boundary audit

**One additional P2 defect reproduced and repaired. The repair and 193 selected tests
are VERIFIED on the frozen snapshot below. Latest shared-source qualification is BLOCKED.**
This claim covers the native L3 collector boundary and the selected lexical tests,
not whole-repository qualification.

HEAD `5571132655a83824731e7909b0e310951edad52b`, dirty shared `main`.
Owned edits: `budgeted_search.rs`, `ranked_page.rs`, `ranked_page_tests.rs` under
`crates/quanta-index-lexical/src/`. Existing edits in core, dependencies, tooling
and other lanes were preserved. No commit, push, reset, delegation or inter-task
communication was performed.

## Root cause and previous coverage gap

1. Tantivy's collector returns an outer `Result`, while these segment collectors
   return another `Result` as their fruit. The search wrapper checked only the
   outer error and the shared budget/interruption state.
2. Callback failures used `CollectionBudget.abort()` to stop the scorer. That
   flag carried no error value; the original error remained inside the fruit.
3. The earlier grouped-harvest fix stopped later segments but still admitted the
   failed fruit to merge. Grouped merge consumed earlier successful fruits before
   reaching a later failed fruit. Its work/byte operations could replace the
   original integrity error with a resource refusal.
4. The earlier regression corrupted the first visited segment. With no preceding
   successful fruit, merge reached its error before spending resources and missed
   this ordering-dependent failure.

Actual red: a malformed UTF-8 group dictionary in the second visited segment,
path grouping and work limit 6 returned `LexicalCollectionBudgetExceeded` instead
of Storage. `rca-red-2` exited 101 with one failing test on stable selected inputs.
The initial `rca-red` attempt timed out at resource admission; no test ran there.

## Repair and adjacent-path review

- `budgeted_collection` now requires fallible child fruits and extracts their
  errors immediately after each segment, before retention, another segment or
  merge. Ranked, whole-set and grouped production routes all use this boundary.
- Direct grouped merge checks every segment result for the first error before
  consuming successful fruits or reserving/charging merge resources. It returns
  the original error without allocating another result buffer.
- Existing callback and grouped-harvest abort behavior remains in place. The new
  boundary does not depend on that side channel, including ranked harvest errors.
- Typed resource/cancellation refusal remains authoritative when already recorded.
  Weight/segment setup errors already return through the outer Result. Ranked
  merge already preflights fruit errors while calculating capacity. No additional
  reproducible defect was found in these reviewed sibling paths.

Three new terminal tests cover 22 subcases, not 22 additional test counts:

- Real dictionary corruption: first/middle/last segment × path/repo grouping ×
  tight/ample work budget = 12 cases. Assert original error, exact visited count,
  no post-error merge work and complete reservation release.
- A fallible collector with no budget handle: three failure positions and a
  successful control × two budgets = 8 cases. Failure must invoke no merge.
- Direct grouped merge with a successful fruit, two distinct errors and exhausted
  work budget: both grouping modes preserve the first error without resource use.

The corruption fixture runs below sealed-generation admission. It proves native
error propagation, not a seal bypass or installed product corruption injection.

## Frozen verification and latest-source boundary

`rca-final-2`: **175 library + 3 cancellation + 4 execution-budget + 6 exact-source
+ 5 ranked-pages = 193 passed, 0 failed, 0 ignored.** All selected inputs were
unchanged from command launch through completion and the initial closeout check.
Before publication, other work then changed eight selected source files: five
core files plus `query_admission.rs`, `searcher/manual_scan.rs` and
`searcher/prepare.rs`. The receipt lists all eight paths and the capture digest. The three owned repair/test
files still match the verified snapshot. The latest shared source is therefore
BLOCKED for qualification; the frozen run remains valid evidence for its source. An unrelated
`tools/benchmark/profile_capture.py` edit is outside this command's dependency and
execution-tool closure. The closure includes local Cargo dependencies, selected
integration fixtures, Cargo config/lock/toolchain, actual wrapper/admission helpers
and the resolved SSTable Git source bytes.

```sh
QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1200 \
./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical --lib --test execution_budget --test ranked_pages --test l3_exact_source --test cancellation_inside_search --locked
```

The first green run `rca-final` also passed 193 tests, but a dependency changed
while it executed; it is **BLOCKED** as source proof and was replaced by the
stable re-run. A fixture delimiter caught by formatting was corrected while that
first command was still queued, before Cargo admission. Counts are never combined
across runs.

- Selected input SHA256: `13437d4c36caeeb10f716d9f1e0610cd77ed32c37a1f4418a6ff75e7b874435b`.
- Red log SHA256: `d0ca483423f9f5fe629c5a0ac79f95f36efbafab127ee17f3530c714bb3d07ce`.
- Green log SHA256: `eb94b002741e58678f1b74effe42168be024aae39991b708ba7905d96b47abb5`.
- Receipt: `L3_RCA.source.json`, SHA256 `2b3f7806f19bfed5bfdd2b0a507b736dd1cfaa82180c7100f4af5d736c3c81d0`.
- SSTable Git revision: `22b3c0e3b554faa8de7a55f917f90c90f2af7050`.
- Owned-file rustfmt and whitespace checks: **VERIFIED**.

Raw logs, source/dirty manifests, dependency metadata, binary hashes, the red and
final patches, and static checks are under `l3-proof/rca/`. The immutable successful receipt is
`l3-proof/rca/verified-snapshot-L3_RCA.source.json`. Prior L3 reports remain
historical; this report and receipt are the current handoff.

Whole-repository CI, unselected integration tests, SDK/CLI, installed-daemon E2E,
physical OOM, peak RSS, performance and ranking-quality qualification: **NOT_RUN**.
No whole-engine completion or absence-of-all-defects claim is made.
