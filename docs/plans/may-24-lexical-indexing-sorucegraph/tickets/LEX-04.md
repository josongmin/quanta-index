# LEX-04 — RE2 Regex Executor

> Status: `shipped`
> Crate: `quanta-index-lq-regex`
> Tests: 81
> Last verified: 2026-05-25
> Wave: 2/3 (LEX-04 is the regex-execution sub-ticket of the lexical content engine; planner lands in Wave 2 as the parse + plan layer for `/…/` leaves and `patterntype:regexp`, executor lands in Wave 3 alongside the parallel executor + deterministic merge).
> Parent RFC: [../rfc.md](../rfc.md) § LQ family, § Pattern semantics, § Engine Decomposition, § Non-Negotiable Invariants, § Error Code Taxonomy, § Claim Discipline.
> Sibling docs: [../feature-scope.md](../feature-scope.md), [../usecase.md](../usecase.md), [../dsl.md](../dsl.md), [../implementation-plan.md](../implementation-plan.md).
> Posture: **breaking-first** — no long-lived shims, no dual surface, no heuristic success path. Per [../../../../CLAUDE.md](../../../../CLAUDE.md) § Agent change posture.
>
> AST-level precise detection shipped for `Possessive (?>...)`, `NamedCaptureRef \k<name>`, and mid-pattern `InlineFlagMidPattern`. Leading-position `(?i)` ACCEPT policy locked at the parse layer.

---

## §1 Purpose

Ship the RE2-class regex execution lane of the lexical kernel as a typed, bounded, deterministic, fail-closed pipeline. Specifically:

1. Accept `/…/` regex leaves and `patterntype:regexp` mode as compiled RE2-dialect patterns (per [../dsl.md](../dsl.md) §3.4).
2. Reject `lookbehind`, `lookahead`, `backreference`, `possessive group`, `named-capture reference`, `inline flag mode-switch` constructs at **parse time** as typed `PARSE_FORBIDDEN_SYNTAX` (per [../rfc.md](../rfc.md) § Error Code Taxonomy `PARSE_*`, [../dsl.md](../dsl.md) §3.4 forbidden features).
3. Enforce the **100,000 NFA-state** budget at planner time using `regex_syntax::hir::analysis::Properties` upper bounds before compilation, surfacing `PLAN_LIMIT_EXCEEDED{dimension=regex-nfa, limit=100000, observed=<n>}` (per [../rfc.md](../rfc.md) § Canonical Query Model §7, [../dsl.md](../dsl.md) §3.4 cost cap, [../dsl.md](../dsl.md) §13 Limits).
4. Pre-filter candidate documents via a trigram authority before regex verification, capping the candidate set per `count:` and `timeout:` budgets (per [../rfc.md](../rfc.md) § Lexical content engine).
5. Surface `EXEC_REGEX_COMPILE_EXPLOSION`, `EXEC_SHARD_TIMEOUT`, `EXEC_MERGE_CANCEL` as typed runtime errors. Never silently degrade. Never partially surface results without an explicit `partial:allow` opt-in (out of scope for this ticket).
6. Conformance: every `UC-LEX-13/14` (regex leaf, regex with anchors) and adjacent `UC-LEX-05/06/08/21/AC-05/AC-06/AC-07/UC-EDGE-03/UC-EDGE-06` row in [../usecase.md](../usecase.md) is provable green or provable red-by-typed-error.

This ticket is the only owner of regex matching semantics in `LQ/Core-1.0` and `LQ/Core-1.0 + patterntype:regexp`. No other crate may compile a user-supplied regex outside this lane.

---

## §2 Background

### 2.1 What exists today

- The Tantivy chunk index ([../../../../crates/quanta-index-lexical/src/](../../../../crates/quanta-index-lexical/src/)) exposes `RegexQuery` over the `text` and `path` fields via Tantivy 0.22.
- No NFA-state pre-check exists. A pathological pattern (e.g. `(a?){100}a{100}`) would currently push compile cost onto Tantivy's automaton stage without a typed planner reject.
- No trigram authority exists yet (per [../implementation-plan.md](../implementation-plan.md) §2.4 row "LEX-02 trigram (for raw-string `'…'` substring search)" = **absent**). LEX-04 inherits the trigram authority that LEX-02 (sibling ticket in this packet) lands as a prerequisite for raw-string and as an accelerator for regex.
- Predecessor doc [../search-plane-implementation-tickets.md](../../search-plane-implementation-tickets.md) records the Tantivy-only state.
- Parser/normalizer are absent today; the canonical AST has only `LqExpr::{Raw, All, Any, Not}` ([../implementation-plan.md](../implementation-plan.md) §2.1) — `Regex` leaf variant is contracted to land in `PRE-CONTRACT-EXT` (Wave 0) and the parser in `PRE-NORM` / LEX-01 (RFC ticket).

### 2.2 Why RE2 not PCRE

RE2 / Rust `regex` crate has linear-time worst-case guarantee in input length, no catastrophic backtracking, no backreferences, no lookaround. Matches Sourcegraph dialect (per [../feature-scope.md](../feature-scope.md) §1.1.1 regex row, [../dsl.md](../dsl.md) §3.4). RFC § Non-Negotiable Invariants §10 ("no unbounded memory in regex/structural matching") is enforceable only on RE2-class engines with a static state-count estimator — backtracking engines cannot bound state count without runtime budget.

