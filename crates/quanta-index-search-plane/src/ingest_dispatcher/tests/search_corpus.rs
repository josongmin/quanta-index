#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning ingest tests use assertions as test-failure reporting"
)]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::{
    BatchIngestMode, ManifestGeneration, SearchPlaneTrackKind, SearchScopeSurface,
};
use quanta_index_core::{
    CoreError, IngestResourcePolicy, SearchCorpusIngestPort, SemanticIngestPort,
};

use crate::ingest_dispatcher::auxiliary::AuxiliaryMutationCoordinator;
use crate::ingest_dispatcher::errors::{
    ERR_SEARCH_CORPUS_BATCH_SHAPE, ERR_SEARCH_CORPUS_DELTA_BASE_NOT_SEALED,
    ERR_SEARCH_CORPUS_GENERATION_CONFLICT,
};
use crate::ingest_dispatcher::generation_plan::generation_pair_from_batch_v1;
use crate::ingest_dispatcher::search_corpus::{
    DirectSearchCorpusMaterializer, IngestResourceStats, SearchCorpusMaterializerParts,
};
use crate::ingest_dispatcher::semantic::DirectSemanticMaterializer;
use crate::ingest_dispatcher::tests::support::{
    FailingRetentionAuthority, FakeSearchCorpusBuilder, FakeSemanticBuilder, MismatchedGeneration,
    MismatchedSemanticIngest, PinnedLexicalHandle, RecordingIncompleteGenerationDiscard,
    RecordingSearchCorpusAuthority, ScriptedSealedReclaim, TestRes, ZeroMutationProbe,
    always_valid_generation, build_then_valid_generation, fixture_search_corpus_batch,
    incomplete_then_valid_generation, memory_aux_catalog, memory_catalog, multi_scope_corpus_batch,
    no_storage_sealed_reclaim, recording_search_corpus_authority, search_corpus_materializer,
    test_incomplete_generation_discard,
};
use crate::readiness::SearchCorpusHistoryRetentionReceiptV1;
use crate::{Ledger, SEARCH_OWNED_SEMANTIC_DIMENSION, SnapshotKey, SnapshotRegistries};

/// QI-BB-029: refused batches change nothing.
///
/// A batch the contract refuses, a delta on a base the ledger never
/// sealed, and a delta on a base whose physical identity disagrees with
/// the ledger all refuse before either builder or the authority is
/// touched.
#[test]
fn malformed_or_baseless_batches_change_zero_bytes() -> TestRes {
    let probe = ZeroMutationProbe::new(always_valid_generation());

    let mut malformed = fixture_search_corpus_batch()?;
    malformed.base_generation = Some(ManifestGeneration::new(3));
    match probe.materializer.publish_batch(&malformed) {
        Err(CoreError::Typed { code, .. }) if code == ERR_SEARCH_CORPUS_BATCH_SHAPE => {}
        other => return Err(format!("mode/base mismatch answered {other:?}").into()),
    }
    probe.assert_nothing_touched("mode/base mismatch")?;

    let mut empty_digest = fixture_search_corpus_batch()?;
    empty_digest.manifest_digest = String::new();
    match probe.materializer.publish_batch(&empty_digest) {
        Err(CoreError::Typed { code, .. }) if code == ERR_SEARCH_CORPUS_BATCH_SHAPE => {}
        other => return Err(format!("empty digest answered {other:?}").into()),
    }
    probe.assert_nothing_touched("empty digest")?;

    let mut unsealed_base = fixture_search_corpus_batch()?;
    unsealed_base.mode = BatchIngestMode::Delta;
    unsealed_base.base_generation = Some(ManifestGeneration::new(3));
    match probe.materializer.publish_batch(&unsealed_base) {
        Err(CoreError::Typed { code, .. }) if code == ERR_SEARCH_CORPUS_DELTA_BASE_NOT_SEALED => {}
        other => return Err(format!("unsealed base answered {other:?}").into()),
    }
    probe.assert_nothing_touched("base never sealed")?;

    // The ledger knows the base, but the physical identity disagrees.
    let mismatched = ZeroMutationProbe::new(Arc::new(MismatchedGeneration));
    mismatched
        .ledger
        .write()
        .map_err(|err| format!("ledger poisoned: {err}"))?
        .record_historically_sealed_search_corpus(
            &unsealed_base.repo_id,
            &unsealed_base.revision_id,
            ManifestGeneration::new(3),
            "manifest:base",
        );
    match mismatched.materializer.publish_batch(&unsealed_base) {
        Err(CoreError::Typed { code, .. }) if code == ERR_SEARCH_CORPUS_GENERATION_CONFLICT => {}
        other => return Err(format!("mismatched base answered {other:?}").into()),
    }
    mismatched.assert_nothing_touched("base identity mismatch")
}

