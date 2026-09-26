//! Search-owned semantic batch derivation.
//!
//! Turns an accepted `SearchCorpusIngestBatch` into a streamed semantic
//! batch: a [`SemanticIngestHeaderV1`] the build knows up front, and a
//! [`DerivedSemanticScopeSource`] that embeds the batch's records one
//! bounded window at a time as the build asks for them (QI-BB-021). The
//! derivation consumes only producer-authored typed semantic sources. The
//! embedder identity is pinned into the header's `EmbeddingModelContract`.
//! This is the ingest counterpart to the
//! query-time model-identity gate: both sides read the same embedder
//! identity so a corpus vector and a query vector can never be silently
//! produced by different models.
//!
//! Every record of the batch is validated before the first window is
//! embedded, so a malformed batch is refused with zero bytes changed and
//! zero provider calls; the provider is called once per window, with the
//! window's texts together, so a window of many small scopes still packs
//! into the fewest token-budget-bounded requests.
//!
//! Extracted from `ingest_dispatcher` so the routing dispatcher no longer owns
//! embedding/redistribution/digest mechanics — it just calls
//! [`derive_semantic_stream_from_semantic_sources_v1`].

use std::collections::{BTreeSet, VecDeque};

use quanta_index_contract::{
    EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract, EmbeddingNormalization,
    EmbeddingRecord, GenerationPin, SearchCorpusIngestBatch, SearchScopeKey, SearchScopeSurface,
    SemanticCorpusKindV1, SemanticReplaceScope, SemanticSourceRecordV1, SemanticSourceScopeKeyV1,
    SemanticTombstoneScope, SourceRoleV1, canonical_order::first_canonical_order_break_v1,
    lex::LanguageCode, lex::SymbolKindCode, validate_semantic_source_record_v1,
};
use quanta_index_core::{
    CoreError, RequestBudgetV1, RequestProviderStageV1, SemanticAdmissionEngine,
    SemanticBatchIdentityV1, SemanticBatchMutationsV1, SemanticEgressPolicyV1,
    SemanticGenerationContractV1, SemanticIngestHeaderV1, SemanticInputClass, SemanticScopeSource,
    SemanticScopeWindowV1, SemanticStreamTallyV1, SemanticStreamWindowPolicy, SemanticWindowFillV1,
    SemanticWindowIssuerV1, SemanticWindowPlacementV1, TextEmbeddingProvider,
};
use sha2::{Digest, Sha256};

const SEMANTIC_SOURCE_POLICY_DIGEST: &str = "semantic-source.v1";
const SEMANTIC_SOURCE_VIEW_POLICY_DIGEST: &str = "semantic-source.v1";

struct ValidatedSemanticSourceScope<'a> {
    scope: SearchScopeKey,
    scope_digest: String,
    semantic_scope: quanta_index_contract::SemanticSourceScopeKeyV1,
    records: Vec<&'a SemanticSourceRecordV1>,
    cluster_memberships: Vec<quanta_index_contract::ClusterMembershipReplaceV1>,
}

fn embedding_model_contract_for(
    embedder: &dyn TextEmbeddingProvider,
    policy_digest: &str,
    view_policy_digest: Option<&str>,
) -> Result<EmbeddingModelContract, CoreError> {
    let dimension = u32::try_from(embedder.dimension()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "semantic derivation: embedding dimension overflow: {err}"
        ))
    })?;
    let model_id = embedder.model_id();
    // A sealed generation promises unit vectors; a provider that does not
    // is a composition defect, not something to record as `None` and serve.
    if embedder.normalization() != EmbeddingNormalization::L2Unit {
        return Err(CoreError::InvalidContract(format!(
            "semantic derivation: embedder {model_id} promises {:?}, the corpus contract requires L2Unit",
            embedder.normalization()
        )));
    }
    Ok(EmbeddingModelContract {
        model_id: model_id.to_string().into_boxed_str(),
        model_version: Some(embedder.model_revision().to_string().into_boxed_str()),
        dimension,
        normalization: EmbeddingNormalization::L2Unit,
        distance_metric: EmbeddingDistanceMetric::Cosine,
        policy_digest: format!("{model_id}:{policy_digest}").into_boxed_str(),
        view_policy_digest: view_policy_digest.map(|value| value.to_string().into_boxed_str()),
    })
}

/// One accepted batch, derived: the header the build knows up front and
/// the source that embeds its replace scopes window by window.
pub(crate) struct DerivedSemanticStreamV1<'a> {
    pub(crate) header: SemanticIngestHeaderV1,
    pub(crate) source: DerivedSemanticScopeSource<'a>,
}

/// One owner scope of the producer's batch, waiting to be embedded.
struct PendingOwnerScope<'a>(ValidatedSemanticSourceScope<'a>);

impl PendingOwnerScope<'_> {
    fn records(&self) -> usize {
        self.0.records.len()
    }

    fn texts<'s>(&'s self) -> Box<dyn Iterator<Item = &'s str> + 's> {
        Box::new(self.0.records.iter().map(|record| record.text.as_str()))
    }
}

/// The scope-at-a-time source of one derived batch.
///
/// Holds the producer's records borrowed and un-embedded, planned into
/// owner scopes in canonical order; each `next_window` takes as many owner
/// scopes as the window policy admits, embeds their texts in one provider
/// call, and issues the embedded replace scopes as one leased window. A
/// Each semantic source scope is one owner and never splits across windows.
pub(crate) struct DerivedSemanticScopeSource<'a> {
    embedder: &'a dyn TextEmbeddingProvider,
    budget: RequestBudgetV1,
    next_window_ordinal: u64,
    embedding_elapsed_ns: Option<u64>,
    model_contract: EmbeddingModelContract,
    policy: SemanticStreamWindowPolicy,
    dimension: usize,
    pending: VecDeque<PendingOwnerScope<'a>>,
    issuer: SemanticWindowIssuerV1,
}

impl<'a> DerivedSemanticScopeSource<'a> {
    fn new(
        embedder: &'a dyn TextEmbeddingProvider,
        model_contract: EmbeddingModelContract,
        policy: SemanticStreamWindowPolicy,
        pending: VecDeque<PendingOwnerScope<'a>>,
        source_egress: Option<&'a SemanticEgressPolicyV1>,
        budget: &RequestBudgetV1,
    ) -> Result<Self, CoreError> {
        // Source-content egress gate (S21-08): when the composition routes
        // derivation through an external provider, the batch needs an
        // explicit external grant with source-content consent before the
        // first window is embedded. `None` is a local (hash) composition
        // with no egress, which keeps the input-matrix-only behavior; the
        // per-text matrix still runs in `embed_window` either way.
        if let Some(egress) = source_egress {
            let _admitted =
                SemanticAdmissionEngine::admit(SemanticInputClass::SourceContent, egress)?;
        }
        let dimension = usize::try_from(model_contract.dimension).map_err(|err| {
            CoreError::InvalidContract(format!(
                "semantic derivation: model contract dimension overflow: {err}"
            ))
        })?;
        Ok(Self {
            embedder,
            budget: budget.clone(),
            next_window_ordinal: 0,
            embedding_elapsed_ns: None,
            model_contract,
            policy,
            dimension,
            pending,
            issuer: SemanticWindowIssuerV1::new(),
        })
    }

    /// Owner scopes still waiting to be embedded.
    #[cfg(test)]
    pub(crate) fn pending_owner_scopes(&self) -> usize {
        self.pending.len()
    }

    /// The residency every window of this source is accounted in.
    #[cfg(test)]
    pub(crate) const fn residency_for_tests(
        &self,
    ) -> &std::sync::Arc<quanta_index_core::SemanticWindowResidencyV1> {
        self.issuer.residency()
    }