### 2.3 Vendor pinning

The `regex` crate (Rust) and its companion `regex_syntax` are the chosen implementation. They are already RE2-class. Vendor pinning policy:

- pin to `regex = "=1.10.x"` (latest patch within the 1.10 series) and `regex_syntax = "=0.8.x"` in workspace `Cargo.toml`.
- `regex` MSRV must be ≤ 1.92 (CLAUDE.md § default MSRV pin).
- `cargo deny` advisories `yanked=deny`, `unmaintained=all`, `unsound=all` applies (CLAUDE.md § supply-chain guard).
- a `regex` minor bump (1.10 → 1.11) requires an ADR-class decision because the NFA state-count semantics can shift — see ADR slot expected at §10 Risks (new ADR-017).

### 2.4 Why "Wave 2/3" rather than a single wave

The regex executor straddles two RFC-level concerns:
1. **Parse-time discipline** (Wave 2, sibling of LEX-01/LEX-02 in the implementation-plan): regex dialect filter, NFA-state pre-check, typed error surface. This subset is implementable as soon as PRE-NORM lands.
2. **Runtime execution** (Wave 3, sibling of LEX-05 in the implementation-plan): cooperative cancellation, trigram pre-filter, candidate-set verification. This subset depends on the parallel executor merge tuple and the per-shard cancellation cadence.

Splitting LEX-04 across these two waves is intentional: the parse-time leg unblocks every `/…/` query at AST level (so AC-05/AC-06 can be conformance-green by Wave 2 exit), while the runtime leg is gated on the executor.

---

## §3 Inputs

- A `LqQueryV1` containing at least one `LqExpr::Regex(RawRegex)` leaf or a `PatternTypeOption::Regexp` mode flag (variants pre-landed in `PRE-CONTRACT-EXT`, per [../implementation-plan.md](../implementation-plan.md) §5.1).
- A `PlanContext { tenant_id, user_id, generation_set }` carrying the per-query generation pin ([../implementation-plan.md](../implementation-plan.md) §5.5 LEX-01).
- A trigram index for `(repo, rev, generation)` produced by the lexical content engine (LEX-03 sibling generation per [../implementation-plan.md](../implementation-plan.md) §4.3 Wave 2).
- An optional `count:<N|all>` (default `1000`, ceiling `10_000` per [../dsl.md](../dsl.md) §6.2; `count:all` honors §7 ceiling of `100_000` and fails closed beyond it per [../usecase.md](../usecase.md) Q-FS-7).
- An optional `timeout:<duration>` (default `5s` soft, per [../dsl.md](../dsl.md) §13).
- An optional `case:<yes|no>` (default per active `patterntype:` mode matrix, [../dsl.md](../dsl.md) §4).
- An optional `(?i)` inline flag at the head of the pattern, normalized at desugar stage 6 to `case:no` ([../dsl.md](../dsl.md) §10).

Inputs out of scope:
- `lookbehind`, `lookahead`, `backreference`, `possessive` constructs (rejected upstream by parser per [../dsl.md](../dsl.md) §3.4).
- `match { … }` structural blocks (LEX-05 sibling ticket / STR-01 family).

---

## §4 Deliverables

### 4.1 Code surface

A new module `crates/quanta-index-lexical/src/regex_lane.rs` (or named `regex_executor.rs` — choice belongs to ADR-017) owning:

1. A `RegexCompiler` type wrapping `regex::Regex` + `regex_syntax::hir::analysis::Properties` with the NFA-state pre-check.
2. A `RegexCandidateFilter` type that consumes trigram-derived candidate sets from the trigram authority and a compiled `RegexCompiler` and emits verified `LexicalCandidate`s.
3. A `RegexCheckpoint` enum (`AtNCandidates(usize)`, `AtMsBoundary(u64)`) consumed by the executor's cooperative-cancellation hook (sibling LEX-05).
4. Hand-rolled `impl serde::Serialize` / `impl serde::Deserialize` for all wire types in this lane — D18 ban on derives applies workspace-wide ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) `rust-no-serde-derive`).
5. No `unwrap` / `unwrap_or` / `Result::ok` on production paths (clippy disallowed-methods rail per [../implementation-plan.md](../implementation-plan.md) §1.4).

### 4.2 Trait surface

Extends the existing `SearchPlaneLexicalIndexBuildPort` and `SearchPlaneLexicalIndexStorePort` (historical homes — subject to G-CONTROL-LOC, per [../implementation-plan.md](../implementation-plan.md) §2.3a):

- `LexicalIndexStorePort::open_trigram_reader(generation: &PublishedGenerationSet) -> Result<TrigramReader, CoreError>`.
- `RegexExecutor::execute(compiled: &CompiledRegex, ctx: &PlanContext, options: &LqOptionSet) -> Stream<LexicalCandidate>`.

No new contract surface beyond what `PRE-CONTRACT-EXT` already lands (the `LqExpr::Regex` variant). New error codes are already enumerated in [../rfc.md](../rfc.md) § Error Code Taxonomy and provisioned by `PRE-CONTRACT-EXT`.

### 4.3 Test surface

