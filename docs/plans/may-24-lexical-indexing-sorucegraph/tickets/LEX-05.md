# LEX-05 — Symbol Index (definitions + references)

> Status: `Ticket spec — draft`
> Wave: 3 (symbol shard authority lands as a sibling under the lexical content authority unified in Wave 2 (RFC LEX-03); the dedicated symbol planner + name/kind/file pushdown lane is Wave 3 work).
> Parent RFC: [../rfc.md](../rfc.md) § LQ/Core-1.0 `type:symbol`, § Engine Decomposition § Lexical content engine, § Recommended Concrete Engine Choices § Lexical content/path/symbol, § Non-Negotiable Invariants, § Error Code Taxonomy, § Claim Discipline.
> Sibling docs: [../feature-scope.md](../feature-scope.md), [../usecase.md](../usecase.md), [../dsl.md](../dsl.md), [../implementation-plan.md](../implementation-plan.md).
> Posture: **breaking-first** — no long-lived shims, no dual surface, no heuristic success path. Per [../../../../CLAUDE.md](../../../../CLAUDE.md) § Agent change posture.

---

## §1 Purpose

Land a ctags-class symbol authority (definitions + references) as a Tantivy sibling shard under the manifest-first generation set, exposed through the search-plane via `type:symbol` queries plus `kind:`, `file:`, `repo:`, `lang:`, `case:`, and `/.../` regex projections over the symbol name index. Specifically:

1. Index symbol **definitions** (function, class, method, variable, macro, ...) per file, per language, into a Tantivy symbol sibling under the existing manifest generation contract (per [../rfc.md](../rfc.md) § Generation model § symbol shard generation).
2. Index symbol **references** (call sites, type usages, identifier read sites) using the same authority schema. The boundary against semantic resolution (true import-resolved definition-of and reference-to) is **locked** below (§3.5) — this lane does **not** resolve cross-file definition identity; that belongs to SEM-01 / BRIDGE-* / cross-ref planner family (RFC § Planner Model item 5).
3. Materialize a `SymbolKind` enum (closes **GAP-01** from [../usecase.md](../usecase.md) §3) wired into the contract crate by `PRE-CONTRACT-EXT` (per [../implementation-plan.md](../implementation-plan.md) §5.1), then consumed by this ticket's executor.
4. Surface `type:symbol kind:<kind>` queries by the symbol kind enum (closes UC-SYM-02 contract gap).
5. Plan and execute regex over symbol names (closes UC-SYM-04: `type:symbol /^Bar/`).
6. Fail closed on unsupported language with typed `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED` (no silent skip — per [../rfc.md](../rfc.md) § Non-Negotiable Invariants §1, §3).
7. Surface a per-language v1 supported set: **Rust, Python, Go, TypeScript** (with TSX), JavaScript. Justified below (§3.4). Outside this set → typed reject, not silent skip.
8. Incremental update boundary (file-level re-index granularity) is **locked** below (§3.6) — does a single-file edit re-index only that file's symbols, or the whole generation? Default answer: per-file delta only, per [../rfc.md](../rfc.md) § Canonical Incremental Write Pipeline §2.

This ticket is the only owner of symbol indexing in the LQ kernel. Tantivy's pre-existing `symbol` field (per [../implementation-plan.md](../implementation-plan.md) §2.4) is **subsumed** by this lane; no parallel path may serve `type:symbol` queries after Wave 3 exit.

---

## §2 Background

### 2.1 What exists today

- Tantivy chunk index ([../../../../crates/quanta-index-lexical/src/](../../../../crates/quanta-index-lexical/src/)) has a `symbol` field. State per [../implementation-plan.md](../implementation-plan.md) §2.4: "**partial** — symbol field exists but no dedicated symbol planner".
- No `SymbolKind` exists in the contract crate. `LexicalCandidate` ([../../../../crates/quanta-index-contract/src/results/candidates.rs](../../../../crates/quanta-index-contract/src/results/candidates.rs)) has no `symbol_kind` field. This is **GAP-01** in [../usecase.md](../usecase.md) §3 and is resolved by `PRE-CONTRACT-EXT` (Wave 0); LEX-05 is the consumer.
- Per [../implementation-plan.md](../implementation-plan.md) §2.2, the historical `SearchPlaneLexicalIndexBuildPort` / `SearchPlaneLexicalIndexStorePort` cover a single Tantivy generation; multi-sibling extension belongs to LEX-03 (RFC ticket) and this ticket's contract.
- No ctags / tree-sitter / scip integration exists.
- Sourcegraph reference (per [../feature-scope.md](../feature-scope.md) § Sourcegraph compatibility delta) uses Zoekt-style symbol indices keyed by ctags universal-ctags output, with a sidecar for kind. We diverge on extractor choice (§3.3 below).

### 2.2 Why "Wave 3"

This ticket depends on:
- `PRE-CONTRACT-EXT` (`SymbolKind`, contract extension for `kind`) — Wave 0.
- LEX-01 parser owning `kind:` filter grammar — Wave 1.
- LEX-03 sibling shard model and manifest-first atomicity assertion — Wave 2.

