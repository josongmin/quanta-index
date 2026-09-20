# Copy/paste prompt — P08 Runtime Supervisor

당신은 S21-09 owner다. P07의 provider executor/reservation contract가 current source에 merge된 뒤 시작한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-09-supervised-runtime-and-bounded-shutdown.md`, P07 handoff.

최우선 P0: `crates/quanta-index-searchd/src/app/searchd.rs::drive`의 owned partial destructure가 servers 외 runtime
fields를 serving 전에 drop한다. `runtime.rs::SearchdRuntime`의 maintenance/lifecycle/state-root lease lifetime comment와
실제 동작이 모순이다. 이 owner bug부터 구조적으로 제거한다.

목표: 하나의 `SearchdSupervisor`가 plane, maintenance, connections, peer-watch, provider tasks, runtime guards,
startup rollback, signal/drain/escalation을 소유하게 한다.

owner files:

- `crates/quanta-index-searchd-runtime/src/lib.rs`
- `crates/quanta-index-searchd/src/app/{searchd,runtime,maintenance}.rs`
- `crates/quanta-index-ipc/src/{server,counters}.rs`
- P07 provider executor integration
- runtime/process test helpers

구현 요구:

- partial destructure 제거; supervisor가 whole runtime 또는 explicit `RuntimeGuards`를 소유
- guards: maintenance, corpus lifecycle, state-root lease; 모든 child 종료/escalation 전 drop 금지
- SIGINT/SIGTERM process cancellation root
- required child registry: join handle, readiness, heartbeat, cancellation, terminal result
- all-or-rollback startup과 역순 cleanup
- accept/connection/provider/maintenance finished handle도 반드시 join/terminal result 수집
- permit/live counter/peer-watch RAII
- cooperative checkpoint와 frozen hard-deadline escalation/exit code
- socket unlink와 lease release는 child/connection/provider termination 뒤
- state-root lock fstat: expected uid, exact mode, regular-file, `nlink == 1`

금지: signal handler만 추가, unbounded join, finished handle drop, kill 불가능한 thread를 남기고 graceful success,
lease를 먼저 release, panic cleanup을 normal tail에 의존.

mandatory proofs:

- release binary SIGINT/SIGTERM bounded shutdown
- required child unexpected exit → readiness down/global drain/non-zero
- second/third spawn failure rollback
- cooperative cancel 무시 worker escalation의 비-graceful 판정
- connection panic/cancel/slowloris join과 exact counters
- real two-process state-root exclusion: 첫 process serving 전체 동안 둘째 refusal, all-child 종료 뒤 획득
- before/after FD/thread/socket inode/lease/residual provider task accounting

macOS focused proof를 Linux production proof로 승격하지 마라. 최종 보고에 source freeze, lifecycle diagram,
owned resource inventory, exit semantics, command/counts, platform별 NOT_RUN, P09 readiness events를 남겨라.
commit/push는 요청 시에만 한다.
