# LEX-02 — Trigram / N-gram index for substring + regex acceleration

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md), and [MAY-27-002](../../../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


| Field | Value |
| --- | --- |
| Ticket ID | `LEX-02` |
| Title | Trigram / N-gram index for substring + regex acceleration |
| Status | shipped |
| Crate | `quanta-index-lq-trigram` |
| Tests | 56 |
| Last verified | 2026-05-25 |
| Wave | 2 (per [rfc.md § Canonical Execution Waves](../rfc.md)) |
| Parent RFC section | [rfc.md § LQ Family `LQ/Core-1.0`](../rfc.md), [rfc.md § Ticket Pack](../rfc.md), [rfc.md § Non-Negotiable Invariants §10](../rfc.md) |
| Sibling planning docs | [feature-scope.md](../feature-scope.md), [usecase.md](../usecase.md), [dsl.md](../dsl.md), [implementation-plan.md](../implementation-plan.md) |
| Owner crate(s) | `quanta-index-lq-trigram` (G-CONTROL-LOC resolved as standalone crate); consumes `quanta-index-core` ports |
| Touches contract crate? | No (sibling shard is internal; no `LqQuery` / `LexicalCandidate` shape change) |
| Posture | Breaking-first; no long-lived shims (per [CLAUDE.md § Agent change posture](../../../../CLAUDE.md)) |
| Claim discipline anchor | [rfc.md § Claim Discipline §1](../rfc.md) (parser conformance + global execution proof leg for `patterntype:raw`) |

---

## §1 Purpose

Build a per-generation N-gram (default `n=3`, "trigram") inverted index that
accelerates two query paths the current chunk index cannot answer without a
full scan:

1. `'…'` raw-string substring leaves ([dsl.md §3.3](../dsl.md)), required by
   `LqLexicalQuery::RawString` and by `patterntype:literal` /
   `patterntype:standard` raw segments.
2. `/…/` regex leaves ([dsl.md §3.4](../dsl.md)) when the planner needs a
   trigram prefilter to bound the candidate set before RE2 evaluation.

The shard is an *authority-grade* candidate producer; it is **not** a
verification surface. Planner intersects trigram postings → bounded candidate
doc set → a separate verify stage (chunk content shard via Tantivy
`RegexQuery` per [dsl.md §3.4](../dsl.md) or `memmem::find` for raw-string)
confirms exact bytes/regex match. This ticket lands the storage layout,
build path, intersect read path, bounded-query fail-closed semantics, and
the criterion bench. Ranking, explain, and BM25 are LEX-05/LEX-06.

---

## §2 Background

### 2.1 Current state

[`crates/quanta-index-lexical/src/lib.rs`](../../../../crates/quanta-index-lexical/src/lib.rs)
returns `CoreError::NotImplemented` for build and open — the predecessor
Tantivy adapter was removed pending the channel-event rewrite. Per
[implementation-plan.md §2.4](../implementation-plan.md) the LEX-02 row
("trigram for raw-string `'…'` substring search") is **absent**.

### 2.2 Why now

[`dsl.md §3.3`](../dsl.md) pins: RawString "requires a trigram or N-gram
index for non-degenerate matching. If the active backend does not expose
such an index, planning fails with `PLAN_LIMIT_EXCEEDED{dimension=
rawstring-backend, limit=0}` — not silently degraded to phrase." Wave 2
cannot exit (per [implementation-plan.md §4.3](../implementation-plan.md))
without an authority backing `'…'`, since UC-LEX-04 ("Raw string", per
[usecase.md §2](../usecase.md)) becomes runnable only when this shard
exists.

### 2.3 Anti-scope

- **No regex verification.** Shard returns a *candidate* set; final regex
  truth comes from Tantivy `RegexQuery` (per [dsl.md §3.4](../dsl.md)
  mapping) on the chunk content shard read path.
- **No tokenizer normalization.** Raw strings preserve bytes; LEX-00
  normalizer does **not** apply to `'…'` leaves ([dsl.md §3.3](../dsl.md)).
- **No multi-byte glob.** Indexes byte trigrams; UTF-8 consequences in §10.
- **No incremental write path.** Wave 2 builds full-generation shards;
  chunk-grain delta is LEX-04.

### 2.4 Authority chain

Per [feature-scope.md §6.1](../feature-scope.md), producer publishes chunk
rows. The trigram shard is a *derivative* under one manifest generation
set; sibling status is asserted at read time by
[rfc.md § Storage-layer enforcement](../rfc.md) `MARKER_OK` invariant.

---

## §3 Inputs

### 3.1 From upstream sources

- chunk rows for `(repo, rev, generation)` via
  [`LexicalChannelOp`](../../../../crates/quanta-index-contract/src/channel)
- `(repo, rev, generation)` tuple from the control-plane catalog (subject to
  **G-CONTROL-LOC** in [implementation-plan.md §2.3a](../implementation-plan.md))
- per-deployment N-gram width `n` (default `3`, floor `3`, ceiling `5`; §9.3)

### 3.2 From sibling tickets

- LEX-00 baseline (invariants freeze) green
- LEX-01 canonical AST + parser — `LqLexicalQuery::RawString` variant exists
  (PRE-CONTRACT-EXT per [implementation-plan.md §5.1](../implementation-plan.md))
- LEX-03 content/path/symbol siblings — same `manifest_generation` per
  [implementation-plan.md §5.7](../implementation-plan.md)

### 3.3 Carriers and types

- `LqLexicalQuery::RawString(RawStringLeaf { bytes: Bytes })` (PRE-CONTRACT-EXT)
- internal `TrigramShard` (private to `quanta-index-lexical`)
- internal ports `LexicalIndexTrigramOpenPort::open_trigram(...)` and
  `LexicalIndexTrigramBuildPort::build_trigram(...)` — **not** contract types

---

## §4 Deliverables

### 4.1 Storage layout (per-generation, immutable)

```
{state_root}/indexes/lexical/{repo_id}/{rev_id}/{generation}/trigram/
├── postings.fst        # Finite-state transducer: trigram -> postings offset
├── postings.varint     # delta-varint-encoded posting lists, doc-id ASCending
├── docid.map           # candidate-id -> (chunk_id, byte_offset_start)
├── meta.cbor           # n, doc_count, total_postings, trigram_count, codec_version
└── MARKER_OK           # per-sibling readiness sentinel (per RFC § Atomicity contract)
```

- `postings.fst` is built from the sorted set of all distinct trigrams found
  across the generation's chunks. FST keying is the 3-byte trigram value;
  payload is the offset into `postings.varint`.
- `postings.varint` stores per-trigram posting lists as
  `varint(doc_count) varint(d_1) varint(d_2 - d_1) … varint(d_n - d_{n-1})`.
  Delta gaps keep on-disk size bounded; see §9 for the size guarantee.
- `docid.map` is a fixed-stride array indexed by candidate id; lookup is
  `O(1)`.
- `meta.cbor` is CBOR-encoded ([rfc.md § Migration and Versioning Policy](../rfc.md)
  bytes-determinism rules apply) hand-rolled per-D18 (no
  `#[derive(serde::Serialize)]` — see
  [tools/ci/semgrep/rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124)).
- `MARKER_OK` is written **last**, after `fsync` on every file above.

### 4.2 Per-deployment knobs

| Knob | Default | Floor | Ceiling | Source |
| --- | --- | --- | --- | --- |
| N-gram width `n` | `3` | `3` | `5` | this ticket |
| Posting list cap per trigram | `2_097_152` (`2^21`) | `65_536` | `16_777_216` | this ticket |
| Candidate set cap pre-verify | `100_000` | `1_000` | `1_000_000` | aligned with [dsl.md §13](../dsl.md) regex NFA budget |
| Trigram set cap per query | `4_096` | `64` | `65_536` | this ticket; see §9.5 |

Exceeding any cap → `PLAN_LIMIT_EXCEEDED` per [dsl.md §12](../dsl.md);
no silent degradation. Carrier: `LexicalErrorCode::PLAN_LIMIT_EXCEEDED`
(landed in PRE-CONTRACT-EXT per
[implementation-plan.md §5.1](../implementation-plan.md)).

### 4.3 Public ports (internal to lexical adapter)

- `LexicalIndexTrigramBuildPort::build_trigram(repo, rev, gen, ops)` —
  consumes the same `&[LexicalChannelOp]` slice the content builder consumes
- `LexicalIndexTrigramOpenPort::open_trigram(repo, rev, gen)` →
  `Box<dyn TrigramSearcher>`
- `TrigramSearcher::intersect(query: &TrigramSearchQuery) ->
  Result<TrigramCandidateSet, CoreError>`

Not contract types; these are
[`quanta-index-core::domains::lexical`](../../../../crates/quanta-index-core/src/domains)
port traits. Adapter impl in
[`crates/quanta-index-lexical/src/`](../../../../crates/quanta-index-lexical/src/lib.rs).

### 4.4 Planner wiring

- `LqPlanner::plan` routes `LqExpr::PatternLeaf(PatternLeaf::RawString(_))`
  through trigram intersect; verify pass on the chunk content shard.
- Regex prefilter: when the regex HIR yields a mandatory literal set (via
  `regex_syntax::hir::literal::Extractor`), the planner runs trigram intersect
  first, then issues a Tantivy `RegexQuery` over only surviving candidates.
- Non-decomposable regex (e.g. `/.*/`, `/\w+/`) falls to the verify-only
  path explicitly — not silent degradation; a metric label records the
  fallback.

### 4.5 Metrics (per [rfc.md § Execution Model § Metric schema](../rfc.md))

| Metric | Unit | Label set |
| --- | --- | --- |
| `lex.trigram.intersect_count` | count | `{ticket_id="LEX-02", wave_id="2", outcome="ok"|"plan_limit_exceeded"|"empty_set"|"verify_failed"}` |
| `lex.trigram.candidate_set_size` | count | `{ticket_id="LEX-02"}` |
| `lex.trigram.intersect_ms` | milliseconds | `{ticket_id="LEX-02"}` |
| `lex.trigram.posting_bytes_read` | bytes | `{ticket_id="LEX-02"}` |
| `lex.trigram.build_ms` | milliseconds | `{ticket_id="LEX-02", phase="full"|"delta"}` |

Cardinality budget: `5 metrics × ≤4 labels × ≤4 values = 80` per
[rfc.md § Observability Requirements](../rfc.md). Closed label set; new
labels require version bump per RFC.

---

## §5 Implementation steps (TDD)

Failing-test-first. Test names are stable and serve as §11 DoD evidence.

### 5.1 Step 1 — `meta.cbor` codec round-trip

Hand-rolled `impl Serialize / Deserialize for TrigramShardMeta` (D18 per
[CLAUDE.md § Build hygiene](../../../../CLAUDE.md)). CBOR canonical bytewise
determinism cross-process. Test:
`crates/quanta-index-lexical/src/trigram/meta.rs::tests::roundtrip_canonical_cbor`.

### 5.2 Step 2 — varint posting-list codec

LEB128 unsigned varint, `(doc_count, deltas…)`. Tests:
`…/postings.rs::tests::varint_roundtrip_property` (proptest 1k shapes);
`…::tests::varint_size_bound` asserts `≤ 5 bytes` per id for `≤ 2^31` ids.

### 5.3 Step 3 — FST keying

`fst` crate pinned per [implementation-plan.md § Risk R2](../implementation-plan.md)
approach. Tests:
`…/fst.rs::tests::fst_keys_are_sorted`,
`…::tests::fst_lookup_miss_returns_none`.

### 5.4 Step 4 — Builder against `&[LexicalChannelOp]`

For each `ChunkUpsert { content_bytes, .. }`, slide a 3-byte window, intern
into a trigram → `Vec<doc_id>` map, emit FST + varint + map files into a
`.building/` dir, then atomic-rename. Tests:
`tests/build_full_generation.rs::builds_marker_ok_last` (atomicity per
[rfc.md § Atomicity contract](../rfc.md));
`…::builder_rejects_non_utf8_chunk` (typed `CoreError::InvalidContract`).

### 5.5 Step 5 — Short-input fast path

A `'…'` leaf shorter than `n` bytes yields `TrigramSearchQuery::None`; the
planner falls to the verify-only path. Test:
`…/intersect.rs::tests::short_input_short_circuits`.

### 5.6 Step 6 — Intersect read path

Input: `Vec<Trigram> = Vec<[u8; 3]>`. Galloping/leapfrog merge across sorted
posting lists. Output:
`TrigramCandidateSet { doc_ids, scanned_trigrams, posting_bytes_read }`.
Tests: `…::tests::intersect_matches_naive_scan` (1k-doc fixture);
`…::tests::intersect_respects_candidate_cap` asserts
`PLAN_LIMIT_EXCEEDED{dimension="trigram-candidate-set", limit=100000}`.

### 5.7 Step 7 — Planner wiring

Extend `LqPlanner::plan_lexical_leaf` (LEX-01) with a `RawString` arm:
open trigram shard → intersect → verify on chunk content shard via
`memmem::find` (`memchr` crate). Tests:
`crates/quanta-index-core/src/domains/query/inbound.rs::tests::rawstring_plan_routes_trigram`,
`…::tests::rawstring_verify_rejects_non_substring` (catches a false positive
where trigrams match but bytes are not contiguous).

### 5.8 Step 8 — Regex prefilter

Extract required literals via `regex_syntax::hir::literal::Extractor`;
convert each `≥ n`-byte literal to its trigram set; intersect. Tests:
`…/regex_prefilter.rs::tests::prefilter_extracts_required_literals` on a 30
regex fixture; `…::tests::prefilter_falls_to_verify_for_pure_wildcards` on
`/.*/`.

### 5.9 Step 9 — Criterion benches

`crates/quanta-index-lexical/benches/lex_02_trigram_bench.rs` groups:
`build_full_1k_chunks`, `intersect_three_trigrams`,
`regex_prefilter_extract_30_patterns`. Regression budget +5% per wave per
[implementation-plan.md §8.2](../implementation-plan.md).

### 5.10 Step 10 — Conformance wire-in

PRE-CONF rail flips §6.2 rows from `blocked` to `ok`.
`cargo test -p quanta-index-contract --test lq_conformance`.

---

## §6 Test plan

### 6.1 Rail matrix (per [implementation-plan.md §8.1](../implementation-plan.md) LEX-03 row analog)

| Rail | Status | Command |
| --- | --- | --- |
| unit | required | `cargo test -p quanta-index-lexical` |
| integration | required | `cargo test -p quanta-index-lexical --test build_full_generation` |
| conformance | required | `cargo test -p quanta-index-contract --test lq_conformance` |
| property | required | unit tests use proptest |
| criterion | required | `cargo bench -p quanta-index-lexical --bench lex_02_trigram_bench` |
| loom | not required (no shared-mutable across threads at read time) | n/a |

### 6.2 Conformance rows in scope

Per [usecase.md §2 A. Lexical content](../usecase.md):

- `UC-LEX-04` — Raw string (`'C:\\Users\\%'`) — primary acceptance for this
  ticket; planner must hit the trigram intersect and verify path.
- `UC-LEX-05` — Regex literal (`/fn\s+handle_\w+/`) — verifies regex
  prefilter extracts literal `fn ` and `handle_` trigrams.
- `UC-LEX-06` — Regex with line anchor (`/^fn foo/`) — verifies the literal
  `fn foo` extraction and that anchor metadata flows to verify-time.
- `UC-LEX-21` — `patterntype:regexp` switch — verifies that the prefilter
  applies under `regexp` mode adjacency.
- `UC-EDGE-03` — Regex compile fail (`/foo(/`) — must surface as
  `PARSE_INVALID_REGEX` before this ticket's code path is reached.
- `AC-05`, `AC-06` — backref / lookbehind — same: parser-side rejection.

> Note on the GOAL's UC labels: the user's brief named UC-LEX-09 / UC-LEX-10
> as the canonical "raw substring" / "regex prefilter" rows. The current
> [usecase.md](../usecase.md) numbering places those concerns on UC-LEX-04
> / UC-LEX-05 / UC-LEX-06. The corpus row id is incidental; the acceptance
> contract is the *semantic anchor*. If usecase.md is later re-numbered to
> add explicit UC-LEX-09 / UC-LEX-10 rows for substring/regex-prefilter,
> this ticket's conformance set extends accordingly (additive, no scope
> change).

