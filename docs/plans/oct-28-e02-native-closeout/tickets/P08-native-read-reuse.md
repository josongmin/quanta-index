# P08 — Native persisted read·reuse consumer와 query별 capability co-cut

<!-- b-closeout-20261103 -->
**2026-11-03 현재:** B source·cold exact suite/Just consumer 및 L3/common/Chunk 인계는 source에 반영됐다. 현재 syntax7/7·영향policy7/7 및 registry/consumer/negative 검사는 통과했다. QE11 374/374·scalar4실행은 historical Source94ca 결과이며 current Source는 별도 검증한다. 현재 단일 executor는 D, accepted45와 prepared54newaccepted0은 분리한다. P60 source gate와 P07 current Native/SDK·Linux/process 수용은 남는다. [단일 현재 checkpoint](../../oct-22-finalizing/VALIDATION.md#e02-b-trace-reuse-final-audit-20261030). 아래 최초 배정/예산/DoD는 유지한다.

작성일: 2026-10-28 KST. 실행 지시용 패킷이며 신규 구현·검증 완료 기록이 아니다. 10-28 최초 패킷 작성 시 실행은 **NOT_RUN**이었으며 후속 현재 상태는 아래 dated checkpoint를 따른다. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: 기존 Query/reuse 담당 + P00. FileText/ItemIndex 원 query body는 FNL410/W205 담당을 유지하고, capability 최종 patch는 P00가 한 번 합류한다.
- 선행: P02 GC visibility, P03 required artifact, P04 Deferred/lifecycle의 실제 반환. query matrix/reader 테스트 준비는 선행 가능하다.
- 착수 조건: 기존 consumer 감사·owner 테스트는 준비 가능. capability 활성화와 제품 판정은 선행 계약 합류 후다.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## 2026-10-30 적용·검증 정정

[현재 source·증거·잔여](../../oct-22-finalizing/VALIDATION.md#e02-b-trace-reuse-final-audit-20261030)가 최신 판정이다. 생산 source와 준비10파일은 같은 Semantica checkout에 반영됐으며 과거 QE R13 357/357은 이전 cohort다. 새 owning 실행은 changing source capture/binding으로 BLOCKED이고 현재 scalar predecessor tests4개·Runtime/SDK/제품 수용은 NOT_RUN이다. 아래 DoD checklist는 원 수용 계약이며 모두 새로 미착수라는 뜻이 아니다. source 존재를 D2/D3 완료로 바꾸지 않는다.

## 현재 소스에서 확인한 사실

- VerifiedPersistedOutputReadV3::try_bind_native_with_current_v3는 현재 Native production read에서 호출된다. binder 재개발이나 생산 caller0이라는 옛 감사는 사용하지 않는다.
- FileTextQuery와 ItemIndexQuery는 실제 try_decode_persisted_original_output_into_birth_v3를 구현한다. supports_persisted_original_output_decode_v3는 trait 기본false 외 override를 현재 확인하지 못했다.
- Native compound publication·coherent ticket·empty binding root reuse·Deferred packet과 Native observed consumer/currentness 경로가 source에 있다. actual prior identity→predecessor hook도 연결됐다. query/mode capability co-cut과 실제 owning/Runtime/SDK 양성·거절·refund 수용이 남는다.

## 수정 허용 파일·함수

| 수정 파일 | 허용 함수·hunk |
| --- | --- |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/memo.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/memo.rs) | `query별 Native durable read policy 소비` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/memo_hit_read/native_prepared_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/memo_hit_read/native_prepared_v3.rs) | `serve_native_persisted_memo_output_v3 / existing binder·decoder·NativeMemo observed row` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute/reuse_admission_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute/reuse_admission_v1.rs) | `SharedInFlight·MemoHit Native read authority/currentness 합류` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_meta_stages/memo_lookup.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/execute_meta_stages/memo_lookup.rs) | `existing Native lookup 결과 소비; Ordinary projection 우회 금지` |
| [packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/snapshot_validation.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-query-executor-owner/src/query_executor/snapshot_validation.rs) | `동일 observed row/snapshot의 physical currentness 검증` |

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 구현 순서

1. P00가 고정한 필수 query/mode matrix에서 실제 producer·decoder·artifact·GC 지원을 대조한다. FileText/ItemIndex existing decoder는 재사용하며 나머지 필수 query의 결손은 해당 canonical query owner에게 귀속한다.
2. artifact I/O 전/await 후/decoder 후의 physical currentness, scope/query/schema/function/host exact binding과 실제 성공 read authority를 동일 row에 연결한다.
3. MemoHit·SharedInFlight·메타/Deferred 재사용은 checked native lender와 원 row/root/epoch를 사용한다. Ordinary DTO clone·Deref/bare Arc·source 없는 canonical decode fallback을 추가하지 않는다.
4. P00에 query별 capability와 flow parent의 co-cut patch를 반환한다. 모든 필수 write/read/GC/consumer 계약이 연결된 같은 candidate에서 실행한 뒤 활성 지원을 선언한다.

## 입출력·오류·custody 계약

입력: 동일 MemoNativeObservedRowV3/root와 verified artifact read, query-owned decoder, 실제 current/read authority. 출력: 기존 typed output+wire pair 및 정확한 성공 read authority/거절. 공개 capability는 결과 요약이며 권한 issuer가 아니다.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [ ] 필수 query×mode matrix의 각 행이 actual production caller와 owner oracle를 갖는다. 누락 행을 새 Unsupported/N/A로 바꿔 종료하지 않는다.
- [ ] cold durable reuse에서 기대 output/trace/binding이 독립 uncached 실행과 동일하며 재계산/불필요 I/O 횟수가 계약과 맞는다.
- [ ] foreign/stale query/schema/root/function/host, I/O 중 generation 변경, decoder 뒤 currentness 변화, malformed typed/wire pair는 성공 read authority0.
- [ ] nonempty 및 genuine complete-empty bindings, rootless never-published miss, old pinned view, concurrent publish/GC/rollback을 같은 epoch로 수용한다.
- [ ] SharedInFlight wait/취소·Deferred completion·last alias/weak drop에서도 원 cause·partial·funding 수명과 최종 환급을 증명한다.

## 검증 대상

기존 native_lookup_tests_v3의 native_reuse_visibility_joins_real_memo_trace_and_binding_roots_v3와 memo_entry_loan_tests_v3 binder 음성을 재사용한다. owning public Runtime query로 FileText/ItemIndex 및 고정된 필수 query set을 검증한다.

새 selector/feature class가 필요하면 existing owning QBC resolver에서 먼저 확정한다. 제안한 테스트를 실행된 proof로 세지 않는다. compile·실제 실행 본문·제품을 별도 보고한다.

## 제외·재사용

- query_trait 기본값/전체 query capability를 전역true로 변경하지 않는다.
- FileText/ItemIndex decoder, native binder, coherent publication engine 재구현0.
- Kernel query_persisted_output_v3.rs / query_trait.rs와 query별 capability hunk는 P00 최종 writer.

## 착수 전 중복 확인

- 재조회 기준 S HEAD `7da55e44a0cea5fc25eceae65407f756bfe1de86`. 계획의 실제 write set만 대조했으며 claim은 잠금/함수 전체 소유권이 아니다.
- `QIDX-E02-NATIVE-DEFERRED-INTEGRATION-20261028`: actual write 경로 1개와 교차. 같은 함수/hunk·불변식·증명 경계가 겹치는지 착수 때 대조한다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
