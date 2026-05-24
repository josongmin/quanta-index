# STR-01 — Structural Pattern Engine

> Status: `Spec — Wave 5 ticket (LQ/Structural-1.2)`
> Parent RFC: [../rfc.md](../rfc.md) § Structural engine, § `LQ/Structural-1.2`
> Parent plan: [../implementation-plan.md](../implementation-plan.md) § 4.6 Wave 5, § 5.12 STR-01
> Grammar source: [../dsl.md](../dsl.md) §8 Structural sub-grammar
> Scope: [../feature-scope.md](../feature-scope.md) §1.3 `LQ/Structural-1.2`
> Conformance corpus rows: [../usecase.md](../usecase.md) §2.E `UC-STR-01..07`, §4 `AC-07`
> Authority posture: **breaking-first** per [../../../../CLAUDE.md](../../../../CLAUDE.md) § Agent change posture
> Schema check: structured outputs must validate against [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json)

---

## §1 Purpose

Stand up the structural pattern engine for `LQ/Structural-1.2`: tree-sitter-
backed syntax cache, language-anchored pattern IR, metavariable / variadic /
`inside` / `outside` / `where` matcher, and the typed `StructuralCandidate`
carrier that closes contract gap GAP-03 from [../usecase.md](../usecase.md) §3.

At wave end:

1. every `match { … }` body in the 5 ship-grammar set produces a structural
   candidate or typed error — never an empty success.
2. the structural engine is **disjoint** from the lexical content engine; it
   does **not** reuse Tantivy regex as a structural-search heuristic ([../rfc.md](../rfc.md) § Structural engine § must not).
3. structural matches expose `StructuralBinding` (metavariable → byte span) on
   the wire as the canonical carrier for `where` constraints and bridge
   consumers.
4. RFC § Claim-Discipline §4 becomes provable against corpus rows
   `UC-STR-01..07` and the negative row `AC-07`.

Out of scope: typed-hole eval (`:[hole.type1]`, deferred per
[../feature-scope.md](../feature-scope.md) §1.3.3); structural → CodeQL bridge
edge (`PLAN_DEFERRED`, BRIDGE-01 follow-on); post-Wave-5 grammars
(C / C++ / Ruby).

## §2 Background

The repo has **no structural engine** ([../implementation-plan.md](../implementation-plan.md) §2.7 — absent across
contract, core, adapter, and conformance). The lexical adapter exposes
`RegexQuery` against the Tantivy `text` field; using that as structural search
is explicitly forbidden by RFC § Structural engine § must not.

Wave 5 unlocks `LQ/Structural-1.2`. STR-01's only blocker is `LEX-06`
([../implementation-plan.md](../implementation-plan.md) §3.2) — by Wave 5 entry the lexical plane has a typed
AST, deterministic merge, and an explainable ranker, so the structural engine
plugs in as a sibling planner without re-litigating the front door.

Per RFC § Engine Decomposition § Structural engine, the owner is tree-sitter
per language + per-file syntax cache + normalized pattern-match IR + syntax-
node walk / capture matcher. Structural and lexical **do not share a candidate
space** for `LQ/Core-1.0` (see §4.6); cross-family ranking is deferred.

Pre-decision dependencies:

- Wave-0 PRE-CONTRACT-EXT has landed `StructuralCandidate { bindings:
  BTreeMap<String, Span> }` per [../implementation-plan.md](../implementation-plan.md) §5.1 DoD item 3.
- Wave-1 LEX-01 parser normalizes `:[X] → $X`, `:[...ARGS] → $...ARGS` at
  the lexer stage per [../dsl.md](../dsl.md) §8.6.
- Wave-2 LEX-03 has a per-generation reader cache pattern this ticket mirrors.
- Wave-4 LEX-06 has shipped explain envelope v2; STR-01 emits structural
  entries through the same schema.
- ADR-003 (per-grammar IR vs unified IR — [../feature-scope.md](../feature-scope.md) §1.3.4 Q4) is resolved
  at wave entry per [../implementation-plan.md](../implementation-plan.md) §10.

## §3 Inputs

Authoritative input docs:

1. [../rfc.md](../rfc.md) § Structural engine, § `LQ/Structural-1.2`, § Non-Negotiable
   Invariants 1, 2, 10, § Error Code Taxonomy.
2. [../feature-scope.md](../feature-scope.md) §1.3 (structural feature catalog), §4.3
   (ticket cross-reference), §6.3 (authority chain), §7 (scale caps).
3. [../usecase.md](../usecase.md) §2.E (`UC-STR-01..07`), §3 (`GAP-03`), §4 (`AC-07`).
4. [../dsl.md](../dsl.md) §2.3 (`LQ/Structural-1.2` EBNF), §8 (structural
   sub-grammar), §10 (normalization), §12 (error taxonomy), §13 (limits).
