# RBR-10 — Semantic owner delete 비용과 replacement 안전성

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

- 구현: `build.rs::build_stream_reported`가 내부 delete/append 등 stage report를 만들지만 기본 `build_stream`은 보고값을 폐기하고 공개 SDK/runner까지 전달하지 않는다. delete 최적화나 제품 기본값 변경은 아직 없다.
- 검증: `/private/tmp/qi-rbr-closeout.FMVGsN/semantic-ingest.log`의 내부 `ingest_stage_report_` 4 tests는 local 통과했다. 현 frozen-source semantic integration, 공개 stage identity, fresh/delta raw 측정, row-set/fault/restart oracle은 `NOT_RUN`; 내부 테스트를 공개 propagation 증거로 사용하지 않는다.
- 잔여: semantic owner→search-plane/SDK→runner의 stage provenance를 먼저 연결하고 비용 비중을 재현한다. 유의한 delete 비용이 확인된 경우에만 bounded predicate 한 후보를 시험하고 durability/재시작/원자적 visibility 및 fresh-root 시간을 검증한다. 아니면 유지 결정. [현재 전수 판정](CURRENT-AUDIT.md).
- 구현 경계: `BatchPublishReceipt`는 replay 가능한 durable acknowledgement라 비결정적 elapsed/call count를 여기에 넣지 않는다. `SemanticScopeStreamBuildPort`의 tally/report, search-corpus publish 호출, IPC/SDK의 **별도 transient observation** 및 runner sidecar를 동일 batch/generation/request identity로 연결해야 한다. 전역 last-report 캐시나 프로세스 누적 metric은 병렬 publish에서 batch provenance를 보장하지 못한다. 이 경로와 embedding·seal·activation 시계의 포함 범위를 정하기 전에는 내부 `build_stream_reported` 수치만으로 ingest 병목을 단정하지 않는다.

- 우선순위: **P1 공개 계측 연결 / P2 조건부 최적화**. `604149ed`에서 내부 report 폐기 경로를 재확인했다. 공개 경로·실측·조건부 결정은 `NOT_RUN`. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-01 observation policy; 공개 전달 설계는 즉시 가능하다.
- 확인된 사실: 임베딩은 이미 window batching, storage는 scope별 delete 후 batch append. 비용 비중은 아직 미측정.

## 파일·함수

- [batch.rs](../../../../benchmarks/retrieval/src/batch.rs): `semantic_scope` — chunk마다 owner scope.
- [semantic_derive.rs](../../../../crates/quanta-index-search-plane/src/semantic_derive.rs): `embed_window` — 기존 batch 호출 유지.
- [semantic/build.rs](../../../../crates/quanta-index-semantic/src/build.rs): `apply_scope_stream`, `apply_window`, `delete_replace_scope_rows`, `delete_replace_scope_memberships`, `append_window`, seal/promotion.
- semantic `build/tests.rs`와 기존 streaming/fault/restart fixtures.
- `crates/quanta-index-core/src/domains/semantic/stream.rs::SemanticScopeStreamBuildPort`, `crates/quanta-index-semantic/src/lib.rs::SemanticAdapter`: 현재 tally-only port와 report를 폐기하는 adapter.
- `crates/quanta-index-search-plane/src/ingest_dispatcher/{semantic,search_corpus}.rs`, contract의 `PublishSearchCorpusBatch`/receipt, SDK publish surface, runner `main.rs`/diagnostics: 별도 transient observation의 end-to-end 연결 소유 경로.

## 작업 순서

1. 내부 report→port/adapter→search-plane→IPC/SDK→runner transient sidecar를 실제 request/repository/revision/batch/generation으로 연결한다. durable `BatchPublishReceipt`의 replay 정체성은 바꾸지 않는다. fresh/delta/replayed/no-op/error/partial/cancel을 구분하고 replay·미관측 stage는 0이나 fresh timing으로 꾸미지 않는다. embedding/delete/append/membership/seal/activate와 filesystem/fsync의 포함·중첩 범위를 명시한다.
2. delete가 실질적 비용이라는 측정이 있을 때 첫 후보로 bounded-window delete predicates를 병합한다. owner ID escaping과 SQL 길이 상한을 지키고 embeddings와 membership semantics를 함께 검증한다.
3. fresh-empty skip은 별도 후속 후보다. 저장소 authority가 truly empty generation, inherited rows 없음, 이전 streamed writes 없음, 해당 owners 최초 등 필요한 조건을 증명할 때만 허용한다. caller flag나 generation 번호만으로 생략하지 않는다.
4. append batching을 새 해결책인 것처럼 중복 구현하지 않는다. state root 재사용·dirty reset·durability 삭제·seal 생략으로 시간을 줄이지 않는다.
5. raw logical row set과 read-after-activation이 baseline과 같음을 확인한 후 development fresh roots에서 ingest latency/resource를 비교한다. 삭제 비용이 작으면 최적화하지 않고 측정 결과로 종료한다. 한 후보만 실험 profile에 고정해 RBR-12 최종 조합에 제출한다.

## 필수 반례

- 동시 batch 교차/잘못된 request·generation·revision, stage 누락/역순/중복, error 후 partial observation, replayed receipt에 이전 elapsed 혼입, transient field가 durable receipt digest에 들어가는 변조 거부. 실제 SDK roundtrip에서 동일 identity와 observed/not-observed 의미를 대조한다.
- fresh, inherited base, 기존 owner 교체, tombstone, empty scope, 반복 window/replay.
- reopened unsealed generation, append failure after delete, membership inconsistency, 중간 cancellation.
- crash 전후 seal/promotion, restart integrity, 이전 active generation 보존, new generation의 원자적 가시성.
- owner 수가 큰 window와 adversarial ID에서 bounded predicate size/escaping.

## 완료

독립 expected row-set oracle·semantic integration·영향받은 daemon scenario가 통과하고 [TEST-PLAN](TEST-PLAN.md)의 fresh-root time-to-searchable 효과 및 품질 guard를 최종 조합에서 충족해야 기본 정책을 바꾼다. 출력에는 미선택 후보와 유지 이유도 남긴다. durability 계약은 성능 목표보다 우선한다.
