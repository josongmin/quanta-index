use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, BundleMode, GenerationId, LqDirectiveSet, LqExpr,
    LqFilterSet, LqOptionSet, LqQuery, ManifestDigest, ManifestGeneration, PreparedBundleOutbox,
    PublishedGenerationSet, PublishedSearchGenerationActivateRequest, RepoId, RevisionId,
};
use quanta_index_core::{ActivationPolicy, BundlePolicy, QueryPolicy};

#[test]
fn bundle_policy_rejects_blank_manifest_path() {
    let result = BundlePolicy::validate_outbox(&PreparedBundleOutbox {
        outbox_id: "outbox-1".into(),
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_digest: ManifestDigest::new("digest"),
        bundle_schema_version: 1,
        prepared_at_ms: 1,
        mode: BundleMode::ServeOnly,
        manifest_ref: BundleArtifactRef {
            relative_path: String::new(),
            encoding: BundleEncoding::Json,
            byte_length: 128,
            content_digest: ManifestDigest::new("digest"),
        },
        base_generation: None,
        changed_artifact_mask: 1,
    });

    assert!(result.is_err(), "unexpected result: {result:?}");
    assert!(
        result
            .err()
            .is_some_and(|error| error.to_string().contains("manifest_ref.relative_path"))
    );
}

#[test]
fn activation_policy_rejects_inactive_generation_request() {
    let result = ActivationPolicy::validate_request(&PublishedSearchGenerationActivateRequest {
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
        lexical_ready: false,
        semantic_ready: true,
        active_at_ms: 42,
    });

    assert!(result.is_err(), "unexpected result: {result:?}");
    assert!(
        result
            .err()
            .is_some_and(|error| error.to_string().contains("lexical_ready"))
    );
}

#[test]
fn query_policy_accepts_non_match_all_queries() {
    let result = QueryPolicy::validate_query(&LqQuery {
        expr: LqExpr::Raw("symbol:BundlePolicy".into()),
        filters: LqFilterSet::default(),
        options: LqOptionSet::default(),
        directives: LqDirectiveSet::default(),
    });
    assert!(result.is_ok(), "unexpected result: {result:?}");
}
