# LEX-05 — Symbol Index (definitions + references)

> Status: `shipped (architecture-corrected)`
> Crate: `quanta-index-lq-symbol`
> Tests: 51
> Last verified: 2026-05-25
> Wave: 3 (symbol shard authority lands as a sibling under the lexical content authority unified in Wave 2 (RFC LEX-03); the dedicated symbol planner + name/kind/file pushdown lane is Wave 3 work).
> Parent RFC: [../rfc.md](../rfc.md) § LQ/Core-1.0 `type:symbol`, § Engine Decomposition § Lexical content engine, § Recommended Concrete Engine Choices § Lexical content/path/symbol, § Non-Negotiable Invariants, § Error Code Taxonomy, § Claim Discipline.
> Sibling docs: [../feature-scope.md](../feature-scope.md), [../usecase.md](../usecase.md), [../dsl.md](../dsl.md), [../implementation-plan.md](../implementation-plan.md).
> Posture: **breaking-first** — no long-lived shims, no dual surface, no heuristic success path. Per [../../../../CLAUDE.md](../../../../CLAUDE.md) § Agent change posture.
>
> **Architecture correction:** tree-sitter dropped. Trait reinterpreted as `SymbolRecordDecoder` consuming `UpsertSymbol.payload` from the channel. The search plane never parses source. §6 / §7 / §9 aligned to per-record apply form (2026-05-25 cleanup pass closing [INDEX.md](INDEX.md) §3.8).

---

## §1 Purpose

Land a symbol authority (definitions + references) as a Tantivy sibling shard under the manifest-first generation set, consuming producer-supplied `SymbolRecord` payloads carried over `LexicalChannelOp::UpsertSymbol` (per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §3.1), exposed through the search-plane via `type:symbol` queries plus `kind:`, `file:`, `repo:`, `lang:`, `case:`, and `/.../` regex projections over the symbol name index. Specifically:

1. Index symbol **definitions** (function, class, method, variable, macro, ...) into a Tantivy symbol sibling under the existing manifest generation contract (per [../rfc.md](../rfc.md) § Generation model § symbol shard generation). The (name, kind, span, lang, parent) tuple is **already extracted by the producer** and arrives as `UpsertSymbol.symbol: SymbolRecord` — search plane decodes and indexes, never parses source.
2. Index symbol **references** (call sites, type usages, identifier read sites) using the same authority schema, sourced from the same `UpsertSymbol` op (the producer distinguishes definition vs reference inside the `SymbolRecord` shape). The boundary against semantic resolution (true import-resolved definition-of and reference-to) is **locked** below (§3.5) — this lane does **not** resolve cross-file definition identity; that belongs to SEM-01 / BRIDGE-* / cross-ref planner family (RFC § Planner Model item 5).
3. Materialize a `SymbolKind` enum (closes **GAP-01** from [../usecase.md](../usecase.md) §3) wired into the contract crate by `PRE-CONTRACT-EXT` (per [../implementation-plan.md](../implementation-plan.md) §5.1), then consumed by this ticket's executor. The producer emits one of these enum values per record; the search plane validates and stores.
4. Surface `type:symbol kind:<kind>` queries by the symbol kind enum (closes UC-SYM-02 contract gap).
5. Plan and execute regex over symbol names (closes UC-SYM-04: `type:symbol /^Bar/`).
6. Fail closed on malformed producer payloads with typed `SYMBOL_PAYLOAD_DECODE_FAIL` / `SYMBOL_RECORD_INVALID` (no silent skip — per [../rfc.md](../rfc.md) § Non-Negotiable Invariants §1, §3). Language is an opaque attribute on the record — search plane does not gate indexing on language coverage.
7. No per-language v1 cut. Language coverage is a producer concern; the search plane stores whatever `lang` value the producer emits and exposes it via the `lang:` filter. See §3.4.
8. Incremental update boundary (per-record re-index granularity) is **locked** below (§3.6) — a single `UpsertSymbol` op affects only the addressed symbol's document set in the symbol sibling, per [../rfc.md](../rfc.md) § Canonical Incremental Write Pipeline §2.

This ticket is the only owner of symbol indexing in the LQ kernel. Tantivy's pre-existing `symbol` field (per [../implementation-plan.md](../implementation-plan.md) §2.4) is **subsumed** by this lane; no parallel path may serve `type:symbol` queries after Wave 3 exit.

---

## §2 Background

### 2.1 What exists today

- Tantivy chunk index ([../../../../crates/quanta-index-lexical/src/](../../../../crates/quanta-index-lexical/src/)) has a `symbol` field. State per [../implementation-plan.md](../implementation-plan.md) §2.4: "**partial** — symbol field exists but no dedicated symbol planner".
- No `SymbolKind` exists in the contract crate. `LexicalCandidate` ([../../../../crates/quanta-index-contract/src/results/candidates.rs](../../../../crates/quanta-index-contract/src/results/candidates.rs)) has no `symbol_kind` field. This is **GAP-01** in [../usecase.md](../usecase.md) §3 and is resolved by `PRE-CONTRACT-EXT` (Wave 0); LEX-05 is the consumer.
- Per [../implementation-plan.md](../implementation-plan.md) §2.2, the historical `SearchPlaneLexicalIndexBuildPort` / `SearchPlaneLexicalIndexStorePort` cover a single Tantivy generation; multi-sibling extension belongs to LEX-03 (RFC ticket) and this ticket's contract.
- The channel surface already ships `LexicalChannelOp::UpsertSymbol { repo, revision, generation, symbol: SymbolRecord }` and `DeleteSymbol` (per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §3.1). The producer `semantica-codegraph-v2` is the sole author of `SymbolRecord`; this ticket is the sole consumer on the search-plane side.
- No ctags / tree-sitter / scip integration exists in this repo, **and none lands here**. Per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §0 (Out-of-scope) and §3.1 authorship rule, symbol extraction is producer-owned. The search plane never parses source.
- Sourcegraph reference (per [../feature-scope.md](../feature-scope.md) § Sourcegraph compatibility delta) uses Zoekt-style symbol indices keyed by in-process ctags extraction. We **diverge structurally**: extraction is upstream in the producer; the search plane is a pure decode + index + query plane (§4.8).

### 2.2 Why "Wave 3"

This ticket depends on:
- `PRE-CONTRACT-EXT` (`SymbolKind`, contract extension for `kind`) — Wave 0.
- LEX-01 parser owning `kind:` filter grammar — Wave 1.
- LEX-03 sibling shard model and manifest-first atomicity assertion — Wave 2.
- Producer `semantica-codegraph-v2` shipping `UpsertSymbol` with a well-defined `SymbolRecord` wire shape — entry-gate dependency tracked in §12 Q-LEX05-1.

At Wave 3 entry, all of those are green; LEX-05 fills the actual symbol authority surface on the search-plane side.

### 2.3 Vendor decision

