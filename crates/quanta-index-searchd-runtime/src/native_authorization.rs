//! Authorize native requests before dispatching one-shot or streaming work.

use quanta_index_contract::native_protocol::NativeIndexRequest;
use quanta_index_ipc::{IpcPlane, PeerCredentials};

pub(crate) fn request_authorized(
    plane: IpcPlane,
    principal: Option<PeerCredentials>,
    owner_uid: u32,
    request: &NativeIndexRequest,
) -> bool {
    let Some(peer) = principal else {
        return false;
    };
    match request {
        NativeIndexRequest::ReadSession(_)
        | NativeIndexRequest::ReadView(_)
        | NativeIndexRequest::ObserveRootView(_)
        | NativeIndexRequest::ObserveRepoMapGraphInstallationV1(_)
        | NativeIndexRequest::ObserveRepoMapEventFrontierV1 { .. }
        | NativeIndexRequest::ResolveProtectedSourceHead { .. }
        | NativeIndexRequest::ResolveProtectedRepoMapHead { .. }
        | NativeIndexRequest::ReadImportedRepoMapMember { .. }
        | NativeIndexRequest::PublishedViewMetadata(_)
        | NativeIndexRequest::ReadProjection { .. }
        | NativeIndexRequest::LookupSourceOwners { .. }
        | NativeIndexRequest::ListProducerSourceOwners { .. }
        | NativeIndexRequest::LookupProjectionOwners { .. }
        | NativeIndexRequest::DiffProjectionSourceOwners { .. }
        | NativeIndexRequest::SearchLiteral { .. }
        | NativeIndexRequest::QueryPaths { .. }
        | NativeIndexRequest::QueryRepositoryPaths { .. }
        | NativeIndexRequest::SearchText { .. }
        | NativeIndexRequest::SearchTextPreview { .. }
        | NativeIndexRequest::SearchSemantic { .. }
        | NativeIndexRequest::QueryGraph { .. }
        | NativeIndexRequest::QueryImportedRepoMap { .. }
        | NativeIndexRequest::QueryRepoMap { .. }
        | NativeIndexRequest::QueryEffortStream { .. } => {
            matches!(plane, IpcPlane::Query | IpcPlane::Control)
        }
        // This also covers ReadRepoMapPriorFileV1. Typed facts can contain
        // preview text and must stay on the producer's owner-only plane.
        _ => plane == IpcPlane::Control && (peer.uid == owner_uid || peer.uid == 0),
    }
}
