#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Semantic adapter — persisted, generation-scoped vector index backed by the
//! `lancedb` crate (LDB-00 §3.2 revised decision).
//!
//! Implements the scope-streamed semantic build port and the open port from
//! `quanta-index-core::domains::semantic` against a real lancedb dataset under
//! `{state_root}/indexes/semantic/generation-v1-{sha256(repo, revision)}/g{generation}/`. The
//! adapter owns a `tokio::runtime::Runtime` and bridges async lancedb calls to
//! the sync port surface via `Runtime::block_on` — this adapter *is* the
//! deliberate async↔sync seam (the `disallowed_methods` rule against
//! `block_on` is honored by a single, narrowly-scoped `#[expect]` on the
//! crate-private `run_blocking` helper — the only `block_on` call site in
//! the crate; both the build and query paths funnel through it). The build
//! drives its scope source on the calling thread, outside the runtime, and
//! enters the seam once per storage step, so a source that embeds over a
//! blocking transport never runs inside the async context.
//!
//! A sealed generation is built once (lancedb table + ANN index + scope
//! manifest + READY/SEALED markers) and opened directly from durable state at
//! query time. There is no boot-time replay. Vendor / file-layout knowledge
//! stays inside this crate; the public surface is the two ports, the
//! metrics source over the dense lanes' tallies (W5 phase 3), plus
//! [`inventory_persisted_generations`] for readiness seeding at the
//! composition root, and [`semantic_state_root`] so the composition root does
//! not hardcode the layout root.
//!
//! See `docs/plans/may-28-lancedb-adoption/` for the full backend decision and
//! migration packet.

mod budget;
mod build;
mod codec;
mod durable_write;
mod errors;
mod generation_contract;
mod integrity;
mod layout;
mod manifest;
mod membership_integrity;
mod sealed_manifest;
mod search;
mod semantic_ingest_fixtures_v1;
mod semantic_row_integrity_v1;
mod sql;
mod vector_index;

pub use semantic_ingest_fixtures_v1::{
    build_resident_batch_v1, embedding_record_v1, ingest_batch_v1,
    legacy_chunk_embedding_record_v1, model_contract_v1, sealed_replace_batch_v1, search_scope_v1,
    tombstone_scope_v1, tombstone_scope_with_semantic_owner_v1,
};

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use quanta_index_contract::{
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    SemanticContentRootsV1,
};
use quanta_index_core::{
    CoreError, FinishedReclaims, GenerationIdentityValidatePort, MetricPointV1, MetricSourcePort,
    RECLAIM_AREA_DIR_NAME, SealedGenerationIdentityProbePort, SealedGenerationScanPort,
    SemanticIndexOpenPort, SemanticIngestHeaderV1, SemanticScopeSource,
    SemanticScopeStreamBuildPort, SemanticStreamTallyV1, SemanticStreamWindowPolicy,
    TrackDiskUsagePort,
    domains::generation::{
        GenerationQuarantineReasonV1, GenerationStorageKeyV1, IncompleteGenerationDiscardOutcomeV1,
        IncompleteGenerationDiscardPort, InventoriedSealedGenerationV1, QuarantineDiscardOutcomeV1,
        QuarantinedGenerationDiscardPort, QuarantinedGenerationV1, SealedGenerationBytesV1,
        SealedGenerationInventoryV1, SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort,
        unique_inode_tree_bytes,
    },
    domains::semantic::{SemanticContentRootsPort, SemanticSearcher},
    reclaim_directory,
};

use crate::budget::DenseLaneTalliesV1;
use crate::codec::FORMAT_UNSUPPORTED_CODE;
use crate::integrity::{SealTalliesV1, read_quarantine_receipt};
use crate::manifest::SemanticManifest;
use crate::search::{LoadedGeneration, PersistedSemanticSearcher, open_generation};

const MAX_SEALED_MARKER_BYTES: usize = 4096;

#[cfg(debug_assertions)]
pub mod test_support {
    /// Debug-only test hook for injected append failure rails.
    pub fn set_append_fail_path(path: Option<&str>) {
        crate::build::set_append_fail_path_for_debug(path);
    }

