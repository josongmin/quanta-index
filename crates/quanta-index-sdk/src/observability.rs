use quanta_index_contract::ipc::{
    MetricsSnapshotRequest, MetricsSnapshotV1, ProcessReadinessRequest, ProcessReadinessV1,
    ProcessRequestEventPlaneV1, ProcessRequestEventsRequestV1, ProcessRequestEventsV1,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
};

use crate::{QuantaIndexClientPayloadV1, SdkError};

/// Read-only observability over the control socket (QI-BB-015).
pub struct ObservabilityNamespace<'a> {
    client: &'a QuantaIndexClientPayloadV1,
}

impl<'a> ObservabilityNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndexClientPayloadV1) -> Self {
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
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)
            | SearchPlaneControlIpcResponse::ProcessRequestEventsV1(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected metrics snapshot, got {}",
                    QuantaIndexClientPayloadV1::control_response_kind(&other)
                )))
            }
        }
    }

    /// Process-wide readiness, not the status of one repository generation.
    pub fn process_readiness(&self) -> Result<ProcessReadinessV1, SdkError> {
        let response =
            self.client
                .dispatch_control(SearchPlaneControlIpcRequest::ProcessReadiness(
                    ProcessReadinessRequest,
                ))?;
        match response {
            SearchPlaneControlIpcResponse::ProcessReadinessReport(report) => {
                report.validate_v1().map_err(|error| {
                    SdkError::Protocol(format!("invalid process readiness report: {error}"))
                })?;
                Ok(report)
            }
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessRequestEventsV1(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected process readiness report, got {}",
                    QuantaIndexClientPayloadV1::control_response_kind(&other)
                )))
            }
        }
    }

    /// Bounded, payload-free process-local request trace. The daemon requires
    /// Admin capability; this client-side API does not grant that capability.
    pub fn request_events(
        &self,
        plane: ProcessRequestEventPlaneV1,
        limit: u16,
    ) -> Result<ProcessRequestEventsV1, SdkError> {
        let request = ProcessRequestEventsRequestV1 { plane, limit };
        request
            .validate_v1()
            .map_err(|error| SdkError::Protocol(error.to_owned()))?;
        match self
            .client
            .dispatch_control(SearchPlaneControlIpcRequest::ProcessRequestEventsV1(
                request,
            ))? {
            SearchPlaneControlIpcResponse::ProcessRequestEventsV1(events) => Ok(events),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected process request events, got {}",
                    QuantaIndexClientPayloadV1::control_response_kind(&other)
                )))
            }
        }
    }
}