The historical ADR-002 slot ("Symbol shard storage layout + extractor") collapses. There is **no extractor choice on the search plane** — extraction is producer-owned. ADR-002 is narrowed to **storage layout only** (Tantivy schema for the symbol sibling). The proposed ADR-018 slot ("Symbol extraction vendor and `tags.scm` curation policy") is **withdrawn** for this repo; the equivalent ADR belongs to `semantica-codegraph-v2`. See §4.8 and §12 Q-LEX05-1.

### 2.4 Reference/def relationship boundary

[../usecase.md](../usecase.md) UC-SYM-01 talks of `(repo_relative_path, start_line)` pointing to "definition". UC-SYM-* rows make no claim about resolving cross-file `def → ref` edges. We **lock** the boundary here:

- LEX-05 indexes raw **lexical** facts: "this file at this line declares a symbol with this name and this kind"; "this file at this line references a symbol with this name".
- LEX-05 does **not** resolve which definition a reference points to. That is import resolution / scope resolution and belongs to SEM-01 / SEM-02.
- LEX-05 does **not** index call edges, import edges, or any graph.
- Cross-ref planner family ([../rfc.md](../rfc.md) § Planner Model item 5: `def:`, `ref:`, `export:`, `import:`) **consumes** this authority but lands later (post-Wave-7).

This locks LEX-05 inside the lexical layer per RFC § Non-Goals ("lexical layer does not perform callgraph, dataflow, taint, PTA, or semantic reasoning").

---

## §3 Inputs

### 3.1 Indexing-time inputs

- `LexicalChannelOp::UpsertSymbol { repo, revision, generation, symbol: SymbolRecord }` events streamed by the producer through the lexical channel (per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §3.1). The op's CBOR body is the **sole** input to this lane's build path.
- `LexicalChannelOp::DeleteSymbol { repo, revision, generation, symbol_id }` for revocations.
- `LexicalChannelOp::Seal { repo, revision, generation }` to flip the symbol sibling's ledger entry to `materialized=true` (per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §5.2).
- `SymbolRecord` shape (producer-owned, decoded here): at minimum `{ name: Box<str>, kind: SymbolKind, span: { path: Box<str>, byte_start: u64, byte_end: u64, line_start: u32, line_end: u32 }, lang: Box<str>, relationship: enum { definition, reference }, parent: Option<SymbolRef>, container_name: Option<Box<str>> }`. The producer-side wire-shape ADR governs additions; search plane decodes via a versioned schema (§12 Q-LEX05-1).
- **No source bytes, no grammar handle, no `tags.scm`**. The search plane never reads source.

### 3.2 Query-time inputs

- A `LqQueryV1` with `LqFilter::Type(TypeFilter::Symbol)` (per [../dsl.md](../dsl.md) §6.4).
- Optional `LqFilter::Kind(SymbolKind)` — new filter variant landed by `PRE-CONTRACT-EXT` (per [../implementation-plan.md](../implementation-plan.md) §5.1 GAP-01).
- Optional pattern leaf: `Keyword`, `Phrase`, or `Regex` (per [../dsl.md](../dsl.md) §3.1–§3.4).
- Optional `file:`, `repo:`, `lang:`, `rev:`, `case:`, `count:`, `timeout:` filters per [../dsl.md](../dsl.md) §6.2.
- `PlanContext { tenant_id, user_id, generation_set }`.

### 3.3 SymbolKind enum (`SymbolKind`)

The enum, landing in `PRE-CONTRACT-EXT` and consumed by LEX-05:

```
function | method | class | struct | enum | enum_member | interface | trait
| module | namespace | macro | type_alias | constant | variable | field
| property | parameter | local_variable | union | impl | typedef
```

Total: 20 values. Hand-rolled serde (D18 ban applies). The producer emits exactly one of these values per `SymbolRecord`; the search plane validates against the enum at decode time and rejects with `SYMBOL_RECORD_INVALID{field=kind}` on mismatch. No silent re-tagging.

Mapping from producer-internal capture vocabulary to `SymbolKind` is performed inside the producer; the search plane sees only the post-mapping enum value.

### 3.4 Language coverage

Language is an opaque `Box<str>` attribute on each `SymbolRecord` (e.g. `"rust"`, `"python"`, `"go"`, `"typescript"`, `"tsx"`, `"javascript"`, `"java"`, ...). The search plane:

1. Stores `lang` as a faceted Tantivy field.
2. Exposes it under the `lang:` filter (per [../dsl.md](../dsl.md) §6.3) without enumeration gating.
3. Never rejects a record for "unsupported language" — language coverage is a **producer concern**. If the producer doesn't ship Ruby symbols, queries for `lang:ruby type:symbol` return empty; if it does, they're indexed and queryable.

The historical v1 cut (Rust / Python / Go / TS / JS) and its `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED` rejection path are **withdrawn from this ticket**. They migrate to the producer-side language ADR. The search plane retains the `lang:` enum in [../dsl.md](../dsl.md) §6.3 as a parse-time validity gate (the DSL grammar still pins accepted filter values), but the symbol shard itself is language-agnostic.

### 3.5 Reference/def boundary lock

| In scope (this ticket) | Out of scope (downstream) |
|---|---|
| "file X line N declares `Foo` as a `class`" | "this `Foo` reference at line N resolves to the definition at file Y line M" |
| "file X line N references the identifier `Foo`" | "this call edge connects `Foo()` to its target" |
| Local-file lexical position of every captured symbol | Cross-file import graph |
| `kind:function` filter against the symbol name index | "give me every function called `foo` that is exported from package `bar`" |
| Regex over symbol names | Type unification, scope resolution |

Downstream owners: SEM-01 (Wave 6, lexical-universe pushdown then semantic resolution), SEM-02 (Wave 7, incremental semantic derivative), cross-ref planner family (post-Wave 8). RFC § Planner Model item 5.

### 3.6 Incremental update boundary lock

A single-file edit causes **per-record delta only**:

1. Producer emits one `UpsertSymbol` op per added/changed symbol and one `DeleteSymbol` per removed symbol (per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §3.1). Whole-file replacement is the producer's choice, not the search plane's invariant.
2. Search-plane's channel dispatcher decodes each op via `SymbolRecordDecoder` and applies it to the symbol sibling builder.
3. The Tantivy symbol shard receives delete-then-insert for each affected symbol document (no re-extraction — the producer already shipped the post-edit state).
4. Generation `manifest_generation` advances per the producer's `Seal` op cadence; sibling `symbol_generation` advances atomically with manifest generation.
5. Symbols not in the producer's delta stream are untouched.

Wave-3 entry gate: this incremental boundary is exactly what RT-01 / LEX-07 (the wave-5 / wave-4 RFC-tickets) consume. The write-packet trace ([../implementation-plan.md](../implementation-plan.md) §5.8) asserts `O(upserts + deletes)` for the symbol sibling — same proof rail, now sourced from the channel op stream rather than from in-process extraction.

---

## §4 Deliverables

### 4.1 Code surface

A new module set under `crates/quanta-index-lexical/src/symbol/`:

1. `symbol/schema.rs` — Tantivy schema for the symbol shard: fields `repo_id`, `revision_id`, `manifest_generation`, `lang` (faceted, free-form string), `repo_relative_path`, `start_line`, `end_line`, `start_byte`, `end_byte`, `name` (analyzed), `name_raw` (raw, for regex), `kind` (faceted), `relationship` (enum: `definition` | `reference`), `container_name` (optional, for nested symbols).
2. `symbol/decode.rs` — `SymbolRecordDecoder` trait + default CBOR-canonical impl. Decodes `UpsertSymbol.symbol` bytes into a typed `Symbol` value; surfaces `SYMBOL_PAYLOAD_DECODE_FAIL` and `SYMBOL_RECORD_INVALID` (§8).
3. `symbol/builder.rs` — `SymbolIndexBuilder` accumulates already-extracted `Symbol` values from the channel stream; `finish()` produces a `SymbolIndex` (Tantivy segment + reader handle).
4. `symbol/store.rs` — per-generation reader cache (mirrors the existing chunk-index reader-cache pattern per [../implementation-plan.md](../implementation-plan.md) §2.4 row "LEX-07 generations governance").
5. `symbol/query.rs` — planner pushdown + Tantivy query construction: `kind:` → faceted filter; pattern leaves → `TermQuery` / `PhraseQuery` / `RegexQuery` over `name` / `name_raw`; `lang:` / `file:` / `repo:` propagated from outer filters via `lookup_by_name` / `lookup_by_kind` / `lookup_by_doc` API.
6. `symbol/relationship.rs` — surface for `relationship: definition | reference` projection (consumer of UC-SYM-* tests; cross-ref planner family will read this surface later).
7. Hand-rolled `impl serde::Serialize` / `impl serde::Deserialize` for every wire type — D18 ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) `rust-no-serde-derive`).
8. `MockSymbolRecordDecoder` (test-only) — deterministic in-memory decoder producing canned `Symbol` values for unit / integration tests, replacing the historical `MockExtractor`.

No language-specific code paths. No grammar handle. No `tags.scm`. No source-byte input.

### 4.2 Trait surface

Extending the sibling-shard port pattern landed by LEX-03:

- `SymbolRecordDecoder::decode(payload: &[u8]) -> Result<Symbol, SymbolDecodeError>` — the only seam between channel bytes and the symbol shard.
- `LexicalIndexBuildPort::apply_symbol_op(op: &LexicalChannelOp, builder: &mut SymbolIndexBuilder) -> Result<(), CoreError>` — channel-dispatched apply for `UpsertSymbol` / `DeleteSymbol`.
- `LexicalIndexStorePort::open_symbol_reader(generation: &PublishedGenerationSet) -> Result<SymbolReader, CoreError>`.
- `SymbolExecutor::execute(plan: &SymbolPlan, ctx: &PlanContext, options: &LqOptionSet) -> Stream<LexicalCandidate>`.

No new RPC surface beyond what `PRE-CONTRACT-EXT` already lands (the `kind:` filter variant and `SymbolKind` enum). No grammar / source-bytes parameters anywhere in the trait surface.

### 4.3 Contract additions (consumed, not defined here)

Landed by `PRE-CONTRACT-EXT`:
- `SymbolKind` enum (per §3.3).
- `LqFilter::Kind(SymbolKind)` variant.
- `LexicalCandidate.symbol_kind: Option<SymbolKind>` field (or `SymbolCandidate` sibling type — choice belongs to Q-UC-1 / Q-LEX05-1 below).

`SymbolRecord` itself is owned by `quanta-index-contract::channel` (per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §3.1 status list) and shared cross-repo with the producer.

### 4.4 Test surface

- `crates/quanta-index-lexical/tests/symbol_unit.rs` — per-error-code and per-kind unit tests, driven by `MockSymbolRecordDecoder`.
- `crates/quanta-index-lexical/tests/symbol_decode.rs` — CBOR decode round-trip and malformed-payload tests against the default `SymbolRecordDecoder`.
- `crates/quanta-index-lexical/tests/symbol_property.rs` — property tests (1k random valid `SymbolRecord` payloads → encode → decode → index → query → round-trip).
- `crates/quanta-index-lexical/tests/symbol_incremental.rs` — per-record delta proof driven by synthetic `UpsertSymbol` / `DeleteSymbol` op streams.
- `crates/quanta-index-lexical/benches/symbol_bench.rs` — `lex_05_symbol_apply_bench`, `lex_05_symbol_query_bench`.
- Conformance rows `usecase-corpus/UC-SYM-{01..06}.toml`, seeded via op-stream fixtures rather than source-file fixtures.

### 4.5 Docs

- ADR-002 (narrowed) — Symbol shard storage layout (Tantivy schema only; extractor scope removed). Forcing function: Wave 3 entry.
- Updates to `docs/handoffs/lq-contract-1.0.md` noting the symbol sibling shard schema, the contract enum, and the `UpsertSymbol` consumption path.
- Cross-link to producer-side wire-shape ADR (location TBD; tracked under Q-LEX05-1 in §12).
- **No** `tags.scm` files. **No** language-specific query files. **No** ADR-018.

### 4.8 Vendor decision

**No code parser dependency on the search plane.** Extraction (whatever vendor — tree-sitter, universal-ctags, scip, or producer-bespoke) lives in `semantica-codegraph-v2` and is governed by that repo's ADRs. The search-plane responsibility is **decode + index + query**:

| Concern | Owner |
|---|---|
| Source parsing, symbol extraction, kind classification, span computation, language detection | Producer (`semantica-codegraph-v2`) |
| Wire encoding of `SymbolRecord` (CBOR canonical) | Shared `quanta-index-contract::channel` |
| Wire decoding via `SymbolRecordDecoder` | This ticket |
| Tantivy schema, builder, reader cache | This ticket |
| Planner pushdown, query execution, candidate streaming | This ticket |

The historical "tree-sitter vs ctags vs scip" comparison table is **withdrawn** — the search plane is downstream of the choice and indifferent to it.

---

## §5 Implementation steps (TDD)

Each step is one PR, starting with a failing test.

### 5.1 Step 1 — `Symbol` value type + CBOR decode

Test: `symbol_decode::round_trip_canonical` encodes a `Symbol` to canonical CBOR, decodes via `SymbolRecordDecoder`, asserts byte-and-field identity. `symbol_decode::malformed_payload_typed_error` feeds truncated / non-CBOR bytes and asserts `SYMBOL_PAYLOAD_DECODE_FAIL`. `symbol_decode::missing_required_field` (e.g. no `name`) asserts `SYMBOL_RECORD_INVALID{field}`.

Implement: `symbol/decode.rs` with `SymbolRecordDecoder` trait + default CBOR-canonical impl. Hand-rolled serde per D18.

### 5.2 Step 2 — `SymbolDoc` schema + Tantivy field set

Test: `symbol_unit::schema_round_trip` writes one `SymbolDoc` to an in-memory Tantivy index and reads it back identical (hand-rolled serde guarantees zero proc-macro derives).

