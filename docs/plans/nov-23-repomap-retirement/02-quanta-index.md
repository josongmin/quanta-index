# 02 · Quanta Index 공개 계약과 서빙 퇴역

기존 Q Graph/SDK와 D/E08이 맡는다. 공개 제거와 Native 정책 결속은 소스에 반영됐다. 현재 후보에서 제품과 새 PID 실행이 남았다. [완료된 소스 계약](../../adr/NOV-23-001-repomap-retirement-and-source-custody.md#implemented-source-contracts)

## 현재 계약과 제거 단위

| 경계 | 남은 작업 | 소스 |
| --- | --- | --- |
| 후보 파일 | 미추적 Source export codec, SDK operation identity, old-wire 제품 회귀, admitted catalog decoder, Structural membership 구현, FD6 compact import codec·Source-only codec·Catalog preflight와 수정된 공용 serde를 참조 모듈과 같은 후보에 포함한다. exact S/Q·Cargo·설치 입력을 동결한다. | [export codec](../../../crates/quanta-index-contract/src/index/native_source_restore/native_source_frame_codec_v1.rs) · [operation identity](../../../crates/quanta-index-sdk/src/native_uds/operation_request_identity_v1.rs) · [wire fixture](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/retired_repomap_wire.rs) · [catalog decoder](../../../crates/quanta-index-contract/src/ipc/ingest/runtime_wire/catalog_admitted.rs) · [membership](../../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/dsl/structural_chunks.rs) · [FD6 codec](../../../crates/quanta-index-contract/src/native_cutover_evidence_v1/source_only_v6.rs) · [FD6 compact import](../../../crates/quanta-index-contract/src/native_cutover_evidence_v1/imported_native_wire_v1.rs) · [공용 serde](../../../crates/quanta-index-contract-base/src/native_serde.rs) · [FD6 preflight](../../../crates/quanta-index-searchd-runtime/src/native_boot/restore_forward/source_only_catalog_v7.rs) |
| 공개 wire와 제품 조립 | 로컬 daemon에서 구 Native 7개·Legacy 4개 프레임 거절, 유지 Graph·File/Path와 Legacy startup·구 CLI 거절의 두 exact가 통과했다. 서명 후보에서 공용 퇴역 입력의 dispatch 전 거절과 DB·operation·root 효과 0, 전용 store open/create 0을 확인한다. | [제품 UDS와 Legacy decoder 회귀](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/retired_repomap_wire.rs) · [유지 builders](../../../crates/quanta-index-searchd-runtime/src/native_product.rs#L2272) |
| 공용 경로의 우회 차단 | 올바른 root·peer·Source의 `ProjectionAdvance`, 관련 staging·resume·조회로 퇴역 ID를 거절한다. 정책에 식별 가능한 퇴역 owner/certificate 부착과 projection/event가 0인지 확인한다. 식별자 없는 불투명 객체는 정본 참조와 회수 여부를 본다. 독립 Graph의 게시·`QueryGraph`와 `QueryPaths`·`QueryRepositoryPaths`를 양성 대조로 실행한다. | [retirement gate](../../../crates/quanta-index-searchd-runtime/src/native_retirement_gate_v2.rs) · [제품 행렬](04-integration-acceptance.md#실제-본문과-통과-조건) |
| 로컬 검사 증거 | CodeSearch·TextMatches·Semantic의 기존 결과는 동일 출하 후보의 실행 판정에서 인수한다. | [기존 명령과 증거](04-integration-acceptance.md#검증-상태) |

## 단일 S/Q 비호환 출하

1. **승인된 FD6 이전 형식을 수용한다.** C-04/D의 확정 코드는 기존 AST-head의 V7 양성 fixture 1건이다. 별도 Legacy RepoMap root와 보호 route 없는 이전 Native는 승인된 이전 실물과 정책을 받은 뒤 지원 범위를 결정한다. [기존 fixture와 조건부 지원](04-integration-acceptance.md#c04-fd6-remaining-code)
2. **FD6 선택과 재정산을 설치에서 검증한다.** 같은 서명 설치에서 CAS, 원 요청의 ACK 유실 재정산, 새 PID의 Source·History, 첫 durable commit 전 실제 이전 Native 역전환 또는 승인 동등 복구를 확인한다. [완료된 V2 소스 계약](../../adr/NOV-23-001-repomap-retirement-and-source-custody.md#implemented-source-contracts) · [운영 수락 조건](04-integration-acceptance.md#c04-fd6-linux-acceptance-20261126)
3. **첫 결합 게시와 후속 BOOT을 설치에서 검증한다.** AST가 Catalog view를 전진시킨 뒤 실제 Linux 별도 PID 재시작 및 목적지 Source·History 이관을 실행한다. 원 canonical request ref, Catalog 선행 profile, 게시 결과 profile, 최초 History의 물리 rows·Original을 확인한다. exact view가 회수된 경우 현재 member가 직접 보존한 첫 History를 조회한다. 보존 floor 안의 누락, 다른 workspace/epoch, 최초 view와 직접 child 모두 부재, 외래 ref/profile은 거절한다. Q D01은 로컬 staged Source·History 강제 종료·재개 1/1을 확보했다. 해당 범위는 기존 증거를 인수한다. NC-H의 S production은 컴파일 중 중단됐고 CLI·Original replay·Required DATA는 해당 레인에 유지한다. [Q D01 강제 종료 1/1](../nov-24-native-enhancement/tickets/D01-durable-recovery.md#작업-범위) · [NC-H 기존 레인](../nov-12-native-cutover/tickets/NC-H-history.md#local-execution) 원 UDS·Kernel 결과의 범위는 ADR이 보존한다. [BOOT readback](../../../crates/quanta-index-searchd-runtime/src/native_boot.rs#L513) · [이전 증거](../../adr/NOV-23-001-repomap-retirement-and-source-custody.md#historical-evidence)
4. **History terminal과 FD6의 제품 경계를 검증한다.** Q 로컬 UDS에서 단독 History의 request GC 후 원 Resume·재Submit과 외래 view/profile 거절을 1/1 통과했다. 서명 설치의 exact-view 부재·퇴역 instance 거절을 남긴다. Pending은 원 body를 요구한다. 이전 Legacy 입력 fixture는 승인 binary·SHA가 필요하며, FD6 1 MiB의 경계와 368 head probe는 기존 D/E08에서 확인한다. [기존 범위](04-integration-acceptance.md#실제-본문과-통과-조건)


<a id="c04-f-release-approval-v2"></a>

## F의 퇴역 승인 readback

**F 검증기 구현 · VERIFIED.** v1 전용 schema 검사와 v2 live 재검증의 경로 누락을 공용 서명 도메인·선택 결속으로 수정했다. 기존 shipped fixture에서 수정 전 퇴역 v2 양성·경로 거절의 두 회귀가 schema 검사에서 실패했다. 수정 후 v2 활성 경로는 수락하고, 같은 bytes의 다른 선택 경로는 live 검증에서 거절했다. Q `28784da2692e` 공유 작업본의 F·승인 파일 네 개는 실행 전후 SHA가 일치했다. `uv run --frozen --offline --extra dev python -B -m pytest -q -p no:cacheprovider -k 'shipped or retirement_approval or signed_bundle' tools/ci/tests/test_check_tantivy_retirement.py tools/ci/tests/test_native_release_approval.py`는 **30 passed · exit 0**이었다. 실제 서명 설치는 **NOT_RUN**이다. [완료 계약](../../adr/NOV-23-001-repomap-retirement-and-source-custody.md#c04-candidate-and-selected-peer) · [기존 shipped 회귀](../../../tools/ci/tests/test_check_tantivy_retirement.py#L1412) · [실제 설치 입력](04-integration-acceptance.md#c04-fd6-v7-positive-fixture)

## DoD

현재 exact S/Q에서 구 wire·CLI·공용 퇴역 ID 효과 0과 Graph·File/Path·유지 SDK 양성을 확인한다. 후보 설치는 이전 Source·History를 보존하고 RepoMap payload·import workspace·보호 RepoMap route를 새로 만들지 않는다. 호환 설치의 공개 조회, 새 PID 재시작과 운영 선택은 [04](04-integration-acceptance.md)에서 판정한다.