### 6.3 Negative tests (fail-closed required)

| Scenario | Expected typed error | Test name |
| --- | --- | --- |
| `'ab'` (input shorter than `n=3`) | not an error — short-circuit to verify-only | `short_input_short_circuits` |
| trigram candidate set > `100_000` | `PLAN_LIMIT_EXCEEDED{dimension=trigram-candidate-set, limit=100000}` | `intersect_respects_candidate_cap` |
| trigram count per query > `4_096` | `PLAN_LIMIT_EXCEEDED{dimension=trigram-set, limit=4096}` | `query_respects_trigram_count_cap` |
| posting list per trigram > `2^21` entries at build time | `CoreError::InvalidContract{code="LEX_TRIGRAM_POSTING_OVERFLOW"}` | `builder_caps_posting_list` |
| non-UTF-8 chunk bytes at build time | `CoreError::InvalidContract{code="LEX_TRIGRAM_NON_UTF8_INPUT"}` | `builder_rejects_non_utf8_chunk` |
| read path sees `MARKER_OK` missing | `CoreError::NotReady` | `read_fails_closed_on_missing_marker` |
| stale sibling generation | `STATE_NOT_READY: STALE_SIBLING` per [rfc.md § Monotonicity rules](../rfc.md) | `read_fails_on_stale_sibling` |

