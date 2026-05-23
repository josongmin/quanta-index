use quanta_index_contract::{
    BundleArtifactRef, PublishedGenerationSet, PublishedSearchBundleManifest,
};

use crate::CoreError;

pub trait PublishedSearchArtifactStorePort {
    fn load_manifest(
        &self,
        reference: &BundleArtifactRef,
    ) -> Result<PublishedSearchBundleManifest, CoreError>;
}

pub trait SearchPlaneLexicalIndexStorePort {
    fn open_lexical_store(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError>;
}

pub trait SearchPlaneVectorIndexStorePort {
    fn open_vector_store(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError>;
}

pub trait SearchPlaneMetadataStorePort {
    fn open_metadata_store(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError>;
}

pub trait SearchPlaneEmbeddingProviderPort {
    fn ensure_embeddings(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError>;
}

pub trait SearchPlaneLexicalIndexBuildPort {
    fn build_lexical_index(
        &mut self,
        manifest: &PublishedSearchBundleManifest,
    ) -> Result<(), CoreError>;
}

pub trait SearchPlaneSemanticIndexBuildPort {
    fn build_semantic_index(
        &mut self,
        manifest: &PublishedSearchBundleManifest,
    ) -> Result<(), CoreError>;
}
