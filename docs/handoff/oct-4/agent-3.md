# Agent 3 — lexical benchmark / performance RCA / benchmark execution repair

작성: 2026-10-04 15:18 KST 이후. 저장소:
`/Users/songmin/Documents/code-new/quanta-index`.

## 1. 현재 맡은 작업과 판정

이 세션의 큰 범위는 lexical 검색 벤치의 신뢰성, Quanta 검색·인덱싱 비용
RCA, 기존 경로를 재사용한 계측·검증 보강이다. 가장 최근 요청은 **벤치
실행 결함부터 수정**하는 것이었고, 그 뒤 현재 상태를 이 문서에 인계한다.

- **완료:** supplemental review 요청의 answerability 임계값 결속, 실제 요청
  생성까지 하는 preflight, 준비된 다른 저장소를 막지 않는 admission 대기열.
- **검증 완료:** 관련 테스트 111개, 실제 3개 저장소 요청 preflight, 실제
  변조 거절 4종, 실제 admission 상태 스캔. 모델·제품 호출은 0회.
- **아직 남음:** 새 supplemental 검수 실행과 unresolved adjudication,
  새로운 qrels/admission 발행, 최종 5제품 검색·재채점.
- **성능 작업:** release SDK 증거와 scanner 대조 캡처는 완료했지만, ASCII
  최적화 효과는 미확정이다. 새 release 규모 실행과 정식 성능 비교도 남는다.
- **출판:** 이 세션의 수정은 shared main 작업트리에 존재한다. 이번 벤치
  결함 수정에 대해 이 에이전트가 commit/push를 실행한 것은 아니다.

코드 구현, focused test, diagnostic capture, qualified benchmark, 제품 배포는
서로 다른 완료 단계다. 어느 하나를 다른 단계의 완료로 취급하지 않는다.

## 2. 최신 소스와 공유 변경 경계

인계 확인 시 main HEAD:
`2062fed3ff86287a0914ece99dc2fe0af829c29b`.
벤치 결함 수정 시작·외부 driver snapshot 기준은 `8ee2f1ea`이다. 그 사이의
main 이동은 다른 세션의 문서 커밋이었다. 다음 작업 전에 HEAD와 변경 파일을
다시 확인해야 한다.

공유 저장소에는 다수의 staged 변경이 있다. staged라고 해서 이 에이전트의
소유이거나 검증된 변경은 아니다. 특히 다음 변경은 다른 소유자와 겹친다.

- CI/pre-commit, Cargo/uv dependency 및 lockfile, tree-sitter JavaScript vendor.
- corpus/oracle/parser, robustness fresh join, query-plan/contract,
  Unicode 17 vendor 관련 변경.
- B08/B09 티켓과 다른 검수·제품 캡처 작업.
- `benchmarks/retrieval/proof-required-tests.json`은 여러 작업이 함께 갱신한다.

따라서 `git add -A`, 전체 dirty tree reset/복사, 다른 worktree 덮어쓰기,
일괄 commit/push를 하지 않는다. 파일별 diff와 테스트 등록부를 대조한다.
기존 B08 티켓은 다른 소유자가 계속 업데이트하므로 이 세션은 상위 INDEX에
수정 내용을 기록했고 B08 본문을 직접 다시 작성하지 않았다.

현재 요청은 handoff 저장이다. 새로운 모델 검수, 제품 검색, 대규모 build나
벤치를 이 문서 작성 과정에서 시작하지 않았다.

## 3. 보존해야 할 계약과 원본

- 기본 gin exact 회귀는 **1,196개**다. 최초 300개만 실행하고 전체 결과처럼
  보고하지 않는다. 코퍼스는 gin 실제 Go 코드 99파일이며, query 수 확대와
  corpus 파일 수 확대를 구분한다.
- 기본 검색, prefix, infix, components, 각 typo edit, no-answer, NL/workflow는
  별도 계약·분모·결과표다. 지원하지 않는 문법과 실제 실행 실패를 구분한다.
