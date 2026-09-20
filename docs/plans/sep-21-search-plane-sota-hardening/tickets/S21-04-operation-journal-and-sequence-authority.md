# S21-04 — Operation Journal and Durable Sequence Authority

Status: `planned`

Depends on: S21-00, S21-01

## Goal

모든 ingest/auxiliary mutation을 replay-first, terminally classified durable operation protocol로 통합한다.

## Root cause

- finalized replay lookup 전에 mutable preflight를 수행
- auxiliary semantic validation이 catalog claim 뒤에 있어 invalid intent가 남음
- in-progress에 owner/lease/fence/terminal refusal semantics가 부족
- durable sequence allocator가 corruption/restore 후 stored max와 reconcile되지 않음

## Target protocol

1. decode/auth/canonical body digest
2. non-mutating operation inspect
3. finalized same-body -> original receipt replay
4. same-key/different-body -> conflict
5. terminal-refused same-body -> recorded refusal replay
6. new request only: immutable epoch를 기준으로 complete semantic/resource prepare
7. `Reject`면 claim 없이 단일 transaction으로 terminal `Refused`; `Admit(PreparedMutationV1)`이면 fenced claim
8. prepared plan만 apply; mutable state를 다시 preflight하지 않음
9. atomic terminal commit이 proof/receipt/fence를 검증: committed/refused/aborted
10. recovery reconciler resolves expired owner and uncertain commit

## Required states

- `Claimed`, `Applying`, `Committed`, `Refused`, `Aborted`, `Uncertain`
- state마다 allowed transition, owner fence, lease, body digest, result/receipt commitment를 정의
- timeout/caller disconnect는 durable mutation failure와 동일하지 않음

## Sequence authority

- positive monotonic sequence with DB CHECK
- unique committed sequence within declared stream/session scope
- versioned/self-digested allocator metadata
- open/import: `next > MAX(all stored terminal sequences)` reconciliation
- regression/duplicate/corruption은 startup refusal 또는 explicit repair mode
- receipt pruning과 replay floor/expired semantics

## Work items

- ingest dispatcher의 preflight/replay/claim 순서 재구성
- auxiliary route의 semantic validators를 preflight owner로 이동
- catalog schema와 migration/import fixture 갱신
- operation status 조회 surface 추가
- stale claim recovery와 bounded retention policy
- serial-ingest invariant를 machine enforcement하거나 multi-worker fencing 구현

## Owner files

- `crates/quanta-index-core/src/domains/idempotency.rs`
- `crates/quanta-index-catalog/src/{idempotency,open,auxiliary}.rs`
- `crates/quanta-index-search-plane/src/ingest_dispatcher/`
- `crates/quanta-index-search-plane/src/auxiliary_authority.rs`
- contract/SDK mutation receipt surfaces

## Critical scenarios

- successful delta -> ACK loss -> base GC/config change -> exact replay
- invalid history/runtime/structural request -> restart -> retry
- same key/different body
- crash at claim/apply/storage/terminal commit boundaries
- restored DB with next below/at max
- stale worker commits after lease/fence replacement

## Acceptance

- finalized replay never depends on current base/provider/config availability
- invalid request leaves no indefinite non-terminal intent
- all terminal outcomes are deterministic on exact retry
- duplicate/non-positive/regressed public sequence cannot be issued
- every in-progress operation has one live owner or bounded recovery path

## Verification

- catalog owner-local transaction/fault tests
- real SQLite restart/restore fixtures
- daemon ingest idempotency/preflight/auxiliary catalog/crash matrix
- public API/wire/fuzz gates for status/receipt changes

## No patch-on-patch rule

단순히 `begin`을 preflight 앞으로 옮기지 않는다. replay inspect와 mutating claim을 분리하고 semantic
refusal/lease/sequence recovery까지 하나의 protocol로 구현한다.

## Final API and schema map

| Owner | Replace / add | Invariant |
|---|---|---|
| `crates/quanta-index-core/src/domains/idempotency.rs` | `begin/finalize`를 `inspect`, `claim_prepared`, `record_refused`, `commit`, `recover`로 교체 | replay 조회는 mutable preflight보다 먼저 |
| `crates/quanta-index-catalog/src/idempotency.rs` | boolean applied row를 typed state, owner, lease, fence, result/receipt digest, row digest로 교체 | stale owner terminal commit 불가 |
| `crates/quanta-index-catalog/src/open.rs` | sequence `CHECK (>0)`, stream-scope `UNIQUE`, `next > MAX(terminal)` reconciliation | restore 후 sequence 회귀 불가 |
| `crates/quanta-index-catalog/src/auxiliary.rs` | validator가 `PreparedMutationV1` 또는 typed rejection을 생성 | invalid intent/in-progress row 0 |
| `crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs` | body digest → inspect/replay/conflict → prepare → claim → apply → commit | ACK loss가 storage/provider work 반복 안 함 |
| operation status contract/SDK | terminal/non-terminal state와 authorization-aware lookup | string parsing 없이 recovery 관찰 |

### DoD additions

- same-key/same-body의 `Committed`와 policy상 persisted `Refused`는 원 terminal payload를 byte-stable하게 재생한다.
- same-key/different-body는 apply/preflight/provider 호출 없이 conflict다.
- `PreparedMutationV1`은 검증한 epoch/input commitment를 가지며 apply는 그것을 재검증하고 drift면 typed abort한다.
- serial-ingest 결정이면 admission cap과 runtime worker topology가 이를 machine-enforce한다.
- retention은 replay floor 아래 key에 `REPLAY_EXPIRED`를 반환하며 신규 작업으로 오인하지 않는다.
- SQLite `fullfsync`/durability 설정은 set 호출뿐 아니라 read-back 및 crash fixture로 증명한다.
