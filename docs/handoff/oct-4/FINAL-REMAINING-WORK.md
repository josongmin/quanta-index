# 2026-10-05 통합 잔여 작업

원본 [agent-1](agent-1.md)·[agent-2](agent-2.md)·[agent-3](agent-3.md)·[agent-4](agent-4.md)·[agent-5](agent-5.md)의 합집합에서 후속 구현·실행으로 충족된 범위를 제거한 현재 작업 목록이다. 전체 29개 종료나 품질·성능·배포 qualification을 뜻하지 않는다.

- 담당·파일/함수·독립 검증·완료 조건: [4개 에픽 + I0](../../plans/oct-4-parallel-closure/README.md), [29개 티켓](../../plans/oct-4-parallel-closure/tickets/INDEX.md).
- 실행 순서·실제 상태: [W0–W6](../../plans/oct-4-parallel-closure/WAVES.md). 개별 명령·관측·증거 범위는 기존 owning ticket이 기준이다.
- 제품 proof/binaries/admissions는 frozen `0e6c7e7e9494b63fdb33f4594df059817459d3b1` 기준이다. 후속 scope 수리/SG/native3는 clean `ae8f96bae1a0db1fc0378861b228cd5359080fa1`에 고정했다. 실제 join이 derived build-provenance 차이도 발견해 추가 consumer 수리·검증 중이다. mutable main이나 과거 receipt를 새 source 결과로 재표기하지 않는다.

## 1. 현재 실행 순서

| 순서 / 웨이브 | 해야 할 일 | 현재 경계 |
| --- | --- | --- |
| 1 / W3–W5 | derived provenance consumer 수리/정적 감사·focused 검증 → 새 fixed-source/control 결속 → 5제품 join | ae8f SG producer/독립 replay12repo·13,347files 및 native3 actual/replay `VERIFIED`. join v4는 정상 derived build revision의 raw equality로 exit2; 추가 수리 검증 중 |
| 2 / W3 | current0e6 proof로 cli/lo/mocha/uvicorn/zustand/django/typeorm/nushell의 원본8 admission 발행 | canonical issuer 실제 실행 중. terminal 미발행 저장소는 ready 아님. bat409 current v4는 이미 strict consumer 검증됨 |
| 3 / W4–W5 | 준비된 C3 저장소의 실제 Quanta/Semble·SG/OG/cs 캡처, required-cell outcomes·source-bound blind union 발행 | existing exploratory pre-review mode로 신규 후보 확보. 새 미검수 후보를 final quality나 0점으로 처리하지 않음 |
| 4 / W1→W3→W5 | SQLAlchemy/Zellij 실제 조정 완료·suite/pack/admission 발행, Tailscale 미결 정책 적용 후 재검수 | actual Opus 주간 한도로 SQL146·Zellij476pairs `BLOCKED`. SQL334pairs 보존. 서비스 reset 관측10월7일01:00KST. Tailscale rubric 입력도 `BLOCKED` |
| 5 / W1–W5 | 다른 lane의 fresh required cells와 독립 name/holdout 평가, 최종 합집합 검수·scoreboard·정책 판정 | 아래 에픽별 잔여와 입력 경계 적용. C3 NL·exact/typo/span·ARB·B09 분모를 합산하지 않음 |
| 6 / W6 | exact producer/source pair·hosted CI·Linux release·실제 운영 gate | authorized host/path/config/state/retention/rollback 입력 `BLOCKED`. local proof를 배포/복구 증거로 승격하지 않음 |

실제 build/test/model/Docker/native/scale jobs는 root가 직렬 실행한다. 에픽별 source 조사·fixture·입력 PREPARE와 순수 readback은 병렬 가능하다. 준비된 저장소는 실패 sibling 때문에 대기하지 않으며 미실행/실패 셀을 ledger에서 제외하지 않는다.

## 2. 에픽별 남은 일

### E1 — 라벨·admission·독립 평가

