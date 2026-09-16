#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Semantic adapter — persisted, generation-scoped vector index backed by the
//! `lancedb` crate (LDB-00 §3.2 revised decision).
//!
//! Implements the batch-native semantic build/open ports from
//! `quanta-index-core::domains::semantic` against a real lancedb dataset under
//! `{state_root}/indexes/semantic/generation-v1-{sha256(repo, revision)}/g{generation}/`. The
//! adapter owns a `tokio::runtime::Runtime` and bridges async lancedb calls to
//! the sync port surface via `Runtime::block_on` — this adapter *is* the
//! deliberate async↔sync seam (the `disallowed_methods` rule against
//! `block_on` is honored by a single, narrowly-scoped `#[expect]` on the
//! crate-private `run_blocking` helper — the only `block_on` call site in
//! the crate; both the build and query paths funnel through it).
//!
//! A sealed generation is built once (lancedb table + ANN index + scope
//! manifest + READY/SEALED markers) and opened directly from durable state at
//! query time. There is no boot-time replay. Vendor / file-layout knowledge
//! stays inside this crate; the public surface is the two ports plus
//! [`scan_persisted_generations`] for readiness seeding at the composition
//! root, and [`semantic_state_root`] so the composition root does not hardcode
//! the layout root.
//!
//! See `docs/plans/may-28-lancedb-adoption/` for the full backend decision and
//! migration packet.

mod build;
mod codec;
mod errors;
mod generation_contract;
mod layout;
mod manifest;
mod membership_integrity;
mod search;
mod semantic_ingest_fixtures_v1;
mod semantic_row_integrity_v1;
mod sql;

pub use semantic_ingest_fixtures_v1::{
    embedding_record_v1, ingest_batch_v1, legacy_chunk_embedding_record_v1, model_contract_v1,
    sealed_replace_batch_v1, search_scope_v1, tombstone_scope_v1,
    tombstone_scope_with_semantic_owner_v1,
};

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quanta_index_contract::{
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, SealedGenerationScanPort, SemanticBatchBuildPort,
    SemanticIndexOpenPort,
    domains::generation::{
        GenerationQuarantineReasonV1, GenerationStorageKeyV1, IncompleteGenerationDiscardOutcomeV1,
        IncompleteGenerationDiscardPort, QuarantinedGenerationV1, SealedGenerationInventoryV1,
        SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort,
    },
    domains::semantic::SemanticSearcher,
};

use crate::manifest::SemanticManifest;
use crate::search::{PersistedSemanticSearcher, open_generation};

#[cfg(debug_assertions)]
pub mod test_support {
    /// Debug-only test hook for injected append failure rails.
    pub fn set_append_fail_path(path: Option<&str>) {
        crate::build::set_append_fail_path_for_debug(path);
    }
}

/// Persisted semantic adapter rooted at a semantic state directory.
///
/// The state directory is typically `{state_root}/indexes/semantic` at the
/// composition root. Owns a tokio runtime used to drive lancedb's async API
/// behind the sync port surface.
pub struct SemanticAdapter {
    state_root: PathBuf,
    runtime: Arc<tokio::runtime::Runtime>,
}

/// Single crate-wide async↔sync seam funnel.
///
/// Every entry from a sync port (`SemanticBatchBuildPort` /
/// `SemanticIndexOpenPort` / `SemanticSearcher`) into the async lancedb crate
/// routes through this one helper. Both the build path (held by
/// `SemanticAdapter`) and the query path (held by `PersistedSemanticSearcher`)
/// call through it, so the workspace `disallowed_methods` exception is
/// localized to exactly one `#[expect]` site rather than mirrored across
/// adapter + searcher.
#[expect(
    clippy::disallowed_methods,
    reason = "the semantic adapter is the deliberate async↔sync seam between the sync port surface and the async lancedb crate; the entire crate funnels through this single helper"
)]
pub(crate) fn run_blocking<F: core::future::Future>(
    runtime: &tokio::runtime::Runtime,
    future: F,
) -> F::Output {
    runtime.block_on(future)
}

