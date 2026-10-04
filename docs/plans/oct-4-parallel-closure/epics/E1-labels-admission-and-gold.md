# E1 — 정답·검수·admission과 독립 평가

- 상태: `PLANNED`. 구현·모델 실행·제품 캡처·verification은 이 계획 작성에서 `NOT_RUN`.
- 담당: E1 담당 1명.
- 7개 티켓. [전체 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md).

## 목적

하나의 source/query/rubric 결속 라벨 권위에서 실제 검수와 admission을 발행하고, 파일 적중·정확한 선언 이름·NL relevance를 각 단위로 평가한다.

## 배경과 현재 구현

- 원본 C3는 240질의 중 7저장소 140질의만 발행됐고 5저장소가 실패했다. 남은 작업의 시작점은 원본 실패 log/raw cache이며 repaired preflight는 실제 모델 판단을 대체하지 않는다.
- 최초 6저장소의 미검수 union375, 별도 lo supplement41과 distinct 신규 lo30은 과거 inventory다. 최신 source/query/capture로 exact pair 집합을 다시 계산해야 한다.
- bind_supplemental_review_tasks·ready/failed admission drain·split/runtime pin 검증·declaration metric·numeric bootstrap cache는 이미 구현돼 있다. 이번 범위는 소비 연결, 실제 판단 발행, 독립 gold와 남은 span 경계다.
- 현 declaration recall/MRR는 symbol unit과 indexed definition span 일치를 본다. 정확한 이름 byte span은 SourceOracleIndex.declaration_name_spans의 독립 oracle에서 별도 평가해야 한다.

## 목표 계약과 변경 원칙

- raw reviewer1/reviewer2/adjudicator → canonical qrels → suite/split/admission을 한 pipeline으로 발행한다. distinct model identity는 human independence 증명이 아니므로 실제 AI provenance와 human_provenance_attested:false를 유지한다.
- final-pool 추가 검수는 같은 pipeline의 다음 qrel revision이다. unknown/unresolved는 명시적 판단·제외로 처리하고 0점/no-answer로 합성하지 않는다.
- 이름 witness, symbol/unit 회수, 파일 Hit@k, source/parser coverage의 분모를 분리한다. B08/B09·file/chunk/symbol·default/explicit query 계약을 하나로 합산하지 않는다.
- 독립 holdout은 기존 진단/튜닝 exposure와 repository/source-family/near-duplicate를 분리하고 license·commit·eligible/underfill을 고정한다.

## 해야 할 일

1. 원본 실패별 재개·valid cache replay를 완료한다. 불명확한 batch exit를 quota로 단정하지 않는다.
2. unjudged union을 실제 2회 AI review+별도 adjudication으로 판단하고 rubric/query/source bytes를 보존한다.
3. 현재 suite와 split/admission producer를 연결한다. review 완료와 ready 셀을 실제 native execution consumer에 넘긴다.
4. 정확한 declaration-name metric과 필요 시 Rust name witness producer를 같은 source authority에서 구현한다. 기존 definition span metric은 명칭과 의미를 유지한다.
5. 미사용 holdout·ambiguity/no-answer·same-name relevance를 독립 발행한다. 각 family의 1000+ 목표는 실제 eligible population·underfill로 판단한다.
6. E2 fresh candidates를 받으면 추가 qrel revision을 발행하고 최종 lane별 score/coverage/CI·pool exposure sensitivity를 재생한다.
7. cold bootstrap이 최종 실행의 병목으로 측정된 경우만 독립 scalar/statistics reference와 fixed draws로 bounded vectorization을 한다.

## 티켓 실행 순서

| 티켓 | 작업 | 우선순위 / 종류 | 선행 결과 |
| --- | --- | --- | --- |
| [O4-E1-01](../tickets/O4-E1-01-original-review-resume.md) | 원본 C3 검수 실패 복구와 실제 판단 발행 | P0 / `EXECUTION` | 즉시 조사·fixture 준비 가능 |
| [O4-E1-02](../tickets/O4-E1-02-supplemental-labels.md) | 미검수 합집합 검수와 원본 라벨 병합 | P0 / `EXECUTION` | [O4-E1-01](../tickets/O4-E1-01-original-review-resume.md) |
| [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md) | 최종 suite·split·admission 연결 | P0 / `INTEGRATION` | [O4-E1-02](../tickets/O4-E1-02-supplemental-labels.md), [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md) | 정확한 선언 이름 span과 unit 회수 평가 | P1 / `CODE_AND_PROOF` | 즉시 조사·fixture 준비 가능 |
| [O4-E1-05](../tickets/O4-E1-05-untouched-holdout.md) | 독립 relevance와 미사용 holdout 발행 | P1 / `DATA_AND_PROOF` | 즉시 조사·fixture 준비 가능 |
| [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md) | 최종 합집합 검수·재채점·정책 판정 | P1 / `EXECUTION_AND_REPORT` | [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md), [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md), [O4-E2-04](../tickets/O4-E2-04-fresh-five-product-captures.md), [O4-E1-05](../tickets/O4-E1-05-untouched-holdout.md) |
| [O4-E1-07](../tickets/O4-E1-07-bounded-bootstrap.md) | cold bootstrap의 결정적 bounded 계산 | P2 / `CONDITIONAL_CODE` | [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md) |

