# May-23 Sourcegraph-Class Lexical Kernel RFC

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md), and [MAY-27-002](../../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


Status: `Planning packet`

> **Architecture correction (2026-05-25)**: the original framing of LEX-05 / LEX-07 / STR-01 / RT-01 implied search-side parsing, git access, and a separate `apply_changes` IPC. That framing was corrected: the search plane decodes producer-authored records over the channel, never authors them. See [tickets/INDEX.md § 3.6](tickets/INDEX.md) for the full correction table and [docs/ssot/producer-handoff.md](../../ssot/producer-handoff.md) for the ratified op catalogue.

## Goal

이 RFC 의 목표는 현재 search stack 을 `source-bound anchored facade` 에서
`Sourcegraph-compatible lexical kernel` 로 재구성하고, 그 위에
`history / structural / runtime-aware / semantic-bridge` capability 를
층별로 올리는 것이다.

최종 목표:

1. `Sourcegraph query ⊂ LQ query`
2. lexical core 는 global, incremental, deterministic 해야 한다
3. semantic/CodeQL 은 lexical candidate authority 위에서만 동작해야 한다
4. parser 와 planner 는 typed AST 를 SSOT 로 가져야 한다
5. filter semantics, index addressing, revision selection, execution mode 는 helper string 이 아니라 explicit type 로 소유되어야 한다

### Operational definitions

These terms are load-bearing across the RFC and must be read literally, not idiomatically.

1. **global** = every `(repo, default-branch, file)` tuple registered in the catalog at query time. Subsetting is performed via `repo:` / `rev:` / `file:` filters. This is **not** "every commit ever" — non-default branches and historical revisions require explicit `rev:` scoping.
2. **incremental** = file-delta granularity at minimum. Adding or removing a single file changes ≤ `O(changed-chunks)` data in any sibling authority, and **never** `O(corpus)`. Bootstrap and recovery paths are explicitly excluded.
3. **deterministic** =
   - (a) the same query against the same `(repo, rev, generation)` yields a **byte-identical canonical result envelope** across instances and binary versions of the same major release;
   - (b) merge order is **totally ordered** (see §Execution Model merge determinism rule).
4. **`Sourcegraph query ⊂ LQ query`** = **parse subset** + **semantic equivalence** for in-scope features per [`feature-scope.md`](feature-scope.md). Out-of-scope Sourcegraph features are explicitly enumerated in the parity delta and are **not** silently degraded.

## Architectural Position

이 packet 의 기본 입장은 `Tantivy-only engine` 이 아니라
`typed lexical planner + Tantivy recall backend + IR exactifier/rerank` 다.

정규 shape:

1. query AST / planner 가 lane 을 선택한다
2. Tantivy chunk/path/symbol index 가 cheap recall 후보를 반환한다
3. HIR / item-index / semantic index 가 candidate 를 exactify 한다
4. reference/import/call/owner relation expansion 과 deterministic rerank/explain 은 post-pass 로 수행한다

즉 lexical kernel 의 global recall 은 Tantivy 를 써도 되지만,
public query semantics 와 final result truth 는 Tantivy query grammar 나 raw hit row 가 아니라
typed AST + IR authority 가 소유한다.

## Design Baseline

기준선은 Sourcegraph lexical query language 다.

Anchor reference: Sourcegraph release tag and Zoekt commit hash to be recorded in [`feature-scope.md`](feature-scope.md) § Sourcegraph compatibility delta. Conformance corpus lives in [`usecase.md`](usecase.md).

핵심 baseline:

1. keyword / phrase / regex
2. boolean `AND / OR / NOT`
3. `repo:` `file:` `lang:` `rev:` `type:` `select:` `count:` `case:` `fork:` `archived:`
4. `content:` `visibility:` `patterntype:` `context:` `boost:` `index:` `timeout:`
5. predicate filters: `repo:has.file(...)`, `repo:has.commit.after(...)`, `file:contains(...)`
6. `repo:<pattern>@rev` sugar
7. lexical-first global repo search

Detailed grammar and per-filter semantics live in [`dsl.md`](dsl.md). Per-feature in/out-of-scope status lives in [`feature-scope.md`](feature-scope.md).

참고:

1. https://sourcegraph.com/docs/code-search/queries
2. https://sourcegraph.com/docs/code_search/reference/queries
3. https://sourcegraph.com/docs/code-search/working/search_filters

## Current Source Truth

### 1. Current product search is not a global lexical engine

1. app route still requires explicit `file` or `path` anchor in product path.
2. request owner is still a bag-of-fields surface, not a typed query AST.
3. lexical parser is still helper-driven string lowering, not a Sourcegraph-class grammar owner.

### 2. Current lexical substrate is partial but useful

1. Tantivy chunk index already has long-lived writer ownership.
2. per-file replace/delete primitives already exist.
3. reader reload + revision bump substrate already exists.
4. symbol regex exists, but current implementation is still O(N) scan/filter.

### 3. Structural/history/runtime-aware/bridge families are not product-complete

1. existing structural DSL is chunk-lane specific, not a top-level product query language.
2. `commit/diff/history` is not a first-class search kind today.
3. runtime-aware filters such as `changed:` or `affected:` do not yet have one canonical metadata/catalog owner.
4. CodeQL bridge is not a typed lexical execution directive today.

### 4. Semantic and lexical are not yet fully rebased

1. semantic/hybrid still carry post-filter residue.
2. lexical filter universe is not yet the one canonical dense candidate authority.
3. local semantic mutation truth is not yet a real incremental derivative model everywhere.

## Non-Goals

1. lexical layer does not perform callgraph, dataflow, taint, PTA, or semantic reasoning
2. lexical layer does not silently widen query semantics with fuzzy defaults
3. lexical layer does not absorb GQLang execution semantics
4. lexical layer does not mix planner directives and analysis payloads into one untyped request bag

## LQ Family

이 RFC 는 `LQ/1.0` 하나가 아니라 `LQ family` 를 정의한다.

### `LQ/Core-1.0`

Sourcegraph-compatible lexical core:

1. keyword
2. exact phrase
3. raw string
4. regex
5. boolean/group
6. repo/file/lang/rev/type/select/count/case/fork/archived filters
7. `repo:<pattern>@rev` sugar
8. typed AST + parser + printer + planner

**`patterntype` default pin**: `LQ/Core-1.0` defaults `patterntype` to `standard`, matching the current Sourcegraph default. The full mode matrix (`literal`, `regexp`, `structural`, `keyword`, `standard`) lives in [`dsl.md`](dsl.md) § `patterntype:` mode matrix.

**Regex dialect**: RE2. Lookbehind, backreferences, and possessive quantifiers are forbidden at parse time and surface as `PARSE_INVALID_REGEX`.

### `LQ/History-1.1`

history / commit / diff search:

1. `type:commit`
2. `type:diff`
3. `author:` `committer:` `message:`
4. `before:` `after:` `since:` `until:`
5. `diff.added:` `diff.removed:` `diff.touched:`

### `LQ/Structural-1.2`

Semgrep-style structural search:

1. `match { ... }`
2. `$X`
3. `$...X`
4. `...`
5. `where`
6. `inside`
7. `outside`
8. Sourcegraph alias normalize: `:[X] -> $X`, `:[...ARGS] -> $...ARGS`

### `LQ/Runtime-1.3`

runtime-aware and incremental metadata filters:

1. `changed:`
2. `dirty:`
3. `stale:`
4. `affected:`
5. `invalidated_by:`
6. `snapshot:`
7. metadata namespaced filters such as `meta.owner:`, `meta.service:`, `meta.layer:`, `meta.surface:`

### `LQ/Bridge-1.4`

lexical candidate bridge:

1. `into:codeql`
2. `scope:results`
3. `with:lexical`

## Canonical Query Model

`LQ/Core-1.0` canonical owner must be a typed AST.

```rust
pub struct LqQueryV1 {
    pub expr: LqBoolExprV1,
    pub filters: Vec<LqFilterV1>,
    pub options: LqOptionSetV1,
    pub directives: Vec<LqDirectiveV1>,
}
```

rules:

1. `expr` owns only pattern/boolean semantics
2. `filters` own scope/result constraints
3. `options` own execution knobs such as `count`, `case`, `timeout`
4. `directives` own execution sinks such as `into:codeql`
5. raw strings remain only as leaf token payloads, never as canonical semi-parsed requests
6. canonical form is **idempotent under normalization**: `normalize(normalize(q)) == normalize(q)`. Detailed normalization rules live in [`dsl.md`](dsl.md) § Normalization rules.
7. **bounded inputs** — every canonical AST must satisfy the following budgets (configurable per deployment, defaults listed):
   - max raw query length: `16 KiB`
   - max AST depth: `32`
   - max boolean fan-out (operand count under one node): `64`
   - max regex NFA state count (RE2 budget): `100_000`
   - max structural pattern node count: `256`
   - budget overrun fails closed with `PARSE_OVERSIZED` or `EXEC_REGEX_COMPILE_EXPLOSION`.
8. **canonical identity** — the canonical AST emits a stable CBOR encoding plus a SHA-256 hash (`canonical_query_hash`). The hash is the canonical query identity used for cache keys, audit log correlation, and result dedup.

## Canonical Grammar Semantics

### Pattern semantics

1. bare word means lexical keyword, not fuzzy-by-default
2. `"..."` means exact phrase
3. `'...'` means raw string with minimal escaping
4. `/.../` means regex pattern
5. fuzzy or approximate search must be an explicit future operator, not default keyword behavior

### Boolean semantics

1. adjacency means `AND`
2. precedence is `NOT > AND > OR`
3. `-foo` is `NOT foo`

### Filter semantics

1. Sourcegraph-compatible filters are parser-first and planner-owned
2. `file:` is the canonical file/path content-scope token
3. `path:` may exist as compatibility alias, but canonical normalized AST should carry one path-scope representation
4. `repo:<pattern>@rev` is supported as sugar and normalized into `repo:<pattern> rev:<rev>`

### `@` policy

1. generic `@repo`, `@lang`, `@file` shorthand is forbidden in core grammar
2. only `repo:<pattern>@rev` sugar is allowed in `LQ/Core-1.0`
3. any broader `@` UX shorthand must remain an editor/UI alias layer, not canonical grammar

## Engine Decomposition

`LQ family` is not one engine. It is one grammar + multiple planners/executors.

### 1. Lexical content engine

recommended owner:

1. Tantivy chunk index as primary recall backend
2. sibling path index
3. sibling symbol index

responsibility:

1. content matching
2. path filtering
3. top-N candidate materialization
4. repo/rev/generation addressed read path

must not:

1. become the final authority for definition/reference/import/call truth
2. become the canonical owner of Sourcegraph/LQ public grammar semantics
3. absorb structural/semantic/graph reasoning that already belongs to IR-backed exactifier lanes

follow-up authority:

1. HIR / item-index / semantic index exactifies lexical candidates
2. graph/relation lanes rerank and explain final results

### 2. History engine

recommended owner:

1. commit metadata index
2. diff hunk content index
3. revision catalog

responsibility:

1. commit message / author / committer search
2. diff-added / removed / touched content search
3. date and revision range restriction

must not:

1. shell out to ad-hoc git scans per user query in product path

### 3. Structural engine

recommended owner:

1. tree-sitter-backed AST cache
2. language-normalized pattern IR
3. structural matcher runtime

responsibility:

1. Semgrep-style structural pattern matching
2. metavariable capture
3. context operators and constraints

must not:

1. pretend Tantivy regex is structural search

### 4. Runtime metadata engine

recommended owner:

1. snapshot catalog
2. invalidation catalog
3. ownership/service/layer metadata registry

responsibility:

1. `changed:`, `dirty:`, `stale:`, `affected:`, `invalidated_by:`
2. `meta.owner:`, `meta.service:`, `meta.layer:`, `meta.surface:`
3. repo/snapshot/head/base scoped metadata filtering

### 5. Bridge engine

recommended owner:

1. lexical candidate export packet
2. CodeQL invocation/request builder
3. result-scope carrier

responsibility:

1. turn lexical candidate set into downstream CodeQL input
2. preserve repo/rev/file/symbol candidate identity
3. avoid embedding semantic reasoning into lexical planner

## Recommended Concrete Engine Choices

이 RFC 는 capability family 를 분리하지만 구현 substrate 는 가능한 한 재사용한다.

### Lexical content/path/symbol

1. primary content engine: existing Tantivy chunk index
2. path filtering/result projection: Tantivy-backed path authority adjacent to content index
3. symbol search/result projection: existing Tantivy symbol index
4. one catalog must map `(repo, rev, generation)` to the three sibling authorities

### History and diff

1. commit metadata index: document set keyed by commit id
2. diff hunk index: document set keyed by `(repo, rev-range, file, hunk-id)`
3. ingestion source: git object/diff walk at indexing time, not request time

### Structural

1. syntax parse substrate: tree-sitter per language
2. stored authority: per-file syntax cache + normalized pattern-match IR
3. match execution: syntax-node walk / capture matcher

### Runtime-aware metadata

1. source of truth: `LexicalChannelOp::UpsertDirty` / `EvictDirty` events plus the search-side in-memory snapshot catalog (per [channel-architecture.md §5.2](../../ssot/channel-architecture.md), op catalogue at [channel-architecture.md §3.1](../../ssot/channel-architecture.md); delta contract at [producer-handoff.md §3.5](../../ssot/producer-handoff.md); producer-authorship rule per [INDEX.md §3.6](tickets/INDEX.md))
2. representation: namespaced metadata tables keyed by canonical doc identity

### Bridge

1. bridge payload is not raw query text
2. bridge payload is a typed lexical candidate packet with repo/rev/generation provenance

## Canonical Read Pipeline

query execution should follow one planner-owned pipeline:

1. parse raw query into `LqQueryV1`
2. normalize query into canonical AST form
3. resolve repo/rev/snapshot scope in catalog
4. choose engine family
5. build bounded execution plan
6. execute repo/shard fanout
7. merge deterministically
8. optionally emit lexical candidate packet into bridge

### 6.5 Failure model

Step 6 (fanout) is the dominant source of operational failure modes. They are enumerated here and **all** surface as typed error codes per §Error Code Taxonomy. No mode silently degrades the response shape.

1. **catalog miss** — the requested `(repo, rev)` or sibling generation is absent from the catalog at scope-resolve time. Returns `STATE_NOT_READY: CATALOG_MISS`. Never silent, never substituted with a "best matching" generation.
2. **shard timeout** — an executor shard exceeds the per-shard budget. Returns `EXEC_SHARD_TIMEOUT`. Fail-closed: a **partial result is not silently surfaced**. Partial-result opt-in is reserved for a future `partial:allow` directive (see [`dsl.md`](dsl.md) for surface).
3. **cancellation** — the cancellation budget propagates via tokio cancel signal end-to-end. In-flight regex evaluation and structural matching must be **cooperative-cancellable at fixed checkpoints** (e.g., per N candidate documents). Returns `EXEC_MERGE_CANCEL`.
4. **per-tenant fanout caps** — each tenant has a configured maximum concurrent shard fanout and a bounded **admission queue**. Overflow returns `PLAN_LIMIT_EXCEEDED`; admission timeout returns `EXEC_SHARD_UNAVAILABLE`.

## Canonical Incremental Write Pipeline

> **NOTE (2026-05-25 producer-authorship correction)**: this section was originally drafted assuming search-side authorship of incremental write derivations. Per [INDEX.md §3.6](tickets/INDEX.md) the model is inverted — producer authors all delta events; search plane consumes via channel ops. Terminology updated accordingly.

steady-state indexing should follow one mutation pipeline:

1. receive producer-authored channel events via `BundleChannelSubscriber::next_event`. Producer owns delta authorship per [producer-handoff.md §3.5](../../ssot/producer-handoff.md); search plane decodes and indexes. Op catalogue + Authorship rule lock: [channel-architecture.md §3.1](../../ssot/channel-architecture.md). Producer-authorship correction: [INDEX.md §3.6](tickets/INDEX.md).
2. derive changed chunk set
3. update lexical content/path/symbol authorities for only affected docs
4. update syntax cache for only affected files
5. update metadata/invalidation catalog for only affected identities
6. if revision advanced, update commit metadata index and diff hunk index
7. apply producer-emitted `SemanticChannelOp::UpsertEmbedding` / `DeleteEmbedding` events to the active-generation HNSW shard. Cross-cascade: `DeleteChunk` removes the matching embedding per [producer-handoff.md §3.5.2](../../ssot/producer-handoff.md). Op catalogue: [channel-architecture.md §3.1](../../ssot/channel-architecture.md).
8. publish one manifest generation set that binds all siblings together

### Atomicity contract

The write pipeline is **manifest-first**.

1. **Linearization point** — the manifest write is the single linearization point for a generation set. Sibling writes (content, symbol, path, structural cache, history catalog, semantic derivative) are **retried** until the manifest's published `generation_set` matches each sibling's `MARKER_OK`.
2. **Failure surface** — if a sibling fails to converge to `MARKER_OK` after the manifest publishes, the manifest entry is transitioned to `state='failed'` and is **not visible to the query layer**. Readers observing this state surface `STATE_NOT_READY: STALE_SIBLING`.
3. **Storage-layer enforcement** — the storage layer asserts the invariant: **no read can observe a `manifest_generation` whose sibling `MARKER_OK` is absent**. This is enforced both as a lint at CI time (static check on adapter code paths) and a runtime check at read time.

### Forbidden steady-state operations (with detection)

1. per-query rebuild
2. request-time git history scan
3. silent full-corpus rebuild in steady state

**Detection** — each `MaterializeUseCase` invocation records a **write-packet trace** with the per-invocation set of mutated `(repo, rev, file, chunk)` identities. CI fixtures and runtime observability inspect the trace to detect:
- per-query rebuild (write-packet trace size correlated 1:1 with read traffic), and
- full-corpus rebuild (write-packet trace touching `O(corpus)` identities outside bootstrap mode).
Detection surfaces a `BRIDGE_SINK_REJECTED` (CI) or a runtime alarm (observability).

## Planner Model

`LqPlannerV1` must route query families explicitly.

1. `type:file|path|symbol` -> lexical planner
2. `type:commit|diff` -> history planner
3. `match { ... }` -> structural planner
4. `into:codeql` -> bridge directive after lexical candidate materialization
5. `def:`, `ref:`, `export:`, `import:` -> cross-ref planner family, not plain lexical parser trick

**Default planner** — when `type:` is absent, the planner routes to the **lexical content engine**. This default is documented and is **not** a heuristic: any change requires a version bump per §Migration and Versioning Policy.

unsupported combinations:

1. must fail with typed planner error
2. must not silently degrade to a different engine

### Unsupported-combo table

| Combination                | Code                       | Reason                                                          |
| -------------------------- | -------------------------- | --------------------------------------------------------------- |
| `type:commit` + `match{}`  | `PLAN_UNSUPPORTED_COMBO`   | history domain has no structural projection                     |
| `type:diff` + `into:codeql`| `PLAN_UNSUPPORTED_COMBO`   | diff hunks have no CodeQL projection                            |
| `match{}` + `into:codeql`  | `PLAN_DEFERRED`            | structural → CodeQL bridge is Wave 6 (`BRIDGE-01` follow-on)    |

This table is the **complete** unsupported-combo set for `LQ/Core-1.0`. New combinations may only be added by minor version bump.

## Incremental Update Model

incremental indexing must be explicit and generation-addressed.

### Canonical write packet

one file change should produce a packet like:

1. repo identity
2. revision/snapshot identity
3. changed file set
4. changed chunk set
5. lexical content mutations
6. path/symbol metadata mutations
7. structural cache invalidation
8. history/diff catalog mutation if revision changes

### Generation model

separate but linked generations:

1. manifest generation
2. content shard generation
3. symbol shard generation
4. path shard generation
5. structural cache generation
6. history catalog generation
7. semantic derivative generation

rules:

1. full rebuild is bootstrap/recovery only
2. steady-state writes are file/chunk delta scoped
3. readers must know which generation set they searched
4. stale generation mismatch must fail closed

### Monotonicity rules

These rules make the generation lattice unambiguous for both writers and readers.

1. `manifest_generation` is **strictly increasing** per `(repo, rev)`. No reuse, no rollback under any code path.
2. Each sibling generation (`content`, `symbol`, `path`, `structural`, `history`, `semantic`) is **non-decreasing** per `(repo, rev)`. A sibling may be `NULL` (not yet built) but **must never transition from non-NULL to NULL**.
3. **Stale** is defined as: sibling generation `<` manifest's referenced sibling generation.
4. Staleness fails closed with `STATE_NOT_READY: STALE_SIBLING`. Stale data is **never** served as a degraded result.

### Semantic derivative model

semantic is downstream derivative, not a co-equal mutable authority.

1. changed chunk set drives semantic upsert/delete
2. lexical doc identity and semantic doc identity must align
3. local semantic backend may claim incremental mutation only if delta proof rails pass
4. otherwise it must be explicitly downgraded to bootstrap/read-mostly mode

## Execution Model

`SearchExecutorV1` must be planner-owned and deterministic.

1. repo fanout
2. shard fanout
3. bounded concurrency
4. timeout budget
5. cancellation
6. deterministic merge
7. explicit `count:all`
8. explicit early-stop mode

must emit metrics:

1. repos scanned
2. shards scanned
3. bytes touched
4. early-stop reason
5. merge time
6. ranking time

### Merge determinism rule

The merge stage uses one **totally ordered** tuple to combine shard results. The shape of the tuple is **engine-family specific**; the rules below define the canonical tuple per family. Every component is total within its domain, so each composite is total — ties cannot exist. This makes the merge **bit-exact reproducible** across runs, instances, and binary versions of the same major release.

#### Baseline merge tuple (catalog / path / non-ranked lanes)

```
merge order = (score DESC, repo_id ASC, manifest_generation ASC, candidate_id ASC)
```

This 4-component tuple applies to lanes that do not pass through the LEX-06 composite ranker (e.g. raw recall debug paths, catalog-only filters).

#### Lexical-ranked merge tuple (LEX-06 lane) — canonical for `type:file|path|symbol`

**Amendment closes `RFC-GAP-LEX-06-1` per [tickets/INDEX.md § 3.1](tickets/INDEX.md).** The LEX-06 ranker requires intra-`(repo, generation)` tie resolution that the 4-component tuple cannot provide; the canonical tuple is therefore 6 components:

```
merge order (lexical) = (
  score DESC,
  repo_id ASC,
  manifest_generation ASC,
  repo_relative_path ASC,
  start_line ASC,
  doc_id ASC
)
```

Each component is total. `repo_relative_path` and `start_line` resolve ties between distinct candidates inside the same `(repo, generation)` (multiple hits in the same file, multiple files in the same repo). Cross-link: [tickets/LEX-06.md § 3.3](tickets/LEX-06.md).

#### Hybrid-fused merge tuple (SEM-02 lane) — canonical for `hybrid(lex, sem, …)` directive

**Amendment closes `RFC-GAP-SEM-02-TUPLE` per [tickets/INDEX.md § 3.1](tickets/INDEX.md).** Hybrid fusion combines lexical and semantic engines; the fused stream needs additional components to break ties between candidates that one engine ranked but the other did not. The canonical tuple is 8 components:

```
merge order (hybrid) = (
  fused_score DESC,
  lex_score DESC NULL_LAST,
  sem_score DESC NULL_LAST,
  repo_id ASC,
  manifest_generation ASC,
  repo_relative_path ASC,
  start_line ASC,
  candidate_id ASC
)
```

`NULL_LAST` on `lex_score` / `sem_score` handles the case where a candidate appears only in one engine's top-k. `manifest_generation` refers to the pinned per-engine generation set captured at query start. Cross-link: [tickets/SEM-02.md § 4.5](tickets/SEM-02.md).

#### Tuple selection rule

The planner selects the tuple based on the active engine family for the resolved query:

1. `hybrid(...)` directive present → 8-component hybrid tuple
2. else, ranker active (default for `type:file|path|symbol`) → 6-component lexical tuple
3. else (raw recall, debug lanes) → 4-component baseline tuple

No tuple is dynamically extended; selection is one-shot at plan time.

### Metric schema

Each metric emitted at this layer must declare:

1. `unit` (e.g., `count`, `bytes`, `milliseconds`)
2. allowed `label` keys (closed set; new labels require version bump)
3. cardinality budget (e.g., labels × value-range cap)

The detailed metric schema, including span attribute names and label cardinality caps, lives in [`implementation-plan.md`](implementation-plan.md) § Observability.

## Compatibility Rules

### hard compatibility goal

`Sourcegraph query ⊂ LQ/Core-1.0`

### compatibility policy

1. existing Sourcegraph lexical queries should parse without source rewrite
2. Sourcegraph structural metavariable aliases may normalize into Semgrep-style captures
3. compatibility aliases must normalize into one canonical AST form before planning

### normalization examples

1. `foo bar` -> `foo AND bar`
2. `repo:core@main` -> `repo:core rev:main`
3. `:[X]` -> `$X`
4. `:[...ARGS]` -> `$...ARGS`

### Conformance corpus ownership

The conformance corpus and its golden encoding format are owned by [`usecase.md`](usecase.md). The CI gate `ci/lq-conformance` runs the corpus on every PR. Drift from the Sourcegraph reference release tag is **reported, not auto-accepted** — accepting drift requires an explicit RFC update naming the new reference tag.

## Non-Negotiable Invariants

1. no generic `@` core grammar
2. no fuzzy-by-default keyword semantics
3. no query-owned source-bound fallback in product path
4. no hidden filter/ranking semantics in helper string functions
5. no semantic correctness claim from post-filter-only output dropping
6. no incremental claim without delta mutation proof
7. no history/structural/bridge claim from parser-only support
8. **no untyped error response** — every failure path ships a typed `code` per [`dsl.md`](dsl.md) § Error taxonomy and §Error Code Taxonomy below. Free-text-only error responses are forbidden.
9. **no cross-tenant data leakage at merge time** — the merge stage enforces tenant scope; cross-tenant candidate ids are an assertion failure, not a degraded result.
10. **no unbounded memory in regex/structural matching** — RE2 NFA state cap, structural pattern node cap, and a query memory soft cap are enforced at compile and execution time.
11. **no scope-widening at planner time** — the planner may **narrow** filters (e.g., propagate `repo:` into shard selection) but **never widen** them. A widened filter is a typed planner failure.
12. **no schema drift without version bump** — moving from `LQ/Core-1.0` to `LQ/Core-1.1` requires an explicit `lq_version` field in the canonical AST and a documented producer/consumer skew window.
13. **no producer-side metadata trust** — every catalog write is validated at write time against the contract crate's typed schema. Producer-provided metadata is treated as input, not authority.

## Ticket Pack

1. `LEX-00` baseline and invariants freeze
2. `LEX-01` canonical query AST and parser
3. `LEX-02` global front door and surface contract cutover
4. `LEX-03` lexical authority unification
5. `LEX-04` incremental lexical indexing kernel
6. `LEX-05` parallel executor and deterministic merge
7. `LEX-06` ranking, explain, and lexical semantics
8. `LEX-07` history and diff search engine
9. `STR-01` structural pattern engine
10. `RT-01` runtime-aware metadata filters
11. `SEM-01` semantic on lexical filter pushdown
12. `SEM-02` incremental semantic derivatives
13. `BRIDGE-01` CodeQL bridge and candidate export
14. `OBS-01` conformance, fences, and final proof

### Architecture-corrected scope notes (2026-05-25)

The following ticket scopes were corrected post-RFC under the producer-authorship rule (see [tickets/INDEX.md § 3.6](tickets/INDEX.md) for the full mistake/correction table and [docs/ssot/producer-handoff.md](../../ssot/producer-handoff.md) for the op catalogue):

- **`LEX-05`** — was authored as "tree-sitter on the search side". Corrected: a `SymbolRecordDecoder` that decodes producer-emitted `UpsertSymbol.symbol` payloads (`SymbolRecord`). No tree-sitter dependency on the search plane.
- **`LEX-07`** — was authored as "search-plane git walk / self-authored commit graph". Corrected: `UpsertCommit` / `UpsertRef` / `UpsertTag` ops drive a channel-subscriber `CommitGraph`. No request-time git access.
- **`STR-01`** — was authored as "tree-sitter on the search side". Corrected: structural matcher traverses producer-emitted `ParseTreeRecord` payloads from `UpsertParseTree`. The original live ship was the truthful root-only subset; breadth expansion later landed in the follow-on may-26 residue pack.
- **`RT-01`** — was authored with a separate `apply_changes` IPC framing (violates §11 producer-authorship rule). Corrected: `UpsertDirty` / `EvictDirty` ops over the same channel; `DirtyBuffer.apply` / `.evict` are channel-subscriber callbacks. The advisory-lock ADR is dropped — channel monotonic seq replaces it.

## Canonical Execution Waves

1. Wave 1: `LEX-00`, `LEX-01`
2. Wave 2: `LEX-02`, `LEX-03`
3. Wave 3: `LEX-04`, `LEX-05`
4. Wave 4: `LEX-06`, `LEX-07`
5. Wave 5: `STR-01`, `RT-01`
6. Wave 6: `SEM-01`, `BRIDGE-01`
7. Wave 7: `SEM-02`
8. Wave 8: `OBS-01`

## Claim Discipline

1. `Sourcegraph-compatible lexical core` is not claimable before parser conformance + front-door parity + global unanchored execution proof
2. `incremental indexing` is not claimable before file-delta write/read generation proof
3. `history search` is not claimable before commit/diff indexed execution exists
4. `structural search` is not claimable before tree-sitter-backed matcher exists
5. `runtime-aware filters` are not claimable before metadata catalog truth exists
6. `CodeQL bridge` is not claimable before typed candidate export exists
7. `semantic rebased on lexical` is not claimable before lexical-universe planning replaces post-filter correctness
8. `deterministic` is not claimable before the **cross-instance reproducibility test** is green (same query against same `(repo, rev, generation)` on two instances → byte-identical envelope).
9. `global` is not claimable before the **N-repo fanout proof** passes; `N` is defined in [`feature-scope.md`](feature-scope.md) § Scale.
10. `ranked correctly` is not claimable before a **golden IR-evaluation set** is wired: precision@10, MAP, and NDCG vs. the reference release are measured and checked in CI.

## Canonical Read Order

1. [`feature-scope.md`](feature-scope.md) — per-feature in/out-of-scope status, Sourcegraph reference anchor, scale targets
2. [`usecase.md`](usecase.md) — conformance corpus (golden files), use-case taxonomy
3. [`dsl.md`](dsl.md) — full grammar, filter semantics, `patterntype:` mode matrix, normalization rules, error taxonomy
4. [`implementation-plan.md`](implementation-plan.md) — execution plan, observability/telemetry schema
5. tickets to be authored against the implementation plan

## Error Code Taxonomy

This RFC owns the **shape** and **family grouping** of every typed error code. The exact wire-format payload schema per code is owned by [`dsl.md`](dsl.md) § Error taxonomy. All failure paths in `LQ/Core-1.0` must map to one of these codes — there is no untyped error response.

### `PARSE_*` — parse and grammar failures

| Code                         | When fires                                                                   | Payload                                  | Retry semantics |
| ---------------------------- | ---------------------------------------------------------------------------- | ---------------------------------------- | --------------- |
| `PARSE_LEX_ERROR`            | tokenizer cannot make progress                                               | `{offset, expected}`                     | not retryable   |
| `PARSE_SYNTAX_ERROR`         | parser cannot match a grammar production                                     | `{offset, expected, found}`              | not retryable   |
| `PARSE_INVALID_REGEX`        | regex pattern violates RE2 dialect (lookbehind, backref, possessive)         | `{offset, dialect_violation}`            | not retryable   |
| `PARSE_INVALID_UTF8`         | input is not valid UTF-8                                                     | `{byte_offset}`                          | not retryable   |
| `PARSE_OVERSIZED`            | query exceeds size, depth, fan-out, or NFA-state budget                      | `{budget, observed}`                     | not retryable   |
| `PARSE_FORBIDDEN_SYNTAX`     | LEX-04 / PRE-NORM — parse-time-detectable forbidden construct (generic `@` outside `repo:<pat>@rev` sugar, lookbehind / lookahead / backref / possessive regex group, inline flag groups outside leading `(?i)`) | `{offset, construct}`                    | not retryable   |
| `PARSE_UNKNOWN_FILTER`       | filter name not in the registered filter set                                 | `{filter_name}`                          | not retryable   |
| `PARSE_INVALID_FILTER_VALUE` | filter value fails its value-grammar                                         | `{filter_name, value, expected_grammar}` | not retryable   |
| `PARSE_INVALID_PATTERNTYPE`  | `patterntype:` value outside the mode matrix                                 | `{value, allowed}`                       | not retryable   |
| `PARSE_UNSUPPORTED_COMBO`    | parse-time-detectable combination forbidden in `LQ/Core-1.0`                 | `{constructs}`                           | not retryable   |

### `PLAN_*` — planner failures

| Code                          | When fires                                                              | Payload                          | Retry semantics |
| ----------------------------- | ----------------------------------------------------------------------- | -------------------------------- | --------------- |
| `PLAN_UNKNOWN_PREDICATE`      | predicate filter name not in the registered predicate set               | `{predicate}`                    | not retryable   |
| `PLAN_LIMIT_EXCEEDED`         | post-plan budget exceeded (e.g., resolved-repo fan-out, admission queue) | `{limit_kind, budget, observed}` | wait-and-retry  |
| `PLAN_UNSUPPORTED_COMBO`      | semantic combination forbidden (see §Planner Model table)               | `{combo}`                        | not retryable   |
| `PLAN_DEFERRED`               | combination is on the roadmap but not active (e.g., `match{}` → CodeQL) | `{combo, wave}`                  | not retryable   |

### `EXEC_*` — execution failures

| Code                              | When fires                                                  | Payload                            | Retry semantics |
| --------------------------------- | ----------------------------------------------------------- | ---------------------------------- | --------------- |
| `EXEC_CATALOG_MISS`               | resolved generation absent at execution time                | `{repo, rev, generation_kind}`     | wait-and-retry  |
| `EXEC_SHARD_TIMEOUT`              | shard exceeded per-shard timeout                            | `{shard_id, budget_ms, elapsed_ms}`| retryable       |
| `EXEC_SHARD_UNAVAILABLE`          | shard unreachable, admission timeout, or marked unhealthy   | `{shard_id, reason}`               | retryable       |
| `EXEC_MERGE_CANCEL`               | merge stage cancelled by upstream cancel signal             | `{at_checkpoint}`                  | retryable       |
| `EXEC_REGEX_COMPILE_EXPLOSION`    | RE2 NFA exceeded state budget at compile time               | `{budget, observed}`               | not retryable   |
| `RANK_INVALID_SIGNAL`             | LEX-06 ranker — NaN / infinity / out-of-domain / missing signal in `RawCandidate` | `{signal, observed, expected_domain?}` | not retryable   |

### `STATE_*` — readiness / generation failures

| Code                          | When fires                                                          | Payload                                          | Retry semantics |
| ----------------------------- | ------------------------------------------------------------------- | ------------------------------------------------ | --------------- |
| `STATE_NOT_READY`             | catalog entry exists but not ready (e.g., still building)           | `{repo, rev, kind, reason}`                      | wait-and-retry  |
| `STATE_STALE_SIBLING`         | sibling generation lags manifest's referenced sibling generation    | `{sibling, manifest_gen, sibling_gen}`           | wait-and-retry  |
| `STATE_GENERATION_REGRESSION` | observed monotonicity violation (non-NULL → NULL, or rollback)      | `{kind, prev, observed}`                         | not retryable   |

### `AUTHZ_*` — authorization failures

| Code              | When fires                                                       | Payload                          | Retry semantics |
| ----------------- | ---------------------------------------------------------------- | -------------------------------- | --------------- |
| `AUTHZ_TENANT_DENY` | request tenant scope rejected by ACL                           | `{tenant_id}`                    | not retryable   |
| `AUTHZ_ACL_MISS`    | repo ACL not yet propagated to query layer                     | `{repo_id}`                      | wait-and-retry  |

### `BRIDGE_*` — bridge / sink failures

Canonical bridge error set locked per [tickets/BRIDGE-01.md § 8.1](tickets/BRIDGE-01.md) (closes `FS-GAP-2` per [tickets/INDEX.md § 3.4](tickets/INDEX.md)). The five Sourcegraph-bridge codes (`BRIDGE_UNSUPPORTED_FILTER`, `BRIDGE_UNSUPPORTED_DIRECTIVE`, `BRIDGE_AMBIGUOUS_FILTER`, plus `BRIDGE_VERSION_PIN`, `BRIDGE_TRANSLATE_FAIL`) added here as a single canonical set; `feature-scope.md § 1.5.2` is updated to point to this table.

| Code                         | When fires                                                          | Payload                          | Retry semantics |
| ---------------------------- | ------------------------------------------------------------------- | -------------------------------- | --------------- |
| `BRIDGE_SINK_REJECTED`       | downstream sink (e.g., CodeQL) refused the candidate packet         | `{sink, reason}`                 | not retryable   |
| `BRIDGE_CANDIDATE_FORMAT_INVALID` | candidate packet failed contract-crate validation              | `{field, expected, observed}`    | not retryable   |
| `BRIDGE_UNSUPPORTED_FILTER`  | BRIDGE-01 — Sourcegraph filter name has no LQ projection            | `{filter_name, source_offset, reason}` | not retryable |
| `BRIDGE_UNSUPPORTED_DIRECTIVE` | BRIDGE-01 — Sourcegraph directive refused (`index:no`, fuzzy `~`, generic `@`, empty input) | `{construct, source_offset, reason}` | not retryable |
| `BRIDGE_AMBIGUOUS_FILTER`    | BRIDGE-01 — Sourcegraph filter resolves to ≥ 2 LQ targets (defensive fail-closed) | `{filter_name, candidates}` | not retryable |
| `BRIDGE_VERSION_PIN`         | BRIDGE-01 — producer's translator version disagrees with consumer's expected skew window per § Migration and Versioning Policy | `{producer_version, consumer_window}` | not retryable |
| `BRIDGE_TRANSLATE_FAIL`      | BRIDGE-01 — Sourcegraph syntax was parser-accepted by SG-side but the translator produced no LQ AST (defensive — catches translator-internal bugs separate from `UNSUPPORTED_*`) | `{source_offset, reason}` | not retryable |

### `HISTORY_*` — history / commit-graph failures (LEX-07)

Closes the LEX-07 group per [tickets/INDEX.md § 3.1](tickets/INDEX.md). Scope corrected post-RFC (producer-authorship rule) — the search plane decodes `UpsertCommit` / `UpsertRef` / `UpsertTag` records; it does **not** walk git. Cross-link: [tickets/LEX-07.md](tickets/LEX-07.md), [docs/ssot/producer-handoff.md § 3.1](../../ssot/producer-handoff.md).

| Code                            | When fires                                                                        | Payload                            | Retry semantics |
| ------------------------------- | --------------------------------------------------------------------------------- | ---------------------------------- | --------------- |
| `HISTORY_REF_NOT_FOUND`         | `rev:` / `tag:` / `parent:` filter references a ref/tag/commit absent from the producer-emitted catalog | `{ref}`                            | not retryable   |
| `HISTORY_RANGE_OVERRUN`         | `revisions:` range exceeds the `HISTORY_REVISIONS_MAX` cap (10,000 commits)       | `{requested, cap}`                 | not retryable   |
| `HISTORY_MERGE_CYCLE`           | commit graph contains a cycle (cannot happen in valid git; indicates producer corruption) | `{at_commit}`                | not retryable   |
| `HISTORY_TRACE_INCOMPLETE`      | parent traversal aborted because a parent commit was not yet decoded from the channel | `{at_commit, missing_parent}`  | wait-and-retry  |
| `HISTORY_UNINDEXED`             | `type:commit` / `type:diff` query against a `(repo, rev)` whose history catalog is absent (steady-state path; no request-time git scan) | `{repo, rev}` | wait-and-retry  |
| `HISTORY_COMMIT_DECODE_FAIL`    | `UpsertCommit` payload failed contract validation at apply time                   | `{op_seq, field, reason}`          | not retryable   |
| `HISTORY_REF_DECODE_FAIL`       | `UpsertRef` / `UpsertTag` payload failed contract validation                      | `{op_seq, field, reason}`          | not retryable   |
| `HISTORY_COMMIT_PARENT_UNKNOWN` | producer emitted a commit whose parent commit-id is not yet known on the channel; topological-emission rule violation per [docs/ssot/producer-handoff.md § 3.1.2](../../ssot/producer-handoff.md) | `{commit_id, parent_id}` | not retryable   |

### `STR_*` — structural matcher failures (STR-01)

Closes the STR-01 group per [tickets/INDEX.md § 3.1](tickets/INDEX.md). Scope corrected post-RFC (producer-authorship rule) — the search plane traverses producer-emitted `ParseTreeRecord` payloads; it does not run tree-sitter on source bytes. Cross-link: [tickets/STR-01.md](tickets/STR-01.md), [docs/ssot/producer-handoff.md § 3.3](../../ssot/producer-handoff.md).

| Code                                | When fires                                                                  | Payload                            | Retry semantics |
| ----------------------------------- | --------------------------------------------------------------------------- | ---------------------------------- | --------------- |
| `STR_PARSE_FAIL`                    | structural pattern in `match { ... }` body fails the structural-pattern grammar | `{offset, expected}`           | not retryable   |
| `STR_INVALID_METAVAR`               | metavariable form is malformed (`$`, `$...`, `$X`, `$...X` exhaustive) or used outside a `match` body | `{offset, form}`              | not retryable   |
| `STR_LANG_NOT_SUPPORTED`            | structural query targets a language outside the v1 ship set (Rust/Python/TypeScript/JavaScript/Go) | `{lang}`                       | not retryable   |
| `STR_PARSE_TREE_DECODE_FAIL`        | `UpsertParseTree` payload failed contract validation at apply time          | `{op_seq, field, reason}`          | not retryable   |
| `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` | structural query target has no `ParseTreeRecord` from the producer on scaffold / non-production paths; the active runtime no longer uses this on the supported happy path | `{repo, file}` | wait-and-retry  |

Current live runtime truth remains narrower than the full STR-01 design
surface: the originally shipped root-only subset has since been expanded by the
may-26 residue pack to cover tree-walk, variadic sibling capture, and
`where` / `inside` / `outside`, while unsupported structural composition still
fails closed as `STR_INVALID_REQUEST`. The active runtime no longer uses
`STR_PRODUCER_PARSE_TREE_UNAVAILABLE` on the supported happy path.

### `DIRTY_*` — runtime metadata / `dirty:` channel failures (RT-01)

Closes the RT-01 group per [tickets/INDEX.md § 3.1](tickets/INDEX.md). Scope corrected post-RFC (producer-authorship rule) — `dirty:` truth comes from producer-emitted `UpsertDirty` / `EvictDirty` channel ops; the separate `apply_changes` IPC framing in the original spec is dropped. Cross-link: [tickets/RT-01.md](tickets/RT-01.md), [docs/ssot/producer-handoff.md § 3.2](../../ssot/producer-handoff.md).

| Code                       | When fires                                                                        | Payload                            | Retry semantics |
| -------------------------- | --------------------------------------------------------------------------------- | ---------------------------------- | --------------- |
| `DIRTY_STALE_GEN`          | `UpsertDirty` references a `(repo, rev, generation)` older than the active manifest's generation | `{op_seq, op_gen, active_gen}` | not retryable   |
| `DIRTY_BUFFER_FULL`        | per-tenant per-repo dirty-buffer cap (10,000 entries default) hit; further `UpsertDirty` ops refused until evict | `{tenant_id, repo_id, cap}` | wait-and-retry  |
| `DIRTY_TTL_EXPIRED`        | dirty entry exceeded TTL (300 s default) without an evict and was reaped         | `{doc_id, age_ms, ttl_ms}`         | not retryable   |
| `DIRTY_BAD_IDENTITY`       | `UpsertDirty` / `EvictDirty` carries a doc identity that cannot be resolved against the manifest | `{doc_id, reason}`           | not retryable   |
| `DIRTY_PAYLOAD_DECODE_FAIL`| `UpsertDirty` / `EvictDirty` payload failed contract validation at apply time     | `{op_seq, field, reason}`          | not retryable   |

### `SEM_*` — semantic / ANN adapter failures (SEM-01)

Closes the SEM-01 group per [tickets/INDEX.md § 3.1](tickets/INDEX.md). Cross-link: [tickets/SEM-01.md](tickets/SEM-01.md).

| Code                          | When fires                                                                        | Payload                            | Retry semantics |
| ----------------------------- | --------------------------------------------------------------------------------- | ---------------------------------- | --------------- |
| `SEM_DIM_MISMATCH`            | embedding vector dimensionality differs from the per-generation `embedding_dim` pin | `{expected, observed}`             | not retryable   |
| `SEM_NOT_READY`               | semantic ANN index for the resolved generation is not yet built                   | `{repo, rev, generation}`          | wait-and-retry  |
| `SEM_INVALID_VECTOR`          | vector contains NaN / infinity / zero-norm (cosine metric requires non-zero norm) | `{slot, reason}`                   | not retryable   |
| `SEM_METRIC_UNSUPPORTED`      | query requests a metric (L2 / Dot) outside the v1 cosine pin                      | `{metric}`                         | not retryable   |
| `SEM_ANN_NONDETERMINISTIC`    | ANN backend returned non-reproducible results across two probes with the same seed (determinism contract violation) | `{seed, drift}`            | not retryable   |
| `SEM_HNSW_PARAMS_INVALID`     | HNSW per-generation parameters (`M`, `efConstruction`, `efSearch`) outside the deployment's allowed range | `{param, value, allowed}`     | not retryable   |

### `HYB_*` — hybrid fusion failures (SEM-02)

Closes the SEM-02 group per [tickets/INDEX.md § 3.1](tickets/INDEX.md). Cross-link: [tickets/SEM-02.md](tickets/SEM-02.md).

| Code                          | When fires                                                                        | Payload                            | Retry semantics |
| ----------------------------- | --------------------------------------------------------------------------------- | ---------------------------------- | --------------- |
| `HYB_INVALID_WEIGHTS`         | `hybrid(...)` weights fail validation (negative, sum-out-of-range, non-finite)    | `{weights, reason}`                | not retryable   |
| `HYB_GEN_MISMATCH`            | lexical and semantic sub-queries resolved to inconsistent generation sets         | `{lex_gen, sem_gen}`               | not retryable   |
| `HYB_PUSHDOWN_INCOMPLETE`     | filter pushdown into one engine but not the other detected at plan time (would skew fusion); fail-closed per § Non-Negotiable Invariants item 5 | `{filter, missing_side}` | not retryable   |
| `HYB_TOP_K_INVALID`           | requested `top_k` for fusion is outside `[1, count_cap]`                          | `{top_k, cap}`                     | not retryable   |
| `HYB_STRATEGY_UNSUPPORTED`    | `strategy=<x>` not in the registered strategy set (v1: `rrf`, `weighted`)         | `{strategy, allowed}`              | not retryable   |
| `HYB_SUBQUERY_INVALID`        | lexical or semantic sub-query failed its own grammar (forwarded from the sub-engine's typed error) | `{side, inner_code}`     | not retryable   |

### `SYMBOL_*` — symbol record / decoder failures (LEX-05)

Closes the LEX-05 group per [tickets/INDEX.md § 3.1](tickets/INDEX.md). Scope corrected post-RFC (producer-authorship rule) — the search plane decodes producer-emitted `SymbolRecord` payloads from `UpsertSymbol`; it does not run tree-sitter on source bytes. Cross-link: [tickets/LEX-05.md](tickets/LEX-05.md), [docs/ssot/producer-handoff.md § 3.4](../../ssot/producer-handoff.md).

| Code                          | When fires                                                                        | Payload                            | Retry semantics |
| ----------------------------- | --------------------------------------------------------------------------------- | ---------------------------------- | --------------- |
| `SYMBOL_PAYLOAD_DECODE_FAIL`  | `UpsertSymbol` payload failed contract validation at apply time                   | `{op_seq, field, reason}`          | not retryable   |
| `SYMBOL_RECORD_INVALID`       | `SymbolRecord` violates field invariants (empty `name`, malformed `span`, `lang` not in `LangId` variants, etc.) | `{field, reason}`              | not retryable   |

### `OBS_*` — observability / audit guards (OBS-01)

Closes the OBS-01 group per [tickets/INDEX.md § 3.1](tickets/INDEX.md). Cross-link: [tickets/OBS-01.md](tickets/OBS-01.md).

| Code                          | When fires                                                                        | Payload                            | Retry semantics |
| ----------------------------- | --------------------------------------------------------------------------------- | ---------------------------------- | --------------- |
| `OBS_CARDINALITY_GUARD`       | a metric or span label set exceeds the declared cardinality budget (4-layer defense) | `{metric, label, observed, budget}` | not retryable |
| `OBS_INVALID_SPAN`            | span attribute set violates the §Observability Requirements schema (missing required attr, unknown label key) | `{span, attribute, reason}` | not retryable   |
| `OBS_INVALID_METRIC`          | metric emission violates the §Execution Model § Metric schema (unit mismatch, unknown label key)               | `{metric, attribute, reason}` | not retryable   |
| `OBS_AUDIT_MISSING_FIELD`     | audit-log entry missing a required field per §Security and Authz Model § Audit trail | `{field}`                         | not retryable   |

## Security and Authz Model

This layer is not the user-authentication boundary — producer/proxy upstream owns user identity verification. This layer **owns enforcement** of the resulting tenant/user scope.

1. **Repo permission filter** — every query carries a `tenant_id` and `user_id`. The planner injects the ACL filter as the **first AND clause** of the canonical AST, before any other planner pass. This is a planner invariant, not a runtime opt-in.
2. **Multi-tenant isolation** — index namespaces are partitioned per tenant. The merge stage asserts that no candidate id resolved from one tenant's namespace can appear in another tenant's result envelope. Violation is a hard assertion failure, not a degraded result.
3. **Audit trail** — every query (success or failure) is logged once with `(tenant_id, user_id, canonical_query_hash, generation_set, latency_ms, result_count, error_code?)`. The audit sink is separate from the operational log sink.
4. **Auth boundary disclaimer** — this layer does not perform credential verification, session validation, or OAuth flow handling. Inbound identity is treated as authenticated; the upstream producer/proxy is the authoritative auth boundary.

## Capacity and SLO Targets

Initial proposed numbers — operators tune per deployment. All numbers are **per-instance** unless noted.

### Capacity targets

| Dimension              | Target        |
| ---------------------- | ------------- |
| repos                  | 100,000       |
| branches per repo      | 1,000         |
| files per branch       | 1,000,000     |
| chunks per file        | 1,000         |

### Latency SLOs

| Workload                          | p50      | p95      | p99      |
| --------------------------------- | -------- | -------- | -------- |
| single-repo lexical query         | < 50 ms  | < 250 ms | < 1 s    |
| 100-repo fanout query             | —        | < 2 s    | —        |

### Throughput SLOs

| Dimension                              | Target                |
| -------------------------------------- | --------------------- |
| index build throughput                 | ≥ 10,000 chunks/sec/node |
| conformance corpus pass rate           | 100% (no drift allowed) |

## Observability Requirements

1. **OpenTelemetry spans** — every request emits the following span tree:
   - `lq.parse`
   - `lq.normalize`
   - `lq.plan`
   - `lq.exec.fanout`
   - `lq.exec.shard` (one per shard)
   - `lq.merge`
   - `lq.bridge` (only when `into:` directive is present)
2. **Structured logs** — every request emits exactly one log line at completion containing `canonical_query_hash` and the metric set declared in §Execution Model.
3. **Audit log** — separate sink for compliance; payload defined in §Security and Authz Model.
4. **Metric schema** — span attribute names, label sets, and cardinality budgets live in [`implementation-plan.md`](implementation-plan.md) § Telemetry.

## Failure and Recovery Model

1. **Index corruption** — detected at segment commit time via `MARKER_OK` + per-segment checksum. Corruption transitions the affected generation to `STATE_NOT_READY`; **never silent**.
2. **Crash mid-write** — `.building/` directories are left in place by the writer. A bootstrap-time sweep validates each `.building/` directory against the control-plane catalog. The sweep is **fail-closed**: it does **not** auto-delete ambiguous state; operator action is required.
3. **Partial shard failure** — query fails closed with `EXEC_SHARD_UNAVAILABLE`. A future `partial:allow` directive may opt in to partial-result responses; until then, partial results are not surfaced.
4. **Dual-write split-brain** — a writer registry in the control plane brokers ownership. A second writer for the same `(repo, rev)` must acquire an advisory lock; lock-acquisition failure causes the second writer to fail closed. The lock entry includes a writer identity and lease expiry for crash recovery.

## Migration and Versioning Policy

1. **`lq_version` field** — the canonical AST carries an explicit `lq_version` field (e.g., `"1.0"`). Producers and consumers may only negotiate compatible versions.
2. **Minor version semantics** — `LQ/Core-1.0` → `LQ/Core-1.1` may **add** capabilities. It may **not** remove or repurpose existing constructs. Removal requires a major version bump.
3. **Producer/consumer skew window** — at most **one minor version** of skew is supported. Wider skew is a deployment error and surfaces as a contract-crate validation failure.
4. **Contract crate breaking changes** — require a coordinated producer + consumer release in the same major-version bump window.
5. **Deprecation** — deprecated constructs survive **2 minor versions** before removal. A deprecation warning is surfaced in the response envelope's `info` field, never in the error path.

## Conformance Suite Reference

1. **Corpus location** — the corpus lives next to [`usecase.md`](usecase.md). Proposed layout: `usecase-corpus/*.toml` (TOML or YAML golden files; final choice owned by [`usecase.md`](usecase.md)).
2. **CI gate** — `ci/lq-conformance` parses every corpus entry, normalizes it to the canonical AST, executes it against a deterministic test fixture, and asserts both result-shape and error-code expectations.
3. **Sourcegraph parity** — each corpus entry is tagged with the Sourcegraph release ref it tracks. Drift triggers an explicit RFC update; the gate does **not** auto-accept the new reference.

## Index Lifecycle

1. **Retention** — keep `N` most-recent generations per `(repo, rev)`. `N` is configurable per tenant. Older generations are eligible for vacuum but not auto-deleted while they are referenced by an active reader.
2. **Compaction** — idle-window background segment merge; compaction must **never** run during a write storm. Compaction respects the manifest-first atomicity contract.
3. **Vacuum** — orphaned `.building/` directories and unreferenced generation directories are purged on the bootstrap sweep, gated by the fail-closed rule in §Failure and Recovery Model.
4. **Repair** — corruption detected alongside missing `MARKER_OK` triggers a **single-generation rebuild** from the prepared bundle if one is available; otherwise the affected generation surfaces as `STATE_NOT_READY` until operator action.

## API Stability Policy

1. **Stable surfaces** — `LqQueryV1` and `LqResponseV1` are stable across patch versions. Their wire format and field set do not change without a minor version bump.
2. **Breaking changes** — require a minor version bump plus the skew window in §Migration and Versioning Policy.
3. **Non-stable surfaces** — planner internals, executor internals, and sibling adapter shapes are explicitly **not stable** and may change in any minor version.
4. **Deprecation timeline** — deprecated stable surfaces survive **2 minor versions** before removal, mirroring §Migration and Versioning Policy.
