//! Search-owned semantic batch derivation.
//!
//! Turns an accepted `SearchCorpusIngestBatch` into a streamed semantic
//! batch: a [`SemanticIngestHeaderV1`] the build knows up front, and a
//! [`DerivedSemanticScopeSource`] that embeds the batch's records one
//! bounded window at a time as the build asks for them (QI-BB-021). The
//! derivation can stay on the legacy chunk-text path or migrate to typed
//! semantic sources, but the embedder identity is always pinned into the
//! header's `EmbeddingModelContract`. This is the ingest counterpart to the
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
//! [`derive_semantic_stream_with_mode_v1`].

use std::collections::{BTreeSet, VecDeque};

use quanta_index_contract::{
    CapabilityStatusV1, ChunkRecord, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
    EmbeddingNormalization, EmbeddingRecord, GenerationPin, OwnerDocKind, SearchCorpusIngestBatch,
    SearchScopeKey, SearchScopeSurface, SemanticCorpusKindV1, SemanticReplaceScope,
    SemanticSourceRecordV1, SemanticSourceScopeKeyV1, SemanticTombstoneScope, SourceRoleV1,
    canonical_order::first_canonical_order_break_v1, lex::LanguageCode, lex::SymbolKindCode,
    validate_semantic_source_record_v1,
};
use quanta_index_core::{
    CoreError, SemanticBatchIdentityV1, SemanticBatchMutationsV1, SemanticGenerationContractV1,
    SemanticIngestHeaderV1, SemanticScopeSource, SemanticScopeWindowV1, SemanticStreamTallyV1,
    SemanticStreamWindowPolicy, SemanticWindowFillV1, SemanticWindowIssuerV1,
    SemanticWindowPlacementV1, TextEmbeddingProvider,
};
use sha2::{Digest, Sha256};

const QUANTA_INDEX_SEMANTIC_DERIVE_MODE_ENV: &str = "QUANTA_INDEX_SEMANTIC_DERIVE_MODE";
const LEGACY_CHUNK_POLICY_DIGEST: &str = "chunk.text";
const SEMANTIC_SOURCE_POLICY_DIGEST: &str = "semantic-source.v1";
const SEMANTIC_SOURCE_VIEW_POLICY_DIGEST: &str = "semantic-source.v1";
const SEMANTIC_SOURCE_FALLBACK_VIEW_POLICY_DIGEST: &str =
    "semantic-source.v1:legacy-fallback:empty-semantic-replace-scopes";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticDerivationModeV1 {
    /// Legacy: embed every ChunkRecord.text (pre-cutover)
    LegacyAllChunkText,
    /// Prefer `semantic_replace_scopes`; if empty and migration allows, fall back to legacy with degraded reason
    SemanticSourcesWithLegacyFallback,
    /// Require semantic sources; empty sources fail closed (card-required path later)
    SemanticSourcesOnly,
}

pub(crate) const DEFAULT_SEMANTIC_DERIVATION_MODE_V1: SemanticDerivationModeV1 =
    SemanticDerivationModeV1::SemanticSourcesWithLegacyFallback;

impl SemanticDerivationModeV1 {
    #[must_use]
    pub(crate) fn from_env_value_v1(value: &str) -> Option<Self> {
        match value {
            "legacy_all_chunk" => Some(Self::LegacyAllChunkText),
            "semantic_with_legacy_fallback" => Some(Self::SemanticSourcesWithLegacyFallback),
            "semantic_only" => Some(Self::SemanticSourcesOnly),
            _ => None,
        }
    }
}

pub(crate) fn semantic_derivation_mode_from_env_v1() -> Result<SemanticDerivationModeV1, CoreError>
{
    match std::env::var(QUANTA_INDEX_SEMANTIC_DERIVE_MODE_ENV) {
        Ok(value) => SemanticDerivationModeV1::from_env_value_v1(value.as_str()).ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "semantic derivation: unknown {QUANTA_INDEX_SEMANTIC_DERIVE_MODE_ENV}={value:?}; \
                 expected legacy_all_chunk | semantic_with_legacy_fallback | semantic_only"
            ))
        }),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_SEMANTIC_DERIVATION_MODE_V1),
        Err(std::env::VarError::NotUnicode(_value)) => Err(CoreError::InvalidContract(format!(
            "semantic derivation: {QUANTA_INDEX_SEMANTIC_DERIVE_MODE_ENV} must be valid UTF-8"
        ))),
    }
}

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
enum PendingOwnerScope<'a> {
    /// One chunk of a legacy replace scope: each chunk is its own owner.
    LegacyChunk {
        scope_index: usize,
        scope: &'a quanta_index_contract::SearchCorpusReplaceScope,
        chunk: &'a ChunkRecord,
    },
    /// One validated semantic source scope: one owner and its records.
    SemanticSource(ValidatedSemanticSourceScope<'a>),
}

