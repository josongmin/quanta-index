# Copy/paste prompt — P05 Query Completeness, Continuation and Provenance

당신은 S21-06 owner다. P04의 `QueryReadViewV2`와 domain evidence가 current source에 merge된 뒤 시작한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-06-query-completeness-continuation-and-provenance.md`,
P04 handoff.

목표: pagination, completeness, ranking, availability, explain이 실제 execution outcome과 동일한 의미를 갖게 한다.

owner files:

- `crates/quanta-index-contract-base/src/query_window.rs`
- contract cursor/result/explanation DTOs
- `crates/quanta-index-core/src/domains/{semantic,hybrid}/`
- `crates/quanta-index-search-plane/src/query_dispatcher/{dense_admission,window,hybrid,hybrid_seed,semantic_query}.rs`
- all cursor codecs/validators, explain route, searchctl/harness renderers

구현 요구:

- `ExecutionOutcomeV2`: Exact, LowerBound, CappedUnknown, InterruptedPartial, Approximate, Unsupported, Unavailable
- `QueryResultWindowV2`에 mandatory completeness/coverage/exhaustion proof
- pageable route: lexical/symbol/history/runtime/structural
- bounded top-k route: semantic/hybrid/hybrid-seed/RepoMap; pagination을 거짓 암시하지 않음
- continuation digest에 route/full pin/read identity/normalized query/constraints/order/cap/aux epochs/version/expiry 포함
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
- independent RRF/window oracle가 order/rank/score/next boundary를 재계산
- cursor의 repo/revision/route/query/order/cap/epoch/read identity/tamper/expiry/version negative matrix
- available-empty, filtered-empty, zero-hit executed, unavailable가 서로 다른 output/provenance
- explain mismatch는 typed fail-closed

최종 보고에 source freeze, route capability matrix, schema bump, oracle independence, commands/counts, NOT_RUN, P06 expected
response context inputs를 남겨라. commit/push는 요청 시에만 한다.
