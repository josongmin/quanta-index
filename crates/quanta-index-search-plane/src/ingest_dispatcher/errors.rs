//! Ingest wire error codes and the `CoreError` -> IPC error envelope.

use quanta_index_contract::SearchPlaneIpcError;
use quanta_index_core::CoreError;

pub(super) fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = err.into_search_plane_wire();
    // Ingest failures carry no query-intent repair metadata (J7Q-06 repair is
    // query-route specific); the wire field stays None.
    SearchPlaneIpcError {
        code,
        message,
        repair: None,
    }
}
