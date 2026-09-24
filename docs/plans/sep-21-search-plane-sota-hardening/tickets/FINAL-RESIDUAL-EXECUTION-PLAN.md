# SEP-21 final residual execution plan

Status: planned; this document is not a proof receipt or implementation result.
The 2026-09-24 prerelease single-IR decision supersedes its P10 conditional
legacy-importer branch: `migrate-state` is removed, old roots are typed-refused,
and rebuild from typed producer input is required. The P10 analysis below is
historical; it does not authorize restoring a V1 success path. Current commands
and limits are in the operator state-cutover runbook. The decision does **not**
establish that every target root is current-format, that retained legacy data
may be discarded, or that a rebuilt target has been qualified.
Scope: current-source residual work after prior P03–P10 code checkpoints;
historical handoff completeness is separately unproven.
The exact HEAD, dirty digest, paired checkout, binary, commands, and verdicts
belong to fresh handoffs and proof manifests, never to this tracked plan.

Original planning baseline: Quanta `d4db8bbb49a7b98e75531ca7dab93fb4c771bf0a`
and Semantica `e702c4d05513aae5667f62a8f991bf2352c8ddfc` (both historical,
not an execution freeze). Current static re-audit below supersedes baseline
implementation-status statements, not the required clean-source proof gates.

### Final current-code SSOT and duplication audit (2026-09-24; static only)

This section is the operative **next-edit** plan. It supersedes all older
implementation-status and conditional-importer statements later in this file;
the older tables remain an audit trail, not an instruction to implement the
same seam twice. Inspected Quanta `main` HEAD
`98d2a7bb431d357d66ffdbf6a7c23a6e23780e85` (ahead 8, behind 2) and
Semantica `main` HEAD `d26ebef34d8e8d58e4382e18500ac052d5940d88`
(ahead 35, behind 28). Both shared trees are extensively dirty and may change
between reads. This audit ran no test, target-root inventory, paired build,
proof, deploy, activation or rollback. Re-freeze affected paths and writer
custody before editing; do not treat these observations as integrated code.

| Boundary | Observed current code | Decision / remaining failure mode |
| --- | --- | --- |
| P06 serving head | `readiness/activation_catalog.rs::ActiveRoots` still holds `SearchCorpusGenerationV1` only; `searchd/src/app/readiness.rs::ProvenActive` keys on generation inventory and scrub epoch. | One **catalog-owned** head event must bind CAS, resolve, read view, SDK and readiness. Content identity alone repeats on B→A→B. Root incarnation comes from the one state-root custody authority, not an SDK counter or second head table. |
| P09 diagnostics | The IPC counters have one bounded, sequenced event ring, loss metadata and process-instance ID. Query/ingest provider stages use the existing request-budget bridge. No authorized control-wire event-window read exists; readiness still has `required_backend: true` and no proof-age limit. | Reuse the ring/bridge. Add one bounded authorized projection and measurable backend/proof detection interval; no second bus, ledger or request-ID allocator. `Admin` currently includes owner UID and root, so confirm operator UID isolation before disclosing events there. |
| P10 state custody | Dirty CLI/runtime remove `migrate-state` and refuse old roots; current-format backup/restore/verify exists. | The prerelease no-importer policy is fixed, but real target roots and retained-data obligations are not inventoried. A retained V1 root is `BLOCKED` for cutover until an authorized rebuild/data decision; do not reconstruct producer IR from a snapshot. |
| P11 RepoMap CAS | The dirty flat V2 request now has mandatory wire `expected_active`; the catalog checks epoch+commitment under `BEGIN IMMEDIATE` before sequence allocation, and exposes an active-head read through the existing control/SDK route. The store still has one private commit/projection path. | **Review/integrate this candidate, do not reimplement CAS or add a head authority.** `for_bundle()` still defaults to `None`; Semantica's ordinary outbox and aggregate Required-member paths both reconstruct that default. Their common dispatch compares against and sends a freshly reconstructed request, discarding any future frozen expectation. Subsequent activations can fail closed; retry-time refresh would erase the caller's CAS intent. The token also lacks an incarnation bound to its actual catalog root, so restored-root ABA is not closed. |
| P11 V1 cutover | The dirty Quanta public store/port/wire/SDK V1 mutation success surfaces are removed; Semantica already has flat V2 publish/activate calls. | Review existing candidate and its old-wire refusal matrix once. Do not restore V1 methods, introduce a V2-to-V1 adapter, or create another private commit primitive. The `ipc/ingest.rs` non-RepoMap decoder changes need their own contract owner/proof. |
| P12A/P12Q proof custody | The dirty no-follow reader and pinned-parent publication candidate is shared by handoff CLI, proof checker, aggregate and manifest writer. The operational registry and independently fixed expected DAG are distinct. | Review the one custody leaf and issue authentic historical/final-source artifacts separately; no duplicate path reader, synthetic handoff, or registry-derived expectation. |

File-level execution units (one writer for each shared seam):

1. **F0 — custody and source freeze, before any release claim.** The release
   owner records both HEADs, dirty path ownership, lockfiles, intended daemon
   binary/host, exact named target roots, producer endpoint-to-physical-root
   mapping, and each root's format/data-retention
   inventory in the handoff/receipt inputs and
   `docs/operator/state-cutover-runbook.md`. Classify each target
   `CURRENT_FORMAT`, `REBUILD_AUTHORIZED`, or `BLOCKED`; do not infer this
   from an empty development root. DoD: an owner-approved disposition for
   every target; missing roots/input/authority stop the affected cutover.
2. **F1 — P06 catalog event and root custody, serial contract owner.** Extend
   `crates/quanta-index-search-plane/src/readiness/{activation_catalog,search_corpus_generation}.rs`
   and the existing persisted active record with one root-incarnation plus
   pair-local monotone head sequence; root-incarnation creation/admission is
   owned by `crates/quanta-index-searchd/src/app/{state_format,runtime}.rs`
   and the offline restore engine, not readiness or SDK. Change
   `crates/quanta-index-contract/src/ipc/{control,split}.rs`, query active
   resolution DTOs, `search-plane/src/query_dispatcher/` read-view binding,
   and `crates/quanta-index-sdk/src/{binding,client,generations,lexical}.rs`
   together so CAS expectation, ACK, resolve and active query carry the
   **same** event token. Update Semantica's existing frozen search-corpus
   expectation path in
   `.../authority_assembly/search_plane_handoff_dispatch/{aggregate_prepare,payload_persistence,dispatch_replay}.rs`
   to persist and replay that token; do not add a second producer-side CAS
   allocator. Invalidate `searchd/src/app/readiness.rs` proof cache on that
   event. DoD: stale B→A→B and controlled-restore tokens reject
   without mutation; exact ACK replay succeeds; no second active map or
   generation-only success route. Byte-identical uncontrolled clones remain
   outside the local guarantee unless an external fence is admitted.
3. **F2 — P11 RepoMap CAS candidate and producer operation intent, one
   paired contract/outbox handoff.** Review the existing changes in
   `crates/quanta-index-contract/src/repomap/terminal_receipt_v2.rs`,
   `crates/quanta-index-catalog/src/candidate.rs`,
   `crates/quanta-index-repomap/src/store.rs`, existing core/control/SDK
   adapters and `tools/ci/inventory/wire-surface.toml`. Retain catalog
   transaction CAS and catalog-backed active-head projection. Make
   `for_bundle()` require an explicit expected-head argument (including
   explicit absence) so no caller silently chooses `None`. Bind **every**
   activation request and active-head response to the F1 root-custody
   contract for the RepoMap catalog's **actual physical root**,
   even when no RepoMap head exists: `Option<RepoMapExpectedActiveV2>` alone
   cannot fence an absent-head request after restore. The catalog must
   compare this root custody identity before **both fresh activation and
   ACK-loss replay**; do not mint a second marker for the same root. On the
   Semantica side, add the SDK read only to
   `quanta-runtime-retrieval-kernel/src/index_sdk_ingress/{facade,repomap}.rs`.
   Cover **both mutually exclusive production modes**: ordinary handoff's
   existing `repo_map_handoff_dispatch/{durable_outbox,prepared_member_validation}.rs`
   and aggregate Required-member custody in
   `prepared_aggregate_owner_pipeline_v1.rs`,
   `ordinary_aggregate_prepare_v2.rs` and `prepared_aggregate_issuer_v1.rs`
   (all under `quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/`).
   Use one shared expected-head codec/target validator, but freeze the token
   in each mode's **existing operation custody**, never a parallel head store.
   The ordinary pre-manifest source intent stays immutable; after manifest
   binding, write one canonical, self-validated bound activation request
   through that outbox's atomic custody **before first dispatch**. Aggregate
   preparation must likewise include its bound request in the existing
   Required-member artifact **before that artifact is published**. Crash
   before durable binding may re-read; crash after it must replay the exact
   token. In `repo_map_handoff_dispatch.rs`, change
   `dispatch_repo_map_handoff_bundle_with_receipts_v2` to validate the source
   axes without comparing the frozen request to a new `for_bundle(None)` and
   send the **supplied bound request**. Change
   `dispatch_exact_repo_map_terminal_receipt_v2` to accept the persisted
   aggregate request instead of constructing another `None` request. The
   currently unreferenced non-test direct-dispatch helper must not remain a
   third unpinned success path: remove it or route it through the same
   custody. A conflict stays terminal for that intent;
   intentional rebase requires a new authorized operation, not retry-time
   refresh. DoD: first and subsequent handoffs, stale absence, two contenders,
   changed-expectation replay, ACK loss, crash at each bind/dispatch/ACK
   boundary, and deliberate rollback have independent oracles for catalog
   row, sequence and query. No second catalog, importer write port, or
   producer-generated head token. The current catalog replay derives the
   original expectation from the self-digested predecessor row; verify this
   across invalidation/restart. Only if it cannot prove exact replay, add
   an explicit field to the **same** catalog activation row with a coherent
   format/open-time refusal rule; never create a parallel receipt table.
