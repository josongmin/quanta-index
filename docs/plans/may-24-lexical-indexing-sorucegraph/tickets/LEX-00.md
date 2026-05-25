# LEX-00: Lexical text normalization layer

| field | value |
|---|---|
| Status | shipped |
| Crate | `quanta-index-lq-text-norm` |
| Tests | 57 |
| Last verified | 2026-05-25 |
| Wave | 1 |
| Owner crate(s) | `quanta-index-lq-text-norm` (G-CONTROL-LOC resolved as standalone crate); consumes `quanta-index-core` ports |
| Touches contract | yes — no schema bump; consumes `LqQuery` / `LqOptionSet::patterntype` / `LqOptionSet::case` carried by [PRE-CONTRACT-EXT §5.1](../implementation-plan.md#51-pre-contract-ext) |
| Size | L — green-field tokenizer pipeline, golden corpus, idempotency proof |
| Depends on | PRE-CONTRACT-EXT, PRE-NORM (parser), PRE-CONF (corpus runner) |
| Blocks | LEX-01 (scorer needs canonical token stream), LEX-03 (sibling shards reuse same analyzer), LEX-04 (incremental writes call the normalizer), LEX-06 (BM25 + adjacency-link reads the token stream) |

> NFC normalization activated via `unicode-normalization` dependency. The NFKC variant of the analyzer actually applies NFKC (not aliased to NFC). Byte-offset semantics are documented as "offset into normalized text", not into the raw input.

---

## 1. Purpose

Stand up the **single canonical text normalization pipeline** that every
lexical authority (content, path, symbol) consumes at both write time and
read time. The pipeline owns:

1. UTF-8 validation + Unicode NFC normalization
2. code-aware token segmentation (camelCase / snake_case / digit boundaries)
3. case folding routing per `case:` filter + `patterntype:` mode
4. per-language analyzer dispatch
5. stop-token and length-cap policy
6. typed-error surface — no silent fallback, no swallowing

Required because today every consumer rolls its own analyzer choice (the
Tantivy adapter pins `en_stem` per predecessor [D4][prev-tickets-d4]), the
parser layer ([dsl.md §3.1](../dsl.md)) refuses to split tokens, and there is
no shared `LexicalNormalizer` port. Without LEX-00, identical tokens written
by `en_stem` cannot be looked up by a query whose parser left the leaf
verbatim, and the per-language analyzer choice becomes implicit and
unauditable.

The pipeline is the **only** legal token producer for any lexical sibling.
Adapters that bypass it must fail closed.

[prev-tickets-d4]: ../../search-plane-implementation-tickets.md

---

## 2. Background

### 2.1 Source contracts

| Source | What it pins |
|---|---|
| [rfc.md § Canonical Grammar Semantics → Pattern semantics](../rfc.md) | bare word is keyword, not fuzzy; no automatic camelCase split at the grammar layer |
| [rfc.md § Non-Negotiable Invariants 1, 2, 8, 11](../rfc.md) | no fuzzy default; no untyped error; no scope-widening; no silent fallback |
| [dsl.md §1.1](../dsl.md) | UTF-8 mandatory; BOM strip at offset 0; non-UTF-8 → `PARSE_INVALID_UTF8` |
| [dsl.md §1.5 quoted strings](../dsl.md) | three quote forms with distinct escape rules; tokenization disabled inside `'…'` and `/…/` |
| [dsl.md §3.1](../dsl.md) | "no automatic camelCase / snake_case split at the grammar layer; any token splitting is the indexing layer's tokenizer concern" — this ticket is that indexing layer |
| [dsl.md §3.3](../dsl.md) | `RawString` semantics: literal substring, **no tokenization applied**; requires trigram/ngram backend |
| [dsl.md §4](../dsl.md) | `patterntype:` mode matrix governs case-default + adjacency semantics |
| [dsl.md §10](../dsl.md) | normalization pipeline ordering; idempotency invariant |
| [feature-scope.md §1.1.6](../feature-scope.md) | `keyword` mode uses `en_stem`; `standard` is default |
| [usecase.md UC-LEX-01..06, UC-LEX-16..17, UC-LEX-21](../usecase.md) | corpus rows that LEX-00 must green |
| [usecase.md AC-01..03, AC-05..06](../usecase.md) | corpus rows that LEX-00 must reject |
| [implementation-plan.md §5.4 LEX-00](../implementation-plan.md#54-lex-00--baseline--invariants-freeze) | DoD row + property-invariants requirement |
| [implementation-plan.md §8.1 row LEX-00](../implementation-plan.md#81-per-ticket-rail-matrix) | unit + integration + property test rails (no criterion required at LEX-00; bench lives on LEX-01) |

### 2.2 Current state of the world

- `crates/quanta-index-lexical/src/lib.rs` ([lib.rs:1](../../../../crates/quanta-index-lexical/src/lib.rs)) is **stubbed out** — `build` / `open` return `CoreError::NotImplemented` pending the channel-driven pipeline (P4). Historical commit `736ddea` carried the Tantivy 0.22 chunk index with `en_stem`; the working-tree wipe in [implementation-plan.md §2.4](../implementation-plan.md#24-lexical-adapter--cratesquanta-index-lexicalsrc) is the active context.
- `crates/quanta-index-core/src/domains/lexical/` ([mod.rs](../../../../crates/quanta-index-core/src/domains/lexical/mod.rs)) exposes `LexicalIndexBuildPort` / `LexicalIndexOpenPort` / `LexicalSearcher` but **no normalizer trait**. LEX-00 adds the port; this is the channel-architecture home, not the deprecated chunk-bytes home — see §12.
- No analyzer registry exists. No per-language dispatch exists. No identifier-splitter exists.

### 2.3 Why "code-aware tokenization" is load-bearing

Sourcegraph users write `getUserName` and expect it to find `getUserName`,
`get_user_name`, `GET_USER_NAME`, and `getUsername` interchangeably (subject
to case mode). The Tantivy `en_stem` analyzer alone does **not** split on
camelCase. Without splitter expansion, UC-LEX-01 (`fooBar` keyword) and
the keyword-mode adjacency in [dsl.md §5.3](../dsl.md) become unreachable
against real-world source code.

Inversely, `patterntype:literal` (Sourcegraph literal mode) **must not**
split — see [dsl.md §4 mode matrix](../dsl.md). The pipeline routes per
mode, not by global flag.

---

## 3. Inputs

### 3.1 Hard inputs (read at write/query time)

| Input | Source | Where carried |
|---|---|---|
| raw text bytes (chunk content, file path, symbol name) | `LexicalChannelOp` write packet | `crates/quanta-index-contract/src/channel/*` (per channel-arch refactor) |
| pattern leaf text | parser output | `LqExpr::{Keyword, Phrase, RawString, Regex}` after PRE-CONTRACT-EXT |
| `patterntype` mode | `LqOptionSet::patterntype` | `crates/quanta-index-contract/src/query/options.rs` (post-PRE-CONTRACT-EXT) |
| `case:` mode | `LqOptionSet::case` | same |
| `lang:` filter (per-document) | manifest metadata + `LqFilter::Lang` | `crates/quanta-index-contract/src/query/filters.rs` |
| inline `(?i)` regex flag | parsed by PRE-NORM and lowered to `case:no` | [dsl.md §3.4](../dsl.md) |

### 3.2 Soft inputs (consulted but not authority)

- per-language analyzer registry — owned by this ticket, populated from a static table
- default per-mode case policy — owned by this ticket, sourced from [dsl.md §4 mode matrix](../dsl.md)

### 3.3 Configuration knobs

| Knob | Default | Floor | Source |
|---|---|---|---|
| max token byte length | 256 | 64 | this ticket |
| max tokens per document | 65 536 | 4 096 | this ticket |
| per-document tokenization CPU soft cap | 50 ms | 5 ms | [dsl.md §13](../dsl.md) per-query budget halved for the per-doc unit |
| identifier-split aggressiveness | aggressive (camel + snake + digit) | aggressive only — no off-switch in `LQ/Core-1.0` | this ticket |

---

## 4. Deliverables

### 4.1 New port (core side)

`LexicalNormalizer` trait at `crates/quanta-index-core/src/domains/lexical/outbound.rs`
(co-located with existing `LexicalIndexBuildPort`):

```text
pub trait LexicalNormalizer: Send + Sync {
    fn normalize_document(
        &self,
        text: &str,
        lang: LangId,
        ctx: NormalizationContext,
    ) -> Result<TokenStream, CoreError>;

    fn normalize_query_leaf(
        &self,
        leaf: &LqLeaf,
        opts: &LqOptionSet,
    ) -> Result<TokenStream, CoreError>;

    fn analyzer_id(&self, lang: LangId, opts: &LqOptionSet) -> AnalyzerId;
}
```

Where:

- `TokenStream` is a typed vector of `(token_bytes, start_offset, end_offset, position_increment, token_kind)` tuples
- `AnalyzerId` is a stable identifier persisted into per-generation analyzer manifest (so query side picks the same analyzer that wrote the index)
- `LqLeaf` is the post-PRE-NORM leaf enum (`Keyword`, `Phrase`, `RawString`, `Regex`)
- `NormalizationContext` carries `{ field: Field, write_phase: bool, patterntype: PatternType, case: CaseMode }`

### 4.2 New policy crate region

`crates/quanta-index-core/src/domains/lexical/normalizer.rs` (new file):

- `LexicalNormalizationPolicy` — the canonical implementation; pure (no IO)
- `LangId` enum mirroring the 60-entry list in [dsl.md §6.3](../dsl.md) — see §12 for the DSL-GAP-2 callback
- `AnalyzerId` struct: `{ lang: LangId, patterntype: PatternType, case: CaseMode, identifier_split: bool, version: u16 }`
- `TokenKind` enum: `Word`, `Identifier`, `Numeric`, `Punctuation`, `Boundary`

### 4.3 Adapter impl

`crates/quanta-index-lexical/src/normalizer/` (new module):

- `mod.rs` — wires the adapter side
- `pipeline.rs` — the staged pipeline (§5)
- `splitter.rs` — identifier splitter (camelCase + snake_case + digit boundaries)
- `analyzers.rs` — per-language analyzer registry; Tantivy-`TextAnalyzer` builders pinned per `AnalyzerId`
- `errors.rs` — typed-error mapping into `CoreError` per [dsl.md §12](../dsl.md)

### 4.4 Golden corpus fixture

`crates/quanta-index-lexical/tests/fixtures/normalizer/` (new):

- `code_lines.toml` — hand-curated `(input, lang, patterntype, case) → expected_tokens` rows
- minimum 80 rows covering Rust + Python + TypeScript + JavaScript + Go (Wave-5 STR-01 ship grammars, [feature-scope.md §1.3.4](../feature-scope.md)) plus mixed/non-code
- explicit camelCase row, explicit snake_case row, explicit digit-boundary row, explicit `patterntype:literal` no-split row

### 4.5 Documentation artifacts

- ADR-NN `docs/adr/ADR-017-lexical-normalizer-pipeline.md` (slot pre-seeded; see §12)
- update `crates/quanta-index-lexical/README.md` (or create) noting the analyzer registry surface

### 4.6 No-shim guarantee

Per [CLAUDE.md § Agent change posture](../../../../CLAUDE.md), the existing
stub `LexicalAdapter` in [lib.rs:25–55](../../../../crates/quanta-index-lexical/src/lib.rs#L25)
is **replaced**, not wrapped. No `#[deprecated]` carrier.

---

## 5. Implementation steps (TDD)

Every step lands a **failing test first**, then code. No step bundles "tests + code"; tests precede.

### 5.1 Step 1 — typed errors in place

1. add `LexicalNormalizerError` variants on `CoreError` (or extend per the existing `CoreError::InvalidContract` carrier — confirm with PRE-CONTRACT-EXT):
   - `NormalizerInvalidUtf8 { byte_offset: u64 }`
   - `NormalizerTokenTooLong { token_len: u32, cap: u32 }`
   - `NormalizerDocTooLarge { token_count: u64, cap: u64 }`
   - `NormalizerUnknownLang { lang: String }`
2. unit test in `crates/quanta-index-core/tests/normalizer_errors.rs` (new): each variant round-trips through `CoreError` typed surface.

### 5.2 Step 2 — `LangId` + analyzer registry skeleton

1. failing test: `tests/analyzer_registry.rs` (new) asserts `LangId::Rust → AnalyzerId { lang: Rust, patterntype: Standard, case: Insensitive, identifier_split: true, version: 1 }`.
2. land `LangId` enum with hand-rolled serde (D18 — no derives), populated from [dsl.md §6.3](../dsl.md) ship subset (12 langs Phase 1; remainder reserved — DSL-GAP-2 callback in §12).
3. land `AnalyzerId` struct, hand-rolled serde, `Eq + Hash`.

### 5.3 Step 3 — `LexicalNormalizer` trait + `NoopNormalizer` for wiring

1. failing test: `tests/normalizer_port.rs` (new) asserts the trait compiles into a `dyn LexicalNormalizer` object and the policy crate exports it under `domains::lexical::LexicalNormalizer`.
2. land trait in `outbound.rs`. Add `pub use` in `domains/lexical/mod.rs`.
3. add `NoopNormalizer` (returns `CoreError::NotImplemented`) so non-test sites compile during transition.

### 5.4 Step 4 — UTF-8 + size validation stage

1. failing tests in `tests/normalizer_utf8.rs` (new):
   - non-UTF-8 byte slice → `NormalizerInvalidUtf8` with offset
   - 256+1 byte token → `NormalizerTokenTooLong`
   - 65 537-token document → `NormalizerDocTooLarge`
2. implement first stage of `LexicalNormalizationPolicy::normalize_document` covering steps 1–2 of [dsl.md §10 pipeline](../dsl.md).

### 5.5 Step 5 — NFC + BOM + case folding policy

1. failing tests:
   - NFD input `"a\u{0301}"` → NFC-normalized `"á"` in token output
   - leading BOM `U+FEFF` stripped at offset 0; mid-token BOM preserved (matches [dsl.md §1.1](../dsl.md))
   - `case:no` + `patterntype:standard` → token is lower-cased
   - `case:yes` + `patterntype:standard` → token preserves case
   - `case:no` + `patterntype:literal` → token preserves case (literal mode default is sensitive per [dsl.md §4](../dsl.md))
   - `(?i)foo` regex inline flag lowered into `case:no` by PRE-NORM, reflected in `NormalizationContext`
2. wire `unicode-normalization` crate (NFC only); ban `unicode-case-mapping` heavy paths; use stdlib `to_lowercase()` for ASCII fast path with Unicode fallback.

### 5.6 Step 6 — identifier splitter

1. failing tests in `tests/normalizer_splitter.rs` (new):
   - `getUserName` → `["getUserName", "get", "user", "name"]` (original + parts; position_increment=0 on parts)
   - `parse_query_v2` → `["parse_query_v2", "parse", "query", "v2"]`
   - `HTTPSConnection` → `["HTTPSConnection", "https", "connection"]` (acronym rule: run of upper followed by upper+lower starts a new word at the lower boundary)
   - `foo123bar` → `["foo123bar", "foo", "123", "bar"]`
   - `patterntype:literal` mode → no split; emits only `["foo_bar_baz"]`
2. implement splitter as a pure function. No heuristics — the rules above are the **complete** rule set for `LQ/Core-1.0`. Edge cases land as named tests.

### 5.7 Step 7 — per-language analyzer dispatch

1. failing tests in `tests/normalizer_languages.rs` (new):
   - `lang:rust` + `fn handle(&self)` → token list includes `fn`, `handle`, `self` after stem
   - `lang:python` + `def handle_request(self):` → token list includes `def`, `handle_request`, `handle`, `request`, `self`
   - `lang:go` + `func HandleRequest(ctx Context)` → camelCase split fires
   - `lang:typescript` + `async function fooBar()` → camelCase split fires
   - unknown `lang:foo` value → `NormalizerUnknownLang { lang: "foo" }` (typed, not silent fallback)
2. each language gets a `TextAnalyzer` (Tantivy 0.22) builder. Pin `en_stem` for Rust / Python / TypeScript / JavaScript / Go phase-1; per-language stemmers are deferred — see §12 Q1.

### 5.8 Step 8 — query-leaf normalizer

1. failing tests in `tests/normalizer_query_leaves.rs` (new):
   - `Keyword("fooBar")` + `patterntype:standard` + `case:no` → expanded token set `{fooBar, foobar, foo, bar}` (case folded + split + original preserved)
   - `Phrase("async fn handle")` + `patterntype:standard` → ordered token sequence `[async, fn, handle]` with positions
   - `RawString("C:\\Users\\%")` + any mode → **single literal token**; splitter and case fold off; emits exactly `["C:\\Users\\%"]` (matches [dsl.md §3.3](../dsl.md))
   - `Regex("/fn\\s+handle_\\w+/")` + any mode → emits **no token stream**; returns a `TokenStream::RegexPassthrough(re)` variant the scorer (LEX-01) consumes
2. mapping from leaves to token streams is the single point where pattern semantics meet index semantics. Document the table in `pipeline.rs` module docs.

### 5.9 Step 9 — idempotency property test

1. failing property test in `crates/quanta-index-core/tests/property_normalizer.rs` (new):
   - generator: random UTF-8 strings (proptest `prop::string::string_regex`)
   - assertion: `normalize_document(normalize_document(t).into_text(), …) == normalize_document(t, …)` token-for-token (where `into_text` is the canonical printer of the token stream — see §6 below)
   - 1 000 cases, configurable; CI runs full count
2. property covers Non-Negotiable Invariant 1 (no fuzzy default), Invariant 2 (idempotency), [dsl.md §10.1](../dsl.md).

### 5.10 Step 10 — golden corpus assertion

1. failing integration test `crates/quanta-index-lexical/tests/golden_normalizer.rs` (new):
   - load each TOML row from §4.4
   - run `LexicalNormalizationPolicy::normalize_document`
   - assert byte-for-byte equality with `expected_tokens`
   - on mismatch print a diff plus the row id
2. matches the corpus discipline per [usecase.md §6](../usecase.md).

### 5.11 Step 11 — conformance corpus rows green

Wire UC-LEX-01, UC-LEX-02, UC-LEX-03, UC-LEX-05, UC-LEX-06, UC-LEX-07, UC-LEX-16, UC-LEX-17, UC-LEX-21 through the PRE-CONF runner. Even though LEX-00 does not own the executor, the parser → normalizer stage must produce the right token stream for these rows. PRE-CONF stub engine accepts the token stream and asserts presence/absence of expected tokens.

### 5.12 Step 12 — wire into existing adapter

Replace `LexicalAdapter::build` body in [crates/quanta-index-lexical/src/lib.rs:43–55](../../../../crates/quanta-index-lexical/src/lib.rs#L43) so that the (still-stubbed) Tantivy build path calls `LexicalNormalizationPolicy::normalize_document` per chunk. LEX-00 does **not** complete the build path (that's LEX-03/LEX-04); LEX-00 leaves the build path returning `NotImplemented` but with the normalizer wired so the next ticket consumes it.

### 5.13 Step 13 — lint pass

- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `python3 tools/ci/lint/lint-doc-paths.py`
- `python3 tools/ci/lint/lint-hexagonal-boundaries.py` — adapter→core only; no core→adapter
- `semgrep` rule `rust-no-serde-derive` ([tools/ci/semgrep/rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124)) green — every new type uses hand-rolled serde per D18

---

## 6. Test plan

| Rail | Path | What it asserts |
|---|---|---|
| unit (core, errors) | `crates/quanta-index-core/tests/normalizer_errors.rs` | each `LexicalNormalizerError` variant round-trips |
| unit (core, registry) | `crates/quanta-index-core/tests/analyzer_registry.rs` | `LangId::Rust → AnalyzerId{...}` mapping |
| unit (core, port) | `crates/quanta-index-core/tests/normalizer_port.rs` | trait object `dyn LexicalNormalizer` compiles |
| unit (adapter, utf8) | `crates/quanta-index-lexical/tests/normalizer_utf8.rs` | stage 1 errors (UTF-8, size) |
| unit (adapter, NFC + case) | `crates/quanta-index-lexical/tests/normalizer_nfc_case.rs` | NFC, BOM, case fold per mode |
| unit (adapter, splitter) | `crates/quanta-index-lexical/tests/normalizer_splitter.rs` | camelCase, snake_case, digit, acronym rules; `patterntype:literal` no-split |
| unit (adapter, languages) | `crates/quanta-index-lexical/tests/normalizer_languages.rs` | per-language dispatch; unknown lang → typed error |
| unit (adapter, query leaves) | `crates/quanta-index-lexical/tests/normalizer_query_leaves.rs` | per-leaf token stream mapping; RawString single token; Regex passthrough |
| property (core) | `crates/quanta-index-core/tests/property_normalizer.rs` | idempotency: `normalize(print(normalize(x))) == normalize(x)`; 1 000 random UTF-8 inputs |
| property (core) | same file, second `proptest!` block | length cap: any input ≥ 65 537 tokens → typed error, no panic, no silent truncate |
| golden (adapter) | `crates/quanta-index-lexical/tests/golden_normalizer.rs` | 80+ TOML rows from `tests/fixtures/normalizer/code_lines.toml` |
| conformance (corpus) | PRE-CONF runner | UC-LEX-01, UC-LEX-02, UC-LEX-03, UC-LEX-05, UC-LEX-06, UC-LEX-07, UC-LEX-16, UC-LEX-17, UC-LEX-21 parse → tokenize green; AC-01..03, AC-05..06 reject |
| CI | `just rust-test-unit` + `just rust-test-integration` | per [CLAUDE.md § Testing](../../../../CLAUDE.md) |

### 6.1 Test naming discipline

Every test name maps 1:1 to a claim. Example pattern:

```text
normalizer_splitter_camel_case_splits_get_user_name
normalizer_splitter_literal_mode_does_not_split
normalizer_languages_unknown_lang_returns_typed_error
property_normalizer_idempotent_under_random_utf8_1k
```

Per [implementation-plan.md §8.3 Coverage policy](../implementation-plan.md#83-coverage-policy), a claim with only a smoke test is `blocked`, not `ok`.

### 6.2 Non-test rails

- semgrep `rust-no-serde-derive` ([rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124))
- clippy `-D warnings` workspace-wide
- `cargo deny` (advisories `yanked=deny`, `unmaintained=all`, `unsound=all`)
- `cargo machete` for unused deps
- `lint-hexagonal-boundaries.py` — normalizer port stays core-side; analyzer impl stays adapter-side; no vendor import (`tantivy`) leaks into `quanta-index-core`

---

## 7. Observability

LEX-00 is **pre-OBS-01**, but every later wave's p99 measurement against the normalizer needs at least the following structured surfaces:

### 7.1 Span (emitting from Wave 1 onwards)

`lq.normalize.tokenize` — a child of `lq.normalize` in the span tree of [rfc.md § Observability Requirements](../rfc.md#observability-requirements). Attributes:

| Attribute | Type | Cardinality budget |
|---|---|---|
| `normalize.lang` | string (enum) | 60 |
| `normalize.patterntype` | string (enum) | 5 |
| `normalize.case` | string (enum) | 3 |
| `normalize.identifier_split` | bool | 2 |
| `normalize.token_count` | int | unbounded — recorded as histogram bucket, not label |
| `normalize.elapsed_us` | int | histogram |
| `normalize.error_code` | string (enum) | 5 (the new normalizer error codes) |

### 7.2 Metric

`quanta_lexical_normalizer_tokens_total` — counter, labels `{lang, patterntype, write_phase}`. Cardinality: 60 × 5 × 2 = 600. Within budget.

`quanta_lexical_normalizer_doc_duration_seconds` — histogram, labels `{lang, write_phase}`. Buckets: `[0.001, 0.005, 0.010, 0.050, 0.250, 1.0]` seconds (covers the §9 envelope).

### 7.3 Audit log

LEX-00 does not write to the audit sink. Audit sink lands in OBS-01 (Wave 8); the normalizer attributes attach to the request span when it does.

### 7.4 What is intentionally NOT observable

- raw input text — never logged (PII / secrets concern)
- per-token byte content — never logged
- the analyzer's internal state — never logged

These are runtime invariants. If a future ticket needs to log token content, it must go through a redaction surface that does not exist today — and the request must surface as `blocked` until that surface lands.

---

## 8. Error scenarios

Every scenario surfaces a typed `CoreError` variant. **No silent fallback. No empty `Vec` substitute. No best-effort continuation.** Per [rfc.md § Non-Negotiable Invariants 8](../rfc.md), the error envelope is typed end-to-end.

| Scenario | Condition | Surface | Notes |
|---|---|---|---|
| invalid UTF-8 in document | byte slice fails `str::from_utf8` | `CoreError::InvalidContract` carrying `NormalizerInvalidUtf8 { byte_offset }` | maps to RFC `PARSE_INVALID_UTF8` at the request envelope layer |
| invalid UTF-8 in query leaf | same on the leaf bytes | same | tested in `normalizer_utf8.rs` |
| token exceeds 256-byte cap | single token after splitting > cap | `CoreError::InvalidContract` carrying `NormalizerTokenTooLong { token_len, cap }` | not silently truncated |
| document exceeds 65 536-token cap | token count > cap | `CoreError::InvalidContract` carrying `NormalizerDocTooLarge { token_count, cap }` | applies write-phase only; query-phase docs are always short |
| unknown `lang:` value | post-alias-resolve lang not in registry | `CoreError::InvalidContract` carrying `NormalizerUnknownLang { lang }` | matches [dsl.md §6.3](../dsl.md) ("Unknown -> `PARSE_INVALID_FILTER_VALUE{filter=lang}`") — surface at the planner layer with this typed payload |
| `patterntype:` value outside mode matrix | parser caught most of this; defensive layer | `CoreError::InvalidContract` carrying `NormalizerUnknownPatternType { value }` | should never fire — guarded by PRE-NORM. If it fires, log as a bug; do not auto-coerce |
| RawString in mode lacking ngram backend | `patterntype:standard` + RawString leaf + active backend is plain Tantivy chunk index | `CoreError::NotImplemented` carrying `NormalizerRawStringRequiresNgram` | matches [dsl.md §3.3](../dsl.md): "If the active backend does not expose such an index, planning fails with `PLAN_LIMIT_EXCEEDED{dimension=rawstring-backend, limit=0}` — not silently degraded to phrase." |
| Regex leaf at document tokenization | document path called `normalize_query_leaf` with `Regex` and asked for tokens | `CoreError::InvalidContract` carrying `NormalizerRegexNotTokenized` | regex leaves return `TokenStream::RegexPassthrough`, never tokens; if a caller still requests tokens, that's a contract bug |
| analyzer-id mismatch at query time | the per-generation analyzer manifest pinned `AnalyzerId v=2`; query side computed `v=1` | `CoreError::NotReady` carrying `NormalizerAnalyzerMismatch { wrote: AnalyzerId, expected: AnalyzerId }` | fail-closed; matches RFC § Monotonicity rules — never serve stale analyzer reads |
| panic from underlying analyzer | Tantivy `TextAnalyzer::token_stream` panics under adversarial input | wrap in `std::panic::catch_unwind`, surface as `CoreError::InvalidContract` carrying `NormalizerAnalyzerPanic { lang }` | this is a defense-in-depth — Tantivy 0.22 has not panicked in our corpus, but invariant 8 forbids untyped errors |

### 8.1 What is forbidden

- empty `TokenStream` returned for any input that the contract says must produce tokens
- a `?` operator that swallows a `Result::Err` into a `default()` token list
- a `.unwrap_or_default()` on any `Result` in the normalizer module (enforced by clippy `disallowed-methods` rail per [implementation-plan.md §1.4](../implementation-plan.md#14-claimability-rule))
- silent fallback from `lang:rust` to a generic analyzer when the rust analyzer is "not yet ready"

---

## 9. Performance envelope

LEX-00 owns the per-document and per-query-leaf tokenization budget. Later tickets (LEX-01 scorer, LEX-05 executor) absorb the rest of the request budget.

| Operation | Target | Source | Measurement |
|---|---|---|---|
| per-document tokenization, 1 KB synthetic code line | p99 ≤ 5 ms | task spec | criterion bench (added in LEX-01; LEX-00 lands a smoke timing assertion in `golden_normalizer.rs` that prints elapsed and warns if > 10 ms) |
| per-query-leaf tokenization, max-realistic leaf (≤ 64 bytes) | p99 ≤ 200 µs | derived from RFC § Latency SLO single-repo p50 ≤ 50 ms minus downstream stages | smoke timing |
| per-document tokenization, 64 KB blob (worst-case chunk size) | p99 ≤ 50 ms | scaled linearly from the 1 KB target | smoke timing |
| analyzer id lookup | p99 ≤ 1 µs | static map lookup | smoke timing |
| identifier splitter, per token | p99 ≤ 2 µs | hot path; no allocation per token (use a reusable scratch buffer) | smoke timing |

### 9.1 Bench discipline

LEX-00 does **not** ship a criterion bench (per [implementation-plan.md §8.1](../implementation-plan.md#81-per-ticket-rail-matrix), the bench row is blank for LEX-00). LEX-01 ships `pre_norm_parse_bench` and includes a normalizer timing slice. LEX-00 lands the smoke timing assertions described above as a guard against pathological regressions (e.g. accidentally O(n²) splitter).

### 9.2 Memory

- per-document scratch buffer reused via `thread_local!` or a pool — pick one; not specified. Whichever, no per-token `String::new()`.
- pool peak size cap: 1 MiB per worker (256 byte max token × 4 096 worst-case tokens × overhead). Exceeding the cap is a bug, not a soft signal.

### 9.3 Concurrency

The normalizer is `Send + Sync`. Internally:

- analyzer registry is `Arc<HashMap<…>>` populated once at startup
- per-call scratch buffers are thread-local
- no global locks, no `RefCell` shared across threads

Loom not required at LEX-00 (no shared mutable state). Loom enters at LEX-04 (writer coordinator).

---

## 10. Risks

| ID | Risk | Probability | Impact | Mitigation | Owner |
|---|---|---|---|---|---|
| LEX00-R1 | Identifier splitter behavior diverges from Sourcegraph's silent expectation | M | M | hand-curated golden corpus (§4.4) plus parity column on every UC-LEX-* row; flip to `SG~` if divergence is unavoidable; flip to `SG!` only with RFC amendment | LEX-00 author + feature-scope owner |
| LEX00-R2 | Per-language analyzer choice changes the index byte layout, invalidating prior generation | H | M | persist `AnalyzerId` in the per-generation manifest; query side fails closed with `NormalizerAnalyzerMismatch` on skew; matches RFC § Monotonicity rules | LEX-00 author |
| LEX00-R3 | NFC normalization performance hit on Asian-language documents | M | L | NFC is `O(n)` over codepoints; benchmark a CJK fixture once it lands; if hit > 10 ms p99 on 1 KB, gate Unicode-heavy normalization behind `lang:` (skip for `lang:rust` etc. where input is ASCII) — but **never** silently skip; the gating is explicit | LEX-00 author |
| LEX00-R4 | `RawString` leaf requires a trigram/ngram backend not present in current Tantivy chunk index | H | M | LEX-00 surfaces `NormalizerRawStringRequiresNgram` typed error; LEX-03 / LEX-06 decide whether to ship a trigram sibling shard; [dsl.md §3.3](../dsl.md) authorizes this. Phase-1 corpus row UC-LEX-04 stays `blocked` until trigram lands | LEX-00 author + LEX-03 owner |
| LEX00-R5 | Idempotency invariant breaks under unicode edge case (combining marks, RTL marks) | M | H | property test (1 000 cases) is the primary guard; add explicit RTL fixture row; if it breaks, treat as a bug, not a feature gap | LEX-00 author |
| LEX00-R6 | Tantivy 0.22 `TextAnalyzer` API drift mid-program | L | M | pin to `=0.22.x` (predecessor R2); audit on every `cargo update` PR | LEX-00 author |
| LEX00-R7 | Stop-token policy diverges from Sourcegraph (we ship `en_stem` only) | M | M | document stop-token set as part of `AnalyzerId v=1`; bump version on any change; corpus row drift triggers `SG~` parity downgrade | LEX-00 author |
| LEX00-R8 | The `domains/lexical/` path is "untracked-new (channel-arch)" per task brief | M | M | LEX-00 lands the normalizer port in whichever path is current at start-of-ticket; if the channel-arch path is not green by then, this ticket blocks on G-CONTROL-LOC ([implementation-plan.md §11](../implementation-plan.md#11-open-questions-and-human-decisions)). See §12 Q1 | LEX-00 author |
| LEX00-R9 | Analyzer-id versioning collision with predecessor `en_stem`-pinned data | L | H | new `AnalyzerId v=1` is intentionally distinct from any pre-LEX-00 implicit choice; any pre-existing index without an embedded `AnalyzerId` fails closed at query time; matches breaking-first posture | LEX-00 author |
| LEX00-R10 | DSL `lang:` enum has 60 entries; LEX-00 ships analyzers for 5 (Phase-1 ship set per [feature-scope.md §1.3.4](../feature-scope.md)) | H | L | Phase-1 ship set: rust, python, typescript, javascript, go. Other 55 entries parse but fail at normalize time with `NormalizerUnknownLang { lang }` until a follow-up lands a default-language analyzer policy. DSL-GAP-2 callback in §12 | LEX-00 author + feature-scope owner |

---

## 11. DoD

Each row cites an evidence artifact. Per [implementation-plan.md §1.4](../implementation-plan.md#14-claimability-rule), no DoD row claims `ok` without a provable artifact. All 17 rows shipped (57 tests in `quanta-index-lq-text-norm`).

| # | Status | Item | Evidence artifact |
|---|---|---|---|
| 1 | ✓ shipped | `LexicalNormalizer` trait lands in core | `crates/quanta-index-core/src/domains/lexical/outbound.rs` exports `LexicalNormalizer`; `tests/normalizer_port.rs::trait_object_compiles` green |
| 2 | ✓ shipped | `LangId` + `AnalyzerId` types ship with hand-rolled serde | `crates/quanta-index-core/src/domains/lexical/normalizer.rs`; semgrep `rust-no-serde-derive` green; `tests/analyzer_registry.rs::known_lang_maps_to_expected_analyzer_id` green |
| 3 | ✓ shipped | UTF-8 + size validation stage | `crates/quanta-index-lq-text-norm/tests/normalizer_utf8.rs::invalid_utf8_returns_typed_error`, `::token_too_long_returns_typed_error`, `::doc_too_large_returns_typed_error` green |
| 4 | ✓ shipped | NFC + BOM + case fold per mode (NFC active via `unicode-normalization`; NFKC variant applies NFKC) | `crates/quanta-index-lq-text-norm/tests/normalizer_nfc_case.rs` 6 named tests green (NFC, BOM-strip-at-0, mid-token-BOM-preserved, case-no-standard, case-yes-standard, case-no-literal) |
| 5 | ✓ shipped | Identifier splitter | `crates/quanta-index-lq-text-norm/tests/normalizer_splitter.rs` 6 named tests green (camel, snake, digit-boundary, acronym, literal-no-split, mixed) |
| 6 | ✓ shipped | Per-language analyzer dispatch | `crates/quanta-index-lq-text-norm/tests/normalizer_languages.rs` 5 named tests green (rust, python, typescript, javascript, go) + `unknown_lang_returns_typed_error` |
| 7 | ✓ shipped | Query-leaf normalizer | `crates/quanta-index-lq-text-norm/tests/normalizer_query_leaves.rs` 4 named tests green (Keyword expansion, Phrase ordering, RawString single token, Regex passthrough) |
| 8 | ✓ shipped | Idempotency property | `crates/quanta-index-core/tests/property_normalizer.rs::normalize_is_idempotent_under_random_utf8` green; 1 000 cases configurable |
| 9 | ✓ shipped | Golden corpus | `crates/quanta-index-lq-text-norm/tests/golden_normalizer.rs::golden_corpus_matches`; fixture `tests/fixtures/normalizer/code_lines.toml` with ≥ 80 rows |
| 10 | ✓ shipped | UC-LEX-01..03, 05..07, 16..17, 21 green in PRE-CONF | conformance runner output per row; runs as `cargo test -p quanta-index-contract --test lq_conformance` |
| 11 | ✓ shipped | AC-01..03, AC-05..06 reject with typed code | same runner |
| 12 | ✓ shipped | Performance smoke | `golden_normalizer.rs::performance_smoke_1kb_under_10ms` green |
| 13 | ✓ shipped | Adapter wiring | normalizer invoked at the build entry point; downstream `build` paths still gated on LEX-03/04 |
| 14 | ✓ shipped | Documentation (incl. "byte-offset into normalized text" contract) | `docs/adr/ADR-017-lexical-normalizer-pipeline.md` populated; analyzer registry documented |
| 15 | ✓ shipped | Lint rails green | `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`; `python3 tools/ci/lint/lint-doc-paths.py`; `python3 tools/ci/lint/lint-hexagonal-boundaries.py`; semgrep; `cargo deny`; `cargo machete` |
| 16 | ✓ shipped | RFC § Claim Discipline §1 (parser conformance leg) partially provable through normalizer | structured agent output validates against `tools/ci/agent/agent_output.schema.json` for UC-LEX-01..03 |
| 17 | ✓ shipped | No silent fallback regression | code review checklist confirms zero `.unwrap_or_default()` / `.ok()` / `.unwrap_or_else(|_| …)` on normalizer paths; clippy disallowed-methods rail green |

---

## 12. Open questions

| Q-ID | Question | Source | Default answer | Forcing function |
|---|---|---|---|---|
| LEX00-Q1 | Owner crate path: is the normalizer port located at `crates/quanta-index-core/src/domains/lexical/` (channel-architecture refactor home) or at the historical `crates/quanta-index-lexical/` home? | task brief — "note `crates/quanta-index-core/src/domains/lexical/` is untracked-new" | core-side: port lives at `domains/lexical/outbound.rs`; adapter-side impl lives at `quanta-index-lexical/src/normalizer/`. Resolves G-CONTROL-LOC for the normalizer slice. | ticket start |
| LEX00-Q2 | Per-language stemmer dispatch — do Python / TypeScript / JavaScript / Go use `en_stem`, or a per-language stemmer pack? | Tantivy 0.22 stemmer set | Phase-1: all five Phase-1 langs share `en_stem`. Per-language stemmer is a Phase-2 follow-up; gated on stemmer pack availability in Tantivy 0.22 | LEX-00 start |
| LEX00-Q3 | Stop-token list — Sourcegraph applies a tiny default list; do we? | feature-scope.md (implicit) | Phase-1: **no** stop-token list. Empty set. Document in ADR-017 with version `v=1`. Adding a stop-token list is a `v=2` bump | LEX-00 start |
| LEX00-Q4 | DSL-GAP-2 (60-entry `lang:` enum vs 5-lang ship set) | [implementation-plan.md § Appendix A.4](../implementation-plan.md#a4-dslmd-follow-ups) | parser accepts all 60; normalizer ships 5 + a 7-entry alias resolution table (per [dsl.md §6.3](../dsl.md)); rest fail at normalize time with typed `NormalizerUnknownLang`. The remaining 55 stay in the enum so parser conformance corpus does not regress | LEX-00 start |
| LEX00-Q5 | Token position semantics — does the identifier splitter emit parts at the same position as the original, or at incrementing positions? | this ticket | parts at position-increment 0 (same position as original); enables phrase queries that span original or split forms | LEX-00 start |
| LEX00-Q6 | Is `AnalyzerId` persisted in the per-generation manifest by LEX-00 or LEX-04? | task brief | LEX-00 defines the type and emits it; LEX-04 persists it in the manifest catalog. If LEX-04 is not green when an integration test needs persistence, the test stays `blocked`, not `ok` | LEX-04 start |
| LEX00-Q7 | Cancellation cooperative-checkpoint — does the normalizer honor tokio cancel signals? | [rfc.md § 6.5 § cancellation](../rfc.md) | Phase-1 normalizer is synchronous; cancellation is honored at the executor layer (LEX-05). The normalizer's bounded per-doc work (50 ms cap) makes intra-document cancel a non-goal | LEX-05 start |
| LEX00-Q8 | RawString backend (Q3 callback) — does LEX-00 ship a trigram shard? | [dsl.md §3.3](../dsl.md) | **no** — LEX-00 surfaces the typed error. Trigram lives in LEX-03 scope. UC-LEX-04 stays `blocked` until that lands | LEX-03 start |

---

## 13. References

### 13.1 Primary

- [rfc.md](../rfc.md) — §Canonical Grammar Semantics → Pattern semantics; §Non-Negotiable Invariants 1, 2, 8, 11; §Observability Requirements; §Monotonicity rules
- [dsl.md](../dsl.md) — §1.1 character set; §1.5 quoted strings; §3 pattern leaf semantics; §4 patterntype matrix; §6.3 lang enum; §10 normalization pipeline; §12 error taxonomy; §13 limits; §16 invariants
- [feature-scope.md](../feature-scope.md) — §1.1.6 patterntype modes; §1.3.4 ship language set; §4.7 flagged gaps
- [usecase.md](../usecase.md) — UC-LEX-01..06, UC-LEX-16..17, UC-LEX-21; AC-01..03, AC-05..06; §0 result-shape vocabulary; §6 conformance gating
- [implementation-plan.md](../implementation-plan.md) — §5.4 LEX-00 DoD; §3 dep graph; §8 test strategy; §11 open questions; Appendix A gap callbacks

### 13.2 Repository sources

- [crates/quanta-index-lexical/src/lib.rs](../../../../crates/quanta-index-lexical/src/lib.rs) — current stub adapter
- [crates/quanta-index-core/src/domains/lexical/mod.rs](../../../../crates/quanta-index-core/src/domains/lexical/mod.rs) — domain home
- [crates/quanta-index-core/src/domains/lexical/outbound.rs](../../../../crates/quanta-index-core/src/domains/lexical/outbound.rs) — port host
- [crates/quanta-index-contract/src/results/candidates.rs](../../../../crates/quanta-index-contract/src/results/candidates.rs) — `LexicalCandidate`
- [crates/quanta-index-contract/src/query/expression.rs](../../../../crates/quanta-index-contract/src/query/expression.rs) — `LqExpr`
- [crates/quanta-index-contract/src/query/options.rs](../../../../crates/quanta-index-contract/src/query/options.rs) — `LqOptionSet`
- [tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive` rule

### 13.3 Governance

- [CLAUDE.md](../../../../CLAUDE.md) — agent change posture; D18 serde-derive ban; verification rule
- [AGENTS.md](../../../../AGENTS.md) — read-order; conflict rule
- [AGENT_RULE_CATALOG.md](../../../../AGENT_RULE_CATALOG.md) — rule catalog

### 13.4 Sibling tickets

- LEX-01 (next) — IDF / scoring foundation; consumes the token stream LEX-00 produces
- LEX-03 — lexical authority unification; sibling shards reuse the analyzer registry
- LEX-04 — incremental write packet; persists the `AnalyzerId` per generation
- LEX-06 — ranking + explain; consumes BM25 + adjacency-link from the token stream

### 13.5 Cross-repo SSOTs

- Producer handoff: [docs/ssot/producer-handoff.md](../../../ssot/producer-handoff.md)
- Ticket index (downstream-migration follow-up tracked under §3.6): [INDEX.md](INDEX.md)