- Quanta chunk 10개와 distinct file 10개를 혼동하지 않는다. `capped`는
  검색 실행 실패가 아니며, completeness와 prefix 채점 계약에 따라 판단한다.
- typo 파일 Hit@10과 정확한 declaration span 복구는 별도 지표다.
- `substring_file`의 경로순·상수 점수를 relevance ranking으로 해석하지 않는다.
- 자동 모델 검수는 사람 검수로 표시하지 않는다. 현재 외부 수정 검증 역시
  `qualified: false`, `human_provenance_attested: false`다.
- 원본 Gin 300/1,196 캡처, `/private/tmp/g3`, 기존 B08 실패 로그·라벨·응답은
  덮어쓰지 않는다. 재시도는 새 외부 output root와 새로운 commitment를 쓴다.
- 새로운 qrels가 나오면 새 suite/pack/admission 및 새 capture를 발행한다.
  이전 runner record를 새로운 정답에 rebind하지 않는다.
- 성능은 동일한 completed-response boundary로 비교한다. Quanta SDK 시간과
  Semble 내부 BM25 호출 시간을 같은 경계의 수치로 비교하지 않는다.

원본 300 RCA 소스는 `c64f6af5d5e2817e346336ceee0e57f5fb25d74f`이다.
현재 main이나 이후 release 캡처 결과와 섞지 않는다. Gin corpus commit은
`d3ffc9985281dcf4d3bef604cce4e662b1a327a6`, 원본 99파일 universe SHA-256은
`d4e1ea025c067f344af640568b5bfd0835ed09c2bfbcf9668b8e9782bc68bd67`이다.

## 4. 최근 완료한 벤치 결함 수정

### 4.1 임계값 누락 — 실제 실행 실패

원본 실패:
`/Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91/c3-bat-supplemental-actual-review-nx9v8k8n/actual-review.stderr`.

외부 `review.py`가 supplemental pool의 task를 직접 `request_payload()`에
전달했는데, 그 task에는 `answerability_min_grade`가 없었다. 기존
`run_batches.py`의 실제 request builder는 이 필드를 요구해서 모델 호출 전에
`KeyError`가 났다. cli/lo의 기존 preflight도 actual request builder 전에
종료하므로 같은 결함을 놓쳤다. 기존 canonical 검수·채점기가 임계값을
보존하지 못한 문제와는 구분해야 한다.

수정 소유 파일:

- `tools/benchmark/retrieval/holdout_review.py`:
  `bind_supplemental_review_tasks(checkout, suite_bytes, tasks)` 추가.
- `tools/ci/tests/test_holdout_review.py`: 임계값/원문/질의/결정 결속 제어 추가.
- `tools/benchmark/retrieval/README.md`: 새 준비 API와 preflight 계약 설명.
- `docs/plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md`: 실제 상태 반영.

새 함수는 원본 suite 전체를 기존 evaluator로 검증하고, query와 임계값을
결속한다. 누락된 요청 임계값은 suite의 값으로 명시한다. suite에서 생략한
역사적 계약은 1을 사용한다. 명시적 요청 값이 suite와 다르면 거절한다.
중복 task/pair, 이미 판단된 pair, source hash/text 변경, grade/unresolved
주입도 거절한다. 입력을 바꾸거나 새로운 grade/answerability를 만들지 않는다.
supplemental 후보 부분집합으로 전체 task의 no-answer를 판정할 수 없다.

### 4.2 대기열 막힘 — 저장소별 readiness 처리

기존 외부 `capture_pairs.py`는 저장소 순서대로 한 admission을 기다렸다.
typeorm admission이 없는 동안, 뒤의 nushell은 준비돼 있어도 대기했다.
실패한 원본 review와 살아 있는 전체 admission 프로세스를 구분하지 못한
것이 원인이었다.

수정 소유 파일:

- `tools/benchmark/retrieval/execution_batch.py`:
  `iter_repository_admissions(...)` 추가.
- 같은 기존 `test_holdout_review.py`: 대기열 테스트 추가.