    /// Take the owner scopes of the next window off the plan.
    fn take_next_window(&mut self) -> Result<Vec<PendingOwnerScope<'a>>, CoreError> {
        let mut fill = SemanticWindowFillV1::default();
        let mut taken = Vec::new();
        while let Some(next) = self.pending.front() {
            let bytes = SemanticStreamWindowPolicy::vector_bytes(next.records(), self.dimension)?;
            match self.policy.place(fill, bytes)? {
                SemanticWindowPlacementV1::Joins => {}
                SemanticWindowPlacementV1::OpensNext => break,
            }
            fill.owner_scopes = fill.owner_scopes.checked_add(1).ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic derivation: window owner count overflow".to_string(),
                )
            })?;
            fill.vector_bytes = fill.vector_bytes.checked_add(bytes).ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic derivation: window vector bytes overflow".to_string(),
                )
            })?;
            let owner = self.pending.pop_front().ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic derivation: plan emptied under its own cursor".to_string(),
                )
            })?;
            taken.push(owner);
        }
        Ok(taken)
    }

    /// Embed one window's owner scopes in ONE provider call and redistribute
    /// the vectors back to their records in order.
    ///
    /// A draining iterator preserves record<->vector alignment without index
    /// arithmetic; an underflow (fewer vectors than records) and a leftover
    /// (more than records) both fail closed rather than silently misalign a
    /// vector with a record.
    fn embed_window(
        &mut self,
        owners: Vec<PendingOwnerScope<'a>>,
    ) -> Result<Vec<SemanticReplaceScope>, CoreError> {
        let texts: Vec<&str> = owners.iter().flat_map(PendingOwnerScope::texts).collect();
        // Pre-I/O admission (S21-08): the same profile-independent input
        // matrix the query path runs, applied to source content before the
        // window's single provider call. The egress/consent gate itself
        // lives at the provider boundary composition.
        for text in &texts {
            quanta_index_core::SemanticAdmissionEngine::admit_input_text(
                quanta_index_core::SemanticInputClass::SourceContent,
                text,
            )?;
        }
        let window_ordinal = self.next_window_ordinal.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract(
                "semantic derivation: provider window ordinal overflow".to_string(),
            )
        })?;
        self.next_window_ordinal = window_ordinal;
        self.budget
            .record_provider_stage_v1(RequestProviderStageV1::IngestWindowStarted {
                window_ordinal,
            });
        // Ingest checks cancellation before durable intent and completes an
        // admitted publish. This diagnostic handoff does not call checkpoint
        // or change the provider's existing mid-commit cancellation policy.
        let embedding_started = std::time::Instant::now();
        let embedded = self.embedder.embed_batch(&texts);
        let elapsed = u64::try_from(embedding_started.elapsed().as_nanos()).map_err(|error| {
            CoreError::InvalidContract(format!(
                "semantic derivation: embedding duration overflow: {error}"
            ))
        })?;
        self.embedding_elapsed_ns = Some(
            self.embedding_elapsed_ns
                .unwrap_or(0)
                .checked_add(elapsed)
                .ok_or_else(|| {
                    CoreError::InvalidContract(
                        "semantic derivation: accumulated embedding duration overflow".to_string(),
                    )
                })?,
        );
        self.budget
            .record_provider_stage_v1(RequestProviderStageV1::IngestWindowReturned {
                window_ordinal,
            });
        let all_vectors = embedded?;
        if all_vectors.len() != texts.len() {
            return Err(CoreError::InvalidContract(format!(
                "semantic derivation: embedder returned {} vectors for {} window texts",
                all_vectors.len(),
                texts.len()
            )));
        }
        let mut vectors = all_vectors.into_iter();
        let mut scopes: Vec<SemanticReplaceScope> = Vec::new();
        for owner in owners {
            let validated = owner.0;
            let embeddings = validated
                .records
                .into_iter()
                .map(|record| {
                    let vector = vectors.next().ok_or_else(|| {
                        CoreError::InvalidContract(
                            "semantic derivation: ran out of embedding vectors while \
                             redistributing semantic source embeddings"
                                .to_string(),
                        )
                    })?;
                    embedding_record_for_semantic_source(record, vector, &self.model_contract)
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            scopes.push(SemanticReplaceScope {
                scope: validated.scope,
                scope_digest: validated.scope_digest,
                embeddings,
                cluster_memberships: validated.cluster_memberships,
            });
        }
        if vectors.next().is_some() {
            return Err(CoreError::InvalidContract(
                "semantic derivation: window embed produced more vectors than the window had records"
                    .to_string(),
            ));
        }
        Ok(scopes)
    }
}

impl SemanticScopeSource for DerivedSemanticScopeSource<'_> {
    fn next_window(&mut self) -> Result<Option<SemanticScopeWindowV1>, CoreError> {
        self.issuer.require_no_window_resident()?;
        if self.pending.is_empty() {
            return Ok(None);
        }
        let owners = self.take_next_window()?;
        let scopes = self.embed_window(owners)?;
        self.issuer.issue(scopes).map(Some)
    }

    fn tally(&self) -> SemanticStreamTallyV1 {
        self.issuer.tally()
    }

    fn embedding_elapsed_ns(&self) -> Option<u64> {
        self.embedding_elapsed_ns
    }
}

fn semantic_header_v1(
    batch: &SearchCorpusIngestBatch,
    batch_digest: String,
    model_contract: EmbeddingModelContract,
    required_corpora: Vec<SemanticCorpusKindV1>,
    corpus_policy_digest: Option<String>,
) -> SemanticIngestHeaderV1 {
    SemanticIngestHeaderV1 {
        pin: GenerationPin::new(
            batch.repo_id.clone(),
            batch.revision_id.clone(),
            batch.generation,
        ),
        contract: SemanticGenerationContractV1 {
            mode: batch.mode,
            base_generation: batch.base_generation,
            model_contract,
            required_corpora,
            corpus_policy_digest,
        },
        batch: SemanticBatchIdentityV1 {
            manifest_digest: batch.manifest_digest.clone(),
            batch_digest,
            seal: batch.seal,
        },
        mutations: SemanticBatchMutationsV1 {
            clear_surfaces: batch.clear_surfaces.clone(),
            tombstone_scopes: semantic_tombstone_scopes_v1(batch),
        },
    }
}