- `crates/quanta-index-lexical/tests/regex_lane_unit.rs` (or `tests/regex_executor.rs`) — unit per-error-code rows.
- `crates/quanta-index-lexical/tests/regex_lane_property.rs` — property tests (proptest) for compile→execute round-trip.
- `crates/quanta-index-lexical/benches/regex_bench.rs` — criterion harness (`lex_04_regex_compile_bench`, `lex_04_regex_verify_bench`).
- Conformance rows in `usecase-corpus/UC-LEX-{05,06,13,14,21}.toml` and `usecase-corpus/AC-{05,06,07}.toml` consumed by `PRE-CONF` (per [../implementation-plan.md](../implementation-plan.md) §5.3).

### 4.4 Docs

- An ADR (new slot: **ADR-017 — RE2 implementation choice and NFA-state estimator policy**) committed to `docs/adr/ADR-017-re2-regex-engine.md` (path pre-seeded per [../implementation-plan.md](../implementation-plan.md) §10 forcing-function table).
- Updates to the existing handoff doc `docs/handoffs/lq-contract-1.0.md` (path planted by [../implementation-plan.md](../implementation-plan.md) §7.3) noting the typed regex error codes.

---

## §5 Implementation steps (TDD)

Each step lands as a separate PR. Each PR starts with a failing test commit.

### 5.1 Step 1 — Pre-check estimator (red → green)

Test first: add `regex_lane_unit::nfa_state_estimator_rejects_explosion` asserting that `(a?){101}a{101}` rejects with `PLAN_LIMIT_EXCEEDED{dimension=regex-nfa}`.

Then implement: a free function `estimate_nfa_states(hir: &regex_syntax::hir::Hir) -> u64` that walks `regex_syntax::hir::analysis::Properties` and computes an upper bound. Compare against the deployment cap (default `100_000`, floor `1_000` per [../dsl.md](../dsl.md) §13).

### 5.2 Step 2 — Parse-time forbidden constructs

Test first: per AC-05 (`/(foo)\1/` → `PARSE_FORBIDDEN_SYNTAX`), AC-06 (`/(?<=foo)bar/` → `PARSE_FORBIDDEN_SYNTAX`), plus negative tests for `(?>…)` and `\k<name>`. Use `usecase-corpus/AC-05.toml` and `usecase-corpus/AC-06.toml`.

Implement: a `validate_re2_dialect(hir: &Hir) -> Result<(), LexicalErrorCode>` predicate that walks the HIR for forbidden node kinds. Surface offset + offending construct in the typed error payload (per [../rfc.md](../rfc.md) § `PARSE_FORBIDDEN_SYNTAX`).

### 5.3 Step 3 — Compile path

Test: round-trip property test — 1k random RE2-valid patterns (proptest strategy derived from the EBNF in [../dsl.md](../dsl.md) §3.4 enabled features) compile, execute, and produce stable `canonical_query_hash` (per [../rfc.md](../rfc.md) § Canonical Query Model §8 and [../dsl.md](../dsl.md) §11.4).

Implement: `RegexCompiler::compile(raw: &RawRegex) -> Result<CompiledRegex, LexicalErrorCode>` that:
1. Calls `regex_syntax::parse` (surfaces `PARSE_INVALID_REGEX` on syntax failure with the RE2 message per [../dsl.md](../dsl.md) §12).
2. Calls `validate_re2_dialect`.
3. Calls `estimate_nfa_states` and checks against deployment cap.
4. Calls `regex::Regex::new` (surfaces `EXEC_REGEX_COMPILE_EXPLOSION` if compile-time state count somehow exceeds budget despite estimator — defense in depth).

### 5.4 Step 4 — Trigram pre-filter

Test: integration — a 100-document fixture where 5 docs contain `fn handle_foo` and 95 do not. Assert that `RegexCandidateFilter::pre_filter(re=/fn\s+handle_\w+/)` returns exactly the 5 candidate docs (no false negatives; false positives allowed because the verify step catches them).

Implement: an HIR-to-required-trigram lowering. For a pattern, derive the set of "must-contain" 3-grams. Intersect candidate document sets from the trigram authority. Trigram authority is owned by LEX-02 (sibling); this ticket consumes its `TrigramReader::candidates_containing(&[Trigram]) -> CandidateIdSet`.

### 5.5 Step 5 — Verify step + cooperative cancel

Test: per UC-EDGE-06 (`timeout:1ms count:all /.*/` → `error:TIMEOUT_EXCEEDED`), plus a loom-style test asserting that a cancel signal between checkpoints causes `EXEC_MERGE_CANCEL` within ≤ 1ms of signal observation.

Implement: `RegexCandidateFilter::verify(candidates, compiled, checkpoint_hook)` that iterates the candidate set, runs `compiled.is_match(chunk_text)` per chunk, and yields verified `LexicalCandidate`. At each `RegexCheckpoint::AtNCandidates(N=64)` or `RegexCheckpoint::AtMsBoundary(M=1ms)` boundary, polls the executor's cancellation token. On cancel → return `EXEC_MERGE_CANCEL`. On `count:` reached → emit `early_stop_reason="count_reached"` and stop.

### 5.6 Step 6 — Wire to LEX-05 executor

Test: UC-LEX-13 (`/fn\s+handle_\w+/`) and UC-LEX-14 (`/^fn foo/`) green end-to-end via PRE-CONF runner against the multi-repo fixture.

Implement: route `LqExpr::Regex` and `PatternTypeOption::Regexp` mode through the LEX-05 fanout. Each shard worker constructs a `CompiledRegex` once per query (per-shard cache, key = `canonical_query_hash`). Bounded concurrency per [../rfc.md](../rfc.md) § Execution Model.