/// QI-BB-021: a batch outside the resource envelope is refused typed
/// before any track is touched, and the same batch under an envelope it
/// fits is admitted and measured.
#[test]
fn a_batch_outside_the_resource_envelope_changes_zero_bytes() -> TestRes {
    let batch = multi_scope_corpus_batch()?;
    let embedded_records: usize = batch.replace_scopes.iter().map(|s| s.chunks.len()).sum();
    let text_bytes: u64 = batch
        .replace_scopes
        .iter()
        .flat_map(|scope| scope.chunks.iter().map(|chunk| chunk.text.len()))
        .map(u64::try_from)
        .sum::<Result<u64, _>>()?;
    let vector_bytes = u64::try_from(embedded_records * SEARCH_OWNED_SEMANTIC_DIMENSION * 4)?;

    let tight = ZeroMutationProbe::with_resource_policy(
        always_valid_generation(),
        IngestResourcePolicy::new(usize::MAX, u64::MAX, vector_bytes - 1)?,
    );
    match tight.materializer.publish_batch(&batch) {
        Err(CoreError::Typed { code, .. })
            if code == quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE => {}
        other => return Err(format!("oversized batch answered {other:?}").into()),
    }
    tight.assert_nothing_touched("resource envelope")?;
    let stats = tight.materializer.resource_stats()?;
    if stats
        != (IngestResourceStats {
            refused: 1,
            ..IngestResourceStats::default()
        })
    {
        return Err(format!("refusal must be counted and nothing admitted: {stats:?}").into());
    }

    let fits = ZeroMutationProbe::with_resource_policy(
        always_valid_generation(),
        IngestResourcePolicy::new(embedded_records, text_bytes, vector_bytes)?,
    );
    let _receipt = fits.materializer.publish_batch(&batch)?;
    let stats = fits.materializer.resource_stats()?;
    let expected = IngestResourceStats {
        admitted: 1,
        refused: 0,
        peak_embedded_records: embedded_records,
        peak_text_bytes: text_bytes,
        peak_vector_bytes: vector_bytes,
    };
    if stats != expected {
        return Err(format!("admitted footprint drifted: {stats:?} != {expected:?}").into());
    }
    Ok(())
}