전체 pending 집합에서 ready/failed cell을 먼저 drain하고 나서 기다린다.
per-repository failure, malformed result, upstream 종료 후 누락, 최종 publish
race를 구분한다. repository identity를 검사한다. 실제 admission source/input
binding과 product proof 검증은 consumer가 계속 수행한다. 실패를 successful
empty cell로 바꾸지 않는다. wait/deadline 정책은 caller가 소유한다.

### 4.3 실제 연결·검증 산출물

최종 새 output root:
`/private/tmp/qi-bench-defect-fix-20261004-46iq_iok`.

| 파일/디렉터리 | 의미 |
| --- | --- |
| `MANIFEST.json` | `8ee2f1ea` 기반 driver source, overlay hash, 원본 입력과 실행 argv |
| `source/` | 당시 HEAD의 tools snapshot에 소유 helper 수정만 반영한 driver source |
| `bat/review.py`, `cli/review.py`, `lo/review.py` | 기존 외부 어댑터의 새 복사본. canonical 준비 함수를 사용 |
| 각 `precommit.json` | script/source/input byte binding |
| 각 `preflight.json` | 실제 reviewer request, model input, response schema 생성 결과 |
| `capture-queue/capture_pairs.py` | 기존 controller의 새 복사본. readiness generator 사용 |
| `capture-queue/preflight.json` | 실제 admission/input-byte 스캔 결과 |
| `control-*/stderr` | 임계값·질의·grade·source text 변조 거절 로그 |
| `RESULT.json` | 검증 결과, 명령, 범위와 미실행 항목 요약 |

수치:

| 저장소 | task | 추가 파일 쌍 | 생성한 reviewer 요청 | 임계값 |
| --- | ---: | ---: | ---: | ---: |
| bat | 18 | 51 | 2 | 2 |
| cli | 20 | 133 | 10 | 2 |
| lo | 15 | 30 | 2 | 2 |
| 합계 | 53 | 214 | 14 | — |

임계값·질의·grade·source text를 각각 변조한 외부 control 4종은 모두 exit 1로
거절됐다. 모델 호출 전에 거절한 것이므로 `VERIFIED_REJECTION`이다.
실제 admission 스캔은 ready 7개(bat/cli/lo/mocha/uvicorn/zustand/nushell),
failed 2개(typeorm/tailscale), pending 3개(django/sqlalchemy/zellij)를 관측했다.
이것은 **당시 관측값**이며 다음 작업에서 반드시 다시 조회한다.

중요한 연결 경계:

- 기존 실행 중인 controller/reviewer 프로세스를 교체·재시작하지 않았다.
- 새 reviewer는 `--preflight`와 `--run`을 명시하도록 만들었다. 이번에는
  `--preflight`만 실행했다. `--run`은 실제 모델 호출을 한다.
- 새 capture-controller preflight는 기존 admission을 스캔했다. 따라서 그
  baseline 그대로의 실행은 새 supplemental 라벨 발행 후 최종 비교를
  대신하지 못한다. 새 admission namespace에 연결해야 한다.
- 새 review namespace의 결과를 cached replay → canonical finalizer → 새 suite
  → matching admission으로 잇는 실행은 아직 완료하지 않았다.
- 전부 diagnostic이다. source snapshot/요청 성공을 quality qualification으로
  승격하지 않는다. `/private/tmp` 산출물은 이후 정리될 수 있으므로 필요한
  보존은 소유자의 기존 evidence 관리 절차를 따른다.

## 5. 최근 실행한 명령과 판정

```sh
.venv/bin/python -m pytest -q \
  tools/ci/tests/test_holdout_review.py \
  tools/ci/tests/test_retrieval_benchmark.py \
  -k 'holdout_review or execution_batch'
```

**VERIFIED:** 111 passed / 571 deselected / 22.83초. 새 제어 31개를 포함한다.
전체 repository test나 모든 benchmark proof test 실행이라는 뜻은 아니다.

