# 06 · 파괴 변경과 대체 구현 판정표

상태 **NOT_RUN**. 이 문서는 01–05의 기능 누락과 전환 경계를 찾는 기준이다. 소스에서 확인된 의존성과 동작을 출발점으로 삼고 실제 순정 후보의 차이는 각 티켓에서 실측한다.

## 현재 패치 확인 범위

2026년 11월 23일, 로컬 13개 패키지를 같은 버전의 crates.io 배포 아카이브와 비교했다. 레지스트리 인덱스의 SHA-256과 아카이브를 13/13 대조한 뒤 추적 파일을 경로와 내용 해시로 비교했다. 이 범위는 **정적 비교 VERIFIED**다. 순정 후보의 API 호출이나 제품 실행은 **NOT_RUN**이다.

- SQLite 변경은 named VFS와 계측에 더해 amalgamation의 검증 읽기 분기와 WAL frame append·checkpoint batch를 포함한다. 포크가 제공하는 FD close는 SQLite 밖에서도 사용된다. [02의 SQL-04/06/11](02-sqlite-and-storage-limits.md)
- Lance 변경은 파일 I/O와 IVF reader 수명에 더해 scanner·LanceDB의 exact 동점 정렬 옵션을 포함한다. Ciborium 변경은 admitted decode 외에도 serializer, tag, integer 및 동적 `Value` 경계를 포함한다. Unicode는 scratch 정책과 정규화 내부 모듈·테이블 파일에 변경이 있다. [03](03-lance.md) · [04](04-cbor-unicode-paired.md)
- 문법 5개는 Rust binding·생성 parser에서 차이가 있고, Go·JavaScript·TypeScript에는 문법 소스 차이도 있다. 생성물 범위와 Python 로드 경계는 [01](01-tree-sitter.md)에서 판정한다.

## 13개 소스와 별도 데이터

| 로컬 패키지 | 삭제로 드러나는 경계 | 대체 구현 또는 변경 | 담당 |
| --- | --- | --- | --- |
| tree-sitter-go | `new(expression)`, built-in shadowing과 variadic 처리, Go Rust/Python 공급자 | 순정 양쪽으로 문법 fixture 수용. 정상 AST 표현 차이는 extractor에서 해소 | [01](01-tree-sitter.md) |
| tree-sitter-javascript | reserved export alias 허용·binding 거절, ABI 15 | 순정 문법과 실제 런타임 ABI 검증. alias로 가짜 선언을 만드는 경우 거절 | [01](01-tree-sitter.md) |
| tree-sitter-python | 고정 생성 parser·ABI·gold 선언 범위 | 공식 언어 배포본과 로드 identity를 결속하고 선언·chunk parity 검증 | [01](01-tree-sitter.md) |
| tree-sitter-rust | 고정 생성 parser·ABI·gold 선언 범위 | 공식 언어 배포본과 로드 identity를 결속하고 선언·chunk parity 검증 | [01](01-tree-sitter.md) |
| tree-sitter-typescript | TS·TSX, generic import·type-star export·줄바꿈 call signature | 구·신 AST를 비교하고 두 grammar entry를 유지. 정상 문법을 파싱하지 못하는 순정 조합은 미수용 | [01](01-tree-sitter.md) |
| rusqlite | pager cache 관측과 진단 출력 | 계측 퇴역. 진단 schema·소비자·테스트에서 같은 변경으로 제거 | [02](02-sqlite-and-storage-limits.md) |
| libsqlite3-sys | 파일·FD·WAL/SHM·용량·종료 hook, SQLite 외 소유 FD close 호출 | Root 하드 한도, 기존 SQLite 트랜잭션, 독립 파일 close 경계로 분리. 파일별 사전 거절 및 hook 전용 수치 변경 | [02](02-sqlite-and-storage-limits.md) |
| lance | IVF metadata 캐시·reader 수명, exact 동점 순서, 포맷과 ANN 정합성 | 실제 reader 해제와 재오픈을 검증한다. exact 동점은 원 스코프에 정렬을 적용한 뒤 top-K를 자르는 경계에서 보존한다. | [03](03-lance.md) |
| lance-io | 원 요청이 소유하는 파일 열기·쓰기·임시 파일 경계 | 표준 I/O와 Quanta 소유의 job·stage·게시·정산 경계로 연결 | [03](03-lance.md) |
| object_store | 로컬 store hook 및 stage namespace 이동·실패 복구 | 표준 store에서 실제 데이터 작업을 수행하고 Quanta가 소유한 준비/확인/공개/회수를 상태 전이로 검증 | [03](03-lance.md) |
| lancedb | 로컬 listing·exact query 옵션, 전이 feature·의존성, 실제 포맷 선택 | 공개 API 어댑터에서 쿼리·디렉터리 생성 경계를 대체하고 순정 버전 조합과 release dependency graph 검증 | [03](03-lance.md) |
| ciborium | Q/S admitted decode·거절·scratch, writer·동적 `Value`·tag·integer, CBOR wire, no-std·CI 등록 | 정식 codec와 공용 바이트·admission 경계를 적용. 입력 한도, 오류·wire bytes·영속 digest를 기준선과 검증 | [04](04-cbor-unicode-paired.md) |
| unicode-normalization | Q/S NFC scratch와 한도, 테이블·정규화 정책, 렌더링·검색 키·경로·식별자 | 정식 NFC와 공용 admission 경계로 전환. 고정 Unicode 데이터와 정체성·거절·정산을 대조 | [04](04-cbor-unicode-paired.md) |
| Unicode 17 reference data | Rust/Python 독립 기준 데이터 참조 | 원본 해시·라이선스와 함께 fixture로 이동하고 양쪽 해시 결속을 검증 | [04](04-cbor-unicode-paired.md) |

