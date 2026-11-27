# P01 · 고정 뷰의 순위 페이지 반복 스캔 제거

우선순위 **P1**. 각 페이지의 파일 순위와 전체 건수 계약을 유지하면서 전체 순회 비용을 낮춘다.

## 현황

**09.4a 계측 `VERIFIED`.** Tree에서 한 페이지 10건으로 끝까지 읽었을 때, 100파일은 총 3,546회, 1,000파일은 총 330,435회 읽었다. 이 기준선은 페이지마다 후보를 다시 평가했다. 아래의 재사용 실행은 페이지당 4건인 별도 fixture에서 검증했다. [페이지 실행](../../../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/page.rs#L39) · [09.4 원장](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#094--고정-뷰-페이지-순회의-반복-비용)

09.4b 별도 작업 트리의 코드 감사에서 첫 페이지의 선택적 순위 수집이 `WorkUsage.bytes`를 강제 차감하는 경로를 확인했다. 수집 활성 상태에서만 bytes 한도를 넘으면 첫 조회가 `ResourceLimited`로 끝날 수 있다. 세션 응답 메모리 예약의 `BackendViolation`을 용량 부족과 함께 삼키는 경로도 있다. 선택적 수집·보관에 실패해도 일반 검색의 결과가 유지되고, 한도 부족 외의 오류는 호출자에게 전달해야 한다. main 작업 트리는 세션 몫으로 직접 수집하며 아래 회귀에서 첫 페이지 work bytes 동등성을 확인했다. [WorkBudget 한도 처리](../../../../crates/quanta-index-core/src/index/budget.rs#L275) · [09.4 재사용 계약](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#094--고정-뷰-페이지-순회의-반복-비용)

A 레인 `2c386d011`의 Lexical lib 실행은 **820 통과·3 실패·1 제외 `FAILED`**였다. 본문 제외 파일의 경로 편집, 경로 필드가 추가된 root JSON, 경로 포맷을 결속한 Tree digest 등 세 fixture를 수정한 main은 Nextest `0c0cf6ff-fdcd-4b6a-a393-425ad6121704`에서 해당 **3/3 `VERIFIED`**다. 별도 P01 순위 재사용 검증은 아래에 기록했다. [경로 편집 회귀](../../../../crates/quanta-index-lexical/src/native/text/tests/map_read_window_tests.rs#L324) · [root 예산 골든](../../../../crates/quanta-index-lexical/src/native/text.rs#L7341) · [Tree digest 골든](../../../../crates/quanta-index-lexical/src/native/text_reader.rs#L1587)

`08fc64167` 격리 후보의 `native_index_uds` `page_reuse` 첫 선택은 공통 빌드 입장 대기 3,600초에서 **exit 124 · 본문 0건**이었다. 두 번째는 입장했으나 호스트 Xcode 라이선스 오류로 빌드 스크립트 링크가 **Cargo exit 101 · 본문 0건**에서 멈췄다. 같은 clean SHA와 선택자에서 설치된 Command Line Tools를 지정해 기존 입장 큐에 한 건 재접수했다. 집중 본문은 **NOT_RUN**이다. 후보의 `text_order_memory_bytes`는 미지정 시 0이며, 회귀는 캐시 활성과 비활성 모두를 비교한다. 합류 전 제품 기본값과 저예산 동등성을 확인한다. [링크 실패](/Users/songmin/Library/Caches/quanta-index/p01-rebase-after-contention.WWgta3KS/nextest.log:7) · [CLT 재접수](/Users/songmin/Library/Caches/quanta-index/p01-rebase-clt.LwqywT5h/nextest.log)

같은 `08fc64167`의 Command Line Tools 재접수는 라이선스 오류를 넘겼다. Runtime의 `OpenGuard::into_held_reader` 미사용 오류로 **컴파일 실패 · 본문 0건**이었다. 메서드는 호출 모듈과 동일한 `legacy-semantic-adapter` 기능에서 컴파일하도록 격리 후보 `55d401e84`에서 수리했다. [실행 로그](/Users/songmin/Library/Caches/quanta-index/p01-rebase-clt.LwqywT5h/nextest.log:486)

제품 설정은 순위 저장 몫을 명시적으로 받을 수 있다. 전체 메모리 여유와 함께 레지스트리 및 순위 저장 몫의 합이 Interactive 한도 안에 있는지 상태 생성 전에 검사한다. 실제 런타임 예약과 같은 검사다. daemon 회귀는 합이 한도를 넘으면 상태·소켓 없이 거절하고, 순위 하나분을 명시하면 부팅 후 UDS로 workspace를 생성하는지 확인한다. 새 본문은 **NOT_RUN**이다. [설정 검사](../../../../crates/quanta-index-searchd-runtime/src/native_product.rs#L1807) · [daemon 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process.rs#L2727)

설정 검사까지 포함한 격리 후보 `ef4c91eba`의 `page_reuse`는 공용 빌드 입장 300초에서 **exit 124 · 본문 0건**으로 종료했다. [종료 로그](/Users/songmin/Library/Caches/quanta-index/p01-rank-share-clt.s8lz3z/page-reuse.log)

후속 후보 `fbc8c9665`는 새 질의의 첫 페이지에서 다른 질의의 캐시를 먼저 비운다. 이전 순위를 사용 중인 조회가 보유한 메모리는 그 조회가 끝날 때 반환하며, 후속 페이지의 캐시 불일치는 다른 질의의 순위를 비우지 않는다. 신규 회귀는 순위 하나만 보유할 수 있는 몫에서 A→B 전환, A의 뒤늦은 이어 읽기, B의 후속 페이지 비용과 캐시를 끈 페이지 바이트를 대조한다. 같은 격리 빌드 위치에서 페이지 회귀와 rank share 부팅 회귀를 순차 접수했다. [후속 실행](/Users/songmin/Library/Caches/quanta-index/p01-replacement-proof-clt.6FHgmo/page-reuse.log)

`fbc8c9665`는 입장 뒤 UDS 테스트 컴파일에서 **exit 101 · 본문 0건**이었다. 이 코호트의 `NativeTextPathPageV1`에는 `code_search` assertion이 요구한 필드와 검사 메서드가 없다. 공용 세션 fixture도 reclaim 설정과 주입한 Text reader를 연결해야 한다. 현행 wire의 점수·경로·owner·Source·cursor·matched 바이트 대조를 수용 조건으로 사용한다. [컴파일 진단](/Users/songmin/Library/Caches/quanta-index/p01-replacement-proof-clt.6FHgmo/page-reuse.log:3)

격리 후속 후보 `d37a8979c`에서 retained page 계약과 공용 fixture를 맞췄다. retained page의 바이트·순위·이어 읽기와 페이지마다 과금된 읽기 수를 대조한다. 같은 빌드 위치의 Nextest `31072ad8-30e2-4d94-aa8f-cbb8dd1d2e3d`에서 `page_reuse` **VERIFIED · 4/4 · exit 0**이었다. A→B 순위 교체, 이전 커서 유지, 페이지 바이트 동등성, 저예산 재순위와 한도 초과 거절을 확인했다. 격리 후보의 제품 rank share 부팅 회귀도 Nextest `07fe5790-5cbf-48d7-8afb-ab8894ee2e2d`에서 **VERIFIED · 1/1 · exit 0**이었다. 한도 초과는 상태·소켓 생성 전에 거절됐고 명시한 한 개분의 몫은 부팅 후 공개 UDS workspace 생성까지 이어졌다. [부팅 실행](/Users/songmin/Library/Caches/quanta-index/p01-wire-fixture-clt.NFU1Ke/product-boot.log) [재실행 로그](/Users/songmin/Library/Caches/quanta-index/p01-wire-fixture-clt.NFU1Ke/page-reuse.log)

main 작업 트리에 첫 페이지의 순위 수집과 질의 교체를 이식했다. 캡처는 세션 시작 시 예약한 몫으로만 과금하고, 보관 시 다시 예약하지 않는다. 현재 main의 CodeSearch excerpt 처리와 직접 세션 API의 기본 예약을 사용한다. main 회귀는 A→B 전환에서 캐시 활성·비활성 첫 페이지의 work bytes까지 대조한다. 정적 검사를 통과했고 통합 본문은 기존 빌드 레인에 접수했다. [페이지 조립](../../../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/page.rs#L53) · [통합 실행](/Users/songmin/Library/Caches/quanta-index/p01-main-integrated-clt.2m2gHt/page-reuse.log)

main 작업 트리의 Nextest `847e05b2-fdf8-4c67-91ba-953bd996d18c`는 **4/4 통과 · exit 0**이었다. A→B 본문은 캐시로 인한 첫 페이지 work bytes 증가가 0임을 검사했다. 고정 뷰 순회 한 건의 Nextest `LEAK` 표시를 단독으로 재실행했다. Nextest `31cd16fa-0d1d-4b15-ab32-84f7590fb378`은 **1/1 · exit 0**이고 표시가 재현되지 않았다. 86파일의 마지막 페이지까지 논리 읽기 합은 owner 8,014→769회, 경로 21,848→2,048회였다. [단독 실행](/Users/songmin/Library/Caches/quanta-index/p01-main-pipe-check-clt.Hhdl4d/nextest.log)

## 작업 범위

- **합류 경계.** main 작업 트리의 페이지 경로는 동일한 view·instance·query·artifact의 순위와 hit 증거를 재사용한다. 세션 release·만료에서 메모리 lease를 반환한다. 낮은 예산에서 선택적 수집이나 예약 거절이 기존 검색을 실패시키지 않는지, 실제 원장 오류는 호출자에게 남는지 확인한다. [main 페이지](../../../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/page.rs#L39) · [09.4b 합류·수용](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#094--고정-뷰-페이지-순회의-반복-비용)
- **합류 검증 순서.** B→A의 09.4b 증분만 현재 브랜치에 맞춰 합친다. A가 건드린 SDK는 Q01에서 고정한 v2 cursor와 renewal을 보존한다. `cached_pages_equal_rescanned_pages_and_cost_only_their_rows`·`an_unfunded_order_re_ranks_every_page_with_identical_results`와 공개 순위 골든을 확인한다. 선택적 보관을 켠 경우에만 작업량 또는 메모리 상한을 넘는 첫 페이지를 독립 fixture로 추가한다. 선택적 보관은 포기해도 필수 검색·정확 건수·이어 읽기는 유지돼야 하며, 용량 거절 외 원장 오류는 전파한다.
- 첫 페이지부터 마지막까지 posting·body 방문량과 유지 메모리를 계측한다. 정확 건수나 순위 계산을 매번 반복하는 지점을 줄이는 설계를 비교한 뒤 변경한다.
- Cursor의 view·instance·query 결속, 동점 순서, 삭제 후 고정 뷰와 만료·해제 동작을 유지한다. I01의 매치 위치 전수 순회는 별도의 결과 단위로 둔다.

## 완료 조건

- [ ] 같은 고정 뷰에서 첫 페이지와 끝까지 순회한 건수가 독립 골든에 일치한다. 동점·재게시·취소·해제와 커서 바꿔 끼우기의 오류를 검증한다.
- [ ] 같은 입력에서 페이지 수를 늘려도 전체 후보 재평가 비용이 페이지 수만큼 반복되지 않는다. 전후 방문량·p95/p99·세션 유지 메모리를 기록한다.
- [ ] 캐시를 끈 첫 페이지가 성공하는 bytes·메모리 상한에서 선택적 순위 수집만 거절할 때 캐시를 켠 결과도 같은 바이트로 성공한다. 컨트롤러 원장 오류는 전파하고, 활성화·비활성화 양쪽의 고정 뷰와 마지막 페이지를 비교한다.

## 의존·인계

현재 고정 뷰 fixture와 단계별 계측을 기존 실행에 사용한다. [Q01](Q01-sdk-codesearch.md)의 파일 결과와 [Q03](Q03-regex-budget.md)의 후보 정책을 인수한다. Q03의 구조 상한과 09.2b의 4,097파일 Tree·Layered 공개 SDK·CLI 본문 2/2는 확인했다. 최종 호스트의 지연·페이지별 작업량과 서명 설치 수용은 남았다. `text_query.rs` 공통 루프 변경은 기존 TEXT·Runtime 세션 오너가 09.4에서 조정하고, [D02](D02-pin-scrub-rebuild.md)에 해제·회수 관측을 전달한다. [09.4](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#094--고정-뷰-페이지-순회의-반복-비용)
