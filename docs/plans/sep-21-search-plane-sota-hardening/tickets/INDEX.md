# SEP-21 Search Plane SOTA Hardening — Ticket Index

Status: current gate is `P00 OWNER_PROOF_GREEN`. The source-bound P00 handoff
(`artifacts/sep-21/handoffs/P00.json`, pinned by its immutable archive receipt) is the current authority and
permits P01R. S21-13 Phase B and every product/runtime proof remain open until their owning lanes produce
same-source receipts. Tracked docs do not embed the result SHA; the handoff owns it.

Authority inputs:

- `docs/analysis/quanta-index-purpose-validation-checklist.md`
- `docs/analysis/quanta-index-purpose-static-audit-2026-09-21.md`
- `docs/analysis/quanta-index-purpose-static-audit-2026-09-21-pass2.md`
- `docs/analysis/quanta-index-purpose-static-audit-2026-09-21-pass3.md`
- `docs/bugbash/sep-16/structural-remediation-plan.md`
- `docs/bugbash/sep-16/test-plan.md`
- [final plan audit](FINAL-AUDIT.md)
- [file-level execution action list](ACTION-LIST.md)
- [copy/paste lane prompt runbook](prompts/README.md)

이 packet은 2026-09-21 정적 감사에서 확인된 목적 적합성 결함을 구조적으로 제거하기 위한
실행 계획이다. 구현 완료나 테스트 통과를 주장하지 않는다.

## 1. 목표

최종 목표는 다음 product invariant를 한 architecture로 만족하는 것이다.

> Producer가 발행한 immutable search facts를 content-bound operation으로 prepare/publish하고,
> versioned activation authority가 exact candidate를 CAS로 전환하며, query는 실제 immutable
> handles를 한 read view에 pin한 뒤 completeness와 provenance를 사실대로 반환한다. Process와
> release authority는 crash, cancellation, restart, migration, provider, cross-repo 경계를
> executable receipt로 증명한다.

SOTA는 추상화 수나 새 crate 수로 판정하지 않는다. 아래 조건이 동일 final source에서 충족되어야
한다.

- identity collision과 same-generation mutation이 구조적으로 불가능
- publish, activation, query visibility, replay receipt가 서로 다른 typed authority로 명확히 분리
- 모든 durable record가 versioned/self-validating/content-bound
- crash point마다 old 또는 new committed state로 수렴하고 ambiguous success가 없음
- query continuation, result completeness, rank provenance가 재현 가능
- 모든 external/provider work와 process resource가 global bounded
- release claim이 exact source/binary/producer/provider/corpus/host receipt에 결속

## 2. RCA

### RC-A — identity가 타입이 아니라 문자열과 위치 규칙에 흩어짐

증상:

- RepoMap filename tuple collision
- graph node discriminant 소실
- cursor와 response가 full request/pin에 결속되지 않음
- activation ACK가 generation number만 반환

근본 원인:

- logical identity, physical object identity, content commitment, activation epoch를 분리된 타입으로
  소유하지 않는다.
- boundary마다 자체 문자열 join/dedup/variant check를 수행한다.

구조적 해법:

- `ArtifactIdentityV1`, `CandidateCommitmentV1`, `ActivationIdentityV1`,
  `CanonicalRequestIdentityV1`를 contract/core owner에 고정한다.
- filesystem key, cursor, receipt, SDK validator가 같은 canonical identity encoder를 사용한다.

### RC-B — durable state가 파일 존재와 memory map의 조합으로 판정됨

증상:

- same-generation overwrite
- digest-unbound activation
- stale activation resurrection
- durable sequence 회귀
- invalid auxiliary request의 orphan in-progress row

근본 원인:

- prepare/publish/activate/invalidate/recover가 하나의 명시적 state machine과 journal에 속하지 않는다.
- pointer record가 candidate commitment를 소유하지 않고 open-time reconciliation도 불완전하다.

구조적 해법:

- operation journal과 candidate/activation ledger를 단일 transaction protocol로 만든다.
- backend bytes는 immutable object이고 durable catalog만 visibility와 lifecycle을 결정한다.
- legacy layout은 dual-read가 아니라 offline importer로 한 번만 전환한다.

### RC-C — read view가 dependency 선언이지 실제 resource lease가 아님

