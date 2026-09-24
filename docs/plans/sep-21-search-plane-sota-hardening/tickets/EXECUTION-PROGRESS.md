# SEP-21 execution progress

Status: open. This is a source-inspection ledger, not a qualification receipt.
The authoritative result SHA, source digest, command counts, and push state belong
to validated lane handoffs and immutable proof manifests, not this document.

## Current checkpoint (2026-09-24)

- P11 Semantica producer dispatch follow-up, starting from Quanta dirty `main`
  HEAD `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85` and Semantica dirty `main`
  HEAD `f5aabb37deb0a79dc492c31e904ee6ee1f9e8a58`: the common RepoMap
  handoff dispatcher now validates the bundle-derived activation target before
  connecting, returns the supplied V2 request with its prior-head CAS
  expectation unchanged, and sends that request through the existing SDK
  ingress. Semantica's existing ingress facade now exposes the Quanta SDK's
  catalog-owned RepoMap active-head read; it does not maintain a producer head
  cache. The registered `incremental-repomap-ack-replay-owner` selector was
  updated from two to three tests for the frozen-expectation/foreign-target
  negative. `python3.12 tools/testing/run_suite_v1.py
  incremental-repomap-ack-replay-owner --resolve-only` exited 0 and selected
  the canonical QBC group lane with three expected tests. Scoped `rustfmt
  --check` and `git diff --check` exited 0. The required QBC compile command
  `scripts/quanta-build-cli profile run quanta-runtime.search-plane.check`
  exited 1 before compilation: free space at the configured target cache was
  54.8 GiB (2.9%), below the 4.75% admission threshold. No owner test executed
  for this Semantica edit. The ordinary outbox and aggregate Required-member
  custody still reconstruct `for_bundle(None)` rather than durably freezing
  the observed head before first dispatch. Root-incarnation binding, exact
  pair proof, release, deployment and activation remain `NOT_RUN`; this
  dispatch patch alone does not close P11. Quanta's shared `main` advanced to
  `e110d9da8b93ec18ccebdde7b39b221b045e7854` during this turn; no
  paired-source proof was attempted after that drift.

- P11 activation replay projection on dirty Quanta `main` HEAD
  `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85`: a new owner test modeled
  the gap after the catalog activation commit and before the store's in-memory
  `activated` projection update. Before the fix, V2 replay returned a success
  ACK but the same process's pinned query returned `NotFound` (behavioral RED,
  one relevant test executed). The final oracle checks both the active-head
  projection reader and the pinned query reader. `RepoMapGenerationStore::commit_activation`
  now refreshes the same catalog-derived in-memory projection for both fresh
  and replayed outcomes before returning the receipt. A store-local activation
  commit gate also serializes catalog commit through projection publish so an
  older concurrent writer cannot overwrite a newer committed head; it is a
  coordination lock, not a second head authority. A concurrent two-activation
  oracle compares the final projection with the independently read catalog
  head and checks the pinned query. It passed, but it was not a deterministic
  pre-fix RED; the stale-writer race is source-traced. The fix does not append
  a sequence, introduce a second durable head, or recompile on replay. The identical
  focused command `./scripts/cargow test -p quanta-index-repomap --test
  candidate_activation_owner_v1
  v2_activation_replay_repairs_missing_in_process_projection -- --exact`
  then passed 1/1; the whole owner target passed 17/17 with subprocess
  crash/replay cases. `./scripts/cargow fmt -p quanta-index-repomap -- --check`
  and scoped `git diff --check` exited 0 after formatting correction. The
  two-file scoped diff digest at this checkpoint is
  `c3d2346cbd6aa543df65c3fcb0871b99f15c4772b6d5a28213949cb2b39c7971`.
  The registered `candidate-activation-owner` local scope also ran via
  `just rust-profile test-candidate-activation-owner` and exited 0:
  nextest run `76569fee-0241-4a9c-98d4-8c575ce44121`, 28/28 tests across
  four binaries, zero skipped. The full `proof-p03-candidate-activation-owner`
  recipe and clean-source proof manifest were not issued.
  This is dirty local P11 owner behavior, not a clean paired-source P11 proof,
  deploy/activation/rollback receipt, or P0/P1-clear verdict.

- P12A manifest-producer sibling on dirty Quanta `main` HEAD
  `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85` (`Darwin arm64`):
  same-byte in-repo symlink dependency alias, digest input and current alias
  were behaviorally RED before this edit. `write-proof-manifest.py` now reuses
  the handoff leaf's no-follow regular-file reader for repo inputs, evidence,
  binary, dependency alias/archive, registry/schema and published alias;
  digest and JSON parsing use the same bytes. Explicit external terminal input
  is also read no-follow. Manifest content archives, immutable leaf/index and
  current alias publish through pinned no-follow parent descriptors; the
  aggregate writer now reuses the same output-custody primitives instead of
  maintaining a second implementation. A broken registered-manifest symlink
  now classifies `FAILED`, not `NOT_RUN`, in both writer and independent
  checker. Added same-byte input/evidence/alias/index/leaf and
  archive/current-alias parent-swap negatives, and registered the manifest
  writer tests in the P12A owner recipe. `just proof-p12a-proof-infrastructure`
  exited 0: test authority OK, 26 registered proofs, **zero manifests
  validated**, 118/118 owner tests passed. Scoped Ruff check/format and
  `git diff --check` exited 0. The scoped P12A code/recipe diff digest was
  `9cf7ad8c11fba28d603e4a183d7c1577312732dda23e0801fc62504bbd1a94b3`.
  This supersedes the preceding checkpoint's statement that the producer
  sibling has no candidate. It does **not** establish a clean exact-pair
  P12A manifest, authentic historical handoff chain, release qualification,
  deployment, activation or rollback. Status: dirty local owner behavior;
  final qualification `NOT_RUN`.

