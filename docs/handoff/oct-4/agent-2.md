# Agent 2 — 벤치 통합·검수·다제품 비교 handoff

작성: 2026-10-04 15:19 KST. 이 문서는 이 채팅의 작업 인계이며, 제품 품질·성능·배포 적격성 판정이 아니다.

## 1. 담당과 현재 작업 위치

- 담당: 벤치 계약/실행 증거 통합, 실제 AI 검수 완성, C3 admission, 외부 제품 시간 경계, 다섯 제품 캡처·채점·보고.
- 구현·유닛테스트 위치: `/Users/songmin/Documents/code-new/quanta-index`, branch `main`.
- 이번 재확인 HEAD: `2062fed3ff86287a0914ece99dc2fe0af829c29b`. 공유 staged 변경이 많다. HEAD만으로 실행 소스를 대표할 수 없다.
- 직전 목록 작성 시 HEAD는 `8ee2f1ea`였다. 이후 변경된 문서 커밋과 다른 채팅의 소스 수정을 반영해 상태를 갱신했다.
- 새 기능 브랜치로 분리하지 않는다. 실행 중 소스 변화를 막기 위한 immutable export/고정 checkout은 검증용이다. 오래된 export를 main에 덮어쓰지 않는다.
- 이 문서 저장 요청에서는 상태·원본 산출물·타 채팅 결과를 읽고 문서만 작성한다. 새 모델 호출, 제품 검색, 벤치 재실행, 커밋·푸시는 실행하지 않았다.
- 공유 dirty/staged 전체는 이 채팅의 변경이 아니다. 다른 채팅 변경을 일괄 수정·스테이징·커밋하지 않는다.

## 2. 사용자 의도와 유지할 계약

1. 기본 Gin lexical 평가는 **1,196질의**다. 기존 300은 하위 집계/과거 RCA 자료이며 기본 실행 분모로 되돌리지 않는다.
2. exact, prefix, infix, components, 오타, no-answer, NL/작업형 평가를 별도 표로 보여준다. 모든 질의를 하나의 순위에 합치지 않는다.
3. Quanta, Semble, Sourcegraph, cs, OpenGrok을 준비된 동일 입력에서 실제 실행한다. 제품별 요청 정책·지원 여부·순위 단위·파일 universe 차이를 기록한다.
4. 시간은 호출 합계, p50/p95, 전체 경과시간, 인덱싱 단계별 시간을 나눠 보여준다. 서로 다른 타이머를 동일 성능 비교로 표시하지 않는다.
5. 구현은 main에서 진행한다. 기존 캡처·frozen corpus·`/private/tmp/g3`는 읽기 전용이며, 새 실행은 새 외부 출력 루트에 둔다.
6. 사용자 승인으로 사람 검수를 독립 AI 검수·조정으로 대체했다. 실제 모델 호출과 source 근거를 보존하고, 사람 reviewer 영수증을 만들지 않는다. 현재 AI 산출물은 `qualified:false`, `human_provenance_attested:false`다.
7. `capped`는 실행 실패나 자동 누락이 아니다. 빈 결과도 정상 검색 응답일 수 있다. 실행 실패, 검색 품질, partial/cap, 미검수는 구분한다.
8. 파일 Hit@10과 정확한 선언 name span 회수는 다른 지표다. 반환 문맥이나 파일 전체를 선언 정답 span으로 사용하지 않는다.
9. 기존 evaluator/planner/IR을 진화시킨다. 중복 채점기, 새 IR 버전 쌍, 불필요한 Semantica API 확대를 추가하지 않는다.

## 3. 이 세션에서 완료·확인한 것

아래는 과거 실행의 범위와 현재 인계 상태다. 이전 테스트를 이후 모든 main 변경에 대한 증거로 승격하지 않는다.