### 6.4 Property tests

- `varint_roundtrip_property` — 1k random posting lists round-trip
- `intersect_associativity_property` — intersect of `(A, B, C)` is order-independent
- `fst_lookup_property` — for each trigram in input set, FST lookup returns
  the right offset
- `manifest_marker_atomicity_property` — under a fault-injection harness
  that aborts at random byte offsets during build, no reader ever observes
  a partial state (per [rfc.md § Atomicity contract](../rfc.md))

---

## §7 Observability

Per [implementation-plan.md §9.1](../implementation-plan.md) Wave 2 OBS
subset:

- OpenTelemetry span: a child of `lq.exec.shard` named `lq.exec.trigram`
  with attributes `{trigram_count, posting_bytes_read, candidate_set_size, outcome}`
- Metrics: §4.5
- Structured log line per query containing
  `{ticket_id="LEX-02", canonical_query_hash, trigram_count, candidate_set_size}`
- Audit log: trigram-shard reads are part of the per-request audit row in
  the OBS-01 sink; no additional audit-specific surface is required here.

---

## §8 Error scenarios

All paths produce typed `LexicalErrorCode` (lands in PRE-CONTRACT-EXT per
[implementation-plan.md §5.1](../implementation-plan.md)). **No untyped or
free-text errors on production paths** ([rfc.md § Non-Negotiable
Invariants §8](../rfc.md)).