/// The physical reclaim protocol under pins and orphans.
///
/// Every sealed directory the receipt does not retain is reclaimed —
/// including one whose authority record was reaped by an earlier pass
/// (the crash orphan) — except a generation a resident handle still
/// pins, which is deferred and reclaimed on the next pass once the pin
/// is gone.
#[test]
fn reclaim_sweeps_orphans_and_defers_pinned_generations() -> TestRes {
    let lexical_reclaim =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[1, 2, 3, 4, 5]);
    let semantic_reclaim =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Semantic, &[2, 3, 4, 5]);
    let snapshots = SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT);
    let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(
            Arc::new(FakeSemanticBuilder::default()),
            Arc::new(RwLock::new(Ledger::new())),
        ));
    let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
        SearchCorpusMaterializerParts {
            builder: Arc::new(FakeSearchCorpusBuilder::default()),
            ledger: lexical_ledger,
            semantic_ingest: semantic_materializer,
            semantic_embedder: Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            authority: Arc::new(RecordingSearchCorpusAuthority::default()),
            lexical_generation_validator: always_valid_generation(),
            semantic_generation_validator: always_valid_generation(),
            lexical_incomplete_discard: test_incomplete_generation_discard(),
            semantic_incomplete_discard: test_incomplete_generation_discard(),
            lexical_reclaim: lexical_reclaim.clone(),
            semantic_reclaim: semantic_reclaim.clone(),
            snapshots: snapshots.clone(),
            idempotency: memory_catalog(),
            resource_policy: IngestResourcePolicy::DEFAULT,
            auxiliary_catalog: memory_aux_catalog(),
            auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
        },
    );
    let mut batch = fixture_search_corpus_batch()?;
    batch.generation = ManifestGeneration::new(5);
    let receipt = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [ManifestGeneration::new(4), ManifestGeneration::new(5)],
    );

    // A query still holds generation 2 on the lexical track.
    let pinned_key = SnapshotKey::new(
        &batch.repo_id,
        &batch.revision_id,
        ManifestGeneration::new(2),
    );
    let pin = snapshots
        .lexical
        .acquire(&pinned_key, || {
            let handle: Arc<dyn quanta_index_core::LexicalSearcher> = Arc::new(PinnedLexicalHandle);
            Ok(crate::OpenedSnapshot {
                handle,
                resident_bytes: 1,
            })
        })?
        .handle;

    let first = materializer.reclaim_retired_generations_v1(&batch, &receipt)?;
    if lexical_reclaim.reclaimed() != [1, 3] || semantic_reclaim.reclaimed() != [2, 3] {
        return Err(format!(
            "first pass drifted: lexical={:?} semantic={:?}",
            lexical_reclaim.reclaimed(),
            semantic_reclaim.reclaimed()
        )
        .into());
    }
    if !first.deferred_pinned.contains(&(
        SearchPlaneTrackKind::Lexical,
        ManifestGeneration::new(2),
        1,
    )) {
        return Err(format!("pinned generation was not deferred: {first:?}").into());
    }
    if lexical_reclaim.remaining() != [2, 4, 5] {
        return Err(format!(
            "pinned generation 2 must survive the pass: {:?}",
            lexical_reclaim.remaining()
        )
        .into());
    }

    // Release the pin: the next pass reclaims the orphan it left behind.
    drop(pin);
    let second = materializer.reclaim_retired_generations_v1(&batch, &receipt)?;
    if !second.deferred_pinned.is_empty() || lexical_reclaim.remaining() != [4, 5] {
        return Err(format!(
            "second pass drifted: deferred={:?} remaining={:?}",
            second.deferred_pinned,
            lexical_reclaim.remaining()
        )
        .into());
    }
    Ok(())
}

#[test]
fn search_corpus_materializer_derives_search_owned_semantic_batch() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_ledger = Arc::new(RwLock::new(Ledger::new()));
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
        DirectSemanticMaterializer::new(semantic_builder.clone(), Arc::clone(&semantic_ledger)),
    );
    let search_corpus_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
    let authority = Arc::new(RecordingSearchCorpusAuthority::default());
    let materializer = search_corpus_materializer!(
        search_corpus_builder,
        Arc::clone(&lexical_ledger),
        semantic_materializer,
        Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        authority.clone(),
        build_then_valid_generation(),
        build_then_valid_generation(),
        test_incomplete_generation_discard(),
        test_incomplete_generation_discard(),
    );
    let mut batch = fixture_search_corpus_batch()?;
    batch.clear_surfaces = vec![SearchScopeSurface::Symbol];
    let receipt = materializer.publish_batch(&batch)?;
    if !receipt.sealed || receipt.accepted_clear_surfaces != 1 {
        return Err("derived semantic search-corpus receipt must preserve seal".into());
    }
    let semantic_batches = semantic_builder.take()?;
    let derived = semantic_batches
        .first()
        .ok_or_else(|| "expected one derived semantic batch".to_string())?;
    if derived.generation != batch.generation || !derived.seal {
        return Err("derived semantic batch lost generation/seal truth".into());
    }
    if derived.clear_surfaces != [SearchScopeSurface::Symbol] {
        return Err("derived semantic batch lost clear-surface truth".into());
    }
    if derived.replace_scopes.len() != 1 {
        return Err("derived semantic batch did not mirror search-corpus scope/chunk count".into());
    }
    let scope = derived
        .replace_scopes
        .first()
        .ok_or_else(|| "derived semantic batch missing replace scope".to_string())?;
    if scope.embeddings.len() != 1 {
        return Err("derived semantic batch did not mirror lexical scope/chunk count".into());
    }
    let embedding = scope
        .embeddings
        .first()
        .ok_or_else(|| "derived semantic batch missing embedding".to_string())?;
    if embedding.embedding_id.as_str() != "chunk-1" {
        return Err("derived semantic embedding_id must equal chunk_id".into());
    }
    if !embedding
        .embedding_input_digest
        .starts_with("search-owned-in:sha256:")
    {
        return Err(format!(
            "input digest must be content-hash based, got {}",
            embedding.embedding_input_digest
        )
        .into());
    }
    if !embedding
        .vector_digest
        .starts_with("search-owned-vec:sha256:")
    {
        return Err(format!(
            "vector digest must be vector-hash based, got {}",
            embedding.vector_digest
        )
        .into());
    }
    let recorded = authority
        .identities
        .lock()
        .map_err(|err| format!("recording authority poisoned: {err}"))?;
    if recorded.as_slice()
        != [(
            batch.repo_id.clone(),
            batch.revision_id.clone(),
            batch.generation,
            batch.manifest_digest,
        )]
    {
        return Err(format!("sealed composite authority was not recorded: {recorded:?}").into());
    }
    drop(recorded);
    Ok(())
}

