# W2-11 — 기존 Lexical planner의 Native token/BM25/regex/filter 실행 완성

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-11)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R15 / NI-04. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: Content/BM25/phrase/path/count·bounded NOT universe·bounded content/path regex와 authoritative source verifier; unsupported metadata/options early refusal. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

기존 Native planner/reader/postings에서 약속한 query capability의 의미·ranking·admission·work bounds를 보존한다.

## Acceptance

- 독립 token/ranking/tie/phrase/path/regex/filter/negative gold와 Unicode/empty/duplicate owner를 검사한다.
- negative-only universe는 bounded이며 regex는 authoritative source verification과 work cap/refusal을 사용한다. unbounded all-owner scan으로 지원을 가장하지 않는다.
- Native SDK/UDS FULL→delta→delete→old pin 및 corrupt posting/cancel/fanout/Excluded·owner/token 경계를 검사한다.

## Bounded regex contract

- At most four content regex leaves and four file/path regex filters, each pattern at most 4,096 bytes. Content regex uses a finite admitted owner census of at most 4,096, including positive-anchored queries; virtual overlay document count obeys the same census limit.
- Canonical typed tokens precede parser/HIR preparation admission. Retain pattern, HIR and engine grants through executor destruction. Charge estimated automaton states times scanned authoritative source/path bytes before verification; exhaustion refuses the whole query.
- Regex/path truth contributes no invented BM25 score. Existing positive Content terms, phrase positions, Boolean/NOT meaning and full matched count remain authoritative. Metadata filters without a producer authority return a typed refusal.

## Source entry points

- [text_query.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/native/text_query.rs)
- [plan.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/plan.rs)
- [text_reader.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/native/text_reader.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
