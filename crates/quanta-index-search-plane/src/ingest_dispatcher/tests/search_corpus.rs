#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning ingest tests use assertions as test-failure reporting"
)]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::{
    BatchIngestMode, GenerationSnapshot, IngestOperationKindV1, ManifestGeneration, OwnerDocKind,
    RepoId, RevisionId, SearchPlaneTrackKind, SearchScopeSurface, SemanticCorpusKindV1,
    SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, IdempotencyCatalogPort as _, IdempotencyKeyV1,
    IngestResourcePolicy, RequestBudgetV1, SearchCorpusIngestPort, SemanticIngestPort,
    SemanticStreamWindowPolicy, SourcePublicationCatalogPort as _,
};

use crate::ingest_dispatcher::auxiliary::AuxiliaryMutationCoordinator;
use crate::ingest_dispatcher::generation_plan::{
    DeferredGcStep, PhysicalGenerationStateV1, SealedGenerationBuildPlanV1,
    batch_publish_receipt_v1, generation_pair_from_batch_v1,
};
use crate::ingest_dispatcher::search_corpus::{
    DirectSearchCorpusMaterializer, IngestResourceStats, SearchCorpusMaterializerParts,
};
use crate::ingest_dispatcher::semantic::DirectSemanticMaterializer;
use crate::ingest_dispatcher::tests::support::{
    CorruptTrackGeneration, FailingRetentionAuthority, FakeSearchCorpusBuilder,
    FakeSemanticBuilder, MemoryIdempotencyCatalog, MismatchedGeneration, MismatchedSemanticIngest,
    PinnedLexicalHandle, RecordingIncompleteGenerationDiscard, RecordingSearchCorpusAuthority,
    ScriptedSealedReclaim, TestRes, ZeroMutationProbe, always_valid_generation,
    build_then_valid_generation, fixture_search_corpus_batch, incomplete_then_valid_generation,
    memory_aux_catalog, memory_catalog, multi_scope_corpus_batch, no_storage_sealed_reclaim,
    recording_search_corpus_authority, restamp_search_corpus_fixture, search_corpus_materializer,
    test_incomplete_generation_discard,
};
use crate::readiness::SearchCorpusHistoryRetentionReceiptV1;
use crate::{
    Ledger, SEARCH_OWNED_SEMANTIC_DIMENSION, SnapshotKey, SnapshotRegistries,
    SnapshotRetirementOwner,
};