- P12A proof-file custody on dirty `main` HEAD
  `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85`: a same-byte in-repo
  symlink proof archive was accepted by the handoff leaf (behavioral RED),
  and a symlinked aggregate output parent reached the writer's redirected
  path (behavioral RED). The current candidate makes one no-follow,
  repo-relative regular-file reader bind digest and JSON parsing to the same
  bytes. The handoff CLI/ledger, proof checker manifest/dependency/evidence/
  binary readers, and aggregate writer reuse that custody rule. The writer
  now uses a no-follow output-parent directory descriptor for temporary
  creation, replace, rollback and parent fsync. Same-byte final/parent
  symlinks, CLI handoff symlink, checker sibling archives and writer output
  symlinks have negative tests. The registered local command
  `just proof-p12a-proof-infrastructure` exited 0 with 86/86 tests; its
  registry check reported 26 registered proofs and **zero validated
  manifests**. The adjacent `test_write_proof_manifest.py` suite also
  executed 23/23. Scoped Ruff check/format and `git diff --check` passed. The
  broad `just python-lint` still exits 1 on nine errors in other dirty
  benchmark/timing/test files; none is in the P12A touched files. The
  P12A code/recipe diff digest at this checkpoint was
  `0e6a21a90524842d7a017bf8b9e54a7b5ccfb67e4c7156fcbebdda5416174dd1`.
  The same-boundary producer in `tools/ci/write-proof-manifest.py` still
  resolves dependency aliases and performs separate path-based hash/parse/
  archive reads; its no-follow publication/custody audit is open. Thus this
  is **not** a P12A P0/P1-clear claim.
  No authentic historical handoff chain, clean exact-pair P12A manifest,
  Linux release proof, deployment, activation or rollback receipt was
  issued. Status: local owner behavior only; final qualification `NOT_RUN`.

- P09 process-instance follow-up on dirty `main` HEAD
  `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85`: the daemon composition
  now reads 128 bits from OS entropy once and injects the same nonzero value
  into query/control/ingest `IpcServerCounters`. A bounded ring window refuses
  an unbound/test counter instead of emitting an anonymous process identity;
  each plane still owns its separate insertion sequence. This is diagnostic
  identity, not a request-ID allocator, catalog epoch, or durable authority.
  `./scripts/cargow test -p quanta-index-ipc --lib --locked` exited 0 with
  55/55 tests after the final IPC edit; `just rust-hexagonal` and `just
  rust-test-authority` exited 0. After the concurrent P11 caller edits became
  visible, `./scripts/cargow --lane fast-lane check -p quanta-index-searchd
  --lib --tests --locked` exited 0 (compilation only). A production-shared
  `ipc_plane_counters_v1` helper is now called by runtime composition; its
  focused OS-entropy/three-plane test,
  `./scripts/cargow test -p quanta-index-searchd --lib
  one_runtime_instance_binds_all_three_plane_windows --locked`, exited 0 with
  1/1 executed after a 3m 07s dependency build. The operator
  principal policy, control wire projection, encoded-byte cap and real-UDS
  process correlation remain open.

- P09 ingest-provider and transport-window follow-up on shared `main` HEAD
  `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85`, dirty source. The
  existing request budget now carries the IPC diagnostic bridge through the
  search-corpus ingest port into `semantic_derive::embed_window`; actual
  provider calls emit checked per-request window ordinals without creating a
  provider-ledger ticket or changing durable-operation cancellation. Earlier
  focused tests on the dirty pre-P11-wire-removal source covered multiple
  windows, provider error, pre-I/O refusal, and journal-replay no-call behavior;
  they are not a current integrated receipt. The same IPC ring now offers a bounded
  insertion-sequenced window with oldest-retained, next-sequence, loss-before/
  after and limit-omission metadata; this is **not** an operator wire path or
  an authorization check. `./scripts/cargow test -p quanta-index-ipc --lib
  --locked` exited 0 with 53/53 tests. `./scripts/cargow test -p
  quanta-index-search-plane --lib --locked` exited 101 before test execution:
  concurrent P11 wire removals left `control_dispatcher.rs` and ingest tests
  referring to removed `RepoMapActivate`/`PublishRepoMapBundle` variants.
  `./scripts/cargow test -p quanta-index-ipc --test admission --locked` also
  exited 101 before execution because its response match still includes the
  removed `RepoMapMutationAck` variant. These are shared-source integration
  failures from that earlier shared-source snapshot, not P09 behavior verdicts.
  After the P11 caller update, the **same** search-plane library command
  exited 0 with 405/405 tests and the same IPC admission command exited 0
  with 3/3 tests, still on dirty and concurrently changing source. `just
  rust-hexagonal`, `just rust-test-authority` and
  scoped `git diff --check` exited 0; the two-file IPC source diff digest was
  `56e65abc32dce9394e8666deb5c90625991252d82b045e37739a3d52f48580e4`.
  P09 real-UDS ingest correlation, operator authorization,
  process-instance binding, serialized response cap, readiness horizon, and
  clean-source owner/release proof remain `NOT_RUN`.

- P09 query-provider correlation follow-up on `main` HEAD
  `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85` with dirty P09 code and
  unrelated concurrent retrieval/SDK work: `RequestBudgetV1` now carries an
  optional diagnostic port from IPC admission to the existing transport ring.
  `ProviderBoundaryQueryEmbedder` records start/return with the real
  `ProviderBudgetLedger` ticket, while the ledger remains the sole
  reservation/settlement/usage authority. A real `handle_connection` test
  proved the admitted envelope and connection IDs on both provider stages;
  provider-boundary owner tests proved ticket/correlation agreement, zero
  markers before local refusal, and return markers on a failed provider call.
  `./scripts/cargow --lane fast-lane check -p quanta-index-core -p
  quanta-index-ipc -p quanta-index-search-plane --locked` exited 0;
  `./scripts/cargow test -p quanta-index-ipc --lib --locked` passed 51/51;
  `./scripts/cargow test -p quanta-index-ipc --test admission --locked`
  passed 3/3 real-UDS admission tests;
  `./scripts/cargow test -p quanta-index-search-plane --test
  provider_boundary_owner_v1 --locked` passed 22/22. `just
  rust-test-authority`, `just rust-public-api` and `just rust-hexagonal`
  exited 0. The P09 test-authority selector now includes the provider owner
  integration target and core library. `just rust-cargo-modules` first exited
  1 for the intentional new core enum/trait snapshot; its exact baseline was
  updated, and the same command then exited 0. These focused
  dirty-source results do **not** prove ingest provider stages, operator
  readout authorization, readiness freshness, the registered P09 owner rail,
  a Linux release process rail, or clean-source qualification.

