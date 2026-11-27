# D01 · 영속 게시와 설치 복구

우선순위 **P0**. 객체 설치 → Catalog 게시 → SDK 영수증 → 서명 설치본 복구의 단일 경계를 소유한다. Pin 정산과 손상 감사·재빌드는 [D02](D02-pin-scrub-rebuild.md) 소관이다.

## 현황

Native는 객체 파일·부모 디렉터리의 설치를 동기화한 뒤 Catalog에서 객체와 뷰를 확정한다. Catalog는 SQLite `WAL`·`FULL`이고 SDK에는 불확실한 응답을 원 operation으로 조회하는 경로가 있다. 기존 Q 코호트에서 첫 mutation page의 Catalog/객체 재오픈 **2/2**, 부분 ACK 프레임 후 동일 operation 재시도 **1/1**은 통과했다. 현행 Native Q·signed SQL worker의 guest/installed 영수증은 없고 설치 SDK·Watch, 03.6 rollback·restore-forward 및 전원 차단은 `NOT_RUN`이다. [객체 설치](../../../../crates/quanta-index-storage/src/object.rs#L1339) · [Catalog 게시](../../../../crates/quanta-index-catalog/src/native/authority.rs#L4816) · [SDK 상태 조회](../../../../crates/quanta-index-sdk/src/native_uds.rs#L1951) · [기존 Q 회귀와 설치 상태](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#2026-11-24-설치-입력과-증분-수용)

## 작업 범위

- **staged Source·History 게시.** staged receipt의 History profile은 목표 profile이다. Catalog는 현재 view·Source를 기존 전제와 비교하고, 확인된 ingress·History root와 제출 요청을 결속한다. Catalog의 stale fence·Source cut 집중 회귀 **2/2 `VERIFIED`**. 로컬 Native SDK의 preview→stage→submit→조회도 Nextest `10f38bae-8f07-4ebf-b182-c884146dd265`에서 **1/1 `VERIFIED`**다. 다른 profile을 선택하고 잘못된 ingress·뷰를 거절하며 저장된 Source·History와 SDK 메모리 반환을 확인했다. [Catalog 확인](../../../../crates/quanta-index-catalog/src/native/authority/source_stage_replay_receipt.rs#L335) · [SDK 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/history.rs#L775)
- **프로세스 강제 종료 회귀.** Nextest `2551dbb4-92c8-45fa-9a10-dffd56a01292`에서 staged Source·History 게시 후 ACK를 읽기 전에 프로세스를 종료하고, 재시작·동일 요청 재생·변경 profile을 검증한 로컬 UDS 본문 **1/1 `VERIFIED`**다. 최초 실패는 준비 마커가 state root에 들어간 fixture 오류였다. 뒤이어 restore-forward, Semantic import와 Literal 창 타입의 빌드 오류를 거쳤다. 이 결과는 SHA가 고정된 로컬 Legacy 바이너리를 사용했다. 서명 설치와 실제 전원 손실은 `NOT_RUN`이다. [강제 종료 fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/history/staged_replay.rs#L30) · [본문 결과](/Users/songmin/Library/Caches/quanta-index/native-public-history-v7-linked.g1Mu242b/nextest.log)
- 동일 operation에서 객체 예약·파일/디렉터리 sync·설치 확인·Catalog commit·ACK를 추적하고 각 경계의 실패 결과를 old/new 완전 뷰와 영수증에 매핑한다. ENOSPC, short write, 파일·디렉터리 fsync 실패, Catalog 실패, 게시 전후 프로세스 종료와 ACK 유실을 주입한다.
- 서명 후보의 SDK·Watch에서 FULL→DELTA→Latest/Exact 조회→새 프로세스 재오픈을 수행하고 독립 FULL의 파일·Source 해시 및 검색 점수·순위에 맞춘다. page ACK·seal 응답 유실은 원 operation·attempt·fence·root·count·Source cut을 확인해 같은 영수증으로 정산한다. [기존 다세대 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/text_shard_contract.rs#L380) · [기존 페이지 절단 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/text_shard_contract.rs#L1209)
- 실제 배포 전환에서 첫 durable commit 이전 rollback과 그 이후 restore-forward·outbox 재생을 기존 운영 계약으로 검증한다. `SIGKILL`은 프로세스 종료 복구로 보고, 전원 손실 주장은 설치 파일시스템·스토리지와 별도 전원 차단 실험에 결속한다. [서명 설치 원장](../../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md#완료-조건) · [활성화·복귀 게이트](../../nov-20-native-cutover-closeout/tickets/08-release-activation.md#08--격리-후보-t-native-통합-ab와-실제-활성화)

## 완료 조건

- [x] **기존 Q 로컬 동결 범위:** 128건 첫 page를 재오픈해 같은 operation·새 fence의 root 재개와 다른 operation 거절을 검증했다. 별도 SDK 회귀는 잘린 ACK의 원 key 재시도를 검증했다. 서명 설치본의 Q 프로세스 재시작·S Source CAS까지 포함하지 않는다. [원 실행 범위](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#2026-11-24-설치-입력과-증분-수용)
- [x] **객체 디렉터리 동기화 집중 회귀:** 디렉터리 열기에서 심볼릭 링크를 따라가지 않고, 열린 디렉터리의 장치·inode를 동기화 전후의 경로와 대조한다. Nextest `4129668d-06c8-4389-8510-5993ed131410`의 `test(directory_sync)` **4/4 `VERIFIED`**. 범위는 저장소 단위 테스트다. [동기화 코드](../../../../crates/quanta-index-storage/src/object.rs#L2413) · [경로 교체 회귀](../../../../crates/quanta-index-storage/src/object.rs#L3392)
- [ ] 각 장애 지점에서 조회 결과가 완전한 이전 또는 새 뷰이고, 부분 객체·혼합 뷰·잘못된 ACK가 공개되지 않는다. 성공 ACK 뒤 새 프로세스에서 같은 뷰와 내용이 조회된다.
- [ ] ACK 유실 재전송은 원 영수증에 정산되며 추가 Source CAS·논리 Q 게시가 **0회**다. Source cut·operation·V9 ACK와 실제 Latest를 같은 서명 설치본에 결속한다.
- [ ] 기존 [03 서명 제품](../../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md)과 [08 배포/복구](../../nov-20-native-cutover-closeout/tickets/08-release-activation.md)의 승인 입력·독립 관측·복귀 게이트를 통과한다. 로컬 회귀, 준비된 입력, 서명되지 않은 ELF는 출하 증거로 승격하지 않는다.

## 의존·인계

[S01](S01-corpus-reconciliation.md)의 코퍼스·Source cut과 [S02](S02-single-edit-delta.md)의 동일 operation DELTA를 입력받는다. Source/Watch 제품 절단은 [INC-03](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md)의 기존 owner와 실행한다. [03](../../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md)·[08](../../nov-20-native-cutover-closeout/tickets/08-release-activation.md)이 서명 공급·배포 판정의 권위다. 객체/Catalog·SDK 게시 오류 수리를 이 티켓에 모으고, 보존/GC와 scrub·재빌드 수용은 [D02](D02-pin-scrub-rebuild.md)에 인계한다.
