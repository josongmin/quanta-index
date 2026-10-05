# O4-E3-04 — 느린 디스크 metering과 readiness 분리 판정

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E3 — Active 선택·read-view lifetime·운영 계약](../epics/E3-selection-and-operational-safety.md) / E3 담당 |
| 우선순위 / 종류 | P1 / `PROOF_THEN_CONDITIONAL_CODE` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | 기존 owner/daemon proof 및 실제 runtime 조립 OS-child slow-disk port5-cadence·active readiness·실제 adapter 완료·정상 stop/join `VERIFIED`. shipping binary/Linux release는 별도 scope |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

디스크 full-tree walk가 backend health/readiness의 freshness를 지연시키는지 재현하고 필요한 경우 bounded health와 paced metering을 분리한다.

## 2026-10-05 실제 runtime OS-child 검증

- frozen `b55f4c6d` + runtime 조립/fixture3파일의 root 작업트리 `oct5-process-actual`에서 실행했다. child는 실제 `build_runtime`과 동일한 private composition, state root, catalog, disk adapters, supervisor, UDS를 사용한다. production composition은 기존 port를 그대로 전달한다. 테스트에서만 실제 `TrackDiskUsagePort` 앞에 gate를 설치한다.
- parent는 빈 production state root의 child에 SDK `publish_and_activate`로 G1 양 트랙을 활성화한 뒤 meter를 멈춘다. 100ms cadence5회 이상에서 refresh 완료가 늘지 않고 skipped work가 증가하는 동안 actual control UDS의 ready·active1·heartbeat·candidate integrity를 검사한다. release 뒤 실제 adapter 완료 token과 refresh/failure counters를 확인하고 child의 정상 stop/join을 요구한다.
- `VERIFIED`: `CARGO_BUILD_JOBS=1 ./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --lib --all-features --locked os_child -- --nocapture --test-threads 1` — exit0,2selected/2passed/0failed/1filtered,32.06s. 이 티켓의 slow-disk child는1.34s에 통과했다. 같은 배치의 timeout/replay는 [E3-05](O4-E3-05-publish-timeout-replay.md)에 기록한다.
- 초기 실행 실패를 성공으로 계산하지 않았다: fixture shutdown 타입·미사용 반환값 compile 오류, retention 설정 누락, Darwin accepted gate의 nonblocking IO 실패를 수정했다. parent gate에 blocking mode를 명시했고 실제 meter의30초 budget은 바꾸지 않았다. fake harness로 production root를 준비하는 경로도 제거했다.
- 검증한 runtime3파일을 원래 main bytes와 대조해 통합했다. `lib` test executable의 실제 OS child이며 shipping release daemon/Linux/운영 qualification으로 확대하지 않는다. 아래 정적 `NOT_RUN` 기록은 이 실행 이전의 상태다.

## 2026-10-04 구현·실패 수리

- health tick과 owned disk-meter worker를 분리하고 `TrackDiskUsagePort`에 `RequestBudgetV1`을 연결했다. tree walker는 디렉터리/엔트리 경계에서 deadline/cancel을 검사하며 boot/background metering은 30초 budget을 갖는다.
- 중앙 workspace 실행에서 `into_supervised_parts` 뒤 원래 timer의 Drop이 live worker를 취소해 harness 8건이 `RequiredChildLost maintenance-timer`로 실패했다. Drop은 자신이 thread를 보유할 때만 stop/join하고, 실제 timer owner가 worker cancellation을 맡도록 수리했다.
- 독립 controlled walker fixture `supervised_handoff_keeps_meter_live_until_its_owner_stops_and_joins`와 기존 in-flight shutdown cancellation fixture가 중앙 workspace lib/bin 실행에서 PASS했다(searchd lib 105 passed/1 ignored). 실제 process readiness/slow-walk scenario는 아직 `NOT_RUN`이다.
- cancellation은 cooperative하다. 이미 막힌 filesystem syscall 또는 외부의 비협조적 callback을 강제 중단한다는 계약은 없다.
- source904의 `just rust-profile test-daemon`은213 passed/1 skipped였고 별도 `process_readiness_owner_v1`은26 passed였다. 실제 active backend root loss는16.38s에 통과했고 inventory-read failure/reopen도 통과했다. controlled slow walker의 heartbeat/stop 증거는 unit scope이며 OS-child slow-disk injection을 실행했다는 뜻은 아니다.

