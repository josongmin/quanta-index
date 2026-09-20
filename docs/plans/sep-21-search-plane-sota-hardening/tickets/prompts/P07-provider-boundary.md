# Copy/paste prompt — P07 Semantic Admission and Provider Boundary

당신은 S21-08 owner다. M2 query/SDK contract가 current source에 merge된 뒤 시작한다.

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
- provider/egress config, secret owner, audit telemetry

구현 순서:

1. common normalization/input policy를 profile-independent core owner에 둔다.
2. generation model/profile compatibility와 tenant/source/query egress policy를 provider fanout 전에 검사한다.
3. global request/thread/FD/inflight-byte/retry/token/cost reservation contract를 구현한다.
4. query text와 source derive content가 같은 classification/consent/region/retention/redaction boundary를 통과하게 한다.
5. `EmbeddingOutcomeV1`에 declared/observed model, dimension, finite/norm, usage/cost/outcome을 담는다.
6. OpenAI adapter의 payload, timeout/retry, validation, secret-safe error를 한 boundary에 둔다.
7. S21-09가 등록할 cancellable `ProviderExecutor` interface와 settlement contract를 freeze한다.

금지: route-local empty check, detached OS thread, semaphore로 unowned work를 감싸기, policy 후 provider call,
raw source/query/credential을 log/metric/cache key/receipt에 기록, provider retry를 operation replay로 오인.

DoD:

- whitespace/punctuation/tokenless/model mismatch/policy denial에서 query와 source derive provider call 0
- hash/stub/OpenAI가 동일 public input/error semantics
- unexpected model/dimension/NaN/norm은 success/cache/receipt 0
- cancel/timeout/retry reservation reconciliation
- redaction/leak scanner 0 findings
- controlled HTTP spy와 budgeted opt-in real-provider proof는 구분 기록

최종 handoff에 source freeze, admission order, reservation/executor API, config caps, egress matrix, commands/counts,
NOT_RUN real-provider proof, P08가 소유할 task lifecycle invariants를 남겨라. commit/push는 요청 시에만 한다.