#[test]
fn search_corpus_receipt_exactly_acknowledges_semantic_replace_and_tombstone_mutations_v1()
-> TestRes {
    let mut batch = fixture_search_corpus_batch()?;
    let semantic_scope = SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        owner_kind: OwnerDocKind::Symbol,
        owner_id: "symbol:receipt-cardinality".to_string(),
    };
    batch.semantic_replace_scopes = vec![SemanticSourceReplaceScopeV1 {
        scope: semantic_scope.clone(),
        scope_digest: "scope:semantic:receipt-cardinality".to_string(),
        sources: Vec::new(),
        cluster_memberships: Vec::new(),
    }];
    let mut retired_scope = semantic_scope;
    retired_scope.owner_id = "symbol:retired-receipt-cardinality".into();
    batch.semantic_tombstone_scopes = vec![retired_scope];
    restamp_search_corpus_fixture(&mut batch)?;
    batch.validate_v1()?;
    batch
        .validate_surface_mutations_v1()
        .map_err(|error| error.to_string())?;

    let receipt = batch_publish_receipt_v1(&batch);
    if receipt.accepted_semantic_replace_scopes != 1
        || receipt.accepted_semantic_tombstone_scopes != 1
    {
        return Err(
            format!("search-corpus receipt lost semantic mutation partition: {receipt:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn delta_finalization_inherits_untouched_chunk_authority_and_retries_after_base_retirement()
-> TestRes {
    let probe = ZeroMutationProbe::new(always_valid_generation());
    let base = multi_scope_corpus_batch()?;
    let retain_base = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &base.repo_id,
        &base.revision_id,
        [base.generation],
    );
    probe
        .materializer
        .finalize_generation_v1(&base, Some(&retain_base))?;
    let mut delta = base.clone();
    delta.generation = ManifestGeneration::new(base.generation.get() + 1);
    delta.base_generation = Some(base.generation);
    delta.mode = BatchIngestMode::Delta;
    delta.source_event.expected_base_event_id = Some(base.source_event.event_id.clone());
    delta.source_event.event_id = "next-event".into();
    delta.manifest_digest = "manifest:delta".into();
    delta.replace_scopes.truncate(1);
    for chunk in &mut delta
        .replace_scopes
        .first_mut()
        .ok_or("fixture a.rs")?
        .chunks
    {
        chunk.chunk_id =
            quanta_index_contract::ChunkId::new(format!("new-{}", chunk.chunk_id.as_str()));
    }
    delta.tombstone_scopes = vec![quanta_index_contract::SearchCorpusTombstoneScope {
        file: base
            .replace_scopes
            .get(2)
            .ok_or("fixture c.rs")?
            .coverage
            .source
            .file
            .clone(),
    }];
    delta.semantic_replace_scopes.clear();
    restamp_search_corpus_fixture(&mut delta)?;
    let retain_delta = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &delta.repo_id,
        &delta.revision_id,
        [delta.generation],
    );
    probe.auxiliary_catalog.fail_next_apply();
    assert!(
        probe
            .materializer
            .finalize_generation_v1(&delta, Some(&retain_delta))
            .is_err()
    );
    {
        let guard = probe.ledger.read().map_err(|e| e.to_string())?;
        assert!(
            guard
                .structural_state(&base.repo_id, &base.revision_id, base.generation)
                .is_some()
        );
        assert!(
            guard
                .structural_state(&delta.repo_id, &delta.revision_id, delta.generation)
                .is_none()
        );
        drop(guard);
    }
    // The retention transaction can retire the original base. Reconciliation
    // must retain the complete already-published target chunk universe.
    for attempt in 0..2 {
        probe
            .materializer
            .finalize_generation_v1(&delta, Some(&retain_delta))?;
        let guard = probe.ledger.read().map_err(|e| e.to_string())?;
        let state = guard
            .structural_state(&delta.repo_id, &delta.revision_id, delta.generation)
            .ok_or("missing delta chunk authority")?;
        let ids: Vec<_> = state
            .chunks()
            .keys()
            .map(quanta_index_contract::ChunkId::as_str)
            .collect();
        assert_eq!(ids, vec!["b-1", "new-a-1", "new-a-2"]);
        assert_eq!(
            state.source_batch_digest(),
            Some(delta.batch_digest.as_str())
        );
        drop(guard);
        if attempt == 0 {
            use crate::ingest_dispatcher::ports::StructuralIngestPort as _;
            // A separate parse-tree update must preserve the chunk completion
            // marker in both live state and the persisted state-meta row.
            let structural = crate::ingest_dispatcher::auxiliary::DirectStructuralMaterializer::new(
                crate::ingest_dispatcher::auxiliary::AuxiliaryMaterializerParts {
                    catalog: probe.auxiliary_catalog.clone(),
                    ledger: probe.ledger.clone(),
                    coordinator: AuxiliaryMutationCoordinator::shared(),
                },
            );
            let _receipt =
                structural.publish_batch(&quanta_index_contract::StructuralIngestBatch {
                    repo_id: delta.repo_id.clone(),
                    revision_id: delta.revision_id.clone(),
                    generation: delta.generation,
                    base_generation: None,
                    manifest_digest: delta.manifest_digest.clone(),
                    batch_digest: "structural:clear".into(),
                    mode: BatchIngestMode::ReplaceGeneration,
                    replace_scopes: Vec::new(),
                    tombstone_scopes: vec![quanta_index_contract::StructuralTombstoneScope {
                        scope: quanta_index_contract::SearchScopeKey {
                            doc_surface: SearchScopeSurface::Chunk,
                            repo_relative_path: delta
                                .replace_scopes
                                .first()
                                .ok_or("a.rs")?
                                .coverage
                                .source
                                .file
                                .repo_relative_path
                                .clone(),
                        },
                    }],
                    seal: false,
                })?;
            let mut restored = Ledger::new();
            let _rows = crate::readiness::restore_auxiliary_rows_into(
                &mut restored,
                probe.auxiliary_catalog.as_ref(),
            )?;
            assert!(
                restored
                    .structural_state(&base.repo_id, &base.revision_id, base.generation)
                    .is_none()
            );
            assert_eq!(
                restored
                    .structural_state(&delta.repo_id, &delta.revision_id, delta.generation)
                    .ok_or("restored delta")?
                    .source_batch_digest(),
                Some(delta.batch_digest.as_str())
            );
            *probe.ledger.write().map_err(|e| e.to_string())? = restored;
        }
    }
    Ok(())
}

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
    match probe
        .materializer
        .publish_batch(&malformed, &RequestBudgetV1::unbounded())
    {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusBatchShapeInvalid,
            ..
        }) => {}
        other => return Err(format!("mode/base mismatch answered {other:?}").into()),
    }
    probe.assert_nothing_touched("mode/base mismatch")?;

    let mut empty_digest = fixture_search_corpus_batch()?;
    empty_digest.manifest_digest = String::new();
    match probe
        .materializer
        .publish_batch(&empty_digest, &RequestBudgetV1::unbounded())
    {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusBatchShapeInvalid,
            ..
        }) => {}
        other => return Err(format!("empty digest answered {other:?}").into()),
    }
    probe.assert_nothing_touched("empty digest")?;

    let mut unsealed_base = fixture_search_corpus_batch()?;
    unsealed_base.mode = BatchIngestMode::Delta;
    unsealed_base.base_generation = Some(ManifestGeneration::new(3));
    restamp_search_corpus_fixture(&mut unsealed_base)?;
    unsealed_base.validate_v1()?;
    match probe
        .materializer
        .publish_batch(&unsealed_base, &RequestBudgetV1::unbounded())
    {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusDeltaBaseNotSealed,
            ..
        }) => {}
        other => return Err(format!("unsealed base answered {other:?}").into()),
    }
    probe.assert_nothing_touched("base never sealed")?;

    probe
        .ledger
        .write()
        .map_err(|e| e.to_string())?
        .record_historically_sealed_search_corpus(
            &unsealed_base.repo_id,
            &unsealed_base.revision_id,
            ManifestGeneration::new(3),
            "manifest:base",
        );
    for result in [
        probe.materializer.preflight_batch(&unsealed_base),
        probe
            .materializer
            .publish_batch(&unsealed_base, &RequestBudgetV1::unbounded())
            .map(|_| ()),
    ] {
        assert!(matches!(result, Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusDeltaBaseNotSealed,
            message,
        }) if message.contains("complete chunk authority")));
    }
    probe.assert_nothing_touched("base has no complete chunk authority")?;

    // The ledger knows the base, but the physical identity disagrees: the
    // base is sealed under another digest, so it is refused with the
    // repair to perform rather than as an opaque conflict.
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
    match mismatched
        .materializer
        .publish_batch(&unsealed_base, &RequestBudgetV1::unbounded())
    {
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationRepairRequired,
            ..
        }) => {}
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
    let embedded_records: usize = batch
        .semantic_replace_scopes
        .iter()
        .map(|scope| scope.sources.len())
        .sum();
    let carried_records = embedded_records
        + batch
            .replace_scopes
            .iter()
            .map(|scope| scope.chunks.len())
            .sum::<usize>();
    let text_bytes: u64 = batch
        .semantic_replace_scopes
        .iter()
        .flat_map(|scope| scope.sources.iter().map(|source| source.text.len()))
        .map(u64::try_from)
        .sum::<Result<u64, _>>()?;
    let source_bytes: u64 = batch
        .replace_scopes
        .iter()
        .map(|scope| u64::try_from(scope.source_bytes.len()))
        .sum::<Result<u64, _>>()?;
    let vector_bytes = u64::try_from(embedded_records * SEARCH_OWNED_SEMANTIC_DIMENSION * 4)?;

    let tight = ZeroMutationProbe::with_resource_policy(
        always_valid_generation(),
        IngestResourcePolicy::new(usize::MAX, u64::MAX, vector_bytes - 1)?,
    );
    // The preflight is the admission point (it tallies); the publish
    // repeats the measurement under its lock and refuses the same way.
    match tight.materializer.preflight_batch(&batch) {
        Err(CoreError::Typed { code, .. })
            if code == quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE => {}
        other => return Err(format!("oversized batch preflight answered {other:?}").into()),
    }
    match tight
        .materializer
        .publish_batch(&batch, &RequestBudgetV1::unbounded())
    {
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
        IngestResourcePolicy::new(carried_records, text_bytes.max(source_bytes), vector_bytes)?,
    );
    fits.materializer.preflight_batch(&batch)?;
    let _receipt = fits
        .materializer
        .publish_batch(&batch, &RequestBudgetV1::unbounded())?;
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

