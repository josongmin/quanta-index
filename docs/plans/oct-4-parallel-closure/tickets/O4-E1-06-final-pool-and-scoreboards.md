# O4-E1-06 — 최종 합집합 검수·재채점·정책 판정

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E1 — 정답·검수·admission과 독립 평가](../epics/E1-labels-admission-and-gold.md) / E1 담당 |
| 우선순위 / 종류 | P1 / `EXECUTION_AND_REPORT` |
| 기준 웨이브 | [W5 — 최종 검수·재채점·정책 판정](../waves/W5-final-scoring-and-policy.md) |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | [O4-E1-03](O4-E1-03-admission-and-split.md), [O4-E2-04](O4-E2-04-fresh-five-product-captures.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

새 5제품 응답에서 생긴 마지막 미검수 union을 닫고 공통 eligible 집합에서 lane별 최종 결과를 재계산한다.

## 배경과 현재 상태

historical 21,815행 join과 fresh Quanta 11,272응답은 같은 시점의 5제품 실행이 아니다. 기존 reports에는 common eligible·operational coverage·repository cluster CI 기능이 있다. 파일 적중 수의 증가만으로 exact-name span 또는 causal quality gain을 설명할 수 없다.

## 착수 입력

- E2-04 raw native records/required-cell outcomes/fresh pooled candidates
- E1-03 admitted suite/pack. E1-04 name metric이 준비된 경우 그 별도 span lane; E1-05 신규 holdout은 이후 policy/unseen qualification의 입력이며 기존 cohort 재채점의 선행 조건은 아니다.

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/holdout_review.py](../../../../tools/benchmark/retrieval/holdout_review.py) | 기존 supplemental/finalization pipeline | 신규 pool만 두 reviewer+adjudicator로 처리해 최종 qrels revision을 발행한다. | OWNED |
| [tools/benchmark/retrieval/evaluator.py](../../../../tools/benchmark/retrieval/evaluator.py) | evaluate / evaluate_complete_scored_file_evidence / repository_cluster_ci | 동일 ID 집합과 raw status로 품질·coverage·unknown/exclusion·uncertainty를 계산한다. | OWNED |
| [tools/benchmark/retrieval/identifier_robustness_fresh_join.py](../../../../tools/benchmark/retrieval/identifier_robustness_fresh_join.py) | build / common eligible & source admission | 실제 새 capture/source-lock/raw bytes에서 join한다. 옛 competitor response와 최신 engine을 혼합하지 않는다. | OWNED |
| [tools/benchmark/retrieval/identifier_robustness_multiproduct_report.py](../../../../tools/benchmark/retrieval/identifier_robustness_multiproduct_report.py) | build / summarize | same-name와 representative gold를 구별하고 ordinal rank·row authority를 검증한다. | OWNED |
| [tools/benchmark/retrieval/lexical_five_product_oracle.py](../../../../tools/benchmark/retrieval/lexical_five_product_oracle.py) | score_record / evaluate | 기존 replay oracle로 요청/unit/status/gold 일치를 검사한다. E2 raw collector 수정과 분리한다. | OWNED |
| [tools/ci/tests/test_identifier_robustness_fresh_join.py](../../../../tools/ci/tests/test_identifier_robustness_fresh_join.py) | label/source/reuse negative | fresh/historical source 혼합과 invalid eligibility·unit·qrel 변경을 거절한다. | OWNED |

## 실행 단계

1. fresh union을 source/query/path ID로 재생하고 E1-02 경로로 추가 판단·최종 qrels/pack/admission을 발행한다.
2. 입력/요청 binding 변경이 native 재실행을 요구하는 셀은 E2-04로 되돌려 새 root에서 실행한다. derived scoring view를 native record로 재명명하지 않는다.
3. exact1196, prefix/infix/components, typo4 default/explicit, no-answer, C3 NL240, Gin20, ARB original/adapted, OSA/CLARC/CSN 별도 scoreboard를 출력한다.
4. common eligible conditional quality와 execution failure를 포함한 운영 coverage를 함께 계산하고 exclusion IDs를 발행한다.
5. pool exposure/leave-one-system-out, repository cluster CI, new19 vs common425 등 cohort 변화를 기록한다.
6. default23 잔여와 NL misses는 policy/quality 진단으로 E4-07에 전달한다. 현재 data의 unseen/human/performance qualification은 실제 gate로 판정한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_identifier_robustness_fresh_join.py tools/ci/tests/test_identifier_robustness_multiproduct_report.py tools/ci/tests/test_lexical_five_product_oracle.py tools/ci/tests/test_retrieval_benchmark.py -q`
- 독립 raw replay의 row/denominator/score/report equality 및 unit/case/span/status/qrel/source mutations를 검증한다.

## 보고서 발행 범위

- repository별 C3/B08/B09 qrel·raw replay는 해당 입력이 준비되면 발행한다. 새 holdout 전체나 다른 저장소 실패 때문에 ready cohort를 막지 않는다.
- E1-04가 미완료인 경우 file/definition 결과는 발행 가능하나 **name recovery 범위는 NOT_RUN**, E1 에픽 전체의 name 평가 작업은 남아 있다.
- E1-05가 미완료면 기존 diagnostic cohort 결과를 unseen/policy-qualified로 승격하지 않는다. E4-07의 정책 변경 acceptance는 새 holdout 이후다.
- 모든 required cells의 inventory가 있다는 사실과 모든 셀 성공·full-cohort qualification을 구분한다. failed/missing/excluded population과 claim 범위를 같이 발행한다.

## 완료 조건

- 미판단 pair가 eligible에 남지 않으며 모든 required cell 결과가 report에서 설명된다.
- 최종 raw recomputation과 각 lane score/coverage/CI가 일치하고 freshness·단위·정책·노출 한계를 명시한다.

## 중단·거절·재개 조건

- final report는 receipt나 사람이 수행하지 않은 review를 합성하지 않는다.
- fresh input/query 변화가 있으면 영향 cells만 새 commitment로 재실행한다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
