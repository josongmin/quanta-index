use crate::{ManifestGeneration, RepoId, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchBundleMutationDelta {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub operations: Vec<SearchBundleMutationOp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchBundleMutationOp {
    UpsertChunk {
        chunk_identity: String,
        text_digest: String,
    },
    DeleteChunk {
        chunk_identity: String,
    },
    UpsertSymbol {
        symbol_id: String,
        symbol_digest: String,
    },
    DeleteSymbol {
        symbol_id: String,
    },
    UpsertEmbedding {
        entity_id: String,
        input_digest: String,
    },
    DeleteEmbedding {
        entity_id: String,
    },
}
