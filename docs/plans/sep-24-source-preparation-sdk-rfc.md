# RFC: Source preparation boundary for the Quanta Index SDK

- Status: **proposed**. No implementation or producer migration is claimed.
- Source snapshot: working-tree code inspected on 2026-09-24; final audit HEAD `f26796896e365a8eb66a516f959c460f792e7506`. Since the first inspection, concurrent commits changed benchmark/proof tooling but not the SDK/contract/lexical/semantic source files used here; those files still have concurrent uncommitted edits. The inspected `semantic_derive.rs` digest was `792f1f1d5516f2ce922ed7dfb70b771ccf90fddb4346363127ebc3ccaaf606d2`; it consumes **only typed semantic sources**, and an empty typed source set is an explicit semantic no-op. Freeze exact source and dirty ownership before implementation.
- Related: [SDK DSL RFC](sep-24-sdk-dsl-rfc.md). Its previous default chunk-text fallback claim was corrected during this review. Do not implement a lexical-only text helper from older copies.

## Decision

Offer two input paths and **one canonical publish path**:

```text
Semantic/code producer ── prepared chunks + typed semantic sources ──┐
                                                                    ├─ SearchCorpusBatch
Plain/Markdown text ───── SDK source preparation ───────────────────┘        │
                                                     typed ingest → validate/build
                                                     → seal/receipt → separate CAS
```

The reusable abstraction is **a prepared search-corpus mutation**, not a universal raw `Document` schema, a daemon plugin, or a second index model. Source preparation is a pure, optional producer-side step. It never performs IPC, embedding, publish or activation. `SearchCorpusBatch` remains the single server-facing authority and idempotency body. Domain-specific parsing and semantic interpretation remain with the producer; the search plane owns vector generation from typed source text.

### Responsibilities

| Owner | Owns | Does not own |
|---|---|---|
| Code/HIR producer | Parse, semantic boundaries, stable owner IDs, cards/graph provenance, source and render digests | Vector generation, index layout |
| SDK preparation module | Deterministic plain/Markdown segmentation, IDs/spans, creation of existing lexical and `DocumentLeaf` semantic source DTOs, local validation | HIR inference, model selection, network retries |
| SDK batch/publish | Canonical mutation body/digest, transport, receipt/response binding | Source parsing, policy interpretation by daemon |
| Search plane | Admission, resource budget, semantic embedding, lexical/vector build, seal, durable receipt and activation proof | Inventing producer semantic meaning |

## Public SDK interface (proposed; not current code)

```rust
// source_prep: small, closed API; all fields private and validated at construction.
pub struct SourceKey { /* canonical path + stable owner ID */ }
impl SourceKey {
    pub fn new(path: &str, owner_id: &str) -> Result<Self, PreparationError>;
}

pub struct TextSource<'a> { /* repo, revision, key, format, borrowed UTF-8 text */ }
impl<'a> TextSource<'a> {
    pub fn plain(repo: RepoId, revision: RevisionId, key: SourceKey, text: &'a str)
        -> Result<Self, PreparationError>;
    pub fn markdown(repo: RepoId, revision: RevisionId, key: SourceKey, text: &'a str)
        -> Result<Self, PreparationError>;
}

pub struct TextPreparationPolicy { /* immutable version + bounded options */ }
impl TextPreparationPolicy {
    pub fn v1() -> Self;
    pub fn max_chunk_bytes(self, bytes: NonZeroUsize) -> Result<Self, PreparationError>;
}

pub fn prepare_text(source: TextSource<'_>, policy: &TextPreparationPolicy)
    -> Result<PreparedText, PreparationError>;
// PreparedText owns BOTH lexical and typed semantic records. It exposes only
// key(), provenance(), and counts; the caller cannot mutate one leg alone.

impl<const SEALED: bool> SearchCorpusBatch<SEALED> {
    pub fn replace_prepared(self, prepared: PreparedText) -> Result<Self, SdkError>;
    pub fn move_prepared(self, old: SourceKey, prepared: PreparedText)
        -> Result<Self, SdkError>;
    pub fn tombstone_prepared(self, old: SourceKey) -> Result<Self, SdkError>;
}
```

