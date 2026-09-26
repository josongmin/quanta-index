# STR-01 — Structural Pattern Engine

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md), and [MAY-27-002](../../../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


> Status: `partial-live[truthful-subset; broader semantics deferred]`
> Crate: `quanta-index-lq-structural`
> Tests: 71
> Last verified: 2026-05-26
> Parent RFC: [../rfc.md](../rfc.md) § Structural engine, § `LQ/Structural-1.2`
> Parent plan: [../implementation-plan.md](../implementation-plan.md) § 4.6 Wave 5, § 5.12 STR-01
> Grammar source: [../dsl.md](../dsl.md) §8 Structural sub-grammar
> Scope: [../feature-scope.md](../feature-scope.md) §1.3 `LQ/Structural-1.2`
> Conformance corpus rows: [../usecase.md](../usecase.md) §2.E `UC-STR-01..07`, §4 `AC-07`
> Authority posture: **breaking-first** per [../../../../CLAUDE.md](../../../../CLAUDE.md) § Agent change posture
> Schema check: structured outputs must validate against [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json)
>
> **Architecture correction:** tree-sitter dropped. Input shifted to producer-supplied `ParseTreeRecord` via `LexicalChannelOp::UpsertParseTree`. The initial live ship was the truthful root-only subset (`TruthfulSubsetAuthorityMatcher`: root-kind exact + single root capture) over materialized parse-tree/chunk authority. That boundary was later expanded in the shipped may-26 residue pack to cover tree-walk, variadic sibling capture, and `where` / `inside` / `outside`.

---

## §1 Purpose

Stand up the structural pattern engine for `LQ/Structural-1.2`: consume
producer-supplied `ParseTreeRecord` per chunk via the `UpsertParseTree`
channel op ([../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §3.1),
expose a typed `ParsedTree` to the search plane, and run a tree-walk
matcher over a language-anchored pattern IR with metavariable /
variadic / `inside` / `outside` / `where` primitives. The pipeline
emits the typed `StructuralCandidate` carrier that closes contract
gap GAP-03 from [../usecase.md](../usecase.md) §3.

Authority shift vs. original draft: **the search plane does not parse
source code.** Per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md)
§0 and §3.1 (Authorship rule), the producer
(`semantica-codegraph-v2`) authors every derived artifact —
`ChunkRecord`, `SymbolRecord`, `ParseTreeRecord`. There is **no
tree-sitter dependency in this crate** and **no source parsing on the
query path**. Pattern matching is search-side AST traversal over
producer-supplied trees only. Query-side DSL parsing (the `match { … }`
body in LQ) is unchanged and remains a search-plane responsibility.

### §1.1 Historical Option A/B note — now resolved

The original spec assumed search-side tree-sitter; that is wrong. The
correction originally carried two valid landing options. That decision is now
resolved for the active runtime path: producer-authored parse trees are wired,
and the initial live executor ran only the truthful root-only subset. Later
may-26 work expanded that boundary. The rest of this subsection is retained as
planning history.

- **Option A (recommended) — STR-01 in v1.** Producer ships
  `LexicalChannelOp::UpsertParseTree { chunk_id, tree: ParseTreeRecord }`
  events on the lexical channel for every chunk in the ship-grammar
  set. Search plane decodes `tree` into `ParsedTree`, caches it per
  `(repo, rev, generation, chunk_id)`, and runs the tree-walk matcher
  on query. STR-01 ships end-to-end at Wave-5 exit.
- **Option B (fallback) — STR-01 deferred to v2.** The
  `quanta-index-structural` crate ships as scaffolding only: contract
  types (`StructuralCandidate`, `StructuralBinding`,
  `ParseTreeRecord` shape), the `StructuralPattern` IR, the
  `parse_pattern` DSL parser, and the `StructuralMatcher` trait. The
  default `match_pattern` implementation returns
  a typed producer-unavailable failure until the producer side lands
  `UpsertParseTree`. Wave-5 exit gate descopes the
  end-to-end conformance rail; corpus rows `UC-STR-01..07` are
  asserted against `MockStructuralMatcher` only.

Selection driver: **producer-side cost of authoring and shipping parse
trees per chunk.** If the producer agrees and lands `UpsertParseTree`
in time, Option A. That wiring has now landed for the active runtime path.
Either way, no source parsing leaks into the search plane.

At wave end (under whichever option is selected):

1. every **supported root-only** `match { … }` body in the 5 ship-grammar set
   produces a structural candidate or typed error — never an empty success.
2. the structural engine is **disjoint** from the lexical content engine; it
   does **not** reuse Tantivy regex as a structural-search heuristic ([../rfc.md](../rfc.md) § Structural engine § must not).
3. structural matches expose `StructuralBinding` (metavariable → byte span) on
   the wire as the canonical carrier for `where` constraints and bridge
   consumers.
4. RFC § Claim-Discipline §4 is currently provable only for the truthful
   root-only subset on the active runtime path; broader semantics are deferred.

Out of scope: typed-hole eval and broader non-root tree-walk semantics (deferred
to the may-26 residue pack); structural → CodeQL bridge edge (`PLAN_DEFERRED`,
BRIDGE-01 follow-on); post-Wave-5 grammars (C / C++ / Ruby); producer-side
parse-tree authorship (lives in `semantica-codegraph-v2`).

## §2 Background

The repo has **no structural engine** ([../implementation-plan.md](../implementation-plan.md) §2.7 — absent across
contract, core, adapter, and conformance). The lexical adapter exposes
`RegexQuery` against the Tantivy `text` field; using that as structural search
is explicitly forbidden by RFC § Structural engine § must not.

Wave 5 unlocks `LQ/Structural-1.2`. STR-01's blockers:

- `LEX-06` ([../implementation-plan.md](../implementation-plan.md) §3.2) — by Wave 5 entry the lexical plane has a typed
  AST, deterministic merge, and an explainable ranker, so the structural engine
  plugs in as a sibling planner without re-litigating the front door.
- `UpsertParseTree` channel op ([../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md) §3.1)
  — now live on the active runtime path for the truthful subset. Remaining
  work is breadth expansion, not producer-path absence.

Authority anchor: per [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md)
§0 and §3.1, the producer authors every derived artifact. The
structural plane therefore consumes producer-supplied
`ParseTreeRecord` per chunk and runs an AST traversal over the
decoded `ParsedTree`. Pattern matching is search-side; source parsing
is not. The original "tree-sitter on search side" framing is
explicitly retired here. Structural and lexical **do not share a
candidate space** for `LQ/Core-1.0` (see §4.6); cross-family ranking
is deferred.

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

### §3.1 Runtime inputs

The matcher consumes **two** input streams; both are typed:

1. **User-supplied structural query** — LQ DSL `match { :[name] … }` body,
   parsed by this crate's `parse_pattern` into `StructuralPattern`. This
   is **query parsing**, not source parsing — the parser walks DSL
   tokens, not source code, and remains a search-plane responsibility.
2. **Producer-supplied parse trees** —
   `LexicalChannelOp::UpsertParseTree { repo, revision, generation,
   chunk_id, tree: ParseTreeRecord }`
   ([../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md)
   §3.1), one event per chunk. `ParseTreeRecord` is a CBOR-encoded
   tree-of-nodes shape whose authoritative wire spec is **owned by the
   producer**; this crate exposes a decoded `ParsedTree { root:
   ParseNode, lang: LangId }` (a search-side type) after consuming
   `UpsertParseTree.tree`. Producer never ships parse trees for
   languages outside the `LangId` ship set (§4.10); queries against
   chunks with absent parse trees return empty deterministically (no
   silent best-effort).

### §3.2 Authoritative input docs

1. [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md)
   §0 (authorship rule), §3.1 (`UpsertParseTree` op, proposed status).
2. [../rfc.md](../rfc.md) § Structural engine, § `LQ/Structural-1.2`, § Non-Negotiable
   Invariants 1, 2, 10, § Error Code Taxonomy.
3. [../feature-scope.md](../feature-scope.md) §1.3 (structural feature catalog), §4.3
   (ticket cross-reference), §6.3 (authority chain), §7 (scale caps).
4. [../usecase.md](../usecase.md) §2.E (`UC-STR-01..07`), §3 (`GAP-03`), §4 (`AC-07`).
5. [../dsl.md](../dsl.md) §2.3 (`LQ/Structural-1.2` EBNF), §8 (structural
   sub-grammar), §10 (normalization), §12 (error taxonomy), §13 (limits).
6. [../implementation-plan.md](../implementation-plan.md) §4.6, §5.12, §10 ADR-003,
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

- `quanta-index-contract` (for `StructuralCandidate`, `LqQueryV1`,
  `LexicalErrorCode`, `ParseTreeRecord` wire shape, `LexicalChannelOp`).
- `quanta-index-channel` (subscriber-side consumption of
  `UpsertParseTree` events; not used directly on the query path —
  the dispatcher feeds decoded `ParsedTree`s into this crate's
  in-memory store).
- workspace-pinned `regex` for `where … == "string"` constraint evaluation only
  (no RE2 NFA over file content — that lives in the lexical adapter).
- workspace-pinned `serde_cbor` (or `ciborium`) for decoding
  `ParseTreeRecord` bytes — confined to the decode adapter; the
  matcher operates on the decoded `ParsedTree` only.

Forbidden dependencies (enforced by hexagonal lint):

- **`tree-sitter`, `tree-sitter-rust`, `tree-sitter-python`,
  `tree-sitter-typescript`, `tree-sitter-javascript`, `tree-sitter-go`,
  any other `tree-sitter-*` crate.** The search plane never parses
  source. Producer authors parse trees per
  [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md)
  §0 and §3.1.
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
- `ParseTreeRecord` — **producer-owned wire spec** carried inside
  `LexicalChannelOp::UpsertParseTree.tree`
  ([../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md)
  §3.1). Lives in `quanta-index-contract::channel` because both the
  producer crate and `quanta-index-structural` depend on the same
  shape. The decoded search-side counterpart `ParsedTree { root:
  ParseNode, lang: LangId }` lives in `quanta-index-structural` and
  is **not** part of the wire surface.

Serialization: hand-rolled `impl serde::Serialize` / `impl serde::Deserialize`
per CLAUDE.md § Build hygiene; `#[derive(Serialize)]` is banned by semgrep
`rust-no-serde-derive` ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)).

