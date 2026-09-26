# RBR-10 — Semantic owner delete 비용과 replacement 안전성

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

- 구현: 단일 canonical `IngestStageReport`를 contract에 두고 `SemanticScopeStreamBuildPort`→semantic materializer→search-corpus port→ingest dispatcher→IPC→SDK에 전달한다. `SearchCorpusPublishOutcome`은 durable receipt와 별도 transient observation을 가진다. SDK `producer().publish_search_corpus_observed` 및 `search_corpus().publish_and_activate_observed`가 관측을 보존한다. 기존 durable API는 같은 구현에서 receipt만 투영한다. runner 연결은 통합 소유자가 진행한다. delete 최적화나 제품 기본값 변경은 없다.
- 검증(local diagnostic 실행): `/private/tmp/qi-rbr10-observation.sW0yqY/leaf-contract-sdk.log`는 최종 receipt leaf/validation boundary 변경 후 contract 146 + SDK 110 tests 전부 통과(ignored/filtered 0, 관측/wire 반례 5 및 관측 baseline + 8 identity/replay/activation/missing 반례 포함)다. `core-semantic-stream.log` 6 tests, `semantic-stages.log` 4 tests, `journal-replay.log` 1 test, `sink-coverage.log` 1 test도 앞서 통과했다(중복 실행 제외 local distinct 총 268; 서로 다른 시점의 실행을 frozen-source 증거로 합성하지 않는다). `leaf-clippy.log`는 최종 변경 후 contract/SDK all-target Clippy `--locked -- -D warnings` 종료 0. 신규 필수 nullable 누락·중복·unknown/옛 bare-receipt/V1 tag 거부, JSON field order/CBOR status tags·strict numeric, partial/finalize-only availability, sink tally와 stage coverage binding을 다룬다. 공유 source 변경과 입력/binary custody 미고정 때문에 **현 frozen-source `VERIFIED` 판정은 `NOT_RUN`**이며, 통합 runner/SDK 실수행 자격도 별도로 닫아야 한다.
- 잔여: 통합 runner sidecar·live daemon/SDK receipt를 재실행하고 fresh/delta 실제 비용을 측정한다. 유의한 delete 비용이 확인된 경우에만 bounded predicate 한 후보를 시험하고 durability/재시작/원자적 visibility 및 fresh-root 시간을 검증한다. 그렇지 않으면 측정에 근거한 유지 결정. [현재 전수 판정](CURRENT-AUDIT.md).
- 구현 경계: journal에는 `BatchPublishReceipt`만 저장하고 관측은 한 호출의 지역 값으로 전달한다. replay는 현재 request identity로 새 `Replayed` 관측을 만들되 모든 measured stage를 `None`으로 둔다. finalize-only와 한 track 복구는 별도 상태이며 오류/uncertain 호출에는 성공 outcome을 반환하지 않는다. nullable wire 필드는 **명시적 null까지 필수**이고 누락/unknown은 decode refusal이다. legacy reader/parallel IR를 추가하지 않는다.
- 혼용 거부: canonical wire discriminant는 `PublishSearchCorpusBatchV2` / `SearchCorpusReceiptV2`다(Rust variant 명칭은 유지). 옛 request tag는 새 daemon의 strict decoder가 dispatch 전에 거부한다. 새 request tag도 옛 closed decoder가 거부한다. response wrapper 변경 후에만 decode 실패하는 commit-before-refusal 구멍을 request tag에서 막는다. JSON/CBOR 양방향 반례 및 body 보존 테스트를 추가했다. durable batch fields/bytes/digest와 journal receipt는 바꾸지 않는다.
- SDK response binding은 `GenerationPin`·batch digest·seal flag만 보존하고 canonical `validate_identity`로 request/envelope/receipt를 대조한다. 관측을 위해 corpus text/vector를 통째로 복제하지 않는다. `validate_for`는 같은 validator에 위임하며 별도 판단 경로를 만들지 않는다.
- CI derive policy 보완: 5 공개 DTO와 test-only old-request decoder의 serde derive를 manual impl로 교체했다. 단일 closed-record visitor 구현이 map/sequence와 원래 field order를 유지하고 typed codec·unknown/duplicate 거부·required nullable를 지킨다. status unit-variant의 snake_case string도 유지한다. 공개 type/method signature 변경, suppression 또는 allowlist 수정은 없다. `derive-final.log`의 최종 전 guarded tree scan은 종료 0; 공식 baseline/installed gate는 통합 소유자가 재검증한다.
- Module cycle 보완: 실제 gate가 `ipc::ingest ↔ ipc::ingest_observation`을 거부했다(`module-cycle-before.log`). durable receipt/format-version/manual serde/impl은 새 `ipc/publish_receipt.rs` leaf로 옮기고 batch-bound `validate_for`만 ingest측에 둬서 의존성을 `ingest → observation → receipt`, `ingest → receipt` 방향으로 정리했다. 기존 receipt body는 HEAD `f9c3b4dc` 대비 14,187 chars 동일하며 wire/journal version 및 crate/ipc 공개 reexports도 유지한다. 전체 module-cycle gate after는 종료 0(`module-cycle-after.log`); cycle baseline 확대나 suppression은 없다. 공식 cargo-modules/public-API baseline 리뷰·재검증은 통합 소유자 담당이다.
- Downstream restart fixture RCA: daemon escalation의 semantic reopen 실패는 durable 데이터 차이가 아니라 `SearchExplanation` 전체 동등성에 새 transient `stage_timings.elapsed_ns`가 포함된 fixture stale이었다. 원 실패는 `restart-original-failure.log`로 보존(SHA256 `f390c7d26bc30626d68e96db3998ae17e142a808fd040150e8882b82ebb5b769`). `e2e_restart_replay_determinism.rs`의 3 full comparisons는 request ID/elapsed만 제외하는 exhaustive projection으로 바꾸고 실제 ID 양수·timing availability/order/kind/calls/count·모든 durable metadata를 유지했다. scoped-semantic 6-stage/1-call/3-scope/3-dense/2-output golden 및 11 mutation 반례를 추가했다. runtime/harness integration 사용처 검색에서 추가 full-explanation equality 누락은 발견하지 못했다. 최종 owning module은 `restart-projection-final.log`에서 21 passed/0 failed/ignored/117 filtered, scoped Clippy `--test runtime_risk_suite --locked -- -D warnings`는 `restart-projection-clippy-current.log` 종료 0; fmt/diff check도 종료 0이다. 이전 중간 Clippy의 doc-format lint와 병렬 floor caller migration E0061은 성공 증거가 아니다. 전체 daemon frozen-source qualification은 통합 소유자가 재수행한다. SDK/telemetry 생산 계약 변경은 없다.
- 외부 daemon compile escalation은 `searchd-runtime/tests/e2e_ingest_idempotency.rs::receipt_of`의 누락된 V2 outcome→durable receipt 투영에서 실패했다(`/private/tmp/qi-rbr-integrate.Fg8v62/daemon-escalation.stderr.log`). 해당 helper를 `outcome.receipt`로 보완했다. 전 Rust `SearchCorpusReceipt` 사용처를 다시 검색했으며 다른 외부 helper는 receipt field read 또는 wildcard 분기였다. daemon gate 재실행은 통합 소유자가 수행한다; 실패한 이전 compile은 테스트 성공 증거가 아니다.