impl SemanticAdapter {
    /// Returns `Err` if the tokio runtime cannot be constructed (a rare system-
    /// resource failure). The construction would otherwise have to panic, which
    /// the workspace lint regime forbids.
    pub fn with_state_root(state_root: PathBuf) -> Result<Self, CoreError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("quanta-index-semantic-lancedb")
            .build()
            .map_err(|err| {
                CoreError::Storage(format!(
                    "semantic: tokio runtime init for lancedb adapter: {err}"
                ))
            })?;
        Ok(Self {
            state_root,
            runtime: Arc::new(runtime),
        })
    }
}

impl SemanticBatchBuildPort for SemanticAdapter {
    fn build_batch(
        &self,
        batch: &quanta_index_contract::SemanticIngestBatch,
    ) -> Result<(), CoreError> {
        run_blocking(&self.runtime, build::build_batch(&self.state_root, batch))
    }
}

impl SemanticIndexOpenPort for SemanticAdapter {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        // A cold open every time, by design: residency belongs to the search
        // plane's snapshot registry, which is the single owner of opened
        // handles across both tracks. A second cache here would double the
        // resident bytes and hide staleness from the registry's invalidation.
        let loaded = Arc::new(run_blocking(
            &self.runtime,
            open_generation(&self.state_root, repo, revision, generation),
        )?);
        Ok(Box::new(PersistedSemanticSearcher::new(
            loaded,
            Arc::clone(&self.runtime),
        )))
    }
}

impl GenerationIdentityValidatePort for SemanticAdapter {
    fn validate_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        if candidate.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(format!(
                "semantic identity validator received {:?} track",
                candidate.track
            )));
        }
        let generation_dir = layout::generation_dir(
            &self.state_root,
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
        );
        if !generation_dir.is_dir() {
            return Err(CoreError::NotFound(format!(
                "semantic: generation directory is absent: {}",
                generation_dir.display()
            )));
        }
        let sealed_digest = std::fs::read_to_string(layout::sealed_marker_path(&generation_dir))
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    CoreError::Typed {
                        code: "GENERATION_IDENTITY_INCOMPLETE".to_string(),
                        message: format!(
                            "semantic: incomplete generation has no sealed marker for generation {}",
                            candidate.manifest_generation.get()
                        ),
                    }
                } else {
                    CoreError::Storage(format!(
                        "semantic: read sealed marker for generation {}: {error}",
                        candidate.manifest_generation.get()
                    ))
                }
            })?;
        let manifest_bytes =
            std::fs::read(layout::manifest_path(&generation_dir)).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    CoreError::Typed {
                        code: "GENERATION_IDENTITY_INCOMPLETE".to_string(),
                        message: format!(
                            "semantic: incomplete generation has no manifest for generation {}",
                            candidate.manifest_generation.get()
                        ),
                    }
                } else {
                    CoreError::Storage(format!(
                        "semantic: read manifest for generation {}: {error}",
                        candidate.manifest_generation.get()
                    ))
                }
            })?;
        let manifest = SemanticManifest::decode(&manifest_bytes)?;
        manifest.validate_scope(
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
        )?;
        if sealed_digest != candidate.manifest_digest
            || manifest.manifest_digest != candidate.manifest_digest
        {
            return Err(CoreError::Typed {
                code: "GENERATION_IDENTITY_DIGEST_MISMATCH".to_string(),
                message: format!(
                    "semantic: durable generation digest mismatch for repo={} revision={} generation={}",
                    candidate.repo_id.as_str(),
                    candidate.revision_id.as_str(),
                    candidate.manifest_generation.get(),
                ),
            });
        }
        // Bypass the query cache so deleted/corrupt physical state cannot be
        // admitted from a stale in-memory searcher.
        let _loaded = run_blocking(
            &self.runtime,
            open_generation(
                &self.state_root,
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            ),
        )?;
        File::open(&generation_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                CoreError::Storage(format!(
                    "semantic: revalidate generation-directory durability {}: {error}",
                    generation_dir.display()
                ))
            })?;
        Ok(())
    }
}