/// One idempotency record per `(kind, generation)`, finalized, so the
/// sweep has something to reconcile against disk.
fn seed_records(
    catalog: &MemoryIdempotencyCatalog,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    generations: &[u64],
) -> TestRes {
    for generation in generations {
        for kind in [
            IngestOperationKindV1::SearchCorpus,
            IngestOperationKindV1::History,
        ] {
            let key = IdempotencyKeyV1 {
                kind,
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                generation: ManifestGeneration::new(*generation),
                batch_digest: format!("{kind}:{generation}"),
            };
            let body = [u8::try_from(*generation)?; 32];
            let claim = match catalog.claim_prepared(&key, &body, "test-seed", u64::MAX, &body)? {
                quanta_index_core::ClaimOutcomeV1::Claimed(claim) => claim,
                quanta_index_core::ClaimOutcomeV1::Replay { .. }
                | quanta_index_core::ClaimOutcomeV1::ReplayRepoMap { .. } => {
                    return Err("seed claim unexpectedly replayed".into());
                }
            };
            let receipt = quanta_index_contract::BatchPublishReceipt::empty_for(
                ManifestGeneration::new(*generation),
                None,
                key.batch_digest.clone(),
            );
            let _sequence = catalog.commit(&claim, &receipt)?;
        }
    }
    Ok(())
}

/// A search-corpus materializer over fakes whose sealed-generation
/// reclaim and idempotency catalog are the given scripted ones.
fn reclaim_materializer(
    lexical_reclaim: &Arc<ScriptedSealedReclaim>,
    semantic_reclaim: &Arc<ScriptedSealedReclaim>,
    idempotency: &Arc<MemoryIdempotencyCatalog>,
) -> DirectSearchCorpusMaterializer {
    reclaim_materializer_with_snapshots(
        lexical_reclaim,
        semantic_reclaim,
        idempotency,
        SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
    )
}

fn reclaim_materializer_with_snapshots(
    lexical_reclaim: &Arc<ScriptedSealedReclaim>,
    semantic_reclaim: &Arc<ScriptedSealedReclaim>,
    idempotency: &Arc<MemoryIdempotencyCatalog>,
    snapshots: SnapshotRegistries,
) -> DirectSearchCorpusMaterializer {
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
        DirectSemanticMaterializer::new(Arc::new(FakeSemanticBuilder::default())),
    );
    DirectSearchCorpusMaterializer::new_with_search_owned_semantics(SearchCorpusMaterializerParts {
        builder: Arc::new(FakeSearchCorpusBuilder::default()),
        ledger: Arc::new(RwLock::new(Ledger::new())),
        semantic_ingest: semantic_materializer,
        semantic_embedder: Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        authority: Arc::new(RecordingSearchCorpusAuthority::default()),
        lexical_generation_validator: always_valid_generation(),
        semantic_generation_validator: always_valid_generation(),
        semantic_content_roots: crate::content_roots_test_support::generation_keyed_content_roots(),
        lexical_incomplete_discard: test_incomplete_generation_discard(),
        semantic_incomplete_discard: test_incomplete_generation_discard(),
        lexical_reclaim: lexical_reclaim.clone(),
        semantic_reclaim: semantic_reclaim.clone(),
        snapshots,
        source_publication: super::support::test_source_catalog(),
        idempotency: idempotency.clone(),
        resource_policy: IngestResourcePolicy::DEFAULT,
        semantic_stream_policy: SemanticStreamWindowPolicy::DEFAULT,
        source_egress_policy: None,
        auxiliary_catalog: memory_aux_catalog(),
        auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
    })
}

#[test]
fn a_post_move_gc_failure_keeps_cleanup_deferred_without_stranding_its_fence() -> TestRes {
    let lexical_reclaim = ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[1, 2]);
    let semantic_reclaim = ScriptedSealedReclaim::new(SearchPlaneTrackKind::Semantic, &[1, 2]);
    lexical_reclaim.fail_after_move_reclaim_of(1)?;
    let snapshots = SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT);
    let materializer = reclaim_materializer_with_snapshots(
        &lexical_reclaim,
        &semantic_reclaim,
        &memory_catalog(),
        snapshots.clone(),
    );
    let mut batch = fixture_search_corpus_batch()?;
    batch.generation = ManifestGeneration::new(2);
    let retention = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [batch.generation],
    );
    let receipt = materializer.reclaim_retired_generations_v1(&batch, &retention)?;
    if !receipt.deferred.contains(&DeferredGcStep::Reclaim(
        SearchPlaneTrackKind::Lexical,
        ManifestGeneration::new(1),
    )) || lexical_reclaim.remaining() != [2]
        || lexical_reclaim.interrupted_left() != 1
    {
        return Err(format!("post-move cleanup was not deferred: {receipt:?}").into());
    }
    let key = SnapshotKey::new(
        &batch.repo_id,
        &batch.revision_id,
        ManifestGeneration::new(1),
    );
    let proof = snapshots.lexical.begin_promotion(&key)?;
    drop(proof);
    Ok(())
}

