# Lexical Capability Matrix

Status: `completed`
Owner ticket: [LXE-00](tickets/LXE-00-truth-freeze-and-executable-matrix.md)

## Phase status summary

- **Phase 0** — V1/V2 strip from ticket pack + matrix skeleton: DONE
- **Phase 1** — LXE-02 planner IR scaffold + LXE-09 structural domain + E2E-00 harness skeleton: DONE (10 tests pass)
- **Phase 2** — LXE-04 regex/trigram + LXE-05 phrase + LXE-06 symbol planner scaffolds: DONE (15 new tests pass)
- **Phase 3** — LXE-03 filter execution + LXE-05/06 planner-arm integration: DONE (12 new tests, total 31 lexical lib tests pass)
- **Phase 4** — execution-body wiring in `lexical/src/lib.rs::search` + search-plane integration of `LexicalPlanner::plan()`: DONE
- **Phase 5 (E2E proof)** — `E2E-01`, `E2E-02`, `E2E-03`, `E2E-04`, `E2E-05`, `E2E-06`, and `E2E-07`: DONE

## Current code-backed snapshot (2026-05-27)

- Green on live rails:
  - `LXE-01`: semantic/hybrid public request intake now uses the canonical
    text-based carriers, and deleted request fields are fail-closed by
    contract decode tests
  - `LXE-03`: repo/file/lang/case/count/select:file/select:repo/select:content and typed-unavailable producer-dependent filters
  - `LXE-04`: materialized trigram-prefilter + authoritative indexed-text exact verify, including SG/native `patterntype:regexp` parity
    and regex-backed `timeout:` fail-closed as `QUERY_TIMEOUT`
  - `LXE-05`: positions-backed exact-adjacent phrase hit and reversed-order phrase miss
  - `LXE-06`: text-route `type:file`/`select:file`/`select:path`/
    `select:content`/`select:content.match` plus text-route/public-frontdoor
    `type:symbol` and `select:symbol` symbol-doc routing, distinct
    `SymbolQueryResponse`, and public `symbol_kind` truth
  - `LXE-09`: structural live subset on materialized parse-tree/chunk
    authority (`match { :[x] }`, `match { function_item }`,
    variadic sibling capture / wildcard skip, and `where` / `inside` /
    `outside` constraints), plus the dedicated Sourcegraph structural subset
- Additional completed proof:
  - `E2E-04` history/structural is complete on the current tree; history
    positive and typed negative rows live in `sdk_frontdoor.rs` and
    `end_to_end.rs`, and structural SG/native parity rows live in
    `e2e_dual_syntax_lowering_parity.rs`
