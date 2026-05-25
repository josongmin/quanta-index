# Lexical Capability Matrix

Status: `proposed`
Owner ticket: [LXE-00](tickets/LXE-00-truth-freeze-and-executable-matrix.md)

Source-backed truth table for the LQ DSL surface, Sourcegraph syntax, planner
lowering, engine execution, response carriers, and proof coverage.

**Status legend** (one per row):

- `executed` — accepted, planned, executed end-to-end, asserted by an E2E row
- `typed-rejected` — accepted by parser, rejected with a typed code before
  execution (no silent drop)
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
| Semantic lexical scope field | `lexical_scope: Option<TextQueryRequest>` | [LXE-01](tickets/LXE-01-active-contract-and-dead-route-cleanup.md) | `pending[LXE-01]` |
| Hybrid lexical field | `lexical: TextQueryRequest` | [LXE-01](tickets/LXE-01-active-contract-and-dead-route-cleanup.md) | `pending[LXE-01]` |
| Direct `LqQuery` request field absent | reflection test | [LXE-01](tickets/LXE-01-active-contract-and-dead-route-cleanup.md) | `pending[LXE-01]` |
| Unknown-variant decode rejection | manual serde impls in [lq-norm/src/ast.rs:391+](../../../crates/quanta-index-lq-norm/src/ast.rs#L391) | [LXE-01](tickets/LXE-01-active-contract-and-dead-route-cleanup.md) | `pending[LXE-01]` |

## 2. LQ leaves (`LqLeaf`)

Defined at [lq-norm/src/ast.rs:196](../../../crates/quanta-index-lq-norm/src/ast.rs#L196).

| Leaf | Parser | Lowering | Planner | Engine | Result carrier | Unit test | E2E | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `Keyword(String)` | lq-norm/parser | search-plane/lowering | TODO planner | lexical/Tantivy term | `LexicalCandidate` | TODO | E2E-01 row | `pending[LXE-02]` |
| `Phrase(String)` | lq-norm/parser | search-plane/lowering | TODO planner | lq-positions | `LexicalCandidate` | TODO | E2E-01 row | `pending[LXE-05]` |
| `RawString(String)` | lq-norm/parser | search-plane/lowering | TODO planner | lq-trigram + verify | `LexicalCandidate` | TODO | E2E-01 row | `pending[LXE-04]` |
| `Regex(String)` | lq-norm/parser + [regex_guard.rs](../../../crates/quanta-index-lq-norm/src/regex_guard.rs) | currently bypasses through Tantivy at [lexical/src/lib.rs:1151](../../../crates/quanta-index-lexical/src/lib.rs#L1151) | TODO planner | lq-regex verify + lq-trigram prefilter | `LexicalCandidate` | [lq-regex/src/](../../../crates/quanta-index-lq-regex/src/) | E2E-01 + E2E-07 rows | `expected-failing[LXE-04]` |
| `StructuralBlock(LqStructuralBlock)` | lq-norm/parser | TODO | TODO planner | lq-structural | `StructuralBinding`/`StructuralCandidate` | [lq-structural](../../../crates/quanta-index-lq-structural/src/) | E2E-04 | `typed-rejected` → `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` at [search-plane/query_dispatcher.rs:235](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs#L235); `pending[LXE-09]` for fixture path |
| `Predicate { name, args }` | lq-norm/parser | in-flight at search-plane/lowering + core/lexical/lowering (`lower_bridge_predicate`) | TODO planner | per-predicate route | varies | TODO | E2E-01 / E2E-02 rows | `pending[LXE-03]` (integrate with in-flight predicate lowering) |

## 3. LQ boolean tree (`LqExpr`)

Defined at [lq-norm/src/ast.rs:306](../../../crates/quanta-index-lq-norm/src/ast.rs#L306).

| Node | Status | Owner |
| --- | --- | --- |
| `Empty` | `executed` (no-op identity) | — |
| `Leaf(LqLeaf)` | per-leaf row above | — |
| `Not(Box<LqExpr>)` | `pending[LXE-02]` | LXE-02 planner IR |
| `All(Vec<LqExpr>)` | `pending[LXE-02]` | LXE-02 |
| `Any(Vec<LqExpr>)` | `pending[LXE-02]` | LXE-02 |
| `SemanticVector { vector_ref, top_k }` | currently `NotImplemented` at [lexical/src/lib.rs:1130](../../../crates/quanta-index-lexical/src/lib.rs#L1130) (lexical adapter); routed via semantic engine | `pending[LXE-07]` |

## 4. LQ filters (`LqFilter`)

Defined at [lq-norm/src/ast.rs:223](../../../crates/quanta-index-lq-norm/src/ast.rs#L223). Currently several filters return `NotImplemented` at [lexical/src/lib.rs:1201-1204](../../../crates/quanta-index-lexical/src/lib.rs#L1201).

| Filter | Field ownership (per LXE-03) | Current behavior | Status |
| --- | --- | --- | --- |
| `Repo { pattern, revs }` | pre-candidate constraint | in-flight via `lower_bridge_predicate` repo branch | `pending[LXE-03]` |
| `File { pattern, scope }` | pre-candidate path constraint | in-flight: file metadata + path field indexing being added | `pending[LXE-03]` |
| `Lang { id }` | pre-candidate metadata constraint | in-flight: language field being added to Tantivy schema | `pending[LXE-03]` |
| `Rev { spec }` | history-domain constraint | `NotImplemented` at [lexical/src/lib.rs:1201](../../../crates/quanta-index-lexical/src/lib.rs#L1201) | `pending[LXE-03]` / `typed-rejected` until history fixture exists |
| `Type { kind }` | route selector → content/path/symbol/commit/diff/structural | `NotImplemented` | `pending[LXE-06]` |
| `Select { dim }` | result-surface selector | `NotImplemented` | `pending[LXE-06]` |
| `Fork { mode }` | producer-dependent | `typed-rejected` (producer metadata absent) | `pending[LXE-03]` |
| `Archived { mode }` | producer-dependent | `typed-rejected` | `pending[LXE-03]` |
| `Visibility { mode }` | producer-dependent | `typed-rejected` | `pending[LXE-03]` |
| `Context { name }` | producer-dependent | `typed-rejected` | `pending[LXE-03]` |
| `Content { leaf }` | content engine | partially executed (delegates to leaf) | `pending[LXE-03]` |

`type:` enum values from `LqType`: `File`, `Path`, `Symbol`, `Commit`, `Diff`, `Repo`. `Commit`/`Diff` route to history (`pending[LXE-08]`). `Repo` selects per-repo aggregation (`pending[LXE-06]`).

`select:` enum values from `LqSelect`: `Repo`, `File`, `Path`, `Symbol`, `Content`, `ContentMatch`. Per-surface routing owned by LXE-06.

## 5. LQ options (`LqOptions`)

Defined at [lq-norm/src/ast.rs:333](../../../crates/quanta-index-lq-norm/src/ast.rs#L333).

| Option | Values | Status |
| --- | --- | --- |
| `pattern_type` | `Literal`, `Keyword`, `Standard`, `Regexp`, `Structural` | `pending[LXE-04]` (regexp), `pending[LXE-09]` (structural), others `pending[LXE-03]` |
| `case` | `Sensitive`, `Insensitive` | `pending[LXE-03]` (must affect content/regex/phrase matching) |
| `count` | `Bounded(u32)`, `All` | `pending[LXE-03]` (result cap after deterministic merge) |

## 6. LQ directives (`LqDirective`)

Defined at [lq-norm/src/ast.rs:322](../../../crates/quanta-index-lq-norm/src/ast.rs#L322).

| Directive | Status |
| --- | --- |
| `IntoCodeQl` | `pending[LXE-10]` (bridge packet export from executed candidates) |
| `ScopeResults` | `executed` (canonical default) |
| `WithLexical` | `executed` (canonical default) |

## 7. Sourcegraph syntax (`lq-bridge`)

Translator at [lq-bridge/src/translator.rs](../../../crates/quanta-index-lq-bridge/src/translator.rs). In-flight changes add `path:` filter, `/ ... /` regex kind, predicate directives.

| SG syntax | Translated to | Status |
| --- | --- | --- |
| `repo:<pattern>` | `LqFilter::Repo` | `pending[LXE-03]` parity with LQ repo |
| `file:<pattern>` | `LqFilter::File` | `pending[LXE-03]` parity |
| `path:<pattern>` | `LqFilter::File { scope: PathOnly }` | in-flight; `pending[LXE-03]` |
| `lang:<id>` | `LqFilter::Lang` | `pending[LXE-03]` |
| `case:yes/no` | `LqCase` | `pending[LXE-03]` |
| `count:N` | `LqCountBound::Bounded` | `pending[LXE-03]` |
| `count:all` | `LqCountBound::All` | `pending[LXE-03]` |
| `type:file` / `type:symbol` / `type:commit` / `type:diff` | `LqFilter::Type` | `pending[LXE-06]` (file/symbol), `pending[LXE-08]` (commit/diff) |
| `select:file` / `select:content` / `select:symbol` | `LqFilter::Select` | `pending[LXE-06]` |
| `patterntype:literal` | `LqPatternType::Literal` + `LqLeaf::RawString` | `pending[LXE-04]` (raw substring) |
| `patterntype:regexp` | `LqPatternType::Regexp` + `LqLeaf::Regex` | `pending[LXE-04]` |
| Boolean `or` | `LqExpr::Any` | `pending[LXE-02]` |
| Boolean `and` (implicit) | `LqExpr::All` | `pending[LXE-02]` |
| Negation `-term` / `NOT` | `LqExpr::Not` | `pending[LXE-02]` |
| Unsupported SG feature | typed translator/bridge error | `pending[LXE-10]` taxonomy |

## 8. Result carriers (`results::*`)

All carrier types currently live in `quanta-index-contract-base/src/results/`; the `quanta-index-contract` results module re-exports.

| Carrier | Location | Status |
| --- | --- | --- |
| `LexicalCandidate` | [contract-base/src/results/candidates.rs](../../../crates/quanta-index-contract-base/src/results/candidates.rs) | `executed` |
| `SymbolCandidate` | TBD (LXE-06) | `pending[LXE-06]` |
| `CommitCandidate` | TBD (LXE-08) | `pending[LXE-08]` |
| `DiffCandidate` (+ `DiffHunkSide`) | [contract-base/src/results/diff_candidate.rs](../../../crates/quanta-index-contract-base/src/results/diff_candidate.rs) | `pending[LXE-08]` wiring |
| `StructuralBinding` | [contract-base/src/results/structural.rs](../../../crates/quanta-index-contract-base/src/results/structural.rs) | `pending[LXE-09]` wiring |
| `StructuralCandidate` | TBD (LXE-09) | `pending[LXE-09]` |
| `BridgeTarget` | [contract-base/src/results/bridge.rs](../../../crates/quanta-index-contract-base/src/results/bridge.rs) | `executed` (CodeQl only) |
| `BridgeCandidatePacket` | [lq-bridge/src/packet.rs](../../../crates/quanta-index-lq-bridge/src/packet.rs) | `executed`; LXE-10 hardens stable contract |

## 9. `SearchExplanation`

Current shape at [contract/src/lex/explanation.rs](../../../crates/quanta-index-contract/src/lex/explanation.rs) carries `contributions`, `ranker_weights_hash`, `strategy`. Augmentation pending in LXE-01.

| Field | Status |
| --- | --- |
| `contributions` | `executed` |
| `ranker_weights_hash` | `executed` |
| `strategy` | `executed` |
| `planner_trace` | `pending[LXE-01+LXE-02]` |
| `engines_touched` | `pending[LXE-01]` |
| `early_stop_reason` | `pending[LXE-01+LXE-10]` |
| `summary` | `pending[LXE-01]` |

## 10. Fail-closed surfaces

| Surface | Typed code | Evidence | Status |
| --- | --- | --- | --- |
| History request without producer | `HISTORY_PRODUCER_UNAVAILABLE` | [search-plane/query_dispatcher.rs:223](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs#L223) | `executed`; `pending[LXE-08]` for live positive path |
| History generation not ready | `HISTORY_GENERATION_NOT_READY` | LXE-08 | `pending[LXE-08]` |
| History shard unavailable | `HISTORY_SHARD_UNAVAILABLE` | LXE-08 | `pending[LXE-08]` |
| Structural request without parse-tree | `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` | [search-plane/query_dispatcher.rs:235](../../../crates/quanta-index-search-plane/src/query_dispatcher.rs#L235) | `executed`; LXE-09 enforces no text fallback |

## 11. E2E harness

| Component | Location | Status |
| --- | --- | --- |
| Tempdir runtime harness | new `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs` | `pending[E2E-00]` (skeleton landed in Phase 0e) |
| Corpus fixtures | new `crates/quanta-index-searchd-runtime/tests/common/e2e_corpus.rs` | `pending[E2E-00]` |
| Matrix inventory test | new `crates/quanta-index-searchd-runtime/tests/e2e_matrix_inventory.rs` | `pending[LXE-00]` |
| Lexical full-fidelity | new `tests/e2e_lexical_full_fidelity.rs` | `pending[E2E-01]` |
| Sourcegraph parity | new `tests/e2e_sourcegraph_parity.rs` | `pending[E2E-02]` |
| Semantic/hybrid | new `tests/e2e_semantic_hybrid.rs` | `pending[E2E-03]` |
| History/structural | new `tests/e2e_history_structural.rs` | `pending[E2E-04]` |
| Restart/replay | new `tests/e2e_restart_replay_determinism.rs` | `pending[E2E-05]` |
| Full corpus rail | new `tests/e2e_full_corpus.rs` + `tests/fixtures/lexical_corpus/` | `pending[E2E-06]` |
| Perf/chaos | new `tests/e2e_perf_chaos.rs` | `pending[E2E-07]` |

## 12. Risk register

- **Predicate lowering in-flight**: `lower_bridge_predicate` is currently being
  added to both [search-plane/lowering.rs](../../../crates/quanta-index-search-plane/src/lowering.rs)
  and [core/domains/lexical/lowering.rs](../../../crates/quanta-index-core/src/domains/lexical/lowering.rs).
  LXE-03 must integrate, not replace. **Open question**: keep the duplicated
  predicate-parsing logic, or lift it into a single helper in lq-norm before
  LXE-03 lands?
- **Regex escape path**: [lexical/src/lib.rs:1151](../../../crates/quanta-index-lexical/src/lib.rs#L1151)
  currently builds a Tantivy regex query directly. LXE-04 must remove this and
  route through `lq-trigram` + `lq-regex` with explicit candidate caps.
- **`SemanticVector` lexical adapter NotImplemented**: [lexical/src/lib.rs:1130](../../../crates/quanta-index-lexical/src/lib.rs#L1130)
  — correct behavior (lexical adapter shouldn't execute vectors) but LXE-07
  must confirm the routing to semantic engine is wired, not silently dropped.
- **`Type`/`Select`/`Rev` filter NotImplemented**: [lexical/src/lib.rs:1201-1204](../../../crates/quanta-index-lexical/src/lib.rs#L1201)
  — LXE-06 (type/select) and LXE-08 (rev/commit/diff) must turn these into
  either executed paths or typed-rejected codes; no silent drop allowed.
- **Structural domain missing in core**: [crates/quanta-index-core/src/domains/](../../../crates/quanta-index-core/src/domains/)
  has no `structural/` directory. LXE-09 must create it.