/// Every step of a physical reclaim pass that fails after the seal is
/// durable fails nothing, is counted once, and is redone by the next pass
/// (QI-BB-020).
///
/// Both tracks hold sealed generations 1–4 and the catalog holds records
/// for 1–3; every pass seals 4 retaining 3 and 4, so 1 and 2 are retired.
///
/// 1. The lexical reclaim of 1 fails, and the semantic listing fails in
///    both of the pass's listings: lexical 2 is reclaimed, semantic
///    nothing, and only 2's records go — the lexical listing proves 2
///    broken, while 1, still held by lexical, proves nothing without the
///    semantic listing.
/// 2. The record forget fails: both tracks reclaim what is left of 1 and
///    2, and 1's records stay.
/// 3. The record listing fails: 1's records still stay.
/// 4. Nothing fails: 1's records go.
///
/// Every seal succeeds, and the failure counter moves by exactly the
/// steps each pass deferred: 2, 1, 1, then 0.
#[test]
fn a_reclaim_pass_step_failing_after_the_seal_is_durable_is_counted_and_redone() -> TestRes {
    let lexical_reclaim = ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[1, 2, 3, 4]);
    let semantic_reclaim =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Semantic, &[1, 2, 3, 4]);
    let idempotency = memory_catalog();
    let materializer = reclaim_materializer(&lexical_reclaim, &semantic_reclaim, &idempotency);
    let mut batch = fixture_search_corpus_batch()?;
    batch.generation = ManifestGeneration::new(4);
    seed_records(&idempotency, &batch.repo_id, &batch.revision_id, &[1, 2, 3])?;
    let retention = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [ManifestGeneration::new(3), ManifestGeneration::new(4)],
    );
    // (failures, lexical on disk, semantic on disk, generations with records)
    let pass = |step: &str, expected: (u64, &[u64], &[u64], &[u64])| -> TestRes {
        materializer.finalize_generation_v1(&batch, Some(&retention))?;
        let failures = materializer.gc_stats()?.failures;
        let lexical = lexical_reclaim.remaining();
        let semantic = semantic_reclaim.remaining();
        let records = idempotency.generations_with_records();
        if (
            failures,
            lexical.as_slice(),
            semantic.as_slice(),
            records.as_slice(),
        ) != expected
        {
            return Err(format!(
                "{step}: failures={failures} lexical={lexical:?} semantic={semantic:?} records={records:?}, expected {expected:?}"
            )
            .into());
        }
        Ok(())
    };

    lexical_reclaim.fail_next_reclaim_of(1)?;
    semantic_reclaim.fail_next_listings(2);
    pass(
        "a failed reclaim and a track that cannot be listed",
        (2, &[1, 3, 4], &[1, 2, 3, 4], &[1, 3]),
    )?;
    idempotency.fail_next_forget();
    pass("a failed record forget", (3, &[3, 4], &[3, 4], &[1, 3]))?;
    idempotency.fail_next_listing();
    pass("a failed record listing", (4, &[3, 4], &[3, 4], &[1, 3]))?;
    pass(
        "a clean pass redoes what is left",
        (4, &[3, 4], &[3, 4], &[3]),
    )?;
    Ok(())
}

/// A refusal the reclaim pass meets is a finding, never a deferred step
/// (QI-BB-020, §3.49).
///
/// The same pair as above, with the lexical listing refused as a directory
/// whose identity contradicts its path is: the pass fails closed with the
/// refusal's code, reclaims and forgets nothing, and counts no failure.
#[test]
fn a_refusal_met_by_the_reclaim_pass_fails_it_closed() -> TestRes {
    let lexical_reclaim = ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[1, 2, 3, 4]);
    let semantic_reclaim =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Semantic, &[1, 2, 3, 4]);
    let idempotency = memory_catalog();
    let materializer = reclaim_materializer(&lexical_reclaim, &semantic_reclaim, &idempotency);
    let mut batch = fixture_search_corpus_batch()?;
    batch.generation = ManifestGeneration::new(4);
    seed_records(&idempotency, &batch.repo_id, &batch.revision_id, &[1, 2, 3])?;
    let retention = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [ManifestGeneration::new(3), ManifestGeneration::new(4)],
    );

    lexical_reclaim.refuse_next_listing();
    let refused = materializer.finalize_generation_v1(&batch, Some(&retention));
    if !matches!(&refused, Err(CoreError::Typed { code, .. })
        if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityScopeMismatch)
    {
        return Err(format!("the refusal fails the pass closed, got {refused:?}").into());
    }
    let failures = materializer.gc_stats()?.failures;
    let untouched = (
        lexical_reclaim.remaining(),
        semantic_reclaim.remaining(),
        idempotency.generations_with_records(),
    );
    if failures != 0 || untouched != (vec![1, 2, 3, 4], vec![1, 2, 3, 4], vec![1, 2, 3]) {
        return Err(format!(
            "a refused pass defers, reclaims and forgets nothing: failures={failures} {untouched:?}"
        )
        .into());
    }
    Ok(())
}

/// Every reclaim pass first finishes what interrupted reclaims left in each
/// track's reclaim area (QI-BB-003 보완 #3, #4).
///
/// Nothing is retired — both tracks hold only the retained 3 and 4 — but
/// the lexical area holds two interrupted reclaims and the semantic one.
///
/// 1. The lexical finish fails: the seal stands, the failure is counted
///    once, the semantic entry is finished, and the lexical entries stay
///    for the next pass.
/// 2. Nothing fails: the lexical entries are finished too.
///
/// Every finished entry counts with its track's reclaimed generations and
/// bytes, exactly once, and no retained generation is touched.
#[test]
fn every_pass_first_finishes_what_interrupted_reclaims_left() -> TestRes {
    // (failures, finished, lexical (generations, bytes), semantic
    // (generations, bytes), entries left (lexical, semantic))
    type Tally = (u64, u64, (u64, u64), (u64, u64), (u64, u64));
    let lexical_reclaim = ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[3, 4]);
    let semantic_reclaim = ScriptedSealedReclaim::new(SearchPlaneTrackKind::Semantic, &[3, 4]);
    let idempotency = memory_catalog();
    let materializer = reclaim_materializer(&lexical_reclaim, &semantic_reclaim, &idempotency);
    let mut batch = fixture_search_corpus_batch()?;
    batch.generation = ManifestGeneration::new(4);
    let retention = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [ManifestGeneration::new(3), ManifestGeneration::new(4)],
    );
    lexical_reclaim.leave_interrupted(2);
    semantic_reclaim.leave_interrupted(1);
    // The scripted reclaim reports 10 bytes per finished entry.
    let pass = |step: &str, expected: Tally| -> TestRes {
        materializer.finalize_generation_v1(&batch, Some(&retention))?;
        let gc = materializer.gc_stats()?;
        let observed: Tally = (
            gc.failures,
            gc.interrupted_reclaims_finished,
            (gc.lexical_reclaimed_generations, gc.lexical_reclaimed_bytes),
            (
                gc.semantic_reclaimed_generations,
                gc.semantic_reclaimed_bytes,
            ),
            (
                lexical_reclaim.interrupted_left(),
                semantic_reclaim.interrupted_left(),
            ),
        );
        if observed != expected {
            return Err(format!("{step}: {observed:?}, expected {expected:?}").into());
        }
        Ok(())
    };

    lexical_reclaim.fail_next_finish();
    pass(
        "a lexical finish the storage failed",
        (1, 1, (0, 0), (1, 10), (2, 0)),
    )?;
    pass(
        "the next pass finishes the rest",
        (1, 3, (2, 20), (1, 10), (0, 0)),
    )?;
    let untouched = (
        lexical_reclaim.remaining(),
        semantic_reclaim.remaining(),
        lexical_reclaim.reclaimed(),
        semantic_reclaim.reclaimed(),
    );
    if untouched != (vec![3, 4], vec![3, 4], Vec::new(), Vec::new()) {
        return Err(format!("the retained generations are untouched: {untouched:?}").into());
    }
    Ok(())
}