### §4.3a Crate-internal traits (search side)

The matcher surface is intentionally small and operates on decoded
trees only — no source parsing:

```text
StructuralPattern              # search-side DSL IR
fn parse_pattern(src: &str) -> Result<StructuralPattern, StructuralError>
    # UNCHANGED — search-side query DSL parser, parses the
    # `match { … }` body. This is query parsing, not source parsing.

ParsedTree { root: ParseNode, lang: LangId }   # decoded from ParseTreeRecord

trait StructuralMatcher {
    fn match_pattern(
        &self,
        pattern: &StructuralPattern,
        tree: &ParsedTree,
    ) -> Result<Vec<StructuralCandidate>, StructuralError>;
}

struct MockStructuralMatcher;                  # unchanged — tests only

struct DefaultStructuralMatcher;               # walk-match impl
```

Default impl is a tree walker that pattern-matches `PatternNode` against
`ParseNode` and binds metavars to `ByteSpan` ranges in the source — the
source bytes themselves are looked up by `chunk_id` from the chunk store
when the binding needs to be rendered, but the walker itself touches only
`ParsedTree` and `StructuralPattern`.

### §4.4 Pipeline stages

Lock the pipeline so each stage is independently testable. No stage
parses source code; stage 3 decodes producer bytes into the
search-side `ParsedTree` shape and nothing more.

