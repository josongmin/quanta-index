# W2-12 — Semantic chunk recipe·identity·incremental membership 구현

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-12)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R21 / NI-05. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: certified manifest/coordinate·per-chunk reuse·changed prior·projection binding SDK·제품 input 설정. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

기존 Semantic port 아래 논리 chunk identity/coordinates/input commitment/membership과 인증된 prior 재사용을 동일 selected view에 묶는다.

SourceAdvance/ProjectionAdvance의 prior는 동일 owner/projection/producer/input/policy/children 인증 후 재사용 후보로만 전달한다. payload/output bytes 동등성을 후보 전달의 필수 조건으로 두지 않는다. desired=new, covered=old 상태는 current query에서 제외하며 old artifact를 current로 rebind하지 않는다. per-chunk reuse와 product chunk 설정을 새로 만들지 않는다.

Native query assembly의 nominal alias는 underlying semantic payload/factory/ANN 비용을 대신 소유하지 않는다. S `published_authority_access.rs`의 vector payload copy→`published_semantic.rs` factory→기존 ANN backend 실제 allocation/custody를 같은 Original과 연결하는 범위는 이 티켓과 W2-13의 기존 owner가 소비한다. W2-02가 semantic engine을 재작성하거나 posthoc nominal grant로 완료 판정하지 않는다.

## Acceptance

- 독립 chunk boundary/logical ID/span/input commitment/membership bijection gold를 사용한다. ID는 ANN physical offset·layout/compaction과 독립이다.
- recipe/model 변경·unchanged per-chunk reuse·changed input의 인증된 prior·delete-before-late-job·delete/recreate/ABA 및 coverage 누락을 검사한다.
- 실제 Semantica certified producer→SDK→Native publish/query/delete/reopen·old pin에서 changed-only provider와 current coverage를 확인한다. ANN membership oracle는 W2-13이 소비한다.

## Source entry points

- [mod.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-semantic/src/native/mod.rs)
- [embedding.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-semantic/src/native/embedding.rs)
- [projection.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-kernel/src/projection.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