### 5.7 Step 7 — Criterion benches

Add `lex_04_regex_compile_bench` and `lex_04_regex_verify_bench` to `crates/quanta-index-lexical/benches/`. Regression budget: p99 may not increase >5% across a wave without an ADR (per [../implementation-plan.md](../implementation-plan.md) §8.2 criterion rule).

### 5.8 Step 8 — ADR-017

Write `docs/adr/ADR-017-re2-regex-engine.md` covering: vendor choice rationale, NFA-state estimator approach, cap configurability, comparison with Tantivy `RegexQuery` (kept only as a sub-component, never as the public regex surface), trigram-pre-filter cost/recall trade-off.

---

## §6 Test plan

### 6.1 Unit (`cargo test -p quanta-index-lexical --test regex_lane_unit`)

| Test name | Asserts |
|---|---|
| `regex_lane_unit::accepts_re2_dialect_anchored` | `/^fn foo/` compiles, NFA states < cap |
| `regex_lane_unit::rejects_backreference` | `/(foo)\1/` → `PARSE_FORBIDDEN_SYNTAX{construct=backreference}` |
| `regex_lane_unit::rejects_lookbehind` | `/(?<=foo)bar/` → `PARSE_FORBIDDEN_SYNTAX{construct=lookbehind}` |
| `regex_lane_unit::rejects_lookahead` | `/foo(?=bar)/` → `PARSE_FORBIDDEN_SYNTAX{construct=lookahead}` |
| `regex_lane_unit::rejects_possessive_group` | `/(?>foo)/` → `PARSE_FORBIDDEN_SYNTAX{construct=possessive}` |
| `regex_lane_unit::rejects_named_capture_ref` | `/(?P<n>foo)\k<n>/` → `PARSE_FORBIDDEN_SYNTAX{construct=named-capture-ref}` |
| `regex_lane_unit::rejects_inline_flag_midpattern` | `/foo(?i)bar/` → `PARSE_FORBIDDEN_SYNTAX{construct=inline-flag-midpattern}` |
| `regex_lane_unit::accepts_inline_case_at_head` | `/(?i)foo/` normalizes to pattern `foo` with `case:no` |
| `regex_lane_unit::nfa_state_estimator_rejects_explosion` | `(a?){101}a{101}` → `PLAN_LIMIT_EXCEEDED{dimension=regex-nfa, limit=100000, observed=>100000}` |
| `regex_lane_unit::nfa_state_estimator_accepts_typical` | `/fn\s+handle_\w+/` → estimated < 1000 states |
| `regex_lane_unit::compile_surfaces_invalid_regex_offset` | `/foo(/` → `PARSE_INVALID_REGEX{offset=5}` |
| `regex_lane_unit::deployment_cap_floor_enforced` | Configuring cap below 1_000 returns config-rejection (floor per [../dsl.md](../dsl.md) §3.4) |

### 6.2 Integration (`cargo test -p quanta-index-lexical --test regex_lane_integration`)

| Test name | Asserts |
|---|---|
| `regex_lane_integration::uc_lex_13_function_handler_regex` | UC-LEX-13 `/fn\s+handle_\w+/` returns the seeded fixture's 5 expected hits in score-desc order, byte-deterministic |
| `regex_lane_integration::uc_lex_14_line_anchor` | UC-LEX-14 `/^fn foo/` returns only line-start matches; verifies `start_line` correctness |
| `regex_lane_integration::uc_lex_06_chunked_anchor` | UC-LEX-06 — anchor `^` per chunked content |
| `regex_lane_integration::uc_lex_21_patterntype_regexp_mode` | `patterntype:regexp fn\s+\w+` parses every bare token as a regex leaf |
| `regex_lane_integration::trigram_recall_no_false_negative_1k_corpus` | Trigram pre-filter has zero false negatives on 1k seeded documents (recall = 1.0) |
| `regex_lane_integration::case_insensitive_inline_flag_normalize` | `/(?i)Foo/` matches `foo` and `FOO`; canonical AST has `case:no` |

### 6.3 Property (`cargo test -p quanta-index-lexical --test regex_lane_property`)

| Test name | Asserts |
|---|---|
| `regex_lane_property::canonical_hash_stable_across_compile` | proptest 1k random RE2-valid patterns × normalize→print→normalize→hash → identical hash (per [../dsl.md](../dsl.md) §10.1, §11.4) |
| `regex_lane_property::dialect_violation_always_typed` | proptest 1k random patterns including forbidden constructs → never panics, always returns one of the typed `PARSE_FORBIDDEN_SYNTAX` codes |
| `regex_lane_property::estimator_upper_bound_holds` | proptest 1k random patterns → estimator ≥ actual compiled NFA state count (defense in depth) |

### 6.4 Loom (`cargo test -p quanta-index-lexical --test regex_cancel_loom --release`)

| Test name | Asserts |
|---|---|
| `regex_cancel_loom::cancel_between_checkpoints_yields_exec_merge_cancel` | A two-thread model with the verifier loop and a cancel signaller: every interleaving observes `EXEC_MERGE_CANCEL` within one checkpoint cadence |

### 6.5 Criterion (`cargo bench -p quanta-index-lexical --bench regex_bench`)

