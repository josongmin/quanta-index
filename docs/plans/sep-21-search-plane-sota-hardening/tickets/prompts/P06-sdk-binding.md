# Copy/paste prompt — P06 SDK and Wire Binding

당신은 S21-07 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. P03/P02B/P05 checkpoint와 handoff가
current M2 stack에 있을 때 시작한다. 이 lane은 P05 public schema를
소비자 쪽에서 닫으며 schema 재설계가 필요하면 수정하지 말고 P05 owner로 되돌린다. immediate P05
checkpoint/handoff/proof binding이 없거나 current source와 다르면 `BLOCKED`로 종료한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-07-sdk-wire-response-binding.md`,
S21-02/S21-04/P05 handoff.

목표: 모든 public SDK method가 response variant뿐 아니라 원 요청과 semantic authority를 검증하게 한다.

owner files:

- `crates/quanta-index-sdk/src/client.rs`
- SDK lexical/semantic/search/history/runtime/structural/RepoMap/control/ingest modules
- `crates/quanta-index-sdk/src/config.rs`
- `crates/quanta-index-contract/src/results/query_responses.rs`
- `crates/quanta-index-contract/src/ipc/{split,control,ingest}.rs`
- contract RepoMap/cursor intrinsic validators
- SDK `error.rs`, `transport.rs`, route wrappers
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
- exported public method 전수와 coverage table exact match; contextual validator 없는 same-variant success path 0
- query-only profile은 dummy control/ingest transport를 만들지 않음

proof node는 `p06-sdk-binding`, canonical release process command는 `just rust-profile test-daemon`이다. public
contract/SDK/wire 변경에는 `just rust-public-api`, `just rust-wire-inventory`, `just rust-fuzz-smoke`를 같은 source에서
실행한다. Linux production-like/release-daemon proof가 필요하다. 최종 보고에 method coverage table, source freeze,
schema/baseline changes, negative matrix counts, NOT_RUN, P07/P08 request context와 `artifacts/sep-21/handoffs/P06.json`을
남겨라. 이 checkpoint에서 M2의 S21-05/06/07을 함께 닫을 수 있는지 판정한다. explicit owner path만 checkpoint
commit하고 current lane branch에 non-force push한다.
