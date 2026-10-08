//! Publication phase regression for the canonical file mutation plan.

use quanta_index_contract::{
    BatchIngestMode, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchPlaneTrackKind, SourceFileCoverage,
    SourceFileKey, SourceFileRevision, SourcePublicationEvent, SymbolCoverage,
    SymbolNameSourcePolicyV1, source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::{
    PublicationValidationOwner, SearchCorpusBatchBuildPort, SearchCorpusPreflightPhaseV1,
};
use sha2::{Digest as _, Sha256};

use crate::LexicalAdapter;
use crate::sealed_generation::publication_proof::PublicationProofs;

fn batch(generation: u64, base: Option<u64>, bytes: &[u8]) -> SearchCorpusIngestBatch {
    let scope = |path: &str, bytes: &[u8]| SearchCorpusReplaceScope {
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("repo").expect("repo"),
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new(format!("source-{generation}")).expect("revision"),
                source_sha256: Sha256::digest(bytes).into(),
            },
            language: quanta_index_contract::lex::LanguageCode::new("rust").expect("language"),
            producer_policy_sha256: [8; 32],
            symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: source_file_unit_set_sha256(&[], &[]).expect("unit commitment"),
            text_admitted: false,
            symbols: SymbolCoverage::NotRequested,
        },
        source_bytes: bytes.to_vec(),
        chunks: Vec::new(),
        symbols: Vec::new(),
    };
    let mut replace_scopes = vec![scope("a.rs", bytes)];
    if base.is_none() {
        replace_scopes.push(scope("keep.rs", b"unchanged"));
    }
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "plan-stream".into(),
            event_id: format!("event-{generation}"),
            expected_base_event_id: base.map(|base| format!("event-{base}")),
            payload_sha256: [0; 32],
        },
        repo_id: RepoId::new("repo").expect("repo"),
        revision_id: RevisionId::new("revision").expect("revision"),
        generation: ManifestGeneration::new(generation),
        base_generation: base.map(ManifestGeneration::new),
        manifest_digest: format!("manifest-{generation}"),
        batch_digest: "0".repeat(64),
        mode: if base.is_some() {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch).expect("event digest");
    batch
}

#[test]
fn publication_derives_file_plan_once_through_preflight_clone_and_writer() {
    let dir = tempfile::tempdir().expect("state");
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let original = batch(1, None, b"before");
    let changed = batch(2, Some(1), b"after");
    for request in [&original, &changed] {
        let mut owner = PublicationValidationOwner::default();
        for phase in [
            SearchCorpusPreflightPhaseV1::BeforeIntent,
            SearchCorpusPreflightPhaseV1::UnderOperationLock,
        ] {
            adapter
                .preflight_batch_with_owner(request, phase, &mut owner)
                .expect("preflight");
        }
        let _stages = adapter
            .build_batch_with_owner(request, &mut owner)
            .expect("build");
        let proofs = owner
            .proof_state::<PublicationProofs>(SearchPlaneTrackKind::Lexical)
            .expect("custody");
        assert_eq!(proofs.file_plan.preparations(), 1);
        let key = super::GenKey {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            generation: request.generation,
        };
        let rows = crate::file_authority::read_manifest(&adapter.index_path(&key))
            .expect("published manifest")
            .expect("file authority");
        let expected = if request.base_generation.is_some() {
            vec![
                changed
                    .replace_scopes
                    .first()
                    .expect("changed source")
                    .coverage
                    .source
                    .clone(),
                original
                    .replace_scopes
                    .get(1)
                    .expect("inherited source")
                    .coverage
                    .source
                    .clone(),
            ]
        } else {
            original
                .replace_scopes
                .iter()
                .map(|scope| scope.coverage.source.clone())
                .collect()
        };
        assert_eq!(
            rows.into_iter().map(|row| row.0).collect::<Vec<_>>(),
            expected
        );
    }
}
