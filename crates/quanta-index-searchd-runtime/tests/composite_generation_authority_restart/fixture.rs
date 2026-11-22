//! Shared source and composite fixtures for pinned old-Q tests.

#![expect(
    clippy::expect_used,
    reason = "fixed fixture IDs satisfy canonical policy in shared test setup"
)]

use std::error::Error;

use quanta_index_contract::{
    SearchCorpusActiveHeadV1, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
};
use quanta_index_sdk::{
    ChunkId, ChunkRecord, LanguageCode, ManifestGeneration, QuantaIndex, RepoId, RepoRelativePath,
    RevisionId, SdkError, SearchCorpusBatch, SearchCorpusGenerationIdentityV1,
    SearchPlaneTrackKind, SourceFileKey, SourcePublicationEvent,
};
use quanta_index_searchd_harness::fixture_source_scope_v1;

pub(super) const REPO: &str = "repo-composite-restart";
pub(super) const REVISION: &str = "revision-composite-restart";
pub(super) const G1: u64 = 41;
pub(super) const G2: u64 = 42;
pub(super) const G1_DIGEST: &str = "manifest:composite-restart:g1";
pub(super) const G2_DIGEST: &str = "manifest:composite-restart:g2";

pub(super) fn repo() -> RepoId {
    RepoId::new(REPO).expect("test fixture ID satisfies canonical policy")
}

pub(super) fn revision() -> RevisionId {
    RevisionId::new(REVISION).expect("test fixture ID satisfies canonical policy")
}

pub(super) fn generation(raw: u64) -> ManifestGeneration {
    ManifestGeneration::new(raw)
}

pub(super) fn fixture_event_v1(
    repo_id: &str,
    raw_generation: u64,
    source_parent_v1: Option<u64>,
) -> SourcePublicationEvent {
    SourcePublicationEvent {
        stream_id: format!("fixture:composite-restart:{repo_id}"),
        event_id: format!("fixture:composite-restart:{repo_id}:{raw_generation}"),
        expected_base_event_id: source_parent_v1
            .map(|prior_v1| format!("fixture:composite-restart:{repo_id}:{prior_v1}")),
        payload_sha256: [0; 32],
    }
}

fn batch(
    raw_generation: u64,
    digest: &str,
    source_parent_v1: Option<u64>,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let path = format!("src/generation_{raw_generation}.rs");
    let text = format!("fn generation_{raw_generation}() {{}}");
    let end_byte = u32::try_from(text.len())?;
    let scope_v1 = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id: repo(),
            repo_relative_path: RepoRelativePath::new(path.clone()),
        },
        revision(),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-generation-{raw_generation}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line: 1,
            text: text.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        Vec::new(),
    )?;
    Ok(SearchCorpusBatch::replace_generation(
        repo(),
        revision(),
        generation(raw_generation),
        digest,
    )
    .source_event(fixture_event_v1(REPO, raw_generation, source_parent_v1))
    .replace_scope(
        scope_v1.coverage,
        scope_v1.source_bytes,
        scope_v1.chunks,
        scope_v1.symbols,
    ))
}

/// The identities the daemon activated, roots included (QI-BB-028).
///
/// The semantic content roots are what the plane sealed and can only be
/// learned from its receipts, so every later comparison and rollback names
/// them from here.
pub(super) struct Activated {
    pub(super) g0: Option<SearchCorpusActiveHeadV1>,
    pub(super) g1: SearchCorpusActiveHeadV1,
    pub(super) g2: SearchCorpusActiveHeadV1,
}

/// Publish and activate `raw_generation`, checking the ack names the
/// batch's tracks and the roots the sealed receipt attested.
pub(super) fn publish_generation(
    client: &QuantaIndex,
    raw_generation: u64,
    digest: &str,
    expected_active: Option<SearchCorpusActiveHeadV1>,
) -> Result<SearchCorpusActiveHeadV1, Box<dyn Error>> {
    let (receipt, activation) = client.search_corpus().publish_and_activate(
        &batch(
            raw_generation,
            digest,
            expected_active
                .as_ref()
                .map(|head_v1| head_v1.generation.lexical.manifest_generation.get()),
        )?,
        expected_active,
    )?;
    let expected_tracks = composite_identity_for(REPO, REVISION, raw_generation, digest);
    if activation.active.generation.lexical != expected_tracks.lexical
        || activation.active.generation.semantic != expected_tracks.semantic
    {
        return Err(format!(
            "activation of G{raw_generation} selected foreign tracks: {:?}",
            activation.active
        )
        .into());
    }
    if Some(&activation.active.generation.semantic_content) != receipt.semantic_content.as_ref() {
        return Err(format!(
            "activation of G{raw_generation} names roots the sealed receipt did not attest: ack={:?} receipt={:?}",
            activation.active.generation.semantic_content, receipt.semantic_content
        )
        .into());
    }
    Ok(activation.active)
}

pub(super) fn publish_two_generations(client: &QuantaIndex) -> Result<Activated, Box<dyn Error>> {
    let g1 = publish_generation(client, G1, G1_DIGEST, None)?;
    let g2 = publish_generation(client, G2, G2_DIGEST, Some(g1.clone()))?;
    Ok(Activated { g0: None, g1, g2 })
}

pub(super) fn rollback_g2_to_g1(
    client: &QuantaIndex,
    activated: &Activated,
) -> Result<(), SdkError> {
    client
        .generations()
        .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: activated.g2.clone(),
            target: activated.g1.generation.clone(),
        })
        .map(|_ack| ())
}

/// The two track snapshots of one composite identity, with placeholder
/// roots: only its tracks are compared against a live identity, whose
/// roots the daemon sealed.
pub(super) fn composite_identity_for(
    repo_id: &str,
    revision_id: &str,
    raw_generation: u64,
    digest: &str,
) -> SearchCorpusGenerationIdentityV1 {
    SearchCorpusGenerationIdentityV1 {
        lexical: quanta_index_sdk::GenerationSnapshot {
            repo_id: RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new(revision_id)
                .expect("test fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: generation(raw_generation),
            manifest_digest: digest.to_string(),
        },
        semantic: quanta_index_sdk::GenerationSnapshot {
            repo_id: RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new(revision_id)
                .expect("test fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: generation(raw_generation),
            manifest_digest: digest.to_string(),
        },
        semantic_content: quanta_index_contract::SemanticContentRootsV1 {
            row_root_digest: format!("sha256:{:0>64x}", 0_u64),
            membership_root_digest: format!("sha256:{:0>64x}", 0_u64),
        },
    }
}

pub(super) fn current_composite(
    client: &QuantaIndex,
) -> Result<SearchCorpusGenerationIdentityV1, SdkError> {
    current_composite_for(client, REPO, REVISION)
}

/// Project the generation from the catalog-owned active head, avoiding a
/// second identity assembled from independently observed track/status reads.
pub(super) fn current_composite_for(
    client: &QuantaIndex,
    repo_id: &str,
    revision_id: &str,
) -> Result<SearchCorpusGenerationIdentityV1, SdkError> {
    let head = client
        .generations()
        .active_head(
            RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
            RevisionId::new(revision_id).expect("test fixture ID satisfies canonical policy"),
        )?
        .ok_or_else(|| SdkError::Protocol("expected an active search corpus head".to_string()))?;
    Ok(head.generation)
}