At Wave 3 entry, all of those are green; LEX-05 fills the actual symbol authority surface.

### 2.3 Symbol-extraction vendor decision (ADR-002 candidate)

[../implementation-plan.md](../implementation-plan.md) §10 lists **ADR-002 — Symbol shard storage layout** with forcing function "Wave 2 / LEX-03 start". This ticket converts ADR-002 from "storage layout" to "extractor + storage layout" because the choice of extractor pins the schema. Three candidate paths:

| Candidate | Pros | Cons |
|---|---|---|
| **tree-sitter per-grammar** | Co-aligns with STR-01 (Wave 5) grammar set; no new vendor; query patterns expressible in tree-sitter query DSL; per-language `tags.scm` files exist for the v1 set | Larger build dependency; per-grammar `tags.scm` maintenance burden; reference vs definition disambiguation needs custom query authoring |
| **universal-ctags** | Mature; covers more languages than we need; existing query format; matches Sourcegraph upstream | External binary at indexing time (anti-RFC-§Canonical Incremental Write Pipeline `forbidden steady-state operations` if invoked per-query — but indexing-time is fine); produces opaque tag records that need parsing |
| **scip** | Indexer ecosystem; rich semantic info | Heavyweight; produces semantic info we explicitly defer to SEM-01; mismatched authority — would leak SEM concerns into lexical lane |

**Proposed choice: tree-sitter per-grammar with curated `tags.scm`**.

Rationale:
1. Aligns with the existing Wave-5 STR-01 grammar pin set ([../feature-scope.md](../feature-scope.md) §1.3.4). Reusing grammars across LEX-05 and STR-01 means one pin set, one update cadence (ADR-005 forcing function).
2. No external binary spawn at indexing time — satisfies RFC § Canonical Incremental Write Pipeline § forbidden operations.
3. Reference vs definition disambiguation is encoded as `@definition.*` / `@reference.*` capture tags in `tags.scm`, which is a static authority pinnable per grammar version.
4. scip is rejected because it overlaps SEM-01 authority and would force lexical to carry semantic truth — violates RFC § Engine Decomposition § Lexical content engine § must not §1.

This choice is logged as **ADR-002 (extended)** with forcing function Wave 2 entry (per [../implementation-plan.md](../implementation-plan.md) §10) and **ADR-018 — Symbol extraction vendor and `tags.scm` curation policy** (new slot, forcing function Wave 3 entry, this ticket).

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

- Producer-published `PublishedSearchBundleManifest` carrying chunk rows + path metadata for `(repo, rev, generation)` (per [../feature-scope.md](../feature-scope.md) §6.1 authority chain).
- A file's UTF-8 source bytes (already retrieved by the lexical content build step).
- A language id derived from the path + producer metadata (per [../dsl.md](../dsl.md) §6.3 `lang:` enum).
- A pinned `tree-sitter` grammar set for the v1 supported languages.
- A per-language `tags.scm` capture authority committed at `crates/quanta-index-lexical/queries/symbols/<lang>.scm` (path proposed; ADR-018 may relocate).

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

Total: 20 values. Hand-rolled serde (D18 ban applies). Sub-set must be representable by every v1 language; values not produced by a given language are simply never emitted for that language — no silent re-tagging.

Mapping from tree-sitter `tags.scm` capture names to `SymbolKind` is a per-grammar lookup table maintained in the same source file as the `tags.scm` (e.g. `crates/quanta-index-lexical/queries/symbols/rust.scm` plus `rust_kind_map.rs`).

### 3.4 Per-language v1 supported set

| Language | tree-sitter grammar | Definition captures | Reference captures | Justification for cut |
|---|---|---|---|---|
| Rust | `tree-sitter-rust` | `@definition.function`, `@definition.method`, `@definition.struct`, `@definition.enum`, `@definition.enum_member`, `@definition.trait`, `@definition.impl`, `@definition.macro`, `@definition.type_alias`, `@definition.constant`, `@definition.module` | `@reference.call`, `@reference.type`, `@reference.macro` | Primary host language; aligns with this repo's own Rust workspace |
| Python | `tree-sitter-python` | `@definition.function`, `@definition.class`, `@definition.method` | `@reference.call`, `@reference.class` | Top-3 OSS volume; widely used in target deployments |
| Go | `tree-sitter-go` | `@definition.function`, `@definition.method`, `@definition.type`, `@definition.interface`, `@definition.constant`, `@definition.variable` | `@reference.call`, `@reference.type` | Top-3 OSS volume; clean ctags story |
| TypeScript (+TSX) | `tree-sitter-typescript` | `@definition.function`, `@definition.method`, `@definition.class`, `@definition.interface`, `@definition.type_alias`, `@definition.enum`, `@definition.constant`, `@definition.variable` | `@reference.call`, `@reference.type` | Top-3 OSS volume; superset of JS surface |
| JavaScript | `tree-sitter-javascript` | `@definition.function`, `@definition.method`, `@definition.class`, `@definition.constant`, `@definition.variable` | `@reference.call`, `@reference.class` | Coverage completeness with TS |

