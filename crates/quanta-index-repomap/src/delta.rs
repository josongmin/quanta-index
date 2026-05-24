use crate::model::RepoMapSnapshotV1;

pub struct RepoMapDeltaApplier;

impl RepoMapDeltaApplier {
    #[must_use]
    pub fn apply(snapshot: RepoMapSnapshotV1) -> RepoMapSnapshotV1 {
        snapshot
    }
}