fn require_embedder_dimension(embedder: &dyn TextEmbeddingProvider) -> Result<(), CoreError> {
    if embedder.dimension() == 0 {
        return Err(CoreError::InvalidContract(
            "semantic derivation: embedding dimension must be non-zero".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn derive_semantic_stream_from_semantic_sources_v1<'a>(
    batch: &'a SearchCorpusIngestBatch,
    embedder: &'a dyn TextEmbeddingProvider,
    policy: SemanticStreamWindowPolicy,
    source_egress: Option<&'a SemanticEgressPolicyV1>,
    budget: &RequestBudgetV1,
) -> Result<DerivedSemanticStreamV1<'a>, CoreError> {
    batch
        .validate_surface_mutations_v1()
        .map_err(|err| CoreError::InvalidContract(format!("semantic derivation: {err}")))?;
    require_embedder_dimension(embedder)?;
    // An empty typed source list is an explicit semantic no-op. Lexical-only
    // deltas can occur when source text changes without changing rendered
    // semantic cards; a replace-generation with no admissible cards is also
    // valid. The producer owns this decision and the batch digest binds it.
    let validated_scopes = validated_semantic_source_scopes_v1(batch)?;
    let model_contract = embedding_model_contract_for(
        embedder,
        SEMANTIC_SOURCE_POLICY_DIGEST,
        Some(SEMANTIC_SOURCE_VIEW_POLICY_DIGEST),
    )?;
    let required_corpora = required_corpora_for_semantic_sources_v1(&validated_scopes);
    let header = semantic_header_v1(
        batch,
        format!("{}:semantic-derive:semantic-source-v1", batch.batch_digest),
        model_contract,
        required_corpora,
        Some(SEMANTIC_SOURCE_POLICY_DIGEST.to_string()),
    );
    let pending = validated_scopes
        .into_iter()
        .map(PendingOwnerScope)
        .collect();
    let source = DerivedSemanticScopeSource::new(
        embedder,
        header.contract.model_contract.clone(),
        policy,
        pending,
        source_egress,
        budget,
    )?;
    Ok(DerivedSemanticStreamV1 { header, source })
}

fn embedding_record_for_semantic_source(
    record: &SemanticSourceRecordV1,
    vector: Vec<f32>,
    model_contract: &EmbeddingModelContract,
) -> Result<EmbeddingRecord, CoreError> {
    let dimension = usize::try_from(model_contract.dimension).map_err(|_err| {
        CoreError::InvalidContract(format!(
            "semantic derivation: model contract dimension {} does not fit usize",
            model_contract.dimension
        ))
    })?;
    if vector.len() != dimension {
        return Err(CoreError::InvalidContract(format!(
            "semantic derivation: embedder returned dim {} for semantic source {}, expected {}",
            vector.len(),
            record.record_id,
            model_contract.dimension
        )));
    }
    let language = semantic_source_language_v1(record)?;
    let symbol_kind = semantic_source_symbol_kind_v1(record)?;
    let view_kind = semantic_source_view_kind_v1(record);
    Ok(EmbeddingRecord {
        embedding_id: EmbeddingId::new(record.record_id.as_str()),
        record_id: record.record_id.clone().into_boxed_str(),
        owner_kind: record.owner_kind,
        owner_id: record.owner_id.clone().into_boxed_str(),
        corpus_kind: record.corpus_kind,
        parent_owner_id: record.parent_owner_id.clone().map(String::into_boxed_str),
        source_doc_id: record.source_doc_id.clone().into_boxed_str(),
        repo_relative_path: record.repo_relative_path.clone(),
        language,
        package: record.package.clone().map(String::into_boxed_str),
        symbol_kind,
        visibility: record.visibility.clone().map(String::into_boxed_str),
        source_role: record.source_role,
        generated: record.generated,
        capability_status: record.capability_status,
        authority_digest: record.authority_digest.clone().into_boxed_str(),
        render_policy_digest: record.render_policy_digest.clone().into_boxed_str(),
        card_schema_version: record.card_schema_version,
        start_byte: 0,
        end_byte: 0,
        start_line: 0,
        end_line: 0,
        snippet: record.text.clone().into_boxed_str(),
        embedding_input_digest: semantic_source_embedding_input_digest(
            model_contract,
            view_kind.as_str(),
            record,
        )
        .into_boxed_str(),
        vector_digest: semantic_vector_digest(model_contract, &vector).into_boxed_str(),
        view_kind: view_kind.into_boxed_str(),
        vector,
    })
}

pub(crate) fn semantic_vector_digest(
    model_contract: &EmbeddingModelContract,
    vector: &[f32],
) -> String {
    let mut vector_bytes = Vec::with_capacity(vector.len().saturating_mul(4));
    for value in vector {
        vector_bytes.extend_from_slice(&value.to_le_bytes());
    }
    let digest = sha256_hex(&[
        model_contract.model_id.as_bytes(),
        model_contract
            .model_version
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
        &model_contract.dimension.to_le_bytes(),
        &vector_bytes,
    ]);
    format!("search-owned-vec:sha256:{digest}")
}

fn validated_semantic_source_scopes_v1(
    batch: &SearchCorpusIngestBatch,
) -> Result<Vec<ValidatedSemanticSourceScope<'_>>, CoreError> {
    let mut replace_scope_keys = BTreeSet::new();
    let mut record_ids = BTreeSet::new();
    let tombstone_scope_keys = validated_semantic_tombstone_scope_keys_v1(batch)?;
    let mut scopes = batch.semantic_replace_scopes.iter().collect::<Vec<_>>();
    scopes.sort_by(|left, right| {
        semantic_scope_sort_key_v1(&left.scope).cmp(&semantic_scope_sort_key_v1(&right.scope))
    });
    scopes
        .into_iter()
        .map(|scope| {
            let scope_key_tuple = semantic_scope_sort_key_v1(&scope.scope);
            if !replace_scope_keys.insert(scope_key_tuple) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic derivation: duplicate semantic replace scope {:?}",
                    scope.scope.owner_id
                )));
            }
            if tombstone_scope_keys.contains(&scope_key_tuple) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic derivation: semantic scope {:?} cannot be replaced and tombstoned in one batch",
                    scope.scope.owner_id
                )));
            }
            if scope.scope.owner_id.is_empty() {
                return Err(CoreError::InvalidContract(
                    "semantic derivation: semantic replace scope owner_id must not be empty"
                        .to_string(),
                ));
            }
            if scope.scope_digest.is_empty() {
                return Err(CoreError::InvalidContract(
                    "semantic derivation: semantic source scope_digest must not be empty"
                        .to_string(),
                ));
            }
            let first_record = scope.sources.first().ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic derivation: semantic source scope must contain at least one source"
                        .to_string(),
                )
            })?;
            let scope_surface = SearchScopeSurface::for_semantic_owner_v1(
                scope.scope.owner_kind,
                scope.scope.corpus_kind,
            );
            let scope_key = SearchScopeKey {
                doc_surface: scope_surface,
                repo_relative_path: first_record.repo_relative_path.clone(),
            };
            let mut records = scope.sources.iter().collect::<Vec<_>>();
            records.sort_by(|left, right| left.record_id.cmp(&right.record_id));
            for record in &records {
                validate_semantic_source_record_v1(record).map_err(|message| {
                    CoreError::InvalidContract(format!(
                        "semantic derivation: invalid semantic source {}: {message}",
                        record.record_id
                    ))
                })?;
                if record.corpus_kind != scope.scope.corpus_kind {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic derivation: semantic source {} corpus_kind {:?} does not match scope {:?}",
                        record.record_id,
                        record.corpus_kind,
                        scope.scope.corpus_kind
                    )));
                }
                if record.owner_kind != scope.scope.owner_kind {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic derivation: semantic source {} owner_kind {:?} does not match scope {:?}",
                        record.record_id,
                        record.owner_kind,
                        scope.scope.owner_kind
                    )));
                }
                if record.owner_id != scope.scope.owner_id {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic derivation: semantic source {} owner_id {:?} does not match scope {:?}",
                        record.record_id,
                        record.owner_id,
                        scope.scope.owner_id
                    )));
                }
                if record.repo_relative_path != scope_key.repo_relative_path {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic derivation: semantic source scope {:?} spans multiple repo_relative_path values",
                        scope.scope.owner_id
                    )));
                }
                if record.source_doc_id.is_empty() {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic derivation: semantic source {} source_doc_id must not be empty",
                        record.record_id
                    )));
                }
                if !record_ids.insert(record.record_id.as_str()) {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic derivation: duplicate semantic source record_id {:?}",
                        record.record_id
                    )));
                }
            }
            let mut cluster_record_ids = BTreeSet::new();
            for membership in &scope.cluster_memberships {
                membership.validate_v1().map_err(|message| {
                    CoreError::InvalidContract(format!(
                        "semantic derivation: invalid cluster membership {}: {message}",
                        membership.cluster_record_id
                    ))
                })?;
                if !cluster_record_ids.insert(membership.cluster_record_id.as_str()) {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic derivation: duplicate cluster membership record_id {:?}",
                        membership.cluster_record_id
                    )));
                }
                let source = records
                    .iter()
                    .find(|record| record.record_id == membership.cluster_record_id)
                    .ok_or_else(|| {
                        CoreError::InvalidContract(format!(
                            "semantic derivation: cluster membership {:?} has no source record in the same replace scope",
                            membership.cluster_record_id
                        ))
                    })?;
                if source.authority_digest != membership.authority_digest {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic derivation: cluster membership {:?} authority digest does not match its source record",
                        membership.cluster_record_id
                    )));
                }
            }
            let cluster_source_count = records
                .iter()
                .filter(|record| record.corpus_kind == SemanticCorpusKindV1::ClusterCard)
                .count();
            if cluster_source_count != scope.cluster_memberships.len() {
                return Err(CoreError::InvalidContract(format!(
                    "semantic derivation: ClusterCard replace scope {:?} must carry one structured membership per source; sources={cluster_source_count} memberships={}",
                    scope.scope.owner_id,
                    scope.cluster_memberships.len()
                )));
            }
            if first_canonical_order_break_v1(&scope.cluster_memberships, |membership| {
                membership.cluster_record_id.as_str()
            })
            .is_some()
            {
                return Err(CoreError::InvalidContract(format!(
                    "semantic derivation: cluster memberships for scope {:?} must use canonical cluster_record_id order",
                    scope.scope.owner_id
                )));
            }
            Ok(ValidatedSemanticSourceScope {
                scope: scope_key,
                scope_digest: scope.scope_digest.clone(),
                semantic_scope: scope.scope.clone(),
                records,
                cluster_memberships: scope.cluster_memberships.clone(),
            })
        })
        .collect()
}

