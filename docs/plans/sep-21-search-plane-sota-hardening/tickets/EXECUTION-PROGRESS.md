# SEP-21 execution progress

Status: open. This is a source-inspection ledger, not a qualification receipt.
The authoritative result SHA, source digest, command counts, and push state belong
to validated lane handoffs and immutable proof manifests, not this document.

## Current checkpoint (2026-09-23)

- Integration follow-up on local `main`: W10 integrate's two unique test
  corrections were cherry-picked as `ed954e0a` (cursor negative wire fixtures)
  and `16c979ba` (atomic lease-holder terminal report). W10 R5 RCA's unique
  benchmark refusal oracle and source-attribution record were cherry-picked
  as `53a6e344`. The source branch tips remain in attached worktrees; no
  worktree or branch was deleted. These are content integrations, not a claim
  that the old branch tips became ancestors of `main`.
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
| 2 | P08 / S21-09 | Candidate `8b78f35` repairs guard custody and passes owner proof; release/process proof is still absent. | No live child can outlast state-root lease custody; required-child failure and hard drain have release process-boundary evidence. |
| 3 | P09 / S21-10 | Production control dispatcher composes `readiness: None`. | Supervisor-owned process readiness is wired; component death or stale heartbeat makes readiness false without confusing it with repository generation status. |
| 4 | P10 / S21-11 | Read-only semantic import is merged. Current follow-up refuses nonempty V1 RepoMap instead of publishing a current root with missing active authority. Boot still migrates pre-catalog auxiliary snapshots. | Source remains byte-identical; nonconvertible RepoMap fails closed without destination; a producer replay path and offline auxiliary conversion are required before active identity/replay floor/high-water parity can close. |
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
  replay contract, offline pre-catalog auxiliary migration, active identity
  equivalence, replay-floor/high-water equivalence, and exact-source owner /
  release proof remain open. No legacy graph is fabricated from a materialized
  view.
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
  `2e8dce9`, `just rust-public-api` passed against the source-bound baseline;
  the R5 merge-tree and local Q1 owner rails passed in the isolated checkout.
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
