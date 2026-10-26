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
//! See `docs/adr/MAY-31-001-lancedb-semantic-generation-authority.md` for the
//! backend and migration authority. The May-28 plan remains historical detail.

#[cfg(feature = "proof")]
pub mod ann_proof;
mod budget;
mod build;
mod codec;
mod control_file;
mod dataset_promotion;
mod durable_write;
mod errors;
mod generation_contract;
mod integrity;
mod layout;
mod manifest;
mod membership_integrity;
pub mod native;
mod physical_measurement;
#[cfg(feature = "proof")]
pub mod proof;
mod qualified_fs;
mod retention_measurement;
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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use lance_io::object_store::file_admission::FileAdmission;
use quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE;
use quanta_index_core::publication_capacity::PublicationAllowance;

use quanta_index_contract::{
    GenerationPin, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchCorpusIngestBatch, SearchPlaneTrackKind, SemanticContentRootsV1,
};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticLaneV1, DiagnosticStageV1, DiagnosticTerminalV1,
    FinishedReclaims, GenerationIdentityValidatePort, GenerationNamespaceReadPort, MetricPointV1,
    MetricSourcePort, RECLAIM_AREA_DIR_NAME, RequestBudgetV1, SealedGenerationIdentityProbePort,
    SealedGenerationScanPort, SemanticGenerationContractV1, SemanticIndexOpenPort,
    SemanticIngestHeaderV1, SemanticScopeSource, SemanticScopeStreamBuildPort,
    SemanticStreamTallyV1, SemanticStreamWindowPolicy, TrackDiskUsagePort,
    domains::generation::{
        GenerationQuarantineReasonV1, GenerationStorageKeyV1, IncompleteGenerationDiscardOutcomeV1,
        IncompleteGenerationDiscardPort, InventoriedSealedGenerationV1, QuarantineDiscardOutcomeV1,
        QuarantinedGenerationDiscardPort, QuarantinedGenerationV1, SealedGenerationBytesV1,
        SealedGenerationInventoryV1, SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort,
    },
    domains::semantic::{SemanticContentRootsPort, SemanticSearcher},
};
use sha2::{Digest as _, Sha256};

use crate::budget::DenseLaneTalliesV1;
use crate::codec::FORMAT_UNSUPPORTED_CODE;
use crate::integrity::{SealTalliesV1, read_quarantine_receipt_with_admission};
use crate::manifest::SemanticManifest;
use crate::sealed_manifest::inspect_sealed_manifest_identity_with_admission;
use crate::search::{
    LoadedGeneration, PersistedSemanticSearcher, open_generation, open_generation_admitted,
};

fn observe_result<T>(
    diagnostic: Option<&DiagnosticContextV1>,
    stage: DiagnosticStageV1,
    operation: impl FnOnce(Option<&DiagnosticContextV1>) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    let span = diagnostic.and_then(|context| {
        context.enter(stage, DiagnosticLaneV1(3)).map_or_else(
            |trace_error| {
                context.health(match trace_error {
                    quanta_index_core::DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                        quanta_index_core::DiagnosticHealthCodeV1::CounterOverflow
                    }
                    quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed => {
                        quanta_index_core::DiagnosticHealthCodeV1::ClockRegressed
                    }
                    quanta_index_core::DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                        quanta_index_core::DiagnosticHealthCodeV1::ForeignCaptureSpan
                    }
                    quanta_index_core::DiagnosticTraceErrorV1::MissingParentSpan => {
                        quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan
                    }
                });
                None
            },
            Some,
        )
    });
    let child = diagnostic
        .zip(span.as_ref())
        .map(|(context, span)| context.child_context(span));
    let result = operation(child.as_ref());
    if let Some(span) = span {
        let outcome = DiagnosticTerminalV1::from_result(&result);
        let _trace_completion = span.finish(outcome);
    }
    result
}

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

/// Build a private admitted Lance session from the held work allowance.
///
/// The composition root supplies one factory backed by
/// its canonical file binder; this crate never creates a capacity ledger.
pub trait SemanticFileAdmissionFactory: Send + Sync {
    fn for_work(
        &self,
        held_work: Arc<PublicationAllowance>,
    ) -> Result<Arc<dyn FileAdmission>, CoreError>;
}

/// Reserve maintenance writes under the global capacity authority.
///
/// A receipt written outside a publication obtains one independent parent.
/// The returned child must be
/// backed by the same canonical physical file binder as Lance writes.
pub trait SemanticMaintenanceFileFactory: Send + Sync {
    fn reserve_sidecar_output(
        &self,
        max_encoded_bytes: u64,
    ) -> Result<Box<dyn quanta_index_core::publication_admission::PublicationOutputPermit>, CoreError>;

    /// Reclaim existing files and directories under the same binder without
    /// granting a new writer parent or file-creation authority.
    fn retirement_admission(&self) -> Result<Arc<dyn FileAdmission>, CoreError>;

