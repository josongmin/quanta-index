# W2-19 — 검증 잔여·통합 A/B·출시

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


상태는 [SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-19)만 갱신한다. 기존 구현의 미수용 검사만 모은 티켓이며 새 feature/producer/issuer/runner 개발 큐가 아니다. 새 정합 실패는 exact caller·원인에 한정해 수리한다.

Oct-18 SEARCH integration hunk: only the `history-text-epoch-manifest` row
in `tools/ci/inventory/wire-surface.toml` is reconciled with the existing
`history_text_index/manifest.rs` format3 producer/decoder and its typed
format1/2 refusal. No Lexical implementation or migration oracle is changed.
The inventory guard verifies metadata agreement; it does not qualify the
existing replay, durability or migration behavior.

## Contract hygiene join

The original digest/decode writers own the Contract strict and API join:
`source_coverage.rs`, `index.rs`, admitted corpus/auxiliary/receipt decoders,
`query/native_content.rs`, `repomap/admitted_v3.rs`, `lex/history.rs`, and
the existing `NativeIndexRequest` enum-size hunk. Exact diagnostic sites are
retained in [the result-contract validation record](../../../plans/oct-10-index-closeout/VALIDATION.md#w2-13-traversal-contract-2026-10-18).
Preserve admission-before-work, original error propagation and closed wire
shape when replacing unchecked operations or resolving lint-policy conflicts.
Do not add blanket lint allowances, weaken gates, or duplicate the decoders.

Review the generated Catalog digest/borrowed auxiliary/RepoMap/scratch API
changes with their original callers before accepting the remaining baseline
delta. W2-13 accepts only its traversal type and durable/overlay result fields.
After the original writer changes, consume one same-source strict/API result;
do not rerun unchanged selectors to relabel this join as qualified.

## Single ownership of acceptance

| 기존 범위 | 남은 검사·소비할 원 oracle | 원 담당 |
| --- | --- | --- |
| W2-01/02/03/04 | actual Original IO/parser·canonical wire·snapshot Ready/Degraded·비UTF8·3+owner parity·거절 before-birth/write0·backing/drop/refund·work | INT 기존 owner fixture |
| W2-05/06/12/14 | actual FULL/delta/delete/rename/recreate/no-op·old/new closure·Required7/seal·semantic/graph certified output·old view·ACK-loss/restart·exact Unknown/Pending | SDK producer + INT + SEARCH 수신; 결과는 W2-05에서 생성 |
| W2-07 | 기존 physical fault/custody·original FD close·pin/retirement·WAL/GC·공용 inventory·실 운영 압박. 새 ANN caller fault는 W2-13 결과 소비 | PHYSICAL |
| W2-08/18 | 실제 producer 출력 boot/Verify/Serve·receipt close fault·격리 rollback/restart·default route | BOOT; W2-18 결과 소비 |
| W2-11/15/17 | lexical capability별 fixed gold/typed refusal·bounded query, overlay/save2session·mask-before-top-k·stale/late/close/cancel, Auto seq/TTL/revision·fairness/비협조 cancel charge·지속 부하 | SEARCH; 기존 query/overlay/Auto 구현 유지 |
| W2-13/16 | selected vectors·membership·recall/progress·파일 수명, 실제 AST authority·source owner 원자 apply | 각 티켓의 원 oracle 소비 |
| W2-09/10 | 같은 source/profile 비용·Off/Record·지속 churn/pin/space·aggregate RSS/temp disk·debt/drain·trace capture/UI | harness/source owner; W2-09 결과 소비 |
| legacy/consumer/release | A/D/C5 완료 유지, 미수용 legacy LQ5/semantic/test-platform·installed/pair/provider/actions·remote CI/release | 기존 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md) oracle/consumer owner |

정식 R01–R40/P2I01–08/UX01–12/AUTO01–10은 기존 RFC/독립 gold에 결합한다. conditional AUTO/외부 consumer 범위를 명시하고 새 GUI 요구로 바꾸지 않는다. 겹치는 테스트/수치는 한 번 실행·기록하고 이 표는 그 결과를 참조한다.

## Release inputs and staged proofs

[proof-authority.toml](../../../../tools/ci/proof-authority.toml)의 아래12개 `staged` node는 기존 release 조건이다. registry는 선택된 proof의 readiness/의존성을 소유하고 **실행 계획·담당·다음 작업은 이 티켓/README만** 소유한다. 현재 stage 선언만으로 모든 관련 테스트/제품 코드가 없다고 단정하지 않는다. 실제 target/feature/fresh-binary binding을 먼저 대조하고 부족한 exact target/observer만 원 담당이 수리한다.