| 항목 | 판정/범위 | 근거 |
| --- | --- | --- |
| main과 B08 작업공간 비교 | `VERIFIED`: 당시 존재한 B08 작업공간 9개 clean, 커밋은 main의 조상. 분기 밖 구현을 가져올 필요 없음 | [B08 티켓](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md), main integration 절 |
| B09 실행 export와 main 비교 | `VERIFIED`: 바인딩된 56파일 중 52 byte-identical, 누락 0. 나머지 4개는 main의 후속 변경 | 같은 티켓. `d1a1b709` 소스를 main에 복사하면 최신 변경이 사라짐 |
| cs NL literal 요청 수정 | `VERIFIED`: `fae6924a`가 main에 통합됨 | 기존 `test_live_lexical_external.py` 검증; 새 실행 상태와 분리 |
| 첫 6저장소 다제품 원본 재생/풀 감사 | `VERIFIED`: 120질의 × 5제품 = 600응답, 새 미검수 합집합 375쌍 | `c3-all-five-pool-audit-gsZWij9u/six-repository-summary.json` |
| NL native 빈 결과 RCA | `VERIFIED`: 세 제품 각각 실제 단일 심볼 control 적중, 심볼+없는 term control 빈 결과. 6/6 control 완료 | `c3-native-nl-zero-control-4QVkyBQg/result.json` |
| 원문 literal AND 필요조건 분석 | `VERIFIED`: 첫 120 NL질의 중 모든 surface term을 한 파일에 포함하는 사례 0/120 | 같은 루트 `literal-and-feasibility.json`. OpenGrok analyzer의 전체 참조 구현이라고 해석하지 않음 |
| C3 native 색인 범위 증거 | `VERIFIED`: 해당 B08 universe 13,347파일의 Sourcegraph stored content 및 OpenGrok source/posting 참조 검사 | §9의 색인 증거 루트. B09의 11,695파일 universe를 대신하지 않음 |
| 이전 main의 집중 테스트 | `VERIFIED`: native capture/review 180개 + diagnostic v9/ingest 5개 = 185개 통과 | §8 명령. 이후 다른 채팅 변경 전체의 검증은 아님 |
| 최종 비교·공통 시간·전체 qualification | `NOT_RUN` | 라벨, 새 캡처, 최종 소스/환경 결속이 남아 있음 |

첫 6저장소 C3에서 Quanta/Semble은 각각 120응답 모두 비어 있지 않았고, SG/OG/cs는 각각 120응답 모두 정상 빈 결과였다. literal term AND와 NL token-OR/BM25의 요청 계약 차이가 확인됐다. 이것으로 전체 제품의 의미 검색 품질 순위를 확정하지 않는다.

## 4. 이번 재확인에서 정정한 최신 실행 상태

### 4.1 원본 C3 검수

- 실제 발행된 suite는 **7저장소 × 20 = 140/240질의**다: bat, cli, lo, mocha, uvicorn, zustand, nushell.
- 이전 감사의 원본 반환 쌍 coverage는 **670/1,324**다. 이번 저장 요청에서 쌍 coverage는 재계산하지 않았고 suite 질의 수와 terminal은 다시 읽었다.
- `c3-review-resume-quota-qcshswey/terminal.json`은 이제 `FAILED`다. **Django 실행 중 / SQLAlchemy·Zellij 대기라는 이전 설명은 폐기한다.**
- TypeORM·Tailscale은 앞선 조사에서 unresolved reviewer/adjudicator 응답으로 확인됐다. Django·SQLAlchemy·Zellij의 상세 원인은 각 실제 로그에서 추가 분해해야 한다. 빠른 실패 시간을 quota 오류로 추정하지 않는다.

| 저장소 | 실제 terminal | 해당 실제 검수 실행 시간 | 남은 일 |
| --- | --- | --- | --- |
| nushell | `VERIFIED`, suite 발행 | 2,444.060초 | 기존 발행/입력 결속 유지 |
| typeorm | `FAILED`, exit 1 | 714.014초 | unresolved 조정 재검수 후 발행 |
| tailscale | `FAILED`, exit 1 | 980.050초 | unresolved 검수 재시도 후 발행 |
| django | `FAILED`, exit 1 | 1,532.325초 | 로그 원인 확인, 유효 캐시 보존해 재시도 |
| sqlalchemy | `FAILED`, exit 1 | 3.744초 | 로그 원인 확인, 재시도 |
| zellij | `FAILED`, exit 1 | 4.271초 | 로그 원인 확인, 재시도 |

