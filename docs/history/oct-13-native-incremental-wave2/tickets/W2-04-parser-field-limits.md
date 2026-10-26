# W2-04 — 필드별 decode 상한을 parser allocation 이전에 적용

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-04)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R11,R12 / NI-01. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: field-before-parser guard·preflight/attempt/thread/abort·input failure DATA. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

schema별 field bound를 실제 parser allocation/growth 전에 적용하고 decode attempt의 owner/thread/abort 계약을 유지한다.

## Acceptance

- 확정 oversized 길이는 field backing birth 전에 거절한다. 허용된 chunked/indefinite 입력은 cap을 넘는 grow 전에 거절한다.
- byte-array와 byte-string을 모두 검사한다. 기존 framing이 금지하는 형태는 그대로 거절하며, 허용된 chunked 입력의 cap 이내 선행 allocation을 위반으로 계산하지 않는다.
- text/bytes/sequence/option과 중첩 guard·owner clone·재진입·동시 decode·panic/cancel에서 bound와 sticky abort/first cause를 보존한다.
- 기존 CBOR acceptance·NFC identity 및 입력 pointer/grant 수명을 genuine Original과 해당 Q contract 회귀로 검증한다.

상한은 schema와 실제 backing에서 도출한다. 사후 length check는 preallocation 증거가 아니며 allocator-private overhead나 process RSS까지 이 계약의 보장으로 확대하지 않는다.

## Source entry points

- [admitted_decode.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract-base/src/admitted_decode.rs)
- [attempt.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract-base/src/admitted_decode/attempt.rs)
- [admitted_cbor_memory_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/original_native_memory_scope_v1/admitted_cbor_memory_v3.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
