# LEX-03 — Phrase position index for adjacency / phrase queries

| Field | Value |
| --- | --- |
| Ticket ID | `LEX-03` |
| Title | Phrase position index for adjacency / phrase queries |
| Wave | 2 (per [rfc.md § Canonical Execution Waves](../rfc.md)) |
| Parent RFC section | [rfc.md § LQ Family `LQ/Core-1.0`](../rfc.md), [rfc.md § Pattern semantics](../rfc.md), [rfc.md § Ticket Pack](../rfc.md) |
| Sibling planning docs | [feature-scope.md](../feature-scope.md), [usecase.md](../usecase.md), [dsl.md](../dsl.md), [implementation-plan.md](../implementation-plan.md) |
| Owner crate(s) | `quanta-index-lexical`, `quanta-index-core`, `quanta-index-control` |
| Touches contract crate? | No (sibling shard is internal; no `LqQuery` / `LexicalCandidate` shape change) |
| Posture | Breaking-first; no long-lived shims (per [CLAUDE.md § Agent change posture](../../../../CLAUDE.md)) |
| Claim discipline anchor | [rfc.md § Claim Discipline §1](../rfc.md) (parser conformance + global execution proof leg for phrase / adjacency) |

---

## §1 Purpose

Build a per-generation **phrase position index** (per-term per-doc position
lists, post-normalize) that accelerates two query paths:

1. `"…"` exact phrase leaves ([dsl.md §3.2](../dsl.md)), required by
   `patterntype:standard` (default) and `patterntype:literal` modes.
2. Adjacency / proximity queries: `keyword` mode `AdjacencyLink` annotation
   ([dsl.md §5.3](../dsl.md)) requires bounded-window position comparison;
   LEX-06 BM25 + proximity boost also reads positions.

Shard stores, per `(term, doc)`, the *post-normalize* position list (§10).
Planner fetches per-term postings, runs **position-window intersect** to
confirm phrase adjacency. Result = candidate-doc + match-span set; no
separate verify pass — positions are truth for adjacency.

This ticket lands storage layout, build path, position-window intersect,
fail-closed `PLAN_LIMIT_EXCEEDED` semantics, and the criterion bench.
Does **not** land BM25 + proximity *scoring* (LEX-06) — but surfaces the
positions LEX-06 consumes.

---

## §2 Background

### 2.1 Current state

[`crates/quanta-index-lexical/src/lib.rs`](../../../../crates/quanta-index-lexical/src/lib.rs)
returns `CoreError::NotImplemented` for build and open. Per
[implementation-plan.md §2.4](../implementation-plan.md) the LEX-03 row
("phrase positions") is **partial** — "Tantivy default schema records
positions for `text`; not exposed through port". The schema choice was
implicit (`en_stem`, predecessor D4) and no port exposes positions today.

### 2.2 Why now

[`dsl.md §3.2`](../dsl.md): "mapping: Tantivy `PhraseQuery` with slop = 0".
[`dsl.md §5.3`](../dsl.md): "In `keyword` mode … `AdjacencyLink`
annotation … (BM25 + proximity boost). Default window: `8 tokens`."

Wave 2 cannot exit (per [implementation-plan.md §4.3](../implementation-plan.md))
without phrase/adjacency authority — `UC-LEX-03` ("async fn handle"),
`UC-LEX-02` (`tokio runtime`) are in Wave-2 scope. The user-supplied GOAL
labels these as UC-LEX-11/UC-LEX-12; current numbering binds the same
semantic anchors at different row ids — see §6.2.

### 2.3 Anti-scope

- **No scoring.** BM25 + proximity boost is LEX-06; this ticket emits
  positions so LEX-06 has them.
- **No multi-line / cross-chunk phrase.** Chunk id = doc id; positions are
  per-chunk. Cross-chunk phrase returns empty (documented in §10).
- **No incremental write path.** Chunk-grain delta is LEX-04.

### 2.4 Authority chain

Per [feature-scope.md §6.1](../feature-scope.md), producer publishes chunk
rows. This ticket consumes them via the LEX-00 normalizer and projects to
`(term, doc, position_list)`.

---

## §3 Inputs

### 3.1 From upstream sources

- chunk rows for `(repo, rev, generation)` via
  [`LexicalChannelOp`](../../../../crates/quanta-index-contract/src/channel)
