# CS-INT-01 — Remaining integration and qualification

Status: `ACTIVE`. Completed L1–L5 decisions are in
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md).
Common capture/process/I/O decisions are in
[SEP-27-004](../../../adr/SEP-27-004-benchmark-capture-and-resource-custody.md).
This ledger owns only remaining code-search integration and acceptance.
[MISC](../../sep-27-misc/tickets/INDEX.md) owns common execution, CI enrollment,
producer/consumer/replay and supported-platform qualification once.

## Implementation ledger and remaining acceptance

The implementation ledger below reflects HEAD `3272662c3cc2cc49cc881ac1b73dea226c01633e`
plus the current parallel working-tree edits. The historical owner-local
evidence section is bound to older HEAD `5522f86b34b82d5f3a04e5ec4ec2aa4a4acfb4a7`.
Its scratch logs and manifests are stale for current qualification; re-execute
selected gates at the release source.

Those owner-local checks used HEAD `5522f86b34b82d5f3a04e5ec4ec2aa4a4acfb4a7`
plus a dirty concurrent checkout on macOS 15.6/arm64, Rust 1.92.0. The
earlier selected nine-crate Rust source/config snapshot (323 files) was unchanged
before and after its tests, SHA-256
`05ec43f6380a9968afa21e07e2184e6485f86d656425b680e77173cd402ea0ee`.
The final combined process snapshot (766 workspace Rust files) was
unchanged during the completed binaries' execution, SHA-256
`4f0f5eebc23bf76cddda34b4bd166edd2797248aeb767e839ea6f09540651fca`.
`Cargo.lock` SHA-256 was
`a4b572ad7bc4345a8163257ac265ea30f2aab4a20cfe73b3ea90f152605df367`;
`scripts/cargow` SHA-256 was
`1b99eddcda1dd4de94eb52f520a322d1d393a223656c21bca60a909aaed0d53d`.
The 36 selected Rust manifest/config input hashes are in
`/tmp/qi-owner-final-rust-build-inputs.sha256` (manifest SHA-256
`6dbc1245adb825de6eaf057ef9840ed2fa298a89ae410af11a683824533d9fdc`);
six executed test binary hashes are in
`/tmp/qi-owner-final-test-binaries.sha256` (manifest SHA-256
`e36ee112c41fa04535b26607ecb5470be4853722639770420c24b9f20106bce4`).
These are owner-local inputs, not a clean-tree or release receipt. The earlier
nine-crate proofs do not inherit the later process source identity.

| Owner | Observed implementation | Remaining acceptance |
| --- | --- | --- |
| INT-C1 | Five ranked-key column constants moved to schema ownership; module cycle, affected ranked/storage tests and scoped all-target Clippy pass without a new cycle baseline. | Full repository and hosted CI at the release source. |
| Process source authority | Delta-base readiness now precedes lexical batch preparation. The E2E producer harness seals source chunks before publishing dependent structural/catalog rows; orphan structural rows refuse at ingest. Synthetic strict-symbol fixtures explicitly declare zero-symbol completion for files with no symbol rows. | Actual Semantica producer and installed SDK/daemon process qualification, crash and platform acceptance. |
| ENG-02 | Shared logical coverage snapshot; manifest format 9 bounded root/pages, changed-page encoding, hard-linked inheritance, strict decode and lifecycle commitment/scrub ownership. The same build passes verified coverage commitment to seal, eliminating its duplicate decode while preserving full page re-hash. See [ADR](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md). | Total pipeline cost/physical heap at growing sizes and current-format public process qualification. Preflight/build/open scans remain O(files). |
| BENCH-02 | Permanent fake-native cs/Sourcegraph/OpenGrok path/hit mutations recompute normalized digests and are refused by live verification. | Remaining admitted formats/callers and real external capture/replay. |
| MISC-04 enrollment | Both owners enrolled in existing Just Python rail, test authority and source closures; live collection rejects empty/duplicate modules. | Hosted CI and actual consumers remain separate. |
| ENG-04 | Typed engine byte refusal, explicit NFA/cache limits, logical preview charges and bounded manual repo-gate retention exist. Structural universe file regex compiles once per request and preserves typed resource refusal. No hard aggregate allocation authority. | Coordinate parser/compiler/cache physical admission across every executor caller while preserving matcher truth/ranges. |
| External producer / rollout | Format 8 and earlier explicitly require rebuilding for format 9. Prior static inspection of the Semantica issuer showed source-byte-bound coverage and a Structural dispatch loader gated on delivered lexical authority; this is not execution proof or a current-source claim. | Run its QBC owner tests, paired consumer migration, installed rebuild and activation/rollback against a frozen combined source. |
| BENCH-01/03/04 | Qualified verdict now refuses insufficient independent query-family clusters; an opt-in OpenGrok indexed inventory/source-view probe brackets local comparator queries. The probe remains diagnostic without posting freshness or other products' index scope. | Independent gold/holdout, fresh external index admission, quiet admitted host and real equal-work comparator/update runs. |