    /// Debug-only delay injection for the cancellation end-to-end proof
    /// (W5 phase 3).
    ///
    /// While armed, every dense lane parks before issuing its vector
    /// query, so a request can only end through its budget. Process-wide;
    /// disarm it before the next served query.
    pub fn hold_dense_lane_until_interrupted(enabled: bool) {
        crate::budget::failpoint::set_hold_dense_lane_until_interrupted(enabled);
    }
}

/// Persisted semantic adapter rooted at a semantic state directory.
///
/// The state directory is typically `{state_root}/indexes/semantic` at the
/// composition root. Owns a tokio runtime used to drive lancedb's async API
/// behind the sync port surface, the window policy every streamed build
/// admits its windows against (QI-BB-021), the dense-lane tallies every
/// searcher it opens reports to (W5 phase 3), and the seal tallies every
/// seal it measures reports to (QI-BB-006 #4).
pub struct SemanticAdapter {
    state_root: PathBuf,
    runtime: Arc<tokio::runtime::Runtime>,
    window_policy: SemanticStreamWindowPolicy,
    query_tallies: Arc<DenseLaneTalliesV1>,
    seal_tallies: Arc<SealTalliesV1>,
    /// Serializes an integrity-scrub step against the removal of a sealed
    /// generation's directory (reclaim, quarantine discard), so a step
    /// finds the generation whole or gone — never files vanishing under it,
    /// which it would report as corruption and quarantine into a directory
    /// being deleted. The lexical adapter guards its directories the same
    /// way.
    directory_lifecycle: Mutex<()>,
}

/// Single crate-wide async↔sync seam funnel.
///
/// Every entry from a sync port (`SemanticScopeStreamBuildPort` /
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
    /// An adapter under the production window policy
    /// ([`SemanticStreamWindowPolicy::DEFAULT`]).
    ///
    /// Returns `Err` if the tokio runtime cannot be constructed (a rare system-
    /// resource failure). The construction would otherwise have to panic, which
    /// the workspace lint regime forbids.
    pub fn with_state_root(state_root: PathBuf) -> Result<Self, CoreError> {
        Self::with_state_root_and_window_policy(state_root, SemanticStreamWindowPolicy::DEFAULT)
    }

    /// An adapter whose streamed builds admit windows against `window_policy`.
    pub fn with_state_root_and_window_policy(
        state_root: PathBuf,
        window_policy: SemanticStreamWindowPolicy,
    ) -> Result<Self, CoreError> {
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
            window_policy,
            query_tallies: Arc::new(DenseLaneTalliesV1::default()),
            seal_tallies: Arc::new(SealTalliesV1::default()),
            directory_lifecycle: Mutex::new(()),
        })
    }

    /// Hold the sealed-generation directory lifecycle (see the field).
    pub(crate) fn directory_lifecycle_guard(&self) -> Result<MutexGuard<'_, ()>, CoreError> {
        self.directory_lifecycle.lock().map_err(|err| {
            CoreError::Storage(format!(
                "semantic: generation directory lifecycle lock poisoned: {err}"
            ))
        })
    }

    /// The window policy this adapter admits streamed windows against.
    #[must_use]
    pub const fn window_policy(&self) -> SemanticStreamWindowPolicy {
        self.window_policy
    }

    /// The semantic state directory this adapter owns.
    pub(crate) fn state_root(&self) -> &Path {
        &self.state_root
    }
}

impl SemanticScopeStreamBuildPort for SemanticAdapter {
    fn build_stream(
        &self,
        header: &SemanticIngestHeaderV1,
        scopes: &mut dyn SemanticScopeSource,
    ) -> Result<SemanticStreamTallyV1, CoreError> {
        build::build_stream(
            &self.runtime,
            &self.state_root,
            self.window_policy,
            header,
            scopes,
            &self.seal_tallies,
        )
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
        // resident bytes and hide a retirement from the registry.
        let loaded = run_blocking(
            &self.runtime,
            open_generation(&self.state_root, repo, revision, generation),
        )?;
        Ok(self.searcher(loaded))
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        let generation_dir = self.sealed_candidate_dir(candidate, "proven open")?;
        let loaded = self.open_loaded(candidate)?;
        sync_generation_directory(&generation_dir)?;
        Ok(self.searcher(loaded))
    }
}