impl PendingOwnerScope<'_> {
    fn records(&self) -> usize {
        match self {
            Self::LegacyChunk { .. } => 1,
            Self::SemanticSource(scope) => scope.records.len(),
        }
    }

    fn texts<'s>(&'s self) -> Box<dyn Iterator<Item = &'s str> + 's> {
        match self {
            Self::LegacyChunk { chunk, .. } => {
                Box::new(std::iter::once(semantic_embedding_input_text(chunk)))
            }
            Self::SemanticSource(scope) => {
                Box::new(scope.records.iter().map(|record| record.text.as_str()))
            }
        }
    }
}

/// The scope-at-a-time source of one derived batch.
///
/// Holds the producer's records borrowed and un-embedded, planned into
/// owner scopes in canonical order; each `next_window` takes as many owner
/// scopes as the window policy admits, embeds their texts in one provider
/// call, and issues the embedded replace scopes as one leased window. A
/// legacy path scope whose chunks span windows is issued as one replace
/// scope per window; a semantic source scope is one owner and never splits.
pub(crate) struct DerivedSemanticScopeSource<'a> {
    embedder: &'a dyn TextEmbeddingProvider,
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
    ) -> Result<Self, CoreError> {
        let dimension = usize::try_from(model_contract.dimension).map_err(|err| {
            CoreError::InvalidContract(format!(
                "semantic derivation: model contract dimension overflow: {err}"
            ))
        })?;
        Ok(Self {
            embedder,
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
        &self,
        owners: Vec<PendingOwnerScope<'a>>,
    ) -> Result<Vec<SemanticReplaceScope>, CoreError> {
        let texts: Vec<&str> = owners.iter().flat_map(PendingOwnerScope::texts).collect();
        let all_vectors = self.embedder.embed_batch(&texts)?;
        if all_vectors.len() != texts.len() {
            return Err(CoreError::InvalidContract(format!(
                "semantic derivation: embedder returned {} vectors for {} window texts",
                all_vectors.len(),
                texts.len()
            )));
        }
        let mut vectors = all_vectors.into_iter();
        let mut scopes: Vec<SemanticReplaceScope> = Vec::new();
        let mut open_legacy_scope: Option<usize> = None;
        for owner in owners {
            match owner {
                PendingOwnerScope::LegacyChunk {
                    scope_index,
                    scope,
                    chunk,
                } => {
                    let vector = vectors.next().ok_or_else(|| {
                        CoreError::InvalidContract(
                            "semantic derivation: ran out of embedding vectors while \
                             redistributing the window's embed result across scopes"
                                .to_string(),
                        )
                    })?;
                    let record = embedding_record_for(
                        chunk,
                        semantic_embedding_input_text(chunk),
                        vector,
                        &self.model_contract,
                    )?;
                    if open_legacy_scope != Some(scope_index) {
                        scopes.push(SemanticReplaceScope {
                            scope: scope.scope.clone(),
                            scope_digest: scope.scope_digest.clone(),
                            embeddings: Vec::new(),
                            cluster_memberships: Vec::new(),
                        });
                        open_legacy_scope = Some(scope_index);
                    }
                    let fragment = scopes.last_mut().ok_or_else(|| {
                        CoreError::InvalidContract(
                            "semantic derivation: legacy window fragment vanished".to_string(),
                        )
                    })?;
                    fragment.embeddings.push(record);
                }
                PendingOwnerScope::SemanticSource(validated) => {
                    open_legacy_scope = None;
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
                            embedding_record_for_semantic_source(
                                record,
                                vector,
                                &self.model_contract,
                            )
                        })
                        .collect::<Result<Vec<_>, CoreError>>()?;
                    scopes.push(SemanticReplaceScope {
                        scope: validated.scope,
                        scope_digest: validated.scope_digest,
                        embeddings,
                        cluster_memberships: validated.cluster_memberships,
                    });
                }
            }
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

pub(crate) fn derive_semantic_stream_with_mode_v1<'a>(
    batch: &'a SearchCorpusIngestBatch,
    embedder: &'a dyn TextEmbeddingProvider,
    mode: SemanticDerivationModeV1,
    policy: SemanticStreamWindowPolicy,
) -> Result<DerivedSemanticStreamV1<'a>, CoreError> {
    match mode {
        SemanticDerivationModeV1::LegacyAllChunkText => {
            derive_semantic_stream_from_search_corpus_batch(batch, embedder, policy)
        }
        SemanticDerivationModeV1::SemanticSourcesWithLegacyFallback
        | SemanticDerivationModeV1::SemanticSourcesOnly => {
            derive_semantic_stream_from_semantic_sources_v1(batch, embedder, mode, policy)
        }
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

pub(crate) fn derive_semantic_stream_from_search_corpus_batch<'a>(
    batch: &'a SearchCorpusIngestBatch,
    embedder: &'a dyn TextEmbeddingProvider,
    policy: SemanticStreamWindowPolicy,
) -> Result<DerivedSemanticStreamV1<'a>, CoreError> {
    batch
        .validate_surface_mutations_v1()
        .map_err(|err| CoreError::InvalidContract(format!("semantic derivation: {err}")))?;
    if !batch.semantic_replace_scopes.is_empty() || !batch.semantic_tombstone_scopes.is_empty() {
        return Err(CoreError::InvalidContract(
            "semantic derivation: legacy_all_chunk cannot consume typed semantic source mutations; select semantic_with_legacy_fallback or semantic_only"
                .to_string(),
        ));
    }
    require_embedder_dimension(embedder)?;
    let model_contract = embedding_model_contract_for(embedder, LEGACY_CHUNK_POLICY_DIGEST, None)?;
    let header = semantic_header_v1(
        batch,
        format!("{}:semantic-derive", batch.batch_digest),
        model_contract,
        vec![SemanticCorpusKindV1::RawCodeFallback],
        None,
    );
    // Every chunk is its own owner scope, in producer order: a path whose
    // chunks do not fit one window is issued as one replace scope per
    // window, which the build handles exactly as one, because it deletes
    // and appends by owner.
    let pending = batch
        .replace_scopes
        .iter()
        .enumerate()
        .flat_map(|(scope_index, scope)| {
            scope
                .chunks
                .iter()
                .map(move |chunk| PendingOwnerScope::LegacyChunk {
                    scope_index,
                    scope,
                    chunk,
                })
        })
        .collect();
    let source = DerivedSemanticScopeSource::new(
        embedder,
        header.contract.model_contract.clone(),
        policy,
        pending,
    )?;
    Ok(DerivedSemanticStreamV1 { header, source })
}

