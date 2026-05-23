use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, BundleMode, GenerationId, ManifestDigest,
    ManifestGeneration, PreparedBundleOutbox, PublishedGenerationSet, RepoId, RevisionId,
};

#[must_use]
pub fn sample_outbox() -> PreparedBundleOutbox {
    PreparedBundleOutbox {
        outbox_id: "outbox-1".into(),
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_digest: ManifestDigest::new("digest"),
        bundle_schema_version: 1,
        prepared_at_ms: 1,
        mode: BundleMode::ServeOnly,
        manifest_ref: BundleArtifactRef {
            relative_path: "bundle/manifest.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 128,
            content_digest: ManifestDigest::new("digest"),
        },
        base_generation: None,
        changed_artifact_mask: 1,
    }
}

#[must_use]
pub fn sample_generation() -> PublishedGenerationSet {
    PublishedGenerationSet {
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_generation: ManifestGeneration::new(7),
        lexical_generation: GenerationId::new(10),
        symbol_generation: GenerationId::new(11),
        structural_generation: None,
        history_generation: None,
        semantic_generation: Some(GenerationId::new(12)),
        metadata_generation: Some(GenerationId::new(13)),
    }
}