5. [../implementation-plan.md](../implementation-plan.md) §4.6, §5.12, §10 ADR-003,
   ADR-005, ADR-012.

Authoritative bound caps from RFC § Canonical Query Model §7 and dsl.md §13:

- max structural pattern node count: **256**
- max structural pattern depth: **16**
- per-query memory soft limit: 256 MiB (default), floor 16 MiB
- per-query CPU soft limit: 5 s (default), floor 250 ms

## §4 Deliverables

### §4.1 Crate

Create the `quanta-index-structural` adapter crate per ADR-012
([../implementation-plan.md](../implementation-plan.md) §10). The crate sits on the
adapter side of the hexagonal boundary; the core port lives in
`quanta-index-core::domains::structural` (path subject to G-CONTROL-LOC per
[../implementation-plan.md](../implementation-plan.md) §2.3a / §11).

Allowed dependencies (tracked in `tools/ci/lint/lint-hexagonal-boundaries.py`):

- `tree-sitter` (engine) plus per-grammar crates: `tree-sitter-rust`,
  `tree-sitter-python`, `tree-sitter-typescript`, `tree-sitter-javascript`,
  `tree-sitter-go`.
- `quanta-index-contract` (for `StructuralCandidate`, `LqQueryV1`,
  `LexicalErrorCode`).
- workspace-pinned `regex` for `where … == "string"` constraint evaluation only
  (no RE2 NFA over file content — that lives in the lexical adapter).

Forbidden dependencies (enforced by hexagonal lint):

- `rusqlite`, `tantivy`, `lancedb`, raw filesystem layout — these are storage
  decisions and stay in adapters that own them.
- direct edges into `quanta-index-lexical` — structural plane is sibling, not
  child.

### §4.2 Port traits in core

Land in `quanta-index-core::domains::structural`:

```
SearchPlaneStructuralIndexBuildPort
  fn build_syntax_cache(
      &self,
      generation: PublishedGenerationId,
      file_set: &ChangedFileSet,
  ) -> Result<SyntaxCacheBuildReport, CoreError>;

SearchPlaneStructuralIndexQueryPort
  fn query_structural(
      &self,
      plan: &StructuralPlan,
      pin: BoundGenerationPin,
  ) -> Result<StructuralResultStream, CoreError>;
```

Both ports must be `Send + Sync` and surface failures only as typed
`CoreError::{InvalidContract, NotReady, NotFound, NotImplemented}` — no
free-text error strings, no untyped fall-through.

### §4.3 Contract surface

Land in `quanta-index-contract` (extends what PRE-CONTRACT-EXT seeded for
GAP-03):

- `StructuralCandidate { candidate_id, repo_id, revision_id,
  manifest_generation, repo_relative_path, span: ByteRange, bindings:
  StructuralBindings }`
- `StructuralBindings { entries: BTreeMap<MetaVar, Span> }` —
  `BTreeMap` for canonical iteration order; `MetaVar` and `Span` are typed
  newtypes; `Eq + Hash + Serialize + Deserialize` per RFC § Migration policy.
- `StructuralPlan` — pattern IR + language-resolution result; lives in the
  contract so the planner can serialize plans into the executor.
- `StructuralErrorPayload` for the typed `STR_*` codes below.

Serialization: hand-rolled `impl serde::Serialize` / `impl serde::Deserialize`
per CLAUDE.md § Build hygiene; `#[derive(Serialize)]` is banned by semgrep
`rust-no-serde-derive` ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)).

### §4.4 Pipeline stages

Lock the pipeline so each stage is independently testable:

1. **parse structural pattern** — consume the `StructuralBlock` from LEX-01
   (aliases already desugared) → typed pattern IR per [../dsl.md](../dsl.md) §8.
2. **language router** — resolve target language per [../dsl.md](../dsl.md) §8.5:
   explicit `lang:` > implied by `file:` regex > per-grammar dispatch. Empty
   resolution → `STR_LANG_RESOLUTION_EMPTY`.
3. **tree-walk match** — walk cached tree-sitter AST per
   `(repo, rev, generation, file)`; emit anchored match positions.
4. **metavar bind** — capture `$X`, `$...X`, `...`, typed-hole placeholders
   (typed-hole eval gated to `NotImplemented`).
5. **`where` evaluator** — apply post-bind constraints
   ([../dsl.md](../dsl.md) §8.3); unbound `HoleRef` → `STR_INVALID_METAVAR`.
6. **emit candidates** — `StructuralCandidate` rows into a **separate result
   space** from lexical (§4.6).
7. **bridge into ranker** — see §4.6.

### §4.5 Generation pinning

