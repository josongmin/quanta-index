//! `DirectSearchCorpusMaterializer`: the lexical + semantic search-corpus
//! ingest path, from resource preflight through sealed-generation finalize
//! and retired-generation reclaim.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::{
    BatchPublishReceipt, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchCorpusIngestBatch, SearchPlaneTrackKind,
};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, AuxiliaryGenerationKeyV1, AuxiliaryRowMutationV1, CoreError,
    GenerationIdentityValidatePort, IdempotencyCatalogPort, IncompleteGenerationDiscardPort,
    IngestBatchFootprint, IngestResourcePolicy, MetricPointV1, MetricSourcePort,
    SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort, SearchCorpusBatchBuildPort,
    SearchCorpusIngestPort, SemanticContentRootsPort, SemanticEgressPolicyV1, SemanticIngestPort,
    SemanticScopeSource as _, SemanticStreamWindowPolicy, TextEmbeddingProvider, count_from_usize,
};

use crate::auxiliary_authority::{structural_chunks_delta_rows, structural_chunks_transition};
use crate::crash_point;
use crate::history_text::HistoryTextIndexParts;
use crate::ingest_dispatcher::auxiliary::AuxiliaryMutationCoordinator;
use crate::ingest_dispatcher::generation_plan::{
    DeferredGcStep, SealedGenerationBuildPlanV1, SearchCorpusPhysicalReclaimReceiptV1,
    batch_publish_receipt_v1, ensure_generation_is_mutable_v1, generation_pair_from_batch_v1,
    inspect_physical_generation_v1, validate_delta_base_v1, validate_physical_generation_v1,
    validate_semantic_publish_receipt_v1,
};
use crate::ingest_dispatcher::ports::SearchCorpusAuthorityWritePort;
use crate::post_durable::defer_storage_failure;
use crate::readiness::{
    SEARCH_CORPUS_LOCK_STRIPES_V1, SearchCorpusHistoryRetentionReceiptV1,
    search_corpus_lock_stripe_v1,
};
use crate::semantic_derive::derive_semantic_stream_from_semantic_sources_v1;
use crate::{
    Ledger, SealedSearchCorpusAuthorityStateV1, SnapshotKey, SnapshotRegistries,
    SnapshotRetireOutcome,
};

/// Direct search-corpus batch materializer that commits lexical and derived
/// semantic generations, then durably admits the complete sealed identity into
/// rollback history.
pub struct DirectSearchCorpusMaterializer {
    builder: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
    semantic_ingest: Arc<dyn SemanticIngestPort + Send + Sync>,
    semantic_embedder: Arc<dyn TextEmbeddingProvider + Send + Sync>,
    authority: Arc<dyn SearchCorpusAuthorityWritePort + Send + Sync>,
    lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    /// The content roots a sealed semantic generation carries, attested on
    /// every sealed receipt (QI-BB-028).
    semantic_content_roots: Arc<dyn SemanticContentRootsPort + Send + Sync>,
    lexical_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    semantic_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    /// Physical GC of retired sealed generations (QI-BB-003): one reclaim
    /// port per track plus the registries that must release their handles
    /// before any bytes go.
    lexical_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    semantic_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    snapshots: SnapshotRegistries,
    idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
    source_publication: Arc<dyn quanta_index_core::SourcePublicationCatalogPort>,
    /// The envelope every batch is measured against before anything is
    /// held (QI-BB-021), and what the measured batches added up to.
    resource_policy: IngestResourcePolicy,
    resource_stats: Mutex<IngestResourceStats>,
    /// What physical GC reclaimed and deferred over the process lifetime
    /// (QI-BB-003): the receipt of every reclaim pass, kept.
    gc_stats: Mutex<SearchCorpusGcStats>,
    /// The window the derived semantic source embeds and issues in, so at
    /// most one window of vectors is resident during a build (QI-BB-021).
    semantic_stream_policy: SemanticStreamWindowPolicy,
    /// Source-content egress composition (S21-08): `Some` gates every
    /// derived batch on the external grant before the first window is
    /// embedded; `None` is a local (hash) composition with no egress.
    source_egress_policy: Option<SemanticEgressPolicyV1>,
    auxiliary_catalog: Arc<dyn AuxiliaryAuthorityCatalogPort + Send + Sync>,
    auxiliary_coordinator: Arc<AuxiliaryMutationCoordinator>,
    operation_locks: [Mutex<()>; SEARCH_CORPUS_LOCK_STRIPES_V1],
    /// The history text index whose epochs go with a forgotten auxiliary
    /// generation (QI-BB-023 follow-up #1); a plane composed without one
    /// has none to reclaim.
    history_text: Option<HistoryTextIndexParts>,
}

/// What the resource envelope admitted and refused, and the widest batch
/// it admitted (QI-BB-021).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IngestResourceStats {
    /// Batches that fit the envelope.
    pub admitted: u64,
    /// Batches refused typed for exceeding it.
    pub refused: u64,
    /// Most embedded records one admitted batch carried.
    pub peak_embedded_records: usize,
    /// Most bytes of text one admitted batch asked to embed.
    pub peak_text_bytes: u64,
    /// Most bytes of vectors one admitted batch expanded into.
    pub peak_vector_bytes: u64,
}

/// What physical GC of retired sealed generations did, per track, over
/// the process lifetime (QI-BB-003).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchCorpusGcStats {
    /// Bytes given back by reclaiming lexical generation directories.
    pub lexical_reclaimed_bytes: u64,
    /// Lexical generation directories reclaimed.
    pub lexical_reclaimed_generations: u64,
    /// Bytes given back by reclaiming semantic generation directories.
    pub semantic_reclaimed_bytes: u64,
    /// Semantic generation directories reclaimed.
    pub semantic_reclaimed_generations: u64,
    /// Retired generations left on disk for a later pass because a
    /// resident handle still had holders.
    pub deferred_pinned: u64,
    /// Reclaim-pass steps that failed after the seal they follow was
    /// durable; each is found again by the next pass (QI-BB-020).
    pub failures: u64,
    /// Reclaims a crash or a failed removal had interrupted, finished from
    /// a track's reclaim area; their bytes and generations are counted
    /// with the track's reclaimed ones (QI-BB-003).
    pub interrupted_reclaims_finished: u64,
    /// The index bytes the retained generations of every pair this
    /// process sealed occupy on disk, as retention last measured each
    /// pair: the number `max_bytes` is enforced against.
    pub retained_index_bytes: BTreeMap<(RepoId, RevisionId), u64>,
}

