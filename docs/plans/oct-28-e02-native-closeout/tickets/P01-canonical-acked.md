# P01 — 고정 4MiB Acked decoder의 중복 canonical 검증 절단

작성일: 2026-10-28 KST. 실행 지시용 패킷이며 신규 구현·검증 완료 기록이 아니다. 현재 실행 상태는 **NOT_RUN**. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: 기존 QIDX-E02-DELTA-TERMINAL-BRIDGE-20261028 canonical 담당. Runtime readback hunk는 기존 NATIVE-DEFERRED 담당과 함수 단위로 인계한다. P06과 같은 담당이면 P01→P06 순서로 실행한다.
- 선행: P00의 정확한 schema/Runtime hunk 분리. E01 complete successor나 원격 provider를 기다리지 않는다.
- 착수 조건: 활성 canonical 담당에게 붙이는 최우선 코드 패킷. 새 독립 canonical writer를 추가하지 않는다.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## 현재 소스에서 확인한 사실

- R34@14e87ec9: replay 76개 중 75통과/1실패, 컴파일 오류0. 실패는 private_identity_row_retains_closed_header_and_decoder_after_data_drop_v3의 Acked 양성이다.
- 당시 wire17150, decoder DATA171120, peak4188440에서 MemoryLimitExceeded. phase decoder 안에서 canonical attempts0–3 각각 complete12897/scratch537850/arena166208을 보존했고 다음 중첩 attempt가 거절됐다. 후속 row 재검증 전에 실패한다.
- Prepared receipt/artifact에는 NativeReceiptDecodedChildrenProofV3/NativeArtifactDecodedChildrenProofV3와 validated-fields serializer가 이미 있다. Canonical Core의 단일 사용 DATA를 최고 Source 전에 reset/replace하면 안 된다.
- 현재 원본 snapshot cache는 회수됐다. R34는 당시 소스의 실행 결과이며 현재 working tree의 재실행 결과는 NOT_RUN이다.

## 수정 허용 파일·함수

| 수정 파일 | 허용 함수·hunk |
| --- | --- |
| [packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/part_3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/part_3.rs) | `PublishedIndexCommitReceiptV1 / PublishedSourceBoundAggregateReceiptV1 validator와 canonical digest의 중복 child 검증` |
| [packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/terminal_receipts_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/terminal_receipts_v3.rs) | `SourceBoundTerminalAggregateReceiptV2 validator/digest의 동일 validated child 소비` |
| [packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/phase_validation_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/phase_validation_v3.rs) | `Acked phase의 child 완료와 교차 digest binding` |
| [packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/part_5.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/part_5.rs) | `closeout root의 같은 native source/phase binding` |
| [packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports_serde/field_schema_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports_serde/field_schema_v3.rs) | `새 published/terminal 전용 decoded-child policy arm; 기존 Prepared arm 불변` |
| [packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports_serde.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports_serde.rs) | `해당 schema와 기존 field serializer 연결; wire field/order 불변` |
| [packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/admitted_decode_v3/root_decode_data_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/indexing_writer_ports/admitted_decode_v3/root_decode_data_v3.rs) | `실제 완료 DTO에 결속된 scoped immutable readback 대여가 필요할 때만 확장` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/indexing_machine_v2/store/source_bound_replay_v2/canonical_json_v2/decoder_scratch_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/indexing_machine_v2/store/source_bound_replay_v2/canonical_json_v2/decoder_scratch_v3.rs) | `동일 완료 owner의 canonical JSON 비교` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/indexing_machine_v2/store/source_bound_replay_v2/row_readback_v2/admitted_identity_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/indexing_machine_v2/store/source_bound_replay_v2/row_readback_v2/admitted_identity_v3.rs) | `read_admitted_identity_with_peak_v3의 실제 completed decoder owner 소비` |

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 구현 순서

1. 실패 fixture·4MiB·최초 cause를 고정하고 manifest→published receipt→aggregate→terminal→Acked→root에서 어떤 validator/serializer가 같은 child를 재진입하는지 표로 확정한다.
2. 기존 decoded-child proof와 validated-fields helper를 확장해 동일 immutable child의 이미 완료된 검증만 재사용한다. parent의 format·Required set·receipt/ref/seal·checkpoint·scope/target 교차 조건과 work는 남긴다.
3. 공개 raw DTO/일반 serializer는 전체 의미 검증을 유지한다. 특정 Source에서 검증된 borrowed owner를 가질 때만 중복을 제거하고, 임의 DTO와 결합할 수 있는 공개 skip bool은 만들지 않는다.
4. Runtime은 같은 DTO·decoder partial·identity/header·funding을 함께 보존하며 highest Source 이후에만 record를 노출한다. Core serializer/별도 digest domain/다른 Source를 만들지 않는다.

## 입출력·오류·custody 계약

입력: 실제 Native decode가 완료한 동일 DTO 및 exterior canonical/decoder DATA. 출력: 기존 admitted identity record, 정확한 canonical wire 비교, 원 typed failure. API 확장은 opaque borrowed proof이며 게시 권한 발급이 아니다.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [ ] 원 Acked 양성이 같은 4MiB/fixture에서 통과한다. 더 작은 phase로 교체, 더 큰 예산, 테스트 제외/무시로 통과시키지 않는다.
- [ ] 기존 Ordinary canonical CBOR/JSON와 Native 결과의 byte/digest 동등성. 고정 gold 또는 기존 reference encoder를 oracle로 사용한다.
- [ ] 중첩 manifest/ref/seal/terminal/checkpoint digest 변조, duplicate/unknown field, foreign group, short work/memory, late cancel에서 record0·원 cause·partial 보존.
- [ ] DTO→decoder→마지막 private header alias의 양 drop 순서에서 조기 환급0, 최종 모든 owner drop 뒤 해당 root 사용량0.
- [ ] P09에서 원 replay 전체 target을 실행한다. R34의 필수76개를 유지하고 추가 테스트 수는 실제 관측값으로 기록한다.

## 검증 대상

1) owning Contract의 가장 작은 native validator/serde 회귀. 2) Runtime source_bound_replay_identity_owner_contract_test의 같은 Native73ab class 전체 target. 기능·gold·예산 변경 없이 최종 owning 결과를 받는다.

새 selector/feature class가 필요하면 existing owning QBC resolver에서 먼저 확정한다. 제안한 테스트를 실행된 proof로 세지 않는다. compile·실제 실행 본문·제품을 별도 보고한다.

## 제외·재사용

- 기존 Prepared receipt/artifact proof 재작성0.
- canonical single-use DATA reset, 성공 scratch 조기 해제, Source 밖 무상 재시도0.
- R34의 75통과를 전체 PASS로 승격하지 않는다.

## 착수 전 중복 확인

- 재조회 기준 S HEAD `7da55e44a0cea5fc25eceae65407f756bfe1de86`. 계획의 실제 write set만 대조했으며 claim은 잠금/함수 전체 소유권이 아니다.
- `QIDX-E02-DELTA-TERMINAL-BRIDGE-20261028`: actual write 경로 2개와 교차. 같은 함수/hunk·불변식·증명 경계가 겹치는지 착수 때 대조한다.
- `QIDX-E02-NATIVE-DEFERRED-INTEGRATION-20261028`: actual write 경로 1개와 교차. 같은 함수/hunk·불변식·증명 경계가 겹치는지 착수 때 대조한다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