- P09 request-event follow-up on moving shared `main`: the code was absorbed
  by local `a0ac1853256d9b507ae8dc76f7437a4c7568434c` together with
  unrelated changes. Tests below ran on its dirty predecessor bytes, not an
  exact-clean-source receipt. IPC now owns one bounded 1024-entry, nonblocking
  request-event tail keyed by validated envelope ID and connection ID, with
  visible dropped-event counter. The generic searchd adapter adds a closed
  route projection and top-level typed error code without new request IDs or
  provider usage settlement. An overload refusal encode failure previously
  emitted `ResponseWriteFailed`; a real `handle_connection` RED failed 1/1,
  then the classification patch made the same test pass 1/1. `./scripts/cargow
  test -p quanta-index-ipc --lib --locked` passed 49/49; `./scripts/cargow
  test -p quanta-index-ipc --test admission --locked` passed 3/3 real-UDS
  admission tests. `./scripts/cargow
  --lane fast-lane check -p quanta-index-searchd --lib --tests --locked`
  passed compilation only. A filtered searchd test attempt was interrupted
  during DataFusion/Lance compilation (exit 130), before test execution.
  `just rust-hexagonal`, `just rust-wire-inventory` and `just
  rust-public-api` passed; `just rust-cargo-modules` was interrupted after
  90 seconds waiting on its tool subprocess (exit 130), with no verdict.
  A real-handler panic terminal/RAII regression test was added after the
  49/49 library run. Its first focused run was interrupted while waiting for
  the shared Cargo cache lock (exit 130), before execution. After commit at
  clean `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85`, the same focused
  command executed 1/1 and passed; the dispatcher panic was deliberately
  caught and one terminal event plus slot release were asserted. The updated
  `./scripts/cargow test -p quanta-index-ipc --lib --locked` suite subsequently
  passed 50/50 on the same HEAD with concurrent dirty documentation; this
  broader run is not a clean-source qualification receipt.
  RR proof-selection audit found that the registered P09 owner scopes omitted
  the new IPC and searchd adapter behavior. `tools/ci/test-authority.toml`
  now selects `ipc-admission` in the P09 integration scope and adds
  `quanta-index-ipc` plus `quanta-index-searchd` to its library scope;
  `just rust-test-authority` passed. The enlarged registered P09 owner rail
  itself remains `NOT_RUN` and will carry the searchd test-build cost.
  Query/ingest still discard `DispatchContextV1`; provider-stage linkage,
  an operator-readable bounded event path, observed backend liveness and an
  explicit readiness proof-age horizon remain open. The P09 owner/release
  proof and clean-source qualification remain `NOT_RUN`.

- R0 read-only inventory on the current host: neither
  `QUANTA_INDEX_STATE_ROOT` nor `QUANTA_INDEX_CACHE_ROOT` was set in this
  shell. The macOS default `/Users/songmin/Library/Caches/quanta-index/state`
  exists but its only observed file was `build-profile/history.jsonl`; no
  `repo-map/` exists there. This limits the default local root only, not an
  external deployment or custom root. Semantica's current checkout was
  `ab1bb476306b02ae81979d359f5ae22c712358ad`, 31 commits ahead and 28
  behind its upstream with two dirty ticket files. Exact-pair proof remains
  `BLOCKED` on that shared checkout. Preserve actual V1 roots and original
  source bundles until the operator identifies them; absence at the default
  path is not a global `NOT_APPLICABLE` waiver.

- P12A aggregate follow-up is now present on shared local `main`
  `fa17e0f14672548bf7116074b7064c7c5e455d03`: the single-handoff
  Git/archive/proof semantics moved into the acyclic
  `tools/ci/lint/handoff_validation.py` leaf; CLI and aggregate checker inject
  the proof checker instead of importing it from the leaf. The aggregate
  schema/writer/checker derive separate product handoff refs, a fixed-chain
  verdict, and the P12A infrastructure handoff from canonical files. Four
  final-source verdicts remain based on final-current proof receipts; missing
  historical handoffs prevent only `production_ready` and final P12 issuance.
  A real `just lane-handoff-chain-check` run exited 1 for missing P00, P01,
  P02A, P02B, P02I and P11 artifacts. After the shared commit, `just
  proof-p12a-proof-infrastructure` exited 0 with test-authority lint,
  proof-authority lint (26 registered, zero manifests validated), and 28/28
  Python tests. This rail ran on a dirty checkout because the aggregate
  regression test and concurrent retrieval-benchmark files were modified;
  it is not an exact-clean-source P12A proof. The ready-path unit tests stub
  an already-validated handoff ledger; the real missing-ledger negative is
  exercised, but a full real-artifact positive chain is still unavailable.
  A further handoff hardening batch requires a non-empty exact Git write set,
  refuses blocked P12A handoffs even after single-record validation, and
  binds each handoff digest to the same no-follow regular-file bytes that
  JSON parsing consumes. The product-chain and infrastructure-ledger failure
  axes are separately covered. `just proof-p12a-proof-infrastructure` then
  exited 0 with 32/32 Python tests; `git diff --check` exited 0. Shared
  `main` advanced again during this session (last observed `1306325e`), and
  unrelated benchmark work plus these P12A edits remained dirty, so this is
  local behavior/governance evidence, not exact-source qualification.
  A later same-scope rerun exited 0 with 33/33 tests after malformed archived
  manifest input was made a `FAILED` ledger entry rather than an unhandled
  aggregate exception; the source remained dirty and this still did not issue
  a P12A proof manifest.