/// The physical reclaim protocol under pins and orphans, and the
/// idempotency records that live and die with each generation.
///
/// Every sealed directory the receipt does not retain is reclaimed —
/// including one whose authority record was reaped by an earlier pass
/// (the crash orphan) — except a generation a resident handle still
/// pins, which is deferred and reclaimed on the next pass once the pin
/// is gone. Records go with the first pass that leaves the generation
/// less than a whole pair: the orphan's, the reclaimed pair's, and the
/// deferred-split generation's (its semantic half went in pass one) —
/// each exactly once across both passes — as do the records of a
/// generation that never sealed on either track; the retained and the
/// sealing generations keep theirs.
#[test]
fn reclaim_sweeps_orphans_and_defers_pinned_generations() -> TestRes {
    let lexical_reclaim =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[1, 2, 3, 4, 5]);
    let semantic_reclaim =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Semantic, &[2, 3, 4, 5]);
    let snapshots = SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT);
    let catalog = memory_catalog();
    let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
        DirectSemanticMaterializer::new(Arc::new(FakeSemanticBuilder::default())),
    );
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
            semantic_content_roots:
                crate::content_roots_test_support::generation_keyed_content_roots(),
            lexical_incomplete_discard: test_incomplete_generation_discard(),
            semantic_incomplete_discard: test_incomplete_generation_discard(),
            lexical_reclaim: lexical_reclaim.clone(),
            semantic_reclaim: semantic_reclaim.clone(),
            snapshots: snapshots.clone(),
            source_publication: super::support::test_source_catalog(),
            idempotency: catalog.clone(),
            resource_policy: IngestResourcePolicy::DEFAULT,
            semantic_stream_policy: SemanticStreamWindowPolicy::DEFAULT,
            source_egress_policy: None,
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
    // Generation 0 never sealed on either track (a batch that crashed or
    // was rejected mid-apply); 1..=5 are the generations on disk above.
    seed_records(
        &catalog,
        &batch.repo_id,
        &batch.revision_id,
        &[0, 1, 2, 3, 4, 5],
    )?;

    // A query still holds generation 2 on the lexical track.
    let pinned_key = SnapshotKey::new(
        &batch.repo_id,
        &batch.revision_id,
        ManifestGeneration::new(2),
    );
    let pin = snapshots
        .lexical
        .acquire(&pinned_key, &RequestBudgetV1::unbounded(), || {
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
    // Records: 0 (never sealed), 1 (lexical-only orphan, reclaimed), 3
    // (reclaimed on both tracks) and 2 (semantic half reclaimed, lexical
    // half deferred: no longer whole) are forgotten, two records each;
    // 4 (retained) and 5 (sealing) keep theirs.
    let expected_first: Vec<(u64, u64)> = vec![(0, 2), (1, 2), (2, 2), (3, 2)];
    if catalog.forgets() != expected_first {
        return Err(format!(
            "first pass must forget the generations that are no longer whole pairs, once each: {:?}",
            catalog.forgets()
        )
        .into());
    }
    if first
        .forgotten_records
        .iter()
        .map(|(generation, records)| (generation.get(), *records))
        .collect::<Vec<_>>()
        != expected_first
    {
        return Err(format!("the receipt must report the forgets: {first:?}").into());
    }
    for kept in [4, 5] {
        if catalog.records_for_generation(ManifestGeneration::new(kept)) != 2 {
            return Err(format!("generation {kept} must keep its records").into());
        }
    }
    if catalog.records() != 4 {
        return Err(format!("expected 4 records left, found {}", catalog.records()).into());
    }

    // Release the pin: the next pass reclaims the orphan it left behind
    // and has nothing left to forget for it.
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
    if catalog.forgets() != expected_first || !second.forgotten_records.is_empty() {
        return Err(format!(
            "the second pass must not forget again: {:?} / {:?}",
            catalog.forgets(),
            second.forgotten_records
        )
        .into());
    }
    Ok(())
}

/// A retained generation whose pair is no longer whole on disk loses its
/// records too.
///
/// With one track discarded, a replay of its batch applies afresh instead
/// of being acked from a record that describes half a pair.
#[test]
fn a_retained_half_pair_loses_its_records() -> TestRes {
    let lexical_reclaim = ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[4, 5]);
    // Generation 4's semantic half is gone.
    let semantic_reclaim = ScriptedSealedReclaim::new(SearchPlaneTrackKind::Semantic, &[5]);
    let catalog = memory_catalog();
    let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
        SearchCorpusMaterializerParts {
            builder: Arc::new(FakeSearchCorpusBuilder::default()),
            ledger: Arc::new(RwLock::new(Ledger::new())),
            semantic_ingest: Arc::new(DirectSemanticMaterializer::new(Arc::new(
                FakeSemanticBuilder::default(),
            ))),
            semantic_embedder: Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            authority: Arc::new(RecordingSearchCorpusAuthority::default()),
            lexical_generation_validator: always_valid_generation(),
            semantic_generation_validator: always_valid_generation(),
            semantic_content_roots:
                crate::content_roots_test_support::generation_keyed_content_roots(),
            lexical_incomplete_discard: test_incomplete_generation_discard(),
            semantic_incomplete_discard: test_incomplete_generation_discard(),
            lexical_reclaim: lexical_reclaim.clone(),
            semantic_reclaim: semantic_reclaim.clone(),
            snapshots: SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
            source_publication: super::support::test_source_catalog(),
            idempotency: catalog.clone(),
            resource_policy: IngestResourcePolicy::DEFAULT,
            semantic_stream_policy: SemanticStreamWindowPolicy::DEFAULT,
            source_egress_policy: None,
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
    seed_records(&catalog, &batch.repo_id, &batch.revision_id, &[4, 5])?;
    let pass = materializer.reclaim_retired_generations_v1(&batch, &receipt)?;
    if !lexical_reclaim.reclaimed().is_empty() || !semantic_reclaim.reclaimed().is_empty() {
        return Err("retained generations are never reclaimed".into());
    }
    if catalog.forgets() != vec![(4, 2)] || pass.forgotten_records.len() != 1 {
        return Err(format!(
            "the half pair's records must go and the whole pair's stay: {:?}",
            catalog.forgets()
        )
        .into());
    }
    if catalog.records_for_generation(ManifestGeneration::new(5)) != 2 {
        return Err("the sealing generation keeps its records".into());
    }
    Ok(())
}

/// A validator that reports one track's generation as sealed but damaged
/// for as long as the scripted reclaim still holds it, and exact once it
/// has been reclaimed and rebuilt.
struct CorruptUntilReclaimed {
    track: SearchPlaneTrackKind,
    reclaim: Arc<ScriptedSealedReclaim>,
}

impl GenerationIdentityValidatePort for CorruptUntilReclaimed {
    fn validate_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        if candidate.track == self.track
            && self
                .reclaim
                .remaining()
                .contains(&candidate.manifest_generation.get())
        {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                message: format!(
                    "injected sidecar damage on {:?} generation {}",
                    candidate.track,
                    candidate.manifest_generation.get()
                ),
            });
        }
        Ok(())
    }
}

