# 잔여 작업 인덱스

Status: `ACTIVE_RESIDUAL`

완료된 결정은 [ADR](../../../adr/README.md), 정확한 과거 문서·실행은
[복구 인덱스](../../../ARCHIVE-INDEX.md#historical-record-recovery)가 소유한다. 이 인덱스에는 **미완료 조건만** 남긴다.
완료·비적용 scope와 중복 RFC/플랜/티켓은 제거했다. B01–B09/J7Q/QIT/SEP-21/MISC의
미완료 작업은 이 목록에만 유지하고, 영구 수용 계약은 기존 ADR가 소유한다.
파일 경로는 동시 작업의 참조를 보존하기 위해 유지한다.

## Owners

| Owner | 소유 경계 |
| --- | --- |
| E1 | review/admission/evaluator/source oracle/split/license/gold/scoring |
| E2 | native collector/index scope/Semble phases/required cells |
| E4 | lexical lifetime/cost/scanner/scale/open-loop/측정 후 정책 판정 |
| I0 | shared DTO/schema/registry/CI/dependency/영향 source/설치·pair·release |

Cargo/Justfile/CI/shared schema는 한 integration owner가 반영한다.
E1→E2는 immutable admitted inputs, E2→E1는 raw/index/clock/outcomes/unjudged keys,
E1→E4/I0는 final qrels/report/denominators/정책 판정이다.
Semantica는 외부 producer이며 fact resolution/join/completion은 그 저장소가 소유한다.

## 현재 코드 잔여

2026-10-08 코드·실행 대조는 main의 packed-source planner·publication typed proof·공식 ARB/BCY scorer·paired R5 owner recipe 수리를 포함한다. 기존 `b9c058e1`의 완료된 F15/query/restart·paged directory,
SDK lifecycle/cache 및 hosted checkpoint의 source·명령·범위는 아래 ADR가 소유한다.
Staged upload, EOF cancellation, checked memory/deadline, scanner custody와 P11 공통
producer/parser/checker/recipes도 구현돼 있다. 추가 수리는 실제 비용·반례 또는 target 계약으로 결정한다.

### 구조적 수리 우선순위

| 순서·경계 | 확인된 원인 또는 입력 상태 | 처리·수용 조건 |
| --- | --- | --- |
| P0 · SDK publish recovery | Publish 성공 후 activation/CAS 실패에서 검증된 receipt가 오류 경로에서 소실된다 | `프로필, 설정` 채팅이 SDK와 Semantica 분류/caller를 같은 묶음으로 수리 중. Published/activation-unknown 상태를 보존하고 무조건 재시도하지 않으며, 단위·daemon 실패 경로와 public API 영향 검증 후 통합 |
| P1 · E4-01 publication planning | 같은 publish 요청에서 `plan_ops`를 반복 계산한다. Coverage는 공유 트리여서 전체 데이터 복제로 계산하지 않는다. 부모/새 세대 full loader와 current-byte 인증 비용은 별도 존재한다 | `Quanta Scale · Large/XL 용량 검증 (2)` 채팅이 RCA/수리 소유. 요청 단위 typed plan/owner로 중복을 통합하고 cold open/scrub와 publication 책임을 구분. 기존 unchanged-file tamper-before-update 계약을 캐시 성공값으로 우회하지 않음 |
| P1 · I0-03 R5 caller | Frozen producer의 parser retained-projection type/export 및 `first_refusal_v1` 계약이 컴파일되지 않는다. Actual 56 diagnostics/exit101 | 완전한 producer 수리와 clean source가 선행. Consumer feature 제거·오류 무시로 통과시키지 않으며 같은 pair의 caller list/run/restart를 재개. Kernel1/1 성공은 별도 |
| P1 · E1-01/03 review → admission | SQLAlchemy/Tailscale 판단은 unresolved/invalid source line으로 실제 실패. Zellij20은 판단 완료이며 기존 source license/split이 맞는다. 옛 model-cache 경로는 부재하지만 동일 assets는 별도 retained capture에서 복구 가능하다 | 최종 판단과 source-bound 검증을 유지. Zellij license/split/model 입력 재검증은 아래 ADR로 이관하고, 최종 source의 Contract/SDK·host/cache/lockfile·새 suite admission/capture를 연결. 오래된 receipt의 source/hash를 변경하지 않음 |
| 외부 입력 · holdout/operations/performance | 독립 gold/acceptance, OSA24 원 corpus, 설치 대상·observer/rollback, qualified host가 없다 | 각 입력이 존재하는 scope만 재개. Proposed SDK/policy나 fixture를 실제 target/qualification의 대체물로 추가 구현하지 않음 |

두 코드 수리 경계는 병렬이며 shared Cargo/schema/CI 및 무거운 build/native 실행은 직렬 통합한다.
Source-bound admission은 수리된 최종 SDK/producer proof 이후에 발행한다. 미완료 판단과 target 입력은
코드 결함과 구분하며, 완료된 ARB/B09/XL을 반복 실행해 이 선행 조건을 대체하지 않는다.

### 코드·테스트 대조 결과

아래는 현재 구현/테스트 범위와 실제 잔여의 구분이다. 테스트 소스가 있다는 사실은
이번 Rust 실행이나 최신 제품 qualification을 뜻하지 않는다.

| 범위 | 실제 코드·기존 테스트 | 남길 작업 |
| --- | --- | --- |
| E1 review/admission | `holdout_review.py`의 blind prepare·frozen validation·finalize, `run.py`의 실행/replay admission 검증, `corpus_binding.py`의 source split 검사; retained Ready9 AI license9repo/9,741files custody 대조 완료 | 실제 reviewer/adjudicator raw·최종 labels·새 holdout license/gold/acceptance·admission 발행 |
| E1 bootstrap | `evaluator.py::mean_ci`의 10,000 draws·16-key/256KiB bounded cache와 `test_bootstrap_cache.py`의 독립 고정 golden·validation controls | 추가 최적화는 actual paired full-caller 비용·parity를 보고 판정 |
| E2 reader/inventory | `live_lexical_external.py`의 selected-project acquired-reader 검증, `opengrok_query_witness.py`, workflow/matrix/ready-drain | 미실행 required cells·actual replay/join; caller가 요구할 경우에만 전체 loaded-reader witness 구현 |
| E4 F15 / Scale | Fault/query/clone-retry/daemon·fixed5bf Large/XL 및 fixed82bc XL 전수 검증6회 원인 추적; packed planner O(N²) 수리와 typed proof/current-byte authentication 통합·library1,035/통합36/daemon300 통과; e227 matching XL32,768 실제 실행·독립 replay 완료 | E4-06 admitted Linux 반복 성능. 완료된 capture 재실행 없음 |
| QIT lifecycle/concurrency | Mixed-corpus lifecycle·SDK history8/8 및 Darwin TSan의 core2/lexical2는 [coverage ADR](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-darwin-tsan-and-contract-execution)로 완료 이관 | 더 넓은 generated/repeat/native inventory. Search-node 한도는 nightly transition 실행이 아님 |
| Semantic | Exact-text semantic/hybrid·OS-process cache·model/revision/dimension matrix4/4와 corrected lib-test strict는 [semantic ADR](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md#text-vector-and-cache-identity)로 완료 이관 | 기존 OpenAI paraphrase rail(ignored)의 실제 API 입력/실행, installed CLI/live-provider release 및 upstream producer 검증 |
| I0 operations | 공통 typed action producer·pre-state refusal·checker/aggregate, optional paired caller/kernel archive | concrete target adapter·독립 observer 계약 구현, authorized target 입력 및 exact-pair/action 실행 |

완료된 foundation은 [review ADR](../../../adr/OCT-05-001-review-admission-and-result-identity.md#owners-and-regressions),
[semantic ADR](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md#text-vector-and-cache-identity),
[test coverage ADR](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#implemented-lifecycle-tests-and-remaining-coverage)가 소유한다.
이 대조에서 실행한 review/bootstrap/OpenGrok 범위는 fixture 검증이며 actual 제품 검색은 아니다.

SDK/Contract `e37123eb`, hosted CI `9221d771`, CS/SG/OG ready9 `09103820`의 완료는
각각 그 소스·입력 범위다. 후속 `a308972a`는 upload/SDK/scale을, `6a3f6afc`는 contract를
변경했다. 이전 결과를 최신 HEAD의 qualification으로 재표기하지 않는다.
새 소스의 CI·선택된 proof 회수는 [I0-02](#o4-i0-02), 실제 제품 수용은 각 잔여 owner가 담당한다.

## Quanta에서 할 작업

| 우선·owner | 실제 잔여 | 종료 조건 |
| --- | --- | --- |
| P0 · I0-02 / CI/integration | 선택된 PR/release·broader inventory; 후속 코드 변경의 영향 rail | 동일 source terminal·inventory·receipt. b9 regular CI 및 fresh release SDK27/original·relocated replay, 0d Contract191Rust/802Python·Darwin TSan4는 ADR로 완료 이관 |
| P1 · E4 / performance | 전체 sync/read/hash/metadata·segment fanout 비용, scanner·Semble·bootstrap 판정 | 원인별 실제 관측 및 독립 parity. 정식 속도는 admitted host·사전 기준·반복 표본 |
| P1 · E1/E2 / quality | labels/admissions·matching Quanta/Semble pair·native replay/full5·독립 채점 | required cells 및 query/unit/source/index scope, 미판단·실패·제외 분모 설명 |
| P1 · E1 / holdout | 실제 미사용 corpus/query/family·license/gold/name-span·typo 평가 | 독립 truth·critical strata·exposure/underfill, file hit와 declaration recovery 구분 |
| P2 · I0-03 / operations | concrete target adapter·provider·installed Linux/state/actions | 대상·독립 pre/post 계약·actual pair/host/config/state/retention/rollback 입력과 실행 |

### 잔여별 실행 가능 조건

| 잔여 | 현재 입력·실행 상태 | 다음 조치 |
| --- | --- | --- |
| E1 judgments/admission/holdout | `BLOCKED`: 151tasks/742pairs의 reviewer 배정·최종 판단0; 새 holdout license/gold/acceptance 부재. Retained Ready9 AI license custody는 완료 | 실제 독립 reviewer/adjudicator raw·해당 승인 입력을 받은 뒤 finalizer/admission/scoring. Blank form을 labels로 채우지 않음 |
| E2 other required native lanes | Gin v3 symbol/file·7개 default robust cohort·explicit typo1,189/독립 OSA1 absence99의 actual pair/replay, 새 B09 8cells, ARB 128captures/384route 공식 채점·replay는 `VERIFIED`; historical B09 current phase33은 `FAILED` | 완료 범위는 [native ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md)로 이관. 원 source 없는 B09 OSA24는 `BLOCKED`; C3 새 labels/admission 및 source별 required inventory는 별도 |
| E2 Semble A/B adoption | `NOT_RUN`: Bat A/A 반복·Zustand0/1 selected parity 완료 | 대체 구현/모드와 사전 whole-caller 기준·고정 source 입력을 선택한 뒤 실제 A/B |
| E4 formal performance | `BLOCKED`: qualified Linux/host timeline·사전 효과/불확실성 기준 부재 | 대상 host·config·동일 boundary와 사전 paired schedule 입력 |
| E4 Scanner historical replay/performance | 옛 root/binary replay와 admitted-host speed는 `BLOCKED`; 별도 f606 fresh build/capture 두 closed receipts와338 parity는 `VERIFIED` | [Closed 진단](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#closed-scanner-build-and-capture-diagnostic)을 유지. 정식 speed/adoption은 별도 host·반복 schedule·사전 기준 필요 |
| QIT/pair/release | 선택된 broader/release는 `NOT_RUN`; e227/fixed77bc caller/list는 producer 컴파일로 `FAILED`, kernel 실제1/1은 `VERIFIED` | b9 fresh SDK27은 완료. Caller 재실행은 완전한 clean producer 수리 후; kernel 결과로 paired/operations 성공을 발행하지 않음 |
| I0 Linux actions/installed provider | `BLOCKED`: target 경로·독립 observer/rollback 계약·설치된 CLI/API grant 부재 | 실제 대상 입력 후 adapter 계약과 action 구현/실행. 현 staged registry로 deploy 완료를 발행하지 않음 |
| Conditional token/regex/bootstrap/policy | 측정·consumer 계약·독립 truth가 선행 | 병목/반례/선택된 정책 없이 새 캐시·API·범용 adapter를 추가하지 않음 |

### 벤치 실행 잔여 — 2026-10-08 소스·입력 대조

- Ready9: 최신151tasks·742pairs union의 독립 판단·final admission이 잔여다.
  Source6a3의5repo, fixedb262의 CLI·Django·Nushell·TypeORM 및 외부091 capture/replay는 완료됐다.
  Source별 raw·완료 범위·허용 reuse는 [Ready9 ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#completed-ready9-capture-scope)이 소유한다.
  NL file20tasks/repo·distinct-file 계약이며 bare-symbol workflow와 구분한다.
  b9 fresh release Bat20tasks/79files의 actual Quanta/Semble pair와 byte-identical
  verdict replay도 [진단 ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-fresh-release-bat-pair-diagnostic)로 완료 이관했다.
  Quanta capped20/Semble success20이며 정식 품질·속도·다른 required lanes의 완료가 아니다.
  9repo/9,741files와106notice pairs의 retained AI license custody는
  [review ADR](../../../adr/OCT-05-001-review-admission-and-result-identity.md#retained-ready9-license-custody)로 완료 이관했다.
  이 scope를 새 holdout 승인·human review·redistribution clearance로 재표기하지 않는다.
- accepted55/PREP·5796 재발행 packet의 명시된 임시 root는 부재하여 원본 replay는 `BLOCKED`다.
  원본 byte 동일 복구 또는 새 source-bound 준비가 선행한다.
- Historical supplemental742pairs ledger와 historical Scanner A/B 실행 root는 현재 부재해 해당 원본 replay는 `BLOCKED`다.
  현재 source별151tasks·742pairs union과 review 입력은 보존돼 있으며 historical ledger와 다른 입력이다.
  Scanner의 durable Bat79files/338queries 입력·frozen a5 관측 캡처/parity는
  [cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#observed-scanner-two-arm-diagnostic)로 완료 이관했다.
  별도 f606 두 fresh build/capture closed receipts와 canonical338 parity 비교는
  [closed 진단 ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#closed-scanner-build-and-capture-diagnostic)로 완료 이관했다.
  옛 binary replay와 정식 speed/adoption은 [E4-03](#o4-e4-03) 잔여다.
- Scale: fixed5bf의 Large4,096·XL32,768 causal 비용/RSS·lifecycle·독립 replay,
  XL release OS-child restart/delete 및 offered-load3,743건은 완료됐다.
  원래 fixture와 cap을 유지했고 registry resident326,772,711bytes로512MiB 안에 들어왔다.
  Target200QPS는 포화로 달성180.039QPS이며 정식 성능 합격이 아니다.
  Fixed8642 Medium·fixed5c Large 및 소스별 기능/CI checkpoint는
  [cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints)이 소유한다.
  이 Mac diagnostic은 Linux physical-I/O·qualified performance를 대신하지 않는다.
  Linux 정식 rail은 macOS에서 unsupported_host로 거절됐으며 admitted host가 필요하다.
- `source-split-prepare/validated.json`은 source split만 검증한다. License/gold/acceptance가 아니다.
  Workflow/matrix/join/decision fixture 통과도 actual product search나 benchmark samples를 대신하지 않는다.

## 실행 순서

| 단계 | 실행·인계 | 선행 |
| --- | --- | --- |
| W0 | source/dirty/owner·claim/input/binary 영향 확인 | shared schema/DTO/registry/CI는 I0 단일 owner |
| W1 | E1 labels/rubric/holdout, E2 index scope, E4 비용·조건부 병목 조사 | 입력 부재는 해당 scope만 `BLOCKED` |
| W2 | 확인된 비용·반례의 owner 수리 및 영향 회귀 | 사전 correctness/resource 계약. 구현된 F15를 다시 만들지 않음 |
| W3 | I0 selected proof/CI → E1 admission ISSUE | PREPARE → 영향 VALIDATE → ISSUE |
| W4 | ready cells native capture/replay/join, matching capacity/A-B | source/input/profile/index/clock에 결속한 raw. 실패 sibling은 ready cell을 막지 않음 |
| W5 | 마지막 unjudged union → labels/admission → scores/CI·정책 판정 | E2 raw 및 E1 독립 truth. Holdout은 기존 ready cohort 재채점의 전역 선행이 아님 |
| W6 | I0 실제 target adapters·provider/Linux/운영 실행 | authorized inputs·independent observers·exact pair·staged registry 수용 |

OG 단독 native capture와 각 clean source의 scanner/capacity 진단은 최종 Quanta SDK proof와
독립 진행할 수 있다. Formal 비교·릴리스가 소비하는 source proof는 해당 scope에서 결속한다.
Source/fixture/static 조사는 병렬 가능하다. 실제 build/test/model/Docker/scale/performance는 host별
직렬 admission을 따른다. 이 문서는 새 병렬 에이전트나 무거운 실행을 등록하지 않는다.

## 공통 실행 조건

- `VERIFIED`는 실행한 범위, `FAILED`는 실행 실패, `BLOCKED`는 필수 입력 부재,
  `NOT_RUN`은 미실행이다. 미측정 조건부 변경을 `NOT_APPLICABLE`로 닫지 않는다.
- 현재 source/dirty/owner·입력·selector를 확인하고 좁은 결정적 rail부터 실행한다.
  새 raw/log/receipt는 checkout 밖 fresh root에 두고 기존 실패/partial를 보존한다.
- source/query/unit/model/profile/runtime/clock 변경은 영향 cells/proof를 재검증한다.
  qrel-only reuse도 원 native binding이 허용해야 한다. 옛 raw의 digest/revision을 바꾸지 않는다.
- unknown/unresolved/missing을 grade0/no-answer/empty success로 채우지 않는다.
  capped/timeout/capacity refusal·common-eligible0은 각각의 outcome으로 남긴다.
- 조건부 [config/generation policy](../../../adr/OCT-04-002-configuration-and-generation-policy.md),
  [preparation SDK](../../../adr/OCT-04-003-source-preparation-sdk.md)는 `Proposed`다.
  실제 operator 요구·producer fixture 없이 구현/API 변경으로 승격하지 않는다.

## E1

Owner: review/admission/evaluator/source oracle/split/gold/scoring.
Contract: [OCT-05-001](../../../adr/OCT-05-001-review-admission-and-result-identity.md).

### O4-E1-01

`BLOCKED` · SQLAlchemy 잔여 146pairs 최종 AI 정답 판정 및 Tailscale UDP
필터 밖 grade1/3 rubric 확정. 원본 SQL334/480과 reviewer raw를 보존한다.
2026-10-08 실제 실행과 한 차례 동일 입력 재시도에서도 SQLAlchemy는 unresolved pair/invalid source line,
Tailscale은 unresolved pair로 `FAILED`였다. 두 저장소의 labels/admission은 미발행이다.
원 frozen corpus/suite/query/rubric과 valid raw는
`qi-b08-closeout-20261004-2i72kj91/c3-review-resume-quota-qcshswey`에 있다.
새 실행·실패 원본은 `/private/tmp/qi-c3-oct8-review-v4/`에 별도로 보존한다.
완료 조건: C3 240tasks의 judgment provenance와 issued/excluded/failed/blocked 집합.
미판단 pair는 채점에서 제외하고 model의 미해결 판단을 grade나 human gold로 강제하지 않는다.

Zellij 20tasks/476pairs의 세 모델 판단·독립 cached replay·canonical AI labels/NL suite 발행은 완료했다.
[C3 실제 closeout](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-c3-ai-review-and-issuance)이
원본·명령·실패 이력·retained source split 대조를 소유한다. 완료된 판단을 다시 실행하지 않는다.
원 source/query/rubric와 requested model은 유지하며 human provenance·qualified verdict는 없다.

### O4-E1-02

`BLOCKED` · 신규151tasks/742pairs의 두 reviewer+adjudicator 판단 및 원 valid labels 병합.
현재 union의 모든742pairs는 원 query/source에 결속된 두 blank form에 포함돼 있다.
새4repo 입력은 `ready9-native-paged-20261007-01a10d0b/blind-review/`, retained
Lo·Mocha·Uvicorn·Zustand 입력과 전체 대조는
`/Users/songmin/.codex/task-evidence/ready9-retained-review-20261007-01a10d0b/review-input-coverage.json`이다.
8repo·160개 form task가151개 판단 대상 task를 포함하며 reviewer identity/label 변경은0이다.
Bat358+51=409를 재호출하지 않는다. 별도 historical 원 ledger
`/private/tmp/qi-current-nine-supplemental-pool-97eedd-actual-v1/ledger.json`은 현재 부재하다.
기록된 SHA `78013ed33e5647bfa5e109fc5edabb422e41dc48d0cbf4f63f713785642818ec`의 동일 원본 없이는
historical replay를 발행하지 않는다. 현재 union의 review는 준비된 입력으로 진행한다.
제품명/순위/점수는 review에 노출하지 않는다.
완료: source/query/rubric/threshold/model에 결속된 labels·독립 raw replay 및 reused/unresolved/excluded 집합.

### O4-E1-03

`NOT_RUN` · ready9 후속 final-source 및 SQLAlchemy/Zellij/Tailscale admission ISSUE.
Zellij canonical AI labels/NL suite20tasks는 발행됐고 retained split의 commit/universe/20families와 일치한다.
기존 local-source license의422files와 새 suite/corpus/split 결속은 재검증됐다. 옛 HF cache 경로 부재는
동일 revision/asset의 retained Gin20 cache에서 fresh external root로 복구했다. 이 입력 복구는
[C3 ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-c3-ai-review-and-issuance)이 소유한다.
새 suite의 full admission·matching final-source Contract/SDK 및 host/cache/lockfile bindings가
미완료여서 admitted native는 `BLOCKED`다. 새 판단이나 license 재승인을 임의로 생성하지 않는다.
SQLAlchemy/Tailscale의 미완료 판단은 [E1-01](#o4-e1-01)이 한 번만 소유한다.
5796 재발행 packet의 임시 경로는 부재해 원본 replay는 `BLOCKED`다. Bat409 merged qrels/suite/pack,
각 repo source/runtime·split/license/review·matching proof를 동일 bytes로 검증한다.
CLI20tasks/519pairs의 historical ISSUE와 중단된 remaining batch를 구분한다.
C5 stale4 suites는 reissue 또는 명시적 exclusion. 완료: 원 labels provenance를 유지한 새
admission/result, wrong-source/threshold/grade/family/unit/runtime/proof/license 거절.

### O4-E1-04

`NOT_RUN` · 다른 supported declaration-name/span·typo cells와 후속 source 영향 평가.
Gin 4개 name-span controls와 canonical v3 symbol1,196·default file1,196의
실제 native 진단은 [ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-gin-declaration-and-robustness-execution)로 완료 이관했다.
Driver f606/binaries b9의 scope를 후속 main·holdout으로 승격하지 않는다.
Independent source oracle·native selected unit으로 same-line/receiver/use-only/Unicode/case를 대조한다.
완료: supported/unsupported 분모·source-attested 선언 identity/name bytes/span. File hit는 name recovery가 아니다.

### O4-E1-05

`BLOCKED` · license approver·사전 acceptance/critical-stratum 기준; 독립 gold/holdout 미발행.
`qi-oct4-unseen-prepare-k7exyv41`의 source/release/split candidate를 검토한다.
Corpus-set5,684와 release code_only6,079는 다른 분모이며 candidate12repo는 미승인이다.
완료: source/query/family development/holdout 분리, exposure/near-copy/parser/license/gold provenance와
ambiguous/excluded/underfilled 집합. 1,000+ family 목표를 복제/exposed source로 채우지 않는다.

### O4-E1-06

`NOT_RUN` · final labels/admissions와 E2 native outcomes/union의 독립 재채점·reports.
Lane별 common eligible/operational coverage/repository-cluster CI/pool sensitivity,
name/NL/no-answer·ARB original/adapted·B09 분모를 유지한다.
완료: raw recomputation과 rows/denominators/scores/report 일치 및 모든 required-cell outcome 설명.

### O4-E1-07

`NOT_RUN` · 조건부 bootstrap 추가 최적화 판정. 실제 matching two-capture와1,196-row paired
full-caller cold compute/RSS·사전 목표/memory ceiling이 선행한다. Symbol 단일-route 진단은 paired caller가 아니다.
채택 시10,000 resamples/method/seed/draw/strata를 independent scalar/reference와 대조하고
NaN/Inf·duplicate task·draw 순서·hidden cache growth를 거절한다. 완료: 최적화+parity 또는 근거 있는 no-code 판정.

## E2

Owner: native external collector/index scope·Semble phases·required cells.
Contract: [OCT-05-002](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md).

### O4-E2-02

`NOT_RUN` · 남은 repository/profile 및 caller가 요구하는 actual reader/source/posting scope 판정.
OG ready9/180의 selected-project acquired-reader 증거는 그 요청 범위의 완료다.
All-project/global/loaded-reader flags는 false다. 전체 서비스 scope를 요구할 때만 별도
loaded-reader witness/consumer를 구현·검증한다. Disk/API/readonly seal·returned hits로 전체 indexed universe를 추정하지 않는다.
완료: scope별 missing/extra/unknown, 실제 service/query/index 결속과 독립 source/directory/settings 분모.

### O4-E2-03

Ready99repo의 terminal inventory는 완료됐다. 잔여는 다른 required inventory의
executed/reused/unsupported/failed/blocked/not_run 설명 및 ready drain이다.
Gin v3 native 및7개 default cohort의 실행/독립 replay는 완료됐다.
B09 원33cells/11,272selected rows의 original commitments 및 diagnostics는 대조 완료다.
Current phase replay33은 producer 증거 부재로 `FAILED`이며 원 완료와 분리한다.
재캡처 입력 대조에서 CLARC2/CSN6 cells의 1,350selected tasks·3,798source files는
원 manifest bytes와 대조돼 준비됐다. OSA24 cells의 원 checkout은 없어 `BLOCKED`다.
준비8 cells의 actual native는 f606에서8/8 exit0·phase/diagnostic/source 및 독립 score replay를 완료했다.
원 successful33 결과와 분리한다. 원본과 재실행은 [ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#retained-b09-scope-reconciliation)이 소유한다.
원 source와 qrel-only reuse를 구분한다. 살아 있는 process/malformed/wrong-repo terminal/
missing/output 경합은 success가 아니다. 완료: 누락 없는 terminal/input-byte inventory와 실패 sibling에 독립적인 실행.

### O4-E2-04

Ready9 capture/replay/projection과 원 source별 완료는
[ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#completed-ready9-capture-scope)가 소유한다.
실제151tasks/742pairs 판단·admission 잔여는 [E1-02](#o4-e1-02)/[E1-03](#o4-e1-03)에만 둔다.
Gin v3 exact symbol/file1,196 및 prefix/infix/components/default typo/declaration absence/
content absence/typo content absence7개 cohort와 explicit typo1,189/독립 OSA1 absence99는
[ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-gin-declaration-and-robustness-execution)로 완료 이관했다.
Gin20의 original manifest bytes 복구·f606 native hybrid pair20·독립 byte-identical
verdict replay는 [진단 ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-retained-gin20-hybrid-pair)로 완료 이관했다.
잔여 selected lanes는 C3 NL240 및 원 source 부재 등 후속 조건을 요구하는 B09 cells다.
ARB current32-token policy의 original13/88·retained adapter-v1 27/88·adapter-v2 88/88은
actual128 captures와384route 공식 채점·독립 replay를 완료했다. Historical original17과 current13을 구분한다.
새 read-only ARB scorer는 official raw sample/gold·base/universe·spec·query·record를 결속하며
top100chunks→20distinct files의 공식 Recall/MRR를 계산한다. 공식 archive/manifest와 descriptor-bound
parsed bytes·transitive clean official source/import origin을 검증하는 BCY extension까지 통합했다.
Focused44와 actual128 prepared-spec binding preflight는 `VERIFIED`; official3archives의
83release-chunk files/57unique snapshots도 복구·rehash했다. Native128/128 source-bound 독립 검증과
공식3arm Recall/MRR·BCY4k/8k/16k/32k 및 반복 score bytes가 검증됐다.
원 controller의 manifest/typed-context 오류와 e227 scorer의 combined/projected pack 거부는 보존했다.
6c5cda3c의 canonical projection 수리·정확한3route 및 digest 대체 거절 회귀까지45/45 통과했다.
[공식 진단](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#arb-current-policy-and-official-file-scoring)이
source/분모/원본 경로를 소유한다. Current-v2 Recall@20 lexical0.484848/semantic0.308712/hybrid0.369318이며
cohort13/27/88 차이와 기존 노출 입력 때문에 paired improvement/SOTA holdout으로 발행하지 않는다.
미실행·refusal·failure를 zero score로 채우지 않으며 official corpus custody가 없는 BCY는 `BLOCKED`다.
원 B09 OSA/CLARC/CSN 캡처33개를 전부 미실행으로 재표기하지 않는다.
현재 decoder의 phase refusal은 [원 scope 대조](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#retained-b09-scope-reconciliation)를 따른다.
Four typo populations1,192/1,178/1,192/1,192와 ARB original/adapted 분모를 합산하지 않는다.
[E2-03](#o4-e2-03)이 required outcomes·original raw/input/source/unit/clock replay를 한 번만 소유한다.

### O4-E2-05

`NOT_RUN` · 대체 구현/모드의 Semble 반복 A-B 및 재사용 채택 판정.
Bat 전체20tasks/79files·warmup1·3 measured repetitions의 두 A/A 실제 실행과
parent/worker 비용·rows/status/score-bit parity는 [native ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-semble-repetition-and-warmup-diagnostic)로 완료 이관했다.
영향 source5개 SHA는 일치하지만 실행별 global HEAD는 관측하지 않았으므로 고정 HEAD qualification이 아니다.
Query pack의 초기 digest·query hash·manifest universe 결속과 실행 후 drift 거절은
[입력 계약](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#semble-admitted-input-identity)으로 완료 이관했다.
Immutable validation·native rows/status parity 및 unattributed residual을 확인한다.
정식 speed는 [E4-06](#o4-e4-06)의 host/boundary/schedule을 따른다.

### O4-E2-06

Zustand 전체20tasks/50files·동일 cold probe/seed/3 schedules의 실제0/1 parity는
[native ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-semble-repetition-and-warmup-diagnostic)로 완료 이관했다.
`NOT_RUN` · 다른 repo에서 quality warmup0을 선택할 때의0/1 parity 및 고정 HEAD 조건.
같은 task set/cold probe/profile/seed/repetitions와 protocol SHA/schedule/phase ledger,
task별 rows/status/score bits를 두 actual run에서 대조한다. 그 전에는1을 유지한다.
Order-sensitive 차이가 있으면0을 채택하지 않으며 speed는 warmup≥1이다.

## E4

Owner: lexical lifecycle/query cost·scanner/scale/load·policy RCA.
Contract: [OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

### O4-E4-01

`NOT_RUN` · matching release의 full/delta/delete/no-op/reopen 전체 비용 qualification.
Native segment 재사용·live-BM25·F15 changed-bucket publication은 구현됐다.
같은 timed seal 응답의 request/source/receipt에 결속한 단계 관측 보존·검증도
[cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#per-seal-ingest-observation)에 반영했다.
Pre-intent/base proof·lexical proof·semantic commitment·source finalization의
opt-in stderr trace도 전체 묶음으로 반영했다. 중첩 span은 exclusive 합산 비용이 아니다.
Scale owner 회귀 44/44와 영향4package strict Clippy는 `VERIFIED`.
Fixed82bc XL attribution은 [전수 검증 추적](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#xl-repeated-validation-trace)으로 완료 이관했다.
단일3,437byte delta90.61s 중 전체 proof6회75.64s, no-op91.64s다. Source/build가 다른 원93.969s와 속도 비교하지 않는다.
Packed-source O(N²) 조회는 이분검색으로 수리했고 planner4/통합33/strict Clippy는 `VERIFIED`다.
주병목 수리는 통합됐다: 발행 범위의 identity/manifest/policy에 결속한 typed proof를 유지하고,
각 기존 refusal 경계에서 현재 bytes를 다시 인증하며 decode/normalization/posting 결과를 재사용한다.
완전한 묶음의 library1,035passed/1ignored·파일 변경 통합36passed·daemon300passed/10skipped와
strict Clippy/format/hexagonal/module 검사는 `VERIFIED`; main 파일은 검증된 bytes와 같다.
e227 matching XL32,768 실제 실행은352.747s exit0이며 canonical profile 독립 replay가 일치한다.
원82bc와 corpus/seed/tier manifest/runtime config·기본 cap이 같다. Delta42.517s/no-op45.034s/delete43.148s를
관측했다. Delta proof6회32.596s 중 cold 독립 walk2회12.585/12.393s와 반복 인증4회1.757–2.298s는
남는다. 공유Mac 단일 profiled 진단이며 qualified speed가 아니다. 성공 bool/mtime 재사용으로
변조 거부를 우회하지 않는다.
Remaining: cold 전수 검증·metadata/custody 읽기·hash/fold, delete-mask O(max_doc), correction/
NoMerge segment fanout, transient peak 및 foreground/maintenance CPU/read/write/fsync 분해.
Causal producer의 source/binary와 independent markers를 결속한다. Seal retention gauge는 unique-inode
regular-file st_size이며 restart gauge0·st_blocks·physical I/O/true peak와 구분한다.
완료: 원인별 exclusive 비용·명시적 clock/resource domain·fresh rebuild score/page parity.

### O4-E4-03

`BLOCKED` · historical Scanner A-B original `/private/tmp/qis.utp62qk5`와 옛 binary가
부재하여 그 실행의 원본 replay는 불가능하다. 기록된 SHA나 과거 수치로 입력을 재구성하지 않는다.
복구된 Bat79files/338queries 입력과 frozen a5의 두 실제 캡처·관측 parity는
[cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#observed-scanner-two-arm-diagnostic)가 소유한다.

별도 f606 두 fresh build/capture closed receipts와 canonical
`query_timing_overhead.py --scanner-ab`338 parity 검증은
[closed 진단 ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#closed-scanner-build-and-capture-diagnostic)로 완료 이관했다.
옛 a5 관측 비교는 중단 target의 normal resume를 포함하므로 그 source의 closed receipt로 승격하지 않는다.
두 진단을 최신 main의 성능 검증으로 재표기하지 않는다.
정식 speed/adoption은 [E4-06](#o4-e4-06)의 host·반복 입력과 사전 whole-caller
keep/modify/withdraw 기준이 필요하다. Child 개선은 whole-call 악화를 상쇄하지 않는다.

### O4-E4-04

`NOT_RUN` · 조건부 persistent token authority. E4-01/03의 actual after-scanner full-caller에서
repeated-scan 병목과 memory/build tradeoff가 충족될 때만 채택한다.
Exhaustive tokenizer/OSA1/source/grammar/folded name witness와 delta/delete/no-op/reopen,
cold-open/build/residency/cap/cancel 독립 수용이 필요하다.

### O4-E4-05

완료 이관 · fixed5bf matching release의 Large4,096·XL32,768 causal/replay,
XL open-loop 및 release daemon의 실제 OS restart/delete 진단·기능 scope는
[cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints)가 소유한다.
원래 fixture·cap을 유지한 이 완료 실행은 잔여가 아니다. 정식 반복 성능은 [E4-06](#o4-e4-06)에 남긴다.
Text paging·source Arc sharing·absolute query deadline과 lexical418·daemon300·strict 검증도
같은 ADR로 이관했다. 선택들은 중복 집계하지 않는다.
Medium OS-child restart·offered-load와 새 runtime/profile 영향 검증은 이 완료 범위와 구분한다.
Frozen `492d2fdc`의 XL 기능 proof는 `VERIFIED`이며 기존e371 runtime/open-loop도 완료다.
이는 최신 F15/RSS source의 측정을 대체하지 않으며 완료된 기능 proof를 미실행으로 재표기하지 않는다.
Scale-supported-v1은 pair1GiB/total2GiB·client600s·source128MiB/100,000records·vector256MiB·
staged body512MiB·process4GiB의 별도 계약이다. SDK30s·ordinary inline cap·이전 history default와 구분한다.
Frozen5796 default timeout/posting-cap 실패를 후속 override 성공으로 재표기하지 않는다.
완료: independent corpus/result/count oracle, over-limit typed refusal, offered/served/errors/timeouts/drops
reconciliation·retained bytes·live/RSS·elapsed. Fixture 축소/cap 상향만으로 요청 gate를 닫지 않는다.

### O4-E4-06

`BLOCKED` · Darwin frequency 등 qualified-host 입력; 정식 반복 실행은 `NOT_RUN`.
Continuous load/frequency/thermal/power/disk timeline·사전 effect/uncertainty·same completed-output boundary,
exact source/binary/input/config/topology를 확보한다. B07 최소5fresh roots/route별1,000warm observations,
warmup≥1·randomized paired schedule·독립 schedule/source/raw replay가 필요하다.
Host probe1회·shared-host/phase/scale 진단으로 qualified performance를 발행하지 않는다.

### O4-E4-07

`NOT_RUN` · 조건부 검색 정책 변경. Independent qrel/span/unused holdout 이후 Default OSA23·Gin4·
NL/semantic residuals를 candidate/contribution/rank unit/budget/cap/source/model/generation으로 추적한다.
Confirmed defect/accepted policy/label ambiguity/unsupported/qualification gap을 구분하고 같은 qrel의
독립 ablation을 수행한다. Explicit OSA1은 default 성공이 아니다. 채택 시 critical strata/no-answer/ambiguity 사전 기준을 충족한다.

## I0

Owner: shared contracts/DTOs/registry/CI/dependency·source impact/release.
Contract: [OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

### O4-I0-02

선택된 PR/release·broader inventory와 후속 코드 변경의 영향 rail이 잔여다.
b9 fresh release SDK27/27 및 original/relocated portable replay는
[coverage ADR](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-fresh-release-sdk-execution)로 완료 이관했다.
0d Contract191Rust/802Python 및 original/relocated portable replay는
[coverage ADR](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-darwin-tsan-and-contract-execution)로 완료 이관했다.
Source0d의 regular CI6jobs 및 Rust4,281passed/0failed/30ignored·Python4,323passed/30skipped는
실제 terminal·inventory·receipt·모든 job source SHA 대조로 완료됐고 아래 ADR로 이관했다.
Bench 성공은 컴파일이다. 이후 docs-only checkpoint를0d CI source로 재표기하지 않는다.
b9의 원 final verify2275는 executor 시작 전 infrastructure_fail이며 원 workflow는 failed로 보존됐다.
실패 final만 재시도한 verify2277은 실제 checkout/source command exit0이고 기존5workers를 상속해
regular CI가 완료됐다. Rust4,281/Python4,323은 원 worker 횟수이며 재시도에서 중복 실행되지 않았다.
후속 코드 변경은 영향 hosted rail을 회수한다.
코드 `6c5cda3`의 regular CI 6jobs는 동일 source에서 모두 `VERIFIED`다.
Rust 4,299selected/executed/passed·0failed·30ignored inventory와 Python 4,410passed/35skipped를
원 receipt/raw/inventory 및 job source로 대조했다. 이후 문서 전용 커밋은 별도 focused lint 범위다.
완료된 regular main CI/F15·query/restart·cache focused/strict·SDK/runtime checkpoint는
[ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints)와
[cache 계약](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md#text-vector-and-cache-identity)이 소유한다.
공개 SDK/contract·wire/state/query 변경은 executable authority의 영향 rail과 portable replay를 실행한다.
Runtime autotests=false의 실제 suite/selector를 사용한다. Auxiliary PR coverage/main,
compiler/focused/local/full/release를 구분하고 과거 결과를 최신 source로 재표기하지 않는다.

### O4-I0-03

구현 잔여 · concrete deploy/activate/restore-forward adapters·independent pre/post contract.
실제 Linux host/config/state/retention/rollback 입력은 `BLOCKED`이며 dependent action은 미실행이다.
공통 typed producer/parser/checker/recipes는 완료됐다.
별도 exact-pair caller는 producer 컴파일 `FAILED`, kernel1/1은 `VERIFIED`; paired/operational acceptance는 `BLOCKED`다.
Runner-candidate-only는 operational receipt가 아니다.
2026-10-08 최초 Semantica 확인 시 `77bc829ad70bd32b9c22f92c849a9d6a2aa6b9da`에서
1,286dirty paths를 확인했다. R5 입력으로 고정한 같은 HEAD의 clean
`.codex-semantica-oct8-scc-proof` checkout도 존재한다. 이후 움직이는 Semantica main과
이 exact-pair 입력을 동일 source로 취급하지 않는다.
실제 Cargo runtime/kernel resolver는 이 Quanta checkout의 contract/IPC/SDK로 해석됐다.
QBC metadata exit0의 child stdout은 owner 로그에 보존되지만 터미널 stdout은 비어 있어
기존 Quanta pipe/check_output 연동은 `FAILED`다. 같은 invocation의 nonce/run/status/원본 bytes를
확인하는 metadata adapter와 default typed Nextest archive로 caller 연결부를 수리했다.
추가 foreign-cwd 회귀에서 실제 Python namespace 충돌을 발견해 Quanta `tools`
package 소유권을 명시했다. 수정 후 출력 bridge·locator·foreign-cwd 회귀 60/60은
`VERIFIED`. Clean Quanta b2599와 fixed77bc producer의 실제 runtime/kernel
resolver preflight도 `VERIFIED`다. 두 profile 모두 같은 Quanta7개 reachable package와
producer nested Cargo.lock에 결속됐으며 원본은 `/private/tmp/qna7vj2zq6/r5-clean-{runtime,kernel}-resolved.json`이다.
b2599 원격 Python CI는 Linux에 없는 `/private/tmp` 기본 경로로 `FAILED`였다.
main48fd의 수리는 OS 임시 경로 정규화·3파일/4경계 변조 거절·typed runner 및 observed harness
호출 경로 대조를 포함한다. Focused128과 full 실패 이후 source-contract tail276은 `VERIFIED`다.
Local full 실행의4,058passed/1failed는 수리 전 결과로 보존하며 최신 full-suite 성공으로 합산하지 않는다.
48fd 원격 Python1576은 동일 source에서4,355passed/30skipped·Ruff check/406-file format으로
`VERIFIED`다. 별도 P00 selection386은 중첩하며 합산하지 않는다. 후속 tooling 수정과 Rust CI는 별도 scope다.
48fd Rust1578의 receipt/raw/inventory 및 source를 대조해4,286selected/executed/passed,
0failed·30ignored inventory를 확인했다. Bench1577/final verify를 포함한48fd regular6contexts는
같은 source에서 성공했다. 후속 c38 regular6contexts도 성공했고 원 Rust1584 receipt/raw/inventory는
4,290selected/executed/passed·0failed·30ignored, Python1582는4,355passed·30skipped다.
Bench는 실제 benchmark 실행이 아닌 compilation이며 이 결과를 후속 tooling source로 재표기하지 않는다.
QBC metadata completion probe는 별도로 `verification source changed during QBC execution`로
`FAILED`다. 원인은 미확정이며 실제 Nextest 실행 실패로 재표기하지 않는다.
Clean Quanta c38/fixed77bc의 첫 actual paired recipe는 새 all-feature release build를13m08s에
완료한 뒤 dependency_lock 경로 기준을 nested workspace에 중복 적용해 `FAILED`였다.
실제 caller/kernel Nextest list/run은 시작하지 않았다. 원 controller log·fresh daemon을
`/private/tmp/qna7vj2zq6/r5-c38-failure.json`과 연결해 보존했다. 검증기는 producer와 같은
paired-checkout 상대 경로를 사용하도록 수리했고, 동일 file guard를 expensive build 전에 실행한다.
Actual resolver 출력의 현재 manifest/lock 대조와 producer→runner 경로 roundtrip·pre-build refusal·custody
focused133은 `VERIFIED`다. 후속 clean9c0/fixed77bc actual recipe는 fresh build를2m29s에
완료한 뒤 host Xcode Python3.9에서 QBC `tomllib` import가 실패했다. 원 controller·daemon은
`/private/tmp/qna7vj2zq6/r5-9c0-failure.json`에 결속해 보존했다. 실제 Nextest list/run은 미시작이다.
현재 shell은 producer의 canonical `python-env.sh` resolver를 모든 Python helper에 사용하며,
QBC module origin/import를 expensive build 전에 검사한다. Actual Python3.12.12로 frozen77bc
QBC import·manifest/lock preflight와 focused133은 `VERIFIED`다. 이후 실제 실행은 아래 source별로 분리한다.
Clean2df/fixed77bc의 세 번째 actual recipe는 fresh release build를5m28s에 완료한 뒤
caller/list의 raw feature composition이 QBC의 미등록 compile-surface admission에서 `FAILED`였다.
실제 caller/kernel assertions는 미실행이며 원 controller·daemon bytes는
`/private/tmp/qna7vj2zq6/r5-2df-failure.json`에 결속했다. 수리된 fixed R5 owner recipe는
caller/kernel 각각의 정확한 list/run4 shapes만 QBC typed command-model admission으로 실행한다.
Feature/target/selector/phase drift는 실행 전에 거절하고 lane/source/immutable receipt custody는
기존 QBC owner가 유지한다. Focused137 및 실제 frozen QBC admission4 shapes의 model 대조는
`VERIFIED`; actual Nextest와 운영 qualification으로 승격하지 않는다.
Clean86b/fixed77bc actual4는 fresh daemon3m17s 후 caller/list completion 단계에서
예상 source digest의 explicit completion environment 누락으로 실패했다. 두 source는 clean/HEAD
일치였으며 assertions/kernel은 `NOT_RUN`다. 원 controller·daemon은
`/private/tmp/qna7vj2zq6/r5-86b-failure.json`에 보존했다. Canonical source digest를 compile admission과
completion 양쪽에 전달하도록 수리했으며 producer equality guard는 유지한다.
Hosted2df Python1594는4,388passed/33skipped 후 외부 helper ShellCheck SC1091에서 실패했다.
그 context는 `FAILED`로 보존하고 supplied producer-source annotation으로 hook을 수리했다.
Clean e227/fixed77bc actual5는 fresh daemon2m45s 후 caller/list의 실제 Cargo 컴파일에서
`quanta-adapters-parser`56 diagnostics·exit101로 `FAILED`다. QBC의 explicit completion와
immutable receipt publication은 완료했으며 caller assertions는 `NOT_RUN`이다.
원본은 `/private/tmp/qna7vj2zq6/r5-e227-failure.json`과 `r5-clean-pair-v5/`에 보존했다.
Sibling kernel을 별도 실제 list/run하여 exact-full-bundle/transition-v2 receipt 테스트1/1을
통과했다(`/private/tmp/qna7vj2zq6/r5-kernel-e227/result.json`). Kernel 성공을 caller/pair/operations로
승격하지 않는다. 이후 Semantica main f574의 committed parser subtree는 fixed77bc와 같고
다수 dirty 변경이 있어 clean repaired producer bundle은 아직 없다. Caller 재실행은
그 완전한 producer 수리 후에만 수행한다. Resolution preflight로 이 테스트를 대체하지 않는다.
외부 작업을 reset/stage하거나 dirty pair를 qualified로 발행하지 않는다.
외부 Semantica R3 expected semantic partition/omission 검증은 producer owner의 별도 연동 잔여다.
기존 producer의 prior-state binding·plan assembly와 독립 expected-partition 검증을 구분한다.
고정77bc source의 `load_prior_shadow_semantic_state_v1`는 이전 lexical batch가 delta이면
누적 semantic 상태의 복원을 거부한다. 연속 G1→G2→G3 semantic 발행 수용은 `NOT_RUN`이며,
producer의 누적 상태·digest·event lineage 수리가 별도 필요하다. Source 거부 경로 확인을
실행 재현으로 발행하거나 Quanta full rebuild로 우회하지 않는다.
Target/Linux 입력 부족이 upstream 검증 코드 구현의 선행은 아니다.
[Installed/pair/action ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#installed-paired-and-operational-acceptance)와
[release/proof](#release-and-proof)가 P00–P12/provider/installed/state/pair/operations의
세부 수용을 소유하고 실행 registry가 graph를 소유한다.
완료: CODE_QUALIFIED/DEPLOYED/ACTIVATED/ROLLBACK_PROVEN 각각의 실제 prerequisites·observed action.

## 잔여 실행 진입점

| 범위 | Canonical command |
| --- | --- |
| Review/unit owner | `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_holdout_review.py tools/ci/tests/test_retrieval_native_span_projection.py tools/ci/tests/test_source_oracle_suite.py -q` |
| Native owner | `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_opengrok_index_scope.py tools/ci/tests/test_live_lexical_external.py -q` |
| Actual quality matrix | `uv run --frozen --extra dev python tools/benchmark/retrieval/run.py quality-matrix --spec <issued-spec.json>`; `quality-matrix-verify --spec <same-spec.json>` |
| Actual external native | `uv run --frozen --extra dev python tools/benchmark/retrieval/live_lexical_external.py --spec <issued-spec.json>`; `--verify <native-root>` |
| Contract/SDK | `just retrieval-contract-proof <fresh-root>`; `just retrieval-sdk-proof-fresh <fresh-root>`; `portable_proof.py verify --receipt <root>/execution-context.json` |
| Scale/open-loop | matching `scale_matrix` / `open_loop_matrix` binaries의 실제 `--help` 및 external output |
| Pair / release | `just rust-verify-hellgate-cross-repo <Semantica-checkout>`; `just proof-p12a-proof-infrastructure`; actual manifests로 proof-authority code/release/final gates |

기존 ID는 아래 경로표와 ADR 계약으로 해석한다. 날짜별 packet/티켓을 다시 만들거나
같은 작업/무거운 rail을 다른 목록에 중복 등록하지 않는다.

## Test and platform

Owner: I0 + 실제 test/adapter owner. Contract:
[선택된 회귀·플랫폼](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-regression-and-platform-acceptance),
[CI provider](../../../adr/SEP-28-001-circleci-provider-and-credit-boundary.md).

| 남은 scope | 종료 조건 |
| --- | --- |
| QIT-00/01/05 | 선택된 catalog의 독립 oracle·public wire negative/re-encode 및 lexical/ANN/fusion/filter/metamorphic 결과. Source registration과 실제 execution을 구분 |
| QIT-02/03/04 | 기존 lifecycle/F15/history fixture 재구현 없음. Darwin core2/lexical2 TSan 완료를 제외한 broader generated/repeat/native 및 추가 선택 storage/marker/CAS crash 결과 |
| QIT-06 / installed · J7Q-02 | 실제 설치된 SDK/CLI/daemon·consumer lifecycle/recovery·operator/preview/explain wire proof. Local/scripted peer 결과는 별도 |
| QIT-07 | 선택된 risk-owner mutation/fuzz/coverage·survivor disposition/expiry·minimized input. 옛 미승인 비율·횟수 목표는 Git history로 퇴역 |
| MISC-03 | 모든 선택 adapter의 실제 large success/failure stdout/stderr·many-entry metadata/archive/JSONL·interrupt·heap/bounded I/O |
| MISC-04/05 / QIT-09 | 완료된 b9 regular CI·fresh SDK27/original·relocated replay 및0d Contract·선택TSan을 제외한 추가 native/installed/Linux scope의 canonical nonzero inventory/terminal·replay. API/model 및 조건부 diagnostics는 선택한 범위만 판정 |
| MISC-06 / QIT-08 | 같은 selector·assertions의 paired test cost 및 query-observer/deadline/fetch parity. 정식 latency/scale/relevance는 E4/E1에 한 번만 실행 |
| MISC-07 | 선택된 registry profile·actual command/input/native capability/replay/exclusion 정합성. 지원하는 실제 multi-repo/product pilot; 작은 fixture로 all-language/full-platform을 승격하지 않음 |

필수 입력이 없으면 그 claim만 BLOCKED, 선택됐으나 미실행이면 NOT_RUN이다.
원 source의 완료된 main CI/cache/lifecycle/history 결과는 ADR에 남기며 새 source 결과와 섞지 않는다.

## Semantic

Owner: producer + semantic/search-plane/SDK/operator. Contract:
[generation/cache](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md),
[selected semantic/ANN](../../../adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md#semantic-and-ann-proof).

- 실제 typed-source ReplaceGeneration/Delta/no-op/tombstone membership과 prior-base,
  resolver/aggregate/outbox retained state·paired restart를 두 저장소의 matching source로 검증한다.
- Partial derivation 후 restart·delete/replacement·complete seal·model/dimension refusal·blocked activation,
  installed CLI/live provider/release를 선택한 rail에서 실행한다. 기존 cache matrix는 완료다.
- 실제 provider request/failure/cache와 선택된 latency/pending-work/seal-lag/model/policy/blocked 이유를
  기존 export에서 확인한다. Boot-time metric 존재는 live 동작 증거가 아니다.
- 증분 reuse는 stable producer semantic-owner identity가 필요하며 불명확하면 full rebuild한다.
  Model/render/normalization 변경은 coordinated rebuild/refusal 및 비용 검증이 필요하다.
  Batched/async worker·새 cache tier/projection은 측정·consumer 요구 이후 별도 결정이다.
- NL semantic relevance는 E1의 독립 pool/holdout, encoder latency/memory는 E4,
  큰 corpus ANN recall은 독립 exhaustive oracle이 소유한다. Exact-symbol 성공으로 대신하지 않는다.

## Release and proof

Owner: I0 + producer/운영 담당. Contract:
[installed/pair/actions](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#installed-paired-and-operational-acceptance).
Proof ID/target/host/staging/graph는 [실행 registry](../../../../tools/ci/proof-authority.toml)와
독립 checker가 소유한다. 이 표는 구현된 foundation을 다시 만드는 작업표가 아니다.

| 기존 scope | 남은 acceptance / 선행 |
| --- | --- |
| R0 / P00 | 선택된 final-source raw nonzero inventory·actual producer/native/SDK 및 relocated replay. Wrong counts/source/binary/host/run·partial/tamper refusal; trusted runner authority |
| R1 / P03–08 | 등록된 concrete release targets와 독립 oracle: activation/exact ACK/publish-only; held physical view·GC/churn/cancel; fixed quality IDs/order/windows; actual SDK wrong identity; real provider identity/egress/budget/cancel; process signal/child/readiness/FD/lease/shutdown |
| R2 / P09 | 실제 release process의 active-root stale/missing/divergent/backend-loss/restoration·cadence. Admin ring denial/caps/wrap/drop/gap/instance/restart/two-UID/request correlation. Existing root probe는 full-content scrub이 아님 |
| R3 / upstream | Semantica aggregate handoff의 별도 expected replace/tombstone/unchanged partition oracle. Supplied scope/prior binding·plan assembly와 구분; 현재 caller/source를 다시 확인하고 producer owner가 구현/검증. 관측된 omission 사고로 표기하지 않음 |
| R4 / S21-11 / P10 | Current-format exclusive-lease backup/verify/restore 및 native exporter mutation/replay·release/authorized target root. Disposable owner fixture는 실제 data/host qualification이 아님 |
| R5 / S21-12 / P11 pair | 두 clean source와 실제 상대 Cargo dependency/lock·fresh build/test/daemon·V2 positive/negative/exact replay. 기존 candidate-only caller archive는 운영 proof가 아님 |
| R6 / S21-12/13 / P11–12 | Concrete deploy/activate/restore-forward adapters·독립 pre/post observer 및 authorized Linux/config/state/retention/rollback 입력. 이어 P12A와 final require-all/bind-source graph; CODE/DEPLOYED/ACTIVATED/ROLLBACK 별도 verdict |

R3 upstream oracle 구현에는 Linux/운영 입력이 선행하지 않는다.
실제 target 입력 부재가 dependent action만 막는다. Historical handoff 감사는 현재 release와 별도다.

## Legacy scope routes

영구 조건은 ADR, 현재 입력/실행 상태는 위 owner에 한 번만 둔다.

| 퇴역한 RFC/플랜/티켓 ID | 현재 owner / 계약 |
| --- | --- |
| CS-BENCH-01/03 · S30-B01/02/03/05/08/09 | E1; [corpus/gold/holdout](../../../adr/OCT-05-001-review-admission-and-result-identity.md#corpus-gold-and-holdout-acceptance), [통계·default](../../../adr/OCT-05-001-review-admission-and-result-identity.md#statistical-units-and-default-decisions) |
| S30-B06 / ARB | E1/E2; per-case official base·original/adapted/no-gold·file/context budget는 위 corpus 계약 |
| CS-BENCH-02/04 · S30-B04 | E2/E4; [native/mutation](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#native-matrix-and-incremental-acceptance) |
| CS-ENG-02 · S30-B07 · J7Q-03/04 | E4; [whole-pipeline/host/sample](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#whole-pipeline-measurement-acceptance) |
| J7Q-01/02 | E1 및 installed owner; [preview/operator](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#consumer-preview-and-operator-acceptance) |
| QIT-00–09 · MISC-03–07 | [Test/platform](#test-and-platform); 품질·비용은 E1/E2/E4 |
| SEM-OWN / May25 | [Semantic](#semantic); async worker/cache/projection 새 설계는 조건부 |
| CS-INT-01 · SEP-21 R0–R6/S21-11/12/13 | I0; [Release/proof](#release-and-proof) |
| CS-ENG-04 · OCT-04 proposed designs | [Deferred regex](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md#deferred-regex-allocation-cap-cs-eng-04) 및 [Proposed ADR](../../../adr/README.md). 선택되지 않은 구현/릴리스 gate를 만들지 않음 |
