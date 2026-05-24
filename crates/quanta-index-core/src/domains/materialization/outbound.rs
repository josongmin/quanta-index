use quanta_index_contract::{PublishedGenerationSet, PublishedSearchBundleManifest};

use crate::CoreError;

/// Bundle artifact bytes for lexical index builds.
///
/// Bytes are already read from the producer-published payload and digest-verified
/// by `searchd::app::materialize` (D17). Build ports receive these slices
/// instead of artifact-store handles; adapters never touch the filesystem for
/// bundle inputs.
pub struct LexicalBuildInput<'a> {
    pub chunk_rows: &'a [u8],
    pub symbol_rows: &'a [u8],
}

/// Semantic build payload.
///
/// `embedding_records` is `None` when the manifest has no embedding artifact
/// (lexical-only generation).
pub struct SemanticBuildInput<'a> {
    pub embedding_records: Option<&'a [u8]>,
}

/// Driven port: open lexical indexes for a generation.
pub trait SearchPlaneLexicalIndexStorePort {
    fn open_lexical_store(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError>;
}

/// Driven port: open vector indexes for a generation.
pub trait SearchPlaneVectorIndexStorePort {
    fn open_vector_store(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError>;
}

/// Driven port: open metadata stores for a generation.
pub trait SearchPlaneMetadataStorePort {
    fn open_metadata_store(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError>;
}

/// Driven port: build lexical indexes from a manifest + caller-verified bytes.
pub trait SearchPlaneLexicalIndexBuildPort {
    fn build_lexical_index(
        &mut self,
        manifest: &PublishedSearchBundleManifest,
        input: LexicalBuildInput<'_>,
    ) -> Result<(), CoreError>;
}

/// Driven port: build semantic indexes from a manifest + caller-verified bytes.
pub trait SearchPlaneSemanticIndexBuildPort {
    fn build_semantic_index(
        &mut self,
        manifest: &PublishedSearchBundleManifest,
        input: SemanticBuildInput<'_>,
    ) -> Result<(), CoreError>;
}