4. **F3 — P09 event projection and freshness, disjoint implementation but
   F1-dependent readiness.** Add one bounded event-window control DTO/route
   in `crates/quanta-index-contract/src/ipc/{control,split}.rs`,
   `crates/quanta-index-search-plane/src/control_dispatcher.rs`,
   `crates/quanta-index-searchd/src/app/ipc_dispatcher.rs`, and the SDK
   observability namespace. Read only
   `crates/quanta-index-ipc/src/counters.rs::request_event_window_v1`; cap
   encoded bytes below the IPC frame maximum and expose loss/truncation.
   Declare an operator principal in `searchd`/control access before wiring
   it; same-UID producer access cannot be called operator-only. Give the
   required backend and physical-proof probe in `searchd/src/app/readiness.rs`
   a maximum observation age and failure horizon; bind its cache to F1's
   event. DoD: real-UDS query+ingest correlation and provider negative,
   unauthorized read denial, bounded frame under full ring, backend loss and
   stale-proof failure within the stated interval. Do not build another
   diagnostic sink or make event data a usage ledger.
5. **F4 — P10 current-format custody and P11 public cutover.** Keep
   `crates/quanta-index-searchd-runtime/src/state_migration.rs`,
   `crates/quanta-index-searchd/src/app/{state_migration,state_format}.rs`
   and CLI on current-format backup/restore/verify plus typed old-root
   refusal. Update `state_migration_owner_v1.rs`, operator runbook and
   P10 proof selection to this contract only after F0 disposition. Review
   the already removed V1 RepoMap store/port/wire/SDK surfaces and the
   Semantica V2 ingress without a compatibility layer. Assign unrelated
   `contract/src/ipc/ingest.rs` decoder tightening to its own proof. DoD:
   exact paired clean-source compile, owner/UDS/crash/replay/old-wire
   refusal rails and source-bound handoff. State-root inventory and deploy,
   activation, rollback remain independent verdicts.
6. **F5 — P12A/P12Q proof, serial after F2–F4.** Review existing
   `tools/ci/lint/handoff_validation.py`, `check-lane-handoff.py`,
   `check-proof-authority.py`, `write-proof-{aggregate,manifest}.py` and
   registered owner negatives; keep one no-follow byte reader and one
   pinned-parent publication primitive. Validate authentic historical
   handoffs against the independent expected DAG, then run the final-source
   aggregate against the same paired SHAs, binary, config and host. DoD:
   missing/alias/symlink/swap/tamper/wrong-pair negatives fail, all required
   proof nodes execute, and deploy/activation/rollback each have their own
   receipt. A dirty local test pass is not P12 closure.

F2's RepoMap catalog CAS and F1's search-corpus activation are separate domain
heads. Reuse one incarnation custody implementation for the **same physical
root**, but never treat distinct roots or producer endpoint names as one
incarnation and never merge the two head tables.
F3 event projection and F5 validator code can be reviewed on disjoint paths,
but `contract`/SDK, state root, producer outbox and final proof artifacts
integrate serially. Re-evaluate the plan if F0 changes target-retention policy,
operator identity, root-fencing guarantee or the canonical wire shape. No
test or verification claim is made by this static plan.

Verification selection after each coherent edit (none run by this audit):
`just rust-wire-inventory` and `just rust-public-api` after contract/SDK
integration; `just proof-p06-sdk-binding-owner` after F1;
`just rust-profile test-candidate-activation-owner` after the F2 catalog
cut and a paired Semantica owner/SDK rail after outbox binding;
`just proof-p09-control-readiness-owner` after F3;
`just proof-p10-state-migration-owner` after F4 custody reconciliation;
`just rust-verify-hellgate-cross-repo` on the final clean pair; and
`just proof-p12a-proof-infrastructure` before
`just proof-authority-final-qualification`. Re-check registered selectors and
actual executed counts, not only recipe exit status. A prior dirty/local
result cannot be reused after any correctness-bound input changes.

### Historical execution overlay (2026-09-24; superseded static snapshot)

This snapshot predates the RepoMap CAS and active-head read candidate now
observed above. Use the final current-code audit for next edits. The P10
conditional importer actions in sections 1, 4 and 7 remain historical design
analysis, not runnable instructions.

| Gate / sole owner | Current-source finding and file-level action | Independent DoD / stop |
| --- | --- | --- |
| R0 target custody; `searchd/src/app/state_format.rs`, `searchd-runtime/src/state_migration.rs`, operator inventory | Record every named target root's format, owner, digest, active generations, retained legacy data and producer rebuild input. The current policy is typed old-root refusal plus rebuild; do not silently revive `migrate-state` or infer a waiver from an empty development root. | A retained legacy target without an explicit authorized rebuild/data-retention decision is `BLOCKED` for cutover. Inventory is not a migration receipt. |
| R1 P06 search-corpus head; `search-plane/src/readiness/{activation_catalog,search_corpus_generation}.rs`, `searchd/src/app/{state_format,runtime}.rs`, contract IPC, query read view, SDK | Extend the **one** catalog active record with root incarnation and pair-local head sequence. Bind CAS, resolve, active query/read-view and SDK to the same event token; keep generation as content identity. Root custody owns incarnation admission. | ABA B→A→B and controlled restore reject old tokens; missing/uncertain incarnation fails closed. An uncontrolled byte-identical clone needs external fencing or an explicitly narrower guarantee. No SDK epoch or second active map. |
| R2 P09 diagnostics; `ipc/src/{server,counters}.rs`, `core/src/request_budget.rs`, search-plane provider entrypoints, `searchd/src/app/{ipc_dispatcher,readiness,runtime}.rs`, control contract | Review the existing bounded event ring, shared process instance and budget bridge. Add only a bounded control projection over that ring after the operator principal is fixed. Give backend and physical-proof checks a stated maximum detection interval; invalidate proof on the P06 head event. | Real UDS query/ingest request-ID correlation, loss/truncation, authorization denial, bounded serialized frame and timed backend-loss negatives. `required_backend: true` and an ageless cached proof cannot certify ongoing health. |
| R3 P11 RepoMap activation CAS; `contract/src/repomap/terminal_receipt_v2.rs`, `catalog/src/candidate.rs`, `repomap/src/store.rs`, SDK/producer activation callers | The flat V2 request binds the **target** candidate axes but has no expected-current-active field. `activate_repomap_candidate` compares only target commitment before replacing the current row. Add one typed expected-active identity (including epoch and candidate commitment, or an equally strong catalog token) to the existing V2 request and compare it with the catalog row **inside the same transaction**, before any sequence allocation. Keep exact-active ACK-loss replay idempotent. Do not impose generation-number monotonicity: an explicit rollback to an older sealed candidate may be valid. | Publish two sealed candidates; activate newer, then submit an older stale request expecting the prior head: typed CAS refusal, unchanged row/epoch/sequence/query. Test expected-absent race, concurrent contenders, exact replay and deliberate rollback with the correct expected token. No second head table, store-local CAS or SDK-inferred epoch. This is distinct from P06's search-corpus head. |
| R4 P11 V2 cutover; `repomap/src/store.rs`, core ports, contract wire, search-plane dispatchers, SDK, Semantica producer | Integrate the existing private commit/projection-replay and flat V2 candidates once, together with R3 CAS. Remove V1 success at **all** public store/port/wire/SDK boundaries; keep the catalog as durable authority and in-memory maps as projections. | Exact paired clean-source compile/owner rails, V1 refusal and V2 publish/activate/ACK replay, then separate deploy/activation/rollback receipts. Do not re-extract a commit primitive or recreate a V1 adapter. |
| R5 P10 current-format custody; `searchd-runtime/src/state_migration.rs`, CLI, `state_migration_owner_v1.rs`, operator runbook, test/proof authority | Accept the explicit no-importer policy only after R0 target/data decision. Prove current-format backup/restore/verify, typed old-root refusal and restore-forward; reconcile historical ticket/prompt prose with the current policy without weakening the independent proof DAG. | A target requiring retained V1 data is blocked pending a separately approved migration/rebuild design; no snapshot-to-source IR. P10 owner/release receipts remain unissued until clean exact-source rails run. |
| R6 P12A/P12Q; `tools/ci/lint/handoff_validation.py`, shared file-custody leaf, proof writers/checker/registry | Review the existing no-follow reader and pinned-parent writer candidate; issue authentic historical handoffs and final-source proofs separately. Keep operational registry and fixed expected DAG independent. | Symlink/swap/tamper negatives, exact paired SHA/binary/host, proof DAG, then distinct deploy/activation/rollback. Dirty local unit passes cannot close P12. |

