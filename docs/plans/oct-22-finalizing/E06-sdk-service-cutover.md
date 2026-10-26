# E06 — 공개 SDK·서비스·BOOT/default 전환

상태는 [SSOT E06](README.md#e06). 원 요구 R31–R36, 종전 W2-08/18을 소유한다. PUB SDK parent·Root 실제 FULL evidence·기존 runtime/BOOT/launcher의 독점 경계를 유지한다.

## Reuse and sequence

funded scoped SDK RPC·opaque ACK/NativeInitialFullPublishedV3·Source cut digest, worker/pump·queue/TTL/cancel·launcher selection/predecessor/CAS/rollback을 재사용한다. target issuer·decoder·launcher·공개 transport를 새로 만들지 않는다.

선택형 genuine FULL의 operation/view/source/digest를 실제 Q receipt와 대조해 candidate Verify/Serve/search/reopen에 인계한다. stage receipt·reservation·ACK만으로 published result를 합성하지 않는다. predecessor는 전환 전에 실제 저장 상태로 보존한다.

## Native control request boundary

2026-10-24 정적 감사에서 공개 `Status`와 `ReadView`가 원 요청 budget을 Catalog mutex 획득까지 전달하지 않는 경로를 확인했다. `ReadView::finish`의 최종 deadline 검사만으로 그 앞의 대기·pin 발급을 막을 수 없어 canonical Core→Catalog→Kernel→Runtime 계약을 함께 수정했다. 실행 결과는 [Catalog/control 통합 검증](VALIDATION.md#e04-e06-catalog-control-closure-20261024)에 기록한다.

`inspect(key, OperationInspectAccess)`는 일반 조회의 `Request(&WorkBudget)`와 오류 후 필수 `PostFailureReconcile`을 명시적으로 구분한다. 일반 조회는 기존 `lock_native_budget`과 읽기 후 checkpoint를 사용한다. 오류 후 durable terminal/Unknown 판정은 원 요청 만료 때문에 생략하지 않는다. `pin_view`→`acquire_ticket`는 같은 budget을 전달하고 COMMIT 직전에 검사한다. 거절 시 rollback하며, COMMIT 성공 뒤에는 ticket을 반환해 기존 owner가 보존·해제한다. ReadSession Open은 취소된 응답에서도 ticket을 먼저 소유하고 같은 키의 재조회에 동일 handle을 반환한다. 오래 보관되는 일반 ticket은 최초 요청 trace context를 붙잡지 않는다. 기존 materialization의 짧은 관측 scope와 mandatory release는 유지한다. 기존 E04 `WorkStatus`·수동 `ClaimWork`/`RenewWork`·`ReadyWork` 수리는 재사용하며 재개발하지 않는다.

공개 one-shot UDS에는 별도의 고정 idle 지연도 있다. 제품 `ACCEPT_IDLE=50ms`를 받은 `UdsServer::run`이 빈 accept queue에서 무조건 sleep하므로, 새 연결은 남은 sleep만큼 지연될 수 있다. 기존 `rustix::poll`의 listener `IN` readiness로 대체했다. source8b63fa2f…에서 IPC strict·서버/admission 영향76/76은 VERIFIED이며 서비스14·daemon3도17/17 VERIFIED다. [원인·범위별 실행](VALIDATION.md#e04-control-budget-closure-20261024)을 따른다. timeout은 기존 shutdown 검사 cadence로 유지하고 admission·peer 인증·connection cap·thread reap/RAII 및 FD 오류 종료를 보존한다. 현재128 trace의 RPC 외부 gap 전부나 원4K 실패를 이 원인으로 단정하지 않는다.

## DoD

- [ ] 실제 source/literal/path·semantic/provider query UDS/SDK의 요청/응답·selection·coverage/freshness·terminal reconcile.
- [ ] live queue/active/lag·TTL·cancellation·owned work pump·Core/Enrichment/Maintenance fairness와 provider 경계.
- [ ] genuine S FULL→Q BOOT verify/serve/reopen, process stop/join/lease와 기존 old selection의 격리 rollback/restart.
- [ ] 명시 지원하는 current-format backup/restore/verify와 Native new-root rebuild/reselection·old-format 거절·competing legacy head/lifetime 경로 retirement. 미지원 importer/Native backup 기능을 추측해 추가하지 않음.
- [ ] E08 통합 A 뒤 default daemon/CLI/launcher 전환, B에서 default smoke·rollback·bounded child/FD/lease/scratch cleanup.
- [ ] installed exact producer/consumer pair·provider identity/egress/budget/cancel과 실제 운영 target/action/독립 observer를 E08에 인계.

제품 선택의 default 전환과 실제 배포 권한은 다르다. 승인 Linux target/설치 bundle/worker provenance가 없으면 해당 실제 process/운영 gate는 BLOCKED다. fixture/local boot pass를 installed qualification으로 승격하지 않는다. [보존 BOOT 계약](../../history/oct-22-finalizing/oct-17-index-closeout/tickets/W2-18-native-cutover.md)과 [state runbook](../../operator/state-cutover-runbook.md)을 따른다.