| Bench name | Budget |
|---|---|
| `lex_04_regex_compile_bench` | p99 < 5ms per pattern at NFA ≤ 10k states |
| `lex_04_regex_verify_bench` | p99 < 1ms per 100-candidate-document chunk |

### 6.6 Conformance (PRE-CONF; `cargo test -p quanta-index-contract --test lq_conformance`)

Rows: UC-LEX-05, UC-LEX-06, UC-LEX-08, UC-LEX-13, UC-LEX-14, UC-LEX-21, AC-05, AC-06, AC-07, UC-EDGE-03, UC-EDGE-06.

Each row's golden TOML file lives in `usecase-corpus/<id>.toml`. Verdicts: `ok` for UC-* on green path, `error_expected` for AC-* and UC-EDGE-* rows.

---

## §7 Observability

OpenTelemetry spans per [../rfc.md](../rfc.md) § Observability Requirements §1 augmented by this lane:

| Span | Attributes (closed set; new requires version bump) |
|---|---|
| `lq.plan.regex.estimate` | `nfa_states_estimated: u64`, `nfa_state_cap: u64`, `accepted: bool`, `ticket_id="LEX-04"`, `wave_id` |
| `lq.exec.shard.regex.compile` | `compile_time_us: u64`, `canonical_query_hash: hex32` |
| `lq.exec.shard.regex.prefilter` | `trigram_set_size: u64`, `candidate_count_in: u64`, `candidate_count_out: u64` |
| `lq.exec.shard.regex.verify` | `verified_count: u64`, `cancel_observed: bool`, `early_stop_reason: enum{none,count_reached,timeout,cancel}` |

Metrics (declared with `unit`, `label` set, cardinality cap per [../rfc.md](../rfc.md) § Metric schema, [../implementation-plan.md](../implementation-plan.md) §9):

- `lq_regex_compile_ms` — histogram, unit=`milliseconds`, labels=`{ticket_id, wave_id}`, cardinality=`O(waves)`.
- `lq_regex_prefilter_reduction_ratio` — gauge, unit=`ratio` (0.0–1.0), labels=same.
- `lq_regex_state_count_estimated` — histogram, unit=`count`, labels=same.
- `lq_regex_reject_reason_total` — counter, unit=`count`, labels=`{ticket_id, wave_id, reason ∈ {lookbehind, lookahead, backref, possessive, named_capture_ref, inline_flag, nfa_explosion}}`.

Audit log: per [../rfc.md](../rfc.md) § Security and Authz Model § Audit trail — `error_code?` field carries `PARSE_FORBIDDEN_SYNTAX` / `PLAN_LIMIT_EXCEEDED` / `EXEC_REGEX_COMPILE_EXPLOSION` / `EXEC_SHARD_TIMEOUT` / `EXEC_MERGE_CANCEL` whenever this lane rejects.

Per-wave OBS subset table reference: this ticket emits at Wave 3 entry (per [../implementation-plan.md](../implementation-plan.md) §9.1 Wave 3 row "+ `lq.exec.fanout`, `lq.exec.shard{*}`, `lq.merge`").

---

## §8 Error scenarios

Every error path is typed. No `panic!`. No `Result::ok().unwrap_or(...)`. No silent fallback.

| Scenario | Surface | Code | Where | Test |
|---|---|---|---|---|
| Pattern uses lookbehind | parse | `PARSE_FORBIDDEN_SYNTAX{construct=lookbehind, offset}` | `validate_re2_dialect` | `regex_lane_unit::rejects_lookbehind`, AC-06 |
| Pattern uses lookahead | parse | `PARSE_FORBIDDEN_SYNTAX{construct=lookahead, offset}` | `validate_re2_dialect` | `regex_lane_unit::rejects_lookahead` |
| Pattern uses backreference | parse | `PARSE_FORBIDDEN_SYNTAX{construct=backreference, offset}` | `validate_re2_dialect` | `regex_lane_unit::rejects_backreference`, AC-05 |
| Pattern uses possessive group | parse | `PARSE_FORBIDDEN_SYNTAX{construct=possessive, offset}` | `validate_re2_dialect` | `regex_lane_unit::rejects_possessive_group` |
| Pattern uses named-capture ref | parse | `PARSE_FORBIDDEN_SYNTAX{construct=named-capture-ref, offset}` | `validate_re2_dialect` | `regex_lane_unit::rejects_named_capture_ref` |
| Pattern uses inline flag mid-pattern | parse | `PARSE_FORBIDDEN_SYNTAX{construct=inline-flag-midpattern, offset}` | `validate_re2_dialect` | `regex_lane_unit::rejects_inline_flag_midpattern` |
| Pattern fails RE2 syntax | parse | `PARSE_INVALID_REGEX{offset, re2_message}` | `regex_syntax::parse` wrap | `regex_lane_unit::compile_surfaces_invalid_regex_offset`, UC-EDGE-03 |
| Pattern exceeds NFA budget at estimator | plan | `PLAN_LIMIT_EXCEEDED{dimension=regex-nfa, limit, observed}` | `estimate_nfa_states` + cap check | `regex_lane_unit::nfa_state_estimator_rejects_explosion`, AC-07 |
| Pattern compiles but `regex::Regex::new` blows out | plan/exec | `EXEC_REGEX_COMPILE_EXPLOSION{budget, observed}` | `regex::Regex::new` wrap | unit test injecting `RegexBuilder::size_limit(small)` |
| Per-shard verify exceeds `timeout:` budget | exec | `EXEC_SHARD_TIMEOUT{shard_id, budget_ms, elapsed_ms}` | checkpoint hook | `regex_lane_integration::uc_edge_06_timeout` |
| Cancellation signal observed mid-verify | exec | `EXEC_MERGE_CANCEL{at_checkpoint}` | checkpoint hook | `regex_cancel_loom::cancel_between_checkpoints_yields_exec_merge_cancel`, UC-OPS-02 |
| Trigram authority returns sibling not ready | exec | `STATE_NOT_READY: STALE_SIBLING{sibling=trigram, manifest_gen, sibling_gen}` | `open_trigram_reader` wrap | integration test against an unready fixture |
| Per-tenant fanout cap exceeded | exec | `PLAN_LIMIT_EXCEEDED{limit_kind=fanout, budget, observed}` | LEX-05 inherits; this lane re-raises | covered by LEX-05 |

