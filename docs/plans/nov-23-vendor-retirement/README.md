# 외부 라이브러리 포크 퇴역

2026년 11월 23일 KST. Quanta Index의 로컬 외부 라이브러리 사본 13개를 Cargo 레지스트리 배포본으로 전환한다. Tree-sitter 문법 5개, rusqlite와 libsqlite3-sys, Lance 계열 4개, Ciborium과 unicode-normalization을 대상으로 한다. 문법 사본에는 호환 패치와 생성물만 고정한 사본이 모두 포함된다. 작업 트리의 Cargo 설정과 실제 호출 경계를 기준으로 작성했다. 일부 구현이 진행 중이며 최종 제품 검증은 **NOT_RUN**이다. [현재 Cargo 설정](../../../Cargo.toml)

현재 작업의 파일별 변경 순서, 병렬 소유 경계, 중단 조건과 검증 명령은 [10 · 실행 작업표](10-execution-work-breakdown.md)에 모은다. 각 티켓의 상세 요구사항과 원래 소유 원장이 완료 판정의 기준이다.

## 제품 계약

- 단일 호스트 배포에서 관리 상태 Root의 물리 용량 하드 한도를 배포 환경이 집행한다. Quanta가 소유한 요청 입장, 큐, 동시성, 논리 Work·메모리 예산은 유지한다. SQLite·Lance 내부 파일마다 원 요청에 결속해 사전 거절하는 계약은 이 전환에서 축소한다. 실제 적용 대상, 포함 경로와 정확한 거절 경계를 [02](02-sqlite-and-storage-limits.md)에서 검증한다.
- 내부 API·포크 전용 Cargo feature·진단·벤치 증거 형식은 영향받는 호출자와 함께 변경할 수 있다. SDK·CBOR wire, 식별자 정규화, 저장 포맷과 공개 게시 결과는 우선 호환을 검증한다. 공개·영속 계약의 파괴 변경이 필요하면 영향을 받는 호출자, 상태 이관, 혼합 버전 거절과 복구를 같은 출하 후보에서 수용한다. [파괴 변경과 판정표](06-breaking-change-matrix.md)
- 의존성은 가급적 현재 잠금 버전의 레지스트리 배포본으로 전환한다. 호환 기능이나 결함 수정 때문에 버전을 올릴 때는 변경 근거와 영향을 같은 티켓에서 검증한다. 별도 Git 포크나 사내 미러에 패치를 옮기는 방식은 퇴역 완료 조건을 충족하지 않는다.
- Semantica는 Quanta Index의 Ciborium·Unicode 사본을 경로로 참조한다. 해당 공급자 핀·락·직접 호출을 두 레포에서 함께 전환한다. Semantica의 다른 외부 의존성 정리는 별도 범위다. [Semantica Cargo 설정](../../../../semantica-codegraph-v2/Cargo.toml)

## 작업 순서

| 티켓 | 변경 범위 | 선행 조건과 완료 기준 |
| --- | --- | --- |
| [01 · Tree-sitter](01-tree-sitter.md) | 벤치 Rust·Python 파서, 문법 사본 5개 | 첫 PR. 선언·chunk·검색 평가 계약을 검증하고 문법 사본을 제거한다. |
| [02 · SQLite와 저장소 한도](02-sqlite-and-storage-limits.md) | rusqlite 계측, SQLite VFS, Root 용량 계약 | 계측 제거를 먼저 검증한다. VFS 제거는 실제 배포 대상의 한도 집행과 저널 복구가 통과한 뒤 진행한다. |
| [03 · Lance 계열](03-lance.md) | lance, lance-io, object_store, lancedb | 02의 공통 Root 계약을 사용한다. 기존 인덱스와 재시작을 검증한 뒤 네 패키지를 같은 Rust 의존성 그래프에서 전환한다. |
| [04 · CBOR·Unicode](04-cbor-unicode-paired.md) | Q/S codec·정규화와 공급자 핀 | 저장소 변경과 독립적으로 준비한다. Q/S 소비자 전환과 기존 wire·identity 검증을 함께 닫는다. |
| [05 · 통합·출하](05-integration-and-release.md) | 잔존 의존성 검사, 런타임 수용, 절체 | 01–04 실제 결과를 인수한다. 최종 후보에서 로컬 포크 0, 게시·복구·운영 조건을 검증한다. |

각 단계에서 순정 배포본으로 바꿨을 때 깨지는 기능을 [판정표](06-breaking-change-matrix.md)와 대조한다. 살아야 하는 기능에는 대체 구현과 독립 기대값을, 바뀌는 계약에는 소비자 이관과 실패 시 복구를 결속한다.

