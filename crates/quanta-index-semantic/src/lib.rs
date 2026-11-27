#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Native ANN uses Lance Dataset directly. The default
//! `legacy-semantic-adapter` feature includes the persisted Semantic adapter.
//!
//! Implements the scope-streamed semantic build port and the open port from
//! `quanta-index-core::domains::semantic` against a Lance dataset under
//! `{state_root}/indexes/semantic/generation-v1-{sha256(repo, revision)}/g{generation}/`. The
//! adapter owns a `tokio::runtime::Runtime` and bridges async Lance calls to
//! the sync port surface via `Runtime::block_on` — this adapter *is* the
//! deliberate async↔sync seam (the `disallowed_methods` rule against
//! `block_on` is honored by a single, narrowly-scoped `#[expect]` on the
//! crate-private `run_blocking` helper — the only `block_on` call site in
//! the crate; both the build and query paths funnel through it). The build
//! drives its scope source on the calling thread, outside the runtime, and
//! enters the seam once per storage step, so a source that embeds over a
//! blocking transport never runs inside the async context.
//!
//! A sealed generation is built once (Lance dataset + ANN index + scope
//! manifest + READY/SEALED markers) and opened directly from durable state at
//! query time. There is no boot-time replay. Vendor / file-layout knowledge
//! stays inside this crate; the public surface is the two ports, the
//! metrics source over the dense lanes' tallies (W5 phase 3), plus
//! `inventory_persisted_generations` for readiness seeding at the
//! composition root, and [`semantic_state_root`] so the composition root does
//! not hardcode the layout root.
//!
//! See `docs/adr/MAY-31-001-lancedb-semantic-generation-authority.md` for the
//! backend and migration authority. The May-28 plan remains historical detail.

mod budget;
mod build_diagnostic;
mod errors;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod layout;
pub mod legacy;
pub(crate) use legacy::{codec, generation_contract, manifest};
pub mod native;
mod semantic_ingest_fixtures_v1;
mod vector_index;

pub(crate) const COLUMN_VECTOR: &str = "vector";

#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod build;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod control_file;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod dataset_promotion;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod durable_write;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod membership_integrity;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod persisted;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod qualified_fs;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod seal_tallies;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod sealed_manifest;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod search;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod semantic_file_authority;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod semantic_row_integrity_v1;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
mod sql;

#[cfg(all(debug_assertions, any(test, feature = "legacy-semantic-adapter")))]
pub use persisted::test_support;
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
pub use persisted::{
    PersistedSemanticGeneration, SemanticAdapter, SemanticGenerationInventoryV1,
    ValidatedPersistedSemanticGenerationV2, inventory_persisted_generations,
    validate_persisted_generation_v2,
};
#[cfg(feature = "proof")]
pub use persisted::{ann_proof, proof};
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
pub(crate) use persisted::{
    inventory_persisted_generations_with_control_lifetime,
    scrub_candidate_from_quarantined_with_control_lifetime,
};
#[cfg(feature = "qualified-memory-uri-candidate")]
pub use qualified_fs::{AdmittedMemorySealedStore, AdmittedMemoryWorkStore};
#[cfg(any(test, feature = "legacy-semantic-adapter"))]
pub use semantic_file_authority::{
    SemanticFileAdmissionFactory, SemanticFileMove, SemanticGenerationFileIo,
    SemanticMaintenanceFileFactory, SemanticStagingPart, SemanticStagingTargetMode,
    SemanticStagingUpload, SemanticWorkListEntry,
};

#[cfg(any(test, feature = "legacy-semantic-adapter"))]
pub use semantic_ingest_fixtures_v1::build_resident_batch_v1;
pub use semantic_ingest_fixtures_v1::{
    embedding_record_v1, ingest_batch_v1, legacy_chunk_embedding_record_v1, model_contract_v1,
    sealed_replace_batch_v1, search_scope_v1, tombstone_scope_v1,
    tombstone_scope_with_semantic_owner_v1,
};

use std::path::{Path, PathBuf};

/// Shared bridge from synchronous Native ANN and persisted generation ports
/// to their asynchronous Lance operations.
#[expect(
    clippy::disallowed_methods,
    reason = "the semantic adapter is the deliberate async↔sync seam between the sync port surface and the async Lance crate; the entire crate funnels through this single helper"
)]
pub(crate) fn run_blocking<F: core::future::Future>(
    runtime: &tokio::runtime::Runtime,
    future: F,
) -> F::Output {
    runtime.block_on(future)
}

/// The semantic sub-root within an overall daemon state root.
///
/// This crate owns the semantic on-disk layout, including where its generation
/// tree is rooted, so the composition root derives the path here rather than
/// hardcoding `indexes/semantic` itself.
#[must_use]
pub fn semantic_state_root(state_root: &Path) -> PathBuf {
    state_root.join("indexes").join("semantic")
}