Languages outside this set return `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id}` at query time (fail-closed). No silent skip.

Java, C, C++, Ruby (per [../feature-scope.md](../feature-scope.md) §1.3.4 STR-01 stretch / post-STR-01 rows) are explicitly **deferred** to a post-Wave-3 LEX-05 follow-up — not silently included with degraded fidelity.

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

A single-file edit causes **per-file delta only**:

1. Producer emits a `bundle_delta` row covering the changed file (per [../implementation-plan.md](../implementation-plan.md) §2.3 row "T1.1 `bundle_delta_applied`").
2. Search-plane's `MaterializeUseCase` invocation re-runs the tree-sitter parse + `tags.scm` capture for **only that file**.
3. The Tantivy symbol shard receives delete-then-insert for that file's symbol document set.
4. Generation `manifest_generation` advances per file delta (not per change-batch); sibling `symbol_generation` advances atomically.
5. Other files in the same generation are not re-extracted.

Wave-3 entry gate: this incremental boundary is exactly what RT-01 / LEX-07 (the wave-5 / wave-4 RFC-tickets) consume. The write-packet trace ([../implementation-plan.md](../implementation-plan.md) §5.8) asserts `O(changed-chunks)` for the symbol sibling under a 1-file delta — same proof rail.

---

## §4 Deliverables

### 4.1 Code surface

A new module set under `crates/quanta-index-lexical/src/symbol/` (or co-located with the existing chunk index — choice belongs to ADR-018):

1. `symbol/schema.rs` — Tantivy schema for the symbol shard: fields `repo_id`, `revision_id`, `manifest_generation`, `lang`, `repo_relative_path`, `start_line`, `end_line`, `start_byte`, `end_byte`, `name` (analyzed), `name_raw` (raw, for regex), `kind` (faceted), `relationship` (enum: `definition` | `reference`), `container_name` (optional, for nested symbols).
2. `symbol/build.rs` — implementation of the per-language extraction pipeline. One pass per file: open grammar, parse, run `tags.scm` query, emit a `SymbolDoc` per capture, batch-write to Tantivy.
3. `symbol/lang_table.rs` — language id → grammar handle + `tags.scm` + kind-map table.
4. `symbol/store.rs` — per-generation reader cache (mirrors the existing chunk-index reader-cache pattern per [../implementation-plan.md](../implementation-plan.md) §2.4 row "LEX-07 generations governance").
5. `symbol/query.rs` — planner pushdown + Tantivy query construction: `kind:` → faceted filter; pattern leaves → `TermQuery` / `PhraseQuery` / `RegexQuery` over `name` / `name_raw`; `lang:` / `file:` / `repo:` propagated from outer filters.
6. `symbol/relationship.rs` — surface for `relationship: definition | reference` projection (consumer of UC-SYM-* tests; cross-ref planner family will read this surface later).
7. Hand-rolled `impl serde::Serialize` / `impl serde::Deserialize` for every wire type — D18 ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) `rust-no-serde-derive`).

### 4.2 Trait surface

Extending the sibling-shard port pattern landed by LEX-03:

- `LexicalIndexBuildPort::build_symbol_shard(manifest: &Manifest, lang_table: &LangTable) -> Result<SymbolBuildArtifact, CoreError>`.
- `LexicalIndexStorePort::open_symbol_reader(generation: &PublishedGenerationSet) -> Result<SymbolReader, CoreError>`.
- `SymbolExecutor::execute(plan: &SymbolPlan, ctx: &PlanContext, options: &LqOptionSet) -> Stream<LexicalCandidate>`.

No new RPC surface beyond what `PRE-CONTRACT-EXT` already lands (the `kind:` filter variant and `SymbolKind` enum).

### 4.3 Contract additions (consumed, not defined here)

Landed by `PRE-CONTRACT-EXT`:
- `SymbolKind` enum (per §3.3).
- `LqFilter::Kind(SymbolKind)` variant.
- `LexicalCandidate.symbol_kind: Option<SymbolKind>` field (or `SymbolCandidate` sibling type — choice belongs to Q-UC-1 / Q-LEX05-1 below).

### 4.4 Test surface

- `crates/quanta-index-lexical/tests/symbol_unit.rs` — per-error-code and per-kind unit tests.
- `crates/quanta-index-lexical/tests/symbol_per_lang.rs` — golden corpus per language.
- `crates/quanta-index-lexical/tests/symbol_property.rs` — property tests (1k random valid identifier names per language → round-trip).
- `crates/quanta-index-lexical/tests/symbol_incremental.rs` — single-file delta proof.
- `crates/quanta-index-lexical/benches/symbol_bench.rs` — `lex_05_symbol_build_bench`, `lex_05_symbol_query_bench`.
- Conformance rows `usecase-corpus/UC-SYM-{01..06}.toml`.

### 4.5 Docs

