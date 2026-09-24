# S21-12 — Cross-Repo Terminal Receipt and Breaking Cutover

Status: exact-pair release qualification staged.

2026-09-24 source update: the local RepoMap mutation surface now has only
source-digest-bound V2 requests/receipts across SDK, IPC, core ports, and
store. The V2 activation identity is flat; V1 opcodes/direct-store methods
and weak persisted projection metadata are refused or removed. The
2026-09-21 static delta below is historical, not a current API description.
The flat V2 request now carries `expected_active`; the catalog transaction
checks that prior-head token before sequence allocation and checks the
persisted prior expectation on replay. See
`crates/quanta-index-contract/src/repomap/terminal_receipt_v2.rs` and
`crates/quanta-index-catalog/src/candidate.rs`. The Semantica durable intent,
exact source pair, and terminal receipt chain still require fresh paired
verification. Quanta-local CAS code is not that proof.
Cross-repo producer qualification, deployment, and activation remain separate
unverified gates; this note does not close S21-12.

Depends on: S21-02, S21-04, S21-07, S21-11

## Goal

Semantica producer의 prepared payload부터 quanta-index durable candidate/activation까지 exact content-bound
terminal receipt를 만들고, producer/SDK/daemon을 하나의 breaking cutover로 전환한다.

## Root cause

- RepoMap direct handoff ACK는 identity 중심이며 exact payload/manifest/content를 증명하지 못함
- aggregate strong terminal receipt는 현재 unsupported
- producer source, linked contract/SDK tree, daemon binary가 하나의 receipt에 결속되지 않음
- dirty/path dependency skew가 release compatibility와 분리됨

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

## 2026-09-21 static implementation delta

- Observed quanta-index HEAD: `4ee230efb927884663c671e623b25cf8892142da`. Runtime, Cargo, daemon, tests, and QBC are `NOT_RUN` by explicit user instruction.
- V1 `RepoMapMutationAck` and its wire decoder remain unchanged. The parallel V2 surface adds `PublishRepoMapBundleV2`, `RepoMapActivateV2`, and phase-tagged `RepoMapTerminalReceiptV2`.
- The V2 publish request carries the exact full source-bundle digest. The daemon recomputes it before mutation. Candidate `projection_meta` durably retains manifest digest plus source-bundle digest, while legacy rows remain readable and are rejected for V2 activation.
- V2 activation compares repo/revision/generation, manifest digest, snapshot id, projection version, authority digest, and source-bundle digest with immutable candidate metadata before activation.
- Publish and activate responses bind those axes to candidate commitment, activation epoch, terminal sequence, and replay status. The SDK exposes separate `publish_v2` and `activate_v2` methods; V1 methods remain compatibility-only.
- Static hostile review found and closed a same-commitment replay substitution: compiled candidate bytes can remain identical while manifest/snapshot/projection/authority custody changes. Catalog replay now requires exact durable object address, content digest, byte size, and projection metadata; the RepoMap owner also re-reads and compares every retained axis before issuing the V2 publish terminal receipt.
- V1 and V2 persistence are separated at the RepoMap owner. V1 keeps the exact legacy projection-metadata JSON shape, including omission of the two V2 custody keys, so upgrade-time V1 replay remains byte-compatible. V2 seals both strong fields together. A V2 call cannot relabel an existing V1 row; it receives a typed candidate conflict and leaves the durable row unchanged.
- V2 publish receipts now use phase-correct stable activation fields: publish does not mutate the active head, so `prior_candidate_commitment=None` and `activation_epoch=0`. Later activation changes therefore cannot rewrite an ACK-loss publish replay. Publish and activate restart replays must equal their original receipts on every field except `mutation.replayed`, which changes to `true`.
- Activation replay now forwards the catalog-reconstructed original `prior_candidate_commitment`; the store no longer discards it in the replay branch. This preserves full receipt identity for superseding activation after restart.
- Projection metadata decoding rejects one-sided keys, present-but-non-string values, invalid manifest digests, and non-canonical source-bundle digests as `CatalogRowCorrupt`; malformed V2 custody cannot fall through as a legacy V1 row or authorize V2 activation.
- Added source-only contract and owner selectors for per-axis source digest sensitivity, strict duplicate/unknown-field refusal, same-commitment axis substitution, durable-row preservation, restart replay, activation replay prior-commitment stability, and terminal-sequence stability. These selectors are not executed.
- Cross-version wire refusal, response-loss restart replay, daemon binary attestation, cross-repo qualification, deployment, activation, and rollback evidence remain unexecuted.
