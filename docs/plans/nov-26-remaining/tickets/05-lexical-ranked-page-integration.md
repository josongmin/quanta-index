# 05 · Lexical 페이지 순위 재사용 합류

**담당과 상태.** 기존 P01·TEXT Runtime 세션 담당. Q `28784da26`의 HEAD는 `page.rs`에서 요청마다 계획·순위를 계산한다. 순위 재사용 및 메모리 몫 설정은 작업 트리에 있고 `page_reuse.rs`는 추적되지 않는다. 기존 통합 작업 트리의 `page_reuse`는 **4/4 `VERIFIED`**다. 한 본문의 Nextest `LEAK` 표시는 같은 본문 단독 1/1에서 재현되지 않았다. 합류 후보의 본문과 1,000파일 비용 수용은 별도로 남는다. [P01 현황과 영수증](../../nov-24-native-enhancement/tickets/P01-ranked-pagination.md) · [09.4 기준선](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#094--고정-뷰-페이지-순회의-반복-비용)

**인계 범위.** 기존 P01의 구현과 로컬 실행을 인수한다. 공유 `NativeReadSessions`의 종료 기록과 Open/Release 재생은 Q01 수리와 로컬 결과를 인수하고 Root/F가 후보에 합친다. P01 담당은 Release·만료·회수에서 순위 몫이 한 번 반환되는지 확인한다. 제품의 `text_order_memory_bytes` 미지정 값은 0이다. 양수로 설정한 재사용 성능과 0인 기본 경로를 구분해 측정한다. [Q01 수리와 검증](../../nov-24-native-enhancement/tickets/Q01-sdk-codesearch.md#후속-수리와-로컬-검증) · [제품 메모리 설정](../../../../crates/quanta-index-searchd-runtime/src/native_product.rs#L1551)

## 합류할 변경

1. 현행 작업 트리에서 기존 재사용 코드와 테스트를 골라 현재 후보에 맞춘다. 첫 페이지에서 페이지 결과와 같은 스캔으로 순위·정확 건수를 계산하고, 후속 페이지는 보유한 순위 경계로 이어서 읽는다. 선택적 수집 때문에 첫 페이지의 필수 검색 비용이나 결과가 바뀌지 않게 한다. [페이지 실행](../../../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/page.rs) · [UDS 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/page_reuse.rs)
2. 캐시 키를 보유 세션의 view·instance·query 및 artifact에 결속한다. 동점 순서와 `matched`를 보존하고, 새 질의 B가 캐시를 교체한 뒤 질의 A의 기존 커서가 같은 결과를 내면서 B의 캐시를 비우지 않는지 확인한다. 페이지 생성 전에 불일치 커서를 거절한다.
3. 세션에 예약한 메모리만 사용하고 release·expiry 때 반환한다. 선택적 보관의 정책상 예산 거절은 기존 재순위 경로로 이어가며 원장 오류는 호출자에게 전달한다. 제품 몫은 기존 `text_order_memory_bytes`에서 받고, 허용된 Interactive 한도를 넘는 설정은 부팅 전에 거절한다. [제품 설정](../../../../crates/quanta-index-searchd-runtime/src/native_product.rs)

## 닫는 조건

- [ ] 기존 `native_index_uds`의 `page_reuse`와 boot rank share 본문을 합류 SHA에서 확인한다. 저예산 fallback, 캐시 교체, release·expiry, 커서 교환 거절, 원장 오류 전파 중 이미 통과한 동일 입력의 결과는 기존 원장에서 인수한다.
- [ ] 공개 SDK v2 커서로 동점·재게시가 포함된 고정 뷰의 끝까지 읽고 캐시 켜기·끄기 각 페이지의 순위·점수·건수·excerpt를 비교한다. 독립 골든은 기존 09.1b를 쓴다.
- [ ] 1,000파일 `page_cost`로 첫 페이지와 전체 순회의 posting·본문 방문, 예약 메모리와 응답 지연을 비교한다. 설정된 보관 몫 안의 후속 페이지에서 전체 후보 스캔이 반복되지 않는 것을 확인한다. 기존 P01 및 07 원장에 같은 후보·입력·결과를 연결한다.
