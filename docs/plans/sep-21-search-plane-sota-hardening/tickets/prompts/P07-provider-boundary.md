# Copy/paste prompt — P07 Semantic Admission and Provider Boundary

당신은 S21-08 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. M2 query/SDK checkpoint와 handoff가
current stack에 있을 때 시작한다. 이 lane은 M3 interface
checkpoint이며 P08 lifecycle integration 전 S21-08을 단독 `done`으로 닫지 않는다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-08-semantic-admission-and-provider-boundary.md`,
S21-00 provider policy ADR, P05/P06 handoff.

목표: provider I/O 전에 가능한 모든 validation/policy를 끝내고 admitted provider work를 process-global로
reserve/cancel/account할 interface를 만든다. S21-08은 admission/interface를 소유하고 S21-09는 task lifecycle/join을
소유한다.

owner files:

- `crates/quanta-index-core/src/domains/semantic/`
- semantic/hybrid route admission
- `crates/quanta-index-search-plane/src/query_embedder.rs`
- `crates/quanta-index-search-plane/src/semantic_derive.rs`
- `crates/quanta-index-embed/src/openai.rs`
- `crates/quanta-index-embed/src/{cache,telemetry,lib}.rs`와 `openai/{batching,retry}.rs`
- `crates/quanta-index-searchd/src/app/{config,runtime}.rs` 중 provider composition section
- provider/egress config, secret owner, audit telemetry

구현 순서:

1. common normalization/input policy를 profile-independent core owner에 둔다.
2. `QueryText`와 `SourceContent`를 별도 data class/grant로 판정한다. 동일 enforcement engine을 사용하되 source
   content는 별도 explicit grant가 없으면 거부한다. tenant/provider/endpoint/region/retention/model-revision/profile
   필드가 하나라도 없으면 I/O 전에 `PROVIDER_EGRESS_DENIED`다.
3. global request/thread/FD/inflight-byte/retry/token/cost reservation contract를 구현한다.
4. query text와 source derive content가 각자의 classification/grant를 보존한 채 같은 redaction/audit engine을
   통과하게 한다.
5. `EmbeddingOutcomeV1`에 declared/observed model, dimension, finite/norm, usage/cost/outcome을 담는다.
6. OpenAI adapter의 payload, timeout/retry, validation, secret-safe error를 한 boundary에 둔다.
7. executor pool 내부 task registry/reservation/settlement와 supervisor-enrollment handle을 구현한다. P08은 이
   handle을 process child registry에 등록해 drain/join/escalation한다.

금지: route-local empty check, detached OS thread, semaphore로 unowned work를 감싸기, policy 후 provider call,
raw source/query/credential을 log/metric/cache key/receipt에 기록, provider retry를 operation replay로 오인.

DoD:

- whitespace/punctuation/tokenless/model mismatch/policy denial에서 query와 source derive provider call 0
- hash/stub/OpenAI가 동일 public input/error semantics
- unexpected model/dimension/NaN/norm은 success/cache/receipt 0
- cancel/timeout/retry reservation reconciliation
- redaction/leak scanner 0 findings
- controlled HTTP spy와 budgeted opt-in real-provider proof는 구분 기록
- provider network/credential/cost 승인이 없으면 real-provider proof는 NOT_RUN이며 hash/stub evidence로 대체 금지

proof node는 `p07-provider-boundary`, canonical release command는 `just rust-profile test-integration-semantic`이며
Linux production-like/release-daemon/승인된 real-provider proof가 필요하다. 최종 handoff
`docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/P07.json`에 source freeze, admission order, reservation/executor API, config caps, egress matrix,
commands/counts, NOT_RUN real-provider proof, P08 lifecycle invariants를 남겨라. explicit owner path만 checkpoint
commit하고 push는 별도 요청 시에만 한다.
