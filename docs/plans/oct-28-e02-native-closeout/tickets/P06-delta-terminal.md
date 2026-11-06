# P06 — 기존 DELTA entry의 supplier 소비·terminal·exact restart 수용

작성일: 2026-10-28 KST. 실행 지시용 패킷이며 신규 구현·검증 완료 기록이 아니다. 현재 실행 상태는 **NOT_RUN**. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: 기존 QIDX-E02-DELTA-TERMINAL-BRIDGE-20261028 담당. P01과 공통 canonical/schema hunk를 동시에 수정하지 않는다.
- 선행: P05의 실제 bundle/issuer 반환과 P01의 finite Acked readback. supplier 합류 전에도 own restore/fault fixture 준비는 가능하다.
- 착수 조건: 대표 생산 caller는 존재한다. 실제 미접합 supplier 입력과 미수용 행동만 이 패킷으로 닫는다.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## L3 실제 owner·consumer 합류 — 2026-11-01

[L3 source](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md#l3-source-only-closeout-20261030)에 공용27/31/34/35/39/37/40과 private32/33/36/38/41/42가 실제 반영됐다. 이번 최소 공용 hunk·문서11만 사용자 명시 승인으로 직접 적용했으며 다른 owner의 변경은 보존했다.

기존 nested whole paid packet/pre-JSON 오류 loan, 실제 admitted signal→actual V9 typed Cancelled, 확인 child exact M-key no-row, paid M DATA 뒤 actual producer marker unwind를 소유 consumer에 연결했다.35는 기존 phase 오류·정상 query 순서와 feature-off8-row/DATA/인자를 유지한다.41은 취소/unwind 뒤 자기 journal·ItemIndex fixture를 확인·소비해 다음 fresh G1의 전역 상태 충돌을 막는다.42는 취소/unwind fixture의 추가 Original keeper와 검증용 lifecycle poll을 제거했다. 실제 V9 typed 거절 뒤 같은 유료 budget의 기록된 원인을 poll 없이 읽고 최종 Source Weak·7축 환불을 검사한다.

공용 합류 BLOCKED는 해소됐다. actual G2 test1의 소스는5개 fresh fixture를 실행하며 registry/expected_tests/표준 예산은 변경하지 않았다. 정적 grammar/TOML/format/honesty/diff는 실행 수용이 아니다. compiler/type/borrow/body/SQL/worker/취소/unwind/예산/process/product NOT_RUN/body0, SourceReady=false. observer panic·release abort·packet DATA/Source 독립 drop-order 증명은 각 별도 범위다.

## 현재 소스에서 확인한 사실

- native_delta_v3.rs에 finish_native_delta_entry_v3 실제 caller가 있고, finish 내부에서 retained/restored ingress·prepare_complete_delta_members_v3·final reconcile·M commit을 호출한다.
- install_restored_delta_ingress_v3는 ingress_v3.rs의 실제 caller가 있다. 이를 다시 연결하라는 티켓을 만들지 않는다.
- complete_delta_terminal_after_members_v3 / install_delta_terminal_continuation_v3 같은 옛 wrapper의 무호출만으로 새 결손을 확정하지 않는다. 현재 final_input/m_commit/recovery의 대표 생산 경로를 기준으로 검증한다.

## 수정 허용 파일·함수

| 수정 파일 | 허용 함수·hunk |
| --- | --- |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/custody_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/custody_v3.rs) | `P05 existing owner 반환을 보존하는 scoped getter/transfer hunk` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3.rs) | `install_complete_delta_successor_v3의 실제 caller 합류 및 finish_native_delta_entry_v3` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3/final_input_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3/final_input_v3.rs) | `complete_or_reconcile_delta_from_retained_inputs_v3의 final/prior exact V9` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3/m_commit_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3/m_commit_v3.rs) | `commit_delta_terminal_m_v3 / finish_terminal_m_continuation_v3` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3/cold_recovery_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3/cold_recovery_v3.rs) | `same-operation restored input 소비` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3/recovery_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3/recovery_v3.rs) | `bound replay·ACK-loss reconciliation` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3.rs) | `stage_native_handoff_for_entry_v130의 실제 finish 결과 소비` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/core_invalidator.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/core_invalidator.rs) | `Native committed-source continuation/checkpoint hunk만` |

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 구현 순서

1. P05 반환을 initial authority를 소모하기 전에 같은 accepted owner/epoch/request/target으로 검증해 한 번 install한다. 기존 missing supplier 거절을 성공으로 바꾸지 않는다.
2. 기존 member/target V5 issuer→최고 Source/Output finisher→final/prior exact V9→기존 V9/CAS/ACK→M/committed-source continuation의 실제 순서를 완성한다.
3. cold restore/ACK loss는 원 operation/attempt/target/key의 durable phase로 복귀한다. 원 initial serial/borrow는 await/V9로 넘기지 않는다.
4. 기존 signed Store/terminal source-gate 회귀를 사용하고 obsolete wrapper 정리는 대표 caller와 동일 의미가 입증될 때만 한다.

## 입출력·오류·custody 계약

입력: P05 successor + retained original authority/state + 실제 installed worker/Store/published authority + prior ACK. 출력: 기존 admitted terminal receipt/opaque continuation과 same-operation checkpoint. 새 publisher·CAS·ACK engine 없음.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [ ] initial authority+Source/query/outcomes 단일 move. 중복 소비 거절, move 뒤 허위 Pending/Terminal0.
- [ ] 실제 Required member를 쓰고 target V5를 발급한 뒤에만 final 검사와 V9를 진행한다. 빈 Required/가짜 FULL/미게시 root로 우회0.
- [ ] before member/after member/before V9/after durable terminal-before ACK/ACK loss의 기존5 durable cut에서 같은 identity로 복구하고 중복 stage/visibility/CAS0.
- [ ] unbound Dispatching은 Pending; bound exact replay만 ACK/continuation. late first Source failure나 unsigned read에서는 write/continuation0.
- [ ] 실제 core_invalidator consumer까지 admitted ACK와 backing이 살아 있고 마지막 payload/DATA drop 뒤 환급된다.

## 검증 대상

기존 terminal_handoff_v3/tests.rs, pending_aggregate_replay_v2/native_restore_v3 source-gate 회귀, source_bound_replay_identity_owner_contract_test 재사용. 마지막은 installed SDK→Runtime→writer→ACK→checkpoint의 same-operation CP3다.

새 selector/feature class가 필요하면 existing owning QBC resolver에서 먼저 확정한다. 제안한 테스트를 실행된 proof로 세지 않는다. compile·실제 실행 본문·제품을 별도 보고한다.

## 제외·재사용

- 기존 finish/ingress/V9/ACK/Native M code와 엔진 재작성0.
- Contract part_3/schema 쓰기는 P01. 공급 producer 내부는 P05/기존 E01.
- PUB의 공개 ACK parent는 해당 PUB 담당과 P00의 최소 patch로만 합류.

## 착수 전 중복 확인

- 재조회 기준 S HEAD `7da55e44a0cea5fc25eceae65407f756bfe1de86`. 계획의 실제 write set만 대조했으며 claim은 잠금/함수 전체 소유권이 아니다.
- `QIDX-E02-DELTA-TERMINAL-BRIDGE-20261028`: actual write 경로 8개와 교차. 같은 함수/hunk·불변식·증명 경계가 겹치는지 착수 때 대조한다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