- Proof rails:
  - `cargo test -p quanta-index-contract --test lxe_unified_surface`
  - `cargo test -p quanta-index-contract --test ipc_query_result_v2_contract`
  - `cargo test -p quanta-index-lexical --test tantivy_smoke`
  - `cargo test -p quanta-index-core --test lexical_policy`
  - `cargo test -p quanta-index-search-plane`
  - `cargo test -p quanta-index-sdk --lib`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_lexical_full_fidelity --test e2e_dual_syntax_lowering_parity -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_perf_chaos -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test end_to_end structural_sourcegraph_query_ -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_restart_replay_determinism -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_matrix_inventory -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime`
  - `just rust-test-full-corpus`
- Closeout rerun refresh (2026-05-27):
  - `cargo check -p quanta-index-contract`: green
  - `cargo check -p quanta-index-sdk`: green
  - `cargo test -p quanta-index-searchd-runtime --test repo_map_end_to_end`: green
  - `cargo test -p quanta-index-sdk --lib`: green
  - `cargo test -p quanta-index-searchd-runtime`: green
  - this refresh re-proves current live-source closure for `E2E-01`,
    `E2E-02`, `E2E-03`, `E2E-04`, `E2E-05`, `E2E-06`, and `E2E-07`

Source-backed truth table for the LQ DSL surface, Sourcegraph syntax, planner
lowering, engine execution, response carriers, and proof coverage.

**Status legend** (one per row):

- `executed` — accepted, planned, executed end-to-end, asserted by an E2E row
- `typed-rejected` — accepted by parser, rejected with a typed code before
  execution (no silent drop)
- `executed[truthful-subset]` — live only for the explicitly documented subset;
  all other accepted shapes stay typed fail-closed
- `expected-failing[LXE-NN]` — wired through E2E harness as a failing row
  pending the named owner ticket
- `pending[LXE-NN]` — not yet exercised by any test; owner ticket carries it
- `dead` — present in source but unreachable; deletion candidate

**Evidence convention**: `file:line` link or owner-ticket id.

---

## 1. Request intake (single front door)

| Concern | Type | Evidence | Status |
| --- | --- | --- | --- |
| Public syntax selector | `TextQuerySyntax { Native, Sourcegraph }` | [contract-base/src/query/syntax.rs:8](../../../crates/quanta-index-contract-base/src/query/syntax.rs#L8) | `executed` |
| Lexical text request | `TextQueryRequest { syntax, query_text, generation, generation_selector, top_k }` | [contract-base/src/query/requests.rs:18](../../../crates/quanta-index-contract-base/src/query/requests.rs#L18) | `executed` |
| Semantic lexical scope field | `lexical_scope: Option<TextQueryRequest>` | [contract/tests/lxe_unified_surface.rs:72](../../../crates/quanta-index-contract/tests/lxe_unified_surface.rs#L72), [contract/tests/ipc_query_result_v2_contract.rs:376](../../../crates/quanta-index-contract/tests/ipc_query_result_v2_contract.rs#L376) | `executed` |
| Hybrid lexical field | `text_query: TextQueryRequest` | [contract/tests/lxe_unified_surface.rs:187](../../../crates/quanta-index-contract/tests/lxe_unified_surface.rs#L187), [contract/tests/ipc_query_result_v2_contract.rs:407](../../../crates/quanta-index-contract/tests/ipc_query_result_v2_contract.rs#L407) | `executed` |
| Direct `LqQuery` request field absent | reflection + public builder tests | [contract/tests/lxe_unified_surface.rs:117](../../../crates/quanta-index-contract/tests/lxe_unified_surface.rs#L117), [sdk/src/tests.rs:541](../../../crates/quanta-index-sdk/src/tests.rs#L541) | `executed` |
| Unknown-variant decode rejection | manual serde impls + legacy-field negative tests | [contract/tests/ipc_query_result_v2_contract.rs:591](../../../crates/quanta-index-contract/tests/ipc_query_result_v2_contract.rs#L591), [contract/tests/ipc_query_result_v2_contract.rs:610](../../../crates/quanta-index-contract/tests/ipc_query_result_v2_contract.rs#L610) | `executed` |

## 2. LQ leaves (`LqLeaf`)

Defined at [lq-norm/src/ast.rs:196](../../../crates/quanta-index-lq-norm/src/ast.rs#L196).

| Leaf | Parser | Lowering | Planner | Engine | Result carrier | Unit test | E2E | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `Keyword(String)` | lq-norm/parser | search-plane/lowering | [planner.rs::plan() `LqLeaf::Keyword` arm](../../../crates/quanta-index-lexical/src/planner.rs) → `PlanLeaf::Content` | live lexical text rail executes against indexed content | `LexicalCandidate` | `planner::tests::single_keyword_leaf_produces_content_plan` | E2E-01 content/path/case/count rows green | `executed` |
| `Phrase(String)` | lq-norm/parser | search-plane/lowering | [phrase.rs::plan_phrase](../../../crates/quanta-index-lexical/src/phrase.rs) → `PlanLeaf::Phrase { plan }` | live lexical rail executes through materialized `lq-positions` sidecars | `LexicalCandidate` | planner phrase tests + `tantivy_smoke` indexed-text/case proof | E2E-01 rows green | `executed` |
| `RawString(String)` | lq-norm/parser | search-plane/lowering | [trigram_plan.rs::plan_raw_substring](../../../crates/quanta-index-lexical/src/trigram_plan.rs) → `PlanLeaf::RawSubstring { plan }` | live lexical rail executes through materialized trigram sidecars + exact verify over authoritative indexed text | `LexicalCandidate` | `planner::tests::raw_substring_plans_ok_on_minimum_needle` + `raw_substring_too_short_is_rejected_typed` | E2E-01 raw-substring row green | `executed` |
| `Regex(String)` | lq-norm/parser + [regex_guard.rs](../../../crates/quanta-index-lq-norm/src/regex_guard.rs) | search-plane/lowering | [regex.rs::plan_regex](../../../crates/quanta-index-lexical/src/regex.rs) → `PlanLeaf::Regex { plan }` with `extract_required_literals` + `dialect_filter` | live lexical rail executes through materialized trigram sidecars + exact verify over authoritative indexed text | `LexicalCandidate` | `planner::tests::regex_leaf_with_extractable_literal_plans_ok` + `regex_leaf_with_lookbehind_is_rejected_typed` + `tantivy_smoke` authority proof | E2E-01 + E2E-07 rows green | `executed` |
| `StructuralBlock(LqStructuralBlock)` | lq-norm/parser | search-plane lowers structural-only boolean trees over `match { ... }` leaves plus executable `repo:` / `file:` / `lang:` filters | routes through `StructuralService` with candidate-scope pruning, bounded `NOT`, and deterministic projection | truthful authority subset over parse-tree/chunk authority | internal `StructuralMatchBinding` / `StructuralMatchCandidate` -> projected `StructuralBinding` / `StructuralCandidate` | `quanta-index-lq-structural` authority tests + search-plane structural tests | `sdk_frontdoor.rs`, `end_to_end.rs`, `e2e_dual_syntax_lowering_parity.rs` | `executed[truthful-subset]` — root-kind exact, root capture, root-kind plus capture, ordered child tree-walk, variadic sibling capture / wildcard skip, `where` / `inside` / `outside`, structural-only boolean composition, and typed holes `expr|stmt|item|type`; filters outside `repo:` / `file:` / `lang:` stay typed fail-closed |
| `Predicate { name, args }` | lq-norm/parser | committed predicate lowering (`lower_bridge_predicate` in search-plane + core/lexical) converts supported predicate forms into filters/plans before reaching the planner | `repo.has.file(path:...)` lowers to repo-gate execution on the lexical rail; `symbol.has.name(...)` lowers to `symbol::plan_symbol` → `PlanLeaf::Symbol { plan }`; unsupported names/arg shapes return `LexicalPlannerError::Unimplemented { owner_ticket: "LXE-03-predicate-extensions" }` | per-predicate route | varies | `planner::tests::predicate_symbol_has_name_plans_through_symbol_route` + lexical/runtime predicate proofs | E2E-01 + E2E-02 predicate rows green | `executed[truthful-subset]` |

## 3. LQ boolean tree (`LqExpr`)

Defined at [lq-norm/src/ast.rs:306](../../../crates/quanta-index-lq-norm/src/ast.rs#L306).

| Node | Status | Owner |
| --- | --- | --- |
| `Empty` | `executed` (no-op identity) | — |
| `Leaf(LqLeaf)` | per-leaf row above | — |
| `Not(Box<LqExpr>)` | `executed[truthful-subset]` | live for the lexical subset admitted by planner/executor; unsupported predicate extensions stay typed-rejected |
| `All(Vec<LqExpr>)` | `executed[truthful-subset]` | same truthful lexical subset |
| `Any(Vec<LqExpr>)` | `executed[truthful-subset]` | same truthful lexical subset |
| retired semantic-vector leaf | removed from the canonical query AST; semantic/hybrid entry is now text-only and query embedding is search-owned before semantic execution | historical only |

## 4. LQ filters (`LqFilter`)

Defined at [lq-norm/src/ast.rs:223](../../../crates/quanta-index-lq-norm/src/ast.rs#L223). Current execution is split between live text-rail filters, doc-kind routing for `type` / `select`, and typed fail-closed surfaces for producer-dependent or cross-domain shapes.

| Filter | Field ownership (per LXE-03) | Current behavior | Status |
| --- | --- | --- | --- |
| `Repo { pattern, revs }` | pre-candidate constraint | live repo match on lexical rail; nested `revs` typed fail-closed to history-required | `executed[truthful-subset]` |
| `File { pattern, scope }` | pre-candidate path constraint | live path/name constraint on lexical text rail | `executed` |
| `Lang { id }` | pre-candidate metadata constraint | live language metadata constraint on lexical text rail | `executed` |
| `Rev { spec }` | history-domain constraint | typed fail-closed (`REV_UNAVAILABLE`) on lexical rail | `typed-rejected` |
| `Type { kind }` | route selector → content/path/symbol/commit/diff/structural | doc-kind routing: `file/path -> text`, `symbol -> symbol route with distinct public `SymbolQueryResponse`, `commit/diff/repo -> typed unavailable on lexical rail`; public runtime witness exists for native/Sourcegraph `type:symbol` on the symbol frontdoor with `symbol_kind` truth | `executed[truthful-subset]` |
| `Select { dim }` | result-surface selector | live repo/file/path/content/content.match projection on the text rail plus native/Sourcegraph `select:symbol` on the symbol frontdoor with `SymbolCandidate` | `executed` |
| `Fork { mode }` | producer-dependent | executes when typed repo metadata bundle is present; otherwise typed fail-closed / not-ready | `executed[truthful-subset]` |
| `Archived { mode }` | producer-dependent | executes when typed repo metadata bundle is present; otherwise typed fail-closed / not-ready | `executed[truthful-subset]` |
| `Visibility { mode }` | producer-dependent | executes when typed repo metadata bundle is present; otherwise typed fail-closed / not-ready | `executed[truthful-subset]` |
| `Context { name }` | producer-dependent | executes when typed repo metadata bundle is present; otherwise typed fail-closed / not-ready | `executed[truthful-subset]` |
| `Content { leaf }` | content engine | delegates to the active lexical leaf execution path, including public `select:content.match` proof on the text frontdoor | `executed` |

