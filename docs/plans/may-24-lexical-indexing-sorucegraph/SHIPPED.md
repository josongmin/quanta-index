# LQ Family Search Plane — Wave 0–8 Shipped

> Authoritative "where we landed" summary for the May-24 Sourcegraph-class Lexical Kernel program.
> Companion doc to [implementation-plan.md](implementation-plan.md) (per-wave gates), [tickets/INDEX.md](tickets/INDEX.md) (RFC reconciliation), [rfc.md](rfc.md) (amended), [feature-scope.md](feature-scope.md), [usecase.md](usecase.md), [dsl.md](dsl.md).
> Parent SSOTs: [../../ssot/channel-architecture.md](../../ssot/channel-architecture.md), [../../ssot/producer-handoff.md](../../ssot/producer-handoff.md).
> Posture: terse program-completion record. Findings, not narrative.

---

## §1. Program identity

| Field | Value |
|---|---|
| Program | LQ family search plane — Wave 0–8 |
| Ship date | 2026-05-25 |
| Crates shipped | 17 (Wave-0 prerequisites + Wave 1–8 ticket crates) |
| Tests passing | 1,072 across the 17 crates |
| Tests failed | 0 |
| Clippy `-D warnings` | 0 (per-crate) |
| Semgrep `tools/ci/semgrep/rules.yml` | 0 findings (all 17 crates) |
| `cargo fmt --check` | green (per-crate) |
| `cargo check` | green (per-crate) |
| Workspace baseline | **green** — `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all pass as of 2026-05-25 |
| RFC ticket roll-up parity | 14 / 14 active spec-sheet ticket scopes shipped; 5 RFC-only roll-up specs deferred (RFC-LEX-02, RFC-LEX-03, RFC-LEX-05, RFC-SEM-02-original, RFC-BRIDGE-01-CodeQL) — see [§3.3](#33-rfc-roll-up-specs-deferred) |
| Spec amendments | 4 spec sheets architecture-corrected (LEX-05 / LEX-07 / STR-01 / RT-01); RFC §Execution Model + §Error Code Taxonomy amended; SSOT updated; new [producer-handoff.md](../../ssot/producer-handoff.md) authored |
| Channel ops proposed | 9 (`UpsertCommit`, `UpsertRef`, `UpsertTag`, `DeleteRef`, `DeleteTag`, `UpsertDirty`, `EvictDirty`, `UpsertParseTree`, `DeleteParseTree`) pending producer agreement per [producer-handoff.md §8](../../ssot/producer-handoff.md) |
| Outstanding integration roadmap items | producer / deployment cutover only (see [§7](#7-integration-roadmap-next)) |

Cross-links: [channel-architecture.md](../../ssot/channel-architecture.md) (parent SSOT, updated 2026-05-25), [producer-handoff.md](../../ssot/producer-handoff.md) (new 2026-05-25), [tickets/INDEX.md](tickets/INDEX.md) (reconciliation), [implementation-plan.md](implementation-plan.md) (per-wave gates).

---

## §2. Crate inventory

All 17 crates ship under [`crates/`](../../../crates/). Test counts are aggregate per-crate `cargo test -p <crate>` runs (unit + integration + proptest cases). Key exported types follow each crate's `lib.rs`.

| # | Crate | Wave | Ticket | Tests | Key types / traits | Notable design locks |
|---|---|---|---|---|---|---|
| 1 | [`quanta-index-contract`](../../../crates/quanta-index-contract/) (`lex::*` subtree) | 0 | PRE-CONTRACT-EXT | 43 | `LexicalErrorCode` (29+ variants), `SymbolKind`, `CommitCandidate`, `DiffCandidate`, `StructuralCandidate`, `BridgeCandidatePacket`, `SearchExplanation` v2, `LqQuery.lq_version`, `LexicalChannelOp` | hand-rolled serde per D18; downstream per-crate `From<ErrorCode>` migration still pending |
| 2 | [`quanta-index-lq-norm`](../../../crates/quanta-index-lq-norm/) | 0 | PRE-NORM | 75 | `LqParser`, `LqCanonical::normalize`, `LqCanonicalHashV1` | recursive-descent parser; CBOR canonical encoding; idempotency proptest 1k cases; 5 originally-deferred items completed |
| 3 | [`quanta-index-conformance`](../../../crates/quanta-index-conformance/) | 0 | PRE-CONF | 32 | `ConformanceRunner::run_one`, `CorpusRow`, `ConformanceVerdict` | TOML corpus format; CLI shim landed; `StubLqEngine` fixture |
| 4 | [`quanta-index-lq-text-norm`](../../../crates/quanta-index-lq-text-norm/) | 1 | LEX-00 | 57 | `LexicalNormalizer`, identifier splitter | NFC + NFKC active; v1 ship-langs = Rust/Python/TS/JS/Go; `patterntype:literal` skips splitter |
| 5 | [`quanta-index-lq-scorer`](../../../crates/quanta-index-lq-scorer/) | 1 | LEX-01 | 43 | `LexicalScorer`, `LexicalScorerManifestPort` | per-generation `idf_table.cbor` + `scorer_manifest.cbor`; `f32 ∈ [0.0, 1.0]` score envelope; BM25 `(k1, b)` pinned (ADR-010 closed) |
| 6 | [`quanta-index-lq-trigram`](../../../crates/quanta-index-lq-trigram/) | 2 | LEX-02 | 56 | byte-trigram index, `memmem::find` verify path | byte trigrams (not code-point); caps 4k trigrams/query, 100k candidates pre-verify; pure-wildcard regex → verify-only |
| 7 | [`quanta-index-lq-positions`](../../../crates/quanta-index-lq-positions/) | 2 | LEX-03 | 72 | phrase position index, adjacency | stopword filter locked OFF forever; 8-token adjacency default; breaking-first `add_token` `Result` for 4 `PLAN_LIMIT_*` caps |
| 8 | [`quanta-index-lq-regex`](../../../crates/quanta-index-lq-regex/) | 3 | LEX-04 | 81 | RE2 executor, NFA estimator, AST-level precise classifier | `regex = "=1.10.x"` pinned; lookbehind / lookahead / backref / possessive parse-rejected; loom test for cancel |
| 9 | [`quanta-index-lq-symbol`](../../../crates/quanta-index-lq-symbol/) | 3 | LEX-05 | 51 | `SymbolRecordDecoder`, `SymbolIndex` | **architecture-corrected** — tree-sitter dependency dropped; decodes `UpsertSymbol.payload` (`SymbolRecord`); ADR-002/018 withdrawn; ADR-022 proposed |
| 10 | [`quanta-index-lq-ranker`](../../../crates/quanta-index-lq-ranker/) | 4 | LEX-06 | 65 | `Ranker::rank`, `Explainer::explain` | linear weighted sum, frozen-per-gen `weights_hash`; 6-component tiebreak tuple `(score, repo, gen, path, line, doc_id)` (RFC-GAP-LEX-06-1 closed); `boost:` enters as `(boost − 1.0) × w.boost_directive` |
| 11 | [`quanta-index-lq-history`](../../../crates/quanta-index-lq-history/) | 4 | LEX-07 | 76 | `CommitGraph` (channel-subscriber callbacks), history decoders | **architecture-corrected** — search-side git dropped; consumes `UpsertCommit` / `UpsertRef` / `UpsertTag` (+ proposed `UpsertDiffHunk`); ADR-023 / ADR-025 proposed |
| 12 | [`quanta-index-lq-structural`](../../../crates/quanta-index-lq-structural/) | 5 | STR-01 | 71 | `StructuralMatcher::match_pattern(pattern, parsed_tree)`, `ParseTreeRecord` decoder | **architecture-corrected** — tree-sitter on search side dropped; consumes producer `UpsertParseTree` (Option A) or surfaces `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` (Option B); ADR-003/005 withdrawn; ADR-024 proposed |
| 13 | [`quanta-index-lq-runtime`](../../../crates/quanta-index-lq-runtime/) | 5 | RT-01 | 32 | `DirtyBuffer` (channel-subscriber callbacks), snapshot / ownership catalogs | **architecture-corrected** — separate `apply_changes` IPC dropped; consumes `UpsertDirty` / `EvictDirty`; channel monotonic seq replaces advisory lock; ADR-017 withdrawn |
| 14 | [`quanta-index-lq-semantic`](../../../crates/quanta-index-lq-semantic/) | 6 | SEM-01 | 112 | `LexicalUniversePushdown::narrow`, ANN integration | HNSW chosen (deterministic seed) over Lance for v1; cosine distance pinned (L2/Dot reserved); D ≤ 1024; RFC3339 timestamps |
| 15 | [`quanta-index-lq-bridge`](../../../crates/quanta-index-lq-bridge/) | 6 | BRIDGE-01 | 70 | `SearchPlaneBridgePort::route`, `BridgeCandidatePacket` | **one-way** Sourcegraph→LQ (RFC-GAP-BRIDGE-TARGET); subset table (adopted/normalized/refused); FS-GAP-2 closed |
| 16 | [`quanta-index-lq-hybrid`](../../../crates/quanta-index-lq-hybrid/) | 7 | SEM-02 | 76 | `hybrid(lex, sem, weights, strategy)` directive, fusion strategies | RRF default (ADR-019); 8-component tiebreak tuple `(fused_score, lex_score NULL_LAST, sem_score NULL_LAST, repo, gen, path, line, doc_id)` (RFC-GAP-SEM-02-TUPLE closed); implicit-hybrid OFF |
| 17 | [`quanta-index-lq-obs`](../../../crates/quanta-index-lq-obs/) | 8 | OBS-01 | 60 | `AuditSink::write`, OTel span helpers, Prometheus exporter | sidecar-mode (ADR-009 closed); root span `lq.query` + 14 child spans; 4-layer cardinality guard; `OBS_CARDINALITY_GUARD` typed event |

**Total: 17 crates / 1,072 tests / 0 failures.**

---

## §3. Ticket → crate map

### 3.1 Spec-sheet → RFC roll-up reconciliation

Per [tickets/INDEX.md §1.1](tickets/INDEX.md) the 17 spec sheets are per-subsystem execution units; some RFC roll-up ids fan out across multiple spec sheets, and two spec sheets reframe the original RFC roll-up scope. This table records the final shipped state.

| RFC roll-up | Spec sheet(s) | Implementing crate(s) | Tests | Test gate |
|---|---|---|---|---|
| (Wave-0 prerequisite, not in RFC pack) | [PRE-CONTRACT-EXT.md](tickets/PRE-CONTRACT-EXT.md) | `quanta-index-contract::lex::*` | 43 | green |
| `LEX-01` "canonical query AST and parser" | [PRE-NORM.md](tickets/PRE-NORM.md) + [LEX-01.md](tickets/LEX-01.md) | `quanta-index-lq-norm` + `quanta-index-lq-scorer` | 75 + 43 | green |
| (Wave-0 prerequisite) | [PRE-CONF.md](tickets/PRE-CONF.md) | `quanta-index-conformance` | 32 | green |
| `LEX-00` "baseline and invariants freeze" | [LEX-00.md](tickets/LEX-00.md) | `quanta-index-lq-text-norm` | 57 | green |
| `LEX-04` "incremental lexical indexing kernel" — trigram shard | [LEX-02.md](tickets/LEX-02.md) | `quanta-index-lq-trigram` | 56 | green |
| `LEX-04` partial — position shard | [LEX-03.md](tickets/LEX-03.md) | `quanta-index-lq-positions` | 72 | green |
| `LEX-04` partial — regex executor | [LEX-04.md](tickets/LEX-04.md) | `quanta-index-lq-regex` | 81 | green |
| `LEX-04` partial — symbol shard | [LEX-05.md](tickets/LEX-05.md) (architecture-corrected) | `quanta-index-lq-symbol` | 51 | green |
| `LEX-06` "ranking, explain, and lexical semantics" — composer | [LEX-06.md](tickets/LEX-06.md) | `quanta-index-lq-ranker` | 65 | green |
| `LEX-07` "history and diff search engine" | [LEX-07.md](tickets/LEX-07.md) (architecture-corrected) | `quanta-index-lq-history` | 76 | green |
| `STR-01` "structural pattern engine" | [STR-01.md](tickets/STR-01.md) (architecture-corrected) | `quanta-index-lq-structural` | 71 | green |
| `RT-01` "runtime-aware metadata filters" | [RT-01.md](tickets/RT-01.md) (architecture-corrected) | `quanta-index-lq-runtime` | 32 | green |
| `SEM-01` partial "semantic on lexical filter pushdown" — adapter | [SEM-01.md](tickets/SEM-01.md) | `quanta-index-lq-semantic` | 112 | green |
| (RFC `SEM-02` retargeted to hybrid fusion, see [§5.5](#55-rfc-roll-up-scope-reframings)) | [SEM-02.md](tickets/SEM-02.md) | `quanta-index-lq-hybrid` | 76 | green |
| (RFC `BRIDGE-01` retargeted to Sourcegraph bridge, see [§5.5](#55-rfc-roll-up-scope-reframings)) | [BRIDGE-01.md](tickets/BRIDGE-01.md) | `quanta-index-lq-bridge` | 70 | green |
| `OBS-01` "conformance, fences, and final proof" — observability | [OBS-01.md](tickets/OBS-01.md) | `quanta-index-lq-obs` | 60 | green |

### 3.2 Architecture-corrected tickets (2026-05-25)

Four spec sheets were re-issued under the producer-authorship rule lock per [tickets/INDEX.md §3.6](tickets/INDEX.md). All four crates ship the corrected interpretation:

| Spec sheet | Original (wrong) | Corrected (shipped) |
|---|---|---|
| [LEX-05.md](tickets/LEX-05.md) | tree-sitter parse on search side | `SymbolRecordDecoder` reads `UpsertSymbol.payload`; no tree-sitter dep |
| [LEX-07.md](tickets/LEX-07.md) | search-plane git access | `CommitGraph.add_commit` is a channel-subscriber callback over `UpsertCommit` / `UpsertRef` / `UpsertTag` |
| [STR-01.md](tickets/STR-01.md) | tree-sitter parse on search side | `match_pattern(pattern, parsed_tree)` over producer-supplied `ParseTreeRecord`; v1 vs v2 ship gating Option A/B |
| [RT-01.md](tickets/RT-01.md) | separate `apply_changes` IPC (violates §11 rule 6) | `DirtyBuffer.apply/evict` callbacks over `UpsertDirty` / `EvictDirty`; advisory-lock ADR dropped |

### 3.3 RFC roll-up specs deferred

The following RFC `§Ticket Pack` roll-up scopes were not authored as spec sheets in this wave; production work is in-program but spec-bookkeeping is deferred:

| RFC ticket | Status | Reason / next step |
|---|---|---|
| `RFC-LEX-02` "global front door and surface contract cutover" | deferred | searchd-level cutover; baseline-side work (channel-arch P4–P6) |
| `RFC-LEX-03` "lexical authority unification" | deferred | functionally implemented across LEX-02 / LEX-03 / LEX-05 sibling shards; needs roll-up author |
| `RFC-LEX-05` "parallel executor and deterministic merge" | deferred | functionally satisfied by LEX-04 + LEX-06 (6/8-comp tiebreak); needs roll-up author |
| `RFC-SEM-02-original` "incremental semantic derivatives" | deferred | original RFC scope; spec retargeted to hybrid fusion ([§5.5](#55-rfc-roll-up-scope-reframings)); incremental-derivative rail lives in `quanta-index-lq-hybrid::SemanticDerivative::apply_delta` |
| `RFC-BRIDGE-01-CodeQL` "CodeQL bridge and candidate export" | deferred | spec retargeted to Sourcegraph→LQ bridge ([§5.5](#55-rfc-roll-up-scope-reframings)); CodeQL invocation builder is contract-only |

These are documentation-only deferrals; the shipped code paths exercise the underlying behaviour through the routed spec sheets above.

---

## §4. SSOT + key docs

| Document | One-line summary | Cross-link |
|---|---|---|
| Channel architecture SSOT | Canonical SSOT for producer/search transport — `BundleChannelPublisher` / `BundleChannelSubscriber` traits, op enums (shipped + proposed), in-memory generation ledger pattern (replaces SQLite control plane). | [../../ssot/channel-architecture.md](../../ssot/channel-architecture.md) |
| Producer handoff doc (NEW) | Wire shapes, emission ordering, and AMB-PROD-1..12 resolutions for the 9 newly proposed channel ops; lock-step cutover handshake. | [../../ssot/producer-handoff.md](../../ssot/producer-handoff.md) |
| Ticket INDEX | Reconciliation table spec-sheet → RFC roll-up, architecture-correction record §3.6, ambiguity register §3.7. | [tickets/INDEX.md](tickets/INDEX.md) |
| Implementation plan | Per-wave entry/exit gates, per-ticket DoD with shipped status, risk register, ADR decision log. | [implementation-plan.md](implementation-plan.md) |
| RFC (amended 2026-05-25) | LQ kernel architecture; §Execution Model (3 canonical merge tuples) and §Error Code Taxonomy (30+ new codes, 7 new families) amended; §Ticket Pack architecture-correction note added. | [rfc.md](rfc.md) |
| Feature scope | Per-feature in/out-of-scope ledger; FS-GAP-2/3 reconciled with RFC. | [feature-scope.md](feature-scope.md) |
| Grammar / DSL | EBNF grammar, filter table, error taxonomy bridge; DSL-GAP-1/2/3 closed. | [dsl.md](dsl.md) |
| Usecase corpus | 139-row conformance corpus (was 100; +39 additive rows: 5 history + 12 semantic + 22 hybrid + 5 promotions). | [usecase.md](usecase.md) |

---

## §5. Spec amendments + corrections

### 5.1 Architecture correction (2026-05-25)

Producer-authorship rule lock landed in [channel-architecture.md §3.1](../../ssot/channel-architecture.md):

> Every payload carried by these ops … is **authored by the producer** in `semantica-codegraph-v2`. Search plane never parses source bytes, never walks git, never computes embeddings. It decodes producer-supplied records and indexes them.

Consequences:

- 4 ticket specs re-issued (LEX-05 / LEX-07 / STR-01 / RT-01) — see [§3.2](#32-architecture-corrected-tickets-2026-05-25).
- 9 new channel ops proposed (`UpsertCommit`, `UpsertRef`, `UpsertTag`, `DeleteRef`, `DeleteTag`, `UpsertDirty`, `EvictDirty`, `UpsertParseTree`, `DeleteParseTree`).
- Tree-sitter dependency dropped on the search side (was anchored in LEX-05 / STR-01 / LEX-00 path-detect; producer is now the sole tree-sitter integrator).
- Separate `apply_changes` IPC framing dropped (`BundleChannelPublisher::publish` is sole producer→search ingress per [channel-architecture.md §11 rule 6](../../ssot/channel-architecture.md)).
- New [producer-handoff.md](../../ssot/producer-handoff.md) authored as the single agreement artefact for the 9 ops + AMB-PROD-1..12.

### 5.2 RFC merge tuple amendment

[rfc.md §Execution Model § Merge determinism rule](rfc.md) extended from a single 4-component tuple to three canonical tuples (closes `RFC-GAP-LEX-06-1` + `RFC-GAP-SEM-02-TUPLE`):

| Lane | Tuple |
|---|---|
| Baseline (catalog / path / non-ranked) | 4-component `(score DESC, repo_id ASC, manifest_generation ASC, candidate_id ASC)` |
| Lexical-ranked (LEX-06) | 6-component `(score, repo_id, manifest_generation, repo_relative_path, start_line, doc_id)` |
| Hybrid-fused (SEM-02) | 8-component `(fused_score, lex_score NULL_LAST, sem_score NULL_LAST, repo_id, manifest_generation, repo_relative_path, start_line, candidate_id)` |

Tuple selection is one-shot at plan time; no dynamic extension.

### 5.3 RFC error code taxonomy extension

[rfc.md §Error Code Taxonomy](rfc.md) grew by 30+ codes across 7 new families to close `RFC-GAP-LEX-04-FORBIDDEN`, `RFC-GAP-LEX-06-2`, `RFC-GAP-LEX-07-CODES`, `RFC-GAP-STR-01-CODES`, `RFC-GAP-RT-01-CODES`, `RFC-GAP-SEM-01-CODES`, `RFC-GAP-SEM-02-CODES`:

| Family | Owning ticket | Codes added |
|---|---|---|
| `HISTORY_*` | LEX-07 | `HISTORY_REF_NOT_FOUND`, `HISTORY_RANGE_OVERRUN`, `HISTORY_MERGE_CYCLE`, `HISTORY_TRACE_INCOMPLETE`, `HISTORY_UNINDEXED`, `HISTORY_COMMIT_DECODE_FAIL`, `HISTORY_REF_DECODE_FAIL`, `HISTORY_COMMIT_PARENT_UNKNOWN` |
| `STR_*` | STR-01 | `STR_PARSE_FAIL`, `STR_INVALID_METAVAR`, `STR_LANG_NOT_SUPPORTED`, `STR_LANG_RESOLUTION_EMPTY`, `STR_TYPED_HOLE_NOT_IMPLEMENTED`, `STR_PARSE_TREE_DECODE_FAIL`, `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` |
| `DIRTY_*` | RT-01 | `DIRTY_STALE_GEN`, `DIRTY_BUFFER_FULL`, `DIRTY_TTL_EXPIRED`, `DIRTY_BAD_IDENTITY`, `DIRTY_PAYLOAD_DECODE_FAIL` |
| `SEM_*` | SEM-01 | `SEM_DIM_MISMATCH`, `SEM_NOT_READY`, `SEM_INVALID_VECTOR`, `SEM_METRIC_UNSUPPORTED`, `SEM_ANN_NONDETERMINISTIC`, `SEM_HNSW_PARAMS_INVALID` |
| `HYB_*` | SEM-02 | `HYB_INVALID_WEIGHTS`, `HYB_GEN_MISMATCH`, `HYB_PUSHDOWN_INCOMPLETE`, `HYB_TOP_K_INVALID`, `HYB_STRATEGY_UNSUPPORTED`, `HYB_SUBQUERY_INVALID` |
| `SYMBOL_*` | LEX-05 | `SYMBOL_PAYLOAD_DECODE_FAIL`, `SYMBOL_RECORD_INVALID` |
| `OBS_*` | OBS-01 | `OBS_CARDINALITY_GUARD`, `OBS_INVALID_SPAN`, `OBS_INVALID_METRIC`, `OBS_AUDIT_MISSING_FIELD` |
| `BRIDGE_*` (extended) | BRIDGE-01 | `BRIDGE_UNSUPPORTED_FILTER`, `BRIDGE_UNSUPPORTED_DIRECTIVE`, `BRIDGE_AMBIGUOUS_FILTER`, `BRIDGE_VERSION_PIN`, `BRIDGE_TRANSLATE_FAIL` (FS-GAP-2 closed; 2 → 7 codes) |
| `PARSE_*` (extended) | LEX-04 | `PARSE_FORBIDDEN_SYNTAX` |
| `EXEC_*` (extended) | LEX-06 | `RANK_INVALID_SIGNAL` |

All codes fold into the single contract-level `LexicalErrorCode` enum (29 v1 variants + the additions above) per [PRE-CONTRACT-EXT.md §4](tickets/PRE-CONTRACT-EXT.md).

### 5.4 ADR catalog status

| Status | ADRs |
|---|---|
| **Withdrawn (5)** | ADR-002 (LEX-05 tree-sitter vendor), ADR-003 (STR-01 engine choice), ADR-005 (tree-sitter grammar pin), ADR-017 (RT-01 separate `apply_changes` IPC + advisory lock), ADR-018 (symbol extraction vendor) |
| **Newly proposed (4)** | ADR-022 (`SymbolRecord` wire-shape ownership), ADR-023 (`CommitRecord` wire-shape ownership), ADR-024 (`ParseTreeRecord` wire-shape + Option A/B), ADR-025 (diff hunk authorship — Option Y per-hunk op) |
| **Closed in-flight** | ADR-001 (parser strategy = hand-rolled), ADR-004 (embedding model ratified), ADR-006 (corpus format = TOML), ADR-007 (planner directive ordering = filter-before-directive), ADR-009 (admission queue = drop-newest), ADR-010 (BM25 `k1, b` pinned), ADR-011 (history crate = `lq-history` shipped), ADR-012 (structural crate = `lq-structural` shipped), ADR-013 (hybrid weights = fixed), ADR-014 (invalidation depth cap shipped), ADR-015 (bridge crate = `lq-bridge` shipped), ADR-016 (audit sink), ADR-019 (RRF default for hybrid fusion) |
| **Still pending** | ADR-008 (ACL source — gated on producer metadata authority, Q-FS-3) |

### 5.5 RFC roll-up scope reframings

Two RFC `§Ticket Pack` entries were re-scoped during execution; the shipped behaviour deviates from the literal RFC title:

| RFC ticket id | Original RFC title | Shipped scope | Tracking |
|---|---|---|---|
| `SEM-02` | "incremental semantic derivatives" | hybrid lex+sem fusion (RRF default + 8-comp tuple) | `RFC-GAP-SEM-02-FRAMING` — RFC §Ticket Pack amendment note in place; original-scope incremental-derivative rail lives in `quanta-index-lq-hybrid::SemanticDerivative::apply_delta` |
| `BRIDGE-01` | "CodeQL bridge and candidate export" | one-way Sourcegraph→LQ syntax translator | `RFC-GAP-BRIDGE-TARGET` — RFC §Ticket Pack amendment note in place; CodeQL invocation builder is contract-only via `BridgeCandidatePacket` |

### 5.6 FS-GAP / DSL-GAP closures

| Gap | Resolution |
|---|---|
| FS-GAP-2 (bridge error code set divergence) | RFC error taxonomy extended to 7 bridge codes; feature-scope.md updated to point at RFC table |
| FS-GAP-3 (scale targets diverge) | RFC SLO wins; feature-scope.md updated (100k repos) |
| DSL-GAP-1 (`count:` cap mismatch) | dsl.md cap honored |
| DSL-GAP-2 (`lang:` 60-entry enum vs 5-lang ship set) | typed split — 5 ship-langs accepted; 55 reserved as `NormalizerUnknownLang` |
| DSL-GAP-3 (`LqCanonicalHashV1` placeholder name) | final name pinned in PRE-CONTRACT-EXT |
| UC-EDGE-10 vs dsl.md §6.1 filter case sensitivity | dsl.md wins (case-insensitive lookup, lower-cased canonical); usecase.md follow-up filed |
| Wave-0 exit verdict (impl-plan "100 rows blocked") | revised — 139-row corpus, PRE-CONF reports per-row `ok` / `error_expected` / `blocked` accurately |
| PRE-CONF corpus location | default `usecase-corpus/` per PRE-CONF §12 |
| PRE-CONF test target | new crate `quanta-index-conformance` per PRE-CONF spec |
| G-CONTROL-LOC (working-tree divergence) | resolved via channel-arch SSOT §5.2 — in-memory ledgers replace SQLite control plane |

---

## §6. Surfaced ambiguities

Three-column register. Status legend: **CLOSED** = decision recorded and shipped; **OPEN** = decision still pending; **DEFERRED** = punted to later wave / external doc; **OWNED-BY-PRODUCER** = blocking item lives in producer repo or producer-handoff §8 handshake.

### 6.1 AMB-PROD-* (producer-handoff owned)

All 12 producer-handoff ambiguities have at least a recommended default per [producer-handoff.md §7](../../ssot/producer-handoff.md). Cutover-blocking items wait on producer sign-off via the §8 handshake protocol.

| ID | Description | Status |
|---|---|---|
| AMB-PROD-1 | Producer commit emission ordering | CLOSED (recommended default: topological per-generation; pending §8 sign-off) |
| AMB-PROD-2 | `CommitRecord` full wire shape | CLOSED (locked in [producer-handoff.md §3.1.1](../../ssot/producer-handoff.md); ADR-023 proposed) |
| AMB-PROD-3 | `DeleteCommit` op presence / force-push handling | CLOSED (no `DeleteCommit` v1; force-push = fresh generation) |
| AMB-PROD-4 | Diff hunk authorship (inline vs separate op) | CLOSED (Option Y recommended; ADR-025 proposed) |
| AMB-PROD-5 | `SymbolRecord` wire-shape ownership | CLOSED (`wire_version: u32` per record; ADR-022 proposed) |
| AMB-PROD-6 | `ParseTreeRecord` wire-shape + version | CLOSED (recursive `ParseNode` + `source_hash`; ADR-024 proposed) |
| AMB-PROD-7 | Producer `UpsertDirty` emission cadence | OWNED-BY-PRODUCER (per-edit / 100 ms debounce both acceptable; producer-side ADR) |
| AMB-PROD-8 | `EvictDirty` vs `UpsertDirty` ordering at same `doc_id` | CLOSED (channel monotonic seq is authority; producer contract guarantees seq ordering) |
| AMB-PROD-9 | Channel WAL retention horizon vs RT-01 TTL | OWNED-BY-PRODUCER (producer keeps segments ≥ `max(subscriber_lag, dirty_ttl=300s)`) |
| AMB-PROD-10 | `DIRTY_BAD_IDENTITY` validation timing | CLOSED (sync at apply per RT-01 §8 + [producer-handoff.md §3.2.6](../../ssot/producer-handoff.md)) |
| AMB-PROD-11 | STR-01 Option A (ship `UpsertParseTree` v1) vs Option B (defer to v2) | OPEN (gated on integrator decision at wave-5 entry; both code paths shipped in `quanta-index-lq-structural`) |
| AMB-PROD-12 | Diff hunk Option Y producer cost | OPEN (non-blocking; flagged for producer-side buffer sizing) |

### 6.2 RFC-GAP-*

| ID | Description | Status |
|---|---|---|
| RFC-GAP-LEX-06-1 | RFC §Merge determinism 4-comp vs LEX-06 6-comp | CLOSED ([§5.2](#52-rfc-merge-tuple-amendment)) |
| RFC-GAP-LEX-06-2 | `RANK_INVALID_SIGNAL` missing | CLOSED (added to RFC `EXEC_*` family) |
| RFC-GAP-LEX-06-3 | `boost:` × ACL interaction unspecified | CLOSED (ACL still first AND clause; boost is post-ACL signal) |
| RFC-GAP-LEX-07-CODES | History error codes missing | CLOSED ([§5.3](#53-rfc-error-code-taxonomy-extension)) |
| RFC-GAP-LEX-07-REVMAX | `HISTORY_REVISIONS_MAX = 10,000` vs `count:all = 100,000` | CLOSED (RFC §Capacity reconciliation note: HISTORY cap is per-query, not corpus-wide) |
| RFC-GAP-LEX-04-FORBIDDEN | `PARSE_FORBIDDEN_SYNTAX` missing | CLOSED (added to RFC `PARSE_*` family) |
| RFC-GAP-STR-01-CODES | Structural error codes missing | CLOSED ([§5.3](#53-rfc-error-code-taxonomy-extension)) |
| RFC-GAP-RT-01-CODES | Dirty error codes missing | CLOSED ([§5.3](#53-rfc-error-code-taxonomy-extension)) |
| RFC-GAP-SEM-01-CODES | Semantic error codes missing | CLOSED ([§5.3](#53-rfc-error-code-taxonomy-extension)) |
| RFC-GAP-SEM-02-CODES | Hybrid error codes missing | CLOSED ([§5.3](#53-rfc-error-code-taxonomy-extension)) |
| RFC-GAP-SEM-02-TUPLE | 4-comp → 8-comp hybrid tuple | CLOSED ([§5.2](#52-rfc-merge-tuple-amendment)) |
| RFC-GAP-SEM-02-FRAMING | SEM-02 original scope = incremental derivatives | DEFERRED ([§5.5](#55-rfc-roll-up-scope-reframings)) — RFC amendment note in place; original-scope rail still ships via `SemanticDerivative::apply_delta` |
| RFC-GAP-BRIDGE-TARGET | BRIDGE-01 original = CodeQL bridge | DEFERRED ([§5.5](#55-rfc-roll-up-scope-reframings)) — RFC amendment note in place; CodeQL invocation builder rides on `BridgeCandidatePacket` |
| RFC-GAP-TICKET-IDS | Systematic numbering mismatch | DEFERRED (low-priority bookkeeping; spec sheets keep file names per `tickets/INDEX.md §1.1` decision) |
| RFC-GAP-1 (LEX-07 scope enumeration: `parent:` / `merge:` / `tag:` / `revisions:` / `since.time:`) | RFC LEX-07 one-liner under-specified | DEFERRED (RFC LEX-07 scope amendment pending) |
| RFC-GAP-2 (Wave-0 absence) | RFC §Canonical Execution Waves starts at Wave-1 | DEFERRED (RFC wave plan amendment pending) |
| RFC-GAP-3 (telemetry doc forward-reference) | RFC §Observability forward-refs implementation-plan | DEFERRED — covered by OBS-01 shipped span schema |
| RFC-GAP-4 (structural / history / runtime / bridge SLOs absent) | RFC §Latency SLOs only covers lexical | DEFERRED — OBS-01 measurement rails ship; per-engine SLO targets need RFC pin |
| RFC-GAP-5 (`tenant_id` / `user_id` carrier) | `LqQuery` did not carry auth identity | CLOSED via PRE-CONTRACT-EXT carrier addition + LEX-02 ACL injection |
| RFC-GAP-6 (writer registry in RFC §Failure and Recovery) | RFC names writer registry | CLOSED — producer is sole publisher per `publisher.lock`; RFC amendment note in place |
| RFC-GAP-PROD (producer-authorship correction) | RFC framing of 4 tickets violated authorship rule | CLOSED ([§5.1](#51-architecture-correction-2026-05-25)) |

### 6.3 FS-GAP / DSL-GAP

| ID | Description | Status |
|---|---|---|
| FS-GAP-1 | `dirty:` Q5 unresolved | CLOSED — RT-01 shipped with `UpsertDirty` channel ops |
| FS-GAP-2 | Bridge error code set divergence | CLOSED ([§5.6](#56-fs-gap--dsl-gap-closures)) |
| FS-GAP-3 | Scale targets diverge | CLOSED ([§5.6](#56-fs-gap--dsl-gap-closures)) |
| DSL-GAP-1 | `count:` cap mismatch | CLOSED ([§5.6](#56-fs-gap--dsl-gap-closures)) |
| DSL-GAP-2 | `lang:` 60-entry enum vs 5-lang ship set | CLOSED ([§5.6](#56-fs-gap--dsl-gap-closures)) |
| DSL-GAP-3 | `LqCanonicalHashV1` placeholder name | CLOSED ([§5.6](#56-fs-gap--dsl-gap-closures)) |

### 6.4 Other surfaced items

| ID | Description | Status |
|---|---|---|
| G-CONTROL-LOC | Working-tree divergence: `quanta-index-control/` deleted, `quanta-index-channel/` added | CLOSED via channel-arch §5.2 in-memory ledger |
| LEX-05 §6/§7/§9 stale refs | Per-file-extract perf rows / metrics referencing tree-sitter | CLOSED by Round-4b cleanup pass |
| LEX-05 §11 row 22 Sourcegraph claim-discipline | Semantics narrowed to "given equivalent extraction" | CLOSED by Round-4b cleanup |
| SEM-01 Lance terminology | Spec mentioned Lance; shipped backend is HNSW | CLOSED by Round-4b cleanup (HNSW pinned) |
| UC-GAP-1 (hybrid usecases missing) | Corpus had no hybrid rows | CLOSED — 22 new UC-HYB-* rows added |
| UC-GAP-2 (no incremental-write UC row) | Corpus had no LEX-04 RFC §2 proof row | DEFERRED — proof rail green via apply-trace; UC-INC-* category pending |
| UC-GAP-3 (no catalog-miss UC row) | Corpus had no `STATE_NOT_READY: CATALOG_MISS` end-to-end row | DEFERRED — typed code shipped; UC row pending |
| 5 missing RFC roll-up specs | RFC-LEX-02, RFC-LEX-03, RFC-LEX-05, RFC-SEM-02-original, RFC-BRIDGE-01-CodeQL | DEFERRED ([§3.3](#33-rfc-roll-up-specs-deferred)) |

---

## §7. Integration roadmap (NEXT)

The 17 shipped crates are **green per-crate**, and the repo-level proof rails are also green. Remaining work is now limited to producer-side cutover, deployment wiring, and one real-engine conformance rail.

| # | Item | Blocking? | Owner |
|---|---|---|---|
| 1 | **Producer-side implementation** of the 9 proposed channel ops (`UpsertCommit`, `UpsertRef`, `UpsertTag`, `DeleteRef`, `DeleteTag`, `UpsertDirty`, `EvictDirty`, `UpsertParseTree`, `DeleteParseTree`) + `UpsertDiffHunk` (Option Y); coordinated cutover via [producer-handoff.md §8](../../ssot/producer-handoff.md) | yes (blocks LEX-07 / RT-01 / STR-01 live integration) | `semantica-codegraph-v2` team |
| 2 | **Live channel subscriber cutover** — connect producer-emitted history / dirty / parse-tree ops to the shipped `searchd::ChannelDispatcher` callbacks and remove any remaining integration stubs | yes (gated on item 1) | integration ticket |
| 3 | **Conformance corpus runner against real pipeline** — replace `StubLqEngine` with the wired `searchd` engine on the 139-row corpus; CI gate `ci/lq-conformance` from "blocked" → "ok" | gated on items 1–2 | tools/ci |
| 4 | **OBS-01 OTel + Prometheus exporter sidecar** wire-up at deployment level (the crate exports are shipped; the sidecar runtime configuration is operator work) | no (does not block repo proof rails; blocks production observability closeout) | ops |
| 5 | **BRIDGE-01 downstream sink cutover** — keep `BridgeCandidatePacket` as the contract authority, then wire the live downstream consumer | gated on items 1 + 4 | integration ticket |
| 6 | **Round 5 — delta API tightening** (landing): `upsert_X`, `remove_X`, `from_prior(...)` APIs across the 4 affected LQ builders (`quanta-index-lq-trigram`, `quanta-index-lq-positions`, `quanta-index-lq-symbol`, `quanta-index-lq-scorer`) so search-side builders are idempotent under at-least-once replay and support cross-generation delta inherit per [producer-handoff.md §3.5](../../ssot/producer-handoff.md). RT-01 dirty buffer / HNSW / `ManifestLedger` (LEX-07) are already idempotent. Cross-link: [tickets/INDEX.md §3.9](tickets/INDEX.md) (delta contract lock) | gated on item 1 for full live integration; per-crate landings independent | LQ-builder agents (4-way parallel) |
| 9 | **Round 7 — semantic-family delta API + cascade + handle ADR** (in flight): land `SemanticIndexBuilder::remove_embedding(doc_id)` to close the `DeleteChunk` → semantic shard cascade per [producer-handoff.md §3.5.2](../../ssot/producer-handoff.md) (Round 7a); ratify [ADR-026](implementation-plan.md) `SemanticVectorRef::Handle` storage model (recommendation: Option B = handle == `embedding_id`); wire `SEM_HANDLE_NOT_FOUND` typed error per [producer-handoff.md §6.5](../../ssot/producer-handoff.md). Cross-link: AMB-PROD-13 / AMB-PROD-14 in [tickets/INDEX.md §3.7](tickets/INDEX.md), Q-RFC-SEM-02-3 in [tickets/RFC-SEM-02.md §12](tickets/RFC-SEM-02.md) | gated on item 1 for live integration; ADR ratification independent | LQ-semantic agent + producer-handoff ADR ratifier |

Other deferred items (not roadmap-blocking but should be tracked):

- AMB-PROD-11 (STR-01 Option A vs B): integrator decision at wave-5 entry; both code paths already ship.
- ADR-008 (ACL source — Q-FS-3): producer metadata authority decision.
- Q-FS-8 (Bridge candidate generation stability across mid-flight activation): cross-wave activation behaviour spec.
- Q-FS-context (`context:` lifecycle): Phase-4+ authz roadmap.
- 5 missing RFC roll-up specs ([§3.3](#33-rfc-roll-up-specs-deferred)): low-priority bookkeeping.

---

## §8. Test gate verdict

### 8.1 Per-crate × per-gate matrix

For each of the 17 crates the four per-crate rails are green:

| Crate | `cargo check` | `cargo clippy --all-targets -- -D warnings` | `cargo test` | `cargo fmt --check` | semgrep |
|---|---|---|---|---|---|
| `quanta-index-contract` (`lex::*`) | green | green | 43 / 43 | green | 0 |
| `quanta-index-lq-norm` | green | green | 75 / 75 | green | 0 |
| `quanta-index-conformance` | green | green | 32 / 32 | green | 0 |
| `quanta-index-lq-text-norm` | green | green | 57 / 57 | green | 0 |
| `quanta-index-lq-scorer` | green | green | 43 / 43 | green | 0 |
| `quanta-index-lq-trigram` | green | green | 56 / 56 | green | 0 |
| `quanta-index-lq-positions` | green | green | 72 / 72 | green | 0 |
| `quanta-index-lq-regex` | green | green | 81 / 81 | green | 0 |
| `quanta-index-lq-symbol` | green | green | 51 / 51 | green | 0 |
| `quanta-index-lq-ranker` | green | green | 65 / 65 | green | 0 |
| `quanta-index-lq-history` | green | green | 76 / 76 | green | 0 |
| `quanta-index-lq-structural` | green | green | 71 / 71 | green | 0 |
| `quanta-index-lq-runtime` | green | green | 32 / 32 | green | 0 |
| `quanta-index-lq-semantic` | green | green | 112 / 112 | green | 0 |
| `quanta-index-lq-bridge` | green | green | 70 / 70 | green | 0 |
| `quanta-index-lq-hybrid` | green | green | 76 / 76 | green | 0 |
| `quanta-index-lq-obs` | green | green | 60 / 60 | green | 0 |
| **Totals** | **17 / 17** | **17 / 17** | **1,072 / 1,072** | **17 / 17** | **0 findings** |

Semgrep rail anchored at [tools/ci/semgrep/rules.yml](../../../tools/ci/semgrep/rules.yml) (rule `rust-no-serde-derive` enforces D18 hand-rolled serde).

### 8.2 Workspace-level rails

| Rail | Status | Blocker |
|---|---|---|
| `cargo check --workspace` | **green** | none |
| `cargo test --workspace` | **green** | none |
| `cargo clippy --workspace --all-targets -- -D warnings` | **green** | none |
| `cargo fmt --all -- --check` | **green** | none |
| `cargo test -p quanta-index-searchd` | **green** | none; validates the harsh local UDS end-to-end pack (`end_to_end`, `explain`, `repo_map_end_to_end`) |
| `just verify-rust-heavy` (Miri / careful / TSan / ASan / mutants / udeps) | **deferred** | intentionally out of the closeout rail; not required for the repo-first claim boundary |
| Conformance corpus runner against real engine | **deferred** | gated on baseline + producer cutover; see [§7](#7-integration-roadmap-next) item 5 |

The per-crate gates remain the implementation proof for the 17 LQ-family crates. The workspace-level rails are now green and prove the repo-internal closeout; the remaining roadmap items in [§7](#7-integration-roadmap-next) are producer / deployment cutovers.

---

## §9. Acknowledgments / decisions log

Major decisions locked during the program. Each row is a final, non-revisitable call within v1; revisiting requires a fresh RFC amendment.

| Decision | Locked at | Rationale |
|---|---|---|
| **Producer-authorship rule** — search plane never parses source, walks git, or computes embeddings | 2026-05-25 ([channel-architecture.md §3.1](../../ssot/channel-architecture.md)) | Cleanest producer/search split; single ingress per channel; structurally inverts the Zoekt/Elasticsearch "extract on search" pattern |
| **Tree-sitter dependency dropped on the search side** | consequence of producer-authorship | 4 ticket specs re-issued (LEX-05 / STR-01 / LEX-07 / RT-01); producer owns the grammar matrix |
| **HNSW chosen over Lance** for SEM-01 ANN backend | LEX-06 / SEM-01 wave 6 | Deterministic seed; cross-instance reproducibility test green; cosine distance pinned |
| **RRF default** for hybrid fusion (SEM-02) | ADR-019 closed | Sum-of-reciprocal-ranks is robust to score-scale skew across lex/sem; weighted strategy available but not default |
| **6-component tiebreak tuple** for lexical-ranked merge (LEX-06) | RFC §Execution Model amendment ([§5.2](#52-rfc-merge-tuple-amendment)) | 4-component baseline cannot resolve intra-`(repo, generation)` ties; `(score, repo, gen, path, line, doc_id)` is total |
| **8-component tiebreak tuple** for hybrid merge (SEM-02) | RFC §Execution Model amendment ([§5.2](#52-rfc-merge-tuple-amendment)) | NULL_LAST on one-engine-only candidates; preserves total ordering across the fused stream |
| **`publisher.lock` is sole ingress**; no advisory lock; no `apply_changes` IPC | channel-arch §4.1 + §11 rule 6 | One-writer-per-track invariant; channel seq monotonicity replaces RT-01 advisory lock; ADR-017 withdrawn |
| **No `DeleteCommit` op**; force-push handled by fresh generation | AMB-PROD-3 / [producer-handoff.md §3.1.3](../../ssot/producer-handoff.md) | Generations are append-only on the wire; rewrites create N+1 |
| **`UpsertDiffHunk` separate op** (Option Y) for diff hunks | ADR-025 proposed / [producer-handoff.md §3.1.4](../../ssot/producer-handoff.md) | Streaming hygiene; per-op size bounded; large commits do not block other commits |
| **In-memory generation ledger** replaces SQLite control plane | channel-arch §5.2 (resolves G-CONTROL-LOC) | Generation state reconstructed from channel on startup; `LexicalGenerationLedger` + `SemanticGenerationLedger` per track |
| **Breaking-first posture** — no long-lived shims | CLAUDE.md `Agent change posture` | Producer + search cuts are lock-step; wire bumps via §8 handshake |
| **Hand-rolled serde** (D18) | semgrep `rust-no-serde-derive` | 0 findings across 17 crates; cold-build seconds bounded; wire shape auditable |

---

## §10. Sign-off

**Spec authorship**: ticket spec sheets in [tickets/](tickets/) + RFC + sibling docs (rfc.md, feature-scope.md, usecase.md, dsl.md, implementation-plan.md).
**Implementation agent**: this repo (search plane).
**Producer agent**: `semantica-codegraph-v2` (out-of-tree; producer-handoff cutover pending).

### 10.1 Verdict

**Per-crate ship gates: green.** 17 crates, 1,072 tests, 0 failures, 0 clippy warnings, 0 semgrep findings, fmt green.

**Workspace proof rails: green.** `cargo check --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and the `searchd` UDS end-to-end pack all pass as of 2026-05-25.

