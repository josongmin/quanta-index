# RFC: Extensible repository-format SDK for Quanta Index

- Status: **DRAFT — discussion only; not approved for implementation**. No implementation or integration proof.
- Product goal: Quanta Index is the repository search plane for code, documentation, configuration, structured files, and eventually supported binary/media assets. Adding a source-format adapter should not require rewriting the public ingest lifecycle. A format is *supported* only when its adapter, engine capability, query behavior, and verification are present; an arbitrary file is never silently treated as fully indexed.
- Source inspection: Quanta HEAD `688238a8f98e6e4bf1fb13605f970a2aecb65232` on 2026-09-24, with concurrent dirty docs. `ChunkRecord` and `SemanticSourceRecordV1` carry text; `semantic_derive.rs` uses `TextEmbeddingProvider`; the SDK currently publishes prepared `SearchCorpusBatch` records. This is a text-capable engine, not a native multimodal ingest contract. Re-freeze before implementation.
- Related: [current SDK DSL RFC](sep-24-sdk-dsl-rfc.md), [text adapter subdesign](sep-24-source-preparation-sdk-rfc.md), [execution plan](sep-24-source-preparation-execution-plan.md).

## 1. Layers and ownership

```text
repository/source discovery (caller or optional scanner)
    → format adapter (Semantica HIR, SDK Markdown, external PDF/diagram, ...)
    → validated PreparedSource with typed index contributions
    → SourceBatch lifecycle (replace/delete/move, digest, publish, receipt, CAS)
    → engine capability admission and typed materializers
    → lexical/semantic/other query routes
```

The **stable SDK boundary is source identity + prepared contributions + lifecycle**, not one universal chunker. A format adapter interprets its source and owns structure, offsets, derived-text provenance, and deterministic IDs. The engine validates and indexes only the typed contributions it supports. Semantica remains the owner of HIR-aware code/card preparation and may continue to use the direct `SearchCorpusBatch` path; no conversion through a generic Markdown parser.

`SearchCorpusBatch` remains the **current engine primitive** for lexical plus typed-text semantic material. It is not the public promise that every future modality will fit its current fields. `SourceBatch` is a proposed high-level SDK facade: in V1 it lowers once to one `SearchCorpusBatch`, uses the existing publish/receipt/CAS path, and adds no RPC or server-side registry. Native image/audio/vector capabilities require an explicit typed contract, materializer, query semantics, and atomic-generation decision before `SourceBatch` can accept them. Do not smuggle binary bytes into `SemanticSourceRecordV1.text` or label OCR text as native image retrieval.

## 2. Public SDK shape (illustrative)

```rust
pub struct SourceKey { /* repo-relative path + stable source owner ID */ }
pub struct SourceContext { /* repo, revision, source key, media type, provenance */ }
pub struct PreparedSource { /* private typed contributions + source/recipe digests + scope keys */ }
pub struct PriorSourceManifest { /* persisted old scope keys + adapter/recipe identity */ }

pub trait SourceAdapter<Input> {
    fn prepare(&self, ctx: SourceContext, input: Input)
        -> Result<PreparedSource, PrepareError>;
}

// Built-in adapters are ordinary implementations; no dynamic daemon registry.
let prepared = MarkdownAdapter::v1().prepare(ctx, markdown)?;
let batch = SourceBatch::delta(repo, revision, generation, base, manifest_digest)
    .replace(prepared)?;
let receipt = client.sources().publish(&batch)?;
// Activation stays explicit; retain batch + receipt on CAS refusal/uncertainty.
```

The trait gives third parties a compile-time plugin seam with their own `Input` types: borrowed text, parsed AST, stream/asset handle, or producer-owned HIR. The generic parameter permits a borrowed `TextSource<'a>` without requiring every adapter input to be owned or materialized as bytes. It does **not** impose a universal raw-byte loader or force an adapter to execute inside `searchd`. The opaque `PreparedSource` has validated constructors/builders for the currently supported **typed** contributions; callers cannot mutate its provenance or drop one leg of a paired text output. New constructors may be added when a real engine capability exists. Avoid public exhaustive enums, `serde_json::Value`/`Any` payloads, global format registries, and a generic transformation DAG. `media_type` is descriptive/routing input, not proof that bytes were parsed as that format.

`SourceBatch::{replace,delete,move}` consumes `PreparedSource` or explicit `PriorSourceManifest` from the prior generation. The prior manifest records old lexical and semantic scope keys, adapter/recipe identity, and source/content digests; it is **not** the daemon's `manifest_digest` and cannot by itself attest raw bytes. A move computes old-key tombstones and new-key replacements without tombstone/replace conflict on an unchanged semantic owner. All contributions for one source have one lifecycle outcome or admission fails. For V1, this can be implemented as a checked wrapper over `SearchCorpusBatch`; no new persisted IR is required.

### What remains explicit