pub(crate) fn derive_semantic_stream_from_semantic_sources_v1<'a>(
    batch: &'a SearchCorpusIngestBatch,
    embedder: &'a dyn TextEmbeddingProvider,
    mode: SemanticDerivationModeV1,
    policy: SemanticStreamWindowPolicy,
) -> Result<DerivedSemanticStreamV1<'a>, CoreError> {
    batch
        .validate_surface_mutations_v1()
        .map_err(|err| CoreError::InvalidContract(format!("semantic derivation: {err}")))?;
    require_embedder_dimension(embedder)?;
    let has_semantic_lifecycle_operation = !batch.semantic_tombstone_scopes.is_empty()
        || !batch.tombstone_scopes.is_empty()
        || !batch.clear_surfaces.is_empty()
        || batch.seal;
    let requires_legacy_replacement_fallback = batch.semantic_replace_scopes.is_empty()
        && (!batch.replace_scopes.is_empty() || !has_semantic_lifecycle_operation);
    if requires_legacy_replacement_fallback {
        return match mode {
            SemanticDerivationModeV1::SemanticSourcesWithLegacyFallback => {
                if !batch.semantic_tombstone_scopes.is_empty() {
                    return Err(CoreError::InvalidContract(
                        "semantic derivation: semantic_with_legacy_fallback cannot combine legacy chunk replacements with typed semantic tombstones"
                            .to_string(),
                    ));
                }
                // The fallback marker describes how these embeddings were
                // rendered, not a different generation-wide corpus policy.
                // Keeping the corpus policy stable lets a later mutation-free
                // seal merge with the generation contract created by this
                // replacement batch. The digests every record carries bind
                // the model identity and dimension, which both contracts
                // share, so the rendered rows are the same under either.
                let model_contract = embedding_model_contract_for(
                    embedder,
                    SEMANTIC_SOURCE_POLICY_DIGEST,
                    Some(SEMANTIC_SOURCE_FALLBACK_VIEW_POLICY_DIGEST),
                )?;
                let mut legacy =
                    derive_semantic_stream_from_search_corpus_batch(batch, embedder, policy)?;
                legacy.header.batch.batch_digest = format!(
                    "{}:semantic-derive:legacy-fallback-empty-semantic-sources",
                    batch.batch_digest
                );
                legacy.header.contract.model_contract = model_contract.clone();
                legacy.header.contract.corpus_policy_digest =
                    Some(SEMANTIC_SOURCE_POLICY_DIGEST.to_string());
                legacy.source.model_contract = model_contract;
                Ok(legacy)
            }
            SemanticDerivationModeV1::SemanticSourcesOnly => Err(CoreError::InvalidContract(
                "semantic derivation: semantic sources required in semantic_only mode".to_string(),
            )),
            SemanticDerivationModeV1::LegacyAllChunkText => {
                derive_semantic_stream_from_search_corpus_batch(batch, embedder, policy)
            }
        };
    }
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
        .map(PendingOwnerScope::SemanticSource)
        .collect();
    let source = DerivedSemanticScopeSource::new(
        embedder,
        header.contract.model_contract.clone(),
        policy,
        pending,
    )?;
    Ok(DerivedSemanticStreamV1 { header, source })
}

