# Copy/paste prompt — P02B Global Sequence and Operation Journal

당신은 S21-04 lane owner다. repo root 기준 prompts의 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 먼저 읽고
적용한다. P01A checkpoint commit과 source-bound handoff의 exact result SHA를 base로 별도 worktree/branch에서
작업한다. P02A와 병렬 실행하되 shared contract/baseline/inventory/generated docs는 수정하지 않고 P02I에 delta만
넘긴다. P01A error enum과 P02A compiler DTO를 임의 변경하지 않는다.
P01A result SHA/handoff/proof binding이 없거나 owner scope가 P02A/shared writer와 겹치면 구현하지 말고
`BLOCKED`로 종료한다.

repo instructions, FINAL-AUDIT, S21-04, amended SEP-21-002/decision registry, P00/P01A handoff를 읽는다. 시작 시
HEAD/dirty/owner scope와 P01A accepted-code table digest를 freeze한다.

## 목표

모든 durable event가 공유하는 state-root-global sequence authority를 먼저 만들고, ingest/auxiliary mutation을
replay-first, immutable-prepare, fenced-claim, terminally-classified protocol로 교체한다.

target flow:

`digest/auth → inspect → replay/conflict → immutable prepare → record_refused | claim_prepared → apply → fenced terminal commit → recover`

## owner scope

- 신규 `crates/quanta-index-catalog/src/sequence.rs`
- `crates/quanta-index-core/src/domains/idempotency.rs`
- `crates/quanta-index-catalog/src/{idempotency,open,auxiliary}.rs`
- `crates/quanta-index-search-plane/src/ingest_dispatcher/`
- `crates/quanta-index-search-plane/src/auxiliary_authority.rs`
- `crates/quanta-index-contract/src/ipc/ingest.rs`의 operation journal/status/terminal receipt section만
- SDK facade/re-export/public baseline/wire inventory는 수정하지 않고 exact integration delta를 P02I에 handoff
- owner-local transaction/fault tests와 P02B proof/test-authority delta

RepoMap persistence/store/activation/quarantine filesystem, compiler owner, shared public/wire baseline은 수정하지 않는다.

## global sequence authority

- `catalog_sequence_v2`는 state root의 sole allocator다. sequence는 SQLite와 wire 모두 exact
  `1..=i64::MAX`, global `UNIQUE`다. `i64::MAX` 발급 transaction이 `next=NULL, exhausted=1`을 함께 commit하고
  이후 allocation은 mutation 전 `SEQUENCE_EXHAUSTED`이며 wrap/cast하지 않는다.
- `catalog_sequence_event_v2`는 every allocated sequence, closed event kind, identity digest, payload digest,
  SEP-21-002의 exact event commitment와 row digest를 보존하는 generic ledger다.
- crate-private `append_sequence_event(tx, kind, identity_digest, payload_digest)`만 allocation할 수 있다. kind는 caller
  문자열이 아니라 closed enum/DB CHECK이며 operation committed/refused/aborted, candidate seal, activation, rollback,
  invalidation, quarantine record/discard를 표현한다.
- allocator update, generic event insert, domain terminal row는 같은 `BEGIN IMMEDIATE` transaction에서 commit한다.
  rollback이면 셋 모두 0이다.
- allocator/event/domain schema는 모든 digest에 `CHECK(length(column)=32)`를 사용한다. `BLOB(32)` 표기만으로
  길이를 강제했다고 간주하지 않으며 allocator row도 SEP-21-002 preimage로 self-digest한다.
- open/restore는 generic ledger만 사용해 exact 분기한다: empty → `(next=1, exhausted=0)`, max가 `i64::MAX` 미만 →
  `(next=max+1, exhausted=0)`, max가 `i64::MAX` → `(next=NULL, exhausted=1)`. 별도 integrity pass가 generic
  event↔kind-appropriate domain row의 exact pair를 전수 검증한다. domain max를 allocator repair에 쓰거나 future
  domain을 high-water SQL UNION으로 열거하지 않는다.

## operation journal

- `begin/finalize`를 `inspect`, `claim_prepared`, `record_refused`, `commit`, `recover`로 대체한다.
- `PreparedMutationV1`에 validated epoch/input commitment를 담고 apply 시 drift를 검출한다.
- catalog row에 typed state, owner, lease, fence, body/result/receipt digest, row digest를 둔다.
- stale owner/fence 교체 뒤 과거 worker commit을 막는다.
- `Committed`와 frozen-policy `Refused`를 exact replay한다.
- replay floor 아래 요청은 `SearchPlaneErrorCodeV2::OperationReplayFloor`로 거부한다.
- state-root 전역 `MutationCoordinatorV1`을 ingest/control/background durable mutation에 machine-enforce한다.
- status 조회는 authorization-ready context를 요구한다.
- SQLite durability pragma는 effective read-back과 crash fixture로 검증한다.

금지: mutable preflight 뒤 replay lookup, claim 뒤 최초 semantic validation, in-progress bool, ACK loss 재실행,
indefinite intent, caller-only sequence validation, domain별 allocator, future-domain hard-coded UNION, free-form error code,
RepoMap filesystem/activation 수정.

## DoD/proof

- ACK loss/base GC/config change replay, invalid auxiliary restart/retry, same-key different-body
- crash at inspect/claim/apply/terminal boundaries, stale worker commit, retention floor
- operation committed/refused와 synthetic future event를 interleave해 global positive unique monotonic sequence 증명
- transaction rollback 시 allocator/event/domain row 모두 0
- restored DB의 next가 max 아래/동일/위일 때 generic ledger 기반 exact reconciliation
- same-body replay가 sequence/storage/provider work를 추가하지 않음
- 신규 refusal은 P01A enum variant이며 raw string code 추가 0

proof node는 `p02b-operation-journal`이다. final handoff `artifacts/sep-21/handoffs/P02B.json`에 source/dirty digest,
schema/API, closed event-kind table/digest, migration impact, refusal/terminal matrix, command/counts, NOT_RUN, P03이
소비할 transaction-coupled sequence API/fixtures를 남긴다. explicit owner path만 단일 checkpoint commit으로 만들고
current lane branch에 non-force push한다.
