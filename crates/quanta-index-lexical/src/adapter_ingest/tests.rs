//! Publication phase regression for the canonical file mutation plan.

use crate::publication_fixture::FixtureBatchBuildPort;
use quanta_index_contract::{
    BatchIngestMode, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchPlaneTrackKind, SourceFileCoverage,
    SourceFileKey, SourceFileRevision, SourcePublicationEvent, SymbolCoverage,
    SymbolNameSourcePolicyV1, source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::{
    LexicalDocumentNamespaceV1, LexicalDocumentVersionRangeV1, LexicalDocumentVersionReservationV1,
    PublicationValidationOwner, SearchCorpusBatchBuildPort, SearchCorpusPreflightPhaseV1,
    SourceEventAllocationRequestV1, SourceEventAllocationV1,
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

fn owner_with_original_reservation(batch: &SearchCorpusIngestBatch) -> PublicationValidationOwner {
    let incarnation = [0x4b; 16];
    let count = u64::try_from(batch.replace_scopes.len()).expect("file count");
    let allocation = SourceEventAllocationV1 {
        request: SourceEventAllocationRequestV1 {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            event: batch.source_event.clone(),
        },
        generation: batch.generation,
        root_incarnation: incarnation,
    };
    let reservation = LexicalDocumentVersionReservationV1 {
        allocation: allocation.clone(),
        file: LexicalDocumentVersionRangeV1 {
            first: 1,
            count,
            scope_sha256: LexicalDocumentVersionRangeV1::scope_digest(
                &incarnation,
                &batch.repo_id,
                LexicalDocumentNamespaceV1::File,
            ),
        },
        native_text: LexicalDocumentVersionRangeV1 {
            first: 0,
            count: 0,
            scope_sha256: LexicalDocumentVersionRangeV1::scope_digest(
                &incarnation,
                &batch.repo_id,
                LexicalDocumentNamespaceV1::NativeText,
            ),
        },
    };
    let mut owner = PublicationValidationOwner::default();
    owner
        .bind_source_allocation(batch, &allocation)
        .expect("source allocation");
    owner
        .bind_lexical_reservation(batch, &reservation)
        .expect("lexical reservation");
    owner
}

#[test]
fn first_reserved_publication_creates_missing_nested_track_root() {
    let state = tempfile::tempdir().expect("state");
    let state = std::fs::canonicalize(state.path()).expect("canonical state root");
    let track = state.join("indexes/lexical");
    let adapter = LexicalAdapter::with_state_root(track.clone());
    let request = batch(1, None, b"source");
    let mut owner = owner_with_original_reservation(&request);
    assert!(!track.exists());
    assert!(adapter.build_batch(&request, None).is_err());
    assert!(!track.exists(), "unreserved input must not create a track");
    assert!(
        adapter
            .build_batch_with_owner(&request, &mut owner, None)
            .expect("first publication")
            .is_some()
    );
    assert!(track.join("lexical-object-pool").is_dir());
}

#[test]
fn first_publication_refuses_non_directory_or_symlinked_track_parent() {
    use std::os::unix::fs::symlink;

    for link in [false, true] {
        let state = tempfile::tempdir().expect("state");
        let state = std::fs::canonicalize(state.path()).expect("canonical state root");
        let parent = state.join("indexes");
        if link {
            let outside = state.join("outside");
            std::fs::create_dir(&outside).expect("outside");
            symlink(&outside, &parent).expect("parent link");
        } else {
            std::fs::write(&parent, b"not a directory").expect("parent file");
        }
        let track = parent.join("lexical");
        let adapter = LexicalAdapter::with_state_root(track.clone());
        let request = batch(1, None, b"source");
        let mut owner = owner_with_original_reservation(&request);
        assert!(
            adapter
                .build_batch_with_owner(&request, &mut owner, None)
                .is_err()
        );
        assert!(!track.exists());
        assert!(!state.join("outside/lexical").exists());
    }
}

#[test]
fn first_publication_refuses_symlinked_track_root() {
    use std::os::unix::fs::symlink;

    let state = tempfile::tempdir().expect("state");
    let state = std::fs::canonicalize(state.path()).expect("canonical state root");
    let outside = state.join("outside");
    let track = state.join("lexical");
    std::fs::create_dir(&outside).expect("outside");
    symlink(&outside, &track).expect("track link");
    let adapter = LexicalAdapter::with_state_root(track);
    let request = batch(1, None, b"source");
    let mut owner = owner_with_original_reservation(&request);
    assert!(
        adapter
            .build_batch_with_owner(&request, &mut owner, None)
            .is_err()
    );
    assert!(!outside.join("lexical-object-pool").exists());
}

#[test]
fn unreserved_publication_refuses_before_creating_shared_storage() {
    let directory = tempfile::tempdir().expect("state");
    let adapter = LexicalAdapter::with_state_root(directory.path().to_path_buf());
    let result = quanta_index_core::SearchCorpusBatchBuildPort::build_batch(
        &adapter,
        &batch(1, None, b"source"),
        None,
    );
    assert!(matches!(
        result,
        Err(quanta_index_core::CoreError::InvalidContract(_))
    ));
    assert!(!directory.path().join("lexical-object-pool").exists());
}

#[test]
fn retention_measurement_includes_shared_closure_and_uncollected_objects() {
    use quanta_index_core::SealedGenerationReclaimPort as _;
    use std::collections::BTreeSet;

    let directory = tempfile::tempdir().expect("state");
    let adapter = LexicalAdapter::with_state_root(directory.path().to_path_buf());
    // Distinct fixed blocks keep physical deduplication from collapsing the
    // logical 128KiB fixture to one shared 32KiB block.
    let source = [
        vec![b'a'; 32 * 1024],
        vec![b'b'; 32 * 1024],
        vec![b'c'; 32 * 1024],
        vec![b'd'; 32 * 1024],
    ]
    .concat();
    let first = batch(1, None, &source);
    let _built = adapter
        .build_fixture_batch(&first)
        .expect("first publication");
    let generations = BTreeSet::from([first.generation]);
    let measured = adapter
        .measure_sealed_generations(&first.repo_id, &first.revision_id, &generations, None)
        .expect("measure closure");
    let generation_dir = adapter.index_path(&crate::GenKey {
        repo_id: first.repo_id.clone(),
        revision_id: first.revision_id.clone(),
        generation: first.generation,
    });
    let private = quanta_index_core::unique_inode_tree_bytes(
        &[generation_dir],
        &|name| crate::generation_dir::is_writer_lock_entry(name),
        None,
    )
    .expect("private bytes");
    assert!(measured.absent.is_empty());
    assert!(measured.bytes.saturating_sub(private) >= 128 * 1024);

    let before = adapter
        .measure_track_physical_bytes(None)
        .expect("whole track");
    let store = crate::lexical_store::ObjectStore::open_existing(directory.path()).expect("pool");
    let binding = crate::lexical_store::AttemptBinding {
        root_incarnation: [3; 16],
        repository_sha256: Sha256::digest(b"orphan-repo").into(),
        revision_sha256: Sha256::digest(b"orphan-revision").into(),
        source_event_sha256: [4; 32],
        target_generation: 99,
        base_root_sha256: None,
        operations_sha256: [5; 32],
        format: 17,
    };
    let lease = store.begin_attempt(&binding, &[]).expect("orphan lease");
    let orphan_bytes = [vec![b'z'; 32 * 1024], vec![b'w'; 32 * 1024]].concat();
    let orphan = store
        .put_blob(
            &lease,
            crate::lexical_store::ObjectKind::FileBody,
            &[],
            &orphan_bytes,
        )
        .expect("orphan body");
    let after = adapter
        .measure_track_physical_bytes(None)
        .expect("whole track");
    assert!(after.saturating_sub(before) >= orphan.bytes);
    let unchanged = adapter
        .measure_sealed_generations(&first.repo_id, &first.revision_id, &generations, None)
        .expect("only committed closure");
    assert_eq!(unchanged, measured);
}

#[test]
fn sealed_replay_and_maintenance_settle_crash_adoptions_before_collection() {
    for replay in [false, true] {
        let directory = tempfile::tempdir().expect("state");
        let adapter = LexicalAdapter::with_state_root(directory.path().to_path_buf());
        let request = batch(1, None, b"source");
        let _built = adapter
            .build_fixture_batch(&request)
            .expect("publish fixed corpus");
        let store =
            crate::lexical_store::ObjectStore::open_existing(directory.path()).expect("store");
        // Simulate an earlier same-event attempt abandoned before it adopted
        // the successful seal. This journal survives process destruction.
        let binding = crate::lexical_store::AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: Sha256::digest(request.repo_id.as_str().as_bytes()).into(),
            revision_sha256: Sha256::digest(request.revision_id.as_str().as_bytes()).into(),
            source_event_sha256: Sha256::digest(
                crate::channel_payloads::encode_cbor(&request.source_event, "event")
                    .expect("event"),
            )
            .into(),
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: request.source_event.payload_sha256,
            format: 17,
        };
        let abandoned = store
            .begin_attempt(&binding, &[])
            .expect("abandoned attempt");
        let orphan = store
            .put_blob(
                &abandoned,
                crate::lexical_store::ObjectKind::FileBody,
                &[],
                b"unpublished orphan",
            )
            .expect("orphan");
        drop(abandoned);
        drop(adapter);
        let adapter = LexicalAdapter::with_state_root(directory.path().to_path_buf());
        assert_eq!(
            store
                .pending_target_scopes()
                .expect("pending attempts")
                .len(),
            1
        );
        if replay {
            let result = adapter
                .build_fixture_batch(&request)
                .expect("sealed replay");
            assert!(result.is_none(), "sealed replay must not rebuild");
            assert!(
                store
                    .pending_target_scopes()
                    .expect("settled replay")
                    .is_empty()
            );
        }
        let mut completed = false;
        for _ in 0..1_000 {
            if quanta_index_core::IntegrityScrubPort::collect_shared_objects_step(&adapter, 32)
                .expect("maintenance recovery and GC")
                == quanta_index_core::SharedObjectCollectionStepV1::Completed
            {
                completed = true;
                break;
            }
        }
        assert!(
            completed,
            "bounded maintenance must finish this fixed fixture"
        );
        assert!(
            store
                .pending_target_scopes()
                .expect("settled maintenance")
                .is_empty()
        );
        assert!(
            store.read_blob(&orphan, 1024, None).is_err(),
            "unreachable abandoned payload is collected"
        );
        let key = super::GenKey {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            generation: request.generation,
        };
        let (file, _) = crate::lexical_publication::read_committed_roots(&adapter.index_path(&key))
            .expect("retained roots");
        let authority = crate::file_authority::paged::PagedFileAuthority::open(
            &file.encode().expect("root bytes"),
            store.clone(),
        )
        .expect("retained authority");
        let retained = authority
            .load_file_for_key(
                &request
                    .replace_scopes
                    .first()
                    .expect("fixed source scope")
                    .coverage
                    .source
                    .file,
                &quanta_index_core::RequestBudgetV1::unbounded(),
            )
            .expect("retained source");
        assert_eq!(&*retained.bytes, b"source");
    }
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
                .preflight_batch_with_owner(request, phase, &mut owner, None)
                .expect("preflight");
        }
        let _stages = adapter
            .build_fixture_batch_with_owner(request, &mut owner)
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
        let directory = adapter.index_path(&key);
        let bytes = crate::lexical_publication::read_root_bytes(
            &directory,
            crate::lexical_publication::FILE_ROOT_NAME,
        )
        .expect("published root");
        let authority = crate::file_authority::paged::PagedFileAuthority::open(
            &bytes,
            crate::lexical_store::ObjectStore::open_existing(adapter.state_root()).expect("store"),
        )
        .expect("file authority");
        let mut rows = Vec::new();
        authority
            .scan_sources(
                &quanta_index_core::RequestBudgetV1::unbounded(),
                &mut |metadata| {
                    rows.push(metadata.source);
                    Ok(())
                },
            )
            .expect("source scan");
        let mut expected = if request.base_generation.is_some() {
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
        rows.sort_by(|left, right| left.file.cmp(&right.file));
        expected.sort_by(|left, right| left.file.cmp(&right.file));
        assert_eq!(rows, expected);
    }
}