impl SearchCorpusGcStats {
    fn record_retention(&mut self, retention: &SearchCorpusHistoryRetentionReceiptV1) {
        let _previous = self.retained_index_bytes.insert(
            (retention.repo_id().clone(), retention.revision_id().clone()),
            retention.retained_index_bytes(),
        );
    }

    fn record(&mut self, receipt: &SearchCorpusPhysicalReclaimReceiptV1) {
        for ((track, _generation), bytes) in &receipt.reclaimed {
            self.count_reclaimed(*track, 1, *bytes);
        }
        for (track, finished) in &receipt.finished_interrupted {
            self.count_reclaimed(*track, finished.entries, finished.bytes);
            self.interrupted_reclaims_finished = self
                .interrupted_reclaims_finished
                .saturating_add(finished.entries);
        }
        self.deferred_pinned = self
            .deferred_pinned
            .saturating_add(count_from_usize(receipt.deferred_pinned.len()));
        self.failures = self
            .failures
            .saturating_add(count_from_usize(receipt.deferred.len()));
    }

    /// Count `generations` directories and their `bytes` as reclaimed on
    /// `track`.
    fn count_reclaimed(&mut self, track: SearchPlaneTrackKind, generations: u64, bytes: u64) {
        let (counted_bytes, counted_generations) = match track {
            SearchPlaneTrackKind::Lexical => (
                &mut self.lexical_reclaimed_bytes,
                &mut self.lexical_reclaimed_generations,
            ),
            SearchPlaneTrackKind::Semantic => (
                &mut self.semantic_reclaimed_bytes,
                &mut self.semantic_reclaimed_generations,
            ),
            SearchPlaneTrackKind::Structural => return,
        };
        *counted_bytes = counted_bytes.saturating_add(bytes);
        *counted_generations = counted_generations.saturating_add(generations);
    }

    /// Bytes reclaimed on both tracks together.
    #[must_use]
    pub fn reclaimed_bytes(&self) -> u64 {
        self.lexical_reclaimed_bytes
            .saturating_add(self.semantic_reclaimed_bytes)
    }

    /// Generation directories reclaimed on both tracks together.
    #[must_use]
    pub fn reclaimed_generations(&self) -> u64 {
        self.lexical_reclaimed_generations
            .saturating_add(self.semantic_reclaimed_generations)
    }

    /// The retained index bytes of every measured pair, together.
    #[must_use]
    pub fn retained_index_bytes_total(&self) -> u64 {
        self.retained_index_bytes
            .values()
            .fold(0_u64, |total, bytes| total.saturating_add(*bytes))
    }
}

impl IngestResourceStats {
    fn record_admitted(&mut self, footprint: &IngestBatchFootprint) {
        self.admitted = self.admitted.saturating_add(1);
        self.peak_embedded_records = self.peak_embedded_records.max(footprint.embedded_records);
        self.peak_text_bytes = self.peak_text_bytes.max(footprint.text_bytes);
        self.peak_vector_bytes = self.peak_vector_bytes.max(footprint.vector_bytes);
    }

    fn record_refused(&mut self) {
        self.refused = self.refused.saturating_add(1);
    }
}

/// Composition-owned ports required to materialize one search-corpus generation.
pub struct SearchCorpusMaterializerParts {
    pub builder: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    pub ledger: Arc<RwLock<Ledger>>,
    pub semantic_ingest: Arc<dyn SemanticIngestPort + Send + Sync>,
    pub semantic_embedder: Arc<dyn TextEmbeddingProvider + Send + Sync>,
    pub authority: Arc<dyn SearchCorpusAuthorityWritePort + Send + Sync>,
    pub lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    pub semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    /// Reads the content roots a sealed semantic generation carries, so a
    /// sealed receipt attests them (QI-BB-028).
    pub semantic_content_roots: Arc<dyn SemanticContentRootsPort + Send + Sync>,
    pub lexical_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    pub semantic_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    pub lexical_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    pub semantic_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    pub snapshots: SnapshotRegistries,
    /// Idempotency records are forgotten with the generation they describe
    /// (QI-BB-032 retention).
    pub idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
    pub source_publication: Arc<dyn quanta_index_core::SourcePublicationCatalogPort>,
    /// The resource envelope one batch may ask the plane to hold (QI-BB-021).
    pub resource_policy: IngestResourcePolicy,
    /// The window the semantic source embeds and issues in (QI-BB-021); the
    /// semantic build admits every window against the same policy.
    pub semantic_stream_policy: SemanticStreamWindowPolicy,
    /// Source-content egress composition (S21-08): `Some` gates every
    /// derived batch on the external grant; `None` is local-only.
    pub source_egress_policy: Option<SemanticEgressPolicyV1>,
    /// The structural chunk universe of every generation is durable in the
    /// auxiliary catalog before the generation is finalized, and auxiliary
    /// generations are forgotten with retention (QI-BB-020).
    pub auxiliary_catalog: Arc<dyn AuxiliaryAuthorityCatalogPort + Send + Sync>,
    pub auxiliary_coordinator: Arc<AuxiliaryMutationCoordinator>,
}

impl DirectSearchCorpusMaterializer {
    #[must_use]
    pub fn new_with_search_owned_semantics(parts: SearchCorpusMaterializerParts) -> Self {
        let SearchCorpusMaterializerParts {
            builder,
            ledger,
            semantic_ingest,
            semantic_embedder,
            authority,
            lexical_generation_validator,
            semantic_generation_validator,
            semantic_content_roots,
            lexical_incomplete_discard,
            semantic_incomplete_discard,
            lexical_reclaim,
            semantic_reclaim,
            snapshots,
            idempotency,
            source_publication,
            resource_policy,
            semantic_stream_policy,
            source_egress_policy,
            auxiliary_catalog,
            auxiliary_coordinator,
        } = parts;
        Self {
            builder,
            ledger,
            semantic_ingest,
            semantic_embedder,
            authority,
            lexical_generation_validator,
            semantic_generation_validator,
            semantic_content_roots,
            lexical_incomplete_discard,
            semantic_incomplete_discard,
            lexical_reclaim,
            semantic_reclaim,
            snapshots,
            idempotency,
            source_publication,
            resource_policy,
            resource_stats: Mutex::new(IngestResourceStats::default()),
            gc_stats: Mutex::new(SearchCorpusGcStats::default()),
            semantic_stream_policy,
            source_egress_policy,
            auxiliary_catalog,
            auxiliary_coordinator,
            operation_locks: std::array::from_fn(|_index| Mutex::new(())),
            history_text: None,
        }
    }

