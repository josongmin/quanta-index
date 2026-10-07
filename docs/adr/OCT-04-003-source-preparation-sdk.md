# OCT-04-003 — Repository-format preparation SDK (draft)

Status: **Proposed / DRAFT — discussion only; not approved for implementation**.
No new public API, format support, wire contract, or engine capability is
approved by this document.

Source audit: `main@b2599e70b262d707771a69440f0e9a0f0c587e2b` on
2026-10-08. Relevant SDK and contract source was read; implementation and
integration tests for this proposal are `NOT_RUN`.
The Sep-24 SDK DSL, source-preparation and repository-format drafts are
recoverable from Git history. Accepted
[MAY-27-002](MAY-27-002-sdk-ingress-and-public-surface-boundary.md) owns the
current public ingress boundary.

## Current boundary

- `quanta-index-sdk/src/lexical.rs` publishes prepared `SearchCorpusBatch`
  values and exposes `publish_and_activate` with a separate CAS result.
  Lexical replacement supplies source bytes/chunks/symbols; semantic derivation
  requires typed `SemanticSourceRecordV1`. No public `SourceAdapter`,
  `PreparedSource` or `SourceBatch` abstraction exists.
- `ChunkRecord` carries text and source spans, but does not alone produce
  semantic index material; semantic derivation consumes typed sources.
  `SearchCorpusReplaceScope` carries complete source bytes independently of
  potentially partial or overlapping chunks.
- SDK `ClientProfile` selects transport availability, not an indexing recipe.
- Contract admission now validates lexical file mutations by source-file key
  (`contract/src/ipc/ingest/validation.rs`), including duplicate replacement
  and record/source-path consistency. The old draft's request to first tighten
  this contract is stale; its Semantica producer assumptions are unverified at
  this source revision.

## Open decision

Quanta Index is the repository search plane for source formats whose prepared
representation, engine capability, query behavior and tests are present. The
stable extension seam should be source identity and typed prepared
contributions, not one universal chunker. A compile-time preparation adapter
may lower plain text/Markdown and producer-defined formats to the existing
typed batch and receipt/CAS flow. It must declare lexical-only or typed-text
semantic coverage explicitly and preserve the direct batch path. Native
image/audio/video or other non-text retrieval requires a separate typed engine,
storage, generation and query contract; OCR/captions are labeled derived text,
not native media indexing. Do not add a daemon plugin registry or a second
durable publication protocol merely to accept another source format.

## Proposed layering

```text
caller discovery / source ownership
  |-- direct producer: HIR, chunks, symbols, typed semantic cards (Semantica)
  `-- optional adapter: Markdown, plain text, producer-defined format
                -> validated prepared text contributions
  -> one canonical SearchCorpusBatch (replace / tombstone / clear)
  -> SDK publish -> validated sealed receipt -> explicit CAS activation
  -> existing search-plane text materializers and query routes
```

The prepared, typed batch is the engine-facing primitive for **current text
capabilities**. Semantica can continue to own HIR-aware chunking and publish a
`SearchCorpusBatch` directly. Optional adapters run in the producer process or
library, not in `searchd`. A new text format can be added at that boundary only
if its output fits the existing typed contracts. A new modality or ranking
behavior needs an explicit contract and engine change.

### Illustrative SDK shape (new types and methods do not exist yet)

```rust
pub struct SourceKey { /* stable producer-owned ID + repo-relative path */ }
pub struct SourceContext { /* repo/revision, SourceKey, declared format */ }
pub struct PreparationProfile { /* adapter + recipe identity and budgets */ }
pub struct PreparedSource { /* private, validated typed contributions */ }
pub struct PriorSourceManifest { /* prior scope keys and source/recipe identity */ }

pub trait SourceAdapter<Input> {
    fn prepare(
        &self,
        input: Input,
        context: &SourceContext,
        profile: &PreparationProfile,
    ) -> Result<PreparedSource, PrepareError>;
}

// Direct producer: existing SDK batch remains the text-ingest primitive.
let direct = SearchCorpusBatch::delta(repo, revision, next, base, manifest_digest)
    .replace_scope(coverage, source_bytes, chunks, symbols)
    .replace_semantic_scope(scope, scope_digest, cards, memberships);

// Optional SDK-only facade, lowered once to the same canonical batch.
let prepared = MarkdownAdapter::new().prepare(markdown, &context, &profile)?;
let batch = SourceBatch::delta(repo, revision, next, base, manifest_digest)
    .replace(prepared)?
    .lower_to_search_corpus_batch()?;
let receipt = client.search_corpus().publish(&batch)?;
// Proposed: retain batch + receipt on CAS refusal or unknown transport result.
let activation = client.search_corpus()
    .activate_published(&batch, &receipt, expected_active);
