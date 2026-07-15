//! Semantic boot path: legacy journal migration (LDB-04) and durable readiness
//! seeding (LDB-03).
//!
//! Replaces the retired boot-time `journal.cbor` replay. On boot the runtime:
//!
//! 1. runs a one-shot, idempotent migration off any legacy semantic journal
//!    into the durable generation directories, then
//! 2. seeds the readiness ledger directly from those sealed generations.
//!
//! There is no steady-state journal authority and no full replay; seeding cost
//! is bounded by the count of sealed generations, not total batch history.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    ManifestGeneration, OwnerDocKind, RepoId, RevisionId, SearchPlaneTrackKind,
    SemanticCorpusKindV1, SemanticIngestBatch,
};
use quanta_index_core::{CoreError, SemanticBatchBuildPort};
use quanta_index_search_plane::{Ledger, LegacySemanticJournalStore};
use quanta_index_semantic::scan_persisted_generations;

/// Outcome of the one-shot legacy semantic journal migration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticMigrationOutcome {
    /// No legacy `journal.cbor` present; nothing to migrate.
    NoLegacyJournal,
    /// Completion marker already present; migration skipped.
    AlreadyMigrated,
    /// Migration ran and applied `imported` batches into durable generations.
    Migrated { imported: usize },
}

/// Report of durable semantic readiness seeding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticSeedReport {
    pub sealed_generations: usize,
}

/// Bounded, payload-free semantic boot observability (LDB-E2E-01 §4).
///
/// Distinguishes a direct durable open (`migration == NoLegacyJournal /
/// AlreadyMigrated`, `seed.sealed_generations` opened) from a one-shot
/// migration run, and carries cold-boot cost for BOTH migration and seeding
/// so a large legacy journal import is visible to operators. Carries only
/// enums, counts, and durations — never vectors, snippets, or path text.
///
/// Intentionally does NOT derive `Eq` / `PartialEq` because the `_micros`
/// fields are wall-clock timings; structural equality across two reports is
/// meaningless. Tests should assert individual fields.
#[derive(Clone, Copy, Debug)]
pub struct SemanticBootReport {
    pub migration: SemanticMigrationOutcome,
    /// Wall-clock cost of `migrate_legacy_semantic_journal` (0 when no journal
    /// existed or migration was already complete — that information is in
    /// `migration` itself, not duration).
    pub migration_micros: u128,
    pub seed: SemanticSeedReport,
    /// Wall-clock cost of `seed_persisted_semantic_readiness`.
    pub seed_micros: u128,
}

type GenerationKey = (RepoId, RevisionId, ManifestGeneration);

fn normalize_legacy_semantic_batch_v1(batch: &SemanticIngestBatch) -> SemanticIngestBatch {
    let mut normalized = batch.clone();
    for scope in &mut normalized.replace_scopes {
        let legacy_owner_id =
            format!("legacy-path:{}", scope.scope.repo_relative_path.as_str()).into_boxed_str();
        for embedding in &mut scope.embeddings {
            if embedding.owner_kind == OwnerDocKind::Chunk
                && embedding.corpus_kind == SemanticCorpusKindV1::RawCodeFallback
            {
                embedding.owner_id = legacy_owner_id.clone();
                embedding.parent_owner_id = Some(legacy_owner_id.clone());
            }
        }
    }
    normalized
}

/// Migrate the legacy semantic journal into durable generations exactly once.
///
/// Idempotent and resumable: generations already sealed on disk (from a prior
/// run or a partial crash) are skipped, so a re-run never tries to mutate a
/// sealed generation. The legacy journal is retained after success; only a
/// completion marker is written.
pub fn migrate_legacy_semantic_journal(
    store: &LegacySemanticJournalStore,
    builder: &(dyn SemanticBatchBuildPort + Send + Sync),
    semantic_root: &Path,
) -> Result<SemanticMigrationOutcome, CoreError> {
    if store.migration_complete() {
        return Ok(SemanticMigrationOutcome::AlreadyMigrated);
    }
    if !store.has_legacy_journal() {
        return Ok(SemanticMigrationOutcome::NoLegacyJournal);
    }

    let mut sealed: BTreeSet<GenerationKey> = BTreeSet::new();
    for record in scan_persisted_generations(semantic_root)? {
        let _present = sealed.insert((record.repo_id, record.revision_id, record.generation));
    }

    let mut imported: usize = 0;
    for legacy_batch in store.legacy_batches() {
        let batch = normalize_legacy_semantic_batch_v1(legacy_batch);
        let key: GenerationKey = (
            batch.repo_id.clone(),
            batch.revision_id.clone(),
            batch.generation,
        );
        if sealed.contains(&key) {
            continue;
        }
        builder.build_batch(&batch)?;
        if batch.seal {
            let _present = sealed.insert(key);
        }
        imported = imported.checked_add(1).ok_or_else(|| {
            CoreError::Storage("semantic migration: imported counter overflow".to_string())
        })?;
    }

    store.mark_migration_complete()?;
    Ok(SemanticMigrationOutcome::Migrated { imported })
}

