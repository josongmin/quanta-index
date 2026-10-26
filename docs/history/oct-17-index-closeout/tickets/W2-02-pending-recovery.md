# W2-02 — pending 저장/claim·startup inventory의 Native 소유권

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


상태/담당/다음 작업은 [SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-02)만 갱신한다. 이전 W2-02의 **미연결 caller만** 남긴 티켓이다.

## Remaining scope

- pending 최초 carrier/JSON persist·claim의 recursive DTO/seed를 기존 Native Original 비용·수명 소유자에 연결한다. canonical serializer/validator·오류 순서·동일 SQL/Immediate transaction을 공유한다.
- startup marker inventory의 genuine Original funding과 실제 startup caller를 연결한다. Native 분기 이전 ordinary replay loader 도달과 격리/복구 권한을 대조한다.
- INT 기존 issuer/snapshot/cost의 [독점 hunk](../../../plans/oct-17-index-closeout/tickets/WRITE-OWNERSHIP.md)를 소비한다. pending root/jobs/delta/SQL은 issuer, taxonomy seed는 snapshot, semantic plan/unit seed는 cost, 공용 parent/COORD는 통합자다.

## Reuse boundary

게시 준비·query materialization·intent·pending→V9 transition·Terminal/checkpoint/ACK·reader pin은 연결 저장된 코드다. **marker removal도 admitted caller로 연결됐다.** 이를 신규 구현으로 배정하지 않고 W2-19에서 실제 실행 수용한다. fresh Native FULL direct begin을 ordinary-only clone 결함으로 다시 등록하지 않는다. driver/SDK 전체 비용 재작성은 범위가 아니다.

Contract의 job/delta·taxonomy·semantic nested seed와 공유 ordinary/Native field body는 저장된 구현을 재사용한다. Runtime의 scalar policy·기존 byte matcher와 공유 formatter·검증 직후 불변 wire view도 다시 만들지 않는다. 이 재사용 경계가 최초 Native carrier/JSON/row backing의 비용 수용을 대신하지 않는다.

## Source and current gap

S root: `/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates`.

- [pending_aggregate_replay_v2.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/indexing_machine_v2/store/pending_aggregate_replay_v2.rs): 최초 `serde_json::to_string`, claim/decode의 `from_str`·재직렬화와 backing 수명을 대조한다.
- [source_bound_aggregate_coordinator_v2.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/assembly_data/source_bound_aggregate_coordinator_v2.rs): `reconcile_issued_transfer_markers_v2`와 admitted marker removal을 구분한다.
- [prepared_commit_artifact_v2.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_commit_artifact_v2.rs): inventory·canonical immutable persistence·공통 marker 삭제 body를 재사용한다.

## Current implementation boundary

- Native fence 비교·bounded scalar/schema 거절·공유 identity formatter·불변 wire view는 저장된 구현이다. 이를 다시 설계하지 않는다. 최초 recursive carrier/JSON·SQL row backing과 manifest normalization clone은 아직 남아 있다.
- 기존 replay owner integration target에 동일 pending persist/retry·canonical corruption·restart takeover 함수를 연결했다. `lib(test)` mock 전체를 복제하거나 테스트 전용 저장/검증 경로를 만들지 않는다.
- 선택적 ImportGraphPort discovery 호출은 해당 feature guard 안에 둔다. traversal/trait body와 enabled Native refusal을 보존한다. 전용 SQLite/replay와 SDK Native의 실행 판정은 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md#native-required-readback-code-checkpoint-2026-10-15)을 소비하고 이 문서에서 별도 상태를 만들지 않는다.
- startup의 `SearchOwnerRequestContextV1`은 현재 문자열·generation·deadline·publication authority만 보유한다. 실제 Original/TargetedSource 공급 경계를 먼저 연결해야 하며, inventory에서 새 예산을 발급하거나 ordinary loader를 우회 성공으로 취급하지 않는다.

## Pending root receiving map

아래는 현재 10개 root 필드의 소비 경계다. nested 전체 수용 완료를 뜻하지 않는다. `ReplayNative*DecodeDataV3`는 기존 Contract의 외부 DATA receiver이며 별도 DTO/codec이 아니다.