impl IncompleteGenerationDiscardPort for SemanticAdapter {
    fn discard_incomplete_generation(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<IncompleteGenerationDiscardOutcomeV1, CoreError> {
        if candidate.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(format!(
                "semantic incomplete-generation discard received {:?} track",
                candidate.track
            )));
        }
        let generation_dir = layout::generation_dir(
            &self.state_root,
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
        );
        if !generation_dir.exists() {
            return Ok(IncompleteGenerationDiscardOutcomeV1::Absent);
        }

        let marker_path = layout::sealed_marker_path(&generation_dir);
        if marker_path.exists() {
            let observed_digest = std::fs::read_to_string(&marker_path).map_err(|error| {
                CoreError::Storage(format!(
                    "semantic: read sealed marker {}: {error}",
                    marker_path.display()
                ))
            })?;
            if observed_digest != candidate.manifest_digest {
                return Err(generation_digest_mismatch(candidate, "sealed marker"));
            }
            let manifest_path = layout::manifest_path(&generation_dir);
            let manifest =
                SemanticManifest::decode(&std::fs::read(&manifest_path).map_err(|error| {
                    CoreError::Storage(format!(
                        "semantic: read manifest {}: {error}",
                        manifest_path.display()
                    ))
                })?)?;
            manifest.validate_scope(
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            )?;
            if manifest.manifest_digest != candidate.manifest_digest {
                return Err(generation_digest_mismatch(candidate, "manifest"));
            }
            return Err(CoreError::Typed {
                code: "GENERATION_IMMUTABLE".to_string(),
                message: format!(
                    "semantic: refusing to discard sealed generation {}",
                    candidate.manifest_generation.get()
                ),
            });
        }

        let manifest_path = layout::manifest_path(&generation_dir);
        if manifest_path.exists() {
            let manifest =
                SemanticManifest::decode(&std::fs::read(&manifest_path).map_err(|error| {
                    CoreError::Storage(format!(
                        "semantic: read incomplete manifest {}: {error}",
                        manifest_path.display()
                    ))
                })?)?;
            manifest.validate_scope(
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            )?;
            if manifest.manifest_digest != candidate.manifest_digest {
                return Err(generation_digest_mismatch(candidate, "incomplete manifest"));
            }
        }
        std::fs::remove_dir_all(&generation_dir).map_err(|error| {
            CoreError::Storage(format!(
                "semantic: discard incomplete generation {}: {error}",
                generation_dir.display()
            ))
        })?;
        Ok(IncompleteGenerationDiscardOutcomeV1::Discarded)
    }
}