- ADR-002 (extended) — Symbol shard storage layout + extractor + `tags.scm` curation policy (forcing function: Wave 3 entry).
- New ADR-018 — Symbol extraction vendor (proposes tree-sitter+`tags.scm`; documents rejected scip and ctags alternatives).
- Updates to `docs/handoffs/lq-contract-1.0.md` noting the symbol sibling shard schema and the contract enum.
- `crates/quanta-index-lexical/queries/symbols/<lang>.scm` × 5 files (one per v1 language).

---

## §5 Implementation steps (TDD)

Each step is one PR, starting with a failing test.

### 5.1 Step 1 — Tree-sitter grammar pin + smoke

Test: `symbol_unit::rust_grammar_loads` asserts `tree-sitter-rust` grammar loads in process; `symbol_unit::rust_tags_scm_parses` asserts `queries/symbols/rust.scm` compiles as a tree-sitter query.

Implement: workspace `Cargo.toml` adds `tree-sitter = "=0.22.x"`, `tree-sitter-rust = "=0.21.x"`, etc. (pin per ADR-005). Commit the five `*.scm` files. Run smoke loader.

### 5.2 Step 2 — `SymbolDoc` schema + Tantivy field set

Test: `symbol_unit::schema_round_trip` writes one `SymbolDoc` to an in-memory Tantivy index and reads it back identical (hand-rolled serde guarantees zero proc-macro derives).

Implement: `symbol/schema.rs` per §4.1 list; `symbol/build.rs::write_one(doc, writer)`.

### 5.3 Step 3 — Per-language extractor (Rust first)

Test: golden Rust fixture file (`tests/fixtures/symbol/rust/sample.rs`) — assert the extractor emits the expected list of `(name, kind, relationship, start_line)` tuples. Golden file lives at `tests/fixtures/symbol/rust/sample.expected.json` and is reviewed by hand at creation.

Implement: `symbol/build.rs::extract_one_file(lang, source) -> Vec<SymbolDoc>`. Use `tree-sitter::Query` against the language's `tags.scm`. Map capture names to `SymbolKind` via `lang_table::kind_for_capture(lang, capture_name)`.

### 5.4 Step 4 — Remaining 4 languages

Repeat Step 3 for Python, Go, TypeScript (+ TSX), JavaScript. Each language has its own golden fixture and `expected.json`.

### 5.5 Step 5 — Unsupported language fail-closed

Test: `symbol_unit::unsupported_lang_fails_closed` — passing `lang_id="ruby"` returns `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id="ruby"}`. No silent skip, no empty-document return.

Implement: in `symbol/build.rs::extract_one_file`, the language lookup must surface a typed error before any extraction work.

### 5.6 Step 6 — Sibling shard wiring under manifest

Test: `symbol_integration::manifest_first_atomicity` asserts that a partially-built symbol shard never becomes readable: simulate a crash mid-write, then attempt to open the symbol reader for that generation → `STATE_NOT_READY: STALE_SIBLING` (per RFC § Storage-layer enforcement).

Implement: `MARKER_OK` per symbol shard; reader-open path asserts presence before returning a reader. Hook into LEX-03's manifest-first contract.

### 5.7 Step 7 — Planner pushdown

Test: `symbol_unit::planner_routes_type_symbol`, `planner_filter_kind`, `planner_filter_lang`, `planner_pattern_keyword`, `planner_pattern_regex`. Each row uses a constructed `LqQueryV1` and asserts the produced `SymbolPlan` has the expected pushed filters.

Implement: `symbol/query.rs::plan(query: &LqQueryV1) -> Result<SymbolPlan, LexicalErrorCode>`. Reject combinations that violate RFC § Planner Model with typed `PLAN_UNSUPPORTED_COMBO`.

### 5.8 Step 8 — Executor + reader cache

Test: `symbol_integration::uc_sym_01_bare_symbol`, `uc_sym_02_kind_function`, `uc_sym_03_file_path_scope`, `uc_sym_04_regex_name`, `uc_sym_05_case_yes`, `uc_sym_06_with_repo`. Each row uses the seeded multi-language fixture and asserts the documented `LexicalCandidate` set, in deterministic order.

