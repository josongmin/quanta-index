# P03 · 식별자 검색 품질과 무정답 회귀

우선순위 **P2**. 사전 평가 없이 기본 tokenizer나 가중치를 바꾸지 않는다.

## 현황

Native Text의 기본 토큰은 문자·숫자·밑줄과 결합 문자의 연속이다. 구 CodeSearch의 순위 계산은 엔진에 합류했다. 공개 regex 경로와 대표 위치 반환은 Q01의 후속 작업이다. `camelCase`·`snake_case` 분할이나 exact definition boost의 품질 평가와 Native·Zoekt 공통 골든 실측은 `NOT_RUN`이다. [토큰 경계](../../../../crates/quanta-index-lq-text-normalizer/src/tokens.rs#L19) · [검색 원장](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#진행-기록) · [검색 품질 ADR](../../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md)

랭킹 리플레이는 이어 읽기가 명시된 페이지에서만 새 커서를 받아들이고, 다음 페이지가 있는 구간에는 실제 파일 반환을 요구한다. 마지막 페이지는 남은 파일을 모두 반환하고 커서 없이 종료해야 순위 평가에 들어간다. 결정적인 Python 회귀에서 무효 continuation·빈 중간 페이지·반복 커서를 거절했다. 랭킹 리플레이와 파일 비교 테스트 **173/173 `VERIFIED`**. [리플레이 검증](../../../../tools/benchmark/retrieval/code_search_rank_study.py#L188) · [회귀](../../../../tools/ci/tests/test_code_search_rank_study.py#L390)

랭킹 CLI는 suite·record·study를 각각 한 번 읽고 검증에 사용한 바이트의 해시를 보고서에 남긴다. 검증 도중 입력 파일을 교체하면 이전 구현은 원본 record에 맞는 study를 거절했다. 교체 회귀를 포함한 랭킹 리플레이·파일 단위 비교 **178/178 `VERIFIED`**. [입력 결속](../../../../tools/benchmark/retrieval/code_search_rank_study.py#L639) · [교체 회귀](../../../../tools/ci/tests/test_code_search_rank_study.py#L225)

식별자 품질 보고서는 같은 manifest 바이트에서 SHA-256과 입력 목록을 얻고, 검증한 suite·census 내용과 채점기가 실제로 쓰는 입력이 같을 때 생성한다. 수정 전 세 교체·불일치 입력이 통과했다. 최신 source lock의 9개 parser pin을 조인 회귀에 반영한 뒤 식별자 보고서·제품 간 집계·조인 회귀 **71/71 `VERIFIED`**. [입력 검증](../../../../tools/benchmark/retrieval/identifier_robustness_report.py#L159) · [불일치 회귀](../../../../tools/ci/tests/test_identifier_robustness_report.py#L1092) · [parser pin 회귀](../../../../tools/ci/tests/test_identifier_robustness_fresh_join.py#L458)

공개 SDK의 `alpha case:yes` 손계산 점수 골든을 `top_k(1)` 6페이지에도 적용한다. 점수 내림차순과 동점 경로 순서, 같은 generation, 남은 건수 6→1과 마지막 continuation 종료는 Nextest `c6fa7d37-b39c-490d-a089-95a955bf4e2a`에서 **1/1 `VERIFIED`**다. 독립 식별자 품질 비교는 `NOT_RUN`이다. [SDK 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/legacy_rank_order.rs#L86) · [실행](/Users/songmin/Library/Caches/quanta-index/native-sparse-rank-overlay-stock.mCPOrE/nextest.log)

## 작업 범위

- 동일 Source·결과 단위에서 식별자 exact·부분문자열, qualified 이름, 경로명, hard negative와 무정답의 독립 평가를 준비한다. 정답 파일·구간과 판단 기준을 변경 전에 고정한다.
- 항목별 Hit@1·MRR@10·Recall과 무정답 오탐률의 실패를 본 뒤, 분할·prefix·오타 허용·exact boost 후보 중 관측된 문제에 해당하는 변경을 선택한다.

## 완료 조건

- [ ] Native와 구 기준본의 파일 단위 결과를 같은 골든으로 비교한다. 질의별 누락·timeout·거절을 보존한다. Zoekt와 공통인 기능은 V02의 동일 범위로 평가한다.
- [ ] 변경 전후에 정확 substring·원문 위치 골든이 유지되고 fixture·점수·분모를 기존 평가 원장에 인계한다. 지표 개선 주장은 독립 평가에서 측정한 범위에 귀속한다.

## 의존·인계

평가 입력을 V02·기존 F‑EVAL과 병렬 준비할 수 있다. 튜닝은 [Q01](Q01-sdk-codesearch.md)·[Q03](Q03-regex-budget.md)의 기준 동작을 확보한 뒤 진행한다. 기초 correctness와 제품 출하 수치는 [V03](V03-signed-release.md)에서 관리한다.
