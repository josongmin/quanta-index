# Copy/paste prompt — P09 Control Authorization, Readiness and Observability

당신은 S21-10 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. immediate P08 supervisor handoff가 start
HEAD와 exact match하고 P02B operation-status contract/schema가 current tracked source에 보존될 때만 시작한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-10-control-authorization-readiness-and-observability.md`,
P08 handoff와 current P02B operation-status contract/schema.

목표: kernel-derived principal을 operation capability에 default-deny로 결속하고 process readiness와 bounded request
diagnostics를 구현한다.

owner files:

- `crates/quanta-index-ipc/src/{socket_access,server}.rs`
- `crates/quanta-index-search-plane/src/control_dispatcher.rs`
- `crates/quanta-index-searchd/src/app/{socket_access,runtime}.rs`
- `crates/quanta-index-contract/src/ipc/{control,metrics,split,mod}.rs`
- `crates/quanta-index-sdk/src/{client,config,observability,runtime}.rs`
- `crates/quanta-index-searchctl/src/{lib,main}.rs`
- `crates/quanta-index-search-plane/src/readiness/`와 bounded diagnostics/metrics owner
- `crates/quanta-index-searchd/src/app/config.rs`
- control/readiness/process negative tests and suite modules, `tools/ci/{test-authority,proof-authority}.toml`, Just recipe
- public API baselines와 `tools/ci/inventory/wire-surface.toml`

위 디렉터리/개념 표현은 wildcard 권한이 아니다. common contract의 owner-freeze table에서 exact path/symbol/base blob과
목적을 확정한 뒤에만 편집한다.

구현 요구:

- socket admission이 `Accepted(PeerCredentials)`와 resolved principal을 보존
- transport가 `DispatchContextV1 { request_id, plane, principal, connection_id, deadline, cancellation }` 생성
- self-asserted payload principal 금지
- 모든 control opcode가 Observe/Operate/MutateGeneration/Admin 중 exactly one capability에 exhaustive mapping
- dispatcher match/mutation 전에 authorize; unknown/default deny
- operation status lookup도 owner/admin capability 확인
- `ProcessReadinessV1`은 supervisor/required planes/maintenance/backend/candidate integrity/provider profile을 합성
- repository generation status와 process readiness를 별도 DTO/command로 유지
- request correlation은 bounded ring/trace sink; request/repo/generation을 unbounded metric label로 쓰지 않음
- backend invocation `engines_executed`와 post-filter `lanes_contributed` 분리

금지: opcode별 ad-hoc UID check, credentials discard, surviving socket=healthy, payload/credential logging,
high-cardinality correlation labels, zero-hit backend를 미실행 처리.

DoD:

- principal×opcode capability negative matrix에서 unauthorized mutation 0 및 typed audit
- unknown/mismatched credentials default deny
- query/control/ingest/maintenance/backend 각각 kill/stale/corrupt 시 global ready=false
- request ID로 queue/dispatch/backend/provider/response/close outcome 상관 가능
- zero-hit executed counter 증가, contribution counter 미증가
- label cardinality와 secret/source/query leakage budget 준수
- CLI help/version/capability snapshot이 current contract에서 생성
- payload가 principal/UID/capability를 spoof해도 kernel-derived principal만 authority
- control opcode registry와 exhaustive capability mapping/wire inventory가 양방향 exact
- provider profile의 required/degraded/disabled 상태별 readiness truth table과 supervisor state 일치
- active repository 0개이고 required process/provider가 healthy이면 `ready=true`; candidate integrity gate는 active
  candidate가 있을 때만 적용

owner node expected tuple은 `id=p09-control-readiness-owner`, `family=P`, `required_host=any`,
`dependencies=[p08-runtime-supervisor-owner]`다. release node는 `id=p09-control-readiness`, `family=P`,
`required_host=linux-production-like`, `dependencies=[p09-control-readiness-owner,p08-runtime-supervisor]`다. transitive
P02B contract는 current source digest로 소비하며 redundant direct dependency를 추가하지 않는다. `test_authority_targets`가
비어 있으면 production implementation 전에 same lane에서 exact tests/suite/target/recipe/selector를 bootstrap한다.
capability/readiness/credential-spoof/component-kill scenarios를 dry-run에서 실제 선택할 수 없을 때 `BLOCKED`다. canonical release command는
Cargo test binary가 아니라 `release-daemon-fresh`의 exact path/hash를 process harness에 주입하는 dedicated recipe여야
한다. registry의 `just rust-profile test-daemon`이 이를 보장하지 않으면 recipe/registry를 먼저 고친다. Linux
production-like/release-daemon proof가 필요하다. component kill/corruption은 disposable local process/state-root에서만
수행하고 shared daemon kill은 별도 승인이 필요하다. 최종 보고에 source freeze, capability table, readiness truth
table, diagnostic bounds, proof counts, NOT_RUN, M3 residual risk와 `artifacts/sep-21/handoffs/P09.json`을 남겨라. explicit
owner path만 checkpoint commit하고 current lane branch에 non-force push한다.
