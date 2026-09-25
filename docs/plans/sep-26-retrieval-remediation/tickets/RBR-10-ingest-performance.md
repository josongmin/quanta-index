# RBR-10 — Semantic owner delete 비용과 replacement 안전성

- 우선순위: P2. 계측/조건부 구현: `NOT_RUN`. 선행: RBR-01.
- 확인된 사실: 임베딩은 이미 window batching, storage는 scope별 delete 후 batch append. 비용 비중은 아직 미측정.

## 파일·함수

- [batch.rs](../../../../benchmarks/retrieval/src/batch.rs): `semantic_scope` — chunk마다 owner scope.
- [semantic_derive.rs](../../../../crates/quanta-index-search-plane/src/semantic_derive.rs): `embed_window` — 기존 batch 호출 유지.
- [semantic/build.rs](../../../../crates/quanta-index-semantic/src/build.rs): `apply_scope_stream`, `apply_window`, `delete_replace_scope_rows`, `delete_replace_scope_memberships`, `append_window`, seal/promotion.
- semantic `build/tests.rs`와 기존 streaming/fault/restart fixtures.

## 작업 순서

1. fresh 및 delta ingest에 대해 owner/window 수, delete 호출/commit 수, embedding/delete/append/membership/seal/activate 시간을 따로 기록한다. filesystem/fsync 포함 여부를 명시한다.
2. delete가 실질적 비용이라는 측정이 있을 때 첫 후보로 bounded-window delete predicates를 병합한다. owner ID escaping과 SQL 길이 상한을 지키고 embeddings와 membership semantics를 함께 검증한다.
3. fresh-empty skip은 별도 후속 후보다. 저장소 authority가 truly empty generation, inherited rows 없음, 이전 streamed writes 없음, 해당 owners 최초 등 필요한 조건을 증명할 때만 허용한다. caller flag나 generation 번호만으로 생략하지 않는다.
4. append batching을 새 해결책인 것처럼 중복 구현하지 않는다. state root 재사용·dirty reset·durability 삭제·seal 생략으로 시간을 줄이지 않는다.
5. raw logical row set과 read-after-activation이 baseline과 같음을 확인한 후 development fresh roots에서 ingest latency/resource를 비교한다. 삭제 비용이 작으면 최적화하지 않고 측정 결과로 종료한다. 한 후보만 실험 profile에 고정해 RBR-12 최종 조합에 제출한다.

## 필수 반례

- fresh, inherited base, 기존 owner 교체, tombstone, empty scope, 반복 window/replay.
- reopened unsealed generation, append failure after delete, membership inconsistency, 중간 cancellation.
- crash 전후 seal/promotion, restart integrity, 이전 active generation 보존, new generation의 원자적 가시성.
- owner 수가 큰 window와 adversarial ID에서 bounded predicate size/escaping.

## 완료

독립 expected row-set oracle·semantic integration·영향받은 daemon scenario가 통과하고 [TEST-PLAN](TEST-PLAN.md)의 fresh-root time-to-searchable 효과 및 품질 guard를 최종 조합에서 충족해야 기본 정책을 바꾼다. 출력에는 미선택 후보와 유지 이유도 남긴다. durability 계약은 성능 목표보다 우선한다.
