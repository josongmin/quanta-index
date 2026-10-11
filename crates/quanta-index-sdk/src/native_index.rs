//! Native incremental submission surface. Transport owns delivery and server
//! execution; cancelling a client future does not cancel an admitted operation.
use quanta_index_contract::index::{
    OperationKey, OperationReply, OperationStatus, SubmitOutcome, UpdateRequest,
};
use std::{future::Future, pin::Pin, sync::Arc};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NativeSubmitError {
    #[error("request was not admitted: {0}")]
    NotAdmitted(String),
    #[error("delivery outcome unknown; inspect the original operation {0:?}")]
    Unknown(OperationKey),
    #[error("transport response does not bind the submitted operation")]
    ResponseMismatch,
    #[error("operation status transport failed: {0}")]
    StatusTransport(String),
}

pub type NativeFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, NativeSubmitError>> + Send + 'a>>;

/// Implementations distinguish known pre-admission rejection from ambiguous
/// delivery. They may not manufacture a fresh key after a timeout or ACK loss.
pub trait NativeIndexTransport: Send + Sync {
    fn submit(&self, request: UpdateRequest) -> NativeFuture<'_, OperationReply<SubmitOutcome>>;
    fn status(&self, operation: OperationKey) -> NativeFuture<'_, OperationReply<OperationStatus>>;
}

pub struct NativeIndexClient<T: NativeIndexTransport + ?Sized> {
    transport: Arc<T>,
}

impl<T: NativeIndexTransport + ?Sized> Clone for NativeIndexClient<T> {
    fn clone(&self) -> Self {
        Self {
            transport: Arc::clone(&self.transport),
        }
    }
}

impl<T: NativeIndexTransport + ?Sized> NativeIndexClient<T> {
    #[must_use]
    pub const fn new(transport: Arc<T>) -> Self {
        Self { transport }
    }

    pub async fn submit(&self, request: UpdateRequest) -> Result<SubmitOutcome, NativeSubmitError> {
        let key = request.operation;
        let reply = self
            .transport
            .submit(request)
            .await
            .map_err(|error| match error {
                NativeSubmitError::Unknown(other) if other != key => {
                    NativeSubmitError::ResponseMismatch
                }
                error @ (NativeSubmitError::NotAdmitted(_)
                | NativeSubmitError::Unknown(_)
                | NativeSubmitError::ResponseMismatch
                | NativeSubmitError::StatusTransport(_)) => error,
            })?;
        if reply.operation != key {
            return Err(NativeSubmitError::ResponseMismatch);
        }
        let outcome = reply.payload;
        let valid = match &outcome {
            SubmitOutcome::Accepted(attempt) => attempt.operation == key,
            SubmitOutcome::Unknown(operation) => *operation == key,
            SubmitOutcome::Terminal(OperationStatus::Published(view)) => {
                view.workspace == key.workspace && view.epoch == key.epoch
            }
            SubmitOutcome::Terminal(OperationStatus::Refused(_)) => true,
            SubmitOutcome::Terminal(OperationStatus::Pending(_) | OperationStatus::Unknown) => {
                false
            }
        };
        if valid {
            Ok(outcome)
        } else {
            Err(NativeSubmitError::ResponseMismatch)
        }
    }

    /// Reconciliation reads the original durable outcome. It performs no retry,
    /// activation or operation cancellation.
    pub async fn status(&self, key: OperationKey) -> Result<OperationStatus, NativeSubmitError> {
        let reply = self
            .transport
            .status(key)
            .await
            .map_err(|error| match error {
                NativeSubmitError::Unknown(other) if other != key => {
                    NativeSubmitError::ResponseMismatch
                }
                error @ (NativeSubmitError::NotAdmitted(_)
                | NativeSubmitError::Unknown(_)
                | NativeSubmitError::ResponseMismatch
                | NativeSubmitError::StatusTransport(_)) => error,
            })?;
        if reply.operation != key {
            return Err(NativeSubmitError::ResponseMismatch);
        }
        let status = reply.payload;
        let valid = match status {
            OperationStatus::Pending(attempt) => attempt.operation == key,
            OperationStatus::Published(view) => {
                view.workspace == key.workspace && view.epoch == key.epoch
            }
            OperationStatus::Unknown | OperationStatus::Refused(_) => true,
        };
        if valid {
            Ok(status)
        } else {
            Err(NativeSubmitError::ResponseMismatch)
        }
    }
}
