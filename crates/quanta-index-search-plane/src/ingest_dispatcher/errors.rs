//! Ingest wire error codes and the `CoreError` -> IPC error envelope.

use quanta_index_contract::SearchPlaneIpcError;
use quanta_index_core::CoreError;

pub(super) const ERR_INVALID: &str = "INVALID_REQUEST";
pub(super) const ERR_NOT_READY: &str = "NOT_READY";
pub(super) const ERR_NOT_FOUND: &str = "NOT_FOUND";
pub(super) const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
pub(super) const ERR_INTERNAL: &str = "INTERNAL";
pub(super) const ERR_SEARCH_CORPUS_GENERATION_CONFLICT: &str = "SEARCH_CORPUS_GENERATION_CONFLICT";
/// The batch's own shape is invalid; nothing was mutated (QI-BB-029).
pub(super) const ERR_SEARCH_CORPUS_BATCH_SHAPE: &str = "SEARCH_CORPUS_BATCH_SHAPE_INVALID";
/// A delta names a base the ledger never sealed; nothing was mutated.
pub(super) const ERR_SEARCH_CORPUS_DELTA_BASE_NOT_SEALED: &str =
    "SEARCH_CORPUS_DELTA_BASE_NOT_SEALED";

pub(super) fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = match err {
        CoreError::InvalidContract(msg) => (ERR_INVALID.to_string(), msg),
        CoreError::Typed { code, message } => (code, message),
        CoreError::NotReady(msg) => (ERR_NOT_READY.to_string(), msg),
        CoreError::NotImplemented(msg) => (ERR_NOT_IMPLEMENTED.to_string(), msg),
        CoreError::NotFound(msg) => (ERR_NOT_FOUND.to_string(), msg),
        CoreError::Storage(msg) => (ERR_INTERNAL.to_string(), msg),
    };
    // Ingest failures carry no query-intent repair metadata (J7Q-06 repair is
    // query-route specific); the wire field stays None.
    SearchPlaneIpcError {
        code,
        message,
        repair: None,
    }
}
