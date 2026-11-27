//! Native benchmark fixtures and pinned external Legacy reference rails.
//!
//! Native indexing, exact ANN oracles and source fixtures are available by
//! default. Daemon code uses this crate only as a development dependency; it
//! is absent from the shipping daemon graph.

mod bench_fixture;
mod e2e_error;
mod fixture_support;
#[cfg(unix)]
pub mod legacy_reference;
#[path = "harness/source_publication.rs"]
mod source_publication;

pub mod ambiguity;
pub mod ann;
pub mod artifact;
pub mod bench_support;
pub mod concurrency;
pub mod history_fixture;
#[path = "scale/native_index.rs"]
pub mod native_index;
/// Shared bounded arrival scheduler for harness load adapters.
pub mod offered_load;
pub mod open_loop_accounting;
pub mod ops;
pub mod product_xfs;
pub mod relevance;
#[path = "scale/diagnostic_span.rs"]
mod scale_diagnostic_span;
#[path = "scale/disk_census.rs"]
pub mod scale_disk_census;
#[path = "scale/failure.rs"]
mod scale_failure;
#[path = "scale/provenance.rs"]
pub mod scale_provenance;
#[path = "scale/refusal.rs"]
pub mod scale_refusal;
#[path = "scale/runtime_policy.rs"]
pub mod scale_runtime_policy;
pub mod scenarios;
pub mod snippet;
mod snippet_fixture;
#[path = "scale/source_fixture.rs"]
pub mod source_fixture;
pub mod tail;
pub mod ui;

pub use bench_fixture::{
    E2eRuntimeCatalogSpec, E2eRuntimeChangedSpec, E2eRuntimeEdgeSpec, E2eRuntimeFacetSpec,
    E2eRuntimeSnapshotSpec, structural_function_tree_record,
};
pub use bench_fixture::{
    RUNTIME_CATALOG_FILES, RUNTIME_DIRTY_EPOCH_MS, STRUCTURAL_TREE_CONTENT,
    STRUCTURAL_TREE_IDENTIFIER, STRUCTURAL_TREE_PATH, runtime_catalog_batch,
    runtime_catalog_fixture_spec,
};
pub use e2e_error::E2eErrorCode;
pub use fixture_support::{
    private_tempdir, semantic_source_scopes_for_chunk_records, stamped_ingest_request,
};
pub use scale_failure::{ScaleAdmissionLimit, ScaleAdmissionRefusal, ScaleStageError};
pub use source_publication::{SourceCorpusFixture, fixture_source_scope_v1};

#[cfg(test)]
mod source_fixture_boundary_tests {
    use super::source_fixture;

    #[test]
    fn canonical_fixture_keeps_source_contract() {
        let tier = source_fixture::ScaleTier::Small;
        assert_eq!(source_fixture::params_for(tier).total_files(), 16);
        let canonical = source_fixture::generate_scoped_corpus(tier, 42).unwrap();
        assert_eq!(canonical[0].source_repo_id, "repo0");
        assert_eq!(canonical[0].repo_relative_path, "src/file_0.rs");
        assert!(canonical[0].content.contains("scale_needle_token"));
        assert!(canonical[0].content.contains("scalereponeedle000"));
        assert!(canonical[0].content.contains("scalefileneedle000file00000"));
    }
}
