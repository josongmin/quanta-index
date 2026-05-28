# PRE-NORM: DSL parser + canonical normalizer + CBOR canonical encoding + SHA-256 hash

| field | value |
|---|---|
| Status | shipped |
| Crate | `quanta-index-lq-norm` |
| Tests | 75 |
| Last verified | 2026-05-25 |
| Wave | 0 |
| Owner crate(s) | `quanta-index-lq-norm` (G-CONTROL-LOC resolved: standalone crate) |
| Touches contract | no — consumes the carrier types landed by PRE-CONTRACT-EXT (`LqQuery`, `LqExpr`, `LqFilter`, `LqOptionSet`, `LqDirective`, `LqCanonicalHashV1`, `LexicalErrorCode`, `TokenSpan`, `LqQueryVersion`). **Breaking change shipped:** `LqLeaf::StructuralBlock` payload changed `String → LqStructuralBlock`. |
| Size | L (~2 weeks engineer-time; parser+normalizer+hasher; 10 `PARSE_*` codes; idempotency + cross-arch property tests) |
| Depends on | PRE-CONTRACT-EXT (every typed carrier the parser emits; every `PARSE_*` code the parser surfaces) |
| Blocks | PRE-CONF (corpus runner consumes `parse → normalize → hash`), LEX-00 (invariant checks point at this parser), LEX-01 (canonical query AST + planner build on top of this), every executor path Wave 3+ |

> All five originally deferred items completed at ship: predicate filters, structural inner sub-grammar, `regex_guard`, leading `(?i)` strip in normalize, and criterion benches. Crate name resolved (G-CONTROL-LOC default position taken).

## 1. Purpose

Stand up the **only** path from raw query text to canonical AST + canonical hash. Without this, LEX-01 cannot start (its DoD requires "100% of UC-* rows parse to canonical AST" per [implementation-plan.md § 5.5](../implementation-plan.md)) and PRE-CONF cannot run (corpus runner pipelines `parse → normalize → hash`).

This ticket lands three deliverables that operate **in series** as a one-pass pipeline ([dsl.md § 10](../dsl.md)):

1. `LqParser::parse(raw: &str) -> Result<LqQuery, LexicalQueryError>` — tokenizer + recursive-descent parser implementing [dsl.md § 2](../dsl.md) EBNF.
2. `LqCanonical::normalize(q: LqQuery) -> LqQuery` — 9-step canonical pass per [dsl.md § 10](../dsl.md), with the hard idempotency invariant `normalize(print(normalize(q))) == normalize(q)`.
3. `LqCanonicalHash::compute(q: &LqQuery) -> LqCanonicalHashV1` — RFC 8949 §4.2.1 deterministic CBOR + SHA-256, carrier landed by PRE-CONTRACT-EXT.

It must do so without **any** silent failure: every parse / normalize / hash failure maps to one of the 10 `PARSE_*` codes (or 2 of the `PLAN_*` codes when the budget cap blows compile time) from the closed `LexicalErrorCode` set ([rfc.md § Error Code Taxonomy](../rfc.md)).

## 2. Background

Current state vs. RFC-mandated state:

- **Parser**: absent. There is no parser today; `LqExpr::Raw(String)` ([crates/quanta-index-contract/src/query/expression.rs:106](../../../../crates/quanta-index-contract/src/query/expression.rs#L106)) carries an unparsed payload by convention. [implementation-plan.md § 2.4](../implementation-plan.md) confirms "LEX-00 normalization pipeline absent" and "LEX-01 IDF stats per generation absent" — neither the parser nor the canonicalizer exist.
- **Canonical AST**: shape exists post-PRE-CONTRACT-EXT (with `lq_version`, typed filter payloads, `LqExpr::Phrase/RawString/Regex/StructuralBlock` variants). This ticket populates it.
- **Canonical hash carrier**: `LqCanonicalHashV1` lands in PRE-CONTRACT-EXT (per [dsl.md § 11.3](../dsl.md) placeholder). This ticket computes it.
- **EBNF**: fully specified in [dsl.md § 2.1–§ 2.5](../dsl.md). Token kinds in [dsl.md § 1.7](../dsl.md). Pattern leaf semantics in [dsl.md § 3](../dsl.md). Mode matrix in [dsl.md § 4](../dsl.md) (default `standard`). Filter semantics in [dsl.md § 6](../dsl.md). Predicate sub-grammar in [dsl.md § 7](../dsl.md). Structural sub-grammar in [dsl.md § 8](../dsl.md). Directive grammar in [dsl.md § 9](../dsl.md). Normalization rules in [dsl.md § 10](../dsl.md). Canonical hash in [dsl.md § 11](../dsl.md). Limits / budgets in [dsl.md § 13](../dsl.md).
- **Error taxonomy**: [dsl.md § 12](../dsl.md) lists 12 codes covering parse + plan layer. Parse-layer subset of 10 codes is owned end-to-end by this ticket (`PLAN_*` codes are touched only when budget exceeded at compile-time pre-check; PRE-NORM emits `PLAN_LIMIT_EXCEEDED` for [dsl.md § 3.4](../dsl.md) RE2 NFA cap and [dsl.md § 13](../dsl.md) AST depth / fan-out caps).
- **D18 / serde**: this ticket touches no serde derives; the carrier types it consumes already have hand-impl serde per PRE-CONTRACT-EXT.

Honest gap call: **physical home for this crate is unresolved** ([implementation-plan.md § 11 G-CONTROL-LOC](../implementation-plan.md)). The ticket is written to be locatable in either a fresh `quanta-index-lq-norm` crate (preferred — keeps parser separate from policy validators in `quanta-index-core`) or as a `quanta_index_core::lq::norm` module. ADR-001 ([implementation-plan.md § 10](../implementation-plan.md)) resolves at this ticket's start.

## 3. Inputs (preconditions)

- Existing surfaces this ticket reads:
  - [crates/quanta-index-contract/src/](../../../../crates/quanta-index-contract/src/) — every carrier type (post-PRE-CONTRACT-EXT)
  - [dsl.md](../dsl.md) — the entire grammar / normalization / hash contract
  - [usecase.md § 2](../usecase.md) — 85 UC-* rows must parse; [usecase.md § 4](../usecase.md) — 15 AC-* rows must fail with the expected code
- Prior-ticket deliverables required green (with proof artifact names):
  - PRE-CONTRACT-EXT DoD §11.1–§11.12 all green (build, semgrep, error-code set, lq_version mandatory, tenant/user mandatory, every new type round-trips, cross-arch CBOR identity, no dual surface, miri clean, handoff doc, deny / machete green)
- Contract types in scope (read-only here; this ticket emits but does not modify): `LqQuery`, `LqExpr`, `LqFilter`, `LqOptionSet`, `LqDirective`, `LqQueryVersion`, `LqCanonicalHashV1`, `LexicalErrorCode`, `LexicalQueryError`, `LexicalErrorPayload`, `TokenSpan`, `Span`, `BoostFactor`, `CountBound`, `CaseOption`, `PatternType`, `IndexMode`, `ForkMode`, `ArchivedMode`, `VisibilityMode`, `BridgeTarget`, `BridgeScope`, `BridgeResultHandle`.

## 4. Deliverables

### 4.1 New files (physical location is one of two — placeholder path uses `crates/quanta-index-lq-norm/src/`; ADR-001 decides)

- file: `crates/quanta-index-lq-norm/src/lib.rs` (or `crates/quanta-index-core/src/lq/mod.rs`) — module root; pub-uses `LqParser`, `LqCanonical`, `LqCanonicalHash`.
- file: `.../src/tokenizer.rs` — `Tokenizer::next(&mut self) -> Result<Token, LexicalQueryError>`; `Token { kind: TokenKind, span: TokenSpan }`; `TokenKind` per [dsl.md § 1.7](../dsl.md). UTF-8 validation at construction; BOM strip per [dsl.md § 1.1](../dsl.md); 16 KiB size cap per [dsl.md § 1.6](../dsl.md).
- file: `.../src/ast.rs` — internal AST mirrored against `LqQuery` carrier; conversion `IntoLqQuery for InternalAst`.
- file: `.../src/parser.rs` — recursive-descent parser implementing the EBNF in [dsl.md § 2.1](../dsl.md) (core), § 2.2 (history), § 2.3 (structural), § 2.4 (runtime), § 2.5 (bridge). Public surface: `pub fn parse(raw: &str) -> Result<LqQuery, LexicalQueryError>`.
- file: `.../src/predicates.rs` — predicate sub-parser per [dsl.md § 7.1–§ 7.3](../dsl.md). Arg type coercion table per [dsl.md § 7.2](../dsl.md).
- file: `.../src/structural.rs` — structural sub-parser per [dsl.md § 8.1–§ 8.6](../dsl.md). Lexer-stage alias normalization (`:[X]` → `$X`, `:[...ARGS]` → `$...ARGS`) so the AST never carries the alias form.
- file: `.../src/regex_guard.rs` — RE2 dialect filter rejecting backref / lookahead / lookbehind / possessive / named-capture-ref / mid-pattern inline flags ([dsl.md § 3.4](../dsl.md)); NFA state-count upper-bound estimator using `regex_syntax::hir::analysis` ([dsl.md § 3.4](../dsl.md) cap = 100_000).
- file: `.../src/normalize.rs` — 9-step canonical pass per [dsl.md § 10](../dsl.md):
  - step 1 UTF-8 validate (consumed by tokenizer already; included for traceability)
  - step 2 size check
  - step 3 BOM strip
  - step 4 tokenize
  - step 5 parse
  - step 6 desugar (`repo:foo@bar` → `repo:foo rev:bar`, `:[X]` → `$X`, `:[...ARGS]` → `$...ARGS`, `(?i)pat` → `pat` with `case:no` option, lang aliases per [dsl.md § 6.3](../dsl.md))
  - step 7 alias resolve (`path:` vs `file:` → one `FileFilter` with `scope` tag; absent `patterntype` → `Mode::Standard`)
  - step 8 constant-fold (`NOT NOT X` → `X`, n-ary AND/OR collapse, empty AND()/OR() removal, OR-within-kind filter merge, dedup; **OR-within-kind sorts pattern strings lexicographically** before merge per [dsl.md § 10.2](../dsl.md) order-stable rule)
  - step 9 emit canonical `LqQuery` with `lq_version = "1.0-pre"` (then bumped to `"1.0"` in LEX-01 wave per [implementation-plan.md § 7.1](../implementation-plan.md))
- file: `.../src/printer.rs` — `LqPrinter::print(q: &LqQuery) -> String` — canonical ASCII reproduction per [dsl.md § 10.1](../dsl.md). Idempotency invariant: `normalize(parse(print(normalize(parse(s))))?)?? == normalize(parse(s))?`.
- file: `.../src/cbor_canonical.rs` — RFC 8949 §4.2.1 canonical encoder: sorted map keys, shortest int encoding, definite lengths only, no tags except the explicit version tag at the root array; rejects f32 NaN / -0 at encode boundary (carrier-level rejection lands in PRE-CONTRACT-EXT `BoostFactor`).
- file: `.../src/hash.rs` — `LqCanonicalHash::compute(q: &LqQuery) -> LqCanonicalHashV1` — wraps cbor_canonical + SHA-256 + populates carrier (digest_hex lowercase no-separator, dsl_version, created_at_unix_ms).
- file: `.../src/limits.rs` — budget enforcement: max query length 16 KiB ([dsl.md § 1.6](../dsl.md)), max AST depth 32 ([dsl.md § 13](../dsl.md)), max boolean fan-out 64, max filter count per kind 32, max regex NFA 100_000, max structural pattern node count 256, max top-K 10_000. Floor checks per [dsl.md § 13](../dsl.md).

### 4.2 New Rust types — manual serde impls (D18) where serialized

Most types in this ticket are **internal-only** (parser state, token streams) and need no serde. Public types are the carriers already landed by PRE-CONTRACT-EXT with hand-impl serde. The two exceptions that need hand-impl serde live in this ticket:

```rust
pub struct LexicalQueryError {       // already in PRE-CONTRACT-EXT; this ticket populates payloads
    pub code: LexicalErrorCode,
    pub message: String,
    pub position: Option<TokenSpan>,
    pub payload: Option<LexicalErrorPayload>,
}

pub enum LexicalErrorPayload {        // already in PRE-CONTRACT-EXT; this ticket constructs variants
    LexExpected { expected: String },
    SyntaxExpected { expected: String, found: String },
    InvalidRegex { dialect_violation: String },
    InvalidUtf8 { byte_offset: u32 },
    Oversized { observed_bytes: u32, budget_bytes: u32 },
    ForbiddenSyntax { construct: String },
    UnknownFilter { filter_name: String },
    InvalidFilterValue { filter_name: String, value: String, expected_grammar: String },
    InvalidPatternType { value: String, allowed: Vec<String> },
    UnsupportedCombo { constructs: Vec<String> },
    PlanLimitExceeded { dimension: String, budget: u64, observed: u64 },
    // ... (29 - 11 = 18 more variants for PLAN_*/EXEC_*/STATE_*/AUTHZ_*/BRIDGE_*, all hand-impl)
}
```

### 4.3 New functions / methods (full signatures)

```rust
pub fn parse(raw: &str) -> Result<LqQuery, LexicalQueryError>;

pub fn normalize(q: LqQuery) -> Result<LqQuery, LexicalQueryError>;
//                                  ^ result not infallible:
//   constant-fold may surface PLAN_LIMIT_EXCEEDED for depth/fan-out caps
//   regex_guard may surface PLAN_LIMIT_EXCEEDED for NFA state cap
//   if the parser already passed but constant-fold reveals over-budget shape

pub fn print(q: &LqQuery) -> String;     // infallible per dsl.md § 10.1

pub fn compute_hash(q: &LqQuery) -> LqCanonicalHashV1;
//   infallible at this layer; q must already be normalized — debug_assert
//   in DEBUG builds that q == normalize(q.clone())?

pub fn parse_normalize_hash(raw: &str)
    -> Result<(LqQuery, LqCanonicalHashV1), LexicalQueryError>;
//   convenience pipeline used by PRE-CONF and Wave-3+ executors
```

### 4.4 New error codes (where they fire)

These codes are **introduced as enum variants** by PRE-CONTRACT-EXT but their **emission sites** land here:

| Code | Emission site (this ticket) | Payload |
|---|---|---|
| `PARSE_INVALID_UTF8` | `tokenizer::Tokenizer::new` when input fails `std::str::from_utf8` | `InvalidUtf8 { byte_offset }` |
| `PARSE_OVERSIZED` | `tokenizer::Tokenizer::new` when `raw.len() > 16384` | `Oversized { observed_bytes, budget_bytes: 16384 }` |
| `PARSE_LEX_ERROR` | `tokenizer::Tokenizer::next` on unterminated quote, bare-position BOM, unknown escape | `LexExpected { expected }` |
| `PARSE_SYNTAX_ERROR` | `parser::Parser::*` on grammar miss (e.g., `(` without matching `)`, trailing operator) | `SyntaxExpected { expected, found }` |
| `PARSE_INVALID_REGEX` | `regex_guard::compile` on RE2-dialect violation other than the forbidden constructs | `InvalidRegex { dialect_violation }` |
| `PARSE_FORBIDDEN_SYNTAX` | `regex_guard::compile` (backref / lookahead / lookbehind / possessive / named-capture-ref / mid-pattern flags) **and** `parser::parse_at_token` (generic `@` outside `repo:<pat>@rev` sugar) | `ForbiddenSyntax { construct }` |
| `PARSE_UNKNOWN_FILTER` | `parser::parse_filter` when `filter_name` not in the closed registry | `UnknownFilter { filter_name }` |
| `PARSE_INVALID_FILTER_VALUE` | `parser::parse_filter_value` when value fails its pinned grammar ([dsl.md § 6.2](../dsl.md)) | `InvalidFilterValue { filter_name, value, expected_grammar }` |
| `PARSE_INVALID_PATTERNTYPE` | `parser::parse_filter` when `patterntype:` value not in `{literal,keyword,standard,regexp,structural}` or seen twice | `InvalidPatternType { value, allowed }` |
| `PARSE_UNSUPPORTED_COMBO` | `normalize::step_8_constant_fold` (e.g., `into:codeql` with empty expression, `into:codeql` with `type:diff` per [dsl.md § 9.3](../dsl.md)); `parser::parse_query` when two `type:` filters present | `UnsupportedCombo { constructs }` |
| `PLAN_LIMIT_EXCEEDED` | `limits::*` (depth>32 / fan-out>64 / filter-count-per-kind>32 / NFA>100_000 / structural-nodes>256 / top-K>10_000) | `PlanLimitExceeded { dimension, budget, observed }` |

Every code carries a `position: Option<TokenSpan>` populated from the token that triggered the failure.

### 4.5 New invariants added to RFC § Non-Negotiable

This ticket does not amend the RFC. It enforces the existing invariants:

- RFC item 1 (no generic `@`) — `parser::parse_at_token` surfaces `PARSE_FORBIDDEN_SYNTAX`.
- RFC item 2 (no fuzzy-by-default) — there is no parse rule for `~token`; surfaces `PARSE_LEX_ERROR` or `PARSE_FORBIDDEN_SYNTAX` depending on whether the `~` is lex-illegal or pattern-position.
- RFC item 8 (no untyped error response) — every failure path returns `Result<_, LexicalQueryError>` with a closed `LexicalErrorCode`; there is **no `String`-returning error path** in the public surface.
- RFC item 10 (no unbounded memory in regex/structural matching) — `regex_guard` enforces NFA cap pre-compile; `limits` enforces structural node cap.
- DSL § 16 invariant 7 (normalization is idempotent) — property test (§6.3).
- DSL § 16 invariant 8 (canonical hash is stable across processes) — property test + cross-arch CI matrix (§6.3).

### 4.6 Items deleted

None directly. This ticket adds new modules. Breaking-first work was done by PRE-CONTRACT-EXT.

### 4.7 New test files

- `.../tests/parser_unit_per_token_kind.rs` — one named test per `TokenKind` in [dsl.md § 1.7](../dsl.md).
- `.../tests/parser_unit_per_grammar_production.rs` — one test per non-terminal in [dsl.md § 2.1](../dsl.md) (Query, Expression, OrExpression, AndExpression, NotExpression, Atom, PatternLeaf, Keyword, Phrase, RawString, Regex, Filter, FilterName, FilterValue, PredicateCall, Directive).
- `.../tests/parser_unit_history_filters.rs` — per [dsl.md § 2.2](../dsl.md).
- `.../tests/parser_unit_structural.rs` — per [dsl.md § 2.3](../dsl.md), § 8.
- `.../tests/parser_unit_runtime.rs` — per [dsl.md § 2.4](../dsl.md).
- `.../tests/parser_unit_bridge.rs` — per [dsl.md § 2.5](../dsl.md), § 9.
- `.../tests/parser_error_per_code.rs` — one named test per `PARSE_*` code listed in §4.4.
- `.../tests/limits_per_dimension.rs` — one named test per dimension in [dsl.md § 13](../dsl.md).
- `.../tests/normalize_idempotency_corpus.rs` — for every UC-* row in [usecase.md § 2](../usecase.md), assert `normalize(normalize(parse(s))) == normalize(parse(s))`.
- `.../tests/hash_determinism.rs` — for every UC-* row, assert `hash(normalize(parse(s)))` is byte-stable across two invocations and across `x86_64` + `aarch64` CI matrix.
- `.../tests/property_normalize_idempotent.rs` — proptest 1k random valid raw strings.
- `.../tests/property_hash_stable.rs` — proptest 1k random valid raw strings.
- `.../tests/property_print_roundtrip.rs` — proptest 1k random AST → print → parse → normalize → equal.
- `.../benches/pre_norm_parse_bench.rs` — criterion, p99 < 1ms per 1 KiB query.
- `.../benches/pre_norm_hash_bench.rs` — criterion, p99 < 200 µs per 1 KiB normalized AST.

## 5. Implementation steps (strict TDD order)

1. **Write failing test**: `parser_unit_per_token_kind::test_keyword` — `parse("fooBar")` returns `Ok(LqQuery { expr: LqExpr::All(vec![LqExpr::Keyword("fooBar".into())]), .. })` with `lq_version = "1.0-pre"`. **Pass**: scaffold `tokenizer` + minimal `parser` recognizing one `KEYWORD` token. **Refactor**: extract `Tokenizer::peek_kind` and `Parser::expect_keyword` helpers.
2. **Write failing test**: `parser_unit_per_token_kind::test_phrase` — `parse(r#""async fn handle""#)` yields `LqExpr::Phrase("async fn handle")`. **Pass**: tokenizer recognizes `PHRASE`; parser routes to `LqExpr::Phrase`. Honor escape rules from [dsl.md § 3.2](../dsl.md): `\\`, `\"`, `\n`, `\r`, `\t`; any other `\X` → `PARSE_LEX_ERROR`.
3. **Write failing test**: `parser_unit_per_token_kind::test_raw_string` — `parse(r"'C:\Users\%'")` yields `LqExpr::RawString` with literal backslashes per [dsl.md § 3.3](../dsl.md). **Pass**: tokenizer recognizes `RAWSTRING`.
4. **Write failing test**: `parser_unit_per_token_kind::test_regex` — `parse("/fn\\s+\\w+/")` yields `LqExpr::Regex`. **Pass**: tokenizer recognizes `REGEX`; `regex_guard::compile` accepts the RE2-valid pattern; constructs `LqExpr::Regex`.
5. **Write failing test**: `parser_error_per_code::test_parse_invalid_utf8` — non-UTF-8 byte sequence → `LexicalErrorCode::PARSE_INVALID_UTF8` with `InvalidUtf8 { byte_offset }`. **Pass**: tokenizer's `Tokenizer::new(&[u8])` validates UTF-8 before exposing `&str`.
6. **Write failing test**: `parser_error_per_code::test_parse_oversized` — 16385-byte input → `PARSE_OVERSIZED { observed_bytes: 16385, budget_bytes: 16384 }`. **Pass**: size check after UTF-8 validation, before BOM strip.
7. **Write failing test**: `parser_error_per_code::test_parse_lex_error_unterminated_phrase` — `parse(r#""unterminated"#)` → `PARSE_LEX_ERROR { LexExpected { expected: "close-quote" } }`. **Pass**: tokenizer detects EOF-before-close.
8. **Write failing test**: `parser_unit_per_grammar_production::test_and_adjacency` — `parse("tokio runtime")` after normalize yields `LqExpr::All(vec![Keyword("tokio"), Keyword("runtime")])` per [dsl.md § 5.3](../dsl.md) (standard mode default). **Pass**: parser `AndExpression { Implicit-AND }` rule; normalize step 8 n-ary collapse.
9. **Write failing test**: `parser_unit_per_grammar_production::test_or` — `parse("panic OR unwrap")` yields `LqExpr::Any(vec![..])`. **Pass**: `OrExpression` production.
10. **Write failing test**: `parser_unit_per_grammar_production::test_not_precedence` — `parse("foo OR NOT bar")` parses as `Or(foo, Not(bar))` per RFC `NOT > AND > OR`. **Pass**: precedence climbing in `NotExpression`.
11. **Write failing test**: `parser_unit_per_grammar_production::test_dash_negation` — `parse("Iterator -dyn")` after normalize equals `parse("Iterator NOT dyn")`. **Pass**: tokenizer's `DASH` only at Atom-start position; parser rewrites into `LqExpr::Not`.
12. **Write failing test**: `parser_unit_per_grammar_production::test_paren_group` — `parse("(panic OR unwrap) lang:rust")` preserves the group binding. **Pass**: `Atom -> "(" Expression ")"` production.
13. **Write failing test**: `parser_unit_per_grammar_production::test_filter_repo_at_rev_sugar` — `parse("repo:foo@main panic!")` normalizes to `repo:foo rev:main` per [dsl.md § 5.2 / § 6.6](../dsl.md). **Pass**: `parser::parse_filter` recognizes `repo:` value `<pat>@<rev>`; normalize step 6 desugars to two filters.
14. **Write failing test**: `parser_unit_per_grammar_production::test_path_to_file_alias` — `parse("path:src/lib")` normalized AST carries `LqFilter::File { scope: FileScope::PathOnly, pattern: ... }` per [dsl.md § 6.2](../dsl.md) one-path-scope rule. **Pass**: normalize step 7 alias resolve.
15. **Write failing test**: `parser_error_per_code::test_parse_unknown_filter` — `parse("not_a_filter:value foo")` → `PARSE_UNKNOWN_FILTER { filter_name: "not_a_filter" }`. **Pass**: closed filter registry; parser rejects unknown names. Maps to AC-10.
16. **Write failing test**: `parser_error_per_code::test_parse_invalid_filter_value_type_dup` — `parse("type:file type:diff foo")` → `PARSE_INVALID_FILTER_VALUE { filter_name: "type", ... }` per [dsl.md § 6.4](../dsl.md). **Pass**: parser tracks per-kind exclusivity.
17. **Write failing test**: `parser_error_per_code::test_parse_invalid_patterntype` — `parse("patterntype:fuzzy foo")` → `PARSE_INVALID_PATTERNTYPE`. **Pass**: closed mode set `{literal,keyword,standard,regexp,structural}`.
18. **Write failing test**: `parser_unit_per_grammar_production::test_count_all_vs_bounded` — `parse("foo count:100")` yields `LqOptionSet { count: CountBound::Bounded(100), .. }`; `parse("foo count:all")` yields `CountBound::All`. **Pass**: `parse_filter_value` for `count:` accepts integer or `all`.
19. **Write failing test**: `parser_error_per_code::test_parse_forbidden_at_shorthand` — `parse("@lang=rust foo")` → `PARSE_FORBIDDEN_SYNTAX { construct: "generic @ shorthand" }`. **Pass**: parser rejects `AT` token outside `repo:<pat>@<rev>` sugar (AC-02, AC-03).
20. **Write failing test**: `parser_error_per_code::test_parse_forbidden_regex_backref` — `parse("/(foo)\\1/")` → `PARSE_FORBIDDEN_SYNTAX { construct: "backreference" }`. **Pass**: `regex_guard::compile` scans for `\\<digit>` outside character classes; rejects. (AC-05.)
21. **Write failing test**: `parser_error_per_code::test_parse_forbidden_regex_lookbehind` — `parse("/(?<=foo)bar/")` → `PARSE_FORBIDDEN_SYNTAX { construct: "lookbehind" }`. **Pass**: `regex_guard` detects `(?<=` / `(?<!` / `(?=` / `(?!` / `(?>` / `\\k<` / named-capture forms; rejects. (AC-06.)
22. **Write failing test**: `parser_unit_predicates::test_repo_has_file` — `parse("repo:has.file(path:Cargo\\.toml) tokio")` parses the predicate. **Pass**: `predicates::parse_call` per [dsl.md § 7](../dsl.md).
23. **Write failing test**: `parser_unit_predicates::test_invalid_predicate_arg_type` — `parse("repo:has.commit.after(yesterday-but-not-a-date)")` → for an invalid duration/date arg → `PARSE_INVALID_FILTER_VALUE { filter_name: "repo", arg: "0" }`. **Pass**: coercion table per [dsl.md § 7.2](../dsl.md).
24. **Write failing test**: `parser_unit_structural::test_match_block` — `parse("match { fn $X(...) { ... } }")` yields `LqExpr::StructuralBlock(..)` with metavariable `$X` and anon ellipsis. **Pass**: `structural::parse_body`.
25. **Write failing test**: `parser_unit_structural::test_sg_alias_normalize` — `parse("match { :[X] ( :[...ARGS] ) }")` normalizes `:[X]` → `$X` and `:[...ARGS]` → `$...ARGS` at lex stage per [dsl.md § 8.6](../dsl.md). **Pass**: lexer-level substitution; AST never carries alias form.
26. **Write failing test**: `parser_unit_structural::test_alias_outside_match_rejected` — `parse(":[X] foo")` (alias outside `match{}`) → `PARSE_FORBIDDEN_SYNTAX`. **Pass**: lexer guards the substitution to inside `match{}` body only.
27. **Write failing test**: `parser_unit_runtime::test_changed_filter` — `parse("changed:since=1d foo")` yields `LqFilter::Changed { ... }`. **Pass**: runtime filter parser.
28. **Write failing test**: `parser_unit_bridge::test_into_codeql` — `parse("Iterator into:codeql")` yields `LqDirective::IntoCodeQl { .. }`. **Pass**: directive parser.
29. **Write failing test**: `parser_error_per_code::test_parse_unsupported_combo_into_with_empty_expr` — `parse("into:codeql")` (no expr) → `PARSE_UNSUPPORTED_COMBO`. **Pass**: normalize step 8 detects directive without candidate-producing expression per [dsl.md § 9.3](../dsl.md).
30. **Write failing test**: `parser_error_per_code::test_parse_unsupported_combo_into_diff` — `parse("type:diff diff.added:unwrap into:codeql")` → `PARSE_UNSUPPORTED_COMBO`. **Pass**: normalize step 8 detects per [dsl.md § 9.3](../dsl.md). Maps to AC-13.
31. **Write failing test**: `limits_per_dimension::test_max_query_length` — 16385-byte input → `PARSE_OVERSIZED`. (Already covered by step 6; restated as a `limits_per_dimension::*` row for traceability.) **Pass**: already done.
32. **Write failing test**: `limits_per_dimension::test_max_ast_depth` — build a 33-deep nested-paren expression → `PLAN_LIMIT_EXCEEDED { dimension: "ast_depth", budget: 32, observed: 33 }`. **Pass**: depth tracker in `parser::parse_expression`.
33. **Write failing test**: `limits_per_dimension::test_max_boolean_fan_out` — 65 OR operands under one node → `PLAN_LIMIT_EXCEEDED { dimension: "boolean_fan_out", budget: 64 }`. **Pass**: post-parse n-ary collapse check.
34. **Write failing test**: `limits_per_dimension::test_max_filter_count_per_kind` — 33 `repo:` filters → `PLAN_LIMIT_EXCEEDED { dimension: "filter_count_repo", budget: 32 }`. **Pass**: normalize step 8 dedup pass enforces.
35. **Write failing test**: `limits_per_dimension::test_max_regex_nfa` — pathological `/a{1,1000000}b{1,1000000}/` → `PLAN_LIMIT_EXCEEDED { dimension: "regex_nfa", budget: 100000 }` (also reachable from `EXEC_REGEX_COMPILE_EXPLOSION` but at parse-time pre-check we emit `PLAN_LIMIT_EXCEEDED`). **Pass**: `regex_guard::estimate_nfa_states` via `regex_syntax::hir::analysis::Properties` upper bound.
36. **Write failing test**: `limits_per_dimension::test_max_structural_pattern_node_count` — 257-node `match{}` body → `PLAN_LIMIT_EXCEEDED { dimension: "structural_nodes", budget: 256 }`. **Pass**: structural parser counts; covers AC-07.
37. **Write failing test**: `normalize_idempotency_corpus::test_uc_*` — for each UC-* row from [usecase.md § 2](../usecase.md), assert `normalize(normalize(parse(s))) == normalize(parse(s))`. **Pass**: implement `normalize` step 8 dedup + n-ary collapse + canonical order (lex-sorted OR-within-kind).
38. **Write failing test**: `property_normalize_idempotent::proptest_random` — proptest 1k random valid raw strings. **Pass**: any normalize bug surfaces here; debug and fix until green.
39. **Write failing test**: `hash_determinism::test_uc_*` — for each UC-* row, compute hash twice; assert identical. **Pass**: implement `cbor_canonical::encode` per RFC 8949 §4.2.1; `hash::compute` wraps with SHA-256.
40. **Write failing test**: `property_hash_stable::proptest_random` — 1k random ASTs round-trip `compute_hash` twice. **Pass**: any encoder non-determinism surfaces here.
41. **Write failing test**: CI matrix step `hash_determinism::test_cross_arch` — same UC-* row computed on `x86_64` and `aarch64` produces identical digest. **Pass**: deterministic encoder + arch-stable `f32` (already enforced by `BoostFactor` rejecting NaN / -0 in PRE-CONTRACT-EXT).
42. **Write failing test**: `property_print_roundtrip::proptest_random` — `normalize(parse(print(normalize(parse(s))?))?)? == normalize(parse(s))?`. **Pass**: printer per [dsl.md § 10.1](../dsl.md); fix any AST-shape information that's lost on print.
43. **Write failing test**: `parser_error_per_code::test_parse_lex_error_at_position` — assert every emitted error carries `position: Some(TokenSpan { byte_offset, byte_len })` pointing at the failing token. **Pass**: tokenizer threads spans through; parser preserves on failure.
44. **Bench**: criterion `pre_norm_parse_bench` — assert p99 < 1ms per 1 KiB query of typical shape (UC-LEX-01 shape ×1000). **Pass**: if regression, profile; allowed budget under [implementation-plan.md § 8.2](../implementation-plan.md) is 5% growth per wave.
45. **Bench**: criterion `pre_norm_hash_bench` — assert p99 < 200 µs per 1 KiB normalized AST. **Pass**: same as above.
46. **Run** `cargo clippy --workspace --all-targets -- -D warnings`. Fix until green.
47. **Run** `semgrep --config tools/ci/semgrep/rules.yml --error` — confirm no `#[derive(Serialize)]` introduced.
48. **Run** `cargo +nightly miri test -p <pre-norm-crate-name>` — Miri must report no UB on parse + normalize + hash paths.

## 6. Test plan

### 6.1 Unit tests

Per the named files in §4.7. Coverage rule: one named test per `TokenKind`, per grammar production, per `PARSE_*` code, per limit dimension. Each test asserts:

- the returned `Result` discriminant matches expectation;
- on `Err`, `code`, `position.is_some()`, and `payload` match the row;
- on `Ok`, the canonical AST shape matches by structural equality on `LqQuery`.

### 6.2 Integration tests

- `tests/parse_normalize_hash_pipeline.rs` — `parse_normalize_hash` end-to-end against a handcrafted 10-row representative subset of the corpus; asserts the trio `(canonical AST, canonical bytes, digest hex)` matches a golden fixture file committed under `tests/fixtures/`.

### 6.3 Property tests (proptest invariants)

- `property_normalize_idempotent::proptest_random` — invariant: `normalize(normalize(parse(s))?)? == normalize(parse(s))?`. 1k cases.
- `property_hash_stable::proptest_random` — invariant: `compute_hash(normalize(parse(s))?) == compute_hash(normalize(parse(s))?)` (recomputed). 1k cases.
- `property_print_roundtrip::proptest_random` — invariant: `normalize(parse(print(normalize(parse(s))?))?)? == normalize(parse(s))?`. 1k cases.
- `property_parse_neg_per_code::proptest_random` — invariant: every `PARSE_*` code is reachable by *some* generated input; assert at least one negative case generates each code (coverage proof).

### 6.4 Conformance corpus rows greened (cite UC-* / AC-* IDs)

This ticket's level of "green" is **parse + normalize + hash green**, not execution green. The full execution-level green is each wave's job.

- All 85 UC-* rows from [usecase.md § 2](../usecase.md) reach `parse_normalize_hash → Ok((q, h))` with stable `h`. PRE-CONF reports `parse_ok` for each.
- 15 AC-* rows ([usecase.md § 4](../usecase.md)) — explicit code mappings:
  - `AC-01` (fuzzy `~similar_to_this`) → `PARSE_FORBIDDEN_SYNTAX` (`~` not in tokenizer's valid prefix set; emits with `construct: "fuzzy operator"`)
  - `AC-02` (generic `@lang`) → `PARSE_FORBIDDEN_SYNTAX`
  - `AC-03` (generic `@file`) → `PARSE_FORBIDDEN_SYNTAX`
  - `AC-04` (SQL injection in filter value) → `PARSE_INVALID_FILTER_VALUE` (value fails value-grammar) **or** `PARSE_UNKNOWN_FILTER` depending on shape — both are typed errors per [usecase.md AC-04](../usecase.md) `INVALID_FILTER`
  - `AC-05` (regex backref) → `PARSE_FORBIDDEN_SYNTAX`
  - `AC-06` (regex lookbehind) → `PARSE_FORBIDDEN_SYNTAX`
  - `AC-07` (structural recursion > cap) → `PLAN_LIMIT_EXCEEDED { dimension: "structural_nodes" }`
  - `AC-08` (request-time git scan) — **not reachable from parser**; predicate parser accepts; the *executor* surfaces this; PRE-NORM does not green AC-08 (PRE-CONF marks as `blocked` until LEX-07 executor lands)
  - `AC-09` (`type:commit` + `match{}`) → `PARSE_UNSUPPORTED_COMBO` (normalize step 8 detects)
  - `AC-10` (unknown filter) → `PARSE_UNKNOWN_FILTER`
  - `AC-11` (helper-string lowering) — **not reachable from parser**; the only entry point is `parse(raw: &str)`; PRE-CONF asserts the *public* surface has no `unsafe_lower(...)` function; PRE-NORM ensures `parse` is the only public entry per LEX-01 DoD §5 → cross-check
  - `AC-12` (source-bound anchored fallback) — not a parser concern; PRE-CONF marks `blocked`
  - `AC-13` (`into:codeql` + `type:diff`) → `PARSE_UNSUPPORTED_COMBO`
  - `AC-14` (tenant-leak attempt) — not a parser concern; LEX-02 / LEX-05 territory
  - `AC-15` (stale generation) — not a parser concern; LEX-04 / LEX-05 territory

### 6.5 Bench guards

- `pre_norm_parse_bench` (criterion) — p99 < 1ms per 1 KiB query, asserted by criterion's `bench_function` baseline-compare against the committed baseline in `target/criterion/baseline/`.
- `pre_norm_hash_bench` (criterion) — p99 < 200 µs per 1 KiB normalized AST.
- Regression budget per [implementation-plan.md § 8.2](../implementation-plan.md): p99 may not increase >5% per wave without an ADR.

### 6.6 Loom / sanitizer / Miri coverage

- Miri: parse / normalize / hash paths must be UB-free under `cargo +nightly miri test`. Aligns with `just rust-miri` ([CLAUDE.md](../../../../CLAUDE.md)).
- No loom (no concurrency in parser).
- No TSAN.
- ASAN: covered by the same Miri rail for memory safety.

## 7. Observability hooks

Per [implementation-plan.md § 9.1](../implementation-plan.md) Wave 1, PRE-NORM lands these spans / metrics ahead of OBS-01:

- OpenTelemetry spans: `lq.parse`, `lq.normalize` (Wave-1 row).
- Metrics: parse error count, labeled by `{ticket_id="PRE-NORM", wave_id=0, code=<LexicalErrorCode>}` — cardinality closed (29 codes × 2 ticket-ids reaching this stage = 58 labels max).
- Structured logs: every parse failure logs `{canonical_query_hash=N/A, raw_byte_len, code, position}` exactly once. Successful parses log nothing at this layer (OBS-01 owns one-log-per-request at the request boundary).
- Audit log: none at this layer.

Per RFC § Observability Requirements, span attributes include `lq_version` once the AST is constructed.

## 8. Error scenarios (negative tests)

| Adversarial input | Expected typed error code | Payload |
|---|---|---|
| `(0xFF, 0xFE, 0x00)` raw bytes (non-UTF-8 BOM tail) | `PARSE_INVALID_UTF8` | `InvalidUtf8 { byte_offset: 2 }` |
| 16385 ASCII bytes of `a` | `PARSE_OVERSIZED` | `Oversized { observed_bytes: 16385, budget_bytes: 16384 }` |
| `"unterminated phrase` (no close quote) | `PARSE_LEX_ERROR` | `LexExpected { expected: "close-quote" }` (UC-EDGE-02) |
| `foo AND` (trailing operator) | `PARSE_SYNTAX_ERROR` | `SyntaxExpected { expected: "Atom", found: "EOF" }` |
| `(()` (unbalanced paren) | `PARSE_SYNTAX_ERROR` | `SyntaxExpected { expected: ")", found: ... }` |
| `/foo(/` (regex compile fail) | `PARSE_INVALID_REGEX` | `InvalidRegex { dialect_violation: "unbalanced paren" }` (UC-EDGE-03) |
| `/(foo)\\1/` | `PARSE_FORBIDDEN_SYNTAX` | `ForbiddenSyntax { construct: "backreference" }` (AC-05) |
| `/(?<=foo)bar/` | `PARSE_FORBIDDEN_SYNTAX` | `ForbiddenSyntax { construct: "lookbehind" }` (AC-06) |
| `@lang=rust foo` | `PARSE_FORBIDDEN_SYNTAX` | `ForbiddenSyntax { construct: "generic @ shorthand" }` (AC-02) |
| `not_a_filter:value foo` | `PARSE_UNKNOWN_FILTER` | `UnknownFilter { filter_name: "not_a_filter" }` (AC-10) |
| `type:file type:diff foo` | `PARSE_INVALID_FILTER_VALUE` | `InvalidFilterValue { filter_name: "type", value: "diff", expected_grammar: "single type filter" }` |
| `patterntype:fuzzy foo` | `PARSE_INVALID_PATTERNTYPE` | `InvalidPatternType { value: "fuzzy", allowed: ["literal","keyword","standard","regexp","structural"] }` |
| `type:commit match { fn $X { ... } }` | `PARSE_UNSUPPORTED_COMBO` | `UnsupportedCombo { constructs: ["type:commit", "match{}"] }` (AC-09 / UC-EDGE-04) |
| `into:codeql` (no expr) | `PARSE_UNSUPPORTED_COMBO` | `UnsupportedCombo { constructs: ["into:codeql", "empty-expression"] }` |
| `type:diff diff.added:unwrap into:codeql` | `PARSE_UNSUPPORTED_COMBO` | `UnsupportedCombo { constructs: ["into:codeql", "type:diff"] }` (AC-13) |
| 33-deep paren nesting | `PLAN_LIMIT_EXCEEDED { dimension: "ast_depth" }` | budget=32 observed=33 |
| 65 OR-operand fan-out | `PLAN_LIMIT_EXCEEDED { dimension: "boolean_fan_out" }` | budget=64 observed=65 |
| pathological regex /a{1,1000000}b{1,1000000}/ | `PLAN_LIMIT_EXCEEDED { dimension: "regex_nfa" }` | budget=100000 observed=>100000 |
| 257-node `match{}` body | `PLAN_LIMIT_EXCEEDED { dimension: "structural_nodes" }` | budget=256 observed=257 (AC-07) |
| `~similar_to_this` | `PARSE_FORBIDDEN_SYNTAX` | `ForbiddenSyntax { construct: "fuzzy operator" }` (AC-01) |
| `Repo:foo bar` (uppercase filter name; [dsl.md § 6.1](../dsl.md) is case-insensitive on lookup — so this is **accepted** post-normalize as `repo:foo` per the dsl rule; however [usecase.md UC-EDGE-10](../usecase.md) says "filter names are case-sensitive lowercase" → `INVALID_FILTER`) | **CONFLICT — see §12** | resolved by §12 default position |

## 9. Performance envelope

- `parse` p99 < 1ms per 1 KiB raw query (`pre_norm_parse_bench`). Per-row corpus mean target ≤ 200 µs.
- `normalize` p99 < 200 µs per parsed AST of typical shape.
- `compute_hash` p99 < 200 µs per normalized AST of typical shape (`pre_norm_hash_bench`).
- Memory: parse + normalize must allocate `O(query_length)`; AST size bounded by limit caps (depth 32, fan-out 64). Worst-case AST size ≤ 8 KiB for a 16 KiB raw input.
- SLO contribution to Wave-0 exit gate: per [implementation-plan.md § 4.1](../implementation-plan.md), Wave-0 requires "PRE-NORM emits a stable hash for every of the 85 UC-* rows; idempotency invariant ... holds in property tests (1k cases)" — DoD §11 below.
- Wave-1 exit gate ([implementation-plan.md § 9.1 Wave 1](../implementation-plan.md)) requires "parser p99" SLO measured — that measurement happens in Wave 1 against this ticket's output.

## 10. Risks & mitigations

| ID | Risk | Mitigation | Source |
|---|---|---|---|
| R4 (cite) | RE2 NFA upper-bound estimator (`regex_syntax::hir::analysis`) overshoots and rejects legitimate queries | corpus assertion for 100 sample regexes; estimator-vs-actual delta logged at WARN; configurable cap per [dsl.md § 3.4](../dsl.md) with a floor of 1_000 | [implementation-plan.md § 4.2 Wave 1 risks](../implementation-plan.md) |
| R13 (cite) | Canonical CBOR encoder bytewise-determinism risk on `f32` boost values | canonical encoding rejects NaN / -0 / 0.0 at `BoostFactor` boundary (PRE-CONTRACT-EXT); cross-arch CI matrix asserts byte identity | [implementation-plan.md § 4.2](../implementation-plan.md), [dsl.md § 11.1](../dsl.md) |
| (wave) | Sourcegraph reference release drift mid-ticket | pin reference tag in PRE-CONTRACT-EXT handoff doc; no auto-accept (per [rfc.md § Conformance corpus ownership](../rfc.md)) | [implementation-plan.md § 4.2](../implementation-plan.md) |
| (new) | Recursive-descent parser stack-overflows on adversarial deep input before the 32-depth cap fires | depth-tracking counter incremented before every recursive call; check **before** descent, not after | [dsl.md § 13](../dsl.md), this ticket §5 step 32 |
| (new) | Printer loses information that survives `parse` (e.g., implicit `patterntype:standard`) and breaks idempotency invariant | printer always emits the canonical form post-normalize, never the source form; property test `property_print_roundtrip` covers | [dsl.md § 10.1](../dsl.md) |
| R14 (cite) | ADR-001 (parser strategy: hand-rolled vs `nom` vs `chumsky`) is unresolved at ticket entry | ADR-001 resolution is forced at PRE-NORM start ([implementation-plan.md § 10](../implementation-plan.md)); ticket cannot start until resolved; default position: hand-rolled recursive descent for byte-deterministic span tracking and zero dependency surface | [implementation-plan.md § 10 ADR-001](../implementation-plan.md) |

## 11. Definition of Done (provable)

All 12 rows shipped (75 tests in `quanta-index-lq-norm`). Crate-name placeholder `<pre-norm-crate>` resolved to `quanta-index-lq-norm`.

1. ✓ shipped — **Parser + normalizer + hasher compile in the workspace.**
   - command: `cargo check --workspace`
   - expected: exit 0
   - proof: build hygiene.
2. ✓ shipped — **Every `PARSE_*` code (10) and `PLAN_LIMIT_EXCEEDED` (1) has a named negative test.**
   - command: `cargo test -p quanta-index-lq-norm --test parser_error_per_code --test limits_per_dimension -- --list`
   - expected: ≥ 11 named tests; all pass when run
   - proof: closes [rfc.md § Error Code Taxonomy](../rfc.md) for parse layer.
3. ✓ shipped — **Idempotency invariant holds for the 85 UC-* rows + 1k random cases.**
   - command: `cargo test -p quanta-index-lq-norm --test normalize_idempotency_corpus --test property_normalize_idempotent`
   - expected: pass
   - proof: closes [dsl.md § 10.1](../dsl.md) hard invariant.
4. ✓ shipped — **Canonical hash is byte-stable across two runs.**
   - command: `cargo test -p quanta-index-lq-norm --test hash_determinism --test property_hash_stable`
   - expected: pass
   - proof: closes [dsl.md § 11.4](../dsl.md) stability guarantee.
5. ✓ shipped — **Canonical hash is byte-stable across `x86_64` + `aarch64`.**
   - command: CI matrix runs `cargo test -p quanta-index-lq-norm --test hash_determinism::test_cross_arch` on both targets
   - expected: identical digests
   - proof: closes Wave-0 exit gate ([implementation-plan.md § 4.1](../implementation-plan.md)).
6. ✓ shipped — **Printer round-trips for all 85 UC-* rows + 1k random cases.**
   - command: `cargo test -p quanta-index-lq-norm --test property_print_roundtrip`
   - expected: pass
   - proof: closes [dsl.md § 10.1](../dsl.md) second invariant.
7. ✓ shipped — **15 AC-* rows return the expected `LexicalErrorCode`.**
   - command: `cargo test -p quanta-index-lq-norm --test parser_error_per_code -- AC_`
   - expected: 13 of 15 rows green (AC-08, AC-11, AC-12, AC-14, AC-15 are not parser-reachable; explicitly marked `blocked` by PRE-CONF)
   - proof: closes [usecase.md § 4](../usecase.md) for parse layer.
8. ✓ shipped — **Bench p99 within target.**
   - command: `cargo bench -p quanta-index-lq-norm --bench pre_norm_parse_bench --bench pre_norm_hash_bench`
   - expected: parse p99 < 1ms / 1 KiB; hash p99 < 200 µs / 1 KiB
   - proof: closes [implementation-plan.md § 5.2 DoD bullet 6](../implementation-plan.md). Criterion benches shipped per the deferred-items closeout.
9. ✓ shipped — **Miri reports no UB.**
   - command: `cargo +nightly miri test -p quanta-index-lq-norm`
   - expected: exit 0
   - proof: heavy correctness rail per [CLAUDE.md](../../../../CLAUDE.md).
10. ✓ shipped — **Semgrep / clippy / fmt green.**
    - command: `semgrep --config tools/ci/semgrep/rules.yml --error && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
    - expected: exit 0 each
    - proof: rails clean.
11. ✓ shipped — **ADR-001 committed.**
    - command: `test -f docs/adr/ADR-001-parser-strategy.md`
    - expected: file exists
    - proof: closes [implementation-plan.md § 10](../implementation-plan.md) ADR-001 slot.
12. ✓ shipped — **One log line per parse failure, structured.**
    - command: integration test asserts a `tracing` capture contains the expected fields for a known-bad input
    - expected: `{code, raw_byte_len, position}` present exactly once
    - proof: closes Wave-1 observability subset ([implementation-plan.md § 9.1](../implementation-plan.md)).

## 12. Open questions

| Q-ID | Question | Forcing function |
|---|---|---|
| ADR-001 ([implementation-plan.md § 10](../implementation-plan.md)) | Parser strategy: hand-rolled recursive descent vs `nom` vs `chumsky` | Step 1 of §5 cannot start without the strategy. **Default position recorded**: hand-rolled recursive descent. Reason: (a) byte-deterministic span tracking, (b) zero new dependencies (lint cost per `cargo machete` / `cargo deny`), (c) no proc-macro expansion cost (aligns with D18 spirit even though serde is the named target). |
| ADR-007 ([implementation-plan.md § 10](../implementation-plan.md)) | Planner directive ordering: filter-before-directive vs directive-before-filter | Affects normalize step 8 stability — when does `into:codeql` get attached relative to filter merge? **Default position**: filters first, directives last (matches [dsl.md § 9.3](../dsl.md) "directives apply after filtering"). |
| Q-FS-6 ([feature-scope.md § 1.1.3](../feature-scope.md)) | `select:` projection enum: full set vs subset for Phase 1 | Parser must accept the closed value set ([dsl.md § 6.5](../dsl.md) lists `repo|file|path|symbol|content|content.match`). **Default**: ship all 6; planner returns `PLAN_DEFERRED` for any not yet wired by LEX-01 / LEX-06. |
| Q-FS-10 ([feature-scope.md § 1.1.4](../feature-scope.md)) | `index:no` mode: parse-rejected vs accepted-then-`NotImplemented` | Parser must decide one of the two paths today. **Default**: accepted at parse; planner emits `PLAN_DEFERRED` (namespace reserved). |
| (new) | `UC-EDGE-10` (uppercase filter name `Repo:foo`) vs [dsl.md § 6.1](../dsl.md) (case-insensitive lookup, lower-cased canonical) — these conflict | **CONFLICT in sibling docs.** Default position: follow [dsl.md § 6.1](../dsl.md) (case-insensitive, normalize to lower-case). File a follow-up against [usecase.md UC-EDGE-10](../usecase.md) to either match (parity becomes `SG~`) or change to a different adversarial input. Records as **UC-GAP-4** for [implementation-plan.md Appendix A.3](../implementation-plan.md). |
| (new) | `keyword`-mode adjacency-link annotation: do we ship `LqExpr::AdjacencyLink(...)` in PRE-NORM, or defer to LEX-06? | [dsl.md § 4.2](../dsl.md), § 5.3 specify the shape but only the ranker consumes it. **Default**: ship the AST variant now (carrier needs it on the wire); planner / ranker wires consumption in LEX-06. |
| DSL-GAP-1 ([implementation-plan.md Appendix A.4](../implementation-plan.md)) | `count:` cap: dsl says 10_000 floor, feature-scope says 100_000 for `count:all` | Parser caps `CountBound::Bounded(N)` at 10_000 ([dsl.md § 6.2](../dsl.md), § 13). `CountBound::All` is unbounded at parse — executor enforces ceiling separately. **Default**: parser uses dsl-aligned 10_000 for `Bounded`, no cap on `All`. |
| DSL-GAP-2 ([implementation-plan.md Appendix A.4](../implementation-plan.md)) | `lang:` enum has 60+ entries; do we ship them all? | Parser closed-set lookup is required. **Default**: ship the full 60+ set as a Phf static map. Cost is one map lookup per `lang:` filter; benign. |
| DSL-GAP-3 ([implementation-plan.md Appendix A.4](../implementation-plan.md)) | `LqCanonicalHashV1` final name | PRE-CONTRACT-EXT pinned `LqCanonicalHashV1`; this ticket uses that name. **Default position**: keep `LqCanonicalHashV1`. |
| G-CONTROL-LOC ([implementation-plan.md § 2.3a](../implementation-plan.md)) | Where does this ticket physically land — new `quanta-index-lq-norm` crate vs module under `quanta-index-core`? | DoD §11 commands use placeholder `<pre-norm-crate>` token; final crate name must be pinned before the first `cargo test -p <name>` is run. **Default position**: new `quanta-index-lq-norm` crate (keeps parser isolated from policy validators in core; matches the recommended owner in [feature-scope.md § 1.1](../feature-scope.md) "parser-first and planner-owned"). |

## 13. References

- Parent RFC: [rfc.md](../rfc.md) — § Canonical Grammar Semantics, § Canonical Read Pipeline, § Error Code Taxonomy, § Non-Negotiable Invariants, § Capacity and SLO Targets
- Scope catalog: [feature-scope.md](../feature-scope.md) — § 1.1 Core-1.0 features, § 9 open questions (Q-FS-6, Q-FS-10)
- Conformance corpus: [usecase.md](../usecase.md) — § 2 UC-* rows, § 4 AC-* rows, § 0 error code table
- Grammar / DSL: [dsl.md](../dsl.md) — § 1 lexical structure, § 2 EBNF, § 3 pattern leaves, § 4 mode matrix, § 5 boolean rules, § 6 filters, § 7 predicates, § 8 structural, § 9 directives, § 10 normalization, § 11 canonical hash, § 12 error taxonomy, § 13 limits, § 16 non-negotiable DSL invariants
- Execution plan: [implementation-plan.md](../implementation-plan.md) — § 4.1 Wave 0, § 5.2 PRE-NORM DoD, § 6 risk register, § 8 test strategy, § 9.1 OBS subset per wave, § 10 ADR-001, ADR-007
- Agent rules: [CLAUDE.md](../../../../CLAUDE.md) — § Agent change posture, § Rule Catalog (Safety: no silent failure)
- Sibling tickets: [PRE-CONTRACT-EXT.md](PRE-CONTRACT-EXT.md), [PRE-CONF.md](PRE-CONF.md)
- Producer handoff SSOT: [docs/ssot/producer-handoff.md](../../../ssot/producer-handoff.md)
- Ticket index (downstream-migration follow-up tracked under §3.6): [INDEX.md](INDEX.md)
- Existing contract code: [crates/quanta-index-contract/src/query/expression.rs](../../../../crates/quanta-index-contract/src/query/expression.rs)
- Semgrep rule: [tools/ci/semgrep/rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124)
