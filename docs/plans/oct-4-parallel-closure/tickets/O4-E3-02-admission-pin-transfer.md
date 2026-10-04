# O4-E3-02 — 선택 admission pin의 read-view 이전

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E3 — Active 선택·read-view lifetime·운영 계약](../epics/E3-selection-and-operational-safety.md) / E3 담당 |
| 우선순위 / 종류 | P0 / `CONDITIONAL_CODE` |
| 기준 웨이브 | [W2 — 확인된 결함 수리·선택 최적화](../waves/W2-repairs-and-selected-optimizations.md) |
| 실행 상태 | 현 Accepted 계약에서 구현 조건 미성립: `NOT_APPLICABLE` (E3-01 dispatcher refusal oracle). OCT-04-001 강화 계약은 `Proposed`; 채택·실제 반례가 있으면 재개 |
| 선행 결과 | [O4-E3-01](O4-E3-01-active-selection-race.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

재현된 selection/acquisition 결함이 있을 때 짧은 catalog/retention 보호를 actual view handle lifetime으로 이전한다.

## 배경과 현재 상태

현재 composite active selection과 read view/snapshot registry는 이미 있다. E3-01의 G1 selection→retirement→view acquisition은 `UNKNOWN_GENERATION`으로 거절하고 opener를 호출하지 않아 현 Accepted 계약상 serve-from-retired 결함이 관측되지 않았다. SEP-21-003은 획득한 view의 handle lifetime과 typed unavailable을 요구하며, 선택만 된 G1의 성공 보장은 명시하지 않는다. OCT-04-001의 admission transfer는 Proposed이므로 현재 구현 조건이 성립하지 않는다. 모든 active handle의 영구 pin이나 SDK retry는 여전히 금지한다.

## 착수 입력

- E3-01 reproducible counterexample와 선택 linearization contract
- current lock ordering·retention required generation·active/predecessor/live-reader obligations

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-search-plane/src/readiness/activation_catalog.rs](../../../../crates/quanta-index-search-plane/src/readiness/activation_catalog.rs) | active selection API | selection+short admission claim을 catalog/retention authority 안에서 얻는 최소 API를 현재 type으로 진화시킨다. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/selection.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/selection.rs) | resolve_optional_selection / resolve_joint_active_selection | generation/token과 admission claim을 단일 선택 결과로 운반한다. parallel IR/version twin을 만들지 않는다. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs) | ReadViewRequestV1 / acquire_read_view / QueryReadViewV2 | handle 획득 완료 시 claim custody를 view로 이전하고 error/panic/cancel에서 release한다. | OWNED |
| [crates/quanta-index-search-plane/src/snapshot_registry.rs](../../../../crates/quanta-index-search-plane/src/snapshot_registry.rs) | snapshot admission/acquire/reconcile | real live flight/handle과 physical identity의 retirement guard를 통합한다. | OWNED |
| [crates/quanta-index-search-plane/src/search_corpus_retention.rs](../../../../crates/quanta-index-search-plane/src/search_corpus_retention.rs) | retention plan | 선택 flight/live reader가 요구하는 generation을 required set에서 보존한다. limit 우회·무제한 pin 금지. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/planning.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/planning.rs) | selection to read-view assembly | 선택 결과를 소비할 모든 text/semantic/hybrid caller를 함께 변경한다. | SHARED |

## 실행 단계

1. E3-01에서 현 Accepted 계약의 typed refusal을 확인했으므로 현재 implementation branch는 `NOT_APPLICABLE`이다. 강화 계약이 채택되거나 Accepted 계약의 별도 실패가 재현될 때만 이 단계를 재개한다.
2. 현재 catalog/retention/registry/mutation-coordinator caller의 lock acquisition graph를 먼저 작성해 기존 순서·역방향 호출을 확인한다. acyclic order와 bounded claim 수명을 정하고 I/O/backend open을 guard 안에 넣지 않는다. 이름 순서만 보고 catalog→retention→registry를 강제하지 않는다.
3. 최소 admission claim을 선택과 함께 획득하고 read-view acquisition 후 기존 live handle 보호로 이전한다.
4. explicit pin/token conflict·joint lexical+semantic snapshot·aux epochs의 refusal 계약을 유지한다.
5. retire/GC/cancel/panic/cache-churn counterexamples와 real daemon case를 re-run한다.

## 채택 시 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-search-plane --lib --all-features --locked read_view`
- `./scripts/cargow test -p quanta-index-search-plane --lib --all-features --locked retention`
- `just rust-profile test-daemon`
- 필요한 공개 surface가 변경되면 just rust-public-api; lock/module boundary 변경이면 just rust-hexagonal 및 just rust-cargo-modules.

## 완료 조건

- E3-01 baseline counterexample가 수정 후 독립 oracle로 통과하고 old view/live flight가 살아 있을 때 실제 GC가 참조 artifact를 제거하지 않는다.
- 모든 exit path가 claim을 해제하며 많은 active pairs가 cache capacity를 넘더라도 무제한 resident pin이 생기지 않는다.

## 중단·거절·재개 조건

- 현재 source에 동등 admission 보호가 이미 있으면 재구현 없이 qualified counterexample를 기존 owner에 연결한다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