Structural shards have their own generation per RFC § Generation model
(`structural cache generation`). Each shard's generation is independently
monotonic per RFC § Monotonicity rules item 2; the manifest publishes a
`generation_set` binding the structural generation to its sibling
lexical/path/symbol generations. The reader-side pin
(`BoundGenerationPin`) is **per-query** and pins the whole `generation_set`
— mid-query cross-shard skew is impossible by construction. Per-shard
staleness fails closed with `STATE_NOT_READY: STALE_SIBLING` per RFC §
Atomicity contract. Open question Q-STR-01-GEN is **closed here**.

### §4.6 Lexical / structural disjointness

`LQ/Core-1.0` keeps structural results in a **separate envelope** from
lexical results. Rationale:

1. metavariable bindings have no `LexicalCandidate` representation
   (GAP-03 from [../usecase.md](../usecase.md) §3); promoting structural into the lexical
   envelope erases the bindings.
2. cross-family ranking is undefined — there is no BM25 score for a tree-walk
   match. Mixing the two forces an arbitrary score blend, forbidden by RFC §
   Non-Negotiable Invariants §4.
3. front-door routing per [../dsl.md](../dsl.md) §4.2 pins
   `patterntype:structural` as single-block: only one top-level
   `match { … }` block is permitted as the Expression.

Consequence: STR-01 emits a `SearchPlaneStructuralQueryResponse` envelope
(disjoint from `SearchPlaneLexicalQueryResponse`). Both share the same
generation_set carrier and merge order discipline (§4.7) but never mix
candidates. Cross-family hybrid ranking → future ticket; see §12 Q-STR-01-RANK.

### §4.7 Deterministic emission order

Structural results must be deterministic across instances per RFC § Merge
determinism rule, adapted for the structural domain:

```
structural merge order = (
    repo_id ASC,
    repo_relative_path ASC,
    span.start ASC,
    span.end ASC,
    canonical_pattern_node_id ASC,
)
```

Every component is total within its domain → ties cannot exist → byte-exact
reproducibility across instances per RFC § Claim Discipline §8.

### §4.8 Engine-choice ADR-003 candidate (locked)

| Candidate | Determinism | Bounded resource | Per-language ergonomics | Verdict |
|---|---|---|---|---|
| **Comby (regex+balanced delimiters)** | partial — balanced-delim heuristics drift per language; corner cases not deterministic across input shape | unbounded backtracking on adversarial inputs | weak — single regex-flavored DSL across all languages | reject |
| **Stack-machine on RE2** | RE2 deterministic ([../dsl.md](../dsl.md) §3.4); flat byte stream cannot express tree structure → custom stack walker → mixed surface | RE2 NFA bounded; stack walker custom (unbounded unless capped) | weak — every grammar needs hand-written stack rules | reject |
| **Tree-sitter native walk** | parse deterministic per grammar version; preorder walk deterministic | per-language grammar pin (ADR-005); query plan O(AST-nodes × pattern-nodes) | strong — one grammar per language; metavariable capture is the upstream idiom | **accept** |

Recommendation: tree-sitter native walk + normalized pattern IR; per-grammar
lowering adapter. Q4 in [../feature-scope.md](../feature-scope.md) §1.3.4 is closed: **unified pattern IR
with per-grammar lowering** (per-grammar IR variants multiply surface area and
defeat the "language-normalized pattern IR" baseline in RFC § Recommended
Concrete Engine Choices § Structural).

### §4.9 Metavariable binding carrier (locked)

`StructuralBinding` is the canonical metavariable carrier on the wire and
closes GAP-03 from [../usecase.md](../usecase.md) §3. Lock per [../dsl.md](../dsl.md) §8.1:

| Hole kind | Binding shape | Carrier field |
|---|---|---|
| `$X` (single-token) | `Span` (one node range) | `bindings[X] = Span{ start, end }` |
| `$...X` (multi-token / variadic) | `Span` (n-ary span) | `bindings[X] = Span{ start, end }` covering the captured range |
| `...` (anonymous wildcard) | not captured | absent from `bindings` (cannot be referenced by `where`) |
| `:[hole.type=...]` (typed hole) | typed binding — gated `NotImplemented` per [../feature-scope.md](../feature-scope.md) §1.3.3 | parser accepts; matcher returns `STR_TYPED_HOLE_NOT_IMPLEMENTED` |

Sourcegraph alias normalize `:[X] → $X` and `:[...ARGS] → $...ARGS` is the
parser's job (already done in LEX-01 / PRE-NORM per
[../dsl.md](../dsl.md) §10 step 6). The structural plane never sees the
alias forms — corpus row `UC-STR-07` asserts this.

### §4.10 Language matrix v1 (ship)

Per [../feature-scope.md](../feature-scope.md) §1.3.4 row table, locked language set for STR-01 ship:

