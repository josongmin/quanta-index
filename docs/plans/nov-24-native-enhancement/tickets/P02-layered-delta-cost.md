# P02 · Native Layered DELTA 쓰기 증폭

우선순위 **P1 계측·병목 확인**, 비용 개선은 실측으로 위치를 정한다. Source 생산자의 전수 검증은 S02 소관이다.

## 현황

일반 SourceAdvance의 canonical 빈 mutation은 Text `plan`이 세 레이아웃 모두 `Reuse`로 처리해 `prepare`·builder를 건너뛴다. Kernel·SQLite·FileObjectStore의 core no-op 집중 회귀는 이전 pin의 BM25·Literal·FileMap 결과와 멤버 객체 접근 0을 확인했다. 직접 `prepare`/Layered builder에 빈 patch를 넣는 경로의 물리 I/O 수용은 별개다. 비어 있지 않은 DELTA는 이전 루트 행 전체를 읽고 새 루트를 인코딩하며, 조건에 따라 파티션을 압축 또는 분할한다. 관리형 객체 재사용에도 기존 wire 검증 I/O가 발생할 수 있다. [plan 재사용](../../../../crates/quanta-index-lexical/src/native/text.rs#L14152) · [루트 읽기](../../../../crates/quanta-index-lexical/src/native/text_segment/build_v2.rs#L104) · [루트 쓰기](../../../../crates/quanta-index-lexical/src/native/text_segment/build_v2.rs#L484) · [기존 객체 검증](../../../../crates/quanta-index-storage/src/object.rs#L1645) · [로컬 no-op 결과](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#2026-11-24-보강-계획에서-추가한-케이스)

LayeredV2는 출력 루트에 최대 4,096행을 먼저 예약하던 경로를 제거했다. FULL은 봉인된 segment 수만큼, DELTA는 기존 파티션 수와 변경 건수로 계산한 분할 상한만큼 예약한다. 루트·경로를 저장하기 전에 이전 행과 segment 임시 버퍼를 반환한다. 파티션 행 상한 모델은 **29,537개 사례 통과**. 4 KiB 메모리로 빈 루트를 준비하고 메모리를 반환하는 회귀와 기존 두 segment의 오버레이·regex·이어 읽기는 Nextest `0c0cf6ff-fdcd-4b6a-a393-425ad6121704`에서 **2/2 `VERIFIED`**. 비어 있지 않은 DELTA의 설치본 물리 I/O는 `NOT_RUN`이다. [루트 메모리 예약](../../../../crates/quanta-index-lexical/src/native/text_segment/build_v2.rs#L561) · [저메모리 회귀](../../../../crates/quanta-index-lexical/src/native/text.rs#L1913)

## 작업 범위

- 로컬 제품 프로세스의 Layered FULL→PATCH·고정 뷰·재시작과 16개 레이어 compaction은 Nextest `be7bdfd8-8884-456d-9316-74656122f712`에서 **2/2 `VERIFIED`**다. 서명 설치본의 물리 I/O와 pin 해제 뒤 회수는 열린 상태다. [FULL·PATCH·재시작](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/segmented_layered.rs#L627) · [16개 레이어](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/segmented_layered.rs#L1212) · [재실행 로그](/Users/songmin/Library/Caches/quanta-index/native-layered-delta-reopen.cqLlrL/nextest.log)
- Native build의 루트·파티션·posting·성공한 객체 reuse·압축 비용을 나눈다. 기존 계측으로 읽기·쓰기 바이트, WAL, sync, 피크 디스크와 게시 지연을 함께 기록한다.
- 1·100·1,000파일 변경과 반복 단일 편집, 동일 본문 rename, compaction 경계를 비교한다. 정상 `plan` no-op과 직접 `prepare`/Layered builder 빈 patch를 분리한다. Tree 직접 `prepare`의 루트 읽기와 Layered 빈 builder의 전체 루트 처리도 계측한다. 고정 변경량에서 파티션 수를 키운다. 집계는 S02의 생산자 Source I/O와 구별한다. [직접 빈 patch 잔여](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#2026-11-24-보강-계획에서-추가한-케이스)

## 완료 조건

- [x] **로컬 정상 no-op 범위:** canonical 빈 mutation의 `Reuse`와 core 객체 접근 0, 이전 pin의 결과 동등성을 집중 회귀로 확인했다. 직접 builder 입력의 물리 0 B나 설치 SDK·Watch 비용은 포함하지 않는다. [원 회귀](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#2026-11-24-보강-계획에서-추가한-케이스)
- [ ] 직접 빈 patch와 정상 제품 경로의 Text 루트·파티션·객체 read/write/sync 바이트를 각각 측정한다. 빈 patch가 정상 경로로 들어오면 `plan` 재사용이 실제로 적용되는지도 확인한다.
- [ ] 루트 크기·레이어 수·재사용 수에 따른 비용을 실제 실행으로 제시하고 지배 경로를 확인한다. 변경량에 비례한다는 주장은 해당 측정이 있는 범위에서만 한다.
- [ ] 수리 후 독립 FULL과 Source·점수·순위가 동일하고 고정 뷰, 재시작, 기존 compaction·회수 계약이 통과한다. [기존 제품 검증](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md)

## 의존·인계

TEXT의 layered build와 전용 계측을 소유한다. [S02](S02-single-edit-delta.md)와 데이터 준비는 병렬 가능하다. Object/Catalog 영속화 변경이 필요하면 [D01](D01-durable-recovery.md)이 해당 파일을 소유한다. 동등 기능 비교 입력과 단계를 [V02](V02-zoekt-comparison.md)에 인계한다.