## 파괴 변경을 허용하는 경계

| 경계 | 처리 |
| --- | --- |
| 내부 Rust API, Cargo feature, 패치 전용 diagnostics, benchmark provenance | 한 PR 또는 한 Q/S 후보에서 호출자·manifest·테스트를 원자적으로 바꾼다. 실제 지원하는 외부 소비자가 확인되면 공개 변경으로 승격한다. 이전 benchmark identity는 이전 실행물에 귀속한다. |
| 외부 SDK·CLI·IPC, 영속 포맷·식별자 | 이번 의존성 전환에서 호환을 먼저 입증한다. 호환이 후보에서 불가능하고 제품이 변경을 수용하면 동일 릴리스에 명시한 계약으로 한 번에 절체한다. 바뀌는 필드·format·정상 및 거절 입력·모든 소비자·기존 데이터 변환·복구 지점을 같은 변경에 기록한다. |
| 지원 중인 기능이 순정 API에 없는 경우 | 기존 앱 경계나 지원 API로 대체하고, 그 경계에서 원 계약의 관측 가능한 증거를 다시 만든다. 가능한 대체가 없으면 명시적 계약 변경으로 승인될 때까지 해당 fork 제거는 `BLOCKED`다. 입력·결과·측정 누락을 성공이나 빈 결과로 처리하지 않는다. |
| 이전 저장 데이터·운영 복구 | 실제 승인 바이너리와 데이터 사본으로 판정한다. 포맷이 호환되면 기존 Root 재오픈을 사용한다. 변경 승인이 필요한 경우에만 기존 export/import·restore-forward로 별도 Root를 구성하고 구·신 digest, 필요한 세대와 영수증, newest acknowledged frontier를 검증한다. 일부 인덱스를 Source에서 다시 만들 경우 해당 인덱스의 검증·활성화 시점과 데이터 손실 허용 범위를 먼저 정의한다. |

## 누락 확인 방법

1. 실행 전 고정 Q/S에서 위 13개 vendor 사본과 **동일 출처/버전의 공식 패키지**의 파일 비교 결과를 대조한다. 배포 패키지 메타데이터·라이선스·문서, 생성물 재생성, 실행 코드·feature·테스트를 변경 종류별로 확인한다. 생성물 차이는 원본 입력과 실제 ABI에서 추적한다. 위 표에 없는 호출·동작 또는 테스트가 나오면 소유 티켓의 처분과 검증을 먼저 추가한다.
2. Rust 공개 API·Cargo feature·C/FFI export·Python import·빌드/실행 옵션·테스트와 운영 스크립트를 역추적한다. Rust host/target의 dev·build·normal 의존성, nested/fuzz lock, S 공급자 bootstrap, 릴리스/compiler 입력을 비교한다. 정적 문자열 검사만으로 런타임 호출이 보존됐다고 판정하지 않는다.
3. 기존 각 호출과 실제 저장·wire 경로에 **기존 결과 → 전환 결과 → 독립 기대값 → 정확한 실행 선택자**를 붙인다. 대체 구현에는 포크에만 있던 동작을 실행한 본문을 넣는다. 퇴역 단계에서는 옛 설정·명령·어뷰징 입력이 준비되지 않은 새 경로로 우회하지 않는지 검사한다.
4. 포크를 하나씩 제거한 후보의 첫 실패 컴파일·실행 위치를 확인한 뒤 대체 구현을 검증한다. 실패가 발생하지 않는 사본은 실제 upstream path가 선택됐는지 먼저 확인한다. 형식이 맞아도 외부 입력을 읽거나 기존 제품 저장소·벤치 분모에 도달하지 않은 결과는 미수용이다.

완료 증거는 [05](05-integration-and-release.md)의 버전 혼합·활성화·복구 기준에서 판정한다. 이 표의 항목은 계획상 인벤토리이며 순정 후보의 빌드와 제품 동작은 아직 검증하지 않았다.
