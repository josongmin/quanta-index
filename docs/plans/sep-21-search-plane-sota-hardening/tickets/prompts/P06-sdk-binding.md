# Copy/paste prompt — P06 SDK and Wire Binding

당신은 S21-07 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. immediate P05 checkpoint와 handoff가
current M2 stack에 있을 때 시작한다. P03/P02B 의미는 current tracked schema/API digest로 소비한다. 이 lane은 P05 public schema를
소비자 쪽에서 닫으며 schema 재설계가 필요하면 수정하지 말고 P05 owner로 되돌린다. immediate P05
checkpoint/handoff/proof binding이 없거나 current source와 다르면 `BLOCKED`로 종료한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-07-sdk-wire-response-binding.md`, P05 handoff.

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
- SDK response-binding negative integration target, `tools/ci/{test-authority,proof-authority}.toml`, `Justfile`의
  P06 dedicated recipe, wire inventory

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
- coverage universe는 wire request를 송신하고 response를 반환하는 exported SDK entrypoint와 namespace wrapper다.
  pure builder setter/getter/local digest method는 제외하되 generated inventory에 사유를 기록한다.
- P05 DTO field/schema/codec와 P02B operation schema/status section은 수정하지 않는다. intrinsic validator 또는 SDK
  contextual binding에 필요한 mandatory field가 부족하면 P05/P02B owner로 되돌리고 `BLOCKED`다.

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

owner node expected tuple은 `id=p06-sdk-binding-owner`, `family=U`, `required_host=any`,
`dependencies=[p05-query-truth-owner]`다. release node는 `id=p06-sdk-binding`, `family=D`,
`required_host=linux-production-like`, `dependencies=[p06-sdk-binding-owner,p05-query-truth]`다. dedicated `just rust-proof-p06-sdk-binding`은 SDK mock negative target과 real UDS
`searchd-runtime-sdk-frontdoor`를 모두 선택한다. `just rust-profile test-daemon` 하나는 positive subordinate rail이며
negative matrix의 대체물이 아니다. public
contract/SDK/wire 변경에는 `just rust-public-api`, `just rust-wire-inventory`, `just rust-fuzz-smoke`를 같은 source에서
실행한다. Linux production-like/release-daemon proof가 필요하다. 최종 보고에 method coverage table, source freeze,
schema/baseline changes, negative matrix counts, NOT_RUN, P07/P08 request context와 `artifacts/sep-21/handoffs/P06.json`을
남겨라. 이 checkpoint에서 M2의 S21-05/06/07을 함께 닫으려면 clean P06 result HEAD에서 P04/P05/P06 mandatory
selectors를 모두 재실행해 same-HEAD M2 integration receipt를 만든다. 없으면 atomic closure를 선언하지 않는다. explicit owner path만 checkpoint
commit하고 current lane branch에 non-force push한다.