Implement: `SymbolExecutor::execute` with per-generation reader cache, bounded concurrency under LEX-05 (the RFC ticket — the parallel executor's fanout consumes this stream).

### 5.9 Step 9 — Regex over symbol names

Test: `symbol_integration::uc_sym_04_regex_name_prefix` — `type:symbol /^Bar/` returns only symbols whose name starts with `Bar`. Cross-conformance with LEX-04 (this packet's regex executor): the regex compiler is shared; the symbol lane just narrows the field to `name_raw`.

Implement: route `LqExpr::Regex` against `name_raw` (raw, no analyzer) using the compiled `regex::Regex` from LEX-04. Inherit NFA-state cap from [../dsl.md](../dsl.md) §3.4.

### 5.10 Step 10 — Single-file incremental delta

Test: `symbol_incremental::single_file_edit_reindexes_only_that_file` — seed a 100-file fixture, mutate one file, run the materialize pipeline; assert the write-packet trace touches `O(1)` files (just the edited one) and the symbol sibling for the new generation matches the expected diff exactly.

Implement: hook into the per-file delta path that LEX-04 (the RFC ticket — incremental lexical indexing kernel) lands. Symbol shard's per-file delete-then-insert is the per-file granularity asserted in §3.6.

### 5.11 Step 11 — Conformance + ADRs

Wire UC-SYM-01..06 golden files into `usecase-corpus/`. Land ADR-002 (extended) and ADR-018. Update `docs/handoffs/lq-contract-1.0.md`.

### 5.12 Step 12 — Criterion benches

Add `lex_05_symbol_build_bench` (per-file extract) and `lex_05_symbol_query_bench` (per-query end-to-end). Regression budget: p99 may not increase >5% per wave without an ADR.

---

## §6 Test plan

### 6.1 Unit (`cargo test -p quanta-index-lexical --test symbol_unit`)

| Test name | Asserts |
|---|---|
| `symbol_unit::rust_grammar_loads` | grammar loads, query compiles |
| `symbol_unit::python_grammar_loads` | as above |
| `symbol_unit::go_grammar_loads` | as above |
| `symbol_unit::typescript_grammar_loads` | as above |
| `symbol_unit::javascript_grammar_loads` | as above |
| `symbol_unit::schema_round_trip` | one `SymbolDoc` round-trips through Tantivy |
| `symbol_unit::unsupported_lang_fails_closed` | `lang_id="ruby"` → `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id="ruby"}` |
| `symbol_unit::corrupted_source_typed_error` | malformed UTF-8 source → `PARSE_INVALID_UTF8{byte_offset}` at indexing time; no panic |
| `symbol_unit::planner_routes_type_symbol` | `type:symbol Foo` plan has `SymbolPlan` with `pattern=Keyword("Foo")` |
| `symbol_unit::planner_filter_kind` | `type:symbol kind:function handler` plan has `kind=Function` |
| `symbol_unit::planner_filter_unknown_kind` | `type:symbol kind:not_a_kind handler` → `PARSE_INVALID_FILTER_VALUE{filter=kind}` |
| `symbol_unit::planner_filter_lang` | `type:symbol lang:rust Foo` plan has `lang=Rust` and prunes other-lang docs |
| `symbol_unit::planner_pattern_regex_uses_name_raw` | `type:symbol /^Bar/` plan binds regex to `name_raw` field |
| `symbol_unit::planner_unsupported_combo_type_symbol_into_codeql_diff` | `type:symbol into:codeql type:diff` mix → `PLAN_UNSUPPORTED_COMBO` |
| `symbol_unit::no_silent_skip_unknown_capture` | a `tags.scm` capture name that does not map to a `SymbolKind` → load-time assertion failure (CI lint, not silent ignore) |

### 6.2 Per-language golden (`cargo test -p quanta-index-lexical --test symbol_per_lang`)

| Test name | Fixture | Asserts |
|---|---|---|
| `symbol_per_lang::rust_sample` | `tests/fixtures/symbol/rust/sample.rs` | extracted list == `sample.expected.json` |
| `symbol_per_lang::python_sample` | `tests/fixtures/symbol/python/sample.py` | as above |
| `symbol_per_lang::go_sample` | `tests/fixtures/symbol/go/sample.go` | as above |
| `symbol_per_lang::typescript_sample` | `tests/fixtures/symbol/typescript/sample.ts` | as above |
| `symbol_per_lang::tsx_sample` | `tests/fixtures/symbol/typescript/sample.tsx` | as above |
| `symbol_per_lang::javascript_sample` | `tests/fixtures/symbol/javascript/sample.js` | as above |

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

| Test name | Asserts |
|---|---|
| `symbol_incremental::single_file_edit_reindexes_only_that_file` | write-packet trace size = `O(1)` files for a 1-file delta against a 100-file fixture |
| `symbol_incremental::file_delete_purges_symbols` | deleting a file removes its symbols from the next generation |
| `symbol_incremental::file_rename_emits_new_doc_set` | renaming a file emits delete-old + insert-new in the same delta |

### 6.5 Property (`cargo test -p quanta-index-lexical --test symbol_property`)

| Test name | Asserts |
|---|---|
| `symbol_property::canonical_hash_stable_symbol_queries` | proptest 1k random `type:symbol …` queries → normalize→print→normalize→hash identical across 2 architectures |
| `symbol_property::extractor_idempotent` | proptest 1k random valid source files per language → extract(extract(file)) == extract(file) (idempotency) |
| `symbol_property::kind_map_total` | every emitted `tags.scm` capture maps to exactly one `SymbolKind` (load-time enforced) |

### 6.6 Criterion (`cargo bench -p quanta-index-lexical --bench symbol_bench`)

| Bench | Budget |
|---|---|
| `lex_05_symbol_build_bench` | p99 < 50 ms per 1 KLOC source file across all 5 languages |
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
| `lq.build.symbol.extract` | `lang: string`, `file_size_bytes: u64`, `symbol_count: u64`, `extract_time_us: u64` |

Metrics:

- `lq_symbol_extract_ms` — histogram per language, labels=`{ticket_id, wave_id, lang}` cardinality≤5.
- `lq_symbol_query_ms` — histogram, labels=`{ticket_id, wave_id}`.
- `lq_symbol_unsupported_lang_total` — counter, labels=`{ticket_id, wave_id, lang_id}`. Cardinality budget: closed enum of v1-rejected langs known at build time.
- `lq_symbol_shard_size_bytes` — gauge per `(repo, rev, generation)`, labels=`{repo_id, generation_id}`. Cardinality budget: per [../rfc.md](../rfc.md) § Metric schema, retention-bounded; see [../implementation-plan.md](../implementation-plan.md) §9.

Audit log: per [../rfc.md](../rfc.md) § Audit trail — `error_code?` carries `STATE_NOT_READY` / `PARSE_INVALID_FILTER_VALUE` / `PLAN_UNSUPPORTED_COMBO` when this lane rejects.

Per-wave OBS subset: Wave 3 entry has `lq.exec.shard.symbol.*` emitting (per [../implementation-plan.md](../implementation-plan.md) §9.1 Wave 3 row).

---

## §8 Error scenarios

| Scenario | Surface | Code | Where | Test |
|---|---|---|---|---|
| `kind:` value not in `SymbolKind` enum | parse | `PARSE_INVALID_FILTER_VALUE{filter=kind, value, allowed=[20 values]}` | parser | `symbol_unit::planner_filter_unknown_kind` |
| `lang:` value outside v1 supported set at query time | exec | `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id}` | `symbol/build.rs::extract_one_file` | `symbol_unit::unsupported_lang_fails_closed` |
| `lang:` value outside v1 set encountered at indexing | indexing | `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id}` | extractor | `symbol_unit::unsupported_lang_fails_closed` |
| Source file fails UTF-8 validation | indexing | `PARSE_INVALID_UTF8{byte_offset}` | extractor pre-check | `symbol_unit::corrupted_source_typed_error` |
| `tags.scm` capture name not mapped to `SymbolKind` | indexing | load-time CI lint failure (NOT silent — covered by `symbol_property::kind_map_total`) | `lang_table::kind_for_capture` | `symbol_property::kind_map_total` + CI lint |
| Tree-sitter parse error on source file | indexing | `STATE_NOT_READY: SYNTAX_ERROR{lang, file}` (file-level error; other files in the generation continue) | extractor | `symbol_incremental::file_with_syntax_error_isolates` |
| Symbol shard `MARKER_OK` absent at read | exec | `STATE_NOT_READY: STALE_SIBLING{sibling=symbol, manifest_gen, sibling_gen}` | reader open | `symbol_integration::manifest_first_atomicity` |
| Symbol generation regression (non-NULL → NULL or rollback) | indexing | `STATE_GENERATION_REGRESSION{kind=symbol, prev, observed}` | writer | covered by LEX-04 (RFC ticket) writer-coordinator |
| `type:symbol` + `match { ... }` | parse/plan | `PLAN_UNSUPPORTED_COMBO{combo="type:symbol, structural"}` | planner | unit |
| `type:symbol` + `into:codeql` + `type:diff` | parse/plan | `PLAN_UNSUPPORTED_COMBO{combo}` | planner | `symbol_unit::planner_unsupported_combo_type_symbol_into_codeql_diff` |
| Per-tenant fanout cap exceeded | exec | `PLAN_LIMIT_EXCEEDED{limit_kind=fanout}` | LEX-05 inherits | covered by RFC LEX-05 |

There is **no silent skip path** for unsupported languages. The single-file syntax-error case isolates to that file's `STATE_NOT_READY` (the rest of the generation proceeds), but the affected file is **not** silently emitted as zero symbols — the file's symbol authority for this generation is explicitly absent, which a `STATE_NOT_READY` reader-side check surfaces to any query that would touch the file's symbol set.

---

## §9 Perf envelope

| Dimension | Target | Source |
|---|---|---|
| Per-file extract (Rust, 1 KLOC) p99 | < 50 ms | tree-sitter parse + query cost |
| Per-file extract (TypeScript, 1 KLOC) p99 | < 70 ms | TS grammar is heavier |
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
| R-LEX05-1 | Tree-sitter grammar version drift breaks `tags.scm` between minor releases | M | M | grammar update PR fails `symbol_per_lang::*` | Pin `tree-sitter-<lang> = "=…"` per ADR-005; `tags.scm` test rail runs against pinned grammar |
| R-LEX05-2 | A new language enters production via path heuristic but no `tags.scm` exists → silent skip risk | H | H | end-user reports "my Java symbols not searchable" | Fail-closed `SYMBOL_LANG_UNSUPPORTED` typed error; no path-extension heuristic; the lang lookup is the only authority |
| R-LEX05-3 | `SymbolKind` enum drift between contract crate and `tags.scm` capture maps | M | H | `symbol_property::kind_map_total` fails | Property test asserts every capture name maps to exactly one `SymbolKind` at load time; CI lint blocks PR otherwise |
| R-LEX05-4 | Symbol shard size grows superlinearly with reference indexing on a generated-code-heavy repo | M | M | `lq_symbol_shard_size_bytes` exceeds 0.5× source size | Cap per-file reference count; emit a typed `PLAN_LIMIT_EXCEEDED{dimension=symbol-refs-per-file}` if a single file produces more than `N` references; default `N=10_000` |
| R-LEX05-5 | Incremental update granularity larger than per-file (e.g. per-chunk would mis-attribute symbols crossing chunk boundaries) | L | H | `symbol_incremental::single_file_edit_reindexes_only_that_file` red | Symbol delta is **file-level**, not chunk-level, even though the chunk index is chunk-level. Documented in §3.6 |
| R-LEX05-6 | TSX grammar choice ambiguity vs vanilla TypeScript causes double-indexing | M | M | per-lang test shows `.tsx` parsed twice | `lang_table` resolves `.tsx` → `tsx` grammar; `.ts` → `typescript` grammar; one path each |
| R-LEX05-7 | Universal-ctags or scip rejection later revisited if grammar maintenance is too costly | L | M | `tags.scm` PR rate spikes | ADR-018 documents the rejected alternatives; reopening requires a new ADR |
| R-LEX05-8 | Reference indexing crosses the def/ref boundary by accident — e.g. by emitting "this reference resolves to that definition" edges | L | H | Code review of `relationship.rs` | Locked at §3.5; no edge-resolution code may land in this lane; CI lint disallows imports of `quanta-index-semantic` from `quanta-index-lexical` |

ADR slots: **ADR-002 (extended)** and **ADR-018** (new). Forcing functions: Wave 2 entry (ADR-002) and Wave 3 entry (ADR-018).

---

## §11 DoD (provable)

Each row is one provable artifact.

1. `SymbolKind` enum, `LqFilter::Kind`, and `LexicalCandidate.symbol_kind` (or `SymbolCandidate` sibling per Q-LEX05-1) land in `PRE-CONTRACT-EXT`; LEX-05 consumes them. Proof: `cargo test -p quanta-index-contract --test contract_round_trip` covers `SymbolKind`; downstream `symbol_unit::planner_filter_kind` validates wire-through.
2. Tree-sitter grammars for Rust, Python, Go, TypeScript (+TSX), JavaScript are pinned in workspace `Cargo.toml`. Proof: `cargo tree -p tree-sitter-rust` (etc.) recorded in ADR-018.
3. `tags.scm` files for the 5 languages live at `crates/quanta-index-lexical/queries/symbols/<lang>.scm`. Proof: `symbol_unit::<lang>_tags_scm_parses` for each.
4. Per-language golden corpus passes. Proof: `symbol_per_lang::{rust, python, go, typescript, tsx, javascript}_sample` all green.
5. UC-SYM-01..06 conformance rows green in PRE-CONF. Proof: `cargo test -p quanta-index-contract --test lq_conformance` + `usecase-corpus/UC-SYM-{01..06}.toml`.
6. Unsupported language → `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id}` end-to-end. Proof: `symbol_unit::unsupported_lang_fails_closed`. No silent skip anywhere in `symbol/build.rs`.
7. Corrupted-source path is typed (`PARSE_INVALID_UTF8` or `STATE_NOT_READY: SYNTAX_ERROR`); no panic. Proof: `symbol_unit::corrupted_source_typed_error` + `symbol_incremental::file_with_syntax_error_isolates`.
8. `kind:` filter rejects unknown kinds at parse time with `PARSE_INVALID_FILTER_VALUE`. Proof: `symbol_unit::planner_filter_unknown_kind`.
9. Single-file edit causes `O(1)` files re-extracted. Proof: `symbol_incremental::single_file_edit_reindexes_only_that_file` with write-packet trace asserting size.
10. Symbol shard `MARKER_OK` enforced at read. Proof: `symbol_integration::manifest_first_atomicity` (consumer of LEX-03 storage-level assertion).
11. Reference vs definition projection works. Proof: `symbol_integration::reference_vs_definition_projection`.
12. Regex over symbol names uses `name_raw` field (no analyzer interference). Proof: `symbol_integration::uc_sym_04_regex_name` + `symbol_unit::planner_pattern_regex_uses_name_raw`.
13. Canonical hash stable across two architectures for symbol queries. Proof: `symbol_property::canonical_hash_stable_symbol_queries` on the CI x86_64 + aarch64 matrix.
14. `kind_for_capture` is total over the union of `tags.scm` captures. Proof: `symbol_property::kind_map_total`.
15. No `unwrap` / `unwrap_or` / `Result::ok` on production paths. Proof: clippy disallowed-methods rail green.
16. No `#[derive(Serialize|Deserialize)]` in this lane. Proof: semgrep `rust-no-serde-derive` green ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)).
17. `core` crate does **not** import `tree-sitter` or any grammar crate. Proof: CI hexagonal-boundary lint ([../../../../tools/ci/lint/lint-hexagonal-boundaries.py](../../../../tools/ci/lint/lint-hexagonal-boundaries.py)) green; `tree-sitter` imports confined to `quanta-index-lexical`.
18. ADR-002 (extended) and ADR-018 land. Proof: files committed at `docs/adr/ADR-002-symbol-shard.md` and `docs/adr/ADR-018-symbol-extraction-vendor.md`.
19. Telemetry spans and metrics declared in §7 emit with the closed attribute set. Proof: integration test asserting span attribute keys; metric scrape asserting cardinality budget.
20. Bench `lex_05_symbol_build_bench` and `lex_05_symbol_query_bench` p99 within budgets stated in §9. Proof: criterion CSV in CI artifacts.
21. Per-wave OBS subset met: Wave 3 entry has `lq.exec.shard.symbol.*` and `lq.build.symbol.extract` emitting. Proof: cross-reference [../implementation-plan.md](../implementation-plan.md) §9.1 Wave 3 row.
22. RFC § Claim-Discipline §1 — `Sourcegraph-compatible lexical core` — partially provable here for the symbol leg: UC-SYM-01..06 are `SG=` parity rows in [../usecase.md](../usecase.md). Full claim awaits Wave-8 OBS-01 conformance gate.
23. Structured agent output for this ticket validates against [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json). Missing evidence → `blocked`, not `ok`.