/// QI-BB-029 보완 #4: a sealed-but-corrupt half pair is repaired by the
/// next `ReplaceGeneration` seal batch without any manual directory
/// deletion.
///
/// The damaged track is reclaimed under its verified identity and rebuilt
/// from the batch, the healthy track is left alone, while a `Delta` seal
/// batch is refused typed with the repair to perform and touches nothing.
#[test]
fn a_sealed_but_corrupt_track_is_rebuilt_by_a_replace_seal_and_refused_for_a_delta() -> TestRes {
    let batch = fixture_search_corpus_batch()?;
    let lexical_reclaim =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[batch.generation.get()]);
    let lexical_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync> =
        Arc::new(CorruptUntilReclaimed {
            track: SearchPlaneTrackKind::Lexical,
            reclaim: Arc::clone(&lexical_reclaim),
        });
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let source_publication = super::support::test_source_catalog();
    let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
        SearchCorpusMaterializerParts {
            builder: lexical_builder.clone(),
            ledger: Arc::clone(&ledger),
            semantic_ingest: Arc::new(DirectSemanticMaterializer::new(semantic_builder.clone())),
            semantic_embedder: Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            authority: Arc::new(RecordingSearchCorpusAuthority {
                identities: Mutex::new(Vec::new()),
                exact: true,
            }),
            lexical_generation_validator: lexical_validator,
            semantic_generation_validator: always_valid_generation(),
            semantic_content_roots:
                crate::content_roots_test_support::generation_keyed_content_roots(),
            lexical_incomplete_discard: test_incomplete_generation_discard(),
            semantic_incomplete_discard: test_incomplete_generation_discard(),
            lexical_reclaim: lexical_reclaim.clone(),
            semantic_reclaim: no_storage_sealed_reclaim(),
            snapshots: SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
            source_publication: source_publication.clone(),
            idempotency: memory_catalog(),
            resource_policy: IngestResourcePolicy::DEFAULT,
            semantic_stream_policy: SemanticStreamWindowPolicy::DEFAULT,
            source_egress_policy: None,
            auxiliary_catalog: memory_aux_catalog(),
            auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
        },
    );

    // A delta cannot rebuild the track it patches: refused typed, nothing
    // reclaimed, nothing built.
    let mut delta = batch.clone();
    delta.mode = BatchIngestMode::Delta;
    delta.base_generation = Some(ManifestGeneration::new(6));
    restamp_search_corpus_fixture(&mut delta)?;
    delta.validate_v1()?;
    let mut base = batch.clone();
    base.generation = ManifestGeneration::new(6);
    base.manifest_digest = "manifest:base".into();
    restamp_search_corpus_fixture(&mut base)?;
    materializer.finalize_generation_v1(
        &base,
        Some(
            &SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
                &base.repo_id,
                &base.revision_id,
                [base.generation],
            ),
        ),
    )?;
    match materializer.publish_batch(&delta, &RequestBudgetV1::unbounded()) {
        Err(CoreError::Typed { code, message })
            if code == quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationRepairRequired
                && message.contains("publish a ReplaceGeneration seal batch") => {}
        other => return Err(format!("delta over a corrupt track answered {other:?}").into()),
    }
    if source_publication
        .inspect_source_event(&delta.repo_id, &delta.source_event)?
        .is_some()
    {
        return Err("a repair-mode refusal must not reserve the source stream".into());
    }
    if !lexical_reclaim.reclaimed().is_empty()
        || !lexical_builder
            .batches
            .lock()
            .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
            .is_empty()
        || !semantic_builder.take()?.is_empty()
    {
        return Err("a refused delta must reclaim and build nothing".into());
    }

    // The replace seal batch rebuilds exactly the damaged track.
    let receipt = materializer.publish_batch(&batch, &RequestBudgetV1::unbounded())?;
    if !receipt.sealed {
        return Err(format!("the repair must seal: {receipt:?}").into());
    }
    if lexical_reclaim.reclaimed() != [batch.generation.get()] {
        return Err(format!(
            "the damaged lexical track must be reclaimed under its identity: {:?}",
            lexical_reclaim.reclaimed()
        )
        .into());
    }
    if lexical_builder
        .batches
        .lock()
        .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
        .len()
        != 1
        || !semantic_builder.take()?.is_empty()
    {
        return Err("only the damaged track is rebuilt".into());
    }
    Ok(())
}