There are no untyped error paths. There is no `partial:allow` opt-in in scope for this ticket — partial results on timeout / cancel are **not surfaced**.

---

## §9 Perf envelope

Targets reference [../feature-scope.md](../feature-scope.md) §7 Scale & capacity scope and [../rfc.md](../rfc.md) § Capacity and SLO Targets.

| Dimension | Target | Source |
|---|---|---|
| `lq.plan.regex.estimate` p99 | < 200 µs | reasonable HIR walk depth bound |
| `lq.exec.shard.regex.compile` p99 | < 5 ms at NFA ≤ 10k states | regex crate compile profile |
| `lq.exec.shard.regex.prefilter` p99 | < 10 ms at 1M-doc index | trigram intersection vs doc-id set |
| `lq.exec.shard.regex.verify` p99 | < 1 ms / 100 candidates | `is_match` per chunk |
| Single-repo regex query (UC-LEX-13 shape) p95 | < 250 ms | [../rfc.md](../rfc.md) § Latency SLOs warm |
| Single-repo regex query (cold) p99 | < 1000 ms | [../rfc.md](../rfc.md) § Latency SLOs cold |
| 100-repo regex fanout p95 | < 2 s | [../rfc.md](../rfc.md) § Latency SLOs fanout |
| NFA-state cap (default) | 100_000 | [../dsl.md](../dsl.md) §13, [../rfc.md](../rfc.md) § Canonical Query Model §7 |
| NFA-state cap (floor) | 1_000 | [../dsl.md](../dsl.md) §3.4 |
| Per-query memory soft cap | 256 MiB | [../dsl.md](../dsl.md) §13 |
| Per-query CPU soft cap | 5 s | [../dsl.md](../dsl.md) §13 |

Regression budget: criterion benches' p99 may not increase >5% per wave without ADR ([../implementation-plan.md](../implementation-plan.md) §8.2).

---

## §10 Risks

| ID | Description | Prob | Impact | Early-warning | Mitigation |
|---|---|---|---|---|---|
| R-LEX04-1 | `regex_syntax::hir::analysis::Properties` upper bound overshoots → false `PLAN_LIMIT_EXCEEDED` rejections of legitimate queries | M | H | `regex_lane_property::estimator_upper_bound_holds` failure on a real-world pattern; user complaint | Property test 10k real-world patterns; add an estimator-vs-actual delta log; ADR-017 documents the tolerated overshoot factor |
| R-LEX04-2 | Tantivy `RegexQuery` is used inside our verify step and diverges in dialect from `regex` crate | M | M | conformance row drift between unit (uses `regex`) and integration (uses Tantivy) | Make `regex::Regex::is_match` the verify authority; Tantivy's regex only feeds the trigram-style automaton; ADR-017 asserts the split |
| R-LEX04-3 | Cancellation checkpoint cadence (default `N=64` candidates or `M=1ms`) too coarse → SLO miss on cancel latency | M | M | `regex_cancel_loom` red on a tighter assertion | Make cadence configurable per deployment; criterion bench `lex_04_regex_verify_bench` measures cancel-observed latency |
| R-LEX04-4 | `regex` crate minor bump shifts state-count semantics → silent regression in cap behavior | M | H | criterion `lex_04_regex_compile_bench` p99 jumps >5%; `regex_lane_property::estimator_upper_bound_holds` red | Pin `regex = "=1.10.x"`; cargo update gated behind ADR; quarterly audit ticket |
| R-LEX04-5 | Trigram authority's required-trigram lowering of a pattern with broad character class (`/\w+/`) → no useful pre-filter → O(corpus) verify | H | M | `lq_regex_prefilter_reduction_ratio` collapses toward 1.0 | Document the failure mode in ADR-017; planner may surface `PLAN_LIMIT_EXCEEDED{dimension=regex-no-anchor}` when the lowering produces no required trigrams and the candidate-set is unbounded — typed reject, not silent slow path |
| R-LEX04-6 | Unicode property classes `\p{L}` blow up character-class size on languages with large alphabets | M | M | NFA state estimator returns very large counts on legitimate patterns | Estimator caps recognized Unicode classes against the deployment cap; documented in ADR-017 |
| R-LEX04-7 | Fuzz-discovered edge case in `regex_syntax` panics | L | H | `cargo +nightly fuzz` red on a pattern | Add `regex_lane_property::dialect_violation_always_typed` proptest + a `cargo-fuzz` corpus seeded with the AC-* rows |
| R-LEX04-8 | RE2-dialect drift in Sourcegraph upstream → corpus row `SG=` → `SG~` | M | M | Sourcegraph release advances; drift report flags a regex row | Per [../usecase.md](../usecase.md) §6 versioning policy: drift reported, never auto-accepted; new ADR or RFC amendment required |

