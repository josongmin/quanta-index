# Copy/paste prompt — P06 SDK and Wire Binding

당신은 S21-07 owner다. S21-02, S21-04, S21-06 contract가 current source에 merge된 뒤 시작한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-07-sdk-wire-response-binding.md`,
S21-02/S21-04/P05 handoff.

목표: 모든 public SDK method가 response variant뿐 아니라 원 요청과 semantic authority를 검증하게 한다.

owner files:

- `crates/quanta-index-sdk/src/client.rs`
- SDK lexical/semantic/search/history/runtime/structural/RepoMap/control/ingest modules
- `crates/quanta-index-sdk/src/config.rs`
- `crates/quanta-index-contract/src/ipc/query_responses.rs`
- contract RepoMap/cursor intrinsic validators
- IPC envelope tests/public API baselines

구현 요구:

- payload move 전에 plane별 closed enum `ExpectedQueryResponseV1`, `ExpectedControlResponseV1`,
  `ExpectedIngestResponseV1` 생성
- request ID 확인 후 intrinsic decoder validation, 그 다음 contextual binding
- pinned selector는 full pin/read identity exact match
- active selector는 resolution proof, activation epoch, read identity 검증
- candidate repo/revision/generation/domain/order/cardinality/projection/commitment 검증
- mutation ACK의 operation key/body digest/target/prior/new commitment/terminal status 검증
- SDK는 sequence positive/intrinsic consistency만 확인; global monotonic authority는 catalog에 둔다.
- mismatch error는 route/axis/stable code를 노출하되 payload를 누출하지 않는다.
- query-only profile이 control/ingest endpoints를 요구하지 않게 한다.

금지: dynamic registry의 누락 가능한 success path, method별 비교 복붙, wildcard success, debug string parsing,
unresolved active selector와 resolved pin의 단순 equality.

DoD:

- 모든 public SDK method가 closed expected enum의 정확히 한 variant에 exhaustive mapping
- correct request ID/variant지만 wrong repo/revision/generation/order/commitment/projection인 mock wire 전부 거부
- active proof missing/stale/wrong activation 거부
- malformed intrinsic shape는 contract decoder, contextual mismatch는 SDK가 구분해 거부
- real UDS positive consumer 및 query-only endpoint proof

최종 보고에 method coverage table, source freeze, schema/baseline changes, negative matrix counts, NOT_RUN, P07/P08에
영향 주는 request context를 남겨라. commit/push는 요청 시에만 한다.