- `(repo, rev, generation)` tuple from control-plane catalog (subject to
  **G-CONTROL-LOC** in [implementation-plan.md §2.3a](../implementation-plan.md))
- LEX-00 normalized term stream — positions are **post-normalize** (§10.1)

### 3.2 From sibling tickets

- LEX-00 baseline + normalizer pipeline green
- LEX-01 canonical AST + parser — `PatternLeaf::Phrase` + `AdjacencyLink`
  annotation exist on the AST (PRE-CONTRACT-EXT)
- LEX-02 trigram shard — orthogonal sibling; shares `manifest_generation`
  per [rfc.md § Atomicity contract](../rfc.md)

### 3.3 Carriers and types

- `LqExpr::PatternLeaf(PatternLeaf::Phrase(PhraseLeaf { tokens: Vec<NormalizedToken> }))`
- internal `PhrasePositionShard` (private)
- internal port `LexicalIndexPositionOpenPort::open_positions(...)` —
  **not** a contract surface

---

## §4 Deliverables

### 4.1 Storage layout (per-generation, immutable)

```
{state_root}/indexes/lexical/{repo_id}/{rev_id}/{generation}/positions/
├── terms.fst         # Finite-state transducer: term -> (offset, doc_count, total_positions)
├── postings.varint   # delta-varint posting lists per term: (doc_id, position_list)
├── meta.cbor         # tokenizer_id, normalizer_version, doc_count, total_postings, codec_version
└── MARKER_OK         # per-sibling readiness sentinel (per RFC § Atomicity contract)
```

- `terms.fst` is the sorted distinct-term set; payload is a fixed-stride
  `(postings_offset, doc_count, total_positions)` triple.
- `postings.varint` layout per term:
  ```
  varint(doc_count)
  for each doc:
      varint(doc_id - prev_doc_id)
      varint(position_count)
      varint(p_1)
      varint(p_2 - p_1)
      …
      varint(p_n - p_{n-1})
  ```
  Delta-varint on both doc gaps and intra-doc position gaps.