Code can develop on disjoint owners, but integration is serial at contract/SDK,
catalog/state-root and proof artifacts. R3 CAS belongs to the **RepoMap catalog
transaction**, not to P06's search-corpus head or P09's diagnostic ring. The
registered P09→P10→P11 proof dependencies remain in force even when P10 has no
importer. Re-freeze the two repositories and assign one writer per shared seam
before editing; a passing local owner target on this dirty tree is not an
exact-pair or release verdict.

R3 token-acquisition detail: the current `RepoMapGenerationStore::activated_generation_for`
returns only a generation from an in-memory projection; the IPC exposes no
full RepoMap activation-row resolution. It is not a CAS oracle. Add one
read-only, authorization-scoped RepoMap active-head projection to the existing
control route (`contract/src/ipc/split.rs`,
`search-plane/src/control_dispatcher.rs`, store/core port, SDK) sourced from
the catalog row, or prove a stricter single-writer token custody invariant
before omitting it. The Semantica handoff/outbox callers in
`repo_map_handoff_dispatch/{durable_outbox,prepared_member_validation}.rs`
must durably capture the exact expectation with
each intent. A retry reuses that expectation; it must not silently refresh it
after a conflict and overwrite another producer's activation. The producer
may resolve and intentionally rebase only as a new authorized operation.
In `catalog/src/candidate.rs`, the replay branch must verify the same
original expected-prior token, not merely the same currently active target
commitment. Persist that expectation (or its canonical request digest) in
the self-validated activation row/terminal authority; update its format and
offline refusal rule coherently. Compare an explicit `None` expectation to
absence and a `Some` expectation to the exact current epoch/commitment in the
immediate transaction. On mismatch, allocate no sequence and mutate no row.
This retains ACK-loss replay without allowing a changed expectation to borrow
an earlier success receipt.

### Latest dirty-source collision audit (2026-09-24; static only)

Quanta `main` HEAD remains `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85`;
the shared tree is extensively dirty and may change between reads. No test,
state-root inventory, release proof, deployment or paired Semantica check was
performed for this audit. These are observed candidate bytes, not accepted
implementation. Re-freeze affected paths and their owner before any edit.

| Priority / seam | Current-source observation | One-authority execution gate |
| --- | --- | --- |
| P0 P10 migration policy versus target custody | The dirty runtime/CLI removed `LegacyStateImporterV1`, `run_offline_migrate_v1` and `migrate-state`; boot refuses old roots. The runbook and staged P10 proof registry now explicitly describe current-format backup/restore/verify and old-root refusal. Historical migration ticket/prompt text remains. | The prerelease no-importer policy is the current contract, not an unapproved implementation branch. R0 must still inventory real target roots and retained data. Block any legacy target without an explicit rebuild/data-retention decision; do not restore importer code or claim that policy alone proves a safe cutover. Reconcile historical prose and exact-source owner/release receipts under the existing P10 proof nodes. |
| P0 P06 serving-event SSOT | `readiness/activation_catalog.rs::ActiveRoots` still maps a pair to `SearchCorpusGenerationV1`; activation/rollback and `ResolveActiveGeneration` still expose generation identity without a catalog head event. No root-incarnation or head-event implementation was found in the inspected contract/search-plane/searchd sources. | One durable catalog head record is the CAS/resolve/read-view/SDK authority. Preserve the existing generation as content identity and existing query resolution route. Root-incarnation creation/admission belongs to state-root custody, not SDK, readiness or RepoMap. Byte-identical uncontrolled clones require an external fence or an explicitly narrower controlled-restore guarantee. |
| P1 P09 existing diagnostic candidate | The dirty IPC ring already has bounded sequenced windows, loss metadata and process-instance identity; runtime constructs one process-instance for query/control/ingest. The control request enum has no event-window read, and `RuntimeReadiness` still reports `required_backend: true` while caching physical proof by generation inventory and scrub epoch without a time horizon. | Wire **one** operator-authorized bounded projection over the existing ring; do not add another ring, ID allocator or process-instance source. Define the backend/physical-proof detection horizon and bind cache invalidation to the P06 head event. Decide whether owner UID is exclusively operator before using `Admin` as the read capability. |
| P1 P11 V2 candidate and missing prior-head CAS | The dirty store has one private `commit_bundle`/`commit_activation`; public V2 paths call them. The replay/projection fix passed its local owner target 17/17. Separately, `RepoMapActivateGenerationRequestV2` has no expected-active identity and the catalog transaction only compares the target commitment: a previously sealed candidate can supersede a newer activation without the caller proving which head it intended to replace. | Preserve the existing private primitive and catalog SSOT. Add the prior-head CAS once in the catalog transaction and the flat V2 request, then propagate it through store/SDK/producer. Use an independent stale-request refusal oracle before P11 paired cutover; do not substitute generation monotonicity for explicit rollback semantics. Dirty local tests do not establish release proof. |

Execution dependency correction: the P11 V2 store primitive can be reviewed
before P10 current-format proof, but the no-importer policy does not prove
that retained target data is disposable. Freeze target inventory first. P09 event-read
work may proceed on disjoint owners; P06 owns head-event semantics before P09
readiness invalidation. P12A's existing shared no-follow reader/writer candidate
is review-and-proof work, not another abstraction to implement.

### Final no-duplicate audit (2026-09-24; current dirty checkout)

Inspected Quanta `main` HEAD `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85`
(`origin/main` ahead 8, behind 2). This is a moving, extensively dirty shared
checkout, **not** an integrated result SHA. The table is a static reachability
review of the observed bytes, not acceptance of another writer's edits.
Semantica `main` HEAD `d26ebef34d8e8d58e4382e18500ac052d5940d88`
was also inspected read-only: its checkout is extensively dirty (ahead 35,
behind 28), and the RepoMap ingress facade/handoff candidate uses flat V2
publish/activate requests and `RepoMapActivateGenerationRequestV2::for_bundle`.
This is a producer candidate to integrate, **not** a missing V2 wrapper to
reimplement. No target state root, clean-source rail, release binary or
deploy/activation/rollback receipt was validated. Later edits invalidate this
inventory.

