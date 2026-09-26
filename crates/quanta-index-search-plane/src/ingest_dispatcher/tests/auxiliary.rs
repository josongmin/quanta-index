use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    AuxEpochV1, ChunkId, DirtyIngestBatch, DirtyMutation, ManifestGeneration, RepoId, RevisionId,
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, HistoryTextBuildV1, HistoryTextIndexPort,
    IngestResourcePolicy, MetricSourcePort, MetricValueV1, SemanticIngestPort,
    SemanticStreamWindowPolicy,
};

use crate::ingest_dispatcher::auxiliary::{
    AuxiliaryMaterializerParts, DirectHistoryMaterializer, DirectRuntimeMetadataMaterializer,
    dirty_publish_receipt_v1,
};
use crate::ingest_dispatcher::ports::{HistoryIngestPort, RuntimeMetadataIngestPort};
use crate::ingest_dispatcher::search_corpus::{
    DirectSearchCorpusMaterializer, SearchCorpusMaterializerParts,
};
use crate::ingest_dispatcher::semantic::DirectSemanticMaterializer;
use crate::ingest_dispatcher::tests::support::{
    FakeSearchCorpusBuilder, FakeSemanticBuilder, TestRes, always_valid_generation, aux_parts,
    fixture_commit, fixture_dirty_batch, fixture_history_batch, fixture_search_corpus_batch,
    memory_catalog, no_storage_sealed_reclaim, recording_search_corpus_authority,
    test_incomplete_generation_discard,
};
use crate::query_dispatcher::tests::support::history_text::MemoryHistoryTextIndex;
use crate::readiness::SearchCorpusHistoryRetentionReceiptV1;
use crate::{HistoryTextIndexParts, Ledger, SEARCH_OWNED_SEMANTIC_DIMENSION, SnapshotRegistries};

#[test]
fn dirty_publish_receipt_binds_exact_auxiliary_batch_without_sealing_v1() {
    let batch = DirtyIngestBatch {
        repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(9),
        overlay_epoch_ms: 7,
        batch_digest: "dirty-batch:exact".to_string(),
        entries: Vec::new(),
    };
    let receipt = dirty_publish_receipt_v1(&batch);
    assert_eq!(receipt.generation, batch.generation);
    assert_eq!(receipt.manifest_digest, None);
    assert_eq!(receipt.batch_digest, batch.batch_digest);
    assert_eq!(receipt.accepted_replace_scopes, 0);
    assert_eq!(receipt.accepted_tombstone_scopes, 0);
    assert!(!receipt.sealed);
}

/// QI-BB-020: a batch whose rows never became durable is never
/// visible, and its receipt never issued; the next attempt applies.
#[test]
fn a_mutation_whose_rows_never_became_durable_is_never_visible() -> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let (parts, catalog) = aux_parts(Arc::clone(&ledger));
    let materializer = DirectHistoryMaterializer::new(parts);
    let batch = fixture_history_batch(9, vec![fixture_commit(1, &[])]);

    catalog.fail_next_apply();
    let refused = materializer
        .publish_batch(&batch)
        .expect_err("a catalog that cannot commit refuses the publish");
    if !matches!(refused, CoreError::Storage(_)) {
        return Err(format!("expected the catalog's failure, got {refused:?}").into());
    }
    {
        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        if guard
            .history_state(&batch.repo_id, &batch.revision_id, batch.generation)
            .is_some()
        {
            return Err("rows that never became durable must not be visible".into());
        }
    }
    if catalog.applies() != 0 {
        return Err("nothing was applied".into());
    }

    let receipt = materializer.publish_batch(&batch)?;
    if receipt.accepted_replace_scopes != 1 || catalog.applies() != 1 {
        return Err(format!("the retry must apply once: {receipt:?}").into());
    }
    let guard = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?;
    let commits = guard
        .history_state(&batch.repo_id, &batch.revision_id, batch.generation)
        .map(|state| state.commits().len());
    drop(guard);
    if commits != Some(1) {
        return Err(format!("the durable batch must be visible, saw {commits:?}").into());
    }
    Ok(())
}