- P12A partial implementation on Quanta `main` base `565be6ac` adds one
  fixed handoff lane policy in `tools/ci/lint/handoff_validation.py`, a pure
  product fork/join/serial-chain check, and a historical directory CLI rail.
  The existing single-handoff Git delta, immutable archive and proof-manifest
  checks run before chain acceptance; current-source HEAD rebinding is not
  imposed on historical handoffs. `python3 -m pytest
  tools/ci/tests/test_check_lane_handoff.py
  tools/ci/tests/test_handoff_validation.py -q` passed 13/13 on a dirty
  working tree. The updated `just proof-p12a-proof-infrastructure` rail passed
  test-authority lint, proof-authority lint (26 registered proofs, zero
  manifests validated), and 25/25 Python tests on the same dirty source.
  The real chain command failed as expected because P00, P01,
  P02A, P02B, P02I and P11 handoffs are absent. Aggregate schema/writer/final
  checker integration and P12A/P12 proof issuance are still `NOT_RUN`; this
  local test result is not P12A closure.

- P06 source follow-up on the same Quanta base `565be6ac` found that
  `quanta-index-searchd-runtime::build_runtime_with_memory_probe` opens the
  lexical and semantic adapters before `SearchCorpusLifecycleOwner::open`.
  Therefore an empty `activations/` directory is not proof of a fresh state
  root: adapter construction may already have created current-format files.
  The final plan now requires fresh-vs-existing root admission immediately
  after lease acquisition, before those constructors. The root-incarnation
  format and lifecycle wiring are still `NOT_RUN`. The supported restore
  boundary (managed restore only vs external clone fencing) was requested
  from the operator; no answer is recorded in this checkpoint.

- Current `$ss` implementation checkpoint on Quanta `main` base
  `565be6ac5934ac13b548fc926e581e5094629ec5` (dirty working-tree
  changes, not a clean-source receipt): P06 SDK control binding now compares
  complete typed search-corpus identities, including semantic content roots
  and an absent first-activation predecessor. The common binding-path
  swapped-root test failed before the fix (1 executed/1 failed), then passed
  after it (1/1). `just rust-profile test-sdk-binding-owner-lib` passed
  637/637 again after both code edits, before this ledger update; it is
  local behavioral evidence, not an exact clean-source receipt.
  `SearchCorpusGenerationV1::new` now delegates
  its shared shape rules to the contract's `validate_v1` instead of keeping
  a parallel validator; the focused owner negative passed 1/1 after its
  expectation moved to the contract error code. The activation owner-local
  slice then passed 13/13. ActiveHead event identity,
  root incarnation, P09/P10/P11/P12 implementation and final-source proof
  remain open. `just fmt-check` passed after formatting this checkpoint.

- A subsequent P09 metric follow-up derives both executed-engine fanout and
  post-filter contributing-lane count at the same successful dispatcher
  boundary. Lexical/single-lane routes derive contribution from returned
  results; semantic/hybrid/explain consume their producer-owned execution
  summary. The independent execution-truth suite passed 10/10, including
  zero-hit `executed=2, contributed=0`; four closed metric-set tests passed
  4/4. `./scripts/cargow --lane fast-lane check -p
  quanta-index-searchd-runtime --test runtime_extended_suite --test
  runtime_risk_suite --locked` passed. Their test bodies and the P09 release
  process target remain `NOT_RUN`.

- 2026-09-24 P08/P09 follow-up is committed at local `main` `3909b5ca`;
  remote publication and post-commit clean-source proof are `NOT_RUN`.
  The focused results below were collected on a shared dirty checkout.
  `searchd` now composes a process-readiness port from supervisor
  phase, actual accept-loop/provider child liveness, maintenance heartbeat,
  activation-catalog identity, and a producer-owned physical active-pair
  proof. Boot's already-proven active pair identity seeds the cache, so the
  first health poll does not repeat a deep open. The proof is cached only for
  an exact active identity and scrub
  invalidation epoch; mutation, scrub corruption/error, or a racing catalog
  change forces a fresh proof or a not-ready result. A process/UDS owner test
  covers zero-active and active-repository reports. The contract decoder and
  SDK reject self-contradictory same-variant reports, including an omitted
  integrity field. A final catalog/scrub recheck refuses a readiness
  observation whose active identity or invalidation epoch raced the probe.
  `searchctl readiness`
  now names process readiness; the former per-repository behavior is exposed
  separately as `searchctl generation-status`. `drive()` also stops its
  shutdown bridge when supervision itself initiates shutdown, avoiding an
  unbounded join after required-child loss. `./scripts/cargow --lane fast-lane
  check -p quanta-index-searchd --lib --locked` completed on an earlier dirty
  source. `just rust-wire-inventory` and `just rust-public-api` passed after
  intentional contract/SDK baseline updates. The changed-package check
  (`./scripts/cargow --lane fast-lane check -p quanta-index-contract -p
  quanta-index-sdk -p quanta-index-searchctl -p quanta-index-searchd-harness
  -p quanta-index-searchd-runtime --locked`) passed on the dirty source.
  Focused contract readiness decoder (1/1), SDK forged-green refusal (1/1),
  and complete searchctl library tests (42/42) passed. The focused searchd
  readiness synthesis tests passed (3/3), as did maintenance heartbeat (1/1)
  and shutdown-bridge (1/1). The P09 owner selector now includes a dedicated
  real-UDS process-readiness test target; `just rust-test-authority`,
  `just proof-authority-lint`, and `./scripts/cargow --lane fast-lane check
  -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --locked`
  passed.
  The real-UDS test execution remains `NOT_RUN`: its extended-suite attempt
  was interrupted after 20 minutes while compiling `lancedb` under concurrent
  Mac load, before the test body or linker ran. No release daemon or Linux
  process receipt is inferred from a test-target compile.
  These changes are not
  yet a clean-source owner or Linux release receipt, and they do not close the
  P09 request-correlation/metrics work outside readiness.