증상:

- RepoMap route가 view 선언 뒤 ambient store를 다시 조회
- activation/retirement 사이 TOCTOU
- cursor가 원래 view/query에서 분리

구조적 해법:

- 모든 declared domain은 `QueryReadViewV2` 안에 immutable typed handle을 보유한다.
- route는 view 밖의 store/registry/ledger에 접근하지 못한다.
- continuation은 read identity와 canonical plan digest를 운반한다.

### RC-D — 결과 의미를 실행 상태가 아니라 관찰된 row 수에서 추론

증상:

- dense `Capped`를 exact/exhausted로 오표시
- raw rank와 dedup rank 불일치
- zero-hit engine을 미실행으로 기록
- empty corpus를 unavailable로 표시
- Unicode tokenless query가 global fallback

구조적 해법:

- lane은 rows와 함께 typed `ExecutionOutcome`, `Coverage`, `Exhaustiveness`, `ContributionTrace`를
  반환한다.
- window/provenance/explain은 이 outcome만 조합하고 row count로 상태를 추정하지 않는다.

### RC-E — runtime thread가 supervised process resource가 아님

증상:

- signal wiring, startup rollback, unexpected-exit propagation 부재
- unbounded join/drain
- panic 시 permit/live counter/peer-watch 누수
- provider cancellation 후 detached work 누적
- `SearchdRuntime` partial destructure로 maintenance/lifecycle/state-root lease가 serving 전에 drop

구조적 해법:

- 하나의 `SearchdSupervisor`가 plane/maintenance/provider task lifetime을 소유한다.
- RAII permit, global cancellation tree, cooperative deadline, hard drain deadline을 분리한다.
- child exit와 readiness를 supervisor state machine에서 파생한다.

### RC-F — proof가 advisory artifact이며 product authority와 분리됨

증상:

- artifact absence-pass
- weak receipt schema
- executable backup/restore/deploy authority 부재
- provider source/query egress policy 부재
- strong cross-repo terminal receipt 부재

구조적 해법:

- mandatory proof manifest와 strict schema를 blocking workflow에 연결한다.
- exact source/binary/external consumer/provider/corpus/host를 하나의 release ledger에 결속한다.

## 3. Target architecture

```text
Producer / SDK
    |
    v
Validated Boundary
  - canonical identity
  - shape/resource/auth checks
  - request digest
    |
    v
Operation Journal --------------------------+
  inspect replay -> prepare -> claim/refuse -> terminal result |
    |                                        |
    v                                        |
Immutable Candidate Builder                 |
  graph compiler / lexical / semantic / aux |
    |                                        |
    v                                        |
Candidate Ledger                            |
  sealed content commitment                 |
    |                                        |
    v                                        |
Activation Transaction                      |
  expected epoch CAS + exact commitment ----+
    |
    v
Snapshot Registry -> QueryReadViewV2 -> Query Executor -> Bound SDK Response

SearchdSupervisor owns transport, queues, maintenance, provider tasks,
readiness, cancellation, and bounded shutdown.
```

Authority rules:

1. Filesystem names are storage addresses, never product identity.
2. Backend file existence never activates a candidate.
3. SQLite candidate/activation ledger is the only visibility authority. Filesystem activation pointers are
   removed; memory maps never outrank the ledger.
4. Publish never implies activate.
5. Repair/import never restores an activation unless the imported ledger proves it.
6. Query code cannot perform ambient-latest reads after view acquisition.
7. Empty, unavailable, partial, capped, approximate, and exact are distinct typed states.
8. No compatibility shim, fallback decoder, dual-write, or long-lived dual authority is allowed.

## 4. Merge strategy — patch-on-patch 방지

각 ticket은 작은 PR 번호가 아니라 책임 단위다. 다음 merge unit은 내부적으로 stacked branch를 쓸 수
있지만, main에는 각 checkpoint가 self-consistent할 때만 들어간다.