`replace_prepared` is for an unchanged scope identity (new document or content update); `move_prepared` handles an old path and/or owner ID without producing a replace-and-tombstone conflict on an unchanged semantic key; `tombstone_prepared` deletes both legs. The batch owns repo, revision, generation and manifest digest. `move_prepared` and `tombstone_prepared` use the batch's repo/revision and the caller's **persisted old key**. None of these methods publishes, embeds or activates.

Typical text producer call:

```rust
let key = SourceKey::new("docs/guide.md", "guide-001")?;
let prepared = prepare_text(
    TextSource::markdown(repo.clone(), revision.clone(), key, markdown)?,
    &TextPreparationPolicy::v1(),
)?;
let batch = SearchCorpusBatch::delta(repo, revision, next_generation, base_generation, manifest_digest)
    .replace_prepared(prepared)?;
let receipt = client.search_corpus().publish(&batch)?;
// Existing explicit CAS activation follows after inspecting the receipt.
```

Delete/rename with old identity:

```rust
let deleted = SearchCorpusBatch::delta(repo.clone(), revision.clone(), next, base, manifest)
    .tombstone_prepared(SourceKey::new("docs/old.md", "guide-001")?)?;
let renamed = SearchCorpusBatch::delta(repo, revision, next, base, manifest)
    .move_prepared(SourceKey::new("docs/old.md", "guide-001")?, prepared_new_path)?;
```

The two batches above are **alternative examples**. `manifest_digest` is producer supplied; this API does not prove that it binds the original source bytes or policy. Build and validate that provenance separately.

The existing direct path remains available for producer-authored chunks/cards:

```rust
// Direct, expert path: existing producer-authored records remain first-class.
let batch = SearchCorpusBatch::replace_generation(repo, revision, generation, manifest)
    .replace_scope(scope, scope_digest, chunks, symbols)
    .replace_semantic_scope(semantic_scope, semantic_digest, sources, memberships);
```

`TextSource` names the repository/revision, path, stable source ID, format and UTF-8 bytes. The policy is an immutable, named preparation recipe. `PreparedText` has private fields and read-only provenance (source digest, policy digest, scope identities, chunk count, semantic-record count). It owns existing typed scope DTOs and can be consumed **once** into a batch. It is an ephemeral SDK value, not a wire/persisted IR. The three prepared-mutation methods reject batch/source repo or revision mismatch, duplicate paths/owner IDs, and conflicting mutations; they do not choose a generation or silently seal. These examples describe a proposed API; they do not currently compile.

The direct path remains valid for code producers. No public `SourcePreparer` trait or runtime registry is needed now: custom producers already implement the contract by supplying chunks and semantic sources. Introduce a trait only if at least two in-repo preparers need shared polymorphic behavior that cannot be expressed by these DTOs.

## Preparation contract

