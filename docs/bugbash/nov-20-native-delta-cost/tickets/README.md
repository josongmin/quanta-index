# Native 증분 갱신 비용 · 2026-11-20

| 티켓 | 코드와 확인된 결과 | 남은 수용 조건 |
| --- | --- | --- |
| [INC-01 · Semantica Source](INC-01-source-closure.md) | 게시 소비자는 열린 세대의 토큰 총계를 읽는다. 같은 경로의 본문을 재사용할 때 base 트리를 한 번 조회한다. CAS 참조가 바뀌면 target 객체를 검증하고 기존 본문·토큰 수를 승계한다. | 직전 SDK 세대 쌍은 3/5. 최종 수정 후 SDK 6개, foreign pin 거절, Native 게시 소비자 및 두 feature 구성의 확인이 남았다. |
| [INC-02 · Q Literal](INC-02-literal-directory.md) | 정식 nextest 9개 통과. 수정한 다중 페이지 성공·거절 두 본문도 정식 재실행에서 2/2 통과했다. | 설치 제품 검증은 INC-03, 최종 성능 수용은 E07에서 각각 관리한다. |
| [INC-03 · 제품 검증](INC-03-product-acceptance.md) | 기존 E02·NC-F 실행 결과를 인수한다. | 실제 SDK·Watch·Q 결과와 현재 후보를 대조한다. 제품 수용은 `NOT_RUN`이다. |

이 세 티켓이 이번 변경의 작업 목록이다. 각 티켓에 구현 위치와 해당 검증 결과를 기록한다. 전체 증분 시간과 XL 성능의 최종 판정은 E07에서 관리한다.

## 코드 작업과 병렬 합류

목표는 **열린 Source에서 처리하는 한 파일 편집**을 변경 본문, 변경 경로, 실제로 영향받은 인덱스 항목의 비용으로 제한하는 것이다. 확인된 전수 작업은 세 군데다. SDK successor가 전체 경로 맵을 복제하고 flat digest를 계산한다. Store의 V1 member CAS 참조는 전체 세대 digest에 묶여 있다. Q에 보내기 전 검증은 두 세대의 전체 경로를 merge한다. 현재 본문 공유와 O(1) 토큰 총계는 제품 경로에서 재사용한다. [Source 비용과 호출 경로](INC-01-source-closure.md)

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

첫 확인에서 S `546dcdde785`의 Source V2 타입이 공유 main에 들어왔다. Store·SDK·게시의 실제 호출 연결은 진행 중이다. Store와 E02의 QBC는 artifact 여유 공간 28 GiB대에서 입장 거절로 본문 0건이다. Q seal 2개는 기존 검증 큐에 제출했고 공유 잠금을 기다린다.

SDK가 호출하는 `generation_source_closure/closure/original_successor_v11.rs::publish_successor_original_v11`는 V1 전체 identity 배열과 flat digest를 받는다. 이 Runtime publisher의 V2 입구와 실제 게시 연결은 E02가 맡는다. `pinned_query_session_generation_seal_v1.rs`의 버전 결속 계약은 C1이 맡고, SDK는 자기 호출부에서 연결한다. 이 두 접점이 합류해야 실제 SDK 편집에서 전체 배열을 제거할 수 있다.

### 1. 기존 수정 마감과 C1 계약 고정

| 병렬 레인 | 독점 작업 범위 | 완료 기준 |
| --- | --- | --- |
| **INC-01 · 기존 Source 수리** | Source wire와 SDK Store의 기존 회귀를 실행한다. 현행 V1 회귀에서 확인된 결함만 해당 파일의 기존 담당이 수리한다. | 최종 FD/non-FD wire, 실제 SDK 세대 쌍 6개, foreign pin, Native 게시 소비자. rename·삭제·총계·기존 본문 공유 및 target CAS 누락·손상 거절을 확인한다. |
| **INC-02 · Q Literal** | Kernel의 `tests/native_bootstrap/literal_seal.rs`에 이미 있는 수정된 본문 2개를 실행한다. | 성공 시 실제로 바뀐 descriptor가 64개를 넘고 마지막 posting이 조회된다. 두 번째 페이지에서 거절되면 Latest와 기존 pin을 유지한다. |
| **INC-03 / E02 · 기준 측정** | SDK의 `ifx150_restart_recovery_product_owner`와 Watch의 `server_socket_daemon_e2e`를 사용한다. 테스트·harness가 기록을 소유하고 각 생산 코드 오너가 필요한 단계 카운터를 연결한다. | 동일한 한 파일 편집으로 무관한 파일 수와 파일당 크기를 독립적으로 바꾼다. 운영 규모와 동일한 예산에서 successor·persist/open·S→Q 검증·Q stage/ACK·검색 노출 시간을 얻는다. |
| **C1 · Source 계약** | `quanta-runtime-source-authority-wire`에서 Source commitment V2의 도메인·canonical 입력·version·편집 증명의 형태를 고정한다. 기존 `patricia256_v1`의 트리 연산을 사용한다. | 순서가 다른 독립 FULL 구축과 동일한 V2 결과, 변경분 적용과 FULL 결과 일치, stale base·누락·중복·alias 영향 누락 거절. 계약 확정 전까지 실제 writer 전환은 대기한다. |