| Seam | Current observation | Execute once / stop condition |
| --- | --- | --- |
| P06 catalog head | `activation_catalog.rs::ActiveRoots` still stores only `SearchCorpusGenerationV1`; `RuntimeReadiness::ProvenActive` caches by generation inventory and scrub epoch. | One catalog-owned head event and one root-incarnation authority, then bind CAS, resolve, read view, SDK and readiness to that event. Do not add a second active map, SDK epoch or query-local identity. Stop restore guarantees until controlled admission or external fencing is chosen. |
| P09 events/readiness | The existing IPC ring and dirty request-budget bridge now have query provider tickets, ingest window stages, bounded sequenced reads and one process-instance candidate. There is still no authorized control-wire reader. `required_backend: true` and an ageless physical-proof cache remain. | Integrate/test the **existing** ring and bridge; add one authorized bounded projection on the existing control route. Set a measurable detection horizon and invalidate physical proof on P06 head event. Do not add a provider ledger, another event bus or a request-ID allocator. |
| P10 current-format custody | The no-importer prerelease policy and dirty CLI/runtime remove `migrate-state`; no original-bundle replay exists or is promised by this policy. | Inventory every real target and its retained-data obligations before cutover. Prove current-format backup/restore/verify and typed old-root refusal on clean source. A target needing retained legacy data stays blocked pending a separate rebuild/data decision; do not add a parallel importer or snapshot-to-IR conversion. |
| P11 V1 cutover | The Quanta **dirty candidate** removes V1 RepoMap wire variants, public core/store V1 mutation methods, SDK V1 signatures, the nested `request_v1` DTO and weak projection-metadata decoder. V2 routes through private `commit_bundle`/`commit_activation`; local contract/SDK/RepoMap/search-plane and daemon-fast tests passed on changing dirty snapshots, not an integrated SHA. The Semantica producer's dirty candidate already calls flat V2 publish/activate. The V2 request still lacks expected-current-active CAS. | Review/integrate both candidates once and close the RepoMap catalog CAS gap before paired cutover; do **not** re-extract commit logic, recreate producer wrappers or reintroduce V1 as an intermediate protocol. Require exact-source paired qualification and deployment/activation/rollback receipts. Current `ipc/ingest.rs` diff also tightens search-corpus/semantic decoders: keep its separate proof explicit before P11 acceptance. |
| P12A proof file custody | Pre-patch, the handoff/checker and manifest-producer path readers followed in-repo symlinks. The current **dirty candidate** shares a no-follow descriptor-walk reader across handoff CLI/ledger, proof checker, aggregate and manifest producer; both writers use one pinned-parent output-custody primitive for their distinct publications. A broken registered-manifest symlink is `FAILED`, not `NOT_RUN`. Same-byte alias/archive, archive/current-alias parent-swap and postpublication rollback negatives are in the owner rail (118 local tests, zero manifests validated). | Review this one candidate and its reader/writer sibling universe; do not build a second file-custody abstraction. Issue authentic clean-source proof; the dirty local P12A pass is not a historical handoff, exact-pair proof or release qualification. |

Integration order is R0 → P06 contract/head → P09 and P12A code review on
disjoint owners → RepoMap prior-head CAS plus existing V2 candidate integration
→ P10 current-format custody proof → P11 paired public cutover → P12A/P12Q
proof. P09 and P12A code
may develop independently, but their registered proof DAG and the shared
contract/SDK/state-root seams impose the serial handoffs below. Existing dirty
work is neither discarded nor declared complete by this sequence.

### Earlier current-source re-audit (2026-09-24; static only)

Quanta source snapshot `a0ac1853256d9b507ae8dc76f7437a4c7568434c`
(`main`, ahead of `origin/main` by six and behind by two commits at inspection).
This shared commit absorbed the P09 IPC and P12A handoff code together with
unrelated retrieval work. The checkout still had dirty `Justfile`, SEP-21
progress/plan docs and a retrieval proof test. Re-freeze before execution;
the repository is not a clean result SHA. Semantica HEAD/dirty state and
actual deployed state roots were **not** revalidated in this pass. No
release proof, deployment, activation or rollback drill ran for this audit.
Local focused tests are recorded separately in `EXECUTION-PROGRESS.md` and
are not a clean-source qualification receipt.

| Lane | Confirmed current source | Residual / no-duplicate decision |
| --- | --- | --- |
| P06 | SDK `binding.rs` compares complete typed activation/rollback identity; the old `same_identity` field subset is gone. `ActivationCatalog::ActiveRoots` remains `BTreeMap<ActivationKey, SearchCorpusGenerationV1>` and persists `PersistedSearchCorpusGenerationRootV1`. | Treat the SDK fix as partial, not another task. Add one catalog-owned event token and extend the existing contract, resolution, read-view and SDK path. No parallel head map or SDK epoch. |
| P09 | IPC code at this HEAD has `RequestEventScope`, a bounded 1024-entry `IpcServerCounters` tail and a dropped count; searchd's generic adapter projects closed route and top-level typed error. Overload encode/write events are distinguished. Query and ingest `PlaneDispatch` still ignore `DispatchContextV1`; the tail has no operator read path. `RuntimeReadiness` still sets `required_backend: true`, and its active proof cache has no age horizon. | Finish provider-stage correlation and a bounded, authorized diagnostic read path on the existing control plane; do not create a second event bus, request-ID allocator or provider-usage ledger. Define the backend/physical-proof detection interval. |
| P10 | `LegacyStateImporterV1` explicitly refuses materialized V1 RepoMap and generic unconsumed legacy objects; no source-bundle replay exists. | Preserve refusal. Replay is conditional on real root plus exact original bundles; do not build an importer-only graph IR or guess a no-legacy waiver. |
| P11 | Public V1 SDK/wire/dispatcher/store mutation remains reachable. `RepoMapActivateGenerationRequestV2.request_v1` is a V1 DTO, and `RepoMapGenerationStore::activate_generation_v2` calls public `activate_generation`. | Extract one private commit primitive, then remove all V1 success surfaces in one breaking cutover. No V2-to-V1 adapter and no duplicate catalog transaction. |
| P12A/Q | `handoff_validation.py` is already the single Git/archive/chain leaf used by CLI and aggregate; aggregate schema/writer/checker have product and infrastructure handoff fields. | Do not reimplement P12A from the old ticket prose. Validate the leaf/aggregate against independent negatives and real artifacts, then issue clean-source proofs. Missing historical handoffs and final-source receipts are not repaired by synthetic records. |

Status of every row above: source observation only (`NOT_RUN` qualification).
No assertion about Semantica's present producer wrappers, deployed state, or
historical proof validity is made from this Quanta-only inspection.

### Earlier SSOT/reachability audit addendum (2026-09-24; static, dirty source)

Inspected HEAD `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85`. This is not
an exact result source: the checkout has in-flight P09 edits in
`core/request_budget.rs`, `ipc/{server,counters}.rs`, and
`search-plane/query_embedder.rs`, plus unrelated dirty retrieval/SDK files.
Those P09 edits are a candidate, not a landed/qualified owner contract. Re-run
this reachability inventory against the assigned writer's frozen source before
cutover. No test, release proof, target-state inventory, or Semantica pair
validation was run for this addendum.

| Severity / seam | Current reachable fact | Structural correction / duplicate fence |
| --- | --- | --- |
| P0 P06 active-head authority | `ActivationCatalog::ActiveRoots` stores only `SearchCorpusGenerationV1`; activation/rollback requests compare generation identities, so a B→A→B serving event can repeat the same CAS value. | Add **one** catalog-owned head record and canonical contract token. Do not add an SDK epoch, second active map, or a query-local token allocator. The root-incarnation admission/fencing decision is an R0 prerequisite for a restore guarantee. |
| P1 P09 provider coverage at that inspection | The dirty query boundary candidate emitted ticket-linked `ProviderStarted/Returned` into the existing IPC ring via `RequestBudgetV1`; ingest's `semantic_derive.rs::embed_window` still lacked budget propagation **at that inspection**. See the current-code delta below for the subsequent ingest patch. | Use the **single** budget-to-IPC bridge: real ledger ticket for query, per-request window ordinal for ingest. The ordinal is diagnostics only, not a usage ledger or new request ID. Do not change durable-operation cancellation semantics as an incidental observability edit. |
| P1 P09 diagnostic disclosure | `IpcServerCounters::recent_request_events_v1` is an in-process read, not an operator wire path. `ControlCapabilityV1::Observe` permits every admitted peer, while event IDs/ticket linkage disclose operational history. | Expose one bounded snapshot through the existing control dispatcher using `Admin`/operator authorization, not the current `Observe` bucket. Include one process-instance ID shared by all three planes, plane, monotonically ordered window/loss metadata, and a fixed maximum response. No new bus or per-request metric labels. |
| P0 P11 public mutation reachability | V1 success is present in `RepoMapBundleIngestPort`, `RepoMapGenerationActivatePort`, the store's public `ingest_bundle`/`activate_generation`, SDK `RepoMapNamespace::publish/activate` and namespace trait, wire variants and dispatchers. V2 activation still carries `request_v1` and calls public V1 activation. | Remove **all** public success entrypoints in one serial cutover, including core ports/SDK namespace and fixtures. Extract one store-private commit implementation; importer uses the public V2 API backed by it. A wire-only removal leaves direct-store/port mutation reachable. |
| P10 conditional authority | `LegacyStateImporterV1` refuses materialized V1 RepoMap before staging publication; current source has no graph-bundle replay. | Preserve refusal until real retained V1 roots and exact original `RepoMapSourceBundle` inventory exist. A materialized snapshot cannot be promoted to producer graph IR or compiled candidate. |