```

`PreparedSource` should be opaque except for checked constructors and read-only
inspection. It contains only currently supported typed contributions; avoid a
universal `Document`, `Any`, `serde_json::Value`, or generic payload bag.
`SourceBatch` is an SDK convenience, not a second durable IR or wire protocol.
Omit it if checked constructors on `SearchCorpusBatch` cover the same use case
more simply. Preparation itself performs no daemon I/O.

## Contracts needed before acceptance

- **Identity and provenance:** bind repository, revision, stable source key,
  original bytes/hash, chosen adapter and recipe digest, output scope keys and
  declared coverage. Extension/MIME routing is a hint, not parse proof. The
  current producer policy/digest fields must not be assumed to attest a recipe
  until its producer-side construction is defined and tested.
- **Capabilities:** preparation declares lexical text, typed semantic text, or
  both. Unsupported capability is a typed refusal. Empty semantic output is
  explicit lexical-only coverage, never implied embedding of chunk text.
- **Offsets and IDs:** claim exact source byte/line spans only when correct;
  transformed text needs provenance and a reversible map before claiming exact
  source spans. IDs, order, scope digest and batch digest must be deterministic
  for equal source plus recipe. Bound input, output and aggregate batch size;
  use actual IPC framing and daemon admission instead of a guessed limit.
- **Mutations:** replace/delete/move require old scope keys from a persisted
  prior-source manifest; a move cannot infer them from the new path. Reconcile
  a source's lexical and semantic contributions as one intended mutation or
  reject it. Keep duplicate replace, tombstone and whole-surface clear
  conflicts fail-closed.
- **Scope ownership:** first prove co-located chunk/symbol and semantic rows
  across replace, clear, delta, replay and restart. Resolve `(surface, path)`
  versus path-only deletion at its canonical owner before promising a
  source-level operation.
- **Publication and recovery:** publish returns a validated sealed receipt;
  activation is explicit CAS. A conflict or unknown transport result must
  leave the batch and receipt available for status reconciliation. The current
  `publish_and_activate` result does not preserve a successful publish receipt
  after later error. Do not add automatic CAS or paid-semantic retries.

## Profile and extension policy

`PreparationProfile` is a producer-side, explicit and hashable recipe:
adapter/version, extraction or chunking choices, emitted capabilities and
resource budgets. A recipe change changes prepared-content identity. It is
separate from SDK `ClientProfile` (socket access), daemon effective config,
semantic model identity and query profile. Expose only choices that alter
output or operational admission; adapter-private parser details need no public
knob. Format adapters may accept their own typed options but resolve them into
one recorded effective recipe.

First prove one maintained plain-text/Markdown adapter and a second independent
adapter fixture with a different input type. Code/HIR chunking remains
producer-owned. Avoid format-specific branching in daemon ingest.

## Implementation blast radius and acceptance order

| Owner | Proposed work |
|---|---|
| `crates/quanta-index-sdk/src/lexical.rs`, new preparation module, `src/lib.rs` | Checked preparation and one lowering to `SearchCorpusBatch`; preserve direct path. Design explicit receipt-preserving activation against daemon proof. |
| `crates/quanta-index-contract/src/{channel/records.rs,source_coverage.rs,ipc/ingest/validation.rs}` | Prefer no wire change; inspect scope ownership and policy identity. Change only for a demonstrated missing invariant. |
| `crates/quanta-index-search-plane/src/`, lexical and semantic adapters | No parser registry. Fix only demonstrated materialization or deletion ownership defects; retain sealed-generation identity. |
| Semantica producer and second fixture producer | Keep direct HIR path, prove compatibility, migrate one-call activation only with a coordinated recovery contract. |

Acceptance sequence: freeze source and producer call sites; make direct and
prepared fixtures with independent expected spans, IDs, coverage and digests;
prove replace/delete/move/clear, changed recipe, conflict refusal, deterministic
replay, size refusal and parse failure; run SDK-to-daemon publish, receipt,
CAS conflict, restart and query for lexical-only and typed-semantic cases;
then run Semantica and second-adapter producer scenarios. Format support,
performance, cross-repository adoption and release qualification remain
`NOT_RUN` until executed.

## Unresolved design choices

1. Does `SourceBatch` add enough value over checked `SearchCorpusBatch`
   constructors to justify another public type?
2. Who persists the prior-source manifest, and how are old scope keys
   recovered after crash or move?
3. Which scope/deletion invariants need repair before source-level replace is
   safe for co-located surfaces?
4. Which typed semantic source can a text adapter emit under the current
   closed corpus-kind vocabulary, and what coverage applies if it cannot?

These questions and the illustrative API are not implementation approval.
