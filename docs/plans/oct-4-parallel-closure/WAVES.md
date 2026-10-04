# OCT-04 웨이브 실행 계획

**W0–W6, 7개 웨이브. 기존 4개 에픽 + I0의 29개 티켓을 모두 배치한다.** 새 구현 티켓이나 별도 evidence 체계를 추가하지 않는다.

[에픽·소유권 지도](README.md) · [티켓 인덱스](tickets/INDEX.md)

- 아래는 실행 계획이다. 작성 당시의 `PLANNED`/`NOT_RUN`은 과거 기준이며, 이후 구현 반영이나 현재 실행 결과를 뜻하지 않는다. 실제 결과는 원래 owning ticket의 명령·관측·scope로 판정한다.
- 작성 시작 HEAD: `main@f23af16f436c76ad4a700b75de4dd5b5771f56a6`. 앞선 감사 문서 27개가 dirty였다. 그 변경을 보존하며 실행 시 source/dirty/ownership을 다시 확인한다.
- 웨이브는 **주된 수행 단계와 인계 순서**다. 모든 repository/티켓을 한꺼번에 기다리는 전역 장벽이 아니다. 준비된 repository·claim별로 다음 단계에 진입한다.
- W2에서 미선택한 최적화와 모든 FAILED/BLOCKED/NOT_RUN 범위는 원래 티켓에 남는다. baseline을 발행한 것과 전체 29개 종료를 구분한다.

## 2026-10-04 실행 스냅샷 — source107

이 표는 초기 티켓 배치를 바꾸지 않으며 전체29개 종료를 뜻하지 않는다. 실제 명령·출력 경로·scope는 owning ticket이 기준이다.

| 웨이브 | 확보한 실행 결과 | 남은 실제 작업 |
| --- | --- | --- |
| W0 | root 단일 통합/중앙 실행, clean `1071692b`와 matching proof source 고정 | 후속 product 변경 시 새 epoch 발행 |
| W1 | owner regressions, Gin4 exact-name 실제 capture/scoring, Sourcegraph12repo native replay, OpenGrok24 sweeps/path-bearing posting replay `VERIFIED` | SQLAlchemy/Zellij/Tailscale 실제 role 판단 재개; whole OpenGrok UID/auxiliary/endpoint bracket; untouched holdout license·사전 acceptance 입력/발행 |
| W2 | 실제 반례의 live-BM25, maintenance fatal ownership/terminal, fixture 수리 및 지원 Active single-RPC 검증 완료 | bootstrap/scanner/barrier/token-authority 최적화는 실제 비용 조건 판정 대기. E3-02는 현 Accepted refusal 계약에서 근거 있는 `NOT_APPLICABLE` |
| W3 | Contract Python788/Rust191, fresh release SDK27, context replay 및 bat/cli/lo/mocha/uvicorn/zustand/nushell admission `VERIFIED` | Django/TypeORM fresh admission 계속; source107 hosted CI 조회는 `[]`/`NOT_RUN` |
| W4 | source107 default small/medium scale·small open-loop3743requests·bat native3제품60rows/5제품 union51pairs·Semble phase replay `VERIFIED`; large timeout/ANN delete seal·independent UUID/deletion regressions `FAILED` | physical row identity를 보존한 ANN 수리→회귀→새 source gate. actual supplemental 판단→새 admission/fresh final pair; parity/remaining tiers/qualification 계속 |
| W5 | name/source oracle 및 source-bound 원본 labels 유지 | 새5제품 blind union의 actual supplemental review·최종 qrel/scoreboard·독립 holdout 정책 판정 |
| W6 | local infrastructure/owner proof와 운영 qualification 경계를 기록 | actual authorized Linux target/config/state/rollback 입력 `BLOCKED`; exact producer pair·CI·배포/활성화/restore/rollback 미실행 |