| 기존 proof 경계 | 남은 수용/입력 | 담당·중복 방지 |
| --- | --- | --- |
| `p03-candidate-activation`, `p04-read-view-lifetime`, `p05-query-truth`, `p06-sdk-binding` | Linux fresh-release daemon의 activation/recovery·실 pin/GC·query quality/native response·actual SDK binding과 source/binary/raw inventory | 원 activation/lifetime/query/SDK oracle. owner-unit/trace identity만으로 process/release 판정 금지 |
| `p07-provider-boundary` | 승인된 실제 provider identity·egress·budget/cancel 입력과 release target | provider/consumer owner. 입력 부재는 BLOCKED이며 다른 Index 코드/기능 검사를 막지 않음 |
| `p08-runtime-supervisor`, `p09-control-readiness` | release-daemon signal/child-loss/FD residue·component-loss/readiness·bounded request diagnostics의 실제 process target/binding 및 Linux 실행 | 기존 Runtime/PHYSICAL process owner. owner tests/target은 존재하므로 전체 supervisor/control 재개발 금지 |
| `p10-state-migration` | exact-source current-format backup/restore/verify·old-root refusal·rollback qualification와 P09 의존성 | BOOT/PHYSICAL 기존 legacy owner. 새 Native backup/restore나 retired migration importer를 요구하지 않음 |
| `p11-cross-repo-cutover` | clean exact producer/consumer pair·resolver lock·attested daemon·terminal receipt와 전체 선택 proof inventory | SDK/INT/BOOT와 기존 cross-repo recipe. runner-candidate-only는 exact-pair proof가 아님; R3 omission/cold-restart는 W2-05 결과 소비 |
| `p11-deployment`, `p11-activation`, `p11-rollback` | 구체적인 승인 Linux target·실 action adapter·독립 pre/post observer 계약·같은 host/config/state/source 연속성 | 기존 운영 target/consumer owner. 미지정 target은 BLOCKED. generic runner·target issuer·실제 배포를 추측해 생성하지 않음 |

P12A infrastructure는 P11 운영 입력과 독립이다. 최종 aggregate의 `--require-all --bind-source`는 선택된 전체 prerequisites를 요구한다. `CODE_QUALIFIED`·`DEPLOYED`·`ACTIVATED`·`ROLLBACK_PROVEN`은 별도 판정이다. [CircleCI 계약](../../../adr/SEP-28-001-circleci-provider-and-credit-boundary.md)의 exact-source hosted run·test-authority artifact를 B/release에서 확인하고 과거 green·local replay·벤치 컴파일을 새 hosted/성능 수용으로 바꾸지 않는다.

## Combined checkpoints

1. **정적 결합:** 현재 공개 API/module·parent/feature·actual caller/권한/취소/drop와 고정 gold를 대조한다. 공용 facade/lock/callsite 수리 저장은 동작 green이 아니다.
2. **영향 검증:** Q는 Justfile/`./scripts/cargow`, S는 기존 owning QBC/actual genuine Original fixture closure로 필요한 strict/tests를 묶는다. lib(test) 필터는 전체 compile closure를 줄이지 않는다. 기존 실패 뒤 다른 필터/축소 feature로 반복하지 않는다.
3. **A / pre-switch:** 실제 producer→searchability·복구·fault/pressure·ANN/headless API·격리 rebuild/rollback과 고정 profile의 제품/비용/운영 조건을 candidate에서 수용한다.
4. **W2-18 switch:** A 뒤에만 실제 default route를 전환한다.
5. **B / post-switch:** default smoke/rollback·최신 coherent integration/full workspace·공개 API/module baseline·remote CI/release를 별도로 판정한다. release를 A의 선행 조건으로 만들지 않는다.

## Existing source/result boundaries

[VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md#native-required-readback-code-checkpoint-2026-10-15)의 source별 결과를 유지한다. 기존 CP43/43·Q41/41·S DTO20/20, SEARCH245/245/UDS39/39·RepoMap51/51/UDS6/6, PHYSICAL151/151, harness45/45 등은 서로 다른 source/selector다. 이를 합산해 현재 workspace/제품 통과로 계산하지 않는다. lock/API/공용 facade 실패와 수리 후 미검증 경계는 새 실제 결과가 나올 때까지 유지한다.

expected는 fixed bytes/digest·reference/model·독립 관측에서 얻는다. 구현식을 다시 계산해 oracle로 삼거나 missing/stale/partial/timeout을 성공으로 만들지 않는다. routine focused 검사에는 별도 receipt/snapshot을 요구하지 않는다. formal replay/release/qualified benchmark만 필요한 source/input/dependencies/config/binary/environment를 결합한다. 모든 실행 결과는 기존 VALIDATION에 한 번 기록하고 새 결과 원장을 만들지 않는다.

## Integration packet

- **담당:** 각 저장소의 기존 통합자 한 명이 공용 parent/export/schema/lock·생성 inventory/공식 selector를 소유한다. 다른 티켓의 private body를 동시에 재개발하지 않는다.
- **G0 / 계약·배정:** 실제 caller/authority·함수 서명·exact write hunk·독립 oracle를 확정한다. 기존 다른 채팅 실행/변경은 인계 없이 재제출/인수하지 않는다.
- **G1 / 정적 합류:** 패킷별 producer→consumer·binding/CAS·grant/backing/FD 수명·cancel/error/drop 및 public API/module/feature 정합을 검토한다. 이 단계까지 compiler/daemon의 반복 실행을 작업별로 분산하지 않는다.
- **G2 / 영향 검증:** 같은 결합 source에서 필요한 fmt/구조·strict·fixed-gold/owner tests를 정식 도구로 묶는다. 첫 원인을 수리한 경우만 영향 배치를 다시 판정한다. 필터 변경으로 compile closure를 우회하지 않는다.
- **G3 / 제품 합류:** 실제 UDS/physical pin/reopen→실 SDK FULL/delta/domain→crash/ACK loss·headless 기능·격리 boot를 원 oracle로 검사한다. 각 결과는 한 번 기록하고 티켓 간 재사용한다.
- **G4 / A:** 동결 입력의 ANN 품질·총비용·Off/Record·운영 압박과 candidate 기능을 수용한다. 외부 provider/target 입력 부재는 해당 release subgate만 BLOCKED로 분리한다.
- **G5 / switch/B:** A 뒤 BOOT가 default를 전환하고 default smoke/rollback·coherent workspace·exact-pair/Linux/hosted CI/release를 판정한다. 실제 운영 action은 승인 target/observer 계약이 있는 범위만 실행한다.

반환물은 각 requested scope의 명령·관측 결과·source 범위·VERIFIED/FAILED/BLOCKED/NOT_RUN 판정과 남은 exact blocker다. 완료율 추정·서로 다른 source의 테스트 수 합산·새 receipt/결과 원장 생성은 하지 않는다.

## Existing funded SDK receiver handoff

- 원 writer는 `QIDX-W2-02-FINAL-QUERY-ASSEMBLY-20261017`이다. root는 selector의 누락된 borrowed lifetime만 보완했다. 새 connector/allocator/reference counter를 만들지 않는다.
- 실제 결합 대상은 S `quanta-runtime-retrieval-kernel/src/index_sdk_ingress/{facade,native_connect_v3,query,repomap,publish,control}.rs`의 `client_ref`와 RPC 소비자다. 기존 ordinary/default handle과 funding을 포함한 handle의 공개 범위를 구분한다.
- `NativeSharedValueV3<Payload,F>`는 funding과 Root를 보호하는 scoped unit access를 제공한다. generic `Deref` 복구·payload clone·grant 분리·Native branch의 임의 unsupported 처리로 compiler를 통과시키지 않는다. 같은 writer가 receiver와 실제 소비자를 함께 결합한다.
- Q의 `NativeSdkConnectDataV1::complete_shared_v1`은 모든 opaque `H`에 대해 완성된 header만 빌리는 기존 수신 포트다. 새 Q connector/accessor를 만들지 않는다. S의 `NativeFunded` facade는 `H: Deref`에 의존하는 `client_ref_v3` 대신 이 header와 원 Source의 scoped unit access를 실제 RPC 소비자까지 함께 연결해야 한다. Core의 ordinary `Deref`는 `F=()`에만 해당하며 funded handle의 읽기 권한을 대신하지 않는다.
- 이 결합 뒤 INT가 기존 pending persist/retry·canonical corruption·restart takeover·genuine Original fence 검사를 재개한다. 결과와 source는 기존 VALIDATION만 소유하며 다른 이름의 oracle/runner를 추가하지 않는다.

## Scope closure without duplicate implementation

G0에서 [공용 충돌 경계](../../../plans/oct-17-index-closeout/tickets/WRITE-OWNERSHIP.md#collision-resolution)의 기존 owner/path/hunk 인계를 확인한다. 인계 전 전체 파일 동시 편집을 허용하지 않는다. 신규 ABI/타입 불확실성에는 통합 큐의 좁은 compiler 검사를 사용할 수 있지만 같은 원인의 필터 변형·대형 daemon 실행은 반복하지 않는다.

코드 계약/actual caller 연결·영향 검증이 끝나면 그 하위 scope의 완료를 보존하고 남은 제품/성능/외부 수용만 추적한다. staged registry·provider/운영 target 부재는 해당 release scope의 BLOCKED이며 통과한 엔진/SDK 코드의 재개발 사유가 아니다. 기능 담당의 독립 gold/결과를 참조하고 cross-ticket 이름만 바꾼 동일 검사를 추가하지 않는다. W2-18이 launcher/action caller를 한 번 구현하며 이 티켓은 배포/활성화/rollback의 실제 target/observer 수용만 소유한다.

## Build-capacity recovery boundary

The integration writer owns only the existing `tools/ci/target_gc.py` and
`tools/ci/tests/test_target_gc.py` failure repair. Rust1.92 incremental crate
directories use fixed-width base36 u64 IDs; deps executables keep their
separate16-hex rule. Unknown/malformed IDs remain untouched. Retain newest
and sole entries, configured age limits and the existing Cargo-lock/trash
protocol. Recheck live process references while holding those locks; missing
census preserves the lane. Enumerate each profile once so dry-run cannot
double-count the same victim. GC cannot imply test or product acceptance,
and estimated action bytes are not a substitute for observed available disk.
No new cache manager, resource ledger, test selector or release requirement.


<a id="funded-sdk-handoff-before-scope-closure"></a>
## Funded SDK handoff before scope closure

Historical implementation instructions. The scoped client/RPC loan is now source-connected; current work/qualification remains in the SSOT.

## Existing funded SDK receiver handoff

- 원 writer `QIDX-W2-02-FINAL-QUERY-ASSEMBLY-20261017`의 terminal usage-error를 확인한 뒤 INT 부모가 저장된 co-cut을 이어 합류했다. 기존 connector/allocator/reference counter를 재사용한다. 현재 상태와 실행 결과는 README/VALIDATION만 소유한다.
- 실제 결합 대상은 S `quanta-runtime-retrieval-kernel/src/index_sdk_ingress/{facade,native_connect_v3,query,repomap,publish,control}.rs`의 `client_ref`와 RPC 소비자다. 기존 ordinary/default handle과 funding을 포함한 handle의 공개 범위를 구분한다.
- `NativeSharedValueV3<Payload,F>`는 funding과 Root를 보호하는 scoped unit access를 제공한다. generic `Deref` 복구·payload clone·grant 분리·Native branch의 임의 unsupported 처리로 compiler를 통과시키지 않는다. 같은 writer가 receiver와 실제 소비자를 함께 결합한다.
- Q의 `NativeSdkConnectDataV1::complete_shared_v1`은 모든 opaque `H`에 대해 완성된 header만 빌리는 기존 수신 포트다. 새 Q connector/accessor를 만들지 않는다. S의 `NativeFunded` facade는 `H: Deref`에 의존하는 `client_ref_v3` 대신 이 header와 원 Source의 scoped unit access를 실제 RPC 소비자까지 함께 연결해야 한다. Core의 ordinary `Deref`는 `F=()`에만 해당하며 funded handle의 읽기 권한을 대신하지 않는다.
- Client custody와 후속 query 비용 수용을 분리한다. 실제 Native lexical/structural query port는 retained funded client로 일반 SDK request/response 경로를 호출한다. IPC/frame 제한은 있지만 per-call Original 인증·응답 회계의 완료 증거는 없다. NIT-070의 기존 ingress/query owner와 결합해 해당 두 실제 포트의 수용을 확인한다. 새 Source issuer/port나 실행 knob를 임의 추가하지 않으며 client/header lifetime 통과로 이 항목을 닫지 않는다.
- 이 결합 뒤 INT가 기존 pending persist/retry·canonical corruption·restart takeover·genuine Original fence 검사를 재개한다. 결과와 source는 기존 VALIDATION만 소유하며 다른 이름의 oracle/runner를 추가하지 않는다.