1. **parse structural pattern** — consume the `StructuralBlock` from LEX-01
   (aliases already desugared) → typed pattern IR per [../dsl.md](../dsl.md) §8.
   Implemented as `parse_pattern(&str) -> Result<StructuralPattern, _>`.
2. **language router** — on the current live subset, explicit `lang:` wins;
   otherwise the runtime compiles the structural block against each
   materialized producer language and applies executable `file:` filters at the
   chunk adapter. There is no dedicated empty-language-resolution code on this
   subset.
3. **decode `UpsertParseTree.tree`** — decode the producer-supplied
   CBOR `ParseTreeRecord` for each in-scope chunk into the
   search-side `ParsedTree { root: ParseNode, lang: LangId }`.
   Malformed payload → `STR_PARSE_TREE_DECODE_FAIL` (§8). Trees for
   languages outside the ship set (§4.10) are dropped silently at
   index time and queries on those chunks return empty
   deterministically (no fallback parser, no best-effort).
4. **walk-match `StructuralPattern` against `ParsedTree`** — preorder
   traversal of `ParseNode` driven by `PatternNode`; emit anchored
   match positions. The walker is purely a CBOR-tree traversal —
   no tokenizer, no parser, no grammar lookup.
5. **metavar bind** — capture `$X`, `$...X`, `...`, typed-hole placeholders
   to `ByteSpan` ranges in source; typed-hole eval gated to
   `NotImplemented`.
6. **`where` evaluator** — apply post-bind constraints
   ([../dsl.md](../dsl.md) §8.3); unbound `HoleRef` → `STR_INVALID_METAVAR`.
7. **emit candidates** — `StructuralCandidate` rows into a **separate result
   space** from lexical (§4.6).
8. **bridge into ranker** — see §4.6.

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

### §4.8 Engine choice (resolved by channel-architecture)

**No engine bake-off.** The original draft compared Comby,
stack-machine-on-RE2, and tree-sitter-native-walk as search-side
parsing engines. That comparison is **void**: per
[../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md)
§0 and §3.1, the search plane does not parse source. Producer ships
parse trees via `UpsertParseTree`. The search plane traverses.

The remaining engineering choice is "how do I walk a CBOR tree" —
trivial preorder recursion over `ParsedTree::root`. No vendor
decision is required, and no tree-sitter dependency lands.

Consequence for ADR-003: the Q4 question in
[../feature-scope.md](../feature-scope.md) §1.3.4 (per-grammar IR vs
unified IR) is **moot** for search-side: the search plane sees a
single `ParseNode` shape regardless of source language, because the
producer normalizes onto the `ParseTreeRecord` wire shape upstream.
Per-grammar lowering, if needed, is a producer-side concern. ADR-003
on the search side reduces to a one-liner: **unified `ParsedTree` /
`PatternNode` IR**.

### §4.9 Metavariable binding carrier (locked)

`StructuralBinding` is the canonical metavariable carrier on the wire and
closes GAP-03 from [../usecase.md](../usecase.md) §3. Lock per [../dsl.md](../dsl.md) §8.1:

