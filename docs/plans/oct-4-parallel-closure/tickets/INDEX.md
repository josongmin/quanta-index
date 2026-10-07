# OCT-04 잔여 작업 인덱스

Status: `ACTIVE_RESIDUAL`

완료된 결정은 [ADR](../../../adr/README.md), 정확한 과거 문서·실행은
[복구 인덱스](../../../ARCHIVE-INDEX.md#historical-record-recovery)가 소유한다. 이 인덱스에는 **미완료 조건만** 남긴다.
완료·비적용 scope는 ADR로 흡수했고, 미완료 수용 조건과 기존 ID의 ADR 연결만 유지한다.
[담당 경계](../README.md). B01–B09/J7Q/QIT/SEP-21은 아래 연결한 개별 수용 조건의 owner다.

## 현재 코드 잔여

최초 소스 대조: 2026-10-07, `6a3f6afc8c286176962e722ce75524aefcfa7607`.
후속 main checkpoint: `3e5294ce` current RSS, `c6a9120d` meter drain/검사 구간,
`49a02c15` NL file pair 변환, `9def97ac` CI receipt admission,
`8599f2e8` F15 publication barrier 복구·fault matrix. 2026-10-07 재대조에서
C6의 필수 hosted CI 완료와 F15 구현·집중 회귀를 확인했다.
완료 범위는 [ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints)가 소유한다.
후속 query/restart·paged directory 수리는 `b09c4aa7`·`70521514`·`b262925b`로 main에 반영됐다.
해당 focused 회귀와 daemon214 검증 및 `8642fa9b` hosted6jobs는 완료됐다.
추가 cache matrix4/4와 normal-library strict는 완료됐다. 새 cache-test 코드의 `--lib --tests` strict도 lint3개 수정 후 통과했으며, Large/XL 및 별도 release 수용은 잔여다.
F14/F15, staged upload, EOF cancellation, checked memory/deadline, scanner custody,
P11 공통 producer/parser/checker/recipes 및 hosted CI 분할은 구현돼 있다.
같은 구현을 다시 만드는 티켓은 제거했다. 추가 수리는 실제 비용·반례 또는 target 계약으로 결정한다.

### 코드·테스트 대조 결과

아래는 현재 구현/테스트 범위와 실제 잔여의 구분이다. 테스트 소스가 있다는 사실은
이번 Rust 실행이나 최신 제품 qualification을 뜻하지 않는다.

| 범위 | 실제 코드·기존 테스트 | 남길 작업 |
| --- | --- | --- |
| E1 review/admission | `holdout_review.py`의 blind prepare·frozen validation·finalize, `run.py`의 실행/replay admission 검증, `corpus_binding.py`의 source split 검사 | 실제 reviewer/adjudicator raw·최종 labels·license/gold/holdout·admission 발행 |
| E1 bootstrap | `evaluator.py::mean_ci`의 10,000 draws·16-key/256KiB bounded cache와 `test_bootstrap_cache.py`의 독립 고정 golden·validation controls | 추가 최적화는 actual paired full-caller 비용·parity를 보고 판정 |
| E2 reader/inventory | `live_lexical_external.py`의 selected-project acquired-reader 검증, `opengrok_query_witness.py`, workflow/matrix/ready-drain | 미실행 required cells·actual replay/join; caller가 요구할 경우에만 전체 loaded-reader witness 구현 |
| E4 F15 | full-build/cold-refusal·replay barrier, fresh 복구 query, interrupted delta clone 수리. main70521514까지 반영했고 owner의 36 I/O·36 SIGKILL cuts 및 저장소 회귀 통과. Medium 실제 restart/delete·release binary custody 및 fixed705 daemon214도 완료. `8642fa9b` hosted도 완료 | Large/XL lifecycle 비용/RSS. 기존 fault/query/daemon matrix 재구현 없음 |
| QIT lifecycle | 기존 6 generated Build traces에 반복 unsealed append/replace/restart가 이미 있음. Corpus/kind/owner 독립 키·Symbol/Module Clear·과거 pin 혼합 adapter 모델 3tests·strict Clippy `VERIFIED`. 새 SDK/실제 daemon의 source 삭제/append/old replay/pin/중복·stale CAS/조기 publication 거절·exact retry/rollback/restart와 독립 row/head checker는 최종 8/8·affected strict 통과. coverage-bound SDK의 독립 Chunk/Symbol clear 금지는 유지 | 구현·focused oracle은 잔여에서 제외. 선언된 generated/repeat inventory·native race detector는 별도 `NOT_RUN` |
| QIT concurrency | 기존 race/CAS/sync-delay 회귀와 새 invocation/response·real-time history checker. 복합 duplicate/reorder/caller-delay/rollback/restart 및 고정 음성 반례는 최종 8/8·affected strict 통과 | 선언된 generated/repeat inventory·native race detector 실행은 별도이며 100,000 search-node 한도는 nightly transition 실행이 아님 |
| Semantic | SDK/UDS→production provider/cache/storage의 FooBar/foobar/foo_bar/foo bar semantic/hybrid cold/warm·uncached parity, model-id/revision/dimension rotation/restoration 및 실제 OS-process cache reopen matrix4/4 `VERIFIED` (lint 수정 후 재실행14.674s)·normal-library strict 통과(19.01s). 새 cache-test 코드의 corrected `--lib --tests` strict도 lint3개 수정 후 통과(4.46s). 기존 composition3tests·이전 cfg(test) strict도 통과. 완료 범위는 [ADR](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md#text-vector-and-cache-identity)이 소유 | 기존 OpenAI paraphrase rail(ignored)의 실제 API 입력/실행, installed CLI/live-provider release 및 upstream producer 검증은 별도 잔여 |
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
| P0 · I0-02 / CI/integration | 추가 cache 테스트 source의 hosted 결과 및 선택된 SDK/Contract·PR/release 결과 회수 | 같은 source의 terminal·inventory·receipt. cache focused4/4·corrected library-test strict 및 `8642fa9b` regular main6jobs는 완료이며 기존 CI/F15 수리는 잔여가 아님 |
| P1 · E4 / Scale | matching Large4,096·XL32,768 lifecycle/capacity/cost·OS restart | 원래 fixture, 명시된 profile, live/RSS/retained bytes·typed over-limit refusal |
| P1 · E4 / performance | 전체 sync/read/hash/metadata·segment fanout 비용, scanner·Semble·bootstrap 판정 | 원인별 실제 관측 및 독립 parity. 정식 속도는 admitted host·사전 기준·반복 표본 |
| P1 · E1/E2 / quality | labels/admissions·matching Quanta/Semble pair·native replay/full5·독립 채점 | required cells 및 query/unit/source/index scope, 미판단·실패·제외 분모 설명 |
| P1 · E1 / holdout | 실제 미사용 corpus/query/family·license/gold/name-span·typo 평가 | 독립 truth·critical strata·exposure/underfill, file hit와 declaration recovery 구분 |
| P2 · I0-03 / operations | concrete target adapter·provider·installed Linux/state/actions | 대상·독립 pre/post 계약·actual pair/host/config/state/retention/rollback 입력과 실행 |

### 벤치 실행 잔여 — 2026-10-07 소스·입력 대조

- CS·SG·OG ready9는 원래 source091의 capture/replay가 완료됐다. 영속 raw는
  `/Users/songmin/.codex/task-evidence/ready9-native-recovery-20261006-01a10d0b-v3`와
  `/Users/songmin/.codex/task-evidence/ready9-sgog-recovery-20261006-01a10d0b-v5`에 있다.
  Producer10roles·Python3.13.9 executable 및 원 corpus 전체 replay를 대조했고 일치했다.
  `VERIFIED` · CS9roots 및 SG/OG9roots의 canonical 재생을 완료했다. 각 제품180tasks,
  총540개 응답이며 모두 diagnostic_unqualified다. SG/OG의 Java reader argv/cwd는 원
  p11-operation-authority 경로에도 결속돼 있다. 옮긴 archived-source 경로는 실제로 거절됐으며,
  동일 producer10roles·Java reader bytes를 가진 원 경로에서 재검증했다. Native raw/실행 기록은
  변경하지 않았다. 새 검색·성능 표본이나 최신 main qualification이 아니며 허용된 reuse만 join한다.
  Ready9의20tasks/repo는 NL file 검색이며 bare-symbol workflow와 다른 계약이다.
  지원되는 Quanta code_search_file/Semble lexical-file pair를 사용한다.
- Quanta/Semble Ready9는 source6a3f6afc의9repo/180tasks를 **모두 시도 완료**했다.
  Bat·lo·Mocha·Uvicorn·Zustand는 native pair·독립 replay·5제품 file score projection 완료,
  CLI·Django·Nushell·TypeORM은 고정 term-directory32MiB에서 actual capture가 실패했다.
  `completed-summary.json`과 `capacity-failures.json`을 직접 대조했다. 원본은
  `/Users/songmin/.codex/task-evidence/ready9-native-pair-20261007-01a10d0b`다.
  Main49a02c15의 공통 adapter 수리도 완료됐다. 통과한5repo의10payload/200rows를 변환했으며
  `unjudged-native-union.json`에는72tasks·191개 task/file 독립 판단 공백이 남는다.
  Paged directory 수리는 `b262925b`로 main에 반영됐고 focused41·strict Clippy·daemon214가 통과했다.
  기존 term64B+block128B 계수의 counting defect는 확인되지 않았다.
  후속 fixed `b262925b`에서 CLI capture·독립 replay40/40과 기존 외부 raw를 재사용한5제품 file projection이 완료됐다.
  원본은 `/Users/songmin/.codex/task-evidence/ready9-native-paged-20261007-01a10d0b`다.
  이 결과는 diagnostic이며 독립 판단/admission 완료가 아니다. 실패 후속 실행은 Django·Nushell·TypeORM3repo가 잔여다.
  독립 판단·final admission도 잔여이며, 완료된5repo 및 CLI capture를 다시 대기 상태로 세지 않는다.
  accepted55/PREP·5796 재발행 packet의 명시된 임시 root는 부재하여 원본 replay는 `BLOCKED`다.
  원본 byte 동일 복구 또는 새 source-bound 준비가 선행한다.
- 신규742pairs ledger와 historical Scanner A/B 실행 root는 현재 부재해 해당 원본 replay는 `BLOCKED`다.
  Scanner의 durable Bat79files/338queries suite·blind pack은 복구해 current validator/parity를 통과했다.
  새 source의 two-arm build/capture는 E4-03 잔여다. 기록된 SHA나 과거 수치로 입력을 재구성하지 않는다.
- Scale 담당은 fixed source의 matching release로 Large/XL 실행을 진행한다. 중복 build를 등록하지 않는다.
  C6의 docs/static/Python/tests/bench/verify 필수 hosted contexts는 모두 성공했다.
  원 nextest/inventory의 SHA-256을 receipt와 대조했고 terminal4,241 tests 통과를 확인했다.
  Aux PR coverage의 pending은 완료된 main 필수 CI와 별도 scope다.
  CI9def97ac·F15 8599f2e8 및 후속 clone/query/restart 수리는 main 반영 완료다.
  Fixed70521514의 daemon214와 Medium restart/delete 회귀2개도 완료됐다.
  `8642fa9b` regular main6jobs와4268 Rust tests·receipt 대조 및 후속 cache4/4·normal-library strict도 완료됐다.
  새 cache-test 코드의 corrected `--lib --tests` strict도 lint3개 수정 후 통과했다.
  새 cache 테스트 source의 hosted 결과는 이전4268 결과와 별도로 회수한다.
  Frozen492d2fdc의 XL lifecycle 기능 proof도 완료돼 있다. 최신 profile의 Large/XL 비용/RSS 종료는 별도 잔여다.
  선택한 Linux 정식 rail은 macOS에서 unsupported_host로 거절됐다. Foreign Rust/Scale도
  실행 중이므로 로컬 진단을 Linux 성능 판정으로 세지 않는다. 실행 전에 host를 다시 확인한다.
- `source-split-prepare/validated.json`은 source split만 검증한다. License/gold/acceptance가 아니다.
  Workflow/matrix/join/decision fixture 통과도 actual product search나 benchmark samples를 대신하지 않는다.
- `VERIFIED` · 수리 후 main49a02c15에서 workflow/matrix/five-product oracle/fresh join/default decision의
  집중 회귀 110개 통과(6.70s). 실행: `PYTHONDONTWRITEBYTECODE=1 uv run --frozen --extra dev python -m pytest tools/ci/tests/test_code_search_workflow.py tools/ci/tests/test_code_search_matrix.py tools/ci/tests/test_lexical_five_product_oracle.py tools/ci/tests/test_identifier_robustness_fresh_join.py tools/ci/tests/test_retrieval_default_decision.py -q -p no:cacheprovider`.
  누락·중복 cell, source/query/profile drift 및 raw/report 불일치 거절의 fixture 검증 범위다.
  Source9221d771의 hosted `just rust-bench-build` 52 executables는 `--no-run` 컴파일 결과이며 성능 표본이 아니다.

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

`BLOCKED` · SQLAlchemy 잔여146pairs·Zellij476pairs 최종 AI 정답 판정 및 Tailscale UDP
필터 밖 grade1/3 rubric 확정. 원본 SQL334/480·reviewer raw를 보존하고 quota/auth/model을 확인한다.
재개 입력은 `qi-b08-closeout-20261004-2i72kj91/c3-review-resume-quota-qcshswey`의 frozen
corpus/suite/query/rubric과 valid raw다. 완료: C3 240tasks의 judgment provenance·issued/excluded/
failed/blocked 설명. 미판단 pair는 채점에서 제외하며 AI를 human으로 표기하지 않는다.
원 `actual-review-terminal.json`과 `result.json`은 SQLAlchemy/Zellij/Tailscale 모두 exit1·`FAILED`다.
`quota-wait.json`의 reset은 2026-10-04 기록이므로 현재 quota 거절의 근거로 재사용하지 않는다.

### O4-E1-02

`BLOCKED` · 신규151tasks/742pairs의 두 reviewer+adjudicator 판단 및 원 valid labels 병합.
Bat358+51=409를 재호출하지 않는다. 원 ledger
`/private/tmp/qi-current-nine-supplemental-pool-97eedd-actual-v1/ledger.json`은 현재 부재하다.
기록된 SHA `78013ed33e5647bfa5e109fc5edabb422e41dc48d0cbf4f63f713785642818ec`의 동일 원본 복구
또는 새 actual-native blind pool 준비가 선행한다. 제품명/순위/점수는 review에 노출하지 않는다.
완료: source/query/rubric/threshold/model에 결속된 labels·독립 raw replay 및 reused/unresolved/excluded 집합.

### O4-E1-03

`NOT_RUN` · ready9 후속 final-source 및 SQLAlchemy/Zellij/Tailscale admission ISSUE.
5796 재발행 packet의 임시 경로는 부재해 원본 replay는 `BLOCKED`다. Bat409 merged qrels/suite/pack,
각 repo source/runtime·split/license/review·matching proof를 동일 bytes로 검증한다.
CLI20tasks/519pairs의 historical ISSUE와 중단된 remaining batch를 구분한다.
C5 stale4 suites는 reissue 또는 명시적 exclusion. 완료: 원 labels provenance를 유지한 새
admission/result, wrong-source/threshold/grade/family/unit/runtime/proof/license 거절.

### O4-E1-04

`NOT_RUN` · 다른 supported declaration-name/span·typo cells와 후속 source 영향 평가.
Gin exact1,196의 historical diagnostic을 제품 비교/최신 main/holdout으로 승격하지 않는다.
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
원 source와 qrel-only reuse를 구분한다. 살아 있는 process/malformed/wrong-repo terminal/
missing/output 경합은 success가 아니다. 완료: 누락 없는 terminal/input-byte inventory와 실패 sibling에 독립적인 실행.

### O4-E2-04

부분 완료 · Ready9 전체9repo capture 시도·5repo pair/replay/projection은 `VERIFIED`.
용량 코드 수리는 main `b262925b`에 반영·push됐다. 실패4repo의 새 source 재실행과191개 독립 판단이 잔여다.
별도 C3의 남은3repo 및 다른 lanes는 이9repo와 구분한다.
CS/SG/OG ready9 source091 capture/replay는 완료돼 있다. Ready9 NL file20tasks/repo는
`natural_language_file`/Semble `lexical-file`의 distinct-file pair로 실행한다.
`code_search_file` atoms·bare-symbol workflow는 이 NL query 계약의 실행 경로가 아니다.
완료된5repo capture/replay/projection 및 main49a02c15 adapter 수리는
[Ready9 ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#completed-ready9-capture-scope)가 소유한다.
원 source `6a3f6afc`의 CLI·Django·Nushell·TypeORM actual capture는 `FAILED`: lexical-file authority의 고정 `term_directory_bytes=32MiB`를 초과했다.
Native runner에 해당 정책을 바꾸는 startup/spec 옵션은 없다. 4개 실패는 native record와
promoted root가 없고, failure descriptor의 stderr/resource 해시까지 확인했다. 원본 입력/명령/raw/결과는
`/Users/songmin/.codex/task-evidence/ready9-native-pair-20261007-01a10d0b`에 보존한다.
RCA: source-key bucket마다 같은 trigram의 offset/count/hash를 모두 상주시키면서 실패 직전 약52만 term 행이 누적됐다.
수리: 128term마다 fence/table SHA만 보관하고, 조회는 인증한 table page와 선택한 posting list의 별도 range/SHA를 검증한다.
생산자/root/cold verifier가 같은 page 단위 산식을 사용한다. 32MiB 한도는 유지하며, posting wire v2와 새 policy identity가 구형 root를 rebuild-required로 거절한다.
`VERIFIED`: `./scripts/cargow --lane test-fast-lane nextest run -p quanta-index-lexical --lib --test f15_file_authority --all-features --locked -E 'test(file_authority::) | binary(f15_file_authority)'` 41/41;
`./scripts/cargow --lane clippy-lane clippy -p quanta-index-lexical --all-targets --all-features --locked -- -D warnings` 통과.
고정 source `b262925b`의 daemon scope도 `VERIFIED`: `just rust-test-e2e test-fast-lane` 4binary·214/214 통과·1 skipped, Nextest `3d7dac7a-73f7-4c10-bc18-a4d421dfb7ac`.
같은 source의 debug runner/searchd build와1529file source closure 재검증은 통과했고, binary copies/SHA는 새 실행 root의 `binary-pins.json`에 고정했다. 최신 main 전체/release qualification은 아니다.
새 source 실제 재실행은 `/Users/songmin/.codex/task-evidence/ready9-native-paged-20261007-01a10d0b`에서 진행한다. Owner 회귀는 실제4repo 용량 proof를 대신하지 않는다.
최종 `completed-summary.json` 및 `unjudged-native-union.json`에72tasks·191개 task/file 독립 판단 공백을 결속해 E1 입력으로 남겼다.
원191개에 대한 두 blank review 입력은 `/Users/songmin/.codex/task-evidence/ready9-unjudged-review-inputs-20261007-01a10d0b`에 원문·해시/threshold를 결속했고 grade/reviewer ID는 null이다.
Ready9 전체 성공·quality/speed qualification은 아니다. 잔여는4repo 실제 재실행 gate, 독립 판단 및 아직 실행하지 않은 inventory다.
원 runtime/input/producer bytes 및 허용 reuse를
검증해 결합하며 source97/5796/091 raw를 새 소스로 재표기하지 않는다.
Required lanes: exact1,196; prefix/infix/components; default/explicit typo; no-answer; C3 NL240;
Gin20; ARB original17/88·adapted88; B09 OSA/CLARC/CSN. Four typo populations
1,192/1,178/1,192/1,192는 서로 합산하지 않는다. ARB v1은 현재 v2 adapted88 결과가 아니다.
완료: raw/exit/request/source/unit/clock 독립 replay 및 마지막 unjudged union의 E1 인계.

### O4-E2-05

`NOT_RUN` · 고정 Semble package/lock/env/model/assets/input의 parent/worker 반복 A-B 비용·재사용 판정.
Immutable validation·native rows/status parity 및 unattributed residual을 확인한다.
정식 speed는 [E4-06](#o4-e4-06)의 host/boundary/schedule을 따른다.

### O4-E2-06

`NOT_RUN` · Bat 밖에서 quality warmup0을 선택할 때의0/1 parity.
같은 task set/cold probe/profile/seed/repetitions와 protocol SHA/schedule/phase ledger,
task별 rows/status/score bits를 두 actual run에서 대조한다. 그 전에는1을 유지한다.
Order-sensitive 차이가 있으면0을 채택하지 않으며 speed는 warmup≥1이다.

## E4

Owner: lexical lifecycle/query cost·scanner/scale/load·policy RCA.
Contract: [OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

### O4-E4-01

`NOT_RUN` · matching release의 full/delta/delete/no-op/reopen 전체 비용 qualification.
Native segment 재사용·live-BM25·F15 changed-bucket publication은 구현됐다.
Remaining: cold 전수 검증·metadata/custody 읽기·hash/fold, delete-mask O(max_doc), correction/
NoMerge segment fanout, transient peak 및 foreground/maintenance CPU/read/write/fsync 분해.
Causal producer의 source/binary와 independent markers를 결속한다. Seal retention gauge는 unique-inode
regular-file st_size이며 restart gauge0·st_blocks·physical I/O/true peak와 구분한다.
완료: 원인별 exclusive 비용·명시적 clock/resource domain·fresh rebuild score/page parity.

### O4-E4-02

완료 · fault/query/clone-retry/restart 구현과 집중 검증은
[ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints)로 이관했다.
최신 hosted gate는 [I0-02](#o4-i0-02), matching Large/XL 비용·RSS·restart는
[E4-01](#o4-e4-01)/[E4-05](#o4-e4-05), 더 넓은 storage 수용은
[QIT-03](../../jul-15-sota-test-hardening/tickets/00-ticket-status-board.md)가 소유한다.
이 ID로 같은 fault/query/daemon 구현·실행을 다시 등록하지 않는다.

### O4-E4-03

입력 복구 완료·fresh capture 재실행 필요 · historical Scanner A-B original `/private/tmp/qis.utp62qk5`와
옛 binary는 부재하여 그 실행의 원본 replay는 `BLOCKED`다. Fixed338/79files·ParseFailed6의 과거
diagnostic은 원 scope의 기록이며 speed/adoption verdict가 아니다.
Durable `qi-b08-closeout-20261004-2i72kj91/mechanical-c4-amended-f3d2ae29/bat/declaration_name_osa1_casefold/`
suite·blind pack 및 frozen Bat revision `4608fc959aa8abf80d32198836511a570b7ae9ea` 입력을 찾았다.
Current suite validation·derived blind-pack exact parity와 source
`8642fa9b3b47ac58e4b3d9f2feaea66c599be29d`의 canonical Unicode-control/ASCII pair identity
검증은 완료됐다. v1은 코드 검증 우선으로 중단한 미완료 build이며 성공 receipt가 아니다.
후속 v2 root는 `/Users/songmin/.codex/task-evidence/scanner-ab-20261007-current-main-01a10d0b-v2`다.
v2 baseline fresh release build는 `VERIFIED`(42m58s)지만 capture는 UDS path155bytes가
macOS103bytes limit을 초과해 `FAILED`다. Capture receipt와 candidate/comparison 결과는 없다.
`scanner_build_custody.py`가 expensive build 전에 기존 runner의 socket-path preflight를
호출하도록 수리했다. ASCII·다중바이트·parent symlink 경로의 build 전 거절을 포함한 custody/source
회귀47/47 및 기존 runner path 회귀2/2가 통과했다. 짧은 외부 output root에서 fixed two-arm을
fresh 재실행한다. Candidate build/capture와 비교는 아직 `NOT_RUN`이다.
이는 복구된 입력의 fresh two-arm 진단이며 historical binary 결과 재생이 아니다.
두 arm의 source→binary build provenance,
independent tokenizer/full-DP·bytes/span/case/order/status/cursor/work/config parity를 대조한다.
`query_timing_overhead.py --scanner-ab`는 항상 diagnostic_unqualified이며 observer on/off와 다른 비교다.
완료: 사전 whole-caller 기준의 keep/modify/withdraw. Child 개선은 whole-call 악화를 상쇄하지 않는다.

### O4-E4-04

`NOT_RUN` · 조건부 persistent token authority. E4-01/03의 actual after-scanner full-caller에서
repeated-scan 병목과 memory/build tradeoff가 충족될 때만 채택한다.
Exhaustive tokenizer/OSA1/source/grammar/folded name witness와 delta/delete/no-op/reopen,
cold-open/build/residency/cap/cancel 독립 수용이 필요하다.

### O4-E4-05

후속 측정 잔여 · 현재 변경 source/profile의 Medium256/Large4,096/XL32,768 비용/RSS·open-loop·실제 OS restart.
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

추가 cache 테스트 source의 hosted 결과 및 selected SDK/Contract·PR/release proof 회수가 잔여다.
Cache focused4/4·normal-library strict 및 lint3개 수정 후 corrected `--lib --tests` strict는 완료됐다. 같은 focused 실행을 다시 잔여로 등록하지 않는다.
`8642fa9b` regular main6jobs는 완료됐고,4268 Rust tests·30 skipped 및 receipt의
source/command/inventory/raw/result 대조를 Scale/Scanner owner가 확인했다.
완료된 regular main 범위는 [ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints)이 소유한다.
`VERIFIED` · `c6a9120d`의 필수 docs/static/Python/tests/bench/verify contexts를 live GitHub API로 확인했다.
CircleCI job2011의 actual receipt·raw/inventory 해시는 일치하고 terminal4,241 tests가 통과했다.
기존6a3 실패의 counter/rename 수리는 완료됐으며 추가 CI 재수리로 남기지 않는다.
Pending aux PR coverage는 main 필수 CI 완료와 별도다.
CI receipt admission `9def97ac`과 F15 `8599f2e8` 및 후속 query/restart·paged directory 수리도 main에 반영됐다.
각 checkpoint의 focused/daemon 및8642 regular main 결과는 완료이며 그 이후 변경 source의 영향 gate만 별도로 회수한다.
과거 C6 결과를 최신 source로 재표기하지 않는다.
기존e371 SDK/Contract/runtime/open-loop 및9221d771 hosted gate는 원 source의 완료로 ADR/Git에 보존한다.
공개SDK/contract·wire/state/query 변경은 registry/Justfile의 영향 rail과 portable replay를 실행한다.
Runtime autotests=false의 실제 suite/selector를 사용한다. Compiler/focused/local 결과를 full/release로 승격하지 않는다.

### O4-I0-03

구현 잔여 · concrete deploy/activate/restore-forward adapters·independent pre/post contract.
실제 Linux host/config/state/retention/rollback 입력은 `BLOCKED`이며 dependent action은 미실행이다.
공통 typed producer/parser/checker/recipes는 완료됐다.
별도 exact-pair caller/kernel build/test/daemon custody는 `NOT_RUN`; runner-candidate-only는 operational receipt가 아니다.
외부 Semantica R3 expected semantic partition/omission 검증은 producer owner의 별도 연동 잔여다.
기존 producer의 prior-state binding·plan assembly와 독립 expected-partition 검증을 구분한다.
Target/Linux 입력 부족이 upstream 검증 코드 구현의 선행은 아니다.
[S21-12](../../sep-21-search-plane-sota-hardening/tickets/S21-12-cross-repo-terminal-receipt-cutover.md)와
[SEP-21 R0–R6](../../sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md)가
P00–P12/provider/installed/state/pair/operations의 세부 수용 및 graph를 소유한다.
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

[Benchmark B01–B09](../../sep-30-code-search-benchmark-trust/tickets/INDEX.md),
[remediation](../../sep-27-code-search-remediation/readme.md),
[MISC](../../sep-27-misc/tickets/INDEX.md), [J7Q](../../jun-7-search-product-quality/tickets-wave2/INDEX.md),
[semantic](../../may-25-search-owned-semantic-derivation/tickets/INDEX.md),
[QIT](../../jul-15-sota-test-hardening/tickets/00-ticket-status-board.md)는 실행 소유 경계다.
같은 작업/무거운 rail을 다른 packet에 중복 등록하지 않는다.