| Language | tree-sitter crate | Pin policy (ADR-005) | Ship? |
|---|---|---|---|
| Rust | `tree-sitter-rust` | workspace-pin exact version | ship |
| Python | `tree-sitter-python` | workspace-pin exact version | ship |
| TypeScript | `tree-sitter-typescript` (TS + TSX subgrammars) | workspace-pin exact version | ship |
| JavaScript | `tree-sitter-javascript` | workspace-pin exact version | ship |
| Go | `tree-sitter-go` | workspace-pin exact version | ship |
| Java | `tree-sitter-java` | stretch — falls to `NotImplemented` if cut for time per [../feature-scope.md](../feature-scope.md) §1.3.4 | stretch (cut from ship gate) |
| C / C++ / Ruby | per [../feature-scope.md](../feature-scope.md) §1.3.4 | post-STR-01 — `NotImplemented` | post-ship |

Divergence from [../feature-scope.md](../feature-scope.md) §1.3.4: Java stretch is
**cut from the Wave-5 exit gate** to keep the wave atomic; partial language
coverage is `NotImplemented` per language per RFC § Non-Negotiable Invariants
§7. Q9 (`feature-scope.md` §9 — sub-language structural support order) defaults
to **gate together for the 5 ship grammars**; Java rides a follow-up ticket.

### §4.11 Conformance gating

Wave-5 exit per [../implementation-plan.md](../implementation-plan.md) §4.6 requires:

- `UC-STR-01..07` green in PRE-CONF;
- `AC-07` (unbounded structural recursion) returns `PLAN_LIMIT_EXCEEDED` per
  [../dsl.md](../dsl.md) §13;
- tree-sitter parse cost p99 < 50 ms per file (criterion `str_01_parse_bench`);
- RFC § Claim-Discipline §4 provable (tree-sitter-backed matcher exists).

## §5 Implementation steps (TDD)

Each step is failing-test-first: red test, then impl, then green in CI.

### §5.1 Step 1 — port traits + contract types
Failing tests: `StructuralCandidate` CBOR round-trip stable across runs;
`StructuralBindings` `BTreeMap` ordering is lexicographic;
`SearchPlaneStructuralIndexQueryPort` trait object is `Send + Sync`. Impl:
hand-rolled serde, no `#[derive]`.

### §5.2 Step 2 — language router
Failing tests over `resolve_language`: explicit `lang:Rust` → `[Rust]`;
`file:^src/.*\.py$` → `[Python]`; no `lang:` no `file:` → ship set; explicit
`lang:Cpp` → `STR_LANG_NOT_SUPPORTED`. Implement per [../dsl.md](../dsl.md) §8.5 ordering;
empty resolution → `STR_LANG_RESOLUTION_EMPTY`.

### §5.3 Step 3 — tree-sitter parser cache
Failing tests: `SyntaxCache::get_or_parse(repo, rev, generation, file, src)`
hits on second call; key includes `(generation, file_content_hash)`;
generation bump invalidates; `MARKER_OK` per entry asserted at pre-flight.
Impl: bounded LRU (default 1 GiB).

### §5.4 Step 4 — pattern IR + tree-walk matcher
Failing tests against fixture sources: `match { fn $X(...) { ... } }` binds
`$X = "foo"` on `fn foo() {…}`; `handle($...ARGS)` binds variadic; `...`
anonymous wildcard not captured; `inside { fn handler {…} } match { unwrap() }`
scopes; `outside { fn test_$_ {…} } match { panic!(...) }` excludes. Impl per
[../dsl.md](../dsl.md) §8.

### §5.5 Step 5 — `where` constraint evaluator
Failing tests: `where $X == $Y` matches when spans byte-identical; `where $F
== "exec"` matches literal text; unbound hole ref → `STR_INVALID_METAVAR{ref:
"$Z"}`.

### §5.6 Step 6 — bounded-input enforcement
Failing tests: pattern > 256 nodes → planner emits
`PLAN_LIMIT_EXCEEDED{dimension: "structural-pattern-node-count", …}`;
depth > 16 → `PLAN_LIMIT_EXCEEDED{dimension: "structural-pattern-depth", …}`;
per-query memory soft limit emits typed early-stop signal (no silent truncate
per RFC § Non-Negotiable Invariants §10). Impl: `regex_syntax`-style upper-
bound pre-check.

### §5.7 Step 7 — typed-hole `NotImplemented` gate
Failing tests: `:[hole.type=expr]` matcher → `STR_TYPED_HOLE_NOT_IMPLEMENTED`;
unknown type-name `:[hole.type=goblin]` parser-rejected →
`PARSE_INVALID_FILTER_VALUE{filter: "hole.type"}` per [../dsl.md](../dsl.md) §8.2.