- Integration follow-up on local `main`: W10 integrate's two unique test
  corrections were cherry-picked as `ed954e0a` (cursor negative wire fixtures)
  and `16c979ba` (atomic lease-holder terminal report). W10 R5 RCA's unique
  benchmark refusal oracle and source-attribution record were cherry-picked
  as `53a6e344`. The source tips `adea09bb`, `39011ab3`, and `47ec2b25`
  were patch-equivalent to `main`, not ancestors of it. After the attached
  worktrees became inactive and clean, the W10 R5 RCA and integrate worktrees
  and branches were removed. Their source commit IDs remain the attribution
  record.
- Focused post-integration results on the shared dirty checkout:
  `./scripts/cargow test -p quanta-index-contract --test lexical_cursor_contract`
  (4/4), `./scripts/cargow test -p quanta-index-searchd-runtime --test
  runtime_supervisor_owner_v1` (15/15), `python3 tools/prompt-manager/pm.py
  lint` (4 generated targets in sync), and `python3 -m pytest
  tools/prompt-manager/tests/test_pm.py -q` (15/15). The harness's
  `authority_responses_are_not_counted_as_served_queries` unit test passed
  (1/1). The prompt-manager and
  Sep-23 test-optimization files were already dirty under another writer and
  were not staged, committed, or overwritten here. These commands are not
  exact-clean-source owner/release receipts.
- `python3 tools/ci/lint/check-proof-authority.py --require-all
  --bind-source --paired-checkout
  github:josongmin/semantica-codegraph-v2=<local checkout>` refused the
  shared dirty source with 75 findings. Historical owner manifests are bound
  to earlier HEAD/dirty/upstream states; P03–P10 release, P11 external,
  P12A and P12 final artifacts remain absent. The count is repeated binding
  findings plus missing nodes, not 75 independent product defects.
- `just proof-p12a-proof-infrastructure` completed its infrastructure checks
  with 11/11 Python tests passed. This checks the existing proof machinery,
  not the missing P12A handoff-DAG implementation or final aggregate.
  `git cherry -v main` reports `-` for all three unique commits in the two
  SEP-21 W10 branches, confirming patch-equivalent content on `main`.
  Both attached worktrees were clean and had no matching live process at the
  cleanup inspection. Their patch-equivalent branch tips were then deleted;
  this does not turn the old branch tips into ancestors of `main`.

- Local `main` was clean at this turn's initial inspection at `c93bf10`.
  W10-R3's clean `codex/sep21-w10-state-custody` branch was merged without
  conflicts at `22e9ba1`. The branch remains intact. This merge is local;
  remote publication and final-source qualification are separate.
- P03–P10 have historical owner-proof manifests and
  `RELEASE_PROOF_PENDING` handoffs. Every corresponding release proof is
  `NOT_RUN`. Those manifests name earlier source revisions and do not qualify
  the current `main` source.
- `artifacts/sep-21/handoffs/` currently contains P03–P10 only. The P00,
  P01, P02A, P02B, P02I, P11, P12A, and P12 handoff chain is absent locally.
  A missing historical handoff cannot be replaced by a fabricated current one.
- P11's four external proof nodes and P12 final qualification remain staged.
  P12A infrastructure is declared executable, but its required handoff-DAG
  aggregate producer is not implemented.
- The merged P10 work establishes read-only legacy semantic import and source
  fingerprint custody, but not current-format RepoMap authority conversion.
  The importer previously declared success after copying V1 RepoMap files
  into an inert `legacy-import/` tree. A current-main follow-up refuses that
  unsupported input typed; see the P10 implementation log below.
- W10's competing wire-oracle commits were source-compared. R4 `550535e`
  was cherry-picked alone as `dc4206a`; integrate `bd824ff` was not merged.
  Its invalid-page decoder fixtures depended on first encoding an invalid
  typed page, whereas R4 independently mutates valid CBOR bytes and pins the
  pre-S21 reader shape. At `dc4206a`, the two contract integration tests
  passed 50/50 and 17/17; `just fmt-check` exited 0. This is local contract
  evidence, not a final-source release receipt.

## Structural work ledger

| Order | Owner | Current finding | Required closeout |
| --- | --- | --- | --- |
| 1 | P06 / S21-07 | Query-plane active resolution and exact SDK pin binding are implemented. Hybrid/semantic-scope server selection now reads one composite active head. Durable activation epoch, request/read-view commitment binding, final-source owner/release proof remain open. | A response from the same repo/revision but wrong resolved generation/epoch/commitment is rejected by a consumer-visible negative oracle. |
| 2 | P08 / S21-09 | Candidate `8b78f35` repairs guard custody and passes owner proof. A current-main follow-up fixes the shutdown bridge join after supervisor-initiated failure; release/process proof is still absent. | No live child can outlast state-root lease custody; required-child failure and hard drain have release process-boundary evidence. |
| 3 | P09 / S21-10 | Current-main patch wires supervisor-owned process readiness and active-pair proof. Focused contract/SDK/CLI/searchd and zero-hit execution/contribution tests passed on dirty source; owner-target compilation passed. Real-UDS execution, clean-source process/release receipts, bounded request-stage diagnostics and release metric proof remain open. | Supervisor-owned process readiness is wired; component death or stale heartbeat makes readiness false without confusing it with repository generation status. |
| 4 | P10 / S21-11 | Read-only semantic and pre-catalog auxiliary import are wired offline. Boot refuses legacy snapshots before adapter/catalog open; mixed roots with unconverted data and nonempty V1 RepoMap fail closed. | Producer replay for materialized V1 RepoMap, source-to-destination active identity/replay floor/high-water equivalence, and final-source owner/release proof remain required. |
| 5 | P11 / S21-12 | V1 RepoMap mutation entrypoints remain reachable; no exact Quanta/Semantica commitment-chain or four P11 receipts. | One clean source pair and attested daemon binary pass publish, activate, replay, incompatibility, deployment, activation, and rollback proofs as separate nodes. |
| 6 | P12A / S21-13B | Aggregate schema/writer/validator do not consume the product handoff DAG or separate P12A infrastructure handoff. | Exact P00–P11 fork/join and serial chain, historical and final receipt ledgers, paired source, binary, and negative tamper cases validate. |
| 7 | P12Q / S21-13B | Final proof graph and P03–P10 release nodes have not run on one final source. | Same-source and same-binary final rerun yields distinct code/deploy/activate/rollback verdicts; no missing or stale mandatory receipt is promoted to green. |