    /// Prepare only the private reclaim-area parent for a named entry.
    fn prepare_retirement_area(&self, entry: &Path) -> Result<(), CoreError>;
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
    /// Read-only authority for cold opens; every build receives a distinct
    /// writer admission from the held work parent below.
    lance_read_admission: Option<Arc<dyn FileAdmission>>,
    lance_work_factory: Option<Arc<dyn SemanticFileAdmissionFactory>>,
    maintenance_file_factory: Option<Arc<dyn SemanticMaintenanceFileFactory>>,
    runtime: Arc<tokio::runtime::Runtime>,
    window_policy: SemanticStreamWindowPolicy,
    retention_measurement_policy:
        quanta_index_core::domains::generation::RetentionMeasurementPolicy,
    query_tallies: Arc<DenseLaneTalliesV1>,
    seal_tallies: Arc<SealTalliesV1>,
    /// Serializes a target build or scrub with mutations of that generation;
    /// delta builds pin their base in the same bounded table.
    generation_mutations: [Mutex<()>; 256],
    /// Builds hold a read guard through their last write. Any directory
    /// removal, including incomplete discard, takes the write guard so it
    /// cannot remove a namespace while a build writes it or before a
    /// retirement fence is settled.
    directory_lifecycle: RwLock<()>,
    scrub_progress: Mutex<quanta_index_core::domains::integrity::IntegrityScrubProgressV1>,
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

fn open_generation_with_read_admission(
    runtime: &tokio::runtime::Runtime,
    state_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    admission: Option<&Arc<dyn FileAdmission>>,
) -> Result<LoadedGeneration, CoreError> {
    admission.map_or_else(
        || {
            run_blocking(
                runtime,
                open_generation(state_root, repo, revision, generation, None),
            )
        },
        |admission| {
            run_blocking(
                runtime,
                open_generation_admitted(
                    state_root,
                    repo,
                    revision,
                    generation,
                    qualified_fs::lance_session(admission),
                    Arc::clone(admission),
                ),
            )
        },
    )
}

impl SemanticAdapter {
    fn retirement_file_factory(
        &self,
    ) -> Result<Option<&dyn SemanticMaintenanceFileFactory>, CoreError> {
        match self.maintenance_file_factory.as_deref() {
            Some(factory) => Ok(Some(factory)),
            None if self.lance_read_admission.is_some() => Err(CoreError::Typed {
                code: quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                message: "qualified semantic retirement lacks same-ledger file admission".into(),
            }),
            None => Ok(None),
        }
    }

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
            lance_read_admission: None,
            lance_work_factory: None,
            maintenance_file_factory: None,
            runtime: Arc::new(runtime),
            window_policy,
            retention_measurement_policy:
                quanta_index_core::domains::generation::RetentionMeasurementPolicy::DEFAULT,
            query_tallies: Arc::new(DenseLaneTalliesV1::default()),
            seal_tallies: Arc::new(SealTalliesV1::default()),
            generation_mutations: std::array::from_fn(|_| Mutex::new(())),
            directory_lifecycle: RwLock::new(()),
            scrub_progress: Mutex::new(
                quanta_index_core::domains::integrity::IntegrityScrubProgressV1::default(),
            ),
        })
    }

    /// Attach the canonical root authority. Qualified builds require a held
    /// work parent and get a new Session; cold opens use read-only admission.
    #[must_use]
    pub fn with_qualified_file_admission(
        mut self,
        read_admission: Arc<dyn FileAdmission>,
        work_factory: Arc<dyn SemanticFileAdmissionFactory>,
    ) -> Self {
        self.lance_read_admission = Some(read_admission);
        self.lance_work_factory = Some(work_factory);
        self
    }

    /// Install a same-ledger reservation factory for scrub/quarantine receipts.
    #[must_use]
    pub fn with_maintenance_file_admission(
        mut self,
        factory: Arc<dyn SemanticMaintenanceFileFactory>,
    ) -> Self {
        self.maintenance_file_factory = Some(factory);
        self
    }

    /// Configure the disposable main-file cap for retention membership.
    #[must_use]
    pub const fn with_retention_measurement_policy(
        mut self,
        policy: quanta_index_core::domains::generation::RetentionMeasurementPolicy,
    ) -> Self {
        self.retention_measurement_policy = policy;
        self
    }

    /// Hold the sealed-generation directory lifecycle (see the field).
    pub(crate) fn directory_lifecycle_guard(&self) -> Result<RwLockWriteGuard<'_, ()>, CoreError> {
        self.directory_lifecycle.write().map_err(|err| {
            CoreError::Storage(format!(
                "semantic: generation directory lifecycle lock poisoned: {err}"
            ))
        })
    }

    pub(crate) fn directory_lifecycle_read_guard(
        &self,
    ) -> Result<RwLockReadGuard<'_, ()>, CoreError> {
        self.directory_lifecycle.read().map_err(|err| {
            CoreError::Storage(format!(
                "semantic: generation directory lifecycle lock poisoned: {err}"
            ))
        })
    }

    fn generation_mutation_stripe(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<usize, CoreError> {
        let mut hasher = Sha256::new();
        hasher.update(
            GenerationStorageKeyV1::for_repo_revision(repo_id, revision_id)
                .as_str()
                .as_bytes(),
        );
        hasher.update(generation.get().to_le_bytes());
        let digest = hasher.finalize();
        let stripe = usize::from(
            digest
                .first()
                .copied()
                .ok_or_else(|| CoreError::Storage("semantic mutation digest is empty".into()))?,
        );
        if self.generation_mutations.get(stripe).is_none() {
            return Err(CoreError::Storage(
                "semantic mutation stripe outside table".into(),
            ));
        }
        Ok(stripe)
    }

    pub(crate) fn generation_mutation_guard(
        &self,
        generation: &GenerationSnapshot,
    ) -> Result<MutexGuard<'_, ()>, CoreError> {
        let stripe = self.generation_mutation_stripe(
            &generation.repo_id,
            &generation.revision_id,
            generation.manifest_generation,
        )?;
        self.generation_mutations
            .get(stripe)
            .ok_or_else(|| {
                CoreError::InvalidContract(format!(
                    "semantic mutation stripe {stripe} is out of range"
                ))
            })?
            .lock()
            .map_err(|error| {
                CoreError::Storage(format!("semantic mutation lock poisoned: {error}"))
            })
    }

    fn generation_build_guards(
        &self,
        header: &SemanticIngestHeaderV1,
    ) -> Result<Vec<MutexGuard<'_, ()>>, CoreError> {
        self.generation_contract_guards(&header.pin, &header.contract)
    }

    fn generation_contract_guards(
        &self,
        pin: &GenerationPin,
        contract: &SemanticGenerationContractV1,
    ) -> Result<Vec<MutexGuard<'_, ()>>, CoreError> {
        let repo_id = &pin.repo_id;
        let revision_id = &pin.revision_id;
        let mut stripes =
            vec![self.generation_mutation_stripe(repo_id, revision_id, pin.manifest_generation)?];
        if let Some(base) = contract.base_generation {
            stripes.push(self.generation_mutation_stripe(repo_id, revision_id, base)?);
        }
        stripes.sort_unstable();
        stripes.dedup();
        stripes
            .into_iter()
            .map(|stripe| {
                self.generation_mutations
                    .get(stripe)
                    .ok_or_else(|| {
                        CoreError::InvalidContract(format!(
                            "semantic mutation stripe {stripe} is out of range"
                        ))
                    })?
                    .lock()
                    .map_err(|error| {
                        CoreError::Storage(format!("semantic mutation lock poisoned: {error}"))
                    })
            })
            .collect()
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
    fn preflight_contract(
        &self,
        pin: &GenerationPin,
        contract: &SemanticGenerationContractV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        observe_result(
            diagnostic,
            DiagnosticStageV1::BuildPreflight,
            |_diagnostic| {
                let _mutation = self.generation_contract_guards(pin, contract)?;
                let _lifecycle = self.directory_lifecycle_read_guard()?;
                build::preflight_contract(
                    &self.state_root,
                    pin,
                    contract,
                    self.lance_read_admission.as_deref(),
                )
            },
        )
    }

    fn build_stream(
        &self,
        header: &SemanticIngestHeaderV1,
        scopes: &mut dyn SemanticScopeSource,
        publication: Option<
            quanta_index_core::publication_admission::PublicationOutputAdmission<'_>,
        >,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<
        (
            SemanticStreamTallyV1,
            quanta_index_contract::IngestStageReport,
        ),
        CoreError,
    > {
        observe_result(diagnostic, DiagnosticStageV1::SemanticBuild, |diagnostic| {
            let _mutation = self.generation_build_guards(header)?;
            let _lifecycle = self.directory_lifecycle_read_guard()?;
            let work_admission = match &self.lance_work_factory {
                Some(factory) => {
                    let held = publication
                        .as_ref()
                        .ok_or_else(|| CoreError::Typed {
                            code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                            message: "qualified semantic build lacks publication admission".into(),
                        })?
                        .held_work_file_allowance()?
                        .ok_or_else(|| CoreError::Typed {
                            code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                            message: "qualified semantic build lacks held work allowance".into(),
                        })?;
                    Some(factory.for_work(held)?)
                }
                None => None,
            };
            build::build_stream_reported_with_diagnostic_and_lance(
                &self.runtime,
                &self.state_root,
                self.window_policy,
                header,
                scopes,
                &build::BuildFileContext {
                    seal_tallies: &self.seal_tallies,
                    lance_file_admission: work_admission.as_ref(),
                },
                publication,
                diagnostic,
            )
        })
    }
}

