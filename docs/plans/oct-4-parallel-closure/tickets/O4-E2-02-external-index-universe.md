# O4-E2-02 — Sourcegraph·OpenGrok 전체 native 색인 범위

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P1 / `DATA_AND_PROOF` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

해당 manifest의 모든 source file이 실제 service index에 있고 요청 전후 동일한 index identity였는지 입증한다.

## 배경과 현재 상태

B08 C3는 13,347파일 native stored-content/source-posting reference 증거가 있다. B09는 11,695파일이며 다른 universe다. sourcegraph_index_scope.py는 receipt 검증기이고 full native path inventory 생성기는 아니다. post-capture probe만으로 before/after 상태를 생성할 수 없다.

## 착수 입력

- 외부 BASE의 sourcegraph-native-content-8l9gxy35, opengrok-source-posting-reference-full-moyyggn_ 역사적 증거
- 새 capture의 정확한 release/manifest/file hashes, live service/runtime/config/image/index tree identity
- native revision/path/content 또는 posting/reference 접근 권한

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/sourcegraph_index_scope.py](../../../../tools/benchmark/retrieval/sourcegraph_index_scope.py) | verify / _verify | 현 receipt의 declared scope와 full inventory/source hash/phase/time binding을 확인한다. 누락 evidence를 validator가 합성하게 하지 않는다. | OWNED |
| [tools/benchmark/retrieval/live_lexical_external.py](../../../../tools/benchmark/retrieval/live_lexical_external.py) | _backend_runtime / _backend_snapshot / _opengrok_indexed_inventory / _opengrok_indexed_view | existing backend inventory/content probe producer를 사용해 new before/after evidence를 bind한다. stale probe 재사용은 조건별로 거절한다. | OWNED |
| [tools/benchmark/retrieval/sourcegraph.py](../../../../tools/benchmark/retrieval/sourcegraph.py) | validate_capture | result content/defs/refs request scope와 indexed universe를 구분한다. | OWNED |
| [tools/ci/tests/test_live_lexical_external.py](../../../../tools/ci/tests/test_live_lexical_external.py) | index inventory mutants | missing/extra file·same path wrong bytes/revision·before/after identity drift controls를 추가한다. | OWNED |
| [tools/ci/tests/test_sourcegraph_parity_inventory.py](../../../../tools/ci/tests/test_sourcegraph_parity_inventory.py) | Sourcegraph universe parity | complete expected file set 및 native index identity를 고정 fixture로 대조한다. | OWNED |

## 실행 단계

1. 각 cohort별 expected native path/hash/revision과 projection mapping을 source manifest에서 열거한다.
2. 서비스 native full inventory와 stored bytes 또는 선언된 source/posting reference를 수집한다. API가 일부 파일만 제공하면 completeness를 선언하지 않는다.
3. index tree/container/process/config의 identity 및 보존 근거를 freeze한다.
4. 새 capture 전후 inventory를 비교하고 모든 file hash/phase/response bounds를 verifier로 검사한다.
5. 과거 캡처는 실제 보존 근거가 있는 범위만 after_only로 새 sidecar에 발행한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_live_lexical_external.py tools/ci/tests/test_sourcegraph_parity_inventory.py -q`
- Positive: full native inventory와 same-universe manifest equality; before/after unchanged index.
- Negative: B08 receipt를 B09에 적용, missing/extra/wrong hash, source revision drift, content/defs/refs 혼동, fabricated pre-capture timestamp 거절.

## 완료 조건

- 제품×repository×profile마다 실제 입증한 source/index scope와 누락·unknown 집합이 있다.
- 실행 서비스 identity를 index inventory와 query bracket에 연결하고 full proof가 없으면 해당 qualification을 BLOCKED로 표시한다.

## 중단·거절·재개 조건

- full analyzer-term equivalence·물리 posting completeness를 단순 file listing/content probe로 주장하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
