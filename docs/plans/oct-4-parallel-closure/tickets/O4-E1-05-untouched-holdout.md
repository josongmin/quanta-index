# O4-E1-05 — 독립 relevance와 미사용 holdout 발행

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E1 — 정답·검수·admission과 독립 평가](../epics/E1-labels-admission-and-gold.md) / E1 담당 |
| 우선순위 / 종류 | P1 / `DATA_AND_PROOF` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | 신규 12repo commit/source freeze `VERIFIED`; license 승인·노출/gold/holdout 발행은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

튜닝에 노출되지 않은 repository/query family를 고정하고 새로운 제품 정책을 판정할 독립 gold를 준비한다.

## 배경과 현재 상태

Gin exact1196, 원래 generated 300, B09 public/global12는 이미 진단·튜닝에 노출됐다. frozen corpus는 unseen relevance를 증명하지 않는다. 현 holdout_sampling/corpus_set와 split validator를 재사용한다.

## 2026-10-04 PREPARE 결과

- 기존 development/C3/C5/선택된 Semble roster와 이름·URL이 겹치지 않는 Go/Rust/Python/TypeScript 각 3개 후보를 exact 40-hex remote commit으로 fetch했다. 확인 범위 밖의 과거 사용 이력이나 query/intent 독립성은 증명하지 않는다.
- `VERIFIED`: `uv run --frozen --extra dev python -m tools.benchmark.retrieval.corpus_set --spec /tmp/qi-e1-05-cohort-draft-20261004/candidate-corpus-set-spec.bound-prepare.json --checkouts /Users/songmin/Documents/code-new/qi-oct4-unseen-prepare-k7exyv41/checkouts --out /Users/songmin/Documents/code-new/qi-oct4-unseen-prepare-k7exyv41/candidate-freeze` — exit 0, 12 repositories / 5,684 admitted code files; canonical `corpus-set.json` SHA-256 `eb4c80ff958a77f86a2051febfc34ad375171c2a20004dd37a97b8951a1325cc`.
- canonical 결과 상태는 `candidate_not_admitted_no_gold_no_pair`다. root license bytes/hash를 수집했지만 승인으로 간주하지 않는다. draft 입력과 제한된 overlap 조사 근거는 `/tmp/qi-e1-05-cohort-draft-20261004/`에 있다.
- license 승인 주체·사전 품질 기준/critical-stratum 허용 회귀는 사용자 입력 대기다. source/near-copy split·query exposure·parser coverage·독립 gold/review·admission·untouched qualification은 `NOT_RUN`이다.

## 착수 입력

- repository/commit/license 목록, query authoring rubric, prior tuning/evaluation exposure ledger
- E1-04의 name/file unit contract, 별도 NL·semantic·workflow·no-answer query 의도

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/corpus_set.py](../../../../tools/benchmark/retrieval/corpus_set.py) | freeze_one / freeze_set | 정확한 commit·manifest·license·selection을 새 외부 release에 freeze한다. | OWNED |
| [tools/benchmark/retrieval/holdout_sampling.py](../../../../tools/benchmark/retrieval/holdout_sampling.py) | build / _no_answer / _declarations | 현재 producer를 사용해 family 중복·eligible population·underfill을 명시한다. 실제 contract 결함일 때만 수정한다. | OWNED |
| [tools/benchmark/corpus_binding.py](../../../../tools/benchmark/corpus_binding.py) | validate_split_manifest | repository/source-family/near-duplicate 노출과 cross-split source 일치를 검사한다. | OWNED |
| [tools/benchmark/retrieval/source_oracle.py](../../../../tools/benchmark/retrieval/source_oracle.py) | SourceOracleIndex | mechanical gold의 존재·hash·parser coverage를 검증하고 semantic relevance 판단과 구분한다. | OWNED |
| [tools/ci/tests/test_corpus_binding.py](../../../../tools/ci/tests/test_corpus_binding.py) | split leakage mutants | same repository/family/source 복제·renamed duplicates·stale release negative를 보강한다. | OWNED |
| [tools/benchmark/retrieval/identifier_robustness_suite.py](../../../../tools/benchmark/retrieval/identifier_robustness_suite.py) | propose / propose_typo_operation | 현 four-edit family producer와 collision/admission 경계를 소비한다. source-query label 권위를 새 sampling scorer로 복제하지 않는다. | READ |
| [tools/benchmark/retrieval/gold_oracle.py](../../../../tools/benchmark/retrieval/gold_oracle.py) | mechanical gold / declaration contracts | 이름·prefix/infix/components·OSA1·no-answer의 independent contract와 coverage를 source bytes에서 검증하고 reviewed NL 의미 gold와 구별한다. | READ |

## 실행 단계

1. 기존 실행에서 노출된 source/query/family를 inventory로 남기고 새 split policy를 실행 전에 확정한다.
2. 최신 코드를 보고 단일 repository만 반복하는 대신 독립 repository/source family를 선택하고 license·commit을 freeze한다.
3. NL 의도와 no-answer/ambiguity를 independent AI review provenance로 발행하고 mechanical target을 relevance로 둔갑시키지 않는다.
4. duplicate/exposure·sample underfill과 미지원 source coverage를 complete denominator에 표시한다.
5. 모델/청킹/기본 typo policy의 acceptance와 critical strata를 결과를 보기 전에 고정한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_corpus_binding.py tools/ci/tests/test_source_oracle_suite.py -q`
- Positive: split manifest를 release bytes로 replay하고 query/source/family의 독립성을 확인한다.
- Negative: 기존 global12를 renamed holdout으로 수입, family 복제 1000개 채움, unknown→no-answer, corpus exposure 누락 거절.

## Unique-query와 holdout 노출 게이트

- exact/prefix/infix/components/insert/delete/substitute/transpose/no-answer/NL/workflow를 요구된 lane별로 population·eligible·admitted·quota·underfill 사유/IDs로 발행한다. unique query≥1,000 요구는 동일 source family 반복이나 성능 observations로 채우지 않는다.
- current holdout_sampling의 baseline_v3/scale_diagnostic_v1과 실제 quotas를 사용한다. 두 profile 모두 reviewed NL/workflow qrels가 없으면 underfill이다. scale profile의 내장 ≥1,000 refusal은 SCALE_MINIMUM_LANES의 mechanical lanes에만 적용되고 natural_language_workflow는 제외된다. profile 통과를 모든 요구 lane의 ≥1,000 issuance로 해석하지 않는다. 요청된 NL/workflow quota는 별도 ledger에서 판정하며 미충족 qualification은 NOT_RUN/BLOCKED다.
- policy/model/chunking 후보와 acceptance를 결과를 보기 전에 고정하고 final holdout 접근은 별도 담당/단일 선택 평가로 제한한다. holdout 실패를 보고 튜닝했으면 그 population은 development exposure로 전환하고 새 미사용 holdout을 발행한다.
- 기존 split validator의 URL/revision/family/exact/near-copy policy는 source leakage를 검사한다. 의미상 query/intent 누출과 과거 사용 이력까지 자동 증명하지는 않으므로 exposure ledger를 별도 대조한다.

## 완료 조건

- 새 holdout과 development input은 license/source/family/query별로 분리되고 source-bound gold가 있다.
- AI reviewer/adjudicator 실제 provenance와 ambiguous/excluded/underfilled 집합을 출력한다.

## 중단·거절·재개 조건

- 새 repository 선택은 실제 입력·license 검증 후 결정한다. 본 티켓이 임의 repo names를 확정하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