| Merge unit | Tickets | Atomic closure |
|---|---|---|
| M0 Contract/proof freeze | S21-00, S21-13A | ADR, identity/state/error schema, proof manifest skeleton, migration and compatibility decision |
| M1 Durable authority | S21-01A, 03, 04, 01B+02 | pure contract first, compiler/journal fork-join, then one live layout/quarantine/activation cutover; legacy handled only offline |
| M2 Query truth | S21-05, 06, 07 | real handles, continuation/completeness, provenance, response binding이 한 contract로 연결 |
| M3 Runtime boundary | S21-08, 09, 10 | provider admission과 supervisor/auth/readiness가 global resource model로 연결 |
| M4 External cutover | S21-11, 12 | backup/restore와 producer terminal receipt가 exact binary/source와 연결 |
| M5 Qualification | S21-13 | mandatory proof graph가 final source에서 모두 green일 때만 closure |

금지:

- old/new activation pointer dual-write
- collision을 피하기 위한 separator 추가만의 filename patch
- stale activation 파일을 open 시 임의 삭제하고 protocol은 그대로 두는 patch
- route별 cursor hash/SDK validator 복붙
- dense `Capped`일 때 무조건 `has_more=true`로 덮는 patch
- signal handler만 추가하고 join/drain/supervision은 그대로 두는 patch
- timeout 이후 detached work를 metric만 추가해 허용하는 patch
- artifact schema만 강화하고 blocking workflow에는 연결하지 않는 patch

## 5. Ticket graph

| Ticket | Owner outcome | Depends on |
|---|---|---|
| [S21-00](S21-00-authority-freeze-and-cutover-contract.md) | authority/ADR/schema/cutover freeze | none |
| [S21-01](S21-01-canonical-identity-and-layout-v3.md) | P01A pure identity/codec/error primitives; P03 live layout/quarantine closure | 00; live phase also 03, 04 |
| [S21-02](S21-02-sealed-candidate-activation-and-recovery.md) | immutable RepoMap candidate, content-bound activation, recovery reconciliation | 00, 01A, 03, 04; closes with 01B in P03 |
| [S21-03](S21-03-repomap-graph-compiler-and-resource-envelope.md) | validated typed graph compiler and bounded materialization | 00, 01A |
| [S21-04](S21-04-operation-journal-and-sequence-authority.md) | replay-first durable operation state machine and sequence authority | 00, 01A |
| [S21-05](S21-05-read-view-v2-and-snapshot-lifetime.md) | actual immutable handle pinning for every declared domain | 00, 02, 04 |
| [S21-06](S21-06-query-completeness-continuation-and-provenance.md) | canonical cursor, completeness, ranking and explain truth | 00, 05 |
| [S21-07](S21-07-sdk-wire-response-binding.md) | shared request/response/receipt semantic validators | 00, 01, 02, 04, 06 |
| [S21-08](S21-08-semantic-admission-and-provider-boundary.md) | pre-I/O model/input gate, bounded provider work, egress policy | 00, 04, 06, 07 |
| [S21-09](S21-09-supervised-runtime-and-bounded-shutdown.md) | signal-aware supervisor, runtime-guard ownership, rollback, RAII, hard drain | 00, 04; provider enrollment closes atomically with 08 |
| [S21-10](S21-10-control-authorization-readiness-and-observability.md) | capability control plane and all-plane health truth | 00, 04, 09 |
| [S21-11](S21-11-state-migration-backup-and-restore.md) | offline migration and executable state lifecycle | 01, 02, 04, 09, 10 |
| [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md) | exact Semantica/SDK/daemon terminal receipt and breaking cutover | 02, 04, 07, 11 |
| [S21-13](S21-13-release-evidence-and-sota-qualification.md) | early proof infrastructure plus final source-bound closeout | phase A: 00; phase B: all |

## 6. Execution waves

| Wave | Tickets | Exit gate |
|---|---|---|
| W0 | 00, 13A | unresolved product decisions 0; exact owner/write set/frozen schemas; blocking proof skeleton |
| W1a | 01A | pure canonical identity/codec/error/security primitives; no live layout closure |
| W1b | 03, 04 | isolated lane commits plus same-HEAD P02I integration proof/handoff |
| W2 | 01B, 02 | compiler/journal을 소비하는 live layout/quarantine/publish/activate/recover one-time cutover |
| W3 | 05, 06, 07 | all query routes use one pinned view and bound response semantics |
| W4 | 08, 09, 10 | provider/process/control resources supervised and bounded |
| W5 | 11, 12 | frozen-state migration plus external producer cutover receipt |
| W6 | 13 | full proof graph on one clean exact source pair |