impl SealedGenerationReclaimPort for SemanticAdapter {
    fn reclaim_sealed_generation(
        &self,
        retired: &GenerationSnapshot,
    ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
        if retired.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(format!(
                "semantic sealed-generation reclaim received {:?} track",
                retired.track
            )));
        }
        let generation_dir = layout::generation_dir(
            &self.state_root,
            &retired.repo_id,
            &retired.revision_id,
            retired.manifest_generation,
        );
        if !generation_dir.exists() {
            return Ok(SealedGenerationReclaimOutcomeV1::Absent);
        }
        let marker_path = layout::sealed_marker_path(&generation_dir);
        if !marker_path.exists() {
            return Err(CoreError::Typed {
                code: "GENERATION_NOT_SEALED".to_string(),
                message: format!(
                    "semantic: refusing to reclaim unsealed generation {} as retired history",
                    retired.manifest_generation.get()
                ),
            });
        }
        let observed_digest = std::fs::read_to_string(&marker_path).map_err(|error| {
            CoreError::Storage(format!(
                "semantic: read sealed marker {}: {error}",
                marker_path.display()
            ))
        })?;
        if observed_digest != retired.manifest_digest {
            return Err(generation_digest_mismatch(retired, "sealed marker"));
        }
        let manifest_path = layout::manifest_path(&generation_dir);
        let manifest =
            SemanticManifest::decode(&std::fs::read(&manifest_path).map_err(|error| {
                CoreError::Storage(format!(
                    "semantic: read manifest {}: {error}",
                    manifest_path.display()
                ))
            })?)?;
        manifest.validate_scope(
            &retired.repo_id,
            &retired.revision_id,
            retired.manifest_generation,
        )?;
        if manifest.manifest_digest != retired.manifest_digest {
            return Err(generation_digest_mismatch(retired, "manifest"));
        }
        let bytes = crate::search::dataset_tree_bytes(&generation_dir)?;
        std::fs::remove_dir_all(&generation_dir).map_err(|error| {
            CoreError::Storage(format!(
                "semantic: reclaim sealed generation {}: {error}",
                generation_dir.display()
            ))
        })?;
        if let Some(parent) = generation_dir.parent() {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "semantic: fsync pair directory {} after reclaim: {error}",
                        parent.display()
                    ))
                })?;
        }
        Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes })
    }

    fn sealed_generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<GenerationSnapshot>, CoreError> {
        let pair_dir = self
            .state_root
            .join(GenerationStorageKeyV1::for_repo_revision(repo_id, revision_id).as_str());
        let mut out = Vec::new();
        if !pair_dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&pair_dir).map_err(|error| {
            CoreError::Storage(format!("semantic: list {}: {error}", pair_dir.display()))
        })? {
            let entry = entry.map_err(|error| {
                CoreError::Storage(format!("semantic: read generation entry: {error}"))
            })?;
            let generation_dir = entry.path();
            if !generation_dir.is_dir() {
                continue;
            }
            let marker_path = layout::sealed_marker_path(&generation_dir);
            if !marker_path.exists() {
                continue;
            }
            let manifest_path = layout::manifest_path(&generation_dir);
            let manifest =
                SemanticManifest::decode(&std::fs::read(&manifest_path).map_err(|error| {
                    CoreError::Storage(format!(
                        "semantic: read manifest {}: {error}",
                        manifest_path.display()
                    ))
                })?)?;
            let generation = ManifestGeneration::new(manifest.generation);
            manifest.validate_scope(repo_id, revision_id, generation)?;
            if layout::generation_dir(&self.state_root, repo_id, revision_id, generation)
                != generation_dir
            {
                return Err(CoreError::Typed {
                    code: "GENERATION_IDENTITY_SCOPE_MISMATCH".to_string(),
                    message: format!(
                        "semantic: manifest does not own physical path {}",
                        generation_dir.display()
                    ),
                });
            }
            let marker_digest = std::fs::read_to_string(&marker_path).map_err(|error| {
                CoreError::Storage(format!(
                    "semantic: read sealed marker {}: {error}",
                    marker_path.display()
                ))
            })?;
            if marker_digest != manifest.manifest_digest {
                return Err(CoreError::Typed {
                    code: "GENERATION_IDENTITY_DIGEST_MISMATCH".to_string(),
                    message: format!(
                        "semantic: sealed marker and manifest disagree at {}",
                        generation_dir.display()
                    ),
                });
            }
            out.push(GenerationSnapshot {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: generation,
                manifest_digest: manifest.manifest_digest.clone(),
            });
        }
        out.sort_by_key(|identity| identity.manifest_generation);
        Ok(out)
    }
}

fn generation_digest_mismatch(candidate: &GenerationSnapshot, source: &str) -> CoreError {
    CoreError::Typed {
        code: "GENERATION_IDENTITY_DIGEST_MISMATCH".to_string(),
        message: format!(
            "semantic: {source} digest conflicts with candidate for repo={} revision={} generation={}",
            candidate.repo_id.as_str(),
            candidate.revision_id.as_str(),
            candidate.manifest_generation.get()
        ),
    }
}

/// The semantic sub-root within an overall daemon state root.
///
/// This crate owns the semantic on-disk layout, including where its generation
/// tree is rooted, so the composition root derives the path here rather than
/// hardcoding `indexes/semantic` itself.
#[must_use]
pub fn semantic_state_root(state_root: &Path) -> PathBuf {
    state_root.join("indexes").join("semantic")
}

