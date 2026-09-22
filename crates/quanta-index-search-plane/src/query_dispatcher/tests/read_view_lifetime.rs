//! SEP-21 P04 owner tests — the V2 read view's domains, handles and
//! evidence (S21-05).
//!
//! Covers the view-level `DoD`: the `RepoMap` domain is a real pinned
//! handle acquired once per view; the identity carries the commitment
//! and activation epoch; every declared domain has exactly one evidence
//! entry (shared physical handles included); an undeclared accessor is
//! a typed refusal.

use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, SearchPlaneQueryIpcRequest,
};
use quanta_index_core::{
    CoreError, READ_VIEW_DOMAIN_UNDECLARED_CODE, ReadDomainV1, ReadResourceGroupV2,
    ReadViewRefusedError, RequiredDomainsV1,
};

use crate::Ledger;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::read_view::{ReadViewRequestV1, assemble_for_test};
use crate::query_dispatcher::tests::support::common::test_activation_catalog;
use crate::query_dispatcher::tests::support::lexical::{RejectLexicalOpener, StubLexicalSearcher};
use crate::query_dispatcher::tests::support::repo_map::{
    StubRepoMapSnapshotPort, into_repo_map_query_response, repo_map_request,
};
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

fn pin() -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(9),
    )
}

#[test]
fn a_repo_map_route_serves_from_one_pinned_handle_with_carried_evidence() {
    let stub = Arc::new(StubRepoMapSnapshotPort::default());
    let stub_port: Arc<dyn quanta_index_core::RepoMapSnapshotAcquirePort + Send + Sync> =
        stub.clone();
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        stub_port,
        Arc::new(FailClosedStructuralProducer),
        Arc::new(RwLock::new(Ledger::default())),
        test_activation_catalog().expect("activation catalog fixture"),
    );
    let response = into_repo_map_query_response(dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::RepoMapQuery(repo_map_request()),
        &quanta_index_core::RequestBudgetV1::unbounded(),
    ))
    .expect("dispatcher returns a response envelope");
    // One request, exactly one acquisition through the acquire port. The
    // guard is scoped to the read: only copies outlive it.
    let evidence = {
        let acquired = stub
            .acquired
            .lock()
            .expect("stub acquire log is not poisoned");
        assert_eq!(acquired.len(), 1);
        acquired.first().cloned().expect("one acquisition evidence")
    };
    assert_eq!(evidence.manifest_generation, 9);
    assert_eq!(
        evidence.candidate_commitment,
        "stub-commitment-repo-map-ipc-9"
    );
    assert_eq!(response.manifest_generation.get(), 9);
}

#[test]
fn an_undeclared_repo_map_accessor_is_a_typed_refusal() {
    // A lexical-only view holds no RepoMap handle; reaching for one is a
    // defect and is refused with the view's own code.
    let searcher = Arc::new(StubLexicalSearcher {
        manifest_digest: None,
        results: Vec::new(),
    });
    let view_pin = pin();
    let request = ReadViewRequestV1::new(
        "lexical",
        &view_pin,
        RequiredDomainsV1::of(ReadDomainV1::LexicalTrack),
    );
    let view =
        assemble_for_test(&request, None, None, None, Some(searcher)).expect("view assembles");
    let err = match view.repo_map() {
        Ok(_snapshot) => panic!("an undeclared domain must refuse"),
        Err(err) => err,
    };
    match &err {
        CoreError::Typed { code, message } => {
            assert_eq!(*code, READ_VIEW_DOMAIN_UNDECLARED_CODE);
            assert!(message.contains("repo-map"), "{message}");
        }
        CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_) => panic!("unexpected refusal: {err:?}"),
    }
    // The refusal is constructible through the view's own error type.
    let refused = ReadViewRefusedError::DomainUndeclared {
        domain: ReadDomainV1::RepoMap,
        pin: pin(),
    };
    assert_eq!(refused.code(), READ_VIEW_DOMAIN_UNDECLARED_CODE);
    assert!(view.identity().evidence_is_exact());
    assert!(
        !view
            .identity()
            .evidence
            .contains_key(&ReadDomainV1::RepoMap)
    );
}

#[test]
fn every_declared_domain_has_exactly_one_evidence_entry() {
    // Shared physical handle: the repo-metadata domains execute against
    // the lexical handle, so three declared domains carry three evidence
    // entries in one resource group.
    let searcher = Arc::new(StubLexicalSearcher {
        manifest_digest: None,
        results: Vec::new(),
    });
    let domains = RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
        .with(ReadDomainV1::RepoMap)
        .with(ReadDomainV1::RepoMetadata(
            quanta_index_core::RepoMetadataAuthorityV1::FileOwnership,
        ));
    let view_pin = pin();
    let request = ReadViewRequestV1::new("hybrid seed", &view_pin, domains);
    let view =
        assemble_for_test(&request, None, None, None, Some(searcher)).expect("view assembles");
    let identity = view.identity();
    assert!(identity.evidence_is_exact());
    assert_eq!(identity.evidence.len(), 3);
    assert_eq!(
        identity
            .evidence
            .get(&ReadDomainV1::RepoMetadata(
                quanta_index_core::RepoMetadataAuthorityV1::FileOwnership
            ))
            .expect("expected repo-metadata evidence")
            .resource_group,
        ReadResourceGroupV2::LexicalTrack
    );
    assert_eq!(
        identity
            .evidence
            .get(&ReadDomainV1::LexicalTrack)
            .expect("expected lexical evidence")
            .resource_group,
        ReadResourceGroupV2::LexicalTrack
    );
    // No undeclared domain ever gains evidence.
    assert!(!identity.evidence.contains_key(&ReadDomainV1::SemanticTrack));
    assert!(!identity.evidence.contains_key(&ReadDomainV1::History));
}
