# RepoMap 출하 제거 및 저장 퇴역

2026년 11월 26일 KST. 제거 경로는 기존 C 소스 대조에 기록했다. C-04 잔여는 S `7d8c230f2de2`·Q `28784da2692e`와 공유 작업 트리에서 재확인했다. 동일 S/Q 서명 동결과 이전 Native 설치 활성화는 대기 중이다. [C 소스 대조](../../../../semantica-codegraph-v2/docs/plans/nov-21-architecture-audit/C-implementation-evidence-20261122.md#c-repomap-source-readback-20261126) · [C-04 잔여](04-integration-acceptance.md#c04-fd6-remaining-code) · [결정과 완료된 소스 계약](../../adr/NOV-23-001-repomap-retirement-and-source-custody.md)

## 현재 상태와 남은 코드

**P11 공용 실행기에서 추가 구현 항목은 확인되지 않았다.** 이후 작업은 Root/F의 동일 후보 실행 증거 인수와 D/E08의 대상별 운영 계약·실제 절체다. [소유 경계와 재검증](04-integration-acceptance.md#p11-integration-remaining)

**병렬 착수는 [작업 배치·파일 소유·합류 순서](04-integration-acceptance.md#parallel-work)를 따른다.** 아래 순서는 통합 우선순위다. [기존 채팅과 중복 여부](04-integration-acceptance.md#owner-overlap-audit) · [채팅 배정](04-integration-acceptance.md#dispatch-receipts)

| 순서 | 남은 작업 | 코드에서 확인한 경계 | 기존 오너 |
| --- | --- | --- | --- |
| 1 | 운영 Dirty catalog 설치 수용 | signed catalog 발행기·피드 판독기와 resident CLI 등록이 소스에 연결됐다. 승인 배포의 grant·발행자 키와 동일 Source/세대의 실제 catalog를 공급해 변경·삭제·no-op과 새 PID 재생을 실행한다. 미공급 Required DELTA는 Source 게시 전에 거절한다. [S 작업](01-semantica.md#dirty-required) | 기존 Dirty 생산·설치 |
| 2 | 새 PID의 DELTA QMP 복원과 구 V3 쓰기 전환 | Watch의 Changed·V16·RecordOnly와 후속 Changed 회귀는 [구현 ADR](../../adr/NOV-23-001-repomap-retirement-and-source-custody.md#signed-watch-v7-v16)에 정리했다. signed 설치 본문은 **0/2**이며 [Watch 수용](04-integration-acceptance.md#watch-v7-v16-acceptance)에서 관리한다. 공용 ACKed DELTA QMP 복원 API와 Resync 역사 SDK DELTA 호출은 반영됐다. 동일 Apply 재호출의 signed 분기는 S `2b84a09742c`에 반영됐다. SDK paid checkpoint는 staged V7 Host를 선택한다. pre-Source predecessor 활성화와 cold Rollback 입장이 S `4b9b84349b0`에 반영됐다. V98·V99는 signed V7 Host·Q peer를 공용 완료 판독에 넘긴다. 결손 QMP와 live successor Host 전달은 별도 SDK 오너가 이어간다. 공용 coordinator는 퇴역 RepoMap Required의 Ready 전이와 게시를 거절한다. 합류 뒤 나머지 신규 V3 Required 발급 0을 확인한다. [S 호출부](01-semantica.md#watch-resync-v3) · [현행 배정](04-integration-acceptance.md#remaining-dispatch-20261126) | Dirty 정산(공용·Resync), Watch SDK, 별도 SDK cold Host |
| 3 | FD6 V7 양성 설치 fixture 1건 | 기존 AST-head의 두 번째 RestoreForward에 승인 V5 `Required`와 원 History, XFS 후보·V7 sidecar·CAS 뒤 기동 승인을 연결한다. 현재 확인 가능한 P07 CURRENT **0/4**, signed 본문 **NOT_RUN**. 별도 Legacy RepoMap root와 보호 route 없는 이전 Native는 지원 승인 대기다. [잔여 작업](04-integration-acceptance.md#c04-fd6-remaining-code) · [완료된 launcher와 peer 계약](../../adr/NOV-23-001-repomap-retirement-and-source-custody.md#c04-candidate-and-selected-peer) | C-04/D |
| 4 | 기존 회귀 실행 | F의 퇴역 v2 승인 readback은 로컬 검증을 마쳤다. IFX150 V7 거절 fixture는 ACKed 12·staged 4를 포함한다. 실제 거절 0/16, Resync F23·IFX150 0/2, paired P11 0/2다. 기존 ranked·Workset·SCV2·Graph·File/Path 레인에서 같은 후보의 본문을 받는다. [등록과 수락](04-integration-acceptance.md#ifx150-p11) · [F 로컬 검증](02-quanta-index.md#c04-f-release-approval-v2) | Root/F와 기존 테스트 오너 |
| 5 | Linux 대상별 action·관측과 실제 복구 | P11 공용 runner는 구현됐다. 승인 대상의 action·독립 observer 등록과 실제 실행이 남았다. 이전 Required 정산, selection CAS, 새 PID 조회와 첫 durable commit 전 기존 Native 역전환 또는 승인 동등 복구는 미실행이다. [절체 조건](04-integration-acceptance.md#c04-fd6-linux-acceptance-20261126) | D/E08 |
| 별도 | 이전 데이터의 저장 퇴역 | 보존 만료와 마지막 참조 해제를 확인한 뒤 전용 범위만 회수한다. [저장 퇴역](03-storage-retirement.md) | S Store, Q Catalog/GC, D/H |

## 티켓과 담당

- [01 Semantica](01-semantica.md) · [02 Quanta Index](02-quanta-index.md) · [03 저장 퇴역](03-storage-retirement.md) · [04 통합·설치 수용](04-integration-acceptance.md)
- 검색 검증은 기존 TEXT의 SDK60·Large, B의 ANN4K·제품 4K, F-EVAL의 Hybrid를 병렬로 진행한다. Root/F가 기존 큐에서 같은 입력과 실행 증거를 한 번 인수한다. B의 4K fixture는 Text와 Semantic만 활성화한다. [07 제품 기준](../nov-20-native-cutover-closeout/tickets/07-product-qualification.md) · [실행 큐](../nov-20-native-cutover-closeout/tickets/LANES.md)