이 시간은 모델 검수 실행 시간이다. 제품 검색 지연이나 전체 파이프라인 시간으로 합치지 않는다.

### 4.2 보충 검수

- 첫 6저장소의 미검수 합집합: bat 51, cli 133, lo 30, mocha 63, uvicorn 54, zustand 44 = **375쌍**.
- 이전에 완료한 lo 보충 **41쌍**은 `c3-lo-supplemental-actual-review-fzs2bigu/issued-merged`에 있다. 새 lo 30쌍과 구분하며 기존 41쌍을 잃지 않는다.
- bat 외부 요청 builder가 `answerability_min_grade` 없이 payload를 만들어 model call 전에 실패했다. cli/lo에서도 재현했다. 잘못된 preflight는 실제 payload 생성까지 도달하지 않았다.
- 기존 supplemental queue는 `FAILED`로 종료했다. 새 375쌍 전체의 실제 검수·qrels 발행은 끝나지 않았다.
- **Agent 3가 canonical 준비/스케줄러 수정을 완료했다.** bat/cli/lo **53 task / 214쌍 / 14 payload** preflight가 통과했고, 실제 모델·제품 호출은 0이다. 이 214쌍을 검수 완료로 세지 않는다.
- 나머지 6저장소 결과의 union은 아직 완성되지 않았다. 375를 전체 최종 미검수 분모로 고정하지 않는다.

### 4.3 대기 프로세스와 실패한 watcher

- 이번 `ps`에서 pair controller PID **32335**와 pool collector PID **88605**는 관측됐다. 각각 `capture_pairs.py`, `collect.py`다.
- `c3-all-five-pool-audit-gsZWij9u/remaining-terminal.json`은 **typeorm non-verified capture 때문에 FAILED**다.
- 프로세스 생존은 진행/완료 증거가 아니다. 다음 작업자가 PID·terminal·생성된 셀을 재확인해야 한다.
- 기존 실행을 중복 시작하거나 살아 있는 driver를 덮어쓰지 않는다. 검증된 새 scheduler로 재개할 때 기존 job과의 소유권·중복 여부를 먼저 정리한다.

## 5. 다른 채팅의 소유권 — 중복 수정 금지

| 담당 채팅 | 파일/범위 | 최신 확인 상태 | Agent 2 경계 |
| --- | --- | --- | --- |
| **「ㅔ벤치 준비 - 코퍼스」**, `01a0f7ae-c9bc-75b1-8ad8-b1897fe045ef`, Agent 3 | `holdout_review.py`, `execution_batch.py`, 관련 README/테스트. ingest·SDK·scale/B07 성능 범위 | 임계값 suite 결속 및 실패-aware scheduling 수정 완료. 111테스트, 실제 payload preflight·변조 거절 결과 존재. 모델 검수·제품 검색 미실행 | canonical 수정 재구현 금지. 실제 driver 채택·검수 실행·admission·통합 검증 담당 |
| **「벤치 - 엔진문제」**, `01a0f7b2-75eb-7972-931e-7113778899d2`, Agent 5 | `declaration_parsers.py`, `identifier_robustness_fresh_join.py`, `retrieval_contract.py`, `query_plan.py`/Rust planner, JS/Unicode vendor, typo ranking 관련 | 새 runtime join, stale AST/source attestation, Python/Rust Unicode 요청 mismatch 3개 수정. 보고된 Python 168/Rust 185, 실제 Unicode SDK 질의 2개 통과 | 동일 수정 중복 구현 금지. 최종 캡처에서 source-lock·요청 identity·raw replay 결속 검증 |
| **「커밋하고 main에 푸시」**, `01a0eb9e-afaf-76a2-ae4c-5b4d6bd8f18e` | 공유 변경 publication | 앞선 staged diff-check가 vendor patch context whitespace에서 멈췄음. 이번 요청에서 이 채팅/remote 상태는 재조회하지 않음 | 공유 전체 변경을 Agent 2 저작으로 커밋하지 않음. 소유 변경과 검증 상태 전달/반영 확인 |

