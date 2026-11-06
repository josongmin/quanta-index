# Isolated lane assignments — 2026-10-29 KST

<!-- b-closeout-20261103 -->
**2026-11-03 현재:** B source·cold exact suite/Just consumer 및 L3/common/Chunk 인계는 source에 반영됐다. 현재 syntax7/7·영향policy7/7 및 registry/consumer/negative 검사는 통과했다. QE11 374/374·scalar4실행은 historical Source94ca 결과이며 current Source는 별도 검증한다. 현재 단일 executor는 D, accepted45와 prepared54newaccepted0은 분리한다. P60 source gate와 P07 current Native/SDK·Linux/process 수용은 남는다. [단일 현재 checkpoint](../oct-22-finalizing/VALIDATION.md#e02-b-trace-reuse-final-audit-20261030). 아래 최초 배정/예산/DoD는 유지한다.

<!-- e02-b-current-20261030 -->
**B 현재 상태(2026-10-30):** P03 실제 Trace/Evidence/forensic→closed memo→admitted Store와 P08 Native reader/checked binder는 source에 연결됐다. 준비10파일과 provider/census도 같은 checkout에 반영됐다. 기존 P03/P08 미착수·미적용·producer 부재 문구는 과거 지시다. 새 predecessor4 scalar tests와 genuine Runtime/SDK/cross-generation/cold는 현재 수용되지 않았고 capability=false다. [유일한 현재 결과·담당별 잔여](../oct-22-finalizing/VALIDATION.md#e02-b-trace-reuse-final-audit-20261030). 아래 최초 배정·D0–D3·예산/DoD는 유지한다.

사용자 지시: 패킷을 각 레인에 할당하고 레인끼리 이야기하지 않으며 끝까지 진행한다. 이 문서는 실행 배정과 경계이며, 기존 E02 진척·완료 SSOT를 대체하지 않는다.

2026-10-30 현재 source·담당 대조: C는 P07만 직접 마감한다. P05 SDK seed→pair→installer 호출이 이미 추가됐고 genuine predecessor 공급은 기존 DELTA Root/B 및 L1–L4 소유다. 아래 10-29 최초 배정/반환 기록 중 C→P05와 installer 무호출을 새 C TODO로 사용하지 않는다. [현재 Provider 판정](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/VALIDATION.md#e02-provider-final-audit-20261030-c)을 우선한다. 후속 사용자의 명시적 중복 대조 요청에 따라 읽기 전용 채팅 대조만 수행했으며 메시지·깨우기·재배정은 하지 않았다.

## 실제 배정

10-29 사용자가 각 레인을 별도 채팅에서 실행하도록 요청하여 아래 3개 local/shared-main 채팅을 생성했다. 이전 `/root/lane_*` subagent는 선행 구현 담당이며 새 채팅과 동시에 수정하지 않는다. 생성 후 세 채팅 모두 `inProgress`와 초기 source/계약 읽기 실행을 확인했다. 이것은 착수 확인이며 코드 완료 증거가 아니다.

| 레인 | 실제 task | 지금 담당 | 다음 담당 / 출구 |
| --- | --- | --- | --- |
| A | `01a183bf-ee89-7861-a43f-b2ce1f298bcb` | P02 Native public GC·다중 query binding·bounded Trace 삭제 | public caller·동일 epoch 삭제/retirement까지 계속 수행 |
| B | `01a183c0-1462-7090-9c39-de40b9984281` | P03 Trace/Evidence·P08 reader 및 QE predecessor exact hunk | 생산 source/준비10파일 적용. 현재 owning source admission BLOCKED; 실제 cross-generation 및 Runtime/SDK 수용은 미완료. Root supplier/family, L1-D 검증 경계 유지 |
| C | `01a183c0-22b5-79c1-a260-fda56a4fc284` | P07 configured Voyage Native transport·codec·오류/partial DATA | P05는 기존 DELTA/E01 담당 유지. C는 별도 ItemIndex/Graph/Chunk·P06 생산 경로·DELTA fixture를 수정/재배정하지 않음 |
| Root | 이 채팅 | P00 공유 ABI/parent, P04 lifecycle/worker, P09 검증 큐 | caller 합류·반환 실패 수리·같은 source의 owning→Runtime/installed SDK 수용 |
| 기존 canonical 담당 | `QIDX-E02-ACKED-CANONICAL-REUSE-20261028` | P01 | 새 worker에 중복 배정하지 않음. 실제 source와 owning replay로 결과 확인 |
| 기존 DELTA 담당 | `QIDX-E02-DELTA-TERMINAL-BRIDGE-20261028` | P06 | P05의 typed 반환을 기존 consumer가 소비. 기존 member/V9/ACK 엔진 재작성 없음 |

P01/P06의 기존 담당은 현재 source claim과 함수 변경으로 확인했다. 다른 채팅을 읽거나 메시지를 보내 배정을 갱신한 결과가 아니다. 이 두 범위의 새로운 최종 수정은 현재 hunk를 재대조한 후 Root가 인계 경계를 결정한다. 활성 claim 자체를 실행 완료 증거로 쓰지 않는다.

새 채팅의 직접 실행 계약: [COMMON](/Users/songmin/.codex/artifacts/qidx-e02-native-chat-lanes-20261029/COMMON.md), [A 소유 범위](/Users/songmin/.codex/artifacts/qidx-e02-native-chat-lanes-20261029/A-GC/OWN.md), [B 소유 범위](/Users/songmin/.codex/artifacts/qidx-e02-native-chat-lanes-20261029/B-TRACE/OWN.md), [C 소유 범위](/Users/songmin/.codex/artifacts/qidx-e02-native-chat-lanes-20261029/C-PROVIDER/OWN.md). 각 채팅은 COMMON과 자기 계약·소스만 읽고 peer 계약/진척을 추적하지 않는다. Root가 자기 HANDOFF와 상태를 회수한다.

## 통신과 source 격리

- A/B/C는 서로 조회·메시지·호출·진척 파일 열람을 하지 않는다. 추가 agent/채팅을 생성하지 않는다.
- 최초 배정과 후속 오류 전달은 Root→해당 레인만 허용한다. 레인의 결과는 자기 채팅 최종 응답과 자기 HANDOFF에 남기고 Root가 상태/반환을 회수한다. 레인 간 API 협상이나 Root에게 직접 채팅 메시지 전송을 하지 않는다.
- 실제 producer/caller/consumer를 확인하기 위한 공용 소스 읽기는 허용한다. 다른 레인의 임시 산출물·채팅·terminal·진척 문서 읽기는 금지한다.
- 수정은 [WRITE-MAP](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md)의 해당 파일·함수만 한다. 공용 parent/Cargo/lib/export·최종 capability는 Root가 한 번 합류한다.
- 다른 owner 파일이 필요하면 최소 patch와 정확한 입력/반환 타입을 checkout 밖에 작성하여 Root에 반환한다. guard 삭제나 기본값으로 API 공급을 대신하지 않는다.
- P02/P03/P07의 13개 최초 경로는 Root의 `QIDX-E02-ISOLATED-PRODUCERS-20261029` claim으로 등록했다. P05 supplier, GC/Trace/provider child와 공용 export를 포함해 28개 경로로 확장했다. 이는 허용 수정 범위이며 28개 파일 모두를 변경했다는 뜻은 아니다. 새 private child/test는 경로를 반환하고 Root가 해당 write set을 갱신한다. 기존 P04 claim은 이 채팅 소유다.
- source diff를 적용하기 전 현재 HEAD·dirty hunk를 확인한다. 다른 actor의 자동 stage/commit이 관측돼도 이 레인에서 실행한 것으로 보고하지 않는다. Git index/HEAD/push 변경은 이번 실행에 포함하지 않는다.

## 레인 종료와 재개

1. 현재 소스에서 이미 있는 구현을 제외한다. 필요한 입력·actual A·기존 canonical owner·오류/partial/backing 수명을 먼저 확정한다.
2. helper와 테스트 전용 생산자는 D1 완료가 아니다. 실제 producer→production caller→consumer 및 양성/거절/취소·unwind/최종 drop 경계를 구현한다.
3. Source/allocator/transport API 설계가 자기 범위에 있으면 그 API까지 구현한다. 설계 선택이 남았다는 이유로 외부 입력 BLOCKED를 붙이지 않는다.
4. 실제 외부 공급이 없으면 정확한 owner·파일·함수·필요 인수·반환/실패·custody를 반환한다. typed refusal를 유지하고 해당 게이트만 미완료로 남긴다. 입력 부재를 성공·N/A로 바꾸지 않는다.
5. Root는 반환된 공용 patch를 정적 검토하고 영향 compile closure를 한 번 합류한다. 컴파일 실패는 해당 묶음, body 실패는 해당 시나리오의 레인으로 돌려 수리한다.
6. 같은 source·입력의 이미 통과한 rail을 이유 없이 반복하지 않는다. 변경된 proof closure와 미해결 필수 gate만 재실행한다.

## 반환 후 실제로 남은 접합

다음은 레인 반환물을 production caller까지 연결하기 위한 후속 작업이다. helper 반환·정적 통과·typed refusal는 해당 기능의 D1/D2 완료가 아니다. 기존 완료 SSOT의 판정을 바꾸는 표가 아니다.

| 작업 / 최종 writer | 수정 위치·기존 구현 재사용 | 출구 조건 |
| --- | --- | --- |
| P02 A + 공용 접합 Root | A의 [public GC](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/invalidation/snapshot_gc/gc.rs), [physical page](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-adapters-in-memory-shared/src/in_memory/core/memo_table/native_physical_v3/gc_v3.rs), [Trace permit](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-invalidation-owner/src/invalidation/trace_invalidator/index/gc_page.rs)를 사용. Root가 기존 binding port/adapter의 preflight·같은 coherent ticket hook·Runtime caller를 접합. 다중 query와 고 fanout continuation을 완성 | page/work bound를 지킨 양성 삭제, pinned/live receipt 보존, 같은 epoch의 Memo/Trace/binding 관찰, 마지막 release 뒤 진전. 현재 Trace의 full-query clear와 단일-query binding만으로 전체 bounded GC 완료를 선언하지 않음 |
| P03 B + carrier/Store Root | actual canonical Trace/Evidence/Args/provenance→closed memo→admitted Trace/Evidence Store·역보상이 source에 있다. producer/paid copy/Evidence ingress 전체 미연결은 폐기한 TODO | 과거 QE R13 357/357은 해당 cohort 증거만 유지. 현재 지원 정책의 Runtime Immediate/Deferred/no-store·nonempty parity·late Source/partial/refund를 수용; 결손 required artifact는 계속 거절 |
| P04 Root | [lifecycle](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-execution/src/persist_lifecycle_v1.rs)와 기존 transaction/worker/ledger 사용. command·channel·Arc/Box·retained receipt의 실제 birth와 funding 수명을 연결 | 3회 유상 실패 뒤 새 admission 없는 네 번째 I/O가 없어야 함. 이미 추가한 typed exhaustion 회귀와 별도로 public Deferred/shutdown drain·unknown-write·최종 환급을 수용 |
| P05 기존 DELTA/E01/P06 담당 | SDK seed→collector→canonical pair→`install_complete_delta_successor_v3`→terminal 호출이 현재 소스에 있다. Root의 actual prior identity 공급과 B의 QE 재인증, L1–L4의 배정된 생산/회귀를 함께 마감 | C 재작업0. 호출 존재는 genuine complete target 또는 실행 수용 증거가 아니다. dirty-only·predecessor 복사·새 FULL 요청 합성을 금지하고 기존 owner가 gold/거절/수명 결과를 반환 |
| P07 C | [기존 Voyage adapter](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/voyage_adapter/mod.rs)의 Native override, canonical request/response codec·caller DATA와 28개 작성 회귀를 유지. 생산 enum5+테스트 패턴1의 lexical wrapping을 수리. 실제 reqwest transport admission hook은 없음 | 실제 HTTP D1 BLOCKED; owning compiler/body와 local/remote SDK·최종 환급은 C에서 NOT_RUN. codec/enum 수리·pre-request refusal를 실제 transport 성공으로 세지 않음 |
| P08 B, 공용 capability Root | Native observed read/binder/decoder와 exact historical/current predecessor split은 적용됐다. Root actual prior supplier/family도 source-present | query/mode capability 기본false 유지. B QE owning와 L1-D Runtime/SDK를 같은 host admission에서 직렬 검증하고 genuine cross-generation/cold/거절·환급 뒤 co-cut 판정 |
| P09 Root | 기존 registered warm lane에서 수정된 compile closure만 재검증. QE owner, Runtime public caller, installed SDK/CP3를 별도 판정 | compiler 성공과 nonzero body, 필수 제품 시나리오를 구별. GC 반환형의 trait Error 모호성처럼 실행에서 확인한 실패는 같은 레인에 돌려 수리 |

공용 접합의 추가 파일은 Root가 현재 claim·dirty hunk를 재대조한 뒤 write set을 확장한다. 위 표는 다른 owner의 파일 전체 수정 권한이나 외부 supplier 구현 완료를 뜻하지 않는다. 레인이 필요한 실제 입력을 반환한 경우 그 레인을 실행 중이라고 보고하지 않으며, 입력 도착 뒤 Root가 정확한 후속 작업으로 재개한다.

### P02의 다음 수정 단위

1. A는 [binding physical owner](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-adapters-foundation/src/in_memory/core/query_key_to_function_id_cache/native_physical_v3.rs)의 기존 `NativeBindingSlotsV3`에 같은 page의 exact clear를 누적한다. 행마다 별도 phase를 만들지 않고 단일 old root/generation에서 prepare→complete→preflight→commit을 한 번 수행한다. Root는 해당 binding port와 adapter parent의 public ABI·delegation만 수정한다. 여러 query를 삭제해도 generation 한 번 전진, duplicate/stale/foreign이면 세 participant 변경0이 oracle다.
2. A는 기존 [Trace authority](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-invalidation-owner/src/invalidation/trace_invalidator/index/native_authority_v3.rs)와 [resident edit/clear](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-invalidation-owner/src/invalidation/trace_invalidator/index/native_resident_v3.rs)를 bounded advance로 이어 준다. 같은 staged edit를 유지하고 완전 clear 전에는 공개 root를 바꾸지 않는다. subpage Source/scratch는 각각 highest finisher 뒤 정리하고 마지막에만 기존 commit permit을 준비한다. fanout>limit의 여러 호출, 중도 cancel/root 변경, backing-last 환급을 확인한다.
3. Root는 위 두 반환 뒤 구체적 hook을 Runtime composition에 연결한다. 같은 ticket에서 전원 preflight→실패 없는 move commit→ticket 밖 retired drop 순서를 고정한다. page ack·통계는 정확히 한 번만 전진한다. 세 root를 관찰하는 concurrent reader가 전후 epoch만 보아야 한다.

새 채팅 생성 전 Root가 현재 claim과 해당 dirty 경로를 대조했고, 위 private body를 A의 명시적 write set으로 위임했다. A는 시작 시 hunk를 다시 확인한다. Root는 해당 private body를 동시에 수정하지 않는다. 공용 binding port·adapter delegation parent·공용 mount는 계속 Root 한 writer다.

### P05 기존 producer의 참조 경계 — C 직접 수정 제외

현재 [native_delta_v3.rs의 `prepare_sdk_handoff_v3`](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3.rs)는 인증된 원 request/transition/target·Original keeper·snapshot-bound source를 보유하고 `sdk_successor_v3::finish_v3`가 기존 SDK facade를 통해 seed→collector→pair→installer→terminal로 연결한다. [기존 census loan](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/change_event_aggregate_input_v3.rs)과 [SDK transition 인증](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/sdk_v3.rs)을 재사용하며 census 자체를 complete target으로 승격하지 않는다. 호출 경로가 없다는 이유로 새 collector/installer를 만들지 않는다.

실제 prior Args·QueryKey·options·HIR identity의 parent-root loan과 QE current-Source 재인증은 기존 DELTA Root/B가 마감하고 ItemIndex/Graph/회귀/Watch는 기존 L1–L4가 담당한다. 이 표는 C의 수정권을 추가하지 않는다. 기존 bundle/issuer/complete 함수와 하위 member/V9/ACK 엔진을 재작성하지 않으며 해당 owner의 source-bound owning·Runtime/SDK 결과를 별도 수용한다. C의 P05 공급/fixture 재작업 계획은 없다.

## 검증 단일 큐

- A/B/C는 저비용 static/format/소유자 요청 resolve를 실행한다. compiler/link/test가 필요한 QBC `owner run`은 Root가 기존 warm lane에서 한 개씩 실행한다.
- Root가 확인할 순서: source/경계→관련 owning behavior→Runtime 공개 caller→installed SDK/CP3. check·test·제품 상태와 실제 실행 수를 구분한다.
- 고정 Acked4MiB·DELTA 원 예산·GC page/work·provider deadline/response bound·원 기능/성능 분모를 유지한다.
- 최종 종료 기준은 [VERIFICATION](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)의 D1+D2와 E02 필수 D3다. 이번 배정 사실은 구현 완료가 아니다.