## 2026-10-04 fatal ownership와 supervisor terminal 수리

- active disk-meter mutex poison/중복 budget/owner 소실은 typed Storage error와 영속 fatal 상태를 남긴다. poisoned cancel handle은 cleanup 목적으로만 회복하며 이후 heartbeat를 healthy로 반환하지 않는다.
- timer가 owned `DiskMeterJoinGuard`를 보유한다. 정상 종료와 callback unwind 모두 budget cancel·sender close·worker join을 완료한 뒤 timer thread가 종료된다. worker panic, owner 소실, terminal report 누락은 정상 `Completed`로 변환되지 않는다.
- 기존 `ChildExitKind`를 원래 timer의 terminal channel로 supervisor에 전달한다. 실제 timer panic은 `Panicked`, worker/ownership fatal은 `Failed`, 정상 cooperative join은 `Completed`다. 별도 감시 thread나 새 terminal enum을 추가하지 않았다.
- `VERIFIED`: `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd --lib --all-features --locked -E 'test(/^app::(maintenance|supervisor)::/)' --success-output final` — exit0, 13 selected/13 passed/97 filtered, tests0.174s. controlled worker panic, poison, terminal 없음, 실제 filesystem walker가 시작한 뒤 timer callback panic, long walk 동안 heartbeat, handoff/정상 stop을 포함한다. source는 `561e5eb96c3fa52fa863d721b48447f047b110e8`의 maintenance/supervisor이며 동시 dirty SDK/harness 테스트는 이 selector의 입력이 아니다.
- `VERIFIED`: sourcebf 중앙 owner972 배치의 runtime supervisor25가 모두 통과했고, current `just rust-profile test-daemon`은 exit0,213 passed/1 skipped,tests231.161s였다. 후속 `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --all-features --locked --test-threads 4`도 exit0,26 passed/0 skipped,tests16.869s였다. 이 결과는 OS-child slow-disk injection이나 Linux release qualification이 아니다.

## 2026-10-05 정적 잔여 판정

- `a_disk_walk_longer_than_three_cadences_does_not_stale_the_health_timer`는 controlled port를 20ms cadence 3회보다 길게 막고 5 ticks, fresh heartbeat, skipped meter를 검사한다. `process_readiness_owner_v1`의 실제 binary child는 active backend root loss/restoration을 별도로 검사한다. 이 둘을 OS-child slow walk 한 건으로 합성하지 않으며 기록된 PASS를 이번에 재실행하지 않았다.
- **실제 daemon child의 controlled slow-disk 3-cadence case는 `NOT_RUN`**이다. production binary config에는 cadence 설정만 있고 filesystem walker를 child에서 결정적으로 멈추는 seam은 없다. 큰 디렉터리나 느린 매체에 의존한 elapsed-time case는 독립 oracle가 아니다. 이를 요구하려면 기존 disk-meter owner가 child에서 제어할 수 있는 walker gate와 종료 custody가 필요하지만, 이 증명만을 위해 새 operational API나 test-only production hook은 추가하지 않는다.
- 정확한 owner selectors: `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd --lib --all-features --locked -E 'test(a_disk_walk_longer_than_three_cadences_does_not_stale_the_health_timer)'`; `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --all-features --locked -E 'test(binary_daemon_detects_lost_active_backend_root)'`. 둘 다 이번 정적 판정에서는 `NOT_RUN`이다.

## 배경과 현재 상태

변경 전 maintenance.tick은 observe_backend 뒤 refresh_disk_usage를 동기 실행하고 tick 끝에서 heartbeat를 기록했다. heartbeat_fresh/required_backend_fresh는 3cadence를 사용한다. disk walk는 logical regular-file bytes이며 physical allocation/merge high water가 아니다. 현재 구현은 아래 worker 분리와 소유권 수리를 반영했다.

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