`type:` enum values from `LqType`: `File`, `Path`, `Symbol`, `Commit`, `Diff`, `Repo`. `File`/`Path`/`Symbol` have live route proof here. `Commit`/`Diff` execute on the dedicated history route with runtime proof under `LXE-08`, and `Repo` stays typed unavailable on the lexical rail.

`select:` enum values from `LqSelect`: `Repo`, `File`, `Path`, `Symbol`, `Content`, `ContentMatch`. All six have live proof on the current tree; `Symbol` uses the distinct public symbol carrier and the rest project on the text rail.

## 5. LQ options (`LqOptions`)

Defined at [lq-norm/src/ast.rs:333](../../../crates/quanta-index-lq-norm/src/ast.rs#L333).

| Option | Values | Status |
| --- | --- | --- |
| `pattern_type` | `Literal`, `Keyword`, `Standard`, `Regexp`, `Structural` | `executed[truthful-subset]` (literal/keyword/standard/regexp live on the lexical rail; structural executes on the dedicated structural route and is typed-rejected on the lexical SG route) |
| `case` | `Sensitive`, `Insensitive` | `executed` |
| `count` | `Bounded(u32)`, `All` | `executed` |
| `timeout_ms` | `Option<u64>` | `executed[truthful-subset]` (native/Sourcegraph lexical surfaces lower `timeout:` into canonical options for regex-backed execution; structural/history/runtime keep timeout typed fail-closed) |

## 6. LQ directives (`LqDirective`)

Defined at [lq-norm/src/ast.rs:322](../../../crates/quanta-index-lq-norm/src/ast.rs#L322).

| Directive | Status |
| --- | --- |
| `IntoCodeQl` | `executed` (bridge packet export from executed candidates is live, and the closed-label metrics surface is now proved on the same current tree) |
| `ScopeResults` | `executed` (canonical default) |
| `WithLexical` | `executed` (canonical default) |

## 7. Sourcegraph syntax (`lq-bridge`)

Translator at [lq-bridge/src/translator.rs](../../../crates/quanta-index-lq-bridge/src/translator.rs). In-flight changes add `path:` filter, `/ ... /` regex kind, predicate directives.

| SG syntax | Translated to | Status |
| --- | --- | --- |
| `repo:<pattern>` | `LqFilter::Repo` | `executed` |
| `file:<pattern>` | `LqFilter::File` | `executed` |
| `path:<pattern>` | `LqFilter::File { scope: PathOnly }` | `executed` |
| `lang:<id>` | `LqFilter::Lang` | `executed` |
| `case:yes/no` | `LqCase` | `executed` |
| `count:N` | `LqCountBound::Bounded` | `executed` |
| `count:all` | `LqCountBound::All` | `executed` |
| `timeout:<duration>` | `LqOptions.timeout_ms` | `executed[truthful-subset]` (native/Sourcegraph lexical regex-backed execution only; unsupported routes stay typed fail-closed) |
| `type:file` / `type:symbol` / `type:commit` / `type:diff` | `LqFilter::Type` | `executed[truthful-subset]` (file live on text rail; symbol-doc routing now has native/Sourcegraph text-route + public symbol-frontdoor proof with distinct carrier and `symbol_kind` truth; commit/diff live on the dedicated history route with runtime proof) |
| `select:file` / `select:content` / `select:symbol` | `LqFilter::Select` | `executed[truthful-subset]` (file/content live on text rail; symbol-doc routing now has native/Sourcegraph text-route + public symbol-frontdoor proof with distinct carrier and `symbol_kind` truth) |
| `patterntype:literal` | `LqPatternType::Literal` + `LqLeaf::RawString` | `executed` |
| `patterntype:regexp` | `LqPatternType::Regexp` + `LqLeaf::Regex` | `executed` |
| Boolean `or` | `LqExpr::Any` | `executed[truthful-subset]` |
| Boolean `and` (implicit) | `LqExpr::All` | `executed[truthful-subset]` |
| Negation `-term` / `NOT` | `LqExpr::Not` | `executed[truthful-subset]` |
| Unsupported SG feature | typed translator/bridge error | `executed[truthful-subset]` (`BRIDGE_UNSUPPORTED_FILTER` / `BRIDGE_UNSUPPORTED_DIRECTIVE` are live; broader engine/prod-data taxonomy is tracked elsewhere) |

## 8. Result carriers (`results::*`)

Most carrier types currently live in
`quanta-index-contract-base/src/results/`; `CommitCandidate` still lives in
`quanta-index-contract/src/results/commit_candidate.rs`.

| Carrier | Location | Status |
| --- | --- | --- |
| `LexicalCandidate` | [contract-base/src/results/candidates.rs](../../../crates/quanta-index-contract-base/src/results/candidates.rs) | `executed` |
| `SymbolCandidate` | [contract/src/results/query_responses.rs](../../../crates/quanta-index-contract/src/results/query_responses.rs) | `executed` |
| `CommitCandidate` | [contract/src/results/commit_candidate.rs](../../../crates/quanta-index-contract/src/results/commit_candidate.rs) | `executed` |
| `DiffCandidate` (+ `DiffHunkSide`) | [contract-base/src/results/diff_candidate.rs](../../../crates/quanta-index-contract-base/src/results/diff_candidate.rs) | `executed` |
| `StructuralBinding` | [contract-base/src/results/structural.rs](../../../crates/quanta-index-contract-base/src/results/structural.rs#L10) | `executed` |
| `StructuralCandidate` | [contract-base/src/results/structural.rs](../../../crates/quanta-index-contract-base/src/results/structural.rs#L93) | `executed` |
| `BridgeTarget` | contract bridge DTO surface (worktree-local file move in progress) | `executed` (CodeQl only) |
| `BridgeCandidatePacket` | bridge packet carrier surface (worktree-local file move in progress) | `executed` |

Internal structural match carriers are intentionally not public result DTOs:
`quanta-index-core` / `searchd` execute on `StructuralMatchBinding` /
`StructuralMatchCandidate`, and `search-plane` projects them into
`StructuralBinding` / `StructuralCandidate` only at the response boundary.

## 9. `SearchExplanation`

Current shape at [contract/src/results/explanation.rs](../../../crates/quanta-index-contract/src/results/explanation.rs) carries `planner_trace`, `engines_touched`, `early_stop_reason`, `summary`, `contributions`, `ranker_weights_hash`, and `strategy`.

| Field | Status |
| --- | --- |
| `contributions` | `executed` |
| `ranker_weights_hash` | `executed` |
| `strategy` | `executed` |
| `planner_trace` | `executed` |
| `engines_touched` | `executed` |
| `early_stop_reason` | `executed[truthful-subset]` (`CountReached` is proved on bounded hybrid execution; non-early-stop paths remain `None`) |
| `summary` | `executed` |

## 10. Fail-closed surfaces

| Surface | Typed code | Evidence | Status |
| --- | --- | --- | --- |
| History generation not materialized | `HISTORY_GENERATION_NOT_READY` | [query_dispatcher.rs:823](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs#L823), [end_to_end.rs:632](../../../crates/quanta-index-searchd-runtime/tests/end_to_end.rs#L632) | `executed` |
| History producer-specific unavailable taxonomy | `HISTORY_PRODUCER_UNAVAILABLE`, `HISTORY_GENERATION_NOT_READY` | [query_dispatcher.rs:815](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs#L815), [sdk_frontdoor.rs:1606](../../../crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs#L1606), [end_to_end.rs:675](../../../crates/quanta-index-searchd-runtime/tests/end_to_end.rs#L675) | `executed` |
| History shard unavailable | `HISTORY_SHARD_UNAVAILABLE` | [query_dispatcher.rs:888](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs#L888), [sdk_frontdoor.rs:1606](../../../crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs#L1606), [end_to_end.rs:742](../../../crates/quanta-index-searchd-runtime/tests/end_to_end.rs#L742) | `executed[truthful-subset]` — proven for required shard materialization absence; deeper authority-corruption taxonomy is not yet modeled |
| Lexical regex timeout | `QUERY_TIMEOUT` | `e2e_perf_chaos.rs` timeout row + search-plane closed-metric taxonomy proof | `executed[truthful-subset]` — regex-backed lexical timeout is live; non-executable routes reject timeout before execution |
| Structural unsupported lang | `STR_LANG_NOT_SUPPORTED` | [sdk_frontdoor.rs:656](../../../crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs#L656) | `executed` |
| Structural generation not ready | `STR_GENERATION_NOT_READY` | [end_to_end.rs:2239](../../../crates/quanta-index-searchd-runtime/tests/end_to_end.rs#L2239) | `executed` |
| Structural shard unavailable | `STR_SHARD_UNAVAILABLE` | [end_to_end.rs:2305](../../../crates/quanta-index-searchd-runtime/tests/end_to_end.rs#L2305) | `executed` |
| Structural invalid request | `STR_INVALID_REQUEST` | [query_dispatcher.rs:816](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs#L816), [sdk_frontdoor.rs:669](../../../crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs#L669) | `executed` |

## 11. E2E harness

| Component | Location | Status |
| --- | --- | --- |
| Tempdir runtime harness | `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs` | `executed` |
| Corpus fixtures | `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/` | `executed[E2E-06]` |
| Matrix inventory test | `crates/quanta-index-searchd-runtime/tests/e2e_matrix_inventory.rs` | `executed` |
| Lexical full-fidelity | `tests/e2e_lexical_full_fidelity.rs` | `executed[E2E-01]` |
| Sourcegraph parity | `tests/e2e_dual_syntax_lowering_parity.rs` | `executed[E2E-02]` |
| Semantic/hybrid | existing `tests/end_to_end.rs` + `tests/dsl_scenarios.rs` + `tests/sdk_frontdoor.rs` | `executed[E2E-03]` |
| History/structural | existing `tests/sdk_frontdoor.rs` + `tests/end_to_end.rs` plus structural parity rows in `tests/e2e_dual_syntax_lowering_parity.rs` | `executed[E2E-04]` |
| Restart/replay | `tests/e2e_restart_replay_determinism.rs` | `executed[E2E-05]` |
| Full corpus rail | `tests/e2e_full_corpus.rs` + `tests/fixtures/lexical_corpus/` | `executed[E2E-06]` |
| Perf/chaos | `tests/e2e_perf_chaos.rs` | `executed[E2E-07]` |

The full-corpus fixture is now route-aware: rows declare `runtime_route =
text|structural|history`, and the shared fixture can carry repo metadata,
structural parse trees, and history shards. Parser-only / deferred rows are no
longer parked in `runtime_rows.toml`.

## 12. Risk register

- **Predicate lowering in-flight**: `lower_bridge_predicate` is currently being
  added to [search-plane/lowering.rs](../../../crates/quanta-index-search-plane/src/lowering.rs)
  and to the lexical lowering surface in `quanta-index-core/src/domains/lexical/lowering.rs` (historical path; removed in the current tree).
  LXE-03 must integrate, not replace. **Open question**: keep the duplicated
  predicate-parsing logic, or lift it into a single helper in lq-norm before
  LXE-03 lands?
- **Regex observability**: regex now routes through materialized trigram +
  exact verify. Remaining risk is missing explain-grade prefilter counts and
  engine trace, not query-string escape.
- **Retired semantic-vector lexical row**: this packet no longer treats
  `SemanticVector` as a live lexical capability row because the AST/public
  query surface is text-only; semantic execution remains owned by semantic and
  hybrid routes after search-owned query embedding.
- **Type/select regression surface**: `type:symbol` / `select:symbol` keep a
  distinct public carrier plus `symbol_kind` truth, history-owned
  `type:commit` / `type:diff` keep public/runtime proof, and
  `select:path` / `select:content.match` now have separate native/Sourcegraph
  frontdoor proof.
- **Structural live surface is still narrower than full public semantics**:
  the current authority-owned matcher now executes root-anchored tree-walk,
  variadic sibling capture / wildcard skip, and `where` / `inside` /
  `outside`, but structural boolean composition and broader Sourcegraph
  structural forms remain explicitly typed fail-closed.
