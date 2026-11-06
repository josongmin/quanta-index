# P02 — Public Native bounded GC와 동일 visibility epoch

작성일: 2026-10-28 KST. 실행 지시용 패킷이며 신규 구현·검증 완료 기록이 아니다. 현재 실행 상태는 **NOT_RUN**. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: GC 전용 담당 1명. 조회한 활성 claim의 해당 GC 파일 exact match는 없었다. 착수 시 현재 함수·dirty hunk와 SSOT 담당을 다시 대조한다.
- 선행: P00의 actual Output A / page funding / coherent ticket ABI. 준비와 테스트 작성은 P01·P03·P04·P07과 독립이다.
- 착수 조건: P00 ABI 확정 뒤 독립 착수 가능. 활성 claim 없음은 수정권 확정과 같지 않다.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## 현재 소스에서 확인한 사실

- SnapshotGC::start/try_start/gc_page/try_gc_page_coherently_v1의 현재 public 인수에는 Native Output admission이 없다.
- 단일 Native map에 SnapshotLocator와 try_next_native_snapshot_v3 / try_visit_native_snapshot_rows_v3가 이미 있다. 별도 GC index/map이나 full scan을 만들지 않는다.
- Ordinary GC pressure target은 Native memo state 등록을 거절한다. 이 guard를 제거하는 것은 Native GC 구현이 아니다.

## 수정 허용 파일·함수

| 수정 파일 | 허용 함수·hunk |
| --- | --- |
| [packages/analysis/quanta-v2/crates/quanta-contract-ports/src/memo_gc_port.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-contract-ports/src/memo_gc_port.rs) | `Native cursor/page/authorization/prepared hook의 기존 port 확장` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/invalidation/snapshot_gc/gc.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/invalidation/snapshot_gc/gc.rs) | `public Native start/page/continuation 경계` |
| [packages/analysis/quanta-v2/crates/quanta-adapters-in-memory-shared/src/in_memory/core/memo_table/gc.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-adapters-in-memory-shared/src/in_memory/core/memo_table/gc.rs) | `Native page 준비/삭제·기존 GC mode 분기` |
| [packages/analysis/quanta-v2/crates/quanta-adapters-in-memory-shared/src/in_memory/core/memo_table/native_physical_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-adapters-in-memory-shared/src/in_memory/core/memo_table/native_physical_v3.rs) | `기존 SnapshotLocator bounded seek/row visitor의 GC 연결 hunk` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-invalidation-owner/src/invalidation/trace_invalidator/index/gc_page.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-invalidation-owner/src/invalidation/trace_invalidator/index/gc_page.rs) | `동일 epoch의 Native observed-row Trace 정리 hook` |

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 구현 순서

1. 기존 snapshot cursor를 같은 table/root/generation에 결속하고 actual A 아래 page/삭제/retired custody를 미리 준비한다. cursor 진행·page ack는 기존 재개 의미를 유지한다.
2. GC page가 Memo/Trace/canonical exact-binding reader와 동일 coherent gate identity/epoch를 사용하게 한다. 단순 lock 순서 일치나 여러 순차 CAS로 공통 가시성을 주장하지 않는다.
3. old pin/live receipt/retention floor를 검사하고 Source 종료 뒤 같은 ticket 아래 순수 CAS/move만 수행한다. 실패 시 prior/retired custody를 ticket 밖에서 회수한다.

## 입출력·오류·custody 계약

입력: 같은 Native root snapshot, 실제 Output A, 기존 safe point·retention guard·live receipt. 출력: funding을 보유하는 bounded page/continuation과 정확한 삭제 통계; 새 authority를 발급하지 않는다.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [ ] public Native caller가 existing locator helper를 실제 소비한다. helper 정의/ordinary guard 삭제만으로 종료하지 않는다.
- [ ] N snapshots·각 M rows에서 page/work 상한을 준수하고 이웃 snapshot/full-map 순회 없이 재개한다. 실제 visit/work와 peak bytes를 측정한다.
- [ ] old pinned view·미완료 receipt가 참조한 행은 보존; 마지막 release 이후 페이지를 재개하면 실제 회수 진전.
- [ ] foreign cursor/table/root/epoch, stale reservation, short A/work, cancel/unwind, CAS conflict, ack loss에서 삭제/통계 이중 적용0.
- [ ] 동시 Memo/Trace/binding readers가 publication/GC 전후 및 rollback 후 하나의 epoch만 관측한다. 마지막 page/partial/retired owner drop 뒤 환급0.

## 검증 대상

기존 native_physical_v3 locator 회귀, memo_gc_port protocol 회귀, snapshot_gc owner 회귀를 확장한다. 실제 public Native GC + concurrent reader + last-release 진행은 owning Runtime test로 수용한다.

새 selector/feature class가 필요하면 existing owning QBC resolver에서 먼저 확정한다. 제안한 테스트를 실행된 proof로 세지 않는다. compile·실제 실행 본문·제품을 별도 보고한다.

## 제외·재사용

- Memo/Trace/binding map·root issuer·coherent transaction 엔진 재개발0.
- capability 활성화와 QE prepare/store parent는 P00/P08 경계다.
- public GC 예산을 전역 unbounded로 바꾸지 않는다.

## 착수 전 중복 확인

- 재조회 기준 S HEAD `7da55e44a0cea5fc25eceae65407f756bfe1de86`. 계획의 실제 write set만 대조했으며 claim은 잠금/함수 전체 소유권이 아니다.
- 이 write set과 교차하는 활성 exact-path claim 없음. 기존 SSOT 담당과 현재 dirty hunk까지 확인해야 착수할 수 있다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
