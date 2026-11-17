use super::*;
use crate::{LegacyQueryCapabilityV1, SdkError};

#[test]
fn native_rejects_legacy_repomap_with_typed_identity_before_socket_io() {
    use quanta_index_contract::index::*;
    let directory = ok_or_fail!(tempfile::tempdir());
    let socket = directory.path().join("native.sock");
    let listener = ok_or_fail!(std::os::unix::net::UnixListener::bind(&socket));
    ok_or_fail!(listener.set_nonblocking(true));
    let selection = ViewSelection::Latest {
        workspace: WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        },
        epoch: 1,
    };
    let client = ok_or_fail!(QuantaIndex::connect(ConnectOptions::from_native_socket(
        socket, selection
    )));
    assert!(client.reader().native().is_ok());
    let error = client
        .repomap()
        .query(sample_repomap_query_request())
        .expect_err("Legacy contract must refuse");
    assert!(matches!(
        error,
        SdkError::UnsupportedNativeQuery {
            capability: LegacyQueryCapabilityV1::RepoMapQuery
        }
    ));
    assert_eq!(
        LegacyQueryCapabilityV1::RepoMapQuery.code(),
        quanta_index_contract::native_protocol::NativeIndexErrorCode::CapabilityUnavailable
    );
    assert_eq!(
        LegacyQueryCapabilityV1::RepoMapQuery.as_str(),
        "legacy.repomap"
    );
    assert_eq!(crate::UNSUPPORTED_QUERY_DIAGNOSTIC_VERSION, 1);
    assert_eq!(
        listener.accept().expect_err("no query connection").kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(error.published_evidence().is_none());
    assert!(error.published_failure_stage().is_none());
}

#[test]
fn injected_legacy_repomap_still_dispatches_its_original_contract() {
    let response = sample_repomap_query_response();
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::RepoMapQuery(response.clone()),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    assert_eq!(
        ok_or_fail!(client.repomap().query(sample_repomap_query_request())),
        response
    );
    assert!(matches!(
        ok_or_fail!(only_query_request(query.as_ref())).payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::RepoMapQuery(_)
    ));
}