#[test]
fn sealed_exact_retry_repairs_authority_without_rebuilding_tracks() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(
            semantic_builder.clone(),
            Arc::new(RwLock::new(Ledger::new())),
        ));
    let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let authority = Arc::new(RecordingSearchCorpusAuthority {
        identities: Mutex::new(Vec::new()),
        exact: true,
    });
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let materializer = search_corpus_materializer!(
        lexical_builder.clone(),
        Arc::clone(&ledger),
        semantic_materializer,
        Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        authority.clone(),
        always_valid_generation(),
        always_valid_generation(),
        test_incomplete_generation_discard(),
        test_incomplete_generation_discard(),
    );
    let batch = fixture_search_corpus_batch()?;
    let receipt = materializer.publish_batch(&batch)?;
    assert!(receipt.sealed);
    assert!(
        lexical_builder
            .batches
            .lock()
            .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
            .is_empty()
    );
    assert!(semantic_builder.take()?.is_empty());
    assert_eq!(
        authority
            .identities
            .lock()
            .map_err(|err| format!("recording authority poisoned: {err}"))?
            .len(),
        1
    );
    let guard = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?;
    guard.validate_historically_sealed_track_identity(
        &generation_pair_from_batch_v1(&batch).0,
        "test exact retry",
    )?;
    drop(guard);
    Ok(())
}

#[test]
fn durable_retention_error_fences_same_process_rollback_authority() -> TestRes {
    let batch = fixture_search_corpus_batch()?;
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    ledger
        .write()
        .map_err(|err| format!("ledger poisoned: {err}"))?
        .record_historically_sealed_search_corpus(
            &batch.repo_id,
            &batch.revision_id,
            batch.generation,
            &batch.manifest_digest,
        );
    let materializer = search_corpus_materializer!(
        Arc::new(FakeSearchCorpusBuilder::default()),
        Arc::clone(&ledger),
        Arc::new(DirectSemanticMaterializer::new(
            Arc::new(FakeSemanticBuilder::default()),
            Arc::new(RwLock::new(Ledger::new())),
        )),
        Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        Arc::new(FailingRetentionAuthority),
        always_valid_generation(),
        always_valid_generation(),
        test_incomplete_generation_discard(),
        test_incomplete_generation_discard(),
    );
    assert!(matches!(
        materializer.publish_batch(&batch),
        Err(CoreError::Storage(_))
    ));
    let rollback = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?
        .validate_historically_sealed_track_identity(
            &generation_pair_from_batch_v1(&batch).0,
            "retention failure",
        );
    assert!(matches!(rollback, Err(CoreError::NotReady(_))));
    Ok(())
}

#[test]
fn non_seal_batch_cannot_mutate_an_already_sealed_generation() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(
            semantic_builder.clone(),
            Arc::new(RwLock::new(Ledger::new())),
        ));
    let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let materializer = search_corpus_materializer!(
        lexical_builder.clone(),
        Arc::new(RwLock::new(Ledger::new())),
        semantic_materializer,
        Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        recording_search_corpus_authority(),
        always_valid_generation(),
        always_valid_generation(),
        test_incomplete_generation_discard(),
        test_incomplete_generation_discard(),
    );
    let mut batch = fixture_search_corpus_batch()?;
    batch.seal = false;
    let result = materializer.publish_batch(&batch);
    let Err(CoreError::Typed { code, .. }) = result else {
        return Err("non-seal mutation of sealed generation unexpectedly succeeded".into());
    };
    assert_eq!(code, "GENERATION_IMMUTABLE");
    assert!(
        lexical_builder
            .batches
            .lock()
            .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
            .is_empty()
    );
    assert!(semantic_builder.take()?.is_empty());
    Ok(())
}

