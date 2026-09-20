# Copy/paste prompt — P02B Operation Journal Lane

당신은 S21-04 lane owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. P01 checkpoint commit과 source-bound
handoff를 exact base로 별도 worktree/branch에서 작업한다. P02A와
병렬 실행하되 shared contract/baseline/inventory/generated docs는 P02I owner에게 delta만 넘긴다. compiler raw DTO를
임의로 변경하지 마라.

읽을 문서:

- repo instructions
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-04-operation-journal-and-sequence-authority.md`
- S21-00 receipt/refusal/concurrency/replay decisions와 S21-01 handoff

목표: 모든 ingest/auxiliary mutation을 replay-first, immutable-prepare, fenced-claim, terminally-classified protocol로
교체한다.

target flow:

`digest/auth → inspect → replay/conflict → immutable prepare → record_refused | claim_prepared → apply prepared plan → fenced terminal commit → recover`

owner files:

- `crates/quanta-index-core/src/domains/idempotency.rs`
- `crates/quanta-index-catalog/src/{idempotency,open,auxiliary}.rs`
- `crates/quanta-index-search-plane/src/ingest_dispatcher/`
- `crates/quanta-index-search-plane/src/auxiliary_authority.rs`
- operation status/receipt contract and SDK section allocated to this lane

구현 요구:

- `begin/finalize`를 `inspect`, `claim_prepared`, `record_refused`, `commit`, `recover`로 대체
- `PreparedMutationV1`에 validated epoch/input commitment를 담고 apply 시 drift를 검출
- catalog row에 typed state, owner, lease, fence, body/result/receipt digest, row digest
- positive terminal sequence `CHECK`, declared stream `UNIQUE`, `next > MAX(terminal)` open/restore reconciliation
- stale owner/fence 교체 뒤 과거 worker commit 불가
- terminal `Committed`와 frozen-policy `Refused` exact replay
- replay-floor 아래 요청의 `OPERATION_REPLAY_FLOOR`
- state-root 전역 `MutationCoordinatorV1`을 ingest/control/background durable mutation 모두에 machine-enforce
- P00이 동결한 exact terminal sequence stream key와 UNIQUE scope를 schema/checker에 반영
- operation status 조회는 authorization-ready context를 요구
- SQLite durability pragma의 effective read-back 및 documented guarantee

금지: mutable preflight 후 replay lookup, claim 후 최초 semantic validation, in-progress bool만 유지, ACK loss 시
provider/storage 재실행, indefinite intent, caller-only sequence validation.

DoD: ACK-loss/base-GC/config-change replay, invalid auxiliary restart/retry, same-key different body, crash at every
boundary, restore regression, stale-worker commit, retention-floor fixtures를 실행하고 counts를 남긴다.

proof node는 `p02b-operation-journal`다. 최종 handoff `artifacts/sep-21/handoffs/P02B.json`에 source/dirty digest, schema/API,
migration impact, refusal/terminal matrix, sequence scope, command/counts, NOT_RUN, P03이 소비할 journal API/fixtures를
남겨라. explicit owner path만 단일 checkpoint commit으로 만들고 push는 별도 요청 시에만 한다.
