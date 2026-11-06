# P03 — Native trace·evidence·forensic producer와 마지막 memo birth

<!-- b-closeout-20261103 -->
**2026-11-03 현재:** B source·cold exact suite/Just consumer 및 L3/common/Chunk 인계는 source에 반영됐다. 현재 syntax7/7·영향policy7/7 및 registry/consumer/negative 검사는 통과했다. QE11 374/374·scalar4실행은 historical Source94ca 결과이며 current Source는 별도 검증한다. 현재 단일 executor는 D, accepted45와 prepared54newaccepted0은 분리한다. P60 source gate와 P07 current Native/SDK·Linux/process 수용은 남는다. [단일 현재 checkpoint](../../oct-22-finalizing/VALIDATION.md#e02-b-trace-reuse-final-audit-20261030). 아래 최초 배정/예산/DoD는 유지한다.

작성일: 2026-10-28 KST. 실행 지시용 패킷이며 신규 구현·검증 완료 기록이 아니다. 10-28 최초 패킷 작성 시 실행은 **NOT_RUN**이었으며 후속 현재 상태는 아래 dated checkpoint를 따른다. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: Trace/evidence 전용 producer 담당 1명. shared QE prepare/store parent는 P00만 쓴다.
- 선행: P00의 기존 persist packet/ArtifactRef·retained DATA 반환 ABI. provider·DELTA 완료를 기다리지 않는다.
- 착수 조건: private producer부터 독립 착수. 최종 caller 연결은 P00 직렬 합류.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## 2026-10-30 적용·검증 정정

[현재 source·증거·잔여](../../oct-22-finalizing/VALIDATION.md#e02-b-trace-reuse-final-audit-20261030)가 최신 판정이다. 생산 source와 준비10파일은 같은 Semantica checkout에 반영됐으며 과거 QE R13 357/357은 이전 cohort다. 새 owning 실행은 changing source capture/binding으로 BLOCKED이고 현재 scalar predecessor tests4개·Runtime/SDK/제품 수용은 NOT_RUN이다. 아래 DoD checklist는 원 수용 계약이며 모두 새로 미착수라는 뜻이 아니다. source 존재를 D2/D3 완료로 바꾸지 않는다.

## 현재 소스에서 확인한 사실

- Native prepare는 실제 Native trace/evidence artifact 슬롯을 채우고 canonical forensic/Args/provenance와 memo ref를 준비한다. Ordinary `trace_bytes=None`은 이 Native 슬롯의 부재가 아니다. Native Store는 admitted artifact를 rollback-tracked 경로로 저장하며 필수 Native 슬롯이 없으면 typed refusal를 유지한다.
- 실제 Native trace frame/checked reader는 task_local_trace_original_v3에 있다. Ordinary ExecutionTrace DTO를 다시 만들어 serialize하면 Native 구현이 아니다.
- Native paid memo producer와 Deferred packet은 이미 있다. packet/metadata/새 trace registry를 재작성하지 않는다.

## 수정 허용 파일·함수

| 수정 파일 | 허용 함수·hunk |
| --- | --- |
| [packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/task_local_trace_original_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/task_local_trace_original_v3.rs) | `기존 OriginalTraceFrameV3/stack에서 실제 admitted wire·artifact 입력 대여` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/task_local_trace_original_v3/read_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/task_local_trace_original_v3/read_v3.rs) | `동일 frame/Current의 checked whole trace lender` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-kernel-contract/src/ports/trace_recorder.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-kernel-contract/src/ports/trace_recorder.rs) | `기존 TraceRecorder의 필요한 scoped producer ABI만` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_prepare/original_output_material_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_prepare/original_output_material_v3.rs) | `기존 Native output material 정책에 admitted artifact 입력 결합` |

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 원 구현 순서 — 현재 source에서 재사용

1. 기존 canonical Trace/evidence encoder와 actual Original frame reader를 연결한다. 전체 trace clone/Ordinary Vec·Map 재구성 대신 scoped checked lender + exterior birth DATA를 사용한다.
2. trace artifact/evidence/forensic build/snapshot/call digest를 같은 실제 Output A에서 준비하고, 참조·exact binding witness를 closed MemoEntry 마지막 birth 전에 넘긴다.
3. P00에 prepare/store 두 parent의 최소 caller 패치와 typed 실패·partial 슬롯을 전달한다. guard를 단순 해제하거나 required artifact를 None으로 바꿔 no-store 성공을 만들지 않는다.

## 입출력·오류·custody 계약

입력: 기존 admitted trace/snapshot reader, 실제 A와 query policy. 출력: 기존 packet이 소비할 실제 ArtifactRef/paid payload·외부 partial DATA·grant·first/secondary cause. 새 artifact schema나 게시 issuer는 추가하지 않는다.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [ ] nonempty inputs/calls/bindings의 실제 canonical trace bytes/digest가 기존 reference와 일치하고 실제 ArtifactStore에 쓰인 ref가 closed memo에 결속된다.
- [ ] 지원 forensic/evidence 모드의 필수 metadata와 payload가 actual producer→prepare→Store→read consumer로 연결된다.
- [ ] empty/complete-empty/incomplete 구별과 required trace 정책 유지; missing/foreign/stale artifact는 write 전 typed refusal.
- [ ] producer 거절, partial encode/I/O 실패, late Source cancel, unwind, Deferred worker loss에서 원인·모든 partial 보존과 최종 환급.
- [ ] Immediate/Deferred/no-store 각각 의도한 실제 public Runtime 경로에서 수용하며 기존 Ordinary 경로 byte/semantic 회귀도 보존.

## 검증 대상

기존 task_local_trace_original_v3 owner 회귀와 QE native_trace_registration_v3/Native persist 회귀 확장. 생산 trace가 있는 genuine Source를 사용한다. 정책상 필수 조건을 제거하는 negative twin도 포함한다.

새 selector/feature class가 필요하면 existing owning QBC resolver에서 먼저 확정한다. 제안한 테스트를 실행된 proof로 세지 않는다. compile·실제 실행 본문·제품을 별도 보고한다.

## 제외·재사용

- QE execute_core_prepare.rs / execute_core_store/native_v3.rs의 parent 최종 쓰기는 P00.
- 새 Trace IR/serializer/registry, ordinary adapter, empty forensic success0.
- 기존 admitted trace residency·paid memo·Deferred 구현을 새로 발급하지 않는다.

## 착수 전 중복 확인

- 재조회 기준 S HEAD `7da55e44a0cea5fc25eceae65407f756bfe1de86`. 계획의 실제 write set만 대조했으며 claim은 잠금/함수 전체 소유권이 아니다.
- 이 write set과 교차하는 활성 exact-path claim 없음. 기존 SSOT 담당과 현재 dirty hunk까지 확인해야 착수할 수 있다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
