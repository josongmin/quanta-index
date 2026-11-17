//! Native benchmark fixtures and optional Legacy runtime/DSL rails.
//!
//! Native indexing, exact ANN oracles and source fixtures are available by
//! default. The explicit `legacy-bench` feature adds `E2eRuntime` and the
//! maintained Legacy matrix generators. Daemon code uses this crate only as
//! a development dependency; it is absent from the shipping daemon graph.

mod fixture_support;
#[cfg(feature = "legacy-bench")]
mod harness;
#[path = "harness/source_publication.rs"]
mod source_publication;

#[cfg(feature = "legacy-bench")]
pub mod ambiguity;
pub mod ann;
pub mod artifact;
#[cfg(feature = "legacy-bench")]
pub mod bench_support;
#[cfg(feature = "legacy-bench")]
pub mod concurrency;
pub mod history_fixture;
#[path = "scale/native_index.rs"]
pub mod native_index;
/// Shared bounded arrival scheduler for harness load adapters.
pub mod offered_load;
#[cfg(feature = "legacy-bench")]
pub mod ops;
#[cfg(feature = "legacy-bench")]
pub mod relevance;
#[cfg(feature = "legacy-bench")]
pub mod scale;
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
pub mod scenarios;
#[cfg(feature = "legacy-bench")]
pub mod snippet;
#[path = "scale/source_fixture.rs"]
pub mod source_fixture;
#[cfg(feature = "legacy-bench")]
pub mod tail;
#[cfg(feature = "legacy-bench")]
pub mod ui;

pub use fixture_support::{
    private_tempdir, semantic_source_scopes_for_chunk_records, stamped_ingest_request,
};
#[cfg(feature = "legacy-bench")]
pub use harness::*;
#[cfg(feature = "legacy-bench")]
pub use scale_failure::ScaleAdmissionRefusal;
pub use scale_failure::{ScaleAdmissionLimit, ScaleStageError};
pub use source_publication::{SourceCorpusFixture, fixture_source_scope_v1};

#[cfg(test)]
mod source_fixture_boundary_tests {
    #[cfg(feature = "legacy-bench")]
    use super::scale;
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

    #[cfg(feature = "legacy-bench")]
    #[test]
    fn legacy_scale_facade_shares_canonical_fixture() {
        let tier = source_fixture::ScaleTier::Small;
        assert_eq!(
            scale::generate_scoped_corpus(tier, 42).unwrap(),
            source_fixture::generate_scoped_corpus(tier, 42).unwrap()
        );
    }
}