Audit verdict: these are source-backed residuals, **not** implementation or
qualification completion. R0 target ownership and the P06 restore/fencing
choice remain unresolved; P10 replay applicability cannot be inferred from the
empty default local cache root.

### Earlier current-code delta and duplication gate (2026-09-24; static only)

Rechecked `98d2a7bb431d357d66ffdbf6a7c23a6e23780e85` with an extensive
dirty shared tree. This section supersedes the **P09 ingest-not-instrumented**
sentence in the addendum above; it does not supersede any release gate. No
test or proof command was run for this review. Other writers' changes in
catalog, contract, RepoMap, proof tooling, retrieval and SDK files have not
been integrated or accepted merely because they are present in the tree.

| Boundary | Current source / decision | Required next edit and no-duplicate check |
| --- | --- | --- |
| P06 catalog | `activation_catalog.rs::ActiveRoots` is still a map to `SearchCorpusGenerationV1`; activation and rollback persist `PersistedSearchCorpusGenerationRootV1` after comparing content identities. `RuntimeReadiness` also caches physical proof by generation inventory. | One catalog head event token must govern CAS, resolve, active request, read-view evidence, and readiness cache invalidation. Do not solve ABA only in SDK or add a second runtime map. Before implementation, decide whether the deployment guarantees controlled restore admission or has an external fencing registry; a byte-identical out-of-band clone is not distinguishable by local state. |
| P09 provider | Dirty `semantic_derive.rs::embed_window` now emits `IngestWindowStarted/Returned` around the existing `embed_batch`, with the budget passed through the existing ingest port/materializer. Query emits real ledger-ticket stages. The ingest ordinal is diagnostic correlation, not a usage/settlement authority; a provider panic can have a terminal panic without a return marker. | Review and integrate this **one** budget-to-IPC candidate rather than reimplementing ingest instrumentation. Add a real-UDS ingest correlation/negative owner oracle to the registered P09 selector. The provider-call boundary may retain the current entry-only cancellation semantics; observability work must not introduce mid-durable-operation cancellation. |
| P09 operator/readiness | A dirty IPC candidate now reads the same ring as a bounded insertion-sequenced window with loss and omission metadata and refuses an unbound process instance. Dirty searchd composition injects one OS-entropy instance into all three plane counters; a focused test of the production-shared constructor passed after concurrent P11 caller edits, but no release/process receipt exists. Neither ring read is an operator wire path. `ControlAccessV1::Admin` currently accepts daemon-owner UID or root; it is **not automatically a distinct operator identity** if producer/clients share that UID. `required_backend: true` and the identity-only `ProvenActive` cache still have no bounded freshness interval. | Integrate and test the single ring/instance candidate, then resolve the principal threat model before exposing IDs: use the existing Admin gate only when service-UID isolation is a declared deployment invariant; otherwise add an explicitly authorized diagnostic principal/capability on the same control surface. Bound the **serialized** snapshot to the IPC frame limit. Define a measurable backend/physical-proof detection horizon and invalidate on the catalog **head event**, not only generation-content change. |
| P10/P11 store | `RepoMapActivateGenerationRequestV2.request_v1` and `store.rs::activate_generation_v2 -> activate_generation(V1)` remain; public core ports, direct store methods, wire and SDK V1 success remain. The offline importer still refuses materialized V1 RepoMap. | One P11 store writer first extracts one private commit path and makes the existing **public V2** publish/activate APIs use it without calling V1 methods. P10, in another crate, replays through those V2 APIs, not the private Rust method. Remove all V1 success across store/ports/wire/SDK/producer in one later compile-atomic breaking cutover. No importer-only write port, V1 adapter, or independently allocated sequence. |
| P12 authority | Operational proof registry and independent expected DAG are different roles; the committed handoff leaf is already shared by CLI and aggregate. | Reconcile real handoffs against the leaf; do not refactor the checker to derive its expected graph from the registry or manufacture historical receipts. Final-source proof remains separate from historical chain, deploy, activation and rollback. |

Before any writer starts, record the exact changed-file owner and diff base for
the shared seams below. The current dirty checkout is **not** a handoff source
or a reason to regenerate wire/public-API baselines. If another writer changes
one of those seams, re-review its semantic contract before a patch is applied.

### Writer and SSOT collision fence

The following are **serial integration seams**, even if independent owners
develop elsewhere. Before editing, re-freeze the named files and assign one
writer; a dirty file is not an invitation to overwrite its current owner.

| Shared seam | Order / single authority | Forbidden overlap |
| --- | --- | --- |
| `contract/src/ipc/{control,split,ingest}.rs`, `sdk/src/{client,binding,repomap}.rs` | P06 defines the catalog-owned active-head token and its query/control binding; P11 then removes V1 RepoMap mutation wire and SDK success surfaces against that result. Update the one wire inventory/public API baseline only after both semantic cuts integrate. | Separate P06/P11 compatibility decoders, SDK-minted epochs, or two `ResolveActiveGeneration` routes. The proposed Sep-24 SDK DSL is a later ergonomic projection over these contracts, not a concurrent protocol owner. |
| `search-plane/src/readiness/{activation_catalog,search_corpus_generation}.rs`, `searchd/src/app/{state_format,state_migration,runtime}.rs` | P06 owns head event and root-incarnation format. P10 consumes that exact format through the existing offline staging/import path. | Migration-local head schema, inferred incarnation from an empty directory, or a second runtime catalog. |
| `core/src/request_budget.rs`, `ipc/src/{server,counters}.rs`, `searchd/src/app/ipc_dispatcher.rs`, `search-plane/src/{query_embedder,semantic_derive}.rs` and ingest materializer/ports | P09 owns the one transport ring and budget diagnostic bridge. Query and ingest provider entrypoints may emit stage markers; only `ProviderBudgetLedger` owns reservation, settlement and usage where it applies. | A route-local ring, new request ID, duplicate cost ledger, provider-stage claims for an uninstrumented ingest path, or labels containing request IDs. |
| `repomap/src/{materializer,store}.rs`, `contract/src/repomap/terminal_receipt_v2.rs` | P11 reviews/integrates the **existing dirty** private `commit_bundle`/`commit_activation` and flat V2 activation DTO candidate; P10 replay, across the crate boundary, calls **only public V2 APIs** and the existing compiler/catalog sequence authority. Verify that the removed nested V1 DTO stays absent after integration. The dirty candidate is not qualified or deployable. | Re-extracting a second commit path, restoring V1 public methods, importer access to an internal method, snapshot-to-source reconstruction, or a second replay sequence allocator. |
| `tools/ci/lint/handoff_validation.py`, proof aggregate schema/writer/checker | P12A keeps one policy leaf and one no-follow file-custody reader for CLI, aggregate and proof-checker callers. Registry declarations and independent expected DAG remain separate by design. | Duplicate Git/archive parser or file traversal, checker-import cycle, registry-derived expected graph, or digest-only acceptance of a symlinked archive. |

The Sep-24 single-IR note concerns internal IR and benchmark artifacts; it
does not authorize removing persisted state/wire format stamps or changing
RepoMap source-bundle versus compiled-candidate authority. The Sep-24 SDK DSL
RFC does not supersede P06/P11 safety contracts. Its API redesign must wait
for those contracts or merge into their sole SDK writer explicitly.

## 0. Decisions and authority boundaries

1. Breaking cutover is allowed. There is one successful RepoMap mutation wire
   contract: V2 publish and V2 activate. Do not preserve V1 success, dual-write,
   or an implicit fallback. An old V1 wire request may be rejected at decode;
   a typed incompatibility response requires a separately designed versioned
   envelope/handshake, not a promise made by deleting a decoder variant.
   P06 and P11 must share this one IPC compatibility decision and wire
   inventory; do not implement separate tombstones or negotiation layers.
2. Producer `RepoMapSourceBundle` is the source graph contract. Its versioned
   canonical-CBOR digest identifies exact producer input. The P02A
   `CompiledRepoMapCandidateV1` is a validated, sorted, derived graph/projection
   IR; its compiled-graph commitment has a different domain and purpose. A
   materialized V1 snapshot is neither of those inputs. Never reconstruct a
   purported source graph from snapshot rows or substitute compiled commitment
   for source-bundle digest.