impl SemanticAdapter {
    fn searcher(&self, loaded: LoadedGeneration) -> Box<dyn SemanticSearcher> {
        Box::new(PersistedSemanticSearcher::new(
            Arc::new(loaded),
            Arc::clone(&self.runtime),
            Arc::clone(&self.query_tallies),
        ))
    }
}

/// Every generation dataset under the semantic track root (QI-BB-015).
impl TrackDiskUsagePort for SemanticAdapter {
    /// Bytes the semantic state root occupies on disk, by unique inode (an
    /// inherited dataset's hard-linked fragments count once), measured
    /// while ingest, seals and reclaims keep running.
    fn track_disk_bytes(&self) -> Result<u64, CoreError> {
        if !self.state_root.exists() {
            return Ok(0);
        }
        unique_inode_tree_bytes(std::slice::from_ref(&self.state_root), &|_name| false).map_err(
            |err| {
                CoreError::Storage(format!(
                    "semantic: measure state root {}: {err}",
                    self.state_root.display()
                ))
            },
        )
    }
}

/// The dense lanes' tallies as scrape points, `semantic_dense_queries_…`
/// and `semantic_budget_interruptions_…` (QI-BB-015, W5 phase 3): the
/// queries each lane handed to the library and the request interruptions
/// it observed inside, across every searcher this adapter opened, and
/// `semantic_dense_exact_completions_total`, the short approximate passes
/// the exact lane answered (QI-BB-025); and the
/// seal tallies, `semantic_seal_…` (QI-BB-006 #4): the bytes every seal
/// hashed itself against what it inherited from its base.
impl MetricSourcePort for SemanticAdapter {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let mut points = self.query_tallies.scrape();
        points.extend(self.seal_tallies.scrape());
        Ok(points)
    }
}

impl GenerationIdentityValidatePort for SemanticAdapter {
    fn validate_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        let generation_dir = self.sealed_candidate_dir(candidate, "identity validator")?;
        // The cold open is the proof (QI-BB-017): it verifies the sealed
        // manifest's file commitment before decoding the scope manifest,
        // checks scope and digest, and opens the tables. Nothing is served
        // from a cache here, so deleted or corrupt physical state cannot be
        // admitted from a stale in-memory searcher.
        let _loaded = self.open_loaded(candidate)?;
        sync_generation_directory(&generation_dir)
    }
}

impl SealedGenerationIdentityProbePort for SemanticAdapter {
    fn probe_sealed_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        let _dir = self.sealed_candidate_dir(candidate, "readiness probe")?;
        Ok(())
    }
}

impl SemanticAdapter {
    /// The generation directory of `candidate`, whose sealed marker names
    /// exactly `candidate`'s digest.
    fn sealed_candidate_dir(
        &self,
        candidate: &GenerationSnapshot,
        what: &str,
    ) -> Result<PathBuf, CoreError> {
        if candidate.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(format!(
                "semantic {what} received {:?} track",
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
        let marker_path = layout::sealed_marker_path(&generation_dir);
        let mut sealed_digest = String::new();
        let _read_bytes = File::open(&marker_path)
            .and_then(|file| file.take(4097).read_to_string(&mut sealed_digest))
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityIncomplete,
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
        if sealed_digest.len() > MAX_SEALED_MARKER_BYTES {
            return Err(CoreError::Storage(format!(
                "semantic: sealed marker {} exceeds {MAX_SEALED_MARKER_BYTES} bytes",
                marker_path.display()
            )));
        }
        if sealed_digest != candidate.manifest_digest {
            return Err(CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
                message: format!(
                    "semantic: sealed marker says {sealed_digest} but the candidate says {} for repo={} revision={} generation={}",
                    candidate.manifest_digest,
                    candidate.repo_id.as_str(),
                    candidate.revision_id.as_str(),
                    candidate.manifest_generation.get(),
                ),
            });
        }
        Ok(generation_dir)
    }

    /// The cold open of `candidate`'s generation.
    fn open_loaded(&self, candidate: &GenerationSnapshot) -> Result<LoadedGeneration, CoreError> {
        run_blocking(
            &self.runtime,
            open_generation(
                &self.state_root,
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            ),
        )
    }
}

/// Make the generation directory's entries durable before a door admits it.
fn sync_generation_directory(generation_dir: &Path) -> Result<(), CoreError> {
    File::open(generation_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CoreError::Storage(format!(
                "semantic: revalidate generation-directory durability {}: {error}",
                generation_dir.display()
            ))
        })
}