/// A sealed semantic generation discovered on disk, for readiness seeding.
///
/// Carries what the manifest says about the generation and nothing the
/// inventory verified about its content: the row root and row count here are
/// claims until [`validate_persisted_generation_v2`] opens the generation.
///
/// `#[non_exhaustive]` reserves the right to add fields without a breaking
/// change at this public surface: this struct is **produced** by
/// [`inventory_persisted_generations`] and **consumed** by the composition
/// root's readiness seeder, both of which already match on named fields.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PersistedSemanticGeneration {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub manifest_digest: String,
    format_version: u32,
    semantic_row_root_digest: String,
    row_count: u64,
}

impl PersistedSemanticGeneration {
    #[must_use]
    pub fn identity(&self) -> GenerationSnapshot {
        GenerationSnapshot {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: self.generation,
            manifest_digest: self.manifest_digest.clone(),
        }
    }
}

/// What the inventory found under a semantic state root (QI-BB-026).
#[derive(Clone, Debug, Default)]
pub struct SemanticGenerationInventoryV1 {
    pub sealed: Vec<PersistedSemanticGeneration>,
    pub quarantined: Vec<QuarantinedGenerationV1>,
}

/// Opaque proof minted only by a successful v7 durable open.
#[derive(Clone, Debug)]
pub struct ValidatedPersistedSemanticGenerationV2 {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    manifest_digest: String,
    semantic_row_root_digest: String,
    row_count: u64,
}

/// Prove one inventoried generation's durable content and mint the witness
/// the legacy migration records.
///
/// This is the deep step the inventory deliberately does not take: it opens
/// the generation, which verifies the schema, the row count, the row root
/// and the membership commitment against the manifest. Callers run it for
/// exactly the generations they need proven.
pub fn validate_persisted_generation_v2(
    semantic_root: &Path,
    record: &PersistedSemanticGeneration,
) -> Result<ValidatedPersistedSemanticGenerationV2, CoreError> {
    if record.format_version != manifest::FORMAT_VERSION {
        return Err(CoreError::Typed {
            code: "LEGACY_SEMANTIC_MIGRATION_DURABLE_FORMAT_UNVERIFIED".to_string(),
            message: format!(
                "semantic generation format {} has no v7 row-root proof",
                record.format_version
            ),
        });
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name("quanta-index-semantic-validate")
        .build()
        .map_err(|err| {
            CoreError::Storage(format!(
                "semantic: tokio runtime init for persisted-generation validation: {err}"
            ))
        })?;
    let _loaded = run_blocking(
        &runtime,
        open_generation(
            semantic_root,
            &record.repo_id,
            &record.revision_id,
            record.generation,
        ),
    )?;
    Ok(ValidatedPersistedSemanticGenerationV2 {
        repo_id: record.repo_id.clone(),
        revision_id: record.revision_id.clone(),
        generation: record.generation,
        manifest_digest: record.manifest_digest.clone(),
        semantic_row_root_digest: record.semantic_row_root_digest.clone(),
        row_count: record.row_count,
    })
}

impl ValidatedPersistedSemanticGenerationV2 {
    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        &self.repo_id
    }
    #[must_use]
    pub fn revision_id(&self) -> &RevisionId {
        &self.revision_id
    }
    #[must_use]
    pub const fn generation(&self) -> ManifestGeneration {
        self.generation
    }
    #[must_use]
    pub fn manifest_digest(&self) -> &str {
        self.manifest_digest.as_str()
    }
    #[must_use]
    pub fn semantic_row_root_digest(&self) -> &str {
        self.semantic_row_root_digest.as_str()
    }
    #[must_use]
    pub const fn row_count(&self) -> u64 {
        self.row_count
    }
}

