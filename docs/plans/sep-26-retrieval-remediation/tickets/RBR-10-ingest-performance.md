# RBR-10 — Semantic owner delete 비용과 replacement 안전성

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

- 구현: 단일 canonical `IngestStageReport`를 contract에 두고 `SemanticScopeStreamBuildPort`→semantic materializer→search-corpus port→ingest dispatcher→IPC→SDK에 전달한다. `SearchCorpusPublishOutcome`은 durable receipt와 별도 transient observation을 가진다. SDK `producer().publish_search_corpus_observed` 및 `search_corpus().publish_and_activate_observed`가 관측을 보존한다. 기존 durable API는 같은 구현에서 receipt만 투영한다. runner 연결은 통합 소유자가 진행한다. delete 최적화나 제품 기본값 변경은 없다.
- 검증: 신규 nullable 필드 누락/옛 bare-receipt wire 거부, 다른 request/repo/revision/generation/batch와 replay/activation 위조, partial/finalize-only availability, sink tally와 stage coverage binding 테스트를 추가했다. 이번 실행 근거는 `/private/tmp/qi-rbr10-observation.sW0yqY/`에 수집 중이다. 실행 완료 전 해당 신규 결과는 `NOT_RUN`이다. 기존 4 internal stage tests는 과거 local 근거이며 현 SDK qualification 대신 쓰지 않는다.
- 잔여: 통합 runner sidecar·live daemon/SDK receipt를 재실행하고 fresh/delta 실제 비용을 측정한다. 유의한 delete 비용이 확인된 경우에만 bounded predicate 한 후보를 시험하고 durability/재시작/원자적 visibility 및 fresh-root 시간을 검증한다. 그렇지 않으면 측정에 근거한 유지 결정. [현재 전수 판정](CURRENT-AUDIT.md).
- 구현 경계: journal에는 `BatchPublishReceipt`만 저장하고 관측은 한 호출의 지역 값으로 전달한다. replay는 현재 request identity로 새 `Replayed` 관측을 만들되 모든 measured stage를 `None`으로 둔다. finalize-only와 한 track 복구는 별도 상태이며 오류/uncertain 호출에는 성공 outcome을 반환하지 않는다. nullable wire 필드는 **명시적 null까지 필수**이고 누락/unknown은 decode refusal이다. length-prefixed CBOR에는 별도 negotiated protocol version이 없으므로 옛/새 payload 혼용은 의도적인 decode 실패다. SDK/daemon을 같은 source에서 함께 갱신해야 하며 legacy reader/parallel IR를 추가하지 않는다.

### 시계 범위

- `semantic.durations.total`: semantic owner의 전체 build, provider windows와 filesystem 준비·promotion·fsync 포함.
- `prepare`: 검증/recovery/staging copy/working tables open. `clear_surfaces`, `stream`, `tombstones`: 해당 pass; `stream` 안에 provider embedding과 admission·delete·append가 중첩된다.
- `embedding`: 실제 provider 호출의 누적 elapsed만 포함; 입력 계획/재분배 제외. 호출하지 않았다면 `None`.
- `semantic_delete`, `membership_delete`, `semantic_append`, `membership_append`: storage operation subinterval이며 pass와 합산해 독립 stage 비용처럼 주장하지 않는다.
- `seal`: vector index/row manifest 계산; unsealed batch는 `None`. `promotion`: dataset/contract promotion·READY/manifest/file-seal/SEALED write 및 해당 fsync. `seal`만 전체 durability 비용으로 사용하지 않는다.
- `lexical_build_ns`: lexical build 전체(해당 adapter의 seal 포함). `finalize_ns`: ledger/catalog finalize. 사전 resource/lock/journal/receipt read 및 outer IPC는 이 stage 합계 밖이며 runner wall과 구별한다.
- `activation_ns`: 항상 `None`. activation은 별도 control request이므로 ingest server observation으로 꾸미지 않는다; runner는 opaque SDK publish+activate wall 또는 별도의 activation scope를 구분한다.

- 우선순위: **P1 공개 관측 local/integration 검증 / P2 조건부 최적화**. `8eac12c5` + 공유 dirty에서 구현 중; exact frozen-source qualification과 실측·조건부 결정은 `NOT_RUN`. [현재 전수 판정](CURRENT-AUDIT.md).
- 확인된 사실: 임베딩은 이미 window batching, storage는 scope별 delete 후 batch append. 비용 비중은 아직 미측정.

## 파일·함수

- [batch.rs](../../../../benchmarks/retrieval/src/batch.rs): `semantic_scope` — chunk마다 owner scope.
- [semantic_derive.rs](../../../../crates/quanta-index-search-plane/src/semantic_derive.rs): `embed_window` — 기존 batch 호출 유지.
- [semantic/build.rs](../../../../crates/quanta-index-semantic/src/build.rs): `apply_scope_stream`, `apply_window`, `delete_replace_scope_rows`, `delete_replace_scope_memberships`, `append_window`, seal/promotion.
- semantic `build/tests.rs`와 기존 streaming/fault/restart fixtures.
- `crates/quanta-index-contract/src/ipc/ingest_observation.rs`: 단일 report/outcome/identity validator와 strict nullable wire 계약.
- `crates/quanta-index-core/src/domains/semantic/stream.rs::SemanticScopeStreamBuildPort`, `crates/quanta-index-semantic/src/lib.rs::SemanticAdapter`: tally + canonical report 반환.
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
