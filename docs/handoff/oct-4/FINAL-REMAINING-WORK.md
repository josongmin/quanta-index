# 2026-10-05 통합 잔여 작업

원본 [agent-1](agent-1.md)·[agent-2](agent-2.md)·[agent-3](agent-3.md)·[agent-4](agent-4.md)·[agent-5](agent-5.md)의 합집합에서 후속 구현·실행으로 충족된 범위를 제거한 현재 작업 목록이다. 전체 29개 종료나 품질·성능·배포 qualification을 뜻하지 않는다.

- 담당·파일/함수·독립 검증·완료 조건: [4개 에픽 + I0](../../plans/oct-4-parallel-closure/README.md), [29개 티켓](../../plans/oct-4-parallel-closure/tickets/INDEX.md).
- 실행 순서·실제 상태: [W0–W6](../../plans/oct-4-parallel-closure/WAVES.md). 개별 명령·관측·증거 범위는 기존 owning ticket이 기준이다.
- 제품 proof/binaries/admissions는 frozen `0e6c7e7e9494b63fdb33f4594df059817459d3b1` 기준이다. 완료된 scope 수리/SG/native3는 clean `ae8f96bae1a0db1fc0378861b228cd5359080fa1` 범위다. derived provenance·frozen-context 경로 수리는 clean `628541e150192b4aaf0ff4ba54566ae28c6ca25f`에 고정했고 retained bat canonical raw replay가 통과했다. 후속 report/verdict JSON 타입 결속을 수리한 clean `97eedd11b70e76c66985b15a968211a2faf92c6d`의 focused 회귀203/203·0failed/skipped가 통과했다. 새 SG12repo/13,347files 독립 replay·bat native3 actual/replay·retained full5제품 join은 통과했다. remaining8 Q/S·native captures/독립 replays/full5제품 joins도 완료됐다. 후속 native metadata4개 타입 결속 수리의 실제 RED4재현/GREEN12passed를 main에 통합했다. current9 신규151tasks/742unjudged pairs가 준비됐지만 actual 판단·final qualification은 남아 있다. mutable main이나 과거 receipt를 새 source 결과로 재표기하지 않는다.
- 원본 agent-4의 과거 임시 증거15고유경로는 현재 없어 replay 가능한 current proof로 사용하지 않는다. 기존50 Markdown의 상대 링크1,246개·29 ticket 배치는 확인했으며 증거 가용성 경계는 [I0-01](../../plans/oct-4-parallel-closure/tickets/O4-I0-01-ownership-and-contract-freeze.md)에 기록했다.

## 현재 진행률 — 범위별 실제 분모

전체29티켓의 공수 가중치는 산정하지 않았다. 아래 비율은 전체 제품 완료율로 합산하지 않는다.