/// QI-BB-020: a batch that fails validation writes nothing and touches
/// nothing — the catalog is never asked.
#[test]
fn a_batch_that_fails_validation_never_reaches_the_catalog() -> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let (parts, catalog) = aux_parts(Arc::clone(&ledger));
    let materializer = DirectHistoryMaterializer::new(parts);
    // A child whose parent is neither in the state nor earlier in the batch.
    let orphan = fixture_history_batch(9, vec![fixture_commit(2, &[1])]);
    match materializer.publish_batch(&orphan) {
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::HistoryCommitParentUnknown,
                ),
            ..
        }) => {}
        other => return Err(format!("orphan commit answered {other:?}").into()),
    }
    if catalog.applies() != 0 || catalog.row_count() != 0 {
        return Err("a refused batch must not reach the catalog".into());
    }
    // The parent earlier in the same batch is enough.
    let ordered = fixture_history_batch(9, vec![fixture_commit(1, &[]), fixture_commit(2, &[1])]);
    let _receipt = materializer.publish_batch(&ordered)?;
    if catalog.applies() != 1 {
        return Err("an ordered batch applies once".into());
    }
    Ok(())
}

/// QI-BB-020: a one-row mutation over a generation holding a thousand
/// rows writes one row.
#[test]
fn a_one_row_dirty_mutation_writes_one_row() -> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let (parts, catalog) = aux_parts(Arc::clone(&ledger));
    let materializer = DirectRuntimeMetadataMaterializer::new(parts);
    let mut seed = fixture_dirty_batch();
    seed.entries = (0..1_000_u32)
        .map(|index| {
            DirtyMutation::Upsert(quanta_index_contract::lex::DirtyRecord {
                wire_version: 1,
                doc_id: ChunkId::new(format!("doc-{index}")),
                applied_at_ms: 1,
                payload_hash: [0; 32],
            })
        })
        .collect();
    let _seeded = materializer.publish_batch(&seed)?;
    let before = catalog.rows_written();
    // 1,000 doc rows, the generation's one meta row and its one epoch row
    // (QI-BB-020 W2).
    if before != 1_002 {
        return Err(format!("seeding wrote {before} rows, expected 1002").into());
    }
    let mut one = fixture_dirty_batch();
    one.batch_digest = "batch:dirty:one".to_string();
    one.entries = vec![DirtyMutation::Upsert(
        quanta_index_contract::lex::DirtyRecord {
            wire_version: 1,
            doc_id: ChunkId::new("doc-500"),
            applied_at_ms: 2,
            payload_hash: [1; 32],
        },
    )];
    let _receipt = materializer.publish_batch(&one)?;
    // The one doc row plus the generation's meta and epoch rows: three
    // rows, not a thousand.
    let written = catalog.rows_written().saturating_sub(before);
    if written != 3 {
        return Err(format!("a one-row mutation wrote {written} rows").into());
    }
    let guard = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?;
    let applied = guard
        .runtime_state(&one.repo_id, &one.revision_id, one.generation)
        .and_then(|state| state.dirty_docs().get(&ChunkId::new("doc-500")))
        .map(crate::readiness::DirtyDocState::applied_at_ms);
    let resident = guard
        .runtime_state(&one.repo_id, &one.revision_id, one.generation)
        .map(|state| state.dirty_docs().len());
    drop(guard);
    if applied != Some(2) || resident != Some(1_000) {
        return Err(
            format!("the row changed in place: applied={applied:?} resident={resident:?}").into(),
        );
    }
    Ok(())
}

/// QI-BB-020: a reader holding a snapshot neither blocks a mutation nor
/// sees it; the ledger holds the new state while the snapshot holds
/// the old one.
#[test]
fn a_reader_holding_a_snapshot_neither_blocks_nor_sees_a_mutation() -> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let (parts, _catalog) = aux_parts(Arc::clone(&ledger));
    let materializer = Arc::new(DirectHistoryMaterializer::new(parts));
    let first = fixture_history_batch(9, vec![fixture_commit(1, &[])]);
    let _receipt = materializer.publish_batch(&first)?;
    let snapshot = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?
        .history_read_at(
            &first.repo_id,
            &first.revision_id,
            first.generation,
            None,
            std::time::Instant::now(),
        )?
        .ok_or("the first batch is visible")?
        .state;
    // The reader has released the lock but still holds the snapshot;
    // a mutation on another thread must complete.
    let second = fixture_history_batch(9, vec![fixture_commit(2, &[1])]);
    let writer = {
        let materializer = Arc::clone(&materializer);
        std::thread::spawn(move || materializer.publish_batch(&second))
    };
    let _receipt = writer
        .join()
        .map_err(|panic| format!("writer panicked: {panic:?}"))??;
    if snapshot.commits().len() != 1 {
        return Err("the held snapshot must not change under the reader".into());
    }
    let after = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?
        .history_state(&first.repo_id, &first.revision_id, first.generation)
        .map(|state| state.commits().len());
    if after != Some(2) {
        return Err(format!("the ledger must hold the mutation, saw {after:?}").into());
    }
    Ok(())
}