/// Inventory a semantic state root for sealed generations (QI-BB-026).
///
/// Returns one record per `(repo, revision, generation)` directory that
/// carries a SEALED marker and a scope-consistent manifest whose digest the
/// marker agrees with. This reads two small files per generation and opens
/// nothing: content (schema, row count, row root, membership) is proven by
/// [`validate_persisted_generation_v2`] and by every open, not here. A
/// directory the inventory cannot trust is quarantined with its path and
/// reason rather than failing the whole inventory; in-progress (materialized
/// but unsealed) generations are skipped so a crashed build never seeds
/// false readiness. Only an unreadable directory listing is an error.
pub fn inventory_persisted_generations(
    semantic_root: &Path,
) -> Result<SemanticGenerationInventoryV1, CoreError> {
    let mut inventory = SemanticGenerationInventoryV1::default();
    if !semantic_root.exists() {
        return Ok(inventory);
    }
    for family_entry in read_dir(semantic_root)? {
        let family_entry = dir_entry(family_entry)?;
        if !is_dir(&family_entry)? {
            continue;
        }
        let family_name = family_entry.file_name();
        if !family_name
            .to_str()
            .is_some_and(GenerationStorageKeyV1::is_canonical_name)
        {
            inventory.quarantined.push(quarantine(
                family_entry.path(),
                GenerationQuarantineReasonV1::NonCanonicalLayout,
                "directory is not a canonical generation family; it needs explicit migration"
                    .to_string(),
            ));
            continue;
        }
        for generation_entry in read_dir(&family_entry.path())? {
            let generation_entry = dir_entry(generation_entry)?;
            if !is_dir(&generation_entry)? {
                continue;
            }
            match inventory_generation_dir(semantic_root, &generation_entry.path()) {
                Ok(Some(record)) => inventory.sealed.push(record),
                Ok(None) => {}
                Err(quarantined) => inventory.quarantined.push(quarantined),
            }
        }
    }
    Ok(inventory)
}

/// One generation directory's inventory outcome: `Ok(Some)` for a sealed
/// manifest that owns the directory and agrees with its marker, `Ok(None)`
/// for an in-progress build, `Err` for a quarantine.
fn inventory_generation_dir(
    semantic_root: &Path,
    generation_dir: &Path,
) -> Result<Option<PersistedSemanticGeneration>, QuarantinedGenerationV1> {
    let Some(generation_name) = generation_dir.file_name().and_then(|name| name.to_str()) else {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory name is not UTF-8".to_string(),
        ));
    };
    let canonical_generation_name = generation_name.strip_prefix('g').is_some_and(|raw| {
        raw.parse::<u64>()
            .is_ok_and(|generation| format!("g{generation}") == generation_name)
    });
    if !canonical_generation_name {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory is not `g<N>`; it needs explicit migration".to_string(),
        ));
    }
    let marker_path = layout::sealed_marker_path(generation_dir);
    if !marker_path.exists() {
        return Ok(None);
    }
    let unreadable = |detail: String| {
        quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
            detail,
        )
    };
    let manifest_path = layout::manifest_path(generation_dir);
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|err| unreadable(format!("read manifest {}: {err}", manifest_path.display())))?;
    let manifest = SemanticManifest::decode(&manifest_bytes)
        .map_err(|err| unreadable(format!("decode manifest: {err}")))?;
    let repo_id = RepoId::new(manifest.repo_id.clone());
    let revision_id = RevisionId::new(manifest.revision_id.clone());
    let generation = ManifestGeneration::new(manifest.generation);
    if let Err(err) = manifest.validate_scope(&repo_id, &revision_id, generation) {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::ScopeMismatch,
            err.to_string(),
        ));
    }
    if GenerationStorageKeyV1::for_repo_revision(&repo_id, &revision_id)
        .generation_dir(semantic_root, generation)
        != generation_dir
    {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::ScopeMismatch,
            format!(
                "manifest names repo={} revision={} generation={}, which does not own this directory",
                repo_id.as_str(),
                revision_id.as_str(),
                generation.get()
            ),
        ));
    }
    let sealed_digest = std::fs::read_to_string(&marker_path)
        .map_err(|err| unreadable(format!("read sealed marker: {err}")))?;
    if sealed_digest != manifest.manifest_digest {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::IdentityDigestMismatch,
            "sealed marker and manifest disagree on the manifest digest".to_string(),
        ));
    }
    Ok(Some(PersistedSemanticGeneration {
        repo_id,
        revision_id,
        generation,
        manifest_digest: manifest.manifest_digest,
        format_version: manifest.format_version,
        semantic_row_root_digest: manifest.semantic_row_root_digest,
        row_count: manifest.row_count,
    }))
}