| Hole kind | Binding shape | Carrier field |
|---|---|---|
| `$X` (single-token) | `Span` (one node range) | `bindings[X] = Span{ start, end }` |
| `$...X` (multi-token / variadic) | `Span` (n-ary span) | `bindings[X] = Span{ start, end }` covering the captured range |
| `...` (anonymous wildcard) | not captured | absent from `bindings` (cannot be referenced by `where`) |
| `:[hole.type=...]` (typed hole) | deferred semantics per [../feature-scope.md](../feature-scope.md) §1.3.3 | outside the current live executable subset; fail closed before execution |

Sourcegraph alias normalize `:[X] → $X` and `:[...ARGS] → $...ARGS` is the
parser's job (already done in LEX-01 / PRE-NORM per
[../dsl.md](../dsl.md) §10 step 6). The structural plane never sees the
alias forms — corpus row `UC-STR-07` asserts this.

### §4.10 Language matrix v1 (ship)

Per [../feature-scope.md](../feature-scope.md) §1.3.4 row table, locked
`LangId` ship set for STR-01. Note: **search-side LangId support
follows producer-side parse-tree authoring.** The search plane
accepts `ParseTreeRecord` for any `LangId` the producer ships; it
neither parses source nor falls back to a search-side parser.

| Language | `LangId` | Producer-side parse-tree shipped? | Ship? |
|---|---|---|---|
| Rust | `LangId::Rust` | yes on the active runtime path | ship |
| Python | `LangId::Python` | yes on the active runtime path | ship |
| TypeScript | `LangId::TypeScript` (TS + TSX) | yes on the active runtime path | ship |
| JavaScript | `LangId::JavaScript` | yes on the active runtime path | ship |
| Go | `LangId::Go` | yes on the active runtime path | ship |
| Java | `LangId::Java` | stretch — producer decides per [../feature-scope.md](../feature-scope.md) §1.3.4 | stretch (cut from ship gate) |
| C / C++ / Ruby | per [../feature-scope.md](../feature-scope.md) §1.3.4 | post-STR-01 | post-ship |

Divergence from [../feature-scope.md](../feature-scope.md) §1.3.4: Java stretch is
**cut from the Wave-5 exit gate** to keep the wave atomic; partial language
coverage is `NotImplemented` per language per RFC § Non-Negotiable Invariants
§7. Q9 (`feature-scope.md` §9 — sub-language structural support order) defaults
to **gate together for the 5 ship grammars**; Java rides a follow-up ticket.

Queries against chunks with no producer-shipped `ParseTreeRecord`
(e.g., a Ruby chunk on a Wave-5 ship) return empty deterministically.
Explicit `lang:<unsupported>` returns `STR_LANG_NOT_SUPPORTED` per §8.

### §4.11 Conformance gating

Wave-5 exit per [../implementation-plan.md](../implementation-plan.md) §4.6 requires:

- current live proof for the original truthful root-only subset against
  producer-shipped `UpsertParseTree` events.
- broader UC-STR semantics beyond the root-only subset are descoped from the
  active runtime claim and held for the follow-on residue pack.
- `AC-07` (unbounded structural recursion) returns `PLAN_LIMIT_EXCEEDED` per
  [../dsl.md](../dsl.md) §13 (applies to pattern IR, not source).
- **Tree-walk cost p99 < 5 ms per `ParsedTree`** (criterion
  `str_01_walk_bench`) on the producer-parse-tree path. Source-parse cost is **not**
  measured on the search side — that cost lives in the producer.
- RFC § Claim-Discipline §4 provable for the original truthful root-only
  subset.

## §5 Implementation steps (TDD)

Each step is failing-test-first: red test, then impl, then green in CI.

### §5.1 Step 1 — port traits + contract types
Failing tests: `StructuralCandidate` CBOR round-trip stable across runs;
`StructuralBindings` `BTreeMap` ordering is lexicographic;
`SearchPlaneStructuralIndexQueryPort` trait object is `Send + Sync`. Impl:
hand-rolled serde, no `#[derive]`.

### §5.2 Step 2 — language router
Failing tests over `resolve_language`: explicit `lang:Rust` → `[Rust]`;
`file:^src/.*\.py$` narrows the live chunk set; no `lang:` compiles against the
materialized producer languages present in scope; explicit `lang:Cpp` →
`STR_LANG_NOT_SUPPORTED`. Implement per [../dsl.md](../dsl.md) §8.5 ordering
without a dedicated empty-resolution code on the current live subset.

### §5.3 Step 3 — decode `UpsertParseTree.tree` into `ParsedTree`
Failing tests: a CBOR `ParseTreeRecord` fixture round-trips into
`ParsedTree { root: ParseNode, lang: LangId }` and back to bytes
byte-identically; malformed payload (truncated, bad tag, cyclic
node-refs if applicable) returns `STR_PARSE_TREE_DECODE_FAIL` with
the structured `{at_offset, reason}` payload; trees for langs outside
the §4.10 ship set are skipped silently at index time but logged at
debug. Impl: hand-rolled CBOR decoder per CLAUDE.md § Build hygiene
(no `#[derive(Deserialize)]`).