#[test]
fn failed_reclaim_releases_a_fence_only_after_the_old_namespace_is_absent() -> TestRes {
    let batch = fixture_search_corpus_batch()?;
    let (lexical, semantic) = generation_pair_from_batch_v1(&batch);
    let plan = SealedGenerationBuildPlanV1 {
        lexical,
        semantic,
        lexical_state: PhysicalGenerationStateV1::Corrupt {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
        },
        semantic_state: PhysicalGenerationStateV1::Exact,
    };
    let key = SnapshotKey::new(&batch.repo_id, &batch.revision_id, batch.generation);
    let semantic_reclaim = no_storage_sealed_reclaim();

    let moved =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[batch.generation.get()]);
    moved.fail_after_move_reclaim_of(batch.generation.get())?;
    let snapshots = SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT);
    match plan.repair_corrupt_v1(
        BatchIngestMode::ReplaceGeneration,
        &snapshots,
        moved.as_ref(),
        semantic_reclaim.as_ref(),
    ) {
        Err(CoreError::Storage(message)) if message.contains("after moving") => {}
        other => return Err(format!("post-move reclaim failure answered {other:?}").into()),
    }
    if moved.remaining().contains(&batch.generation.get()) {
        return Err("post-move failure left the old namespace present".into());
    }
    let replacement_proof = snapshots.lexical.begin_promotion(&key)?;
    drop(replacement_proof);

    let unmoved =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[batch.generation.get()]);
    unmoved.fail_next_reclaim_of(batch.generation.get())?;
    let snapshots = SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT);
    match plan.repair_corrupt_v1(
        BatchIngestMode::ReplaceGeneration,
        &snapshots,
        unmoved.as_ref(),
        semantic_reclaim.as_ref(),
    ) {
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationRepairRequired,
            ..
        }) => {}
        other => return Err(format!("pre-move reclaim failure answered {other:?}").into()),
    }
    if !unmoved.remaining().contains(&batch.generation.get())
        || !matches!(
            snapshots.lexical.begin_promotion(&key),
            Err(CoreError::Typed { code, .. })
                if code == quanta_index_contract::SearchPlaneErrorCodeV2::UnknownGeneration
        )
    {
        return Err("pre-move failure released the old namespace fence".into());
    }
    Ok(())
}

#[test]
fn repairing_a_generation_settles_a_failed_scrubs_old_fence() -> TestRes {
    let batch = fixture_search_corpus_batch()?;
    let (lexical, semantic) = generation_pair_from_batch_v1(&batch);
    let plan = SealedGenerationBuildPlanV1 {
        lexical,
        semantic,
        lexical_state: PhysicalGenerationStateV1::Corrupt {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        },
        semantic_state: PhysicalGenerationStateV1::Exact,
    };
    let key = SnapshotKey::new(&batch.repo_id, &batch.revision_id, batch.generation);
    let snapshots = SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT);
    let _failed_receipt = snapshots
        .lexical
        .retire(&key, SnapshotRetirementOwner::IntegrityScrub)?;
    let lexical_reclaim =
        ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[batch.generation.get()]);
    let semantic_reclaim = no_storage_sealed_reclaim();

    plan.repair_corrupt_v1(
        BatchIngestMode::ReplaceGeneration,
        &snapshots,
        lexical_reclaim.as_ref(),
        semantic_reclaim.as_ref(),
    )?;
    if lexical_reclaim
        .remaining()
        .contains(&batch.generation.get())
    {
        return Err("repair left the old generation namespace present".into());
    }
    let replacement = snapshots.lexical.begin_promotion(&key)?;
    drop(replacement);
    Ok(())
}

/// A delta over a base that is sealed but damaged on one track is refused
/// before any mutation with the repair to perform.
#[test]
fn a_delta_over_a_corrupt_base_is_refused_with_the_repair() -> TestRes {
    let probe = ZeroMutationProbe::new(Arc::new(CorruptTrackGeneration {
        corrupt: SearchPlaneTrackKind::Semantic,
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
    }));
    let mut delta = fixture_search_corpus_batch()?;
    delta.mode = BatchIngestMode::Delta;
    delta.base_generation = Some(ManifestGeneration::new(3));
    restamp_search_corpus_fixture(&mut delta)?;
    delta.validate_v1()?;
    probe
        .ledger
        .write()
        .map_err(|err| format!("ledger poisoned: {err}"))?
        .record_historically_sealed_search_corpus(
            &delta.repo_id,
            &delta.revision_id,
            ManifestGeneration::new(3),
            "manifest:base",
        );
    for (label, outcome) in [
        ("preflight", probe.materializer.preflight_batch(&delta)),
        (
            "publish",
            probe
                .materializer
                .publish_batch(&delta, &RequestBudgetV1::unbounded())
                .map(|_receipt| ()),
        ),
    ] {
        match outcome {
            Err(CoreError::Typed { code, message })
                if code == quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationRepairRequired
                    && message.contains("publish a ReplaceGeneration seal batch") => {}
            other => return Err(format!("{label} over a corrupt base answered {other:?}").into()),
        }
    }
    probe.assert_nothing_touched("corrupt delta base")
}

