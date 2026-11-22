use super::*;
use crate::{LegacyQueryCapabilityV1, SdkError};
use quanta_index_contract::{
    PUBLIC_TOP_K_MAX, QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax,
};
use std::os::unix::net::UnixListener;
use std::time::Duration;

fn native_query_listener() -> (tempfile::TempDir, UnixListener, QuantaIndex) {
    use quanta_index_contract::index::*;
    let directory = ok_or_fail!(tempfile::tempdir());
    let socket = directory.path().join("native.sock");
    let listener = ok_or_fail!(UnixListener::bind(&socket));
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
    let client = ok_or_fail!(QuantaIndex::connect(
        ConnectOptions::from_native_socket(socket, selection)
            .with_request_io_timeout(Duration::from_millis(125))
    ));
    assert!(client.reader().native().is_ok());
    (directory, listener, client)
}

fn assert_no_query_connection(listener: &UnixListener) {
    assert_eq!(
        listener.accept().expect_err("no query connection").kind(),
        std::io::ErrorKind::WouldBlock
    );
}

fn assert_native_query_refusal(
    listener: &UnixListener,
    error: SdkError,
    expected: LegacyQueryCapabilityV1,
    syntax: TextQuerySyntax,
    top_k: u32,
) {
    assert!(
        matches!(
            &error,
            SdkError::UnsupportedNativeQuery { capability } if *capability == expected
        ),
        "{syntax:?}, top_k={top_k}: expected {expected:?}, got {error:?}"
    );
    assert_no_query_connection(listener);
}

fn legacy_query_text(syntax: TextQuerySyntax, query: &str, top_k: u32) -> TextQueryRequest {
    TextQueryRequest {
        syntax,
        query_text: query.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(sample_generation_pin()),
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

#[test]
fn native_rejects_legacy_repomap_with_typed_identity_before_socket_io() {
    let (_directory, listener, client) = native_query_listener();
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
    assert_no_query_connection(&listener);
    assert!(error.published_evidence().is_none());
    assert!(error.published_failure_stage().is_none());
}

#[test]
fn native_rejects_legacy_runtime_metadata_in_both_syntaxes_before_socket_io() {
    let (_directory, listener, client) = native_query_listener();
    let runtime = client.runtime();
    let query = "dirty:no quartz";
    let capability = LegacyQueryCapabilityV1::RuntimeMetadata;

    for top_k in [0, 10, PUBLIC_TOP_K_MAX + 1] {
        for (syntax, builder) in [
            (TextQuerySyntax::Native, runtime.query().native(query)),
            (
                TextQuerySyntax::Sourcegraph,
                runtime.query().sourcegraph(query),
            ),
        ] {
            let error = builder
                .pinned(sample_generation_pin())
                .top_k(top_k)
                .execute()
                .expect_err("Native SDK must refuse the Legacy runtime builder");
            assert_native_query_refusal(&listener, error, capability, syntax, top_k);

            let error = runtime
                .query_request(RuntimeMetadataQueryRequest {
                    text_query: legacy_query_text(syntax, query, top_k),
                    cursor: None,
                })
                .expect_err("Native SDK must refuse the Legacy runtime DTO");
            assert_native_query_refusal(&listener, error, capability, syntax, top_k);
        }
    }
}

#[test]
fn native_rejects_legacy_structural_in_both_syntaxes_before_socket_io() {
    let (_directory, listener, client) = native_query_listener();
    let structural = client.structural();
    let native = "match { function_item { { identifier :[name] } } }";
    let sourcegraph = r#"patterntype:structural "function_item { { identifier :[name] } }""#;
    let capability = LegacyQueryCapabilityV1::Structural;

    for top_k in [0, 10, PUBLIC_TOP_K_MAX + 1] {
        for (syntax, query, builder) in [
            (
                TextQuerySyntax::Native,
                native,
                structural.query().native(native),
            ),
            (
                TextQuerySyntax::Sourcegraph,
                sourcegraph,
                structural.query().sourcegraph(sourcegraph),
            ),
        ] {
            let error = builder
                .pinned(sample_generation_pin())
                .top_k(top_k)
                .execute()
                .expect_err("Native SDK must refuse the Legacy structural builder");
            assert_native_query_refusal(&listener, error, capability, syntax, top_k);

            let error = structural
                .query_request(StructuralQueryRequest {
                    text_query: legacy_query_text(syntax, query, top_k),
                    cursor: None,
                })
                .expect_err("Native SDK must refuse the Legacy structural DTO");
            assert_native_query_refusal(&listener, error, capability, syntax, top_k);
        }
    }
}

#[test]
fn native_rejects_legacy_subroutes_before_their_local_validation() {
    let (_directory, listener, client) = native_query_listener();
    let search = client.search();
    let hybrid = || {
        search
            .hybrid()
            .native("needle")
            .semantic_text("needle")
            .pinned(sample_generation_pin())
            .top_k(0)
    };
    for (error, capability) in [
        (
            hybrid()
                .files(0)
                .expect_err("Native must refuse Legacy hybrid files"),
            LegacyQueryCapabilityV1::HybridFileV1,
        ),
        (
            hybrid()
                .go_test_trace_file("", 0)
                .expect_err("Native must refuse the Legacy Go trace query"),
            LegacyQueryCapabilityV1::GoTestTraceFileV1,
        ),
    ] {
        assert_native_query_refusal(&listener, error, capability, TextQuerySyntax::Native, 0);
    }

    let mut membership = sample_cluster_membership_request();
    membership.limit = 0;
    let error = search
        .cluster_membership_read_v1(membership)
        .expect_err("Native must refuse Legacy cluster membership");
    assert!(matches!(
        error,
        SdkError::UnsupportedNativeQuery {
            capability: LegacyQueryCapabilityV1::ClusterMembershipRead
        }
    ));
    assert_no_query_connection(&listener);
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
