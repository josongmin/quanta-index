use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapEntryDtoV1, RepoMapSnapshotMetaV1, RevisionId,
};

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapEntryV1 {
    pub subject_identity: String,
    pub subject_kind: String,
    pub owner_path: String,
    pub score: f32,
    pub rank: u32,
}

impl RepoMapEntryV1 {
    #[must_use]
    pub fn to_dto(&self) -> RepoMapEntryDtoV1 {
        RepoMapEntryDtoV1 {
            subject_identity: self.subject_identity.clone(),
            subject_kind: self.subject_kind.clone(),
            owner_path: self.owner_path.clone(),
            score: self.score,
            rank: self.rank,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapSnapshotV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_meta: RepoMapSnapshotMetaV1,
    pub entries: Vec<RepoMapEntryV1>,
}