다른 두 채팅은 각각 `/Users/songmin/Documents/code-new/quanta-index/docs/handoff/oct-4/agent-3.md`, `/Users/songmin/Documents/code-new/quanta-index/docs/handoff/oct-4/agent-5.md` 작성 요청을 진행 중이다. 이 문서의 링크 확인 시 두 파일은 아직 없었다. 생성 후 각 문서의 최종 소유권과 대조한다.

Agent 3 증거: `/private/tmp/qi-bench-defect-fix-20261004-46iq_iok/RESULT.json`을 직접 읽었다. `actual_api_calls=0`, `actual_product_calls=0`, 111 passed, 571 deselected, 22.83초다. 당시 admission scan의 Django 등 pending 상태는 이후 원본 terminal 실패로 바뀌었으므로 재사용하지 않는다.

Agent 5 증거: `/Users/songmin/Documents/code-new/qi-b09-session-hardening-20261004-0snzc83u/REPORT.md`를 확인했다. 11,272 기존 응답 identity, 367 증거파일 해시, OSA 8,726 task쌍 gold/qrels 보존은 과거 산출물 무결성 범위다. 다섯 제품 최신 실행 완료가 아니다.

## 6. Agent 2 남은 액션리스트

### A2-01 — 실패 검수 재개와 실제 라벨 발행 [P0]

- 5개 실패 저장소의 `actual-review.log`, terminal, 유효 cached batches를 대조해 원인을 분리한다.
- Agent 3의 frozen-suite-bound 준비 함수를 새 외부 driver가 사용하도록 연결한다. 실제 model payload까지 생성하는 preflight를 필수로 한다.
- 기존 유효 호출을 재사용하고 실패/invalid raw도 보존한다. unresolved 쌍의 등급을 강제로 채우지 않는다.
- 원본 240질의 검수와 supplemental union을 실제 독립 모델 2개+별도 조정으로 완료한다. 기존 lo 41쌍을 보존한다.
- 완료: 검수 범위·모델 identity·query/source/rubric 결속, task별 포함/제외·쌍 분모가 재생 가능하고 실제 판단되지 않은 쌍이 채점 대상에 남지 않음.
- 상태: `FAILED` 원본/보충 실행을 보존한 상태. 새 검수 완료는 `NOT_RUN`.

### A2-02 — admission 및 실행 스케줄 통합 [P0]

- 소유 접점: `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/retrieval/run.py`, 기존 admission/schema/proof inventory 호출. `holdout_review.py`·`execution_batch.py` 수정은 Agent 3 소유.
- 준비된 저장소부터 실행하면서 실패/미발행 저장소를 명시적으로 남긴다. 순차 controller가 첫 실패에 무한 대기하지 않게 실제 채택 경로를 확인한다.
- 최종 qrels→suite→blind pack→family split→manifest/receipt를 현 소스에서 검증한다. 필요한 license/model/SDK/contract 입력이 없으면 `BLOCKED`로 표시한다.
- NL-only 진단과 objective+reviewed+no-answer 혼합 decision을 분리한다. 기존 mechanical no-answer strata를 확인한 뒤 필요성을 판단하며 억지 NL negative를 만들지 않는다.
- 완료: ready/failed/pending inventory가 실제 terminal과 일치, 잘못된 family/threshold/query/source/unit 조합 거절, 발행된 각 저장소에 admission 결과 존재.

### A2-03 — native completed-response 타이머 [P1, 독립 진행 가능]