INC-01과 C1은 현재 V1 수정과 V2 계약 테스트를 서로 다른 파일에서 진행한다. V1과 V2가 함께 소비하는 부모 모듈·export 수정은 C1 오너가 한 번 합류시킨다. 기준 측정은 SDK의 기존 `NativeFreshDeltaEntryV3` 단계와 Watch의 실제 편집 구간을 operation에 결속한다. 여러 실패·rename·재시작을 포함한 테스트 실행 시간은 정확성 검증 원장에 기록한다. E07은 기존 phase trace와 exporter를 사용한다.

### 2. Source V2 코드 수리

아래 작업은 C1 계약 고정 후 병렬로 개발한다. 각 레인은 계약의 기존 fixture 또는 고정 벡터에서 먼저 검증하고, 세 레인이 모두 통과하면 실제 SDK·Watch 호출을 한 번 연결한다.

| 코드 레인 | 파일과 구현 | 독립 검증 |
| --- | --- | --- |
| **C1 · 저장과 복원** | `quanta-sdk-session-backend-owner/durable_normal_source_attribution_generation_store_v1.rs`의 CAS·manifest·index 저장 경로와 `original_full_source_input_v3.rs`의 Store 접점을 맡는다. Source wire 오너가 소유한 `original_generation_source_input_v3/{factory_v3,source_tree_v3}.rs`와 member validator를 연결한다. 본문은 세대 독립 CAS에 저장하고, manifest의 인증된 leaf가 경로·언어·revision·parser identity와 본문을 결속한다. 검증된 base subtree를 공유하고 바뀐 leaf만 저장한다. | 같은 본문이 두 세대에서 같은 객체를 가리킨다. warm 변경 시 무관한 본문 CAS 작업 0 B. cold에서 처음 읽는 객체는 무결성을 검증하며 손상·누락은 원래 사유로 거절한다. 이전 pin·취소·마지막 drop의 수명을 검사한다. |
| **SDK · 변경 입력** | `quanta-sdk-runtime-executor`의 `repo_state/authority/original_source_successor_v11.rs`와 실제 호출부 `facts_public_source_preparation_v11.rs`, `runtime_disk_state_attribution_request_v1.rs`를 맡는다. 이전 Source root와 정규화된 변경 목록으로 바뀐 경로만 조회·갱신한다. | replace·delete·rename·noop의 root·멤버십·토큰 총계를 독립 FULL 값과 대조한다. 같은 편집에서 코퍼스를 키워도 전체 경로 복제·열거가 0회다. |
| **E02 · 게시 검증과 전달** | `quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_source_validation.rs`와 Runtime의 `change_event_aggregate_input_v3.rs` 및 게시 소비부를 맡는다. 출발 root·도착 root·편집 목록이 결속된 경우 전수 merge를 변경 집합 확인으로 대체한다. 같은 제품 경로에 남은 전체 census나 요청 인코딩은 계측으로 범위를 특정한다. | 동일한 파일 전이·Q stage·ACK가 나온다. 빠진 전이, 중복 경로, 다른 세대·저장소, 예산 거절은 Q stage 전에 거절한다. 이벤트 유실로 변경 집합을 확정할 수 없으면 전체 재조정으로 보내고 그 비용을 기록한다. |