| Scenario | Code | Carries | Retry semantics |
| --- | --- | --- | --- |
| input shorter than `n` | (not an error) | n/a | n/a |
| empty trigram set after extraction | (not an error — falls to verify-only) | n/a | n/a |
| candidate set cap exceeded | `PLAN_LIMIT_EXCEEDED` | `{dimension="trigram-candidate-set", limit, observed}` | wait-and-retry |
| trigram count cap exceeded | `PLAN_LIMIT_EXCEEDED` | `{dimension="trigram-set", limit, observed}` | wait-and-retry |
| posting overflow at build | `CoreError::InvalidContract` carrying `"LEX_TRIGRAM_POSTING_OVERFLOW"` | `{trigram, observed_postings}` | not retryable |
| non-UTF-8 input at build | `CoreError::InvalidContract` carrying `"LEX_TRIGRAM_NON_UTF8_INPUT"` | `{chunk_id, byte_offset}` | not retryable |
| sibling `MARKER_OK` absent at read time | `CoreError::NotReady` mapped to `STATE_NOT_READY: STALE_SIBLING` | `{repo, rev, generation, sibling="trigram"}` | wait-and-retry |
| monotonicity violation (non-NULL → NULL trigram gen) | `STATE_GENERATION_REGRESSION` | `{repo, rev, prev_gen, observed_gen}` | not retryable |
| codec version mismatch in `meta.cbor` | `CoreError::InvalidContract` carrying `"LEX_TRIGRAM_CODEC_VERSION"` | `{expected, observed}` | not retryable |

