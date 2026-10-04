# O4-E3-05 — admitted publish timeout과 operation replay

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E3 — Active 선택·read-view lifetime·운영 계약](../epics/E3-selection-and-operational-safety.md) / E3 담당 |
| 우선순위 / 종류 | P1 / `PROOF_THEN_CONDITIONAL_CODE` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

client timeout 이후 durable publish 상태와 exact operation replay를 검증해 timeout을 rollback으로 오해하는 동작을 막는다.

## 배경과 현재 상태

SDK default I/O deadline30s, ingest budget120s, process-wide serial ingest admission과 query admission은 별도다. peer_watch는 hangup cancel을 전달하고 ingest dispatcher는 entry budget check 후 admitted publish를 durable settle한다. async ACK/parallel dispatch를 새로 도입할 근거는 없다.

## 착수 입력

- controlled slow publish port·operation identity/digest, actual SDK timeout/hangup
- retained active/predecessor generations, journal terminal states와 same/opposite digest replay

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-ipc/src/server/peer_watch.rs](../../../../crates/quanta-index-ipc/src/server/peer_watch.rs) | hangup budget cancellation | actual peer disconnect/cancel propagation을 검증한다. 입증된 전달 결함만 수정한다. | OWNED |
| [crates/quanta-index-ipc/src/admission.rs](../../../../crates/quanta-index-ipc/src/admission.rs) | serial ingest vs query slot | 한 slow admitted publish와 independent query admission을 검증한다. concurrency 확대는 별도 decision으로 남긴다. | READ |
| [crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs](../../../../crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs) | dispatch / operation journal terminal | entry refusal와 admitted durable settlement·typed replay를 실제 record로 대조한다. | OWNED |
| [crates/quanta-index-search-plane/src/ingest_dispatcher/tests/idempotency.rs](../../../../crates/quanta-index-search-plane/src/ingest_dispatcher/tests/idempotency.rs) | operation replay tests | timeout 후 same op/digest replay, different digest conflict, partial source-loss cases를 추가한다. | OWNED |
| [crates/quanta-index-searchd-runtime/tests/e2e_ingest_idempotency.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_ingest_idempotency.rs) | runtime_fast_suite ingress seam | client deadline·hangup→operation inspect/replay를 actual daemon에서 확인한다. | OWNED |
| [crates/quanta-index-sdk/src/client.rs](../../../../crates/quanta-index-sdk/src/client.rs) | default I/O/dispatch timeout | 타이밍·receipt 처리 bug가 재현될 때만 변경한다. 기본 timeout 자동상향으로 통과시키지 않는다. | OWNED |

## 실행 단계

1. entry 취소와 accepted durable operation을 controlled barrier로 분리한다.
2. 30초 기본 client timeout보다 느린 publish를 actual SDK에서 보내고 timeout/hangup 및 server terminal을 관측한다.
3. operation identity로 durable state를 조회하고 같은 identity/digest를 replay한다.
4. 같은 op의 wrong source digest/다른 payload를 거절하고 중복 seal/publish나 implicit activate가 없음을 검증한다.
5. query admission·shutdown·source custody 실패 경계를 확인하고 재현된 오류만 canonical operation owner에서 고친다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-search-plane --lib --all-features --locked idempotency`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked e2e_ingest_idempotency`
- shared ingress/generation 변경 시 just rust-profile test-daemon; IPC/decode 변경 시 just rust-fuzz-smoke.

## 완료 조건

- timeout/hangup과 실제 operation terminal이 구분되고 exact replay는 같은 durable receipt/result를 만든다.
- 동일 identity의 conflicting input은 typed refusal이며 old active/rollback-required generations를 보존한다.

## 중단·거절·재개 조건

- 30s client timeout을 publish rollback 또는 index activation 완료로 표시하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