- ready 저장소 하나의 성공은 required8/12 전체 matrix 성공이 아니다. 개별 실행 driver의 required ledger에서 미선택 셀은 `NOT_RUN`으로 유지한다.
- provider quota 응답의 재개 시각은2026-10-04 23:40 KST다. 무응답 pair를 grade/no-answer로 채우지 않는다. AI 실제 판단과 human provenance를 구분한다.
- native/Gin/phase 진단과 Darwin/hash-dev owner proof를 독립 gold·learned quality·속도·Linux release qualification으로 승격하지 않는다.
- current 문서 점검 `VERIFIED`:29 ticket files가 INDEX/WAVES에 모두 나타나고1177 relative links가 실제 대상에 연결되며 빈 section heading은 없었다. `git diff --check`도 exit0이다. 문서 점검을 제품 실행 결과로 합산하지 않는다.

## 중앙 실행의 배치 규칙

- **먼저 코드·독립 fixture·정적 점검을 병렬로 준비하고, 실행 검증은 I0가 모아서 수행한다.** 에픽별로 pytest/Rust tests·builds·실제 capture·모델/성능 jobs를 따로 시작하지 않는다.
- 현재 4개 실행 슬롯은 **root가 E1+I0**, 나머지 3개가 **E2/E3/E4**를 맡는다. 에픽 소유권은 유지하고 SHARED 파일은 root가 통합한다. I0는 별도 다섯 번째 실행 슬롯을 요구하지 않는다.
- W1에서는 source 조사·producer/consumer 구현·반례 fixture·실행 입력 준비를 우선한다. source에서 확정할 수 없는 재현·병목·역할 검수는 중앙 실행 배치 전까지 `NOT_RUN`이며, 입력이 없으면 해당 scope만 `BLOCKED`다.
- W2의 조건부 변경은 이미 확보한 실제 반례/측정 또는 채택한 계약에 근거한다. 새 실행 증거가 필요한 경우, 준비된 W1 fixture/profile을 I0가 한 배치로 판정한 뒤 **필요한 W2만 재개**한다. 추정으로 pin/token/storage/scanner 변경을 선행하지 않는다.
- W3에서는 **통합 → 정적 점검 → owner regressions → 영향 surface gates → matching binary/input → admission ISSUE**를 순서대로 수행한다. 재현 배치에서 새 결함이 확인되면 해당 W2 수리 후 W3 영향 범위를 다시 검증한다.
- W4의 제품 캡처·ingest·정식 성능·scale은 같은 host에서 직렬 실행한다. W5의 실제 final-pool 검수·재채점은 같은 raw/qrel 권위를 사용한다. W6의 provider/Linux/restore/actions는 각 실제 입력을 요구한다.
- 일괄 검증은 한 번의 실행으로 모든 웨이브를 닫는다는 뜻이 아니다. 코드/계약이 바뀌면 영향 검증을 다음 중앙 배치에 모으고, 필요한 proof가 없으면 다음 단계로 승격하지 않는다.

## 1. 웨이브 요약

| 웨이브 | 기준 티켓 | 병렬 작업 / 담당 | 다음 단계 진입 조건 |
| --- | --- | --- | --- |
| [W0 — 소유권·실행 범위 고정](waves/W0-ownership-and-scope.md) | 1개 | 담당·SHARED 파일·host resource와 이번 실행 scope를 먼저 고정한다. | 선택 scope의 파일/계약/출력 namespace·자원 담당 확정 |
| [W1 — 근거·정답·producer 병렬 준비](waves/W1-evidence-and-producers.md) | 14개 | 라벨/독립 oracle·native scope/timer/controller·safety 반례·비용 측정 코드를 준비한다. | 선택 cohort의 code-ready proposals·독립 fixture·실행 입력 준비; 실제 proof는 중앙 배치 |
| [W2 — 확인된 결함 수리·선택 최적화](waves/W2-repairs-and-selected-optimizations.md) | 5개 | 증거가 확보된 계약 실패를 수리하고 병목 기반 최적화를 선택한다. | 선택 source에 필요한 수리·회귀 fixture 준비; 미확인 조건은 중앙 재현 뒤 판정 |
| [W3 — 소스 통합·검증·admission ISSUE](waves/W3-source-validation-and-admission.md) | 2개 | producer까지 통합하고 owner/surface 검증을 일괄 수행한 뒤 같은 source의 admission을 발행한다. | 필수 owner/surface gates·matching binaries·suite/split/admission 결속 |
| [W4 — 실제 캡처·성능·scale](waves/W4-native-capture-performance-and-scale.md) | 4개 | 필요한 warmup parity 뒤 native captures·반복 성능·tier별 load/restart를 실행한다. | 선택 required cells의 immutable raw/coverage·clock/host·claim별 proof 확보 |
| [W5 — 최종 검수·재채점·정책 판정](waves/W5-final-scoring-and-policy.md) | 2개 | 새 candidate pool을 실제 검수하고 독립 gold/holdout에서 정책을 판정한다. | 해당 scope의 final qrel/scoreboard·분모/CI·정책 disposition; source 변경 시 재검증 |
| [W6 — release·운영·전체 잔여 판정](waves/W6-release-and-final-closure.md) | 1개 | CODE/release/actions를 별도 증거로 판정하고 모든 29개 티켓의 미완료를 남긴다. | 요청 qualification의 필수 proof 충족; BLOCKED/NOT_RUN이면 해당 작업 잔여 |