| Canonical field | Reuse | Remaining actual connection |
| --- | --- | --- |
| `schema_version` | inline u32·공유 validator·bounded Native 거절 | Native outer visitor의 동일 version/duplicate/missing 검사 |
| `pending_plan_v1` | `ReplayNativeIndexDeltaPlanDecodeDataV3`, 기존 job/delta nested seed | coordinator의 최초 plan clone·outer receiving |
| `pending_attempt_v2` | Runtime의 공유 scalar validator/serializer | 실제 문자열·private plan ID backing·partial DATA receiving |
| `pending_owner_fence_digest_v2` | caller의 admitted canonical material/hash·paid String | persist/claim의 새 backing과 그 수명 연결 |
| `source_join_v1` | `ReplayNativeAnalysisDeltaRefDecodeDataV3` | 최초 analysis-delta clone·outer receiving |
| `product_scope_v1` | `ReplayNativeSearchProductScopeDecodeDataV3` | 최초 scope clone·outer receiving |
| `prepared_handoff_set_v1` | `ReplayNativePreparedHandoffSetDecodeDataV3` | 최초 handoff clone·outer receiving |
| `prepared_artifact_ref_v3` | `ReplayNativePreparedArtifactRefDecodeDataV3` | 최초 ref clone·outer receiving |
| `frozen_request_v2` | `ReplayNativeFrozenRequestDecodeDataV3`, 기존 FULL의 admitted frozen producer | change-event pending의 ordinary frozen 생성 연결 |
| `replay_locator_v9` | Runtime의 공유 scalar validator/serializer | 실제 세 문자열 backing·partial DATA receiving |

JSON encode·row read·claim은 이 root 밖의 별도 backing 경계다. decoder/검증 오류와 실패 candidate도 최고 Source 밖 DATA에 남기고, 성공 DTO 이동 뒤에도 같은 Original group을 최종 drop까지 유지한다. manifest 정규화의 clone을 제거할 때도 기존 정렬 순서·digest domain·coverage 검증을 공유해야 한다.

## Acceptance

- ordinary/Native canonical wire·고정 digest·variant/duplicate/missing/foreign 거절과 오류 순서를 유지한다. admission 거절 전 allocation/write0, 정상 backing 최종 drop 후 환급을 독립 관측한다.
- claim의 동일 transaction·exact request/ref/frozen/target과 원 typed cause를 보존한다. crash/restart·ACK loss에서 recollection·deadline/budget reset·맹목 재송신으로 성공을 만들지 않는다.
- startup의 inactive-scope loader0·foreign/손상 row 격리·다음 정상 row 진행, 실제 Original의 read/decode/work/drop를 검사한다. 동일 admitted custody 재사용과 mandatory fresh 읽기는 구분한다.
- 기존 genuine Original/record/marker fixture와 owning QBC로 W2-19 영향 배치에 합류한다. 새 codec/issuer/runner는 만들지 않는다.

의존: SDK FULL producer 재구현 없이 독립 진행 가능. Contract/COORD parent는 단일 writer가 결합한다. 과거 source별 실패/수리 저장과 새 동작 성공은 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md#native-required-readback-code-checkpoint-2026-10-15)에서 구분한다.

## Implementation packet

- **착수:** 기존 INT issuer가 pending 최초 생성→serialize→SQL persist→claim→decode→V9 이동의 actual caller/할당 backing을 대조한다. 저장된 recursive seed 변경을 먼저 소비하고 빠진 nested 필드만 보완한다.
- **구현 순서:** 저장된 Contract job/delta·taxonomy·semantic plan/unit seed를 소비하고 Runtime pending root·JSON/SQL·claim의 미연결 backing만 보완한다. seed 자체를 완료 항목에서 재개발로 되돌리지 않는다. startup inventory는 기존 Original의 실제 upstream 공급부터 연결한다. COORD/Contract parent는 INT 통합자가 한 번 결합한다.
- **정적 완료:** canonical bytes/validator·같은 Immediate transaction·원 typed cause, 부분 실패 cleanup과 backing-before-grant drop를 producer→consumer 전체에서 확인한다.
- **반환물:** exact 함수/hunk 목록, nested 필드 누락0 대조표, 재사용한 seed, 영향 fixture/selector와 미실행 검사. 이 자료는 해당 티켓 본문/통합자 인계에만 반영한다.
- **후속:** W2-19의 genuine Original·재시작/거절/환급 배치에 합류한다. SDK FULL 완료를 기다릴 필요는 없지만 공용 COORD 함수 충돌은 인계 후 직렬 처리한다.