/// Seed semantic readiness directly from sealed durable generations.
///
/// Mirrors the lexical readiness seed, but uses each generation's manifest
/// digest so the per-`(repo, revision, generation)` semantic readiness state is
/// reconstructed from durable truth rather than a replayed RAM graph.
pub fn seed_persisted_semantic_readiness(
    ledger: &Arc<RwLock<Ledger>>,
    semantic_root: &Path,
) -> Result<SemanticSeedReport, CoreError> {
    let generations = scan_persisted_generations(semantic_root)?;
    let mut guard = ledger
        .write()
        .map_err(|err| CoreError::Storage(format!("semantic seed: ledger poisoned: {err}")))?;

    let mut highest: Option<(ManifestGeneration, String)> = None;
    let mut sealed_generations: usize = 0;
    for record in &generations {
        guard.record_track_materialized(
            &record.repo_id,
            &record.revision_id,
            SearchPlaneTrackKind::Semantic,
            record.generation,
            Some(record.manifest_digest.as_str()),
        );
        guard.record_track_seal_with_digest(
            &record.repo_id,
            &record.revision_id,
            SearchPlaneTrackKind::Semantic,
            record.generation,
            record.manifest_digest.as_str(),
        );
        let take_higher = match &highest {
            Some((current, _)) => record.generation.get() > current.get(),
            None => true,
        };
        if take_higher {
            highest = Some((record.generation, record.manifest_digest.clone()));
        }
        sealed_generations = sealed_generations.checked_add(1).ok_or_else(|| {
            CoreError::Storage("semantic seed: generation counter overflow".to_string())
        })?;
    }
    if let Some((generation, digest)) = highest {
        guard.semantic_seal_with_digest(generation, digest.as_str());
    }
    drop(guard);
    Ok(SemanticSeedReport { sealed_generations })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use quanta_index_contract::{
        BatchIngestMode, EmbeddingDistanceMetric, EmbeddingModelContract, EmbeddingNormalization,
        EmbeddingRecord, OwnerDocKind, RepoRelativePath, SearchScopeKey, SearchScopeSurface,
        SemanticCorpusKindV1, SemanticIngestBatch, SemanticReplaceScope,
    };
    use quanta_index_core::{GenerationStorageKeyV1, SemanticIndexOpenPort};
    use quanta_index_semantic::{
        SemanticAdapter, embedding_record_v1, ingest_batch_v1, legacy_chunk_embedding_record_v1,
    };

    use super::{
        Arc, BTreeSet, GenerationKey, Ledger, LegacySemanticJournalStore, ManifestGeneration,
        RepoId, RevisionId, RwLock, SearchPlaneTrackKind, SemanticMigrationOutcome,
        migrate_legacy_semantic_journal, normalize_legacy_semantic_batch_v1,
        scan_persisted_generations, seed_persisted_semantic_readiness,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn repo_id() -> RepoId {
        RepoId::new("repo-boot")
    }

    fn revision_id() -> RevisionId {
        RevisionId::new("rev-boot")
    }

    fn model_contract() -> EmbeddingModelContract {
        EmbeddingModelContract {
            model_id: "text-embed".to_string().into_boxed_str(),
            model_version: Some("1".to_string().into_boxed_str()),
            dimension: 3,
            normalization: EmbeddingNormalization::L2Unit,
            distance_metric: EmbeddingDistanceMetric::Cosine,
            policy_digest: "policy:boot".to_string().into_boxed_str(),
            view_policy_digest: None,
        }
    }

    fn embedding(id: &str, path: &str, vector: Vec<f32>) -> Result<EmbeddingRecord, String> {
        legacy_chunk_embedding_record_v1(id, path, vector)
    }

    fn batch(
        generation: ManifestGeneration,
        id: &str,
        path: &str,
        vector: Vec<f32>,
        seal: bool,
    ) -> Result<SemanticIngestBatch, String> {
        Ok(ingest_batch_v1(
            repo_id(),
            revision_id(),
            generation,
            None,
            format!("manifest:{}", generation.get()),
            format!("batch:{}", generation.get()),
            BatchIngestMode::ReplaceGeneration,
            model_contract(),
            vec![SemanticReplaceScope {
                scope: SearchScopeKey {
                    doc_surface: SearchScopeSurface::Chunk,
                    repo_relative_path: RepoRelativePath::new(path),
                },
                scope_digest: format!("scope:{path}"),
                embeddings: vec![embedding(id, path, vector)?],
            }],
            Vec::new(),
            seal,
        ))
    }

    fn build_durable(adapter: &SemanticAdapter, batch: &SemanticIngestBatch) -> TestResult {
        use quanta_index_core::SemanticBatchBuildPort as _;
        adapter.build_batch(batch)?;
        Ok(())
    }

    fn build_legacy_durable(adapter: &SemanticAdapter, batch: &SemanticIngestBatch) -> TestResult {
        build_durable(adapter, &normalize_legacy_semantic_batch_v1(batch))
    }

    #[test]
    fn legacy_normalization_uses_path_stable_owner_and_preserves_scv2_owner() -> TestResult {
        let mut legacy_batch = batch(
            ManifestGeneration::new(1),
            "legacy-a",
            "src/lib.rs",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        legacy_batch.replace_scopes[0].embeddings.push(embedding(
            "legacy-b",
            "src/lib.rs",
            vec![0.0, 1.0, 0.0],
        )?);
        legacy_batch.replace_scopes[0]
            .embeddings
            .push(embedding_record_v1(
                "symbol-card",
                "src/lib.rs",
                OwnerDocKind::Symbol,
                "symbol:authoritative",
                SemanticCorpusKindV1::SymbolCard,
                vec![0.0, 0.0, 1.0],
            )?);

        let normalized = normalize_legacy_semantic_batch_v1(&legacy_batch);
        let embeddings = &normalized.replace_scopes[0].embeddings;
        assert_eq!(
            embeddings[0].embedding_id,
            legacy_batch.replace_scopes[0].embeddings[0].embedding_id
        );
        assert_eq!(
            embeddings[0].record_id,
            legacy_batch.replace_scopes[0].embeddings[0].record_id
        );
        assert_eq!(embeddings[0].owner_id, embeddings[1].owner_id);
        assert_eq!(
            embeddings[0].parent_owner_id,
            Some(embeddings[0].owner_id.clone())
        );
        assert_eq!(embeddings[2], legacy_batch.replace_scopes[0].embeddings[2]);
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts seeded readiness via assert macros"
    )]
    fn seed_reconstructs_readiness_from_sealed_generations() -> TestResult {
        let temp = tempfile::tempdir()?;
        let semantic_root: PathBuf = temp.path().join("indexes").join("semantic");
        let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
        build_durable(
            &adapter,
            &batch(
                ManifestGeneration::new(3),
                "a",
                "a.rs",
                vec![1.0, 0.0, 0.0],
                true,
            )?,
        )?;
        build_durable(
            &adapter,
            &batch(
                ManifestGeneration::new(5),
                "b",
                "b.rs",
                vec![0.0, 1.0, 0.0],
                true,
            )?,
        )?;

        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let report = seed_persisted_semantic_readiness(&ledger, &semantic_root)?;
        assert_eq!(report.sealed_generations, 2);

        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        assert_eq!(
            guard.track_sealed(&repo_id(), &revision_id(), SearchPlaneTrackKind::Semantic),
            Some(ManifestGeneration::new(5))
        );
        assert_eq!(guard.semantic_sealed(), Some(ManifestGeneration::new(5)));
        assert_eq!(
            guard.track_manifest_digest(&repo_id(), &revision_id(), SearchPlaneTrackKind::Semantic),
            Some("manifest:5")
        );
        drop(guard);
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts unsealed generations are not seeded via assert macros"
    )]
    fn seed_skips_unsealed_generations() -> TestResult {
        let temp = tempfile::tempdir()?;
        let semantic_root: PathBuf = temp.path().join("indexes").join("semantic");
        let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
        build_durable(
            &adapter,
            &batch(
                ManifestGeneration::new(2),
                "a",
                "a.rs",
                vec![1.0, 0.0, 0.0],
                false,
            )?,
        )?;

        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let report = seed_persisted_semantic_readiness(&ledger, &semantic_root)?;
        assert_eq!(report.sealed_generations, 0);
        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        assert_eq!(
            guard.track_sealed(&repo_id(), &revision_id(), SearchPlaneTrackKind::Semantic),
            None
        );
        drop(guard);
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts corrupted sealed generations fail boot seeding via assert macros"
    )]
    fn seed_rejects_corrupted_sealed_generation() -> TestResult {
        let temp = tempfile::tempdir()?;
        let semantic_root: PathBuf = temp.path().join("indexes").join("semantic");
        let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
        let generation = ManifestGeneration::new(9);
        build_durable(
            &adapter,
            &batch(generation, "a", "a.rs", vec![1.0, 0.0, 0.0], true)?,
        )?;

        let manifest_path = GenerationStorageKeyV1::for_repo_revision(&repo_id(), &revision_id())
            .generation_dir(&semantic_root, generation)
            .join("semantic-manifest.cbor");
        let manifest_bytes = std::fs::read(&manifest_path)?;
        let mut value: ciborium::value::Value = ciborium::from_reader(&manifest_bytes[..])
            .map_err(|err| format!("decode manifest cbor: {err}"))?;
        let mut bumped = false;
        if let ciborium::value::Value::Map(entries) = &mut value {
            for (key, val) in entries.iter_mut() {
                if key.as_text() == Some("row_count") {
                    *val = ciborium::value::Value::Integer(ciborium::value::Integer::from(99_u64));
                    bumped = true;
                    break;
                }
            }
        }
        if !bumped {
            return Err("expected `row_count` field in manifest CBOR".into());
        }
        let mut tampered = Vec::new();
        ciborium::into_writer(&value, &mut tampered)
            .map_err(|err| format!("encode manifest cbor: {err}"))?;
        std::fs::write(&manifest_path, &tampered)?;

        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let Err(err) = seed_persisted_semantic_readiness(&ledger, &semantic_root) else {
            return Err("corrupted sealed generation must fail boot seeding".into());
        };
        assert!(
            matches!(err, quanta_index_core::CoreError::Storage(ref message) if message.contains("row count")),
            "expected row-count integrity failure, got: {err:?}"
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts no-op migration outcome via assert macros"
    )]
    fn migration_without_journal_is_noop() -> TestResult {
        let temp = tempfile::tempdir()?;
        let semantic_root: PathBuf = temp.path().join("indexes").join("semantic");
        let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
        let store = LegacySemanticJournalStore::open(temp.path().join("semantic"))?;
        let outcome = migrate_legacy_semantic_journal(&store, &adapter, &semantic_root)?;
        assert_eq!(outcome, SemanticMigrationOutcome::NoLegacyJournal);
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts migration import + idempotency + equivalence via assert macros"
    )]
    fn migration_imports_journal_idempotently_and_matches_clean_build() -> TestResult {
        let temp = tempfile::tempdir()?;
        let legacy_root = temp.path().join("semantic");
        let semantic_root: PathBuf = temp.path().join("indexes").join("semantic");

        // Stage a legacy journal: build gen 7 across two batches, sealing on the
        // second (mirrors the old append-then-seal producer cadence).
        let mut first = batch(
            ManifestGeneration::new(7),
            "emb-1",
            "x.rs",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        first.batch_digest = "batch:7:a".to_string();
        let second = batch(
            ManifestGeneration::new(7),
            "emb-2",
            "y.rs",
            vec![0.0, 1.0, 0.0],
            true,
        )?;
        LegacySemanticJournalStore::write_legacy_journal(&legacy_root, &[first, second])?;

        let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
        let store = LegacySemanticJournalStore::open(&legacy_root)?;
        let outcome = migrate_legacy_semantic_journal(&store, &adapter, &semantic_root)?;
        assert_eq!(outcome, SemanticMigrationOutcome::Migrated { imported: 2 });

        // The durable generation now exists and serves both surviving embeddings.
        let mut migrated_keys: BTreeSet<GenerationKey> = BTreeSet::new();
        for record in scan_persisted_generations(&semantic_root)? {
            let _present =
                migrated_keys.insert((record.repo_id, record.revision_id, record.generation));
        }
        assert!(migrated_keys.contains(&(repo_id(), revision_id(), ManifestGeneration::new(7))));

        let migrated_searcher =
            adapter.open(&repo_id(), &revision_id(), ManifestGeneration::new(7))?;
        let migrated_hits = migrated_searcher.search(&[0.0, 1.0, 0.0], 5)?;
        let migrated_ids: BTreeSet<String> = migrated_hits
            .iter()
            .map(|c| c.candidate_id.clone())
            .collect();

        // Re-running migration is idempotent (marker present -> AlreadyMigrated).
        let store_again = LegacySemanticJournalStore::open(&legacy_root)?;
        let outcome_again =
            migrate_legacy_semantic_journal(&store_again, &adapter, &semantic_root)?;
        assert_eq!(outcome_again, SemanticMigrationOutcome::AlreadyMigrated);

        // Equivalence: a clean durable build of the same batches yields the same
        // surviving embeddings.
        let clean_root: PathBuf = temp.path().join("clean").join("indexes").join("semantic");
        let clean_adapter = SemanticAdapter::with_state_root(clean_root)?;
        let mut clean_first = batch(
            ManifestGeneration::new(7),
            "emb-1",
            "x.rs",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        clean_first.batch_digest = "batch:7:a".to_string();
        build_legacy_durable(&clean_adapter, &clean_first)?;
        build_legacy_durable(
            &clean_adapter,
            &batch(
                ManifestGeneration::new(7),
                "emb-2",
                "y.rs",
                vec![0.0, 1.0, 0.0],
                true,
            )?,
        )?;
        let clean_searcher =
            clean_adapter.open(&repo_id(), &revision_id(), ManifestGeneration::new(7))?;
        let clean_hits = clean_searcher.search(&[0.0, 1.0, 0.0], 5)?;
        let clean_ids: BTreeSet<String> =
            clean_hits.iter().map(|c| c.candidate_id.clone()).collect();

        assert_eq!(migrated_ids, clean_ids);
        assert!(migrated_ids.contains("emb-2"));
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts crash-resume skips already-sealed generations via assert macros"
    )]
    fn migration_resume_skips_already_sealed_generations() -> TestResult {
        let temp = tempfile::tempdir()?;
        let legacy_root = temp.path().join("semantic");
        let semantic_root: PathBuf = temp.path().join("indexes").join("semantic");
        let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;

        // Simulate a migration that crashed after sealing gen 1 but before
        // writing the MIGRATED marker: gen 1 is durable on disk, marker absent.
        build_legacy_durable(
            &adapter,
            &batch(
                ManifestGeneration::new(1),
                "emb-1",
                "a.rs",
                vec![1.0, 0.0, 0.0],
                true,
            )?,
        )?;

        // The legacy journal still carries BOTH generations.
        let g1 = batch(
            ManifestGeneration::new(1),
            "emb-1",
            "a.rs",
            vec![1.0, 0.0, 0.0],
            true,
        )?;
        let g2 = batch(
            ManifestGeneration::new(2),
            "emb-2",
            "b.rs",
            vec![0.0, 1.0, 0.0],
            true,
        )?;
        LegacySemanticJournalStore::write_legacy_journal(&legacy_root, &[g1, g2])?;

        let store = LegacySemanticJournalStore::open(&legacy_root)?;
        let outcome = migrate_legacy_semantic_journal(&store, &adapter, &semantic_root)?;
        // gen 1 already sealed -> skipped; only gen 2 re-applied.
        assert_eq!(outcome, SemanticMigrationOutcome::Migrated { imported: 1 });

        let searcher = adapter.open(&repo_id(), &revision_id(), ManifestGeneration::new(2))?;
        let hits = searcher.search(&[0.0, 1.0, 0.0], 5)?;
        let ids: Vec<String> = hits.iter().map(|c| c.candidate_id.clone()).collect();
        assert_eq!(ids, vec!["emb-2".to_string()]);
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts mid-generation crash resume equals a clean build via assert macros"
    )]
    fn migration_resume_mid_generation_matches_clean_build() -> TestResult {
        // Crash BETWEEN batches of one generation: gen 7's first two (unsealed)
        // batches were applied to disk, but the sealing batch never completed.
        // Because replace/tombstone are absolute (remove-path-then-set), a resume
        // that re-replays the full journal over the partial working set must
        // converge to the same sealed generation a clean build produces.
        // gen 7 = [A replace x.rs->emb-1 (no seal),
        //          B replace x.rs->emb-2 (no seal, same path overwrites),
        //          C replace y.rs->emb-3 (seal)]
        let make_batch =
            |generation: u64, id: &str, path: &str, vector: Vec<f32>, seal: bool, tag: &str| {
                let mut b = batch(ManifestGeneration::new(generation), id, path, vector, seal)?;
                b.batch_digest = format!("batch:{generation}:{tag}");
                Ok::<SemanticIngestBatch, String>(b)
            };
        let a = make_batch(7, "emb-1", "x.rs", vec![1.0, 0.0, 0.0], false, "a")?;
        let b = make_batch(7, "emb-2", "x.rs", vec![0.0, 1.0, 0.0], false, "b")?;
        let c = make_batch(7, "emb-3", "y.rs", vec![0.0, 0.0, 1.0], true, "c")?;

        // Resumed root: simulate the crash by applying A and B (unsealed) first,
        // then migrate the full journal [A, B, C] with no MIGRATED marker.
        let resumed = tempfile::tempdir()?;
        let resumed_semantic: PathBuf = resumed.path().join("indexes").join("semantic");
        let resumed_adapter = SemanticAdapter::with_state_root(resumed_semantic.clone())?;
        build_legacy_durable(&resumed_adapter, &a)?;
        build_legacy_durable(&resumed_adapter, &b)?;
        let store = LegacySemanticJournalStore::write_legacy_journal(
            resumed.path().join("semantic"),
            &[a.clone(), b.clone(), c.clone()],
        )
        .and_then(|()| LegacySemanticJournalStore::open(resumed.path().join("semantic")))?;
        let _outcome =
            migrate_legacy_semantic_journal(&store, &resumed_adapter, &resumed_semantic)?;
        let resumed_searcher =
            resumed_adapter.open(&repo_id(), &revision_id(), ManifestGeneration::new(7))?;

        // Clean root: a fresh linear build of [A, B, C].
        let clean = tempfile::tempdir()?;
        let clean_semantic: PathBuf = clean.path().join("indexes").join("semantic");
        let clean_adapter = SemanticAdapter::with_state_root(clean_semantic)?;
        build_legacy_durable(&clean_adapter, &a)?;
        build_legacy_durable(&clean_adapter, &b)?;
        build_legacy_durable(&clean_adapter, &c)?;
        let clean_searcher =
            clean_adapter.open(&repo_id(), &revision_id(), ManifestGeneration::new(7))?;

        // Both must serve exactly the surviving embeddings {emb-2@x.rs, emb-3@y.rs}.
        let resumed_ids: BTreeSet<String> = resumed_searcher
            .search(&[0.0, 1.0, 0.0], 10)?
            .iter()
            .chain(resumed_searcher.search(&[0.0, 0.0, 1.0], 10)?.iter())
            .map(|c| c.candidate_id.clone())
            .collect();
        let clean_ids: BTreeSet<String> = clean_searcher
            .search(&[0.0, 1.0, 0.0], 10)?
            .iter()
            .chain(clean_searcher.search(&[0.0, 0.0, 1.0], 10)?.iter())
            .map(|c| c.candidate_id.clone())
            .collect();

        assert_eq!(resumed_ids, clean_ids);
        let mut expected: BTreeSet<String> = BTreeSet::new();
        let _e2 = expected.insert("emb-2".to_string());
        let _e3 = expected.insert("emb-3".to_string());
        assert_eq!(resumed_ids, expected);
        Ok(())
    }
}