### 중앙 검증 배치를 넣는 위치

검증을 모으되, W2의 착수에 필요한 반례·병목 판정까지 마지막으로 미루지 않는다. 준비된 scope마다 아래 배치를 수행하고, 같은 host의 무거운 실행은 직렬로 배정한다.

| 위치 | root / I0가 실행할 배치 | 다음 작업 |
| --- | --- | --- |
| W1 준비 뒤 | 독립 fixture의 최소 owner 재현, 필요한 실제 역할 검수·native scope 확인·whole-call profile | 확인된 correctness 실패와 채택한 계약은 W2 수리. 병목이 미확인인 최적화는 판정 대기 |
| W2 통합 뒤 / W3 | 정적 점검→영향 owner regressions→public API·wire·selection 등 mandatory gates→matching binary/input 검증 | 통과한 같은 source에서만 admission ISSUE, W4 진입 |
| W4 | 실제 5제품 캡처·raw replay, 조건부 warmup parity, 반복 성능·tier/load/OS restart | 필수 셀 outcome과 blind union을 W5에 인계 |
| W5 | 실제 final-pool 검수→독립 qrels 재채점→name/NL/holdout·정책 acceptance | source 변경이면 W3와 영향 W4 재개; 결과가 고정되면 W6 |
| W6 | CI·exact source pair·provider/Linux·backup/restore·실제 운영 gate | 모든 29개 티켓의 요청 scope를 실제 결과로 종료 판정 |

W1 재현 배치는 W3의 전체 source gate를 대신하지 않는다. 실패한 scope는 수리 owner에게 돌려주고, 다음 중앙 배치에서 해당 반례와 영향 검증을 다시 실행한다. 다른 ready scope의 구현·입력 준비는 계속한다.

## 2. 실행 경로와 같은 웨이브 내부 순서

| 목표 | 경로 | 반드시 지킬 조건 |
| --- | --- | --- |
| 현 계약의 baseline quality | W0→W1→W3→W4 품질→W5 재채점 | W1에서 알려진 해당 scope 결함이 나오면 W2 수리 필수. warmup1 가능; completed timer 부재 시 기존 transport의 정확한 quality diagnostic 의미만 유지 |
| 구조 수리·선택 개선 효과 | W0→W1→선택 W2→W3→W4→W5 | 포함 변경의 독립 owner oracle·mandatory gates·matching binaries·affected native cells/performance 요구 |
| name 회수·unseen 정책 | W1 E1-04/05 준비→W3/4 해당 capture→W5 | 독립 name/NL gold·holdout quota/exposure·사전 policy acceptance. 준비된 file report와 별도 qualification |
| baseline completed speed | W1 timer/scope/phase→W3 source+admission→W4 성능 | 20 frozen tasks·5fresh roots·route당1000 measured warm observations·warmup≥1·한 Quanta route·host admission. single-RPC와 XL tier 자동 선행 없음 |
| 대규모 capacity/tail/restart | W1 lifecycle→W3→W4 해당 tier/load/restart | 256/4096/32768 및 lifecycle/OS restart별 결과. timeout/posting-cap refusal은 성공으로 계산하지 않음 |
| CODE/release/actions | W1 input/target 준비→W3 current source→W6 자체 registry gate | CODE는 전체 품질/최적화를 자동 선행으로 하지 않는다. 품질/unseen/성능 포함 출하 주장에는 해당 W4/5 proof 추가 |

