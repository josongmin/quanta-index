# S21-09 — Supervised Runtime and Bounded Shutdown

Status: historical design record; current implementation and proof state must be read from source and `tools/ci/proof-authority.toml`.

2026-09-26: reporting-child finished-handle loss is repaired in serving/drain/
rollback; six owner counterexamples added. Actual release-daemon component-loss
proof remains NOT_RUN. See [current status](CURRENT-RESIDUAL-2026-09-26.md).

2026-09-26 follow-up: drain, required-child-loss and rollback now charge
cooperative waiting and stop callbacks to one phase-origin hard deadline;
unrepresentable deadlines exhaust the budget. The final source-stable
`just rust-profile test-runtime-supervisor-owner` executed 24 cases, all passed,
zero skipped, on main `7d581d0572447e4f29ce0d0ea2f9d2fbc8b1353c`
with unrelated Python/docs dirt. This is local owner proof, not arbitrary
blocking-callback containment or the mandatory release/Linux process proofs.
Raw-log and source digests are in [execution evidence](EXECUTION-PROGRESS.md).

Depends on: S21-00, S21-04, S21-08

## Goal

release daemon의 plane, maintenance, connection, provider task를 하나의 supervised lifecycle로 소유하고
startup/shutdown/panic을 bounded state transition으로 만든다.

## Initial audit root cause

- release binary에 signal wiring이 없음
- plane을 순차 spawn하지만 unexpected exit를 shutdown 전에 관측하지 않음
- partial startup 실패 시 이미 시작한 plane rollback이 없음
- connection/plane/maintenance join에 hard deadline이 없음
- panic cleanup이 normal tail에 의존
- `searchd.rs::drive`의 owned partial destructure가 `_maintenance`, `_search_corpus_lifecycle`,
  `_state_root_lease`를 server spawn 전에 drop하여 serving lifetime ownership을 깨뜨림

## Supervisor state machine

```text
Starting -> Ready -> Draining -> Stopped
    |         |         |
    +-------> Failed <--+
```

- child registry가 name, join handle, readiness, heartbeat, stop token, terminal result를 소유
- any required child unexpected exit -> readiness down -> global cancellation -> non-zero outcome
- startup은 all-or-rollback
- drain은 cooperative deadline과 hard deadline/escalation을 분리

## Original work items (recheck current source)

1. SIGINT/SIGTERM을 process cancellation root에 연결
2. query/control/ingest/maintenance/provider executors를 supervised child로 등록
3. startup rollback은 역순 stop/join 후 lease/socket cleanup
4. connection permit/live counter/peer-watch를 RAII guard로 전환
5. child panic payload와 close reason을 terminal event로 수집
6. connection, mutation, maintenance, provider별 cooperative cancellation checkpoint
7. hard drain deadline과 escalation policy
8. socket unlink와 state-root lease release는 all-child termination 뒤 수행
9. cadence/config upper bounds와 zero/overflow validation
10. process exit code semantics 고정
11. 기존 state-root lock file의 owner와 exact mode를 검증하고 foreign/permissive lock을 거부

## Fault scenarios

- second/third plane spawn failure
- accept loop and connection dispatcher panic
- maintenance panic and blocked filesystem operation
- signal during startup, mutation, provider call, GC, response write
- child ignores cooperative cancel
- repeated slowloris connections at shutdown
- second daemon start and lease ownership
- pre-existing foreign-owner or permissive-mode regular lock file

## Owner files

- `crates/quanta-index-searchd-runtime/src/`
- `crates/quanta-index-searchd/src/app/{searchd,runtime,maintenance}.rs`
- `crates/quanta-index-ipc/src/{server,counters}.rs`
- provider executor from S21-08
- process test helpers

## Acceptance

- real SIGINT/SIGTERM reaches `Stopped` within configured hard deadline or deterministic escalation
- one required child death cannot leave a partial-ready process
- startup failure leaves thread/listener/socket inode/lease residue 0
- panic/cancel paths reconcile all permits/live counters/peer-watch resources
- every join is owned and deadline-governed
- shutdown receipt names unfinished/escalated work without claiming graceful success
- state-root lease는 path/regular-file뿐 아니라 expected owner와 exact permission을 만족

## Verification

- real release binary child-process signal tests
- injected spawn/accept/dispatch/maintenance/provider failures
- FD/thread/socket/lease before-after accounting
- daemon lifecycle, socket admission, process envelope, chaos rails
- Linux production host proof required for final closeout

## No patch-on-patch rule

signal handler만 추가하거나 join에 개별 timeout을 흩뿌리지 않는다. supervisor registry와 cancellation
tree가 모든 process resources의 유일 owner가 되게 한다.

## P0 static evidence and structural fix

- `crates/quanta-index-searchd/src/app/searchd.rs:21-27`은 `SearchdRuntime`에서 세 server만 move하고 `..`로
  나머지 owned fields를 즉시 drop한다.
- `crates/quanta-index-searchd/src/app/runtime.rs:1036-1043`은 maintenance/lifecycle/state-root lease가 runtime
  drop까지 살아 있다고 명시한다. 현재 `drive`는 이 불변식을 위반하므로 두 번째 daemon이 첫 daemon의
  serving 중 같은 state root lease를 획득할 수 있다.
- `drive`를 `SearchdSupervisor`로 교체하고 `RuntimeGuards { maintenance, lifecycle, state_root_lease }`와
  모든 child registry를 supervisor가 직접 소유한다. guard drop은 모든 child/connection/provider join 또는
  명시적 hard escalation 뒤에만 가능하다.

### File-level action list

| File / symbol | Logic | DoD |
|---|---|---|
| `crates/quanta-index-searchd-runtime/src/lib.rs` | signal root, supervisor state machine, child registry, deadline/escalation receipt | signal→non-zero/Stopped 결정적 |
| `crates/quanta-index-searchd/src/app/searchd.rs::drive` | partial destructure 제거; supervisor가 whole runtime/guards 소유 | serving interval 전체 lease 유지 |
| `crates/quanta-index-searchd/src/app/runtime.rs::SearchdRuntime` | servers와 lifetime guards를 명시적 ownership bundle로 분리 | field-order comment에 의존하지 않음 |
| `crates/quanta-index-searchd/src/app/maintenance.rs` | cancellable checkpoint와 join handle 반환 | detached maintenance 0 |
| `crates/quanta-index-ipc/src/server.rs` | accept/connection join을 registry에 등록; finished handle도 join 결과 수집 | panic/early finish 유실 0 |
| `crates/quanta-index-ipc/src/counters.rs` 및 peer-watch | permit/live/counter를 RAII guard로 통합 | cancel/panic 후 baseline |

### Mandatory process proofs

- `e2e_supervisor_lifecycle.rs`: required child unexpected exit → readiness false → global drain → non-zero.
- `e2e_signal_shutdown.rs`: release binary SIGINT/SIGTERM, hard deadline, socket/lease/thread/FD residue 0.
- `e2e_startup_rollback.rs`: second/third spawn failure마다 역순 cleanup.
- `e2e_hard_drain.rs`: cooperative cancel 무시 worker의 escalation이 graceful로 오표기되지 않음.
- `supervised_connection_accounting.rs`: connection panic/cancel/slowloris에서 join과 counters exact.
- real two-process state-root test: 첫 process serving 동안 둘째가 typed lease refusal; 첫 process의 모든 child 종료
  뒤에만 획득 가능. lock은 expected uid, exact mode, regular-file, `nlink == 1`을 검증한다.