impl GenerationNamespaceReadPort for SemanticAdapter {
    fn generation_namespace_is_empty(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<bool, CoreError> {
        let _lifecycle = self.directory_lifecycle_read_guard()?;
        for directory in [
            self.state_root.clone(),
            self.state_root
                .join(GenerationStorageKeyV1::for_repo_revision(repo_id, revision_id).as_str()),
        ] {
            match std::fs::symlink_metadata(&directory) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    return Err(CoreError::Storage(format!(
                        "semantic: generation namespace {} is not a directory",
                        directory.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "semantic: inspect generation namespace {}: {error}",
                        directory.display()
                    )));
                }
            }
        }
        let pair_dir = self
            .state_root
            .join(GenerationStorageKeyV1::for_repo_revision(repo_id, revision_id).as_str());
        let mut entries = std::fs::read_dir(&pair_dir).map_err(|error| {
            CoreError::Storage(format!(
                "semantic: read generation namespace {}: {error}",
                pair_dir.display()
            ))
        })?;
        match entries.next() {
            None => Ok(true),
            Some(Ok(_entry)) => Ok(false),
            Some(Err(error)) => Err(CoreError::Storage(format!(
                "semantic: inspect generation namespace entry {}: {error}",
                pair_dir.display()
            ))),
        }
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
        let loaded = open_generation_with_read_admission(
            &self.runtime,
            &self.state_root,
            repo,
            revision,
            generation,
            self.lance_read_admission.as_ref(),
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
    /// inherited dataset's hard-linked fragments count once), observed while
    /// writers continue. Exact admission and retention use the snapshot port.
    fn track_disk_bytes(&self, budget: &RequestBudgetV1) -> Result<u64, CoreError> {
        budget.checkpoint("semantic-disk-usage:entry")?;
        quanta_index_scratch_sqlite::physical::measure_live_inode_tree_bytes(
            &self.state_root,
            &|_name| false,
            self.retention_measurement_policy.max_main_file_bytes(),
            budget,
            budget.diagnostic_context(),
        )
    }
}

/// The dense lanes' tallies as scrape points.
///
/// `semantic_dense_queries_…` and `semantic_budget_interruptions_…`
/// (QI-BB-015, W5 phase 3) count the
/// queries each lane handed to the library and the request interruptions
/// it observed inside, across every searcher this adapter opened.
///
/// `semantic_dense_exact_completions_total` counts the short approximate passes
/// the exact lane answered (QI-BB-025).
///
/// The seal tallies, `semantic_seal_…` (QI-BB-006 #4), count the bytes every seal
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