- `meta.cbor` is CBOR-encoded ([rfc.md § Migration and Versioning Policy](../rfc.md)
  bytes-determinism rules apply), hand-rolled per-D18 (no
  `#[derive(serde::Serialize)]` — see
  [`tools/ci/semgrep/rules.yml:124`](../../../../tools/ci/semgrep/rules.yml#L124)).
- `MARKER_OK` is written **last**, after `fsync` on every file above.

### 4.2 Per-deployment knobs

| Knob | Default | Floor | Ceiling | Source |
| --- | --- | --- | --- | --- |
| Adjacency window (tokens) | `8` | `1` | `64` | [dsl.md §5.3](../dsl.md) |
| Phrase length cap (tokens) | `64` | `2` | `512` | this ticket; see §9.3 |
| Position list cap per (term, doc) | `4_096` | `64` | `65_536` | this ticket |
| Position list cap per term across docs | `33_554_432` (`2^25`) | `1_048_576` | `134_217_728` | this ticket |
| Adjacency scan depth (max docs scanned for position intersect) | `100_000` | `1_000` | `1_000_000` | this ticket; aligned with RFC NFA budget |

Exceeding any cap → `PLAN_LIMIT_EXCEEDED` per [dsl.md §12](../dsl.md);
no silent degradation.

### 4.3 Public ports (internal to lexical adapter)

- `LexicalIndexPositionBuildPort::build_positions(repo, rev, gen, ops) -> Result<(), CoreError>`
- `LexicalIndexPositionOpenPort::open_positions(repo, rev, gen) -> Result<Box<dyn PositionSearcher>, CoreError>`
- `PositionSearcher::phrase_match(query: &PhraseSearchQuery) -> Result<PhraseCandidateSet, CoreError>`
- `PositionSearcher::adjacency_score_input(query: &AdjacencyQuery) -> Result<AdjacencyEvidence, CoreError>`

These are **not** contract types. They are
[`quanta-index-core::domains::lexical`](../../../../crates/quanta-index-core/src/domains)
port traits. Adapter implementation lives in
[`crates/quanta-index-lexical/src/`](../../../../crates/quanta-index-lexical/src/lib.rs).

### 4.4 Planner wiring

- `LqPlanner::plan` routes `LqExpr::PatternLeaf(PatternLeaf::Phrase(_))`
  through the position-shard `phrase_match` path; the result candidate
  carries match-span data so LEX-06's rerank step can consume it directly.
- For `AdjacencyLink` annotations (keyword mode), the planner calls
  `adjacency_score_input` to fetch a typed evidence packet for LEX-06.
  In Wave 2 (this ticket) the evidence is *emitted* but not yet *scored* —
  LEX-06 wires it into BM25 + proximity boost.

### 4.5 Stopword policy (locked)

**No stopword filter.** Positions are stored for every token the LEX-00
normalizer emits.

Rationale:

1. Sourcegraph parity ([feature-scope.md §1.1.1](../feature-scope.md)):
   "exact phrase leaf — `"..."` → phrase match (token sequence)". A phrase
   like `"the quick brown fox"` must hit literally; dropping `the` would
   degrade the user-visible match shape.
2. RFC § Non-Negotiable Invariants §2 (no fuzzy-by-default) — a stopword
   filter is a hidden semantic that widens matches; forbidden.
3. The cost (storage growth from indexing common terms) is bounded by the
   per-term posting cap in §4.2.

If a deployment later wants stopword filtering, it lands as an explicit
opt-in carrier on the normalizer (LEX-00), not as a hidden behavior here.

### 4.6 Metrics (per [rfc.md § Execution Model § Metric schema](../rfc.md))

| Metric | Unit | Label set |
| --- | --- | --- |
| `lex.position.phrase_match_count` | count | `{ticket_id="LEX-03", wave_id="2", outcome="ok"|"empty_set"|"plan_limit_exceeded"}` |
| `lex.position.phrase_window_scan_docs` | count | `{ticket_id="LEX-03"}` |
| `lex.position.adjacency_evidence_emitted` | count | `{ticket_id="LEX-03"}` |
| `lex.position.position_bytes_read` | bytes | `{ticket_id="LEX-03"}` |
| `lex.position.intersect_ms` | milliseconds | `{ticket_id="LEX-03"}` |
| `lex.position.build_ms` | milliseconds | `{ticket_id="LEX-03", phase="full"|"delta"}` |

Cardinality budget: `6 metrics × ≤4 labels × ≤4 values = 96` per
[rfc.md § Observability Requirements](../rfc.md). Closed label set; new
labels require version bump.

---

## §5 Implementation steps (TDD)

Failing-test-first; test names are stable and serve as §11 DoD evidence.

### 5.1 Step 1 — `meta.cbor` codec

Hand-rolled `impl Serialize / Deserialize for PhraseShardMeta` (D18).
Carries `tokenizer_id` + `normalizer_version` so a shard built against a
different normalizer is rejected at read time. Test:
`crates/quanta-index-lexical/src/positions/meta.rs::tests::roundtrip_canonical_cbor`.

### 5.2 Step 2 — Nested varint posting codec

Doc-list outside, position-list inside; both delta-varint. Tests:
`…/postings.rs::tests::nested_varint_roundtrip_property` (1k proptest
shapes); `…::tests::nested_varint_size_bound` asserts `≤ 5 bytes` per
position id for `≤ 2^31` id space.

### 5.3 Step 3 — terms.fst with triple payload

Fixed 16-byte payload `(u64 offset, u32 doc_count, u32 total_positions)`.
Tests: `…/fst.rs::tests::fst_payload_stable_under_rebuild`,
`…::tests::fst_lookup_miss_returns_none`.

### 5.4 Step 4 — Builder against normalized term stream

Run LEX-00 normalizer on `ChunkUpsert.content_bytes`; emit
`(term, doc_id, position)` triples; aggregate per-(term,doc); write FST +
nested-varint into `.building/`, atomic-rename. Tests:
`tests/build_positions.rs::builds_marker_ok_last` (atomicity per
[rfc.md § Atomicity contract](../rfc.md));
`…::normalizer_version_recorded` (older normalizer → `STATE_GENERATION_REGRESSION`
per [rfc.md § Monotonicity rules](../rfc.md)).

### 5.5 Step 5 — Phrase match (position-window intersect)

Input: ordered `Vec<NormalizedToken>`. Per token: fetch posting list,
intersect docs, position-walk with `pos(t_{i+1}) - pos(t_i) == 1`
(slop=0). Output: `PhraseCandidateSet { docs_with_spans, scan_docs }`.
Tests: `…/intersect.rs::tests::phrase_match_naive_equivalence` (proptest),
`…::tests::phrase_match_rejects_non_contiguous`.

### 5.6 Step 6 — Common-token phrase fixture

Even though the stopword *filter* is forbidden (§4.5), real-world phrases
include short tokens. Test: `…::tests::phrase_match_common_tokens` on
`"the quick brown fox"` against 1k-doc fixture.

### 5.7 Step 7 — Phrase length cap

`> 64` tokens (default) → `PLAN_LIMIT_EXCEEDED`. Test:
`…::tests::phrase_length_cap_fails_closed` asserts
`PLAN_LIMIT_EXCEEDED{dimension="phrase-length", limit=64, observed}`.

### 5.8 Step 8 — Adjacency scan depth cap

Bound pathological case (single doc with `100k+` repetitions of the same
token pair). Test: `…::tests::adjacency_scan_depth_cap` asserts
`PLAN_LIMIT_EXCEEDED{dimension="adjacency-scan-docs", limit=100000}`.

### 5.9 Step 9 — Adjacency evidence path

For `AdjacencyLink(t_a, t_b)`: fetch posting lists; per-doc count
position-pairs within window `w` (default 8); emit
`AdjacencyEvidence { docs: Vec<(DocId, pair_count, min_gap)> }`. LEX-06
consumes; this ticket only emits. Test:
`…::tests::adjacency_evidence_pair_count_correct` vs naive scan.

### 5.10 Step 10 — Planner wiring

Extend `LqPlanner::plan_lexical_leaf` (LEX-01) with `Phrase` arm calling
`open_positions` → `phrase_match`. Tests:
`crates/quanta-index-core/src/domains/query/inbound.rs::tests::phrase_plan_routes_position_shard`,
`…::tests::adjacency_link_emits_evidence_for_ranker`.

### 5.11 Step 11 — Criterion benches

`crates/quanta-index-lexical/benches/lex_03_position_bench.rs` groups:
`build_full_1k_chunks`, `phrase_match_4_token_phrase`,
`adjacency_evidence_2_terms`.

### 5.12 Step 12 — Conformance wire-in

PRE-CONF rail flips §6.2 rows from `blocked` to `ok`.
`cargo test -p quanta-index-contract --test lq_conformance`.

---

## §6 Test plan

### 6.1 Rail matrix (per [implementation-plan.md §8.1](../implementation-plan.md) LEX-03 row analog)

| Rail | Status | Command |
| --- | --- | --- |
| unit | required | `cargo test -p quanta-index-lexical` |
| integration | required | `cargo test -p quanta-index-lexical --test build_positions` |
| conformance | required | `cargo test -p quanta-index-contract --test lq_conformance` |
| property | required | proptest in unit tests |
| criterion | required | `cargo bench -p quanta-index-lexical --bench lex_03_position_bench` |
| loom | not required | n/a |

### 6.2 Conformance rows in scope

Per [usecase.md §2 A. Lexical content](../usecase.md):

- `UC-LEX-02` — Multi-word adjacency = AND (`tokio runtime`) — adjacency
  evidence emitted; primary acceptance for the adjacency path.
- `UC-LEX-03` — Exact phrase (`"async fn handle"`) — primary acceptance for
  the phrase path.
- `UC-LEX-22` — Boolean OR (`panic! OR unwrap()`) where one branch contains
  a phrase / parens token sequence; verifies parser → position shard
  hand-off under `OR` precedence.
- `UC-EDGE-02` — Unbalanced quote (`"unterminated phrase`) — parser-side
  rejection, must surface before this ticket's code path is reached.

> Note on the GOAL's UC labels: the user's brief named UC-LEX-11 /
> UC-LEX-12 as "phrase + adjacency" rows; the current
> [usecase.md](../usecase.md) numbering places those concerns on
> UC-LEX-02 / UC-LEX-03 (with UC-LEX-22 covering composition). The corpus
> row id is incidental; the acceptance contract is the semantic anchor.

### 6.3 Negative tests (fail-closed required)

| Scenario | Expected typed error | Test name |
| --- | --- | --- |
| empty phrase `""` | `PARSE_INVALID_FILTER_VALUE` (raised in LEX-01 parser, not here) | n/a (parser-layer) |
| 1-token phrase | accepted; reduces to a `Keyword` leaf via LEX-00 normalizer; not an error | `phrase_single_token_reduces_to_keyword` |
| phrase length > `64` tokens | `PLAN_LIMIT_EXCEEDED{dimension=phrase-length}` | `phrase_length_cap_fails_closed` |
| per-(term, doc) position list > `4_096` at build | `CoreError::InvalidContract{code="LEX_POSITION_LIST_OVERFLOW"}` | `builder_caps_per_doc_position_list` |
| per-term total positions > `2^25` at build | `CoreError::InvalidContract{code="LEX_POSITION_TERM_OVERFLOW"}` | `builder_caps_per_term_positions` |
| adjacency scan depth > `100_000` docs | `PLAN_LIMIT_EXCEEDED{dimension=adjacency-scan-docs}` | `adjacency_scan_depth_cap` |
| normalizer version mismatch at read | `STATE_GENERATION_REGRESSION` | `normalizer_version_recorded` |
| sibling `MARKER_OK` absent | `CoreError::NotReady` → `STATE_NOT_READY: STALE_SIBLING` | `read_fails_closed_on_missing_marker` |
| chunk-boundary phrase (cross-doc) | empty result (not an error — documented behavior) | `phrase_does_not_span_chunk_boundary` |

### 6.4 Property tests

- `nested_varint_roundtrip_property` — round-trip property
- `phrase_match_naive_equivalence` — random phrase queries vs naive scan
- `position_set_associativity_property` — phrase intersect order independence
- `manifest_marker_atomicity_property` — fault-injection harness asserts no
  reader observes a partial state ([rfc.md § Atomicity contract](../rfc.md))
- `adjacency_evidence_pair_count_correct` — random adjacency queries vs
  naive scan

---

## §7 Observability

Per [implementation-plan.md §9.1](../implementation-plan.md) Wave 2 OBS
subset:

- OpenTelemetry span: a child of `lq.exec.shard` named `lq.exec.positions`
  with attributes
  `{phrase_token_count, scan_docs, position_bytes_read, outcome}`
- Metrics: §4.6
- Structured log line per query containing
  `{ticket_id="LEX-03", canonical_query_hash, phrase_token_count, scan_docs}`
- Audit log: position-shard reads are part of the per-request audit row in
  the OBS-01 sink; no additional surface required.

---

## §8 Error scenarios

All paths produce typed `LexicalErrorCode` (lands in PRE-CONTRACT-EXT per
[implementation-plan.md §5.1](../implementation-plan.md)). **No untyped or
free-text errors on production paths** ([rfc.md § Non-Negotiable
Invariants §8](../rfc.md)).

| Scenario | Code | Carries | Retry semantics |
| --- | --- | --- | --- |
| empty phrase `""` | `PARSE_INVALID_FILTER_VALUE` (parser) | `{filter="phrase", value=""}` | not retryable |
| phrase length cap exceeded | `PLAN_LIMIT_EXCEEDED` | `{dimension="phrase-length", limit, observed}` | wait-and-retry |
| adjacency scan depth cap exceeded | `PLAN_LIMIT_EXCEEDED` | `{dimension="adjacency-scan-docs", limit, observed}` | wait-and-retry |
| per-(term, doc) position overflow at build | `CoreError::InvalidContract{code="LEX_POSITION_LIST_OVERFLOW"}` | `{term, doc_id, observed}` | not retryable |
| per-term position overflow at build | `CoreError::InvalidContract{code="LEX_POSITION_TERM_OVERFLOW"}` | `{term, observed}` | not retryable |
| normalizer version mismatch at read | `STATE_GENERATION_REGRESSION` | `{expected_version, observed_version}` | not retryable |
| sibling `MARKER_OK` absent | `CoreError::NotReady` mapped to `STATE_NOT_READY: STALE_SIBLING` | `{repo, rev, generation, sibling="positions"}` | wait-and-retry |
| codec version mismatch in `meta.cbor` | `CoreError::InvalidContract{code="LEX_POSITION_CODEC_VERSION"}` | `{expected, observed}` | not retryable |
| chunk-boundary phrase (cross-doc) | (not an error — empty result, audited) | n/a | n/a |

**No heuristic widening (e.g. dropping stopwords), no slop-fallback, no
empty-Ok substitution** (RFC § Non-Negotiable Invariants §1, §2, §11).

---

## §9 Perf envelope

### 9.1 Per-query latency contribution to single-repo SLO

Per [rfc.md § Capacity and SLO Targets § Latency SLOs](../rfc.md) the
single-repo lexical query envelope is p50 < 50 ms, p95 < 250 ms, p99 < 1 s.
The position-shard contribution is bounded to:

- p50 < 5 ms phrase match on a 1k-chunk fixture (4-token phrase)
- p95 < 25 ms phrase match
- p99 < 80 ms phrase match

Bench: `lex_03_position_bench::phrase_match_4_token_phrase`. Regression
budget +5% per wave per [implementation-plan.md §8.2](../implementation-plan.md).

### 9.2 Position list compression target

Empirical target: ≤ `0.6 ×` raw chunk-bytes total disk usage for the
position shard at default knobs, on the standard 1k-chunk fixture.
Delta-varint nesting (doc-gap outside, position-gap inside) compresses
well on natural-language and code corpora where token positions are
densely packed.

Asserted by
`crates/quanta-index-lexical/tests/build_positions.rs::position_disk_size_within_0_6x_raw_bytes`.

### 9.3 Phrase length cap rationale

`64` tokens default. Beyond this:

- the position-window intersect cost grows linearly with phrase length
- adjacency intersect on `> 64` distinct terms is rarely a "phrase" intent;
  more often an exotic regex that should land on the trigram (LEX-02) or
  regex (LEX-03 content-shard read path) lane

Configurable up to `512`. Floor `2` (a single-token phrase reduces to a
keyword leaf via LEX-00 normalization — see §6.3).

### 9.4 Bound on adjacency scanning depth

`100_000` docs default. Beyond this, fail closed. Adversarial input
"two universally-common tokens" (e.g. `the` + `and`) hits this cap quickly
on large corpora; the operator's lever is to narrow scope via
`repo:` / `file:` / `lang:` before adjacency evaluation, per the same
playbook as LEX-02 §9.6.

### 9.5 Build throughput

Per [feature-scope.md §7](../feature-scope.md) Phase 1 build throughput
target is `≥ 10 MB/s` wall-clock. Position build must not be the bottleneck:

- per-chunk tokenizer run: O(content_bytes) via LEX-00 normalizer
- aggregation: O(unique_terms_per_chunk) per chunk
- empirical target: ≥ 50 MB/s in-process position-shard build alone
  (so the combined build pipeline stays ≥ 10 MB/s when LEX-02 + LEX-03 run
  in sequence)

---

## §10 Risks

| ID | Description | Probability | Impact | Early-warning signal | Mitigation |
| --- | --- | --- | --- | --- | --- |
| LEX-03-R1 | LEX-00 normalizer churn invalidates all position shards mid-program | M | H | `normalizer_version_recorded` test fails on shards built at the older version | normalizer version is in `meta.cbor`; mismatch surfaces `STATE_GENERATION_REGRESSION`; storage layer enforces atomic rebuild |
| LEX-03-R2 | Position-list storage growth exceeds `0.6 ×` raw bytes | M | M | `position_disk_size_within_0_6x_raw_bytes` fails | per-term and per-(term, doc) caps; FST + delta-varint compression |
| LEX-03-R3 | Phrase-spanning chunk boundaries surprises callers | M | M | conformance row UC-LEX-03 fails on a fixture where the phrase legitimately spans a chunk | documented behavior; chunk-id is doc-id; cross-chunk phrase = empty result; expand if a future ticket lands sentence-level chunking |
| LEX-03-R4 | Adjacency evidence shape change forces LEX-06 rework | M | M | LEX-06 spec drift | pin `AdjacencyEvidence` shape in this ticket; LEX-06 consumes via port |
| LEX-03-R5 | Adversarial input with two extremely common terms (`the` + `and`) saturates adjacency scan | M | H | bench `adjacency_evidence_2_terms` p99 jumps >5× | adjacency-scan-depth cap (§4.2); operator narrows scope |
| LEX-03-R6 | Stopword decision regretted later — caller wants smaller index | L | M | operator feedback after Wave 8 | locked policy in §4.5; future change requires explicit DSL minor bump and an opt-in normalizer surface; not introduced here |
| LEX-03-R7 | G-CONTROL-LOC unresolved (per [implementation-plan.md §2.3a](../implementation-plan.md)) blocks ticket entry | H | M | manifest registration call site has no physical owner | ticket entry gate; resolve before Wave-2 start |
| LEX-03-R8 | LEX-00 normalizer not yet landed (Wave-0/1) and this ticket depends on it for positions semantics | M | H | wave entry gate fails | dependency gate; this ticket cannot enter until LEX-00 normalizer pipeline merges |

### 10.1 LEX-00 normalizer interplay (locked)

Positions are recorded **after** the LEX-00 normalizer pipeline. Consequences:

1. The `normalizer_version` lives in `meta.cbor`. A normalizer change is a
   breaking shard-format change; storage-layer enforcement under
   [rfc.md § Storage-layer enforcement](../rfc.md) surfaces it as
   `STATE_GENERATION_REGRESSION`. **No silent semantic drift.**
2. Query-side phrase tokens are normalized *with the same* normalizer
   before lookup. The carrier `PhraseLeaf { tokens: Vec<NormalizedToken> }`
   embeds normalizer-aware tokens so the planner cannot accidentally compare
   raw to normalized.
3. Case folding, stemming, and any other normalizer behavior is the
   normalizer's authority — this ticket does not duplicate it. A
   case-sensitive query (`case:yes`, [usecase.md UC-LEX-16](../usecase.md))
   passes a tag through the carrier; the normalizer respects it; positions
   reflect it.
4. Position numbers refer to *normalized-stream offsets*, not byte offsets.
   The chunk content shard owns byte-offset translation at result-assembly
   time (LEX-06 explain path).

---

## §11 DoD (provable)

Every DoD row cites a test name and path.

| # | DoD item | Evidence |
| --- | --- | --- |
| 1 | per-generation position shard layout exists (§4.1) | `crates/quanta-index-lexical/tests/build_positions.rs::layout_matches_spec` |
| 2 | `meta.cbor` codec hand-rolled (D18) | `crates/quanta-index-lexical/src/positions/meta.rs::tests::roundtrip_canonical_cbor` + semgrep `rust-no-serde-derive` green at [`tools/ci/semgrep/rules.yml:124`](../../../../tools/ci/semgrep/rules.yml#L124) |
| 3 | nested varint posting round-trips | `crates/quanta-index-lexical/src/positions/postings.rs::tests::nested_varint_roundtrip_property` |
| 4 | terms.fst with triple payload is stable | `crates/quanta-index-lexical/src/positions/fst.rs::tests::fst_payload_stable_under_rebuild` |
| 5 | builder emits `MARKER_OK` last, atomic | `crates/quanta-index-lexical/tests/build_positions.rs::builds_marker_ok_last` |
| 6 | `meta.cbor` records `normalizer_version`; mismatch fails closed | `crates/quanta-index-lexical/tests/build_positions.rs::normalizer_version_recorded` |
| 7 | single-token phrase reduces via normalizer | `crates/quanta-index-lexical/src/positions/intersect.rs::tests::phrase_single_token_reduces_to_keyword` |
| 8 | phrase match equivalent to naive scan | `…::tests::phrase_match_naive_equivalence` |
| 9 | phrase rejects non-contiguous matches | `…::tests::phrase_match_rejects_non_contiguous` |
| 10 | common-token phrase ("the quick brown fox") matches | `…::tests::phrase_match_common_tokens` |
| 11 | phrase length cap surfaces `PLAN_LIMIT_EXCEEDED` | `…::tests::phrase_length_cap_fails_closed` |
| 12 | adjacency scan depth cap surfaces `PLAN_LIMIT_EXCEEDED` | `…::tests::adjacency_scan_depth_cap` |
| 13 | per-(term, doc) position cap at build | `…::tests::builder_caps_per_doc_position_list` |
| 14 | per-term position cap at build | `…::tests::builder_caps_per_term_positions` |
| 15 | planner routes `Phrase` leaf to position shard | `crates/quanta-index-core/src/domains/query/inbound.rs::tests::phrase_plan_routes_position_shard` |
| 16 | `AdjacencyLink` emits evidence for ranker | `…::tests::adjacency_link_emits_evidence_for_ranker` |
| 17 | cross-chunk phrase returns empty (documented) | `…::tests::phrase_does_not_span_chunk_boundary` |
| 18 | fault-injection atomicity property | `…::tests::manifest_marker_atomicity_property` |
| 19 | disk size stays within `0.6 ×` raw bytes | `…::tests::position_disk_size_within_0_6x_raw_bytes` |
| 20 | conformance rows green: `UC-LEX-02`, `UC-LEX-03`, `UC-LEX-22` | `cargo test -p quanta-index-contract --test lq_conformance` |
| 21 | criterion bench compiles & runs | `cargo bench -p quanta-index-lexical --bench lex_03_position_bench` |
| 22 | clippy `-D warnings`, `cargo fmt --check`, `cargo deny`, semgrep green | wave-exit CI |
| 23 | OpenTelemetry span `lq.exec.positions` emits with §4.6 attributes | `crates/quanta-index-lexical/tests/observability.rs::tests::emits_lq_exec_positions_span` |
| 24 | metric `lex.position.phrase_match_count{outcome=plan_limit_exceeded}` increments on cap miss | `…::tests::metric_increments_on_cap_miss` |
| 25 | no stopword filter behavior (negative test asserts every token has positions, including the `the` row) | `…::tests::no_stopword_filter` |

---

## §12 Open questions

| Q-ID | Question | Blocking | Forcing function |
| --- | --- | --- | --- |
| LEX-03-Q1 | Stopwords — locked here as "no filter". Confirm with feature-scope owner that this is the program-wide answer. Once locked, requires a DSL minor bump to revisit. | Wave-2 entry | review of §4.5 before ticket starts |
| LEX-03-Q2 | Adjacency default window is `8` per [dsl.md §5.3](../dsl.md). Confirm with LEX-06 owner that the rank curve is calibrated for that window before LEX-06 wave entry. | Wave-4 entry (LEX-06) | LEX-06 IR-eval calibration |
| LEX-03-Q3 | Phrase position storage layout: nested-varint here vs `roaring` bitmaps. Nested-varint wins on size; `roaring` wins on intersect cost at large scale. Decide before Wave-3 incremental work. | Wave-3 entry (LEX-04) | bench `phrase_match_4_token_phrase` vs `roaring` prototype |
| LEX-03-Q4 | Phrase across chunk boundaries — locked as "empty result". Is there an operator demand for cross-chunk phrase? If yes, that requires a separate sentence/paragraph-level shard, out of scope here. | post-Wave-8 | operator feedback after Wave 8 |
| LEX-03-Q5 | G-CONTROL-LOC (see [implementation-plan.md §2.3a](../implementation-plan.md)) — where does the manifest entry for the positions sibling get registered? | Wave-2 entry | resolve before this ticket starts |
| LEX-03-Q6 | The user-supplied GOAL referenced UC-LEX-11 / UC-LEX-12 as the canonical "phrase + adjacency" rows; the current corpus places these on UC-LEX-02 / UC-LEX-03 (with UC-LEX-22 covering composition). Is the corpus renumbering an expected sibling change? | Wave-2 exit gate | usecase.md sync; not blocking ticket implementation |

---

## §13 References

- [rfc.md](../rfc.md) — parent
  - § LQ Family, § Pattern semantics, § Atomicity contract, § Monotonicity
    rules, § Error Code Taxonomy
- [feature-scope.md](../feature-scope.md)
  - §1.1.1 (pattern leaves), §1.1.2 (boolean composition), §6.1 (Core-1.0
    authority chain), §7 (scale targets)
- [usecase.md](../usecase.md)
  - §2 A. Lexical content (UC-LEX-02 adjacency, UC-LEX-03 phrase, UC-LEX-22
    OR composition); §2 H. (UC-EDGE-02 unbalanced quote)
- [dsl.md](../dsl.md)
  - §3.2 (Phrase semantics), §5.3 (adjacency — proximity boost, window=8),
    §10 (normalization), §12 (error taxonomy), §13 (limits and budgets),
    §16 (DSL invariants)
- [implementation-plan.md](../implementation-plan.md)
  - §2.4 (lexical adapter current state — LEX-03 row "partial"), §4.3
    (Wave 2 exit gate), §5.7 (sibling unification DoD), §8 (test strategy),
    §9 (observability), §11 (G-CONTROL-LOC)
- [CLAUDE.md](../../../../CLAUDE.md)
  - § Agent change posture (breaking-first), § Build hygiene (D18 — no
    serde derives)
- [AGENTS.md](../../../../AGENTS.md)
  - shared rule catalog
- [`crates/quanta-index-lexical/src/lib.rs`](../../../../crates/quanta-index-lexical/src/lib.rs)
  — current adapter scaffold (returns `NotImplemented`)
- [`tools/ci/semgrep/rules.yml:124`](../../../../tools/ci/semgrep/rules.yml#L124)
  — `rust-no-serde-derive` rule
- [LEX-02.md](LEX-02.md) — Wave-2 sibling (trigram / regex prefilter); shares
  generation manifest and `MARKER_OK` invariant

---

> End of `LEX-03.md`.