**No heuristic widening, no empty-Ok fallback, no "best matching" generation
substitution** (RFC § Non-Negotiable Invariants §1, §11).

---

## §9 Perf envelope

### 9.1 Per-query latency contribution to single-repo SLO

Per [rfc.md § Capacity and SLO Targets § Latency SLOs](../rfc.md) the
single-repo lexical query envelope is p50 < 50 ms, p95 < 250 ms, p99 < 1 s.
The trigram intersect path's contribution is bounded to:

- p50 < 5 ms intersect time on a 1k-chunk fixture (3 trigrams)
- p95 < 25 ms intersect time
- p99 < 80 ms intersect time

Bench: `lex_02_trigram_bench::intersect_three_trigrams`. Regression budget
+5% per wave (per [implementation-plan.md §8.2](../implementation-plan.md)).

### 9.2 Index size bound

For a corpus of `D` chunks averaging `C` bytes per chunk:

- distinct trigram count ≤ `min(D × (C - 2), 256^3) = min(D × (C - 2), 16_777_216)`
- per-trigram posting list size (delta-varint): `≤ D × 5 bytes` worst case
- total bytes on disk: `O(min(D × C, 256^3) + sum of posting bytes)`
- empirical target: ≤ `1.2 ×` raw chunk-bytes total disk usage at default
  knobs