3. The search-corpus activation catalog owns the serving active-head event.
   The sealed generation owns content identity. The SDK and read view consume
   catalog authority; they do not mint an independent epoch or commitment.
   A content commitment is derived by one versioned canonical encoder from
   the validated complete generation, not maintained as a second writable
   field. Physical artifact-open proof remains separate from metadata binding.
4. Direct immutable `Pinned` reads remain legal without an active-head token.
   Only a request that claims to read the current `Active` head must carry and
   prove the resolved active-head token. Lexical/semantic head identity does
   not claim a single instant for auxiliary history/runtime/structural reads.
5. `proof-authority.toml` is the operational proof registry. The checker's
   fixed expected proof graph/verdict set and lane-handoff policy are
   independent acceptance oracles against a weakened or forged registry;
   do not delete them in the name of SSOT. Share validation *logic*, not an
   untrusted declaration and its sole expected answer.

### Static SSOT / duplicate-implementation audit at the planning baseline

| Concern | Existing owner to extend | Do not add or duplicate |
| --- | --- | --- |
| Corpus content vs serving event | `SearchCorpusGenerationV1` validates the sealed lexical+semantic identity; `ActivationCatalog` persists and serves the active root. | A second mutable content commitment, SDK-owned activation epoch, per-track serving head, or a replacement generation type. Add the event token *around* the existing generation. |
| Read evidence | `QueryReadViewV2` and `ReadIdentityV2` already acquire and describe declared domains. `ResolveActiveGeneration` is the query-plane resolution route. | A second read-view acquisition, parallel active-resolution endpoint, or a purported globally atomic history/runtime/structural snapshot. Extend the existing resolution/identity path only for an `Active` claim. |
| Request correlation/usage | IPC `DispatchContextV1` and `RequestBudgetV1::correlation` already carry admitted IDs; IPC counters, query execution trace and provider audit already have separate purposes. | A new request-ID allocator, replaying provider billing into a second ledger, or treating metric samples as terminal request events. One bounded diagnostic event sink may reference the existing provider audit by correlation. |
| RepoMap source and derived data | `RepoMapSourceBundle` plus its versioned digest is producer input; the existing compiler produces `CompiledRepoMapCandidateV1`; the V2 store/journal owns publish, activate and sequence. | Snapshot-to-source reconstruction, importer-only compiler, independently allocated replay sequence, or source digest inferred from compiled commitment. |
| Proof acceptance | `proof-authority.toml` declares operational nodes; `check-proof-authority.py` has fixed expected DAG/verdicts; `check-lane-handoff.py` has the lane policy and Git/archive checks. | Registry-derived expected answers, a second handoff validator in the aggregate writer, or historical handoffs masquerading as final-source proof. |

R1 SDK checkpoint already present in current source:
`crates/quanta-index-sdk/src/binding.rs` compares full typed identities,
including `semantic_content`, for activation/rollback ACKs. Retain its
swapped-root negative test. The remaining SDK work is the new catalog-owned
head-token comparison; do not restore `same_identity` or implement a second
field-by-field matcher.

These are source-traced design boundaries, not executed correctness proof. Before
editing each owner, compare its current exported types and consumers again;
the baseline HEAD and dirty state can change while other writers work.

## 1. Admission / freeze — required before state mutation or proof issuance

- Freeze Quanta and Semantica HEAD, dirty paths, `Cargo.lock` digests, and path
  ownership immediately before each writer. Do not overwrite unrelated work
  in either shared main checkout. The paired source and dependency roots must
  be clean for exact-pair proof issuance.
- Read-only inventory the actual target state roots: absent/current/legacy
  format, **all** legacy RepoMap snapshot keys and activation records, original
  producer bundle availability for every retained snapshot, active identities,
  terminal sequence high-water, and required rollback boundary. Record
  file/object digests, not just counts or a human summary.
- Decide P10 replay applicability from that inventory: `REPLAY_REQUIRED` only when a target has
  materialized V1 RepoMap and trustworthy original graph bundles for every
  retained snapshot; `BLOCKED` when it has such data without a required
  bundle; `NOT_APPLICABLE` for **replay on a named no-legacy target** only with
  an explicit scope-bound release waiver. This does not waive P10's mandatory
  offline migration, restore/refusal tests or registered owner/release proof.
  If inactive snapshots may be intentionally
  discarded, that data-loss boundary needs a separate explicit operator
  decision and receipt, not an importer default. The existing typed refusal
  always remains.
- Freeze the release guarantee for P09 backend health: either an on-demand
  check or a stated maximum detection interval. Do not claim immediate
  detection from a cached boot proof.
- Freeze the supported restore boundary. A local root cannot distinguish an
  out-of-band byte-for-byte copy from its original. Require the controlled
  restore entrypoint and deny serving an unadmitted copy, or introduce an
  external incarnation/fencing registry; a local epoch alone is insufficient.

Stop: missing source bundle, ambiguous state-root ownership, dirty paired
checkout, or absent deployment/activation/rollback authority is a typed
`BLOCKED` node, never a guessed input or successful aggregate.
Independent P09 code development may continue on assigned dirty files, but it
cannot establish a target restore/replay claim or issue exact-source proof.

## 2. R1 — P06 active-head identity and restore fencing (serial first)

Root cause: a generation/content identity is currently also used as the
activation event and as the CAS expectation. Rollback and reactivation of the
same generation cause ABA; restoring an older backup can reuse an epoch.

| Owner files | Action |
| --- | --- |
| `crates/quanta-index-search-plane/src/readiness/search_corpus_generation.rs`, `readiness/activation_catalog.rs`, `search_corpus_lifecycle.rs` | Keep `SearchCorpusGenerationV1` as the content identity and retain its existing delegation to `SearchCorpusGenerationIdentityV1::validate_v1`; do not write another validator. Replace the generation-only active map/root with one validated `ActiveSearchCorpusHeadV2` containing that generation, the root incarnation and checked pair-local activation sequence. Activation and rollback advance the sequence under the existing pair mutation guard, persist before visibility, and refuse overflow/uncertain durability. Extend the existing catalog snapshot and lifecycle construction; do not add a second catalog. |
| `crates/quanta-index-searchd/src/app/state_format.rs`, `app/state_migration.rs`, `app/runtime.rs`, `crates/quanta-index-searchd-runtime/src/lib.rs`, `state_migration.rs` | Introduce one versioned durable root-incarnation authority under state-root custody and inject it into the existing lifecycle. Capture fresh-vs-existing root status immediately after lease acquisition and before lexical/semantic adapter constructors create directories; do not infer freshness later from an empty `activations/` directory. Heads reference the incarnation; the offline manifest's object digest can bind its record without becoming a second live head authority. Mint a new incarnation only for a proven fresh root, offline V1→V2 conversion, and controlled restore; rewrite heads in staging before manifest-last publication. Existing nonempty roots with no provable identity fail closed; no live dual-read. |
| `crates/quanta-index-contract/src/ipc/control.rs`, `ipc/split.rs`, query request/response DTO owners | Introduce one breaking typed active-head token (root incarnation, sequence, complete generation/content digest). Place the versioned canonical full-generation commitment encoder here; never use the pair lock-name digest as content evidence. Activation/rollback CAS expectation and ACK, the existing `ResolveActiveGeneration` route, and active query request share its canonical type/encoder. Retire incompatible V1 wire shapes; do not keep a generation-only active-resolution success route. A direct pinned selector does not require this token. |
| `crates/quanta-index-search-plane/src/query_dispatcher/{selection,dispatcher}.rs`, `query_dispatcher/read_view/view.rs`, `crates/quanta-index-core/src/domains/read_view/identity.rs` | Resolve one composite catalog head, compare the request token against the head, then compare acquired lexical/semantic artifacts with the sealed generation. Extend the existing `QueryReadViewV2`/`ReadIdentityV2` evidence for active claims rather than opening another view. Populate active-head evidence from the catalog-owned token; do not synthesize it from result rows or RepoMap's separate activation epoch. |
| `crates/quanta-index-sdk/src/{client,binding}.rs` and relevant SDK route modules | Keep the already-landed complete typed identity equality. Freeze the new catalog-owned token via the existing query-plane resolution route before the final active request and exact-bind the response to it. Preserve query-only and legal `rev:at.time` behavior. A successful first resolution is server authority, not independent proof against a server that forges both calls. |

