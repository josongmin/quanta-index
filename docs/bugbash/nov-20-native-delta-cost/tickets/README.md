# Native 증분 갱신 비용 · 2026-11-20

| 티켓 | 코드와 확인된 결과 | 남은 수용 조건 |
| --- | --- | --- |
| [INC-01 · Semantica Source](INC-01-source-closure.md) | Facts 본문 조회 3곳은 경로별 Original Current를 사용한다. Store V4는 인증된 선행 트리를 재사용하고 Native는 보유 트리 쌍에서 변경 행을 재생한다. 동결 QBC에서 C1 Wire 13/13, Store V4 1/1, 디스크 writer 1/1 통과. | Facts·disk V2 target 발급 연결, Store target 전체 검증 전환, SDK 소유 테스트 5개가 Runtime Analysis 의존 크레이트 컴파일에서 막혀 있다. 제품 경로 FULL parity·warm 전체 열거 0회·무관 본문 0 B는 `NOT_RUN`이다. [코드·영수증](INC-01-source-closure.md#코드-단위-현황과-검증) |
| [INC-02 · Q Literal](INC-02-literal-directory.md) | 정식 nextest 9개 통과. 수정한 다중 페이지 성공·거절 두 본문도 정식 재실행에서 2/2 통과했다. | 설치 제품 검증은 INC-03, 최종 성능 수용은 E07에서 각각 관리한다. |
| [INC-03 · 제품 검증](INC-03-product-acceptance.md) | 기존 E02·NC-F 본문과 신규 pre-V9·129건 페이지·page ACK 유실·seal 응답 유실·Java 첫 DELTA 본문을 같은 Source/Q 코호트에서 인수한다. | 현행 Linux Q 실행 파일과 서명 V39 SQL worker가 결속된 설치 입력이 없다. 신규 본문 0/5, 기존 SDK·Watch 본문 0/3이며 제품 수용은 `NOT_RUN`이다. |

이 세 티켓이 이번 변경의 작업 목록이다. 각 티켓에 구현 위치와 해당 검증 결과를 기록한다. 전체 증분 시간과 XL 성능의 최종 판정은 E07에서 관리한다.

### 2026-11-22 현행 판정

- S `5dd6acd003b`·Q `4d2ee79a1`과 Q 변경분을 봉인한 입력에서 retrieval 가드 244개와 해당 정책 gate가 통과했다. 동일 S 스키마와 SQL 문장 207·209·210의 SQLite 검증은 19개 조건을 통과했다. 신규 Product selector 5개는 각각 기대 본문 하나로 해석됐다.
- 위 코호트의 SDK 통합 QBC는 호스트 컴파일 잠금에서 exit 75로 끝났다. 기존 C1 Wire 13/13, 디스크 1/1과 후속 Original pin 1/1·FULL inventory 2/2·디스크 1/1의 영수증은 INC-01에 기록했다. SDK successor는 의존 크레이트 컴파일에서 실패해 본문 0/5였다. Store 후속 통과 기록은 아래에 있다. 설치 Product 본문은 미실행이다.
- Store의 V1·V2 CAS 본문 읽기 바이트 계측은 staging 경로에 연결됐다. warm participant commit은 V1 target·predecessor 수화, V2 전체 census와 본문 검사를 수행한다. 공유 본문 digest가 없는 legacy 행이 하나라도 있으면 V1 본문 전체도 검사한다. 무관 본문 0 B는 미충족이다. [구현·검증 세부](INC-01-source-closure.md#코드-단위-현황과-검증) · [제품 인수 조건](INC-03-product-acceptance.md#2026-11-22-현행-제품-인수-상태)

### 2026-11-23 후속 검증

- Q Kernel의 첫 페이지 영속성·재개·fence 회귀는 **2/2 VERIFIED**다. Store의 Source finisher와 선행 컴파일 수리를 포함한 동결본에서 V4 회귀와 inventory 전이 회귀가 각각 **1/1 VERIFIED**다. [Store 원장](INC-01-source-closure.md#코드-단위-현황과-검증) · [Q 회귀와 제품 원장](INC-03-product-acceptance.md#2026-11-22-현행-제품-인수-상태)
- 기존 P07 레인이 새 Linux Source와 SDK wheel을 만들었고 그 뒤 S Facts FULL·SDK 게시 코드가 변경됐다. 현행 서명·제품 수용은 **0/5 NOT_RUN**이다. 같은 attempt의 ACK·G2와 Source CAS 1회, 페이지·seal 응답 유실, Java 첫 DELTA와 warm 0 B가 제품 검증에 남아 있다. [제품 인수 조건](INC-03-product-acceptance.md#2026-11-22-현행-제품-인수-상태)

## 11월 21일 코드 작업과 병렬 합류 계획

착수 당시 목표는 **열린 Source에서 처리하는 한 파일 편집**을 변경 본문, 변경 경로, 실제로 영향받은 인덱스 항목의 비용으로 제한하는 것이었다. 당시 확인한 전수 작업은 세 군데였다. SDK successor가 전체 경로 맵을 복제하고 flat digest를 계산했다. Store의 V1 member CAS 참조는 전체 세대 digest에 묶여 있었다. Q에 보내기 전 검증은 두 세대의 전체 경로를 merge했다. 아래는 당시 역할 배정과 구현 계획이다. 현행 구현·미해결 경계는 [INC-01 코드 단위 현황](INC-01-source-closure.md#코드-단위-현황과-검증)에서 판정한다.

### 채팅 배정 · 11월 21일

아래 여섯 채팅을 quanta-index 프로젝트에 배정했다. Store가 기존 INC-01의 V1 회귀도 맡는다. C1은 Source V2의 공용 계약과 export를, Store는 V1/V2 저장·복원 호출부를 맡는다. SDK는 executor의 준비 코드, E02는 S→Q 게시 코드, INC-03은 제품 테스트와 구간 계측, Q는 Literal 회귀를 각각 소유한다. 검증 결과는 INC-01·02·03에 반영한다.

| 채팅 | 인수할 결과 |
| --- | --- |
| 증분 C1 · Source V2 계약과 루트 | 공용 타입과 변경·FULL 동등성 |
| 증분 Store · 본문 CAS와 세대 복원 | Store 영속 계약, V1 회귀, V2 복원 |
| 증분 SDK · 변경 경로로 다음 세대 생성 | 실제 SDK 호출의 변경 경로 처리 |
| 증분 E02 · 인증된 변경 검증과 Q 게시 | 완전성 검증·Q ACK·재시작 연결 |
| 증분 INC-03 · SDK·Watch 작업량과 수용 | 기존 제품 본문에서 operation별 계측 |
| 증분 Q · Literal 다중 페이지 seal 마감 | 수정한 성공·거절 본문 2개 |

첫 확인에서 S `546dcdde785`의 Source V2 타입이 공유 main에 들어왔다. 당시 Store·SDK·게시의 실제 호출 연결은 진행 중이었다. Store와 E02의 QBC는 artifact 여유 공간 28 GiB대에서 입장 거절로 본문 0건이었다. Q seal 2개는 기존 검증 큐에 제출하고 공유 잠금을 기다렸다.

SDK가 호출하는 `generation_source_closure/closure/original_successor_v11.rs::publish_successor_original_v11`는 V1 전체 identity 배열과 flat digest를 받는다. 이 Runtime publisher의 V2 입구와 실제 게시 연결은 E02가 맡는다. `pinned_query_session_generation_seal_v1.rs`의 버전 결속 계약은 C1이 맡고, SDK는 자기 호출부에서 연결한다. 이 두 접점이 합류해야 실제 SDK 편집에서 전체 배열을 제거할 수 있다.

### 1. 기존 수정 마감과 C1 계약 고정

| 병렬 레인 | 독점 작업 범위 | 완료 기준 |
| --- | --- | --- |
| **INC-01 · 기존 Source 수리** | Source wire와 SDK Store의 기존 회귀를 실행한다. 현행 V1 회귀에서 확인된 결함만 해당 파일의 기존 담당이 수리한다. | 최종 FD/non-FD wire, 실제 SDK 세대 쌍 6개, foreign pin, Native 게시 소비자. rename·삭제·총계·기존 본문 공유 및 target CAS 누락·손상 거절을 확인한다. |
| **INC-02 · Q Literal** | Kernel의 `tests/native_bootstrap/literal_seal.rs`에 이미 있는 수정된 본문 2개를 실행한다. | 성공 시 실제로 바뀐 descriptor가 64개를 넘고 마지막 posting이 조회된다. 두 번째 페이지에서 거절되면 Latest와 기존 pin을 유지한다. |
| **INC-03 / E02 · 기준 측정** | SDK의 `ifx150_restart_recovery_product_owner`와 Watch의 `server_socket_daemon_e2e`를 사용한다. 테스트·harness가 기록을 소유하고 각 생산 코드 오너가 필요한 단계 카운터를 연결한다. | 동일한 한 파일 편집으로 무관한 파일 수와 파일당 크기를 독립적으로 바꾼다. 운영 규모와 동일한 예산에서 successor·persist/open·S→Q 검증·Q stage/ACK·검색 노출 시간을 얻는다. |
| **C1 · Source 계약** | `quanta-runtime-source-authority-wire`에서 Source commitment V2의 도메인·canonical 입력·version·편집 증명의 형태를 고정한다. 기존 `patricia256_v1`의 트리 연산을 사용한다. | 순서가 다른 독립 FULL 구축과 동일한 V2 결과, 변경분 적용과 FULL 결과 일치, stale base·누락·중복·alias 영향 누락 거절. 계약 확정 전까지 실제 writer 전환은 대기한다. |

당시 INC-01과 C1은 V1 수정과 V2 계약 테스트를 서로 다른 파일에서 진행하도록 배정됐다. V1과 V2가 함께 소비하는 부모 모듈·export 수정은 C1 오너가 한 번 합류시키는 계획이었다. 기준 측정은 SDK의 기존 `NativeFreshDeltaEntryV3` 단계와 Watch의 실제 편집 구간을 operation에 결속한다. 여러 실패·rename·재시작을 포함한 테스트 실행 시간은 정확성 검증 원장에 기록한다. E07은 기존 phase trace와 exporter를 사용한다.

### 2. Source V2 코드 수리

아래 작업은 C1 계약 고정 후 병렬로 개발한다. 각 레인은 계약의 기존 fixture 또는 고정 벡터에서 먼저 검증하고, 세 레인이 모두 통과하면 실제 SDK·Watch 호출을 한 번 연결한다.

| 코드 레인 | 파일과 구현 | 독립 검증 |
| --- | --- | --- |
| **C1 · 저장과 복원** | `quanta-sdk-session-backend-owner/durable_normal_source_attribution_generation_store_v1.rs`의 CAS·manifest·index 저장 경로와 `original_full_source_input_v3.rs`의 Store 접점을 맡는다. Source wire 오너가 소유한 `original_generation_source_input_v3/{factory_v3,source_tree_v3}.rs`와 member validator를 연결한다. 본문은 세대 독립 CAS에 저장하고, manifest의 인증된 leaf가 경로·언어·revision·parser identity와 본문을 결속한다. 검증된 base subtree를 공유하고 바뀐 leaf만 저장한다. | 같은 본문이 두 세대에서 같은 객체를 가리킨다. warm 변경 시 무관한 본문 CAS 작업 0 B. cold에서 처음 읽는 객체는 무결성을 검증하며 손상·누락은 원래 사유로 거절한다. 이전 pin·취소·마지막 drop의 수명을 검사한다. |
| **SDK · 변경 입력** | `quanta-sdk-runtime-executor`의 `repo_state/authority/original_source_successor_v11.rs`와 실제 호출부 `facts_public_source_preparation_v11.rs`, `runtime_disk_state_attribution_request_v1.rs`를 맡는다. 이전 Source root와 정규화된 변경 목록으로 바뀐 경로만 조회·갱신한다. | replace·delete·rename·noop의 root·멤버십·토큰 총계를 독립 FULL 값과 대조한다. 같은 편집에서 코퍼스를 키워도 전체 경로 복제·열거가 0회다. |
| **E02 · 게시 검증과 전달** | `quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_source_validation.rs`와 Runtime의 `change_event_aggregate_input_v3.rs` 및 게시 소비부를 맡는다. 출발 root·도착 root·편집 목록이 결속된 경우 전수 merge를 변경 집합 확인으로 대체한다. 같은 제품 경로에 남은 전체 census나 요청 인코딩은 계측으로 범위를 특정한다. | 동일한 파일 전이·Q stage·ACK가 나온다. 빠진 전이, 중복 경로, 다른 세대·저장소, 예산 거절은 Q stage 전에 거절한다. 이벤트 유실로 변경 집합을 확정할 수 없으면 전체 재조정으로 보내고 그 비용을 기록한다. |

C1의 content root와 membership root는 별도 도메인이다. content root의 입력은 canonical path·content digest·provider alias이고, membership은 언어 축까지 포함한다. 세대 manifest가 두 root를 저장소·스냅샷에 결속한다. Q의 event/operation identity는 별도 계약이다. 각 reader는 자기 버전의 독립 FULL oracle로 identity를 검증한다. 이미 검증한 base에서 편집을 적용해 인증된 target과 일치할 때만 증분 게시를 허용한다. S→Q wire 변경이 필요하면 E02 통합 담당이 버전 결속과 소비자 수정까지 맡는다. [기존 C1/C2 설계](/Users/songmin/Documents/code-new/semantica-codegraph-v2/docs/bugbash/oct-10-incremental-engine/final-design-contracts.md)

### 3. 검증과 게시 전환

1. 한 소스 조합에서 허용된 변경만 path-copy했는지 확인한다. warm 한 파일 편집에서 **무관한 본문 CAS 읽기·복사·해시·쓰기 0 B, 전체 경로 열거 0회**를 목표로 삼는다. 직접 방문한 경로와 트리 node를 기록한다. 카운터가 연결되지 않았거나 관측 누락이 있으면 증거로 인정하지 않는다. cold 최초 구축·복원은 별도 결과다.
2. E02 통합 담당이 manifest, pin, SDK replay/cache, Q seal의 version 소비를 맞춘다. 성공한 V1 작업의 replay와 아직 유효한 pin을 보존하면서 미결 작업·GC 경계를 검증한다. sealed checkpoint에서 저장소별 활성 writer 하나를 전환한다. rename은 이전 경로 삭제와 새 경로 추가를 한 전이에 게시한다. Q 게시 후 ACK 유실 및 독립 재시작에서 같은 operation으로 결과를 한 번만 반영한다.
3. E02·NC-F의 기존 본문에서 SDK 및 Watch 편집을 Q ACK와 새 검색 결과까지 검증한다. E07이 기존 4,096/32,768파일·Mixed 조건에서 warm lexical top-20 p95 ≤100 ms/p99 ≤500 ms, 한 파일 staging→ACK→검색 p95 ≤1초/p99 ≤5초 및 기존 RSS·공간 기준을 판정한다. E07의 Text 전용 부하와 INC-02 packed Literal은 각각 해당 경로의 증거로 기록한다. [제품 본문](INC-03-product-acceptance.md) · [E07 기준](../../../plans/oct-22-finalizing/E07-performance-operations.md) · [기존 실행 담당](../../../plans/nov-20-native-cutover-closeout/tickets/07-product-qualification.md)

IDE retained writer, 최초 FULL 구축, V9 memo, ImportGraph, Text·Semantic 및 compaction은 기존 담당이 소유한다. 이 계획에서는 실제 편집 trace에서 해당 경로가 남은 비용을 설명할 때 그 결과를 기존 담당에게 연결한다.

불변 본문과 변경 경로 공유는 Venti에서, 결과가 같을 때 후속 계산을 멈추는 조건은 증분 빌드 연구에서 근거를 얻는다. Quanta의 성능 수치는 위 제품 실행에서 판정한다. [Venti, FAST 2002](https://static.usenix.org/publications/library/proceedings/fast02/quinlan/quinlan_html/index.html) · [Build Systems à la Carte, JFP 2020](https://www.cambridge.org/core/journals/journal-of-functional-programming/article/build-systems-a-la-carte-theory-and-practice/097CE52C750E69BD16B78C318754C7A4)

Q Kernel·Catalog의 변경 적용·원자 게시, RepoMap의 영향 owner 적용과 Semantic의 구조적 diff는 기존 구현을 사용한다. Text의 선택형 segmented Patch, ANN, XL·Mixed 성능 수용은 각각 기존 담당에 남긴다. 전체 제품 마감 상태와 실행 결과는 기존 원장에서 관리한다. [Source 적용](../../../../crates/quanta-index-kernel/src/source.rs#L1064) · [Catalog 게시](../../../../crates/quanta-index-catalog/src/native/authority.rs#L4055) · [RepoMap](../../../../crates/quanta-index-repomap/src/native/graph/affected.rs#L36) · [Semantic](../../../../crates/quanta-index-semantic/src/native/embedding/ann_directory/source_reverse.rs#L250) · [NC-TEXT](../../../plans/nov-12-native-cutover/tickets/NC-TEXT-build-final.md) · [NC-B](../../../plans/nov-12-native-cutover/tickets/NC-B-semantic.md#current-remaining-acceptance) · [전체 에픽](../../../plans/oct-22-finalizing/README.md) · [성능 원장](../../../plans/nov-5-indexing-bugbash/README.md)