/// The sealed manifest of a generation sealed under `candidate`'s digest.
///
/// The marker must exist and agree with both the candidate and the
/// manifest, and the manifest must be the current format and own the
/// candidate's scope. Reads two small files, no rows.
fn read_sealed_scope_manifest(
    semantic_root: &Path,
    candidate: &GenerationSnapshot,
) -> Result<SemanticManifest, CoreError> {
    let generation_dir = layout::generation_dir(
        semantic_root,
        &candidate.repo_id,
        &candidate.revision_id,
        candidate.manifest_generation,
    );
    let sealed_digest = std::fs::read_to_string(layout::sealed_marker_path(&generation_dir))
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityIncomplete,
                    message: format!(
                        "semantic: generation {} has no sealed marker",
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
    if sealed_digest != candidate.manifest_digest {
        return Err(generation_digest_mismatch(candidate, "sealed marker"));
    }
    let manifest_path = layout::manifest_path(&generation_dir);
    let manifest = SemanticManifest::decode(&std::fs::read(&manifest_path).map_err(|error| {
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
    Ok(manifest)
}

impl SemanticContentRootsPort for SemanticAdapter {
    fn sealed_content_roots(
        &self,
        sealed: &GenerationSnapshot,
    ) -> Result<SemanticContentRootsV1, CoreError> {
        if sealed.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(format!(
                "semantic content roots port received {:?} track",
                sealed.track
            )));
        }
        let manifest = read_sealed_scope_manifest(&self.state_root, sealed)?;
        Ok(SemanticContentRootsV1 {
            row_root_digest: manifest.semantic_row_root_digest,
            membership_root_digest: manifest.cluster_membership_root_digest,
        })
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
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable,
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
        let _lifecycle = self.directory_lifecycle_guard()?;
        if !generation_dir.exists() {
            return Ok(SealedGenerationReclaimOutcomeV1::Absent);
        }
        let marker_path = layout::sealed_marker_path(&generation_dir);
        if !marker_path.exists() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationNotSealed,
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
        // Out of the generation namespace by one durable rename, then
        // removed: a crash leaves a reclaim-area entry, never a partial tree
        // that would read as an unsealed build (QI-BB-003).
        reclaim_directory(
            &self.state_root,
            &generation_dir,
            &GenerationStorageKeyV1::for_repo_revision(&retired.repo_id, &retired.revision_id)
                .reclaim_entry_name(retired.manifest_generation),
        )?;
        Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes })
    }

    fn finish_interrupted_reclaims(&self) -> Result<FinishedReclaims, CoreError> {
        let _lifecycle = self.directory_lifecycle_guard()?;
        quanta_index_core::finish_interrupted_reclaims(&self.state_root)
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
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityScopeMismatch,
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
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
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

    /// Bytes the named generation directories occupy together, by unique
    /// inode, dataset and index files included.
    fn measure_sealed_generations(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        let mut roots = Vec::with_capacity(generations.len());
        let mut absent = BTreeSet::new();
        for generation in generations {
            let generation_dir =
                layout::generation_dir(&self.state_root, repo_id, revision_id, *generation);
            if generation_dir.is_dir() {
                roots.push(generation_dir);
            } else {
                let _new = absent.insert(*generation);
            }
        }
        let bytes = unique_inode_tree_bytes(&roots, &|_name| false).map_err(|err| {
            CoreError::Storage(format!(
                "semantic: measure sealed generations of repo={} revision={}: {err}",
                repo_id.as_str(),
                revision_id.as_str()
            ))
        })?;
        Ok(SealedGenerationBytesV1 { bytes, absent })
    }
}

fn generation_digest_mismatch(candidate: &GenerationSnapshot, source: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
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

/// Opaque proof minted only by a successful durable open.
#[derive(Clone, Debug)]
pub struct ValidatedPersistedSemanticGenerationV2 {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    manifest_digest: String,
    semantic_row_root_digest: String,
    row_count: u64,
}

/// Prove one inventoried current-format generation's durable content and
/// mint the witness used by boot and explicit validation.
///
/// This is the deep step the inventory deliberately does not take: it opens
/// the generation, which verifies the schema, the row count, the row root
/// and the membership commitment against the manifest. Callers run it for
/// exactly the generations they need proven.
pub fn validate_persisted_generation_v2(
    semantic_root: &Path,
    record: &PersistedSemanticGeneration,
) -> Result<ValidatedPersistedSemanticGenerationV2, CoreError> {
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
        // Reclaims in progress, finished by the reclaim port (QI-BB-003).
        if family_name == RECLAIM_AREA_DIR_NAME {
            continue;
        }
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
    if GenerationStorageKeyV1::generation_of_dir_name(generation_name).is_none() {
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
    // A receipt the integrity scrub left (QI-BB-017): the generation's
    // bytes were proven not to match its seal, so it is set aside under
    // that proof rather than seeded and refused at every door.
    match read_quarantine_receipt(generation_dir) {
        Ok(Some(receipt)) => {
            return Err(quarantine(
                generation_dir.to_path_buf(),
                GenerationQuarantineReasonV1::ContentCorrupt,
                receipt.detail,
            ));
        }
        Ok(None) => {}
        Err(err) => return Err(unreadable(format!("read quarantine receipt: {err}"))),
    }
    let manifest_path = layout::manifest_path(generation_dir);
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|err| unreadable(format!("read manifest {}: {err}", manifest_path.display())))?;
    let manifest = SemanticManifest::decode(&manifest_bytes).map_err(|err| {
        if let CoreError::Typed { code, message } = &err
            && *code == FORMAT_UNSUPPORTED_CODE
        {
            quarantine(
                generation_dir.to_path_buf(),
                GenerationQuarantineReasonV1::FormatUnsupported,
                message.clone(),
            )
        } else {
            unreadable(format!("decode manifest: {err}"))
        }
    })?;
    let repo_id = RepoId::new(manifest.repo_id.clone())
        .map_err(|error| unreadable(format!("semantic manifest has invalid repo ID: {error}")))?;
    let revision_id = RevisionId::new(manifest.revision_id.clone()).map_err(|error| {
        unreadable(format!(
            "semantic manifest has invalid revision ID: {error}"
        ))
    })?;
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

impl QuarantinedGenerationDiscardPort for SemanticAdapter {
    fn discard_quarantined_generation(
        &self,
        entry: &QuarantinedGenerationV1,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        if entry.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(format!(
                "semantic quarantine discard received {:?} track",
                entry.track
            )));
        }
        let _lifecycle = self.directory_lifecycle_guard()?;
        discard_quarantined_directory(
            &self.state_root,
            &inventory_persisted_generations(&self.state_root)?.quarantined,
            entry,
        )
    }
}

/// Remove `entry.path` if `quarantined_now` — the inventory taken this
/// instant — names it under the same reason (QI-BB-026).
///
/// The lexical adapter keeps the same protocol over its own inventory and
/// byte measure; the two diverge only in those, and each adapter must own
/// the deletion of what it owns, so the protocol is not shared as code.
fn discard_quarantined_directory(
    semantic_root: &Path,
    quarantined_now: &[QuarantinedGenerationV1],
    entry: &QuarantinedGenerationV1,
) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
    let not_quarantined = |why: String| CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
        message: format!(
            "semantic: refusing to discard {}: {why}",
            entry.path.display()
        ),
    };
    let Some(current) = quarantined_now
        .iter()
        .find(|quarantined| quarantined.path == entry.path)
    else {
        if std::fs::symlink_metadata(&entry.path).is_ok() {
            return Err(not_quarantined(
                "the path is not quarantined now; a sealed, in-progress or repaired directory is not this port's to remove"
                    .to_string(),
            ));
        }
        return Ok(QuarantineDiscardOutcomeV1::Absent);
    };
    if current.reason != entry.reason {
        return Err(not_quarantined(format!(
            "it is quarantined as {} now, not {} as listed; list again",
            current.reason.as_code_str(),
            entry.reason.as_code_str()
        )));
    }
    if !entry.path.starts_with(semantic_root) {
        return Err(not_quarantined(format!(
            "the path is outside the semantic root {}",
            semantic_root.display()
        )));
    }
    let metadata = std::fs::symlink_metadata(&entry.path).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: inspect quarantined {}: {error}",
            entry.path.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Err(not_quarantined(
            "the path is not a directory; the inventory quarantines directories only".to_string(),
        ));
    }
    let bytes = crate::search::dataset_tree_bytes(&entry.path)?;
    std::fs::remove_dir_all(&entry.path).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: discard quarantined {}: {error}",
            entry.path.display()
        ))
    })?;
    if let Some(parent) = entry.path.parent() {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                CoreError::Storage(format!(
                    "semantic: fsync {} after discarding quarantine: {error}",
                    parent.display()
                ))
            })?;
    }
    Ok(QuarantineDiscardOutcomeV1::Discarded { bytes })
}