fn semantic_scope_sort_key_v1(
    scope: &SemanticSourceScopeKeyV1,
) -> (&'static str, &'static str, &str) {
    (
        scope.corpus_kind.as_code_str(),
        scope.owner_kind.as_code_str(),
        scope.owner_id.as_str(),
    )
}

fn validated_semantic_tombstone_scope_keys_v1(
    batch: &SearchCorpusIngestBatch,
) -> Result<BTreeSet<(&'static str, &'static str, &str)>, CoreError> {
    let mut keys = BTreeSet::new();
    for scope in &batch.semantic_tombstone_scopes {
        if scope.owner_id.is_empty() {
            return Err(CoreError::InvalidContract(
                "semantic derivation: semantic tombstone owner_id must not be empty".to_string(),
            ));
        }
        let key = semantic_scope_sort_key_v1(scope);
        if !keys.insert(key) {
            return Err(CoreError::InvalidContract(format!(
                "semantic derivation: duplicate semantic tombstone scope {:?}",
                scope.owner_id
            )));
        }
    }
    Ok(keys)
}

fn semantic_tombstone_scopes_v1(batch: &SearchCorpusIngestBatch) -> Vec<SemanticTombstoneScope> {
    batch
        .semantic_tombstone_scopes
        .iter()
        .cloned()
        .map(|semantic_scope| SemanticTombstoneScope { semantic_scope })
        .collect()
}

fn required_corpora_for_semantic_sources_v1(
    scopes: &[ValidatedSemanticSourceScope<'_>],
) -> Vec<SemanticCorpusKindV1> {
    let mut corpora: Vec<SemanticCorpusKindV1> = scopes
        .iter()
        .map(|scope| scope.semantic_scope.corpus_kind)
        .collect();
    corpora.sort_by_key(|corpus_kind| corpus_kind.as_code_str());
    corpora.dedup();
    corpora
}

fn semantic_source_language_v1(record: &SemanticSourceRecordV1) -> Result<LanguageCode, CoreError> {
    let language = record.language.as_deref().ok_or_else(|| {
        CoreError::InvalidContract(format!(
            "semantic derivation: semantic source {} missing language",
            record.record_id
        ))
    })?;
    LanguageCode::new(language).map_err(|message| {
        CoreError::InvalidContract(format!(
            "semantic derivation: semantic source {} invalid language {:?}: {message}",
            record.record_id, language
        ))
    })
}

fn semantic_source_symbol_kind_v1(
    record: &SemanticSourceRecordV1,
) -> Result<Option<SymbolKindCode>, CoreError> {
    record
        .symbol_kind
        .as_deref()
        .map(|symbol_kind| {
            SymbolKindCode::new(symbol_kind).map_err(|message| {
                CoreError::InvalidContract(format!(
                    "semantic derivation: semantic source {} invalid symbol_kind {:?}: {message}",
                    record.record_id, symbol_kind
                ))
            })
        })
        .transpose()
}

fn semantic_source_view_kind_v1(record: &SemanticSourceRecordV1) -> String {
    format!(
        "{}.{}",
        semantic_corpus_kind_slug_v1(record.corpus_kind),
        semantic_source_role_slug_v1(record.source_role)
    )
}

fn semantic_source_embedding_input_digest(
    model_contract: &EmbeddingModelContract,
    view_kind: &str,
    record: &SemanticSourceRecordV1,
) -> String {
    let card_schema_version = record.card_schema_version.to_le_bytes();
    let digest = sha256_hex(&[
        model_contract.model_id.as_bytes(),
        model_contract
            .model_version
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
        &model_contract.dimension.to_le_bytes(),
        view_kind.as_bytes(),
        record.authority_digest.as_bytes(),
        record.render_policy_digest.as_bytes(),
        &card_schema_version,
        record.text.as_bytes(),
    ]);
    format!("search-owned-in:sha256:{digest}")
}

fn semantic_corpus_kind_slug_v1(corpus_kind: SemanticCorpusKindV1) -> &'static str {
    match corpus_kind {
        SemanticCorpusKindV1::SymbolCard => "symbol",
        SemanticCorpusKindV1::ModuleCard => "module",
        SemanticCorpusKindV1::ClusterCard => "cluster",
        SemanticCorpusKindV1::RawCodeFallback => "raw_code",
        SemanticCorpusKindV1::DocumentLeaf => "document_leaf",
        SemanticCorpusKindV1::DocumentSection => "document_section",
        SemanticCorpusKindV1::DocumentSummary => "document_summary",
        SemanticCorpusKindV1::TestBehavior => "test_behavior",
        SemanticCorpusKindV1::RepositorySummary => "repository_summary",
    }
}

fn semantic_source_role_slug_v1(source_role: SourceRoleV1) -> &'static str {
    match source_role {
        SourceRoleV1::CardText => "card",
        SourceRoleV1::RawFallbackText => "raw_fallback",
        SourceRoleV1::DocumentText => "document",
        SourceRoleV1::SummaryText => "summary",
    }
}

fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0x1f]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        use std::fmt::Write as _;
        // Infallible write into a String; the explicitly-typed binding keeps the
        // `must_use` Result acknowledged (crate denies `let_underscore_must_use`).
        let _written: Result<(), std::fmt::Error> = write!(hex, "{byte:02x}");
    }
    hex
}

/// Reassemble one resident `SemanticIngestBatch` from a header and the
/// replace scopes a stream issued, for tests that compare derived batches.
#[cfg(test)]
pub(crate) fn assemble_semantic_batch_v1(
    header: &SemanticIngestHeaderV1,
    replace_scopes: Vec<SemanticReplaceScope>,
) -> quanta_index_contract::SemanticIngestBatch {
    quanta_index_contract::SemanticIngestBatch {
        repo_id: header.pin.repo_id.clone(),
        revision_id: header.pin.revision_id.clone(),
        generation: header.pin.manifest_generation,
        base_generation: header.contract.base_generation,
        manifest_digest: header.batch.manifest_digest.clone(),
        batch_digest: header.batch.batch_digest.clone(),
        mode: header.contract.mode,
        model_contract: header.contract.model_contract.clone(),
        required_corpora: header.contract.required_corpora.clone(),
        corpus_policy_digest: header.contract.corpus_policy_digest.clone(),
        clear_surfaces: header.mutations.clear_surfaces.clone(),
        replace_scopes,
        tombstone_scopes: header.mutations.tombstone_scopes.clone(),
        seal: header.batch.seal,
    }
}