Independent negatives / DoD: B resolve → A rollback → B reactivation
rejects the old token; old-backup restore rejects old token even if generation
and pair sequence repeat; stale activation/rollback CAS mutates zero bytes;
wrong content, missing incarnation, overflow, parent-fsync uncertainty, and
reopen mismatch all refuse; fresh direct-pinned query still works. Add contract
decoder, catalog crash/reopen, real query-only UDS, SDK same-variant and
semantic-content-root swap,
offline restore and old/new format refusal tests. Re-run the registered P06
owner rail, wire inventory, public API, fuzz smoke, and changed package checks
on one clean result HEAD. An out-of-band clone cannot serve under the declared
deployment admission/fencing policy. Old V1 roots are offline input only.

## 3. R2 — P09 request events and bounded readiness freshness (can develop beside R1)

Root cause: IPC creates `DispatchContextV1`, but query/ingest adapters discard
it; metric aggregates and tails are not a request-stage terminal record.
`required_backend=true` reports boot assembly, not necessarily current health.

| Owner files | Action |
| --- | --- |
| `crates/quanta-index-ipc/src/{server,counters}.rs` | Preserve the committed one bounded request-event tail (1024 entries), nonzero envelope ID, connection ID, dropped count and corrected overload encode/write classification. Keep transport-owned admission/terminal events and counter authority. Add/retain panic/abort terminal tests, and make explicit that ring loss prevents a complete per-request trace. Do not add another sink or ID allocator. |
| `crates/quanta-index-core/src/request_budget.rs`, `crates/quanta-index-searchd/src/app/ipc_dispatcher.rs`, `crates/quanta-index-search-plane/src/{query_embedder,semantic_derive}.rs`, `ingest_dispatcher/{dispatcher,search_corpus,semantic}.rs`, existing provider-audit owner | Keep generic backend start/return and closed route/typed-error projection in one adapter. Validate the in-flight per-request budget→IPC bridge, then trace the materializer/semantic source port signatures from ingest dispatch to each `embed_window`. Emit stage markers for actual calls only: query references the ledger's real ticket; ingest uses a bounded per-request window ordinal under the same transport identity, never a fake ticket. Thread diagnostic context without silently changing the ingest contract that checks cancellation at entry and finishes admitted durable work; any switch to `embed_batch_within` after durable intent needs a separately proved replay/uncertainty design. No second provider usage settlement, query/source bytes, credentials or high-cardinality metric labels. |
| `crates/quanta-index-contract/src/ipc/{split,control}.rs`, `crates/quanta-index-search-plane/src/control_dispatcher.rs`, `crates/quanta-index-searchd/src/app/{runtime,ipc_dispatcher}.rs`, SDK/searchctl observability owners | Expose a fixed-limit read of the **existing** IPC event tail under `ControlCapabilityV1::Admin` only if daemon-owner UID is exclusively an operator identity; otherwise add a stricter diagnostic principal/capability on this same control path. `Observe` is too broad for per-request IDs. Include dropped count, one boot/process-instance ID across planes, plane and bounded ordered window metadata so clients cannot infer a complete trace after loss or across restart. Bound serialized bytes to the IPC frame limit, not just the number of ring entries; reject unauthorized peers before reading the ring. |
| `crates/quanta-index-searchd/src/app/readiness.rs`, maintenance/scrub owners, process-readiness contract | Replace the unconditional backend component with an observed proof/freshness status. Define the detection horizon and invalidate cached active integrity on catalog **head-event** mutation (including same-generation reactivation), scrub findings and expired proof. Keep process readiness separate from generation status. |

DoD: `(process instance, plane, connection ID, nonzero envelope ID)` links
queue→backend→response/close across overload, deadline, disconnect and panic;
provider stages exist only for routes that actually made provider calls.
Ring overflow is visible and never treated as a
complete trace; an observer cannot read request IDs; no payload/secret leak;
required child or backend failure and
stale proof make `ready=false` within the declared horizon. P09 real-UDS owner
and release process rail must be executed separately. Before that owner rail,
update `tools/ci/test-authority.toml` so its P09 selector executes the
`provider-boundary-owner-v1` ticket/correlation negatives and new ingest
window-stage negatives, not just IPC/readiness tests. Existing
executed/contributed engine metrics are not reimplemented.

## 4. R3 — P10 offline state conversion, conditional on admission inventory

Root cause: the old RepoMap snapshot is a materialized view, not the graph IR
the current compiler and V2 terminal contract require. Copying it as current
authority would produce a false-success cutover.

| Owner files | Action |
| --- | --- |
| `crates/quanta-index-searchd-runtime/src/state_migration.rs`, `crates/quanta-index-searchd/src/app/state_migration.rs`, `app/state_format.rs`, `cli/command.rs`, owner integration tests | First resolve the latest dirty candidate that removes `migrate-state`: retain/restore **one** offline migration path in the existing engine if real retained legacy roots or the approved product contract require it; otherwise make an explicit breaking no-migration decision and revise all public/operator/proof owners together. On the retained path, preserve materialized-only refusal until exact bundles are admitted. If `REPLAY_REQUIRED`, accept a frozen original `RepoMapSourceBundle` for each retained legacy snapshot key, plus the old activation map, source fingerprint and original bytes. Canonically decode/re-encode and compare bytes before calculating the existing versioned source-bundle digest. Reject absent, duplicate, ambiguous and noncanonical input before publishing a destination. Mark legacy files consumed only after corresponding V2 replay is verified; generic unconsumed-object refusal still catches residue. Never add a second importer. |
| `crates/quanta-index-repomap/src/{materializer,store}.rs`, catalog sequence/replay owners | For `REPLAY_REQUIRED`, first review and integrate the **existing dirty** P11 V2/private-commit candidate. Fix its sole production primitive if the review finds a real defect; do not extract a second one or restore public V1 methods. The importer calls public V2 publish/activate methods, not a private cross-crate primitive. Do not introduce an importer-only graph compiler or a second sequence allocator. Compare source-bundle digest, compiled commitment and active identity against independently frozen evidence; define and check an explicit monotonic mapping for legacy high-water/replay floor instead of assuming old and new sequence numbers are equal. |
| state-format/backup/restore owners and operator runbook | Perform R1 root-incarnation conversion in the same offline staging flow, deep-open/scrub, write manifest last, atomically cut over and state the exact pre/post-first-mutation rollback policy. |

DoD: source remains read-only; interruption or disagreement never publishes a
serving destination; every retained snapshot and active mapping has an explicit
old→new inventory record, and complete source bundles reproduce the required
observable identity/sequence contract. An intentionally discarded inactive
snapshot is named in a separately approved data-loss receipt. If actual
targets have no legacy RepoMap, retain negative refusal tests and record the
scope waiver; do not pretend replay ran.

## 5. R4 — P11 one successful RepoMap mutation protocol (review current candidate; paired cutover after conditional R3)

Historical root cause: V2 was added while V1 SDK, wire, dispatcher and public
store mutation remained reachable. The current dirty candidate removes most
V1 success entrypoints and changes V2 to use one private store primitive;
those edits have not been accepted as an integrated or qualified result.

| Owner files | Action |
| --- | --- |
| `crates/quanta-index-repomap/src/store.rs` | Review the existing `commit_bundle` and `commit_activation` extraction for validation-before-commit, exact replay, object verification, catalog transaction and terminal sequence ownership. `ingest_bundle_v2`/`activate_generation_v2` must call only these validated private paths. Prove that a catalog-committed activation followed by projection-update failure/retry cannot return a replay ACK while the in-process `activated` projection remains stale; reconcile from the durable catalog row or fail serving closed. Do not repeat the extraction, add an importer-only commit or allow V2 to call a public V1 mutation. Keep the store candidate non-release until the whole paired cutover is proven. |
| `crates/quanta-index-core/src/domains/repomap/outbound.rs`, `crates/quanta-index-contract/src/repomap/terminal_receipt_v2.rs`, `ipc/{ingest,split}.rs`, `crates/quanta-index-search-plane/src/{ingest_dispatcher/dispatcher,control_dispatcher}.rs`, `crates/quanta-index-sdk/src/{repomap,client,binding}.rs` | Review the current V1 wire/port/SDK removals and flat V2 activation DTO rather than redoing them. Confirm the only remaining `request_v1` reference in this DTO owner is a negative decoder fixture; no V1 mutation request may remain V2 authority. Finish exhaustive response validation, wire inventory, public API baseline, fixtures and negative old-tag decode tests. Search exported methods and direct-store paths again after integration. Hard removal means old wire decode refusal unless a separately approved versioned-envelope handshake is built. |
| `crates/quanta-index-contract/src/ipc/ingest.rs` non-RepoMap decoder edits | The current shared diff also removes defaults for unrelated search-corpus, semantic and contributor fields. Assign that change to its actual contract owner with independent old/new wire tests, or exclude it from the P11 cutover; file overlap alone is not authorization to widen P11 semantics. |
| Semantica `quanta-runtime-retrieval-kernel/src/index_sdk_ingress/{repomap,facade}.rs`, `quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/repo_map_handoff_dispatch.rs`, producer handoff tests | Review the current dirty V2-only ingress/facade and `for_bundle` handoff candidate; search public exports for any remaining V1 mutation success before editing. Preserve the existing prepared-member → publish receipt → activate receipt chain and bind it to exact source bundle, candidate, and activation axes. Convert only reachable legacy fixtures/wrappers still found after the paired contract freezes; do not create parallel producer authority. |