1. V1 supports UTF-8 plain text and Markdown only. Markdown section boundaries are preferred; both formats use the same bounded text splitter for oversized sections. No PDF/HTML/OCR, language-server parsing, automatic summaries, LLM transforms or arbitrary user callbacks.
2. Output **both** `ChunkRecord` lexical records and `SemanticSourceRecordV1` `DocumentLeaf`/`DocumentText` records under one source identity. The semantic text is a deterministic projection of the source, never invented by the daemon. Fill `language` with a validated explicit code and set source role, capability status, authority digest and render-policy digest by a documented mapping. Use lexical `SearchScopeSurface::Chunk` at the canonical path and semantic `(DocumentLeaf, File, stable owner_id)`; do not infer a shared scope key from their different wire types. No implicit chunk-text fallback. Refuse a prepared result with zero lexical or zero semantic records.
3. Preserve exact UTF-8 byte offsets and line spans for lexical chunks. V1 should keep source slices verbatim, including CRLF, and define heading recognition only outside fenced code; split oversized sections at UTF-8 boundaries with no overlap. Specify whether spans are half-open and whether line numbers are one-based against current `ChunkRecord` consumers before freezing goldens. Duplicate headings use ordinal order, not heading text, for record identity. No transform may claim exact spans without a reversible mapping. Semantic source records currently have no byte/line spans and derivation writes zeroes to embedding spans: semantic results can promise a document path and snippet, not an exact original-text jump. Empty/whitespace-only input is a typed refusal; deletion is a separate mutation.
4. Separate **stable scope identity** from content-derived record IDs. Lexical identity is `(Chunk, canonical repo-relative path)`; semantic identity is `(DocumentLeaf, File, stable source owner_id)`. Neither includes revision, content or policy. Bind `source_doc_id` to the stable source identity, and derive record/chunk IDs with separate domain tags plus policy identity, ordinal and content. Require unique path and owner ID per prepared source in a batch. Domain-separated source/policy/scope digests include exact source bytes, format, parser/splitter version and every supported option; `batch_digest` comes from the wire body. The daemon attests **the submitted batch**, not the truth of original source bytes or the caller's manifest provenance. Changed text/policy may replace a whole scope; V1 does not promise unchanged chunk IDs across edits. Current SDK semantic scopes are sorted, but lexical replace/tombstone scopes append in call order: canonicalize those vectors by scope key or explicitly require canonical caller order before claiming equivalent source sets produce the same batch digest.
5. Bound bytes before parsing and bound emitted chunks/sources, total lexical **and** semantic text bytes, and serialized batch size before publish. The current `IngestResourcePolicy` counts lexical and semantic records, but its `text_bytes` counts only semantic embedding text; it does not by itself cap lexical payload bytes. The SDK's `to_wire_batch()` clones scope vectors and lexical dispatch serializes scope clones again, so one-source bounds alone do not cap peak memory across a batch. Set measured defaults below server limits, reject aggregate overflow, and make server-side lexical/encoded-byte admission a prerequisite for claiming a robust untrusted-producer boundary. If that requires changing `quanta-index-core/src/ingest_resource.rs`, do so with negative tests; do not describe current transport as streaming.
6. A failed parse, unsupported format or invalid mapping fails closed. No automatic downgrade from semantic code/card input to plain-text preparation. Plain text is selected explicitly by the caller, never inferred from arbitrary bytes.
7. The first API targets one repository per batch. `ChunkRecord.source_repo_id` has no equivalent in `SemanticSourceRecordV1`; federated semantic provenance requires a separate contract decision, not guessed identity.

### Replacement, deletion and collision rules

- A delta over an existing generation must replace **both** scopes for changed text. A delete must tombstone both stored keys: lexical `(Chunk, old_path)` and semantic `(DocumentLeaf, File, old_owner_id)`. Derive tombstones from persisted prior identity or explicit caller-supplied old identity; a new `TextSource` alone cannot recover renamed paths or owner IDs. For a rename with the **same** owner ID, tombstone the old lexical path, replace the new lexical path and replace the existing semantic key with records bearing the new path; do **not** tombstone and replace that semantic key in one batch. If owner ID also changes, tombstone the old semantic key and replace the new one. Replace-generation starts a fresh generation and has no prior-scope tombstones to inherit.
- Do not silently change owner ID when policy or content changes. If an external producer chooses a new owner ID, it must supply the old semantic key for deletion. Reject two prepared sources with the same path or owner key, including conflicts with direct-path mutations. No automatic `clear_surface`: it is broader than one source.
- The lexical adapter currently deletes Tantivy documents by **path only** on replace/tombstone, regardless of `SearchScopeSurface`. Therefore a helper cannot safely share one path with independently owned lexical scopes. V1 requires exclusive lexical ownership per path or a separately validated adapter/contract change that makes surface isolation real. Test replacement against co-located code/symbol records; otherwise block onboarding that overlaps them.

