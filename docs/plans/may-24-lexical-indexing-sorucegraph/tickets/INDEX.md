# Tickets — Index and Reconciliation

> Master index for the 17 ticket spec sheets under [`tickets/`](.) authored against the May-23 Sourcegraph-class Lexical Kernel RFC.

Parent docs: [rfc.md](../rfc.md) · [feature-scope.md](../feature-scope.md) · [usecase.md](../usecase.md) · [dsl.md](../dsl.md) · [implementation-plan.md](../implementation-plan.md)

---

## 1. ID-namespace reconciliation (read this first)

The RFC [§Ticket Pack](../rfc.md#L572) defines **14 coarse roll-up tickets** with high-level titles. The 17 spec sheets in this directory are **per-subsystem execution units** that were authored at finer granularity. **File names overlap with RFC ticket IDs but do not always describe the same scope.**

This is an honest-gap call (filed as `RFC-GAP-TICKET-IDS`). The spec sheets are useful as-is for executors — they pin vendor choices, error codes, test plans, and DoD checklists at the subsystem level. The INDEX maps spec-sheet → RFC ticket roll-up so engineers can satisfy a RFC ticket by closing the right combination of spec sheets.

**Decision (proposed, requires sign-off):** keep file names; treat each `tickets/<ID>.md` as `SPEC-<ID>` (a subsystem spec sheet), and treat RFC `<ID>` (a roll-up). The mapping table below is canonical until either (a) spec files are renamed `SPEC-*`, or (b) the RFC ticket pack is amended.

### 1.1 Spec-sheet → RFC roll-up map

| File | Scope (per spec sheet) | RFC roll-up id | RFC roll-up title | Maps cleanly? |
|---|---|---|---|---|
| [PRE-CONTRACT-EXT.md](PRE-CONTRACT-EXT.md) | Contract crate extension (GAP-01..06, `ErrorCode`, authz carrier) | — | Wave-0 prerequisite — not in RFC ticket pack | new |
| [PRE-NORM.md](PRE-NORM.md) | DSL parser + canonical normalizer + CBOR/SHA-256 hash | `LEX-01` | "canonical query AST and parser" | yes |
| [PRE-CONF.md](PRE-CONF.md) | Conformance corpus runner consuming usecase.md 100 rows | — | Wave-0 prerequisite — not in RFC ticket pack | new |
| [LEX-00.md](LEX-00.md) | Lexical text normalization layer (tokenize/fold/lang routing) | `LEX-00` | "baseline and invariants freeze" | yes |
| [LEX-01.md](LEX-01.md) | IDF / scoring foundation (per-generation BM25 stats) | `LEX-06` partial | "ranking, explain, and lexical semantics" — scorer half | **shifted** |
| [LEX-02.md](LEX-02.md) | Trigram / N-gram index | `LEX-04` partial | "incremental lexical indexing kernel" — trigram shard | **shifted** |
| [LEX-03.md](LEX-03.md) | Phrase position index + adjacency | `LEX-04` partial | "incremental lexical indexing kernel" — position shard | **shifted** |
| [LEX-04.md](LEX-04.md) | RE2 regex executor + NFA estimator | `LEX-04` partial | "incremental lexical indexing kernel" — regex executor | **shifted** |
| [LEX-05.md](LEX-05.md) | Symbol index (tree-sitter + `tags.scm`) | `LEX-04` partial | "incremental lexical indexing kernel" — symbol shard | **shifted** |
| [LEX-06.md](LEX-06.md) | Composite ranker (weighted-linear, frozen-per-gen weights) | `LEX-06` | "ranking, explain, and lexical semantics" — composer half | yes |
| [LEX-07.md](LEX-07.md) | Generation governance + history extensions (parent/merge/tag/revisions/since.time) | `LEX-07` | "history and diff search engine" | yes (extends) |
| [STR-01.md](STR-01.md) | Structural search via tree-sitter unified IR | `STR-01` | "structural pattern engine" | yes |
| [RT-01.md](RT-01.md) | Runtime metadata + `dirty:` apply-changes channel | `RT-01` | "runtime-aware metadata filters" | yes |
| [SEM-01.md](SEM-01.md) | Semantic vector adapter + ANN integration | `SEM-01` partial | "semantic on lexical filter pushdown" — adapter half | **shifted** |
| [SEM-02.md](SEM-02.md) | Hybrid lex+sem fusion (RRF default) | — | RFC `SEM-02` = "incremental semantic derivatives" — DIFFERENT SCOPE | **drift** |
| [BRIDGE-01.md](BRIDGE-01.md) | Sourcegraph syntax → LQ translator | — | RFC `BRIDGE-01` = "CodeQL bridge and candidate export" — DIFFERENT TARGET | **drift** |
| [OBS-01.md](OBS-01.md) | Observability + SLO instrumentation (OTel + Prometheus) | `OBS-01` | "conformance, fences, and final proof" — observability half | yes |

### 1.2 Unresolved by these specs (still owed for RFC parity)

The following RFC ticket scopes are **not** covered by any spec sheet authored in this wave:

- `RFC-LEX-02` — "global front door and surface contract cutover" (IPC/searchd cutover layer)
- `RFC-LEX-03` — "lexical authority unification" (multi-shard lexical merge / engine cohesion)
- `RFC-LEX-05` — "parallel executor and deterministic merge" (fanout + merge planner)
- `RFC-SEM-02` — "incremental semantic derivatives" (per-gen ANN re-build incrementality)
- `RFC-BRIDGE-01` — "CodeQL bridge and candidate export" (we authored a Sourcegraph bridge instead)

**Action required (must precede ticket execution):** either rename current files to `SPEC-*` and author the five missing RFC-ticket roll-ups, or amend the RFC ticket pack to match the spec-sheet decomposition. Tracked in [§5 of this INDEX](#5-action-items).

---

## 2. Spec sheets — one-line summaries

### Wave 0 — Prerequisites (must land before any LQ-family ticket)

| File | Lines | Lock highlights |
|---|---|---|
| [PRE-CONTRACT-EXT.md](PRE-CONTRACT-EXT.md) | 403 | Closes GAP-01..06; `LexicalErrorCode` v1 = 29 variants; `tenant_id`/`user_id` carrier surface added |
| [PRE-NORM.md](PRE-NORM.md) | 417 | Parser+normalizer+`LqCanonicalHashV1` (CBOR+SHA-256); idempotency property; bounded inputs (16 KiB / depth 32 / fan-out 64 / NFA 100k / 256 structural) |
| [PRE-CONF.md](PRE-CONF.md) | 452 | New `quanta-index-conformance` crate proposal; consumes `usecase-corpus/` (proposed location); junit XML + per-ticket p99 attribution |

### Wave 1 — Lexical foundations

| File | Lines | Lock highlights |
|---|---|---|
| [LEX-00.md](LEX-00.md) | 521 | `LexicalNormalizer` port; v1 ship-langs = Rust/Python/TypeScript/JavaScript/Go; identifier-splitter at camelCase/snake_case/digit boundaries; `patterntype:literal` skips splitter |
| [LEX-01.md](LEX-01.md) | 586 | `LexicalScorer` + `LexicalScorerManifestPort`; per-generation `idf_table.cbor` + `scorer_manifest.cbor`; `f32 ∈ [0.0, 1.0]` score envelope; ADR-010 forces BM25 (k1, b) values |

### Wave 2 — Index acceleration shards

| File | Lines | Lock highlights |
|---|---|---|
| [LEX-02.md](LEX-02.md) | 556 | **Byte trigrams** (not code-point) — UTF-8 self-sync; verify via `memmem::find`; caps (4k trigrams/query, 100k candidates pre-verify); pure-wildcard regex → explicit verify-only path |
| [LEX-03.md](LEX-03.md) | 571 | **Stopword filter locked OFF, forever** (Sourcegraph parity + RFC §Invariants); 8-token adjacency window default; cross-chunk phrase = empty (documented, not error); positions are post-normalize |

### Wave 3 — Advanced executors

| File | Lines | Lock highlights |
|---|---|---|
| [LEX-04.md](LEX-04.md) | 360 | `regex = "=1.10.x"` pinned; lookbehind/lookahead/backref/possessive parse-rejected; NFA bound via `regex_syntax::hir::analysis::Properties`; loom test for cancel |
| [LEX-05.md](LEX-05.md) | 488 | tree-sitter + curated `tags.scm` (ctags-binary and scip rejected); v1 langs = Rust/Python/Go/TS+TSX/JS; ref/def boundary = local positions only (cross-file → SEM-01) |

### Wave 4 — Ranking + history

| File | Lines | Lock highlights |
|---|---|---|
| [LEX-06.md](LEX-06.md) | 503 | Linear weighted sum, frozen-per-gen `weights_hash`; tiebreak tuple = 6 components `(score, repo, gen, path, line, doc_id)`; `boost:` enters as `(boost − 1.0) × w.boost_directive` so default `boost=1.0` is identity |
| [LEX-07.md](LEX-07.md) | 523 | `merge:` locked to **merge-result-only**; `revisions:` capped at 10k commits; `since:` → `since.time:`/`since.commit:` parse-time split; 5 new history error codes; write-packet trace fields locked |

### Wave 5 — Structural + runtime

| File | Lines | Lock highlights |
|---|---|---|
| [STR-01.md](STR-01.md) | 632 | tree-sitter native walk + unified pattern IR (Comby + stack-machine-on-RE2 rejected); `StructuralBinding` carrier closes GAP-03; v1 langs = Rust/Python/TS/JS/Go; lex+str result envelopes **disjoint**; 256-node + 16-depth caps |
| [RT-01.md](RT-01.md) | 650 | `dirty:` source = producer-marked via `apply_changes` IPC (closes Q5); per-tenant per-repo buffer; cap 10k entries / TTL 300s defaults; write-coordinator gated (shares LEX-04 advisory lock); 4 dirty error codes |

### Wave 6/7 — Semantic + hybrid

| File | Lines | Lock highlights |
|---|---|---|
| [SEM-01.md](SEM-01.md) | 379 | `LqExpr::SemanticVector { vector_ref, top_k }`; distance = **cosine** (L2/Dot reserved); D ≤ 1024; ANN determinism via pinned RNG seed or exact-NN ≤ 100k docs; 5 new sem error codes; 12 new UC-SEM-* rows |
| [SEM-02.md](SEM-02.md) | 499 | `hybrid(lex, sem, weights={…}, strategy=rrf|weighted)` directive; **RRF default** (ADR-019); merge tuple 4→8 components; 22 new UC-HYB-* rows (closes UC-GAP-1); implicit-hybrid OFF |

### Wave 8 — Bridge + observability

| File | Lines | Lock highlights |
|---|---|---|
| [BRIDGE-01.md](BRIDGE-01.md) | 367 | **One-way** Sourcegraph→LQ; subset table buckets (adopted/normalized/refused); FS-GAP-2 closed by error taxonomy lock; `BridgeCandidate` carries `source_syntax` + `translator_version`; p99 ≤ 1ms |
| [OBS-01.md](OBS-01.md) | 503 | OTel SDK + Prometheus exporter (sidecar-mode, ADR-009); root span `lq.query` + 14 child spans enumerated; cardinality guard 4-layer defense; `OBS_CARDINALITY_GUARD` typed event; closes RFC-GAP-3 + RFC-GAP-4 |

**Total: 8,410 lines across 17 spec sheets.**

---

## 3. Aggregated gap inventory (collected from all 9 agent reports)

### 3.1 RFC amendments required (must close before Wave-3 entry)

| ID | Source ticket | Description |
|---|---|---|
| RFC-GAP-LEX-06-1 | LEX-06 | RFC § Execution Model § Merge determinism rule lists 4 tiebreak components; ranking needs 6. Amend tuple. |
| RFC-GAP-LEX-06-2 | LEX-06 | RFC § Error Code Taxonomy missing `RANK_INVALID_SIGNAL`. Add to EXEC_* family. |
| RFC-GAP-LEX-06-3 | LEX-06 | `boost:` × ACL interaction unspecified in RFC § Security/Authz. Clarify. |
| RFC-GAP-LEX-07-CODES | LEX-07 | Add `HISTORY_REF_NOT_FOUND`, `HISTORY_RANGE_OVERRUN`, `HISTORY_MERGE_CYCLE`, `HISTORY_TRACE_INCOMPLETE`, `HISTORY_UNINDEXED` to error taxonomy. |
| RFC-GAP-LEX-07-REVMAX | LEX-07 | `HISTORY_REVISIONS_MAX = 10,000` conflicts with feature-scope.md `count:all = 100,000`. Reconcile RFC § Capacity. |
| RFC-GAP-LEX-04-FORBIDDEN | LEX-04 | Add `PARSE_FORBIDDEN_SYNTAX` to taxonomy for lookbehind/lookahead/backref/possessive parse-rejections. |
| RFC-GAP-STR-01-CODES | STR-01 | Add `STR_PARSE_FAIL`, `STR_INVALID_METAVAR`, `STR_LANG_NOT_SUPPORTED`, `STR_LANG_RESOLUTION_EMPTY`, `STR_TYPED_HOLE_NOT_IMPLEMENTED`. |
| RFC-GAP-RT-01-CODES | RT-01 | Add `DIRTY_STALE_GEN`, `DIRTY_BUFFER_FULL`, `DIRTY_TTL_EXPIRED`, `DIRTY_BAD_IDENTITY` + `STATE_NOT_READY` extensions. |
| RFC-GAP-SEM-01-CODES | SEM-01 | Add `SEM_DIM_MISMATCH`, `SEM_NOT_READY`, `SEM_INVALID_VECTOR`, `SEM_METRIC_UNSUPPORTED`, `SEM_ANN_NONDETERMINISTIC`. |
| RFC-GAP-SEM-02-CODES | SEM-02 | Add `HYB_INVALID_WEIGHTS`, `HYB_GEN_MISMATCH`, `HYB_PUSHDOWN_INCOMPLETE`, `HYB_TOP_K_INVALID`, `HYB_STRATEGY_UNSUPPORTED`, `HYB_SUBQUERY_INVALID`. |
| RFC-GAP-SEM-02-TUPLE | SEM-02 | Extend merge tuple from 4 to 8 components for hybrid fusion. |
| RFC-GAP-SEM-02-FRAMING | SEM-02 | Original SEM-02 = "incremental semantic derivatives"; spec reframes as hybrid fusion. RFC amendment OR rename file. |
| RFC-GAP-BRIDGE-TARGET | BRIDGE-01 | Original BRIDGE-01 = "CodeQL bridge"; spec reframes as Sourcegraph bridge. RFC amendment OR rename + author CodeQL bridge separately. |
| RFC-GAP-TICKET-IDS | INDEX | Systematic numbering mismatch between RFC §Ticket Pack and spec sheets (see §1.1). |

All `RFC-GAP-*-CODES` rows fold into a single contract-bump PR via **PRE-CONTRACT-EXT** §4 (single source for `LexicalErrorCode` v1 = 29 variants + history/structural/dirty/sem/hyb additions).

### 3.2 ADR slot collisions (must renumber)

Implementation-plan.md §10 pre-seeded 16 ADR slots. Spec sheets surfaced overlapping claims on the same numbers:

| Conflicting slot | Claimants |
|---|---|
| ADR-008 | BRIDGE-01 (Sourcegraph routing) |
| ADR-009 | OBS-01 (OTel surface) |
| ADR-017 | LEX-04 (RE2 estimator), RT-01 (inbound mutation channel), SEM-01 (ANN determinism), LEX-00 (normalizer pipeline) — **4-way collision** |
| ADR-018 | LEX-05 (symbol extractor), SEM-02 (fusion crate location), LEX-01 (compaction × IDF) — **3-way collision** |
| ADR-019 | SEM-02 (fusion strategy default) |
| ADR-021 | SEM-02 (over-fetch policy) — optional |

**Resolution proposed**: extend implementation-plan.md §10 to numbers ADR-001 through ADR-025 with unique scoped IDs. Owner: whoever lands the next impl-plan revision. Until then, every ticket's ADR reference is provisional.

### 3.3 usecase.md additive rows (must merge when respective tickets land)

| Source | New row range | Count | Closes |
|---|---|---|---|
| LEX-07 | UC-HIST-09..13 | 5 | RFC-GAP-1 (parent:/merge:/tag:/revisions:/since.time:) |
| SEM-01 | UC-SEM-01..12 | 12 | Missing semantic corpus coverage |
| SEM-02 | UC-HYB-01..22 | 22 | UC-GAP-1 (hybrid usecases missing) |
| BRIDGE-01 | UC-BR-01..04 promoted to `ok` | 4 | UC-BR-* `pending` rows |
| RT-01 | UC-RT-08 promoted to `ok` | 1 | `dirty:` pending |

**Total new rows: 39 additive UC-* rows + 5 status promotions.**

### 3.4 Inter-sibling-doc conflicts (must reconcile)

| ID | Description | Default lock |
|---|---|---|
| UC-EDGE-10 vs dsl.md §6.1 | Filter name case sensitivity — usecase says case-sensitive lowercase, dsl says case-insensitive | Follow dsl.md (case-insensitive lookup, lower-cased canonical); file follow-up against usecase.md |
| FS-GAP-2 | Bridge error code set divergence (RFC: 2 codes; feature-scope: 5) | Lock all 5 → `LexicalErrorCode` v1 has 29 variants |
| FS-GAP-3 | Scale targets diverge (feature-scope 10K repos vs RFC 100K) | RFC SLO wins; feature-scope.md must update |
| DSL-GAP-1 | `count:` cap mismatch with feature-scope.md | Follow dsl.md cap |
| DSL-GAP-2 | `lang:` 60-entry enum vs 5-lang ship set | Typed-error split: 5 ship-langs accepted, 55 reserved as `NormalizerUnknownLang` |
| DSL-GAP-3 | `LqCanonicalHashV1` placeholder name | Rename via PRE-CONTRACT-EXT |
| Wave-0 exit verdict | impl-plan says "100 rows blocked"; reality 15+ are `error_expected` | impl-plan §4.1 sentence revision |
| PRE-CONF corpus location | impl-plan vs usecase.md vs rfc.md disagree on directory | Default `usecase-corpus/`; tracked in PRE-CONF §12 |
| PRE-CONF test target | impl-plan says `quanta-index-contract --test lq_conformance`; spec says new `quanta-index-conformance` crate | Default new crate per PRE-CONF |

### 3.5 G-CONTROL-LOC (resolved via channel-arch SSOT)

Working-tree state diverges from `736ddea` snapshot: `quanta-index-control/` deleted, `quanta-index-channel/` added, core `domains/{channel,lexical,semantic}` added. **Resolution**: [`docs/ssot/channel-architecture.md`](../../../ssot/channel-architecture.md) §5.2 — generation state is reconstructed from channel events in-memory (no SQLite); per-track ledgers (`LexicalGenerationLedger` / `SemanticGenerationLedger`) live in `domains/<track>`. The abstract "control-plane crate" reference in every ticket maps to this in-memory ledger pattern.

### 3.6 Producer-authorship correction (CRITICAL — fixed 2026-05-25)

**Original mistake**: LEX-05 / STR-01 / LEX-07 / RT-01 spec sheets were authored assuming the **search plane** parses source bytes, walks git, or accepts a separate `apply_changes` IPC. This is wrong. Per [`docs/ssot/channel-architecture.md`](../../../ssot/channel-architecture.md) §0 + §3.1 (Authorship rule lock):

> Every payload carried by these ops … is **authored by the producer** in `semantica-codegraph-v2`. Search plane never parses source bytes, never walks git, never computes embeddings. It decodes producer-supplied records and indexes them.

**Corrected ticket specs** (2026-05-25):

| Ticket | Mistake | Correction |
|---|---|---|
| [LEX-05.md](LEX-05.md) | tree-sitter parse on search side | `SymbolRecordDecoder` trait — decodes `UpsertSymbol.symbol` (producer-extracted `SymbolRecord`). No tree-sitter dep. |
| [STR-01.md](STR-01.md) | tree-sitter parse on search side | `UpsertParseTree { tree: ParseTreeRecord }` op (proposed). Search plane traverses producer-supplied trees. **Option A** = v1 ship with producer agreement; **Option B** = v2 deferral. |
| [LEX-07.md](LEX-07.md) | search-plane git access / commit graph self-author | `UpsertCommit { commit: CommitRecord }` + `UpsertRef` + `UpsertTag` ops (proposed). `CommitGraph.add_commit` is a channel-subscriber callback. |
| [RT-01.md](RT-01.md) | separate `apply_changes` IPC (violates §11 rule 6) | `UpsertDirty` + `EvictDirty` ops (proposed). `DirtyBuffer.apply` / `.evict` are channel-subscriber callbacks. Drops ADR-017 (advisory lock) — channel monotonic seq replaces it. |

**SSOT update** ([channel-architecture.md §3.1](../../../ssot/channel-architecture.md)):
- **Shipped** ops (already in contract): `FullBundle`, `UpsertChunk`, `DeleteChunk`, `UpsertSymbol`, `DeleteSymbol`, `Seal`, `UpsertEmbedding`, `DeleteEmbedding`.
- **Proposed, pending producer agreement**: `UpsertCommit`, `UpsertRef`, `UpsertTag`, `DeleteRef`, `DeleteTag`, `UpsertDirty`, `EvictDirty`, `UpsertParseTree`, `DeleteParseTree`.

**Code impact** (16 LQ crates already on disk): trait + data shapes preserved. Only naming intent shifts:
- `lq-symbol` `SymbolExtractor` → reinterpreted as `SymbolRecordDecoder` (input = wire bytes, not source bytes)
- `lq-structural` `StructuralMatcher::match_pattern(pattern, source)` → reinterpreted as `match_pattern(pattern, parsed_tree)` (input = decoded `ParsedTree`)
- `lq-history` `CommitGraph::add_commit/ref/tag` → reinterpreted as channel-subscriber callbacks
- `lq-runtime` `DirtyBuffer::apply` → reinterpreted as channel-subscriber callback (drop `apply_changes` IPC framing)
- `lq-text-norm` `detect_lang(path)` → still valid; could be eliminated if producer ships `lang` in chunk metadata (open)

### 3.7 New ambiguities surfaced during the correction pass

Filed by the 4 correction agents. Need resolution before integration:

| ID | Description | Owner |
|---|---|---|
| AMB-PROD-1 | Producer commit emission ordering (topological? buffered?) | producer-handoff doc |
| AMB-PROD-2 | `CommitRecord` full wire shape (parents, applied_at_ms, author, committer, message, is_merge, tags, ...) | `quanta-index-contract::channel` pin |
| AMB-PROD-3 | `DeleteCommit` op absent — force-push handling unclear | producer-handoff doc |
| AMB-PROD-4 | **Diff hunk authorship gap** — LEX-07 §4.5 needs `diff_hunks/` sibling, but channel has no `UpsertDiffHunk` op | proposed: add `UpsertDiffHunk` op OR inline in `UpsertCommit.commit.hunks` |
| AMB-PROD-5 | `SymbolRecord` wire shape ownership (producer ADR + search-plane bump policy) | contract pin |
| AMB-PROD-6 | `ParseTreeRecord` wire shape + version field policy | contract pin |
| AMB-PROD-7 | Producer `UpsertDirty` emission cadence (per-edit? batched? debounce?) | producer-handoff doc |
| AMB-PROD-8 | `EvictDirty` vs `UpsertDirty` ordering at same doc_id — channel seq monotonicity is authority but producer contract should guarantee it | producer-handoff doc |
| AMB-PROD-9 | Channel WAL retention horizon vs RT-01 TTL (300s default) — crash-recovery completeness | channel-arch SSOT §4.3 amendment |
| AMB-PROD-10 | `DIRTY_BAD_IDENTITY` validation timing (sync at apply vs async eventually-consistent) | RT-01 §8 amendment |
| AMB-PROD-11 | Q-STR-01-OPTION — A (v1 with `UpsertParseTree`) or B (v2 deferral)? | integrator wave-entry |
| AMB-PROD-13 | `DeleteChunk` cascade silent on semantic shard — Q-RFC-SEM-02-3 surfaced the gap in [producer-handoff.md §3.5.2](../../../ssot/producer-handoff.md) | **RESOLVED** via §3.5.2 amendment (this round) + Round 7a HNSW delta API landing (`SemanticIndexBuilder::remove_embedding(doc_id)`); embedding wire identity is `chunk_id` per §3.5.1 |
| AMB-PROD-14 | `SemanticVectorRef::Handle` server-side storage model — Round 6b shipped `LqExprExt::SemanticVector { vector_ref: SemanticVectorRef::{Inline \| Handle}, top_k }` but the handle resolution surface is unpinned | **DEFERRED** to [ADR-026](../implementation-plan.md) (recommendation: Option B = handle == `embedding_id`); typed error `SEM_HANDLE_NOT_FOUND` wired in [producer-handoff.md §6.5](../../../ssot/producer-handoff.md) |

### 3.8 Stale references in corrected specs (follow-up pass needed)

The correction was targeted (key sections only) to keep diffs reviewable. Stale references remain in:

- `LEX-05.md` §6 (test names like `symbol_per_lang::*`), §7 (`lq_symbol_extract_ms` metric, `lq.build.symbol.extract` span), §9 (per-file extract perf rows) — should be reworded to "per-record apply" form.
- `LEX-05.md` §11 row 22 (Sourcegraph claim-discipline) — semantics narrow to "given equivalent extraction".

Follow-up: a tidy-up PR aligning §6/§7/§9 of LEX-05 with the new architecture (low-risk, no behavior change).

### 3.9 Delta contract lock (Round 5, 2026-05-25)

The delta-handling contract for every `Upsert*` / `Delete*` op in the channel
catalogue (per [channel-architecture.md §3.1](../../../ssot/channel-architecture.md)) was locked in
[producer-handoff.md §3.5](../../../ssot/producer-handoff.md) as part of the Round-5 builder-hardening
work. The lock covers:

- per-record identity rules (wire identity vs search-side shard identity per §3.5.1)
- delete cascade graph — `DeleteChunk` cascades to trigram + positions + scorer + symbol + structural; `DeleteSymbol` does not cascade; no `DeleteCommit` (force-push = fresh generation per §3.1.3)
- replay safety — at-least-once delivery, apply-then-ack atomic order per §3.5.3
- in-generation last-write-wins under strict channel seq order per §3.5.4
- cross-generation `FullBundle` reset vs `from_prior(...)` delta inherit (Round-5 builder API) per §3.5.5
- ordering — channel seq monotonicity is sole authority; history track topological emission with `CommitGraph::with_buffering` fallback per §3.5.6

Round-5 builder hardening (search-side) lands `upsert_X`, `remove_X`, and
`from_prior(...)` APIs on the 4 affected LQ builders (trigram, positions,
symbol, scorer) so they are idempotent under §3.5.3 replay and support §3.5.5
cross-generation delta inherit. The contract section is normative; the builder
implementations are the search-side enforcement of it.

#### Audit findings (cross-doc consistency)

| Doc | Line(s) | Finding | Disposition |
|---|---|---|---|
| [rfc.md](../rfc.md) | 350 ("source of truth: apply-changes outputs plus snapshot/invalidation catalog"), 384 ("ingest changed file set from apply-changes / repo delta authority") | Stale `apply-changes` terminology in §Canonical Read Pipeline / §Canonical Incremental Write Pipeline; the canonical input is now producer-emitted `Upsert*` / `Delete*` channel ops per [producer-handoff.md §3.5](../../../ssot/producer-handoff.md). | **RESOLVED 2026-05-25**: rfc.md §Runtime metadata engine L350 rewritten to `LexicalChannelOp::UpsertDirty` / `EvictDirty` + snapshot catalog cross-link; §Canonical Incremental Write Pipeline step 1 rewritten to `BundleChannelSubscriber::next_event` with producer-handoff §3.5 / channel-architecture §3.1 / INDEX §3.6 cross-links. |
| [rfc.md §Canonical Incremental Write Pipeline](../rfc.md) | 380–410 (steps 1–8 + Atomicity contract) | Steps describe the search-side mutation pipeline but predate the producer/search split. Steps 1 ("ingest changed file set from apply-changes") and 7 ("derive semantic delta packets from changed chunk/doc identities") implicitly assume search-side authorship; producer-handoff §3.5.5 mode (b) replaces step 7 with channel-driven `Upsert*` against an open prior gen. | **RESOLVED 2026-05-25**: top-of-section NOTE added stating producer-authorship inversion per INDEX §3.6; step 1 reframed to `BundleChannelSubscriber::next_event`; step 7 reframed to `SemanticChannelOp::UpsertEmbedding` / `DeleteEmbedding` with `DeleteChunk` cascade cross-link to producer-handoff §3.5.2. Atomicity contract (linearization point + sibling MARKER_OK invariant) preserved — orthogonal to authorship axis. |
| [tickets/LEX-04.md](LEX-04.md) §3 Inputs | full §3 | LEX-04 is the regex-executor spec sheet; its §3 inputs are query-shaped (`LqQueryV1` + `RawRegex` + `PlanContext`), not delta-shaped. No delta-semantics reflection needed — LEX-04 is downstream of the delta-applying builders. The "incremental lexical indexing kernel" RFC roll-up is satisfied across LEX-02 / LEX-03 / LEX-05 / LEX-04 spec sheets per [§1.1](#11-spec-sheet--rfc-roll-up-map). | No edit. Delta semantics are owned by the upstream builders (trigram / positions / symbol / scorer), not by the regex executor; LEX-04 consumes the trigram authority after deltas have been applied. |

These follow-ups do not block Round-5 builder hardening; they are doc-debt
items against the post-correction RFC text.

---

## 4. Execution sequencing (canonical order)

This supersedes implementation-plan.md §4 wave-by-wave plan **only where renumbering applies**. The wave order itself is unchanged.

```
Wave 0 (prerequisite):
    PRE-CONTRACT-EXT  →  PRE-NORM  →  PRE-CONF
    (sequential; PRE-NORM needs ErrorCode from PRE-CONTRACT-EXT;
     PRE-CONF needs the parser from PRE-NORM)

Wave 1:
    LEX-00 (normalizer)        ↘
                                LEX-01 (scorer) — Wave-1 exit gate
    [RFC-LEX-02 surface cutover — not yet specced; must be authored]

Wave 2:
    LEX-02 (trigram)           ↘
                                LEX-03 (positions) — Wave-2 exit gate
    [RFC-LEX-03 lexical authority unification — not yet specced]

Wave 3:
    LEX-04 (regex executor) — needs trigram from LEX-02
    LEX-05 (symbol shard)
    [RFC-LEX-04 incremental indexing kernel rollup proof — emerges from
     LEX-02+LEX-03+LEX-04+LEX-05 closure]
    [RFC-LEX-05 parallel executor + merge — not yet specced]

Wave 4:
    LEX-06 (ranker) — needs scorer from LEX-01
    LEX-07 (history + gen governance) — needs gen-pin clean state

Wave 5:
    STR-01 (structural)        →  RT-01 (dirty:)
    (STR-01 should ship before RT-01 so the apply-changes channel
     can carry structural deltas)

Wave 6:
    SEM-01 (semantic adapter) — needs SearchExplanation v1
    BRIDGE-01 (Sourcegraph bridge)
    [RFC-BRIDGE-01 CodeQL bridge — not yet specced]

Wave 7:
    SEM-02 (hybrid fusion) — needs SEM-01 + LEX-06
    [RFC-SEM-02 incremental semantic derivatives — not yet specced]

Wave 8:
    OBS-01 (observability + SLO) — cross-cutting, instrumentation
                                    must be present from Wave 1 onward;
                                    Wave-8 finalizes SLO gates and
                                    closes the conformance proof
```

---

## 5. Action items (must close before any ticket execution)

| # | Action | Owner |
|---|---|---|
| 1 | Resolve `G-CONTROL-LOC` (working-tree divergence). Pick the channel-architecture refactor or revert to `quanta-index-control` | Repo lead |
| 2 | Decide ticket-ID strategy: rename spec files `SPEC-*` OR amend RFC §Ticket Pack | RFC author |
| 3 | Author the 5 missing RFC roll-up specs (RFC-LEX-02, RFC-LEX-03, RFC-LEX-05, RFC-SEM-02, RFC-BRIDGE-01-CodeQL) | spec author |
| 4 | Merge all 14 `RFC-GAP-*-CODES` into a single PRE-CONTRACT-EXT contract bump | contract owner |
| 5 | Renumber ADR slots in implementation-plan.md §10 to resolve 4-way ADR-017 + 3-way ADR-018 collisions | impl-plan owner |
| 6 | Land additive UC-* rows (39 new + 5 status promotions) into usecase.md | usecase.md owner |
| 7 | Reconcile §3.4 inter-sibling-doc conflicts (8 items) | each doc's owner |
| 8 | Update implementation-plan.md §5 DoD rows where spec sheets refined the scope | impl-plan owner |
| 9 | After items 1–8 settle, run full doc-paths lint and conformance corpus dry-run | CI |
| 10 | Land `SEM_HANDLE_NOT_FOUND` typed error in [`crates/quanta-index-lq-semantic/src/errors.rs`](../../../../crates/quanta-index-lq-semantic/src/errors.rs) (currently absent) and wire it into the semantic resolver per [ADR-026](../implementation-plan.md) Option B; gated on Round 7a HNSW delta API landing | LQ-semantic agent |

---

## 6. Hard requirements honored by every spec sheet

- **No silent failure / no silent fallback** — every cap surfaces `PLAN_LIMIT_EXCEEDED` or a typed code; no auto-skip
- **No serde proc-macro derives** (D18 — semgrep `rust-no-serde-derive`) — every contract-touching ticket mandates hand-rolled `impl Serialize / impl Deserialize`
- **Provable DoD** — every checklist item ties to a command (test path, criterion bench, lint script) + expected output
- **Path-agnostic for control plane** — spec sheets reference `<control-plane>` abstractly while G-CONTROL-LOC pends
- **Breaking-first posture** (CLAUDE.md `Agent change posture`) — no long-lived compatibility shims
- **Markdown link syntax throughout** — every code reference uses inline markdown links rather than backtick-only paths or bare URLs
