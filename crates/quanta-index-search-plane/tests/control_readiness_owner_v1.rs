//! P09 owner tests (integration surface): the S21-10 control-authorization
//! capability table and access matrix, exercised through the public
//! search-plane API.
//!
//! Registered `control-readiness-owner-v1` target. Edge coverage:
//! exhaustive opcode→capability mapping, every principal class, absent
//! credential context, and the default-deny direction (an unknown peer may
//! never mutate; it may only observe).

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    reason = "fixture construction asserts the static fixture satisfies the canonical policy"
)]

use quanta_index_contract::{
    CurrentGenerationRequest, GenerationSnapshot, GenerationStatusRequest, ManifestGeneration,
    MetricsSnapshotRequest, ProcessReadinessRequest, QuarantineDiscardRequest,
    QuarantineInventoryRequest, QuarantineTargetV1, QuarantinedRepoMapFileEntryV1, RepoId,
    RepoMapActivateGenerationRequestV2, RepoMapActiveHeadRequestV2, RevisionId,
    SearchCorpusGenerationIdentityV1,
    SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneControlIpcRequest,
    SearchPlaneErrorCodeV2, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SearchPlaneTrackKind, SemanticContentRootsV1,
};

fn fixture_repo() -> RepoId {
    RepoId::new("r").expect("static fixture ID satisfies canonical policy")
}

fn fixture_revision() -> RevisionId {
    RevisionId::new("v").expect("static fixture ID satisfies canonical policy")
}

fn snapshot(track: SearchPlaneTrackKind) -> GenerationSnapshot {
    GenerationSnapshot {
        repo_id: fixture_repo(),
        revision_id: fixture_revision(),
        track,
        manifest_generation: ManifestGeneration::new(1),
        manifest_digest: format!("sha256:{}", "a".repeat(64)),
    }
}

fn identity() -> SearchCorpusGenerationIdentityV1 {
    SearchCorpusGenerationIdentityV1 {
        lexical: snapshot(SearchPlaneTrackKind::Lexical),
        semantic: snapshot(SearchPlaneTrackKind::Semantic),
        semantic_content: SemanticContentRootsV1 {
            row_root_digest: format!("sha256:{}", "b".repeat(64)),
            membership_root_digest: format!("sha256:{}", "c".repeat(64)),
        },
    }
}
use quanta_index_search_plane::{ControlAccessV1, ControlCapabilityV1};

fn observe_requests() -> Vec<SearchPlaneControlIpcRequest> {
    vec![
        SearchPlaneControlIpcRequest::CurrentGeneration(CurrentGenerationRequest {
            repo_id: fixture_repo(),
            revision_id: fixture_revision(),
            track: SearchPlaneTrackKind::Lexical,
        }),
        SearchPlaneControlIpcRequest::GenerationStatus(GenerationStatusRequest {
            repo_id: fixture_repo(),
            revision_id: fixture_revision(),
        }),
        SearchPlaneControlIpcRequest::MetricsSnapshot(MetricsSnapshotRequest),
        SearchPlaneControlIpcRequest::QuarantineInventory(QuarantineInventoryRequest),
        SearchPlaneControlIpcRequest::ProcessReadiness(ProcessReadinessRequest),
    ]
}

fn admin_requests() -> Vec<SearchPlaneControlIpcRequest> {
    vec![
        SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
            SearchPlaneActivateSearchCorpusGenerationCasRequest {
                candidate: identity(),
                expected_active: None,
            },
        ),
        SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
            SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                target: identity(),
                expected_active: identity(),
            },
        ),
        SearchPlaneControlIpcRequest::RepoMapActivateV2(RepoMapActivateGenerationRequestV2 {
            repo_id: RepoId::new("r").expect("static fixture ID"),
            revision_id: RevisionId::new("v").expect("static fixture ID"),
            manifest_generation: quanta_index_contract::ManifestGeneration::new(1),
            manifest_digest: "a".repeat(64),
            snapshot_id: "snapshot-1".to_string(),
            projection_version: 1,
            authority_digest: "sha256:".to_string() + &"b".repeat(64),
            source_bundle_digest: "sha256:".to_string() + &"c".repeat(64),
            expected_active: None,
        }),
        SearchPlaneControlIpcRequest::RepoMapActiveHeadV2(RepoMapActiveHeadRequestV2 {
            repo_id: fixture_repo(),
            revision_id: fixture_revision(),
        }),
        SearchPlaneControlIpcRequest::QuarantineDiscard(QuarantineDiscardRequest {
            target: QuarantineTargetV1::RepoMapFile(QuarantinedRepoMapFileEntryV1 {
                file_name: "f".to_string(),
                reason: "r".to_string(),
            }),
        }),
    ]
}

#[test]
fn every_opcode_maps_to_exactly_one_capability() {
    // Exhaustive: observe excludes sensitive head tokens; admin includes
    // mutations and the CAS-token read. A new opcode that forgets its
    // capability cannot compile, and a mis-classified one fails here.
    for request in observe_requests() {
        assert_eq!(
            ControlCapabilityV1::required_for(&request),
            ControlCapabilityV1::Observe,
            "read-only opcode misclassified: {request:?}"
        );
    }
    for request in admin_requests() {
        assert_eq!(
            ControlCapabilityV1::required_for(&request),
            ControlCapabilityV1::Admin,
            "mutating opcode misclassified: {request:?}"
        );
    }
}

#[test]
fn the_access_matrix_is_default_deny_for_admin() {
    // The daemon's own in-process context holds both capabilities.
    assert!(ControlAccessV1::InProcessOperator.permits(ControlCapabilityV1::Observe));
    assert!(ControlAccessV1::InProcessOperator.permits(ControlCapabilityV1::Admin));

    // A peer running as the socket owner, or as root, is the operator.
    assert!(
        ControlAccessV1::Peer {
            uid: 1000,
            owner_uid: 1000
        }
        .permits(ControlCapabilityV1::Admin)
    );
    assert!(
        ControlAccessV1::Peer {
            uid: 0,
            owner_uid: 1000
        }
        .permits(ControlCapabilityV1::Admin)
    );

    // Any other admitted peer observes and never mutates.
    let observer = ControlAccessV1::Peer {
        uid: 2000,
        owner_uid: 1000,
    };
    assert!(observer.permits(ControlCapabilityV1::Observe));
    assert!(!observer.permits(ControlCapabilityV1::Admin));

    // The refusal is attributable, not anonymous.
    assert_eq!(observer.principal_name(), "peer-observer");
    assert_eq!(
        ControlAccessV1::Peer {
            uid: 1000,
            owner_uid: 1000
        }
        .principal_name(),
        "peer-operator"
    );
    assert_eq!(
        ControlAccessV1::InProcessOperator.principal_name(),
        "in-process-operator"
    );
}

#[test]
fn the_authorization_refusal_code_is_the_closed_enum_variant() {
    // The wire code exists, round-trips and is what a denial carries.
    let code = SearchPlaneErrorCodeV2::ControlAuthorizationDenied;
    assert_eq!(code.as_wire_str(), "CONTROL_AUTHORIZATION_DENIED");
    assert_eq!(
        SearchPlaneErrorCodeV2::from_wire_str("CONTROL_AUTHORIZATION_DENIED"),
        Some(code)
    );
    assert!(
        SearchPlaneErrorCodeV2::ALL.contains(&code),
        "the variant is part of the closed table's ALL set"
    );
}