```sh
.venv/bin/ruff check tools/benchmark/retrieval/holdout_review.py \
  tools/benchmark/retrieval/execution_batch.py tools/ci/tests/test_holdout_review.py
.venv/bin/ruff format --check tools/benchmark/retrieval/holdout_review.py \
  tools/benchmark/retrieval/execution_batch.py tools/ci/tests/test_holdout_review.py
```

**VERIFIED:** lint/format 통과. 변경 소유 파일의 `git diff HEAD --check`도 통과.
기존 `proof_inventory.collect_pytest()`와 `proof-required-tests.json`의 Python
identity 집합이 일치함을 확인했다. 당시 **772개 수집**이며 772개 실행이 아니다.
등록부는 공유 파일이므로 이 세션의 test뿐 아니라 다른 소유자의 항목도 있다.

외부 actual adapter 명령:

```sh
.venv/bin/python /private/tmp/qi-bench-defect-fix-20261004-46iq_iok/bat/review.py --preflight
.venv/bin/python /private/tmp/qi-bench-defect-fix-20261004-46iq_iok/cli/review.py --preflight
.venv/bin/python /private/tmp/qi-bench-defect-fix-20261004-46iq_iok/lo/review.py --preflight
.venv/bin/python /private/tmp/qi-bench-defect-fix-20261004-46iq_iok/capture-queue/capture_pairs.py --preflight
```

**VERIFIED:** 모두 exit 0. 각 precommit의 원본 script/source hash를 실행 후
다시 확인했다. **NOT_RUN:** 실제 새 모델 검수, 새 qrels/admission 발행,
제품 검색 및 최종 5제품 재채점.

## 6. 이 세션의 앞선 성능·RCA 작업

아래는 최근 벤치 수정과 별도의 source/receipt 범위다. 재실행했다고 섞지 않는다.

### 현재 소스에 존재하는 구현

- timed response마다 timer 종료 후 독립 golden 검증. invalid 응답은 latency
  aggregate에 넣지 않는다. completed-response canonical timing과 raw binding.
- SDK `execute_observed`, request-local IPC encode/connect/write/read/decode
  관측, request-event sidecar와 server ring의 ID join.
- typo shortlist/token scan/cache/materialization 계측. ASCII byte scanner,
  취소 점검 prepass, Unicode fallback의 원문 byte-index 유지.
- ingest preparation preflight/coverage write/file source write child clocks.
  diagnostic schema 9 / protocol lock 7 / phase schema 4. 역사적 8 replay 유지.
- direct capture의 두 번째 ingest validation에서 빠진
  `detailed_file_authority=True` 수정. positive/negative focused controls 유지.
- scale phase CPU/RSS 샘플과 누락/gap/coverage 거절. RSS는 경계 probe를 포함한
  sampled maximum이며, process CPU는 sampler/parent probe를 포함한다.
  macOS `ps` child CPU와 physical I/O는 해당 지표가 아니다.
- scale history 2 generations, open-loop 8 generations, 기본 16 MiB 유지.
  명시적 256 MiB profile 지원을 기본 제한 통과로 바꾸지 않는다.

관련 소유 경계:

- 검색: `crates/quanta-index-lexical/src/searcher/code_search.rs`.
- 인덱싱: `adapter_ingest.rs`, `adapter_open.rs`, `file_authority.rs`,
  `crates/quanta-index-contract/src/ipc/ingest_observation.rs`.
- SDK/IPC: `crates/quanta-index-sdk/src/{lexical,client,transport}.rs`,
  `crates/quanta-index-ipc/src/server.rs`.
- runner: `benchmarks/retrieval/src/{diagnostics,sdk,request_events,main}.rs`.
- benchmark: `tools/benchmark/retrieval/{run,query_timing_overhead}.py`.
- scale/load: `crates/quanta-index-searchd-harness/src/{scale,open_loop,harness}.rs`
  및 기존 matrix binaries.

### 고정 소스와 증거 위치

