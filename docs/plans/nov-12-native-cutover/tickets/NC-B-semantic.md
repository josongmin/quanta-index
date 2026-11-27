# NC-B · Native exact·ANN과 파일 신원

## 소유 범위

`crates/quanta-index-semantic/src/native/embedding/ann_query.rs`, `ann_materialization.rs`, `embedding.rs`의 exact/ANN·파일 후보 hunk와 필요한 같은 owner의 private child·전용 테스트를 맡는다. 구현된 graph/file/recipe/verifier와 기존 회귀를 재사용한다. 추가 수정은 실제 결함이 확인된 인수 hunk에 한정한다. runtime I/O 등록·Cargo/lock·P07 제공자는 각 기존 writer와 F가 연결한다. [공용 계약과 인수](../README.md#레인별-착수와-인계)

## 구현과 TDD

1. `route_regions`, `search`, materialization·seal, owner vector와 source contribution을 읽고 기존 exact·pin·게시 구현을 재사용한다.
2. 독립 전수 oracle에서 빈/희소/상관 필터, 다중 region·centroid 상위 8개 밖 정답, 동일 파일 다중 벡터·동점·source-free projection을 먼저 확인한다. source/projection owner를 파일 owner로 임의 치환하지 않는다.
3. 제품은 Native 관리 LanceDB 리전과 원본 재채점을 사용한다. 기존 HNSW는 format 4 뷰와 회귀 검증에 남긴다. 선택과 재채점의 예산 및 exact/approximate·제한 도달을 결과에 보존하고 최신 소스를 원 oracle로 검증한다.
4. 완료된 Query 공급이 소비하는 projection→source-file 인증·파일별 최고 점수·동점·coverage 계약을 유지한다. dense-only 파일을 lexical 후보 교집합으로 축소하지 않는다.
5. delete/DELTA·이전 pin·병합·재오픈·실패/취소에서 기존 Catalog 게시·보존을 사용한다. 새 LanceDB 리전은 format 5로 게시하며, format 4 작업공간의 전환은 원본 재구축을 사용한다.

## 완료 조건

[DOD](../DOD.md#native-lancedb-ann-검증)의 독립 exact·Recall·필터 구간·예산·재오픈을 제품 LanceDB 경로에서 확인한다. 아래 결과는 각 실행 당시의 백엔드와 소스 범위에 해당한다. 관련 소스·recipe·생산 artifact와 실행 결과를 결속하고 C/F가 같은 계약을 소비한다.

## 상태

- state — Component implementation and source-scoped receipts are indexed below. B129 compaction and B134 Kernel carry have owning bodies. B143 records a repaired-cohort ANN4K pass and a product watchdog failure. Product acceptance, installed recovery and retirement remain in [Current remaining acceptance](#current-remaining-acceptance).
- historical revision — `ade5fadc1a084fbb81eba831861e996081d80867` was the implementation-intake checkpoint. Each later body retains its own source and inputs below; this hash does not bind current Main or reserve a runner.
- ownership_handoff — consumed F original 03:58:27 and F-HANDOFF-02 04:02:19 KST; recorded before SHA matched. B owns the granted exact/ANN/file hunk, private mounts and direct native seal call sites. B41/B42 completed serially through the existing lane under their recorded authorization. That historical handoff does not reserve a current runner or source freeze.
- changed — native_graph binary HNSW and canonical build/seal/query; projection/source-file authentication, max-per-file/dedup-before-window, retained file metadata/parts; synchronous with_ann reader; dedicated file/graph/ANN4K/lifecycle oracles. Existing unrelated dirty hunk preserved.
- source_ready — The Native graph/query/file/cache, pure recipe and Legacy outer verifier are mounted in current source and have the focused execution evidence recorded below. B45/B46's hashes and compile-pending statements describe their historical handoff, not the present code. No missing module mount or declaration-only implementation was found in B117's reviewed scope. Whole-product acceptance remains separate.
- consumed — existing SelectedOwner/member and Catalog delta/retention; C-B-FILE-02; F-HANDOFF-02; SC-F-B-DIAGNOSTIC-01; SC-F-B-EXPORT-08; C-HYBRID-MOUNT-17 and final C-B-REBIND-22 SHA consumption confirmed.
- residual — B41/B42 cover selected-owner eligibility, exact/file, lifecycle and Query scopes at their recorded cohorts. The latest product failure, post-repair 4K rerun and remaining owner boundaries are listed under [Current remaining acceptance](#current-remaining-acceptance).

## shared_contracts

<a id="b-source-01--existing-authority-and-proposed-file-reader-seam"></a>
### B-SOURCE-01 · implemented file reader authority

Observed path: `NativeSemanticReader::query` → `ann_query::search` or `search_exact` → `SelectedOwner::load_selected/current_member` → `rank_member` → `finish_ranked` → runtime `native_semantic_result::convert`. The implemented Query consumer preserves the vector API and file-candidate mode in the same reader/ticket; projection owner IDs cannot be relabeled after vector top-K.

- Inputs: selected `QueryContext` ticket/view, instance/format/recipe, source owners root and projection owners binding from that same view, query vector, file filter, top-K file window, owner/region/graph/rescore limits, shared `WorkBudget` request/cancellation/deadline.
- SourceLocal file identity: `member.owner` only after selected source root lookup equals `member.input.source` and the actual `SourceRecord`/registered children authenticate.
- Projection file identity: contribution `source_owner` and `source` from selected producer root; selected source map lookup must equal `source`. Decode that exact source record and validate its registered children. `SourceRecord` has no owner field; selected map membership is the identity authority. A source-free projection yields no file candidate; stale/foreign binding returns typed `CorruptObject`, never a guessed identity.
- Aggregation: authenticate and apply source-file predicate, then max cosine per canonical source `OwnerKey`, deterministic tie by source owner then projection owner/vector ID, then file top-K. Different source producer IDs never collapse by path. Dense-only files remain eligible; no lexical eligibility intersection for Hybrid's dense pool.
- Result: preserve coverage, stale count and `NativeSemanticTraversal`; region or visit/rescore caps never certify exhaustive. Retained candidate/ID backing keeps its memory lease through consumer emission. Failure/cancellation retains the original typed cause and drops local custody; session release remains C/kernel authority.

The initial vector-only/Lance gap and five before-modification hashes are
historical, retained in the [recovery entry](../../../ARCHIVE-INDEX.md#2026-11-16-query-supplier-ticket-retirement).
Current Native product materialization and query use managed LanceDB regions.
Source-file authentication and file aggregation use the existing selected-view
contract. See the [implemented decision](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#decision).
B119 and the source-scoped proof below retain their own limits; whole-product,
filtered/churn, installed and comparative acceptance are not closed here.

## Current remaining acceptance

The implementation, compiler retries and source handoffs B01–B111 are retired as
work instructions. Implemented [ANN mutation/admission and pure-recipe contracts](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#semantic-mutation-and-admission)
and the [existing-only physical verifier](../../../adr/NOV-07-001-native-index-execution-and-custody.md#existing-only-semantic-recovery-verification)
are reused. Original source/commands/failures remain in the
[recovery entry](../../../ARCHIVE-INDEX.md#2026-11-16-semantic-ticket-history-compaction).
This ticket remains active for its original unclosed acceptance:

| Boundary | Remaining proof / owner |
| --- | --- |
| B-PRODUCT-01 owning ANN4K | The recorded LanceDB body passed 1/1, exit 0, with 640/640 file and 10/10 raw recall in each of six nonempty strata. Its cohort is recorded in the [B product ledger](../../nov-21-bugbash-source-advance-4k/04-product-acceptance.md#기존-실행기-결과). Acceptance needs a pass compatible with the Product4K source and resolver inputs. |
| B43 whole SDK product | The latest Product4K body published all 4,096 sources and failed in owner materialization at the 1,200s watchdog. Source and resolver drift were recorded. Catalog recovery regressions passed 7/7. The late diagnostic stopped before its body, and the request-budget gap and remaining evidence are tracked in the [current B ledger](../../nov-21-bugbash-source-advance-4k/04-product-acceptance.md#현재-코드와-실행-판정). |
| B lifecycle and physical reclaim | B134 passed the real Kernel/Catalog/FileObjectStore multi-region carry, prior-view pin and reopen regressions 3/3 after the supplied SourcePathOwners APIs and two compiler fixes. B130 passed the existing Runtime last-pin body for held files, release and physical reclaim through FileObjectStore/Catalog/Kernel on its recorded inputs. The fixed 4K product checks selected historical views with retention disabled. Operating reclaim timing remains in F. |
| Query supply and product acceptance | Reuse the existing C Query/Hybrid/History supplier and public wiring. F owns bound-peer/caller acceptance, independent Hybrid holdout/latency, and paired product performance on the selected release inputs. |
| Genuine Legacy/Source and installed recovery | The existing-only verifier and Runtime callback are mounted, with focused physical/CBOR bodies. [D](NC-D-boot.md) and [F](NC-F-integration.md) own the joined Source/ACK, production Linux custody and signed installed rollback/restore-forward acceptance. |
| Engine/consumer retirement | [F](NC-F-integration.md) owns the coherent Runtime/Harness/Cargo/lock/vendor/test-authority change and actual caller and release-closure acceptance. The retired outside-only demount proposals remain historical. |

The earlier ANN4K pass and product failure share the captured inputs documented
in [B143](#b143--4k-product-watchdog-attribution). Keep the original quiet-host
admission and independent oracles for the retry.
To close the combined 4K acceptance, check each terminal for the exact expected
test body, one pass, exit 0 and empty source/resolver drift. Compare the two
captured `source_before` digests across the common source roots, including
added or removed files, and the captured resolver inputs. Each selector checks
drift during its own execution, so a shared edit between selectors can
invalidate the combined result. Revalidate affected
selectors through the existing serial rail if their recorded inputs differ.

## dependency_requests

| ID | Existing disposition |
| --- | --- |
| B-F-OWN-01 / B-F-GRAPH-01 | Consumed. Graph build/seal/query and directory mutation are current source, not a pending Lance declaration/mount task. Existing hunk ownership remains; no historical SHA creates new write authority. |
| B-C-FILE-01 / C-B-FILE-02 | Consumed by the [closed Query supplier](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#closed-query-supplier-scope). Preserve raw vector SearchHit parts and form authenticated files before the candidate window; no new C implementation queue. |
| B-F-IO-01 | Retirement remains F's coherent consumer/dependency cut. Native graph build/query does not require an AnnLanceSession; production closure and installed acceptance must be checked independently. |

## oracle plan and proof_requests

The following original independent oracles and product requirements are retained.
They are acceptance contracts, not duplicate submissions. Reuse the existing
canonical source/feature/target closure and serial admission for the affected
scope. Compiler/body0, selector resolution and quiet-host waiting are not PASS.

- **B-ORACLE-FILE-01:** two projection owners → one source file plus a dense-only second file, multi-vector duplicates, same cosine tie, source-free output and different producers with same path. Independent fixed expected file order. Stale/foreign source root/ref and invalid source record children refuse before candidate admission. Candidate file K=2 must retain the second file despite duplicate first-file vectors.
- **B-ORACLE-ANN-01:** deterministic sorted input/seed/recipe; fixed vectors with brute-force independent dot/norm oracle; empty, ≤1%, >1–10%, >10% eligibility and correlated filters. Compare exact, selected-region exact and native graph independently. Include ≥9 regions with the nearest row outside centroid top8; region routing loss remains visible.
- **B-ORACLE-LIFE-01:** reuse owning `native::embedding::tests::ann_product_tests::{native_ann_selected_region_survives_replacement_and_deletion,delta_page_authenticates_scanned_reference_without_second_map_read,empty_source_vectors_across_65_owners_do_not_seal_a_region,overlay_filling_owner_limit_needs_no_durable_owner_allocation}` plus consolidation/maintenance/source_reverse suites. Native artifact replay/reopen and prior root/graph reachability must execute after replacing Lance.
- **B-ORACLE-BUDGET-01:** visit/region/rescore upper boundaries, exact/approximate/capped result markers, corrupt seal/edge/recipe refusal, cancellation and memory release on partial failure.
- **B-PRODUCT-01:** original ANN4K 4096×64/Q64/K10 with independent exact file oracle, previous-view custody across delete/reopen and actual runtime build/search. The owning ANN4K, whole SDK product, active pin/reclaim, and release-closure receipts are tracked under [Current remaining acceptance](#current-remaining-acceptance).

## proof

B112–B119 retain a compact index of their source-scoped results below.
The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains every command, RED/GREEN result and input
identity. Current acceptance follows the backlog above.

## history · initial handoff and implementation sequence

Initial B01–B111 compilation/ownership/proposal/freeze sequences are recoverable
through the retired fragment table below. Their then-current queue, PID, outside
patch and source hashes do not describe today's execution. B112–B119 retain their original scopes in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).
Current execution follows the active acceptance and B132 controller.

### B-ANN-PRODUCT-ADMISSION-AND-MODULE-BASELINE-112

The identity export and packed cursor/count producer-consumer repairs are
supplied. Run `ca4b0f33-3c82-47c0-88ce-d30444a44f30` passed 34/34, and
`e8fc5313-90b9-410e-9812-32532dc1bfdd` passed 5/5 on their original inputs.
The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains commands, earlier compile repairs and hashes.

### B-SEMANTIC-RANK-WINDOW-MEMORY-AUDIT-113

The ranker releases a displaced hit before funding its replacement.
The two fixed-memory REDs failed in `db737ff3-f639-487a-90a5-6bd7e2a4659b`.
The corrected embedding scope passed 153/153 in
`a738cbe9-6785-4e50-9696-121a8086f73d`. The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains the
fixed caps, refund assertions and admission observations.

### B-RELATED-TEST-ORACLE-AUDIT-114

The independent result checkers cover score, order, source provenance,
posting tuples and full restart output. The original scopes passed 185/185
in `05dbd592-745b-4b35-ab02-d0e6d79a11b9`, Runtime 4/4 in
`46038bbc-c5fb-43e5-8cc5-565f5032fbfb`, and physical identity 2/2 in
`c01003ff-775c-4d2d-9f3a-aeb2f6cd2e79`. Their REDs, commands and proof
limits are in the [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction).

### B-HYBRID-ERROR-AND-RRF-ORACLE-AUDIT-115

The real UDS Hybrid error, retry, independent RRF and shared Work scope
passed 2/2 in `8858d7c1-0b1e-444b-a953-6e4f4b11692b`. These overlap the
earlier Runtime scope. The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains their original attribution.

### B-COMPLETION-EVIDENCE-AUDIT-116

This audit reused B112–B115 and reconciled the overlapping Hybrid cases.
Its historical admission wait produced no 4K terminal. The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction)
preserves that reconciliation. The current 4K gate follows
[the active backlog](#current-remaining-acceptance).

### B-SESSION-HARDENING-GRAPH-RECALL-ORACLE-117

Recall counts unique candidates against the independent exact top ten and
rejects duplicate rows. The duplicate counterexample ran in
`6d8e6e1f-9f78-4834-b348-812fc69e4af1`. The owning scope passed 35/35 in
`b5bb1555-3bfd-42aa-b0c3-71487f548362`. The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains the
controls, partial-traversal result and original source.

### B-SESSION-HARDENING-SIBLING-ORACLES-118

The selected-strata, overlay and carry checkers compare complete rows.
Semantic passed 5/5 in `be79b29c-7d48-48b9-8933-ebe1e681f033`.
Runtime checker integration passed 2/2 in
`01b07745-3c36-4980-9743-a3fd46e8c6fa`. The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains the
two behavioral REDs, 576-owner fixture and selected feature scope.

### B-SELECTED-COVERAGE-AND-BOUNDED-REOPEN-AUDIT-119

The 64-input bounded ANN process passed 2/2 in
`ca0bb075-5e35-440f-b4c4-5e20d491a0e9`. The real pinned-view Hybrid
scope passed 2/2 in `0759ee56-1c4e-48f9-921d-36ac5a4ddf5f`. The
configured owner cap was eight. The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains independent
coverage checks, the corrected cap expectation and source hashes.

### B-OLD-VIEW-AND-POST-OVERLAY-OBSERVATION-AUDIT-120

Source-scoped old-view/post-overlay oracle passed 4/4 in `213907e1-e19f-4fc0-8a60-1553686ef73f`; whole-product second restart remained unexecuted. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-ANN4K-AND-PRODUCT-EXECUTION-121

ANN4K `4953c891-02a8-4c07-bd75-dc2c0d84b7ff` passed 1/1 with independent 640/640 selected-file recall. Product `e10a9dc8-bccd-4425-a20e-f4ab096a1cf6` failed 1/1 at seed55 IPC Read timeout with 788ms left on the original 1,200s watchdog; initial 4,096 materializations and first reopen checks had completed. Readiness dominated observed wall, but the worker-stage cause was unproved. The product capture had source drift, so the two results were not combined into one current qualification. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-MATERIALIZATION-PHASE-RCA-122

The 128-completion prefix `e8bbcb60-5d68-4c13-b23b-2df6de901cdc` failed its 180s diagnostic watchdog after 39 exact receipts; provider processed 759 distinct source inputs. Those are different authorities. Its trace exposed Native Installed under SemanticBuild instead of SemanticSeal, so worker-stage attribution was invalid until the producer repair. No full-4K or Off/Record claim follows. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-NATIVE-SEAL-OBSERVATION-OWNER-REPAIR-123

The canonical append path emits successful Installed under SemanticSeal and emits no Installed on failure. Nextest `aa49b1b2-ab83-45ec-9770-970fa0e78b8a` passed 7/7 contract bodies; the unchanged reader's 16/16 check also passed. This corrected trace ownership without changing graph bytes, budgets or the product timeout verdict. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-ANN-ACCEPTANCE-ORACLE-AUDIT-124

Independent graph/selected-strata, Runtime view/overlay and trace oracles passed their recorded 651/651, 16/16 and 4/4 scopes, including `7ca33d2c-8aeb-4d88-a81f-003ec1b4ac20` and `80e7e6e6-2bd8-4c6a-9684-4e473675efe0`. These were not a combined 4K product run. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-ANN-PREFIX-OBSERVATION-RCA-125

The Native trace producer correction passed 11/11 in `07317ed2-c311-41b2-8554-5c3b78564ae3`. The rejected pre-fix trace was retained; a valid new prefix capture, not repaired historical bytes, was required for phase attribution. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-CATALOG-CONFIRM-REGRESSION-126

Catalog `3d9f0cdd-7a6c-4c23-94fe-6fb8f330a336` passed 6/6 for trace registration, atomic confirm/reserve and replay refusal on that source. It did not establish ANN4K/product completion. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-QUALITY-COHORT-AND-INDEPENDENT-TERMINALS-127

The quality runner was corrected to include resolved local build inputs and to run the independently selected product body even after ANN4K failure. Each terminal remained source-scoped; a runner repair was not a body pass. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-OLD-VIEW-ANN-SECOND-RESTART-128

The focused provider/quality/overlay/deadline oracle passed 5/5 in `74fe3328-4008-43ae-bc45-3429f333380d`. It checks that the original selected view survives restart without re-embedding persisted corpus rows; the actual second restart remained in the whole-4K product body. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-PRODUCT-COMPACTION-DEADLINE-129

The B127 controller recorded two admitted original bodies with empty `source_drift` and `resolution_drift`. ANN4K Nextest `5a0cc4cd-fd2d-43a9-98fa-c11b5746b7c5` passed1/1 (38.486s,327 skipped, exit0). Independent recall was640/640 overall and640/640 in each of six nonempty eligibility strata. Each nonempty raw sibling recalled10/10. The empty stratum returned no hits.

Product Nextest `f1aaae4c-80ae-4f8d-8480-ac09344993a7` failed its sole body (819.938s,56 skipped,exit100). SourceAdvance published all4096 inputs in18.449s. The observer spent737.234s in readiness, with4,046 exact public status RPCs and89.626s of direct RPC wall time. It observed1,909 completions before the refusal and a later bounded non-atomic status snapshot observed2,612 complete,1 blocked and1,483 pending. There were430.353s left in the product watchdog. The worker diagnostic for `row-953`, dispatch1, identifies `first_denial=deadline`, `failed_stage=Compaction` and the physical install checkpoint in `quanta-index-storage/src/object.rs:906`. Charged work was6,725 reads,1,478 writes,3,265,490 bytes and1,351 mutations. Its sampled Catalog renewals returned `Ok` in1,016ms and1,772ms. `ResourceLimited` was already terminal at settlement. This Off run does not attribute the elapsed compaction time to one internal subphase or establish ongoing host isolation. Logs are under `/tmp/qidx-ann-materialization-rca-t9bu2q50/quality-b127`.

`consolidation::prepare` applied each selected region deletion immediately to the previous intermediate COW root. The intervening region roots never reached a published directory. Nextest `35e86c83-535a-4542-989a-af800be8d4b4` executed both counterexamples against the old path and failed2/2 (exit100). The two-region carry exceeded its two-application budget. A malformed second seal left a first-region map rewrite in the attempt. The three captured test and implementation files were stable during that run.

The carry now authenticates all selected seals and members first, builds the final member map, and applies one sorted exact-CAS removal batch against the original region map. The one-region retirement path shares the root/count check. The regression asserts both original descriptors in the one region-map application, the exact final member map, old root reachability, and released temporary memory. Carry selection, file references, work and lifetime policies are preserved. The actual product run is still required to measure the deadline after this structural removal of intermediate writes.

Nextest `a4b309d1-cf2d-4a28-87aa-810cb87ca41b` passed15/15 (4.523s,313 skipped,exit0), including both original counterexamples, sparse member diff, region capacity, retirement and independent carry query. The three edited files kept their captured hashes. Scoped formatting, diff and hexagonal boundary checks passed. The post-fix controller queues the original 4K bodies behind the existing host admission and work limits. Its state and logs are under `/tmp/qidx-ann-materialization-rca-t9bu2q50/quality-b129`.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-semantic --lib -E 'test(ann_product_tests::maintenance::consolidation_) | test(carry_trace_) | test(retirement_) | test(obsolete_and_absent_regions_retire_but_pending_searchable_and_old_view_survive) | test(region_edits_reject_unchanged_map_root) | test(native_ann_small_runs_carry_and_retain_independent_nearest_neighbor)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

### B-ANN-REMAINING-ACCEPTANCE-130

Three independent reviews covered the compaction correction, product test execution, and cutover DoD ownership. The three edited compaction files match the B129 GREEN hashes. The carry applies original descriptor references in ordinal order; final directory changes require a changed root and consistent count, and a failed attempt cannot replace the caller directory. No new concrete P0–P2 was identified in those reviewed paths. The B129 post-repair quality controller remains the sole ANN4K/product runner. Its phase diagnosis stays conditional on the actual product terminal.

The existing last-pin test covers a separate requirement. It checks old closure inode identity while pinned during actual reclaim, physical absence after release and retirement, and preserved current ANN result. This uses an existing body through the Runtime, Kernel, Catalog and FileObjectStore. The focused refresh uses the canonical cooperative build lock and leaves the 4K quiet-host checks unchanged. Runtime test output is outside the checkout at `/tmp/qidx-ann-materialization-rca-t9bu2q50/last-pin-b130/PIN.log`.

Three invocations exited101 before test execution as shared History and recovery changes reached the target. The Search Plane History boot decoder now selects its existing `HistorySourceInputV1` explicitly before calling `validate_shape`, matching the adjacent retained-replay entry point. The Runtime History read-only fixture imports the existing Contract `Digest` type locally. The recovery invalid-config test passes `None` to the new optional History FD7 argument, consistent with its direct no-descriptor fixture. The latest invocation selects the original last-pin body and both affected Runtime tests and writes `PIN-HISTORY-RECOVERY.log` in the same external directory.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-searchd-runtime --lib -E 'test(=native_object_write_io::ann_enospc::real_managed_ann_last_pin_reclaims_old_searchable_closure) | test(=publication_sqlite_vfs::tests::read_only::legacy_history_source_requires_exact_original_ack_and_complete_inventory) | test(=native_recovery::tests::startup_config_failure_returns_bound_refusal)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

Nextest `08d168a6-bb34-4e79-bcf7-bf6a0723ad4b` passed3/3 (117.125s,660 skipped,exit0). The last-pin body passed in116.865s and verified both old-file reclaim and current ANN result preservation. The History body verified original receipt and inventory binding, and the recovery body verified the operation- and nonce-bound refusal before selection mutation. The six tracked fix/test/compaction inputs remained stable. These are scoped Runtime lib outcomes on dirty `ade5fadc1`, and the current ANN4K/product bodies still need their own terminals. Formal operating reclaim and installed Linux acceptance remain separate.

### B-STORAGE-COMPACTION-DEPENDENCY-131

The carry regression drives Semantic through `GraphMap` to verify the original region CAS, exact selected member map and bounded application count. The production `FileObjectStore` delegates multiple edits to its streamed original-base CAS path. Later shared Storage changes reached that path, so the existing tests were refreshed on dirty `ade5fadc1`.

Nextest `e03a3875-3a77-47ce-ab6d-175c84b8827f` ran5/5 PASS (0.925s,221 skipped,exit0). The cases cover nonempty base equivalence to a serial map and independent model, shared ancestor application, and late conflict or duplicate refusal before reservation with unchanged original roots and physical files. Five captured map implementation files remained stable. This verifies the Storage primitive. The managed ANN compaction combining it with two region removals and the product deadline still require the existing full4K body. The B129 controller remains the sole post-repair 4K runner and was waiting for admission at this checkpoint. B130's History and recovery inputs subsequently changed, so its terminal remains scoped to its recorded source.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-storage --lib -E 'test(sorted_existing_root) | test(late_existing_root_conflict_preserves_all_original_roots) | test(late_stream_conflict_or_duplicate_never_reserves_a_map_object)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

### B-4K-EXECUTION-RESTORED-AFTER-HOST-REBOOT-132

The host rebooted at 09:43:35 KST on 2026-11-17. The B129 controller and its `/tmp` state and logs were absent. The last pre-reboot observation had both post-repair bodies pending. No post-repair terminal could be recovered, and no original selector or runner process was present. Current source still mounts both fixed selectors and the batched compaction path.

One controller was restored outside the checkout at `/Users/songmin/Library/Caches/quanta-index/ann4k-resume-20261117`. It uses the same two `cargow` commands, original quiet-host preflight, cooperative lock, serial execution, and independent per-body terminal checks. Its state and logs survive ordinary reboot cleanup. The controller re-resolves the local Cargo closure and records source drift for each execution. The reboot does not create a body result. Original workload, recall thresholds, work limits and deadlines remain unchanged.

A mocked subprocess check verified the original two-rail order, independent preflights and terminal states, product dispatch after the ANN4K failure, and protection against reusing the same live state directory. No Rust body executed in that check. The sole restored controller is waiting for the unchanged host admission.

On 2026-11-18 the Codex Desktop update interrupted monitoring and the controller was absent. Its durable state still showed ANN4K waiting, with no ANN4K/Product4K body logs or matching live selector process. At12:18 KST the same unmodified controller was restored in its existing directory, detached from the app process. The prior state is preserved as `state-before-app-update-20261118-121805.json`; the new PID is32142. Both bodies still require actual original-admission execution and terminal results.

### B-CARRY-FINAL-CAS-REFUSAL-133

The existing multi-page Semantic carry fixture now puts the 65-row source at the earlier ordinal, so carry order differs from CAS order. With one write available, the final region CAS is reached after member staging and denied by the write limit. The test checks the actual typed refusal, both map applications, sorted original descriptor references, unmodified caller directory, prior fileset reachability and returned memory grants. A fresh work budget permits the same fixture to complete and validates the exact selected member map. No production inputs or 4K limits changed.

Nextest `f8383f93-01ac-4242-9847-8734f6ed874a` executed3/3 PASS (1.715s,325 skipped,exit0) on dirty `ade5fadc1`. The two compaction implementation files and the edited test file retained their captured hashes. The selected bodies also cover later seal validation failure before any map apply and a sparse member diff with exact base selection. Terminal output is at `/Users/songmin/Library/Caches/quanta-index/ann4k-resume-20261117/CARRY-ATOMIC.log`.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-semantic --lib -E 'test(=native::embedding::tests::ann_product_tests::maintenance::consolidation_multi_page_carry_installs_one_final_member_map) | test(=native::embedding::tests::ann_product_tests::maintenance::consolidation_atomic_late_failure_preserves_caller_directory) | test(=native::embedding::tests::ann_product_tests::maintenance::consolidation_reuses_largest_selected_root_with_exact_sparse_diff)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

### B-MANAGED-MULTI-REGION-CARRY-134

The real Kernel/Catalog/FileObjectStore fixture now publishes 256 sources and materializes four 64-owner seals. It independently expects two old regions `(1,128)` and `(2,64)` to become one region `(3,256)`. Fixed alpha top-1 score, source-root identity, old Partial/new Complete coverage, exact descriptor replacement, the held historical view, storage reopen and final pin release are checked through the actual query/session and diagnostic paths. The fixture stages mutations in canonical owner order, including IDs above255.

The first build stopped before any body because concurrent Core/contract-base APIs were absent from the compiled inputs. Those APIs were supplied in the shared checkout. The next build exposed an unused SQL row count in Kernel source staging and an ambiguous stale-owner counter type in Semantic path scope. The row count is now explicitly bound and the counter uses its existing `u32` result type. A subsequent run executed3 bodies, with2 PASS and the new fixture refusing unsorted mutations before materialization. Sorting the fixture input fixed that construction error.

Nextest `3f64855d-d9ec-4231-926c-d1df16f8eae8` then executed3/3 PASS (150.960s,6 skipped,exit0). The new multi-region body completed in87.431s. Nine captured implementation/test inputs retained their hashes through execution. The shared dirty checkout ended at `2f15171fe`; this is focused integration evidence. Terminal output is at `/Users/songmin/Library/Caches/quanta-index/ann4k-resume-20261117/KERNEL-CARRY-retry3.log`. Scoped rustfmt and diff checks passed. The original B132 runner continues to own ANN4K and full4K product acceptance under the original admission.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-kernel --test native_query_session_real -E 'test(=ann_regions::managed_ann_multi_region_carry_unlinks_both_old_descriptors_across_reopen) | test(=ann_regions::managed_ann_second_seal_preserves_prior_selected_view_across_reopen) | test(=ann_regions::managed_ann_region_reopens_after_owner_replacement_with_old_view_pinned)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

### B-STATIC-DEADLINE-AND-FACADE-AUDIT-135

Selected artifact cache/file provenance, original-root CAS and both product oracles were reviewed. Focused deadline/facade run `fd37295b-95a1-4b65-bb13-499d42ebd4e9` passed 3/3; full product remained separate. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-SEMANTIC-MODULE-CYCLES-136

Provider-independent `native::recipe` owns recipe identity, limits and output-policy digests; embedding/chunk callers share it. Focused run `1c7a6c91-2e66-4c71-ad42-53f2106d2c3b` passed 5/5. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-CARRY-IMMUTABLE-MEMBER-REUSE-137

Within one attempt, exact owner+immutable member ref reuses an already authenticated retained artifact while preserving per-source row accounting and selected coverage. Physical second-read RED `bd179f6d-5606-4b17-bf63-9a5eea2dbd73` became 1/1 PASS `43af013a-1f10-4791-9362-b9453f311dbe`; five adjacent carry cases passed `c17dc3b6-ee24-421d-9476-c2761c41cce1`. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-MODULE-CYCLE-INVENTORY-138

The cycle checker now keeps a sibling test-only fixture outside production graph and resolves visible path aliases. Its Python regression passed 40/40. Workspace `rust-module-cycles` still exited 1 with 16 unbaselined groups, none Semantic; this was a structural finding, not product proof. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-EXISTING-FILE-ERROR-OWNERSHIP-139

The shared ExistingFileReadFailure type moved out of the reader/metadata dependency cycle. Focused `67ad8fe3-d656-41b0-813c-23ae94417c77` passed 1/1; public/file product acceptance remained distinct. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-EXISTING-TREE-COMMITMENT-OWNERSHIP-140

The private tree commitment owns sealed file commitment/mismatch types, removing the existing-tree/generation cycle without changing the wire. Focused `028c711b-5a15-48e2-8901-56e8fc163e67` passed 10/10. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-MODULE-ALIAS-OWNERSHIP-141

The checker rejects two logical modules claiming one physical source, including inline siblings. Its regression passed 41/41; workspace module-cycle inventory still exited 1 on 16 unbaselined groups. This does not close Runtime/product gates. [원문 복구](../../../ARCHIVE-INDEX.md#older-native-cutover-cohorts).

### B-CARRY-SELECTED-ARTIFACT-BACKING-142

The carry reserved space for every declared source row before it knew which
members it would retain. A valid 64-member delta and sealed region with the
same immutable members requested a second, 128-slot allocation even though
the sealed source added no slots. The owning regression failed with
`Some(Memory)` and `ResourceLimited` (Nextest
`d843a0b6-f183-4539-869e-8e8300d6169f`, 1 body, exit 100).

Consolidation now reserves backing when an authenticated, currently selected
member adds a slot. Optional geometric growth stays within the source and
region bounds. When the optional grant is refused, admission requests the
exact required slot. The previous grant stays alive through reallocation.
Source row counts, coverage checks, original map CAS and file retention use
the existing authority paths.

The repaired fixture checks the 64-member duplicate carry without requesting
the refused allocation. With one stale member, it also checks that refusal
of optional 64-slot backing still permits the 63 selected members and leaves
`first_denial` empty. Both cases verify the final map and released leases.
Nextest `bc796a3b-f3a6-4cd9-bceb-9cfecff29273` executed 10/10 PASS (15.596s,
321 skipped, exit 0), including the new regression, multi-page CAS, capacity
and nearest-neighbor carry. Both changed Rust files and `Cargo.lock` retained
their captured hashes through execution. Scoped rustfmt, diff, module
discipline and hexagonal checks passed.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-semantic --lib -E 'test(/^native::embedding::tests::ann_product_tests::maintenance::consolidation_/) | test(carry_trace_) | test(native_ann_small_runs_carry_and_retain_independent_nearest_neighbor)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

### B143 · 4K product watchdog attribution

The original admission accepted both selectors. ANN4K Nextest
`e1e78e47-7a03-4f89-b624-99d3fec2ea6f` passed 1/1 in 36.947s, with 640/640
for each of the six nonempty eligibility strata and no hits for the empty
stratum. Product Nextest `ab0014fa-2851-4e64-af77-cf14c98fd920` failed its
sole body in 1,225.878s including startup and failure cleanup (exit 100).
Both runs recorded empty source and resolver drift. All 1,054 common source
entries and all 67 resolver inputs matched.

The product published the initial 4,096 sources and observed all 4,096 work
completions. Initial readiness took 814.467s, including 43.921s of direct
WorkStatus RPC wall time across 6,719 calls. Initial and cold-reopen quality,
and selected delta queries, passed before the original absolute deadline
expired during an unsaved-overlay effort stream. The final request delivered
Fast and timed out before Deep. These observations locate the expiry and show
the largest measured phase. They do not attribute the underlying latency to
the provider, HNSW, Catalog, or storage.

The product fixture now records elapsed wall time and remaining watchdog for
each phase, including shutdown and restart, and identifies the failing query
seed during the unsaved-overlay sweep. The selected views, workload, oracle,
request budgets, and absolute deadline are unchanged. The continuation retains
the recorded ANN4K pass and retries only the product selector through the
original execution function and admission.
Formatting, test authority, module discipline and hexagonal checks passed.
The focused deadline and result-oracle invocation timed out waiting for the
shared build lock (exit 124, zero bodies). The product retry is waiting for
the original timing admission.

Daemon restart and graceful shutdown now use the remaining product watchdog
alongside their original local limits. The HTTP fixture applies one absolute
deadline across each exchange and interrupts its active socket during shutdown.
Nextest `203f1db3-9bfc-4d52-9a77-c9f4f1061a8e` executed 5/5 PASS (21.707s,
60 skipped, exit 0). The selection covered two daemon deadline regressions,
two HTTP exchange and shutdown regressions, and the actual daemon Semantic
materialization, query and reopen body. Whole-product 4K remains pending.

The follow-up audit found that a late fixture shutdown could classify an
already-expired exchange as intentional cancellation. The stub records the
first shutdown instant and compares it to the original exchange deadline.
Two regressions cover both event orders and the actual socket timeout during
a stalled read. Formatting, test authority and architecture checks passed.
The focused invocation for this follow-up ended at the shared build lock
with exit 124 and zero test bodies.

The product oracle now checks the full durable `Latest` identity before and
after overlay close and after the second restart. The overlay Deep and exact
path results must carry exactly the replacement and tombstone revisions.
The second restart also replays the query whose independent exact result ranks
the replacement first. HTTP fixture completion drains accepted exchanges under
their existing deadlines and propagates their errors. Forced cleanup interrupts
the active socket.

Formatting, test authority, module discipline and hexagonal checks passed for
this change. The focused nine-body selection ended at the shared build lock
after 1,800 seconds (exit 124, zero bodies). The prior ANN4K pass is retained.

The final restart provider oracle now seals the input inventory after the HTTP
fixture drains accepted exchanges. Its focused test supplies the request body
after finish begins and checks that the returned inventory includes that input.
Nextest `63fdf0d3-ff12-4d69-b641-50737c56bc6a` ran 9/9 PASS (14.453s,
60 skipped, exit 0) in `native_product_process`. Both `embedding_exchange_resolves_shutdown_and_timeout_in_event_order`
and `embedding_exchange_caps_the_socket_timeout_during_stalled_read` passed. The run also covered fixture
shutdown, both query oracles, and actual daemon Semantic materialization, query and reopen.

The product-only retry passed original host and build admission. Nextest
`6747f847-21c9-4c86-be68-50a2bcd88a3e` ran 1 body and failed in 1,227.596s
of Nextest wall time (exit 100). Initial materialization completed all 4,096
owners in 915.319s, including 48.088s of direct WorkStatus RPC wall time. Initial
and reopened ANN sweeps took 91.954s and 86.295s. The incremental sweep had
53.003s left and exhausted the original watchdog at seed 38. Overlay and the
second restart were not reached. The measured phases establish where the time
was spent. Worker-stage attribution remains open.

The provider oracle repair preceded compilation of `native_product_process`.
The controller observed 36 source edits during the run, including the two test
files for this repair, and zero resolver drift. Comparing the captured starts
to the earlier ANN4K run found differences on 5 of 1,054 shared source paths
and 2 of 67 resolver inputs. The retry is a concrete failed product body
without same-input qualification.

The readiness log separates direct public WorkStatus RPC wall time, actual
observer pauses, the first complete receipt observation, and time after that
observation, which includes the product's Latest and coverage checks.
Observer pauses overlap worker execution.
The durable ANN sweep logs search SDK wall time separately from independent
local result validation, including the failing request and its seed. Neither
log aligns the harness clock with the daemon clock.

For the existing 4K prefix Record diagnostic, use the trace path logged by the
fixture. Pass the fixture's 256 MiB cap explicitly to offline replay:

```sh
python3 -m tools.benchmark.retrieval.causal_cost_diagnostic build \
  --input "$trace_path" --max-capture-bytes 268435456 --out "$index_path"
python3 -m tools.benchmark.retrieval.causal_cost_diagnostic query \
  --index "$index_path" --stage native_coverage_save
```

If the daemon terminates without a healthy trace footer, use `closed-workers`
with the same input and cap and a new output path. Its results cover only
observed closed worker scopes. Exact public receipts come from the harness
log. The prefix scope covers 128 observed completions.

The offline diagnostic and ingest reader suites passed 80/80. Formatting,
test authority, module discipline, and hexagonal checks passed. The focused
three-test Rust selection ended at the shared build lock after 1,800 seconds
(exit 124, zero bodies).

The product overlay stream now checks the Fast lexical result on the original
requests. For `row-0`, the unscoped and replacement-path requests must return
the one unsaved hit with its path, owner and overlay revision. The tombstoned
and deleted paths must return empty Fast pages. The focused oracle exercises
an empty matching page, a durable hit in place of the overlay hit, and a hit
outside the selected path.

Nextest `c29d5946-cd11-4083-a07f-db471035a464` subsequently passed all four
focused bodies (exit 0, 70 skipped). The selection covered the readiness
window, failed SDK call timing, prefix completion receipts and the Fast
overlay oracle. The private Cargo target compiled the workspace's patched
SQLite dependency. The earlier LanceDB audit and build RCA remain source-bound
in the [audit preimage](../../../ARCHIVE-INDEX.md#nov-20-ann-audit-preimage);
the four-body result above is the scoped current ticket evidence.

## Retired section links

These historical IDs recover B01–B111 from the [exact preimage](../../../ARCHIVE-INDEX.md#2026-11-16-semantic-ticket-history-compaction).
Current work follows Current remaining acceptance; old queue/overlay/freeze text
is not an implementation instruction.

<a id="b-red-file-01--frozen-behavioral-test-request"></a><!-- B-RED-FILE-01 · frozen behavioral test request -->
<a id="b-graph-integration-02"></a><!-- B-GRAPH-INTEGRATION-02 -->
<a id="b-file-api-03--final-planned-custody-shape-for-c"></a><!-- B-FILE-API-03 · final planned custody shape for C -->
<a id="b-graph-validation-03--coherent-compile-batch"></a><!-- B-GRAPH-VALIDATION-03 · coherent compile batch -->
<a id="b-red-compile-closure-04"></a><!-- B-RED-COMPILE-CLOSURE-04 -->
<a id="b-owning-queue-05--requested-current-source-selectors"></a><!-- B-OWNING-QUEUE-05 · requested current source selectors -->
<a id="b-ann4k-owner-06--actual-selected-query-fixture"></a><!-- B-ANN4K-OWNER-06 · actual selected query fixture -->
<a id="b-graph-review-07"></a><!-- B-GRAPH-REVIEW-07 -->
<a id="b-budget-review-08--original-product-oracle-reused"></a><!-- B-BUDGET-REVIEW-08 · original product oracle reused -->
<a id="b-compile-closure-09"></a><!-- B-COMPILE-CLOSURE-09 -->
<a id="b-compile-closure-10"></a><!-- B-COMPILE-CLOSURE-10 -->
<a id="b-red-consumed-11--b-green-file-11"></a><!-- B-RED-CONSUMED-11 / B-GREEN-FILE-11 -->
<a id="b-source-ready-12--frozen-sourceconsumer-handoff-not-qualification"></a><!-- B-SOURCE-READY-12 · frozen source/consumer handoff, not qualification -->
<a id="b-test-extend-13--test-only-oracle-completion"></a><!-- B-TEST-EXTEND-13 · test-only oracle completion -->
<a id="b-consumed-14--actual-consumer-custody-joined"></a><!-- B-CONSUMED-14 · actual consumer custody joined -->
<a id="b-source-change-15--f-native-runtime-removal-request-accepted"></a><!-- B-SOURCE-CHANGE-15 · F Native runtime removal request accepted -->
<a id="b-proof-selectors-16--stable-cohort-owning-execution"></a><!-- B-PROOF-SELECTORS-16 · stable-cohort owning execution -->
<a id="b-test-custody-17--test-only-parts-lifetime-invariant"></a><!-- B-TEST-CUSTODY-17 · test-only parts lifetime invariant -->
<a id="b-test-graph-18--malformed-binaryedge-oracle-extension"></a><!-- B-TEST-GRAPH-18 · malformed binary/edge oracle extension -->
<a id="b-test-map-19--avoid-fixture-wide-copying-on-point-lookup"></a><!-- B-TEST-MAP-19 · avoid fixture-wide copying on point lookup -->
<a id="b-test-determinism-20--serialparallel-region-construction"></a><!-- B-TEST-DETERMINISM-20 · serial/parallel region construction -->
<a id="b-handoff-21--current-verification-boundary"></a><!-- B-HANDOFF-21 · current verification boundary -->
<a id="b-ann4k-red-22--actual-budget-failure-phase-isolation-before-repair"></a><!-- B-ANN4K-RED-22 · actual budget failure, phase isolation before repair -->
<a id="b-consume-physical-23--f-actual-kerneluds-scope"></a><!-- B-CONSUME-PHYSICAL-23 · F actual Kernel/UDS scope -->
<a id="b-source-change-24--executed-construction-rca-and-bounded-reuse"></a><!-- B-SOURCE-CHANGE-24 · executed construction RCA and bounded reuse -->
<a id="b-original-rerank-25--test-only-authoritative-score-oracle"></a><!-- B-ORIGINAL-RERANK-25 · test-only authoritative score oracle -->
<a id="b-source-change-24-applied--frozen-for-same-owning-repeat"></a><!-- B-SOURCE-CHANGE-24 applied · frozen for same owning repeat -->
<a id="b-ann4k-green-26--same-original-rail-closed"></a><!-- B-ANN4K-GREEN-26 · same original rail closed -->
<a id="b-lifecycle-rca-27--actual-remaining-owner-failures"></a><!-- B-LIFECYCLE-RCA-27 · actual remaining owner failures -->
<a id="b-consume-product-consumer-28--pending-exact-lifecycle-repeat"></a><!-- B-CONSUME-PRODUCT-CONSUMER-28 · pending exact lifecycle repeat -->
<a id="b-construction-bound-29--actual-row511-red-and-canonical-bounded-insertion"></a><!-- B-CONSTRUCTION-BOUND-29 · actual row511 RED and canonical bounded insertion -->
<a id="b-owning-green-30--same-original-rail-actual-physical-producer"></a><!-- B-OWNING-GREEN-30 · same original rail, actual physical producer -->
<a id="b-product-admission-31--current-external-execution-dependency"></a><!-- B-PRODUCT-ADMISSION-31 · current external execution dependency -->
<a id="b-session-hardening-32--scope-freeze-and-confirmed-test-gap"></a><!-- B-SESSION-HARDENING-32 · scope freeze and confirmed test gap -->
<a id="b-session-hardening-green-33--executed-owner-proof-bounded-verdict"></a><!-- B-SESSION-HARDENING-GREEN-33 · executed owner proof, bounded verdict -->
<a id="b-session-hardening-34--second-invocation-upper-visit-cap-finding--red-request"></a><!-- B-SESSION-HARDENING-34 · second invocation, upper-visit cap finding / RED request -->
<a id="b-upper-cap-red-35--raw-behavioral-failure-consumed-canonical-repair"></a><!-- B-UPPER-CAP-RED-35 · raw behavioral failure consumed, canonical repair -->
<a id="b-filter-plan-36--read-only-design-while-f56-source-is-frozen"></a><!-- B-FILTER-PLAN-36 · read-only design while F56 source is frozen -->
<a id="b-upper-cap-green-37--actual-redrepairgreen-closeout"></a><!-- B-UPPER-CAP-GREEN-37 · actual RED→repair→GREEN closeout -->
<a id="b-remaining-execution-38--current-user-resumes-remaining-b-work"></a><!-- B-REMAINING-EXECUTION-38 · current user resumes remaining B work -->
<a id="b-eligibility-red-39--actual-record-terminal-observed-mount-prepared-oracle"></a><!-- B-ELIGIBILITY-RED-39 · actual Record terminal observed, mount prepared oracle -->
<a id="b-eligibility-repair-40--actual-four-stratum-red-and-canonical-score-accounting"></a><!-- B-ELIGIBILITY-REPAIR-40 · actual four-stratum RED and canonical score accounting -->
<a id="b-direct-verification-41--explicit-current-user-authority"></a><!-- B-DIRECT-VERIFICATION-41 · explicit current user authority -->
<a id="b-eligibility-green-42--actual-repaired-body-and-affected-consumer-cohort"></a><!-- B-ELIGIBILITY-GREEN-42 · actual repaired body and affected consumer cohort -->
<a id="b-product-admission-43--full-ann-body-not-entered-execution-boundary-returned"></a><!-- B-PRODUCT-ADMISSION-43 · full ANN body not entered, execution boundary returned -->
<a id="b-diagnostic-decoupling-44--accept-exact-native-caller-ownership-no-active-runner"></a><!-- B-DIAGNOSTIC-DECOUPLING-44 · accept exact Native caller ownership; no active runner -->
<a id="b-atomic-handoff-45--correct-mount-order-and-exact-one-time-caller-loan"></a><!-- B-ATOMIC-HANDOFF-45 · correct mount order and exact one-time caller loan -->
<a id="b-adoption-verified-46--exact-root-integration-returned-compile-pending-root"></a><!-- B-ADOPTION-VERIFIED-46 · exact Root integration returned; compile pending Root -->
<a id="b-diagnostic-behavior-47--consume-actual-c-terminal-narrow-remaining-semantic-scope"></a><!-- B-DIAGNOSTIC-BEHAVIOR-47 · consume actual C terminal, narrow remaining semantic scope -->
<a id="b-pure-recipe-conditional-loan-48--offline-validation-without-executable-provider"></a><!-- B-PURE-RECIPE-CONDITIONAL-LOAN-48 · offline validation without executable provider -->
<a id="b-pure-recipe-proposal-49--b-sole-writer-outside-preparation-only"></a><!-- B-PURE-RECIPE-PROPOSAL-49 · B sole writer, outside preparation only -->
<a id="b-pure-recipe-outside-50--concrete-candidate-and-exact-api-handoff"></a><!-- B-PURE-RECIPE-OUTSIDE-50 · concrete candidate and exact API handoff -->
<a id="b-cohort-boundary-51--actual-f86-terminals-consumed-explicit-release-pending"></a><!-- B-COHORT-BOUNDARY-51 · actual F86 terminals consumed; explicit release pending -->
<a id="b-pure-recipe-live-52--explicit-root-release-one-batch-no-rust-runner"></a><!-- B-PURE-RECIPE-LIVE-52 · explicit Root release; one batch, no Rust runner -->
<a id="b-pure-recipe-frozen-53--live-complete-api-compilebehavior-pending-sole-root-queue"></a><!-- B-PURE-RECIPE-FROZEN-53 · live complete API; compile/behavior pending sole Root queue -->
<a id="b-root-queue-54--source-consumed-actual-pure-contract-run-selected-next"></a><!-- B-ROOT-QUEUE-54 · source consumed, actual pure-contract run selected next -->
<a id="b-pure-contract-verified-55--actual15-bodies-pass-d-product-still-body0"></a><!-- B-PURE-CONTRACT-VERIFIED-55 · actual15 bodies PASS; D product still body0 -->
<a id="b-offline-consumer-verified-56--actual-old-compose-red-and-integrated-product-green"></a><!-- B-OFFLINE-CONSUMER-VERIFIED-56 · actual old-compose RED and integrated product GREEN -->
<a id="b-gc-custody-57--readonly-answer-for-roots-exact-stale-test-executable"></a><!-- B-GC-CUSTODY-57 · readonly answer for Root's exact stale test executable -->
<a id="b-legacy-readonly-proposal-58--actual-owner-gap-outside-only-canonical-decoder-hunk"></a><!-- B-LEGACY-READONLY-PROPOSAL-58 · actual owner gap; outside-only canonical decoder hunk -->
<a id="b-retirement-dependencies-59--larger-physical-supplier-blocked-no-new-engine-path"></a><!-- B-RETIREMENT-DEPENDENCIES-59 · larger physical supplier BLOCKED; no new engine path -->
<a id="b-production-demount-outside-60--concrete-coupled-sourcemanifest-candidate"></a><!-- B-PRODUCTION-DEMOUNT-OUTSIDE-60 · concrete coupled source/manifest candidate -->
<a id="b-ann-oracle-leaf-outside-61--exact-inputoracle-transfer-rootd-import-boundary"></a><!-- B-ANN-ORACLE-LEAF-OUTSIDE-61 · exact input/oracle transfer; Root/D import boundary -->
<a id="b-f91-terminal-reconciliation-62--terminal-observed-no-automatic-source-release"></a><!-- B-F91-TERMINAL-RECONCILIATION-62 · terminal observed; no automatic source release -->
<a id="b-d-physical-loan-boundary-63--exact-supplier-status-for-d-intake"></a><!-- B-D-PHYSICAL-LOAN-BOUNDARY-63 · exact supplier status for D intake -->
<a id="b-physical-verifier-ownership-64--implementation-accepted-work-remains-active"></a><!-- B-PHYSICAL-VERIFIER-OWNERSHIP-64 · implementation accepted, work remains active -->
<a id="b-cbor-allocation-audit-65--concrete-verifier-allocation-dependency"></a><!-- B-CBOR-ALLOCATION-AUDIT-65 · concrete verifier allocation dependency -->
<a id="b-root-coupled-packet-66--current-exact-preimages-and-d-layout-ownership"></a><!-- B-ROOT-COUPLED-PACKET-66 · current exact preimages and D layout ownership -->
<a id="b-existing-verifier-implemented-67--actual-outside-source-no-physical-acceptance-claim"></a><!-- B-EXISTING-VERIFIER-IMPLEMENTED-67 · actual outside source; no physical acceptance claim -->
<a id="b-actual-core-export-reconciliation-68--final-verifier-overlay-namespace"></a><!-- B-ACTUAL-CORE-EXPORT-RECONCILIATION-68 · final verifier overlay namespace -->
<a id="b-roots-memory-custody-69--borrowed-consumer-fixes-output-lifetime"></a><!-- B-ROOTS-MEMORY-CUSTODY-69 · borrowed consumer fixes output lifetime -->
<a id="b-d-shared-hash-extraction-70--exact-exclusive-generic-source-transfer-accepted"></a><!-- B-D-SHARED-HASH-EXTRACTION-70 · exact exclusive generic source transfer accepted -->
<a id="b-provider-main-drift-71--readonly-custody-check-after-roots-outside-only-write-violation"></a><!-- B-PROVIDER-MAIN-DRIFT-71 · readonly custody check after Root's outside-only write violation -->
<a id="b-single-shared-hash-consumer-72--concrete-d-facade-consumed-private-duplicate-removed"></a><!-- B-SINGLE-SHARED-HASH-CONSUMER-72 · concrete D facade consumed; private duplicate removed -->
<a id="b-core-hash-actual-and-delivery-73--four-executed-pass-exact-source-correction"></a><!-- B-CORE-HASH-ACTUAL-AND-DELIVERY-73 · four executed PASS; exact source correction -->
<a id="b-minimal-data-prerequisites-74--current-main-base-without-native-demount"></a><!-- B-MINIMAL-DATA-PREREQUISITES-74 · current-Main base, without Native demount -->
<a id="b-cbor-admission-regressions-75--source-backed-review-and-four-reusable-owning-bodies"></a><!-- B-CBOR-ADMISSION-REGRESSIONS-75 · source-backed review and four reusable owning bodies -->
<a id="b-first-main-compiler-boundary-76--body0-d-owns-layout-correction"></a><!-- B-FIRST-MAIN-COMPILER-BOUNDARY-76 · body0; D owns layout correction -->
<a id="b-production-compile-and-seal-verified-77--actual-root-retry-terminals"></a><!-- B-PRODUCTION-COMPILE-AND-SEAL-VERIFIED-77 · actual Root retry terminals -->
<a id="b-current-caller-boundary-78"></a><!-- B-CURRENT-CALLER-BOUNDARY-78 -->
<a id="b-local-code-audit-79--physical-wrapper-and-single-canonical-validation"></a><!-- B-LOCAL-CODE-AUDIT-79 · physical wrapper and single canonical validation -->
<a id="b-native-allocation-audit-80--original-scratch-bounds"></a><!-- B-NATIVE-ALLOCATION-AUDIT-80 · original scratch bounds -->
<a id="b-current-demount-rebase-81--physical-supplier-preserved"></a><!-- B-CURRENT-DEMOUNT-REBASE-81 · physical supplier preserved -->
<a id="b-delta-counter-audit-82--fail-closed-at-producer-and-metadata-boundary"></a><!-- B-DELTA-COUNTER-AUDIT-82 · fail closed at producer and metadata boundary -->
<a id="b-failure-atomicity-audit-83--four-executed-reds-and-current-broad-green"></a><!-- B-FAILURE-ATOMICITY-AUDIT-83 · four executed REDs and current broad GREEN -->
<a id="b-page-contract-audit-84"></a><!-- B-PAGE-CONTRACT-AUDIT-84 -->
<a id="b-projected-reverse-page-85"></a><!-- B-PROJECTED-REVERSE-PAGE-85 -->
<a id="b-region-append-audit-86"></a><!-- B-REGION-APPEND-AUDIT-86 -->
<a id="b-map-root-audit-87"></a><!-- B-MAP-ROOT-AUDIT-87 -->
<a id="b-selected-coverage-audit-88"></a><!-- B-SELECTED-COVERAGE-AUDIT-88 -->
<a id="b-page-continuation-audit-89"></a><!-- B-PAGE-CONTINUATION-AUDIT-89 -->
<a id="b-graph-base-cycle-audit-90"></a><!-- B-GRAPH-BASE-CYCLE-AUDIT-90 -->
<a id="b-prior-layout-audit-91"></a><!-- B-PRIOR-LAYOUT-AUDIT-91 -->
<a id="b-single-directory-read-audit-92"></a><!-- B-SINGLE-DIRECTORY-READ-AUDIT-92 -->
<a id="b-payload-authority-audit-93"></a><!-- B-PAYLOAD-AUTHORITY-AUDIT-93 -->
<a id="b-delta-member-read-audit-94"></a><!-- B-DELTA-MEMBER-READ-AUDIT-94 -->
<a id="b-projected-path-source-read-audit-95"></a><!-- B-PROJECTED-PATH-SOURCE-READ-AUDIT-95 -->
<a id="b-source-local-path-read-audit-96"></a><!-- B-SOURCE-LOCAL-PATH-READ-AUDIT-96 -->
<a id="b-materialized-policy-audit-97"></a><!-- B-MATERIALIZED-POLICY-AUDIT-97 -->
<a id="b-small-ann-exact-delta-read-audit-98"></a><!-- B-SMALL-ANN-EXACT-DELTA-READ-AUDIT-98 -->
<a id="b-superseded-ann-owner-slot-audit-99"></a><!-- B-SUPERSEDED-ANN-OWNER-SLOT-AUDIT-99 -->
<a id="b-ann-cap-and-prior-profile-audit-100"></a><!-- B-ANN-CAP-AND-PRIOR-PROFILE-AUDIT-100 -->
<a id="b-projected-ann-file-eligibility-audit-101"></a><!-- B-PROJECTED-ANN-FILE-ELIGIBILITY-AUDIT-101 -->
<a id="b-exact-file-eligibility-audit-102"></a><!-- B-EXACT-FILE-ELIGIBILITY-AUDIT-102 -->
<a id="b-partial-path-file-owner-limit-audit-103"></a><!-- B-PARTIAL-PATH-FILE-OWNER-LIMIT-AUDIT-103 -->
<a id="b-partial-path-source-record-audit-104"></a><!-- B-PARTIAL-PATH-SOURCE-RECORD-AUDIT-104 -->
<a id="b-projected-direct-file-cap-audit-105"></a><!-- B-PROJECTED-DIRECT-FILE-CAP-AUDIT-105 -->
<a id="b-sealed-ann-member-binding-read-audit-106"></a><!-- B-SEALED-ANN-MEMBER-BINDING-READ-AUDIT-106 -->
<a id="b-graph-owner-member-commitment-audit-107"></a><!-- B-GRAPH-OWNER-MEMBER-COMMITMENT-AUDIT-107 -->
<a id="b-projected-direct-file-source-read-audit-108"></a><!-- B-PROJECTED-DIRECT-FILE-SOURCE-READ-AUDIT-108 -->
<a id="b-sealed-ann-artifact-decode-audit-109"></a><!-- B-SEALED-ANN-ARTIFACT-DECODE-AUDIT-109 -->
<a id="b-sealed-ann-cache-memory-audit-110"></a><!-- B-SEALED-ANN-CACHE-MEMORY-AUDIT-110 -->
<a id="b-sealed-ann-cache-custody-audit-111"></a><!-- B-SEALED-ANN-CACHE-CUSTODY-AUDIT-111 -->