DoD: every V1 wire/API/direct-store mutation is absent or non-successful;
old/new producer-daemon pairings cannot mutate; exact V2 replay does not
recompile/reseal on publish or advance the catalog transaction/terminal
sequence on activate (read-only custody verification is allowed); one clean Quanta/Semantica source pair,
lock digest, release daemon binary and host are attested. Coordinate a stopped
producer/daemon cutover; do not infer deploy/activate/rollback from code proof.

## 6. R5 — P12A proof closure, then P12Q qualification

Historical root cause: aggregate validation consumed proof manifests but not
the product handoff graph. The current implementation adds that graph;
it still needs reconciliation and clean-source proof. The operational registry
and fixed checker oracles must not be collapsed into self-validation.

| Owner files | Action |
| --- | --- |
| `tools/ci/lint/{check-lane-handoff,handoff_validation}.py` | Preserve the one fixed lane policy, Git/archive checks and historical fork/join. Close the confirmed same-byte, in-repo symlink archive acceptance: make the leaf own a single repo-relative reader that walks each path component with no-follow directory descriptors, opens a regular final file, then hashes and parses the **same bytes**. Make the CLI single-handoff/product-directory adapters use that leaf read, not a second following JSON reader. Historical result SHA need not be current HEAD. Keep proof-checker injection; no leaf import back into the aggregate checker. |
| `tools/ci/{proof-aggregate.schema.json,write-proof-aggregate.py}`, `tools/ci/lint/check-proof-authority.py` | Reconcile the already-present `product_handoffs`, `product_chain_status` and `infrastructure_handoff` fields. Reuse the already-loaded handoff leaf's no-follow file-custody reader for proof-checker evidence, binary and dependency paths; do not clone the traversal implementation or introduce an import cycle. Avoid digest/read TOCTOU and keep path identity distinct from byte equality. Independently compare the aggregate to fixed expected dependencies/verdicts. Four final-source verdicts derive from final-current receipts only; historical chain gates `production_ready` separately. |
| `tools/ci/write-proof-manifest.py`, `tools/ci/tests/test_write_proof_manifest.py` | Review the now-present dirty producer repair: `_repo_bytes` uses the handoff no-follow reader; dependency alias/archive hash and parse bind to one byte sequence; fixed registry/schema and external terminal input read no-follow; content archive, immutable index/leaf and current alias use pinned parent descriptors. Retain same-byte input/evidence/alias/index/leaf, both parent-swap, broken-symlink status and postpublication rollback negatives. Do not add a second file-custody reader or treat the local 118-test pass as clean-source proof. |
| `tools/ci/tests/{test_check_lane_handoff,test_write_proof_aggregate,test_check_proof_authority}.py`, `Justfile`, test/proof authority registry | Retain the current dirty candidate's same-byte symlink, CLI, checker and aggregate-writer negatives; add missing parent-swap cases where supported. Retain missing/duplicate/reordered, wrong fork/join, tampered archive, wrong pair/binary/host and historical-versus-final negatives. Add a real positive chain only from authentic artifacts. Re-run the executable P12A owner rail on a clean result SHA. Do not issue P12 final proof from P12A. |

Historical P00/P01/P02A/P02B/P02I handoffs do not exist locally. Recover only
authentic archived source-bound results if available; do not fabricate
backdated commits, counts or manifests. If history cannot be reconstructed,
record a new explicitly named clean-source integration baseline and revise
the acceptance contract under review before P12Q; never silently weaken the
checker to accept the gap. P11 and P12A handoffs are issued only at their real
future result HEADs.

P12Q DoD: same clean source pair and same attested release binary satisfy all
mandatory owner/release nodes; `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED` and
`ROLLBACK_PROVEN` are separate results; absent, stale, wrong-host, zero-run,
failed or unapproved input cannot become ready. Linux process, deployment,
activation and rollback are distinct receipts and approvals.

## 7. Historical conditional execution order and stop rules

The R3 importer branch in this section predates the current no-importer
decision. Execute the **Current execution overlay** above instead. Retain this
section only to explain former dependency reasoning and registered commands;
it does not authorize an importer or waive a target data-custody decision.

1. R0 inventory/authority freeze; no legacy replay without data and source IR.
2. R1 P06 root/head/contract breaking change; integrate and verify before
   any state migration or paired producer edit.
3. R2 P09 can be coded independently. R5 validator *code* can be developed
   independently, but P12A proof issuance still depends on the P11 exact pair.
   Share no product files or proof artifacts between writers.
4. R3 P10 inventory/input validation can proceed after R1. Before writing
   replay, resolve the current dirty removal of `migrate-state` against the
   real target inventory and approved product contract. Restore/extend only
   the existing offline engine when migration remains required; if support is
   explicitly removed, update operator, CLI, ticket, test selector and proof
   authorities in one breaking decision, not by deleting tests alone. If replay is
   required, first review/integrate the **already changed** R4 store V2/private
   commit candidate and prove its public V2 behavior without restoring V1.
   The importer then uses those V2 methods. **That store candidate and R3 are
   not release checkpoints.** Do not add a temporary importer adapter and
   later remove it.
   If no real legacy target exists, issue the explicit scope waiver and retain
   refusal.
   The registered P10 owner proof depends on the P09 owner proof; the P10
   release proof depends on the P09 release proof. This is proof ordering,
   not a requirement to write the importer before its production primitive.
5. R4 finishes/reviews the in-flight V1-success removal across store, core
   ports, wire, dispatchers and SDK, verifies the flat V2 activation DTO,
   then coordinates the producer as one compile-atomic paired cutover after
   R1 and conditional P10. The current dirty deletion is not a separate P11
   result or permission to mix unrelated ingest decoder changes into its
   proof. Its registered cross-repo proof follows the P10 release node. Use
   a clean paired checkout and named authority; do not edit another writer's
   dirty Semantica main.
6. R5 P12A handoff/proof, then P12Q on one final clean source pair and binary.

At each result checkpoint: inspect HEAD/dirty state → run cheapest owner
checks → run registered owner proof on clean result source → validate immutable
handoff → non-force publish and verify remote SHA. A later source change
invalidates final-source receipts. Compilation is not a test; owner tests are
not release proof; release proof is not deployment or activation. No material
state-root mutation, external provider egress, deployment, activation or
rollback drill is authorized by this planning document.

Registered proof commands at the corresponding clean-source checkpoints:

| Gate | Exact registered command | Current authority |
| --- | --- | --- |
| P06 owner → release | `just proof-p06-sdk-binding-owner` → `just rust-profile test-daemon` | owner executable; release staged |
| P09 owner → release | `just proof-p09-control-readiness-owner` → `just rust-profile test-daemon` | owner executable; release staged |
| P10 owner → release | `just proof-p10-state-migration-owner` → `just rust-profile test-daemon-all` | owner recipe exists but the current dirty test candidate omits migration; contract/selector must be reconciled before owner acceptance; release staged |
| P11 pair → deploy → activate → rollback | `just rust-verify-hellgate-cross-repo` → `just proof-p11-deployment` → `just proof-p11-activation` → `just proof-p11-rollback` | all staged |
| P12A → P12Q | `just proof-p12a-proof-infrastructure` → `just proof-authority-final-qualification` | P12A executable but dependency-blocked; P12Q staged |

The repeated `rust-profile` commands have different registered targets and
manifest identities; the command alone is not a receipt. A staged node must
first land its missing target and pass registry/checker admission. The
registered proof DAG, required host, exact source pair, binary binding, raw
result, artifact and digest govern acceptance; these commands have not been
run as part of this planning review.
