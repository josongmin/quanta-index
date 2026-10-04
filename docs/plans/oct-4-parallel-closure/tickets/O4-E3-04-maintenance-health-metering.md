# O4-E3-04 — 느린 디스크 metering과 readiness 분리 판정

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E3 — Active 선택·read-view lifetime·운영 계약](../epics/E3-selection-and-operational-safety.md) / E3 담당 |
| 우선순위 / 종류 | P1 / `PROOF_THEN_CONDITIONAL_CODE` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | cooperative metering·supervisor ownership 수리 통합; 중앙 Rust 재검증 중 |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

디스크 full-tree walk가 backend health/readiness의 freshness를 지연시키는지 재현하고 필요한 경우 bounded health와 paced metering을 분리한다.

## 2026-10-04 구현·실패 수리

- health tick과 owned disk-meter worker를 분리하고 `TrackDiskUsagePort`에 `RequestBudgetV1`을 연결했다. tree walker는 디렉터리/엔트리 경계에서 deadline/cancel을 검사하며 boot/background metering은 30초 budget을 갖는다.
- 중앙 workspace 실행에서 `into_supervised_parts` 뒤 원래 timer의 Drop이 live worker를 취소해 harness 8건이 `RequiredChildLost maintenance-timer`로 실패했다. Drop은 자신이 thread를 보유할 때만 stop/join하고, 실제 timer owner가 worker cancellation을 맡도록 수리했다.
- 독립 controlled walker fixture `supervised_handoff_keeps_meter_live_until_its_owner_stops_and_joins`를 추가했다. 기존 in-flight shutdown cancellation fixture와 함께 중앙 재실행 중이다. 실제 process readiness/slow-walk scenario는 아직 `NOT_RUN`이다.
- cancellation은 cooperative하다. 이미 막힌 filesystem syscall 또는 외부의 비협조적 callback을 강제 중단한다는 계약은 없다.

## 배경과 현재 상태

maintenance.tick은 observe_backend 뒤 refresh_disk_usage를 동기 실행하고 tick 끝에서 heartbeat를 기록한다. heartbeat_fresh/required_backend_fresh는 3cadence를 사용한다. disk walk는 logical regular-file bytes이며 physical allocation/merge high water가 아니다. 실제 slow-walk 실험은 남아 있다.

## 착수 입력

- controlled TrackDiskUsagePort/identity probe, 3cadence 이상 blocked walk fixture
- active vs zero-active roots, missing/wrong restored identity, shutdown/poisoned maintenance cases

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-searchd/src/app/maintenance.rs](../../../../crates/quanta-index-searchd/src/app/maintenance.rs) | tick / observe_backend / refresh_disk_usage / MaintenanceTallies | slow metering fixture로 먼저 판정한다. 필요하면 bounded health cadence와 paced metering completion/age/error를 현재 maintenance owner에서 분리한다. | OWNED |
| [crates/quanta-index-searchd/src/app/readiness.rs](../../../../crates/quanta-index-searchd/src/app/readiness.rs) | backend and heartbeat readiness | 현재 exact active identity/token freshness를 사용하고 stale disk gauge를 backend 건강으로 오해하지 않게 한다. | OWNED |
| [crates/quanta-index-searchd/src/app/runtime.rs](../../../../crates/quanta-index-searchd/src/app/runtime.rs) | maintenance composition/lifecycle | 새 pacing이 필요하면 supervised stop/join과 resource ownership을 composition root에서 연결한다. | OWNED |
| [crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs) | runtime_extended_suite process cases | 실제 daemon의 post-boot active root loss·zero-active·slow metering detection time을 검증한다. | OWNED |
| [crates/quanta-index-searchd-runtime/tests/process_readiness_owner_v1.rs](../../../../crates/quanta-index-searchd-runtime/tests/process_readiness_owner_v1.rs) | owner readiness rail | fixed ports/clock/bounds와 operator readiness observations를 검증한다. | OWNED |

## 실행 단계

1. 3cadence보다 긴 controlled walker를 두고 backend observation/heartbeat/readiness 시간을 독립적으로 관측한다.
2. active root remove/rename와 zero-active root absence를 분리해 expected detection contract를 정한다.
3. baseline이 health cadence를 막으면 paced metering을 current timer/supervisor lifecycle에 연결한다.
4. meter values에 age/error를 유지하고 walk failure를 zero usage·healthy로 변환하지 않는다.
5. shutdown 중 walker block, observer failure/poison, restore wrong identity 및 continuous queries를 검증한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-searchd --lib --all-features --locked maintenance`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --all-features --locked`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked e2e_process_readiness`
- runtime/state root 변경 시 just rust-profile test-daemon + P09 scenario proof.

## 완료 조건

- 필요 backend loss는 declared interval에 false readiness로 관측되고 slow logical metering의 health 영향이 source-bound fixture로 판정된다.
- 수정 시 bounded lifecycle·age/error·shutdown 계약을 지킨다. baseline이 이미 충족하면 증거로 no-code closure 가능.

## 중단·거절·재개 조건

- byte gauge를 physical hard quota로 표시하지 않는다. deep content scrub와 identity probe를 동일 proof로 취급하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