### §5.4 Step 4 — walk-match `StructuralPattern` against `ParsedTree`
Failing tests against fixture `ParseTreeRecord` payloads (decoded into
`ParsedTree` — **no source parsing in the test setup**):
`match { fn $X(...) { ... } }` binds `$X = "foo"` on a tree whose root
encodes `fn foo() {…}`; `handle($...ARGS)` binds variadic; `...`
anonymous wildcard not captured; `inside { fn handler {…} } match { unwrap() }`
scopes; `outside { fn test_$_ {…} } match { panic!(...) }` excludes.
Impl: preorder recursion over `ParseNode` driven by `PatternNode`
per [../dsl.md](../dsl.md) §8. Metavar bindings are `ByteSpan`
ranges in the original source bytes (looked up by `chunk_id` only
when the binding needs to be rendered).

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

### §5.7 Step 7 — deferred-semantics gate
Failing tests: `:[hole.type=expr]` or other broader non-root-only structural
shape on the live executor → typed invalid request before execution;
unknown type-name `:[hole.type=goblin]` parser-rejected →
`PARSE_INVALID_FILTER_VALUE{filter: "hole.type"}` per [../dsl.md](../dsl.md) §8.2.

### §5.8 Step 8 — deterministic emission order
Property test: matcher run twice → byte-identical envelope (RFC § Claim
Discipline §8). Cross-instance reproducibility: single-binary two-process CI
step asserts identical CBOR envelope. Impl: explicit preorder over
`ParsedTree::root` + §4.7 merge tuple. Because the tree itself comes
from the producer over the channel (deterministic ingress), and the
walker is fixed-order recursion, search-side platform variation
cannot perturb the result.

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