| 증거 | 소스/경로 | 확인 범위 |
| --- | --- | --- |
| Candidate engine | `7ff8251e2340063470bd6c58279528df4428dee8`, `/private/tmp/qi-perf-final-source-20261004-2bkgywrr/quanta-index` | 당시 전체 tracked overlay를 동결. 외부 소유 변경 전체에 대한 리뷰는 아님 |
| Fixed diagnostic driver | `33f8f16d5c712e9ac90a735645cb180a4fee5de9`, `/private/tmp/qi-diag9-direct-driver-20261004-pc025yok/source` | engine snapshot과 Python driver/test/registry 3파일만 차이 |
| ASCII baseline | `1818705897f39700063b1c9659e4dac5e223d6e5`, `/private/tmp/qi-ascii-ab-baseline-20261004-JJvp6N/source` | 같은 계측에서 scanner branch만 비교 |
| Candidate release SDK | `/private/tmp/qi-perf-release-20261004-7ff8251e-r4u4jix4/sdk` | 25/25; independent portable proof verify 통과 |
| Baseline release SDK | `/private/tmp/qi-perf-ascii-base-20261004-18187058-3pnkvuuh/sdk` | 25/25; independent portable proof verify 통과 |
| Gin child clocks | `/private/tmp/g9-7m7inh5r/capture` | 99파일/2질의, schema 9 direct capture 성공 |
| Gin request-local join | `/private/tmp/i9-lhyxoxdp` | 2질의, request join/clock bounds/zero dropped events |
| ASCII 20-cell A/B | `/private/tmp/a9-21c9z44a` | 5 lanes × ABBA, arm/lane별 2 fresh roots |
| 독립 A/B 재검증 | `/private/tmp/a9-21c9z44a/audit-current/REPORT.json` | 20개 contract 통과, 17,850개 비교 행 동일 |
| 기존 canonical fuzz | `/private/tmp/qi-index-fuzz-20261004-pkv5cl21` | 4 targets 각 60초 성공, 기존 seed byte 보존 |

release proof 명령은 각 고정 checkout에서
`just retrieval-sdk-proof-fresh <new-root>/sdk`, 이어서
`python tools/benchmark/retrieval/portable_proof.py verify --receipt <root>/sdk/execution-context.json`
이다. 기존 성공 root로 재실행하지 않는다. 현재 main 전체에 대한 proof로
표시하지 않는다.

당시 focused rails: Python benchmark/timing 597개, debug SDK/join 3개,
ASCII correctness 6개, ingest contract 8개, tiny lifecycle 2개, scale 29개,
open-loop 20개 통과. 이 숫자는 각각 다른 selector/source 시점이다.
6,194-document test는 중단됐고 통과가 아니다.

### 성능에 대해 확인한 범위

20개 native capture는 모두 exit 0이고 총 23,800 scored rows를 담는다.
lane의 첫 capture를 나머지 3개와 비교한 17,850/17,850개 normalized hash와
raw non-clock rows가 동일했다. score/count/window/cursor/work counter를
시간 필드 제거와 함께 임의로 버린 비교가 아니다.

| lane | arm별 평균 completed-call 합계 변화(candidate vs baseline) |
| --- | ---: |
| exact 1,196 | -1.58% |
| insertion 1,192 | -1.69% |
| deletion 1,178 | +8.75% |
| substitution 1,192 | -2.70% |
| transposition 1,192 | -6.11% |

token-scan 합계는 typo 4 lanes에서 감소했지만 deletion의 전체 호출 시간은
증가했다. arm당 2 roots이고 host가 혼잡했으며 frequency가 unavailable이었다.
**효과 미확정**이고 speedup 또는 causal regression으로 단정하지 않는다.
subprocess wall 합계 280.835초는 query time 합계가 아니고 compile을 포함하지 않는다.

Gin 2질의 child-clock run의 단일 관측: preparation 1,020.992ms 중 coverage
write 975.685ms, file authority 1,386.603ms 중 source write 1,370.929ms,
text authority 398.059ms 중 collection 6.179ms/shard build 331.405ms/publication
58.631ms. SDK publish 3,581.142ms는 자식 lexical work를 포함하므로 합산하지 않는다.
저장 단계가 크다는 근거는 있지만 **fsync 원인 확정이나 batching 근거는 아니다**.

