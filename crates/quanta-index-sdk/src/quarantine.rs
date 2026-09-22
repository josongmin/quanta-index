use quanta_index_contract::ipc::{
    QuarantineDiscardAck, QuarantineDiscardRequest, QuarantineInventoryRequest,
    QuarantineInventoryV1, QuarantineTargetV1, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponse,
};

use crate::{QuantaIndex, SdkError};

/// The daemon's quarantine over the control socket (QI-BB-026): what boot
/// set aside, listed live, and the one way to discard an entry.
pub struct QuarantineNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> QuarantineNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Everything quarantined right now, per authority.
    pub fn inventory(&self) -> Result<QuarantineInventoryV1, SdkError> {
        let response =
            self.client
                .dispatch_control(SearchPlaneControlIpcRequest::QuarantineInventory(
                    QuarantineInventoryRequest,
                ))?;
        match response {
            SearchPlaneControlIpcResponse::QuarantineInventory(inventory) => Ok(inventory)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected quarantine inventory, got {}",
                QuantaIndex::control_response_kind(&other)
            ))),
        }
    }

    /// Discard one entry exactly as [`Self::inventory`] listed it.
    ///
    /// The daemon re-inventories before removing anything and refuses,
    /// typed, an entry it does not quarantine at that moment.
    pub fn discard(&self, target: &QuarantineTargetV1) -> Result<QuarantineDiscardAck, SdkError> {
        let response =
            self.client
                .dispatch_control(SearchPlaneControlIpcRequest::QuarantineDiscard(
                    QuarantineDiscardRequest {
                        target: target.clone(),
                    },
                ))?;
        match response {
            SearchPlaneControlIpcResponse::QuarantineDiscardAck(ack) => {
                if ack.target != *target {
                    return Err(SdkError::Protocol(
                        "quarantine discard ack names a different target than was sent".to_string(),
                    ));
                }
                Ok(ack)
            }
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected quarantine discard ack, got {}",
                QuantaIndex::control_response_kind(&other)
            ))),
        }
    }
}
