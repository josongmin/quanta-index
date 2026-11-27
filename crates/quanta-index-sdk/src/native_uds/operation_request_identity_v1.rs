//! Read-only immutable operation identity for Source terminal reconciliation.

use super::*;

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Retain the exact decoded response and its raw backing under the Native
    /// publication budget. Unknown or reclaimed operations return Q's error.
    pub fn operation_request_identity_admitted_v1(
        &self,
        operation: OperationKey,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        read_session::admitted_budget_checkpoint(budget)?;
        let reply = self.request_publication_admitted_ref(
            &NativeIndexRequest::OperationRequestIdentityV1(operation),
            budget,
            memory,
        )?;
        match reply.response() {
            NativeIndexResponse::OperationRequestIdentityV1(identity)
                if identity.operation == operation && identity.payload.key == operation => {}
            NativeIndexResponse::Error(_) => {}
            _ => return Err(NativeUdsError::Unknown(operation)),
        }
        read_session::admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }
}