- 1차 소유 파일: `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/retrieval/live_lexical_external.py`, `/Users/songmin/Documents/code-new/quanta-index/tools/ci/tests/test_live_lexical_external.py`.
- 현재 `_http`는 request 생성 후부터 raw bytes까지, `_process`는 프로세스/stdout 종료까지 측정한다. decoder/정규화는 그 밖이다. 해당 파일들은 이번 확인에서 HEAD 대비 delta가 없었다.
- 요청 구성 전 시작→HTTP/process→정규화된 응답 materialization 완료까지 하나의 monotonic clock을 둔다. raw 산출물 저장은 측정 밖으로 둔다.
- 과거 `elapsed_ms`는 transport 시간으로 유지한다. decode 시간을 별도로 더하거나 boundary 이름만 바꾸어 공통 시간으로 승격하지 않는다.
- 유닛테스트: 고정 fake clock golden, 빈 결과·오류·절단·timeout, 순서/경로/시간 변조 거절. 표본의 성공만 선택한 시간 평균을 전체 비교로 표시하지 않는다.
- 완료: boundary가 요청/응답 기록과 재생에 결속되고 실제 반복 실행은 B07 담당과 단일 실행권으로 수행.
- 상태: 구현 및 공통 성능 실행 `NOT_RUN`.

### A2-04 — 최종 다섯 제품 셀 실행·raw replay [P1]

- cohort×repository×product×profile×unit의 required-cell inventory를 실행 전에 고정한다. 오래된 남은 셀 수를 재사용하지 않는다.
- 기존 정상 캡처는 입력·요청·source와 영향을 대조해 재사용 가능 여부를 결정한다. 영향 없는 셀을 무조건 반복하지 않는다.
- 신규 실행은 새 외부 루트에서 한다. 제품별 실제 색인 범위·service/config/binary identity를 필요한 전후 구간에 결속한다.
- SG/OG의 B08 13,347파일 증거를 B09 11,695파일에 적용하지 않는다. 서비스 query/content/defs/refs 범위도 혼동하지 않는다.
- 완료: 성공/빈 결과/unsupported/cap/partial/error/missing을 모두 분리한 inventory, 원본 HTTP/process bytes 재생, 미검수 union 후속 검수 완료.

### A2-05 — 평가 묶음별 결과표 [P1]

| 묶음 | 단위/완료 기준 |
| --- | --- |
| Gin exact **1,196** | 파일 Hit@10/MRR@10와 운영 coverage. 기존 300은 별도 부분집계 |
| prefix / infix / components | 각각 별도 분모·요청 정책·unsupported·cap 표시 |
| 오타 | 삽입/삭제/치환/전치 별도. 기본 검색과 명시적 typo 정책 별도. file hit와 정확한 declaration-name span 회수 분리 |
| no-answer | 불필요 결과 반환 및 false positive, 실패 응답과 정상 empty 구분 |
| C3 12저장소 NL 240 | 최종 검수된 file qrels로 채점. 제품별 NL→literal/OR/BM25 정책을 명시 |
| Semble 공식 Gin 20 | 의미 11/구조 6/심볼 3 사례 분리, 다제품 blind union 완성. 1,196에 합산 금지 |
| ARB Gin 88 | 이미 adapted 88/88 완료한 과거 범위 보존. 공식 원문 17/88 부분 실행과 adapted를 분리. upstream 고정 snapshot/all-files 유지 |
| B09 OSA / CLARC / CSN | 이미 준비된 입력의 범위별 분석, unknown/excluded 분모 보존. 전체 benchmark import 완료라고 쓰지 않음 |

- 기존 `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/retrieval/evaluator.py`와 비교 reporter를 사용한다. 계산 수식 중복 구현을 피한다.
- 공통 eligible task ID 집합과 conditional quality, 운영 점수/coverage를 함께 출력한다. 미검수 파일을 grade 0으로 바꾸지 않는다.
- 다중 저장소 불확실성은 repository cluster로 계산한다. 한 Gin 저장소 1,196이 독립 저장소 표본 1,196인 것처럼 해석하지 않는다.
- pooling exposure·leave-one-system-out 민감도를 점검한다. tuning에 노출된 release를 unseen holdout으로 발표하지 않는다.
- 완료: 원본 row 재계산과 보고서 일치, 단위·case·span·분모 변조 거절, 품질 순위와 MRR 순위를 구분.

