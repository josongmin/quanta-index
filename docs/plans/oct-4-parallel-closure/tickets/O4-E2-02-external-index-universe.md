# O4-E2-02 — Sourcegraph·OpenGrok 전체 native 색인 범위

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P1 / `DATA_AND_PROOF` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | Sourcegraph owner tests160개·실제 owned guest lifecycle `VERIFIED`; 12repo fresh native batch 실행 중 |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

해당 manifest의 모든 source file이 실제 service index에 있고 요청 전후 동일한 index identity였는지 입증한다.

## 2026-10-04 producer와 실제 실행

- existing v1 receipt/verifier에 full V3 path inventory와 bounded native stored-byte reader를 연결했다. 전후 index/runtime/projection/source/control identity를 검사하며 payload hash를 manifest와 대조한다. posting/analyzer equivalence나 성능 qualification을 발행하지 않는다.
- `--scope-batch`는 같은 release의 distinct repositories에서 `BoundRelease.begin`을 1회 실행하고 cell 전후 complete release bytes를 recheck한다. expensive replay 전에 spec/token/producer/owner bytes를 고정하며 next-cell 변경도 거절한다.
- `VERIFIED`: `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_live_lexical_external.py -q --tb=short -k 'sourcegraph or native_index_scope or native_stored_body or native_worker or native_reader or index_scope_spec or native_scope_cli or scope_batch or native_listener'` — 65 passed / 81 deselected / 36.66s, exit 0. 최초 실행의 cleanup EPERM과 새 fixture의 순서 의존 기대값을 수리한 뒤 결과다.
- `FAILED`: 실제 bat scope CLI, output `/Users/songmin/Documents/code-new/qi-e2-sg-native-20261004-khoe7ry_/bat`. 6071에 listener가 없었다. 과거 6071은 별도로 띄운 native reader 포트이고 container restart가 이 reader를 재생하지 않았다. 현 6072는 indexserver이며 read endpoint로 치환하지 않는다.
- amd64 container의 `/proc/PID/exe`가 Rosetta translator임을 실제 확인했다. guest reader binary/mapping과 translator identity를 분리 결속하는 owned start/read/stop lifecycle를 통합했다. Python owner 전체는 `test_live_lexical_external.py test_sourcegraph_parity_inventory.py` 160 passed/285.05s였다. 실제 Docker pilot에서 guest SHA `cd47f95e…945e3`와 Rosetta SHA `723a1aee…f241b`, guest inode mappings3개 및 PID/start ticks를 결속했고 owned child/supervisor 종료·동일-token 종료 재시도·listener 부재를 확인했다. mount shadow/기존 listener/없는 binary는 실제로 거절됐다. direct script CLI의 import 실패도 source root 초기화로 수리해 module 및 checkout 밖 cwd direct invocation fixture2개를 통과했다. 현12repo fresh native batch는 `/Users/songmin/Documents/code-new/qi-e2-sg-native-owned-20261004-6hcetaoe`에서 실행 중이다. OpenGrok fresh before/after는 `NOT_RUN`이다.

## 배경과 현재 상태

B08 C3는 13,347파일 native stored-content/source-posting reference 증거가 있다. B09는 11,695파일이며 다른 universe다. sourcegraph_index_scope.py는 현재 receipt 검증기와 bounded full native path/stored-byte producer를 포함한다. post-capture probe만으로 before/after 상태를 생성할 수 없다.

## 2026-10-04 translator export 실제 보완

- 첫 owned 12-repository batch는 `/proc/PID/exe`에 대한 `docker cp -L` 실패로 중단됐다. remote identity SHA mismatch가 아니며 translator output 파일이 생성되지 않았다. 실패 root `qi-e2-sg-native-owned-20261004-6hcetaoe`를 보존했다.
- producer는 PID/start ticks/SHA를 고정한 proc FD를 최대256MiB/50초의 고유 O_EXCL regular file로 복사하고 Docker cp 후 host bytes를 비교한다. 성공/실패/시간초과에는 tombstone→owned worker 종료 확인→자기 파일 정리를 수행하고 cleanup 미확인은 거절한다. native replay의 translator SHA 검사는 유지한다.
- `VERIFIED`: `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_sourcegraph_translator_export.py -q --tb=short` — 8 passed, 0.21s. success/stale ticks/wrong remote SHA/wrong host SHA/overcap/cp failure/cleanup failure/indeterminate export를 포함한다.
- `VERIFIED`: 실제 Docker owned translator pilot `qi-e2-translator-pilot-20261004-ux7nyh6w` — 1,726,424 bytes, SHA `723a1aee626399b5620cbf46f11637d6b7fa79777b23e0ad4d1c9ae2a45f241b`; guest bytes/mapping identity 유지, host SHA 일치, worker/file 및 owned service cleanup 완료. corpus scope/query bracket proof는 아니다.
- current full E2 regression과 새 `qi-e2-sg-native-owned-v4-20261004-r5aasda9/batch.json`의 12repo/13,347파일 canonical scope batch는 실행 중이다. 완료된 범위로 선표시하지 않는다.

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

## Existing scope 소비와 추가 producer

- B08의 기존 13,347-file receipts는 원본 bytes·같은 universe/서비스/index revision·유효 retention/phase binding을 replay한 범위에서 소비한다. 모든 파일 probe를 무조건 다시 만들지 않는다. B09 11,695파일 또는 새 서비스 bracket에 소급 적용하지 않는다.
- Sourcegraph producer는 full path/SHA/native bytes와 terminal progress를 발행하고 canonical receipt verifier가 이를 재생한다. 새 live 소비는 owned guest proof를 요구하며 legacy direct replay는 proof level을 별도로 표기한다. diagnostic join을 final native qualification gate로 사용하지 않는다.
- bounded API/listing/SSE cap·partial/unknown과 collector MAX_INDEX_FILES 등의 actual limit을 확인한다. 기대 population이 bound를 넘으면 explicit refusal/streamed bounded plan 또는 비적격 scope로 처분하며 단순 limit 상향으로 completeness를 주장하지 않는다.
- post-capture receipt는 after_only다. before/after snapshot identity는 실제 fresh query bracket에서 수집한다; timestamp와 precondition을 사후 합성하지 않는다.

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
