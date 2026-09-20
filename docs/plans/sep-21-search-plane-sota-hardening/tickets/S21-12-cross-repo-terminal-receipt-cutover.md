# S21-12 — Cross-Repo Terminal Receipt and Breaking Cutover

Status: `planned`

Depends on: S21-02, S21-04, S21-07, S21-11

## Goal

Semantica producer의 prepared payload부터 quanta-index durable candidate/activation까지 exact content-bound
terminal receipt를 만들고, producer/SDK/daemon을 하나의 breaking cutover로 전환한다.

## Root cause

- RepoMap direct handoff ACK는 identity 중심이며 exact payload/manifest/content를 증명하지 못함
- aggregate strong terminal receipt는 현재 unsupported
- producer source, linked contract/SDK tree, daemon binary가 하나의 receipt에 결속되지 않음
- dirty/path dependency skew가 release compatibility와 분리됨

## Drift-prone observed blocker snapshot — observed 2026-09-21

- quanta-index clean HEAD `3ad279a08879de35fa96a5495a3382af28f095d0`의
  `BatchPublishReceipt`에는 `accepted_replace_scopes`와 `accepted_tombstone_scopes`만 있고
  semantic-specific receipt fields가 없다.
- 현재 quanta-index dirty patch는 `accepted_semantic_replace_scopes`와
  `accepted_semantic_tombstone_scopes`를 추가하지만 committed/frozen authority가 아니다.
- 현재 관찰한 Semantica HEAD는 `61ad7aab23f719f9a184dc4d01dcfa29196d4549`이고 44개 dirty path가
  있으며, runtime source는 semantic-specific fields를 소비한다.
- 따라서 현재 exact pair는 compile/QBC closure가 아니라 `BLOCKED`다. 과거에 전달된 Semantica
  SHA나 dirty field alignment를 current GREEN으로 승격하지 않는다.

이 snapshot은 현재-source 주장에 재사용하지 않는다. 구현/검증 시작 시 양 repo HEAD, dirty digest, resolved
dependency root를 다시 freeze한다.

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

## Work items

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