Parallelism:

- 전체 graph는 `P00 → P01A → (P02A ∥ P02B) → P02I → P03 → P04 → … → P11 → P12A → P12Q`이다.
- 오직 P02A graph compiler와 P02B operation/global-event journal만 병렬 가능하다. 서로 다른 worktree/branch에서
  같은 P01A base를 사용하며 P03은 P02I same-HEAD 통합 proof 이후 시작한다.
- W3는 P04→P05→P06 순차 stack이다. P06은 P05 public schema를 재설계하지 않는다.
- W4는 P07→P08→P09 순차 stack이다. provider executor enrollment는 P08에서 lifecycle closure한다.
- 같은 contract DTO/baseline 파일을 동시에 수정하는 병렬 작업과 그 밖의 병렬 lane은 금지한다.

Handoff rule: 각 순차 lane은 immediate predecessor handoff만 직접 검증한다. P02I는 P02A/P02B 두 handoff와 proof를
동일 HEAD에서 검증한다. 이 규칙은 transitive provenance를 버린다는 뜻이 아니다. P12A aggregate producer가 P00부터
P11까지 전 체인과 fork/join을 검증하도록 구현하고 P12Q가 final source pair에서 재검증한다. 매 lane이 모든 과거
artifact를 재검증하는 방식은 금지한다.

## 7. Finding coverage

| Audit finding | Ticket(s) | Required closing evidence |
|---|---|---|
| P0 filename tuple collision | 01, 11 | property + frozen collision migration fixture |
| P0 unauthenticated activation pointer | 02 | corruption/failpoint/restart proof |
| P0 stale activation resurrection | 02, 11 | corrupt -> restart -> publish -> restart negative scenario |
| P0 same-generation overwrite | 02 | same identity/same content replay; different content conflict |
| P0 digest-unbound activation | 02, 12 | content-bound CAS and external receipt |
| P0 finalized replay after mutable preflight | 04 | base GC/config change replay scenario |
| P0 runtime partial destructure drops state-root lease before serving | 09, 11 | serving interval two-process exclusion and guard-lifetime proof |
| P1 malformed graph silent transformation | 03 | typed uniqueness/referential negative corpus |
| P1 materialization amplification | 03 | deterministic cardinality/byte/CPU/RSS envelope |
| P1 auxiliary orphan intent | 04 | terminal-refused/restart/retry convergence |
| P1 SDK response/receipt unbound | 07 | wrong-but-same-variant negative matrix |
| P1 durable sequence regression | 04, 11 | restored DB reconciliation and duplicate refusal |
| P1 RepoMap read-view TOCTOU | 05 | activation/retire/query barrier scenario |
| P1 dense capped marked exact | 06 | filtered refill partial/exhaustiveness matrix |
| P1 hybrid-seed rank mismatch | 06 | independent RRF oracle |
| P1 Unicode RepoMap global fallback | 03, 06 | multilingual/tokenless golden corpus |
| P1 model/input gate after I/O | 08 | zero provider-call refusal proof |
| P1 engines/contribution/availability confusion | 06, 10 | zero-hit execution and readiness provenance |
| P1 cursor full-context binding | 06, 07 | cross-query/repo/route/cap tamper matrix |
| P1 nested cap/typed window identity | 06 | raw-wire invalid/mismatch matrix |
| P1 explain reconciliation/zero digest | 06 | typed fail-closed reconciliation oracle |
| P1 missing signal/supervision/rollback | 09 | real child process signal and spawn-failure proof |
| P1 unbounded drain | 09 | cooperative and hard-deadline proof |
| P1 panic accounting leak | 09 | injected panic RAII reconciliation |
| P1 detached provider residual work | 08, 09 | global task/FD/request/cost cap proof |
| P1 operation authorization gap | 10 | principal/capability negative matrix |
| P1 partial-alive readiness/diagnostics | 10 | kill-one-plane and request-correlation proof |
| P1 artifact absence-pass | 13 | missing/stale/wrong-source artifacts fail blocking job |
| P1 provider egress authority gap | 08, 13 | profile policy and redacted audit receipt |
| P1 backup/restore/deploy authority gap | 11, 13 | executable frozen-state restore and rollback receipt |
| P1 strong cross-repo receipt unsupported | 12 | exact producer payload/daemon ACK/aggregate closeout |
| P1 finished child panic/close result discarded | 09, 10 | supervisor terminal event and global-failure propagation |
| P1 panic leaks live/dispatch counters and peer-watch | 09 | RAII reconciliation and no detached watcher proof |
| P1 persisted receipt wire/replay compatibility | 00, 04, 11, 12 | frozen receipt fixture and offline import/replay |
| P1 general quarantine provenance/recovery | 01, 10, 11 | append-only incident identity and repair audit |
| P1 SQLite durability setting claim/read-back mismatch | 04, 11, 13 | effective pragma read-back or downgraded documented guarantee |
| P1 boot-time live legacy migration remains after cutover | 11 | production boot legacy readers/migrators count 0 |
| P1 operation-status authorization | 04, 10 | observe/admin capability negative matrix |
| P1 active-selector response lacks resolution proof | 06, 07 | activation epoch/read-identity response binding |
| P1 declared vs provider-observed model/version/usage/cost | 08, 13 | provider outcome and real-provider receipt |
| P1 profile-dependent empty/tokenless semantic behavior | 08 | zero-I/O common refusal matrix |
| P2 weak verification receipt writer | 13 | malformed/non-terminal/short-SHA receipt rejection |
| P2 composite local scope widens strict thread cap | 13 | strictest-cap composition test and receipt |
| P2 existing state-root lock owner/mode unchecked | 09, 11 | foreign/permissive lock startup refusal |
| P2 state-root lock hardlink count unchecked | 09, 11 | `nlink == 1` enforcement |
| P2 Symbol result identity contract ambiguous | 00, 07 | stable identity/lifetime contract and binding tests |
| P2 query-only SDK requires control/ingest socket configuration | 07, 10 | least-privilege endpoint profiles |
| P2 CLI help/version and capability drift | 10, 13 | generated help/capability snapshot from current contract |
| P2 stale SSOT/documentation claims | 00, 13 | canonical-source update and generated-doc lint |
| P2 hard-drain escalation semantics unresolved | 00, 09 | forced-termination policy and exit-code proof |
| P2 process readiness vs repo readiness scope | 00, 10 | separate schemas and empty-daemon decision |
| P2 label-free metrics vs request correlation sink | 10 | closed counters plus bounded diagnostic sink |
| P2 incomplete quality artifact family inventory | 13 | frozen mandatory family registry |
| Decision RepoMap focus semantics | 00, 03 | strict-scope or explicit-hint behavior proof |
| Checklist G1-03 contradicts canonical IPC exception | 00, 13 | corrected architecture oracle and lint alignment |
| Checklist proposed rows were never applied | 13 | checklist source updated and row-to-proof registration |

