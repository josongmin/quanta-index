use quanta_index_contract::{
    RepoMapQueryRequestV1, RepoMapQueryResponseV1, RepoMapSnapshotReadRequestV1,
    RepoMapSnapshotReadResponseV1,
};

use crate::CoreError;

pub trait RepoMapQueryPort: Send + Sync {
    fn query(&self, request: RepoMapQueryRequestV1) -> Result<RepoMapQueryResponseV1, CoreError>;
}

pub trait RepoMapSnapshotReadPort: Send + Sync {
    fn read_snapshot(
        &self,
        request: RepoMapSnapshotReadRequestV1,
    ) -> Result<RepoMapSnapshotReadResponseV1, CoreError>;
}