### §5.8 Step 8 — deterministic emission order
Property test: matcher run twice → byte-identical envelope (RFC § Claim
Discipline §8). Cross-instance reproducibility: single-binary two-process CI
step asserts identical CBOR envelope. Impl: explicit preorder + §4.7 merge tuple.

### §5.9 Step 9 — conformance corpus rows
Add 8 `*.toml` files under `usecase-corpus/structural/` per [../usecase.md](../usecase.md) §6:
`UC-STR-01.toml` … `UC-STR-07.toml`, `AC-07.toml`. Each has a query string +
`[expected]` block. Run PRE-CONF: rows go from `blocked` → `ok` at STR-01 exit
via `cargo test -p quanta-index-contract --test lq_conformance`.

### §5.10 Step 10 — observability + metrics
Tests assert: `lq.structural` span emits per query; `structural_parse_time_ms`,
`structural_match_time_ms`, `structural_candidates_emitted` recorded with
`{ticket_id: "STR-01", wave_id: 5}` dimension.

### §5.11 Step 11 — exit-gate proof
Assemble proof bundle; structured agent output validates against
[../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json). Each DoD item (§11) cites
test name + path + assertion per [../implementation-plan.md](../implementation-plan.md) §1.4. Unprovable
items surface as `blocked`, not `ok`.

## §6 Test plan

Per-rail expectations per [../implementation-plan.md](../implementation-plan.md) §8.1
(STR-01 row: unit ✓ / integration ✓ / conformance ✓ / property ✓ / criterion ✓):

### §6.1 Unit

- `quanta-index-core::domains::structural::tests` — port trait shape,
  contract round-trip.
- `quanta-index-structural::language::tests` — language router resolution.
- `quanta-index-structural::matcher::tests` — per-primitive matcher (`$X`,
  `$...X`, `...`, `where`, `inside`, `outside`).
- `quanta-index-structural::caps::tests` — bounded-input limit enforcement.

### §6.2 Integration

Per-language end-to-end against fixture sources (one fixture per language,
each ≥ 5 sample files):

- `rust_fixture::test_uc_str_01` … `test_uc_str_07`.
- `python_fixture::test_uc_str_01` … `test_uc_str_07`.
- `typescript_fixture::test_uc_str_01` … (per ship grammar).
- `javascript_fixture::test_uc_str_01` …
- `go_fixture::test_uc_str_01` …

### §6.3 Conformance

- `cargo test -p quanta-index-contract --test lq_conformance` walks the
  7 UC-STR-* rows + AC-07 against the live STR-01 engine.
- Verdict is `ok` for UC rows, `error_expected` for AC-07.

### §6.4 Property tests

- Random pattern generation × 1k cases: every generated pattern that fits the
  bounded caps either matches or returns a typed structural error — never
  empty `Ok`.
- Random metavariable binding round-trip: serialize/deserialize 1k random
  `StructuralBindings` → byte-identical CBOR.
- Determinism: 1k pattern × fixture pairs run twice → byte-identical envelope.

### §6.5 Criterion benches

- `str_01_parse_bench` — per-language parse cost; gate p99 < 50 ms per file.
- `str_01_match_bench` — match cost over 1k-file corpus; gate p99 < 1 s
  (Wave-5 exit SLO per [../implementation-plan.md](../implementation-plan.md) §9.2).

### §6.6 Mock policy

Per [../implementation-plan.md](../implementation-plan.md) §8.4: no mocked storage
adapters past wave entry. Tree-sitter is the engine; fixture sources live in
`crates/quanta-index-structural/tests/fixtures/<lang>/`. Producer-side
manifests may be fixture-mocked for cases that do not exercise end-to-end
producer integration.

## §7 Observability

Per [../implementation-plan.md](../implementation-plan.md) §9.1 Wave-5 OBS subset and
RFC § Observability Requirements:

### §7.1 OpenTelemetry spans

- `lq.structural` — root span for the structural query phase.
- `lq.structural.parse_pattern` — pattern IR construction.
- `lq.structural.resolve_lang` — language router.
- `lq.structural.tree_walk` — per-file match execution (one child span per
  file).
- `lq.structural.where_eval` — constraint evaluation.

Each span carries `{ticket_id, wave_id, lang, file_hash_truncated}`.

### §7.2 Metrics (closed label set)

| Metric | Unit | Labels | Cardinality cap |
|---|---|---|---|
| `structural_parse_time_ms` | ms | `{lang}` | 5 |
| `structural_match_time_ms` | ms | `{lang}` | 5 |
| `structural_candidates_emitted` | count | `{lang}` | 5 |
| `structural_pattern_node_count` | count | none | 1 |
| `structural_bound_violation` | count | `{dimension}` (node-count / depth) | 2 |

New labels require a version bump per RFC § Metric schema.

### §7.3 Audit log

