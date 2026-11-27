# 02 · ItemIndex의 선택 경로 DELTA와 게시 소비

## 현행 판정 · 2026-11-26

**제품 코드 미완료.** Semantica main `86ecd07699d`에서 일반 Original DELTA seed는 committed Query Source의 경로 slice를 Current 안에서 빌려 전체 경로 문자열 복사를 피한다. 이미 검증된 target census와 소수 affected upsert를 가진 경우 selector는 누락 경로만 찾는 sparse 경로를 사용한다. 그러나 유지 파일마다 predecessor ItemIndex와 QE 결과를 다시 인증하고, Evidence 개수·행 생성, seal 값 대여, terminal map retain과 V9 memo capsule 작성이 전체 map을 순회한다. 이는 최초 전체 입력과 각 소비자의 별도 비용이다. [현재 코드·비용 원장](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-26)

**착수 경계.** 기존 `증분 SDK · 변경 경로로 다음 세대 생성` 담당이 공유 `native_delta_seed_v3.rs`의 witness 호출 수리를 `077bc3cdf56`에 합류했다. 이 변경을 기준으로 QE의 target Current 재사용 권위와 영향·삭제 집합 계약부터 시작한다. [04](04-ifx150-source-publication-integration.md)의 후속 컴파일 수리가 같은 함수에 겹치는 경우만 기존 SDK·Runtime 담당이 조정한다. ≤16 sparse selector와 경로 slice 대여는 기존 구현을 사용한다.

| 경계 | 이미 구현된 것 | 남은 코드·계약 |
| --- | --- | --- |
| seed·selector | `native_delta_seed_v3.rs`는 committed Source 경로를 빌리고, `item_index/native_delta_v3.rs`는 같은 target census의 affected 위치를 찾는다. ≤16 affected의 생산자 sparse 누락 경로 처리는 존재한다. | 유지 파일별 QE 재검증과 seed의 전체 범위 결속을 없앨 새 snapshot 사용 권위가 없다. 영향을 받지 않은 결과를 단순 Arc 재사용만으로 새 세대에 게시할 수 없다. |
| Evidence·seal | map funding이 같은 발급 그룹일 때 header 중복 순회를 일부 생략한다. | `native_evidence_v3.rs`의 `EvidenceMapCount`와 `EvidenceMapRows`, seal의 checked 값 대여는 별도 전체 순회다. 개수만 없애도 행 생성 비용은 남는다. |
| terminal·memo | `native_terminal_publication_v3.rs`는 봉인된 ItemIndex map을 retain한다. `native_memo_capsule_v4.rs`는 완전한 managed map을 요구하고 V9용 row를 캡처한다. | 변경 행만 소비하려면 이전 map·새 snapshot·삭제/rename·V9 참조를 결속한 합성 권위와 새 wire/reader 계약이 필요하다. 기존 complete-map 계약을 조용히 약화시키면 안 된다. |

Semantica 진입점: [seed와 유지 파일](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/sdk/search_builder/source_bound/snapshot_collector/native_delta_seed_v3.rs:192), [sparse selector](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/sdk/search_builder/source_bound/snapshot_collector/subqueries/item_index/native_delta_v3.rs:893), [Evidence 전체 count](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/sdk/search_builder_retrieval_helpers/native_evidence_v3.rs:364), [terminal retain](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/native_terminal_publication_v3.rs:454), [memo capsule](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_owner_pipeline_v1/native_memo_capsule_v4.rs:87).

## 구현 경계

1. **새 세대 재사용 권위.** QE가 predecessor의 각 결과와 의존성·query key·snapshot·Source/Output Current를 대조해, 정확히 영향 없는 결과만 target snapshot에서 사용할 수 있다는 증표를 발급한다. import/graph 영향, 언어·파일 신원, rename의 old/new 경로, delete 및 producer alias를 포함한다. 영향 범위가 불명확하면 전체 재계산 또는 typed 거절로 보낸다.
2. **선택 합성.** 인증된 predecessor map에 영향·변경·삭제 경로의 결과만 적용하고, 경로 중복·누락·foreign map·다른 발급 그룹을 거절한다. 같은 target census의 전체 cardinality 및 선택된 경로 membership은 유지하되, 무변경 행의 QE 재실행과 전체 key 복사를 피하는 persistent/structural sharing이 필요하다. 새 결과와 기존 결과의 allocation·work·취소 수명은 각각 원 owner에 귀속한다.
3. **소비자 이관.** `native_terminal_publication_v3.rs`의 seal/retain, `native_evidence_v3.rs`의 count/row, `native_memo_capsule_v4.rs`의 V9 row와 복원 reader가 합성 map을 같은 권위로 읽도록 연결한다. 전체 output materialization이나 모든 EvidenceHit 반환이 공개 계약상 필요한 경우 그 O(N)은 별도 비용으로 보고하고, 원문·QE 재계산 0과 구분한다. wire 변경은 구형 V9/cold reader와 함께 버전 결속한다.
4. **격리 후보 판정.** `codex/itemindex-evidence-census-20261125`의 `0fcfb3d35c2b`·`e7a6addf1fe`는 생산자 종류별 count를 반환해 `EvidenceMapCount`만 줄이는 후보이며 main에 미합류다. 생산자 타입 검사는 exit 0이고 소비자 QBC는 컴파일 exit 101, 본문 0이다. [생산자 결과](/Users/songmin/Library/Caches/semantica-codegraph-v2/itemindex-symbol-check.MZQFfEGD/result.json) · [소비자 결과](/Users/songmin/Library/Caches/semantica-codegraph-v2/itemindex-runtime-census.daD1U6zJ/result.json) 현행 입력과 feature로 재검증한 뒤 합류 여부를 정한다. 이 후보를 선택 DELTA 전체 완료로 취급하지 않는다. [후보·실행 이력](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#남은-실행-검증)

## 실패 사례와 단계별 인수

- **정확성:** create·replace·delete·rename, import와 graph 영향, 같은 경로의 다른 repository, 동일 점수·동일 key, 0/1/16/17/128/129개 affected, foreign/stale QE 권위, 혼합 managed group과 cold V9 복원을 검증한다. 각 target의 ItemIndex 행·Evidence 개수/순서와 공개 결과를 동일 최종 파일의 독립 FULL로 대조한다.
- **비용:** 원본 경로 문자열 복사, target census 재검증, 유지 predecessor 파일 방문, seed/map membership, Evidence count/rows, seal value walk와 memo row 캡처를 각각 계수한다. 필요한 전체 DTO/wire 행은 0회 조건에서 분리한다. 현재 [IFX150 비용 단언](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/ifx150_restart_recovery_test_support_v1/native_initial_full_recovery_v3/native_checked_delta_v3/single_edit_cost_v3.rs:1042)은 이 범주를 모두 0회로 요구하므로 소비자 계약 이관 전에는 통과 주장할 수 없다.
- **재개·게시:** 같은 attempt·Q operation의 ACK/G2, Source CAS 1회, old view 보존, Q 응답 유실과 새 PID 복원에서 map/V9/검색 결과를 확인한다. 변경 후보의 정적 검사나 compile 0본문은 제품 수용이 아니다.

**비중복 소유권:** Semantica Runtime/QE·SDK publication owner가 구현하고 기존 INC-01/INC-03 및 IFX150/QBC 레인에서 판정한다. Store 선택 검증은 [01](01-store-sparse-warm-commit.md), Q Lexical/ANN 자체는 각 기존 owner가 맡는다.