impl SealedGenerationScanPort for SemanticAdapter {
    fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
        let inventory = inventory_persisted_generations(&self.state_root)?;
        Ok(SealedGenerationInventoryV1 {
            sealed: inventory
                .sealed
                .iter()
                .map(|record| InventoriedSealedGenerationV1 {
                    identity: record.identity(),
                    path: layout::generation_dir(
                        &self.state_root,
                        &record.repo_id,
                        &record.revision_id,
                        record.generation,
                    ),
                })
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
            repo_id: RepoId::new("../../repo-alpha")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("/rev-alpha")
                .expect("static fixture ID satisfies canonical policy"),
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
            vector_index: crate::manifest::VectorIndexSealV1 {
                mode: crate::manifest::VECTOR_INDEX_MODE_EXACT.to_string(),
                library: "lancedb".to_string(),
                library_version: "0.30.0".to_string(),
                index_min_rows: 256,
                ann: None,
            },
        }
    }

    #[test]
    fn readiness_probe_refuses_oversized_sealed_marker() {
        let temp = tempfile::tempdir().expect("fixture state root");
        let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf()).expect("adapter");
        let sealed = candidate(6, "digest-a");
        let generation_dir = layout::generation_dir(
            temp.path(),
            &sealed.repo_id,
            &sealed.revision_id,
            sealed.manifest_generation,
        );
        std::fs::create_dir_all(&generation_dir).expect("generation directory");
        std::fs::write(
            layout::sealed_marker_path(&generation_dir),
            vec![b'x'; 4097],
        )
        .expect("oversized marker fixture");
        let error = adapter
            .probe_sealed_generation_identity(&sealed)
            .expect_err("oversized marker must be refused");
        assert!(
            matches!(error, CoreError::Storage(message) if message.contains("exceeds 4096 bytes"))
        );
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
            CoreError::Typed { ref code, .. } if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable
        ));
        let mut conflict = sealed;
        conflict.manifest_digest = "digest-b".to_string();
        let conflict_error = adapter
            .discard_incomplete_generation(&conflict)
            .expect_err("sealed digest conflict must fail closed");
        assert!(matches!(
            conflict_error,
            CoreError::Typed { ref code, .. }
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch
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
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch
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