- [E1-01](../../plans/oct-4-parallel-closure/tickets/O4-E1-01-original-review-resume.md): SQLAlchemy/Zellij의 실제 조정 잔여를 완료한다. 한도 실패·유효 partial raw를 보존하며 final receipt를 합성하지 않는다. Tailscale의 필터 패키지 밖 UDP 상태 테스트에 대한 grade1/3 경계를 확정해야 한다.
- [E1-02](../../plans/oct-4-parallel-closure/tickets/O4-E1-02-supplemental-labels.md)·[E1-06](../../plans/oct-4-parallel-closure/tickets/O4-E1-06-final-pool-and-scoreboards.md): 새 actual 응답의 미판단 합집합만 실제 두 reviewer+adjudicator로 검수하고 재채점한다. bat 원본358+supplemental51의409판단은 재수행하지 않는다. AI 판단을 human review로 표시하지 않는다.
- [E1-03](../../plans/oct-4-parallel-closure/tickets/O4-E1-03-admission-and-split.md): 나머지 current admissions와 후속 merged revisions를 canonical suite/pack/license/split/proof에 연결한다. 원본 source107 admissions는 과거 scope다.
- [E1-04](../../plans/oct-4-parallel-closure/tickets/O4-E1-04-precise-name-span.md): Gin4 control을 전체 exact/name/typo population으로 확장해 declaration-name span·ID 회수를 file hit와 별도 평가한다.4개 성공을1196개 또는 typo 전체 성공으로 바꾸지 않는다.
- [E1-05](../../plans/oct-4-parallel-closure/tickets/O4-E1-05-untouched-holdout.md): 준비된 미사용12repo/6079files의 license approver·사전 acceptance/critical strata·exposure를 확정하고 독립 gold/holdout을 발행한다. 입력 미결 `BLOCKED`다.
- [E1-07](../../plans/oct-4-parallel-closure/tickets/O4-E1-07-bounded-bootstrap.md):1196-row의 full-caller cold cost/목표·memory ceiling을 판정한다. bat20 whole-verdict의 추가 numeric kernel 최적화는 actual profile에서 병목 조건이 성립하지 않아 해당 범위 `NOT_APPLICABLE`이다.

### E2 — native 범위·응답·캡처

- [E2-02](../../plans/oct-4-parallel-closure/tickets/O4-E2-02-external-index-universe.md): 새 fixed-source SG scope epoch를 닫는다. OpenGrok의 전체 live-doc/source UID·auxiliary 관측을 canonical actual query 전후의 immutable index/endpoint/consumer binding으로 연결한다. 현재 whole indexed-universe flag는false다.
- [E2-03](../../plans/oct-4-parallel-closure/tickets/O4-E2-03-required-cells-and-scheduling.md)·[E2-04](../../plans/oct-4-parallel-closure/tickets/O4-E2-04-fresh-five-product-captures.md): current required cells별 실제 completion/refusal/missing과 blind union을 발행한다. exact1196, prefix/infix/components, default/explicit typo, no-answer, C3 NL240, Gin20, ARB original/adapted, B09 OSA/CLARC/CSN은 각 입력·unit별로 유지한다.
- [E2-01](../../plans/oct-4-parallel-closure/tickets/O4-E2-01-native-completed-timer.md)·[E2-05](../../plans/oct-4-parallel-closure/tickets/O4-E2-05-semble-process-attribution.md): 정식 반복 실행에서 completed-response boundary와 Semble parent/process 비용 귀속을 검증한다. 기존 timer/phase 구현을 다시 만들지 않는다.
- [E2-06](../../plans/oct-4-parallel-closure/tickets/O4-E2-06-quality-only-warmup.md): bat 밖에서 warmup0을 채택할 경우에만 자체 protocol/normalized rows/status/f64 parity를 실행한다. 그 전에는1회 유지하며 정식 speed에는0회 정책을 적용하지 않는다.

### E3 — 선택·운영 안전성의 잔여 proof

- [E3-01](../../plans/oct-4-parallel-closure/tickets/O4-E3-01-active-selection-race.md): 별도 OS-child의 같은 retention race 범위를 기존 supported refusal/view 계약에서 판정한다. dispatcher/runtime owner·daemon proof를 그 OS 시나리오로 재명명하지 않는다.
- [E3-04](../../plans/oct-4-parallel-closure/tickets/O4-E3-04-maintenance-health-metering.md)·[E3-05](../../plans/oct-4-parallel-closure/tickets/O4-E3-05-publish-timeout-replay.md): actual daemon slow-disk3-cadence, 기본30초 별도 OS-process admitted publish timeout→operation inspect→exact replay 범위를 검증한다.
- [E3-06](../../plans/oct-4-parallel-closure/tickets/O4-E3-06-operator-event-proof.md): 다른 OS UID와 Linux process-truth 범위는 미실행이다. 실제 입력·지원 seam을 정적 대조하고 가능한 proof를 준비한다.
- [E3-02](../../plans/oct-4-parallel-closure/tickets/O4-E3-02-admission-pin-transfer.md)는 현 Accepted 계약에서 `NOT_APPLICABLE`; 강화 계약 채택·새 실제 반례가 있을 때만 재개한다. [E3-03](../../plans/oct-4-parallel-closure/tickets/O4-E3-03-atomic-active-query-rpc.md)의 지원7 Active variant single-RPC 구현은 재작성하지 않는다. 후속 효과/운영 qualification만 해당 scope에서 수행한다.

