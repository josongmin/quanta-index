# O4-E2-02 — Sourcegraph·OpenGrok 전체 native 색인 범위

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P1 / `DATA_AND_PROOF` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | Sourcegraph owner169·v5 및 OpenGrok v2 path-bearing replay `VERIFIED`; 후속 전체 live17,615 문서·auxiliary 필드 형태·source13,347 path/UID term 독립 replay `VERIFIED`. 실제 query 전후 whole-index 결속과 canonical universe qualification 미완료 |
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
- `VERIFIED`: producer format을 끝낸 뒤 고정한 source로 current full E2 regression 169 passed/347.08s. 앞선 전체 실행은 producer bytes가 중간에 바뀌어 168 pass/1 fail이었고, 해당 case fresh replay와 새 전체 배치로 source-stable 결과를 확인했다.
- v4 root `qi-e2-sg-native-owned-v4-20261004-r5aasda9`는 format 반영을 위해 canonical `BoundRelease.begin` 도중 중단했다. native capture/service는 시작되지 않았으며 interrupted terminal을 외부 root에 보존했다.
- current root는 `/Users/songmin/Documents/code-new/qi-e2-sg-native-owned-v5-20261004-d5al5tlg/batch.json`이다. C3 12repo/13,347파일의 complete release validation 및 owned native scope producer는 exit0으로 끝났고 각 repository receipt를 보존했다.
- `VERIFIED`: clean source904의 uv CPython3.13.9에서 `uv run --frozen --extra dev python /tmp/quanta-e2-sg-v5-replay-20261004.py --source-root /Users/songmin/.codex/worktrees/oct4-qualified-source/quanta-index --batch /Users/songmin/Documents/code-new/qi-e2-sg-native-owned-v5-20261004-d5al5tlg/batch.json` — exit0. canonical `BoundRelease.begin`, 각 cell 전후 complete release recheck 및 `sourcegraph_index_scope.verify(require_owned_service=True)`가 12개 모두 `owned_guest_translator_v1`을 확인했다. bat79/cli1014/django2368/lo130/mocha473/nushell1947/sqlalchemy652/tailscale2532/typeorm3608/uvicorn72/zellij422/zustand50, 합계13,347파일이다.
- replay 출력은 `qualified:false`, scope `Sourcegraph native stored-byte and path replay only`다. 이 실행은 새 benchmark query before/after bracket, analyzer/posting equivalence, ranking/speed qualification을 발행하지 않는다. B09 11,695파일 universe에도 적용하지 않는다. owned reader cleanup 후 새 live capture가 service를 restart하면 기존 runtime/index identity를 자동 승계하지 않는다.
- 최초 system Python3.9 replay는 `zip(strict=True)` 미지원으로 실패했다. 원본 producer/receipt bytes를 바꾸지 않고 frozen uv 환경에서 전체 replay를 다시 실행했다.

## 2026-10-04 OpenGrok fresh scope 재개

