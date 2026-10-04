# O4-E3-03 — 단일 RPC의 Active 선택·검색·응답 결속

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E3 — Active 선택·read-view lifetime·운영 계약](../epics/E3-selection-and-operational-safety.md) / E3 담당 |
| 우선순위 / 종류 | P1 / `CODE_AND_PROOF` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | [O4-E3-01](O4-E3-01-active-selection-race.md), [O4-E3-02](O4-E3-02-admission-pin-transfer.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

SDK Active 요청에서 사전 resolve 왕복을 제거할 수 있는 원자적 선택/검색 계약을 만들되 generation/token/ABA 검증을 유지한다.

## 배경과 현재 상태

SdkClient.pin_active_selector는 ResolveActiveGeneration을 먼저 보내고 explicit pin+ResolvedActive token으로 다음 query를 구성한다. server는 Active selector를 지원하지만 기존 응답 pin만으로 A→B→A token identity를 검증할 수 있다고 가정할 수 없다. 과거 2개 query의 resolve1.5–1.6ms는 비용 위치일 뿐 절감 보장이 아니다.

## 착수 입력

- E3-01/02의 admission decision과 linearization point
- active/pinned/tokened/cursor/RevAtTime/current binding fixtures, malformed response controls

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-sdk/src/client.rs](../../../../crates/quanta-index-sdk/src/client.rs) | pin_active_selector / dispatch_query_inner | atomic active-query route가 final selected snapshot을 반환할 때 pre-resolve를 제거하고 exact response binding을 유지한다. | OWNED |
| [crates/quanta-index-sdk/src/binding.rs](../../../../crates/quanta-index-sdk/src/binding.rs) | query generation/domain/response validation | generation/token/domain/variant/row identity를 actual selected response와 검증한다. missing/wrong snapshot 거절. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/selection.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/selection.rs) | active and joint selection | 한 snapshot에서 track pin/token을 결정하고 acquired view까지 결속한다. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/planning.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/planning.rs) | planned lexical/semantic query | 선택 authority와 response materialization 사이에 ambient latest read를 추가하지 않는다. | SHARED |
| [crates/quanta-index-contract/src/ipc/split.rs](../../../../crates/quanta-index-contract/src/ipc/split.rs) | SearchPlaneQueryIpcRequest/Response | 현 DTO에서 selected snapshot/token 증거가 부족할 때만 I0와 현재 wire를 진화시킨다. | SHARED |
| [crates/quanta-index-sdk/src/tests/query_tests.rs](../../../../crates/quanta-index-sdk/src/tests/query_tests.rs) | observed active query RPCs | fixed atomic snapshot expected rows와 RPC count, token conflicts/ABA/continuation tests를 추가한다. | OWNED |
| [benchmarks/retrieval/tests/sdk_roundtrip.rs](../../../../benchmarks/retrieval/tests/sdk_roundtrip.rs) | real-daemon query observation | 단일RPC actual SDK seam과 normalized result parity를 검증한다. | SHARED |

## 실행 단계

1. 현 response의 selected token/snapshot 증거를 확인해 필요한 최소 wire/binding 변경을 확정한다.
2. 한 catalog selection→acquired read-view→result response를 같은 선택 identity로 결속한다.
3. SDK raw Active/ResolvedActive/explicit pin callers를 함께 업데이트하고 old two-step fallback 계층을 장기 유지하지 않는다.
4. ABA, concurrent activate, explicit conflict, cursor continuation, stale generation, ancestor domain, credential/deadline/cancel/reconnect cases를 실행한다.
5. representative fixed fixture→full exact1196 순서로 row/order/count/status/byte/unit parity를 검증하고 E4-06에 source change를 넘긴다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- ./scripts/cargow test -p quanta-index-sdk --lib --all-features --locked
- ./scripts/cargow test -p quanta-index-retrieval-bench --test sdk_roundtrip --all-features --locked
- just rust-public-api; wire/decode 변경 시 just rust-fuzz-smoke; generation selection 변경 시 just rust-profile test-daemon.
- Negative: response token/variant/domain/row identity mutation, A→B→A stale token 및 cursor rebind 거절.

## 완료 조건

- native Active query가 선택된 generation/token과 exact-bound response로 하나의 query RPC를 실행한다.
- atomic linearization·result identity 보존과 runtime tests가 입증되며 speedup은 E4-06에서 따로 판정한다.

## 중단·거절·재개 조건

- server Active 지원만 보고 client pre-resolve부터 삭제하지 않는다. token 증거 없이 fixed-pin query를 Active query와 동등하다고 표시하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