request join run에서 SDK execute 3.855/3.384ms, client read 합계
3.620/3.259ms를 관측했다. server event는 frame decode 후에 시작한다.
SDK와 server 시간을 빼서 IPC latency로 표시하면 안 된다.

## 7. 남은 작업 — 우선순위, 소유 경계, 완료 조건

| 순서 | 실제 작업 | 소유/재사용 | 완료 조건 |
| --- | --- | --- | --- |
| P0-1 | 새 namespace에서 bat/cli/lo supplemental actual review 및 cached raw replay 연결 | 위 repaired reviewer, 기존 batch call/cache validator | 두 independent passes와 adjudicator, 모든 추가 pair 판단, unresolved 거절, 원본 valid 판단 보존. AI provenance 유지 |
| P0-2 | typeorm/tailscale unresolved 실패와 나머지 original review 상태 재조회·처리 | B08 검수 소유자, 기존 raw source/valid cached batches | unresolved를 억지 grade로 바꾸지 않고 canonical validator로 완료 검증. 당시 pending/live 상태를 사실로 재사용하지 않음 |
| P0-3 | 새로운 merged qrels/suite/pack/admission 발행 | 기존 `holdout_review.finalize_file_review_labels`, suite evaluator, annotation/adjudication/admission owners | query/threshold/source/grade/identity 결속, threshold 누락·old record rebind 거절. 새 source-bound commitment |
| P0-4 | 새 admission에 repaired capture controller 연결하고 5제품 최종 scoring | 기존 Quanta/Semble pair 및 Sourcegraph/cs/OpenGrok native collectors | 모든 선택 task의 실행·실패·coverage 포함. incomplete pool은 grade 0으로 치환하지 않음. cohort와 query type별 독립 집계 |
| P1-1 | deletion lane의 전체 호출 증가 재대조 후 ASCII patch 판단 | `code_search.rs`, 기존 A/B collector | 동일 소스 차이·binary·관측 경계로 반복. 성능 판정은 admitted host/사전 decision rule. 해로운 prepass면 수정·철회 |
| P1-2 | coverage/source 저장 비용을 full/delta/delete/no-op/reopen에서 profile | 기존 ingest/file authority 및 stage children | 자식 clock bounds, fresh rebuild와 결과 동일. 저장 방식 변경 시 per-write sync/rename/parent-directory crash-cut 증거 |
| P2-1 | 실제 필요할 때 server prevalidation/ingress 계측 보강 | 기존 SDK/IPC request-local path | 정확한 request/connection join과 zero drops. generation pin, credentials, deadline/cancellation/reconnect/partial response 보존 |
| P2-2 | 새 matching release scale/load 실행 | 기존 scale/open-loop matrix | 실제 256→4,096→32,768; full/delta/delete/reopen 및 CPU/RSS. refusal는 capacity 성공으로 표시하지 않음 |
| P2-3 | 정식 성능·다중 저장소 비교 | B07/B08 기존 collectors 및 admitted host | 동일 boundary/unit, 5 fresh roots와 route당 1,000 warm observations, randomized paired blocks/continuous host checks/uncertainty |
| Integration | 소유 변경 통합·최종 publication | shared schema/proof registry 단일 통합 소유자 | 현재 HEAD/overlay 재고정, affected tests/required inventories 일치. 전체 main/release 결과와 focused proof 구분 |

병렬로 진행할 경우 검수·qrels 발행은 같은 저장소에서 선후 의존성을 유지한다.
검색/index/scale 소스 소유자는 분리할 수 있으나 schema/reader/registry는 한
통합 담당자가 수정한다. heavy build 및 성능 실행은 같은 host에서 동시에 하지 않는다.

불필요한 재구현 금지:

- preview-after-page, trigram shortlist, OSA cache/witness, Unicode fallback은 이미 있다.
- touched-shard reuse, coverage inheritance/cache, digest skip도 이미 있다.
- 별도 parallel harness나 두 번째 scorer/admission 체계를 추가하지 않는다.
- sort 비용이 작다는 현재 자료만으로 top-k rewrite를 하지 않는다.
- connection reuse나 durability batching을 원인 계측 전에 선행하지 않는다.