| 같은 웨이브 | 내부 순서 / 병렬 제한 |
| --- | --- |
| W1 | E1-01→E1-02는 repository별 순차. E1 name/holdout, E2 source 준비, E3 safety, E4 비용 조사는 에픽 간 병렬. 같은 파일은 한 owner가 통합 |
| W2 | E3-01 결과→E3-02→E3-03. E4-01 profile→E4-02; E4-01/03 after-scanner→E4-04. numeric과 구조 변경은 별도 owner이나 shared proposals는 I0 반영 |
| W3 | **모든 선택 producer/code PREPARE→I0 VALIDATE→E1 admission ISSUE**. E1-03 코드는 W1에서 준비하고 발행만 검증 뒤 수행 |
| W4 | warmup0 선택 시 E2-06 actual parity→canonical protocol/config/admission 재ISSUE→E2-04. 실제 capture/ingest/scale/정식 성능은 같은 host에서 직렬 |
| W5 | E1 final-pool 실제 검수→재채점/scoreboard→E4 독립 gold/holdout 기반 정책 판정. E4 RCA 준비는 먼저 가능 |
| W6 | source qualification과 Linux/provider/state/actions는 각 실제 prerequisite로 판정. source 변경 시 W3와 영향 범위 재검증 |

## 3. 기준 티켓 배치 — 29개

각 티켓은 기준 웨이브에 한 번만 배치한다. 이는 모든 작업 phase가 그 웨이브 안에서 끝난다는 뜻이 아니다.

