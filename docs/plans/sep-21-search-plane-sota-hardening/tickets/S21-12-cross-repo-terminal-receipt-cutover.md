# S21-12 — Cross-Repo Terminal Receipt and Breaking Cutover

Status: `ACTIVE — exact-pair and operational qualification remain staged`.
Completed resolver pre/postflight and V2 receipt decisions are in
[SEP-27-005](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)
and the [SEP-21 registry](../../../adr/SEP-21-DECISION-REGISTRY.md).
Resolver mapping needs typed receipt custody and actual fresh build/test results;
[the residual ledger](CURRENT-RESIDUAL-2026-09-26.md) owns remaining status.

Depends on: S21-02, S21-04, S21-07, S21-11

## Goal

Semantica producer의 prepared payload부터 quanta-index durable candidate/activation까지 exact content-bound
terminal receipt를 만들고, producer/SDK/daemon을 하나의 breaking cutover로 전환한다.

## Source-pair entry condition

The 2026-09-21 dirty-checkout snapshot is retained in Git history. Before
qualification, freeze both repositories' current HEAD, dirty digest,
resolved dependency roots, public receipt fields, and the daemon binary.
Do not infer pair compatibility from one repository's V2 API or an old build.

해제 조건:

1. quanta-index receipt schema를 coherent commit으로 freeze
2. public API/wire inventory와 receipt semantic validator를 같은 commit에서 고정
3. Semantica가 그 exact contract/SDK tree를 가리키는 coherent commit을 freeze
4. 두 HEAD/dirty digest/daemon binary를 결속한 cross-repo qualification receipt 발급

## Terminal receipt chain

```text
Producer source identity
 -> prepared payload digest
 -> canonical request/body digest
 -> operation journal terminal receipt
 -> sealed candidate commitment
 -> activation identity/epoch (when requested)
 -> daemon binary + contract/SDK source identity
```

각 arrow는 이전 identity를 포함한 domain-separated commitment다.

## Paired acceptance work

1. producer preparation manifest와 canonical payload digest schema 확정
2. SDK request context가 payload/operation identity를 운반
3. daemon publish/activation receipts를 exact candidate에 결속
4. aggregate closeout가 모든 component receipt를 검증
5. producer HEAD/dirty digest/resolved dependency roots 기록
6. daemon HEAD/dirty digest/binary SHA/features/toolchain 기록
7. old producer/new daemon, new producer/old daemon을 typed incompatible로 거부
8. cutover order, deployment freeze, rollback boundary 문서화
9. receipt retention과 replay-window semantics 연결

## Owner surfaces

- quanta-index contract/SDK/RepoMap ingest-control APIs
- Semantica RepoMap handoff and terminal receipt owners
- cross-repo hellgate tooling
- release receipt schema

## Negative scenarios

- same identity, different payload digest
- same generation, different candidate commitment
- SDK linked against different contract tree
- daemon binary from different HEAD/features
- dirty producer or daemon source
- receipt field omission/zero digest/order mismatch
- ACK loss and exact replay
- unsupported legacy producer

## Acceptance

- identity-only ACK로 strong closeout 성공 불가
- aggregate receipt에서 exact producer payload와 active candidate를 추적 가능
- path dependency/source mismatch가 blocking failure
- breaking cutover 뒤 old wire/receipt 자동수용 0
- replay가 original terminal receipt를 반환하고 storage/provider work를 반복하지 않음
- rollback 가능 범위가 receipt에 명시됨

## Verification

- frozen exact-head producer checkout and clean quanta-index checkout
- cross-repo positive/negative wire matrix
- actual built daemon binary hash
- publish-only and publish+activate flows
- restart/replay and aggregate closeout
- `just rust-verify-hellgate-cross-repo <producer_root>` 또는 후속 canonical profile

P11은 한 통과로 deploy/activate/rollback을 동시 암시하지 않고 독립 proof manifest 네 개를 발급한다.

| Proof ID | Meaning | Dependency |
|---|---|---|
| `p11-cross-repo-cutover` | exact source-pair protocol/wire/replay qualification | P03/P02B/P06/P10 |
| `p11-deployment` | attested release daemon deployment receipt | cross-repo qualification |
| `p11-activation` | deployed binary/config의 production activation receipt | deployment |
| `p11-rollback` | activated deployment의 rollback drill receipt | activation + P10 restore proof |

앞 proof의 pass는 뒤 proof를 암시하지 않는다. 승인이 없거나 실행하지 않은 deployment/activation/rollback은
각각 `NOT_RUN`으로 남고 P12 `production_ready=false`다.

## Stop conditions

- Semantica owner가 payload manifest/terminal receipt schema를 수용하지 않음
- exact producer checkout이나 buildable daemon binary가 없음
- dirty/path dependency state를 고정할 수 없음

이 경우 product code를 identity-only ACK로 우회하지 않고 external closure를 `blocked`로 둔다.

## No patch-on-patch rule

기존 identity-only ACK에 optional digest 몇 개를 덧붙여 strong receipt로 간주하지 않는다. producer
preparation부터 daemon terminal commit까지 하나의 mandatory commitment chain으로 breaking cutover한다.

## Final cross-repo action map

| Stage | quanta-index owner | Semantica owner | Mandatory binding |
|---|---|---|---|
| prepare | request/manifest contract | RepoMap bundle producer | producer HEAD+dirty digest, canonical payload digest |
| publish | SDK + ingest dispatcher + S21-04 journal | handoff client | operation key/body digest, terminal sequence, replay status |
| candidate | S21-02 catalog/RepoMap store | receipt consumer | candidate commitment, schema/profile, object digest |
| activate | control dispatcher/SDK | closeout aggregator | prior/new commitment, activation epoch, CAS result |
| qualify | cross-repo hellgate/receipt writer | registered producer profile | exact contract/SDK roots, daemon binary SHA/features/toolchain |

### Mandatory compatibility matrix

- old producer/new daemon and new producer/old daemon: typed incompatible, storage/provider mutation 0.
- same identity/different payload, same generation/different candidate, missing/zero/reordered commitment field: refuse.
- ACK loss exact replay: identical terminal receipt, no rebuild/re-embed/reactivation.
- query-only SDK profile and mutation-capable SDK profile를 각각 compile/run evidence로 분리한다.
- Semantica current snapshot은 `observed_at`, HEAD, dirty-path digest, dependency resolution을 receipt에 포함하고
  closeout 직전 재-freeze한다.
