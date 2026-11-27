# S02 · 생산자 한 파일 편집 DELTA 비용

우선순위 **P0 제품 증분 검증**. Semantica Source 발급·Store·SDK·Watch의 쓰기 경로를 소유한다. Q Native Layered 루트·파티션 증폭은 [P02](P02-layered-delta-cost.md) 소관이다.

## 현황

Facts·disk의 staged V4 후보는 두 C1 root를 Source manifest·durable pointer·복구에 전달한다. 현행 SDK는 준비된 C1이 있으면 `issue_ordered_original_from_c1_v2`로 target을 발급한다. 기존 입력은 V1 경로를 쓴다. 실제 잔여는 V1 호환 pin의 전체 투영, Store V4의 준비·HEAD 커밋 전 전체 물리·본문 검증, ItemIndex seed의 유지 파일 재검증이다. S `1386f7ade60`은 변경 본문의 중복 framed digest 계산을 제거했다. ItemIndex 종류별 집계 후보는 격리 브랜치에 있다. 변경 Source만 준비하는 일부 코드와 Wire 경로 작업량 회귀 **18/18**은 기존 동결 범위에서 확인됐지만 Store·SDK의 후속 소유 본문은 컴파일 실패 또는 미실행이며, 현행 설치 SDK·Watch warm 비용과 독립 FULL parity는 `NOT_RUN`이다. [현행 SDK V2 분기](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/repo_state/authority/original_source_successor_v11.rs:542) · [현재 생산자 코드와 소유 본문](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-25) · [Store 전수 검사](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-session-backend-owner/src/durable_normal_source_attribution_generation_store_v1/source_tree_manifest_v4.rs:3658) · [설치 수용](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#현행-판정--2026-11-25)

## 작업 범위

- 연결된 V2 발급의 정확한 predecessor·변경 행과 Source CAS 뒤의 committed manifest·seal을 검증한다. 준비·재시작·ACK 유실에서 미게시 target을 공개 Source head로 오인하지 않는다. Store의 무변경 CAS 대여는 불변 custody를 증명할 때만 전체 검증을 건너뛴다. [기존 단계별 소유](../../nov-23-vendor-retirement/05-integration-and-release.md#warm-delta)
- Store V4의 전체 preflight·이전 세대 혼합 tree 재구성·inventory 전체 참조 검사, ItemIndex의 유지 경로 열거를 변경 경로 기반으로 바꾼다. 독립 FULL identity와 손상·foreign holder 거절은 유지한다. [남은 구현 경계](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-25)
- 동일 operation의 Runtime 발급·pin, Store 준비/커밋, ItemIndex, E02 게시, Q submit/ACK, Watch Latest를 단계별로 계측한다. 기존 Store 관측과 별도로 Runtime·ItemIndex·Q ACK까지 전체 경로 방문과 무관 본문 read/copy/hash/write, 변경 본문 읽기, WAL·sync·지연을 기록한다. 관측 누락은 0으로 간주하지 않는다. [INC-01 계측](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-25) · [INC-03 비용 입력](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#한-파일-편집-비용-측정)
- 무관 파일 **1개 × 256 B**, **32개 × 256 B**, **1개 × 16,384 B**의 세 행을 각각 독립 FULL 기준점에서 시작해 같은 한 파일을 편집한다. 경로 수와 파일 크기 영향을 따로 판정한다. Q 객체 재사용 wire 검증과 생산자 Source 본문 재검사는 별도 비용으로 기록한다.

## 수리 절차

1. **연결된 V2 발급의 방문 비용과 게시를 검증한다.** 현행 Source/Cargo와 기존 QBC Wire·Store·SDK 소비자를 같은 후보로 결속한다. Facts·disk 양쪽에서 정확한 predecessor와 변경 행으로 준비한 V2 target이 실제 Source CAS·committed holder·복구까지 이어져야 한다. 기존 V1 데이터의 조회·재생, 다른 holder, 오래된 predecessor, 준비 중 종료·커밋 후 ACK 유실을 검증한다. [INC-01 발급 경계](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-25)
2. **Store의 warm 전수 검증을 줄인다.** V2 holder의 불변 객체 주소·바이트와 세대 보존 권위를 증명한 경우 변경 경로에만 물리 대조와 inventory 갱신을 건다. 상속 객체의 권위를 복구할 수 없는 경우 기존 검증 경로를 유지하고 별도 케이스로 기록한다. 혼합 포맷 재개방, foreign holder·상속 객체 손상과 동시에 GC가 도는 경우를 기존 Store 회귀로 확인한다.
3. **ItemIndex를 변경 경로로 합성한다.** 이전 ItemIndex에 영향·변경·삭제 경로만 투영한다. import/rename으로 영향받는 유지 경로는 재검증하며, QE가 재사용 권위를 낸 유지 결과만 인수한다. 독립 FULL과 경로·Source·공개 결과를 맞춘 뒤 전수 열거 제거를 Work 카운터로 확인한다.
4. **세 제품 행을 기존 선택자로 실행한다.** SDK 비용 `ifx150-source-v2-single-edit-cost-product-owner`, Watch 비용 `p60-native-watch-warm-zero-full-tree-walk-owner-v11`, 독립 FULL·재시작 `p60-native-completed-watch-process-owner-v11`의 현재 담당 큐를 재사용한다. 세 무관 파일 행 모두에서 변경 본문 1건, 무관 본문 I/O·해시 0 B, Store 검증·경로 열거 0회를 같은 operation으로 확인한다. [소유 선택자와 계측](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#한-파일-편집-비용-측정)

## 완료 조건

- [x] **코드 연결 범위.** Facts·disk의 준비된 C1 target을 Source manifest·durable pointer·복구에 연결했다. [구현 경계](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-25)
- [ ] 설치 SDK와 Watch 각각에서 FULL→한 파일 교체·추가·삭제·rename·no-op→DELTA→재시작을 수행한다. 새 Source·C1·공개 검색 결과가 같은 입력의 독립 FULL과 일치하고 이전 뷰도 정확히 읽힌다. [INC-03 제품 본문](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#현행-판정--2026-11-25)
- [ ] 세 비용 행 모두에서 warm 전체 경로 열거 **0회**, Store 전체 본문 검증 **0회**, 무관 본문 read/copy/hash/write 각각 **0 B**, 변경 본문 읽기 **1건**을 동일 operation으로 입증한다. 기존 소스에서 남은 전체 검증을 제거하지 못하면 미충족으로 기록한다. cold 최초 구축·복원은 별도 분모다.
- [ ] 응답 유실과 재시작에서 같은 attempt·operation·root·Source cut·V9 ACK를 복구하고 Source CAS와 Q 논리 Submit의 중복을 허용하지 않는다. 현행 signed 제품 본문이 없으면 `NOT_RUN`을 유지한다. [기존 절단·멱등성](../../nov-23-vendor-retirement/05-integration-and-release.md#warm-delta)

## 의존·인계

[INC-01](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md)·[INC-03](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md)의 기존 Semantica/E02/NC-F owner와 exact 본문을 재사용한다. [S01](S01-corpus-reconciliation.md)의 완전 코퍼스·Source cut을 입력받는다. Q Native 내부 write/read amplification 수치와 개선은 [P02](P02-layered-delta-cost.md)에 인계하며, 동일 서명 설치본의 영속 장애 절단은 [D01](D01-durable-recovery.md)에 인계한다.
