# W2-18 — Native 새-root rebuild/reselection·rollback과 default product 전환

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-18)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R35,R36 / NI-07. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: select/receipt/Verify/Serve·CAS switch/rollback·two-root rebuild. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

명시적 새-root rebuild/reselection·CAS switch/rollback과 default routing 전환을 기존 producer/boot authority로 수행한다.

격리된 candidate root의 rebuild/select/Serve/rollback 리허설은 W2-19 A 이전에 구현·실행할 수 있다. A는 그 결과를 소비한다. A 통과 뒤에만 실제 default routing을 바꾸고 B에서 default smoke/rollback·CI/release를 판정한다. 외부 producer 부재는 실제 producer target 인계 수용을 막으며 독립 boot/receipt/cross-root fixture까지 막지 않는다. 공통 순서는 [SSOT](../../../plans/oct-13-native-incremental-wave2/tickets/README.md#병렬-실행과-통합-장벽)만 소유한다.

## Acceptance

- authoritative producer의 FULL operation/target issuer 결과를 새-root rebuild→select/receipt→격리 candidate의 opt-in selection/Serve로 인계한다. 실제 default-routing CAS는 아래 A 이후 전환 단계에서만 수행한다. alias/foreign/symlink/incomplete root·활성화 전후 crash·cross-root rollback을 검사한다.
- W2-19 A 통과 뒤 default routing을 변경하고 경쟁 lifetime/head consumer를 퇴역한다. 이어 W2-19 B의 실제 default-route smoke/rollback·remote CI·release를 검사한다.
- 명시적 새-root rebuild/reselection 정책을 유지한다. Native backup/restore·제자리 추측 migration은 지원으로 추가하지 않는다.

## Source entry points

- [state_migration.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/state_migration.rs)
- [native_product.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_product.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
