# S21-00 — Authority Freeze and Cutover Contract

Status: `done` — decisions frozen in `docs/adr/SEP-21-001..004` and `SEP-21-DECISION-REGISTRY.md`

Depends on: none

## Goal

구현 전에 identity, durable state, activation, query outcome, compatibility, migration, external receipt의
단일 계약을 고정한다. 이후 ticket이 서로 다른 의미를 구현하는 것을 막는다.

## RCA scope

- 같은 `generation`이 logical identity, content identity, serve head 의미를 동시에 부담
- 현재 source와 sep-16 계획, 1/2/3차 감사의 용어가 완전히 정규화되지 않음
- focus hint, exporter/deploy ownership, legacy state 지원 범위가 owner decision 없이 열려 있음

## Required decisions

1. `RepoId`/`RevisionId`의 허용 문자열과 Unicode normalization 정책
2. logical generation과 immutable artifact-set/content commitment의 관계
3. candidate publish와 activation의 exact transaction boundary
4. activation rollback/CAS/epoch semantics
5. legacy persisted state의 지원 방식: offline import only
6. cursor security model: signed opaque stateless token
7. `focus_subjects`: non-empty strict scope, unresolved refusal, no global fallback
8. empty/partial/capped/approximate/unavailable의 public response semantics
9. provider egress profile과 source/query data classification owner
10. supported deployment/backup/restore/rollback boundary
11. Semantica producer와 SDK의 breaking cutover order

## Required contract artifacts

- ADR: authority and lifecycle state machine
- ADR: canonical identity and digest domains
- ADR: read-view and continuation semantics
- ADR: process supervision and shutdown semantics
- wire/persisted consumer inventory update proposal
- legacy state inventory with importer/rebuild/discard class
- exact error code table; string parsing 금지
- finding-to-ticket-to-proof ledger

Canonical accepted artifacts:

- `docs/adr/SEP-21-001-canonical-identity-and-digest-domains.md`
- `docs/adr/SEP-21-002-durable-authority-and-operation-lifecycle.md`
- `docs/adr/SEP-21-003-read-view-continuation-and-provider-policy.md`
- `docs/adr/SEP-21-004-process-supervision-state-cutover-and-proof.md`
- `docs/adr/SEP-21-DECISION-REGISTRY.md`
- `tools/ci/proof-authority.toml`
- `tools/ci/proof-manifest.schema.json`
- `tools/ci/lint/check-proof-authority.py`

## Owner files

- `crates/quanta-index-contract-base/src/`
- `crates/quanta-index-contract/src/`
- `crates/quanta-index-core/src/domains/`
- `tools/ci/inventory/wire-surface.toml`
- this plan packet

## Structural constraints

- contract DTO는 vendor/storage path를 노출하지 않는다.
- public identity type마다 canonical encoder와 fallible digest가 하나만 존재한다.
- old/new live decoder 동시 지원은 금지한다. migration input decoder는 product IPC surface 밖에 둔다.
- `G1-03`은 canonical IPC codec exception을 반영해 수정하되 vendor/process ownership 금지는 유지한다.

## Acceptance

- 열린 product decision이 owner와 deadline 없이 남지 않음
- 모든 subsequent ticket이 사용하는 type/state/error 이름이 한 표에 존재
- wire and persisted format bump가 inventory와 migration class를 가짐
- merge unit별 breaking cutover/rollback 가능 범위가 명시됨
- compatibility shim 또는 heuristic fallback 제안이 없음

## Proof

- static architecture review
- `just rust-public-api`와 `just rust-wire-inventory`는 구현 ticket에서 실행; 이 ticket 문서 작성만으로
  통과를 주장하지 않음

## Stop conditions

- producer owner가 terminal receipt payload를 확정하지 않음
- legacy state 보존/폐기 결정이 없음
- cursor security model이 결정되지 않음
- focus semantics가 결정되지 않음

이 경우 후속 구현을 시작하지 않고 `blocked`로 둔다.

## No patch-on-patch rule

결정되지 않은 contract를 임시 optional field, zero digest sentinel, dual decoder로 먼저 구현하지 않는다.
ADR와 breaking cutover contract가 frozen된 뒤 owner implementation을 시작한다.

## Final audit amendments — mandatory decisions

후속 ticket 착수 전에 아래 항목까지 ADR의 명시적 값으로 고정한다.

| Decision | Required output | Blocking consumer |
|---|---|---|
| persisted receipt evolution | `BatchPublishReceipt` canonical-CBOR version bump, old/new refusal matrix, inventory entry | S21-04, S21-11, S21-12 |
| RepoMap authority | SQLite candidate/activation ledger가 유일한 visibility authority; object store는 immutable bytes만 소유 | S21-02 |
| terminal refusal | `Refused`를 저장·재생할 범위와 retention/replay-floor | S21-04 |
| ingest concurrency | one state-root-global `MutationCoordinatorV1` and fenced prepared mutations | S21-04 |
| state-root format | root manifest/version, old binary/new root 및 new binary/old root의 typed refusal | S21-11 |
| raw identifier policy | already-NFC UTF-8, case-sensitive, 1..=512 bytes, controls rejected, no silent normalization | S21-01 |
| physical layout/security | `objects/sha256/aa/bb/<60hex>.cbor`; effective-UID, exact mode, no-follow, inode and nlink checks | S21-01, S21-09, S21-11 |
| terminal sequence scope | one positive state-root-global transactional `catalog_sequence_v2` stream | S21-02, S21-04, S21-11 |
| shutdown escalation | cooperative deadline 뒤 process abort/non-zero exit 여부; kill 불가능한 Rust thread를 graceful로 표기 금지 | S21-09 |
| provider policy | tenant/source/query classification별 egress consent, region, retention, budget owner | S21-08 |
| active selector binding | resolution epoch/read identity로 검증; 원 요청 selector와 resolved pin의 단순 equality 금지 | S21-07 |

### File-level action list

- `docs/adr/`: 위 결정을 네 ADR의 state table, wire table, refusal table에 기록한다.
- `tools/ci/inventory/wire-surface.toml`: 바뀌는 public/persisted DTO마다 producer, consumer, version,
  decoder, migration class를 등록한다.
- `tools/ci/test-authority.toml`: 각 결정의 negative scenario와 proof family를 구현 전 등록한다.
- 이 plan packet: 결정 ID를 S21-01~13의 dependency와 acceptance에 역참조한다.

### DoD additions

- 표의 모든 decision에 owner, frozen value, decision artifact, deadline이 있다.
- `TBD`, optional compatibility field, zero-digest sentinel, dual live decoder가 없다.
- persisted receipt와 state-root format의 이전/이후 compatibility matrix가 executable fixture 이름까지 가진다.
