//! Property-based invariants for core policy validators.
//!
//! Express the safety-relevant rules of `BundlePolicy`, `ActivationPolicy`,
//! and `QueryPolicy` as universally quantified properties and let `proptest`
//! search for counter-examples.

use proptest::prelude::*;
use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, BundleMode, GenerationId, LqDirectiveSet, LqExpr,
    LqFilterSet, LqOptionSet, LqQuery, ManifestDigest, ManifestGeneration, PreparedBundleOutbox,
    PublishedGenerationSet, PublishedSearchGenerationActivateRequest, RepoId, RevisionId,
};
use quanta_index_core::{ActivationPolicy, BundlePolicy, CoreError, QueryPolicy};

fn outbox(
    outbox_id: String,
    relative_path: String,
    bundle_schema_version: u32,
) -> PreparedBundleOutbox {
    PreparedBundleOutbox {
        outbox_id,
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_digest: ManifestDigest::new("digest"),
        bundle_schema_version,
        prepared_at_ms: 1,
        mode: BundleMode::ServeOnly,
        manifest_ref: BundleArtifactRef {
            relative_path,
            encoding: BundleEncoding::Json,
            byte_length: 128,
            content_digest: ManifestDigest::new("digest"),
        },
        base_generation: None,
        changed_artifact_mask: 1,
    }
}

fn activate_request(
    lexical_ready: bool,
    semantic_ready: bool,
) -> PublishedSearchGenerationActivateRequest {
    PublishedSearchGenerationActivateRequest {
        generation: PublishedGenerationSet {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(7),
            lexical_generation: GenerationId::new(10),
            symbol_generation: GenerationId::new(11),
            structural_generation: None,
            history_generation: None,
            semantic_generation: None,
            metadata_generation: None,
        },
        lexical_ready,
        semantic_ready,
        active_at_ms: 42,
    }
}

fn lq_query(expr: LqExpr) -> LqQuery {
    LqQuery {
        expr,
        filters: LqFilterSet::default(),
        options: LqOptionSet::default(),
        directives: LqDirectiveSet::default(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn bundle_policy_rejects_whitespace_only_outbox_id(
        whitespace in r"[ \t\n]*",
        relative_path in r"bundle/[a-z0-9_/.-]{1,32}",
        version in 1u32..=64u32,
    ) {
        let result = BundlePolicy::validate_outbox(&outbox(whitespace, relative_path, version));
        prop_assert!(matches!(result, Err(CoreError::InvalidContract(_))));
    }

    #[test]
    fn bundle_policy_rejects_whitespace_only_relative_path(
        outbox_id in r"[A-Za-z0-9_-]{1,32}",
        whitespace in r"[ \t\n]*",
        version in 1u32..=64u32,
    ) {
        let result = BundlePolicy::validate_outbox(&outbox(outbox_id, whitespace, version));
        prop_assert!(matches!(result, Err(CoreError::InvalidContract(_))));
    }

    #[test]
    fn bundle_policy_rejects_zero_schema_version(
        outbox_id in r"[A-Za-z0-9_-]{1,32}",
        relative_path in r"bundle/[a-z0-9_/.-]{1,32}",
    ) {
        let result = BundlePolicy::validate_outbox(&outbox(outbox_id, relative_path, 0));
        prop_assert!(matches!(result, Err(CoreError::InvalidContract(_))));
    }

    #[test]
    fn bundle_policy_accepts_well_formed_outbox(
        outbox_id in r"[A-Za-z0-9_-]{1,32}",
        relative_path in r"bundle/[a-z0-9_/.-]{1,32}",
        version in 1u32..=64u32,
    ) {
        let result = BundlePolicy::validate_outbox(&outbox(outbox_id, relative_path, version));
        prop_assert!(result.is_ok(), "unexpected reject: {result:?}");
    }

    #[test]
    fn activation_policy_requires_lexical_ready(semantic_ready in any::<bool>()) {
        let result = ActivationPolicy::validate_request(&activate_request(false, semantic_ready));
        prop_assert!(matches!(result, Err(CoreError::NotReady(_))));
    }

    #[test]
    fn activation_policy_requires_semantic_ready(lexical_ready in any::<bool>()) {
        let result = ActivationPolicy::validate_request(&activate_request(lexical_ready, false));
        prop_assert!(matches!(result, Err(CoreError::NotReady(_))));
    }

    #[test]
    fn activation_policy_accepts_fully_ready(_unused in 0u8..=0u8) {
        let result = ActivationPolicy::validate_request(&activate_request(true, true));
        prop_assert!(result.is_ok());
    }

    #[test]
    fn query_policy_rejects_match_all(_unused in 0u8..=0u8) {
        let result = QueryPolicy::validate_query(&lq_query(LqExpr::MatchAll));
        prop_assert!(matches!(result, Err(CoreError::InvalidContract(_))));
    }

    #[test]
    fn query_policy_accepts_raw_expressions(
        raw in r"[A-Za-z][A-Za-z0-9_]{0,32}:[A-Za-z0-9_]{1,32}",
    ) {
        let result = QueryPolicy::validate_query(&lq_query(LqExpr::Raw(raw)));
        prop_assert!(result.is_ok());
    }
}
