> HISTORICAL — pre-migration document from `docs/plans/oct-17-index-closeout/tickets/W2-16-stub-authority.md`. Current work: [Oct-22 SSOT](../../../../plans/oct-22-finalizing/README.md). Old TODO/status/counts do not create work or qualify current source.

# W2-16 — 실제 AST/Stub authority와 apply 인계

상태는 [SSOT](README.md#w2-16)만 갱신한다. headless 기능/API 범위다.

## Remaining scope

- 기존 `StubAstCertificatePort`에 실제 supported-language AST authority adapter를 연결하고 Runtime production caller에서 등록한다. missing/unsupported는 명시 거절하며 fixture를 실제 parser로 계산하지 않는다.
- 실제 source owner의 repo/caller를 명시해 apply 직전 원본 binding/bytes 확인과 전체 edits 원자 적용 계약을 인계한다. 외부 consumer 코드를 Index 내부 API의 미구현으로 바꾸거나 새 화면/앱을 추측해 만들지 않는다. 실제 적용 주체는 Semantica buffer source owner로 확정하며, 아래 cross-repo 구현 잔여를 외부 caller 지정 대기로 돌리지 않는다.

## Reuse and source

- [stub.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/native/stub.rs): trusted certificate port·stable ID·좌표/manifest 인증.
- [native.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native.rs): `configure_stub`; `configure_published_stub`와 Native product의 명시적 `stub_ast` 정책 구성이 실제 pinned projection reader를 등록한다. independent fixture의 grammar를 실제 parser provenance로 계산하지 않는다.
- [native_uds.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/native_uds.rs): `stub(request, exact_source, budget)` 사전검증과 caller-authenticated source 계약은 이미 있다.

UTF8/UTF16·CRLF 공통 converter·all-edit preflight·Prepared exact tuple 검증은 재구현하지 않는다. `binding.source`는 SourceRecord identity이며 raw bytes SHA256이 아니다. SDK가 받은 문자열/Prepared 성공이 실제 저장소 인증·apply CAS 성공을 증명하지 않는다.

## Ownership and acceptance

실제 AST producer 담당은 adapter/private body, SEARCH는 기존 port 소비, 공용 writer는 Runtime parent 등록, 실제 source owner는 외부 apply hunk를 소유한다. 다른 SDK public parent와 병렬 덮어쓰지 않는다.

독립 Unicode/CRLF/UTF8 scalar/UTF16 surrogate·trailing empty line gold, stale/foreign/overlap·selected reorder·identity를 검사한다. source owner guard 아래 전체 현재 binding과 bytes를 확인하고 all-edits를 한 번에 적용한다. stale multi-edit는 partial apply0이다. API/fixture 선정 결과와 실제 AST 공급/외부 apply 수용은 각각 판정하며 W2-19가 소비한다.

## Implementation packet

[W2-05 C](W2-05-producer-full-delta.md#native-publication-caller)의 실제 committed Source/owner/path를 Root가 기존 AST composition에 넘기고 공개 SDK parent는 PUB가 결합한다. 기존 signed queue/Stub route·copier·apply 엔진을 다시 만들지 않는다. 후속 명령은 자체 genuine Current/Original을 사용하며 [W2-19](W2-19-integration-acceptance.md#combined-checkpoints)가 실제 queue/restart/Full supersession·ONE Delta/CAS/drop 결과를 소비한다.

- **착수:** 아래 확정 경로의 producer/consumer·IR·정확한 guarded apply 인계를 한 계약으로 묶는다. 기존 S parser와 buffer owner가 실제 구현 주체다. 등록 포트만 만들거나 가짜 인증자를 붙여 완료 처리하지 않는다.
- **독립 구현:** AST producer 담당은 실제 인증 adapter/private body, SEARCH는 기존 certificate port 소비를 수정한다. Runtime `configure_stub` 등록은 공용 writer에게 exact 생성자/구성 hunk로 인계한다.
- **apply 인계:** 기존 source owner가 full binding/bytes를 guard 아래 비교하고 all-edits를 원자 commit하는 경로를 제공한다. SDK preflight body/좌표 converter는 유지한다.
- **정적 완료/반환물:** 실제 language/producer/등록 caller·source-owner 경계, AST→binding→좌표→edit tuple의 대응, 거절/partial apply0 oracle·영향 fixture. ANN/producer 개발과 독립 진행 가능하며 public Runtime/SDK parent 결합만 직렬 처리한다.

## Decided product path

```text
Semantica exact grammar/parser + canonical symbol identity
  → per-source AST certificate output
  → existing ProjectionOwnerContribution + complete affected transition
  → selected Q view / exact source / pinned certificate verifier
  → existing Stub Prepared byte edits
  → Semantica frozen source owner + UTF16 Delta + atomic buffer CAS
```

- **확장 경계:** [ProjectionBinding/ProjectionOwnerContribution](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/index.rs)과 [projection apply](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-kernel/src/projection.rs)의 source-owner/source-ref·complete affected-domain·object child/GC 계약을 재사용한다. 기존 StageProjectionOutput/Owner/Certificate·ReadProjection wire가 있다. SourceRecord child 규칙이나 event frontiers를 AST 보관 필드로 바꾸지 않는다. 별도 SourceLocal projection/profile member가 인증 출력을 소유한다.
- **인증 IR:** 새 AST artifact payload에는 exact SourceRecord ref, language, full grammar fingerprint, producer/schema 및 실제 parser/grammar build 정책과 정렬된 item의 full stable identity/AST digest/byte span/label을 묶는다. [TreeSitterParserPort](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/infra/parsers/src/engines/tree_sitter/parser.rs)가 실제 문법/노드를 생산한다. AST digest는 실제 grammar와 canonical node contents/subtree를 고정 domain으로 묶어야 하며, 현재 SymbolRecord의 이름/범위를 AST 증거로 간주하지 않는다.
- **stable identity:** [stable_symbol_id128_v2](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/core/codegraph-lang-common/src/stable_symbol_id_v2.rs)의 language/canonical FileId/semantic symbol path CBOR preimage에 대한 원래 full BLAKE3-256을 재사용한다. 기존128비트를 padding/재해시해 full identity로 꾸미지 않는다. full hash 노출과 기존 issued Java parse의 opt-in certificate 생성 경로를 사용한다. 상위 source/publisher 결합과 실제 parser 수용은 별도다.
- **읽기 비용/수명:** 요청의 pin된 `(view, source, projection, policy)`에서 certificate를 한 번만 admitted decode해 item lookup과 같은 요청의 preflight가 공유한다. items마다 object lookup/JSON decode를 반복하거나 무제한 전역 cache를 만들지 않는다. bounded session/DATA와 실제 backing을 Source finisher·최종 drop까지 유지한다. old/missing/foreign grammar·source·owner·projection은 명시 거절한다. 인증 projection이 같은 selected SourceCut에 게시되기 전에는 Stub을 성공시키지 않는다.
- **실제 apply:** [source_edit_pipeline_v1](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/ide_source_command_executor_v1/source_edit_pipeline_v1.rs) 및 [Delta dispatcher](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/ide_source_command_executor_v1/command_dispatch_v1.rs)로 전달한다. frozen S writer session/path/incarnation/base version/expected head와 Q full binding·current bytes를 source-owner guard 아래 확인하고 기존 byte→UTF16 converter로 ONE Delta를 제출한다. Q overlay revision과 S DocumentVersion은 서로 다른 축이다. stale이면 conflict/partial apply0이며 자동 rebase하지 않는다.
- **독립 검증:** 실제 grammar/subtree fixed gold, 기존 full stable hash의128-prefix 관계, malformed/duplicate/missing/foreign cert, projection old/new affected closure·delete/no-op, 요청당 decode1회·allocation/read 거절·cancel/unwind/refund, Unicode/CRLF/UTF16 및 stale multi-edit atomicity를 각각 검증한다. parser/cert publisher·Q adapter/등록·S guarded apply가 모두 붙기 전에는 정적 설계를 기능 구현으로 올리지 않는다.

## Cross-module contract and implementation boundaries

- **원 node와 identity:** [ScopeSymbolIdentityV1](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/core/codegraph-cfg-dfg-kernel/src/ports/language_plugin_scope_types.rs)는 node/span 자체를 보유하지 않는다. parser callback에서 canonical semantic path와 exact syntax node/span을 동시에 포착한다. 이후 이름/label로 노드를 재탐색하면 overload·동명이인을 잘못 결합할 수 있다. 기존 Native full-hash 생산 결과를 노출하고128-bit prefix 관계를 고정 golden으로 검사한다.
- **문법 provenance:** [GrammarFingerprint](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/infra/parsers/src/contract/types/grammar.rs)의 version 문자열·optional hash만으로 grammar bytes를 입증하지 않는다. hash가 없는 기본값을 임의의 전체 grammar hash로 승격하지 않는다. 실제 사용 grammar package/build의 승인된 policy digest와 fingerprint를 함께 바인딩하고 승인 provenance가 없으면 거절한다. 현재 Java 정책은 locked registry package provenance이며 binary attestation이 아니다. parser port의 일반 engine fingerprint/ABI만으로 문법을 인정하지 않고 실제 retained Tree의 language가 고정 Java grammar와 같은지 확인한다. 정확한 hash domain·node-kind/relative-tree/content 입력·정렬·empty/delete·bounds는 공용 IR 결합에서 producer/reader의 한 계약으로 확정한 뒤 병렬 착수한다.
- **두 owner의 구분:** projection contribution의 owner는 projection producer를 사용한다. `OwnerKey { producer: AST_PRODUCER_ID, stable_id: source_owner.stable_id }`로 대응시키고 원 source owner와 SourceRecord ref는 별도로 보존한다. source owner를 그대로 contribution owner로 복사하면 Kernel의 `owner.producer == contribution.producer` 불변식에 어긋난다. 경로→source stable ID의 기존 frozen policy를 재사용하며 별도 path hash를 만들지 않는다.
- **후속 view join:** [native_stub_transport.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_stub_transport.rs)의 `saved_frontier`는 원 save receipt와 후속 AST projection view를 결합한다. 저장 receipt의 document/session/revision/source 인증은 유지하고, 요청 pin view의 같은 workspace/epoch·비퇴행 sequence·exact owner→SourceRecord 및 projection의 current SourceCut/policy를 함께 검증한다. 삭제/교체·future/foreign·epoch/ABA는 계속 거절한다. 기존 Source CAS receipt를 projection view로 다시 쓰지 않는다. 과거 selected-view 등치 구현 설명을 신규 미구현으로 재배정하지 않는다.
- **의존성 방향:** parser/language adapter가 AST를 생산하고 retrieval-kernel private publisher가 기존 QSDK staging을 호출한다. 두 기능의 composition은 기존 상위 Runtime/SDK owner에 둔다. retrieval-kernel에 parser 의존성, Q daemon에 언어 parser, 별도 AST 재파싱 경로를 넣지 않는다.
- **요청 수명:** 최대128 item과 prepare/preflight 재확인이 같은 bounded attestation session/DATA를 공유한다. lookup마다 전체 certificate를 다시 decode하지 않는다. Source finisher 이전 이동·backing 분리·ordinary fallback이 없어야 한다. pin/source/certificate child backing을 마지막 소비/거절·unwind 후까지 보존한다.

독점 구현 패킷은 S parser+publisher, Q private reader, S private guarded apply의3개다. Q Contract IR·Runtime 등록/profile·saved-frontier와 S 상위 orchestration/parent/export는 기존 통합자만 수정한다. private 파일 경로는 제안이며 현재 claim/hunk 인수 전에는 수정하지 않는다. Java를 첫 실제 관통 대상으로 삼되 다른 supported language를 완료로 계산하지 않는다.

기존 oracle에 더할 판별 사례는 duplicate-name/overload의 node↔ID 고정 대응, 동일 source의 후속 projection view 허용, source 교체/삭제/epoch·stale certificate 거절, grammar policy 교체, zero-output/delete complete transition, 요청당 decode1회 및 실패 후 최종 환급, Unicode/CRLF·stale multi-edit 부분 적용0이다. 현재 상태와 실행 결과는 각각 README와 VALIDATION에만 둔다.

## File packets for the next implementation checkpoint

공용 IR은 기존 Q Contract Stub namespace의 private child로 한 번 정의한다. 새 persisted AST output과 기존 Stub request를 구분하며 Native18 저장 포맷·SourceRecord schema는 유지한다. 기존 ReadProjection은 전체 binding만 반환해 delta CAS에 필요한 owner별 이전 contribution을 알 수 없다. 이를 위해 bounded LookupProjectionOwners 요청(tag38)/ProjectionOwners 응답(tag31)을 같은 exact pin·admission·canonical decoder에 추가한다. source-ref와 contribution-ref를 서로 다른 typed row 필드로 유지하며 최대128개 정렬·유일 owner와 historical view를 검사한다. 아래 신규 파일 이름은 구현 제안이며 현재 claim 확인 후 확정한다.

| 담당 | 기존 파일/hunk | 기존 private 모듈 / 통합 경계 |
| --- | --- | --- |
| A / S 생산 | `packages/core/codegraph-lang-common/src/stable_symbol_id_v2/native_hash_v3.rs`, `packages/core/codegraph-lang-java/src/language_parser_plugin/identity_extract/traversal_v1.rs`, 기존 parser callback | parser 측 certificate extraction child; `packages/analysis/quanta-v2/crates/quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_ast_certificate_publish_v1.rs`의 staging body. parser→publisher composition은 통합자 |
| B / Q 읽기 | 기존 `StubAstCertificatePort` 구현 경계·projection read | `crates/quanta-index-searchd-runtime/src/native_stub_certificate_projection_v1.rs`. `native_stub_transport.rs`의 session/saved-frontier와 `native.rs` 등록은 공용 writer |
| C / S 적용 | `quanta-sdk-runtime-executor/src/client/ide_source_command_executor_v1/source_edit_pipeline_v1.rs`·기존 buffer authority Delta admission | 같은 IDE 모듈의 기존 `native_stub_apply_v1.rs`. command dispatch/SDK parent는 기존 owner 인계 |

full stable hash 노출은 기존 해시 함수의 생산 결과를 공유하며 별도 preimage/재해시를 만들지 않는다. 각 패킷의 private body와 독립 golden을 먼저 합류하고 부모/export/Cargo는 repo 통합자가 소비자와 함께 수정한다. 실제 배정·상태·순서는 README만 갱신한다.

## Durable parser composition and later command authority

- Source operation은 ACK 전에 immutable V5와 exact Source publication을 durable AST queue에 인계한다. AST는 독립 operation이므로 실제 producer 재진입의 Original과 같은 committed snapshot의 Source 권한을 사용한다. 선택적 AST 예산 거절을 Source ACK의 선행 조건으로 만들거나, 완료한 FULL의 keeper/deadline을 재사용하지 않는다. 기존 V9Lifecycle의 보존 계약을 소비하며 별도 pin ledger를 중복 생성하지 않는다. 향후 cleanup retirement에는 미완료 AST 의존성을 포함해야 한다.
- Source publication view는 immutable Source/V5 증거이며, AST `UpdateRequest.expected_view`는 실제 Q 게시 CAS basis다. 최초 bind에서 같은 workspace/epoch·같은 SourceCut의 후속 basis를 사용할 수 있다. 같은 sequence의 다른 digest, 퇴행 basis, 다른 SourceCut은 거절한다. 이 조건은 store의 후보 검증이며 실제 Q exact view/Source 인증은 기존 publisher가 수행한다.
- Bound 뒤에는 operation/event와 canonical request bytes를 변경하지 않는다. 재시작은 저장된 요청을 Native DATA로 복원하고, terminal 처리는 그 요청과 SQL claim을 다시 대조한 뒤 bound basis보다 뒤의 실제 publication을 확인해 event frontier를 전진시킨다. Source 증거를 최신 view로 덮거나 새 operation으로 우회하지 않는다. SourceCut 자체가 바뀐 경우는 별도의 complete affected-domain/coalescing 증거가 필요하다.
- registered Java/source-loan adapter와 AST publisher 모듈을 실제 upper Native prepare가 소비해야 한다. 실제 parser path/bytes/snapshot·grammar 증거와 output backing을 같은 Original에서 결합하고 aggregate의 immutable evidence와 strict ProjectionAdvance request/frontier를 persist/replay까지 이어야 한다. publisher 정의나 generic bind callback만으로 source completeness를 발급하지 않는다. 기존 SourceAdvance/Structural/RepoMap payload를 다른 의미로 바꾸지 않는다.
- `apply_native_stub_issued_v4`는 현재 issued V4가 발급한 genuine Current/Original과 exact accepted buffer snapshot을 사용한다. FULL 결과는 고정 publication/root 증거만 공급한다. FULL 절대 deadline/keeper 재사용, caller가 만든 root/bytes/observed-head, ordinary retained-source getter 우회는 허용하지 않는다. generic retained Native history reader 지원은 이 accepted-buffer 경로의 필수 구현과 구분한다.
- 후속 Current의 root 검사는 FULL의 유일한 sealed constructor가 인증한 고정 workspace digest/view를 사용한다. 과거 ACK DTO의 decoder grant를 새 Original의 소유물처럼 검증·순회하지 않는다. 실제 ACK 검사와 backing 유지는 FULL owner에 남고, Current는 자신의 root hash·선택 view·IPC 및 apply 비용을 지불한다. 서로 다른 Original grant를 허용하도록 소유권 검사를 완화하지 않는다.