    /// Wire the history text index whose epochs retention reclaims with
    /// the auxiliary generations it forgets.
    #[must_use]
    pub fn with_history_text(mut self, history_text: HistoryTextIndexParts) -> Self {
        self.history_text = Some(history_text);
        self
    }

    /// What the resource envelope has admitted and refused so far.
    pub fn resource_stats(&self) -> Result<IngestResourceStats, CoreError> {
        self.resource_stats
            .lock()
            .map(|stats| *stats)
            .map_err(|err| {
                CoreError::Storage(format!(
                    "direct search-corpus materialize: resource stats poisoned: {err}"
                ))
            })
    }

    /// What physical GC has reclaimed and deferred so far, and what the
    /// measured pairs retain.
    pub fn gc_stats(&self) -> Result<SearchCorpusGcStats, CoreError> {
        self.gc_stats
            .lock()
            .map(|stats| stats.clone())
            .map_err(|err| {
                CoreError::Storage(format!(
                    "direct search-corpus materialize: gc stats poisoned: {err}"
                ))
            })
    }

    fn record_reclaim_receipt(
        &self,
        retention: &SearchCorpusHistoryRetentionReceiptV1,
        receipt: &SearchCorpusPhysicalReclaimReceiptV1,
    ) -> Result<(), CoreError> {
        let mut stats = self.gc_stats.lock().map_err(|err| {
            CoreError::Storage(format!(
                "direct search-corpus materialize: gc stats poisoned: {err}"
            ))
        })?;
        stats.record_retention(retention);
        stats.record(receipt);
        drop(stats);
        Ok(())
    }
}

/// The resource envelope's tallies as scrape points, `ingest_…`, and
/// physical GC's, `search_corpus_gc_…` (QI-BB-015).
impl MetricSourcePort for DirectSearchCorpusMaterializer {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let stats = self.resource_stats()?;
        let gc = self.gc_stats()?;
        Ok(vec![
            MetricPointV1::counter("ingest_batches_admitted_total", stats.admitted),
            MetricPointV1::counter("ingest_batches_refused_total", stats.refused),
            MetricPointV1::gauge_count(
                "ingest_peak_embedded_records",
                count_from_usize(stats.peak_embedded_records),
            ),
            MetricPointV1::gauge_count("ingest_peak_text_bytes", stats.peak_text_bytes),
            MetricPointV1::gauge_count("ingest_peak_vector_bytes", stats.peak_vector_bytes),
            MetricPointV1::counter(
                "search_corpus_gc_reclaimed_bytes_total",
                gc.reclaimed_bytes(),
            ),
            MetricPointV1::counter(
                "search_corpus_gc_reclaimed_generations_total",
                gc.reclaimed_generations(),
            ),
            MetricPointV1::counter("search_corpus_gc_deferred_pinned_total", gc.deferred_pinned),
            MetricPointV1::counter("search_corpus_gc_failures_total", gc.failures),
            MetricPointV1::counter(
                "search_corpus_gc_interrupted_reclaims_finished_total",
                gc.interrupted_reclaims_finished,
            ),
            MetricPointV1::counter(
                "search_corpus_gc_lexical_reclaimed_bytes_total",
                gc.lexical_reclaimed_bytes,
            ),
            MetricPointV1::counter(
                "search_corpus_gc_lexical_reclaimed_generations_total",
                gc.lexical_reclaimed_generations,
            ),
            MetricPointV1::counter(
                "search_corpus_gc_semantic_reclaimed_bytes_total",
                gc.semantic_reclaimed_bytes,
            ),
            MetricPointV1::counter(
                "search_corpus_gc_semantic_reclaimed_generations_total",
                gc.semantic_reclaimed_generations,
            ),
            MetricPointV1::gauge_count(
                "search_corpus_retained_index_bytes",
                gc.retained_index_bytes_total(),
            ),
        ])
    }
}

impl DirectSearchCorpusMaterializer {
    /// The batch's own shape and surface-mutation authority (QI-BB-029):
    /// storage-free, refused typed.
    fn validate_batch_shape_v1(batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        batch.validate_v1().map_err(|err| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusBatchShapeInvalid,
            message: format!("direct search-corpus materialize: {err}"),
        })?;
        batch.validate_surface_mutations_v1().map_err(|err| {
            CoreError::InvalidContract(format!("direct search-corpus materialize: {err}"))
        })
    }

    /// Measure `batch` against the envelope (QI-BB-021) without touching the
    /// tallies: the pure check `publish_batch` repeats under its lock.
    fn measure_resource_envelope(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        self.resource_policy
            .admit_search_corpus_batch(batch, self.semantic_embedder.dimension())
            .map(|_footprint| ())
    }

    /// Measure `batch` against the envelope and tally the outcome
    /// (QI-BB-021). This is the admission point: it runs once per batch,
    /// in `preflight_batch`, before anything is held; a batch that does not
    /// fit is refused typed here with zero bytes changed and no record.
    fn admit_resource_envelope(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        let outcome = self
            .resource_policy
            .admit_search_corpus_batch(batch, self.semantic_embedder.dimension());
        {
            let mut stats = self.resource_stats.lock().map_err(|err| {
                CoreError::Storage(format!(
                    "direct search-corpus materialize: resource stats poisoned: {err}"
                ))
            })?;
            match &outcome {
                Ok(footprint) => stats.record_admitted(footprint),
                Err(_refused) => stats.record_refused(),
            }
        }
        outcome.map(|_footprint| ())
    }
}