This is asserted by
`crates/quanta-index-lexical/tests/build_full_generation.rs::trigram_disk_size_within_1_2x_raw_bytes`
against the standard 1k-chunk fixture.

### 9.3 N-gram width rationale

- `n=2` (bigram): too many false positives (256² = 65,536 distinct grams,
  intersect rarely narrows enough to bound the verify pass).
- `n=3` (trigram): default; standard literature pin; intersect cardinality
  empirically narrows candidate sets to ≤ `O(D / 256)` for queries with ≥ 3
  trigrams.
- `n=4`/`n=5`: storage grows ~256× per `n` step; only useful for known-long
  query distributions. Configurable but ceiling is `n=5`.

### 9.4 Build throughput

Per [feature-scope.md §7](../feature-scope.md) Phase 1 build throughput
target is `≥ 10 MB/s` wall-clock. Trigram build must not be the bottleneck:

- Per-chunk slide is O(content_bytes); a single-thread pass over the chunk
  set is the build dominant cost.
- Empirical target: ≥ 80 MB/s in-process trigram build alone (so the
  combined build pipeline stays ≥ 10 MB/s).

### 9.5 Trigram-count-per-query cap

A raw-string `'…'` leaf of `L` bytes yields `L - 2` trigrams. The default
cap of `4_096` allows leaves up to 4,098 bytes. Beyond this, we
`PLAN_LIMIT_EXCEEDED{dimension=trigram-set}` rather than degrade.
A regex prefilter's literal extraction can also produce many trigrams; the
same cap applies.

### 9.6 At-scale degradation strategy

There is **no silent degradation**. When any cap is exceeded the planner
emits `PLAN_LIMIT_EXCEEDED`. The operator's available levers:

1. raise the per-deployment cap up to the ceiling (§4.2)
2. reduce query scope via `repo:` / `file:` / `lang:` to shrink the
   candidate pool *before* trigram intersect
3. accept the fail-closed envelope and surface a typed error to the caller

Option (1) is not silent — it requires an operator config change with an
audit-log row. Options (2) and (3) preserve the no-silent-fallback
invariant.

---

## §10 Risks

| ID | Description | Probability | Impact | Early-warning signal | Mitigation |
| --- | --- | --- | --- | --- | --- |
| LEX-02-R1 | Multi-byte UTF-8 trigram semantics confuse callers | M | M | property test failure on a Unicode corpus | choice locked in §10 below; documented and tested |
| LEX-02-R2 | Adversarial input creates `O(D)` trigram intersect | M | H | criterion `intersect_three_trigrams` p99 jumps >5× | per-query trigram cap (§4.2); `PLAN_LIMIT_EXCEEDED` |
| LEX-02-R3 | Storage growth from trigram shard exceeds `1.2 ×` raw bytes | M | M | size assertion in `trigram_disk_size_within_1_2x_raw_bytes` fails | per-trigram posting cap; FST compresses well on text corpora |
| LEX-02-R4 | Builder bottlenecks index build throughput SLO | L | M | bench `build_full_1k_chunks` slows | optional Rayon-parallel chunk pass; budget reservation in the manifest |
| LEX-02-R5 | Regex prefilter extracts no literal for common `/^…$/` patterns | M | M | bench `prefilter_extracts_required_literals` shows >50% miss rate | document fallback-to-verify-only path; the engine retains correctness; only performance regresses; not a fail-closed scenario |
| LEX-02-R6 | FST crate API drift (Burntsushi maintenance cadence) | L | L | `cargo update` flags `fst ^0.4 → 0.5` | pin to `=0.4.x` per [implementation-plan.md § Risk R2](../implementation-plan.md) approach |
| LEX-02-R7 | G-CONTROL-LOC unresolved (per [implementation-plan.md §2.3a](../implementation-plan.md)) blocks ticket entry | H | M | manifest registration call site has no physical owner | ticket entry gate; resolve before Wave-2 start |

### Multi-byte / Unicode handling (locked choice)

Trigrams are **byte trigrams**, not code-point trigrams.

Rationale:

1. raw strings preserve bytes ([dsl.md §3.3](../dsl.md)); the indexable
   substrate is bytes
2. byte trigrams are O(1) constant width — no Unicode-segmenter dependency
3. UTF-8 self-synchronization guarantees that a byte trigram which crosses
   a code-point boundary is still meaningful as a discriminator on the
   indexed corpus, even though it has no "character" interpretation