#[test]
fn exact_lexical_missing_semantic_retry_builds_only_missing_track() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(
            semantic_builder.clone(),
            Arc::new(RwLock::new(Ledger::new())),
        ));
    let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let materializer = search_corpus_materializer!(
        lexical_builder.clone(),
        Arc::new(RwLock::new(Ledger::new())),
        semantic_materializer,
        Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        recording_search_corpus_authority(),
        always_valid_generation(),
        build_then_valid_generation(),
        test_incomplete_generation_discard(),
        test_incomplete_generation_discard(),
    );
    let receipt = materializer.publish_batch(&fixture_search_corpus_batch()?)?;
    assert!(receipt.sealed);
    assert!(
        lexical_builder
            .batches
            .lock()
            .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
            .is_empty()
    );
    assert_eq!(semantic_builder.take()?.len(), 1);
    Ok(())
}

#[test]
fn incomplete_lexical_exact_semantic_retry_discards_and_rebuilds_only_lexical() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(
            semantic_builder.clone(),
            Arc::new(RwLock::new(Ledger::new())),
        ));
    let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let lexical_discard = Arc::new(RecordingIncompleteGenerationDiscard::default());
    let materializer = search_corpus_materializer!(
        lexical_builder.clone(),
        Arc::new(RwLock::new(Ledger::new())),
        semantic_materializer,
        Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        recording_search_corpus_authority(),
        incomplete_then_valid_generation(),
        always_valid_generation(),
        lexical_discard.clone(),
        test_incomplete_generation_discard(),
    );

    let receipt = materializer.publish_batch(&fixture_search_corpus_batch()?)?;
    assert!(receipt.sealed);
    assert_eq!(lexical_discard.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        lexical_builder
            .batches
            .lock()
            .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
            .len(),
        1
    );
    assert!(semantic_builder.take()?.is_empty());
    Ok(())
}

#[test]
fn jointly_incomplete_tracks_keep_staged_data_for_normal_seal() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(
            semantic_builder.clone(),
            Arc::new(RwLock::new(Ledger::new())),
        ));
    let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let lexical_discard = Arc::new(RecordingIncompleteGenerationDiscard::default());
    let semantic_discard = Arc::new(RecordingIncompleteGenerationDiscard::default());
    let materializer = search_corpus_materializer!(
        lexical_builder.clone(),
        Arc::new(RwLock::new(Ledger::new())),
        semantic_materializer,
        Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        recording_search_corpus_authority(),
        incomplete_then_valid_generation(),
        incomplete_then_valid_generation(),
        lexical_discard.clone(),
        semantic_discard.clone(),
    );

    let receipt = materializer.publish_batch(&fixture_search_corpus_batch()?)?;

    assert!(receipt.sealed);
    assert_eq!(lexical_discard.calls.load(Ordering::SeqCst), 0);
    assert_eq!(semantic_discard.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        lexical_builder
            .batches
            .lock()
            .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
            .len(),
        1
    );
    assert_eq!(semantic_builder.take()?.len(), 1);
    Ok(())
}

#[test]
fn search_corpus_materializer_rejects_mismatched_semantic_receipt_before_authority_admission()
-> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let authority = Arc::new(RecordingSearchCorpusAuthority::default());
    let materializer = search_corpus_materializer!(
        Arc::new(FakeSearchCorpusBuilder::default()),
        Arc::clone(&ledger),
        Arc::new(MismatchedSemanticIngest),
        Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        authority.clone(),
        build_then_valid_generation(),
        build_then_valid_generation(),
        test_incomplete_generation_discard(),
        test_incomplete_generation_discard(),
    );
    let batch = fixture_search_corpus_batch()?;
    let result = materializer.publish_batch(&batch);
    assert!(matches!(result, Err(CoreError::InvalidContract(_))));
    assert!(
        authority
            .identities
            .lock()
            .map_err(|err| format!("recording authority poisoned: {err}"))?
            .is_empty()
    );
    let historical = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?
        .validate_historically_sealed_track_identity(
            &quanta_index_contract::GenerationSnapshot {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: batch.generation,
                manifest_digest: batch.manifest_digest,
            },
            "test",
        );
    assert!(matches!(historical, Err(CoreError::Typed { .. })));
    Ok(())
}