Per RFC § Audit trail, the audit row already carries `canonical_query_hash`
and `error_code?` — structural-specific fields ride inside
`SearchExplanation` v2 (`engines: [structural]`).

### §7.4 SLO budgets

Wave-5 exit gate ([../implementation-plan.md](../implementation-plan.md) §9.2):

- structural p99 < 1 s (single-repo, ≤ 1000 files).
- `structural_parse_time_ms` p99 < 50 ms per file.

Soft limits emit a typed early-stop signal; they do **not** silently
truncate (RFC § Non-Negotiable Invariants §10).

## §8 Error scenarios

All error scenarios surface typed `LexicalErrorCode` values per [../dsl.md](../dsl.md)
§12 and RFC § Error Code Taxonomy. New STR-01-introduced codes:

| Code | When fires | Payload | Retry |
|---|---|---|---|
| `STR_PARSE_FAIL` | structural body fails the §8.1 grammar (unbalanced `{`, malformed metavariable). Detected at LEX-01 parser but routed to STR-01 for body-specific diagnostics | `{offset, expected, found}` | not retryable |
| `STR_INVALID_METAVAR` | metavariable used in unsupported position (e.g. `$X` outside `match { … }`, unbound hole-ref in `where`) | `{position, ref}` | not retryable |
| `STR_LANG_NOT_SUPPORTED` | explicit `lang:<id>` outside the §4.10 ship set | `{lang}` | not retryable |
| `STR_LANG_RESOLUTION_EMPTY` | language router resolves to empty set per [../dsl.md](../dsl.md) §8.5 step 3 | `{file_pattern_seen, lang_seen}` | not retryable |
| `STR_TYPED_HOLE_NOT_IMPLEMENTED` | typed-hole eval, deferred per [../feature-scope.md](../feature-scope.md) §1.3.3 | `{hole_type}` | not retryable (until follow-on ticket) |
| `STATE_NOT_READY: SYNTAX_CACHE_UNBUILT` | syntax cache for `(repo, rev, generation, file)` is absent (producer publishes manifest but structural sibling not yet built) | `{repo, rev, generation, file}` | wait-and-retry |
| `PLAN_LIMIT_EXCEEDED` | structural pattern exceeds 256-node or 16-depth cap; emitted from the planner stage before the matcher runs | `{dimension, limit, observed}` | not retryable |

Reused codes (already defined in RFC § Error Code Taxonomy):

- `PARSE_FORBIDDEN_SYNTAX` — `:[…]` outside `match { … }` per [../dsl.md](../dsl.md) §8.6.
- `PARSE_INVALID_FILTER_VALUE{filter: "hole.type"}` — unknown
  type-name per [../dsl.md](../dsl.md) §8.2.
- `PLAN_UNSUPPORTED_COMBO` — `type:commit + match{}` and `type:diff +
  match{}` per RFC § Unsupported-combo table.
- `PLAN_DEFERRED` — `match{} + into:codeql` (structural → CodeQL bridge is
  `BRIDGE-01` follow-on per RFC § Unsupported-combo table).
- `STATE_STALE_SIBLING` — structural shard generation lags manifest-referenced
  structural generation; emitted per RFC § Atomicity contract.

Negative-test corpus targets:

| Anti-row | Asserted code | Source |
|---|---|---|
| `AC-07` (unbounded recursion) | `PLAN_LIMIT_EXCEEDED` | [../usecase.md](../usecase.md) §4 |
| `AC-09` (commit + structural) | `PLAN_UNSUPPORTED_COMBO` | [../usecase.md](../usecase.md) §4 |
| structural pattern with unbalanced bracket | `STR_PARSE_FAIL` (new) | this spec |
| `$X` used outside `match { … }` | `PARSE_FORBIDDEN_SYNTAX` | [../dsl.md](../dsl.md) §8.6 |
| `where $Z == "foo"` where `$Z` was never bound | `STR_INVALID_METAVAR` | this spec |

No silent fallback under any of the rows above (RFC § Non-Negotiable Invariants
§§1, 2, 8). No empty `Ok` when authority is absent — `STATE_NOT_READY` per
repo policy ([../feature-scope.md](../feature-scope.md) §1.5.2 / repo fail-closed posture).

## §9 Performance envelope

Per [../feature-scope.md](../feature-scope.md) §7 and [../implementation-plan.md](../implementation-plan.md) §9.2:

| Dimension | Target | Floor / cap | Source |
|---|---|---|---|
| structural pattern node count | n/a | 256 (hard cap) | RFC § Canonical Query Model §7 |
| structural pattern depth | n/a | 16 (hard cap) | [../feature-scope.md](../feature-scope.md) §7 |
| tree-sitter parse time (per file) | p99 < 50 ms | n/a | Wave-5 exit gate |
| structural query p99 (single-repo) | < 1 s | n/a | Wave-5 exit gate |
| per-query memory soft limit | 256 MiB default | 16 MiB floor | [../dsl.md](../dsl.md) §13 |
| per-query CPU soft limit | 5 s default | 250 ms floor | [../dsl.md](../dsl.md) §13 |
| syntax cache size | 1 GiB default | configurable | this spec |