### A2-06 — 완료 판정·소유 변경 반영 [P1]

- 최종 변경에 맞는 좁은 테스트→실제 요청/SDK seam→필요한 캡처 순서로 검증한다. Rust는 `./scripts/cargow`/Justfile 사용.
- proof inventory/schema/driver의 공통 파일 변경은 단일 통합 담당으로 조율한다. Agent 3/5가 만지는 파일을 동시에 덮어쓰지 않는다.
- commit/push 담당과 소유 diff·검증 범위를 구분한다. vendor patch whitespace는 leading context space를 무작정 제거하지 않는다. artifact/provenance 변경 여부는 해당 소유자가 판단한다.
- 최신 source/overlay 및 raw evidence에 맞게 B08/B09·handoff 상태를 갱신한다. old tests·compile·commit을 새 품질/배포 qualification으로 승격하지 않는다.
- 여기서 추가하지 않을 것: 새 Semantica API, 중복 IR, 전체 CoIR/CORE 수입, 증거 없이 새로운 ranker/청킹 교체. 기존 typo literal-first misses와 NL relevance 개선은 엔진 담당의 정책 작업과 연결한다.

## 7. 실행 순서와 병렬 경계

1. 즉시 병렬 가능: native 타이머 구현/유닛테스트, 실패 로그 조사, 기존 산출물 분모·raw replay 감사.
2. Agent 3/5 수정 소비: 실제 payload/큐/planner/parser/source-lock 통합 확인 후 원본·supplemental 검수 재개.
3. 저장소별 검수·admission 완료 시 그 저장소의 ready 셀 실행. 하나의 실패가 나머지를 막지 않으며 누락은 실패/미실행으로 남긴다.
4. 다제품 새 결과 union 검수→최종 qrels freeze→재채점. 입력/요청이 바뀌면 영향받는 캡처만 새로 실행한다.
5. 공통 타이머 성능 실행은 B07 담당과 직렬 실행한다. noisy host나 자원 admission 실패를 우회해 성능 qualification을 만들지 않는다.
6. 최종 source와 소유 변경 반영 확인 후 영향 fixture/contract/SDK checks, 보고서·티켓 마감.

## 8. 실행 명령과 검증 범위

### 이전 이 채팅의 테스트

```sh
.venv/bin/python -m pytest tools/ci/tests/test_live_lexical_external.py tools/ci/tests/test_holdout_review.py -q
```

`VERIFIED`: 당시 main `e43cda8c`와 관측 overlay에서 **180 passed / 139.41초**. 이후 Agent 3/5 변경 전체를 보장하지 않는다.

```sh
.venv/bin/python -m pytest tools/ci/tests/test_retrieval_benchmark.py -q -k 'diagnostic_v9_replay_requires_new_children or direct_quanta_capture_accepts_current_ingest_children or ingest_preparation_file_children'
```

`VERIFIED`: **5 passed / 565 deselected / 3.29초**. diagnostic v9 및 ingest child 증거 접점만 검증했다.

### 이번 handoff 작성 시 실행한 확인

- `git rev-parse HEAD`, `git branch --show-current`, `git status --short`, `git log -3 --oneline`: `VERIFIED`, 현재 main/공유 overlay 확인.
- `read_thread` 두 채팅의 마지막 완료 턴: `VERIFIED`, Agent 3/5 소유 수정·보고된 proof scope 확인. 모든 타 채팅 테스트를 여기서 재실행한 것은 아님.
- 원본 resume/union/supplement terminal과 issued suite JSON 읽기: `VERIFIED`, 140/240 발행, 원본 5개 실패 및 watcher 실패 확인.
- `ps -axo pid,ppid,etime,command`: `VERIFIED`, 위 2개 controller 생존 관측. 완료 판정은 아님.
- `git diff HEAD --stat -- tools/benchmark/retrieval/live_lexical_external.py tools/ci/tests/test_live_lexical_external.py`: `VERIFIED`, 타이머 소유 후보 파일의 delta 없음.
- 새 모델 호출/새 제품 호출/전체 벤치/quiet 성능/최종 admission/커밋·푸시: 이번 요청에서는 `NOT_RUN`.
- 최종 판정: 기존 구현과 focused proof 일부 완료. **실제 미완료 검수·최종 admission·다섯 제품 최신 비교·공통 성능은 아직 닫히지 않았다.**

