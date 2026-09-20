# Copy/paste prompt — P09 Control Authorization, Readiness and Observability

당신은 S21-10 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. P08 supervisor handoff와 P02B
operation-status handoff가 current source에 결속될 때만 시작한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-10-control-authorization-readiness-and-observability.md`,
P02B/P08 handoff.

목표: kernel-derived principal을 operation capability에 default-deny로 결속하고 process readiness와 bounded request
diagnostics를 구현한다.

owner files:

- `crates/quanta-index-ipc/src/{socket_access,server}.rs`
- `crates/quanta-index-search-plane/src/control_dispatcher.rs`
- `crates/quanta-index-searchd/src/app/{socket_access,runtime}.rs`
- control/readiness/status contract, SDK, searchctl, config and metrics/diagnostics owners

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

proof node는 `p09-control-readiness`, canonical release command는 `just rust-profile test-daemon`이며 Linux
production-like/release-daemon proof가 필요하다. component kill/corruption은 disposable local process/state-root에서만
수행하고 shared daemon kill은 별도 승인이 필요하다. 최종 보고에 source freeze, capability table, readiness truth
table, diagnostic bounds, proof counts, NOT_RUN, M3 residual risk와 `docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/P09.json`을 남겨라. explicit
owner path만 checkpoint commit하고 push는 별도 요청 시에만 한다.
