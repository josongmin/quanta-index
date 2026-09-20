# Copy/paste prompt — P08 Runtime Supervisor

당신은 S21-09 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. P07 interface checkpoint SHA와
handoff가 current M3 stack에 있을 때 시작한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-09-supervised-runtime-and-bounded-shutdown.md`, P07 handoff.

최우선 P0: `crates/quanta-index-searchd/src/app/searchd.rs::drive`의 owned partial destructure가 servers 외 runtime
fields를 serving 전에 drop한다. `runtime.rs::SearchdRuntime`의 maintenance/lifecycle/state-root lease lifetime comment와
실제 동작이 모순이다. 이 owner bug부터 구조적으로 제거한다.

목표: 하나의 `SearchdSupervisor`가 plane, maintenance, connections, peer-watch, provider tasks, runtime guards,
startup rollback, signal/drain/escalation을 소유하게 한다.

owner files:

- `crates/quanta-index-searchd-runtime/src/lib.rs`
- `crates/quanta-index-searchd-runtime/src/bin/quanta-index-searchd.rs`
- `crates/quanta-index-searchd/src/app/{searchd,runtime,maintenance}.rs`
- `crates/quanta-index-searchd/src/{lib.rs,app/mod.rs}` composition/re-export section
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
- cooperative checkpoint와 frozen hard deadline 125초
- clean operator shutdown exit 0; required child death/startup rollback/hard deadline exit 70; 두 번째 signal은 즉시
  `128 + signal`; hard deadline은 graceful success가 아님
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
- live work가 남은 상태의 정상 `drive` return 0; child 종료 전 lease drop 0

proof node는 `p08-runtime-supervisor`, canonical release command는 `just rust-profile test-daemon-all`이다. macOS
focused proof를 Linux production proof로 승격하지 마라. 최종 보고에 source freeze, lifecycle diagram, owned
resource inventory, exit semantics, command/counts, platform별 NOT_RUN, P09 readiness events와
`docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/P08.json`을 남겨라. 이 checkpoint에서 S21-08/09의 M3 lifecycle closure를 함께 판정한다. explicit
owner path만 checkpoint commit하고 push는 별도 요청 시에만 한다.
