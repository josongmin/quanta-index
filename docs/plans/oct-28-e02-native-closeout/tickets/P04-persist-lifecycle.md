# P04 — Persist worker·ledger·retained cleanup의 실제 과금과 refill

작성일: 2026-10-28 KST. 실행 지시용 패킷이며 신규 구현·검증 완료 기록이 아니다. 현재 실행 상태는 **NOT_RUN**. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: 기존 QIDX-E02-NATIVE-DEFERRED-INTEGRATION-20261028 담당. 현재 exact claim 5파일과 일치한다.
- 선행: P00의 admitted constructor/dispatcher birth ABI. Native trace/provider 완료와 독립으로 준비할 수 있다.
- 착수 조건: 현재 담당에서 직접 이어갈 독립 코드 패킷. 이미 구현된 Deferred 회귀는 재사용한다.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## 현재 소스에서 확인한 사실

- typed 실패 보존·conditional abort·unknown-publication 분류와 기존 3회 rollback의 유료 failure bank는 구현돼 있다.
- PersistRollbackGuardV1::new와 await_tracked_persist_side_effect_inner_v1에는 mpsc/oneshot·Box command/future·Arc ledger/retained retry owner의 물리 생성이 있다.
- LifecycleStateV1의 failures Vec/String과 shutdown retry owner 순회도 있다. Native 호출 경로의 실제 할당을 claim 범위로 좁혀 Source 안 생성인지/외부 DATA인지 확인해야 한다.

## 수정 허용 파일·함수

| 수정 파일 | 허용 함수·hunk |
| --- | --- |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_store/transaction.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_store/transaction.rs) | `PersistRollbackGuardV1::new / tracked command / RetainedPersistCleanupV1 / rollback_with_retry_v1` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_store/transaction/native_commit_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_store/transaction/native_commit_v3.rs) | `동일 native ledger·completion·rollback 이동` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_store/rollback.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_store/rollback.rs) | `PersistRollbackLedgerV1의 Native retained receipts/banks` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/persist_lifecycle_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/persist_lifecycle_v1.rs) | `Native admission·retained cleanup·shutdown/refill custody` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/persist_cleanup_receipt_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/persist_cleanup_receipt_v1.rs) | `Native typed uncertainty/receipt의 물리 owner와 최종 drop` |

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 구현 순서

1. 기존 dispatcher/worker 하나를 유지하며 Native channel cell/header/command/future/Arc weak/header/ledger/receipt의 실제 물리 birth와 drop 경계를 열거한다. 추정 상수만 청구하지 않는다.
2. 기존 Original allocator/closed payload·task birth를 사용해 allocation-before-birth와 exterior DATA를 연결한다. `PersistRollbackGuardV1::new` 내부는 P04가 소유하고, QE `constructors.rs`의 공용 구성 합류만 P00가 맡는다.
3. 고정3회 bank를 무상 reset하지 않는다. shutdown/manual 추가 retry에는 새 genuine admitted refill을 받거나 typed exhausted 상태로 custody를 보존한다.
4. ordinary semantics와 기존 bounded capacity·드레인·supersede·unknown outcome 책임을 유지한다. 두 번째 worker/queue/cleanup engine을 만들지 않는다.

## 입출력·오류·custody 계약

입력: 기존 same-operation Native packet·dispatcher admission·retained rollback owner. 출력: 같은 worker에 전달되는 funded command/ledger/completion과 shutdown까지 살아 있는 실패·cleanup custody.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [ ] 모든 Native 물리 birth의 실제 payer/operation/group이 확인되고 해당 bound를 넘는 생성 전에 거절한다.
- [ ] queue full/closed·unpolled/cancelled future·worker panic/loss·superseded write·unknown artifact outcome에서 허위 success/drained0.
- [ ] 3회 이후 retry가 이전 bank를 무상 재사용하지 않으며 failed cleanup의 receipt·partial·원 first/secondary cause가 refill 또는 명시 cleanup까지 유지된다.
- [ ] 마지막 payload/command/receipt/shared alias·weak owner 뒤에만 환급; shutdown closes admission before wait, timeout은 실패로 보존.
- [ ] 새 회귀와 기존 QE334/관련 lifecycle 테스트를 영향 범위에서 통과. 숫자334 자체를 최신 분모로 강제하지 않는다.

## 검증 대상

기존 native_deferred_packet_tests_v3, native_persist_failure_tests_v3, transaction_tests, persist_lifecycle_tests_v1를 재사용/확장한다. fourth/manual retry와 real queue/worker drop counter oracle가 필요하다.

새 selector/feature class가 필요하면 existing owning QBC resolver에서 먼저 확정한다. 제안한 테스트를 실행된 proof로 세지 않는다. compile·실제 실행 본문·제품을 별도 보고한다.

## 제외·재사용

- typed error owner/PaidFiniteSourceFailureReadyV3, 기존3회 rollback bank, 공통 worker 재작성0.
- raw allocation을 Native grant로 재표시하거나 permit을 함수 종료 시 먼저 해제하지 않는다.
- QE constructors.rs/Cargo/root exports는 P00 patch로 합류. transaction 내부 constructor는 P04 범위다.

## 착수 전 중복 확인

- 재조회 기준 S HEAD `7da55e44a0cea5fc25eceae65407f756bfe1de86`. 계획의 실제 write set만 대조했으며 claim은 잠금/함수 전체 소유권이 아니다.
- `QIDX-E02-NATIVE-DEFERRED-INTEGRATION-20261028`: actual write 경로 5개와 교차. 같은 함수/hunk·불변식·증명 경계가 겹치는지 착수 때 대조한다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