## Historical owner-local evidence boundary

- `VERIFIED` on the recorded source, owner-local: `just rust-module-cycles`, `just rust-cargo-modules`,
  `just rust-public-api` (contract and SDK), and
  `./scripts/cargow --lane clippy-lane clippy -p quanta-index-contract -p quanta-index-core -p quanta-index-lexical -p quanta-index-lq-regex --all-targets --locked --message-format short -- -D warnings`.
  The Clippy log is `/tmp/qi-clippy-owner-4.log`, SHA-256
  `a2719d99a07bab19cb3d54f3430286b9b31a84541f54c47ebedccf9bfbeb6118`.
- `VERIFIED` on the recorded source, owner-local: `./scripts/cargow test --locked -p quanta-index-lexical
  --lib --test sealed_manifest --test sealed_commitment_cost --test ranked_pages
  --test l2_file_mutation --test generation_delta_base_carryforward
  --test l4_match_anchored_preview --test planner_authority
  --test unicode_normalization_goldens --test l3_exact_source
  --test execution_budget --test text_authority_shards -- --nocapture`:
  307 passed, zero failed/ignored. Log `/tmp/qi-owner-lexical-final.log`, SHA-256
  `bda562cad5b4ea05fb33a5f808daaa1e586527a99012e1d32cd07b20aae23e42`.
  The page fixture observed 1024/2048/4096 files, fresh coverage bytes
  20362/22742/25869 and inherited page inodes 252/255/255. This is page
  writing only, not total ingest or RSS.
- `VERIFIED` on the recorded source, owner-local: `./scripts/cargow test --locked -p
  quanta-index-contract -p quanta-index-core -p quanta-index-lq-regex --lib
  -- --nocapture`: 349 passed, zero failed/ignored. Log
  `/tmp/qi-owner-contract-core-regex-final.log`, SHA-256
  `03881429b58b564b2ce18bbdbe2fbcd09d73da1dc35181e6169e0f4066421531`.
- `VERIFIED` on the recorded source, lexical scan owner: `./scripts/cargow test
  --locked -p quanta-index-lexical --test tantivy_smoke -- --nocapture`:
  38 passed, zero failed/ignored. Log `/tmp/qi-owner-manual-current.log`,
  SHA-256 `0c0da5b4f001b87aaf8c34c78088d2ffe28ae6f8eef9136d25fe69a59a824eb9`.
- `VERIFIED` on the recorded source, combined process: `./scripts/cargow test
  --locked -p quanta-index-search-plane --lib -p quanta-index-searchd-runtime
  --test runtime_fast_suite --test runtime_extended_suite --test runtime_risk_suite
  --test l4_preview_sdk -- --nocapture`: search-plane library 474, L4 SDK 7,
  extended 70, fast 63 and risk 139 passed; one fast case requiring external
  OpenAI API/operator consent was ignored under the registered policy. Log
  `/tmp/qi-owner-final-process.log`, SHA-256
  `20b69b5910cd1eba9863a6de156dde0527d5d2889a6d9b92f100d345ce8c2a4f`.
  These process tests use the local synthetic producer and development hash
  embedder; they do not exercise the external Semantica producer or an installed
  service.
- `VERIFIED` on the recorded source, Python: `uv run --frozen --extra dev python
  -m pytest tools/ci/tests/test_live_lexical_external.py
  tools/ci/tests/test_code_search_workflow.py
  tools/ci/tests/test_pair_capture.py tools/ci/tests/test_corpus_release.py
  -q -p no:cacheprovider`: 94 passed, one expected duplicate-archive warning.
  The selected 225 Python/config input file hashes were unchanged during the
  run; their sorted manifest SHA-256 is
  `0222b51f5a629fa4ca317d9400246a03bd77f40ed5369af656278fe71d725caf`.
  Log `/tmp/qi-owner-python-current.log`, SHA-256
  `352fe9356a07488a84dec2b0823f7d51694cad0fcff4209accde6c6b75ae160e`.
  These use local fake native services. `just rust-test-authority`, `just
  rust-ignored-test-policy`, `just lint-doc-paths`, `just lint-prompt-drift`,
  affected Rust `rustfmt --check` and `git diff --check` pass locally.
