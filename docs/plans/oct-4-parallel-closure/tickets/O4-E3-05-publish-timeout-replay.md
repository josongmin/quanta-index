# O4-E3-05 — admitted publish timeout과 operation replay

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E3 — Active 선택·read-view lifetime·운영 계약](../epics/E3-selection-and-operational-safety.md) / E3 담당 |
| 우선순위 / 종류 | P1 / `PROOF_THEN_CONDITIONAL_CODE` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | 기본30초 SDK read timeout→별도 OS-child admitted publish Committed→child 종료/재조립→exact replay/build0·digest conflict refusal `VERIFIED`. shipping release daemon은 별도 scope |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

client timeout 이후 durable publish 상태와 exact operation replay를 검증해 timeout을 rollback으로 오해하는 동작을 막는다.

## 배경과 현재 상태

SDK default I/O deadline30s, ingest budget120s, process-wide serial ingest admission과 query admission은 별도다. runtime lib `timed_out_uds_peer_does_not_cancel_admitted_publish_or_replay_after_runtime_reassembly`는 2초 client policy의 실제 UDS read timeout, peer hangup metric, journal Committed, runtime 재조립 뒤 exact replay와 physical build 0회를 확인한다. 기본30초 timeout과 별도 OS process daemon 결과로 확대하지 않는다. async ACK/parallel dispatch를 새로 도입할 근거는 없다.

## 2026-10-05 기본 SDK deadline의 실제 OS-child 검증

- root frozen `b55f4c6d` + runtime 조립/fixture3파일의 `oct5-process-actual`에서 [E3-04](O4-E3-04-maintenance-health-metering.md)와 같은 명령을 실행했다: `CARGO_BUILD_JOBS=1 ./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --lib --all-features --locked os_child -- --nocapture --test-threads 1` — exit0,2selected/2passed/0failed/1filtered,32.06s.
- `default_sdk_timeout_in_os_child_still_commits_and_replays_without_rebuild`는 실제 disk-backed runtime child의 admitted build gate에서 catalog `InFlight`를 확인한다. SDK의 기본 정책을 사용해 정확한 `Read`/`DEFAULT_CLIENT_IO_TIMEOUT` typed timeout 및 peer hangup을 관측한 뒤 gate를 release한다. journal `Committed`의 applied/durable sequence를 확인하고 child를 종료·회수한다.
- 같은 state root를 실제 runtime으로 재조립해 원래 durable receipt와 exact SDK replay의 equality를 검사하고 build0을 요구한다. 동일 source event의 다른 canonical digest는 `BatchDigestConflict`, conflict journal은 `Absent`, 원래 receipt는 불변이며 추가 build0이어야 한다.
- 준비용 harness와 child production state root를 분리해 초기 `STATE_ROOT_FORMAT_UNSUPPORTED` 실패를 수정했다. accepted gate는 blocking mode와 bounded read를 명시한다. 기본 SDK30초/ingest budget/생산자 계약을 바꾸지 않았다.
- 검증한 runtime3파일을 main에 통합했다. actual OS process의 test-composed runtime 범위이며 shipping release daemon/Linux 배포는 별도다. 아래 정적 `NOT_RUN` 기록은 이 실행 이전의 상태다.

## 2026-10-05 정적 잔여 판정

- runtime lib의 controlled lexical build-port gate는 admitted operation을 멈춘 상태에서 실제 UDS의 2초 read timeout·peer hangup, Committed inspect, 재조립 후 exact replay와 conflicting digest refusal을 검사한다. `runtime_fast_suite`의 `binary_restart_replays_original_operation_and_refuses_conflicting_source_digest`는 두 별도 daemon child 사이의 durable replay를 검사하되 timeout을 유도하지 않는다. 기록된 PASS를 이번에 재실행하지 않았다.
- **기본 30초 SDK deadline을 넘는 별도 OS-child admitted publish 결합 case는 `NOT_RUN`**이다. controlled build-port wrapper는 runtime lib에만 주입되며 production binary에는 해당 gate가 없다. 임의 대용량 입력이나 OS I/O 지연으로 30초 초과를 기대하면 admission 시점과 결과가 비결정적이다. child-visible admitted build gate가 없다면 동일 계약의 결정적 OS-process proof를 만들 수 없다. 이 잔여 증명만을 위한 production hook은 추가하지 않는다.
- 정확한 owner selectors: `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --lib --all-features --locked -E 'test(timed_out_uds_peer_does_not_cancel_admitted_publish_or_replay_after_runtime_reassembly)'`; `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked -E 'test(binary_restart_replays_original_operation_and_refuses_conflicting_source_digest)'`. 둘 다 이번 정적 판정에서는 `NOT_RUN`이다.

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

## 남은 검증 계획 — default30s/OS process/daemon profile NOT_RUN

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