- `str_01_decode_bench` — `ParseTreeRecord` CBOR decode cost; gate
  p99 < 2 ms per record (search-side decode only — producer-side
  parse cost is the producer repo's concern).
- `str_01_walk_bench` — tree-walk match cost over a 1k-tree corpus; gate
  p99 < 1 s (Wave-5 exit SLO per [../implementation-plan.md](../implementation-plan.md) §9.2).

### §6.6 Mock policy

Per [../implementation-plan.md](../implementation-plan.md) §8.4: no mocked storage
adapters past wave entry. Fixture `ParseTreeRecord` CBOR payloads
live in `crates/quanta-index-structural/tests/fixtures/<lang>/`,
authored alongside the corresponding source fixtures so they can be
re-generated from the producer side if the wire shape drifts.
`MockStructuralMatcher` is permitted for unit-level tests and is the
historical fallback only. The active runtime and end-to-end conformance rail use
the real matcher with fixture `UpsertParseTree` events fed through
`MockSubscriber`.

## §7 Observability

Per [../implementation-plan.md](../implementation-plan.md) §9.1 Wave-5 OBS subset and
RFC § Observability Requirements:

### §7.1 OpenTelemetry spans

- `lq.structural` — root span for the structural query phase.
- `lq.structural.parse_pattern` — pattern IR construction (DSL parser).
- `lq.structural.resolve_lang` — language router.
- `lq.structural.decode_tree` — `ParseTreeRecord` → `ParsedTree`
  decode (one child span per record).
- `lq.structural.tree_walk` — per-tree match execution (one child
  span per `ParsedTree`).
- `lq.structural.where_eval` — constraint evaluation.

Each span carries `{ticket_id, wave_id, lang, chunk_id_truncated}`.

### §7.2 Metrics (closed label set)

| Metric | Unit | Labels | Cardinality cap |
|---|---|---|---|
| `structural_decode_time_ms` | ms | `{lang}` | 5 |
| `structural_match_time_ms` | ms | `{lang}` | 5 |
| `structural_candidates_emitted` | count | `{lang}` | 5 |
| `structural_pattern_node_count` | count | none | 1 |
| `structural_bound_violation` | count | `{dimension}` (node-count / depth) | 2 |
| `structural_parse_tree_decode_fail` | count | `{lang}` | 5 |

New labels require a version bump per RFC § Metric schema.

### §7.3 Audit log

Per RFC § Audit trail, the audit row already carries `canonical_query_hash`
and `error_code?` — structural-specific fields ride inside
`SearchExplanation` v2 (`engines: [structural]`).

### §7.4 SLO budgets

Wave-5 exit gate ([../implementation-plan.md](../implementation-plan.md) §9.2):

- structural p99 < 1 s (single-repo, ≤ 1000 chunks).
- `structural_decode_time_ms` p99 < 2 ms per record (search-side
  decode only — producer-side parse cost is owned by the producer
  repo).

Soft limits emit a typed early-stop signal; they do **not** silently
truncate (RFC § Non-Negotiable Invariants §10).

## §8 Error scenarios

All error scenarios surface typed `LexicalErrorCode` values per [../dsl.md](../dsl.md)
§12 and RFC § Error Code Taxonomy. New STR-01-introduced codes:

| Code | When fires | Payload | Retry |
|---|---|---|---|
| `STR_PARSE_FAIL` | structural body fails the §8.1 grammar (unbalanced `{`, malformed metavariable). Detected at LEX-01 parser but routed to STR-01 for body-specific diagnostics. Refers to **query DSL** parsing, not source parsing. | `{offset, expected, found}` | not retryable |
| `STR_PARSE_TREE_DECODE_FAIL` | malformed `UpsertParseTree.tree` CBOR payload at channel ingress (truncated frame, bad tag, structural cycle, schema-version mismatch). Routes to dispatcher; matching that chunk falls back to empty deterministic result and the dispatcher marks the chunk's parse-tree slot absent. **New code per §1.1 authority shift.** | `{chunk_id, at_offset, reason}` | not retryable (producer must re-ship) |
| `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` | historical scaffold fallback / non-production test double path where parse-tree producer wiring is absent. The active runtime no longer emits this on the supported happy path. | `{repo, rev, generation}` | wait-and-retry / non-production only |
| `STR_INVALID_METAVAR` | metavariable used in unsupported position (e.g. `$X` outside `match { … }`, unbound hole-ref in `where`) | `{position, ref}` | not retryable |
| `STR_LANG_NOT_SUPPORTED` | explicit `lang:<id>` for a `LangId` outside the §4.10 producer ship set. Producer ships parse trees for langs in the ship set only; trees for other langs are dropped at index time and queries against unknown-lang chunks return empty deterministically. Explicit `lang:` for an out-of-set language returns this typed code rather than empty. | `{lang}` | not retryable |
| `STR_INVALID_REQUEST` | live executor receives a structural shape or filter outside the current root-only + `repo:` / `file:` / `lang:` truthful subset | `{reason}` | not retryable |
| `STATE_NOT_READY: PARSE_TREE_UNBUILT` | parse-tree store for `(repo, rev, generation, chunk_id)` is absent because the channel has not yet delivered the `UpsertParseTree` event for that chunk (producer published manifest seal but the tree event lags). Renamed from `SYNTAX_CACHE_UNBUILT` per §1 authority shift — there is no search-side syntax cache. | `{repo, rev, generation, chunk_id}` | wait-and-retry |
| `PLAN_LIMIT_EXCEEDED` | structural pattern exceeds 256-node or 16-depth cap; emitted from the planner stage before the matcher runs. Applies to **pattern IR**, not parse-tree size — parse-tree size is bounded by the producer-side chunking strategy. | `{dimension, limit, observed}` | not retryable |

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
| `UpsertParseTree.tree` truncated mid-frame | `STR_PARSE_TREE_DECODE_FAIL` (new) | this spec, §1.1 |
| query against `(repo,rev,gen)` with no shipped parse trees | `STR_GENERATION_NOT_READY` | active runtime harness |

No silent fallback under any of the rows above (RFC § Non-Negotiable Invariants
§§1, 2, 8). No empty `Ok` when authority is absent — `STATE_NOT_READY` per
repo policy ([../feature-scope.md](../feature-scope.md) §1.5.2 / repo fail-closed posture).

## §9 Performance envelope

Per [../feature-scope.md](../feature-scope.md) §7 and [../implementation-plan.md](../implementation-plan.md) §9.2:

| Dimension | Target | Floor / cap | Source |
|---|---|---|---|
| structural pattern node count | n/a | 256 (hard cap) | RFC § Canonical Query Model §7 |
| structural pattern depth | n/a | 16 (hard cap) | [../feature-scope.md](../feature-scope.md) §7 |
| `ParseTreeRecord` CBOR decode time (per record) | p99 < 2 ms | n/a | Wave-5 exit gate (Option A) |
| structural query p99 (single-repo) | < 1 s | n/a | Wave-5 exit gate |
| per-query memory soft limit | 256 MiB default | 16 MiB floor | [../dsl.md](../dsl.md) §13 |
| per-query CPU soft limit | 5 s default | 250 ms floor | [../dsl.md](../dsl.md) §13 |
| `ParsedTree` in-memory store size | 1 GiB default | configurable | this spec |

Source-parse time is **not** a search-side dimension and does not
appear on this gate; it lives in the producer repo.

Criterion regression budget per [../implementation-plan.md](../implementation-plan.md)
§8.2: p99 may not increase > 5% across the wave without an ADR.

Memory accounting: decoded `ParsedTree` instances are reference-counted
and held in a bounded LRU keyed by `(repo, rev, generation, chunk_id)`.
Per-query allocations are tracked in the executor's per-query budget
(RFC § Failure model §6.5 budget pattern).

## §10 Risks

Wave-specific risk register per [../implementation-plan.md](../implementation-plan.md) §6:

| ID | Description | Prob | Impact | Early-warning | Mitigation |
|---|---|---|---|---|---|
| R-STR-01-1 | **Producer `ParseTreeRecord` wire shape drift** — producer updates the CBOR schema (renames `ParseNode` fields, changes tag values) without a coordinated search-side version bump | M | H | dispatcher sees `STR_PARSE_TREE_DECODE_FAIL` spike; conformance corpus mismatches stored fixtures | version-tag the wire shape (open question Q-STR-01-WIRE-VER, §12); fail closed with typed decode error; cross-repo CI fixture pinned both sides; never silently best-effort |
| R-STR-01-2 | NFA-like DoS via deeply-nested patterns | M | H | criterion `str_01_walk_bench` p99 jumps > 5× | 256-node + 16-depth hard caps; pre-walk pattern complexity estimate; fuzz harness on the pattern IR (matches `R4` from [../implementation-plan.md](../implementation-plan.md) §6) |
| R-STR-01-3 | broader tree-walk expansion stalls after the root-only live subset | M | M | residue ticket set stays open while only root-kind exact / single root capture ship | keep the current subset explicit, and carry STR-02/03/04 as the follow-on expansion pack |
| R-STR-01-4 | parse-tree store memory blowup on monorepo (10 GiB+ trees) | M | H | RSS > 4× baseline post-fill | LRU eviction; store-size knob; per-generation scoping |
| R-STR-01-5 | metavariable binding wire-format ambiguity (variadic vs single under same name) | L | M | property test fails on binding-name reuse | reject duplicate hole names at parser stage ([../dsl.md](../dsl.md) §8.1) |
| R-STR-01-6 | language-router / file-filter mismatch silently widens or narrows the live chunk set incorrectly | L | M | UC fixture mismatch | per-test fixture asserts router output and file-filter application; fail closed or return no matches rather than silently widening scope |
| R-STR-01-7 | deferred-semantics gate leaks (parser accepts broader semantics and the live executor silently drops them) | L | H | UC fixture covers non-root-only rows | fail-closed: live executor returns `STR_INVALID_REQUEST` for shapes outside the truthful subset; never empty `Ok` |
| R-STR-01-8 | cross-instance non-determinism (walker visits in different order on different platforms) | L | H | cross-instance CI step diffs by ≥ 1 byte | enforce explicit preorder over `ParsedTree::root` per §5.8; producer-side tree shape is already deterministic at ingress |

## §11 Definition of Done (provable)

Every item below is provable per [../implementation-plan.md](../implementation-plan.md)
§1.4 claimability rule (test name + path + assertion cited). Treat this list as
the historical scaffold ledger plus current proof points. The original live
runtime ship for this ticket was the truthful root-only subset over
producer-shipped `ParseTreeRecord`; the later may-26 residue pack expanded the
current runtime beyond that boundary.

1. ✓ shipped — **`StructuralCandidate` carrier ships** —
   `crates/quanta-index-contract/tests/structural_candidate_round_trip.rs::cbor_round_trip` asserts byte-identical CBOR
   across two runs; closes GAP-03 from [../usecase.md](../usecase.md) §3.
2. ✓ shipped — **5 ship `LangId`s recognized on `ParseTreeRecord` decode** —
   `crates/quanta-index-lq-structural/tests/lang_decode.rs::ship_set_present`
   asserts a fixture `ParseTreeRecord` for each ship `LangId` decodes
   into `ParsedTree` and runs through the matcher.
3. partial-live — **crate-local scaffold coverage exists for broader matcher
   primitives, but the active runtime remains root-only** —
   `crates/quanta-index-lq-structural/tests/matcher.rs::*` covers broader
   fixtures, while the live runtime proof remains the truthful root-only
   subset.
4. ✓ shipped — **`:[X] → $X` and `:[...ARGS] → $...ARGS` normalization done at LEX-01
   lexer stage** —
   `crates/quanta-index-contract/tests/lq_conformance.rs::uc_str_07` asserts
   parser output AST has zero `:[…]` nodes.
5. deferred from live runtime — **`:[hole.type=…]` is outside the current
   executable subset** — broader semantics live in the may-26 residue pack;
   the active runtime keeps fail-closed behavior instead of claiming support.
6. ✓ shipped — **`AC-07` (unbounded recursion) returns `PLAN_LIMIT_EXCEEDED`** —
   `crates/quanta-index-contract/tests/lq_conformance.rs::ac_07` per
   [../dsl.md](../dsl.md) §13.
7. ✓ shipped — **`ParseTreeRecord` decode cost p99 < 2 ms per record** —
   criterion `str_01_decode_bench` asserts the gate
   ([../implementation-plan.md](../implementation-plan.md) §4.6 exit).
   Source-parse cost is not a search-side DoD item.
8. ✓ shipped — **Structural query p99 < 1 s (single-repo)** — criterion
   `str_01_walk_bench` per [../implementation-plan.md](../implementation-plan.md) §9.2.
9. ✓ shipped — **Cross-instance reproducibility** —
   `tests/cross_instance_structural.rs::byte_identical` runs two processes
   and diffs the CBOR envelope (RFC § Claim Discipline §8 adapted for
   structural).
10. ✓ shipped — **Per-shard generation pin locked** — `StructuralPlan` carries the
    full `generation_set` per §4.5; property test
    `crates/quanta-index-lq-structural/tests/generation_pin.rs::no_cross_shard_skew`
    asserts no candidate ever ships with a sibling-mismatched generation.
11. ✓ shipped — **Lexical / structural disjointness** —
    `crates/quanta-index-lq-structural/tests/disjoint_envelope.rs::no_mixing`
    asserts structural results never appear inside a
    `SearchPlaneLexicalQueryResponse`.
12. partial-live — **RFC § Claim-Discipline §4 provable for the truthful
    root-only subset** — bundle citation in the structured agent output names
    `crates/quanta-index-lq-structural/tests/authority_match.rs::*` plus the
    live runtime SDK frontdoor structural proof as the current claim surface.
13. ✓ shipped — **No `unwrap`, `unwrap_or`, `Result::ok` regressions** on production paths
    (clippy disallowed-methods rail per [../implementation-plan.md](../implementation-plan.md) §1.4 item 4).
14. ✓ shipped — **No `#[derive(Serialize)]` / `#[derive(Deserialize)]` regressions** —
    semgrep `rust-no-serde-derive` green
    ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)).