#[test]
fn search_corpus_materializer_derives_search_owned_semantic_batch() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(semantic_builder.clone()));
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
    batch.clear_surfaces = vec![SearchScopeSurface::Module];
    restamp_search_corpus_fixture(&mut batch)?;
    batch.validate_v1()?;
    let receipt = materializer.publish_batch(&batch, &RequestBudgetV1::unbounded())?;
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
    if derived.clear_surfaces != [SearchScopeSurface::Module] {
        return Err("derived semantic batch lost clear-surface truth".into());
    }
    if derived.replace_scopes.len() != 1 {
        return Err("derived semantic batch did not retain its typed source scope".into());
    }
    let scope = derived
        .replace_scopes
        .first()
        .ok_or_else(|| "derived semantic batch missing replace scope".to_string())?;
    if scope.embeddings.len() != 1 {
        return Err("derived semantic batch did not retain its typed source row".into());
    }
    let embedding = scope
        .embeddings
        .first()
        .ok_or_else(|| "derived semantic batch missing embedding".to_string())?;
    if embedding.embedding_id.as_str() != "symbol-1"
        || embedding.view_kind.as_ref() != "symbol.card"
    {
        return Err("derived semantic embedding must preserve typed source identity".into());
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

/// Both tracks of `batch`'s pair are sealed in `ledger`.
///
/// At the batch's generation and digest — what activation reads as each
/// track's current sealed identity — whether the batch built them or found
/// them sealed.
fn pair_sealed_in_ledger(
    ledger: &RwLock<Ledger>,
    batch: &quanta_index_contract::SearchCorpusIngestBatch,
) -> TestRes {
    let guard = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?;
    for track in [
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ] {
        let sealed = guard.track_sealed(&batch.repo_id, &batch.revision_id, track);
        let digest = guard.track_manifest_digest(&batch.repo_id, &batch.revision_id, track);
        if sealed != Some(batch.generation) || digest != Some(batch.manifest_digest.as_str()) {
            return Err(format!(
                "the ledger holds the {track:?} track sealed at {sealed:?} with {digest:?}"
            )
            .into());
        }
    }
    guard.validate_semantic_generation(
        &batch.repo_id,
        &batch.revision_id,
        batch.generation,
        Some(batch.manifest_digest.as_str()),
        true,
        "retried seal",
    )?;
    drop(guard);
    Ok(())
}

/// A seal that finds both tracks sealed exact only records.
///
/// The retry after a crash between the seals and the authority record
/// (QI-BB-029 완료 기준 #2) records the authority, and both tracks in a
/// ledger that, like a restarted daemon's, knew neither.
#[test]
fn sealed_exact_retry_repairs_authority_without_rebuilding_tracks() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(semantic_builder.clone()));
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
    let receipt = materializer.publish_batch(&batch, &RequestBudgetV1::unbounded())?;
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
    pair_sealed_in_ledger(&ledger, &batch)
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
        Arc::new(DirectSemanticMaterializer::new(Arc::new(
            FakeSemanticBuilder::default()
        ))),
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
        materializer.publish_batch(&batch, &RequestBudgetV1::unbounded()),
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
    // Retention can durably remove a delta's base before reporting an I/O
    // failure. Complete target chunks must already survive that boundary.
    let guard = ledger.read().map_err(|e| e.to_string())?;
    let state = guard
        .structural_state(&batch.repo_id, &batch.revision_id, batch.generation)
        .ok_or("retention ran before target chunks became durable")?;
    assert_eq!(
        state.source_batch_digest(),
        Some(batch.batch_digest.as_str())
    );
    assert_eq!(
        state
            .chunks()
            .keys()
            .map(quanta_index_contract::ChunkId::as_str)
            .collect::<Vec<_>>(),
        vec!["chunk-1"]
    );
    for track in [
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ] {
        assert!(
            guard
                .track_state(&batch.repo_id, &batch.revision_id, track)
                .is_none()
        );
    }
    assert!(
        serde_json::from_str::<crate::readiness::StructuralStateMeta>(
            r#"{"seal_requested":false}"#,
        )
        .is_err()
    );
    drop(guard);
    Ok(())
}

#[test]
fn non_seal_batch_cannot_mutate_an_already_sealed_generation() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(semantic_builder.clone()));
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
    let result = materializer.publish_batch(&batch, &RequestBudgetV1::unbounded());
    let Err(CoreError::Typed { code, .. }) = result else {
        return Err("non-seal mutation of sealed generation unexpectedly succeeded".into());
    };
    assert_eq!(
        code,
        quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusBatchShapeInvalid
    );
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

/// A lexical track sealed exact and a semantic one missing: the retry
/// builds only the semantic track, and the ledger — fresh, like a
/// restarted daemon's — holds both.
#[test]
fn exact_lexical_missing_semantic_retry_builds_only_missing_track() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(semantic_builder.clone()));
    let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let materializer = search_corpus_materializer!(
        lexical_builder.clone(),
        Arc::clone(&ledger),
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
    let batch = fixture_search_corpus_batch()?;
    let receipt = materializer.publish_batch(&batch, &RequestBudgetV1::unbounded())?;
    assert!(receipt.sealed);
    assert!(
        lexical_builder
            .batches
            .lock()
            .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
            .is_empty()
    );
    assert_eq!(semantic_builder.take()?.len(), 1);
    pair_sealed_in_ledger(&ledger, &batch)
}

/// An incomplete lexical track is rebuilt; a semantic one sealed exact is kept.
///
/// The retry after a crash inside the lexical seal discards and rebuilds
/// the lexical track, only records the semantic one, and the ledger —
/// fresh, like a restarted daemon's — holds both.
#[test]
fn incomplete_lexical_exact_semantic_retry_discards_and_rebuilds_only_lexical() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(semantic_builder.clone()));
    let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let lexical_discard = Arc::new(RecordingIncompleteGenerationDiscard::default());
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let materializer = search_corpus_materializer!(
        lexical_builder.clone(),
        Arc::clone(&ledger),
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

    let batch = fixture_search_corpus_batch()?;
    let receipt = materializer.publish_batch(&batch, &RequestBudgetV1::unbounded())?;
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
    pair_sealed_in_ledger(&ledger, &batch)
}

#[test]
fn jointly_incomplete_tracks_keep_staged_data_for_normal_seal() -> TestRes {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(semantic_builder.clone()));
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

    let receipt = materializer.publish_batch(
        &fixture_search_corpus_batch()?,
        &RequestBudgetV1::unbounded(),
    )?;

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
    let result = materializer.publish_batch(&batch, &RequestBudgetV1::unbounded());
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