- **SQLAlchemy·Zellij:** [agent-2](agent-2.md)의 C3 검색 평가용 소스 저장소다. 각각 자연어 질의20개를 사용해 검색 결과 파일의 관련성을 평가한다. Quanta Index에 해당 제품을 도입하거나 교체하는 작업이 아니다.
- **검색 정답 검수의 최종 판정:** 두 reviewer의 판단을 adjudicator가 검수해 검색 정답을 확정한다. 기존 모델의 한도 소진으로 최종 AI 판정자를 교체했다. 이전의 `대체 조정` 표현은 이 작업을 뜻했다.
- **Linux:** 원본 [agent-1의 Release 잔여](agent-1.md#3-remaining-work-and-decision-order)에 포함된 daemon 릴리스·운영 검증이다. 실제 두 UID 소켓 접근 테스트는 그 권한 검증의 누락을 추가로 보완한 component 증거다. 이 테스트의 통과는 Linux 릴리스·배포 완료를 뜻하지 않는다.

| 범위 | 현재 | 비율 | 남은 작업 |
| --- | --- | --- | --- |
| 원본5개 통합·29티켓 웨이브 배치 | 29/29·중복0·링크 누락0 | 100% | 후속 결과를 기존 티켓에 반영 |
| Gin exact symbol/name 실제 캡처·독립 채점 | 1,196/1,196질의 | 100% | 다른 name/typo cells·최신 main qualification은 별도 |
| C3 NL 기존 라벨의 기본 admission 및5제품 캡처/독립 replay/join | 9/12repo·180/240tasks | 75% | SQLAlchemy·Zellij·Tailscale3repo와 후속 final revisions |
| current9 신규 보충 검수 | 0/742query-file pairs·151tasks 입력 준비 | 0% | 실제 두 reviewer+adjudicator→canonical merge/admission→재채점 |
| 검색 정답 검수 최종 판정 — SQLAlchemy | 334/480pairs | 69.6% | 146pairs·모델 한도 대기 |
| 검색 정답 검수 최종 판정 — Zellij | 0/476pairs | 0% | 476pairs·모델 한도 대기; 기존 두 reviewer 보존 |
| 권한 검증 보완 — Linux 실제 두 UID 테스트 | 1/1 test | 100% | shipping daemon/Linux release·운영 범위는 별도 |
| 실제 runtime OS-child의 selection race·slow disk·기본 SDK timeout/replay | 3/3시나리오 | 100% | shipping release·최신 source qualification은 I0 범위 |
| 실제 deploy/activate/restore/rollback | 이번 요청의 운영 실행 미착수 | 0% | authorized host/config/state/retention/rollback 입력 |

신규 보충 검수와 SQLAlchemy/Zellij 검색 정답 검수의 최종 판정 잔여는 최소1,364개 query-file pairs다. 역할별 호출 수는 이 pair 수와 다르며 Tailscale 정책, 독립 holdout, 다른 lane과 성능/운영 검증은 이 분모 밖이다.

## 1. 현재 실행 순서 — 코드 먼저

2026-10-05 사용자 지시에 따라 현재 작업은 구현·회귀 fixture·최소 owner 검증까지다. 모델 검수·대규모 capture·성능·CI·배포 입력은 후속 qualification이며 코드 작업의 대기 조건으로 사용하지 않는다.

| 코드 웨이브 | 담당 / 작업 | 현재 구현 상태 |
| --- | --- | --- |
| C1 / 병렬 구현 | E2 native collector/consumer 연결, E4 scanner A/B 비교 경로 | OpenGrok `native_index_reader`를 capture/verify 전후 bracket에 연결. 전수 live docs·source UID·directory/settings 역할·owned execution/raw custody 검사 구현. scanner `--scanner-ab` 구현 및 기존 on/off 회귀 포함21passed |
| C2 / 근거 있는 수리 | 발견된 collector correctness 결함 | Sourcegraph 비차단 pipe의 `EAGAIN` 재대기 수리. OpenGrok JSON key 순서 고정; deployed ABI의 index-only objuid와 directory parent dirpath 반영 |
| C3 / 중앙 통합 | 독립 positive/negative fixture 및 좁은 owner rail | E2 native/collector/pipe35passed, E4 scanner/on-off21passed. 명령과 검증 범위는 기존 E2-02/E4-03에 기록. 모델 quota·운영 입력 없이 진행 |

E1 name-span producer/evaluator 및 E3 selection-retirement·maintenance cancellation·publish-timeout 구현은 현재 source에 이미 있다. 증거를 더 수집해야 하는 항목을 새 코드 결함으로 취급하지 않는다. barrier/token/storage 최적화는 채택한 계약이나 확인된 원인이 있을 때만 변경한다.

### 후속 qualification 실행 순서

| 순서 / 웨이브 | 해야 할 일 | 현재 경계 |
| --- | --- | --- |
| 1 / W3–W5 | 새 미판정151tasks/742pairs 실제 검수·canonical labels·재채점 | current9 blind pool9/9 PREPARED. source97 Q/S8·native8·독립 replays/full5 joins VERIFIED; bat20 및 required8 각0–5 common eligible는 diagnostic 범위. actual reviewer/adjudicator quota BLOCKED, unknown을0점으로 처리하지 않음 |
| 2 / W3 | current0e6 admission에 후속 final merged revisions 연결 | canonical issuer exit0/8 terminal·aggregate 및 각16-input hash readback VERIFIED. bat 포함9repo/180tasks/4,262judgments의 admission 범위; 후속 final revisions·나머지3개는 미완료 |
| 3 / W4–W5 | 나머지3 C3 저장소와 다른 lane의 required cells, 후속 source/qrel 영향 재검증 | all12 PREPARE9 ready/3 admission BLOCKED; current9의 source97 captures/joins와 blind union 완료. metadata owner 수리의 focused proof는 별도 scope이며 새 source의 fresh native qualification은 아직 미발행 |
| 4 / W1→W3→W5 | SQLAlchemy/Zellij 검색 정답 검수의 최종 판정 완료·suite/pack/admission 발행, Tailscale 미결 정책 적용 후 재검수 | actual Opus 주간 한도로 SQL146·Zellij476pairs `BLOCKED`. SQL334pairs 보존. 서비스 reset 관측10월7일01:00KST. Tailscale rubric 입력도 `BLOCKED` |
| 5 / W1–W5 | 다른 lane의 fresh required cells와 독립 name/holdout 평가, 최종 합집합 검수·scoreboard·정책 판정 | 아래 에픽별 잔여와 입력 경계 적용. C3 NL·exact/typo/span·ARB·B09 분모를 합산하지 않음 |
| 6 / W6 | exact producer/source pair·hosted CI·Linux release·실제 운영 gate | authorized host/path/config/state/retention/rollback 입력 `BLOCKED`. local proof를 배포/복구 증거로 승격하지 않음 |

실제 build/test/model/Docker/native/scale jobs는 root가 직렬 실행한다. 에픽별 source 조사·fixture·입력 PREPARE와 순수 readback은 병렬 가능하다. 준비된 저장소는 실패 sibling 때문에 대기하지 않으며 미실행/실패 셀을 ledger에서 제외하지 않는다.

## 2. 에픽별 남은 일

### E1 — 라벨·admission·독립 평가

- [E1-01](../../plans/oct-4-parallel-closure/tickets/O4-E1-01-original-review-resume.md): SQLAlchemy/Zellij 검색 정답 검수의 최종 판정을 완료한다. 한도 실패·유효 partial raw를 보존하며 final receipt를 합성하지 않는다. Tailscale의 필터 패키지 밖 UDP 상태 테스트에 대한 grade1/3 경계를 확정해야 한다.
- [E1-02](../../plans/oct-4-parallel-closure/tickets/O4-E1-02-supplemental-labels.md)·[E1-06](../../plans/oct-4-parallel-closure/tickets/O4-E1-06-final-pool-and-scoreboards.md): 새 actual 응답의 미판단 합집합만 실제 두 reviewer+adjudicator로 검수하고 재채점한다. bat 원본358+supplemental51의409판단은 재수행하지 않는다. AI 판단을 human review로 표시하지 않는다.
- [E1-03](../../plans/oct-4-parallel-closure/tickets/O4-E1-03-admission-and-split.md): 나머지 current admissions와 후속 merged revisions를 canonical suite/pack/license/split/proof에 연결한다. 원본 source107 admissions는 과거 scope다.
- [E1-04](../../plans/oct-4-parallel-closure/tickets/O4-E1-04-precise-name-span.md): product0e6/driverb55의 Gin 전체 exact1,196 symbol/name 실제 캡처·독립 채점은 완료했다(MRR@10=1, 평균 Recall@10=0.9985493335876968;4 capped). 남은 지원 가능한 다른 name/typo cells와 최신 source 영향을 판정하며 file-only/미지원 unit은 회수 성공으로 계산하지 않는다.
- [E1-05](../../plans/oct-4-parallel-closure/tickets/O4-E1-05-untouched-holdout.md): 준비된 미사용12repo/6079files의 license approver·사전 acceptance/critical strata·exposure를 확정하고 독립 gold/holdout을 발행한다. 입력 미결 `BLOCKED`다.
- [E1-07](../../plans/oct-4-parallel-closure/tickets/O4-E1-07-bounded-bootstrap.md):1196-row의 full-caller cold cost/목표·memory ceiling을 판정한다. bat20 whole-verdict의 추가 numeric kernel 최적화는 actual profile에서 병목 조건이 성립하지 않아 해당 범위 `NOT_APPLICABLE`이다.

### E2 — native 범위·응답·캡처

- [E2-02](../../plans/oct-4-parallel-closure/tickets/O4-E2-02-external-index-universe.md): fixed-source97eedd SG12/13,347files native replay는 완료됐다. OpenGrok fresh readonly Tomcat/config/source/webapp/GET/PUT403 계약·root directory/project 검증을 통합했고 focused48passed 및 upstream raw String 계약1passed다. retained corpus의 독립 재구성과 전수17,615/source13,347/directory4,256/settings12 실제 raw replay도 통과했다. 실제 새 Java/Lucene capture·service loaded-reader 결속은 남아 있으며 기존 whole indexed-universe qualification은false다.
- [E2-03](../../plans/oct-4-parallel-closure/tickets/O4-E2-03-required-cells-and-scheduling.md)·[E2-04](../../plans/oct-4-parallel-closure/tickets/O4-E2-04-fresh-five-product-captures.md): current required cells별 실제 completion/refusal/missing과 blind union을 발행한다. exact1196, prefix/infix/components, default/explicit typo, no-answer, C3 NL240, Gin20, ARB original/adapted, B09 OSA/CLARC/CSN은 각 입력·unit별로 유지한다.
- [E2-01](../../plans/oct-4-parallel-closure/tickets/O4-E2-01-native-completed-timer.md)·[E2-05](../../plans/oct-4-parallel-closure/tickets/O4-E2-05-semble-process-attribution.md): 정식 반복 실행에서 completed-response boundary와 Semble parent/process 비용 귀속을 검증한다. 기존 timer/phase 구현을 다시 만들지 않는다.
- [E2-06](../../plans/oct-4-parallel-closure/tickets/O4-E2-06-quality-only-warmup.md): bat 밖에서 warmup0을 채택할 경우에만 자체 protocol/normalized rows/status/f64 parity를 실행한다. 그 전에는1회 유지하며 정식 speed에는0회 정책을 적용하지 않는다.

### E3 — 선택·운영 안전성의 잔여 proof

- [E3-01](../../plans/oct-4-parallel-closure/tickets/O4-E3-01-active-selection-race.md)·[E3-04](../../plans/oct-4-parallel-closure/tickets/O4-E3-04-maintenance-health-metering.md)·[E3-05](../../plans/oct-4-parallel-closure/tickets/O4-E3-05-publish-timeout-replay.md)의 요청 OS-child3시나리오는 실제 통과해 해당 잔여에서 제거했다. 같은 main bytes에 검증한 delta를 통합했으며 shipping release·후속 source qualification은 아래 I0에서 별도 수행한다.
- [E3-06](../../plans/oct-4-parallel-closure/tickets/O4-E3-06-operator-event-proof.md): Linux 실제2UID UDS component v4는1/1passed·2.59s/exit0이며 source/overlay 전후·owned cleanup도 `VERIFIED`다. 검증한 fixture/policy2파일을 main에 통합했고 v1–v3 실제 실패는 보존했다. shipping Linux daemon/release는 미실행이다. P11 운영 process-truth는 authorized host/path/config/state/retention/rollback 입력 `BLOCKED`로 별도 유지한다.
- [E3-02](../../plans/oct-4-parallel-closure/tickets/O4-E3-02-admission-pin-transfer.md)는 현 Accepted 계약에서 `NOT_APPLICABLE`; 강화 계약 채택·새 실제 반례가 있을 때만 재개한다. [E3-03](../../plans/oct-4-parallel-closure/tickets/O4-E3-03-atomic-active-query-rpc.md)의 지원7 Active variant single-RPC 구현은 재작성하지 않는다. 후속 효과/운영 qualification만 해당 scope에서 수행한다.

### E4 — 비용 원인·조건부 변경·성능·scale

- [E4-01](../../plans/oct-4-parallel-closure/tickets/O4-E4-01-index-phase-profile.md): seal streaming/posting scan·fsync/physical I/O·token 비용을 독립 계측으로 분리한다. mixed lifecycle/system CPU를 특정 원인의 비용으로 바꾸지 않는다.
- [E4-02](../../plans/oct-4-parallel-closure/tickets/O4-E4-02-generation-durable-barriers.md)·[E4-04](../../plans/oct-4-parallel-closure/tickets/O4-E4-04-source-token-authority.md): 실제 병목 조건 뒤에만 durable group barrier 또는 token authority를 채택한다. 변경 시 old/new root·fault injection·delta/delete/no-op/reopen·memory/build 비용을 독립 검증한다.
- [E4-03](../../plans/oct-4-parallel-closure/tickets/O4-E4-03-ascii-scanner-decision.md): 별도 scanner A/B comparator/CLI와 독립 회귀는 구현했고 기존 on/off 포함21passed다. 실제 whole-call 측정 후 유지/수정/철회는 후속 qualification이다. 원본 root가 없는 과거+8.75% 관측을 새 proof로 소비하지 않는다.
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