- Adapter/version and recipe policy; no extension-based silent fallback. A host may route by path/media type, but the selected adapter and effective recipe are recorded.
- Supported output capability: lexical text, typed-text semantic, and later explicitly added native modalities. `UnsupportedCapability` is a typed refusal; a metadata-only or OCR-derived-text mode must be selected and labeled as reduced coverage.
- Stable source identity, content/recipe digests, exact source spans only when reversible, source-derived child IDs, and old scope keys for delete/rename. No claim that a vector result points to exact source bytes when the semantic DTO lacks spans.
- Resource budgets for source bytes, output count/bytes, and aggregate batch; IPC already has a 16 MiB frame cap. Host/SDK prepares before publish, engine independently admits before durable intent.

## 3. Supported-format progression

| Stage | Adapter output | Engine change |
|---|---|---|
| Current | Semantica code/HIR → lexical chunks + typed semantic cards; external producers may emit these DTOs directly. | Existing text lexical and text embedding paths. |
| First SDK cut | Plain text/Markdown and a custom adapter proof → `PreparedSource` with lexical chunks + `DocumentLeaf` typed semantic text. | Path/surface mutation authority and lexical byte admission hardening; no new modality. |
| Next format families | Config/JSON/YAML, HTML/PDF-extracted text, notebooks, and diagrams **only with owned adapters, fixtures, and explicit representations**. | Reuse existing typed-text path when accurate; add contract fields only for demonstrated provenance/query gaps. |
| Native media | Image/audio/video binary or non-text vectors. | New typed contribution, provider/model contract, storage/query route, generation/receipt semantics, migration and cost policy. SDK facade may stay; engine contract cannot. |

The table is a **capability roadmap**, not a claim that those formats already work. Containers such as PDFs and notebooks may yield multiple derived parts with page/cell provenance; image OCR/captions are text derivatives. Native media indexing is separate from extracting text. Binary, oversized, encrypted, Git LFS pointer, symlink, or unsupported input gets an explicit result rather than a false searchable-success receipt.

## 4. Integration rules and tradeoffs

1. Freeze the lexical mutation unit before exposing the high-level facade. Current validation keys lexical scopes by `(surface,path)` while Tantivy deletion and several side states retire by `path`. The [execution plan](sep-24-source-preparation-execution-plan.md) prefers path ownership **only if** a Semantica same-path chunk/symbol/clear/delta fixture proves it preserves semantics. Otherwise implement full scope isolation, not an SDK-specific exception.
2. V1 `PreparedSource` accepts only typed contributions representable by the current engine. The first adapter must emit both lexical and typed semantic text when it claims both capabilities; it must never ask the daemon to infer semantic text from chunks. Third-party adapters may intentionally emit lexical-only with explicit declared coverage.
3. Keep format adapters independently packaged where dependencies are heavy. The core SDK may define the small trait/result/batch facade; Markdown parser and future PDF/OCR dependencies belong to adapter packages or opt-in features, not unconditional core/daemon dependencies.
4. A new format using existing contributions requires adapter fixtures, deterministic source/recipe IDs, replace/delete/move/replay, and real query checks. A new modality or searchable corpus kind requires contract/daemon/query capability proof. The SDK type shape may remain stable, but **semantic API additions and engine migrations are legitimate**; promise extensibility, not zero future contract changes.
5. Do not generalize the query DSL into a schema-free query language. Text lexical/semantic routes remain typed. A native modality gets its own typed query path or an explicitly validated cross-modal route with model compatibility and ranking evidence.

## 5. Independent acceptance

- One Semantica HIR fixture, one Markdown source, and one external adapter with a different input type all converge to an independently specified current `SearchCorpusBatch` body/digest. Prove cross-adapter coexistence in one generation, collisions, replay, delete/move, receipt binding, CAS conflict, restart, and real lexical/semantic search.
- Negative cases: wrong repo/revision, duplicate source/scope, missing old keys, unsupported capability, malformed provenance, parser error, over-budget input, partial output, and changed adapter recipe. No partial commit or silent downgrade.
- Source-bound owner-local tests first; then clean-source SDK/daemon/Semantica E2E with SHA, dirty state, config/embedding identity, command, raw result, and artifact digest. Benchmark build/publish peak RSS and latency for admitted near-limit and many-source batches. Nothing in this RFC is execution proof.

## Reference boundary

[LlamaIndex ingestion pipeline](https://docs.llamaindex.ai/en/stable/module_guides/loading/ingestion_pipeline/) separates loading/transformation from indexing; [Haystack DocumentWriter](https://docs.haystack.deepset.ai/docs/documentwriter) requires format converters before writing; [Unstructured chunking](https://docs.unstructured.io/api-reference/partition/chunking) distinguishes partitioned elements from chunks. These support the **adapter-before-index** boundary. Quanta deliberately keeps typed authority and generation lifecycle instead of adopting their broad runtime pipeline surface.