- `FAILED`: fresh root `qi-b08-closeout-20261004-2i72kj91/opengrok-native-full-20261004-v1`의 full-scope 명령은 canonical release validation 후 stopped container를 만나 exit1이었다. observed exit137, `OOMKilled=false`이며 종료 원인은 확인되지 않았다. indexed-view cell이나 benchmark query를 실행한 결과가 아니다.
- 기존 C3 전용 container `9a3e2dc263e2f28eb3502dd7d42958341fc39461bd4c505cd9efe792f5ef8e48`의 image/owned volumes/startup 설정을 확인해 재개했다. `NOMIRROR=1`, periodic sync0이며 initial startup sync의 `Sync done` 및 `Waiting for reindex to be triggered`를 실제 관찰했다. 이를 이전 index identity가 유지됐다는 증거로 사용하지 않는다.
- 새 외부 root `qi-b08-closeout-20261004-2i72kj91/opengrok-native-full-20261004-v2`에서 `uv run --frozen --extra dev python /private/tmp/quanta-e2-og-scope-driver-20261004.py --spec /private/tmp/quanta-e2-og-scope-input-20261004.json --output /Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91/opengrok-native-full-20261004-v2`를 실행 중이다. release/producer/token 입력을 재검사하고 전체 API path/served-byte before/after와 기존 vendor/Lucene의 path-bearing posting reference를 수집한다. 완성 receipt 및 재생 전에는 PASS를 주장하지 않는다. 이 scope driver는 benchmark query를 제출하지 않으며 `query_bracketed=false`다.
- 중간 raw posting 관측은 manifest의13,347 path-bearing documents에 대해 path set/source SHA/`full` term·frequency·position·offset 불일치0이다. Java producer는 `stored.path == null`인 live auxiliary4,268개를 내용/UID/type 검증 없이 건너뛴다. path-bearing UID의 nonnull/global uniqueness는 raw 재집계 관측이며 현재 probe의 필수 predicate가 아니다. 전체 live UID universe 또는 auxiliary provenance를 입증했다는 표현을 사용하지 않는다.
- 기존 named-volume snapshot은 native runtime·12projects/132 index artifacts의 digest를 기록하지만 canonical live collector의 bind/readonly backend snapshot과 호환되지 않으며 API host-port→container 연결도 검사하지 않는다. collector의 `opengrok_indexed_universe_attested=false`는 유지한다. source/UID/auxiliary 분류·native endpoint binding·실제 query 전후 snapshot 소비 및 독립 replay를 보완하기 전 qualification은 미충족이다. index hash가 같거나 v2 scope driver가 끝났다는 사실로 이 gap을 해소하지 않는다.
- v2 producer는 exit0으로24개 repository-phase sweep을 모두 끝냈다. prequery scope의 네 captured snapshots는12projects/132 artifacts/136,606,249bytes, index SHA `ae8623d124eeac39af460ce985d4b901ddac2c22e90d3350df354be2d09a9873`다. producer 종료만으로 raw replay를 PASS로 표시하지 않는다. clean source107과 canonical uv Python에서 별도 offline replay를 실행 중이며, API path/served bytes·path-bearing UID/type/posting 및 retained snapshot identity를 재검증한다. auxiliary 내용·물리 index 재읽기·실제 query bracket은 이 replay의 범위 밖이다.
- 별도 현재 관측 `docker inspect --format '{{json .NetworkSettings.Ports}}' <C3-container>`은 `8080/tcp → 127.0.0.1:18083`를 반환했다. 현재 endpoint 매핑의 read-only 관측이며, producer/consumer가 실제 query 전후에 이를 검사한다는 보장은 아니다.

## 2026-10-04 OpenGrok v2 독립 raw replay 완료

- `VERIFIED`: clean source107에서 `uv run --frozen --extra dev python /tmp/quanta-e2-og-fullscope-replay-20261004-v1.py --spec /private/tmp/quanta-e2-og-scope-input-20261004.json --capture /Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91/opengrok-native-full-20261004-v2 --source-root /Users/songmin/.codex/worktrees/oct4-qualified-source/quanta-index --source-head 1071692b2dd4d5a77db54f79ecd0e80a1a20b2a7 --output /Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91/opengrok-native-full-20261004-v2-offline-replay-v1.json` — exit0.
- 실제 replay는80 API inventory responses,24 repository-phase sweeps의26,694 served files,4 retained native snapshots 및13,347 path-bearing full-posting rows를 검증했다. 해당 row들의 UID nonnull/global uniqueness 및 stored type/reference가 일치했다. index SHA는 `ae8623d124eeac39af460ce985d4b901ddac2c22e90d3350df354be2d09a9873`다.
- result는 `qualified:false`, `query_bracketed:false`, `entire_uid_universe_verified:false`, `auxiliary_document_contents_verified:false`를 유지한다. auxiliary4,268의 count만 관측됐으며 물리 index 재읽기·새 query request·native API port bracket은 이 실행 범위가 아니다. 기존 producer summary의 전체 UID 표현을 이 증거 범위로 승격하지 않는다.