Implement: `symbol/schema.rs` per §4.1 list. `SymbolDoc` is the in-Tantivy projection of a decoded `Symbol`.

### 5.3 Step 3 — `SymbolIndexBuilder` accumulates decoded symbols

Test: `symbol_unit::builder_accumulates_then_finishes` — feed a sequence of `UpsertSymbol` ops (using `MockSymbolRecordDecoder`), call `finish()`, assert the resulting `SymbolIndex` contains the expected document set in deterministic order.

Implement: `symbol/builder.rs::SymbolIndexBuilder::{apply_upsert, apply_delete, finish}`. No language branches; the path is uniform across record values.

### 5.4 Step 4 — `MockSymbolRecordDecoder` + multi-language fixture

Test: `symbol_unit::mixed_lang_fixture` seeds a builder with synthetic `SymbolRecord` values spanning `"rust"`, `"python"`, `"go"`, `"typescript"`, `"javascript"` (and any other strings the test desires — language is opaque), asserts the resulting index queries return per-`lang` partitions correctly.

Implement: `MockSymbolRecordDecoder` returning canned `Symbol` values; reused across the rest of the test rail.

### 5.5 Step 5 — Malformed-record fail-closed

Test: `symbol_unit::invalid_record_fails_closed` — a `SymbolRecord` with an unknown `kind` value, an empty `name`, or a non-monotonic `span` returns `SYMBOL_RECORD_INVALID{field}` from the decoder, the builder rejects it, and the dispatcher does **not** ack the op (per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §4.6 corruption policy). No silent skip, no empty-document insert.

Implement: validation hook in `symbol/decode.rs::decode` after structural CBOR decode succeeds; the channel dispatcher's error path treats this as `Corrupted` per §4.6.

### 5.6 Step 6 — Sibling shard wiring under manifest

Test: `symbol_integration::manifest_first_atomicity` asserts that a partially-built symbol shard never becomes readable: simulate a crash mid-write, then attempt to open the symbol reader for that generation → `STATE_NOT_READY: STALE_SIBLING` (per RFC § Storage-layer enforcement).

Implement: `MARKER_OK` per symbol shard; reader-open path asserts presence before returning a reader. Hook into LEX-03's manifest-first contract.

### 5.7 Step 7 — Planner pushdown

Test: `symbol_unit::planner_routes_type_symbol`, `planner_filter_kind`, `planner_filter_lang`, `planner_pattern_keyword`, `planner_pattern_regex`. Each row uses a constructed `LqQueryV1` and asserts the produced `SymbolPlan` has the expected pushed filters.

Implement: `symbol/query.rs::plan(query: &LqQueryV1) -> Result<SymbolPlan, LexicalErrorCode>`. Reject combinations that violate RFC § Planner Model with typed `PLAN_UNSUPPORTED_COMBO`.

### 5.8 Step 8 — Executor + reader cache (`lookup_by_name` / `lookup_by_kind` / `lookup_by_doc`)

Test: `symbol_integration::uc_sym_01_bare_symbol`, `uc_sym_02_kind_function`, `uc_sym_03_file_path_scope`, `uc_sym_04_regex_name`, `uc_sym_05_case_yes`, `uc_sym_06_with_repo`. Each row uses an op-stream-seeded multi-language fixture and asserts the documented `LexicalCandidate` set, in deterministic order.

Implement: `SymbolExecutor::execute` with per-generation reader cache. Public query surface: `lookup_by_name(name, opts)`, `lookup_by_kind(kind, opts)`, `lookup_by_doc(doc_id)`.

### 5.9 Step 9 — Regex over symbol names

