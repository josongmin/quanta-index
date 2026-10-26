# W2-16 — 실제 AST/Stub authority와 apply 인계

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


상태는 [SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-16)만 갱신한다. headless 기능/API 범위다.

## Remaining scope

- 기존 `StubAstCertificatePort`에 실제 supported-language AST authority adapter를 연결하고 Runtime production caller에서 등록한다. missing/unsupported는 명시 거절하며 fixture를 실제 parser로 계산하지 않는다.
- 실제 source owner의 repo/caller를 명시해 apply 직전 원본 binding/bytes 확인과 전체 edits 원자 적용 계약을 인계한다. 외부 consumer 코드를 Index 내부 API의 미구현으로 바꾸거나 새 화면/앱을 추측해 만들지 않는다. consumer 인계가 없으면 그 제품 수용은 BLOCKED로 남긴다.

## Reuse and source

- [stub.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/native/stub.rs): trusted certificate port·stable ID·좌표/manifest 인증.
- [native.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native.rs): `configure_stub`; 조사한 Q production 등록 caller는 확인되지 않았고 테스트는 `FixedStubAst`를 쓴다.
- [native_uds.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/native_uds.rs): `stub(request, exact_source, budget)` 사전검증과 caller-authenticated source 계약은 이미 있다.

UTF8/UTF16·CRLF 공통 converter·all-edit preflight·Prepared exact tuple 검증은 재구현하지 않는다. `binding.source`는 SourceRecord identity이며 raw bytes SHA256이 아니다. SDK가 받은 문자열/Prepared 성공이 실제 저장소 인증·apply CAS 성공을 증명하지 않는다.

## Ownership and acceptance

실제 AST producer 담당은 adapter/private body, SEARCH는 기존 port 소비, 공용 writer는 Runtime parent 등록, 실제 source owner는 외부 apply hunk를 소유한다. 다른 SDK public parent와 병렬 덮어쓰지 않는다.

독립 Unicode/CRLF/UTF8 scalar/UTF16 surrogate·trailing empty line gold, stale/foreign/overlap·selected reorder·identity를 검사한다. source owner guard 아래 전체 현재 binding과 bytes를 확인하고 all-edits를 한 번에 적용한다. stale multi-edit는 partial apply0이다. API/fixture 선정 결과와 실제 AST 공급/외부 apply 수용은 각각 판정하며 W2-19가 소비한다.

## Implementation packet

- **착수:** 실제 지원 언어 AST producer와 source-owner apply caller/repo를 먼저 식별한다. 기존 담당이 없거나 caller가 제공되지 않으면 그 외부 인계만 BLOCKED로 기록하고 가짜 parser/consumer를 만들지 않는다.
- **독립 구현:** AST producer 담당은 실제 인증 adapter/private body, SEARCH는 기존 certificate port 소비를 수정한다. Runtime `configure_stub` 등록은 공용 writer에게 exact 생성자/구성 hunk로 인계한다.
- **apply 인계:** 기존 source owner가 full binding/bytes를 guard 아래 비교하고 all-edits를 원자 commit하는 경로를 제공한다. SDK preflight body/좌표 converter는 유지한다.
- **정적 완료/반환물:** 실제 language/producer/등록 caller·source-owner 경계, AST→binding→좌표→edit tuple의 대응, 거절/partial apply0 oracle·영향 fixture. ANN/producer 개발과 독립 진행 가능하며 public Runtime/SDK parent 결합만 직렬 처리한다.
