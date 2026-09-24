//! Semantic boot path: durable readiness seeding.
//!
//! On boot the runtime only seeds the readiness ledger directly from the
//! sealed generations. A legacy state root is refused typed at boot
//! (see `state_format::refuse_legacy_state_root_v1`).
//! There is no steady-state journal authority and no full replay; seeding
//! cost is bounded by the count of sealed generations, not total batch
//! history.

use std::sync::{Arc, RwLock};

use quanta_index_contract::SearchPlaneTrackKind;
use quanta_index_core::{CoreError, SealedGenerationScanPort};
use quanta_index_search_plane::Ledger;

use super::boot_inventory::{TrackInventoryReportV1, seed_track_readiness};

/// Report of durable semantic readiness seeding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticSeedReport {
    pub sealed_generations: usize,
    /// Directories the inventory set aside (QI-BB-026); the paths and
    /// reasons are on the runtime's `BootInventoryReportV1`.
    pub quarantined_generations: usize,
}

impl SemanticSeedReport {
    #[must_use]
    pub fn from_track_report(report: &TrackInventoryReportV1) -> Self {
        Self {
            sealed_generations: report.sealed_generations,
            quarantined_generations: report.quarantined.len(),
        }
    }
}

/// Bounded, payload-free semantic boot observability (LDB-E2E-01 §4).
///
/// Carries the sealed-generation count and seed duration without payloads.
///
/// Intentionally does NOT derive `Eq` / `PartialEq` because the `_micros`
/// fields are wall-clock timings; structural equality across two reports is
/// meaningless. Tests should assert individual fields.
#[derive(Clone, Copy, Debug)]
pub struct SemanticBootReport {
    pub seed: SemanticSeedReport,
    /// Wall-clock cost of `seed_persisted_semantic_readiness`.
    pub seed_micros: u128,
}

/// Seed semantic readiness directly from the sealed-generation inventory.
///
/// Mirrors the lexical readiness seed: each inventoried generation's manifest
/// digest reconstructs the per-`(repo, revision, generation)` semantic
/// readiness state from durable identity rather than a replayed RAM graph.
/// Nothing is opened here (QI-BB-026); the active pairs are proven once by
/// the lifecycle's restart rehydrate.
pub fn seed_persisted_semantic_readiness(
    ledger: &Arc<RwLock<Ledger>>,
    scanner: &dyn SealedGenerationScanPort,
) -> Result<TrackInventoryReportV1, CoreError> {
    seed_track_readiness(ledger, SearchPlaneTrackKind::Semantic, scanner)
        .map_err(|err| CoreError::Storage(format!("semantic seed: {err}")))
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixtures in this module are built with known fixed lengths; an out-of-range index is a test authoring bug that should fail loudly"
)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning boot tests assert with `assert!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]
mod tests {
    use std::path::PathBuf;

    use quanta_index_contract::ManifestGeneration;
    use quanta_index_contract::{
        BatchIngestMode, EmbeddingDistanceMetric, EmbeddingModelContract, EmbeddingNormalization,
        EmbeddingRecord, RepoId, RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface,
        SemanticIngestBatch, SemanticReplaceScope,
    };
    use quanta_index_core::{GenerationStorageKeyV1, SemanticIndexOpenPort};
    use quanta_index_semantic::{
        SemanticAdapter, ingest_batch_v1, legacy_chunk_embedding_record_v1,
    };

    use super::{Arc, Ledger, RwLock, SearchPlaneTrackKind, seed_persisted_semantic_readiness};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn repo_id() -> RepoId {
        RepoId::new("repo-boot").expect("static fixture ID satisfies canonical policy")
    }

    fn revision_id() -> RevisionId {
        RevisionId::new("rev-boot").expect("static fixture ID satisfies canonical policy")
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
                cluster_memberships: Vec::new(),
            }],
            Vec::new(),
            seal,
        ))
    }

    fn build_durable(adapter: &SemanticAdapter, batch: &SemanticIngestBatch) -> TestResult {
        quanta_index_semantic::build_resident_batch_v1(adapter, batch)?;
        Ok(())
    }

    /// A ledger whose durable search-corpus authority retains
    /// `generations` of the fixture pair under the fixture digests: the
    /// serving boundary the seed measures the inventory against.
    fn ledger_retaining(generations: &[u64]) -> Arc<RwLock<Ledger>> {
        let mut ledger = Ledger::new();
        for generation in generations {
            ledger.record_historically_sealed_search_corpus(
                &repo_id(),
                &revision_id(),
                ManifestGeneration::new(*generation),
                &format!("manifest:{generation}"),
            );
        }
        Arc::new(RwLock::new(ledger))
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts seeded readiness via assert macros"
    )]
    fn seed_reconstructs_readiness_from_sealed_generations() -> TestResult {
        let temp = tempfile::tempdir()?;
        let semantic_root: PathBuf = temp.path().join("indexes").join("semantic");
        let adapter = SemanticAdapter::with_state_root(semantic_root)?;
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

        let ledger = ledger_retaining(&[3, 5]);
        let report = seed_persisted_semantic_readiness(&ledger, &adapter)?;
        assert_eq!(report.sealed_generations, 2);
        assert!(report.quarantined.is_empty());
        assert!(report.orphaned.is_empty());

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
        let adapter = SemanticAdapter::with_state_root(semantic_root)?;
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
        let report = seed_persisted_semantic_readiness(&ledger, &adapter)?;
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

    /// A content defect no longer stops boot seeding (QI-BB-026).
    ///
    /// The inventory lists the generation and the ledger seeds it; the door
    /// that opens it refuses the forged scope manifest as a corrupt sidecar
    /// (QI-BB-017).
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts a content-corrupted generation seeds but does not open via assert macros"
    )]
    fn seed_lists_a_content_corrupted_generation_and_open_refuses_it() -> TestResult {
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

        let ledger = ledger_retaining(&[9]);
        let report = seed_persisted_semantic_readiness(&ledger, &adapter)?;
        assert_eq!(report.sealed_generations, 1);
        assert!(report.quarantined.is_empty());
        let Err(err) = adapter.open(&repo_id(), &revision_id(), generation) else {
            return Err("the corrupted generation must not open".into());
        };
        assert!(
            matches!(
                err,
                quanta_index_core::CoreError::Typed { ref code, ref message }
                    if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt
                        && message.contains("semantic-manifest.cbor")
            ),
            "expected the sealed manifest to refuse the forged scope manifest, got: {err:?}"
        );
        Ok(())
    }
}
