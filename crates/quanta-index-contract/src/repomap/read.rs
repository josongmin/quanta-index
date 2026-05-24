use crate::GenerationPin;

use super::{RepoMapEntryDtoV1, RepoMapSnapshotMetaV1};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSnapshotReadRequestV1 {
    pub generation: GenerationPin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapSnapshotReadResponseV1 {
    pub generation: GenerationPin,
    pub snapshot_meta: RepoMapSnapshotMetaV1,
    pub entries: Vec<RepoMapEntryDtoV1>,
}
