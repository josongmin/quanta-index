//! Typed ingest IPC contract (QI-ING-01).
//!
//! Producer / search-plane integration surface for batch publishes. The
//! producer sends a [`SearchPlaneIngestIpcRequestEnvelope`] over UDS
//! `ingest.sock`; searchd's ingest dispatcher applies the typed batch through
//! direct authority stores / builders and forwards repo-map bundles to the
//! repo-map owner. The producer never opens an internal transport adapter
//! directly.
//!
//! Wire shape: every DTO in this module implements `Serialize` /
//! `Deserialize` manually. Workspace bans proc-macro serde derives
//! (CLAUDE.md "no proc-macro derives for serialization"); the manual impls
//! keep cold-build cost bounded and make the wire shape auditable in review.
//! Unknown fields and duplicate fields fail-closed; missing required fields
//! raise `missing_field` rather than synthesising defaults.
//!
//! Wire tagging: enum variants use serde's native externally-tagged shape
//! (`{"Upsert": {...}}`) via `serialize_newtype_variant` /
//! `deserialize_enum`. This is format-agnostic — works under CBOR, JSON, or
//! any other serde transport — and reads naturally without needing a
//! format-specific intermediate value type.
//!
//! Existing query / control envelopes in `split.rs` use adjacent tagging via
//! `#[serde(tag = "kind", content = "payload")]`. They predate this module
//! and live on a separate migration timeline (see workspace rule
//! `check-rust-derive-allowlist.py`); the wire format difference between the two is
//! intentional for the new ingest surface.

mod corpus_wire;
pub use corpus_wire::{
    BatchIngestMode, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchCorpusSurfaceMutationConflictV1, SearchCorpusTombstoneScope, SearchScopeKey,
    SearchScopeSurface,
};

mod semantic_wire;
pub use semantic_wire::{
    EmbeddingDistanceMetric, EmbeddingModelContract, EmbeddingNormalization, SemanticIngestBatch,
    SemanticReplaceScope, SemanticTombstoneScope,
};

mod history_wire;
pub use history_wire::{
    FileContributorEntry, FileContributorIdentityEntry, FileContributorIngestBatch,
    FileOwnershipEntry, FileOwnershipIngestBatch, HistoryDiffHunkUpsert, HistoryIngestBatch,
    HistoryRefDelete, HistoryRefMutation, HistoryRefUpsert, HistoryTagDelete, HistoryTagMutation,
    HistoryTagUpsert, RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch, RepoDescriptionEntry,
    RepoDescriptionIngestBatch, RepoMetaEntry, RepoMetaIngestBatch, RepoTopicEntry,
    RepoTopicIngestBatch,
};

mod runtime_wire;
pub use runtime_wire::{
    DirtyDelete, DirtyIngestBatch, DirtyMutation, RuntimeCatalogIngestBatch, RuntimeChangedRecord,
    RuntimeDocFacetRecord, RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord,
    StructuralIngestBatch, StructuralReplaceScope, StructuralTombstoneScope, StructuralTreeRecord,
};

mod payload_digest;
pub use payload_digest::source_event_payload_sha256;

mod source_upload;
pub use source_upload::{
    SOURCE_PUBLICATION_UPLOAD_DEFAULT_BYTES, SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
    SOURCE_PUBLICATION_UPLOAD_PART_BYTES, SourcePublicationUploadAck,
    SourcePublicationUploadCommit, SourcePublicationUploadIdentity, SourcePublicationUploadPart,
};

mod validation;
pub use validation::{SearchCorpusBatchShapeErrorV1, validate_lexical_file_mutations_v1};

mod envelope;
pub use envelope::*;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "serde roundtrip tests use assert_eq! for compact proof"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixed nonempty fixture vectors intentionally fail the test if their shape changes"
)]
mod tests;
