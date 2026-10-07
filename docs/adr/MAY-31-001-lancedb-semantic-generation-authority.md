# MAY-31-001 — LanceDB Semantic Generation Authority

Status: `Accepted`

Decided: 2026-05-30

Amended: 2026-05-31 — three hardening rounds retained the backend decision and
closed verified lifecycle, integrity and restart findings.

Amended: 2026-09-21 — the state-root V2 cutover removed the boot-time legacy
importer. Legacy journal markers now cause typed refusal before adapters open;
conversion is offline-only under SEP-21-004.

Consolidated: 2026-09-27

## Context

Semantic state was historically reconstructed from `journal.cbor` into an
in-memory index during boot. An initial replacement implemented the desired
generation layout with an in-house CBOR and HNSW store, but that contradicted
the explicit LanceDB adoption decision and created a second backend design.

## Decision

### Backend and boundary

The durable semantic backend is the real `lancedb` crate. The semantic adapter
owns the async runtime and contains the async-to-sync bridge behind the existing
synchronous ports. Vendor types and layout knowledge do not escape the adapter.

Supply-chain and build-cost exceptions are explicit, named and scoped. They do
not relax deny-by-default policy for unrelated dependencies.

### Producer and derivation boundary

Producers publish typed semantic-source replacement/tombstone scopes, including
explicit `RawCodeFallback` when chosen. `semantic_derive.rs` consumes only those
sources and derives vectors in bounded windows under the search-owned model
contract. `ChunkRecord.text` remains lexical text; it is not an implicit dense
fallback. The retired derive-mode selector, chunk-text/subscriber worker design
and public producer-authored vector ingress are not alternative live paths.

Semantic and hybrid callers send text. Query embedding and admitted corpus
vectors must agree on the selected generation's provider/model/revision/dimension
contract. Typed semantic ownership is not a universal `embedding_id == chunk_id`
rule; owner/corpus identity follows the typed source contract. An empty semantic
source list is a no-op, not independent evidence of complete producer coverage.
Typed producer coverage, prior-state binding and paired restart proof remain
separate cross-repository acceptance.

The superseded May worker/job-state/render-policy structs were proposals;
removing their tickets does not assert those proposed APIs were implemented.
Open provider, observability and proof work remains in the semantic residual
ledger. Model or dimension changes require coordinated generation rebuild.

### Durable authority

Semantic data is generation-scoped under the semantic state root. Each sealed
generation binds repository, revision, generation, model and manifest identity,
row-set integrity and lifecycle markers. Build occurs in staging; serving opens
only a validated sealed generation.

The adapter opens the persisted LanceDB dataset and vector index directly.
Boot-time replay is not a steady-state serving mechanism. Missing, unsealed,
schema-incompatible, model-incompatible or integrity-invalid state fails
closed and cannot become readiness.

### Activation and readiness

Generation selection stays outside vendor code. Runtime readiness is seeded
from validated persisted generations and remains scoped by the canonical
generation identity. A cached handle does not create authority beyond its
sealed manifest and active generation binding.

### Legacy migration

`state_root/semantic/journal.cbor`, old migration markers and mixed legacy/current
roots are unsupported by the live daemon. Their presence is detected before
adapter open and returns `STATE_ROOT_FORMAT_UNSUPPORTED` without mutation. The
hot path contains no legacy decoder or importer.

Any required conversion is an offline state-root operation governed by
[SEP-21-004](SEP-21-004-process-supervision-state-cutover-and-proof.md): preserve
the old root, build and scrub a current staging root, publish its manifest last
and cut over atomically. The legacy journal is never a second live writer,
serve authority or silent fallback.

The current workflow implements no legacy converter. Retained legacy data needs
an explicit producer rebuild and retention decision under
[SEP-27-005](SEP-27-005-catalog-recovery-supervision-and-proof-custody.md).

### Query model and score authority

Query embedding and the selected generation must agree on admitted model,
revision and dimensions. Preserve typed provider-unavailable/model-mismatch
ordering and refuse invalid input; no dimension-only identity guess. Reject
nonfinite native cosine distance before ranked-score construction. Hybrid
execution and contribution truth follow
[SEP-26-001](SEP-26-001-retrieval-query-publication-and-result-proof.md), not the
historical lexical-scoped/RRF description from the June seam audit.

### Text, vector and cache identity

The daemon's `QueryEmbedderAdapter` forwards the supplied text to the same
provider instance used for corpus derivation. There is no shared semantic-text
lowercase, camel-case split or identifier-folding step. Provider tokenization
remains provider-owned; lexical query planning is a separate policy.

