# P00 — 공용 ABI·parent·검증 큐의 단일 합류

작성일: 2026-10-28 KST. 실행 지시용 패킷이며 당시 실행 상태는 **NOT_RUN**이었다. 이 작성 시점 표기를 현재 실행 verdict로 재사용하지 않는다. 최신 상태는 기존 E02 SSOT/VALIDATION이 소유하며, L1 bank의 확인된 구현·공용 접합과 남은 D 검증은 [2026-10-30 최종 감사](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/VALIDATION.md#e02-l1-final-audit-20261030)를 따른다. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: Root 통합 담당. 기존 FNL410/W205/PUB의 실제 담당 함수는 인계 없이 가져오지 않는다.
- 선행: 선행 없음. P01–P08 시작 전에 ABI와 writer hunk를 확정하고, 산출물 도착 후 합류한다.
- 착수 조건: 즉시: 공용 함수별 write map과 입력 ABI 확정. 코드 합류/판정은 공급 순서에 따른다.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## 현재 소스에서 확인한 사실

- 현재 S main과 Q main 모두 공유 작업이 있다. claim은 잠금이 아니며 같은 파일도 함수·hunk·불변식이 다르면 조율 후 작업할 수 있다.
- QE에는 Native compound commit/rollback, 같은 coherent ticket의 Memo·binding·Trace publication과 실제 binder 호출이 있다. 이를 새 transaction으로 다시 만들지 않는다.
- Runtime Cargo/feature는 FNL410, query/decoder 인접 파일은 W205와 겹친다. 이 문서는 새 담당자 배정이나 claim 인계 사실을 만들지 않는다.

## 수정 허용 파일·함수

| 수정 파일 | 허용 함수·hunk |
| --- | --- |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_flow.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_flow.rs) | `Native producer/reader 결과의 실제 flow 합류 hunk` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_prepare.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_prepare.rs) | `prepare_execution_persist_v1 및 Native trace/evidence 결과의 마지막 memo birth 합류` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_store/native_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_core_store/native_v3.rs) | `persist_prepared_original_execution_v3의 정책별 artifact 소비 hunk만` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/constructors.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/constructors.rs) | `기존 dispatcher/lifecycle의 admitted constructor 합류 hunk` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/Cargo.toml](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/Cargo.toml) | `기존 Native proof feature closure; 다른 lane 알고리즘 수정 제외` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/sdk/public_exports_tail.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/sdk/public_exports_tail.rs) | `기존 SDK 담당과 조율한 공개 export hunk만` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/orchestration/public_exports_tail.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/orchestration/public_exports_tail.rs) | `해당 packet의 orchestration export hunk만` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/lib.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/lib.rs) | `해당 packet의 공용 mount/export hunk만` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/lib.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/lib.rs) | `해당 packet의 공용 private module mount hunk만` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-kernel-contract/src/domain/contracts/query_persisted_output_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-kernel-contract/src/domain/contracts/query_persisted_output_v3.rs) | `여러 lane에 공통인 read binding ABI 변경만` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-kernel-contract/src/domain/contracts/query_trait.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-kernel-contract/src/domain/contracts/query_trait.rs) | `공통 trait/capability contract hunk만` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-surface-foundation/src/file_text.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-surface-foundation/src/file_text.rs) | `supports_persisted_original_output_decode_v3 등 최종 capability hunk만; decoder body는 기존 query owner` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-surface-symbol-owner/src/symbol_query_rail/item_index_query.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-surface-symbol-owner/src/symbol_query_rail/item_index_query.rs) | `최종 capability hunk만; 기존 ItemIndex query/decoder body 불변` |

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 구현 순서

1. 지원 query/mode 집합과 Immediate/Deferred/no-store/SharedInFlight/forensic 조합을 기존 E02 계약에서 고정한다. 구현 후 지원 범위를 줄여 DoD를 닫지 않는다.
2. P02 GC port, P03 artifact 결과, P04 lifecycle birth/refill, P05 supplier 반환, P08 read 권한의 정확한 기존 타입·함수·인수·실패 슬롯을 먼저 확정한다. 새 issuer/IR/registry 대신 기존 owner의 최소 확장을 선택한다.
3. worker는 전용 파일만 수정하고, 이 표의 parent 변경은 함수 단위 패치와 oracle를 넘긴다. Root가 한 번 적용한다. lib/export/Cargo/root mount의 동시 최종 쓰기는 허용하지 않는다.
4. P01–P08의 source+API 준비를 확인한 뒤 묶음으로 영향 검증한다. 컴파일 실패는 같은 compile closure의 합류를 멈추고, body 실패는 해당 시나리오만 수리한다.

## 입출력·오류·custody 계약

입력: 각 패킷의 실제 typed owner + custody + first/secondary failure + direct caller 패치. 출력: 같은 source에서 연결된 Runtime/QE 구성과 P09의 실행 요청. 패킷 간 ABI 요청은 이 티켓과 기존 SSOT/HANDOFF로 전달한다.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [ ] 각 허용 함수/hunk의 최종 writer 1명, claim 충돌 미정 0, 입력 누락을 기본값으로 합성한 경로 0.
- [ ] 같은 Original의 실제 Output A, immutable owner, 최고 Source finisher, coherent visibility epoch가 producer→consumer에서 유지된다.
- [ ] capability 활성화는 P02/P03/P04/P08 계약을 함께 반영한 candidate에서만 수행한다. 필수 거절 guard는 생산자가 공급될 때 정확한 소비 분기로 치환하며 전역 삭제하지 않는다.
- [ ] 각 코드 패킷은 D2, 최종 제품은 P09의 D3가 필요하다. source/compile/owner/product를 각각 판정한다.

## 검증 대상

공용 ABI 변경 전후 boundary preflight. 기존 QE Native compound/reuse/Deferred 회귀를 재사용하고 영향 Runtime integration을 같은 frozen root에서 실행한다.

새 selector/feature class가 필요하면 existing owning QBC resolver에서 먼저 확정한다. 제안한 테스트를 실행된 proof로 세지 않는다. compile·실제 실행 본문·제품을 별도 보고한다.

## 제외·재사용

- E01 producer 내부, Type/PTA/DFG 엔진, SDK/PUB 소유 parent를 임의 수정하지 않는다.
- 기존 oct-22-finalizing SSOT 4개 문서는 이 패키지 작성에서 수정하지 않는다. 진행/최종 판정 SSOT도 복제하지 않는다.

## 착수 전 중복 확인

- 재조회 기준 S HEAD `7da55e44a0cea5fc25eceae65407f756bfe1de86`. 계획의 실제 write set만 대조했으며 claim은 잠금/함수 전체 소유권이 아니다.
- `OCT23-FNL410-IMPLEMENT-ROOT-INTEGRATION`: actual write 경로 4개와 교차. 같은 함수/hunk·불변식·증명 경계가 겹치는지 착수 때 대조한다.
- `QIDX-E02-NATIVE-DEFERRED-INTEGRATION-20261028`: actual write 경로 5개와 교차. 같은 함수/hunk·불변식·증명 경계가 겹치는지 착수 때 대조한다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