## 착수 입력과 추가 qualification

- 2026-10-05 실제 whole live observation `VERIFIED`: clean source107의 `uv run --frozen --extra dev python /private/tmp/qi-og-full-live-docs-prep-20261005/observe.py --input /private/tmp/qi-og-full-live-docs-prep-20261005/input.json --output /private/tmp/qi-og-full-live-docs-actual-20261005-v1 --source-root /Users/songmin/.codex/worktrees/oct4-qualified-source/quanta-index --source-head 1071692b2dd4d5a77db54f79ecd0e80a1a20b2a7` — exit0. 모든 live stored field의 typed values 및 indexed field의 term 수/빈도/digest를 관측했고 before/after12repo API path set, native index digest, container/image/pid/restart, published127.0.0.1:18083→8080, named-volume RW, config/74JAR/source/input을 검사했다. owned remote helper cleanup도 성공 조건이다.
- 독립 offline readback `VERIFIED`: 같은 source의 `uv run --frozen --extra dev python /private/tmp/qi-og-full-live-docs-readback-20261005-v2.py --capture-root /private/tmp/qi-og-full-live-docs-actual-20261005-v1 --output /private/tmp/qi-og-full-live-docs-offline-readback-20261005-v2.json` — exit0. live17,615 = source path13,347 + path-field/string 둘 다 없는4,268; repository당3segments, 총36segments다. path 없는4,256문서는 stored `d/loc/numl` + indexed `d/dirpath`, 나머지12문서는 stored `objser/objver` + indexed `objuid` 형태였다. path 없는 문서의 stored `u/type/project/associatedpath` 및 indexed `u`는 없었다. 누락된 UID나 source 연결을 생성하지 않는다.
- 후속 root 독립 streaming 대조도 exit0: source path13,347의 set이 frozen code-only manifest와 정확히 일치하고 duplicate path0이다. source13,347의 stored UID는 모두 nonempty/unique이며 indexed `u`의 distinctTerms=occurrences=1과 `SHA256(big-endian length || UTF-8 stored UID || big-endian frequency1)`를 모든 행에서 재계산해 raw digest와 일치했다. auxiliary를 file UID로 세거나 `objuid`를 source UID로 바꾸지 않는다.
- raw197,194,162bytes의 SHA는 `efb5df4894282b4f523340129a7f9b251b5b83ba3b3f415bc3a9d9c8a9de60d9`, native index SHA는 기존 `ae8623d124eeac39af460ce985d4b901ddac2c22e90d3350df354be2d09a9873`와 같다. 실제 terminal `result.json`은 `observation_only`, `qualified=false`, `query_bracketed=false`, `entire_uid_universe_attested=false`다. 이번 관측은 query를 실행하지 않았고 named-volume RW를 canonical readonly bind로 취급하지 않는다. whole query bracket·producer의 auxiliary 의미/분모·canonical consumer qualification은 별도 보완 범위다.
- auxiliary `d` 값과 frozen source ancestor directories의 단순 set은 동일하지 않았다(저장소마다 extra1, bat missing1). source file set은 정확히 일치한다. 이 차이만으로 source 누락이나 corruption을 판정하지 않으며 native directory/object 필드의 의미를 확인하기 전 source association을 합성하지 않는다.
- 2026-10-05 static PREPARE: `/private/tmp/qi-e2-opengrok-next-20261005/bat-og-only-spec.json`은 기존 canonical v2의 OG-only `indexed_view_probe:full`을 사용한다. source107 원본 bat admission에 결속하며 actual query 전후 API inventory/served bytes를 새 root에서 관측하도록 준비했다. 실행은 `NOT_RUN`; supplemented bat409나 새 sourcecef의 spec이 아니다. auxiliary4,268/whole UID/native readonly backend 결속을 이 API bracket으로 승격하지 않는다.

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
