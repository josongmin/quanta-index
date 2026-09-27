# L1–L5 integration audit — 2026-09-27

> Historical report: one-off evidence files were removed from the repository. This report alone is not current verification.

Latest remaining-work audit after the format-8 merge:
[CS-INT-01](../rfcs/CS-INT-01-integration-and-qualification.md#current-remaining-work-audit--2026-09-27).
The execution records below are historical source-scoped receipts. Fresh static
audit found the `ranked_keys`/`ranked_page` module cycle and reproduced BENCH-02's
native/normalized scoring mismatch; required combined-source execution remains
NOT_RUN. Earlier compilation/selected passes do not override these outcomes.

Status: **partial**. Selected lexical behavior and the whole-workspace
all-targets/all-features compile check executed successfully on the recorded
source profiles below. These commands do not constitute full workspace test
qualification. Aggregate regex heap admission remains **BLOCKED**. Full
workspace tests, retrieval benchmark validity, release, and deployment are
**NOT_RUN**.

## Source and ownership

- The selected lexical, contract and L5 Rust tests below ran while HEAD was
  `3887fac3d090e9af86d1508ed2ba4125bf55bcbb`, with dirty source captured
  in their individual input manifests. Concurrent work subsequently committed
  that source in `31a0eef6`, then the lockfile and an additional L4 regression
  in `0e99f25a`. At the later integration check, HEAD was
  `0e99f25ab3e6928eaf294a8336d5456730fb3435`; an unrelated L4 proof
  directory remained untracked. Another writer subsequently committed the
  shared dirty tree as `a7f2812314def54a78989bac985cb6eed26806d0`
  while final tests were running, then continued editing L4 test inputs.
  These are distinct source states. No commit, push, reset, agent dispatch,
  task message, or task polling was used by this audit.
- The lexical verification bound 818 Git-visible Rust/config/vendor input files.
  Its before and after canonical file-hash-map digest was identical:
  `0af1558c1c4763c678546ebdea6f8c7c7d357c9eaedf8781584215ca4afc2721`.
  Pre and
  post file manifests retain the exact
  dirty inventory and individual hashes. The manifest covers the selected
  Rust input profile, not every mutable planning or Python file in the checkout.
- Rust toolchain: `rustc 1.92.0 (ded5c06cf)`,
  `aarch64-apple-darwin`, LLVM 21.1.3; `Cargo.lock` SHA-256
  `58dda6f980fd3d2ad4975b3be18c74e60351be045d78206728f88475a6ba6bbc`.
  Selected test binary hashes bind all
  eight executed binaries. The raw test outcomes are in the terminal output of
  the command below; a manually transcribed count is not independent evidence.

## Historical lane disposition

| Lane | Current-source execution | Excluded or unresolved claim |
| --- | --- | --- |
| L1 query/domain/window | **VERIFIED** on the recorded Rust profile, 18 lexical integration tests and a later 4-test exact-window process slice | Exhaustive final-head daemon/SDK and whole-product semantic quality **NOT_RUN**. |
| L2 file mutation/coverage | **VERIFIED** on the recorded Rust profile, 23 lexical integration tests, a scan-experiment adapter query test, and an 8-test real-process restart/rollback slice | Final-head crash cuts and cross-stream activation **NOT_RUN**. Delta coverage storage amplification remains open. |
| L3 exact source/budget | **VERIFIED** on the recorded Rust profile, 6 integration tests plus selected common budget/cancellation tests and 3 explain tests | Whole lexical/query dispatcher and broader process qualification **NOT_RUN**. `index:no` explicit language filtering remains typed unavailable by design. |
| L4 anchored preview/witness | **VERIFIED** on the recorded Rust profile, 12 integration tests plus lexical library regressions and a 7-test real-daemon SDK slice | Aggregate compiler and retained-cache heap ceiling **BLOCKED**; logical charges do not enforce physical allocation. Later L4 test edits have their own source boundary. |
| L5 parser/producer coverage | **VERIFIED** on the recorded Rust profile: 87 library + 26 parser integration tests, 113 passed. A later frozen-source Python consumer run passed all 552 selected tests with matching source and environment inventories. | The Rust result above binds an earlier source digest; a final combined current-source Rust/Python claim awaits the newer Rust rerun. Process/benchmark proof **NOT_RUN**. |

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

Its JUnit records 552 selected and
executed, zero failed/error/skipped; SHA-256
`3ccb9615337c467d7250fd60a782691063bfe264bf25dc27ae10100`. The
worktree was clean at `3887fac3` when rechecked. This is a consumer-only
frozen-source result, not a current shared-tree or Rust producer result.
The shared interpreter was Python 3.13.9; its post-run
90-package freeze has SHA-256
`473fc8b212c34913bbb93b1eea738ac8adc3bacc2774b15ddecdb6f35975cc91`.
That post-run inventory is diagnostic; it does not independently prove the
installed package set was unchanged during the earlier 12-minute run.

The current-source L5 Rust command was:

```text
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=3600 ./scripts/cargow --lane test-fast-lane test -p quanta-index-retrieval-bench --lib --test l5_parser_regressions --locked
```

Its raw log records exit 0, 87 library and 26
integration tests executed, zero failed/ignored. The 846-file Rust/vendor/
benchmark input pre and
post maps both digest to
`4e0a4e6dd84d6a2de765a5cbae1065b5df8dc85269d37335e7b2f6f158ad73cc`;
the two test binary hashes bind execution.
This does not compose with the earlier Python run's different source.

The DTO/hash owner command used the same build lane:

```text
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=3600 ./scripts/cargow --lane test-fast-lane test -p quanta-index-contract-base -p quanta-index-contract --lib --locked
```

Its raw log records contract 163 and base 53,
**216 executed, 0 failed/ignored**. The 818-file input
pre and
post maps have the same
`0af1558c1c4763c678546ebdea6f8c7c7d357c9eaedf8781584215ca4afc2721`
digest as the selected lexical run; binary hashes
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
6. The first all-targets workspace check exposed an actual stale consumer:
   `scan_vs_index` still constructed pre-L2 `SearchCorpusReplaceScope` fields
   and omitted `SearchCorpusIngestBatch.source_event`. It now builds canonical
   file coverage and a source publication event, stamps the IPC-owned batch
   digest, validates the mutation set before building, and tests a sealed
   generation through an indexed query. `quanta-index-ipc`, `sha2`, and the
   test's `tempfile` dependency are recorded in the crate manifest and lockfile.
7. The next all-targets check exposed seven more lexical integration targets
   and a core resource-policy target still constructing pre-L2 file scopes.
   Those fixtures now bind authored source bytes, file identity, unit-set hash
   and source-publication identity. The lexical test helper generates the
   direct-adapter batch token without importing the IPC adapter across the
   hexagonal boundary. The two old tests that staged `seal=false` source
   batches were changed to exercise the current fail-closed publication
   contract; staged partial publication is no longer a valid test setup.
8. The following all-targets checks exposed three more stale consumers:
   `quanta-index-sdk` lacked public reexports of the five L2 coverage and
   source-publication types used by its runtime tests; a runtime explain
   fixture omitted the new candidate source and preview fields; and the
   SCV2 semantic-source wire test built a batch without a source event. The
   SDK baseline and fixtures now match the canonical contract. The final
   all-targets/all-features workspace check exits 0.
9. A sealed generation's raw mutation was refused by the coverage boundary
   before the typed `GENERATION_IMMUTABLE` door. `build_ops` now checks the
   immutable generation before the independent-raw-mutation guard, preserving
   the specific typed refusal and preventing mutation. The sealed-manifest
   regression passes. The damaged-base-shard regression now asserts the
   stronger observed result: the delta creates no target directory and its
   open returns `NotFound`.
10. QI-BB-006's index-byte oracle previously charged the new
    `source-file-coverage.cbor` full snapshot as index data. It now reports
    that sidecar separately and keeps the inherited-index byte budget
    unchanged. This is a scope correction, not a claim that delta storage is
    cheap: in the 400-filler fixture the delta rewrote 169,994 coverage bytes
    and 398,643 bytes in its touched text-authority shard. The full snapshot
    is tied to generation identity and publication, so its rewrite cost
    remains a distinct production cost gap.

`just rust-policy`, `just fmt-check`, `just rust-public-api`,
`just rust-cargo-modules`, `just rust-llvm-lines`, `git diff --check`, and
202 focused Python policy tests passed; raw result and JUnit are in
l1-l5-proof. `rust-policy`'s proof-authority step
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
raw LLVM listings and guard outputs are retained here.

## Late integration execution

The final pre-commit source map and a later stable rehash both bind HEAD
`0e99f25a` plus dirty inputs: 1,154 local Rust/config/Python/fixture files,
25 pinned external dependency files, `Cargo.lock` SHA-256
`10787c9d9c8a642ab2312b60e51d45482b4d49db5f25ad5cddb340a1a7f2f972`,
and source digest
`f58a440d36797eb25ee09e41a1348dd6eed8cb8603cb6ba83bc72011d9302fff`.
See pre,
stable rehash, and the
capture script. The shared checkout
subsequently moved to `a7f28123` and three L4 test/proof inputs changed; the
later map is a drift diagnostic, not
part of the earlier result. A still later live observation
bound HEAD `a7f28123` plus dirty inputs to source digest
`6c3f954d48a6ec7ccba0bc50fca3dc9b5ab02df7d804aaa422533e468cac19c5`:
12 local source/proof-script files differ from the frozen `f58a440d…` map,
including L4 test code and benchmark Python. It is an observation of concurrent
work, not a completed qualification rail.

- **VERIFIED, compile scope:** `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=3600 just rust-check`
  exited 0 on the stable profile; its underlying command was
  `./scripts/cargow --lane all-targets-lane check --workspace --all-targets --all-features --locked`.
  Raw log, SHA-256
  `fdff45b5319401a34e700c94a21f34f5559fcf4e1d2254af9f0b2788d489fd97`.
  This is compilation only.
- **VERIFIED, selected lexical execution:** the command above was rerun after
  the integration repairs and exited 0 with 193 library plus 74 integration
  tests, **267 executed, 0 failed/ignored**. Raw log,
  SHA-256 `02a5cba10e21ef9bad013ff641976abf6c71c4ec62d92d05e53a51b946a9303a`.
  The seven migrated lexical targets also exited 0 with 40 tests; their
  raw log, SHA-256
  `95212c96ccde80bd189c283ac9fa74fe3d42c8642bbf399cbb7f8541424fbb6d`.
  Formatting of one migrated test file changed during that command; the six
  text-authority tests were subsequently rerun after formatting. Do not
  promote the 40-test aggregate to a single unchanged-source receipt.
  Executed binary hashes are
  recorded separately.
- **VERIFIED, formatted text-authority target:**
  `./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical --test text_authority_shards --locked`
  reran the six tests after the final formatting edit and exited 0. The
  raw log has SHA-256
  `498c66c2e4959f758eb02c771cda3a91e4fd37d6ffef817fd42431d971b65897`.
- **VERIFIED, selected process/integration paths before later L4 edits:**
  `runtime_extended_suite composite_generation_authority_restart` passed 8,
  including real child-process restart and rollback;
  `runtime_risk_suite explain` passed 3;
  `l4_preview_sdk` passed 7, including two fresh daemon starts; and
  `runtime_risk_suite e2e_exact_count_window` passed 4. All commands used
  `./scripts/cargow --lane test-fast-lane test -p quanta-index-searchd-runtime
  --test <target> <selector> --locked`, with no selector for `l4_preview_sdk`.
  Their restart,
  explain,
  L4 SDK, and
  window raw logs have SHA-256
  `31700f7c535db661e7135e8b152e1956d6dea7129d6dc2cf7cce896ff2843617`,
  `04c319dc809cd79bbb6b54f39b36ceaa52e8d53c0bebc961570bdc99129e70c5`,
  `e9ef4fcbc52152f340e1cceb7d9ea8b60f04928e1fcaafffbe50aaef86deab89`,
  and `b0bd298a3e7f50852bb41a1ed9f739461d836cccba001be8185bcd343e89b0ae`.
  This is a selected process slice, not exhaustive L1–L5 process proof.
- **VERIFIED, corrected contract fixtures:**
  `./scripts/cargow --lane test-fast-lane test -p quanta-index-core --test ingest_resource_policy --locked`
  passed 7, and the equivalent `-p quanta-index-contract --test scv2_01_semantic_source_wire`
  command passed 7. Core and
  contract logs have SHA-256
  `39ac447fc9ec66648eaf898c76ee59528ee95e50e5c072b0728b4b0eb6330529`
  and `b24f1ed0f466750f7bf1c4dc6e7379f22f041b9762cc3e3fb2d3b62d50a2c20a`.
- **VERIFIED, frozen Python consumer scope:** on a detached filesystem
  worktree with the same `f58a440d…` source map, the five named pytest files
  above exited 0, **552 executed, 0 failed/error/skipped**, in 765.38 s.
  The command used the checkout's pinned Python 3.13.9 environment:
  `/Users/songmin/Documents/code-new/quanta-index/.venv/bin/python -m pytest -q
  tools/ci/tests/test_retrieval_benchmark.py
  tools/ci/tests/test_retrieval_contract_proof.py
  tools/ci/tests/test_retrieval_sdk_proof.py
  tools/ci/tests/test_portable_proof.py
  tools/ci/tests/test_benchmark_source_closure.py
  --junitxml=/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-27-code-search-remediation/handoffs/l1-l5-proof/l5-python-frozen-final.junit.xml
  --tb=short` from `/tmp/qi-l1-l5-frozen-final`.
  Pre and
  post source maps are
  identical; pre/post `uv pip freeze` SHA-256 is identically
  `473fc8b212c34913bbb93b1eea738ac8adc3bacc2774b15ddecdb6f35975cc91`.
  JUnit SHA-256 is
  `657634687041d2493f20a2a1c87017e2f244034e91b75eefe860a560eee25c64`;
  raw log SHA-256 is
  `e5fadc9ba477589339703d53080c8accc0f682f8430e606656702e6bd2eaa592`.
  This qualifies those consumer tests on the frozen source, not the later
  shared-tree HEAD or an installed deployment.
- **VERIFIED, static gates on the pre-commit source:** `just rust-policy`,
  `just fmt-check`, `just rust-public-api`, and `git diff --check` exited 0.
  Policy,
  format, and
  API logs are retained. The
  policy step reports registry-only proof authority and 17 absent benchmark
  artifacts; it does not supply execution or benchmark qualification.

## Remaining closure at the historical execution boundary

Current ticket dispositions in CS-INT-01 supersede this historical inventory.

- **BLOCKED:** enforce or prove an aggregate L4 regex allocation ceiling across
  AST/HIR planning, automata compilation, retained engines and search caches.
  The measured pre-mitigation `needle\w{120}` compiler peak exceeded the old
  16 MiB reservation. Current state-proportional logical charging mitigates
  request work but does not bind the allocator. See
  [L4_REGEX_BUDGET_RESIDUAL.md](L4_REGEX_BUDGET_RESIDUAL.md).
- **VERIFIED source cost mechanism; cost qualification NOT_RUN:** delta coverage
  persistence rewrites a generation-wide snapshot
  for a one-file update; the observed 169,994 fresh bytes are outside the
  index-byte oracle. No storage-amplification ceiling or shard/inheritance
  design has been qualified. This is a scalability/cost issue, not a demonstrated
  source-freshness or atomicity defect. The sample is pre-format-8 and needs a
  new measurement after the merge. Touched text-authority shards are likewise
  rewritten; the separate shard test measures their cost.
- **NOT_RUN:** full workspace test CI, exhaustive final-source daemon/SDK and
  crash/restart suite for L1–L4, relevance/latency benchmarks,
  proof-manifest aggregation, and release/deployment activation. A same-source
  L5 Rust 113-test rerun, contract/base 216-test rerun, scan-experiment target,
  and changed-package Clippy were queued but never acquired the shared Rust
  build lock while an unrelated isolated L4 build compiled dependencies.
  These were interrupted before Cargo execution and are **NOT_RUN**, not
  failures or passes. L1 keyset/ranked process expansions likewise remained
  unexecuted after interruption. The earlier 113/216/scan passes retain only
  their recorded older-source scope.
  Earlier owner receipts are useful regression evidence, not composable with
  this new source digest.