fn embedding_record_for(
    chunk: &quanta_index_contract::ChunkRecord,
    embedding_input_text: &str,
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
            "semantic derivation: embedder returned dim {} for chunk {}, expected {}",
            vector.len(),
            chunk.chunk_id.as_str(),
            model_contract.dimension
        )));
    }
    Ok(EmbeddingRecord {
        embedding_id: EmbeddingId::new(chunk.chunk_id.as_str()),
        record_id: chunk.chunk_id.as_str().to_string().into_boxed_str(),
        owner_kind: OwnerDocKind::Chunk,
        owner_id: chunk.chunk_id.as_str().to_string().into_boxed_str(),
        corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
        parent_owner_id: None,
        source_doc_id: chunk.chunk_id.as_str().to_string().into_boxed_str(),
        repo_relative_path: chunk.repo_relative_path.clone(),
        language: chunk.language.clone(),
        package: None,
        symbol_kind: None,
        visibility: None,
        source_role: SourceRoleV1::RawFallbackText,
        generated: false,
        capability_status: CapabilityStatusV1::Degraded,
        authority_digest: "search-owned:legacy-chunk-text"
            .to_string()
            .into_boxed_str(),
        render_policy_digest: "search-owned:legacy-chunk-text"
            .to_string()
            .into_boxed_str(),
        card_schema_version: 0,
        start_byte: chunk.start_byte,
        end_byte: chunk.end_byte,
        start_line: chunk.start_line,
        end_line: chunk.end_line,
        snippet: chunk.derived_snippet().to_string().into_boxed_str(),
        embedding_input_digest: semantic_embedding_input_digest(
            model_contract,
            "chunk.text",
            embedding_input_text,
        )
        .into_boxed_str(),
        vector_digest: semantic_vector_digest(model_contract, &vector).into_boxed_str(),
        view_kind: "chunk.text".to_string().into_boxed_str(),
        vector,
    })
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

pub(crate) fn semantic_embedding_input_text(chunk: &quanta_index_contract::ChunkRecord) -> &str {
    chunk.text.as_ref()
}

