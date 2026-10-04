# O4-E4-07 — 기본 typo·NL·semantic 잔여의 정책 RCA

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E4 — 인덱싱·typo 실행 비용·release 성능·scale](../epics/E4-storage-query-and-scale.md) / E4 담당 |
| 우선순위 / 종류 | P2 / `PROOF_THEN_CONDITIONAL_CODE` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | [O4-E1-04](O4-E1-04-precise-name-span.md), [O4-E1-05](O4-E1-05-untouched-holdout.md), [O4-E1-06](O4-E1-06-final-pool-and-scoreboards.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

라벨·unit·source가 검증된 residual을 candidate generation/ranking/policy별로 분해하고 필요할 때만 query policy를 바꾼다.

## 배경과 현재 상태

B09 frozen diagnostic에서 default23는 모두 ordinary literal-first suppression이고 explicit4363/4363이었다. Gin exact4는 symbol control에서 존재를 확인했지만 file@10/span recovery 해결은 아니다. CLARC/CSN NL improvements와2query semantic pilot은 broad semantic file-quality를 qualified하지 않는다.

## 착수 입력

- E1 최종 independent qrels/span/holdout acceptance 및 affected raw task traces
- source/model/generation/candidate lane contribution·rank-unit·budget/cap observations
- 기본 literal-first vs explicitOSA1, scoredKeywordOR vs semantic raw query의 각각 계약

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-lexical/src/searcher/code_search.rs](../../../../crates/quanta-index-lexical/src/searcher/code_search.rs) | literal-first / explicit OSA1 execution mode | default policy 변경은 ambiguity/no-answer regression과 holdout acceptance가 필요할 때만 수행한다. | OWNED |
| [crates/quanta-index-lexical/src/searcher/code_search/ranking.rs](../../../../crates/quanta-index-lexical/src/searcher/code_search/ranking.rs) | source-attested declaration ranking | distance/declaration/occurrence 불변식을 독립 gold로 확인한다. gold 교체로 rank gain을 만들지 않는다. | OWNED |
| [tools/benchmark/retrieval/query_plan.py](../../../../tools/benchmark/retrieval/query_plan.py) | canonical lexical NL planner | 현 scored OR/UCD17 request와 raw semantic query를 보존하고 actual mismatch·route failure만 고친다. | OWNED |
| [benchmarks/retrieval/src/query_plan.rs](../../../../benchmarks/retrieval/src/query_plan.rs) | Rust planner/effective request | Python producer/consumer와 같은 contract를 함께 변경하고 request-only replay를 실제 search proof와 구분한다. | SHARED |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs) | candidate execution/contribution merge | READ: E3의 selection/view 계약을 소비해 failed stage를 추적한다. 입증된 ranking/fusion 변경이 필요하면 I0가 route hunk를 통합한다. | READ |
| [crates/quanta-index-search-plane/src/query_dispatcher/ranking.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/ranking.rs) | query ranking kernel | 구체적인 ranking 결함이 확인되면 현재 owner를 최소 변경한다. global RRF tuning은 isolated holdout ablation 뒤에만 허용한다. | SHARED |
| [tools/ci/tests/test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) | planner/policy oracle | request/ambiguity/no-answer·critical strata 및 common cohort regression을 I0를 통해 등록한다. | SHARED |

## 실행 단계

1. default23/Gin4/NL/zero-overlap tasks에서 source inclusion, candidate lane execution, literal suppression, budget/cap, ranking unit을 각각 대조한다.
2. 같은 qrels/source/model에서 lexical/semantic/hybrid와 file/symbol unit ablation을 실행한다.
3. 기본 정책 유지 또는 revision의 user-visible acceptance와 ambiguity/no-answer tradeoff를 사전 holdout으로 판정한다.
4. 실제 원인에 맞는 producer/planner/ranker만 수정하고 Rust/Python effective request identity를 함께 갱신한다.
5. source/query policy가 바뀌면 I0-02와 E2-04의 affected captures를 새 source epoch에서 재실행한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-lexical --test l3_exact_source --locked`
- `./scripts/cargow test -p quanta-index-retrieval-bench --lib --bins --locked`
- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_benchmark.py -q -k 'query_plan or natural_language or default'`
- Independent same-qrel/source/model ablation; wrong case/unit/source/model/generation, unjudged negatives와 literal regression refusal.

## 완료 조건

- 각 residual은 confirmed defect/accepted policy/label ambiguity/unsupported unit/qualification gap으로 근거 있게 판정된다.
- policy 변경은 independent critical strata와 unused holdout에서 수용 조건을 만족한다. 변경 근거가 없으면 accepted behavior로 종료한다.

## 중단·거절·재개 조건

- explicit success를 default success로 치환하지 않는다. semantic pilot2개로 모델/청킹/globalfusion 교체를 정당화하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
