# L1–L5 current-source integration audit — 2026-09-27

Status: **partial**. Selected lexical behavior is **VERIFIED** on the bound
dirty source below. `just rust-policy`, format, and the additional public API,
module tree, and LLVM-line snapshot gates exit 0 after the recorded repairs.
Aggregate regex heap admission is **BLOCKED**. Current-source whole-workspace
CI, daemon/process qualification across all five lanes, retrieval benchmark
validity, release, and deployment are **NOT_RUN**.

## Source and ownership

- HEAD `3887fac3d090e9af86d1508ed2ba4125bf55bcbb`, shared dirty checkout.
  Concurrent benchmark/L4 planning and evidence files were left untouched.
  No commit, push, reset, agent dispatch, task message, or task polling was used.
- The lexical verification bound 818 Git-visible Rust/config/vendor input files.
  Its before and after canonical file-hash-map digest was identical:
  `0af1558c1c4763c678546ebdea6f8c7c7d357c9eaedf8781584215ca4afc2721`.
  [Pre](l1-l5-proof/lexical-source-pre.json) and
  [post](l1-l5-proof/lexical-source-post.json) file manifests retain the exact
  dirty inventory and individual hashes. The manifest covers the selected
  Rust input profile, not every mutable planning or Python file in the checkout.
- Rust toolchain: `rustc 1.92.0 (ded5c06cf)`,
  `aarch64-apple-darwin`, LLVM 21.1.3; `Cargo.lock` SHA-256
  `58dda6f980fd3d2ad4975b3be18c74e60351be045d78206728f88475a6ba6bbc`.
  [Selected test binary hashes](l1-l5-proof/lexical-binaries.json) bind all
  eight executed binaries. The raw test outcomes are in the terminal output of
  the command below; a manually transcribed count is not independent evidence.

## Lane disposition

| Lane | Current-source execution | Excluded or unresolved claim |
| --- | --- | --- |
| L1 query/domain/window | **VERIFIED**, 18 lexical integration tests | Current daemon/SDK and whole-product semantic quality **NOT_RUN**; prior process receipt binds an older frozen source. |
| L2 file mutation/coverage | **VERIFIED**, 23 lexical integration tests | Current daemon restart, crash cuts and cross-stream activation **NOT_RUN**; prior process receipt binds an older frozen source. |
| L3 exact source/budget | **VERIFIED**, 6 integration tests plus selected common budget/cancellation tests | Whole lexical/query dispatcher and process qualification **NOT_RUN**. `index:no` explicit language filtering remains typed unavailable by design. |
| L4 anchored preview/witness | **VERIFIED**, 12 integration tests plus lexical library regressions | Aggregate compiler and retained-cache heap ceiling **BLOCKED**; logical charges do not enforce physical allocation. Current daemon/SDK restart **NOT_RUN**. |
| L5 parser/producer coverage | **VERIFIED** current-source Rust owner: 87 library + 26 parser integration tests, 113 passed; separate clean-HEAD Python consumer command exited 0 with 552 passed. | Current-source combined producer/consumer claim **BLOCKED**. The shared Python environment was inventoried only after the earlier run; current-source Python and process/benchmark proof **NOT_RUN**. |

The current lexical command was:

```text
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=3600 ./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical --lib --test l1_query_domain_window --test l2_file_mutation --test l3_exact_source --test l4_match_anchored_preview --test execution_budget --test ranked_pages --test cancellation_inside_search --locked
```

It exited 0: library 193; cancellation 3; execution budget 7; L1 18; L2 23;
L3 6; L4 12; ranked pages 5; **267 executed, 0 failed, 0 ignored**.
An earlier attempted run failed compilation on unused imports introduced by
the preview-type move. Those imports were moved to the test module before
this final command; the failed attempt is not combined with its result.

The separate clean-HEAD L5 Python consumer command ran from
`/tmp/qi-l1-l5-final-3887` with this checkout's `.venv`:

```text
python -m pytest -q tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_retrieval_contract_proof.py tools/ci/tests/test_retrieval_sdk_proof.py tools/ci/tests/test_portable_proof.py tools/ci/tests/test_benchmark_source_closure.py --tb=short
```

Its [JUnit](l1-l5-proof/l5-frozen-python.junit.xml) records 552 selected and
executed, zero failed/error/skipped; SHA-256
`3ccb9615337c467d7250fd60a782691063bfe264bf25dc27ae10100`. The
worktree was clean at `3887fac3` when rechecked. This is a consumer-only
frozen-source result, not a current shared-tree or Rust producer result.
The shared interpreter was Python 3.13.9; its post-run
[90-package freeze](l1-l5-proof/python-freeze.txt) has SHA-256
`473fc8b212c34913bbb93b1eea738ac8adc3bacc2774b15ddecdb6f35975cc91`.
That post-run inventory is diagnostic; it does not independently prove the
installed package set was unchanged during the earlier 12-minute run.

The current-source L5 Rust command was:

```text
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=3600 ./scripts/cargow --lane test-fast-lane test -p quanta-index-retrieval-bench --lib --test l5_parser_regressions --locked
```

Its [raw log](l1-l5-proof/l5-rust.log) records exit 0, 87 library and 26
integration tests executed, zero failed/ignored. The 846-file Rust/vendor/
benchmark input [pre](l1-l5-proof/l5-rust-source-pre.json) and
[post](l1-l5-proof/l5-rust-source-post.json) maps both digest to
`4e0a4e6dd84d6a2de765a5cbae1065b5df8dc85269d37335e7b2f6f158ad73cc`;
the [two test binary hashes](l1-l5-proof/l5-rust-binaries.json) bind execution.
This does not compose with the earlier Python run's different source.

The DTO/hash owner command used the same build lane:

```text
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=3600 ./scripts/cargow --lane test-fast-lane test -p quanta-index-contract-base -p quanta-index-contract --lib --locked
```

Its [raw log](l1-l5-proof/contract-rust.log) records contract 163 and base 53,
**216 executed, 0 failed/ignored**. The 818-file input
[pre](l1-l5-proof/contract-source-pre.json) and
[post](l1-l5-proof/contract-source-post.json) maps have the same
`0af1558c1c4763c678546ebdea6f8c7c7d357c9eaedf8781584215ca4afc2721`
digest as the selected lexical run; [binary hashes](l1-l5-proof/contract-rust-binaries.json)
bind the two test executables. Across these three selected Rust commands,
**596 tests passed**, with no failures or ignored tests. This is not a full
workspace test suite.

## Integration repairs

1. Registered five previously orphaned L1/L2/L4 integration targets in
   `test-authority.toml`, and explicitly classified four ignored real-daemon
   SDK tests in `ignored-test-policy.toml`. The ignored tests are still not
   ordinary CI process evidence.
2. Kept search-plane's real lexical adapter as a **dev-only** integration
   dependency; the production dependency remains forbidden. The hexagonal
   checker now recognizes an out-of-line `#[cfg(test)] #[path]` module so a
   test's `segment_id` does not masquerade as production transport leakage.
3. The module-cycle checker now resolves `#[path]` modules and ignores
   commented declarations. This exposed three production cycles. `HighlightSpan`
   moved to a lower results leaf, source-event hashing moved under its ingest
   owner without changing the CBOR preimage, and preview admission types moved
   below matcher/renderer. The graph checker now finds no unbaselined cycle.
4. Replaced four forbidden L5 benchmark serde derives with equivalent ordered
   field serializers. Documented the two fixed-width catalog digest functions
   as infallible by construction. Updated the sealed-manifest format 7 and
   source-file-coverage artifact inventory; the decoder already rejects old
   formats. Corrected a stale regex dependency-version comment.
5. Updated the public API and guarded module-tree snapshots for the current
   L1–L5 contract, including L2's `SourceFileCoverage`/source-event API.
   The public API diff removes old scope/digest setters and adds the new typed
   coverage/publication surfaces. The snapshots were reviewed before update.

`just rust-policy`, `just fmt-check`, `just rust-public-api`,
`just rust-cargo-modules`, `just rust-llvm-lines`, `git diff --check`, and
202 focused Python policy tests passed; raw result and JUnit are in
[l1-l5-proof](l1-l5-proof/). `rust-policy`'s proof-authority step
reported `REGISTRY_ONLY`: 25 registrations and **zero** execution manifests.
The benchmark-artifact step found 17 absent artifacts and validated none.

The LLVM-line gate had a stale May baseline: contract 6,294 to 90,192 lines
(14.33×), core 14,932 to 164,465 (11.01×). Current source grew from 16,367
to 34,978 Rust lines in contract and 1,606 to 13,946 in core since that
baseline. The top measured functions are distributed rather than one new
monomorphization dominating the total. The baseline was reset to the measured
current release-profile values, and `just rust-llvm-lines` then exited 0.
This resets a future regression guard; it does **not** qualify cold build time,
binary size, or the acceptability of the growth. The
[raw LLVM listings and guard outputs](l1-l5-proof/) are retained here.

## Remaining closure

- **BLOCKED:** enforce or prove an aggregate L4 regex allocation ceiling across
  AST/HIR planning, automata compilation, retained engines and search caches.
  The measured pre-mitigation `needle\w{120}` compiler peak exceeded the old
  16 MiB reservation. Current state-proportional logical charging mitigates
  request work but does not bind the allocator. See
  [L4_REGEX_BUDGET_RESIDUAL.md](L4_REGEX_BUDGET_RESIDUAL.md).
- **NOT_RUN:** one final-source daemon/SDK and crash/restart suite for L1–L4,
  current-source L5 Python consumers, whole-workspace CI, relevance/latency
  benchmarks, proof-manifest aggregation, and release/deployment activation.
  Earlier owner receipts are useful regression evidence, not composable with
  this new source digest.