### 시계 범위

- `semantic.durations.total`: semantic owner의 전체 build, provider windows와 filesystem 준비·promotion·fsync 포함.
- `prepare`: 검증/recovery/staging copy/working tables open. `clear_surfaces`, `stream`, `tombstones`: 해당 pass; `stream` 안에 provider embedding과 admission·delete·append가 중첩된다.
- `embedding`: 실제 provider 호출의 누적 elapsed만 포함; 입력 계획/재분배 제외. 호출하지 않았다면 `None`.
- `semantic_delete`, `membership_delete`, `semantic_append`, `membership_append`: storage operation subinterval이며 pass와 합산해 독립 stage 비용처럼 주장하지 않는다.
- `seal`: vector index/row manifest 계산; unsealed batch는 `None`. `promotion`: dataset/contract promotion·READY/manifest/file-seal/SEALED write 및 해당 fsync. `seal`만 전체 durability 비용으로 사용하지 않는다.
- `lexical_build_ns`: lexical build 전체(해당 adapter의 seal 포함). `finalize_ns`: ledger/catalog finalize. 사전 resource/lock/journal/receipt read 및 outer IPC는 이 stage 합계 밖이며 runner wall과 구별한다.
- `activation_ns`: 항상 `None`. activation은 별도 control request이므로 ingest server observation으로 꾸미지 않는다; runner는 opaque SDK publish+activate wall 또는 별도의 activation scope를 구분한다.

- 우선순위: **P1 공개 관측 integration 자격 / P2 조건부 최적화**. `f9c3b4dc` + 공유 dirty에서 구현/local diagnostic 검증; 실행 중 공유 source가 변경됐으므로 exact frozen-source qualification과 실측·조건부 결정은 `NOT_RUN`. [현재 전수 판정](CURRENT-AUDIT.md).
- 확인된 사실: 임베딩은 이미 window batching, storage는 scope별 delete 후 batch append. 비용 비중은 아직 미측정.

## 파일·함수

- [batch.rs](../../../../benchmarks/retrieval/src/batch.rs): `semantic_scope` — chunk마다 owner scope.
- [semantic_derive.rs](../../../../crates/quanta-index-search-plane/src/semantic_derive.rs): `embed_window` — 기존 batch 호출 유지.
- [semantic/build.rs](../../../../crates/quanta-index-semantic/src/build.rs): `apply_scope_stream`, `apply_window`, `delete_replace_scope_rows`, `delete_replace_scope_memberships`, `append_window`, seal/promotion.
- semantic `build/tests.rs`와 기존 streaming/fault/restart fixtures.
- `crates/quanta-index-contract/src/ipc/ingest_observation.rs`: 단일 report/outcome/identity validator와 strict nullable wire 계약.
- `crates/quanta-index-contract/src/ipc/publish_receipt.rs`: request/observation을 참조하지 않는 durable receipt leaf와 그대로 유지한 manual wire codec/journal format constant.
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