15. ✓ shipped — **Structured agent output validates against
    [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json)** — any unprovable item
    surfaces as `blocked`, not `ok`.

## §12 Open questions

Surface remaining ambiguities; each blocks at least one DoD item or follow-on
ticket:

- **Q-STR-01-OPTION** — RESOLVED. The active runtime ships the producer
  parse-tree path whose initial live surface was the truthful root-only subset.
  Remaining work is breadth expansion, not Option A/B selection.
- **Q-STR-01-WIRE-VER** — `ParseTreeRecord` versioning policy.
  Options: (a) embed a `wire_version: u32` field on every record and
  reject mismatches with `STR_PARSE_TREE_DECODE_FAIL`; (b) carry the
  version on the `LexicalChannelOp::UpsertParseTree` op tag itself
  (new op tag per version, breaking-first per CLAUDE.md). Default:
  (a) for forward-compat, (b) for any incompatible change. **Owner:
  contract-crate amendment.**
- **Q-STR-01-RANK** — cross-family ranking when both lexical and structural
  matches are reachable from the same user intent. STR-01 keeps result spaces
  disjoint (§4.6); a future hybrid-result ticket must address this. **Owner:
  LEX-06 follow-up + new RFC § Ranking amendment.**
- **Q-STR-01-TREE-RECOVERY** — when a stored `ParsedTree` is
  evicted by LRU under memory pressure and re-requested mid-query, do
  we re-decode the cached `ParseTreeRecord` bytes (which we still
  have) or fail closed with `STATE_NOT_READY: PARSE_TREE_UNBUILT`?
  **Default: re-decode** — the producer-shipped bytes are the
  authority and decode is bounded. Confirm at impl time.