/// A search-corpus materializer over fakes that shares `ledger` and the
/// auxiliary catalog and coordinator of `aux`, for the seal's auxiliary
/// bookkeeping.
fn sealing_materializer(
    ledger: &Arc<RwLock<Ledger>>,
    aux: &AuxiliaryMaterializerParts,
) -> DirectSearchCorpusMaterializer {
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
        DirectSemanticMaterializer::new(Arc::new(FakeSemanticBuilder::default())),
    );
    DirectSearchCorpusMaterializer::new_with_search_owned_semantics(SearchCorpusMaterializerParts {
        builder: Arc::new(FakeSearchCorpusBuilder::default()),
        ledger: Arc::clone(ledger),
        semantic_ingest: semantic_materializer,
        semantic_embedder: Arc::new(crate::HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        authority: recording_search_corpus_authority(),
        lexical_generation_validator: always_valid_generation(),
        semantic_generation_validator: always_valid_generation(),
        semantic_content_roots: crate::content_roots_test_support::generation_keyed_content_roots(),
        lexical_incomplete_discard: test_incomplete_generation_discard(),
        semantic_incomplete_discard: test_incomplete_generation_discard(),
        lexical_reclaim: no_storage_sealed_reclaim(),
        semantic_reclaim: no_storage_sealed_reclaim(),
        snapshots: SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
        source_publication: super::support::test_source_catalog(),
        idempotency: memory_catalog(),
        resource_policy: IngestResourcePolicy::DEFAULT,
        semantic_stream_policy: SemanticStreamWindowPolicy::DEFAULT,
        source_egress_policy: None,
        auxiliary_catalog: aux.catalog.clone(),
        auxiliary_coordinator: Arc::clone(&aux.coordinator),
    })
}

/// QI-BB-020: retention forgets unretained auxiliary generations.
///
/// Sealing a generation under a retention receipt forgets the
/// auxiliary generations older than it that the receipt does not
/// retain — in memory and in the catalog — and leaves the retained and
/// the just-sealed ones.
#[test]
fn retention_forgets_auxiliary_generations_the_receipt_does_not_retain() -> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let (aux, catalog) = aux_parts(Arc::clone(&ledger));
    let history = DirectHistoryMaterializer::new(aux.clone());
    for generation in [3, 4] {
        let _receipt = history.publish_batch(&fixture_history_batch(
            generation,
            vec![fixture_commit(1, &[])],
        ))?;
    }
    let materializer = sealing_materializer(&ledger, &aux);
    let mut batch = fixture_search_corpus_batch()?;
    batch.generation = ManifestGeneration::new(5);
    let receipt = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [ManifestGeneration::new(4), ManifestGeneration::new(5)],
    );
    materializer.finalize_generation_v1(&batch, Some(&receipt))?;

    let guard = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?;
    let forgotten = guard
        .history_state(
            &batch.repo_id,
            &batch.revision_id,
            ManifestGeneration::new(3),
        )
        .is_none();
    let retained = guard
        .history_state(
            &batch.repo_id,
            &batch.revision_id,
            ManifestGeneration::new(4),
        )
        .is_some();
    let sealed_chunks = guard
        .structural_state(
            &batch.repo_id,
            &batch.revision_id,
            ManifestGeneration::new(5),
        )
        .map(|state| state.chunks().len());
    drop(guard);
    if !forgotten || !retained || sealed_chunks != Some(1) {
        return Err(format!(
            "ledger drifted: forgotten={forgotten} retained={retained} sealed_chunks={sealed_chunks:?}"
        )
        .into());
    }
    let generations = catalog.generations();
    if generations != BTreeSet::from([4, 5]) {
        return Err(format!(
            "catalog must hold exactly the retained generations, holds {generations:?}"
        )
        .into());
    }
    Ok(())
}

/// What the reap test observes: the catalog's generations, whether the
/// ledger still holds generation 3's history, and the ledger's chunk count
/// for generation 5.
type ReapObservation = (BTreeSet<u64>, bool, Option<usize>);