ADR slot pre-seeded: **ADR-017 — RE2 implementation choice and NFA-state estimator policy**. Forcing function: Wave 2 entry for the parse-time leg of this ticket. ADR file path: `docs/adr/ADR-017-re2-regex-engine.md`.

---

## §11 DoD (provable)

Each row is one provable artifact. Per [../implementation-plan.md](../implementation-plan.md) §1.4 claimability rule. All 18 rows shipped (81 tests in `quanta-index-lq-regex`). AST-level precise detection landed for `Possessive (?>...)`, `NamedCaptureRef \k<name>`, and mid-pattern `InlineFlagMidPattern`; leading-position `(?i)` ACCEPT policy is locked.

1. ✓ shipped — RE2 dialect filter rejects all 6 forbidden constructs at parse time with typed `PARSE_FORBIDDEN_SYNTAX`, payload includes offset + construct name. AST-level precise detection landed for `Possessive`, `NamedCaptureRef`, `InlineFlagMidPattern`. Proof: `regex_lane_unit::rejects_{lookbehind, lookahead, backreference, possessive_group, named_capture_ref, inline_flag_midpattern}` green in `cargo test -p quanta-index-lq-regex --test regex_lane_unit`.
2. ✓ shipped — NFA-state estimator rejects `(a?){101}a{101}` and similar with `PLAN_LIMIT_EXCEEDED{dimension=regex-nfa, limit=100000, observed>100000}`. Proof: `regex_lane_unit::nfa_state_estimator_rejects_explosion`.
3. ✓ shipped — Estimator upper bound holds on 1k random RE2-valid patterns. Proof: `regex_lane_property::estimator_upper_bound_holds`.
4. ✓ shipped — Trigram pre-filter has zero false negatives on a 1k-document seeded corpus. Proof: `regex_lane_integration::trigram_recall_no_false_negative_1k_corpus`.
5. ✓ shipped — Cancellation observed mid-verify yields `EXEC_MERGE_CANCEL` within ≤ 1ms of signal under loom interleavings. Proof: `regex_cancel_loom::cancel_between_checkpoints_yields_exec_merge_cancel`.
6. ✓ shipped — `timeout:1ms count:all /.*/` returns typed `EXEC_SHARD_TIMEOUT` end-to-end, no partial results surfaced. Proof: `regex_lane_integration::uc_edge_06_timeout` + UC-EDGE-06 golden row.
7. ✓ shipped — UC-LEX-13 and UC-LEX-14 conformance rows green in PRE-CONF. Proof: `cargo test -p quanta-index-contract --test lq_conformance` + `usecase-corpus/UC-LEX-13.toml` + `usecase-corpus/UC-LEX-14.toml`.
8. ✓ shipped — AC-05, AC-06, AC-07, UC-EDGE-03, UC-EDGE-06 conformance rows surface the documented typed error code. Proof: same harness, `error_expected` verdict.
9. ✓ shipped — Canonical hash stable for every regex query across two runs and two architectures. Proof: `regex_lane_property::canonical_hash_stable_across_compile` × CI matrix x86_64 + aarch64.
10. ✓ shipped — No `unwrap` / `unwrap_or` / `Result::ok` on production paths in this lane. Proof: clippy `-D warnings` with disallowed-methods rail green.
11. ✓ shipped — No `#[derive(Serialize|Deserialize)]` in this lane. Proof: semgrep `rust-no-serde-derive` ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)) green.
12. ✓ shipped — Vendor pinning: `regex = "=1.10.x"`, `regex_syntax = "=0.8.x"` in workspace `Cargo.toml`. Proof: `cargo tree -p regex` output checked into ADR-017 references; `cargo deny` green.
13. ✓ shipped — ADR-017 lands at `docs/adr/ADR-017-re2-regex-engine.md` covering vendor choice, estimator policy, cap configurability, Tantivy-vs-regex-crate split. Leading-position `(?i)` ACCEPT policy recorded.
14. ✓ shipped — Telemetry spans `lq.plan.regex.estimate`, `lq.exec.shard.regex.{compile, prefilter, verify}` emit with the closed attribute set declared in §7. Proof: integration test asserting span attribute keys.
15. ✓ shipped — Metric `lq_regex_reject_reason_total` emits one increment per rejected pattern with the documented `reason` label. Proof: integration test asserting metric scrape.
16. ✓ shipped — Per-wave OBS subset met: Wave 3 entry has spans emitting. Proof: cross-reference [../implementation-plan.md](../implementation-plan.md) §9.1 Wave 3 row.
17. ✓ shipped — Bench `lex_04_regex_compile_bench` and `lex_04_regex_verify_bench` p99 within budgets stated in §9. Proof: criterion CSV in CI artifacts; regression budget enforced.
18. ✓ shipped — Structured agent output for this ticket validates against [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json). Missing evidence → `blocked`, not `ok`.

---

## §12 Open questions

