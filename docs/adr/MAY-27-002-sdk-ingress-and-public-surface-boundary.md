# MAY-27-002 — SDK Ingress and Public Surface Boundary

Status: `Accepted`

Decided: 2026-05-27

Consolidated: 2026-09-27

Implementation note (2026-10-08): optional source preparation is integrated in
local main `433a9363`, using the existing `SearchCorpusBatch` primitive. The
breaking `publish_outcome`, `activate_published`, and `SdkError::AfterPublish`
contract remains on `codex/sdk-preparation-final`; paired Semantica Runtime
acceptance is pending. Main retains `ActivationAfterPublish`; CLI and benchmark
consumers now handle it (`c75668c7`). See
[OCT-04-003](OCT-04-003-source-preparation-sdk.md#remaining-coupled-integration).

Source programs: May-24 lexical/indexing closeout and May-25 SDK/ingest IPC
cutover

## Context

The first search-plane and lexical programs mixed public API design, transport
cutover, implementation order and dated local receipts. They also described a
retired channel-based runtime and several vendor-specific intermediate
architectures. Those packets are useful history but cannot remain current API
or runtime authority.

The durable boundary is the external SDK and contract relationship. Query,
publication and lifecycle behavior must converge on that boundary without
allowing a second raw transport path to become public authority.

## Decision

### Public entrypoint

The supported external Rust path uses `quanta-index-sdk`. The contract crate
owns closed DTO and wire shapes; it does not own transport, storage or
application policy. The SDK owns typed request construction, transport
invocation and exact request/response identity binding.

Raw IPC remains an adapter and negative-test seam rather than a second supported
application client. Contract and IPC types are public Rust types, so this is an
ownership rule enforced by dependency/API review and public-surface checks, not
a claim that construction is technically impossible.

### Namespace and authority split

One client exposes typed namespaces for lexical, semantic, symbol, history,
runtime, structural, RepoMap, generation/control and observability operations.
Each namespace lowers to the canonical contract route. Namespace convenience
does not create a parallel engine or a second source of query semantics.

Producer-owned authority families publish through family-specific typed
batches. Lexical source records use the search-corpus batch; history, runtime
and structural records retain their distinct identities and readiness rules.
One family cannot reuse lexical readiness or silently encode its records as
lexical payload.

### Publish lifecycle

Batch construction is deterministic and validates repository, revision,
generation, scope and mutation conflicts before transport. The canonical body
digest binds the bytes submitted to the daemon. The SDK validates that the
returned receipt and response correspond to the request it sent.

Publish, seal, activation and query visibility are separate service operations
and claims. Activation uses an explicit compare-and-swap lifecycle.

Main `433a9363` exposes `SearchCorpusNamespace::publish` and
`publish_and_activate`; verified post-publication failures retain original
evidence in `ActivationAfterPublish`. These calls are not atomic.

**Isolated breaking candidate:**
`SearchCorpusNamespace::publish_outcome(&batch)` returns the validated
`SearchCorpusPublishOutcome` with the original
`SourcePublicationBinding` and committed `BatchPublishReceipt`. This matters for
source-event replay, where the original publication target can differ from
the submitted batch target. `PublishedBatchEvidence::from(&outcome)` packages
that identity and receipt; callers use
`SearchCorpusNamespace::activate_published(&evidence, expected_active)` for a
separate, explicit activation CAS; activation requires a sealed receipt.
Invalid evidence or expected-head input is refused before control I/O, while
control failures retain the checked evidence in `AfterPublish`. The SDK also retains the convenience
`publish_and_activate` path. Both paths perform separate service operations,
not an atomic transaction.

After a verified publish, a later observation or activation error carries the
original evidence and typed cause in `SdkError::AfterPublish`, with stage
`PublishedBatchFailureStage::Observation` or `Activation`. The caller can
recover it through `SdkError::published_evidence()`,
`published_receipt()`, or `published_publication()` and reconcile the
original publication before retrying. A transport or response failure before
a validated publish outcome remains uncertain and does not manufacture a
receipt. `GenerationNamespace` and `ControlClient` do not own this
search-corpus activation CAS; the method is on `SearchCorpusNamespace`.

### Retired paths

The former channel crate and channel-specific public publishers are not part of
the live workspace or SDK dependency surface. Search-plane application code
owns ingest dispatch and durable lifecycle coordination. Historical channel,
in-memory HNSW and early Lance adapter descriptions do not override current
source or the later semantic-generation ADR.

## Consequences

- Adding a public operation requires a contract route, SDK namespace or
  existing namespace extension, exact binding validation and an owner proof.
- Generic namespace plumbing stays internal. Public APIs remain family-shaped.
- Current support is read from SDK exports, contract route inventories and
  current runtime code, not from archived ticket status.
- Verified post-publish observation or activation failure retains original
  typed evidence. Pre-outcome uncertainty still requires event-identity and
  active-head reconciliation; neither public activation path is atomic.
- Cross-repository producer adoption, installed-process behavior and release
  qualification remain separate from this accepted repo-local boundary.

## Related decisions

- [JUN-02-001](JUN-02-001-search-dsl-authority-and-runtime-contract.md)
- [MAY-31-001](MAY-31-001-lancedb-semantic-generation-authority.md)
- [SEP-21-001](SEP-21-001-canonical-identity-and-digest-domains.md)
- [SEP-21-002](SEP-21-002-durable-authority-and-operation-lifecycle.md)
- [SEP-21-003](SEP-21-003-read-view-continuation-and-provider-policy.md)
- [Source-preparation SDK](OCT-04-003-source-preparation-sdk.md)

## Historical record

The absorbed packets and exact consolidation boundary are listed in
[the completed-plan archive](../ARCHIVE-INDEX.md#historical-record-recovery).