Criterion regression budget per [../implementation-plan.md](../implementation-plan.md)
§8.2: p99 may not increase > 5% across the wave without an ADR.

Memory accounting: tree-sitter parse trees are reference-counted; the syntax
cache enforces a bounded LRU eviction. Per-query allocations are tracked in
the executor's per-query budget (RFC § Failure model §6.5 budget pattern).

## §10 Risks

Wave-specific risk register per [../implementation-plan.md](../implementation-plan.md) §6:

| ID | Description | Prob | Impact | Early-warning | Mitigation |
|---|---|---|---|---|---|
| R-STR-01-1 | tree-sitter grammar version drift (a single grammar update changes node-kind names) | M | H | grammar publishes a new minor; CI parse-bench mismatches stored fixtures | workspace-pin **exact** version per ADR-005; quarterly audit ticket; per-grammar regression fixture |
| R-STR-01-2 | NFA-like DoS via deeply-nested patterns | M | H | criterion `str_01_match_bench` p99 jumps > 5× | 256-node + 16-depth hard caps; pre-walk pattern complexity estimate; fuzz harness on the pattern IR (matches `R4` from [../implementation-plan.md](../implementation-plan.md) §6) |
| R-STR-01-3 | per-grammar adapter surface explosion (one adapter per language × per node-kind set) | M | M | adapter LoC > 2k per language | ADR-003 locks **unified pattern IR + lowering adapter**; adapter LoC budget 800 per language |
| R-STR-01-4 | syntax cache memory blowup on monorepo (10 GiB+ source) | M | H | RSS > 4× baseline post-cache fill | LRU eviction; cache-size knob; per-generation cache scoping |
| R-STR-01-5 | metavariable binding wire-format ambiguity (variadic vs single under same name) | L | M | property test fails on binding-name reuse | reject duplicate hole names at parser stage ([../dsl.md](../dsl.md) §8.1) |
| R-STR-01-6 | language-router false negative — `file:` regex implies wrong language | L | M | UC fixture mismatch | per-test fixture asserts router output; emit `STR_LANG_RESOLUTION_EMPTY` instead of silent skip |
| R-STR-01-7 | typed-hole `NotImplemented` gate leaks (parser silently accepts and matcher silently drops) | L | H | UC fixture covers typed-hole row | fail-closed: matcher always returns `STR_TYPED_HOLE_NOT_IMPLEMENTED` when a typed hole is reached; never empty `Ok` |
| R-STR-01-8 | cross-instance non-determinism (tree-sitter walker visits in different order on different platforms) | L | H | cross-instance CI step diffs by ≥ 1 byte | enforce explicit preorder per §5.8; pin per-grammar walker version |

## §11 Definition of Done (provable)

Every item below is provable per [../implementation-plan.md](../implementation-plan.md)
§1.4 claimability rule (test name + path + assertion cited):

1. **`StructuralCandidate` carrier ships** —
   `crates/quanta-index-contract/tests/structural_candidate_round_trip.rs::cbor_round_trip` asserts byte-identical CBOR
   across two runs; closes GAP-03 from [../usecase.md](../usecase.md) §3.
2. **5 ship grammars pinned and integrated** —
   `crates/quanta-index-structural/tests/language_pin.rs::ship_set_present`
   asserts all 5 ship grammars compile and parse a fixture per language.
3. **Metavariable / variadic / `inside` / `outside` / `where` functional** —
   `crates/quanta-index-structural/tests/matcher.rs::*` covers each
   primitive against fixtures.
4. **`:[X] → $X` and `:[...ARGS] → $...ARGS` normalization done at LEX-01
   lexer stage** —
   `crates/quanta-index-contract/tests/lq_conformance.rs::uc_str_07` asserts
   parser output AST has zero `:[…]` nodes.
5. **`:[hole.type=…]` returns typed `NotImplemented`** —
   `crates/quanta-index-structural/tests/typed_hole.rs::not_implemented_gate`
   per [../feature-scope.md](../feature-scope.md) §1.3.3.
6. **`AC-07` (unbounded recursion) returns `PLAN_LIMIT_EXCEEDED`** —
   `crates/quanta-index-contract/tests/lq_conformance.rs::ac_07` per
   [../dsl.md](../dsl.md) §13.
7. **Tree-sitter parse cost p99 < 50 ms per file** —
   criterion `str_01_parse_bench` asserts the gate ([../implementation-plan.md](../implementation-plan.md) §4.6 exit).