| 웨이브 | 담당 | 티켓 | 주된 수행 단계 |
| --- | --- | --- | --- |
| [W0](waves/W0-ownership-and-scope.md) | I0 | [O4-I0-01](tickets/O4-I0-01-ownership-and-contract-freeze.md) | 공통 파일 소유권·계약·source epoch 관리 |
| [W1](waves/W1-evidence-and-producers.md) | E1 | [O4-E1-01](tickets/O4-E1-01-original-review-resume.md) | 원본 C3 검수 실패 복구와 실제 판단 발행 |
| [W1](waves/W1-evidence-and-producers.md) | E1 | [O4-E1-02](tickets/O4-E1-02-supplemental-labels.md) | 미검수 합집합 검수와 원본 라벨 병합 |
| [W1](waves/W1-evidence-and-producers.md) | E1 | [O4-E1-04](tickets/O4-E1-04-precise-name-span.md) | 정확한 선언 이름 span과 unit 회수 평가 |
| [W1](waves/W1-evidence-and-producers.md) | E1 | [O4-E1-05](tickets/O4-E1-05-untouched-holdout.md) | 독립 relevance와 미사용 holdout 발행 |
| [W1](waves/W1-evidence-and-producers.md) | E2 | [O4-E2-01](tickets/O4-E2-01-native-completed-timer.md) | 외부 제품의 completed-response 시간 경계 |
| [W1](waves/W1-evidence-and-producers.md) | E2 | [O4-E2-02](tickets/O4-E2-02-external-index-universe.md) | Sourcegraph·OpenGrok 전체 native 색인 범위 |
| [W1](waves/W1-evidence-and-producers.md) | E2 | [O4-E2-03](tickets/O4-E2-03-required-cells-and-scheduling.md) | 필수 셀 inventory와 실패 분리 스케줄 |
| [W1](waves/W1-evidence-and-producers.md) | E2 | [O4-E2-05](tickets/O4-E2-05-semble-process-attribution.md) | Semble process 비용의 phase 귀속 |
| [W1](waves/W1-evidence-and-producers.md) | E3 | [O4-E3-01](tickets/O4-E3-01-active-selection-race.md) | Active 선택·retention·view 획득 경합 재현 |
| [W1](waves/W1-evidence-and-producers.md) | E3 | [O4-E3-04](tickets/O4-E3-04-maintenance-health-metering.md) | 느린 디스크 metering과 readiness 분리 판정 |
| [W1](waves/W1-evidence-and-producers.md) | E3 | [O4-E3-05](tickets/O4-E3-05-publish-timeout-replay.md) | admitted publish timeout과 operation replay |
| [W1](waves/W1-evidence-and-producers.md) | E3 | [O4-E3-06](tickets/O4-E3-06-operator-event-proof.md) | 기존 operator diagnostics·process truth 검증 |
| [W1](waves/W1-evidence-and-producers.md) | E4 | [O4-E4-01](tickets/O4-E4-01-index-phase-profile.md) | 인덱싱 lifecycle 비용·resource 원인 분해 |
| [W1](waves/W1-evidence-and-producers.md) | E4 | [O4-E4-03](tickets/O4-E4-03-ascii-scanner-decision.md) | ASCII scanner 전체 호출 효과 판정 |
| [W2](waves/W2-repairs-and-selected-optimizations.md) | E1 | [O4-E1-07](tickets/O4-E1-07-bounded-bootstrap.md) | cold bootstrap의 결정적 bounded 계산 |
| [W2](waves/W2-repairs-and-selected-optimizations.md) | E3 | [O4-E3-02](tickets/O4-E3-02-admission-pin-transfer.md) | 선택 admission pin의 read-view 이전 |
| [W2](waves/W2-repairs-and-selected-optimizations.md) | E3 | [O4-E3-03](tickets/O4-E3-03-atomic-active-query-rpc.md) | 단일 RPC의 Active 선택·검색·응답 결속 |
| [W2](waves/W2-repairs-and-selected-optimizations.md) | E4 | [O4-E4-02](tickets/O4-E4-02-generation-durable-barriers.md) | generation durable publication의 그룹 barrier |
| [W2](waves/W2-repairs-and-selected-optimizations.md) | E4 | [O4-E4-04](tickets/O4-E4-04-source-token-authority.md) | source-bound distinct token authority |
| [W3](waves/W3-source-validation-and-admission.md) | I0 | [O4-I0-02](tickets/O4-I0-02-matching-source-proof.md) | 최종 source의 Contract·SDK·CI 검증 |
| [W3](waves/W3-source-validation-and-admission.md) | E1 | [O4-E1-03](tickets/O4-E1-03-admission-and-split.md) | 최종 suite·split·admission 연결 |
| [W4](waves/W4-native-capture-performance-and-scale.md) | E2 | [O4-E2-06](tickets/O4-E2-06-quality-only-warmup.md) | 기존 quality-only warmup=0 정책의 실제 parity |
| [W4](waves/W4-native-capture-performance-and-scale.md) | E2 | [O4-E2-04](tickets/O4-E2-04-fresh-five-product-captures.md) | 5제품 실제 캡처와 blind union 반환 |
| [W4](waves/W4-native-capture-performance-and-scale.md) | E4 | [O4-E4-05](tickets/O4-E4-05-release-scale-load.md) | matching release scale·load·restart 실행 |
| [W4](waves/W4-native-capture-performance-and-scale.md) | E4 | [O4-E4-06](tickets/O4-E4-06-qualified-performance.md) | 동일 응답 경계의 정식 반복 성능 |
| [W5](waves/W5-final-scoring-and-policy.md) | E1 | [O4-E1-06](tickets/O4-E1-06-final-pool-and-scoreboards.md) | 최종 합집합 검수·재채점·정책 판정 |
| [W5](waves/W5-final-scoring-and-policy.md) | E4 | [O4-E4-07](tickets/O4-E4-07-policy-and-semantic-residuals.md) | 기본 typo·NL·semantic 잔여의 정책 RCA |
| [W6](waves/W6-release-and-final-closure.md) | I0 | [O4-I0-03](tickets/O4-I0-03-release-operational-gates.md) | SEP-21 release·paired producer·배포 게이트 |

## 4. 웨이브에 걸쳐 계속하는 작업