`EmbeddingCacheIdentityV1` binds model ID, revision, dimension and vector
normalization policy, then keys entries by the exact UTF-8 input bytes.
`FooBar`, `foobar`, `foo_bar` and `foo bar` therefore have separate cache
identities. This does not require separate output vectors: the explicit
development hash provider preserves token case, splits non-alphanumeric
separators, and can embed `foo_bar` and `foo bar` identically. Real-provider
equivalence is not inferred from that development tokenizer.

The shared `L2UnitEmbeddingProvider` normalizes vectors before OpenAI/PotionCode
consumers; the development hash provider normalizes its own result. The selected
sealed semantic manifest supplies the serving vector-normalization contract.
The common semantic/hybrid query gate preserves provider-error ordering, checks
model/revision, then validates the vector against the opened searcher. Cache
hits also validate dimension, finite components and the declared normalization;
invalid hits are evicted and recomputed. Model/revision changes cannot reuse the
old identity's entries.

Owners: [daemon composition](../../crates/quanta-index-searchd/src/app/runtime.rs),
[shared normalization](../../crates/quanta-index-core/src/domains/semantic/service.rs),
[cache identity and regressions](../../crates/quanta-index-embed/src/cache.rs),
[query gate](../../crates/quanta-index-search-plane/src/query_dispatcher/routes/semantic.rs)
and [serving manifest](../../crates/quanta-index-semantic/src/manifest.rs).
Existing cache rotation/invalid-hit/mixed-batch and query model/provider-refusal
tests are implemented. The SDK frontdoor already routes semantic and hybrid
queries; `runtime_fast_suite` also checks search-owned text derivation with the
explicit hash profile. Daemon composition tests couple query/corpus model
identity and exercise cache-enabled/disabled branches. Metrics E2E checks
provider/cache counters at boot; it does not issue a cache-hit query.

The real-provider paraphrase test already exists as the ignored
`end_to_end::openai_semantic_paraphrase_outranks_unrelated_v1` in
`runtime_fast_suite`. It requires the actual API key and provider grant; its
selected execution is a proof/input gap, not missing provider test code.

The [public cache matrix](../../crates/quanta-index-searchd-runtime/src/public_cache_tests.rs)
uses SDK/UDS, production daemon composition, the persistent cache and real
catalog/lexical/semantic adapters. The daemon-owned `test-provider-transport`
feature substitutes only HTTP I/O with fixed orthogonal vectors; production
assembly has no command or environment selector for this test seam.
The matrix sends all four exact text forms through cold/warm queries, checks
provider inputs and hit/miss counters, and compares exact ranked IDs and score
bits with an independently uncached run. Reopening the same root must retain
cache entries. Changing either model ID or revision must use a separate
namespace; rotating back must recover only the original namespace. Old pins
retain model-mismatch refusal and provider-error precedence. Identical vectors
for `foo_bar` and `foo bar` do not merge their cache entries.

`VERIFIED` on 2026-10-07: `./scripts/cargow --lane test-daemon-lane test
-p quanta-index-searchd-runtime --lib public_cache_tests --locked -- --nocapture`
passed both tests (0 failures, 0 ignored; final rerun 4.29s). This is the focused public-path
matrix with HTTP replaced and complete runtime teardown/reassembly.
The existing `quanta-index-searchd --lib build_semantic_embedders` selector also
passed all three composition regressions (4.08s), covering query/corpus model
coupling and enabled/disabled cache-directory branches.
Runtime `clippy -p quanta-index-searchd-runtime --lib --tests --all-features
--locked -- -D warnings` also passed, including the library test configuration.
The fixed transport does not qualify real-provider relevance, live egress or
the upstream producer path; the ignored live-provider rail remains separate.

## Rejected alternatives

- in-memory HNSW plus LanceDB as an optional sidecar;
- permanent dual-write to journal and LanceDB;
- boot-time legacy import;
- silent journal replay after persisted-open failure;
- vendor-specific logic in core, contract or dispatcher layers;
- the superseded in-house CBOR/HNSW backend.

## Consequences

- A LanceDB upgrade is a storage-contract and supply-chain change.
- Migration, direct-open and restart evidence remain distinct from query
  quality or ANN performance qualification.
- Exact current layout and manifest fields remain code-owned by the semantic
  adapter; this ADR owns the architectural choice and authority boundary.

## Historical record

The LDB-00 through LDB-E2E-01 packet is indexed in
[the completed-plan archive](../ARCHIVE-INDEX.md#historical-record-recovery).