impl SearchCorpusIngestPort for DirectSearchCorpusMaterializer {
    /// Everything that refuses without mutating, in the order the
    /// dispatcher relies on before it records durable intent (QI-BB-029):
    /// shape and surface authority, the resource envelope (tallied here),
    /// then — for a delta — that both tracks hold the base as the exact
    /// sealed identity the ledger recorded. No lock is taken: the checks
    /// read the ledger and the tracks' sealed identities only, and
    /// `publish_batch` repeats the delta-base check under its lock before
    /// the first mutation.
    fn preflight_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        Self::validate_batch_shape_v1(batch)?;
        self.admit_resource_envelope(batch)?;
        self.builder.preflight_batch(batch)?;
        if let Some(base_generation) = batch.base_generation
            && !self.can_recover_completed_target_v1(batch)?
        {
            self.preflight_delta_base_v1(batch, base_generation)?;
        }
        Ok(())
    }

    fn publish_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<quanta_index_contract::SearchCorpusPublishOutcome, CoreError> {
        use quanta_index_contract::{
            IngestObservationStatus, SearchCorpusIngestObservation, SearchCorpusPublishOutcome,
        };
        let mut observation = SearchCorpusIngestObservation {
            request_id: budget.response_request_id(),
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            batch_digest: batch.batch_digest.clone(),
            status: IngestObservationStatus::Executed,
            semantic: None,
            lexical_build_ns: None,
            finalize_ns: None,
            activation_ns: None,
        };
        Self::validate_batch_shape_v1(batch)?;
        self.measure_resource_envelope(batch)?;
        let stripe = search_corpus_lock_stripe_v1(&batch.repo_id, &batch.revision_id);
        let operation_lock = self.operation_locks.get(stripe).ok_or_else(|| {
            CoreError::Storage(format!(
                "direct search-corpus materialize: computed operation-lock stripe {stripe} outside configured range"
            ))
        })?;
        let _operation_guard = operation_lock.lock().map_err(|err| {
            CoreError::Storage(format!(
                "direct search-corpus materialize: operation-lock stripe {stripe} poisoned: {err}"
            ))
        })?;

        // Repeat immutable base/candidate ownership admission under this
        // operation's lock before reservation, provider calls or either builder.
        self.builder.preflight_batch(batch)?;
        if let Some(base_generation) = batch.base_generation
            && !self.can_recover_completed_target_v1(batch)?
        {
            self.preflight_delta_base_v1(batch, base_generation)?;
        }
        if !batch.seal {
            let (lexical, semantic) = generation_pair_from_batch_v1(batch);
            ensure_generation_is_mutable_v1(
                self.lexical_generation_validator.as_ref(),
                &lexical,
                "lexical",
            )?;
            ensure_generation_is_mutable_v1(
                self.semantic_generation_validator.as_ref(),
                &semantic,
                "semantic",
            )?;
        }
        let sealed_plan = if batch.seal {
            let plan = self.preflight_sealed_generation_v1(batch)?;
            plan.validate_repair_mode_v1(batch.mode)?;
            Some(plan)
        } else {
            None
        };
        let build_lexical = sealed_plan
            .as_ref()
            .is_none_or(SealedGenerationBuildPlanV1::build_lexical);
        let build_semantic = sealed_plan
            .as_ref()
            .is_none_or(SealedGenerationBuildPlanV1::build_semantic);
        // Constructing the borrowed stream validates semantic source records,
        // the model contract, and egress admission without embedding. These
        // pure refusals must precede reservation and physical repair too.
        let mut derived = if build_semantic {
            Some(derive_semantic_stream_from_semantic_sources_v1(
                batch,
                self.semantic_embedder.as_ref(),
                self.semantic_stream_policy,
                self.source_egress_policy.as_ref(),
                budget,
            )?)
        } else {
            None
        };
        // Known admission refusals must not consume a stream slot. Once
        // admitted, reserve before reclaim, discard, or any provider/build work.
        let binding = Self::source_binding_v1(batch);
        match self.source_publication.reserve_source_event(&binding)? {
            quanta_index_core::SourceEventReservationV1::Reserved(_) => {}
            quanta_index_core::SourceEventReservationV1::Existing(record) => {
                if record.binding != binding
                    || record.phase != quanta_index_core::SourceEventPhaseV1::Pending
                {
                    return Err(CoreError::Typed { code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy, message: "source event already has an original publication; replay or reconcile its journal before materializing".into() });
                }
            }
        }
        if sealed_plan
            .as_ref()
            .is_some_and(SealedGenerationBuildPlanV1::is_finalize_only)
        {
            let started = std::time::Instant::now();
            self.finalize_sealed_generation_v1(batch)?;
            observation.finalize_ns = Some(elapsed_ingest_ns(started)?);
            observation.status = IngestObservationStatus::FinalizeOnly;
            return Ok(SearchCorpusPublishOutcome {
                publication: quanta_index_contract::SourcePublicationBinding::for_batch(batch),
                receipt: self.sealed_receipt_v1(batch)?,
                observation: Some(observation),
            });
        }
        if let Some(plan) = sealed_plan.as_ref() {
            plan.discard_incomplete_v1(
                self.lexical_incomplete_discard.as_ref(),
                self.semantic_incomplete_discard.as_ref(),
            )?;
            plan.repair_corrupt_v1(
                batch.mode,
                &self.snapshots,
                self.lexical_reclaim.as_ref(),
                self.semantic_reclaim.as_ref(),
            )?;
        }

        if !build_semantic || !build_lexical {
            observation.status = IngestObservationStatus::PartialRecovery;
        }
        // Search-owned semantic derivation is mandatory work for every
        // accepted search-corpus batch; there is no lexical-only downgrade
        // path. The semantic track builds first: its records are embedded
        // window by window as the build asks for them (QI-BB-021), and the
        // embedding provider is the one network dependency of a batch, so a
        // provider failure refuses the batch before the lexical track has
        // mutated, as the all-at-once derivation did. Every source record is
        // validated before the first window is embedded.
        if let Some(derived) = derived.as_mut() {
            let (semantic_receipt, semantic_report) = self
                .semantic_ingest
                .publish_stream(&derived.header, &mut derived.source)?;
            observation.semantic = Some(Box::new(semantic_report));
            validate_semantic_publish_receipt_v1(
                &derived.header,
                derived.source.tally(),
                &semantic_receipt,
            )?;
            if batch.seal {
                crash_point::reached(crash_point::AFTER_SEMANTIC_SEAL);
            }
        }
        if build_lexical {
            let started = std::time::Instant::now();
            self.builder.build_batch(batch)?;
            observation.lexical_build_ns = Some(elapsed_ingest_ns(started)?);
        }
        if batch.seal {
            let (lexical, semantic) = generation_pair_from_batch_v1(batch);
            validate_physical_generation_v1(
                self.lexical_generation_validator.as_ref(),
                &lexical,
                "lexical post-build",
            )?;
            validate_physical_generation_v1(
                self.semantic_generation_validator.as_ref(),
                &semantic,
                "semantic post-build",
            )?;
            crash_point::reached(crash_point::BEFORE_AUTHORITY_RECORD);
            let started = std::time::Instant::now();
            self.finalize_sealed_generation_v1(batch)?;
            observation.finalize_ns = Some(elapsed_ingest_ns(started)?);
            return Ok(SearchCorpusPublishOutcome {
                publication: quanta_index_contract::SourcePublicationBinding::for_batch(batch),
                receipt: self.sealed_receipt_v1(batch)?,
                observation: Some(observation),
            });
        }
        let started = std::time::Instant::now();
        self.finalize_generation_v1(batch, None)?;
        observation.finalize_ns = Some(elapsed_ingest_ns(started)?);
        Ok(SearchCorpusPublishOutcome {
            publication: quanta_index_contract::SourcePublicationBinding::for_batch(batch),
            receipt: batch_publish_receipt_v1(batch),
            observation: Some(observation),
        })
    }
}

