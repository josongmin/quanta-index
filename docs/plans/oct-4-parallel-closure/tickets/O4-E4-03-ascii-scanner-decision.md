# O4-E4-03 — ASCII scanner 전체 호출 효과 판정

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E4 — 인덱싱·typo 실행 비용·release 성능·scale](../epics/E4-storage-query-and-scale.md) / E4 담당 |
| 우선순위 / 종류 | P1 / `EXECUTION_THEN_CONDITIONAL_CODE` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

mixed A/B 결과에서 deletion lane 악화의 원인을 분해해 scanner를 유지·수정·철회할 근거를 만든다.

## 배경과 현재 상태

typo_text_is_ascii는16384B 단위 cancellation prepass 후 byte scanner 또는 Unicode tokenizer를 선택한다. 기존 20captures/17850 output comparisons는 동등성을 보였으나 deletion completed-call 평균+8.75%, host contended, arm당2roots였다. substage scan 감소는 whole-call 개선을 증명하지 않는다.

## 착수 입력

- candidate/baseline scanner만 다른 exact source/binary tuple
- Gin exact1196 및 four typo lanes1192/1178/1192/1192, independent tokenizer+full-DP OSA oracle
- 같은 observation clocks, allowed host/fresh-root schedule와 predeclared acceptance

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-lexical/src/searcher/code_search.rs](../../../../crates/quanta-index-lexical/src/searcher/code_search.rs) | typo_text_is_ascii / typo_witness / scan_typo_token_spans | prepass·token scan·cache·budget checkpoints의 whole-call work를 비교한다. 해로운 이중 순회가 재현되면 단일 scanner를 수정하거나 optimization을 철회한다. | OWNED |
| [crates/quanta-index-lexical/tests/l3_exact_source.rs](../../../../crates/quanta-index-lexical/tests/l3_exact_source.rs) | actual file/case/typo ranking | Unicode boundary·byte witness·default/explicit result/order/cursor를 보존하는 fixture를 추가한다. | OWNED |
| [tools/benchmark/retrieval/query_timing_overhead.py](../../../../tools/benchmark/retrieval/query_timing_overhead.py) | existing A/B observations | 동일 non-clock rows·work counters를 보존한 A/B를 실행한다. | OWNED |
| [tools/ci/tests/test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) | scanner phase/read replay | shared test hunk는 I0가 반영한다. 시간 fields 외 의미 있는 counters를 비교에서 삭제하지 않는다. | SHARED |

## 실행 단계

1. ascii prepass와 scan의 반복 bytes/cache misses/cancel checks를 actual selector source로 지도화한다.
2. fixed corpus에서 independent tokenizer/full OSA oracle를 먼저 통과시킨다.
3. 같은 source 차이·matching release binary·host admission으로 paired randomized roots를 실행한다.
4. deletion whole-call과 exact/다른 edits를 재대조해 유지/수정/철회한다.
5. source가 바뀌면 narrow oracle와 affected lanes를 다시 실행하고 E4-06으로 효과/uncertainty를 전달한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-lexical --lib --locked typo`
- `./scripts/cargow test -p quanta-index-lexical --test l3_exact_source --locked`
- Positive: 모든 byte span/source/case/order/status/cursor/work counters가 동일.
- Negative: mixed Unicode join·short names·token cap·cancellation·cache stale identity가 틀리면 reject; substage만 개선된 whole-call 악화는 acceptance 실패.

## 완료 조건

- scanner 변경에 대한 current-source functional proof와 whole-call decision이 있다.
- qualification 표본·host 조건이 없으면 효과 미확정과 필요한 반복 실행을 명시한다.

## 중단·거절·재개 조건

- 과거+8.75%를 causal regression으로 확정하거나 compile pass로 scanner acceptance를 처리하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