C1의 content root와 membership root는 별도 도메인이다. content root의 입력은 canonical path·content digest·provider alias이고, membership은 언어 축까지 포함한다. 세대 manifest가 두 root를 저장소·스냅샷에 결속한다. Q의 event/operation identity는 별도 계약이다. 각 reader는 자기 버전의 독립 FULL oracle로 identity를 검증한다. 이미 검증한 base에서 편집을 적용해 인증된 target과 일치할 때만 증분 게시를 허용한다. S→Q wire 변경이 필요하면 E02 통합 담당이 버전 결속과 소비자 수정까지 맡는다. [기존 C1/C2 설계](../../../../../semantica-codegraph-v2/docs/bugbash/oct-10-incremental-engine/final-design-contracts.md)

### 3. 검증과 게시 전환

1. 한 소스 조합에서 허용된 변경만 path-copy했는지 확인한다. warm 한 파일 편집에서 **무관한 본문 CAS 읽기·복사·해시·쓰기 0 B, 전체 경로 열거 0회**를 목표로 삼는다. 직접 방문한 경로와 트리 node를 기록한다. 카운터가 연결되지 않았거나 관측 누락이 있으면 증거로 인정하지 않는다. cold 최초 구축·복원은 별도 결과다.
2. E02 통합 담당이 manifest, pin, SDK replay/cache, Q seal의 version 소비를 맞춘다. 성공한 V1 작업의 replay와 아직 유효한 pin을 보존하면서 미결 작업·GC 경계를 검증한다. sealed checkpoint에서 저장소별 활성 writer 하나를 전환한다. rename은 이전 경로 삭제와 새 경로 추가를 한 전이에 게시한다. Q 게시 후 ACK 유실 및 독립 재시작에서 같은 operation으로 결과를 한 번만 반영한다.
3. E02·NC-F의 기존 본문에서 SDK 및 Watch 편집을 Q ACK와 새 검색 결과까지 검증한다. E07이 기존 4,096/32,768파일·Mixed 조건에서 warm lexical top-20 p95 ≤100 ms/p99 ≤500 ms, 한 파일 staging→ACK→검색 p95 ≤1초/p99 ≤5초 및 기존 RSS·공간 기준을 판정한다. E07의 Text 전용 부하와 INC-02 packed Literal은 각각 해당 경로의 증거로 기록한다. [제품 본문](INC-03-product-acceptance.md) · [E07 기준](../../../plans/oct-22-finalizing/E07-performance-operations.md) · [기존 실행 담당](../../../plans/nov-20-native-cutover-closeout/tickets/07-product-qualification.md)

IDE retained writer, 최초 FULL 구축, V9 memo, ImportGraph, Text·Semantic 및 compaction은 기존 담당이 소유한다. 이 계획에서는 실제 편집 trace에서 해당 경로가 남은 비용을 설명할 때 그 결과를 기존 담당에게 연결한다.

불변 본문과 변경 경로 공유는 Venti에서, 결과가 같을 때 후속 계산을 멈추는 조건은 증분 빌드 연구에서 근거를 얻는다. Quanta의 성능 수치는 위 제품 실행에서 판정한다. [Venti, FAST 2002](https://static.usenix.org/publications/library/proceedings/fast02/quinlan/quinlan_html/index.html) · [Build Systems à la Carte, JFP 2020](https://www.cambridge.org/core/journals/journal-of-functional-programming/article/build-systems-a-la-carte-theory-and-practice/097CE52C750E69BD16B78C318754C7A4)

Q Kernel·Catalog의 변경 적용·원자 게시, RepoMap의 영향 owner 적용과 Semantic의 구조적 diff는 기존 구현을 사용한다. Text의 선택형 segmented Patch, ANN, XL·Mixed 성능 수용은 각각 기존 담당에 남긴다. 전체 제품 마감 상태와 실행 결과는 기존 원장에서 관리한다. [Source 적용](../../../../crates/quanta-index-kernel/src/source.rs#L1064) · [Catalog 게시](../../../../crates/quanta-index-catalog/src/native/authority.rs#L4055) · [RepoMap](../../../../crates/quanta-index-repomap/src/native/graph/affected.rs#L36) · [Semantic](../../../../crates/quanta-index-semantic/src/native/embedding/ann_directory/source_reverse.rs#L250) · [NC-TEXT](../../../plans/nov-12-native-cutover/tickets/NC-TEXT-build-final.md) · [NC-B](../../../plans/nov-12-native-cutover/tickets/NC-B-semantic.md#current-remaining-acceptance) · [전체 에픽](../../../plans/oct-22-finalizing/README.md) · [성능 원장](../../../plans/nov-5-indexing-bugbash/README.md)