fn elapsed_ingest_ns(started: std::time::Instant) -> Result<u64, CoreError> {
    u64::try_from(started.elapsed().as_nanos()).map_err(|error| {
        CoreError::InvalidContract(format!(
            "ingest observation: elapsed duration overflow: {error}"
        ))
    })
}

impl DirectSearchCorpusMaterializer {
    fn source_binding_v1(
        batch: &SearchCorpusIngestBatch,
    ) -> quanta_index_core::SourceEventBindingV1 {
        let (target, _) = generation_pair_from_batch_v1(batch);
        quanta_index_core::SourceEventBindingV1 {
            event: batch.source_event.clone(),
            target,
            journal_key: quanta_index_core::IdempotencyKeyV1 {
                kind: quanta_index_contract::IngestOperationKindV1::SearchCorpus,
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                batch_digest: batch.batch_digest.clone(),
            },
        }
    }

    /// A completed original target can outlive its base after retention, before
    /// the operation journal acknowledges it. A sealed pair alone is not proof
    /// that its complete chunk transaction ran. Require that durable marker and
    /// the exact pending source reservation, then validate both physical tracks.
    /// The caller has already validated the lexical source-event binding.
    fn can_recover_completed_target_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<bool, CoreError> {
        if !batch.seal {
            return Ok(false);
        }
        let complete = self.ledger.read().map_err(|error| {
            CoreError::Storage(format!("direct search-corpus materialize: ledger poisoned while inspecting completed target: {error}"))
        })?.structural_state(&batch.repo_id, &batch.revision_id, batch.generation)
            .is_some_and(|state| state.source_batch_digest() == Some(batch.batch_digest.as_str()));
        if !complete {
            return Ok(false);
        }
        let Some(record) = self
            .source_publication
            .inspect_source_event(&batch.repo_id, &batch.source_event)?
        else {
            return Ok(false);
        };
        if record.phase != quanta_index_core::SourceEventPhaseV1::Pending
            || record.binding != Self::source_binding_v1(batch)
        {
            return Ok(false);
        }
        Ok(self
            .preflight_sealed_generation_v1(batch)?
            .is_finalize_only())
    }

