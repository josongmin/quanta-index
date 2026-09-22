use quanta_index_contract::ipc::{
    MetricsSnapshotRequest, MetricsSnapshotV1, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponse,
};

use crate::{QuantaIndex, SdkError};

/// Read-only observability over the control socket (QI-BB-015).
pub struct ObservabilityNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> ObservabilityNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Every counter, gauge and histogram the daemon has aggregated since
    /// it started, plus what its diagnostic rings kept and dropped.
    pub fn metrics_snapshot(&self) -> Result<MetricsSnapshotV1, SdkError> {
        let response =
            self.client
                .dispatch_control(SearchPlaneControlIpcRequest::MetricsSnapshot(
                    MetricsSnapshotRequest,
                ))?;
        match response {
            SearchPlaneControlIpcResponse::MetricsSnapshot(snapshot) => Ok(snapshot),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected metrics snapshot, got {}",
                    QuantaIndex::control_response_kind(&other)
                )))
            }
        }
    }
}
