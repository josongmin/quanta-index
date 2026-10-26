# W2-15 — Overlay API 격리와 save ACK/frontier handoff

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-15)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R25,R26,R28 / NI-08. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: bounded peer/lease overlay·path-bound save 원 CAS identity·searchable frontier/text gate·SDK/daemon/config·atomic corpus capture와 동일 Native text evaluator의 mask/merge. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

ephemeral overlay와 durable query를 session/document/revision identity로 격리하고 save와 Fast/Deep 결과의 동일 snapshot/provenance를 유지한다.

## Acceptance

- 실제 headless SDK→daemon→engine에서 session/workspace/document/revision 격리와 monotonicity·empty/Excluded·save/current searchable frontier handoff를 검사한다.
- durable mask/merge와 virtual corpus BM25/candidate ranking, atomic Fast/Deep snapshot/provenance를 동일 query gold로 검사한다.
- typing/save/close/reopen/workspace switch/delete/late Deep/다른 session save race 및 shared subscriber cancel에서 stale 적용을 거절하고 원 budget/deadline을 보존한다.
- UX01,03–06,09,10,12의 engine 계약과 request-to-result/save-to-search latency를 검증한다. GUI render는 외부 consumer 범위다.

## Query integration contract

- `NativeOverlayTextQuery` carries the selected durable view, authenticated overlay session and optional expected corpus identity. Capture every owner/revision/deletion/retired watermark under one lock; validate the same immutable snapshot after execution and before reply assembly completes.
- `TextOverlayReadDocument` borrows the captured immutable Arc payloads. Ephemeral contributions reuse the durable owner-build token batches. Subtract masked admitted contributions from document/token/term-DF statistics and combine candidates before Boolean/phrase evaluation and top-k ranking.
- Unsaved hits carry `Overlay { revision }`; immutable hits carry the actual selected source reference. The durable member artifact and captured corpus stamp are both explicit. Do not fabricate source objects or mutate the durable member for a query.
- `QueryEffortStream.overlay` selects an authenticated session and optional expected corpus identity. Capture once before Fast; validate the same snapshot before/after each stage. Overlay Fast, overlay Deep and EffortDone carry the same full corpus stamp; SDK rejects mixed durable/overlay streams or changed view/revisions.
- SourceUtf8 Deep uses the configured embedding provider and the existing file-local chunk planner. It masks selected durable owners before exact heap top-k and retains unsaved revision/chunk provenance. Projection/certified inputs without producer authority refuse before provider calls. Fast-only requests do not allocate Deep request rows.
- Overlay semantic `scanned_owners` counts durable rows visited plus active overlay rows, including deletion masks; `skipped_stale` counts stale durable rows. One global owner-scan cap covers both scans. These are work counters, so a masked durable owner and its overlay revision may both contribute.
- Corpus validation establishes the captured snapshot at the server validation point. A mutation can race with transport delivery; equality of Fast/Deep/Done stamps does not prove that the editor is still current when applying a result. The consumer must compare the complete session/corpus/revision fence under its own current-state guard before applying a late result. Do not hold the overlay mutation lock across socket writes.
- Runtime validates overlay chunk UTF-8 coordinates and SHA256 input digest against captured text before emission. Original cancellation/deadline and provider custody remain in the retained query session. These new paths require the consolidated library and actual UDS regressions before acceptance.

## Source entry points

- [query.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-core/src/index/query.rs)
- [update.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-kernel/src/update.rs)
- [native_overlay_query.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_overlay_query.rs)
- [text_overlay.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/native/text_overlay.rs)
- [native_effort.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_effort.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