**Production cutover still depends on external / deployment items**:

1. Producer-side: 9 new channel ops + Option-Y `UpsertDiffHunk` cutover via [producer-handoff.md §8](../../ssot/producer-handoff.md) handshake — [§7](#7-integration-roadmap-next) item 1.
2. Deployment-side: OTel / Prometheus sidecar wiring and bridge downstream sink — [§7](#7-integration-roadmap-next) items 4–5.

**Outstanding blockers**

| Blocker | Side | Item |
|---|---|---|
| Producer ops cutover + `wire_version=1` handshake | producer | [§7](#7-integration-roadmap-next) item 1 |
| STR-01 Option A vs B (AMB-PROD-11) | integrator | [§6.1](#61-amb-prod--producer-handoff-owned) |
| Live channel subscriber cutover for producer-fed history / dirty / parse-tree data | integration | [§7](#7-integration-roadmap-next) item 2 |
| Real-engine conformance CI cutover | tools/ci | [§7](#7-integration-roadmap-next) item 3 |
| OTel / Prometheus sidecar + bridge downstream sink | ops / integration | [§7](#7-integration-roadmap-next) items 4–5 |
| ADR-008 (ACL source) | producer / RFC | [§5.4](#54-adr-catalog-status) |

**Repo-internal closeout: complete.** The 17 LQ-family crates and the live `searchd` path are green in the current worktree. The roadmap in [§7](#7-integration-roadmap-next) is the canonical sequence to finish producer / deployment integration.

---

## End of SHIPPED record

This document is the authoritative "where we landed" doc for the May-24 Lexical Kernel program. Per CLAUDE.md generated-doc rules, this file is hand-authored (not produced by `tools/prompt-manager/`) and is checked into `docs/plans/may-24-lexical-indexing-sorucegraph/` alongside the source spec sheets it summarises. Any post-completion drift between this doc and the per-ticket spec sheets / SSOTs / RFC must be filed as a follow-up against this file's owner.
