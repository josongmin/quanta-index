use crate::model::RepoMapSnapshot;

pub struct RepoMapDeltaApplier;

impl RepoMapDeltaApplier {
    #[must_use]
    pub fn apply(snapshot: RepoMapSnapshot) -> RepoMapSnapshot {
        snapshot
    }
}