/// Drain a derived stream window by window into one resident batch.
#[cfg(test)]
pub(crate) fn drain_semantic_stream_v1(
    mut derived: DerivedSemanticStreamV1<'_>,
) -> Result<quanta_index_contract::SemanticIngestBatch, CoreError> {
    let mut replace_scopes = Vec::new();
    while let Some(window) = derived.source.next_window()? {
        replace_scopes.extend(window.scopes().iter().cloned());
        drop(window);
    }
    Ok(assemble_semantic_batch_v1(&derived.header, replace_scopes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HashingQueryTextEmbedder, SEARCH_OWNED_SEMANTIC_DIMENSION};
    use quanta_index_contract::OwnerDocKind;
    use quanta_index_core::{
        RequestStageDiagnosticPortV1, SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE,
        SEMANTIC_STREAM_WINDOW_SCOPES, SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE,
        SEMANTIC_STREAM_WINDOW_VECTOR_BYTES, SemanticEgressGrantV1,
    };

    #[derive(Debug, Default)]
    struct RecordingProviderStages(std::sync::Mutex<Vec<RequestProviderStageV1>>);

    impl RequestStageDiagnosticPortV1 for RecordingProviderStages {
        fn record_provider_stage_v1(&self, stage: RequestProviderStageV1) {
            self.0.lock().expect("provider stage recorder").push(stage);
        }
    }

    /// Derive under the production window and drain the stream into one batch.
    fn derive_semantic_batch_from_semantic_sources_v1(
        batch: &SearchCorpusIngestBatch,
        embedder: &dyn TextEmbeddingProvider,
    ) -> Result<quanta_index_contract::SemanticIngestBatch, CoreError> {
        drain_semantic_stream_v1(derive_semantic_stream_from_semantic_sources_v1(
            batch,
            embedder,
            SemanticStreamWindowPolicy::DEFAULT,
            None,
            &RequestBudgetV1::unbounded(),
        )?)
    }
    use quanta_index_contract::{
        BatchIngestMode, ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath,
        RevisionId, SearchCorpusReplaceScope, SearchCorpusTombstoneScope, SearchScopeSurface,
        SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, lex::LanguageCode,
    };

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn fixture_chunk() -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: LanguageCode::new("rust").map_err(str::to_string)?,
            start_byte: 0,
            end_byte: 24,
            start_line: 1,
            end_line: 1,
            text: "lexical chunk body".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        })
    }

    fn fixture_semantic_source() -> SemanticSourceRecordV1 {
        SemanticSourceRecordV1 {
            record_id: "source-record-1".to_string(),
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id: "symbol-1".to_string(),
            source_doc_id: "doc-1".to_string(),
            parent_owner_id: None,
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: Some("rust".to_string()),
            package: Some("crate".to_string()),
            symbol_kind: Some("function".to_string()),
            visibility: Some("pub".to_string()),
            source_role: SourceRoleV1::CardText,
            generated: false,
            capability_status: quanta_index_contract::CapabilityStatusV1::Full,
            raw_fallback_reason: None,
            authority_digest: "auth:sha256:1".to_string(),
            render_policy_digest: "render:sha256:1".to_string(),
            card_schema_version: 1,
            text: "semantic card body".to_string(),
        }
    }

    fn fixture_search_batch() -> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
        Ok(SearchCorpusIngestBatch {
            repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-1")
                .expect("static fixture ID satisfies canonical policy"),
            generation: ManifestGeneration::new(7),
            base_generation: None,
            manifest_digest: "manifest:lex".to_string(),
            batch_digest: "batch:lex".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SearchCorpusReplaceScope {
                scope: SearchScopeKey {
                    doc_surface: SearchScopeSurface::Chunk,
                    repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                },
                scope_digest: "scope:lex".to_string(),
                chunks: vec![fixture_chunk()?],
                symbols: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: vec![SemanticSourceReplaceScopeV1 {
                scope: SemanticSourceScopeKeyV1 {
                    corpus_kind: SemanticCorpusKindV1::SymbolCard,
                    owner_kind: OwnerDocKind::Symbol,
                    owner_id: "symbol-1".to_string(),
                },
                scope_digest: "scope:semantic".to_string(),
                sources: vec![fixture_semantic_source()],
                cluster_memberships: Vec::new(),
            }],
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        })
    }

    #[test]
    fn semantic_derivation_sources_embed_deterministically() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let batch = fixture_search_batch()?;
        let first = derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder)?;
        let second = derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder)?;
        if first != second {
            return Err("semantic source derivation must be deterministic".into());
        }
        let embedding = first
            .replace_scopes
            .first()
            .and_then(|scope| scope.embeddings.first())
            .ok_or_else(|| "expected one semantic embedding".to_string())?;
        if embedding.embedding_id.as_str() != "source-record-1" {
            return Err("semantic source derivation must use record_id as embedding_id".into());
        }
        if embedding.owner_kind != OwnerDocKind::Symbol
            || embedding.owner_id.as_ref() != "symbol-1"
            || embedding.view_kind.as_ref() != "symbol.card"
        {
            return Err(format!(
                "unexpected semantic embedding identity: owner_kind={:?} owner_id={} view_kind={}",
                embedding.owner_kind, embedding.owner_id, embedding.view_kind
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn typed_source_input_digest_binds_text_and_authority() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let batch = fixture_search_batch()?;
        let model = derive_semantic_stream_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticStreamWindowPolicy::DEFAULT,
            None,
            &RequestBudgetV1::unbounded(),
        )?
        .header
        .contract
        .model_contract;
        let source = fixture_semantic_source();
        let view = semantic_source_view_kind_v1(&source);
        let original = semantic_source_embedding_input_digest(&model, &view, &source);
        let mut changed_text = source.clone();
        changed_text.text.push_str(" changed");
        let mut changed_authority = source;
        changed_authority.authority_digest.push_str(":changed");
        if original == semantic_source_embedding_input_digest(&model, &view, &changed_text)
            || original == semantic_source_embedding_input_digest(&model, &view, &changed_authority)
            || !original.starts_with("search-owned-in:sha256:")
        {
            return Err("typed input digest must bind source text and authority".into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_empty_typed_sources_are_a_noop() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.semantic_replace_scopes.clear();
        batch.seal = false;
        let derived = derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder)?;
        if !derived.replace_scopes.is_empty() || !derived.required_corpora.is_empty() {
            return Err(
                "empty typed scope must derive no semantic rows or required corpora".into(),
            );
        }
        Ok(())
    }

    #[test]
    fn lexical_delta_without_semantic_sources_preserves_tombstone_and_uses_no_provider() -> TestRes
    {
        let embedder = RecordingEmbedder::new();
        let mut batch = fixture_search_batch()?;
        batch.mode = BatchIngestMode::Delta;
        batch.base_generation = Some(ManifestGeneration::new(6));
        batch.semantic_replace_scopes.clear();
        batch.tombstone_scopes = vec![SearchCorpusTombstoneScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                repo_relative_path: RepoRelativePath::new("src/old.rs"),
            },
        }];
        batch.semantic_tombstone_scopes = vec![SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id: "symbol-deleted".to_string(),
        }];
        batch.seal = false;

        let derived = derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder)?;
        if !derived.replace_scopes.is_empty()
            || !derived.required_corpora.is_empty()
            || batch.tombstone_scopes.len() != 1
            || derived.tombstone_scopes.len() != 1
            || batch.semantic_tombstone_scopes.first()
                != derived
                    .tombstone_scopes
                    .first()
                    .map(|scope| &scope.semantic_scope)
            || derived.corpus_policy_digest.as_deref() != Some(SEMANTIC_SOURCE_POLICY_DIGEST)
            || derived.model_contract.view_policy_digest.as_deref()
                != Some(SEMANTIC_SOURCE_VIEW_POLICY_DIGEST)
            || derived.mode != BatchIngestMode::Delta
            || derived.base_generation != batch.base_generation
            || derived.seal
        {
            return Err(
                format!("lexical delta semantic no-op changed contract: {derived:?}").into(),
            );
        }
        if !embedder.calls()?.is_empty() {
            return Err("empty typed source delta must not call the provider".into());
        }
        batch.semantic_tombstone_scopes.clear();
        let lexical_only = derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder)?;
        if !lexical_only.tombstone_scopes.is_empty() || batch.tombstone_scopes.len() != 1 {
            return Err("lexical tombstone must not create a semantic tombstone".into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_tombstone_batch_does_not_require_replace_sources() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.replace_scopes.clear();
        batch.semantic_replace_scopes.clear();
        batch.semantic_tombstone_scopes = vec![SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id: "symbol-deleted".to_string(),
        }];
        batch.seal = false;

        let derived = derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder)?;

        if !derived.replace_scopes.is_empty() {
            return Err("tombstone-only derivation must not produce replacement scopes".into());
        }
        if derived.tombstone_scopes.len() != 1 {
            return Err("tombstone-only derivation must preserve exactly one tombstone".into());
        }
        if !derived.required_corpora.is_empty() {
            return Err("tombstone-only derivation must not require semantic corpora".into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_seal_batch_does_not_require_replace_sources() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.replace_scopes.clear();
        batch.semantic_replace_scopes.clear();
        batch.semantic_tombstone_scopes.clear();
        batch.seal = true;

        let derived = derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder)?;

        if !derived.replace_scopes.is_empty() || !derived.tombstone_scopes.is_empty() {
            return Err(
                "seal-only derivation must not produce replacement or tombstone scopes".into(),
            );
        }
        if !derived.required_corpora.is_empty() {
            return Err("seal-only derivation must not require semantic corpora".into());
        }
        if !derived.seal {
            return Err("seal-only derivation must preserve the seal flag".into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_clear_batch_does_not_require_replace_sources() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.mode = BatchIngestMode::Delta;
        batch.base_generation = Some(ManifestGeneration::new(6));
        batch.replace_scopes.clear();
        batch.semantic_replace_scopes.clear();
        batch.semantic_tombstone_scopes.clear();
        batch.clear_surfaces = vec![SearchScopeSurface::Chunk];
        batch.seal = false;

        let derived = derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder)?;

        if !derived.replace_scopes.is_empty() || !derived.tombstone_scopes.is_empty() {
            return Err(
                "clear-only derivation must not produce replacement or tombstone scopes".into(),
            );
        }
        if !derived.required_corpora.is_empty() {
            return Err("clear-only derivation must not require semantic corpora".into());
        }
        if derived.clear_surfaces != [SearchScopeSurface::Chunk] {
            return Err("clear-only derivation must preserve the requested chunk surface".into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_uses_typed_semantic_sources_v1() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let derived =
            derive_semantic_batch_from_semantic_sources_v1(&fixture_search_batch()?, &embedder)?;
        let embedding = derived
            .replace_scopes
            .first()
            .and_then(|scope| scope.embeddings.first())
            .ok_or_else(|| "expected one typed semantic embedding".to_string())?;
        if embedding.embedding_id.as_str() != "source-record-1"
            || embedding.view_kind.as_ref() != "symbol.card"
        {
            return Err(format!(
                "semantic source identity changed: id={} view_kind={}",
                embedding.embedding_id.as_str(),
                embedding.view_kind
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_invalid_source_fails_closed() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        let source = batch
            .semantic_replace_scopes
            .first_mut()
            .and_then(|scope| scope.sources.first_mut())
            .ok_or_else(|| "semantic fixture must contain one source".to_string())?;
        source.source_role = SourceRoleV1::DocumentText;
        match derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder) {
            Err(CoreError::InvalidContract(message)) if message.contains("CardText") => Ok(()),
            other => Err(format!("invalid semantic source must fail closed, got {other:?}").into()),
        }
    }

    #[test]
    fn semantic_derivation_propagates_owner_tombstone_without_fake_path() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch
            .semantic_tombstone_scopes
            .push(SemanticSourceScopeKeyV1 {
                corpus_kind: SemanticCorpusKindV1::ModuleCard,
                owner_kind: OwnerDocKind::Module,
                owner_id: "module-deleted".to_string(),
            });
        let derived = derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder)?;
        let tombstone = derived
            .tombstone_scopes
            .first()
            .ok_or_else(|| "expected semantic owner tombstone".to_string())?;
        if batch.semantic_tombstone_scopes.first() != Some(&tombstone.semantic_scope) {
            return Err("semantic owner tombstone identity was not preserved".into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_rejects_duplicate_replace_scope() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        let duplicate_scope = batch
            .semantic_replace_scopes
            .first()
            .cloned()
            .ok_or_else(|| "semantic fixture must contain one replace scope".to_string())?;
        batch.semantic_replace_scopes.push(duplicate_scope);
        match derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder) {
            Err(CoreError::InvalidContract(message))
                if message.contains("duplicate replace scope") =>
            {
                Ok(())
            }
            other => Err(format!("duplicate replace scope must fail closed, got {other:?}").into()),
        }
    }

    #[test]
    fn semantic_derivation_rejects_replace_tombstone_conflict() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        let replacement_scope = batch
            .semantic_replace_scopes
            .first()
            .map(|scope| scope.scope.clone())
            .ok_or_else(|| "semantic fixture must contain one replace scope".to_string())?;
        batch.semantic_tombstone_scopes.push(replacement_scope);
        match derive_semantic_batch_from_semantic_sources_v1(&batch, &embedder) {
            Err(CoreError::InvalidContract(message))
                if message.contains("replaced and tombstoned") =>
            {
                Ok(())
            }
            other => {
                Err(format!("replace/tombstone conflict must fail closed, got {other:?}").into())
            }
        }
    }

    #[test]
    fn semantic_derivation_canonicalizes_scope_and_record_order() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut forward = fixture_search_batch()?;
        let mut second_scope = forward
            .semantic_replace_scopes
            .first()
            .cloned()
            .ok_or_else(|| "semantic fixture must contain one replace scope".to_string())?;
        second_scope.scope.owner_id = "symbol-2".to_string();
        second_scope.scope_digest = "scope:semantic:2".to_string();
        let second_source = second_scope
            .sources
            .first_mut()
            .ok_or_else(|| "semantic fixture replace scope must contain one source".to_string())?;
        second_source.owner_id = "symbol-2".to_string();
        second_source.record_id = "source-record-2".to_string();
        forward.semantic_replace_scopes.push(second_scope);
        let mut reverse = forward.clone();
        reverse.semantic_replace_scopes.reverse();

        let forward_derived = derive_semantic_batch_from_semantic_sources_v1(&forward, &embedder)?;
        let reverse_derived = derive_semantic_batch_from_semantic_sources_v1(&reverse, &embedder)?;
        if forward_derived != reverse_derived {
            return Err("semantic derivation must canonicalize producer scope order".into());
        }
        Ok(())
    }

    // ---- QI-BB-021 follow-up #2: scope-streamed derivation ----

    /// A hashing embedder that records the texts of every provider call.
    struct RecordingEmbedder {
        inner: HashingQueryTextEmbedder,
        calls: std::sync::Mutex<Vec<Vec<String>>>,
        fail: bool,
    }

    impl RecordingEmbedder {
        fn new() -> Self {
            Self {
                inner: HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION),
                calls: std::sync::Mutex::new(Vec::new()),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                fail: true,
                ..Self::new()
            }
        }

        fn calls(&self) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
            Ok(self
                .calls
                .lock()
                .map_err(|err| format!("recording embedder poisoned: {err}"))?
                .clone())
        }
    }

    impl TextEmbeddingProvider for RecordingEmbedder {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            self.calls
                .lock()
                .map_err(|err| CoreError::Storage(format!("recording embedder poisoned: {err}")))?
                .push(texts.iter().map(|text| (*text).to_string()).collect());
            if self.fail {
                return Err(CoreError::Storage(
                    "scripted ingest provider failure".to_string(),
                ));
            }
            self.inner.embed_batch(texts)
        }
        fn model_id(&self) -> &str {
            self.inner.model_id()
        }
        fn model_revision(&self) -> &str {
            self.inner.model_revision()
        }
        fn dimension(&self) -> usize {
            self.inner.dimension()
        }
        fn normalization(&self) -> EmbeddingNormalization {
            self.inner.normalization()
        }
    }

    /// Five typed owners distributed over three files.
    fn typed_batch_a2_b1_c2() -> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
        let mut batch = fixture_search_batch()?;
        batch.semantic_replace_scopes = [
            ("a-1", "a.rs", "alpha one"),
            ("a-2", "a.rs", "alpha two"),
            ("b-1", "b.rs", "beta one"),
            ("c-1", "c.rs", "gamma one"),
            ("c-2", "c.rs", "gamma two"),
        ]
        .into_iter()
        .map(|(id, path, text)| {
            let mut record = fixture_semantic_source();
            record.record_id = id.to_string();
            record.owner_id = id.to_string();
            record.repo_relative_path = RepoRelativePath::new(path);
            record.text = text.to_string();
            SemanticSourceReplaceScopeV1 {
                scope: SemanticSourceScopeKeyV1 {
                    corpus_kind: SemanticCorpusKindV1::SymbolCard,
                    owner_kind: OwnerDocKind::Symbol,
                    owner_id: id.to_string(),
                },
                scope_digest: format!("scope:{id}"),
                sources: vec![record],
                cluster_memberships: Vec::new(),
            }
        })
        .collect();
        Ok(batch)
    }

    fn ids_of(scopes: &[SemanticReplaceScope]) -> Vec<String> {
        scopes
            .iter()
            .flat_map(|scope| {
                scope
                    .embeddings
                    .iter()
                    .map(|record| record.embedding_id.as_str().to_string())
            })
            .collect()
    }

    // CASE-COVERS: under a two-owner window, five typed owners stream as
    // three windows, each embedded in ONE provider call carrying exactly the
    // window's texts; vectors keep canonical owner order; the source
    // never had more than one window out.
    #[test]
    fn typed_owners_stream_in_policy_windows() -> TestRes {
        let embedder = RecordingEmbedder::new();
        let batch = typed_batch_a2_b1_c2()?;
        let policy = SemanticStreamWindowPolicy::new(2, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
        let stages = std::sync::Arc::new(RecordingProviderStages::default());
        let budget = RequestBudgetV1::unbounded().with_diagnostics(stages.clone());
        let mut derived = derive_semantic_stream_from_semantic_sources_v1(
            &batch, &embedder, policy, None, &budget,
        )?;
        if derived.source.pending_owner_scopes() != 5 {
            return Err("five typed records are five owner scopes".into());
        }
        let mut windows: Vec<Vec<(String, Vec<String>)>> = Vec::new();
        let residency = std::sync::Arc::clone(derived.source.residency_for_tests());
        while let Some(window) = derived.source.next_window()? {
            if residency.outstanding_windows() != 1 {
                return Err(format!(
                    "exactly one window is out while it is held, saw {}",
                    residency.outstanding_windows()
                )
                .into());
            }
            windows.push(
                window
                    .scopes()
                    .iter()
                    .map(|scope| {
                        (
                            scope.scope.repo_relative_path.as_str().to_string(),
                            ids_of(std::slice::from_ref(scope)),
                        )
                    })
                    .collect(),
            );
            drop(window);
            if residency.outstanding_windows() != 0 {
                return Err("a dropped window releases its lease".into());
            }
        }
        let expected: Vec<Vec<(String, Vec<String>)>> = vec![
            vec![
                ("a.rs".into(), vec!["a-1".into()]),
                ("a.rs".into(), vec!["a-2".into()]),
            ],
            vec![
                ("b.rs".into(), vec!["b-1".into()]),
                ("c.rs".into(), vec!["c-1".into()]),
            ],
            vec![("c.rs".into(), vec!["c-2".into()])],
        ];
        if windows != expected {
            return Err(format!("windows cut wrong: {windows:?}").into());
        }
        let calls = embedder.calls()?;
        let expected_calls: Vec<Vec<String>> = vec![
            vec!["alpha one".into(), "alpha two".into()],
            vec!["beta one".into(), "gamma one".into()],
            vec!["gamma two".into()],
        ];
        if calls != expected_calls {
            return Err(format!("one provider call per window with its texts: {calls:?}").into());
        }
        let expected_stages = [
            RequestProviderStageV1::IngestWindowStarted { window_ordinal: 1 },
            RequestProviderStageV1::IngestWindowReturned { window_ordinal: 1 },
            RequestProviderStageV1::IngestWindowStarted { window_ordinal: 2 },
            RequestProviderStageV1::IngestWindowReturned { window_ordinal: 2 },
            RequestProviderStageV1::IngestWindowStarted { window_ordinal: 3 },
            RequestProviderStageV1::IngestWindowReturned { window_ordinal: 3 },
        ];
        let observed_stages = stages
            .0
            .lock()
            .map_err(|error| format!("provider stage recorder: {error}"))?;
        if observed_stages.as_slice() != expected_stages {
            return Err(format!("ingest window stage order differs: {observed_stages:?}").into());
        }
        drop(observed_stages);
        let tally = derived.source.tally();
        if tally.windows != 3 || tally.replace_scopes != 5 || tally.rows != 5 {
            return Err(format!("tally counts windows, scopes and rows: {tally:?}").into());
        }
        if residency.peak_windows() != 1 {
            return Err(format!("peak windows out at once: {}", residency.peak_windows()).into());
        }
        let vector_bytes =
            SemanticStreamWindowPolicy::vector_bytes(2, SEARCH_OWNED_SEMANTIC_DIMENSION)?;
        if tally.peak_vector_bytes != vector_bytes || residency.peak_vector_bytes() != vector_bytes
        {
            return Err(format!(
                "peak resident bytes is one two-row window: tally={} residency={} expected={vector_bytes}",
                tally.peak_vector_bytes,
                residency.peak_vector_bytes()
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn failed_ingest_provider_window_has_return_marker_without_a_successful_window() -> TestRes {
        let embedder = RecordingEmbedder::failing();
        let batch = typed_batch_a2_b1_c2()?;
        let stages = std::sync::Arc::new(RecordingProviderStages::default());
        let budget = RequestBudgetV1::unbounded().with_diagnostics(stages.clone());
        let mut derived = derive_semantic_stream_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticStreamWindowPolicy::DEFAULT,
            None,
            &budget,
        )?;
        match derived.source.next_window() {
            Err(CoreError::Storage(message)) if message == "scripted ingest provider failure" => {}
            other => return Err(format!("provider failure must propagate: {other:?}").into()),
        }
        if embedder.calls()?.len() != 1 || derived.source.tally().windows != 0 {
            return Err("failed call cannot issue a successful semantic window".into());
        }
        let recorded = stages
            .0
            .lock()
            .map_err(|error| format!("provider stage recorder: {error}"))?;
        if recorded.as_slice()
            != [
                RequestProviderStageV1::IngestWindowStarted { window_ordinal: 1 },
                RequestProviderStageV1::IngestWindowReturned { window_ordinal: 1 },
            ]
        {
            return Err(format!("failed provider stage pair differs: {recorded:?}").into());
        }
        drop(recorded);
        Ok(())
    }

    // CASE-COVERS: the byte ceiling cuts windows too, and an owner scope
    // whose vectors alone exceed it is refused typed before any provider
    // call, because no window can carry it and it must not be split.
    #[test]
    fn a_window_is_cut_by_vector_bytes_and_an_owner_over_the_bound_is_refused() -> TestRes {
        let embedder = RecordingEmbedder::new();
        let batch = typed_batch_a2_b1_c2()?;
        let two_rows =
            SemanticStreamWindowPolicy::vector_bytes(2, SEARCH_OWNED_SEMANTIC_DIMENSION)?;
        let policy = SemanticStreamWindowPolicy::new(SEMANTIC_STREAM_WINDOW_SCOPES, two_rows)?;
        let mut derived = derive_semantic_stream_from_semantic_sources_v1(
            &batch,
            &embedder,
            policy,
            None,
            &RequestBudgetV1::unbounded(),
        )?;
        let mut rows_per_window = Vec::new();
        while let Some(window) = derived.source.next_window()? {
            rows_per_window.push(window.rows()?);
            if window.vector_bytes() > two_rows {
                return Err("no window exceeds the byte ceiling".into());
            }
            drop(window);
        }
        if rows_per_window != [2, 2, 1] {
            return Err(format!("byte ceiling cuts at two rows: {rows_per_window:?}").into());
        }

        // One semantic owner with two records under a one-row ceiling.
        let mut two_record_owner = fixture_search_batch()?;
        let scope = two_record_owner
            .semantic_replace_scopes
            .first_mut()
            .ok_or("fixture carries one semantic scope")?;
        let mut second = fixture_semantic_source();
        second.record_id = "source-record-2".to_string();
        scope.sources.push(second);
        let one_row = SemanticStreamWindowPolicy::vector_bytes(1, SEARCH_OWNED_SEMANTIC_DIMENSION)?;
        let policy = SemanticStreamWindowPolicy::new(SEMANTIC_STREAM_WINDOW_SCOPES, one_row)?;
        let embedder = RecordingEmbedder::new();
        let mut derived = derive_semantic_stream_from_semantic_sources_v1(
            &two_record_owner,
            &embedder,
            policy,
            None,
            &RequestBudgetV1::unbounded(),
        )?;
        match derived.source.next_window() {
            Err(CoreError::Typed { code, .. })
                if code == SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE => {}
            other => {
                return Err(format!(
                    "an owner over the byte ceiling is refused typed, got {other:?}"
                )
                .into());
            }
        }
        if !embedder.calls()?.is_empty() {
            return Err("a refused owner costs no provider call".into());
        }
        Ok(())
    }

    // CASE-COVERS: a source refuses to issue a second window while the
    // first is still resident, and issues it once the first is dropped.
    #[test]
    fn a_second_window_is_refused_while_the_first_is_resident() -> TestRes {
        let embedder = RecordingEmbedder::new();
        let batch = typed_batch_a2_b1_c2()?;
        let policy = SemanticStreamWindowPolicy::new(1, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
        let mut derived = derive_semantic_stream_from_semantic_sources_v1(
            &batch,
            &embedder,
            policy,
            None,
            &RequestBudgetV1::unbounded(),
        )?;
        let first = derived
            .source
            .next_window()?
            .ok_or("five owners issue a first window")?;
        match derived.source.next_window() {
            Err(CoreError::Typed { code, .. })
                if code == SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE => {}
            other => {
                return Err(format!(
                    "a second window while one is resident is refused typed, got {other:?}"
                )
                .into());
            }
        }
        drop(first);
        let second = derived.source.next_window()?;
        if second.is_none() {
            return Err("the next window is issued once the first is dropped".into());
        }
        if embedder.calls()?.len() != 2 {
            return Err("the refused request cost no provider call".into());
        }
        Ok(())
    }

    // CASE-COVERS: the window policy is invisible in what a batch derives
    // to. The same batch streamed one owner at a time and under the
    // production window drains to identical replace scopes, embeddings and digests.
    #[test]
    fn windowing_leaves_no_trace_in_the_derived_batch() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let one_owner = SemanticStreamWindowPolicy::new(1, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
        let batch = typed_batch_a2_b1_c2()?;
        {
            let streamed =
                drain_semantic_stream_v1(derive_semantic_stream_from_semantic_sources_v1(
                    &batch,
                    &embedder,
                    one_owner,
                    None,
                    &RequestBudgetV1::unbounded(),
                )?)?;
            let whole = drain_semantic_stream_v1(derive_semantic_stream_from_semantic_sources_v1(
                &batch,
                &embedder,
                SemanticStreamWindowPolicy::DEFAULT,
                None,
                &RequestBudgetV1::unbounded(),
            )?)?;
            if ids_of(&streamed.replace_scopes) != ids_of(&whole.replace_scopes)
                || streamed
                    .replace_scopes
                    .iter()
                    .flat_map(|scope| scope.embeddings.iter())
                    .ne(whole
                        .replace_scopes
                        .iter()
                        .flat_map(|scope| scope.embeddings.iter()))
            {
                return Err("windowing changed the derived rows".into());
            }
            let mut streamed_header = streamed;
            streamed_header.replace_scopes.clear();
            let mut whole_header = whole;
            whole_header.replace_scopes.clear();
            if streamed_header != whole_header {
                return Err("windowing changed the derived header".into());
            }
        }
        Ok(())
    }

    // CASE-COVERS: a malformed source record is refused before the first
    // window, so the provider is never called for it.
    #[test]
    fn an_invalid_source_costs_no_provider_call() -> TestRes {
        let embedder = RecordingEmbedder::new();
        let stages = std::sync::Arc::new(RecordingProviderStages::default());
        let budget = RequestBudgetV1::unbounded().with_diagnostics(stages.clone());
        let mut batch = fixture_search_batch()?;
        let source = batch
            .semantic_replace_scopes
            .first_mut()
            .and_then(|scope| scope.sources.first_mut())
            .ok_or_else(|| "semantic fixture must contain one source".to_string())?;
        source.source_role = SourceRoleV1::DocumentText;
        match derive_semantic_stream_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticStreamWindowPolicy::DEFAULT,
            None,
            &budget,
        ) {
            Err(CoreError::InvalidContract(message)) if message.contains("CardText") => {}
            Err(other) => {
                return Err(
                    format!("invalid semantic source must fail closed, got {other:?}").into(),
                );
            }
            Ok(_derived) => {
                return Err("invalid semantic source must fail closed, got a stream".into());
            }
        }
        if !embedder.calls()?.is_empty() {
            return Err("a refused batch costs no provider call".into());
        }
        if !stages
            .0
            .lock()
            .map_err(|error| format!("provider stage recorder: {error}"))?
            .is_empty()
        {
            return Err("a refused batch must not emit provider stages".into());
        }
        Ok(())
    }

    // S21-08: source content without an explicit external grant is refused
    // before the first window, so the provider is never called for it; a
    // consented grant admits the batch.
    #[test]
    fn source_content_without_consent_costs_no_provider_call() -> TestRes {
        let embedder = RecordingEmbedder::new();
        let batch = fixture_search_batch()?;
        let denied = SemanticEgressPolicyV1::Denied;
        match derive_semantic_stream_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticStreamWindowPolicy::DEFAULT,
            Some(&denied),
            &RequestBudgetV1::unbounded(),
        ) {
            Err(CoreError::Typed { code, .. })
                if code.as_wire_str() == "PROVIDER_EGRESS_DENIED" => {}
            Err(other) => {
                return Err(
                    format!("a denied source batch must fail closed, got {other:?}").into(),
                );
            }
            Ok(_derived) => {
                return Err("a denied source batch must fail closed, got a stream".into());
            }
        }
        if !embedder.calls()?.is_empty() {
            return Err("a refused batch costs no provider call".into());
        }

        let grant = SemanticEgressGrantV1 {
            tenant_id: "unit-test-tenant".to_string(),
            provider_id: "openai".to_string(),
            endpoint: "https://unit.test/v1".to_string(),
            region: "unit-test-region".to_string(),
            retention: "unit-test-30d".to_string(),
            model_id: "text-embedding-3-small".to_string(),
            model_revision: "unit-test".to_string(),
            profile: "unit-test-release".to_string(),
            source_content_consent: true,
        };
        let policy = SemanticEgressPolicyV1::External(grant);
        let _derived = derive_semantic_stream_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticStreamWindowPolicy::DEFAULT,
            Some(&policy),
            &RequestBudgetV1::unbounded(),
        )
        .map_err(|err| format!("a consented batch must derive: {err:?}"))?;
        Ok(())
    }
}