    fn inventory_sealed_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<bool, CoreError> {
        let dir = layout::generation_dir(
            &self.state_root,
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
        );
        for component in [dir.parent(), Some(dir.as_path())] {
            let component = component.ok_or_else(|| {
                CoreError::InvalidContract("semantic generation has no family directory".into())
            })?;
            match std::fs::symlink_metadata(component) {
                Ok(metadata) if metadata.file_type().is_dir() => {}
                Ok(_) => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "semantic: inspect {} for point inventory: {error}",
                        component.display()
                    )));
                }
            }
        }
        match inventory_generation_dir_with_admission(
            &self.state_root,
            &dir,
            self.lance_read_admission.as_deref(),
        ) {
            Ok(Some(observed)) => Ok(observed.identity() == *candidate),
            Ok(None) | Err(InventoryGenerationError::Quarantined(_)) => Ok(false),
            Err(InventoryGenerationError::Infrastructure(error)) => Err(error),
        }
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
        let sealed_digest = control_file::read_string_bounded_with_admission(
            &marker_path,
            control_file::MAX_SEALED_MARKER_BYTES,
            self.lance_read_admission.as_deref(),
        )
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityIncomplete,
                    message: format!(
                        "semantic: incomplete generation has no sealed marker for generation {}",
                        candidate.manifest_generation.get()
                    ),
                }
            } else {
                crate::errors::fs_err("read sealed marker", &marker_path, &error)
            }
        })?;
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
        open_generation_with_read_admission(
            &self.runtime,
            &self.state_root,
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
            self.lance_read_admission.as_ref(),
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
    admission: Option<&dyn FileAdmission>,
) -> Result<SemanticManifest, CoreError> {
    let generation_dir = layout::generation_dir(
        semantic_root,
        &candidate.repo_id,
        &candidate.revision_id,
        candidate.manifest_generation,
    );
    let marker_path = layout::sealed_marker_path(&generation_dir);
    let sealed_digest = control_file::read_string_bounded_with_admission(
        &marker_path,
        control_file::MAX_SEALED_MARKER_BYTES,
        admission,
    )
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityIncomplete,
                message: format!(
                    "semantic: generation {} has no sealed marker",
                    candidate.manifest_generation.get()
                ),
            }
        } else {
            crate::errors::fs_err("read sealed marker", &marker_path, &error)
        }
    })?;
    if sealed_digest != candidate.manifest_digest {
        return Err(generation_digest_mismatch(candidate, "sealed marker"));
    }
    let manifest_path = layout::manifest_path(&generation_dir);
    let manifest = SemanticManifest::decode(
        &control_file::read_bounded_with_admission(
            &manifest_path,
            control_file::MAX_SCOPE_MANIFEST_BYTES,
            admission,
        )
        .map_err(|error| crate::errors::fs_err("read manifest", &manifest_path, &error))?,
    )?;
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
        let manifest = read_sealed_scope_manifest(
            &self.state_root,
            sealed,
            self.lance_read_admission.as_deref(),
        )?;
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
        pending_batch: Option<&SearchCorpusIngestBatch>,
    ) -> Result<IncompleteGenerationDiscardOutcomeV1, CoreError> {
        if pending_batch.is_some() {
            return Err(CoreError::InvalidContract(
                "semantic incomplete-generation discard cannot preserve a lexical attempt".into(),
            ));
        }
        let _lifecycle = self.directory_lifecycle_guard()?;
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
        if control_file::regular_file_present(&marker_path).map_err(|error| {
            CoreError::Storage(format!(
                "semantic: inspect sealed marker {}: {error}",
                marker_path.display()
            ))
        })? {
            let observed_digest = control_file::read_string_bounded_with_admission(
                &marker_path,
                control_file::MAX_SEALED_MARKER_BYTES,
                self.lance_read_admission.as_deref(),
            )
            .map_err(|error| crate::errors::fs_err("read sealed marker", &marker_path, &error))?;
            if observed_digest != candidate.manifest_digest {
                return Err(generation_digest_mismatch(candidate, "sealed marker"));
            }
            let manifest_path = layout::manifest_path(&generation_dir);
            let manifest = SemanticManifest::decode(
                &control_file::read_bounded_with_admission(
                    &manifest_path,
                    control_file::MAX_SCOPE_MANIFEST_BYTES,
                    self.lance_read_admission.as_deref(),
                )
                .map_err(|error| crate::errors::fs_err("read manifest", &manifest_path, &error))?,
            )?;
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
        if control_file::regular_file_present(&manifest_path).map_err(|error| {
            CoreError::Storage(format!(
                "semantic: inspect incomplete manifest {}: {error}",
                manifest_path.display()
            ))
        })? {
            let manifest = SemanticManifest::decode(
                &control_file::read_bounded_with_admission(
                    &manifest_path,
                    control_file::MAX_SCOPE_MANIFEST_BYTES,
                    self.lance_read_admission.as_deref(),
                )
                .map_err(|error| {
                    crate::errors::fs_err("read incomplete manifest", &manifest_path, &error)
                })?,
            )?;
            manifest.validate_scope(
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            )?;
            if manifest.manifest_digest != candidate.manifest_digest {
                return Err(generation_digest_mismatch(candidate, "incomplete manifest"));
            }
        }
        crate::qualified_fs::reclaim_directory(
            &self.state_root,
            &generation_dir,
            &format!(
                "incomplete-{}",
                GenerationStorageKeyV1::for_repo_revision(
                    &candidate.repo_id,
                    &candidate.revision_id
                )
                .reclaim_entry_name(candidate.manifest_generation),
            ),
            self.retirement_file_factory()?,
        )?;
        Ok(IncompleteGenerationDiscardOutcomeV1::Discarded)
    }
}

