# O4-E1-04 — 정확한 선언 이름 span과 unit 회수 평가

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E1 — 정답·검수·admission과 독립 평가](../epics/E1-labels-admission-and-gold.md) / E1 담당 |
| 우선순위 / 종류 | P1 / `CODE_AND_PROOF` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | name-span producer/scorer·formal oracles 및 새 Gin4 exact-symbol/name capture·scoring `VERIFIED`; 전체 name/typo scoreboard qualification `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

같은 파일 적중과 정확한 declaration-name/ID 회수를 별도 관측으로 평가하고 반환 context 확대가 성공을 만들지 못하게 한다.

## 배경과 현재 상태

evaluator에는 indexed_span_diagnostics와 declaration_recall_at_k/declaration_mrr_at_k가 이미 있다. 현재 _declaration_match는 symbol unit의 정확한 indexed definition span과 judgment를 비교한다. source_oracle.declaration_name_spans는 별도의 name bytes를 제공하고 Rust RawDefinition은 definition bytes를 보유한다. 이 두 범위를 같다고 가정할 수 없다.

## 2026-10-04 중앙 검증

- `VERIFIED`: parser의 선언 이름 witness를 runner-local symbol registry와 연결하고 evaluator에서 file/definition/name 회수를 구분했다. context 확대나 같은 파일의 다른 선언을 name 회수로 계산하지 않는다.
- 명령: `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_native_span_projection.py tools/ci/tests/test_source_oracle_suite.py tools/ci/tests/test_holdout_review.py tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_retrieval_latency_status.py -q --tb=short` — 786 passed, 618.45s, exit 0. 이 수는 전체 선택 배치이며 name-span 전용 테스트 수가 아니다.
- `VERIFIED`: clean source107의 formal Contract proof는 Rust191을 모두 실행해 통과했다. name-inventory의 partial/foreign/wrong-byte 거절1 및 local-name의 same-line usage/Unicode/dotted namespace 고정 oracle2를 포함한다. 처음 required registry에서 이3 IDs가 빠져 거절됐고, 기존188을 유지하며 추가한 뒤 전체 proof와 별도 context verify를 통과했다.
- 실제 Gin checkout `d3ffc9985281dcf4d3bef604cce4e662b1a327a6`의99 Go files 및 exact-name4개에 대해 source SHA/byte witnesses를 읽어 PREPARE했다. matching release binaries 뒤 canonical source oracle의 symbol/name judgments로 새 suite/pack을 생성하고 실제 단독 `exact_symbol_name` 캡처를 할 계획이다. 아직 input 생성/capture/replay는 `NOT_RUN`이다. 과거 file-only controls를 새 name-span gold로 재결속하지 않는다.
- 현재 exact symbol route는 OSA 오타를 교정하지 않는다. 기존 `go_declaration_name_osa1_v1`의 distinct-file scoreboard를 name-span recovery로 환산하지 않는다. 새 실제 SDK symbol capture와 지원 가능한 name lane의 전체 scoreboards는 `NOT_RUN`; fixture 통과를 benchmark 완료로 승격하지 않는다.

## 2026-10-04 Gin4 실제 exact-name 진단

- helper v1은 `responseWriter` receiver 내부의 `Write`를 함수명으로 선택해 witness assertion에서 실패했다. 실제 Go 선언 anchor와 독립 source oracle는 함수명 `[1797,1802)`를 가리킨다. 두 줄의 helper 수리 뒤 새 root에서 suite/pack을 생성했다. 제품/평가기 계약을 수정하지 않았다.
- 긴 Documents output root는 control socket116 bytes/limit103 사전 점검에서 exit2로 거절됐다. 기존 실패 출력을 보존하고 짧은 `/private/tmp/qi-name4-v3`로 새 입력을 생성했다.
- `VERIFIED`: clean source107에서 `uv run --frozen --extra dev python -m tools.benchmark.retrieval.run quanta --spec /private/tmp/qi-name4-v3/capture-spec.json` — exit0, 실제 matching fresh release runner/searchd, Gin99 Go files, `exact_symbol_name`/symbol 한 route, 4 completed results.
- `VERIFIED`: `uv run --frozen --extra dev python -m tools.benchmark.retrieval evaluate-diagnostic --repo /Users/songmin/Documents/code-new/qi-large-scale-rerun-20260927/full-checkouts/gin --suite /private/tmp/qi-name4-v3/source-oracle/go-declaration-symbol-suite.json --runner /private/tmp/qi-name4-v3/capture/strategy-00-fw_strict/record.json --output /private/tmp/qi-name4-v3/name-report.json` — exit0. L0065/L0248/L0990/L1291의 declaration 및 name recovery MRR@10/Recall@10 모두1.0, coverage4/4, `source_oracle_complete_v1`.
- report는 `diagnostic_unqualified`, oracle는 `go_exact_local_name_v3`, rank unit은 symbol이다. source-bound 좁은4-query 진단이며 전체 Gin/OSA typo, file-only 외부 제품, unseen gold, human review 또는 품질 비교 qualification을 주장하지 않는다.

## 착수 입력과 남은 범위

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