## Execution constraints

- Reissue P10 proof/handoff against the eventual final source. Its earlier
  handoff revision is not the merged `main` and cannot serve as P11's exact
  predecessor.
- P11 needs separately confirmed Semantica read/edit/commit/push authority.
  Provider egress, deployment, activation, and rollback are separate
  approvals. Without them, record the specific node as `NOT_RUN` or `BLOCKED`.
- The clean W10-R3 worktree and its ancestor branch were removed after the
  merge; its commits remain reachable from `main`. The clean W10-R1 worktree
  and its patch-equivalent branch were removed after preserving its original
  commit at `archive/w10-r4-oracle-source`. The W10 integrate worktree
  remains intact: its latest `8c30c9e` open-loop fix is patch-equivalent to
  main `22f0f0a`, its `bd824ff` oracle commit is superseded by R4, and a
  daemon test process plus ignored fuzz artifacts were present at cleanup
  inspection. Do not remove that active worktree or its branch yet.
- A focused code test is owner evidence only. Release, Linux process,
  external-provider, deployment, and activation proof are not inferred from it.
- P06 cannot independently reject a wrong same-domain active generation
  with the present request contract: `Active` carries repo/revision, not an
  expected pin/epoch/commitment. A response-only proof is self-attestation.
  The correctness-first path is active resolve then exact pinned query; the
  additional IPC round trip is an explicit product decision.
- P09 must not wire a synthetic `ready=true`: boot's active-pair proof is not
  automatically a fresh integrity proof after mutation or disk damage.
  Supervisor child liveness, maintenance heartbeat freshness, backend open,
  active-head integrity, and provider claim need one owned, fail-closed
  observation model before `readiness: None` can be replaced.

## Current-main implementation log

### P06 active-selector binding — main-checkout work in progress

- Current main follow-up: `query_dispatcher::selection::resolve_joint_active_selection`
  obtains one lexical+semantic composite head when both selectors are
  `Active`. Hybrid, hybrid-seed and semantic-with-lexical-scope share that
  authority instead of independently reading lexical and semantic heads.
  The explicit lexical pin must match the composite head; differing active
  repo/revision pairs fail before opening a lane. Focused dirty-source tests:
  `./scripts/cargow test -p quanta-index-search-plane
  joint_active_selection_uses_one_composite_head_and_checks_explicit_pin
  --lib` (1/1), `query_dispatcher::tests::semantic` (8/8), and
  `query_dispatcher::tests::hybrid` (18/18). After the test-oracle lint
  correction, focused Clippy (`./scripts/cargow clippy -p
  quanta-index-search-plane --lib --tests -- -D warnings`) and the joint
  selection test (1/1) passed on the updated dirty source. This is a
  server-side same-snapshot correction, not durable epoch/commitment or a
  release proof.

- Work is being performed directly on local `main`, with the unrelated
  Sep-23 retrieval-benchmark and agent-rule edits preserved. The active-pin
  checkpoint is committed at `599ad8b`; the non-lexical time-rebind
  restriction is committed at `98153c1`. Neither is final-source release
  proof. The lexical planner preflight is committed at `ee6d1b4` and the
  semantic active-authority correction at `f10a6c4`; neither is final-source
  release proof.
- The query IPC now exposes a read-only active-generation resolution opcode
  backed by `ActivationCatalog::resolve_record`. SDK query dispatch centrally
  resolves active lexical/semantic selectors on that same query socket,
  binds the final response to the resolved pin, and keeps the query-only
  client independent of the control socket. Final requests carry both the
  resolved pin and original `Active` selector: the server rechecks that the
  active head still equals the pin. Semantic reads also retain catalog
  manifest-digest validation in the acquired read view.
  A producer test advances the active head between resolution and a request
  carrying the old pin plus `Active`; the latter fails before any lane opens
  (focused test passed). It is a changed-generation race oracle, not an
  activation-epoch proof.
  An independent scripted resolution followed by a wrong same-domain query
  generation is rejected by the SDK.
- Local focused evidence so far: `./scripts/cargow check -p quanta-index-sdk
  -p quanta-index-search-plane -p quanta-index-contract`, SDK lib (101/101
  after the lexical-preflight SDK tests), contract active-resolution CBOR/JSON
  test, search-plane catalog-resolution test, real UDS SDK binding owner target
  (13/13), `just rust-wire-inventory`, and `just rust-public-api` after the
  intentional contract/SDK baseline update passed. `just rust-fuzz-smoke 5`
  and the daemon all-target check were started but stopped during dependency
  compilation under concurrent shared-main Cargo contention; neither has a
  pass result. Final-source P06 owner/release manifests remain pending.
  Structural active selection remains unsupported by
  `ActivationCatalog::resolve_record`; the SDK refuses it explicitly.