Consequence: a query `'한'` (3 bytes in UTF-8: `0xED 0x95 0x9C`) reduces to a
single trigram; this is exactly correct for substring matching. A query
`'한국'` (6 bytes) yields 4 trigrams; intersect is over those 4.

False positives can occur if two distinct code-point sequences happen to
share a byte trigram. The verify pass (`memmem::find` over raw bytes)
catches these — the trigram shard is a candidate authority, the verify is
truth.

Tested by `intersect_unicode_corpus` against a fixture of ≥ 1k chunks each
containing CJK characters.

---

## §11 DoD (provable)

Every DoD row cites a test name and path. Per
[implementation-plan.md §1.4](../implementation-plan.md), a ticket is
`done` only when every DoD item is provable. All 22 rows shipped (56 tests in `quanta-index-lq-trigram`).

| # | Status | DoD item | Evidence |
| --- | --- | --- | --- |
| 1 | ✓ shipped | per-generation trigram shard layout exists (§4.1) | `crates/quanta-index-lq-trigram/tests/build_full_generation.rs::layout_matches_spec` |
| 2 | ✓ shipped | `meta.cbor` codec is hand-rolled (D18) | `crates/quanta-index-lq-trigram/src/trigram/meta.rs::tests::roundtrip_canonical_cbor` + semgrep `rust-no-serde-derive` green at [`tools/ci/semgrep/rules.yml:124`](../../../../tools/ci/semgrep/rules.yml#L124) |
| 3 | ✓ shipped | varint posting encoder round-trips | `crates/quanta-index-lq-trigram/src/trigram/postings.rs::tests::varint_roundtrip_property` |
| 4 | ✓ shipped | FST keying is sorted, miss returns None | `crates/quanta-index-lq-trigram/src/trigram/fst.rs::tests::fst_keys_are_sorted`, `…fst_lookup_miss_returns_none` |
| 5 | ✓ shipped | builder emits `MARKER_OK` last, atomic | `crates/quanta-index-lq-trigram/tests/build_full_generation.rs::builds_marker_ok_last` |
| 6 | ✓ shipped | builder rejects non-UTF-8 chunk with typed error | `crates/quanta-index-lq-trigram/tests/build_full_generation.rs::builder_rejects_non_utf8_chunk` |
| 7 | ✓ shipped | input shorter than `n` short-circuits | `crates/quanta-index-lq-trigram/src/trigram/intersect.rs::tests::short_input_short_circuits` |
| 8 | ✓ shipped | intersect matches naive scan on 1k-doc fixture | `…intersect_matches_naive_scan` |
| 9 | ✓ shipped | candidate cap surfaces `PLAN_LIMIT_EXCEEDED` | `…intersect_respects_candidate_cap` |
| 10 | ✓ shipped | trigram count cap surfaces `PLAN_LIMIT_EXCEEDED` | `…query_respects_trigram_count_cap` |
| 11 | ✓ shipped | planner routes `RawString` leaf to trigram shard | `crates/quanta-index-core/src/domains/query/inbound.rs::tests::rawstring_plan_routes_trigram` |
| 12 | ✓ shipped | verify pass rejects trigram false positives | `…rawstring_verify_rejects_non_substring` |
| 13 | ✓ shipped | regex prefilter extracts required literals | `crates/quanta-index-lq-trigram/src/trigram/regex_prefilter.rs::tests::prefilter_extracts_required_literals` |
| 14 | ✓ shipped | regex prefilter falls to verify on pure wildcards | `…prefilter_falls_to_verify_for_pure_wildcards` |
| 15 | ✓ shipped | byte-trigram Unicode handling tested | `…intersect_unicode_corpus` |
| 16 | ✓ shipped | fault-injection atomicity property | `…manifest_marker_atomicity_property` |
| 17 | ✓ shipped | disk size stays within `1.2 ×` raw bytes | `…trigram_disk_size_within_1_2x_raw_bytes` |
| 18 | ✓ shipped | conformance rows green: `UC-LEX-04`, `UC-LEX-05`, `UC-LEX-06`, `UC-LEX-21` | `cargo test -p quanta-index-contract --test lq_conformance` |
| 19 | ✓ shipped | criterion bench compiles & runs | `cargo bench -p quanta-index-lq-trigram --bench lex_02_trigram_bench` |
| 20 | ✓ shipped | clippy `-D warnings`, `cargo fmt --check`, `cargo deny`, semgrep green | wave-exit CI |
| 21 | ✓ shipped | OpenTelemetry span `lq.exec.trigram` emits with §4.5 attributes | `crates/quanta-index-lq-trigram/tests/observability.rs::tests::emits_lq_exec_trigram_span` |
| 22 | ✓ shipped | metric `lex.trigram.intersect_count{outcome=plan_limit_exceeded}` increments on cap miss | `…metric_increments_on_cap_miss` |

---

## §12 Open questions

| Q-ID | Question | Blocking | Forcing function |
| --- | --- | --- | --- |
| LEX-02-Q1 | N-gram width `n` per-deployment policy: do we allow `n=4`/`n=5` in Phase 1 or pin to `n=3` only? | Wave-2 entry | resolve before §4.2 knob lands |
| LEX-02-Q2 | Regex literal extraction crate: `regex-syntax::hir::literal::Extractor` (stable) vs `aho-corasick`-based custom extractor (more granular) — which yields the best prefilter precision? | Step 8 of §5 | bench `prefilter_extracts_required_literals` decides |
| LEX-02-Q3 | Per-trigram posting cap interplay with `count:all` (§Q-FS-7 in [implementation-plan.md §11](../implementation-plan.md)) — when the trigram shard caps before the user's `count:all` cap, which limit fires first? Default answer: trigram cap fires first; `PLAN_LIMIT_EXCEEDED` carries `dimension=trigram-candidate-set`. | Wave-3 entry (LEX-05 owns `count:all`) | confirm in LEX-05 spec |
| LEX-02-Q4 | Shared trigram dictionary across files within a generation vs per-file: shared (this ticket's default) gives smaller dictionary; per-file gives finer invalidation. | Wave-3 entry (LEX-04 incremental) | decide before incremental write path lands |
| LEX-02-Q5 | G-CONTROL-LOC (see [implementation-plan.md §2.3a](../implementation-plan.md)) — where does the manifest entry for trigram sibling get registered? | Wave-2 entry | resolve before this ticket starts |
| LEX-02-Q6 | The user-supplied GOAL referenced UC-LEX-09 / UC-LEX-10 as the canonical "raw substring" / "regex prefilter" rows; the current corpus places these on UC-LEX-04 / UC-LEX-05 / UC-LEX-06. Is the corpus renumbering an expected sibling change? | Wave-2 exit gate | usecase.md sync; not blocking ticket implementation, only conformance binding |

---

## §13 References

- [rfc.md](../rfc.md) — parent
  - § LQ Family, § Non-Negotiable Invariants §10 (memory caps), § Atomicity
    contract, § Monotonicity rules, § Error Code Taxonomy
- [feature-scope.md](../feature-scope.md)
  - §1.1.1 (pattern leaves), §6.1 (Core-1.0 authority chain), §7 (scale targets)
- [usecase.md](../usecase.md)
  - §2 A. Lexical content (UC-LEX-04, UC-LEX-05, UC-LEX-06, UC-LEX-21);
    §4 (AC-05, AC-06)
- [dsl.md](../dsl.md)
  - §3.3 (RawString backend requirement), §3.4 (regex dialect), §12 (error
    taxonomy), §13 (limits and budgets), §16 (DSL invariants)
- [implementation-plan.md](../implementation-plan.md)
  - §2.4 (lexical adapter current state), §4.3 (Wave 2), §5.7 (LEX-03 DoD
    sibling reference), §8 (test strategy), §9 (observability), §11
    (G-CONTROL-LOC)
- [CLAUDE.md](../../../../CLAUDE.md)
  - § Agent change posture (breaking-first), § Build hygiene (D18 — no
    serde derives)
- [AGENTS.md](../../../../AGENTS.md)
  - shared rule catalog
- [`crates/quanta-index-lexical/src/lib.rs`](../../../../crates/quanta-index-lexical/src/lib.rs)
  — current adapter scaffold (returns `NotImplemented`)
- [`tools/ci/semgrep/rules.yml:124`](../../../../tools/ci/semgrep/rules.yml#L124)
  — `rust-no-serde-derive` rule
- [docs/ssot/producer-handoff.md](../../../ssot/producer-handoff.md) — producer handoff SSOT
- [INDEX.md](INDEX.md) — ticket index (downstream-migration follow-up tracked under §3.6)

---

> End of `LEX-02.md`.