impl SealedGenerationReclaimPort for SemanticAdapter {
    fn reclaim_sealed_generation_with_settlement(
        &self,
        retired: &GenerationSnapshot,
        on_absent: &dyn Fn() -> Result<(), CoreError>,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
        observe_result(diagnostic, DiagnosticStageV1::Compaction, |diagnostic| {
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
            match std::fs::symlink_metadata(&generation_dir) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    on_absent()?;
                    return Ok(SealedGenerationReclaimOutcomeV1::Absent);
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "semantic: inspect generation namespace {}: {error}",
                        generation_dir.display()
                    )));
                }
            }
            let marker_path = layout::sealed_marker_path(&generation_dir);
            if !control_file::regular_file_present(&marker_path).map_err(|error| {
                CoreError::Storage(format!(
                    "semantic: inspect sealed marker {}: {error}",
                    marker_path.display()
                ))
            })? {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationNotSealed,
                    message: format!(
                        "semantic: refusing to reclaim unsealed generation {} as retired history",
                        retired.manifest_generation.get()
                    ),
                });
            }
            let observed_digest = control_file::read_string_bounded_with_admission(
                &marker_path,
                control_file::MAX_SEALED_MARKER_BYTES,
                self.lance_read_admission.as_deref(),
            )
            .map_err(|error| crate::errors::fs_err("read sealed marker", &marker_path, &error))?;
            if observed_digest != retired.manifest_digest {
                return Err(generation_digest_mismatch(retired, "sealed marker"));
            }
            let manifest_path = layout::manifest_path(&generation_dir);
            let manifest = SemanticManifest::decode(
                &control_file::read_bounded_with_admission(
                    &manifest_path,
                    control_file::MAX_SCOPE_MANIFEST_BYTES,
                    self.lance_read_admission.as_deref(),
                )
                .map_err(|error| crate::errors::fs_err("read manifest", &manifest_path, &error))?,
            )?;
            manifest.validate_scope(
                &retired.repo_id,
                &retired.revision_id,
                retired.manifest_generation,
            )?;
            if manifest.manifest_digest != retired.manifest_digest {
                return Err(generation_digest_mismatch(retired, "manifest"));
            }
            let bytes = crate::search::dataset_tree_bytes_observed(
                &self.state_root,
                &generation_dir,
                diagnostic,
            )?;
            self.scrub_progress
                .lock()
                .map_err(|error| {
                    CoreError::Storage(format!("semantic scrub progress poisoned: {error}"))
                })?
                .clear_if_invalidated(|paused| paused == retired);
            // Out of the generation namespace by one durable rename, then
            // removed: a crash leaves a reclaim-area entry, never a partial tree
            // that would read as an unsealed build (QI-BB-003).
            let reclaimed = crate::qualified_fs::reclaim_directory(
                &self.state_root,
                &generation_dir,
                &GenerationStorageKeyV1::for_repo_revision(&retired.repo_id, &retired.revision_id)
                    .reclaim_entry_name(retired.manifest_generation),
                self.retirement_file_factory()?,
            );
            match std::fs::symlink_metadata(&generation_dir) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => on_absent()?,
                Ok(_) if reclaimed.is_ok() => {
                    return Err(CoreError::Storage(
                    "semantic reclaim reported removal but generation namespace remains present"
                        .into(),
                ));
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "semantic reclaim failed ({reclaimed:?}); namespace probe failed: {error}"
                    )));
                }
            }
            reclaimed?;
            Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes })
        })
    }

    fn finish_interrupted_reclaims(
        &self,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<FinishedReclaims, CoreError> {
        observe_result(diagnostic, DiagnosticStageV1::Compaction, |diagnostic| {
            let _lifecycle = self.directory_lifecycle_guard()?;
            crate::qualified_fs::finish_interrupted_reclaims(
                &self.state_root,
                diagnostic,
                self.retirement_file_factory()?,
            )
        })
    }

    fn sealed_generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Vec<GenerationSnapshot>, CoreError> {
        observe_result(
            diagnostic,
            DiagnosticStageV1::RetentionRootSnapshot,
            |_diagnostic| {
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
                    if !control_file::regular_file_present(&marker_path).map_err(|error| {
                        CoreError::Storage(format!(
                            "semantic: inspect sealed marker {}: {error}",
                            marker_path.display()
                        ))
                    })? {
                        continue;
                    }
                    let manifest_path = layout::manifest_path(&generation_dir);
                    let manifest = SemanticManifest::decode(
                        &control_file::read_bounded_with_admission(
                            &manifest_path,
                            control_file::MAX_SCOPE_MANIFEST_BYTES,
                            self.lance_read_admission.as_deref(),
                        )
                        .map_err(|error| {
                            crate::errors::fs_err("read manifest", &manifest_path, &error)
                        })?,
                    )?;
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
                    let marker_digest = control_file::read_string_bounded_with_admission(
                        &marker_path,
                        control_file::MAX_SEALED_MARKER_BYTES,
                        self.lance_read_admission.as_deref(),
                    )
                    .map_err(|error| {
                        crate::errors::fs_err("read sealed marker", &marker_path, &error)
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
            },
        )
    }

    /// Bytes the named generation directories occupy together, by unique
    /// inode, dataset and index files included.
    fn begin_pair_measurement(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        budget: quanta_index_core::RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<
        Box<dyn quanta_index_core::domains::generation::SealedGenerationByteSession + '_>,
        CoreError,
    > {
        Ok(Box::new(
            retention_measurement::SemanticRetentionSession::open(
                self,
                repo_id,
                revision_id,
                budget,
                diagnostic,
            )?,
        ))
    }

    fn measure_sealed_generations(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        self.begin_pair_measurement(
            repo_id,
            revision_id,
            self.retention_measurement_policy.physical_scan_budget()?,
            diagnostic,
        )?
        .measure(generations)
    }

    fn begin_track_physical_measurement(
        &self,
    ) -> Result<
        Box<dyn quanta_index_core::domains::generation::TrackPhysicalByteSession + '_>,
        CoreError,
    > {
        Ok(Box::new(
            physical_measurement::SemanticPhysicalSession::begin(self)?,
        ))
    }

    fn measure_track_physical_bytes(
        &self,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<u64, CoreError> {
        let budget = self.retention_measurement_policy.physical_scan_budget()?;
        self.begin_track_physical_measurement()?
            .measure(&budget, diagnostic)
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

enum InventoryGenerationError {
    Quarantined(QuarantinedGenerationV1),
    Infrastructure(CoreError),
}

fn inventory_unreadable_io(
    path: &Path,
    action: &str,
    error: &std::io::Error,
) -> InventoryGenerationError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData
    ) {
        InventoryGenerationError::Quarantined(quarantine(
            path.to_path_buf(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
            format!("{action}: {error}"),
        ))
    } else {
        InventoryGenerationError::Infrastructure(crate::errors::fs_err(action, path, error))
    }
}

fn inventory_unreadable_error(path: &Path, error: CoreError) -> InventoryGenerationError {
    if matches!(
        &error,
        CoreError::Storage(_) | CoreError::StorageIo { .. } | CoreError::InvalidContract(_)
    ) || matches!(&error, CoreError::Typed { code, .. } if *code == quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE)
    {
        InventoryGenerationError::Infrastructure(error)
    } else {
        InventoryGenerationError::Quarantined(quarantine(
            path.to_path_buf(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
            error.to_string(),
        ))
    }
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
/// This opens the generation and verifies its sealed sidecars and dataset
/// layout, schema, row count, vector index and membership row count against
/// the manifest. The recorded row root is not recomputed by this cheap door;
/// callers use the semantic row commitment or the dataset scrub when they
/// need payload integrity. Callers run this for the generations they need.
pub fn validate_persisted_generation_v2(
    semantic_root: &Path,
    record: &PersistedSemanticGeneration,
) -> Result<ValidatedPersistedSemanticGenerationV2, CoreError> {
    validate_persisted_generation_with_read_admission(semantic_root, record, None)
}

/// Qualified boot/standalone validation: both Lance objects and directly
/// opened sidecars use the same read-only canonical file binder.
pub fn validate_persisted_generation_v2_admitted(
    semantic_root: &Path,
    record: &PersistedSemanticGeneration,
    read_admission: &Arc<dyn FileAdmission>,
) -> Result<ValidatedPersistedSemanticGenerationV2, CoreError> {
    validate_persisted_generation_with_read_admission(semantic_root, record, Some(read_admission))
}

fn validate_persisted_generation_with_read_admission(
    semantic_root: &Path,
    record: &PersistedSemanticGeneration,
    read_admission: Option<&Arc<dyn FileAdmission>>,
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
    let _loaded = open_generation_with_read_admission(
        &runtime,
        semantic_root,
        &record.repo_id,
        &record.revision_id,
        record.generation,
        read_admission,
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
/// carries a SEALED marker, a scope-consistent manifest, and a current-format
/// sealed manifest bound to the same digest. This reads three bounded control
/// files per generation and opens no dataset. The open door checks layout,
/// schema, row count, index identity and membership count; it does not
/// recompute the semantic row root or hash dataset payload bytes. An
/// invalid identity is quarantined with its path and reason; in-progress
/// generations are skipped. I/O failures propagate rather than masquerading
/// as proven corruption or a successful inventory.
pub fn inventory_persisted_generations(
    semantic_root: &Path,
) -> Result<SemanticGenerationInventoryV1, CoreError> {
    inventory_persisted_generations_with_admission(semantic_root, None)
}

pub fn inventory_persisted_generations_with_admission(
    semantic_root: &Path,
    admission: Option<&dyn FileAdmission>,
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
            match inventory_generation_dir_with_admission(
                semantic_root,
                &generation_entry.path(),
                admission,
            ) {
                Ok(Some(record)) => inventory.sealed.push(record),
                Ok(None) => {}
                Err(InventoryGenerationError::Quarantined(quarantined)) => {
                    inventory.quarantined.push(quarantined);
                }
                Err(InventoryGenerationError::Infrastructure(error)) => return Err(error),
            }
        }
    }
    Ok(inventory)
}

/// Inventory one semantic generation directory.
///
/// `Ok(Some)` is a sealed manifest that owns the directory and agrees with its
/// marker; `Ok(None)` is an in-progress build. `Err` distinguishes quarantine
/// from infrastructure failure.
#[cfg(test)]
fn inventory_generation_dir(
    semantic_root: &Path,
    generation_dir: &Path,
) -> Result<Option<PersistedSemanticGeneration>, InventoryGenerationError> {
    inventory_generation_dir_with_admission(semantic_root, generation_dir, None)
}

fn inventory_generation_dir_with_admission(
    semantic_root: &Path,
    generation_dir: &Path,
    admission: Option<&dyn FileAdmission>,
) -> Result<Option<PersistedSemanticGeneration>, InventoryGenerationError> {
    let candidate =
        inventory_generation_dir_before_sealed_manifest(semantic_root, generation_dir, admission)?;
    let Some(record) = candidate.as_ref() else {
        return Ok(None);
    };
    if let Err(error) = inspect_sealed_manifest_identity_with_admission(
        generation_dir,
        &record.manifest_digest,
        admission,
    ) {
        let reason = match &error {
            CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestMissing
                    | quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                ..
            } => GenerationQuarantineReasonV1::FormatUnsupported,
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            } => GenerationQuarantineReasonV1::ContentCorrupt,
            CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
                ..
            } => GenerationQuarantineReasonV1::IdentityDigestMismatch,
            CoreError::Storage(_) | CoreError::StorageIo { .. } => {
                return Err(InventoryGenerationError::Infrastructure(error));
            }
            CoreError::InvalidContract(_)
            | CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_) => GenerationQuarantineReasonV1::IdentityUnreadable,
        };
        return Err(InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            reason,
            format!("sealed manifest admission: {error}"),
        )));
    }
    Ok(candidate)
}

pub(crate) fn scrub_candidate_from_quarantined_with_admission(
    semantic_root: &Path,
    entry: &QuarantinedGenerationV1,
    admission: Option<&dyn FileAdmission>,
) -> Result<Option<PersistedSemanticGeneration>, CoreError> {
    if entry.track != SearchPlaneTrackKind::Semantic {
        return Ok(None);
    }
    match inventory_generation_dir_before_sealed_manifest(semantic_root, &entry.path, admission) {
        Ok(record) => Ok(record),
        Err(InventoryGenerationError::Quarantined(_)) => Ok(None),
        Err(InventoryGenerationError::Infrastructure(error)) => Err(error),
    }
}

fn inventory_generation_dir_before_sealed_manifest(
    semantic_root: &Path,
    generation_dir: &Path,
    admission: Option<&dyn FileAdmission>,
) -> Result<Option<PersistedSemanticGeneration>, InventoryGenerationError> {
    let Some(generation_name) = generation_dir.file_name().and_then(|name| name.to_str()) else {
        return Err(InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory name is not UTF-8".to_string(),
        )));
    };
    if GenerationStorageKeyV1::generation_of_dir_name(generation_name).is_none() {
        return Err(InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory is not `g<N>`; it needs explicit migration".to_string(),
        )));
    }
    let unreadable = |detail: String| {
        quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
            detail,
        )
    };
    let marker_path = layout::sealed_marker_path(generation_dir);
    match control_file::regular_file_present(&marker_path) {
        Ok(true) => {}
        Ok(false) => return Ok(None),
        Err(error) => {
            return Err(inventory_unreadable_io(
                generation_dir,
                "inspect sealed marker",
                &error,
            ));
        }
    }
    // A receipt the integrity scrub left (QI-BB-017): the generation's
    // bytes were proven not to match its seal, so it is set aside under
    // that proof rather than seeded and refused at every door.
    match read_quarantine_receipt_with_admission(generation_dir, admission) {
        Ok(Some(receipt)) => {
            return Err(InventoryGenerationError::Quarantined(quarantine(
                generation_dir.to_path_buf(),
                GenerationQuarantineReasonV1::ContentCorrupt,
                receipt.detail,
            )));
        }
        Ok(None) => {}
        Err(error) => return Err(inventory_unreadable_error(generation_dir, error)),
    }
    let manifest_path = layout::manifest_path(generation_dir);
    let manifest_bytes = control_file::read_bounded_with_admission(
        &manifest_path,
        control_file::MAX_SCOPE_MANIFEST_BYTES,
        admission,
    )
    .map_err(|error| inventory_unreadable_io(generation_dir, "read manifest", &error))?;
    let manifest = SemanticManifest::decode(&manifest_bytes).map_err(|err| {
        if let CoreError::Typed { code, message } = &err
            && *code == FORMAT_UNSUPPORTED_CODE
        {
            InventoryGenerationError::Quarantined(quarantine(
                generation_dir.to_path_buf(),
                GenerationQuarantineReasonV1::FormatUnsupported,
                message.clone(),
            ))
        } else {
            InventoryGenerationError::Quarantined(unreadable(format!("decode manifest: {err}")))
        }
    })?;
    let repo_id = RepoId::new(manifest.repo_id.clone()).map_err(|error| {
        InventoryGenerationError::Quarantined(unreadable(format!(
            "semantic manifest has invalid repo ID: {error}"
        )))
    })?;
    let revision_id = RevisionId::new(manifest.revision_id.clone()).map_err(|error| {
        InventoryGenerationError::Quarantined(unreadable(format!(
            "semantic manifest has invalid revision ID: {error}"
        )))
    })?;
    let generation = ManifestGeneration::new(manifest.generation);
    if let Err(err) = manifest.validate_scope(&repo_id, &revision_id, generation) {
        return Err(InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::ScopeMismatch,
            err.to_string(),
        )));
    }
    if GenerationStorageKeyV1::for_repo_revision(&repo_id, &revision_id)
        .generation_dir(semantic_root, generation)
        != generation_dir
    {
        return Err(InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::ScopeMismatch,
            format!(
                "manifest names repo={} revision={} generation={}, which does not own this directory",
                repo_id.as_str(),
                revision_id.as_str(),
                generation.get()
            ),
        )));
    }
    let sealed_digest = control_file::read_string_bounded_with_admission(
        &marker_path,
        control_file::MAX_SEALED_MARKER_BYTES,
        admission,
    )
    .map_err(|error| inventory_unreadable_io(generation_dir, "read sealed marker", &error))?;
    if sealed_digest != manifest.manifest_digest {
        return Err(InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::IdentityDigestMismatch,
            "sealed marker and manifest disagree on the manifest digest".to_string(),
        )));
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
    fn discard_quarantined_generation_with_settlement(
        &self,
        entry: &QuarantinedGenerationV1,
        on_absent: &dyn Fn() -> Result<(), CoreError>,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        if entry.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(format!(
                "semantic quarantine discard received {:?} track",
                entry.track
            )));
        }
        let _lifecycle = self.directory_lifecycle_guard()?;
        let outcome = discard_quarantined_directory(
            &self.state_root,
            &inventory_persisted_generations_with_admission(
                &self.state_root,
                self.lance_read_admission.as_deref(),
            )?
            .quarantined,
            entry,
            self.retirement_file_factory()?,
        );
        match std::fs::symlink_metadata(&entry.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => on_absent()?,
            Ok(_) if outcome.is_ok() => {
                return Err(CoreError::Storage(
                    "semantic quarantine discard reported removal but path remains present".into(),
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "semantic quarantine discard failed ({outcome:?}); namespace probe failed: {error}"
                )));
            }
        }
        let outcome = outcome?;
        let mut progress = self.scrub_progress.lock().map_err(|error| {
            CoreError::Storage(format!("semantic scrub progress poisoned: {error}"))
        })?;
        progress.clear_if_invalidated(|paused| {
            layout::generation_dir(
                &self.state_root,
                &paused.repo_id,
                &paused.revision_id,
                paused.manifest_generation,
            )
            .starts_with(&entry.path)
        });
        drop(progress);
        Ok(outcome)
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
    retirement_factory: Option<&dyn SemanticMaintenanceFileFactory>,
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
    let bytes = crate::search::dataset_tree_bytes(semantic_root, &entry.path)?;
    crate::qualified_fs::reclaim_quarantined_directory(
        semantic_root,
        &entry.path,
        retirement_factory,
    )?;
    Ok(QuarantineDiscardOutcomeV1::Discarded { bytes })
}