8. **Structural query p99 < 1 s (single-repo)** — criterion
   `str_01_match_bench` per [../implementation-plan.md](../implementation-plan.md) §9.2.
9. **Cross-instance reproducibility** —
   `tests/cross_instance_structural.rs::byte_identical` runs two processes
   and diffs the CBOR envelope (RFC § Claim Discipline §8 adapted for
   structural).
10. **Per-shard generation pin locked** — `StructuralPlan` carries the
    full `generation_set` per §4.5; property test
    `crates/quanta-index-structural/tests/generation_pin.rs::no_cross_shard_skew`
    asserts no candidate ever ships with a sibling-mismatched generation.
11. **Lexical / structural disjointness** —
    `crates/quanta-index-structural/tests/disjoint_envelope.rs::no_mixing`
    asserts structural results never appear inside a
    `SearchPlaneLexicalQueryResponse`.
12. **RFC § Claim-Discipline §4 provable** — bundle citation in the structured
    agent output names `crates/quanta-index-structural/tests/matcher.rs::*`
    as the proof that "tree-sitter-backed matcher exists".
13. **No `unwrap`, `unwrap_or`, `Result::ok` regressions** on production paths
    (clippy disallowed-methods rail per [../implementation-plan.md](../implementation-plan.md) §1.4 item 4).
14. **No `#[derive(Serialize)]` / `#[derive(Deserialize)]` regressions** —
    semgrep `rust-no-serde-derive` green
    ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)).
15. **Structured agent output validates against
    [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json)** — any unprovable item
    surfaces as `blocked`, not `ok`.

## §12 Open questions

Surface remaining ambiguities; each blocks at least one DoD item or follow-on
ticket:

- **Q-STR-01-RANK** — cross-family ranking when both lexical and structural
  matches are reachable from the same user intent. STR-01 keeps result spaces
  disjoint (§4.6); a future hybrid-result ticket must address this. **Owner:
  LEX-06 follow-up + new RFC § Ranking amendment.**
- **Q-STR-01-CACHE-RECOVERY** — when the syntax cache for a file is corrupted
  (e.g. partial write after crash), do we re-parse on the fly or fail closed
  with `STATE_NOT_READY`? **Default per RFC § Failure and Recovery Model:
  fail closed** — operator action to rebuild. Confirm at impl time.
- **Q-STR-01-JAVA** — Java is cut from the Wave-5 ship gate (§4.10). When does
  it land? Default: follow-up ticket, post-Wave-5. **Owner: feature-scope
  amendment.**
- **Q-STR-01-CROSS-LANG-PATTERN** — single `match { … }` across a multi-
  language repo when no `lang:` filter is given: do we dispatch per language
  in parallel and union results, or fail closed asking for explicit
  `lang:`? **Default per [../dsl.md](../dsl.md) §8.5 step 3:** dispatch per
  matched file. Result ordering still deterministic per §4.7.
- **Q-STR-01-RAW-IN-PATTERN** — does a raw string `'…'` inside a
  `match { … }` body match literally (no tokenization) or as a structural
  literal? **Default: literal text match at the leaf-token boundary; no
  cross-token regex.** Confirm at impl time.

## §13 References

- [../rfc.md](../rfc.md) — May-23 Sourcegraph-Class Lexical Kernel RFC
  (§ Structural engine, § `LQ/Structural-1.2`, § Non-Negotiable Invariants,
  § Error Code Taxonomy, § Claim Discipline §4, § Atomicity contract,
  § Merge determinism rule).
- [../feature-scope.md](../feature-scope.md) — Feature Scope Catalog
  (§1.3 structural feature catalog, §4.3 ticket cross-reference, §6.3
  authority chain, §7 scale caps, §9 open questions Q4 / Q9).
- [../usecase.md](../usecase.md) — Usecase Catalog & Golden Corpus
  (§2.E `UC-STR-01..07`, §3 GAP-03, §4 `AC-07`, §6 conformance plan).
- [../dsl.md](../dsl.md) — DSL Formal Grammar
  (§2.3 EBNF, §8 structural sub-grammar, §10 normalization, §12 error
  taxonomy, §13 limits).
- [../implementation-plan.md](../implementation-plan.md) — Implementation Plan
  (§4.6 Wave 5, §5.12 STR-01 DoD, §6 R4 risk, §8.1 test rails, §9.1 OBS
  subset, §10 ADR-003 / ADR-005 / ADR-012).
- [../../../../CLAUDE.md](../../../../CLAUDE.md) — Agent change posture, build
  hygiene (D18 no-serde-derive).
- [../../../../AGENTS.md](../../../../AGENTS.md) — agent router.
- [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — structured agent output
  schema.
- [../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive` rule.