- `CONDITIONAL_CODE`는 병목/계약 실패 조건이 실제로 성립한 경우 구현한다. 조건 미성립은 근거가 있는 `NOT_APPLICABLE`로 닫는다.
- `PROOF_FIRST`/`PROOF_THEN_CONDITIONAL_CODE`는 baseline 결과와 독립 expected contract를 먼저 발행한다.
- 선행 결과가 `BLOCKED`/`NOT_RUN`이면 의존 실행은 완료로 표시하지 않는다. 소스 조사·fixture 준비는 계속 가능하다.

## 파일 소유권과 수정 위치

`OWNED`: 이 에픽 담당자가 해당 파일의 변경을 통합한다. `SHARED`: I0가 최종 공유 checkout에 반영하며 이 에픽은 구체적인 변경 proposal와 검증을 제출한다. `READ`: 기존 구현을 소비/검증하며 새 수정의 소유권을 뜻하지 않는다. 정확한 수정 내용·알고리즘·테스트는 각 연결 티켓의 파일 표에 있다.

| 파일 | 현재 진입점 / 확인할 경계 | 소유 모드 | 구체적 작업 |
| --- | --- | --- | --- |
| [benchmarks/retrieval/proof-required-tests.json](../../../../benchmarks/retrieval/proof-required-tests.json) | actual collected test identities | SHARED | [O4-E1-07](../tickets/O4-E1-07-bounded-bootstrap.md) |
| [benchmarks/retrieval/src/record.rs](../../../../benchmarks/retrieval/src/record.rs) | PublishedUnitRegistry / symbol span_accounting assembly | SHARED | [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md) |
| [benchmarks/retrieval/src/symbols.rs](../../../../benchmarks/retrieval/src/symbols.rs) | RawDefinition / extract_corpus_symbols | SHARED | [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md) |
| [docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md) | 현재 terminal inventory | OWNED | [O4-E1-01](../tickets/O4-E1-01-original-review-resume.md) |
| [tools/benchmark/corpus_binding.py](../../../../tools/benchmark/corpus_binding.py) | validate_split_manifest / capture_gold_batch / validate_gold<br>validate_split_manifest | OWNED | [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md), [O4-E1-05](../tickets/O4-E1-05-untouched-holdout.md) |
| [tools/benchmark/retrieval/README.md](../../../../tools/benchmark/retrieval/README.md) | supplemental API 계약 | SHARED | [O4-E1-02](../tickets/O4-E1-02-supplemental-labels.md) |
| [tools/benchmark/retrieval/admission.schema.json](../../../../tools/benchmark/retrieval/admission.schema.json) | required fields | SHARED | [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md) |
| [tools/benchmark/retrieval/corpus_set.py](../../../../tools/benchmark/retrieval/corpus_set.py) | freeze_one / freeze_set | OWNED | [O4-E1-05](../tickets/O4-E1-05-untouched-holdout.md) |
| [tools/benchmark/retrieval/evaluator.py](../../../../tools/benchmark/retrieval/evaluator.py) | validate_suite / _check_split_leakage<br>_declaration_match / declaration_* / indexed_span_diagnostics<br>evaluate / evaluate_complete_scored_file_evidence / repository_cluster_ci<br>mean_ci / _bootstrap_bounds / query_family_cluster_ci / repository_cluster_ci | OWNED | [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md), [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md), [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md), [O4-E1-07](../tickets/O4-E1-07-bounded-bootstrap.md) |
| [tools/benchmark/retrieval/holdout_review.py](../../../../tools/benchmark/retrieval/holdout_review.py) | validate_completed_forms / finalize_file_review_labels<br>capture_review_pool / bind_supplemental_review_tasks / finalize_file_review_labels<br>기존 supplemental/finalization pipeline | OWNED | [O4-E1-01](../tickets/O4-E1-01-original-review-resume.md), [O4-E1-02](../tickets/O4-E1-02-supplemental-labels.md), [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md) |
| [tools/benchmark/retrieval/holdout_sampling.py](../../../../tools/benchmark/retrieval/holdout_sampling.py) | build / _no_answer / _declarations | OWNED | [O4-E1-05](../tickets/O4-E1-05-untouched-holdout.md) |
| [tools/benchmark/retrieval/identifier_robustness_fresh_join.py](../../../../tools/benchmark/retrieval/identifier_robustness_fresh_join.py) | build / common eligible & source admission | OWNED | [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md) |
| [tools/benchmark/retrieval/identifier_robustness_multiproduct_report.py](../../../../tools/benchmark/retrieval/identifier_robustness_multiproduct_report.py) | build / summarize | OWNED | [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md) |
| [tools/benchmark/retrieval/lexical_five_product_oracle.py](../../../../tools/benchmark/retrieval/lexical_five_product_oracle.py) | score_record / evaluate | OWNED | [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md) |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | validate_admission_manifest / verify_admission_bundle / freeze_admission | SHARED | [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md) |
| [tools/benchmark/retrieval/source_oracle.py](../../../../tools/benchmark/retrieval/source_oracle.py) | declaration_census / SourceOracleIndex.declaration_name_spans<br>SourceOracleIndex | OWNED | [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md), [O4-E1-05](../tickets/O4-E1-05-untouched-holdout.md) |
| [tools/ci/tests/test_corpus_binding.py](../../../../tools/ci/tests/test_corpus_binding.py) | split/runtime/source rejection<br>split leakage mutants | OWNED | [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md), [O4-E1-05](../tickets/O4-E1-05-untouched-holdout.md) |
| [tools/ci/tests/test_holdout_review.py](../../../../tools/ci/tests/test_holdout_review.py) | completed review / finalization controls<br>supplemental_request controls | OWNED | [O4-E1-01](../tickets/O4-E1-01-original-review-resume.md), [O4-E1-02](../tickets/O4-E1-02-supplemental-labels.md) |
| [tools/ci/tests/test_identifier_robustness_fresh_join.py](../../../../tools/ci/tests/test_identifier_robustness_fresh_join.py) | label/source/reuse negative | OWNED | [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md) |
| [tools/ci/tests/test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) | bootstrap/cache tests | SHARED | [O4-E1-07](../tickets/O4-E1-07-bounded-bootstrap.md) |
| [tools/ci/tests/test_retrieval_native_span_projection.py](../../../../tools/ci/tests/test_retrieval_native_span_projection.py) | actual native span fixtures | OWNED | [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md) |
| [tools/ci/tests/test_source_oracle_suite.py](../../../../tools/ci/tests/test_source_oracle_suite.py) | source name gold | OWNED | [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md) |

