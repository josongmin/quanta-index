use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, BundleMode, GenerationId, ManifestDigest,
    ManifestGeneration, PreparedBundleOutbox, PublishedGenerationSet,
    PublishedSearchBundleDeltaApplyRequest, PublishedSearchBundleManifest, RepoId, RevisionId,
    SearchBundleMutationDelta, SearchBundleMutationOp,
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

#[must_use]
pub fn sample_artifact_ref(kind: &str, byte_length: u64, digest: &str) -> BundleArtifactRef {
    BundleArtifactRef {
        relative_path: format!("bundle/{kind}.arrow"),
        encoding: BundleEncoding::ArrowIpc,
        byte_length,
        content_digest: ManifestDigest::new(digest),
    }
}

#[must_use]
pub fn sample_manifest() -> PublishedSearchBundleManifest {
    let generation = sample_generation();
    PublishedSearchBundleManifest {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        manifest_generation: generation.manifest_generation,
        bundle_schema_version: 1,
        lexical_chunk_rows: sample_artifact_ref("chunk_rows", 1024, "sha256:chunk"),
        symbol_rows: sample_artifact_ref("symbol_rows", 2048, "sha256:symbol"),
        metadata_rows: Some(sample_artifact_ref("metadata_rows", 512, "sha256:metadata")),
        graph_rows: Some(sample_artifact_ref("graph_rows", 256, "sha256:graph")),
        embedding_input_views: None,
        embedding_records: Some(sample_artifact_ref(
            "embedding_records",
            4096,
            "sha256:embedding",
        )),
        mutation_delta: None,
    }
}

#[must_use]
pub fn sample_manifest_required_only() -> PublishedSearchBundleManifest {
    let generation = sample_generation();
    PublishedSearchBundleManifest {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        manifest_generation: generation.manifest_generation,
        bundle_schema_version: 1,
        lexical_chunk_rows: sample_artifact_ref("chunk_rows", 1024, "sha256:chunk"),
        symbol_rows: sample_artifact_ref("symbol_rows", 2048, "sha256:symbol"),
        metadata_rows: None,
        graph_rows: None,
        embedding_input_views: None,
        embedding_records: None,
        mutation_delta: None,
    }
}

#[must_use]
pub fn sample_manifest_all_optionals() -> PublishedSearchBundleManifest {
    let generation = sample_generation();
    PublishedSearchBundleManifest {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        manifest_generation: generation.manifest_generation,
        bundle_schema_version: 1,
        lexical_chunk_rows: sample_artifact_ref("chunk_rows", 1024, "sha256:chunk"),
        symbol_rows: sample_artifact_ref("symbol_rows", 2048, "sha256:symbol"),
        metadata_rows: Some(sample_artifact_ref("metadata_rows", 512, "sha256:metadata")),
        graph_rows: Some(sample_artifact_ref("graph_rows", 256, "sha256:graph")),
        embedding_input_views: Some(sample_artifact_ref(
            "embedding_input_views",
            128,
            "sha256:embedding-input",
        )),
        embedding_records: Some(sample_artifact_ref(
            "embedding_records",
            4096,
            "sha256:embedding",
        )),
        mutation_delta: None,
    }
}

#[must_use]
pub fn sample_delta_request() -> PublishedSearchBundleDeltaApplyRequest {
    delta_request_for(&sample_generation())
}

/// Build a delta-apply request targeting the supplied generation.
///
/// Used by tests that need to apply against a specific non-active generation
/// (delta governance: applying against the currently-active generation is
/// rejected).
#[must_use]
pub fn delta_request_for(
    generation: &PublishedGenerationSet,
) -> PublishedSearchBundleDeltaApplyRequest {
    PublishedSearchBundleDeltaApplyRequest {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        generation: generation.clone(),
        delta: SearchBundleMutationDelta {
            repo_id: generation.repo_id.clone(),
            revision_id: generation.revision_id.clone(),
            manifest_generation: generation.manifest_generation,
            operations: vec![
                SearchBundleMutationOp::UpsertChunk {
                    chunk_identity: "chunk-1".into(),
                    text_digest: "tdig-1".into(),
                },
                SearchBundleMutationOp::DeleteSymbol {
                    symbol_id: "sym-1".into(),
                },
            ],
        },
    }
}