Coverage rule: ticket의 owner-local test만으로 finding을 close하지 않는다. 표의 negative/recovery/
consumer proof와 최종 S21-13 receipt가 모두 있어야 한다.

## 8. Global Definition of Done

- 모든 ticket이 `done`이고 dependency checkpoint가 동일 final HEAD를 가리킴
- legacy live path, dual authority, compatibility shim, TODO gate가 없음
- SQLite ledger 외 RepoMap activation visibility authority가 없음
- state-root lease가 runtime construction부터 모든 child 종료까지 연속 보유됨
- required child/connection/provider/peer-watch class에 detached `JoinHandle`이 없음
- hard deadline 초과 시 live worker를 남긴 채 정상 return하거나 lease를 release하지 않음
- public contract/SDK/wire change가 intentional baseline과 fuzz evidence를 가짐
- new/changed tests가 `tools/ci/test-authority.toml` 및 실제 blocking profile에 등록됨
- owner-local positive/negative/recovery와 D/P/F/Q/X 증거가 역할별로 분리됨
- clean quanta-index HEAD, clean producer HEAD, daemon binary SHA-256가 하나의 receipt에 결속
- process/deploy proof가 동일한 attested daemon binary를 재빌드 없이 승격
- artifact absence, stale SHA, dirty mismatch, provider mismatch가 모두 blocking failure
- 목적 체크리스트의 모든 mandatory P0/P1이 `PASS`; `NOT_RUN`/`BLOCKED`가 남으면
  `PRODUCTION_READY` 금지
