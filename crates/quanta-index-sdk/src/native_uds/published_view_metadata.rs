//! Historical operation metadata is data, never a read ticket or CAS basis.
use super::*;

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Read the original published metadata without pinning a retired view.
    /// The returned identity is not authority to query or access its objects.
    pub fn published_view_metadata(
        &self,
        operation: OperationKey,
        budget: &RequestBudgetV1,
    ) -> Result<CatalogView, NativeUdsError> {
        read_session::admitted_budget_checkpoint(budget)?;
        let NativeIndexResponse::PublishedViewMetadata(reply) =
            self.request(NativeIndexRequest::PublishedViewMetadata(operation), budget)?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected published metadata reply".into(),
            ));
        };
        validate_reply(operation, &reply)?;
        read_session::admitted_budget_checkpoint(budget)?;
        Ok(reply.payload)
    }

    /// Retain the original raw/decoded grants through every metadata borrow.
    /// Canonical Error replies remain charged values for caller classification.
    pub fn published_view_metadata_admitted(
        &self,
        operation: OperationKey,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        // The shared admitted parent checkpoints the original budget and uses
        // validate_reply at its request/reply binding gate before returning.
        self.request_publication_admitted_ref(
            &NativeIndexRequest::PublishedViewMetadata(operation),
            budget,
            memory,
        )
    }
}

pub(super) fn validate_reply(
    operation: OperationKey,
    reply: &OperationReply<CatalogView>,
) -> Result<(), NativeUdsError> {
    let view = &reply.payload;
    if reply.operation != operation
        || view.identity.workspace != operation.workspace
        || view.identity.epoch != operation.epoch
    {
        return Err(NativeUdsError::Protocol(
            "published metadata operation or namespace differs".into(),
        ));
    }
    let digest = view.identity_digest().map_err(|error| {
        NativeUdsError::Protocol(format!("published metadata view digest: {error}"))
    })?;
    if view.identity.digest != digest {
        return Err(NativeUdsError::Protocol(
            "published metadata view digest differs".into(),
        ));
    }
    Ok(())
}