## 9. 인계에 필요한 경로

| 경로 | 용도/주의 |
| --- | --- |
| `/Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91` | 이번 C3 통합 BASE. 이하 상대 이름은 모두 이 BASE 아래다 |
| `ai-review-current/<repo>/nl-suite-current` | 현재 발행된 7개 suite. 기존 issuance를 임의 덮어쓰지 않음 |
| `c3-review-resume-quota-qcshswey` | 실제 원본 검수 driver, repository별 log/result/terminal, 실패 5개 |
| `c3-shared-source-two-pass-tm8zadsm` | 기존 실제 모델 driver/batch plan. payload·cache·모델 identity 재개 지점 |
| `c3-bat-supplemental-actual-review-nx9v8k8n` | 실패한 외부 supplemental 요청 builder·원본 raw/precommit |
| `c3-supplemental-queue-163-w6lwIs1x` | 실패한 종속 큐. terminal을 성공으로 바꾸지 않음 |
| `c3-lo-supplemental-actual-review-fzs2bigu/issued-merged` | 이미 발행한 lo 보충 41쌍 |
| `c3-all-five-pool-audit-gsZWij9u` | 첫 600응답 canonical audit, 새 375쌍, remaining watcher terminal |
| `c3-native-nl-zero-control-4QVkyBQg` | 실제 control 6개 및 source AND 분석. 벤치 질의에 합산 금지 |
| `c3-common-pairs-short-41cceb67-of8l_3rn` | pair capture controller |
| `c3-fresh-pair-pool-collector-41cceb67-vtvk59k7` | paired result pool collector |
| `sourcegraph-native-content-8l9gxy35` | 해당 B08 native stored-content/index scope 증거 |
| `opengrok-source-posting-reference-full-moyyggn_` | 해당 B08 source/posting 참조 scope 증거 |
| `release-holdout`, `c3-nl-split-preparation` | 12저장소 release와 global split. 입력을 변경하면 새 release/증거로 분리 |
| `/Users/songmin/.codex/worktrees/b08-final-keyword-proof/quanta-index` | C3 기존 clean 실행 소스 `41cceb67c326bd7251f08e89668583f85dd47a9f`. main의 새 구현과 혼동 금지 |
| `/Users/songmin/Documents/code-new/qi-b08-c3-nl-review-20261003-OmZWCc` | 기존 240 NL질의·검수 baseline. read-only |
| `/Users/songmin/Documents/code-new/qi-s30-b08-holdout-20261002/checkouts` | frozen corpus checkout. read-only |
| `/Users/songmin/Documents/code-new/qi-b09-structural-fix-20261004-ji1PLR` | 다른 채팅의 33캡처/11,272응답, frozen source `d1a1b709`. Agent 5 소유 |
| `/Users/songmin/Documents/code-new/qi-b09-session-hardening-20261004-0snzc83u` | Agent 5 후속 3개 결함 감사·proof |
| `/private/tmp/qi-bench-defect-fix-20261004-46iq_iok` | Agent 3 실제 payload preflight·scheduler·111테스트 결과. tmp이므로 소멸 가능 |

기존 진행/결과 티켓: [B08](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md), [B09](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md), [B07](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md).

**다음 작업의 첫 단계:** 이 문서의 timestamp/PID/HEAD를 다시 확인하고, Agent 3의 완료 수정으로 실제 failed review 재개 경로를 연결한다. 기존 queue가 살아 있다는 이유로 검수 진행 중이라고 보고하지 않는다.
