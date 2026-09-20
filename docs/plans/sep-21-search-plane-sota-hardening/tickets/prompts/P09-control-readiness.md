# Copy/paste prompt — P09 Control Authorization, Readiness and Observability

당신은 S21-10 owner다. P08 supervisor events/state와 P02B operation status contract가 current source에 merge된 뒤
시작한다.

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

최종 보고에 source freeze, capability table, readiness truth table, diagnostic bounds, proof counts, NOT_RUN, M3 residual
risk를 남겨라. commit/push는 요청 시에만 한다.
