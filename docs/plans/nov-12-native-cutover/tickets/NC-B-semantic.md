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
| B-PRODUCT-01 owning ANN4K | Re-run the original selector with LanceDB under the existing admission. The [B143 graph cohort](#b143--4k-product-watchdog-attribution) passed 1/1 at independent recall 640/640. Compare common inputs with the LanceDB product run. |
| B43 whole SDK product | Re-run the original product selector with LanceDB under the existing admission. The [B143 graph retry](#b143--4k-product-watchdog-attribution) exceeded its 1,200s watchdog at incremental ANN seed 38, before Overlay Close and the second restart. It recorded source drift. |
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

The repeat review found no additional confirmed production P0–P2 in persisted graph/directory validation, selected source/member/chunk binding, cache custody, or file/raw/overlay failure paths. The actual storage port authenticates the complete object digest; `load_seal`'s header-only validation remains confined to maintenance. A fake port that breaks that storage contract is not a production reproducer.

Two missing observations remained in the original4K product acceptance body. After SourceAdvance/materialization it checked the new replacement/deletion but did not re-query the old Exact view. After overlay Close it checked durable exact paths but did not repeat an ordinary ANN query. These are missing product-proof obligations, not demonstrated bad production output.

The same4K body now performs two old-view exact-path queries after the changed view is materialized: `src/0000.rs` must still match original row0 and `src/0122.rs` must still match original row122. The existing changed-view checks independently require replacement row0 and absence of row122. The fixture already requires retention to be disabled, so these historical Exact reads are within its existing contract; no new active-pin/retention-GC claim is made.

After overlay Close, one ordinary query0 against changed_view is checked with the existing independent durable `delta.rows` score/order/source oracle. Its owner page must equal the same view/query's pre-overlay `changed_sweep.pages[0]`. The original post-Close exact-path checks remain. This observes ANN-route durable output after the unsaved contribution; it is not a claim that every overlay lifecycle variant was executed. All three added semantic requests and the selected-view read use the existing changed_deadline. The original4096x64/Q64/K10, recall floor, work/read policies and absolute1200s watchdog are unchanged; no second Q64 sweep or new fixture is added.

Changed code is limited to `tests/native_product_process/ann_quality.rs`; source SHA256 is `804065590c96a8220e6da45209048193afd4ef74d5077fe80ded767cc8ce890e`. The test source was unchanged during verification. Other writers changed Harness feature/source routing, Runtime Cargo/configuration and unrelated files during the wider capture; those hunks were preserved. The focused result below is not a clean, frozen full-product qualification.

VERIFIED compilation and reused checkers/deadline rails: nextest `213907e1-e19f-4fc0-8a60-1553686ef73f` ran4/4 PASS in0.210s (46 skipped, exit0). It executed the base score/order and overlay-order counterexamples plus both original-deadline cases: expired input refuses before work and a live handoff preserves the same Instant/result. The native_product_process target, including the added4K SDK calls, compiled. An earlier2-test oracle pass overlaps this four-test selection and is not counted again. None of these four bodies executes the added lifecycle requests.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-searchd-runtime --test native_product_process --no-tests fail -E 'test(quality_result_oracle_rejects_wrong_score_and_owner_order) | test(overlay_quality_oracle_rejects_misordered_page) | test(selected_change_phase_)' --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate --status-level fail --final-status-level fail
```

Scoped rustfmt, `just rust-hexagonal`, and `git diff --check` passed. Disposable records remain outside the checkout at `/tmp/qidx-ann-lifecycle-audit-9_z6xlag`. The own admission-only4K waiter was stopped before editing, with no test body active, and restored after the focused check. It still uses the original host gate and serial owning commands at `/tmp/qidx-session-hardening-691a9ukw/quality/state.json`.

Residual/state: partial. The added old-view and post-overlay SDK observations are NOT_RUN as part of `product_native_ann_quality_4096x64_reopens_at_fixed_recall`; owner-local `native_ann4k_selected_file_recall_matches_independent_exact` is also NOT_RUN. Compiler success and the four helper bodies do not close those product obligations. No additional actual behavioral failure, cap/threshold/deadline relaxation, external chat access or unrelated process interruption is claimed.

### B-ANN4K-AND-PRODUCT-EXECUTION-121

State: partial. The user resumed both original4K bodies in this chat with the existing admission and no threshold, budget or watchdog changes. The existing single serial controller was retained; no parallel or duplicate Rust runner was submitted. External chat messages were not requested or answered.

VERIFIED owner-local ANN4K: nextest `4953c891-02a8-4c07-bd75-dc2c0d84b7ff`, discovered/executed1, passed1, failed0, skipped325, exit0, 39.288s. It ran the actual4096x64/Q64/K10 native graph body, independent exact recall640/640, empty eligibility, all six nonempty uniform/correlated eligibility strata640/640, and each nonempty raw sibling10/10. This is not a whole SDK product result.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-semantic --lib -E 'test(native_ann4k_selected_file_recall_matches_independent_exact)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

The original `python3 tools/ci/timing/check_host_contention.py` admitted execution without an override; the canonical build-test resource lock was acquired and released. HEAD remained `ade5fadc1a084fbb81eba831861e996081d80867` on shared dirty main. The controller's relevant Rust/Cargo snapshot had `source_drift=[]`; ANN4K test SHA256 remained `f635b538f12e21ff6bed42bd02f7470a78f2ac0c6e7d089c85a027fbf45d99bd`. Disposable state and complete output remain outside the checkout at `/tmp/qidx-session-hardening-691a9ukw/quality`.

The same controller then advanced to the original product command, passed the unchanged admission and acquired the same resource lock. FAILED: nextest `e10a9dc8-bccd-4425-a20e-f4ab096a1cf6`, discovered/executed1, passed0, failed1, skipped49, exit100, 1216.696s including failure cleanup.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-searchd-runtime --test native_product_process -E 'test(product_native_ann_quality_4096x64_reopens_at_fixed_recall)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

Observed failure: `ANN quality incremental selected ANN quality sweep failed` at query seed55, IPC Read timed out with788ms remaining on the original absolute1200s deadline. Initial4096 materializations completed, initial and cold-reopen Q64/provider checks passed, the one-owner incremental materialization completed in1.979s, and B120's two old-Exact observations were reached before the failed sweep. Changed-view exact paths and overlay/post-Close did not execute. B124 later found that the original fixture also omitted a restart after the changes.

Initial readiness took902.756s;7002 WorkStatus RPCs consumed53.982s direct RPC wall time (5.98%). The observer recorded444 distinct receipts at421s and4096 at902s. Its round-robin census uses a different order from the Catalog ready queue, so these samples do not measure instantaneous worker throughput. SourceAdvance submit alone took19.386s. It excludes source staging, leaving other deadline consumption unattributed. Two renewal fence-loss samples reconciled Terminal(Published)/finish Ok. Static review found no demonstrated full4096 scan per scheduler claim. The readiness/worker/semantic persisted-I/O costs require phase diagnosis before changing production semantics.

The product controller recorded source drift only in `crates/quanta-index-core/src/index/records.rs`; the failure must not be presented as a frozen current-source qualification or attributed to that edit without evidence. Between ANN4K and product launches only RepoMap `native/file_map.rs` changed in the captured cohort; RepoMap is not an ANN4K library dependency. The product test source retained SHA256 `804065590c96a8220e6da45209048193afd4ef74d5077fe80ded767cc8ce890e`. The controller exited after the failure and released its lock; any next proof remains serial and subject to the original admission. State remains partial with structural RCA open.

### B-MATERIALIZATION-PHASE-RCA-122

State: partial; production cause and repair remain unproved. Read-only review of owned pump, Catalog ready/claim/status, Semantic materialization and the readiness observer found no demonstrated per-tick4096-entry pending scan. WorkStatus uses indexed single-work reads. Fixed-policy worker/resource observations are bounded constant work. Semantic live-region maintenance repeats authenticated graph-object reads, but removing body/header validation would weaken an existing authority boundary and no timing attribution identifies those reads as the dominant cost. No speculative production patch was applied.

The existing ignored `diagnostic_native_ann_4096_source_128_completed_prefix` fixture was selected to retain the4K source inventory and original worker/resource policies while observing the first128 exact public completion receipts. Its existing180s diagnostic watchdog and256MiB trace cap were preserved; it cannot qualify full4K quality. Command uses the B121 Runtime target/front door/lane with `--run-ignored only`, selector `test(diagnostic_native_ann_4096_source_128_completed_prefix)` and explicit `QIDX_NATIVE_ANN_TRACE_DIR`, `QIDX_NATIVE_ANN_DIAGNOSTIC_TRACE_MODE=record`, `QIDX_NATIVE_TRACE_MAX_OUTPUT_BYTES=268435456`.

Two admitted compile attempts executed zero bodies (exit101). First, Scratch `edit_write::ordinal_fields` called the new `key/expected_wire/next_wire` API before the concurrent Core producer exposed it. The Core input then changed to expose those methods, justifying the second attempt. It progressed to Storage, where `prepare_resident_roots` still mixed MapEdit/OrdinalMapEdit and two map cursor consumers still accessed the retired `position.edit` field. These shared files were visibly being edited by another writer; this chat preserved them. The resident helper has since changed to a generic borrowed-edit input, while the known field consumers are monitored before any further Rust retry. No same-input failure loop or external chat contact is used.

Disposable compile logs/source manifests are at `/tmp/qidx-ann-materialization-rca-t9bu2q50/compile-1-failed` and `compile-2-failed`. Those attempts had no body or trace. The known Storage cursor field consumers were subsequently updated by the existing writer, so the changed cohort was admitted and compiled on the third attempt (51.06s).

Actual prefix diagnosis FAILED: nextest `e8bbcb60-5d68-4c13-b23b-2df6de901cdc`, executed1/failed1/skipped49, exit100, 193.066s including cleanup. The original180s diagnostic watchdog expired after39/128 exact observed completion receipts; the provider had processed759 distinct source inputs with no duplicates. Those counts measure different authorities and must not be substituted for one another. Readiness RPC377 calls/5.319s direct time over148.354s. A93,235,895-byte daemon capture was saved outside the checkout; it is not an Off/Record comparison or whole-product proof. Source drift was confined to Lexical edit-spool production/tests, so it is not a frozen cohort qualification.

The unchanged canonical trace reader rejected that capture with `ANN region observation targets wrong owner stage`. RCA found the Native producer in `ann_materialization::seal_delta` emits Installed on a SemanticBuild span, while the public trace contract requires SemanticSeal. Legacy ann_files uses the correct stage. This is a confirmed Native trace producer/caller contract defect, separate from the product watchdog's still-open dominant cause. The unchanged reader's16 ANN-region tests pass, including wrong-stage rejection. Repair is scoped to the Native producer and an actual Native seal regression; reader validation, full graph validation, product budgets and quality oracles remain intact. A fresh valid capture is required before worker-stage attribution.

### B-NATIVE-SEAL-OBSERVATION-OWNER-REPAIR-123

The pre-patch Native capture is a real failing input to the unchanged canonical reader; its Installed event targets a SemanticBuild owner, violating the required SemanticSeal contract. A focused pre-patch unit-test waiter was stopped while still admission-only (exit143, executed0); it is NOT_RUN, not an additional RED execution. The observed capture rejection remains the behavioral RED evidence.

The canonical `AnnDirectory::append_region` now owns the seal span and Installed event. Its private inner function preserves the prior full graph-body/seal, descriptor, recipe, row, generation, COW map-root and next-directory checks unchanged. Only a successful inner return emits the exact returned descriptor reference, validated fileset and pre-mutation ordinal, then closes SemanticSeal with Ok. Failure emits no Installed event and closes with Error before returning the original error. `seal_delta` keeps graph build/install in SemanticBuild and no longer emits an incorrectly owned region event. This is a trace contract repair; it does not by itself establish the product timeout's dominant cause or change graph bytes, budgets, admission or quality checks.

Two actual Native regressions are added in the existing maintenance test module:64-owner materialization validates the loaded region's exact fileset/descriptor identity and successful seal owner; a genuine one-row graph with a two-row descriptor requires CorruptObject, unchanged directory, zero writes, no Installed observation and a non-aborted Seal Error terminal. Source review found no weakened validation or duplicate Native installed emitter. Scoped rustfmt and diff checks pass; Python's unchanged ANN-region reader selection passed16/16.

The initial eight-test queue unnecessarily composed the contract-only regressions with timing-bearing ANN4K, making the seven ordinary regressions wait for the quality gate. That admission-only waiter was stopped before any Rust body (exit143/executed0). The scopes were separated: ordinary contract regressions retain canonical cargow resource admission; ANN4K/product/timing diagnosis retain the original quiet-host admission. No timing body was run under relaxed admission.

VERIFIED contracts: nextest `aa49b1b2-ab83-45ec-9770-970fa0e78b8a`, discovered/executed7, passed7, failed0, skipped321, exit0,0.278s. Both new Native success/refusal regressions, existing fileset install/carry diagnostics and region-append refusal/admission regressions ran. Canonical resource admission waited84.686s; compilation took2m51s. The captured relevant Rust/Cargo source snapshot had `source_drift=[]`. This is contract proof, not ANN4K quality or latency proof. HEAD remained `ade5fadc1a084fbb81eba831861e996081d80867` on shared dirty main.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-semantic --lib -E 'test(native_materialization_installed_region_observation_has_seal_owner) | test(native_append_rejected_descriptor_has_failed_seal_without_installed_observation) | test(fileset_install_trace_has_balanced_terminal_without_changing_result_or_work) | test(carry_trace_) | test(region_append_rejects_missing_map_root_without_changing_directory) | test(region_append_admits_map_key_before_installing_descriptor)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

Disposable contract state/output: `/tmp/qidx-native-ann-seal-contract-v79ve0c_/contract`. The queued prefix diagnosis was stopped before admission and before any Rust body to integrate B124's schema and product-oracle audit. The restart of this diagnosis retains source4096, prefix128, diagnostic180s, cap256MiB and original host admission. A valid current capture is still required for worker-stage cost attribution.

### B-ANN-ACCEPTANCE-ORACLE-AUDIT-124

Parallel source review covered the graph/ANN4K selected-strata oracles, Runtime selected views and overlay, and the trace producer/reader. The new Native Installed event exposed a schema assumption that all installed events had a null descriptor. Replaying its HNSW event with a real descriptor reference failed at the schema boundary. The schema now requires the descriptor for Native HNSW and keeps the file-based rule for legacy Flat/IVF. The v5 packed-event replay exercises accepted descriptor identity and rejected null. `.venv/bin/python -m pytest -q tools/ci/tests/test_causal_cost_timeline.py tools/ci/tests/test_causal_cost_diagnostic.py tools/ci/tests/test_causal_cost_native_ingest.py` ran651/651 PASS.

The Runtime WorkStatus observer accepted the same publication sequence for distinct tracked intents, although Catalog publishes at most one completion in each transaction. It also accepted malformed Claim identity and did not reconcile the highest observed completion with Latest. The shared checker now rejects duplicate sequences, wrong Claim identity, zero fence/lease, a regressed Latest, and a conflicting view identity at the observed highest sequence. The64-owner managed and4K readiness paths both consume this checker. The64-owner readiness probe also uses its existing absolute deadline.

Durable and overlay exact-path checks now require Exhausted traversal, exhaustive output, and zero stale hits for the fixture's complete bounded SourceLocal path. The original4K body now performs a second managed restart after change publication and overlay Close. It compares the changed view's full ANN query0 result with the validated pre-restart result, and checks replacement/deletion paths in both the original and changed historical views. Every added query uses the original1200s deadline. Ordinary Exact reads acquire request pins, so the restart check covers historical view availability. The active-pin/reclaim contract has separate owning runtime/kernel coverage.

Runtime `native_product_process` VERIFIED: nextest `7ca33d2c-8aeb-4d88-a81f-003ec1b4ac20` ran16/16 PASS in30.720s,38 skipped, exit0. This ran the new oracle counterexamples and actual64-owner daemon/reopen. The test files remained stable on the passing attempt. The broader captured cohort changed in Catalog read-only authority, Search Plane history, and one Lexical test, so this is a focused execution, with no frozen product claim. Earlier attempts ended with zero bodies at a restart error-conversion type mismatch and a denied unused insert result. Both were corrected before the16-body pass. Disposable state/output is under `/tmp/qidx-ann-proof-oracles-w11m0jva`.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-searchd-runtime --test native_product_process -E 'test(ann_quality::work_status::tests::) | test(exact_path_completion_oracle_rejects_capped_approximate_and_stale_results) | test(quality_result_oracle_rejects_wrong_score_and_owner_order) | test(overlay_quality_oracle_rejects_misordered_page) | test(selected_change_phase_) | test(sealed_region_oracle_rejects_wrong_coverage_binding) | test(product_daemon_seals_managed_ann_region_and_reopens_selected_view)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

The Kernel's managed ANN reopen test now requires the persisted sealed graph's Approximate path before/after a replacement and after re-opening storage, validates the held pin and observes its count1 before release and0 after. The real UDS Hybrid test observes exactly one pin on Open and after the query, zero after Release, and exact exhausted output with zero stale hits on the filtered SourceLocal path.

Kernel/Runtime integration VERIFIED: nextest `80e7e6e6-2bd8-4c6a-9684-4e473675efe0` ran4/4 PASS in98.203s,79 skipped, exit0. Both actual Native pinned reopens and both actual UDS Hybrid bodies ran. The changed test files were stable. The shared cohort recorded unrelated-to-these-changes Lexical source edits in `native/literal_blocks.rs` and `native/text_spool/posting_case_pages.rs`, so these are selected integration results.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-kernel -p quanta-index-searchd-runtime --test native_query_session_real --test native_index_uds -E 'test(managed_ann_region_reopens_after_owner_replacement_with_old_view_pinned) | test(managed_ann_second_seal_preserves_prior_selected_view_across_reopen) | test(hybrid_dense_only_original_view_partial_coverage_and_provider_refusal_keep_one_pin) | test(hybrid_spends_one_cumulative_work_budget_across_both_actual_readers)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

The original4K prefix trace was queued serially after those four tests released the resource lock. Its state and capture are outside the checkout under `/tmp/qidx-ann-materialization-rca-t9bu2q50/diagnostic-b124` and `trace-b124`. It uses the original quiet-host admission. The earlier full product failure and its dominant latency cause remain open, with the strengthened full4K product and changed-cohort ANN4K pending.

### B-ANN-PREFIX-OBSERVATION-RCA-125

Read-only inspection of the original prefix trace isolated the observer gap. The pre-fix capture cannot pass the canonical reader because eleven Native installed-region events carry the wrong stage and descriptor shape. The stored bytes were left intact. A diagnostic examination of packed begin/end records checked the worker ancestry and one capture clock. It found759 successful worker-owned `native_publish` lane3 spans and one aborted worker without a publication. Two publish spans under nonworker requests are outside this count. The harness observed39 exact public WorkStatus receipts in that run. The two observations have different clocks and boundaries, so the difference does not measure per-owner observation latency.

The prefix observer previously scanned source order, advancing at most two unready owner intents per page before waiting500–1,000ms. With the full180s available to it, a pending prefix can prevent it from reaching any of the completed intents beyond slot361. The fixture spends part of that same deadline on staging and SourceAdvance. The production queue orders equal-due initial work by intent digest and can defer work, while a prefix diagnostic needs the exact identities already completed. This explains how a run that published well over128 owner jobs could fail the observation target. It does not diagnose the full4K product deadline.

In that old trace the 760 worker intervals occupied138.638s. `native_coverage_save` was the largest direct repeated worker phase at55.771s across759 calls. Its direct reserve/install/confirm spans accounted for10.983/15.701/28.584s. Within those spans the SQLite COMMIT and write windows overlapped. Worker lane22 reported2,990,458,448 requested bytes, of which1,243,766,784 were in lane25 checkpoint descendants. Those are requested callback bytes. Neither they nor the inclusive spans establish completed device writes or make a durability step removable. The current Kernel `Materialize` coverage save calls `install_with_map` and the managed Storage/Catalog reserve/install/confirm path. A fresh valid capture is required to attribute current costs before changing that authority path.

The B124 prefix controller was still checking original admission when stopped for the candidate-selection repair. It executed zero bodies. No other process or validation lock was interrupted.

The ignored diagnostic now selects candidates from the live Catalog through a read-only connection and its created-view index. The created Source view, original operation, and exact intent digest inventory bind each candidate. Foreign or duplicate key, creator mismatch, and scoped terminal failure are errors. The connection closes before the existing SDK `CompletionProgress` authenticates the chosen128 distinct public receipts. It still enforces the original180s deadline and64/2 probe limits. Selection time and SDK receipt time are logged separately. This diagnostic does not change the4K product readiness path.

Two ordinary counterexamples exercise selection of128 completed rows beyond a3968-row pending source prefix and reject wrong-scope, duplicate or failed terminal records. One supplies Pending through the SDK-facing receipt checker after successful candidate selection and requires zero completed receipts. Nextest `07317ed2-c311-41b2-8554-5c3b78564ae3` executed11/11 PASS (7.777s,45 skipped, exit0), covering those two bodies and nine existing receipt regressions. The two changed test files stayed stable. Other shared Catalog, Contract, IPC, Lexical and Runtime inputs changed during the broader source capture, so the result covers this execution's selected test scope. The serial original-admission prefix controller and its output are under `/tmp/qidx-ann-materialization-rca-t9bu2q50/diagnostic-b125`. It has no body result at this checkpoint.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-searchd-runtime --test native_product_process -E 'test(prefix_catalog_) | test(ann_quality::work_status::tests::)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

The current coverage-save audit traced the Kernel owner-map preparation through Core `install_with_map` and Storage's original-base coverage CAS. An available prepared scope installs the reachable owner, searchable delta, and coverage DAG in one group. The standalone Storage grouping tests count a synthetic sink group. The real managed phase still owns reserve, physical install and confirm. The Catalog has separately changed its confirm-to-next-reserve transaction to reuse confirmed child authority within the same attempt and transaction. Its current regression checks10 fewer reads and152 fewer bytes with unchanged writes, plus rollback/reopen; B125's Runtime run did not execute that Catalog regression. These facts prevent attributing the earlier trace or a standalone group count to the current managed durability cost. The independent review of the prefix candidate cursor found agreement with the actual created-view index and public receipt identity. Current trace and full4K execution remain the deciding evidence.

### B-CATALOG-CONFIRM-REGRESSION-126

Nextest `3d9f0cdd-7a6c-4c23-94fe-6fb8f330a336` executed6/6 Catalog library bodies (1.198s,381 skipped, exit0). The cases cover exact trace registration, atomic confirmation and next reservation, protection reuse in the same transaction, replay refusal for invalid lifecycle, queue digest authentication and late rollback, and durable protection when the queue is absent. The wider source snapshot changed only in `quanta-index-storage/src/map.rs`, outside the Catalog dependency closure. This run verifies the selected Catalog tests.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-catalog --lib -E 'test(managed_attempt_phase_trace_preserves_off_work_and_exact_registration) | test(attempt_advance_commits_confirmed_child_and_next_reservation_together) | test(attempt_advance_reuses_protected_children_in_the_same_transaction) | test(managed_confirm_replay_still_refuses_corrupt_or_retiring_lifecycle) | test(confirming_child_queue_is_authenticated_and_late_failure_rolls_back) | test(confirming_child_without_queue_keeps_durable_protection_without_candidate)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
.venv/bin/python -m pytest -q tools/ci/tests/test_causal_cost_worker_scopes.py tools/ci/tests/test_causal_cost_native_triage.py tools/ci/tests/test_causal_cost_native_staging.py
.venv/bin/python -m pytest -q tools/ci/tests/test_check_host_contention.py tools/ci/tests/test_native_process_monitor.py
```

The trace diagnostics passed59/59 (0.21s) and the host admission and process monitor tests passed43/43 (0.41s). The serial order originally required the optional prefix and canonical replay before either quality body. Both quality bodies create independent fixtures. The prefix wait was deferred with zero Rust bodies started, and the existing quality runner was launched directly. It checks the original quiet-host gate before each body and preserves the fixed4096×64 input and product1200s deadline. The proof logs and state are outside the checkout at `/tmp/qidx-ann-materialization-rca-t9bu2q50/quality-b126`. Both current quality bodies are pending at this checkpoint.

### B-QUALITY-COHORT-AND-INDEPENDENT-TERMINALS-127

The B126 quality runner's hard-coded Rust inventory omitted resolved local dependencies and build inputs. It also exited after a failed ANN4K body, preventing execution of the independently selected product body. The outside-checkout B127 runner uses the repository's Cargo metadata closure and tracked/untracked file inventory, records deleted source paths, and captures manifest/config inputs before and after each execution. A second original preflight runs immediately before starting Cargo after source capture. The same cooperative resource lock, selectors, 4096×64 fixture, quality thresholds and product deadline apply. The runner records each terminal separately and attempts the next admitted body after either test result.

A mocked ANN4K failure preserved exit100 and still dispatched the product body with its own original preflight. Live read-only snapshots resolved16 local roots and1,037 paths for Semantic, and38 local roots and2,637 paths for Runtime, including an SQLite build script, the installed-artifact trust package and the tracked deletion. These checks verify orchestration and observed source inputs. The B126 runner was superseded during admission with zero bodies launched. B127 state and logs are under `/tmp/qidx-ann-materialization-rca-t9bu2q50/quality-b127`. Runner `passed` records the actual Nextest terminal. A current-source qualification also depends on reviewing `source_drift` and `resolution_drift` against the affected proof inputs.

Independent source review checked the normal ANN search cache path and interleaved member memory regression, and the mounted `ExistingFileIdentityV1` path through existing object I/O, Runtime revalidation and Semantic file projection. No new concrete P0–P2 was found in those paths. The current 4K body results and installed Linux recovery remain unverified.

The Core module snapshot matched the current source and the hexagonal boundary check passed. Source closure and module snapshot unit tests passed88/88 (44.24s).

```sh
CARGO_NET_OFFLINE=true python3 tools/ci/lint/check-cargo-modules-snapshot.py --packages quanta-index-core
just rust-hexagonal
.venv/bin/python -m pytest -q tools/ci/tests/test_benchmark_source_closure.py tools/ci/tests/test_check_cargo_modules_snapshot.py
```

### B-OLD-VIEW-ANN-SECOND-RESTART-128

The second restart exercised the changed view's ordinary ANN path and both views' exact-path lookup. It could pass if retained original-view ANN traversal failed after the changed publication while original exact lookup still worked. Initial and first-reopen ANN sweeps precede that publication. The second restart now requests original-view ANN query0 using the original deadline, checks it against the original source owner root and independent vector oracle, and compares its ordered owner page to the original sweep. The second restart has one additional search and reuses the captured source root. Existing exact-path checks and the changed-view ANN result comparison remain in the fixture.

The test also checks provider input after the reopened daemon exits. This interval starts after the prior daemon has stopped. Only the requested query input is legitimate, and persisted rows must not be re-embedded. The focused oracle accepts a cache hit and repeated valid query input and rejects a corpus row or a foreign query. Fixed input, the existing Q64 sweeps, recall floor, timeout and admission remain in effect. The fixture has retention disabled. An original exact view surviving this scenario establishes the selected historical view within that configuration.

`rustfmt --edition 2024 --check --config skip_children=true` and scoped `git diff --check` passed. Nextest `74fe3328-4008-43ae-bc45-3429f333380d` compiled the Runtime product target and ran5/5 focused bodies (0.154s,52 skipped,exit0), including the provider counterexample and the existing quality, overlay and deadline oracles. The edited test source hash remained `e37428e32a96dbf8395a23eda23b198cf46dba6e761d6b88615db5c9814dce83`. The independent second-restart requests execute only in the whole4K body, which remains pending alongside the current ANN4K. The old seed55 report attributes the IPC timeout to the remaining788ms of the original deadline. It does not identify a seed-specific or storage-path defect.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-searchd-runtime --test native_product_process -E 'test(restart_provider_oracle_rejects_reembedded_corpus) | test(quality_result_oracle_rejects_wrong_score_and_owner_order) | test(overlay_quality_oracle_rejects_misordered_page) | test(selected_change_phase_)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

`just rust-test-authority` passed on the current worktree. The product target is registered under `pr-workspace-nextest`, and the exact product test name matches the default exclusive pool override. Source review confirmed the fixed workload, independent reference and per-stratum denominators, original funding, SDK and worker deadlines, and unchanged 1200s product watchdog.

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

The static pass traced selected artifact cache identity and eviction, file provenance/ranking, compaction original-root CAS and failure atomicity, ExistingFileIdentityV1 wiring, and both original 4K test oracles. No additional confirmed P0/P1 or ANN production correctness defect was identified. The stale-heavy active-region hard cap and hypothetical allocator excess capacity were not promoted to defects without a supported contract violation.

The product test's last exact-path request already checked completion against the original watchdog. The remaining P2 validation gap was the final restart startup and cleanup/provider tail, where a successful return could occur after the original absolute deadline. `with_original_quality_deadline` now checks successful phase completion too. After `stub.finish`, final completion checks that same deadline. A deterministic regression exercises one nanosecond before, exactly at and after the deadline. Failed phase results retain their original errors.

Semantic Legacy DATA decoding moved from its facade to private `legacy/generation_data.rs`, retaining the public `legacy::with_decoded_generation_data` path. Lexical preview admission definitions moved from its facade to private `preview/budget.rs`, retaining the crate-visible reexports and feature gates. Both moved implementation bodies were checked byte-for-byte. No persisted or wire contract changed.

The pass began on shared dirty `857aa3e7a` and ended on shared dirty `c36e45e46`. VERIFIED static checks included module discipline (52 facades), Hexagonal boundaries, scoped rustfmt, diff checks, and `CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true just rust-cargo-modules` (Contract/Core/Kernel trees unchanged,exit0). The whole module-cycle checker failed because its crate-local inventory rejects Runtime's existing `../../../quanta-index-kernel/tests/common/native_catalog_profile.rs` reference. The referenced file exists. A scoped Semantic/Lexical scan also observes five existing source cycles. The two Semantic cycles are already present in HEAD. Those observations do not qualify the global cycle gate.

The focused deadline command below did not enter Cargo or any test body. Cooperative resource admission waited1800s and returned exit124. Its three selected tests are NOT_RUN. The five captured moved/helper inputs remained stable. Output is at `/Users/songmin/Library/Caches/quanta-index/ann4k-resume-20261117/STATIC-DEADLINE.log`. The original full4K controller remains the only quality runner, with repaired-cohort ANN4K and whole-product results still pending original quiet-host admission.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-searchd-runtime --test native_product_process -E 'test(=ann_quality::quality_completion_refuses_the_original_deadline_boundary) | test(=ann_quality::selected_change_phase_refuses_expired_original_deadline_before_work) | test(=ann_quality::selected_change_phase_preserves_live_original_deadline_and_result)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

On 2026-11-18 the same command acquired the original cooperative lock and ran 3/3 PASS (Nextest `fd37295b-95a1-4b65-bb13-499d42ebd4e9`,0.066s,56 skipped,exit0). Three captured test inputs retained their hashes. The shared dirty checkout ended at `40cd6dc5b`. Output is at `/Users/songmin/Library/Caches/quanta-index/ann4k-resume-20261117/STATIC-DEADLINE-retry.log`. This verifies the watchdog helpers. The full product and ANN4K selectors still await the original admission in the existing controller.

### B-SEMANTIC-MODULE-CYCLES-136

`native::recipe` now owns provider-independent recipe identity, limits and output-policy digests. Embedding and certified chunk decoding both consume that module. The public `native` recipe paths remain stable. The recipe fingerprint body is byte-identical to the prior implementation, and all hash domains and validation limits were retained.

`native::embedding::lexical_eligibility` now owns the selected lexical set and its retained backing. Lexical scope and overlay both consume it. View and entry access is read-only. The constructor, duplicate refusal and overlay membership checks retain their prior bodies. Existing partial raw-vector exact-path behavior consumes the same owner cap in exact and approximate traversal, and cap truncation is visible. The file path continues to apply eligibility before the admitted owner window.

The repository module-cycle checker on archived `a68062c26b3c1797d321e4ad3f8beca7508f5d99` Semantic source found both prior cycles. The same checker on current Semantic source found zero (exit0). Module discipline (52 facades), Hexagonal boundaries, `just rust-test-authority`, the guarded Contract/Core/Kernel module snapshots, scoped rustfmt and diff checks passed. The global cycle inventory failure recorded in B135 remains outside the Semantic ownership change.

The focused proof uses the existing recipe goldens and lexical scope tests. The first attempt reached the compiler but exited101 because the Legacy test fixture still imported `generation_contract` and `manifest` through the old facade. The fixture now imports both from their defining modules. Before the first build reached Semantic, the shared checkout also supplied a missing `entries()` accessor at the durable owner count call site.

The same selector then executed 5/5 PASS (Nextest `1c7a6c91-2e66-4c71-ad42-53f2106d2c3b`, 0.203s,325 skipped,exit0). The 11 recorded Semantic inputs retained their hashes between retry submission and terminal. The shared dirty checkout ended at `ea5137dd4`. The run proves the moved recipe fingerprints and limits, same-view overlay membership, duplicate rejection and forged eligibility refusal. Terminal output is at `/Users/songmin/Library/Caches/quanta-index/ann4k-resume-20261117/SEMANTIC-CYCLES-retry.log`.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-semantic --lib -E 'test(pure_text_recipe_) | test(lexical_scope_tests::)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

### B-CARRY-IMMUTABLE-MEMBER-REUSE-137

The carry could reopen and decode the same artifact after authenticating it in
delta and keeping it for the final region. An exact owner and immutable member
reference in a later source now reuse that retained result within the same
attempt. Each source still accounts for its declared rows and checks current
selected coverage. Different references still authenticate before stale or
conflict handling. The final map CAS uses the same original roots.

The 192-owner multi-page carry fixture permits one physical read of the
repeated artifact during carry. Its original final-map, prior-view and write
refusal assertions also run. The unchanged code failed on the second physical
read (Nextest `bd179f6d-5606-4b17-bf63-9a5eea2dbd73`, 1 body, exit100). The same
selector passed 1/1 after repair (Nextest `43af013a-1f10-4791-9362-b9453f311dbe`,
0.190s, exit0). The repaired implementation, test, three adjacent ANN source
files and `Cargo.lock` kept their captured hashes through the run.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-semantic --lib -E 'test(=native::embedding::tests::ann_product_tests::maintenance::consolidation_multi_page_carry_installs_one_final_member_map)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

The other five consolidation bodies passed in one run (Nextest
`c17dc3b6-ee24-421d-9476-c2761c41cce1`, 5/5, 12.366s, exit0). They cover sparse
diff after member replacement and deletion, late failure atomicity, foreground
row bounds, a wrong delta owner count, and region-capacity carry. The six
captured inputs above remained stable on shared dirty `2cb1c117e`.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-semantic --lib -E 'test(native::embedding::tests::ann_product_tests::maintenance::consolidation_) & not test(=native::embedding::tests::ann_product_tests::maintenance::consolidation_multi_page_carry_installs_one_final_member_map)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

### B-MODULE-CYCLE-INVENTORY-138

The workspace cycle checker stopped before building the Runtime production
graph. Runtime's `#[cfg(test)]` dense install module imports a Catalog profile
fixture from the Kernel workspace member. The checker now validates that a
sibling member's fixture exists, then applies the declaring module's test-only
scope after resolving aliases. Missing fixtures, paths outside the workspace
and cross-crate production imports still fail. The checker never reads the
sibling fixture into the importing crate's production graph.

A second inventory failure exposed a `pub(crate)` path declaration whose source
was already under a renamed parent. The path scanner now uses the same
visibility pattern as ordinary module declarations and resolves the physical
source through the recorded module aliases. The regression asserts the actual
production cycle through the visible child.

The cross-crate test fixture and the visible alias both reproduced the original
inventory error. `.venv/bin/python -m pytest -q tools/ci/tests/test_check_module_cycles.py`
then passed 40/40, including production escape and missing source refusals.
Scoped Ruff lint, Ruff format and diff checks passed. The workspace check uses
the repository's existing cycle baseline.

`just rust-module-cycles` completed the workspace graph and returned exit 1
with 16 unbaselined groups across Catalog, Contract, Core, IPC, Lexical, Search
Plane, Runtime and Searchd. It reported no Semantic group. The Core group
includes the existing read boundary: `existing_file` consumes `ObjectReadIoPort`
and the metadata visitor in `ports` names `ExistingFileReadFailureV1` from
`existing_file`. This is a source dependency finding. The ANN4K and SDK product
selectors still own behavioral acceptance.

### B-EXISTING-FILE-ERROR-OWNERSHIP-139

The admitted file reader requires `ObjectReadIoPort`. The metadata visitor's
callback and return signature previously reached back into that reader for
`ExistingFileReadFailureV1`. The existing `index::error` module now owns the
shared error, and the metadata port and tree reader import it from there. The
public `index::existing_file::ExistingFileReadFailureV1` path re-exports the same
type. The enum and its conversion, display and source implementations match
their original bytes.

The current repository cycle analyzer on Core reproduced both directions. The
same graph assertion then passed with the admitted read port edge present and
the metadata port's reverse edge absent. `index::existing_file` is outside
every Core cycle. The original typed-error and file-identity paths are exercised
by the existing `legacy_active_repomap_verifies_real_objects_without_reconciliation_writes`
Runtime body.

The whole-workspace cycle check completed with exit 1 for the remaining groups.
The Core port group no longer contains the file reader. The guarded Core module
snapshot records the relocated type. Its repeat reports drift only in the
concurrent `protected_source_routes` tree. Module discipline and hexagonal
checks passed. The workspace error-shape gate reports the missing Error
implementation on Contract's `NativeCutoverSourceJoinErrorV1`.

The first runtime test invocation exited 101 before any test ran. The Lexical
dependency's cold posting tournament had attempted `checked_add` on a checked
sum's `Option<usize>`. The current shared source chains the sums through
`and_then`, so the same selector was submitted again through the existing lane.

The retry executed 1/1 PASS (Nextest `67ad8fe3-d656-41b0-813c-23ae94417c77`,
1.008s,632 skipped,exit0) on shared dirty `d7b1b24a6`. The four edited Core
sources, RepoMap reader, Runtime test source and `Cargo.lock` retained their
captured hashes. The body checks admitted identity revalidation, both typed
error variants, oversize and corrupt input refusal, changed file and UUID
refusal, no rematerialization of missing data, and read-owner retirement.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-searchd-runtime --lib -E 'test(=publication_sqlite_vfs::tests::read_only::legacy_active_repomap_verifies_real_objects_without_reconciliation_writes)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

### B-EXISTING-TREE-COMMITMENT-OWNERSHIP-140

The generation module delegates layout reads to `index::existing_tree`. The
reader depended back on the generation module for the sealed file commitment
and mismatch types. The private `tree_commitment` module now owns both types.
The crate root and `domains::generation` re-export those definitions. The
definitions, display and error implementations match the original bytes. Both
verifiers still execute their original code.

The scoped Core graph assertion reproduced the reverse edge before the move
and passed after it, with the generation-to-reader edge present. The canonical
workspace graph dropped the entire `domains::generation, index::existing_tree`
group and returned exit 1 for 15 remaining groups. The guarded Core snapshot
was updated for the two moved definitions. Its repeat reports only the
concurrent `protected_source_routes` tree drift. Scoped rustfmt, diff, module
discipline and hexagonal boundary checks passed.

The existing Core tree suite executed 10/10 PASS (Nextest
`028c711b-5a15-48e2-8901-56e8fc163e67`,0.198s,366 skipped,exit0) on shared dirty
`2f09e5579`. The four edited Rust files and `Cargo.lock` retained their captured
hashes. The bodies cover the ordinary layout, digest and mismatch ordering,
one-byte-overrun refusal, resumable scrub, reader release and inherited inode
handling. The admitted Runtime FD fixtures are Linux-only and were not run here.

```sh
CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-core --lib -E 'test(/^domains::generation::tree_commitment_tests::/)' --no-tests fail --test-threads 1 --no-fail-fast --failure-output immediate --success-output immediate
```

### B-MODULE-ALIAS-OWNERSHIP-141

The B138 alias resolver could transfer a source file from its first explicit
module to a second owner. For two inline sibling declarations of the same
file, the checker dropped the first module and accepted a real dependency
cycle. The earlier checker refused the second declaration. The new ownership
check keeps parent alias resolution and refuses a second distinct owner for
that physical source, naming both logical modules.

The regression failed before repair because the checker returned no cycles and
raised no error. Removing the ownership check from the repaired implementation
reproduced the empty result for the same fixture. The final
`.venv/bin/python -m pytest -q tools/ci/tests/test_check_module_cycles.py`
executed 41/41 PASS. Scoped Ruff lint, format and diff checks passed, as did
`just rust-test-authority`. `just rust-module-cycles` completed the workspace
graph and exited 1 on 16 unbaselined cycle groups, including the Harness
snippet group now present in the shared worktree.

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
SQLite dependency. [LanceDB audit and build RCA](../../../bugbash/nov-20/findings.md#이번-확인-결과)

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