01을 먼저 병합할 수 있다. 02의 순수 계측 제거와 04 소유 검증은 독립적이다. 02와 03의 제품 저장소 변경은 같은 Root 정책으로 수용한다. 서로 다른 방식으로 한도를 계산하는 배포 경로를 추가하지 않는다. 공유 Cargo·lockfile·등록 원장은 기존 통합 작성자가 한 번에 맞춘다.

## Native 엔진의 파일별 실행 계획

포크 퇴역과 별개로 남은 엔진 작업을 실제 파일·함수·선택자에 대응시켰다. 구현과 실행 결과는 기존 담당 티켓에 기록한다. 아래 문서는 작업 진입점과 조건을 정의한다.

| 작업 | 시작할 코드와 변경 | 첫 판정과 인수 |
| --- | --- | --- |
| [07 · Product4K와 Text 초기 게시](07-engine-product4k.md) | Q B/TEXT. [Text postings](../../../crates/quanta-index-lexical/src/native/text.rs)의 term dictionary 준비와 posting map 적용 시간을 분리한다. 초기 게시가 끝나면 [Runtime worker](../../../crates/quanta-index-searchd-runtime/src/native_worker.rs)·Kernel/Catalog의 materialization 단계에서 지배 비용을 확인한 뒤 해당 경계만 바꾼다. | 현재 포크 상태로 동일 4K 후보의 첫 실패를 잡는다. 원 예산·단일 슬롯의 ANN4K와 tracing Off Product4K를 05에 인수한다. |
| [08 · Source V2 DELTA](08-engine-source-delta.md) | S Store V4 준비·commit, Runtime의 target 발급, SDK 직접 호출과 Native resume 사용처를 한 V2 계약으로 연결한다. Q에서는 owner edits·ingress replay의 미연결 부분을 조정한다. | 독립 FULL parity, warm 전체 열거 0회·무관 본문 0B, 동일 operation에서 Source CAS 1회와 최신 검색 결과를 확인한다. |
| [09 · 검색 품질·운영·출하](09-engine-query-and-release.md) | Native read session, Semantic 후보, Hybrid 융합에 대한 기존 파일 단위 캡처를 사용한다. 후보 생성·융합·대기 중 실제 실패가 확인된 위치에만 검색 코드 변경을 둔다. | 최종 후보의 동일 뷰 ANN/Hybrid 품질, E07 운영 수치, signed Q/S 공개 SDK/CLI·History·복구를 05에 연결한다. |

07과 08은 기존 포크 후보에서 바로 시작한다. 01의 벤치 문법 전환과 독립 개발은 진행할 수 있고, 품질 비교에 쓰는 parser·코퍼스·qrels는 양쪽 실험에 같은 정책으로 고정한다. SQLite·Lance 또는 codec 전환 성능을 잴 때는 해당 비교의 양쪽에 같은 엔진 수정과 같은 RepoMap 지원 범위를 적용한다. 공용 Q Catalog·Runtime 및 S Source/SDK 변경은 기존 소유자와 통합 작성자가 조정한다.

첫 소유 변경은 **07의 최초 실패 단계 계측**과 **08의 미게시 V2 target·commit 결속 구현**이다. 09의 코드 변경은 같은 뷰에서 얻은 후보·순위·대기 시간으로 실패 위치를 확인한 뒤 시작한다. 공용 실행 자원이 필요한 진단·정식 시험은 기존 큐에서 실행한다.

## 기존 작업과의 연결

- Tantivy·RepoMap 퇴역은 각 기존 원장이 소유한다. 07–09는 기존 4K·SDK60·Large·Hybrid의 작성 범위와 판정 경계를 가리킨다. 같은 코호트의 실제 실행은 기존 큐에서 한 번 인수한다. [전환 원장](../nov-20-native-cutover-closeout/tickets/README.md) · [RepoMap 퇴역](../nov-23-repomap-retirement/README.md) · [현행 4K](../nov-21-bugbash-source-advance-4k/04-product-acceptance.md)
- 첫 구현 전 관련 변경 경로의 작성자, dirty hunk, 실제 후보 HEAD를 확인한다. 하나의 Q/S 후보를 기존 설치·실행 큐에 맡기고, 실행 중 영향 입력이 바뀌면 해당 범위의 결과만 다시 검증한다.
- vendored 소스 삭제의 범위는 현재 트리와 차기 출하 입력이다. 과거 Git 이력 재작성이나 이전 승인 배포물·복구 Root 삭제는 포함하지 않는다.
