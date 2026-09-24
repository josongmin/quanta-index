# Copy/paste prompt — P05 Query Completeness, Continuation and Provenance

> Historical lane prompt. Recheck current source, registry, and [residual plan](../FINAL-RESIDUAL-EXECUTION-PLAN.md) before use. Do not treat this text as a current execution order or proof receipt.

당신은 S21-06 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. P04 M2-stack checkpoint SHA와
handoff의 `QueryReadViewV2`/domain evidence가 current stack에 있을 때
시작한다. 이 lane은 M2 stacked checkpoint B이며 S21-05/06을 아직 `done`으로 닫지 않는다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-06-query-completeness-continuation-and-provenance.md`,
P04 handoff.

목표: pagination, completeness, ranking, availability, explain이 실제 execution outcome과 동일한 의미를 갖게 한다.

owner files:

- `crates/quanta-index-contract-base/src/results/query_window.rs`
- contract cursor/result/explanation DTOs
- `crates/quanta-index-core/src/domains/{semantic,hybrid}/`
- `crates/quanta-index-search-plane/src/query_dispatcher/{dense_admission,window,semantic_query}.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher/routes/{hybrid,hybrid_seed}.rs`
- cursor key owner, all cursor codecs/validators, response budget/keyset page/ranking, explain route,
  `crates/quanta-index-contract/src/results/query_responses.rs`, searchctl/harness renderers
- owner-local keyset/runtime/oracle tests, suite modules, `tools/ci/{test-authority,proof-authority}.toml`, Just recipe

구현 요구:

- closed `ExecutionOutcomeV2`: `ExactExhausted`, `LowerBound { continuation }`, `CappedUnknown { cap }`,
  `InterruptedPartial { reason }`, `Approximate { method, quality_contract }`
- unsupported/unavailable/not-ready는 success outcome이 아니라 stable typed refusal
- `QueryResultWindowV2`에 mandatory completeness/coverage/exhaustion proof
- pageable route: lexical/symbol/history/runtime/structural
- bounded top-k route: semantic/hybrid/hybrid-seed/RepoMap; pagination을 거짓 암시하지 않음
- `CursorEnvelopeV2`는 canonical CBOR + HMAC-SHA256 + base64url-no-pad이며 route/full pin/read identity/normalized
  query/constraints/order/cap/aux epochs/key ID/issued-at/expiry/version을 결속한다.
- persistent random 32-byte cursor key, mode `0600`, default TTL 15분/max 1시간. signature/version/expiry/context를
  resource acquisition 전에 검증하고 unsigned/V1 live decoder는 제거한다.
- dense admission의 examined/refill ceiling/cap/cancel outcome을 outer window까지 보존
- typed candidate identity를 fusion/dedup/window/explain 전체에 사용
- post-dedup compact rank를 RRF와 contribution trace가 공유
- `engines_executed`와 `lanes_contributed`, zero-hit와 unavailable을 분리
- zero digest sentinel 제거

금지: returned `< top_k`로 exhaustion 추론, `Capped => has_more=true` route patch, raw-row rank를 provenance에 사용,
availability를 hit count로 추론, client-editable boundary를 continuation으로 신뢰.

DoD:

- `has_more=false`는 verifiable exhaustion proof 없이는 생성 불가
- capped/partial/approximate가 exact로 승격되는 code path 0
- `allow_partial=false`에서 partial lane이 생기면 whole-query typed refusal
- independent RRF/window oracle가 order/rank/score/next boundary를 재계산
- cursor의 repo/revision/route/query/order/cap/epoch/read identity/tamper/expiry/version negative matrix
- available-empty, filtered-empty, zero-hit executed는 서로 다른 provenance; unavailable는 typed refusal
- explain mismatch는 typed fail-closed

owner node expected tuple은 `id=p05-query-truth-owner`, `family=U`, `required_host=any`,
`dependencies=[p04-read-view-lifetime-owner]`다. release node는 `id=p05-query-truth`, `family=Q`,
`required_host=linux-production-like`, `dependencies=[p05-query-truth-owner,p04-read-view-lifetime]`다. dedicated `just rust-proof-p05-query-truth`는 최소
`searchd-runtime-e2e-keyset-cursors`, `searchd-runtime-e2e-exact-count-window`,
`searchd-runtime-e2e-semantic-scope-cap`, `searchd-runtime-explain`, independent RRF/window oracle, full-field cursor
tamper target과 `just rust-verify-quality-all`을 모두 실행한다. dry-run selection artifact와 command별 counts를 분리한다.
현재 command가 quality rail만 실행하면 구현 전에 recipe/registry를 이 lane에서 고치고 source-bound proof를 다시
만든다. target이 비어 있거나 실제 선택되지 않으면 `BLOCKED`다. public DTO/decoder 변경에는 `just rust-public-api`,
`just rust-wire-inventory`, `just rust-fuzz-smoke`를 추가한다. Linux production-like/release-daemon proof가 필요하다.
최종 보고에 source freeze, route capability matrix, schema bump, oracle independence, command별 covered/excluded surface와
counts, NOT_RUN, P06 expected response context inputs와 `artifacts/sep-21/handoffs/P05.json`을 남겨라. explicit owner
path만 checkpoint commit하고 current lane branch에 non-force push한다.