## Extension rule and configuration

`TextPreparationPolicy::v1()` identifies the exact splitting behavior and bounded size defaults; changing behavior creates a new policy identity. Caller options may choose **supported** input format and an explicit bounded size within this recipe; all effective options must be in the policy digest. They cannot override daemon embedding model, index build recipe, query profile or resource admission. The producer must construct manifest provenance from the actual prepared source/policy digests and scope set; `replace_prepared` cannot prove that an arbitrary caller-supplied `manifest_digest` did so. Provide a digest recipe and independent golden, and document this guarantee as producer-owned until a separate attested manifest contract exists.

Add a new built-in preparer only for a real source format with an owner and fixture corpus. Its output must be expressible as existing typed records or else first define the missing contract. New ranking semantics, semantic corpus kinds, model options and server profiles remain separate server changes. Avoid universal `Document<Value>`, dynamic plugins and source-format inference.

## Integration and ownership

| Area | First implementation |
|---|---|
| `crates/quanta-index-sdk/src/source_prep.rs` (new), `src/lib.rs` | Pure `TextSource`, policy, preparation result and validation; optional dependency only if Markdown parsing needs it. |
| `crates/quanta-index-sdk/src/lexical.rs` | `replace_prepared`, `move_prepared` and `tombstone_prepared` lower to existing paired scope mutations; aggregate collision/size preflight and canonical lexical mutation order. No second publisher. |
| `crates/quanta-index-contract/src/{channel/records.rs,ipc/semantic_source.rs,ipc/ingest.rs}` | Reuse current DTOs; touch only on a demonstrated missing representation. |
| `crates/quanta-index-core/src/ingest_resource.rs`, `crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs` | Decide and implement server admission for total lexical/encoded payload bytes before claiming untrusted-producer safety; verify refusal before durable intent. |
| `crates/quanta-index-lexical/src/adapter.rs` | First prove exclusive path ownership; change path-only deletion only if the concrete producer must coexist with another lexical surface at the same path. |
| `crates/quanta-index-search-plane/src/semantic_derive.rs` | No expected change for typed-source preparation. Verify through the real daemon. |
| Semantica producer | Keep its current semantic boundaries and direct path; compile/integration proof, no forced conversion to the text helper. |

Gate implementation on one concrete plain/Markdown producer use case. First build the pure preparer and independently specified golden records; then lower into the existing batch and prove two-lane ingestion. Required scenarios: Unicode/CRLF/fenced headings/long sections, duplicate headings, empty/unsupported input, stable replay, permutation of two source insertion orders, changed policy/source, delta replace, paired deletion, rename with stable and changed owner IDs, duplicate owner/path, wrong repo/revision, lexical and encoded-byte resource refusal, and co-located lexical-surface collision. Run actual lexical and typed semantic queries against the activated generation with fixed expected paths/snippets; verify deleted terms disappear, including after restart. Exercise publish receipt, replay and CAS conflict separately. Compare final wire body/digest against an independently assembled batch, and measure peak RSS/latency for worst admitted source and multi-source batches. Source-bound SDK/daemon/producer integration rails and clean-checkout E2E receipts remain required; these are planned, not current proof.

## External precedents and limits

- [LlamaIndex ingestion pipeline](https://developers.llamaindex.ai/python/framework/module_guides/loading/ingestion_pipeline/) separates document transformation from index insertion. Quanta adopts the boundary, not an open transformation chain or global settings.
- [Unstructured chunking](https://docs.unstructured.io/api-reference/partition/chunking) distinguishes basic and section-aware strategies. Quanta V1 needs one deterministic text policy with explicit format handling, not its broader strategy set.
- [Qdrant point idempotence](https://qdrant.tech/documentation/manage-data/points/) illustrates why stable IDs/replays matter; Quanta's actual idempotency authority remains its canonical batch digest and durable receipt.

These sources inform design only. Current-code observations are static. New API, end-to-end behavior and performance are **NOT_RUN**.
