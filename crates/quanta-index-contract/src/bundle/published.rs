use crate::{
    BundleArtifactRef, BundleMode, GenerationId, ManifestDigest, ManifestGeneration, RepoId,
    RevisionId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedBundleOutbox {
    pub outbox_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_digest: ManifestDigest,
    pub bundle_schema_version: u32,
    pub prepared_at_ms: u64,
    pub mode: BundleMode,
    pub manifest_ref: BundleArtifactRef,
    pub base_generation: Option<ManifestGeneration>,
    pub changed_artifact_mask: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedGenerationSet {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub lexical_generation: GenerationId,
    pub symbol_generation: GenerationId,
    pub structural_generation: Option<GenerationId>,
    pub history_generation: Option<GenerationId>,
    pub semantic_generation: Option<GenerationId>,
    pub metadata_generation: Option<GenerationId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleManifest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub bundle_schema_version: u32,
    pub lexical_chunk_rows: BundleArtifactRef,
    pub symbol_rows: BundleArtifactRef,
    pub metadata_rows: Option<BundleArtifactRef>,
    pub graph_rows: Option<BundleArtifactRef>,
    pub embedding_input_views: Option<BundleArtifactRef>,
    pub embedding_records: Option<BundleArtifactRef>,
    pub mutation_delta: Option<BundleArtifactRef>,
}