| Q-ID | Question | Source | Blocking |
|---|---|---|---|
| Q-LEX04-1 | Does the deployment cap on NFA states sit in `LqOptionSet` (per-query overridable down to floor) or in deployment config only (per [../dsl.md](../dsl.md) §13)? RFC § Canonical Query Model §7 implies deployment-config-only. **Default answer**: deployment-config-only; `LqOptionSet` has no field for it; per-query overrides are out of scope. Forces: ADR-017. |
| Q-LEX04-2 | When the required-trigram lowering of a pattern produces an empty trigram set (e.g. `/.*/`), do we reject with `PLAN_LIMIT_EXCEEDED{dimension=regex-no-anchor}` or fall back to O(corpus) verify? **Default answer**: reject — fail-closed posture, no heuristic slow path. Caller can add `/fn .*/` style anchor. Open: is the `dimension` key new? If so, document under [../dsl.md](../dsl.md) §12. |
| Q-LEX04-3 | Does `LQ/Core-1.0 + patterntype:regexp` also accept Sourcegraph's structural fallback when a pattern matches `match { ... }` shape? **Default answer**: no — `patterntype:structural` is the only path; mixing yields `PARSE_UNSUPPORTED_COMBO`. Cross-reference [../dsl.md](../dsl.md) §4.2 final bullet. |
| Q-LEX04-4 | Should `\p{L}` and other Unicode property classes count toward the NFA state cap with their full character-class size, or with a normalized stand-in count? **Default answer**: full size — defensive; ADR-017 records the choice and the worst-case (`\p{L}` ~140k codepoints would exceed the default cap, forcing the caller to anchor or refine). |
| Q-LEX04-5 | Where does the trigram authority physically live — same Tantivy index segment as content, separate segment, separate crate? **Default answer**: separate Tantivy field within the lexical content index, per LEX-03 sibling-shard model. ADR-002 (symbol shard layout) and LEX-03 own the storage layout; this ticket consumes whatever LEX-03 lands. |
| Q-LEX04-6 | When `case:no` is set and the pattern contains an explicit `(?i)` head flag, are they merged or is the duplicate rejected? **Default answer**: merged — `(?i)` desugars at stage 6 to `case:no` per [../dsl.md](../dsl.md) §10, and `case:no` set explicitly is a no-op against the same canonical AST. |
| Q-LEX04-7 | Conformance ordering: do we activate `regex_lane_integration::uc_lex_13` at Wave 2 (parse + plan only, executor is stub) or wait until Wave 3 (real executor)? **Default answer**: Wave 2 entry runs the row through PRE-CONF's `StubLqEngine`; the `ok` verdict requires Wave 3 real-executor wiring. Until then the row is `blocked`. Cross-reference [../implementation-plan.md](../implementation-plan.md) §8.4 mock policy. |

---

## §13 References

- [../rfc.md](../rfc.md) — May-23 Sourcegraph-Class Lexical Kernel RFC: § LQ family `LQ/Core-1.0` regex dialect; § Canonical Query Model § bounded inputs; § Non-Negotiable Invariants §10 (no unbounded memory in regex/structural matching); § Error Code Taxonomy `PARSE_*`, `PLAN_*`, `EXEC_*`; § Capacity and SLO Targets; § Observability Requirements; § Claim Discipline.
- [../feature-scope.md](../feature-scope.md) — §1.1.1 regex leaf row; §1.1.6 `patterntype:` mode matrix; §7 Scale & capacity scope; §9 Q7 (`count:all` ceiling); §10 References.
- [../usecase.md](../usecase.md) — UC-LEX-05/06/08/13/14/21 regex rows; AC-05/06/07 forbidden-regex anti-rows; UC-EDGE-03 regex compile fail; UC-EDGE-06 timeout exceeded; §0 error code SSOT; §3 contract gap GAP-06 (`LexicalErrorCode` enum).
- [../dsl.md](../dsl.md) — §3.4 regex dialect (RE2, forbidden constructs, anchors, mapping, compile-time cost cap); §4 `patterntype:` mode matrix; §6.2 filter table (`patterntype:` row); §10 normalization (`(?i)` desugar); §11 canonical hash; §12 error taxonomy; §13 limits and budgets; §16 non-negotiable DSL invariants (no silent regex dialect drift away from RE2).
- [../implementation-plan.md](../implementation-plan.md) — §1 claimability rule; §2.1 contract state; §2.3a working-tree divergence (G-CONTROL-LOC); §2.4 lexical adapter state; §5 per-ticket DoD discipline; §8 test strategy; §9 observability and SLO gates; §10 decision log (ADR slots); §11 open questions.
- [../../../../CLAUDE.md](../../../../CLAUDE.md) — Agent change posture (breaking-first); Rule Catalog (Safety, Architecture, Build hygiene D18, Verification, Documentation, Testing).
- [../../../../AGENTS.md](../../../../AGENTS.md) — Shared agent router.
- [../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive` (D18 enforcement).
- [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — Structured agent output schema (`ok` / `blocked` / `error_expected`).
- [../../../../tools/ci/lint/lint-doc-paths.py](../../../../tools/ci/lint/lint-doc-paths.py) — Doc-link linter (run after this ticket lands).
- [../../../ssot/producer-handoff.md](../../../ssot/producer-handoff.md) — producer handoff SSOT.
- [INDEX.md](INDEX.md) — ticket index (downstream-migration follow-up tracked under §3.6).