## 8. 규모/성능/검수에 남은 증거 부족

- 최신 release scale harness 실행은 `NOT_RUN`. SDK proof는 scale binary를
  build/execute하지 않는다. 별도 matching release harness를 만들어야 한다.
- 역사적 medium 256 diagnostic은 통과했지만 새 코드의 검증은 아니다.
  old large는 timeout 또는 history limit, XL은 posting cap refusal가 있었다.
- 명시적 `--client-timeout-ms 300000 --history-max-bytes 268435456` profile은
  default가 아니며 원래 limit 통과로 주장하지 않는다. scale history 2와
  open-loop history 8을 서로 바꾸지 않는다.
- same-process reopen과 실제 OS-process restart는 다르다. 후자는 미검증이다.
- sampled RSS maximum은 true peak가 아니고 disk directory-size delta는
  physical write I/O가 아니다.
- 파일별 durable write가 file sync/rename/parent-directory sync를 수행하는
  코드 경로는 확인했다. 실제 syscall 분해 측정은 `NOT_RUN`이며 비root
  `fs_usage` 시도는 권한 때문에 실패했다. 임의 sudo 실행을 하지 않았다.
- 지속 host 관측, frequency/thermal/load admission을 건너뛰지 않는다.
  현재 호스트의 frequency unavailable을 stable로 대체하지 않는다.
- 5제품 외부 indexed-file universe와 subjective gold의 검수 상태가 각
  contract에서 필요한 수준으로 충족돼야 한다. 미충족이면 diagnostic만 가능하다.
- 모든 질의 유형을 1,000개 이상으로 늘리는 이전 요구는 중복 family 복제나
  무조건적 sample 채움으로 완료 처리하지 않는다. 기존 lane issuance/underfill
  ledger를 재확인하고 충분한 실제 eligible population으로 새 release를 만든다.

## 9. 다음 담당자가 처음 할 일

1. `git rev-parse HEAD`, 소유 파일 `git diff HEAD`, shared dirty ownership 확인.
   B08 최신 owner ticket과 실제 external terminals를 같이 읽어 현재 상태 갱신.
2. 이 세션의 main helper와 frozen external driver helper bytes를 비교.
   더 새 변경이 있으면 기존 MANIFEST를 덮지 말고 새 output root에 재결속.
3. 위 111-test selector를 변경 범위에 맞춰 실행. registry는 현재 수집과 비교하고
   다른 소유자의 test를 제거하지 않음. 개수만 같다고 inventory 통과가 아님.
4. 기존 live reviewer/controller가 무엇을 실행 중인지 확인하고 중복 모델 호출,
   같은 service index 교체, 동일 output 경합을 피함. 새 대기열과 old live
   대기열을 둘 다 돌려서 repaired workflow라고 주장하지 않음.
5. 새 라벨 issuance/admission부터 완료한 뒤 final 5제품 capture/scoring으로 진행.
   실제 호출 전 원본/새 root·source·input·query contract를 명시함.
6. 성능 작업은 위 P1 이후, 작은 결정적 owner test와 source confidence를 먼저
   확보하고 실제 필요한 build/run만 실행. 도구는 `just`/`./scripts/cargow` 사용.

## 10. 관련 문서

- [벤치 티켓 현황](../../plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md)
- [B07 성능·인덱싱](../../plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md)
- [B08 검수·holdout 소유 티켓](../../plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md)
- [B09 외부 견고성](../../plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md)
- [규모 검증](../../plans/jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md)
- [tail/load 검증](../../plans/jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md)
- [canonical benchmark API 설명](../../../tools/benchmark/retrieval/README.md)

**최종 인계 판정:** bench request preparation/queue readiness는 focused 및 actual
preflight로 `VERIFIED`. 새 actual review/issuance/최종 5제품 비교와 현재 integrated
main 전체 검증·새 scale·정식 성능 판정은 이 인계 범위에서 `NOT_RUN`이다.
