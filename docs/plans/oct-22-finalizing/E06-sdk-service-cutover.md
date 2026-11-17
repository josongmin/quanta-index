# E06 — 공개 SDK·서비스·BOOT/default 전환

상태는 [SSOT E06](README.md#e06). 원 요구 R31–R36, 종전 W2-08/18을 소유한다. PUB SDK parent·Root 실제 FULL evidence·기존 runtime/BOOT/launcher의 독점 경계를 유지한다.

## Reuse and sequence

funded scoped SDK RPC·opaque ACK/NativeInitialFullPublishedV3·Source cut digest, worker/pump·queue/TTL/cancel·launcher selection/predecessor/CAS/rollback을 재사용한다. target issuer·decoder·launcher·공개 transport를 새로 만들지 않는다.

선택형 genuine FULL의 operation/view/source/digest를 실제 Q receipt와 대조해 candidate Verify/Serve/search/reopen에 인계한다. stage receipt·reservation·ACK만으로 published result를 합성하지 않는다. predecessor는 전환 전에 실제 저장 상태로 보존한다.

## Native control request boundary

완료된 Status/ReadView budget 전달·실패 후 reconciliation·ticket/Open 보존은
[Native control 계약](../../adr/NOV-12-001-native-query-and-semantic-engine.md#request-budgeted-control-and-ticket-custody),
UDS accept 대기는 [IPC 계약](../../adr/NOV-07-001-native-index-execution-and-custody.md#decision)을 재사용한다.
당시 source별 영향176/176·daemon3/3 및 UDS 영향76/76·서비스/daemon17/17은
[control 검증](VALIDATION.md#e04-e06-catalog-control-closure-20261024)과
[UDS 검증](VALIDATION.md#e04-control-budget-closure-20261024)에 귀속하며 현재 제품 수용으로 합산하지 않는다.

## Implemented default routing

일반 `QuantaIndex::connect`/query-only와 CLI 명령은 Native로 연결한다.
명시 Legacy 진입점은 별도 경로다. 완료된 연결부를 새 구현 작업으로 발급하지 않는다.
[SDK 선택](../../../crates/quanta-index-sdk/src/config.rs)·[SDK 연결](../../../crates/quanta-index-sdk/src/client.rs)·[CLI 분기](../../../crates/quanta-index-searchctl/src/lib.rs)가 소스 권위다.
실제 Source/조회 호출자·current selected view·설치 FULL/BOOT·rollback 수용은
[Native 활성 원장](../nov-12-native-cutover/README.md#현재-종료-판정)이 소유한다.
아래 E08 A→전환→B는 설치 selection과 제품 수용 순서이며 일반 라우팅 구현의 재작업 지시가 아니다.

## DoD

- [ ] 실제 source/literal/path·semantic/provider query UDS/SDK의 요청/응답·selection·coverage/freshness·terminal reconcile.
- [ ] live queue/active/lag·TTL·cancellation·owned work pump·Core/Enrichment/Maintenance fairness와 provider 경계.
- [ ] genuine S FULL→Q BOOT verify/serve/reopen, process stop/join/lease와 기존 old selection의 격리 rollback/restart.
- [ ] 명시 지원하는 current-format backup/restore/verify와 Native new-root rebuild/reselection·old-format 거절·competing legacy head/lifetime 경로 retirement. 미지원 importer/Native backup 기능을 추측해 추가하지 않음.
- [ ] E08 통합 A 뒤 default daemon/CLI/launcher 전환, B에서 default smoke·rollback·bounded child/FD/lease/scratch cleanup.
- [ ] installed exact producer/consumer pair·provider identity/egress/budget/cancel과 실제 운영 target/action/독립 observer를 E08에 인계.

제품 선택의 default 전환과 실제 배포 권한은 다르다. 승인 Linux target/설치 bundle/worker provenance가 없으면 해당 실제 process/운영 gate는 BLOCKED다. fixture/local boot pass를 installed qualification으로 승격하지 않는다. [보존 BOOT 계약](../../history/oct-22-finalizing/oct-17-index-closeout/tickets/W2-18-native-cutover.md)과 [state runbook](../../operator/state-cutover-runbook.md)을 따른다.