| 원래 티켓 / 담당 | 준비 | 실제 발행·판정 | 후속 재검증 |
| --- | --- | --- | --- |
| [O4-E1-03](tickets/O4-E1-03-admission-and-split.md) / E1 | W1 producer/schema/fixtures PREPARE | W3 source validation 뒤 repository admission ISSUE | W5 final qrel·protocol/config/source 변경의 영향 binding 재검사 |
| [O4-E2-06](tickets/O4-E2-06-quality-only-warmup.md) / E2 | W1 0/1 spec/protocol/fixture | W4 matching source의 actual parity 뒤0 선택 | source/profile 변경 시 parity·canonical input/ISSUE 재검증; speed는 warmup≥1 유지 |
| [O4-E3-04](tickets/O4-E3-04-maintenance-health-metering.md), [O4-E3-05](tickets/O4-E3-05-publish-timeout-replay.md), [O4-E4-03](tickets/O4-E4-03-ascii-scanner-decision.md) / 각 owner | W1 proof/whole-call 판정 | 결함/비용 실패 시 W2 원래 티켓의 code phase | W3 gates→W4 영향 실행 |
| [O4-I0-02](tickets/O4-I0-02-matching-source-proof.md) / I0 | W1 SHARED proposals/registry 준비 | W3 selected epoch VALIDATE | W5 policy/scorer·후속 W2 source 변경 때 새 epoch |
| [O4-I0-03](tickets/O4-I0-03-release-operational-gates.md) / I0 | W1 provider/Linux/pair/restore/actions actual input/target inventory | W3 이후 준비된 CODE scope, W6 release/actions/전체 잔여 | current exact pair/source/result 변경 시 해당 qualification 다시 판정 |

## 5. 다음 웨이브로 넘기는 최소 결과

| 인계 | 넘길 결과 | 거절 / 재개 |
| --- | --- | --- |
| W1/2→W3 | owned diff·SHARED proposal, independent oracle/fixtures, selected owner commands/results, labels·scope·이번 epoch 포함 변경 | known correctness failure·소유 충돌·필수 producer 누락이면 영향 scope 수리/준비로 복귀 |
| W3→W4 | matching binaries·source/producer/config와 issued suite/split/pack/admission 경로·revision/digest, required cells | stale/mutable latest·binary/source mismatch·잘못된 profile이면 재검증/재ISSUE |
| W4→W5 | immutable native request/response·runtime/index_scope/protocol/clock, candidate pair keys·required-cell coverage·host/claim proof | 누락·unjudged·partial/unsupported를 성공/0점/no-answer로 합성 금지 |
| W5→W6 | final qrels·scoring view/report·분모/CI·name/NL/holdout exposure·policy disposition와 current source 영향 | source 변경은 W3→영향 W4→W5 replay. raw binding이 허용하지 않으면 재캡처 |

자세한 raw 재사용/인계 계약은 [README §2.1–2.2](README.md)를 따른다. 새 raw/log/capture/receipt는 checkout 밖 fresh root에 두고 기존 원본을 덮어쓰지 않는다.

## 6. 담당·자원 규칙

- **E1–E4 에픽별 한 owner와 단일 I0 역할**을 유지한다. 이번 4개 슬롯에서는 root가 E1/I0를 겸한다. 웨이브가 바뀌어도 동일 evaluator·collector·SDK·lexical source 권위를 분리하지 않는다.
- OWNED 파일은 해당 에픽 담당이 통합한다. SHARED 파일은 I0가 반영하고 다른 담당은 구체적인 proposal와 proof를 넘긴다.
- source reading·fixture 작성·정적 점검은 병렬이다. 실행 owner checks·shared service index/model jobs·heavy builds·제품/ingest/scale/정식 성능은 I0가 검증 배치와 resource별 admission/시간대를 관리한다.
- filter/명령·구체적인 파일·단계별 oracle와 DoD는 원래 티켓이 기준이다. 웨이브 문서는 새 완화 계약이 아니다.

## 7. 완료 상태와 문서 검증

- `VERIFIED`: 29개 기준 배치의 누락/중복·원래 dependency 순서·같은 웨이브의 순차 경계·local links·ticket/index/epic 동기화·whitespace를 확인한다.
- `NOT_RUN`: 이 계획 작성에서 제품 구현/tests·actual model/capture/warmup parity·성능/scale·CI/paired/Linux·배포/활성화/롤백 실행.
- FAILED/BLOCKED/NOT_RUN ledger만 정리하면 요청된 qualification은 남는다. 조건 미성립 NOT_APPLICABLE은 actual evidence와 trigger를 요구한다.
- 모든 29개 티켓의 요청 scope가 실제 완료 또는 근거 있는 비적용이어야 전체 종료다. 준비된 baseline/partial report와 미선택 최적화를 구분한다.
