> HISTORICAL — pre-migration document from `docs/plans/oct-17-index-closeout/tickets/W2-19-integration-acceptance.md`. Current work: [Oct-22 SSOT](../../../../plans/oct-22-finalizing/README.md). Old TODO/status/counts do not create work or qualify current source.

# W2-19 — 검증 잔여·통합 A/B·출시

상태는 [SSOT](README.md#w2-19)만 갱신한다. 기존 구현의 미수용 검사만 모은 티켓이며 새 feature/producer/issuer/runner 개발 큐가 아니다. 새 정합 실패는 exact caller·원인에 한정해 수리한다.


## Contract hygiene join

현재 공개 API/module·wire inventory·format/parent/feature/lock을 actual producer/consumer와 대조한다. 과거 전체 strict 실패나 개별 baseline diff를 현재 미해결 작업으로 복제하지 않는다. 새 차이만 원 writer가 exact hunk로 수리하며 unchecked decode·원 오류·admission-before-birth를 lint 완화로 우회하지 않는다. 현재 실행 결과는 README/VALIDATION을 소비한다.

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
| legacy/consumer/release | A/D/C5 완료 유지, 미수용 legacy LQ5/semantic/test-platform·installed/pair/provider/actions·remote CI/release | 기존 [VALIDATION](../../oct-10-index-closeout/VALIDATION.md) oracle/consumer owner |

정식 R01–R40/P2I01–08/UX01–12/AUTO01–10은 기존 RFC/독립 gold에 결합한다. conditional AUTO/외부 consumer 범위를 명시하고 새 GUI 요구로 바꾸지 않는다. 겹치는 테스트/수치는 한 번 실행·기록하고 이 표는 그 결과를 참조한다.

## Release inputs and staged proofs

[proof-authority.toml](../../../../../tools/ci/proof-authority.toml)의 아래 `staged` proof 경계는 기존 release 조건이다. registry는 선택된 proof의 readiness/의존성, 이 티켓은 합격 조건을 소유한다. **현재 상태·담당·다음 완료 조건은 README만** 갱신한다. 현재 stage 선언만으로 모든 관련 테스트/제품 코드가 없다고 단정하지 않는다. 실제 target/feature/fresh-binary binding을 먼저 대조하고 부족한 exact target/observer만 원 담당이 수리한다.

| 기존 proof 경계 | 남은 수용/입력 | 담당·중복 방지 |
| --- | --- | --- |
| `p03-candidate-activation`, `p04-read-view-lifetime`, `p05-query-truth`, `p06-sdk-binding` | Linux fresh-release daemon의 activation/recovery·실 pin/GC·query quality/native response·actual SDK binding과 source/binary/raw inventory | 원 activation/lifetime/query/SDK oracle. owner-unit/trace identity만으로 process/release 판정 금지 |
| `p07-provider-boundary` | 승인된 실제 provider identity·egress·budget/cancel 입력과 release target | provider/consumer owner. 입력 부재는 BLOCKED이며 다른 Index 코드/기능 검사를 막지 않음 |
| `p08-runtime-supervisor`, `p09-control-readiness` | release-daemon signal/child-loss/FD residue·component-loss/readiness·bounded request diagnostics의 실제 process target/binding 및 Linux 실행 | 기존 Runtime/PHYSICAL process owner. owner tests/target은 존재하므로 전체 supervisor/control 재개발 금지 |
| `p10-state-migration` | exact-source current-format backup/restore/verify·old-root refusal·rollback qualification와 P09 의존성 | BOOT/PHYSICAL 기존 legacy owner. 새 Native backup/restore나 retired migration importer를 요구하지 않음 |
| `p11-cross-repo-cutover` | clean exact producer/consumer pair·resolver lock·attested daemon·terminal receipt와 전체 선택 proof inventory | SDK/INT/BOOT와 기존 cross-repo recipe. runner-candidate-only는 exact-pair proof가 아님; R3 omission/cold-restart는 W2-05 결과 소비 |
| `p11-deployment`, `p11-activation`, `p11-rollback` | 구체적인 승인 Linux target·실 action adapter·독립 pre/post observer 계약·같은 host/config/state/source 연속성 | 기존 운영 target/consumer owner. 미지정 target은 BLOCKED. generic runner·target issuer·실제 배포를 추측해 생성하지 않음 |

P12A infrastructure는 P11 운영 입력과 독립이다. 최종 aggregate의 `--require-all --bind-source`는 선택된 전체 prerequisites를 요구한다. `CODE_QUALIFIED`·`DEPLOYED`·`ACTIVATED`·`ROLLBACK_PROVEN`은 별도 판정이다. [CircleCI 계약](../../../../adr/SEP-28-001-circleci-provider-and-credit-boundary.md)의 exact-source hosted run·test-authority artifact를 B/release에서 확인하고 과거 green·local replay·벤치 컴파일을 새 hosted/성능 수용으로 바꾸지 않는다.

## Combined checkpoints

1. **정적 결합:** 현재 공개 API/module·parent/feature·actual caller/권한/취소/drop와 고정 gold를 대조한다. M/P/W/C/I/PUB의 독립 인계, 실제 source 적용, mount, 공용 caller 합류와 전체 source 정적 마감을 각각 구분한다. 적용 검사만 통과한 외부 patch나 `MODULE_HANDOFF`는 생산 경로 완료가 아니다. 오래된 handoff의 이미 구현된 TODO를 재배정하지 않는다.
2. **영향 검증:** Q는 Justfile/`./scripts/cargow`, S는 기존 owning QBC/actual genuine Original fixture closure로 필요한 strict/tests를 묶는다. lib(test) 필터는 전체 compile closure를 줄이지 않는다. 기존 실패 뒤 다른 필터/축소 feature로 반복하지 않는다.
3. **A / pre-switch:** 실제 producer→searchability·복구·fault/pressure·ANN/headless API·격리 rebuild/rollback과 고정 profile의 제품/비용/운영 조건을 candidate에서 수용한다.
4. **W2-18 switch:** A 뒤에만 실제 default route를 전환한다.
5. **B / post-switch:** default smoke/rollback·최신 coherent integration/full workspace·공개 API/module baseline·remote CI/release를 별도로 판정한다. release를 A의 선행 조건으로 만들지 않는다.

## Existing source/result boundaries

VALIDATION은 scope별 최신 결과를 갱신하는 요약이다. 이전 실패·수리·source/selector별 원문은 연결된 dated history에 보존하며, 요약에 실행 경과를 누적하지 않는다. current compiler·focused gold·product·운영·full suite·CI/release를 구분한다. 다른 source의 수치 합산, skipped/ignored/interrupted/Unknown/Pending의 성공 승격, 결과 원장 복제를 하지 않는다.

- 독립 구현 인계 비율은 고정된 bundle 분모와 제외된 공용 합류를 함께 기록한다. 이를 전체 코드·잔여 공수·제품 수용 비율로 전용하지 않는다. 실제 FULL→게시→query→동일 operation 복구는 별도 통합 oracle다.
- Reader owner의 Linux/macOS pass, C object/archive compilation, direct VFS fault test, installed signed worker와 genuine SDK/daemon recovery는 서로 다른 proof scope다. installed-worker `ignored`는 실행 성공이 아니며 이미지/digest/provenance 입력이 필요한 해당 process gate만 `BLOCKED`로 남긴다.
- C fault 하네스는 SQLite의 `NDEBUG` 전처리 영향 뒤에도 assertion이 실제 실패함을 control로 보여야 한다. assertion-disabled run은 제외하고, 수리한 하네스의 결과만 원 Source/물리 IO 수용에 사용한다.
- 집중 span/RepoMap guard 통과는 그 source/selector만 증명한다. 같은 파일의 read-host negative mutation 실패나 full-scope GOD/format 실패를 함께 `PASS`로 바꾸지 않는다. guard가 추출된 canonical child를 따라가도록 수리하되 실제 의미/음성 oracle를 유지한다.
- 최초 FULL의 pending 부재, terminal epoch의 실제 lender 보유권, NativeCarried successor 소비, writer 이후 typed failure의 whole-owner 수명과 reader Arc pre/post-birth 거절을 실제 호출 경로에서 묶어 확인한다. producer helper나 constructor 단독 결과를 caller→consumer 수용으로 승격하지 않는다.
- planner/receipt/pending의 move 수리는 원 실패 rollback·counter 시점·durable wire/CAS를 유지해야 한다. 고정 크기 집계와 original value 이동으로 clone이 줄어도 paid Native pin/Source의 async residency와 restart 재획득까지 검증되기 전에는 Native 증분 수용이 아니다.

## Integration packet

새 구현/상태 원장을 만들지 않고 아래 기존 packet의 실제 결과를 한 번 소비한다. 담당·현재 상태·다음 조건은 [SSOT](README.md#remaining-work), 파일/hunk는 [WRITE-OWNERSHIP](WRITE-OWNERSHIP.md#collision-resolution), source/실행 결과는 [VALIDATION](../../oct-10-index-closeout/VALIDATION.md)만 갱신한다.

| 소비할 기존 작업 | 이 티켓에 넘겨야 하는 결과 | 중복 방지 |
| --- | --- | --- |
| [W2-05 A](W2-05-producer-full-delta.md#native-chunk-plan)·[B](W2-05-producer-full-delta.md#native-chunk-dispatch) | 실제 여러 파일 plan→planner→세 topology Chunk 문서/dispatch와 고정 gold·거절/drop 회귀 | M/I/W 지원 source·query/recipe/IR 재개발 없이 실제 caller에서 수용 |
| [W2-05 C](W2-05-producer-full-delta.md#native-publication-caller) | same-writer reconcile→manifest→generation→SearchPlan→root/CAS의 실제 artifact·stale/foreign/uncertain/abort 결과 | P writer bank·I Parent/receipt·M plan·Root witness는 원 owner를 한 번 소비 |
| [W2-05 D](W2-05-producer-full-delta.md#wire-canonical-custody) | raw physical/header failure 보존과 최고 Source 전/후 lifetime·마지막 drop 환급 | 일반 Source·Args 완료나 mock pass로 Wire owner 수용 대체 금지 |
| [W2-05 E](W2-05-producer-full-delta.md#sdk-producer-custody)·[W2-02](W2-02-pending-recovery.md#terminal-publication-join) | PUB 실패 입력 보존·실제 SDK/ACK·동일 operation의 owner 회귀와5 process cut | Runtime/PUB 공용 parent는 단일 writer. 기존 opaque ACK·signed Store·IFX150 fixture 재사용 |
| [W2-16](W2-16-stub-authority.md#implementation-packet)·[W2-18](W2-18-native-cutover.md#implementation-packet) | 실제 committed Source→AST apply 및 genuine 선택형 FULL→BOOT/rollback | 원 Source/target을 한 번 생산·소비하고 fresh command Original/권한 경계 유지 |
| [W2-13](W2-13-ann-product-integration.md)·[W2-09](W2-09-cost-observability.md) | 기존 원4K 제품 결과 후 same-source Large/XL·mixed·Off/Record·운영 | SEARCH의 실행을 재제출하지 않고 PHYSICAL 원 oracle 소비 |

1. **코드 접합 확인:** 실제 producer/caller/consumer·모든 필요한 feature/import·Source finisher·typed 원인·partial/backing-last를 확인한다. 외부 candidate/지원 API/mount만으로 해당 행을 닫지 않는다. 코드의 나머지 완료 여부를 전역 campaign/worktree 청소 완료에 종속시키지 않는다.
2. **owner 체크포인트:** Runtime은 P의 기존 owning profile/현재 진행 결과를 먼저 소비하고, SDK/CP는 기존 PUB/PY 큐의 별도 분모로 검증한다. 성공한 owning test와 같은 compile을 중복 제출하지 않는다. 실패는 immutable 입력과 live source의 차이를 대조해 실제 owner만 수리한 뒤 같은 rail로 재검증한다. filter/feature 축소·무권한 Clone·Ordinary fallback으로 우회하지 않는다.
3. **제품 체크포인트:** 실제 FULL/delta/query·same operation/target/policy 복구, AST/BOOT 인계·기존4K/운영 oracle가 모두 필요한 범위를 통과해야 [A](#combined-checkpoints)를 판정한다. Linux zero-selected/ignored는 process 성공이 아니다. 설치 worker/외부 authority 누락은 해당 gate만 BLOCKED다.
4. **마감:** 기존 A→W2-18 default switch→B/full-suite·exact-source CI/release를 유지한다. 소스 구현·정적·컴파일·실행 본문·제품·운영·remote CI/release를 각각 기록하고 다른 source/selector의 수치를 합산하지 않는다. 유예/외부 입력 미수용은 명시하며7개 전체를 완료 처리하지 않는다.

반환물은 기존 command/profile·observed exit·실행/미실행 본문·원인·검사 범위이며 이 티켓 안에 별도 per-run 결과표를 만들지 않는다. 실제 입력/소스가 바뀐 경우에만 영향 근거에 맞춰 재검증한다.

## Existing funded SDK receiver handoff

과거 accessor 재작성/채팅 인계 지시는 [보존 이력](../../../oct-17-index-closeout/tickets/W2-19-integration-acceptance.md#funded-sdk-handoff-before-scope-closure)에 있다. 기존 scoped receiver/RPC를 다시 구현하지 않고 아래 계약을 검증한다.

- S `index_sdk_ingress/facade.rs::with_client_v3`는 NativeFunded에서 `with_funded_client_into_v3`를 소비하고, query/publish/control/RepoMap은 같은 facade를 통해 실제 SDK request를 호출한다. 현재 source는 일반 `H: Deref`로 funded payload를 꺼내는 새 accessor를 요구하지 않는다.
- 기존 `NativeSharedValueV3`·`NativeSdkConnectDataV1::complete_shared_v1`와 whole bank/Original funding을 유지한다. generic Deref 복구·payload clone·grant 분리·별도 Q connector/allocator/reference counter를 추가하지 않는다.
- Client/header custody와 후속 query의 per-call Original/응답 비용·제품 수용은 다른 경계다. NIT-070의 기존 ingress/query owner oracle를 소비하고 lifetime source 확인만으로 후속 명령 수용을 완료 처리하지 않는다.
- [W2-02 terminal](W2-02-pending-recovery.md#terminal-publication-join)은 이 실제 client/RPC를 재사용해 pending/V9/ACK·동일 operation 복구를 검증한다. 새로운 이름의 decoder/runner/oracle를 만들지 않는다.

## Scope closure without duplicate implementation

[공용 충돌 경계](WRITE-OWNERSHIP.md#collision-resolution)를 따른다. 코드 계약/실제 연결/영향 gold가 닫힌 범위를 외부 target·staged proof 때문에 재개발하지 않는다. 기존 launcher/action은 W2-18, 실제 운영 target/observer와 aggregate 수용은 W2-19가 소유한다. target GC/build-capacity의 과거 수리 packet은 이력이며 재작업 지시가 아니다.

## Historical section links

아래는 이전 deep link의 호환 anchor다. 원문은 [보존 이력](../../../oct-17-index-closeout/tickets/W2-19-integration-acceptance.md)에서만 확인하며 현재 할 일을 추가하지 않는다.

<a id="build-capacity-recovery-boundary"></a>