- **Q-STR-01-JAVA** — Java is cut from the Wave-5 ship gate (§4.10). When does
  it land? Default: follow-up ticket, post-Wave-5. **Owner: feature-scope
  amendment.**
- **Q-STR-01-CROSS-LANG-PATTERN** — single `match { … }` across a multi-
  language repo when no `lang:` filter is given: do we dispatch per language
  in parallel and union results, or fail closed asking for explicit
  `lang:`? **Default per [../dsl.md](../dsl.md) §8.5 step 3:** dispatch per
  matched chunk. Result ordering still deterministic per §4.7.
- **Q-STR-01-RAW-IN-PATTERN** — does a raw string `'…'` inside a
  `match { … }` body match literally (no tokenization) or as a structural
  literal? **Default: literal text match at the leaf-token boundary; no
  cross-token regex.** Confirm at impl time.

## §13 References

- [../../../../docs/ssot/channel-architecture.md](../../../../docs/ssot/channel-architecture.md)
  — **Canonical SSOT, authoritative input source for STR-01.** §0
  (scope, search plane consumes producer-supplied data only), §3.1
  (`LexicalChannelOp::UpsertParseTree` op definition, authorship
  rule, proposed-status note). The active runtime now consumes this
  producer-authored parse-tree path for the truthful subset; remaining work is
  breadth expansion on top of that authority.
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
- [../../../ssot/producer-handoff.md](../../../ssot/producer-handoff.md) — producer handoff SSOT (authoritative `ParseTreeRecord` wire shape; Option A entry gate; delta-handling identity / cascade / replay contract in §3.5, including `DeleteParseTree` independent-of-`DeleteChunk` semantics).
- [INDEX.md](INDEX.md) — ticket index (architecture correction context: §3.6 producer-authorship correction, §3.7 ambiguities surfaced, §3.8 stale-references follow-up).