Test: `symbol_integration::uc_sym_04_regex_name_prefix` — `type:symbol /^Bar/` returns only symbols whose name starts with `Bar`. Cross-conformance with LEX-04 (this packet's regex executor): the regex compiler is shared; the symbol lane just narrows the field to `name_raw`.

Implement: route `LqExpr::Regex` against `name_raw` (raw, no analyzer) using the compiled `regex::Regex` from LEX-04. Inherit NFA-state cap from [../dsl.md](../dsl.md) §3.4.

### 5.10 Step 10 — Per-record incremental delta

Test: `symbol_incremental::per_record_delta_isolates` — seed an index with 10k synthetic `UpsertSymbol` ops, then apply a single `UpsertSymbol` (for an updated symbol) plus a single `DeleteSymbol`; assert the write-packet trace touches `O(1)` symbol documents (one delete + one insert pair) and the new generation matches the expected diff exactly.

Implement: hook into the channel-dispatched apply path that LEX-04 (the RFC ticket — incremental lexical indexing kernel) lands. Per-record delete-then-insert is the granularity asserted in §3.6.

### 5.11 Step 11 — Conformance + ADRs

Wire UC-SYM-01..06 golden files into `usecase-corpus/` (op-stream-seeded). Land ADR-002 (narrowed). Update `docs/handoffs/lq-contract-1.0.md`.

### 5.12 Step 12 — Criterion benches

Add `lex_05_symbol_apply_bench` (per-`UpsertSymbol` apply latency end-to-end through decoder + builder) and `lex_05_symbol_query_bench` (per-query end-to-end). Regression budget: p99 may not increase >5% per wave without an ADR.

---

## §6 Test plan

### 6.1 Unit (`cargo test -p quanta-index-lexical --test symbol_unit`)

| Test name | Asserts |
|---|---|
| `symbol_unit::schema_round_trip` | one `SymbolDoc` round-trips through Tantivy |
| `symbol_unit::decoder_accepts_canonical_cbor` | `MockSymbolRecordDecoder` accepts canonical CBOR payload, returns typed `Symbol` |
| `symbol_unit::invalid_record_fails_closed` | unknown `kind`, empty `name`, non-monotonic span → `SYMBOL_RECORD_INVALID{field}`; builder rejects |
| `symbol_unit::planner_routes_type_symbol` | `type:symbol Foo` plan has `SymbolPlan` with `pattern=Keyword("Foo")` |
| `symbol_unit::planner_filter_kind` | `type:symbol kind:function handler` plan has `kind=Function` |
| `symbol_unit::planner_filter_unknown_kind` | `type:symbol kind:not_a_kind handler` → `PARSE_INVALID_FILTER_VALUE{filter=kind}` |
| `symbol_unit::planner_filter_lang` | `type:symbol lang:rust Foo` plan has `lang=Rust` and prunes other-lang docs (free-form `lang` attribute filter) |
| `symbol_unit::planner_pattern_regex_uses_name_raw` | `type:symbol /^Bar/` plan binds regex to `name_raw` field |
| `symbol_unit::planner_unsupported_combo_type_symbol_into_codeql_diff` | `type:symbol into:codeql type:diff` mix → `PLAN_UNSUPPORTED_COMBO` |

### 6.2 Per-record apply (`cargo test -p quanta-index-lexical --test symbol_apply`)

Per-language testing is **not** a search-plane concern — extraction is producer-owned (§4.8). The apply rail exercises the channel-decoded record path, varying `lang` as an opaque attribute on the synthetic `SymbolRecord` payload rather than via per-grammar fixtures.

| Test name | Fixture | Asserts |
|---|---|---|
| `symbol_apply::single_record_round_trip` | one synthetic `UpsertSymbol` op carrying a canonical `SymbolRecord` | decoded `Symbol` indexed; `lookup_by_name` returns it |
| `symbol_apply::mixed_lang_attribute` | op stream with `lang ∈ {"rust","python","go","typescript","javascript","ruby"}` (opaque) | every record indexed; `lang:` filter partitions correctly without enumeration gating |
| `symbol_apply::malformed_payload_typed_error` | `UpsertSymbol.payload` truncated CBOR | `SYMBOL_PAYLOAD_DECODE_FAIL`; op not acked |
| `symbol_apply::delete_then_upsert_ordering` | `DeleteSymbol` then `UpsertSymbol` at same `symbol_id` | post-apply state matches upsert |
| `symbol_apply::kind_facet_population` | op stream covering all 20 `SymbolKind` values | each `kind:` filter returns its partition |
| `symbol_apply::container_name_preserved` | nested-symbol record with `container_name` populated | `container_name` round-trips through schema |

### 6.3 Integration (`cargo test -p quanta-index-lexical --test symbol_integration`)

| Test name | Asserts |
|---|---|
| `symbol_integration::uc_sym_01_bare_symbol` | UC-SYM-01: `type:symbol Foo` returns hits with `(repo_relative_path, start_line)` at the expected `definition` location |
| `symbol_integration::uc_sym_02_kind_function` | UC-SYM-02: `type:symbol kind:function handler` returns only `kind=function` symbols (closes GAP-01 wire-through) |
| `symbol_integration::uc_sym_03_file_path_scope` | UC-SYM-03: `type:symbol file:src/.* Bar` — path pre-filter then symbol scan |
| `symbol_integration::uc_sym_04_regex_name` | UC-SYM-04: `type:symbol /^Bar/` — regex over `name_raw`; cross-uses LEX-04 lane |
| `symbol_integration::uc_sym_05_case_yes` | UC-SYM-05: `type:symbol case:yes Foo` — case-sensitive; lower-cased `foo` does not match |
| `symbol_integration::uc_sym_06_with_repo` | UC-SYM-06: `type:symbol repo:^github\.com/q/.* parse_query` — repo fanout then symbol shard |
| `symbol_integration::manifest_first_atomicity` | partially-built symbol shard never reads — `STATE_NOT_READY: STALE_SIBLING` |
| `symbol_integration::cross_lang_fixture_isolation` | a query without `lang:` filter against a 5-language fixture returns hits across all 5 languages in deterministic merge order |
| `symbol_integration::reference_vs_definition_projection` | result set distinguishes `relationship=definition` from `relationship=reference` per the schema |

### 6.4 Incremental (`cargo test -p quanta-index-lexical --test symbol_incremental`)

Per-record granularity is the search-plane invariant. File-level grouping is producer-side; the channel ships an op per affected symbol.

| Test name | Asserts |
|---|---|
| `symbol_incremental::per_record_delta_isolates` | write-packet trace size = `O(upserts + deletes)` for a delta of `N` symbol ops against a 10k-record fixture |
| `symbol_incremental::delete_purges_record` | a single `DeleteSymbol` op removes exactly that symbol from the next generation |
| `symbol_incremental::rename_via_delete_then_upsert` | producer-emitted delete-old + insert-new pair lands as two ops; post-state matches the upsert |

### 6.5 Property (`cargo test -p quanta-index-lexical --test symbol_property`)

| Test name | Asserts |
|---|---|
| `symbol_property::canonical_hash_stable_symbol_queries` | proptest 1k random `type:symbol …` queries → normalize→print→normalize→hash identical across 2 architectures |
| `symbol_property::decoder_idempotent` | proptest 1k random valid `SymbolRecord` payloads → `decode(encode(decode(p))) == decode(p)` (round-trip idempotency) |
| `symbol_property::kind_enum_total` | every emitted `SymbolKind` enum value round-trips through hand-rolled serde without loss (load-time enforced) |

### 6.6 Criterion (`cargo bench -p quanta-index-lexical --bench symbol_bench`)

| Bench | Budget |
|---|---|
| `lex_05_symbol_apply_bench` | p99 < 5 ms per single `UpsertSymbol` op end-to-end (decode + index-apply); language-agnostic |
| `lex_05_symbol_query_bench` | p99 < 50 ms per single-repo symbol query (warm) |

### 6.7 Conformance (PRE-CONF; `cargo test -p quanta-index-contract --test lq_conformance`)

Rows: UC-SYM-01, UC-SYM-02, UC-SYM-03, UC-SYM-04, UC-SYM-05, UC-SYM-06.

UC-SYM-02 (`kind:function`) requires GAP-01 closure — `PRE-CONTRACT-EXT` must have landed. UC-SYM-04 (`/^Bar/`) requires LEX-04 (this packet's regex executor) green. Both are entry-gate dependencies for this conformance row going `ok`.

---

## §7 Observability

OpenTelemetry spans:

| Span | Attributes |
|---|---|
| `lq.exec.shard.symbol.plan` | `kind_filter: option<string>`, `lang_filter: option<string>`, `has_regex: bool`, `ticket_id="LEX-05"`, `wave_id` |
| `lq.exec.shard.symbol.read` | `docs_scanned: u64`, `kind_facet_size: u64`, `early_stop_reason: enum` |
| `lq.build.symbol.apply` | `lang: string` (opaque attribute from record), `payload_size_bytes: u64`, `op_kind: enum{upsert,delete}`, `apply_time_us: u64` |

Metrics:

- `lq_symbol_apply_ms` — histogram per apply op (decode + index), labels=`{ticket_id, wave_id, op_kind}`. Apply = decode + index; no per-language partitioning (language is producer-owned).
- `lq_symbol_query_ms` — histogram, labels=`{ticket_id, wave_id}`.
- `lq_symbol_record_invalid_total` — counter, labels=`{ticket_id, wave_id, error_code}`. Cardinality budget: closed enum of `SYMBOL_PAYLOAD_DECODE_FAIL` / `SYMBOL_RECORD_INVALID` codes from §8.
- `lq_symbol_shard_size_bytes` — gauge per `(repo, rev, generation)`, labels=`{repo_id, generation_id}`. Cardinality budget: per [../rfc.md](../rfc.md) § Metric schema, retention-bounded; see [../implementation-plan.md](../implementation-plan.md) §9.

Audit log: per [../rfc.md](../rfc.md) § Audit trail — `error_code?` carries `STATE_NOT_READY` / `PARSE_INVALID_FILTER_VALUE` / `PLAN_UNSUPPORTED_COMBO` when this lane rejects.

Per-wave OBS subset: Wave 3 entry has `lq.exec.shard.symbol.*` emitting (per [../implementation-plan.md](../implementation-plan.md) §9.1 Wave 3 row).

---

## §8 Error scenarios

| Scenario | Surface | Code | Where | Test |
|---|---|---|---|---|
| `kind:` value not in `SymbolKind` enum | parse | `PARSE_INVALID_FILTER_VALUE{filter=kind, value, allowed=[20 values]}` | parser | `symbol_unit::planner_filter_unknown_kind` |
| `UpsertSymbol.payload` bytes fail structural CBOR decode | apply | `SYMBOL_PAYLOAD_DECODE_FAIL{at_seq, reason}` | `symbol/decode.rs::SymbolRecordDecoder::decode` | `symbol_decode::malformed_payload_typed_error` |
| `SymbolRecord` decoded but a required field is missing / out of range (e.g. empty `name`, unknown `kind` enum, non-monotonic `span`) | apply | `SYMBOL_RECORD_INVALID{at_seq, field}` | `symbol/decode.rs` post-decode validation | `symbol_decode::missing_required_field`, `symbol_unit::invalid_record_fails_closed` |
| Corrupted producer op stream (crc / seq monotonicity) | apply | `ChannelError::Corrupted` → track marked `degraded` per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §4.6 | channel layer | covered by `quanta-index-channel` test suite (consumed here) |
| Symbol shard `MARKER_OK` absent at read | exec | `STATE_NOT_READY: STALE_SIBLING{sibling=symbol, manifest_gen, sibling_gen}` | reader open | `symbol_integration::manifest_first_atomicity` |
| Symbol generation regression (non-NULL → NULL or rollback) | indexing | `STATE_GENERATION_REGRESSION{kind=symbol, prev, observed}` | writer | covered by LEX-04 (RFC ticket) writer-coordinator |
| `type:symbol` + `match { ... }` | parse/plan | `PLAN_UNSUPPORTED_COMBO{combo="type:symbol, structural"}` | planner | unit |
| `type:symbol` + `into:codeql` + `type:diff` | parse/plan | `PLAN_UNSUPPORTED_COMBO{combo}` | planner | `symbol_unit::planner_unsupported_combo_type_symbol_into_codeql_diff` |
| Per-tenant fanout cap exceeded | exec | `PLAN_LIMIT_EXCEEDED{limit_kind=fanout}` | LEX-05 inherits | covered by RFC LEX-05 |

The historical `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED` row is **removed**. Language coverage is no longer a search-plane concern (§3.4); if the producer ships symbols for a language the search plane has no special-case treatment for, those symbols are still indexable and queryable — `lang` is an opaque attribute. The "unsupported language" concept moves to the producer side.

There is **no silent skip path** for malformed records. `SYMBOL_PAYLOAD_DECODE_FAIL` and `SYMBOL_RECORD_INVALID` block the offending op from being acked, per the channel corruption policy ([../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §4.6) — the producer is the single source of truth and must republish a valid op.

---

## §9 Perf envelope

| Dimension | Target | Source |
|---|---|---|
| Per-record apply (single `UpsertSymbol`, canonical payload) p99 | < 5 ms | CBOR decode + Tantivy doc insert; language-agnostic (extraction lives upstream) |
| Per-record apply (single `DeleteSymbol`) p99 | < 2 ms | symbol-id lookup + delete |
| Single-repo symbol query (UC-SYM-01 shape) p95 | < 250 ms | [../rfc.md](../rfc.md) § Latency SLOs warm |
| Single-repo symbol query cold p99 | < 1 s | as above |
| 100-repo symbol fanout p95 | < 2 s | [../rfc.md](../rfc.md) § Latency SLOs fanout |
| Symbol shard size relative to source size | ≤ 0.5× source bytes | budget driven by [../feature-scope.md](../feature-scope.md) §7 storage targets |
| Single-file delta incremental p99 | < 100 ms wall-clock for the symbol sibling | per [../implementation-plan.md](../implementation-plan.md) §4.4 LEX-04 (RFC ticket) bench rail |
| Symbol query memory soft cap | 256 MiB per query | [../dsl.md](../dsl.md) §13 |

Regression budget: criterion benches' p99 may not increase >5% per wave without ADR.

---

## §10 Risks

| ID | Description | Prob | Impact | Early-warning | Mitigation |
|---|---|---|---|---|---|
| R-LEX05-1 | Producer `SymbolRecord` wire shape drift (added field, renamed field, changed enum variant) breaks search-plane decode | M | H | `symbol_decode::round_trip_canonical` fails after producer bump | Versioned `SymbolRecord` schema with explicit `wire_version` field; search-plane decoder pins a min/max accepted range; cross-repo bump is gated by Q-LEX05-1 ADR (§12) |
| R-LEX05-2 | Producer language coverage gaps — producer ships no records for some language → end users see "empty results for `lang:java`" | M | M | `lq_symbol_query_zero_hits_total{lang}` rises | Surface coverage via a per-lang gauge `lq_symbol_lang_record_count{lang}` so operators can see what the producer is emitting; no search-plane fix path (this is a producer scope item) |
| R-LEX05-3 | `SymbolKind` enum drift between contract crate and producer-side mapping | M | H | producer emits an unknown `kind` value → `SYMBOL_RECORD_INVALID{field=kind}` spikes | Shared `SymbolKind` enum lives in `quanta-index-contract`; producer and search plane import the same definition; CI cross-repo lint blocks PRs that diverge |
| R-LEX05-4 | Symbol shard size grows superlinearly with reference indexing on a generated-code-heavy repo | M | M | `lq_symbol_shard_size_bytes` exceeds 0.5× source size | Cap per-record-stream size; emit a typed `PLAN_LIMIT_EXCEEDED{dimension=symbol-records-per-generation}` if the producer ships more than `N` symbols for a single generation; default `N=10_000_000` |
| R-LEX05-5 | Incremental update granularity larger than per-record (e.g. producer ships whole-generation refresh instead of per-symbol upserts) | L | M | `symbol_incremental::per_record_delta_isolates` red | Boundary lives in `semantica-codegraph-v2`; search plane accepts both per-record and bulk patterns but the bench rail asserts per-record is the steady-state |
| R-LEX05-6 | Reference indexing crosses the def/ref boundary by accident — e.g. by emitting "this reference resolves to that definition" edges | L | H | Code review of `relationship.rs` | Locked at §3.5; no edge-resolution code may land in this lane; CI lint disallows imports of `quanta-index-semantic` from `quanta-index-lexical` |

ADR slots: **ADR-002 (narrowed — storage layout only)**. Forcing function: Wave 3 entry (ADR-002). The historical ADR-018 ("Symbol extraction vendor and `tags.scm` curation") slot is **withdrawn** for this repo and migrates to `semantica-codegraph-v2` (Q-LEX05-1).

---

## §11 DoD (provable)

Each row is one provable artifact. All 22 active rows shipped under the corrected architecture (51 tests in `quanta-index-lq-symbol`). Row 7 stays withdrawn. The architecture correction (tree-sitter dropped; trait reinterpreted as `SymbolRecordDecoder` over `UpsertSymbol.payload`) is the load-bearing change that makes rows 3 / 4 / 6 / 14 / 17 provable on the search plane.

1. ✓ shipped — `SymbolKind` enum, `LqFilter::Kind`, and `LexicalCandidate.symbol_kind` (or `SymbolCandidate` sibling per Q-LEX05-2) land in `PRE-CONTRACT-EXT`; LEX-05 consumes them. Proof: `cargo test -p quanta-index-contract --test contract_round_trip` covers `SymbolKind`; downstream `symbol_unit::planner_filter_kind` validates wire-through.
2. ✓ shipped — `SymbolRecord` wire shape lives in `quanta-index-contract::channel`; producer and search plane both import the same definition. Proof: `cargo test -p quanta-index-contract --test channel_op_round_trip` covers `LexicalChannelOp::UpsertSymbol`.
3. ✓ shipped (architecture-corrected) — `SymbolRecordDecoder` trait + default CBOR-canonical impl land at `crates/quanta-index-lq-symbol/src/decode.rs`. The trait consumes `UpsertSymbol.payload`, not source bytes. Proof: `symbol_decode::round_trip_canonical` green.
4. ✓ shipped — `MockSymbolRecordDecoder` lands and is wired into the per-error-code and integration test rails. Proof: `symbol_unit::*` and `symbol_integration::*` consume it.
5. ✓ shipped — UC-SYM-01..06 conformance rows green in PRE-CONF, seeded via op-stream fixtures. Proof: `cargo test -p quanta-index-contract --test lq_conformance` + `usecase-corpus/UC-SYM-{01..06}.toml`.
6. ✓ shipped — Malformed `UpsertSymbol.payload` → `SYMBOL_PAYLOAD_DECODE_FAIL` end-to-end. Malformed semantic content (missing required field, invalid enum, non-monotonic span) → `SYMBOL_RECORD_INVALID{field}`. Neither acks the op. Proof: `symbol_decode::malformed_payload_typed_error`, `symbol_decode::missing_required_field`, `symbol_unit::invalid_record_fails_closed`.
7. — (withdrawn — there is no source-bytes path in the search plane; UTF-8 / syntax errors are producer-side concerns).
8. ✓ shipped — `kind:` filter rejects unknown kinds at parse time with `PARSE_INVALID_FILTER_VALUE`. Proof: `symbol_unit::planner_filter_unknown_kind`.
9. ✓ shipped — Per-record delta causes `O(1)` documents touched. Proof: `symbol_incremental::per_record_delta_isolates` with write-packet trace asserting size.
10. ✓ shipped — Symbol shard `MARKER_OK` enforced at read. Proof: `symbol_integration::manifest_first_atomicity` (consumer of LEX-03 storage-level assertion).
11. ✓ shipped — Reference vs definition projection works. Proof: `symbol_integration::reference_vs_definition_projection`.
12. ✓ shipped — Regex over symbol names uses `name_raw` field (no analyzer interference). Proof: `symbol_integration::uc_sym_04_regex_name` + `symbol_unit::planner_pattern_regex_uses_name_raw`.
13. ✓ shipped — Canonical hash stable across two architectures for symbol queries. Proof: `symbol_property::canonical_hash_stable_symbol_queries` on the CI x86_64 + aarch64 matrix.
14. ✓ shipped (architecture-corrected) — Decoder is total over the accepted `wire_version` range and rejects any out-of-range version with a typed error. Proof: `symbol_decode::version_range_enforcement`.
15. ✓ shipped — No `unwrap` / `unwrap_or` / `Result::ok` on production paths. Proof: clippy disallowed-methods rail green.
16. ✓ shipped — No `#[derive(Serialize|Deserialize)]` in this lane. Proof: semgrep `rust-no-serde-derive` green ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)).
17. ✓ shipped (architecture-corrected) — `core` crate does **not** import `tree-sitter`, any grammar crate, or any code-parser dependency. Search-plane crates contain no source-parsing code. Proof: CI hexagonal-boundary lint ([../../../../tools/ci/lint/lint-hexagonal-boundaries.py](../../../../tools/ci/lint/lint-hexagonal-boundaries.py)) green; workspace `cargo tree` shows no `tree-sitter*` dependency under any search-plane crate.
18. ✓ shipped — ADR-002 (narrowed — Tantivy storage layout for the symbol sibling) lands. Proof: file committed at `docs/adr/ADR-002-symbol-shard.md`. (ADR-018 slot withdrawn per §10 / Q-LEX05-1.)
19. ✓ shipped — Telemetry spans (`lq.build.symbol.apply`, `lq.exec.shard.symbol.*`) and metrics (`lq_symbol_apply_ms`, `lq_symbol_query_ms`, `lq_symbol_record_invalid_total`, `lq_symbol_shard_size_bytes`) declared in §7 emit with the closed attribute set. Proof: integration test asserting span attribute keys; metric scrape asserting cardinality budget.
20. ✓ shipped — Bench `lex_05_symbol_apply_bench` and `lex_05_symbol_query_bench` p99 within budgets stated in §9 (channel-decode + index-apply costs). Proof: criterion CSV in CI artifacts.
21. ✓ shipped — Per-wave OBS subset met: Wave 3 entry has `lq.exec.shard.symbol.*` and `lq.apply.symbol.*` emitting. Proof: cross-reference [../implementation-plan.md](../implementation-plan.md) §9.1 Wave 3 row.
22. ✓ shipped — RFC § Claim-Discipline §1 — `Sourcegraph-compatible lexical core` — narrowed for the symbol leg to: **given equivalent extraction (producer parity assumed), our query plane returns equivalent candidates** for UC-SYM-01..06. Extraction parity itself is a producer-side concern (`semantica-codegraph-v2`), outside this lane. Full claim awaits Wave-8 OBS-01 conformance gate.
23. ✓ shipped — Structured agent output for this ticket validates against [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json). Missing evidence → `blocked`, not `ok`.

---

## §12 Open questions

| Q-ID | Question | Source | Blocking |
|---|---|---|---|
| Q-LEX05-1 | (cross-repo wire-shape ownership) Who owns `SymbolRecord` wire-shape versioning, and what is the bump policy? **Default answer**: the producer (`semantica-codegraph-v2`) authors an ADR per shape revision; the search plane pins a `[min_wire_version, max_wire_version]` range in `quanta-index-contract::channel`; bumping the max requires (a) a producer ADR landing first, (b) a coordinated search-plane PR that updates the decoder + adds a `symbol_decode::version_<N>` round-trip test, (c) a backfill plan for in-flight WAL segments that still carry the old shape. Forces: cross-repo coordination; entry-gate for this ticket's decoder rail. |
| Q-LEX05-2 | (closes GAP-01) Does `LexicalCandidate` gain a `symbol_kind: Option<SymbolKind>` field, or do we introduce a sibling type `SymbolCandidate`? **Default answer**: `Option<SymbolKind>` on `LexicalCandidate` — keeps response shape uniform; PRE-CONTRACT-EXT is the deciding ticket. Forces: Wave 0 entry (PRE-CONTRACT-EXT). |
| Q-LEX05-3 | Does the symbol authority index **references** in v1, or only **definitions**? **Default answer**: both, but with the boundary lock at §3.5 — references are local lexical positions only, never resolved cross-file. The choice is the producer's; search plane indexes whatever `relationship` value the record carries. Forces: producer ADR; affects shard size. |
| Q-LEX05-4 | Are nested symbols (e.g. Python `class Foo: def bar(): ...`) flattened or hierarchical? **Default answer**: flattened with a `container_name: Option<String>` field on each doc — search returns the inner symbol; the outer is queryable separately. The producer decides at extraction time. Forces: schema choice in §4.1 and the producer-side wire-shape ADR. |
| Q-LEX05-5 | (Wave-3 entry gate, callback to RT-01 / RFC LEX-07) Does the per-record delta path consult RT-01's runtime metadata catalog for `changed:` semantics, or is symbol-side delta purely channel-driven? **Default answer**: channel-driven only; RT-01 consumes the symbol-side write-packet trace, not the other way round. Forces: cross-ticket boundary documented in §3.6. |
| Q-LEX05-6 | (cross-ref boundary) When a downstream cross-ref planner family (RFC § Planner Model item 5) lands, does it consume `relationship=reference` rows from this shard directly, or via a SEM-* adapter? **Default answer**: directly, post-Wave-7; the cross-ref planner is a *consumer* of this authority, not a *deriver*. Forces: post-Wave-8 cross-ref planner ticket scope. |
| Q-LEX05-7 | Does `case:no` apply to both `name` (analyzed) and `name_raw` (raw) fields when the pattern is a `Keyword` leaf? **Default answer**: yes — `name_raw` is lowercased for indexing when `case:no` is the active option; one canonical AST per query. Forces: schema bookkeeping. |
| Q-LEX05-8 | Maximum symbol records per generation before the apply path rejects as `PLAN_LIMIT_EXCEEDED{dimension=symbol-records-per-generation}` — default `10_000_000` per §10 R-LEX05-4 has no [../dsl.md](../dsl.md) §13 row yet. **Default answer**: add a row to [../dsl.md](../dsl.md) §13: `max symbol records per generation = 10_000_000`; ADR-002 (narrowed) enshrines. |
| Q-LEX05-9 | (entry-gate callback to RT-01 / RFC LEX-07 boundary) Wave 5 RT-01 ships `changed:<scope>` semantics that depend on the symbol authority for scope=`symbol`. Is that within LEX-05 scope (export a `SymbolDeltaSet` to RT-01) or RT-01 scope (RT-01 derives its own scope-symbol mapping)? **Default answer**: RT-01 derives — LEX-05 is a read-only authority; RT-01 walks the write-packet trace and the symbol shard. Forces: Wave 5 entry gate. |

---

## §13 References

- [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) — Canonical SSOT for the producer/search-plane split: §0 (Out-of-scope: bundle creation, generation assignment, delta authorship); §3.1 (`LexicalChannelOp::UpsertSymbol { repo, revision, generation, symbol: SymbolRecord }` — authoritative input surface for this ticket); §3.1 authorship rule (producer authors every payload, search plane decodes); §4.6 (corruption / fail-closed policy that `SYMBOL_PAYLOAD_DECODE_FAIL` and `SYMBOL_RECORD_INVALID` hook into); §5.2 (sealed-generation ledger that the symbol sibling flips into via `Seal`).
- [../rfc.md](../rfc.md) — May-23 Sourcegraph-Class Lexical Kernel RFC: § Lexical content engine; § Recommended Concrete Engine Choices § Lexical content/path/symbol; § Generation model § symbol shard generation; § Canonical Incremental Write Pipeline; § Non-Negotiable Invariants §1, §3, §10; § Atomicity contract; § Storage-layer enforcement; § Planner Model item 5 (cross-ref boundary); § Error Code Taxonomy; § Capacity and SLO Targets; § Observability Requirements; § Claim Discipline.
- [../feature-scope.md](../feature-scope.md) — §1.1.3 `type:` row; §1.3.4 STR-01 grammar pin set (alignment); §4.1 LEX-* cross-reference; §6.1 Core-1.0 authority chain (truth provider + write-time validation + read-time service + failure model); §7 Scale & capacity scope; §9 Open questions Q1 (predicate evaluation timing) and §4.7 flagged gaps; §10 References.
- [../usecase.md](../usecase.md) — UC-SYM-01..06 rows; AC-07 (structural recursion — adjacent to symbol when over-applied via cross-domain); §3 Contract gap GAP-01 (`SymbolKind`); §0 error code SSOT; §6 Conformance reference plan.
- [../dsl.md](../dsl.md) — §1.4 identifier chars; §2.1 EBNF (filter productions); §6.2 filter table; §6.3 `lang:` enum; §6.4 `type:` values; §6.7 pattern kind per filter; §10 normalization; §11 canonical hash; §12 error taxonomy; §13 limits and budgets; §16 non-negotiable DSL invariants.
- [../implementation-plan.md](../implementation-plan.md) — §1.4 claimability rule; §2.1 contract state (GAP-01); §2.2 historical port surface; §2.3a working-tree divergence (G-CONTROL-LOC); §2.4 lexical adapter state; §4.4 Wave 3 plan; §5.1 PRE-CONTRACT-EXT DoD; §5.7 LEX-03 DoD; §5.8 LEX-04 (RFC ticket) DoD; §8 test strategy; §9 observability and SLO gates; §10 ADR slots (ADR-002, ADR-005, ADR-018 new); §11 open questions Q-UC-1, Q-FS-4, Q-FS-omitted-history.
- [../../../../CLAUDE.md](../../../../CLAUDE.md) — Agent change posture; Rule Catalog (Safety, Architecture, Build hygiene D18, Verification).
- [../../../../AGENTS.md](../../../../AGENTS.md) — Shared agent router.
- [../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive` (D18 enforcement).
- [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — Structured agent output schema.
- [../../../../tools/ci/lint/lint-doc-paths.py](../../../../tools/ci/lint/lint-doc-paths.py) — Doc-link linter.
- [../../../../tools/ci/lint/lint-hexagonal-boundaries.py](../../../../tools/ci/lint/lint-hexagonal-boundaries.py) — Hexagonal-boundary lint (asserts `core` does not import `tree-sitter`).
- [../../../../crates/quanta-index-lexical/src/](../../../../crates/quanta-index-lexical/src/) — Lexical adapter crate.
- [../../../../crates/quanta-index-contract/src/results/candidates.rs](../../../../crates/quanta-index-contract/src/results/candidates.rs) — `LexicalCandidate` shape (target of GAP-01 extension).
- [../../../ssot/producer-handoff.md](../../../ssot/producer-handoff.md) — Producer handoff SSOT (authoritative wire shape for `UpsertSymbol.payload`; delta-handling identity / cascade / replay contract in §3.5).
- [INDEX.md](INDEX.md) §3.6 (producer-authorship correction) and §3.8 (stale references in corrected specs) — architecture-correction context and cleanup tracker.