pub(crate) fn semantic_embedding_input_digest(
    model_contract: &EmbeddingModelContract,
    view_kind: &str,
    embedding_input_text: &str,
) -> String {
    let digest = sha256_hex(&[
        model_contract.model_id.as_bytes(),
        model_contract
            .model_version
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
        &model_contract.dimension.to_le_bytes(),
        view_kind.as_bytes(),
        embedding_input_text.as_bytes(),
    ]);
    format!("search-owned-in:sha256:{digest}")
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
        .tombstone_scopes
        .iter()
        .map(|scope| SemanticTombstoneScope {
            scope: Some(scope.scope.clone()),
            semantic_scope: None,
        })
        .chain(
            batch
                .semantic_tombstone_scopes
                .iter()
                .cloned()
                .map(|scope| SemanticTombstoneScope {
                    scope: None,
                    semantic_scope: Some(scope),
                }),
        )
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
    use quanta_index_core::{
        SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE, SEMANTIC_STREAM_WINDOW_SCOPES,
        SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES,
    };

    /// Derive under the production window and drain the stream into one
    /// batch, as the all-at-once derivation used to return.
    fn derive_semantic_batch_from_semantic_sources_v1(
        batch: &SearchCorpusIngestBatch,
        embedder: &dyn TextEmbeddingProvider,
        mode: SemanticDerivationModeV1,
    ) -> Result<quanta_index_contract::SemanticIngestBatch, CoreError> {
        drain_semantic_stream_v1(derive_semantic_stream_from_semantic_sources_v1(
            batch,
            embedder,
            mode,
            SemanticStreamWindowPolicy::DEFAULT,
        )?)
    }

    fn derive_semantic_batch_with_mode_v1(
        batch: &SearchCorpusIngestBatch,
        embedder: &dyn TextEmbeddingProvider,
        mode: SemanticDerivationModeV1,
    ) -> Result<quanta_index_contract::SemanticIngestBatch, CoreError> {
        drain_semantic_stream_v1(derive_semantic_stream_with_mode_v1(
            batch,
            embedder,
            mode,
            SemanticStreamWindowPolicy::DEFAULT,
        )?)
    }
    use quanta_index_contract::{
        BatchIngestMode, ChunkId, ChunkRecord, ClusterMembershipReplaceV1, ManifestGeneration,
        RepoId, RepoRelativePath, RevisionId, SearchCorpusReplaceScope, SearchScopeSurface,
        SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, SymbolId, lex::LanguageCode,
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
            text: "legacy chunk body".to_string().into_boxed_str(),
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
        let first = derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        )?;
        let second = derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        )?;
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
    fn semantic_derivation_semantic_only_empty_sources_fail() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.semantic_replace_scopes.clear();
        batch.seal = false;
        match derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        ) {
            Err(CoreError::InvalidContract(message))
                if message.contains("semantic sources required") =>
            {
                Ok(())
            }
            other => Err(format!("semantic_only must fail on empty sources, got {other:?}").into()),
        }
    }

    #[test]
    fn semantic_derivation_semantic_only_tombstone_batch_does_not_require_replace_sources()
    -> TestRes {
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

        let derived = derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        )?;

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
    fn semantic_derivation_semantic_only_seal_batch_does_not_require_replace_sources() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.replace_scopes.clear();
        batch.semantic_replace_scopes.clear();
        batch.semantic_tombstone_scopes.clear();
        batch.seal = true;

        let derived = derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        )?;

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
    fn semantic_derivation_semantic_only_clear_batch_does_not_require_replace_sources() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.mode = BatchIngestMode::Delta;
        batch.base_generation = Some(ManifestGeneration::new(6));
        batch.replace_scopes.clear();
        batch.semantic_replace_scopes.clear();
        batch.semantic_tombstone_scopes.clear();
        batch.clear_surfaces = vec![SearchScopeSurface::Chunk];
        batch.seal = false;

        let derived = derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        )?;

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
    fn semantic_derivation_legacy_mode_still_embeds_chunks() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.semantic_replace_scopes.clear();
        let derived = derive_semantic_batch_with_mode_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::LegacyAllChunkText,
        )?;
        let embedding = derived
            .replace_scopes
            .first()
            .and_then(|scope| scope.embeddings.first())
            .ok_or_else(|| "expected one legacy semantic embedding".to_string())?;
        if embedding.embedding_id.as_str() != "chunk-1"
            || embedding.view_kind.as_ref() != "chunk.text"
        {
            return Err(format!(
                "legacy mode must still embed chunks, got id={} view_kind={}",
                embedding.embedding_id.as_str(),
                embedding.view_kind
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_production_default_prefers_typed_semantic_sources_v1() -> TestRes {
        if DEFAULT_SEMANTIC_DERIVATION_MODE_V1
            != SemanticDerivationModeV1::SemanticSourcesWithLegacyFallback
        {
            return Err("production default must prefer typed semantic sources".into());
        }
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let derived = derive_semantic_batch_with_mode_v1(
            &fixture_search_batch()?,
            &embedder,
            DEFAULT_SEMANTIC_DERIVATION_MODE_V1,
        )?;
        let embedding = derived
            .replace_scopes
            .first()
            .and_then(|scope| scope.embeddings.first())
            .ok_or_else(|| "expected one typed semantic embedding".to_string())?;
        if embedding.embedding_id.as_str() != "source-record-1"
            || embedding.view_kind.as_ref() != "symbol.card"
        {
            return Err(format!(
                "production default silently selected legacy chunk derivation: id={} view_kind={}",
                embedding.embedding_id.as_str(),
                embedding.view_kind
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_production_default_sealed_legacy_batch_falls_back_v1() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.semantic_replace_scopes.clear();
        if !batch.seal {
            return Err("fixture must exercise a sealed legacy replacement batch".into());
        }
        let derived = derive_semantic_batch_with_mode_v1(
            &batch,
            &embedder,
            DEFAULT_SEMANTIC_DERIVATION_MODE_V1,
        )?;
        let embedding = derived
            .replace_scopes
            .first()
            .and_then(|scope| scope.embeddings.first())
            .ok_or_else(|| "sealed legacy replacement must not lose its embedding".to_string())?;
        if embedding.embedding_id.as_str() != "chunk-1"
            || embedding.view_kind.as_ref() != "chunk.text"
        {
            return Err(format!(
                "sealed legacy replacement did not use explicit fallback: id={} view_kind={}",
                embedding.embedding_id.as_str(),
                embedding.view_kind
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_fallback_replacement_and_empty_seal_share_generation_policy_v1()
    -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut replacement = fixture_search_batch()?;
        replacement.semantic_replace_scopes.clear();
        replacement.seal = false;
        let replacement_derived = derive_semantic_batch_with_mode_v1(
            &replacement,
            &embedder,
            DEFAULT_SEMANTIC_DERIVATION_MODE_V1,
        )?;

        let mut seal = replacement;
        seal.replace_scopes.clear();
        seal.seal = true;
        let seal_derived = derive_semantic_batch_with_mode_v1(
            &seal,
            &embedder,
            DEFAULT_SEMANTIC_DERIVATION_MODE_V1,
        )?;

        if replacement_derived.corpus_policy_digest != seal_derived.corpus_policy_digest {
            return Err(format!(
                "fallback replacement and empty seal must preserve one generation policy: replacement={:?} seal={:?}",
                replacement_derived.corpus_policy_digest, seal_derived.corpus_policy_digest
            )
            .into());
        }
        if replacement_derived
            .model_contract
            .view_policy_digest
            .as_deref()
            != Some(SEMANTIC_SOURCE_FALLBACK_VIEW_POLICY_DIGEST)
        {
            return Err(
                "fallback replacement must retain its distinct embedding view policy".into(),
            );
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_semantic_only_sealed_legacy_replacement_fails_v1() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.semantic_replace_scopes.clear();
        match derive_semantic_batch_with_mode_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        ) {
            Err(CoreError::InvalidContract(message))
                if message.contains("semantic sources required") =>
            {
                Ok(())
            }
            other => Err(format!(
                "semantic_only must not discard a sealed legacy replacement, got {other:?}"
            )
            .into()),
        }
    }

    #[test]
    fn semantic_derivation_explicit_legacy_rejects_typed_semantic_replacement_v1() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        match derive_semantic_batch_with_mode_v1(
            &fixture_search_batch()?,
            &embedder,
            SemanticDerivationModeV1::LegacyAllChunkText,
        ) {
            Err(CoreError::InvalidContract(message))
                if message.contains("cannot consume typed semantic source mutations") =>
            {
                Ok(())
            }
            other => Err(format!(
                "legacy mode must reject typed semantic replacement instead of discarding it, got {other:?}"
            )
            .into()),
        }
    }

    #[test]
    fn semantic_derivation_explicit_legacy_rejects_typed_semantic_tombstone_v1() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        batch.semantic_replace_scopes.clear();
        batch
            .semantic_tombstone_scopes
            .push(SemanticSourceScopeKeyV1 {
                corpus_kind: SemanticCorpusKindV1::ClusterCard,
                owner_kind: OwnerDocKind::OwnerMap,
                owner_id: "cluster-deleted".to_string(),
            });
        match derive_semantic_batch_with_mode_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::LegacyAllChunkText,
        ) {
            Err(CoreError::InvalidContract(message))
                if message.contains("cannot consume typed semantic source mutations") =>
            {
                Ok(())
            }
            other => Err(format!(
                "legacy mode must reject typed semantic tombstone instead of discarding it, got {other:?}"
            )
            .into()),
        }
    }

    #[test]
    fn semantic_derivation_explicit_legacy_rejects_structured_cluster_membership_v1() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let mut batch = fixture_search_batch()?;
        let scope = batch
            .semantic_replace_scopes
            .first_mut()
            .ok_or_else(|| "semantic fixture must contain one replace scope".to_string())?;
        scope.scope.corpus_kind = SemanticCorpusKindV1::ClusterCard;
        scope.scope.owner_kind = OwnerDocKind::OwnerMap;
        scope.scope.owner_id = "cluster-1".to_string();
        let source = scope
            .sources
            .first_mut()
            .ok_or_else(|| "semantic fixture must contain one source".to_string())?;
        source.record_id = "cluster-record-1".to_string();
        source.corpus_kind = SemanticCorpusKindV1::ClusterCard;
        source.owner_kind = OwnerDocKind::OwnerMap;
        source.owner_id = "cluster-1".to_string();
        source.symbol_kind = None;
        source.visibility = None;
        source.authority_digest = "cluster-authority:sha256:1".to_string();
        let cluster_record_id = source.record_id.clone();
        let authority_digest = source.authority_digest.clone();
        scope.cluster_memberships = vec![ClusterMembershipReplaceV1 {
            cluster_record_id,
            authority_digest,
            members: vec![SymbolId::new("symbol:a"), SymbolId::new("symbol:b")],
        }];

        match derive_semantic_batch_with_mode_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::LegacyAllChunkText,
        ) {
            Err(CoreError::InvalidContract(message))
                if message.contains("cannot consume typed semantic source mutations") =>
            {
                Ok(())
            }
            other => Err(format!(
                "legacy mode must reject structured cluster membership instead of discarding it, got {other:?}"
            )
            .into()),
        }
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
        match derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        ) {
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
        let derived = derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        )?;
        let tombstone = derived
            .tombstone_scopes
            .first()
            .ok_or_else(|| "expected semantic owner tombstone".to_string())?;
        if tombstone.scope.is_some() {
            return Err("semantic owner tombstone must not synthesize a legacy path scope".into());
        }
        if tombstone.semantic_scope.as_ref() != batch.semantic_tombstone_scopes.first() {
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
        match derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        ) {
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
        match derive_semantic_batch_from_semantic_sources_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        ) {
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

        let forward_derived = derive_semantic_batch_from_semantic_sources_v1(
            &forward,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        )?;
        let reverse_derived = derive_semantic_batch_from_semantic_sources_v1(
            &reverse,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
        )?;
        if forward_derived != reverse_derived {
            return Err("semantic derivation must canonicalize producer scope order".into());
        }
        Ok(())
    }

    #[test]
    fn semantic_derivation_unknown_mode_value_fails_closed() -> TestRes {
        if SemanticDerivationModeV1::from_env_value_v1("unknown-mode").is_some() {
            return Err("unknown derive mode must not parse".into());
        }
        Ok(())
    }
    // ---- QI-BB-021 follow-up #2: scope-streamed derivation ----

    /// A hashing embedder that records the texts of every provider call.
    struct RecordingEmbedder {
        inner: HashingQueryTextEmbedder,
        calls: std::sync::Mutex<Vec<Vec<String>>>,
    }

    impl RecordingEmbedder {
        fn new() -> Self {
            Self {
                inner: HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION),
                calls: std::sync::Mutex::new(Vec::new()),
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

    fn legacy_chunk(
        id: &str,
        path: &str,
        text: &str,
    ) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        let mut chunk = fixture_chunk()?;
        chunk.chunk_id = quanta_index_contract::ChunkId::new(id);
        chunk.repo_relative_path = RepoRelativePath::new(path);
        chunk.text = text.to_string().into_boxed_str();
        Ok(chunk)
    }

    fn legacy_scope(path: &str, chunks: Vec<ChunkRecord>) -> SearchCorpusReplaceScope {
        SearchCorpusReplaceScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                repo_relative_path: RepoRelativePath::new(path),
            },
            scope_digest: format!("scope:{path}"),
            chunks,
            symbols: Vec::new(),
        }
    }

    /// Three legacy files with 2, 1 and 2 chunks: five owner scopes.
    fn legacy_batch_a2_b1_c2() -> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
        let mut batch = fixture_search_batch()?;
        batch.semantic_replace_scopes.clear();
        batch.replace_scopes = vec![
            legacy_scope(
                "a.rs",
                vec![
                    legacy_chunk("a-1", "a.rs", "alpha one")?,
                    legacy_chunk("a-2", "a.rs", "alpha two")?,
                ],
            ),
            legacy_scope("b.rs", vec![legacy_chunk("b-1", "b.rs", "beta one")?]),
            legacy_scope(
                "c.rs",
                vec![
                    legacy_chunk("c-1", "c.rs", "gamma one")?,
                    legacy_chunk("c-2", "c.rs", "gamma two")?,
                ],
            ),
        ];
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

    // CASE-COVERS: under a two-owner window, five legacy chunks stream as
    // three windows, each embedded in ONE provider call carrying exactly the
    // window's texts; a path whose chunks span windows is issued as one
    // replace scope per window; the vectors keep producer order; the source
    // never had more than one window out.
    #[test]
    fn legacy_chunks_stream_in_policy_windows_and_a_path_spans_windows() -> TestRes {
        let embedder = RecordingEmbedder::new();
        let batch = legacy_batch_a2_b1_c2()?;
        let policy = SemanticStreamWindowPolicy::new(2, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
        let mut derived = derive_semantic_stream_with_mode_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::LegacyAllChunkText,
            policy,
        )?;
        if derived.source.pending_owner_scopes() != 5 {
            return Err("five chunks are five owner scopes".into());
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
            vec![("a.rs".into(), vec!["a-1".into(), "a-2".into()])],
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
        let tally = derived.source.tally();
        if tally.windows != 3 || tally.replace_scopes != 4 || tally.rows != 5 {
            return Err(format!("tally counts windows, fragments and rows: {tally:?}").into());
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

    // CASE-COVERS: the byte ceiling cuts windows too, and an owner scope
    // whose vectors alone exceed it is refused typed before any provider
    // call, because no window can carry it and it must not be split.
    #[test]
    fn a_window_is_cut_by_vector_bytes_and_an_owner_over_the_bound_is_refused() -> TestRes {
        let embedder = RecordingEmbedder::new();
        let batch = legacy_batch_a2_b1_c2()?;
        let two_rows =
            SemanticStreamWindowPolicy::vector_bytes(2, SEARCH_OWNED_SEMANTIC_DIMENSION)?;
        let policy = SemanticStreamWindowPolicy::new(SEMANTIC_STREAM_WINDOW_SCOPES, two_rows)?;
        let mut derived = derive_semantic_stream_with_mode_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::LegacyAllChunkText,
            policy,
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
        let mut derived = derive_semantic_stream_with_mode_v1(
            &two_record_owner,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
            policy,
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
        let batch = legacy_batch_a2_b1_c2()?;
        let policy = SemanticStreamWindowPolicy::new(1, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
        let mut derived = derive_semantic_stream_with_mode_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::LegacyAllChunkText,
            policy,
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
    // production window drains to identical replace scopes, embeddings and
    // digests, for legacy chunks and for typed semantic sources.
    #[test]
    fn windowing_leaves_no_trace_in_the_derived_batch() -> TestRes {
        let embedder = HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION);
        let one_owner = SemanticStreamWindowPolicy::new(1, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
        let legacy = legacy_batch_a2_b1_c2()?;
        let mut typed = fixture_search_batch()?;
        let mut second_scope = typed
            .semantic_replace_scopes
            .first()
            .cloned()
            .ok_or("fixture carries one semantic scope")?;
        second_scope.scope.owner_id = "symbol-2".to_string();
        second_scope.scope_digest = "scope:semantic:2".to_string();
        let record = second_scope
            .sources
            .first_mut()
            .ok_or("fixture scope carries one source")?;
        record.owner_id = "symbol-2".to_string();
        record.record_id = "source-record-2".to_string();
        typed.semantic_replace_scopes.push(second_scope);
        for (batch, mode) in [
            (&legacy, SemanticDerivationModeV1::LegacyAllChunkText),
            (&typed, SemanticDerivationModeV1::SemanticSourcesOnly),
        ] {
            let streamed = drain_semantic_stream_v1(derive_semantic_stream_with_mode_v1(
                batch, &embedder, mode, one_owner,
            )?)?;
            let whole = drain_semantic_stream_v1(derive_semantic_stream_with_mode_v1(
                batch,
                &embedder,
                mode,
                SemanticStreamWindowPolicy::DEFAULT,
            )?)?;
            // Fragments of one legacy path merge back into the path's rows;
            // compare rows, not fragment boundaries.
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
                return Err(format!("windowing changed the derived rows in {mode:?}").into());
            }
            let mut streamed_header = streamed;
            streamed_header.replace_scopes.clear();
            let mut whole_header = whole;
            whole_header.replace_scopes.clear();
            if streamed_header != whole_header {
                return Err(format!("windowing changed the derived header in {mode:?}").into());
            }
        }
        Ok(())
    }

    // CASE-COVERS: a malformed source record is refused before the first
    // window, so the provider is never called for it.
    #[test]
    fn an_invalid_source_costs_no_provider_call() -> TestRes {
        let embedder = RecordingEmbedder::new();
        let mut batch = fixture_search_batch()?;
        let source = batch
            .semantic_replace_scopes
            .first_mut()
            .and_then(|scope| scope.sources.first_mut())
            .ok_or_else(|| "semantic fixture must contain one source".to_string())?;
        source.source_role = SourceRoleV1::DocumentText;
        match derive_semantic_stream_with_mode_v1(
            &batch,
            &embedder,
            SemanticDerivationModeV1::SemanticSourcesOnly,
            SemanticStreamWindowPolicy::DEFAULT,
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
        Ok(())
    }
}