fn quarantine(
    path: PathBuf,
    reason: GenerationQuarantineReasonV1,
    detail: String,
) -> QuarantinedGenerationV1 {
    QuarantinedGenerationV1 {
        track: SearchPlaneTrackKind::Semantic,
        path,
        reason,
        detail,
    }
}

impl SealedGenerationScanPort for SemanticAdapter {
    fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
        let inventory = inventory_persisted_generations(&self.state_root)?;
        Ok(SealedGenerationInventoryV1 {
            sealed: inventory
                .sealed
                .iter()
                .map(PersistedSemanticGeneration::identity)
                .collect(),
            quarantined: inventory.quarantined,
        })
    }
}

fn read_dir(path: &Path) -> Result<std::fs::ReadDir, CoreError> {
    std::fs::read_dir(path)
        .map_err(|err| CoreError::Storage(format!("semantic: list {}: {err}", path.display())))
}

fn dir_entry(entry: std::io::Result<std::fs::DirEntry>) -> Result<std::fs::DirEntry, CoreError> {
    entry.map_err(|err| CoreError::Storage(format!("semantic: read directory entry: {err}")))
}

fn is_dir(entry: &std::fs::DirEntry) -> Result<bool, CoreError> {
    let file_type = entry.file_type().map_err(|err| {
        CoreError::Storage(format!(
            "semantic: file type {}: {err}",
            entry.path().display()
        ))
    })?;
    Ok(file_type.is_dir())
}

#[cfg(test)]
mod incomplete_generation_discard_tests {
    use super::*;
    use crate::manifest::FORMAT_VERSION;

    fn candidate(generation: u64, digest: &str) -> GenerationSnapshot {
        GenerationSnapshot {
            repo_id: RepoId::new("../../repo-alpha"),
            revision_id: RevisionId::new("/rev-alpha"),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        }
    }

    fn manifest_for(identity: &GenerationSnapshot) -> SemanticManifest {
        let membership_commitment =
            crate::membership_integrity::cluster_membership_commitment_v1(Vec::new())
                .expect("empty membership commitment");
        SemanticManifest {
            format_version: FORMAT_VERSION,
            repo_id: identity.repo_id.as_str().to_string(),
            revision_id: identity.revision_id.as_str().to_string(),
            generation: identity.manifest_generation.get(),
            manifest_digest: identity.manifest_digest.clone(),
            model_id: "test-model".to_string(),
            model_version: None,
            dimension: 3,
            distance_metric: "cosine".to_string(),
            normalization: "l2_unit".to_string(),
            row_count: 0,
            semantic_row_root_digest: format!("sha256:{}", "0".repeat(64)),
            built_at_unix_nanos: 0,
            present_corpora: Vec::new(),
            required_corpora: Vec::new(),
            card_schema_versions: Vec::new(),
            render_policy_digests: Vec::new(),
            corpus_policy_digest: None,
            cluster_membership_root_digest: membership_commitment.root_digest,
            cluster_membership_cluster_count: membership_commitment.cluster_count,
            cluster_membership_member_row_count: membership_commitment.member_row_count,
        }
    }

