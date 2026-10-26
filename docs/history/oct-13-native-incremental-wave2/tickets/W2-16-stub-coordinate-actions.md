# W2-16 — Stub capability와 좌표·AST·action fencing

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-16)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R27,R28 / NI-08. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: stable-ID/AST certificate port·UTF8/16·all-edit preflight·daemon/SDK fence. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

인증된 AST/Stub·좌표·item identity를 definition/usages/completion/refactor 및 atomic edit apply fence에 연결한다.

## Acceptance

- 실제 supported-language AST/Stub producer authority를 소비한다. missing/unsupported는 명시 거절하며 fixed fixture를 제품 parser로 계산하지 않는다.
- 독립 Unicode/UTF8/UTF16/CRLF edit interpreter gold와 view/document/revision/AST binding, 다른 session save/late Deep/stale AST를 검사한다.
- definition/usages/completion/refactor payload와 selected-row reorder, overlap/foreign edit 및 consumer apply 직전 원본 fence를 검사한다. stale multi-edit는 partial apply하지 않는다. UX06–08,11,12의 headless engine API가 범위다.

## Source verification contract

- The single SDK `stub(request, exact_source, budget)` API requires the caller to authenticate `exact_source` against the requested `binding.source`. The SDK and Lexical producer use one bounded Contract coordinate converter, including UTF8 scalar, UTF16 surrogate, CRLF and trailing-empty-line rules.
- Prepared replies must match the requested target, converted start/end and replacement bytes as an exact multiset, in non-overlapping byte order. Item membership and equal replacement strings alone are insufficient.
- `binding.source` identifies a SourceRecord object, not the SHA256 of raw source bytes. Passing a string is not a storage authentication proof. The source owner must compare the full current binding and source while holding its own apply/CAS guard, then commit all edits atomically. A valid preflight reply does not itself perform or certify that external CAS.

## Source entry points

- [native_result.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_result.rs)
- [plan.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/plan.rs)
- [ports.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-core/src/index/ports.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