---

## §12 Open questions

| Q-ID | Question | Source | Blocking |
|---|---|---|---|
| Q-LEX05-1 | (closes GAP-01) Does `LexicalCandidate` gain a `symbol_kind: Option<SymbolKind>` field, or do we introduce a sibling type `SymbolCandidate`? **Default answer**: `Option<SymbolKind>` on `LexicalCandidate` — keeps response shape uniform; PRE-CONTRACT-EXT is the deciding ticket. Forces: Wave 0 entry (PRE-CONTRACT-EXT). |
| Q-LEX05-2 | Does the symbol authority index **references** in v1, or only **definitions**? **Default answer**: both, but with the boundary lock at §3.5 — references are local lexical positions only, never resolved cross-file. Forces: ADR-018; affects schema and shard size. |
| Q-LEX05-3 | Are nested symbols (e.g. Python `class Foo: def bar(): ...`) flattened or hierarchical? **Default answer**: flattened with a `container_name: Option<String>` field on each doc — search returns the inner symbol; the outer is queryable separately. Forces: schema choice in §4.1. |
| Q-LEX05-4 | (Wave-3 entry gate, callback to RT-01 / RFC LEX-07) Does the single-file delta path consult RT-01's runtime metadata catalog for `changed:` semantics, or is symbol-side delta purely manifest-driven? **Default answer**: manifest-driven only; RT-01 consumes the symbol-side write-packet trace, not the other way round. Forces: cross-ticket boundary documented in §3.6. |
| Q-LEX05-5 | (cross-ref boundary) When a downstream cross-ref planner family (RFC § Planner Model item 5) lands, does it consume `relationship=reference` rows from this shard directly, or via a SEM-* adapter? **Default answer**: directly, post-Wave-7; the cross-ref planner is a *consumer* of this authority, not a *deriver*. Forces: post-Wave-8 cross-ref planner ticket scope. |
| Q-LEX05-6 | (Java / C / C++ / Ruby) Are post-v1 languages added by ticket-amend or via new tickets? **Default answer**: new tickets (LEX-05-EXT-N for each language), because each requires `tags.scm` curation + golden fixture authorship. Forces: post-Wave-3 follow-up. |
| Q-LEX05-7 | Does `case:no` apply to both `name` (analyzed) and `name_raw` (raw) fields when the pattern is a `Keyword` leaf? **Default answer**: yes — `name_raw` is lowercased for indexing when `case:no` is the active option; one canonical AST per query. Forces: schema bookkeeping. |
| Q-LEX05-8 | Maximum symbols per file before the per-file delta is rejected as `PLAN_LIMIT_EXCEEDED{dimension=symbol-per-file}` — default `10_000` per [../dsl.md](../dsl.md) §13 has no row for it. **Default answer**: add a row to [../dsl.md](../dsl.md) §13: `max symbols per file = 10_000`; ADR-018 enshrines. |
| Q-LEX05-9 | (entry-gate callback to RT-01 / RFC LEX-07 boundary) Wave 5 RT-01 ships `changed:<scope>` semantics that depend on the symbol authority for scope=`symbol`. Is that within LEX-05 scope (export a `SymbolDeltaSet` to RT-01) or RT-01 scope (RT-01 derives its own scope-symbol mapping)? **Default answer**: RT-01 derives — LEX-05 is a read-only authority; RT-01 walks the write-packet trace and the symbol shard. Forces: Wave 5 entry gate. |

---

## §13 References

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