/// The reap of retired auxiliary generations and the sealed generation's
/// chunk universe are one catalog transaction (QI-BB-020 보완 #4).
///
/// Generations 3 and 4 carry history; sealing 5 while retaining 4 and 5
/// with the catalog's transaction failing is refused, and leaves the
/// catalog and the ledger exactly as they were: 3 still in both, 5's
/// chunks in neither. The retried seal reaps 3 and lands 5 in both, with
/// one catalog transaction.
#[test]
fn a_seal_whose_reap_transaction_fails_changes_neither_catalog_nor_ledger() -> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let (aux, catalog) = aux_parts(Arc::clone(&ledger));
    let history = DirectHistoryMaterializer::new(aux.clone());
    for generation in [3, 4] {
        let _receipt = history.publish_batch(&fixture_history_batch(
            generation,
            vec![fixture_commit(1, &[])],
        ))?;
    }
    let materializer = sealing_materializer(&ledger, &aux);
    let mut batch = fixture_search_corpus_batch()?;
    batch.generation = ManifestGeneration::new(5);
    let receipt = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [ManifestGeneration::new(4), ManifestGeneration::new(5)],
    );
    let observed = || -> Result<ReapObservation, Box<dyn std::error::Error>> {
        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        let holds_3 = guard
            .history_state(
                &batch.repo_id,
                &batch.revision_id,
                ManifestGeneration::new(3),
            )
            .is_some();
        let chunks_5 = guard
            .structural_state(
                &batch.repo_id,
                &batch.revision_id,
                ManifestGeneration::new(5),
            )
            .map(|state| state.chunks().len());
        drop(guard);
        Ok((catalog.generations(), holds_3, chunks_5))
    };

    catalog.fail_next_apply();
    let refused = materializer.finalize_generation_v1(&batch, Some(&receipt));
    if !matches!(refused, Err(CoreError::Storage(_))) {
        return Err(format!("the failed transaction refuses the seal, got {refused:?}").into());
    }
    let after_refusal = observed()?;
    if after_refusal != (BTreeSet::from([3, 4]), true, None) {
        return Err(format!("a refused seal changed state: {after_refusal:?}").into());
    }

    let applies = catalog.applies();
    materializer.finalize_generation_v1(&batch, Some(&receipt))?;
    let after_retry = observed()?;
    let transactions = catalog.applies().saturating_sub(applies);
    if after_retry != (BTreeSet::from([4, 5]), false, Some(1)) || transactions != 1 {
        return Err(format!(
            "the retried seal reaps 3 and lands 5 in one transaction: {after_retry:?} in {transactions}"
        )
        .into());
    }
    Ok(())
}

/// `history_text_gc_failures_total` as the history text parts scrape it.
fn gc_failures(history_text: &HistoryTextIndexParts) -> Result<u64, Box<dyn std::error::Error>> {
    match history_text
        .scrape()?
        .into_iter()
        .find(|point| point.name == "history_text_gc_failures_total")
        .map(|point| point.value)
    {
        Some(MetricValueV1::Counter(count)) => Ok(count),
        other => Err(format!("the parts scrape the gc failure counter: {other:?}").into()),
    }
}

/// Forgotten generations' text indexes are swept from the disk, and a
/// discard that fails after the seal is durable fails nothing and is
/// retried (QI-BB-020).
///
/// Generations 3 and 4 carry history, and so text indexes; generation 2
/// has an index on disk the ledger never knew, as a discard that failed on
/// an earlier pass leaves one. Sealing 5 while retaining 4 and 5, with the
/// index's next discard failing: the seal succeeds and generation 3's rows
/// are forgotten; the sweep's first discard (generation 2's, the lowest)
/// fails and is counted and its index stays, while generation 3's is
/// swept and the retained 4's is kept. Sealing 6 while retaining 5 and 6
/// sweeps the leftover 2 and the newly forgotten 4, and counts nothing
/// more.
#[test]
fn forgotten_generations_text_indexes_are_swept_and_a_failed_discard_is_retried() -> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let (aux, catalog) = aux_parts(Arc::clone(&ledger));
    let index = MemoryHistoryTextIndex::shared();
    let history_text = HistoryTextIndexParts::new(index.clone());
    let history =
        DirectHistoryMaterializer::new(aux.clone()).with_history_text(history_text.clone());
    for generation in [3, 4] {
        let _receipt = history.publish_batch(&fixture_history_batch(
            generation,
            vec![fixture_commit(1, &[])],
        ))?;
    }
    let mut batch = fixture_search_corpus_batch()?;
    let key = |generation: u64| AuxiliaryGenerationKeyV1 {
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: ManifestGeneration::new(generation),
    };
    let _left_behind = index.publish_epoch(
        &key(2),
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full { docs: Vec::new() },
    )?;
    let materializer = sealing_materializer(&ledger, &aux).with_history_text(history_text.clone());
    let durable = |generation: u64| -> Result<bool, Box<dyn std::error::Error>> {
        Ok(!index.durable_epochs(&key(generation))?.is_empty())
    };

    batch.generation = ManifestGeneration::new(5);
    index.fail_next_discard();
    let receipt = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [ManifestGeneration::new(4), ManifestGeneration::new(5)],
    );
    materializer.finalize_generation_v1(&batch, Some(&receipt))?;
    if catalog.generations() != BTreeSet::from([4, 5]) {
        return Err(format!("generation 3 is forgotten: {:?}", catalog.generations()).into());
    }
    if gc_failures(&history_text)? != 1 || !durable(2)? || durable(3)? || !durable(4)? {
        return Err(format!(
            "one failed discard is counted and only its index stays: failures={} 2={} 3={} 4={}",
            gc_failures(&history_text)?,
            durable(2)?,
            durable(3)?,
            durable(4)?
        )
        .into());
    }

    batch.generation = ManifestGeneration::new(6);
    let receipt = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [ManifestGeneration::new(5), ManifestGeneration::new(6)],
    );
    materializer.finalize_generation_v1(&batch, Some(&receipt))?;
    if gc_failures(&history_text)? != 1 || durable(2)? || durable(3)? || durable(4)? {
        return Err(format!(
            "the sweep removes every index the ledger no longer knows: failures={} 2={} 3={} 4={}",
            gc_failures(&history_text)?,
            durable(2)?,
            durable(3)?,
            durable(4)?
        )
        .into());
    }
    Ok(())
}

