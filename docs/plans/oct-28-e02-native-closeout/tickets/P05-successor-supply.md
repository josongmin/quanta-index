# P05 — Complete DELTA successor의 기존 bundle/issuer 실제 공급

최신 소스/담당 대조: 2026-10-30 KST. 최초 10-28의 installer 호출0 관측은 현재 TODO가 아니다. SDK seed→collector→pair→installer→terminal 호출 경로가 추가됐으며 실제 predecessor identity 공급/재인증과 수용은 기존 DELTA Root·B·L1–L4 담당에서 마감한다. **C Provider의 직접 재작업 목록에서 제외**한다. C는 이 경로의 owning 실행 결과를 수집하지 않았으므로 완료/실패를 발급하지 않는다. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: `QIDX E02 DELTA 게시·복구 구현`의 actual predecessor identity 공급·SDK/terminal 접합과 기존 E01/ItemIndex/Graph 생산 담당. QE 재인증은 `QIDX E02 B Trace·Evidence 및 재사용 구현`, L1 A–D·L2·L3·L4는 각 배정 범위를 유지한다. C 또는 새 E01 담당에게 중복 배정하지 않는다.
- 선행: 실제 E01 unchanged 재취득·dirty 교체·delete 제거·target Graph/CallGraph/Chunk 반환. ABI/fixture 준비는 다른 lane과 병렬이며, 실제 입력 부재는 이 공급의 D1 접합과 이에 의존하는 D2/D3 gate를 BLOCKED로 남긴다.
- 착수 조건: 기존 E01 담당에서 API/독립 oracle를 먼저 준비. typed 반환이 없으면 supplier gate만 BLOCKED로 남긴다.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## 현재 소스에서 확인한 사실

- `prepare_sdk_handoff_v3`→`sdk_successor_v3::finish_v3`→인증 ItemIndex seed→DELTA collector→`collect_and_install_native_delta_successor_v3`→`produce_native_delta_repo_map_pair_v3`→`install_complete_delta_successor_v3`→`finish_native_delta_entry_v3`의 소스 호출 경로가 있다. installer 정의만 있다는 이전 관측은 폐기한다.
- 실제 prior Args·QueryKey·options·HIR identity를 parent pin에서 대여하고 QE의 current-Source 재인증으로 소비하는 부분은 기존 담당이 현재 수정 중이다. 호출 경로 존재만으로 genuine complete target/D1 또는 compiler/body/제품 수용을 선언하지 않는다.
- finish_native_delta_entry_v3→prepare_complete_delta_members_v3는 이미 연결돼 있고 successor가 없으면 typed refusal한다. terminal 엔진을 새로 만들 일이 아니다.
- build_source_bound_repo_map_export_bundle_native_v3와 issue_native_repo_map_export_authority_v3는 이미 있다. semantic_state/native_successor_v3.rs는 semantic scope serializer이며 complete RepoMap supplier와 동일시하지 않는다.

## 수정 허용 파일·함수

| 수정 파일 | 허용 함수·hunk |
| --- | --- |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/repo_map_export_bundle/native_bundle_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/repo_map_export_bundle/native_bundle_v3.rs) | `기존 build_source_bound_repo_map_export_bundle_native_v3의 complete target 입력 반환 경계만` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/repo_map_export_authority/native_issue_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/repo_map_export_authority/native_issue_v3.rs) | `기존 target-bound issuer의 원 request/target 검증 및 소유 인계` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/batch_prepare/native_repo_map_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/batch_prepare/native_repo_map_v3.rs) | `complete_repo_map_bundle_v3의 기존 checked successor 공급 결합` |

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 구현 순서

1. 기존 E01 query owner가 반환하는 complete ItemIndex/HIR/Chunk/Graph/CallGraph와 unchanged paid carry를 소비한다. dirty-only rows를 FULL census로 승격하지 않는다.
2. 기존 final structured view의 실제 row를 유료 visitor로 빌려 projection/chunk/pending/completeness·retained ports와 target RepoMap/ImportGraph capability를 결합한다. 전체 query 재수집/DTO 역변환/새 request 발급은 하지 않는다.
3. 반환은 현재 installer가 요구하는 기존 (SourceBoundRepoMapExportBundleV1, SourceBoundRepoMapExportAuthorityV1)와 원 custody를 유지한다. 필요한 ABI는 producer→installer 양끝에 구체적으로 제시한다.
4. P06에 실제 생산 caller에서 installer로 넘길 최소 patch를 반환한다. custody_v3.rs/terminal_handoff_v3.rs 최종 쓰기는 P06뿐이다.

## 입출력·오류·custody 계약

입력: 원 DELTA request·same transition/target·complete Source witness·실제 E01 query-owned output. 출력: 기존 bundle+issuer, 필요한 retained Graph/CallGraph/Chunk custody와 exact target binding. metadata/accepted outcomes는 Required member capability의 대체물이 아니다.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [ ] upsert/delete/rename/recreate/no-op과 unchanged successor의 경로·ID/span/text/order/closure가 독립 cold-target gold와 일치한다.
- [ ] predecessor target/foreign request/누락 member·stale epoch·incomplete domain을 인계 전에 거절한다.
- [ ] 실제 생산 함수가 기존 installer를 거쳐 terminal successor slot을 채운다. helper/tuple 정의만 제출하지 않는다.
- [ ] Source 종료 전 data가 사라지지 않고, 늦은 거절 후에도 caller가 원 whole-failure·abort 책임을 갖는다.
- [ ] E01 private candidate가 Main 미적용이면 SourceReady=false/NOT_RUN을 유지한다. 적용되지 않은 supplier 패치를 구현 완료로 세지 않는다.

## 검증 대상

기존 E01 genuine query→planner→writer oracle와 target-bound RepoMap/import graph 회귀 사용. P06 consumer에서 exact request/target negative와 실제 install 호출을 owning test로 확인한다.

새 selector/feature class가 필요하면 existing owning QBC resolver에서 먼저 확정한다. 제안한 테스트를 실행된 proof로 세지 않는다. compile·실제 실행 본문·제품을 별도 보고한다.

## 제외·재사용

- E01 Source/Graph/IR·query solver·전체 snapshot collector·canonical bundle serializer 재개발0.
- terminal_handoff_v3.rs / custody_v3.rs 쓰기는 P06.
- FULL preparer 재호출로 DELTA request를 재발급하지 않는다.

## 최초 착수 전 관측 — 현재 배정은 위 10-30 경계 우선

- 재조회 기준 S HEAD `7da55e44a0cea5fc25eceae65407f756bfe1de86`. 계획의 실제 write set만 대조했으며 claim은 잠금/함수 전체 소유권이 아니다.
- 이 write set과 교차하는 활성 exact-path claim 없음. 기존 SSOT 담당과 현재 dirty hunk까지 확인해야 착수할 수 있다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