- The follow-up restricts time-rebind tolerance to lexical queries: symbol,
  semantic, hybrid, history, runtime and structural responses retain exact
  request-pin binding even when their query text contains the token. A new
  `ResolveLexicalGeneration` opcode reuses the production lexical planner to
  select the ancestor before execution; SDK binds the final response to that
  result. Contract round-trip and SDK positive/wrong-same-repo/foreign-repo
  negatives passed. The search-plane real planner test (ancestor and
  before-history empty) and invalid-timeref test passed. The real UDS owner
  target passed 13/13, `just rust-wire-inventory` passed, and
  `just rust-public-api` passed against the intentional baseline update.
- A post-checkpoint read-view audit found that rewriting a Semantic `Active`
  selector to `Pinned` suppressed the server's
  `expected_manifest_digest` check. The main-checkout follow-up keeps the
  Semantic `Active` selector alongside the resolved explicit pin. The
  server's semantic selection validates equality and carries the catalog
  digest into read-view validation; SDK still exact-binds the response pin.
  The focused SDK wrong-generation negative, producer selection oracle and
  SDK lib 102/102 passed after this correction.
- This does **not** close S21-07. Activation epoch/content binding beyond the
  resolved generation pin is not yet a request-level contract. The two-step
  planner resolution trusts the server's first result and does not prove
  catalog epoch or immutable content in the final read view. Do not promote
  the focused oracle to those acceptance claims.

### W10 contract oracles and request-ID failure classification

- R4 independently checks producer encode refusal and consumer decode
  refusal on keyset-page invariants. The consumer fixtures start from valid
  encoded pages and mutate raw CBOR; a producer-side refusal cannot make a
  consumer negative test vacuous. The V0 `SearchExplanation` pin matches the
  pre-S21 field set and verifies both old payload defaulting and old-reader
  rejection of each new field.
- `IpcError::ZeroRequestId` is a request-correlation protocol failure. The
  open-loop harness previously had no match arm for it, so its binary could
  not compile against the current IPC error enum. The harness now classifies
  it as `zero_request_id` with an owner-local regression test. This harness
  change was committed as `22f0f0a`; the full `open_loop_matrix` binary suite
  passed 9/9 and `just fmt-check` exited 0. The equivalent peer commit
  `8c30c9e` was not merged, nor was its duplicate oracle predecessor.

### P10 unsupported RepoMap cutover — in-progress follow-up to `22e9ba1`

- Current-main follow-up: pre-catalog history/runtime/structural snapshots are
  classified as legacy before any boot adapter or catalog opens. The offline
  importer decodes them read-only and applies all derived rows to the staged
  catalog in one transaction. The old boot-time migrate-and-delete entrypoint
  is removed. An inventory gate rejects any source data file without a
  converter, including a mixed legacy marker plus lexical/catalog authority;
  corrupt snapshots and symlinks publish no destination. The source-side
  semantic `MIGRATED` refusal remains typed and its inert lock residue remains
  accepted. No producer graph is fabricated from V1 RepoMap snapshots.
- Focused dirty-source results after this follow-up: the search-plane
  nonempty auxiliary import oracle passed (1/1); the state migration owner
  suite passed 46/46, including source-byte equality and no-destination
  negatives; `just rust-public-api` reported contract and SDK APIs unchanged.
  These results do not issue a P10 proof manifest or Linux release proof.

- RCA: V1 `RepoMapSnapshot` contains materialized entries, not the graph
  `nodes`/`edges` and source-bundle commitment required by current
  `RepoMapSourceBundle`. Verbatim carry into `legacy-import/` kept bytes but
  produced zero serving RepoMap candidates. This was a false-success cutover,
  not authority migration.
- `LegacyStateImporterV1` now refuses any nonempty V1 RepoMap activation or
  snapshot directory with `StateRootFormatUnsupported` and an explicit
  producer-replay instruction. Empty legacy layout markers may be consumed;
  the source is not modified and no destination is published on refusal.
  The owner suite replaces the prior inert-byte success oracle with a
  negative materialized-RepoMap oracle and a convertible semantic-only path.
- This is a safety correction, **not P10 completion**. A lossless producer
  replay contract, active identity equivalence, replay-floor/high-water
  equivalence, and exact-source owner/release proof remain open. No legacy
  graph is fabricated from a materialized view.
- Focused local verification on this follow-up: `just fmt-check` exited 0;
  `./scripts/cargow test -p quanta-index-searchd-runtime --test
  state_migration_owner_v1` exited 0 with 41 passed, 0 failed, 0 ignored;
  `git diff --check` exited 0. This is owner-fixture evidence, not a final
  P10 proof manifest or release qualification.
- Exact code/doc checkpoint `03aabbf`: `just
  proof-p10-state-migration-owner` exited 0. Registered integration scope ran
  40/40 passed, lib scope 86/86 passed with one separately reported skipped
  test; hexagonal, wire inventory and public API checks passed. The command
  does not issue the missing final-source P10 release proof or P11 handoff.

### P08 custody correction — candidate checkpoint `8b78f35`

- Owner: `SearchdSupervisor` and its runtime owner suite. On hard escalation,
  startup rollback with unfinished children, required-child loss, or second
  signal, the supervisor transfers unfinished joins together with runtime
  guards to a custody reaper. The supervisor returns within its deadline,
  but the state-root lease is not released before the child exits. Reaper
  spawn refusal retains custody until process exit; the terminal supervision
  outcome is already non-green.
- Same-boundary correction: a second signal interrupts the drain's polling
  loop rather than waiting for a child that ignores cancellation. The
  process-parent lease fixture now waits for the completed `held` report,
  not merely for file creation. A quiet 10 ms poll is not a hard-deadline
  expiry. A reported child must actually finish before its join is taken;
  otherwise it is escalated with its guards at the hard deadline.
- Production spawn topology correction: accept loops run in the one
  registered supervisor thread. The already-running maintenance timer is
  directly adopted by its join handle, avoiding an adapter-spawn failure
  that would strand the timer. Startup enrolls maintenance and provider
  custody before starting any accept loop. The provider child retains
  attempt custody past a failed first drain, so a non-green hard-deadline
  result cannot release the state-root lease while an attempt still runs.
  A poisoned provider-pool lock is recovered for drain rather than
  misreported as an empty pool. An early drop/unwind of the supervisor
  now requests shutdown and transfers child handles with guards to the
  same custody reaper. A panicking child stop callback cannot unwind that
  reaper before it joins its children.
