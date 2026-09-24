# Copy/paste prompt — P08 Runtime Supervisor

> Historical lane prompt. Recheck current source, registry, and [residual plan](../FINAL-RESIDUAL-EXECUTION-PLAN.md) before use. Do not treat this text as a current execution order or proof receipt.

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
- supervisor/process suite module과 exact test files, `tools/ci/{test-authority,proof-authority}.toml`, required Just recipe

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
- live work가 남은 상태에서 graceful `drive` return 발생 건수 = 0; child 종료 전 lease drop 발생 건수 = 0

owner node expected tuple은 `id=p08-runtime-supervisor-owner`, `family=U`, `required_host=any`,
`dependencies=[p07-provider-boundary-owner]`이며 disposable local processes만 사용한다. release node는
`id=p08-runtime-supervisor`, `family=P`, `required_host=linux-production-like`,
`dependencies=[p08-runtime-supervisor-owner,p07-provider-boundary]`다. `test_authority_targets`가 비어 있으면 production implementation 전에 같은
lane에서 proof bootstrap을 먼저 수행한다. exact process test/suite, target ID, recipe, selector를 등록하고 dry-run이
mandatory process/signal/rollback/lease scenario를 실제 선택하지 못할 때 `BLOCKED`다. canonical
`just rust-proof-p08-runtime-supervisor`는 먼저 `just rust-profile release-daemon-fresh`로 exact release daemon path/SHA를
동결하고 process harness에 `QUANTA_INDEX_SEARCHD_BIN`으로 주입한다. release proof에서 `CARGO_BIN_EXE_*` 사용은 금지한다.
`test-daemon-all`은 subordinate scenario rail이며 manifest binary와 실제 child executable이 exact match해야 한다.
signal/kill/slowloris/lease proof는 task가 직접 spawn한 disposable process/state root만 대상으로 한다. facade/re-export 변경에는 `just rust-hexagonal`, `just rust-cargo-modules`,
`just rust-public-api`를 추가한다. macOS focused proof를 Linux production proof로 승격하지 마라. 최종 보고에 source freeze, lifecycle diagram, owned
resource inventory, exit semantics, command/counts, platform별 NOT_RUN, P09 readiness events와
`artifacts/sep-21/handoffs/P08.json`을 남겨라. S21-08/09 lifecycle closure를 선언하려면 clean P08 result HEAD에서
P07/P08 mandatory selectors를 재실행해 same-HEAD M3 integration receipt를 만든다. 없으면 atomic closure를 선언하지 않는다. explicit
owner path만 checkpoint commit하고 current lane branch에 non-force push한다.