- `FAILED` broad Python style gates on the recorded 5522 checkout: `just
  python-lint` reported 20 Ruff errors in three files, and `just
  python-format-check` reported 73 files requiring formatting. These are not
  current defects: both commands returned exit 0 on clean HEAD
  `ce466325236821d5bbc4e51dc5c7b4c418fe2f8c`. The old raw logs are
  `/tmp/qi-owner-python-lint-current.log` (SHA-256
  `f0f5c1a7de6b3afbe5d416912d0cb4bbe9c5ce78523fa729bc70a8d7fbb9aa71`)
  and `/tmp/qi-owner-python-format-current.log` (SHA-256
  `90f6b7d2e4da87574a325db423ea6650baf3022b5828cf64237e01593e5edeed`).
  No repository qualification is claimed from the style checks or old focused
  tests.
- Earlier working-tree Python tooling before the current parallel changes: `just python-lint` and `just
  python-format-check` exit 0 with Ruff 0.16.8; `just python-test` exits 0
  with 2,610 passed and nine Linux-only skips on macOS. Test log
  `/tmp/qi-python-test-complete.log` has SHA-256
  `ffa3898c110ca59edb79a1e7f6810321e3914b28ef7f6a576986648512916d27`.
  This is local tooling coverage, not hosted CI or installed-service proof.

## Current owner-local verification

- Current `3272662c` plus working-tree edits: `just rust-profile test-fast`
  passed 2,339 tests across 48 suites; `just rust-profile test-daemon`
  passed 207 tests with one registered skip. The benchmark parser integration
  owner passed 26/26, and the three selected lexical integration owners passed
  24 tests with three manual cost probes ignored. `just fmt-check`, `just
  python-lint`, `just python-format-check`, `just lint-doc-paths` and
  prompt-manager lint exit 0.
  `just python-test` passes 2,626 tests with nine Linux-only skips; the
  BENCH-03 retrieval subset passes 341/341. These are local source and daemon
  harness checks, not external-producer or installed-release proof.
- Whole-workspace `just rust-clippy` is **VERIFIED** on the current working
  tree (`--workspace --all-targets --all-features --locked -- -D warnings`).
  Repair covered the retrieval build script, searchctl enum sizing, benchmark
  and scan helpers, and test-only source/lock/fixture warnings across the
  search-plane, SDK, contract and lexical crates. The source-publication
  fixture has a test-only assertion/indexing lint exception with a recorded
  reason; production lint severity was not reduced. This is local static
  checking, not runtime or hosted CI qualification.
- `NOT_RUN` for full repository/hosted CI, installed daemon, actual external
  producer/consumer migration, independent benchmark and physical regex/RSS
  qualification. The local scratch logs do not supply that evidence.

## Serial acceptance boundary

1. Preserve unrelated edits and one owner for shared schema/SDK/storage/selector
   changes. Recheck each recorded finding against the source selected for repair.
2. Retain legal domain/projection/empty/window semantics, canonical file mutation,
   event lineage/strict coverage, exact-name/federated top-k, cancellation and
   matcher-aligned previews. Add only regressions for demonstrated new gaps.
3. Execute migrated format-9 fixtures: seal/open/tamper/resident accounting,
   unchanged-segment inheritance and total fresh-byte accounting. The old private
   SSTable graph and format-7 receipts are historical.
4. Agree external issuer/SDK/reader/registry/cursor/evaluator cutover and rejection
   behavior. A source digest is not full-file byte or parser completeness proof.
5. Enroll the required tests and adopted normative ADRs in existing rails/closures.
   Required case identities come from live collection; ignored process cases
   must be explicitly executed when their public/lifecycle claim is selected.
6. Routine checks use the narrowest decisive terminal result. Formal replay,
   release and qualified benchmarks bind relevant source, dependencies, config,
   binaries, inputs and environment; refuse missing/stale/partial evidence.
7. After all normative edits, execute the combined required scope once and reuse
   compatible captures. An admitted quiet host and independent labels/assets
   precede measurement; exploratory results retain their reduced guarantees.

## Required scope and exclusions

- Contract/engine: exact-reference legal/empty/count/cursor, strict storage decode,
  same-path federation, wrong-source and interruption/resource controls.
- Publication: failed-parse replacement/repair, source replay/conflict/reorder,
  Delta inheritance, read-while-write, activation/retention and real crash cuts.
- Preview: original NFC/folded/UTF-8/CRLF bytes, Boolean focus, oversize/overlap,
  optional refusal, restart and mutable-checkout drift/deletion.
- Producer: current grammar/parser/ownership inventory, retained malformed files,
  capability-enabled lexical and strict symbol Vite paths through actual daemon.
- Final-source full/installed/platform proof belongs to MISC-04/05. Real host /
  independent benchmark input admission belongs to BENCH-01/03/04 and MISC-06/07.
- ANN replacement, learned reranking, new default weights, embedding-free
  activation and expanded platform/product support are separate decisions.

Current contract details and commands are in the ADRs, registered `Justfile` /
`./scripts/cargow` rails and MISC acceptance. Historical handoffs, test totals and
RCA bodies are recoverable through the [plan archive](../../ARCHIVE-INDEX.md).