## 병렬 착수와 의존 경계

E1-01, E1-04, E1-05 준비는 시작 가능하다. 같은 evaluator/source_oracle/holdout 파일 변경은 E1 담당자 한 명이 순서대로 합친다. 실제 model jobs·output namespace는 하나의 실행 owner가 관리한다.

원본/보충 검수 → 첫 admission(E1-03) → E2 fresh capture → 마지막 pool 검수(E1-06). final qrel만 바뀌면 요청 binding을 검사해 raw 재채점 가능성을 결정하고, 요청·source·unit이 바뀐 셀만 재실행한다.

## 에픽 완료 조건

- 모든 task/pair의 issued/excluded/failed/blocked 상태가 raw에서 재생되고 미판단 pair가 scored population에 없다.
- 최종 suite·split·admission과 파일/name/symbol/NL lane 분모·점수·CI가 독립 oracle와 일치한다.
- unseen·human·performance qualification은 실제 증거가 있는 범위에만 부여한다.

## 원본과 계약 근거

- [docs/handoff/oct-4/agent-1.md](../../../../docs/handoff/oct-4/agent-1.md)
- [docs/handoff/oct-4/agent-2.md](../../../../docs/handoff/oct-4/agent-2.md)
- [docs/handoff/oct-4/agent-3.md](../../../../docs/handoff/oct-4/agent-3.md)
- [docs/handoff/oct-4/agent-4.md](../../../../docs/handoff/oct-4/agent-4.md)
- [docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md)
- [docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md)