- Local proof before the candidate commit: `just fmt-check` exited 0;
  `./scripts/cargow test -p quanta-index-searchd-runtime --test
  runtime_supervisor_owner_v1` exited 0 with 15 passed, 0 failed, 0 ignored;
  `./scripts/cargow test -p quanta-index-embed
  poisoned_registry_does_not_claim_a_live_attempt_is_drained --lib -- --exact
  pool::tests::poisoned_registry_does_not_claim_a_live_attempt_is_drained`
  exited 0 with 1 passed. Earlier registered owner-suite passes reached
  13 and 14 tests before later source changes, and hexagonal/wire-inventory
  checks passed. The module-snapshot check was interrupted to continue the
  owner fix. These commands are not exact-commit P08 proof. An earlier
  owner-suite run failed one fixture because the parent observed a
  partially written helper report; its exact-content gate fixed that race.
- Exact candidate-commit owner proof: at `8b78f35`, `just
  proof-p08-runtime-supervisor-owner` exited 0: 15 owner tests passed,
  hexagonal boundaries passed, wire inventory matched, and contract/core
  module trees were unchanged. This is owner proof only, not the
  `p08-runtime-supervisor` release/process node.
- Remaining for P08: the required daemon process rail on a release-bound,
  final source/binary. The release-bound `p08-runtime-supervisor` node
  remains `NOT_RUN`.

## Audit corrections

- W10 R5 public-API RCA (inspection at `main@db46b12`, original Q1 source
  `2e8dce9`): the earlier R5 `FAIL` compared the aggregate
  `384cecb..2e8dce9` baseline diff with the correlation-only allowlist. That
  range includes interleaved work from other owners, while the allowlist also
  omitted R3's required typed corruption error. The four added lines in
  `tools/ci/lint/baselines/public-api/quanta-index-contract.txt` represent two
  symbols emitted through two export paths, with no removed lines:
  - `FileOwnerProjectionErrorV1: Error` (two export paths) came from
    `dae991e` and its baseline follow-up `e42cb82`. It is a test-optimization
    rail change, not W10 R1–R4. Keep it in the aggregate baseline because the
    production impl exists; exclude it from W10's change attribution.
  - `LegacySemanticJournalCorrupt` (two export paths) came from W10 R3
    `ea8feba` and its baseline follow-up `eb744e0`. The read-only legacy
    journal decoder maps corrupt CBOR to this typed error, and
    `state_migration_owner_v1` asserts it for corrupt/truncated input. R3's
    P10 contract explicitly requires fail-closed corrupt input. This is an
    intentional P10 error-surface addition omitted from the R5 prompt's
    correlation-focused allowlist; removing it or mapping it to an unrelated
    error to satisfy that list would weaken the owner contract.
  - W10 R1 `e3167dc`, R2 `721a128`, and R4 `dc4206a` changed no public-API
    baseline lines. The wire inventory is identical at `384cecb` and
    `2e8dce9`. Later P06 active-resolution additions in
    `2e8dce9..db46b12` belong to a separate source revision and are not R5
    evidence.
  The baseline was updated in `e42cb82` and `eb744e0`, before one serial R5
  rebaseline. Shared-main interleaving broke the intended single-writer
  chronology; Git commit provenance plus the producer source and owner oracle
  are the recoverable attribution evidence. Do not rewrite those commits or
  delete a real public API entry to manufacture a one-commit baseline history.
  R5 baseline finding disposition: **mixed-owner attribution and incomplete
  allowlist**, with the P10 typed-error addition explicitly documented as
  the R3 exception in S21-11. At
  `2e8dce9`, the isolated checkout reported `just rust-public-api`, the R5
  merge-tree, and local Q1 owner rails as passed. This ledger does not embed
  their raw command results or establish current-source qualification.
  Future shared-branch R5 reviews must pair each added baseline symbol with
  its source-producing commit and owner ticket before applying a lane
  allowlist. A baseline-only follow-up commit is not the origin of the API
  change, and an aggregate range is not a lane-specific write set.
  This correction does not issue a current-main owner manifest or qualify
  P10's remaining RepoMap migration, P11, Linux release, deployment,
  activation, rollback, or P12. Re-run those rails on one final clean source
  before claiming their verdicts.
- Current-main harness compile RCA at `db46b12`: P06 added
  `ActiveGenerationSnapshot` and `ResolvedLexicalGeneration` query responses,
  but the searchd harness and its open-loop binary retained exhaustive matches
  for the old enum. Building the P10 owner target failed before running a test
  with E0004 in `harness.rs`, `concurrency.rs`, and `open_loop.rs`. The harness
  fixes landed independently on main as `9d37f6d` and `0e9cd17`; this RCA
  branch retains only the missing concurrency negative oracle and does not
  duplicate those implementation commits. The harness treats either
  authority response to a normal search route as an
  unexpected response, never as a served result or benchmark row. Readiness
  waits terminate on such a response so the route can report the protocol
  mismatch instead of retrying until timeout. This is a consumer exhaustiveness
  correction for the current P06 contract, not a new W10 R5 public API delta.
- W10 execution truth and request correlation are integrated on local main;
  they are no longer listed as unmerged work.
- The production semantic adapter reruns a short approximate pass through an
  exact lane when the scope has unseen rows. Therefore a short ANN result
  alone is not a proven `ExactExhausted` defect. Do not reopen that claim
  without a reachable counterexample.
- The W10-R3 custody branch was merged at `22e9ba1`; its inert RepoMap copy
  success oracle was replaced on current main by a fail-closed oracle.
  Neither approach alone proves active-authority conversion or replay-floor /
  high-water parity; P10 remains open.