impl SealedGenerationScanPort for SemanticAdapter {
    fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
        let inventory = inventory_persisted_generations_with_admission(
            &self.state_root,
            self.lance_read_admission.as_deref(),
        )?;
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
mod namespace_read_tests {
    use super::*;

    #[test]
    fn absent_or_empty_pair_is_empty_but_any_entry_is_occupied() {
        let temp = tempfile::tempdir().expect("state root fixture");
        let state_root = temp.path().join("semantic");
        let adapter = SemanticAdapter::with_state_root(state_root.clone()).expect("adapter");
        let repo = RepoId::new("repo").expect("static repo ID");
        let revision = RevisionId::new("revision").expect("static revision ID");
        assert!(
            adapter
                .generation_namespace_is_empty(&repo, &revision)
                .expect("absent root")
        );
        assert!(!state_root.exists(), "probe must not create the root");

        let pair =
            state_root.join(GenerationStorageKeyV1::for_repo_revision(&repo, &revision).as_str());
        std::fs::create_dir_all(&pair).expect("create empty pair");
        assert!(
            adapter
                .generation_namespace_is_empty(&repo, &revision)
                .expect("empty pair")
        );
        std::fs::create_dir(pair.join("g1")).expect("create incomplete generation");
        assert!(
            !adapter
                .generation_namespace_is_empty(&repo, &revision)
                .expect("incomplete")
        );
        std::fs::remove_dir(pair.join("g1")).expect("remove incomplete generation");
        std::fs::write(pair.join("unknown-entry"), b"unknown").expect("create unknown entry");
        assert!(
            !adapter
                .generation_namespace_is_empty(&repo, &revision)
                .expect("unknown")
        );
        std::fs::remove_file(pair.join("unknown-entry")).expect("remove unknown entry");
        std::fs::create_dir(pair.join("g2")).expect("create sealed generation dir");
        std::fs::write(
            pair.join("g2").join(layout::MARKER_SEALED_FILE_NAME),
            b"digest",
        )
        .expect("create sealed marker");
        assert!(
            !adapter
                .generation_namespace_is_empty(&repo, &revision)
                .expect("sealed")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_pair_is_not_accepted_as_empty() {
        let temp = tempfile::tempdir().expect("state root fixture");
        let state_root = temp.path().join("semantic");
        std::fs::create_dir(&state_root).expect("create root");
        let adapter = SemanticAdapter::with_state_root(state_root.clone()).expect("adapter");
        let repo = RepoId::new("repo").expect("static repo ID");
        let revision = RevisionId::new("revision").expect("static revision ID");
        let pair =
            state_root.join(GenerationStorageKeyV1::for_repo_revision(&repo, &revision).as_str());
        std::os::unix::fs::symlink(temp.path(), pair).expect("create pair symlink");
        assert!(
            adapter
                .generation_namespace_is_empty(&repo, &revision)
                .is_err()
        );
    }
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
            policy_digest: "policy:fixture".to_string(),
            view_policy_digest: None,
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
    fn inventory_quarantines_pre_uniqueness_semantic_manifest() {
        let temp = tempfile::tempdir().expect("fixture state root");
        let sealed = candidate(61, "digest-pre-uniqueness");
        let generation_dir = layout::generation_dir(
            temp.path(),
            &sealed.repo_id,
            &sealed.revision_id,
            sealed.manifest_generation,
        );
        std::fs::create_dir_all(&generation_dir).expect("generation directory");
        std::fs::write(
            layout::sealed_marker_path(&generation_dir),
            sealed.manifest_digest.as_bytes(),
        )
        .expect("sealed marker");
        let mut old = manifest_for(&sealed);
        old.format_version = 11;
        std::fs::write(
            layout::manifest_path(&generation_dir),
            old.encode().expect("old manifest bytes"),
        )
        .expect("old manifest");
        assert!(matches!(
            inventory_generation_dir(temp.path(), &generation_dir),
            Err(InventoryGenerationError::Quarantined(entry))
                if entry.reason == GenerationQuarantineReasonV1::FormatUnsupported
                    && entry.detail.contains("rebuild")
        ));
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
                .discard_incomplete_generation(&candidate, None)
                .expect("discard incomplete"),
            IncompleteGenerationDiscardOutcomeV1::Discarded
        );
        assert!(!generation_dir.exists());
        assert!(generation_dir.starts_with(temp.path()));
        assert_eq!(
            adapter
                .discard_incomplete_generation(&candidate, None)
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
            .discard_incomplete_generation(&sealed, None)
            .expect_err("sealed exact must be immutable");
        assert!(matches!(
            exact_error,
            CoreError::Typed { ref code, .. } if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable
        ));
        let mut conflict = sealed;
        conflict.manifest_digest = "digest-b".to_string();
        let conflict_error = adapter
            .discard_incomplete_generation(&conflict, None)
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
            .discard_incomplete_generation(&candidate, None)
            .expect_err("manifest conflict must fail closed");
        assert!(matches!(
            error,
            CoreError::Typed { ref code, .. }
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch
        ));
        assert!(generation_dir.exists());
    }

    #[test]
    fn discard_refuses_a_dangling_incomplete_manifest_symlink() {
        let temp = tempfile::tempdir().expect("tempdir");
        let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf()).expect("adapter");
        let candidate = candidate(11, "digest-a");
        let generation_dir = layout::generation_dir(
            temp.path(),
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
        );
        std::fs::create_dir_all(&generation_dir).expect("create incomplete generation");
        let manifest = layout::manifest_path(&generation_dir);
        std::os::unix::fs::symlink(generation_dir.join("missing-manifest"), &manifest)
            .expect("create dangling manifest symlink");

        assert!(
            adapter
                .discard_incomplete_generation(&candidate, None)
                .is_err()
        );
        assert!(generation_dir.is_dir());
        assert!(
            std::fs::symlink_metadata(&manifest)
                .expect("preserve dangling manifest")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn dangling_sealed_marker_is_quarantined_and_cannot_be_discarded_as_incomplete() {
        let temp = tempfile::tempdir().expect("tempdir");
        let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf()).expect("adapter");
        let candidate = candidate(10, "digest-a");
        let generation_dir = layout::generation_dir(
            temp.path(),
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
        );
        std::fs::create_dir_all(&generation_dir).expect("create generation");
        std::fs::write(
            layout::manifest_path(&generation_dir),
            manifest_for(&candidate).encode().expect("encode manifest"),
        )
        .expect("write manifest");
        std::os::unix::fs::symlink(
            generation_dir.join("missing-marker-target"),
            layout::sealed_marker_path(&generation_dir),
        )
        .expect("make dangling sealed marker");

        let inventory = inventory_persisted_generations(temp.path()).expect("inventory");
        assert!(inventory.sealed.is_empty());
        let [entry] = inventory.quarantined.as_slice() else {
            panic!("dangling marker must have exactly one quarantine entry: {inventory:?}");
        };
        assert_eq!(entry.path, generation_dir);
        assert_eq!(
            entry.reason,
            GenerationQuarantineReasonV1::IdentityUnreadable
        );
        assert!(
            adapter
                .discard_incomplete_generation(&candidate, None)
                .is_err()
        );
        assert!(
            generation_dir.exists(),
            "incomplete discard removed a sealed marker"
        );
        assert!(matches!(
            adapter.discard_quarantined_generation(entry),
            Ok(QuarantineDiscardOutcomeV1::Discarded { .. })
        ));
        assert!(!generation_dir.exists());
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
