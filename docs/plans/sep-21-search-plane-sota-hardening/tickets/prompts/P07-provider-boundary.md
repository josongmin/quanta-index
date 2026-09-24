# Copy/paste prompt — P07 Semantic Admission and Provider Boundary

> Historical lane prompt. Recheck current source, registry, and [residual plan](../FINAL-RESIDUAL-EXECUTION-PLAN.md) before use. Do not treat this text as a current execution order or proof receipt.

당신은 S21-08 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. M2 query/SDK checkpoint와 handoff가
current stack에 있을 때 시작한다. 이 lane은 M3 interface
checkpoint이며 P08 lifecycle integration 전 S21-08을 단독 `done`으로 닫지 않는다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-08-semantic-admission-and-provider-boundary.md`,
S21-00 provider policy ADR, P06 handoff. P05 의미는 current tracked schema/API digest로 소비한다.

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
- provider owner-local tests/suite module, `tools/ci/{test-authority,proof-authority}.toml`, required Just recipe

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
7. reservation/settlement API와 ownership-bearing supervisor-enrollment handle 생성까지만 구현한다. actual
   spawn/register/cancel/join/escalation과 terminal result 수집은 P08 owner다.

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

owner node expected tuple은 `id=p07-provider-boundary-owner`, `family=U`, `required_host=any`,
`dependencies=[p06-sdk-binding-owner]`이며 loopback/stub만 허용한다. release node는
`id=p07-provider-boundary`, `family=X`, `required_host=linux-production-like`,
`dependencies=[p07-provider-boundary-owner,p06-sdk-binding]`다. `test_authority_targets`가 비어 있으면 production implementation 전에 같은
lane에서 proof bootstrap을 먼저 수행한다. exact test file/suite, target ID, dedicated recipe, selector를 실제 선택 가능한
상태로 등록한다. placeholder/ignored-only/zero-test target은 금지하며 dry-run selection이 mandatory scenario를 포함하지
못할 때 `BLOCKED`다. canonical `just rust-proof-p07-provider-boundary`는 core admission, query/source path,
embed adapter의 loopback-only controlled HTTP, reservation/settlement, leak scan을 실행한다. `test-integration-semantic`은
subordinate storage rail이다. 비-loopback endpoint/credential/cost를 쓰는 real-provider subrail은 별도 승인과 fixed budget이
있을 때만 실행하고 별도 count/artifact로 기록한다. Linux production-like/release-daemon/승인된 real-provider proof가 필요하다. 최종 handoff
`artifacts/sep-21/handoffs/P07.json`에 source freeze, admission order, reservation/executor API, config caps, egress matrix,
commands/counts, NOT_RUN real-provider proof, P08 lifecycle invariants를 남겨라. explicit owner path만 checkpoint
commit하고 current lane branch에 non-force push한다.