    #[test]
    fn discard_is_contained_and_idempotent_for_incomplete_generation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf()).expect("adapter");
        let candidate = candidate(7, "digest-a");
        let generation_dir = layout::generation_dir(
            temp.path(),
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
        );
        std::fs::create_dir_all(&generation_dir).expect("create incomplete generation");
        std::fs::write(generation_dir.join("partial"), b"partial").expect("write partial");

        assert_eq!(
            adapter
                .discard_incomplete_generation(&candidate)
                .expect("discard incomplete"),
            IncompleteGenerationDiscardOutcomeV1::Discarded
        );
        assert!(!generation_dir.exists());
        assert!(generation_dir.starts_with(temp.path()));
        assert_eq!(
            adapter
                .discard_incomplete_generation(&candidate)
                .expect("idempotent absent"),
            IncompleteGenerationDiscardOutcomeV1::Absent
        );
    }

    #[test]
    fn discard_refuses_sealed_exact_and_digest_conflict() {
        let temp = tempfile::tempdir().expect("tempdir");
        let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf()).expect("adapter");
        let sealed = candidate(8, "digest-a");
        let generation_dir = layout::generation_dir(
            temp.path(),
            &sealed.repo_id,
            &sealed.revision_id,
            sealed.manifest_generation,
        );
        std::fs::create_dir_all(&generation_dir).expect("create generation");
        std::fs::write(
            layout::manifest_path(&generation_dir),
            manifest_for(&sealed).encode().expect("encode manifest"),
        )
        .expect("write manifest");
        std::fs::write(layout::sealed_marker_path(&generation_dir), b"digest-a")
            .expect("write marker");

        let exact_error = adapter
            .discard_incomplete_generation(&sealed)
            .expect_err("sealed exact must be immutable");
        assert!(matches!(
            exact_error,
            CoreError::Typed { ref code, .. } if code == "GENERATION_IMMUTABLE"
        ));
        let mut conflict = sealed;
        conflict.manifest_digest = "digest-b".to_string();
        let conflict_error = adapter
            .discard_incomplete_generation(&conflict)
            .expect_err("sealed digest conflict must fail closed");
        assert!(matches!(
            conflict_error,
            CoreError::Typed { ref code, .. }
                if code == "GENERATION_IDENTITY_DIGEST_MISMATCH"
        ));
        assert!(generation_dir.exists());
    }

    #[test]
    fn discard_refuses_incomplete_manifest_digest_conflict() {
        let temp = tempfile::tempdir().expect("tempdir");
        let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf()).expect("adapter");
        let observed = candidate(9, "digest-a");
        let generation_dir = layout::generation_dir(
            temp.path(),
            &observed.repo_id,
            &observed.revision_id,
            observed.manifest_generation,
        );
        std::fs::create_dir_all(&generation_dir).expect("create generation");
        std::fs::write(
            layout::manifest_path(&generation_dir),
            manifest_for(&observed).encode().expect("encode manifest"),
        )
        .expect("write manifest");
        let mut candidate = observed;
        candidate.manifest_digest = "digest-b".to_string();

        let error = adapter
            .discard_incomplete_generation(&candidate)
            .expect_err("manifest conflict must fail closed");
        assert!(matches!(
            error,
            CoreError::Typed { ref code, .. }
                if code == "GENERATION_IDENTITY_DIGEST_MISMATCH"
        ));
        assert!(generation_dir.exists());
    }

    /// A legacy raw layout is quarantined with its path, not a boot failure
    /// (QI-BB-026); the inventory is otherwise empty.
    #[test]
    fn inventory_quarantines_legacy_raw_identity_directories() {
        let temp = tempfile::tempdir().expect("tempdir");
        let legacy_family = temp.path().join("repo-alpha");
        std::fs::create_dir_all(legacy_family.join("rev-alpha/g1")).expect("create legacy layout");

        let inventory = inventory_persisted_generations(temp.path())
            .expect("inventory tolerates legacy layout");
        assert!(inventory.sealed.is_empty());
        assert_eq!(inventory.quarantined.len(), 1);
        let Some(entry) = inventory.quarantined.first() else {
            panic!("one quarantine record expected");
        };
        assert_eq!(entry.path, legacy_family);
        assert_eq!(
            entry.reason,
            GenerationQuarantineReasonV1::NonCanonicalLayout
        );
        assert_eq!(entry.track, SearchPlaneTrackKind::Semantic);
    }
}
