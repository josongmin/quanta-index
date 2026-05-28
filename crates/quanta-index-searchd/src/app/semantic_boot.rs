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

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};
use quanta_index_core::{CoreError, SemanticBatchBuildPort};
use quanta_index_search_plane::{Ledger, SemanticAuthorityStore};
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
/// migration run, and carries the cold-boot seed cost. Carries only enums,
/// counts, and a duration — never vectors, snippets, or path text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticBootReport {
    pub migration: SemanticMigrationOutcome,
    pub seed: SemanticSeedReport,
    pub seed_micros: u128,
}

type GenerationKey = (RepoId, RevisionId, ManifestGeneration);

/// Migrate the legacy semantic journal into durable generations exactly once.
///
/// Idempotent and resumable: generations already sealed on disk (from a prior
/// run or a partial crash) are skipped, so a re-run never tries to mutate a
/// sealed generation. The legacy journal is retained after success; only a
/// completion marker is written.
pub fn migrate_legacy_semantic_journal(
    store: &SemanticAuthorityStore,
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
    for batch in store.legacy_batches() {
        let key: GenerationKey = (
            batch.repo_id.clone(),
            batch.revision_id.clone(),
            batch.generation,
        );
        if sealed.contains(&key) {
            continue;
        }
        builder.build_batch(batch)?;
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
        BatchIngestMode, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
        EmbeddingNormalization, EmbeddingRecord, OwnerDocKind, RepoRelativePath, SearchScopeKey,
        SearchScopeSurface, SemanticIngestBatch, SemanticReplaceScope, lex::LanguageCode,
    };
    use quanta_index_core::SemanticIndexOpenPort;
    use quanta_index_semantic::SemanticAdapter;

    use super::{
        Arc, BTreeSet, GenerationKey, Ledger, ManifestGeneration, RepoId, RevisionId, RwLock,
        SearchPlaneTrackKind, SemanticAuthorityStore, SemanticMigrationOutcome,
        migrate_legacy_semantic_journal, scan_persisted_generations,
        seed_persisted_semantic_readiness,
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
        let language =
            LanguageCode::new("rust").map_err(|err| format!("fixture language invalid: {err}"))?;
        Ok(EmbeddingRecord {
            embedding_id: EmbeddingId::new(id),
            owner_kind: OwnerDocKind::Chunk,
            owner_id: format!("owner-{id}").into_boxed_str(),
            source_doc_id: format!("doc-{id}").into_boxed_str(),
            repo_relative_path: RepoRelativePath::new(path),
            language,
            symbol_kind: None,
            start_byte: 0,
            end_byte: 8,
            start_line: 1,
            end_line: 2,
            snippet: format!("fn {id}() {{}}").into_boxed_str(),
            embedding_input_digest: format!("in:{id}").into_boxed_str(),
            vector_digest: format!("vec:{id}").into_boxed_str(),
            view_kind: "raw_chunk".to_string().into_boxed_str(),
            vector,
        })
    }

    fn batch(
        generation: ManifestGeneration,
        id: &str,
        path: &str,
        vector: Vec<f32>,
        seal: bool,
    ) -> Result<SemanticIngestBatch, String> {
        Ok(SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: format!("manifest:{}", generation.get()),
            batch_digest: format!("batch:{}", generation.get()),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: SearchScopeKey {
                    doc_surface: SearchScopeSurface::Chunk,
                    repo_relative_path: RepoRelativePath::new(path),
                },
                scope_digest: format!("scope:{path}"),
                embeddings: vec![embedding(id, path, vector)?],
            }],
            tombstone_scopes: Vec::new(),
            seal,
        })
    }

    fn build_durable(adapter: &SemanticAdapter, batch: &SemanticIngestBatch) -> TestResult {
        use quanta_index_core::SemanticBatchBuildPort as _;
        adapter.build_batch(batch)?;
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
        let adapter = SemanticAdapter::with_state_root(semantic_root.clone());
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
        let adapter = SemanticAdapter::with_state_root(semantic_root.clone());
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
        reason = "test asserts no-op migration outcome via assert macros"
    )]
    fn migration_without_journal_is_noop() -> TestResult {
        let temp = tempfile::tempdir()?;
        let semantic_root: PathBuf = temp.path().join("indexes").join("semantic");
        let adapter = SemanticAdapter::with_state_root(semantic_root.clone());
        let store = SemanticAuthorityStore::open(temp.path().join("semantic"))?;
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
        SemanticAuthorityStore::write_legacy_journal(&legacy_root, &[first, second])?;

        let adapter = SemanticAdapter::with_state_root(semantic_root.clone());
        let store = SemanticAuthorityStore::open(&legacy_root)?;
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
        let store_again = SemanticAuthorityStore::open(&legacy_root)?;
        let outcome_again =
            migrate_legacy_semantic_journal(&store_again, &adapter, &semantic_root)?;
        assert_eq!(outcome_again, SemanticMigrationOutcome::AlreadyMigrated);

        // Equivalence: a clean durable build of the same batches yields the same
        // surviving embeddings.
        let clean_root: PathBuf = temp.path().join("clean").join("indexes").join("semantic");
        let clean_adapter = SemanticAdapter::with_state_root(clean_root);
        let mut clean_first = batch(
            ManifestGeneration::new(7),
            "emb-1",
            "x.rs",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        clean_first.batch_digest = "batch:7:a".to_string();
        build_durable(&clean_adapter, &clean_first)?;
        build_durable(
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
}