/// A refusal the history text sweep meets fails the seal closed rather
/// than being counted as a deferred discard (QI-BB-020, §3.49).
///
/// Generations 3 and 4 carry text indexes; sealing 5 while retaining 4 and
/// 5 with the index refusing its listing (as an entry that is not a
/// generation is refused): the seal answers the refusal's code, counts no
/// GC failure, and generation 3's index stays where it is.
#[test]
fn a_refused_history_text_listing_fails_the_seal_closed() -> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let (aux, _catalog) = aux_parts(Arc::clone(&ledger));
    let index = MemoryHistoryTextIndex::shared();
    let history_text = HistoryTextIndexParts::new(index.clone());
    let history =
        DirectHistoryMaterializer::new(aux.clone()).with_history_text(history_text.clone());
    for generation in [3, 4] {
        let _receipt = history.publish_batch(&fixture_history_batch(
            generation,
            vec![fixture_commit(1, &[])],
        ))?;
    }
    let materializer = sealing_materializer(&ledger, &aux).with_history_text(history_text.clone());
    let mut batch = fixture_search_corpus_batch()?;
    batch.generation = ManifestGeneration::new(5);
    let receipt = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
        &batch.repo_id,
        &batch.revision_id,
        [ManifestGeneration::new(4), ManifestGeneration::new(5)],
    );

    index.refuse_next_listing();
    let refused = materializer.finalize_generation_v1(&batch, Some(&receipt));
    if !matches!(&refused, Err(CoreError::Typed { code, .. })
        if *code == quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexForeignEntry)
    {
        return Err(format!("the refusal fails the seal closed, got {refused:?}").into());
    }
    let generation_3 = AuxiliaryGenerationKeyV1 {
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id,
        generation: ManifestGeneration::new(3),
    };
    if gc_failures(&history_text)? != 0 || index.durable_epochs(&generation_3)?.is_empty() {
        return Err("a refused sweep counts nothing and discards nothing".into());
    }
    Ok(())
}

/// An auxiliary receipt names its batch digest and no manifest
/// (QI-BB-032): the two identities are distinct fields.
#[test]
fn direct_dirty_materializer_names_the_batch_digest_and_no_manifest() -> TestRes {
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let (parts, _catalog) = aux_parts(Arc::clone(&ledger));
    let materializer = DirectRuntimeMetadataMaterializer::new(parts);
    let batch = fixture_dirty_batch();
    let receipt = materializer.publish_batch(&batch)?;
    if receipt.manifest_digest.is_some()
        || receipt.batch_digest != batch.batch_digest
        || receipt.generation != batch.generation
        || receipt.accepted_replace_scopes != 1
        || receipt.accepted_tombstone_scopes != 0
        || receipt.sealed
    {
        return Err(format!(
            "unexpected dirty materialize receipt: batch={batch:?} receipt={receipt:?}"
        )
        .into());
    }
    Ok(())
}
