# O4-E1-04 — 정확한 선언 이름 span과 unit 회수 평가

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E1 — 정답·검수·admission과 독립 평가](../epics/E1-labels-admission-and-gold.md) / E1 담당 |
| 우선순위 / 종류 | P1 / `CODE_AND_PROOF` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

같은 파일 적중과 정확한 declaration-name/ID 회수를 별도 관측으로 평가하고 반환 context 확대가 성공을 만들지 못하게 한다.

## 배경과 현재 상태

evaluator에는 indexed_span_diagnostics와 declaration_recall_at_k/declaration_mrr_at_k가 이미 있다. 현재 _declaration_match는 symbol unit의 정확한 indexed definition span과 judgment를 비교한다. source_oracle.declaration_name_spans는 별도의 name bytes를 제공하고 Rust RawDefinition은 definition bytes를 보유한다. 이 두 범위를 같다고 가정할 수 없다.

## 착수 입력

- 고정 same-line two declarations, same-name receiver, use-only, Unicode/case fixtures
- source file hashes, declaration census name/definition spans, PublishedUnitRegistry 및 actual symbol capture

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/source_oracle.py](../../../../tools/benchmark/retrieval/source_oracle.py) | declaration_census / SourceOracleIndex.declaration_name_spans | gold name bytes와 containing definition identity를 독립 source에서 열거해 case policy별로 검사한다. | OWNED |
| [tools/benchmark/retrieval/evaluator.py](../../../../tools/benchmark/retrieval/evaluator.py) | _declaration_match / declaration_* / indexed_span_diagnostics | 기존 definition metric을 보존하고 name-span authority가 있을 때만 별도 recovery 값을 낸다. file/context rows는 unsupported로 명시한다. | OWNED |
| [benchmarks/retrieval/src/symbols.rs](../../../../benchmarks/retrieval/src/symbols.rs) | RawDefinition / extract_corpus_symbols | 필요한 name witness를 parser에서 수집해 definition/SymbolId와 결속한다. symbol wire 확대 전에 runner-local registry로 충족 가능한지 확인한다. | SHARED |
| [benchmarks/retrieval/src/record.rs](../../../../benchmarks/retrieval/src/record.rs) | PublishedUnitRegistry / symbol span_accounting assembly | 선택된 symbol unit과 독립 name witness를 결속한다. context 재검색으로 name 위치를 추정하지 않는다. | SHARED |
| [tools/ci/tests/test_retrieval_native_span_projection.py](../../../../tools/ci/tests/test_retrieval_native_span_projection.py) | actual native span fixtures | correct file/wrong name·same-line declarations·use-only·expanded context negative를 추가한다. | OWNED |
| [tools/ci/tests/test_source_oracle_suite.py](../../../../tools/ci/tests/test_source_oracle_suite.py) | source name gold | 고정 byte offsets·case/Unicode independent golden을 검증한다. | OWNED |

## 실행 단계

1. 현재 symbol unit definition span과 name-span source gold를 exact fixture에서 대조해 필요한 필드를 결정한다.
2. unit_id/source digest/definition/name witness의 최소 runner record 계약을 I0와 확정한다. product wire 변경이 꼭 필요하면 명시적으로 별도 public-surface gate를 건다.
3. 같은 줄의 두 선언과 동명 다른 receiver에서 SymbolId를 보존하는 producer를 연결한다.
4. 기존 evaluator 안에서 file@10, definition recovery, name recovery를 별도 출력한다.
5. 작은 actual SDK symbol capture와 replay를 실행한 뒤 OSA/정확 이름 lane에서 지원 가능한 셀만 평가한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_native_span_projection.py tools/ci/tests/test_source_oracle_suite.py tools/ci/tests/test_retrieval_benchmark.py -q`
- `./scripts/cargow test -p quanta-index-retrieval-bench --lib --locked`
- Negative: file-only row·잘못된 SymbolId/source hash·사용처·같은 줄의 다른 선언·context enlargement·case mismatch가 name recovery에 기여하면 실패.

## 완료 조건

- 정답 name bytes는 source에서 독립 생성되고 native selected unit과 연결된다.
- unsupported 제품/unit은 분모와 이유를 별도 표시하며 file hit를 name recovery로 승격하지 않는다.

## 중단·거절·재개 조건

- 전체 benchmark의 precise span 완료를 작은 fixture로 주장하지 않는다.
- 공통 record/schema 수정은 I0, search-plane/SDK 공개 surface는 E3와 소유권을 합의한다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