### E4 — 비용 원인·조건부 변경·성능·scale

- [E4-01](../../plans/oct-4-parallel-closure/tickets/O4-E4-01-index-phase-profile.md): seal streaming/posting scan·fsync/physical I/O·token 비용을 독립 계측으로 분리한다. mixed lifecycle/system CPU를 특정 원인의 비용으로 바꾸지 않는다.
- [E4-02](../../plans/oct-4-parallel-closure/tickets/O4-E4-02-generation-durable-barriers.md)·[E4-04](../../plans/oct-4-parallel-closure/tickets/O4-E4-04-source-token-authority.md): 실제 병목 조건 뒤에만 durable group barrier 또는 token authority를 채택한다. 변경 시 old/new root·fault injection·delta/delete/no-op/reopen·memory/build 비용을 독립 검증한다.
- [E4-03](../../plans/oct-4-parallel-closure/tickets/O4-E4-03-ascii-scanner-decision.md): current source의 ASCII 전체 호출 A/B로 유지/수정/철회를 판정한다. 원본 root가 없는 과거+8.75% 관측을 새 proof로 소비하지 않는다.
- [E4-05](../../plans/oct-4-parallel-closure/tickets/O4-E4-05-release-scale-load.md): default4096의30초 timeout 및32768의4M-posting capacity refusal을 실패로 유지하고 지원 목표/비용을 판정한다. large300초/256MiB 진단 성공·4096 OS restart 성공을 default capacity 성공으로 덮어쓰지 않는다.
- [E4-06](../../plans/oct-4-parallel-closure/tickets/O4-E4-06-qualified-performance.md): frequency authority와 지속 quiet-host admission을 확보한 뒤 같은 binaries/input·응답 경계·사전 acceptance의 정식 반복 성능을 실행한다. 현재 Darwin frequency 입력은 `BLOCKED`다.
- [E4-07](../../plans/oct-4-parallel-closure/tickets/O4-E4-07-policy-and-semantic-residuals.md): default OSA23 잔여·NL/semantic misses를 독립 qrels/name/no-answer/holdout으로 RCA하고 정책을 판정한다. explicit OSA1의 성공을 default 성공으로 전용하지 않는다.

### I0 — 통합·source·release

- [I0-01](../../plans/oct-4-parallel-closure/tickets/O4-I0-01-ownership-and-contract-freeze.md)·[I0-02](../../plans/oct-4-parallel-closure/tickets/O4-I0-02-matching-source-proof.md): SHARED 파일을 단일 owner가 통합하고 source 변경마다 영향 gates/binaries/admissions/captures를 재판정한다. frozen0e6 proof를 후속 main 전체 proof로 승격하지 않는다. exact hosted CI는 `NOT_RUN`이다.
- [I0-03](../../plans/oct-4-parallel-closure/tickets/O4-I0-03-release-operational-gates.md): 실제 producer pair·Linux target/config/state/retention/rollback 입력과 typed operational authority/recipes를 연결한 뒤 deploy/activate/restore/rollback을 실행한다. local259 infrastructure pass나 실패 ledger를 CODE/release/actions 종료로 세지 않는다.

## 3. 잔여 계산·거절 규칙

- 5개 handoff의 초기 실패 숫자나 watcher 생존 여부 대신 current source·원본 terminal·canonical consumer 결과를 사용한다. 새 namespace를 쓰며 실패 raw/receipt를 덮어쓰지 않는다.
- `VERIFIED`는 실행한 해당 범위, `FAILED`는 실행 실패, `BLOCKED`는 필요한 입력 부재, `NOT_RUN`은 미실행이다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- unknown/missing/unresolved를0점·no-answer·완료로 채우지 않는다. cap·completed-empty·timeout·capacity refusal을 서로 구별한다.
- 품질·name-span·unseen/human·speed·OS restart·Linux/운영 qualification은 각각 독립적으로 판정한다. 전체29개 종료는 각 요청 완료 조건이 실제로 충족된 뒤만 가능하다.
