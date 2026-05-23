use crate::{ManifestGeneration, RepoId, RepoRelativePath, RevisionId};

#[derive(Clone, Debug, PartialEq)]
pub struct LexicalCandidate {
    pub candidate_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub repo_relative_path: RepoRelativePath,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f32,
    pub snippet: String,
}
