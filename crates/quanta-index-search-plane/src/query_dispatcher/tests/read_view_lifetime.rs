//! SEP-21 P04 owner tests — the V2 read view's domains, handles and
//! evidence (S21-05).
//!
//! Every declared domain has exactly one evidence entry, including domains
//! that share a physical handle.

use std::sync::Arc;

use quanta_index_contract::{GenerationPin, ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{ReadDomainV1, ReadResourceGroupV2, RequiredDomainsV1};

use crate::query_dispatcher::read_view::{ReadViewRequestV1, assemble_for_test};
use crate::query_dispatcher::tests::support::lexical::StubLexicalSearcher;

fn pin() -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(9),
    )
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
    let domains = RequiredDomainsV1::of(ReadDomainV1::LexicalTrack).with(
        ReadDomainV1::RepoMetadata(quanta_index_core::RepoMetadataAuthorityV1::FileOwnership),
    );
    let view_pin = pin();
    let request = ReadViewRequestV1::new("hybrid seed", &view_pin, domains);
    let view =
        assemble_for_test(&request, None, None, None, Some(searcher)).expect("view assembles");
    let identity = view.identity();
    assert!(identity.evidence_is_exact());
    assert_eq!(identity.evidence.len(), 2);
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