    /// The receipt of a sealed batch, attesting the content roots the
    /// semantic generation sealed (QI-BB-028) so the producer can name
    /// them when it activates. Read from the sealed manifest the seal
    /// just wrote — never from the batch.
    fn sealed_receipt_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let (_lexical, semantic) = generation_pair_from_batch_v1(batch);
        let roots = self
            .semantic_content_roots
            .sealed_content_roots(&semantic)
            .map_err(|source| CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
                message: format!(
                    "direct search-corpus materialize: sealed semantic generation {} carries no attestable content roots: {source}",
                    semantic.manifest_generation.get()
                ),
            })?;
        let mut receipt = batch_publish_receipt_v1(batch);
        receipt.attest_semantic_content(roots);
        Ok(receipt)
    }

    /// A delta may only build on a base that both tracks hold as the exact
    /// sealed identity the ledger recorded (QI-BB-029).
    ///
    /// Runs before any adapter mutates, so an absent, unsealed or mismatched
    /// base refuses the batch with zero bytes changed instead of letting the
    /// lexical track clone and seal on it and the semantic track refuse it
    /// afterwards. The digest comes from the ledger's sealed-track record;
    /// a base the ledger never sealed is refused outright rather than
    /// trusted because a directory happens to exist.
    fn preflight_delta_base_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
        base_generation: ManifestGeneration,
    ) -> Result<(), CoreError> {
        let tracks = [
            (
                SearchPlaneTrackKind::Lexical,
                &self.lexical_generation_validator,
                "lexical",
            ),
            (
                SearchPlaneTrackKind::Semantic,
                &self.semantic_generation_validator,
                "semantic",
            ),
        ];
        for (track, validator, label) in tracks {
            // Read the ledger only long enough to copy the digest out; the
            // physical validation below reads files and must not hold it.
            let recorded = self
                .ledger
                .read()
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "direct search-corpus materialize: ledger poisoned while preflighting delta base: {err}"
                    ))
                })?
                .sealed_track_identity_digest(
                    &batch.repo_id,
                    &batch.revision_id,
                    track,
                    base_generation,
                );
            let Some(digest) = recorded else {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusDeltaBaseNotSealed,
                    message: format!(
                        "direct search-corpus materialize: delta base generation {} is not a sealed {label} track for repo={} revision={}; refusing before any mutation",
                        base_generation.get(),
                        batch.repo_id.as_str(),
                        batch.revision_id.as_str(),
                    ),
                });
            };
            let base = GenerationSnapshot {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                track,
                manifest_generation: base_generation,
                manifest_digest: digest,
            };
            validate_delta_base_v1(validator.as_ref(), &base, &format!("{label} delta base"))?;
        }
        if self.ledger.read().map_err(|error| CoreError::Storage(format!(
            "direct search-corpus materialize: ledger poisoned while inspecting base chunk authority: {error}"
        )))?.structural_state(&batch.repo_id, &batch.revision_id, base_generation)
            .is_none_or(|state| state.source_batch_digest().is_none()) {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusDeltaBaseNotSealed,
                message: "source delta base has no complete chunk authority; rebuild the source generation before publication".into(),
            });
        }
        Ok(())
    }

    /// Computes the convergent per-track recovery plan before any mutation.
    ///
    /// The authority is consulted first so a batch that names a different
    /// digest than the one recorded for the generation is refused typed;
    /// past that, the plan follows the tracks' physical states alone. An
    /// authority that already knows the identity while a track is absent,
    /// incomplete or damaged is the repair case (a discarded, crashed or
    /// corrupt track being rebuilt under the recorded identity), and one
    /// that knows nothing is the first seal; both converge the same way.
    fn preflight_sealed_generation_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<SealedGenerationBuildPlanV1, CoreError> {
        let _known: SealedSearchCorpusAuthorityStateV1 =
            self.authority.inspect_sealed_search_corpus(
                &batch.repo_id,
                &batch.revision_id,
                batch.generation,
                batch.manifest_digest.as_str(),
            )?;
        let (lexical, semantic) = generation_pair_from_batch_v1(batch);
        let lexical_state = inspect_physical_generation_v1(
            self.lexical_generation_validator.as_ref(),
            &lexical,
            "lexical preflight",
        )?;
        let semantic_state = inspect_physical_generation_v1(
            self.semantic_generation_validator.as_ref(),
            &semantic,
            "semantic preflight",
        )?;
        Ok(SealedGenerationBuildPlanV1 {
            lexical,
            semantic,
            lexical_state,
            semantic_state,
        })
    }

    fn finalize_sealed_generation_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<(), CoreError> {
        // Retention can retire the base authority before its receipt returns,
        // including an I/O failure after a durable removal. Checkpoint the
        // complete target chunks first, while the base is still available.
        // This does not publish either track or advance rollback history.
        self.apply_source_finalization_v1(batch, None, false)?;
        // Fence rollback before entering the durable retention owner. A
        // delete may succeed while the following directory fsync fails; in
        // that state the old same-process ledger is not authoritative. Only a
        // reconciled retained-set receipt clears this pair-local fence.
        self.ledger
            .write()
            .map_err(|err| {
                CoreError::Storage(format!(
                    "direct search-corpus materialize: ledger poisoned while fencing retention mutation: {err}"
                ))
            })?
            .fence_search_corpus_history_v1(&batch.repo_id, &batch.revision_id);
        let retention = self.authority.record_sealed_search_corpus(
            &batch.repo_id,
            &batch.revision_id,
            batch.generation,
            batch.manifest_digest.as_str(),
        );
        let retention = match retention {
            Ok(receipt) => receipt,
            Err(error @ CoreError::Storage(_)) => return Err(error),
            Err(error) => {
                // Typed/contract rejections are pre-mutation outcomes of this
                // port and therefore do not create durability ambiguity.
                self.ledger
                    .write()
                    .map_err(|err| {
                        CoreError::Storage(format!(
                            "direct search-corpus materialize: ledger poisoned while clearing rejected retention mutation fence: {err}; original error: {error}"
                        ))
                    })?
                    .clear_search_corpus_history_fence_v1(
                        &batch.repo_id,
                        &batch.revision_id,
                    );
                return Err(error);
            }
        };
        crash_point::reached(crash_point::AFTER_RETENTION_RECEIPT);
        self.finalize_generation_v1(batch, Some(&retention))
    }

    pub(super) fn finalize_generation_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
        retention: Option<&SearchCorpusHistoryRetentionReceiptV1>,
    ) -> Result<(), CoreError> {
        if batch.seal && retention.is_none() {
            return Err(CoreError::InvalidContract(
                "direct search-corpus materialize: sealed generation requires durable retention receipt"
                    .to_string(),
            ));
        }
        self.apply_source_finalization_v1(batch, retention, true)
    }

    fn apply_source_finalization_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
        retention: Option<&SearchCorpusHistoryRetentionReceiptV1>,
        publish_tracks: bool,
    ) -> Result<(), CoreError> {
        const WHAT: &str = "direct search-corpus materialize";
        // The chunk universe and the reap of the auxiliary generations the
        // retention receipt retired are one durable transaction before the
        // generation is visible (QI-BB-020): validated against the ledger,
        // written to the catalog together, then applied under the write
        // lock with the track bookkeeping. A failed transaction leaves the
        // catalog and the ledger as they were, the retried seal redoes it.
        // The coordinator serializes every auxiliary mutation, so the set
        // read here is the set the write lock forgets.
        let _serial = self
            .auxiliary_coordinator
            .lock(&crate::ingest_dispatcher::auxiliary::coordinator_owner())?;
        let (chunks, reaped_auxiliary) = {
            let guard = self.ledger.read().map_err(|err| {
                CoreError::Storage(format!(
                    "{WHAT}: ledger poisoned while finalizing generation: {err}"
                ))
            })?;
            let epoch = guard.structural_next_epoch(
                &batch.repo_id,
                &batch.revision_id,
                batch.generation,
            )?;
            let current =
                guard.structural_state(&batch.repo_id, &batch.revision_id, batch.generation);
            let already_complete = current.is_some_and(|state| {
                state.source_batch_digest() == Some(batch.batch_digest.as_str())
            });
            if !publish_tracks && already_complete {
                return Ok(());
            }
            let mut chunks = if already_complete {
                // The previous atomic chunk transaction may have retired the
                // base before the operation journal acknowledged this batch.
                // Reuse only a complete target bound to this original body.
                let mut delta = structural_chunks_transition(current, epoch, batch);
                delta.removed.clear();
                delta.upserts.clear();
                delta.clear = false;
                delta
            } else {
                let inherited = match batch.base_generation {
                    Some(base) => {
                        let state = guard
                            .structural_state(&batch.repo_id, &batch.revision_id, base)
                            .filter(|state| state.source_batch_digest().is_some())
                            .ok_or_else(|| CoreError::NotReady(
                                "source delta has no complete base chunk authority; rebuild the source generation".into(),
                            ))?;
                        Some(state)
                    }
                    None => None,
                };
                let mut delta = structural_chunks_transition(inherited, epoch, batch);
                if let Some(base) = inherited {
                    let mut next = base.clone();
                    next.apply_chunks_delta(&delta);
                    delta.upserts = next.chunks().values().cloned().collect();
                }
                // This is the whole target chunk universe, not a patch against
                // an empty target. Persist inherited rows in the same existing
                // auxiliary transaction before retirement removes their base.
                delta.clear = true;
                delta.removed.clear();
                delta
            };
            chunks.meta.seal_requested =
                current.is_some_and(crate::readiness::StructuralAuthorityState::seal_requested);
            chunks.meta.source_batch_digest = Some(batch.batch_digest.clone());
            // Auxiliary generations older than the one being sealed that the
            // retention receipt does not retain go with it; a newer
            // generation still being staged is never touched.
            let reaped: Vec<ManifestGeneration> = retention.map_or_else(Vec::new, |retention| {
                guard
                    .auxiliary_generations_older_than(
                        &batch.repo_id,
                        &batch.revision_id,
                        batch.generation,
                    )
                    .into_iter()
                    .filter(|generation| !retention.retains(*generation))
                    .collect()
            });
            drop(guard);
            (chunks, reaped)
        };
        let mut delta = structural_chunks_delta_rows(&chunks)?;
        delta.rows.extend(reaped_auxiliary.iter().map(|generation| {
            AuxiliaryRowMutationV1::ForgetGeneration(AuxiliaryGenerationKeyV1 {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: *generation,
            })
        }));
        let _durable = self.auxiliary_catalog.apply(&delta)?;
        if retention.is_some() {
            crash_point::reached(crash_point::AFTER_CATALOG_TRANSACTION);
        }
        {
            let mut guard = self.ledger.write().map_err(|err| {
                CoreError::Storage(format!(
                    "{WHAT}: ledger poisoned while finalizing generation: {err}"
                ))
            })?;
            if let Some(retention) = retention {
                guard.apply_search_corpus_history_retention_receipt_v1(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    retention,
                )?;
            }
            guard.apply_structural_chunks_delta(&chunks, std::time::Instant::now())?;
            if !publish_tracks {
                return Ok(());
            }
            // Both tracks of the pair are recorded here and only here,
            // whether this batch built them or found them sealed on disk: a
            // seal retried after a crash does not rebuild a track the crash
            // left sealed, and a restarted daemon never seeded one the
            // authority had not recorded, so this record is what makes it
            // the track's identity (QI-BB-029).
            for track in [
                SearchPlaneTrackKind::Lexical,
                SearchPlaneTrackKind::Semantic,
            ] {
                guard.materialize_track(
                    &batch.repo_id,
                    &batch.revision_id,
                    track,
                    batch.generation,
                    Some(batch.manifest_digest.as_str()),
                );
                if batch.seal {
                    guard.seal_track_with_digest(
                        &batch.repo_id,
                        &batch.revision_id,
                        track,
                        batch.generation,
                        batch.manifest_digest.as_str(),
                    );
                }
            }
            if batch.seal {
                guard.record_historically_sealed_search_corpus(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    batch.manifest_digest.as_str(),
                );
            }
            for generation in &reaped_auxiliary {
                guard.forget_auxiliary_generation(&batch.repo_id, &batch.revision_id, *generation);
            }
        }
        if retention.is_some() {
            crash_point::reached(crash_point::AFTER_LEDGER_RECONCILE);
        }
        // Forgotten generations' text indexes go with their rows, swept
        // from the disk against the generations the ledger still knows, so
        // one a reader still holds, or whose discard failed, is found again
        // by the next seal of the pair.
        if let Some(history_text) = &self.history_text {
            let known: BTreeSet<ManifestGeneration> = self
                .ledger
                .read()
                .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?
                .auxiliary_generations_older_than(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                )
                .into_iter()
                .collect();
            history_text.sweep_forgotten_after_durable(
                &batch.repo_id,
                &batch.revision_id,
                batch.generation,
                &known,
            )?;
        }
        if let Some(retention) = retention {
            let receipt = self.reclaim_retired_generations_v1(batch, retention)?;
            self.record_reclaim_receipt(retention, &receipt)?;
        }
        Ok(())
    }

    /// Physical GC for the pair the batch just sealed (QI-BB-003).
    ///
    /// Runs only after the durable authority has been reaped and the ledger
    /// reconciled from the receipt, so no query can resolve or pin a retired
    /// generation any more (`UNKNOWN_GENERATION`). For each track, the
    /// reclaims a crash or a failed removal left in its reclaim area are
    /// finished first; then every sealed generation on disk that the receipt
    /// does not retain and that is older than the one being sealed is an
    /// orphan of this or an earlier retention pass; it is fenced out of the
    /// snapshot registry first — a resident handle dropped, an open in flight
    /// fenced without waiting for its opener, so its handle is refused rather
    /// than admitted; reclaim waits for a later pass if that open or any
    /// reader is still live. A pinned
    /// generation is deferred, not deleted under a reader; the next pass will
    /// find it again, and boot lists it as an orphan meanwhile. Sweeping from
    /// the filesystem rather than from the receipt's reaped set is what makes
    /// a crash between reap and reclaim recoverable — and a reclaim that
    /// fails recoverable the same way: the seal it follows is durable and
    /// stands, so a listing or reclaim the storage failed is recorded on the
    /// receipt as a [`DeferredGcStep`], counted
    /// (`search_corpus_gc_failures_total`) and found again by the next pass
    /// (QI-BB-020). What fails closed: a refusal — a directory whose identity
    /// contradicts its path is a finding a retry would only repeat (§3.49) —
    /// and process state, such as a poisoned snapshot registry. The receipt
    /// is kept: its bytes and counts feed the
    /// `search_corpus_gc_…` metrics.
    pub(super) fn reclaim_retired_generations_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
        retention: &SearchCorpusHistoryRetentionReceiptV1,
    ) -> Result<SearchCorpusPhysicalReclaimReceiptV1, CoreError> {
        let mut receipt = SearchCorpusPhysicalReclaimReceiptV1::default();
        for (port, track) in self.reclaim_tracks() {
            if track == SearchPlaneTrackKind::Semantic {
                crash_point::reached(crash_point::BETWEEN_TRACK_RECLAIMS);
            }
            // What a crash or a failed removal left in the track's reclaim
            // area goes first: it is out of every namespace already, and
            // nothing else ever finds it (QI-BB-003).
            match port.finish_interrupted_reclaims() {
                Ok(finished) => {
                    if finished.entries > 0 {
                        let _prior = receipt.finished_interrupted.insert(track, finished);
                    }
                }
                Err(error) => {
                    defer_storage_failure(error)?;
                    let _new = receipt
                        .deferred
                        .insert(DeferredGcStep::FinishInterrupted(track));
                }
            }
            let sealed = match port.sealed_generations_for_pair(&batch.repo_id, &batch.revision_id)
            {
                Ok(sealed) => sealed,
                Err(error) => {
                    defer_storage_failure(error)?;
                    let _new = receipt.deferred.insert(DeferredGcStep::ListSealed(track));
                    continue;
                }
            };
            for retired in sealed {
                let generation = retired.manifest_generation;
                if retention.retains(generation) || generation >= batch.generation {
                    continue;
                }
                let key = SnapshotKey::new(&batch.repo_id, &batch.revision_id, generation);
                let fence = match track {
                    SearchPlaneTrackKind::Lexical => self.snapshots.lexical.retire(&key)?,
                    SearchPlaneTrackKind::Semantic => self.snapshots.semantic.retire(&key)?,
                    SearchPlaneTrackKind::Structural => {
                        return Err(CoreError::InvalidContract(
                            "search-corpus physical reclaim: structural is not a search-corpus track"
                                .to_string(),
                        ));
                    }
                };
                if let SnapshotRetireOutcome::StillReferenced { holders } = fence {
                    let _deferred = receipt.deferred_pinned.insert((track, generation, holders));
                    continue;
                }
                crash_point::reached(crash_point::AFTER_FENCE);
                match port.reclaim_sealed_generation(&retired) {
                    Ok(SealedGenerationReclaimOutcomeV1::Absent) => {}
                    Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes }) => {
                        let _prior = receipt.reclaimed.insert((track, generation), bytes);
                    }
                    Err(error) => {
                        defer_storage_failure(error)?;
                        let _new = receipt
                            .deferred
                            .insert(DeferredGcStep::Reclaim(track, generation));
                    }
                }
            }
        }
        crash_point::reached(crash_point::BEFORE_RECORD_FORGET);
        self.forget_broken_pair_records_v1(batch, &mut receipt)?;
        Ok(receipt)
    }

    /// Both search-corpus tracks' reclaim ports, lexical first.
    fn reclaim_tracks(
        &self,
    ) -> [(
        &Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
        SearchPlaneTrackKind,
    ); 2] {
        [
            (&self.lexical_reclaim, SearchPlaneTrackKind::Lexical),
            (&self.semantic_reclaim, SearchPlaneTrackKind::Semantic),
        ]
    }

    /// Idempotency records live and die with their generation (QI-BB-032
    /// 보완 #5).
    ///
    /// A record answers a replay with "this body is durably applied"; that
    /// is only true while the generation is a whole sealed pair on disk.
    /// After the sweep, every generation the catalog knows for the pair —
    /// older than the one being sealed, so the records of the batch in
    /// flight and of any newer staging generation are never touched — is
    /// reconciled against what each track still holds as sealed: a
    /// generation reclaimed on either track (this pass or an earlier one),
    /// absent because it never sealed, or discarded on one track is no
    /// longer whole, and its records are forgotten so a replay applies
    /// afresh instead of being acked from a record that describes nothing.
    /// A track deferred under a pin still holds the generation, so its
    /// records survive until the pass that reclaims it. Forgetting is
    /// idempotent; a crash between reclaim and forget, and a listing or
    /// forget the storage failed (recorded on the receipt as a
    /// [`DeferredGcStep`]), are repaired by the next pass (QI-BB-020); a
    /// refusal fails closed.
    fn forget_broken_pair_records_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
        receipt: &mut SearchCorpusPhysicalReclaimReceiptV1,
    ) -> Result<(), CoreError> {
        let mut held: BTreeMap<SearchPlaneTrackKind, BTreeSet<ManifestGeneration>> =
            BTreeMap::new();
        for (port, track) in self.reclaim_tracks() {
            match port.sealed_generations_for_pair(&batch.repo_id, &batch.revision_id) {
                Ok(sealed) => {
                    let _prior = held.insert(
                        track,
                        sealed
                            .iter()
                            .map(|sealed| sealed.manifest_generation)
                            .collect(),
                    );
                }
                Err(error) => {
                    defer_storage_failure(error)?;
                    let _new = receipt.deferred.insert(DeferredGcStep::ListSealed(track));
                }
            }
        }
        let generations = match self
            .idempotency
            .generations_for_pair(&batch.repo_id, &batch.revision_id)
        {
            Ok(generations) => generations,
            Err(error) => {
                defer_storage_failure(error)?;
                let _new = receipt.deferred.insert(DeferredGcStep::ListRecords);
                return Ok(());
            }
        };
        for generation in generations {
            if generation >= batch.generation {
                continue;
            }
            // One listed track lacking the generation proves it broken; a
            // track that could not be listed proves nothing, so a
            // generation every listed track holds waits for a pass that
            // lists both.
            let broken = held.values().any(|sealed| !sealed.contains(&generation));
            if !broken {
                continue;
            }
            match self
                .idempotency
                .forget_generation(&batch.repo_id, &batch.revision_id, generation)
            {
                Ok(records) => {
                    let _prior = receipt.forgotten_records.insert(generation, records);
                }
                Err(error) => {
                    defer_storage_failure(error)?;
                    let _new = receipt
                        .deferred
                        .insert(DeferredGcStep::ForgetRecords(generation));
                }
            }
        }
        Ok(())
    }
}
