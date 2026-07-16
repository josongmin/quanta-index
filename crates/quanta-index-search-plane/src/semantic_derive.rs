//! Search-owned semantic batch derivation.
//!
//! Turns an accepted `SearchCorpusIngestBatch` into a `SemanticIngestBatch`.
//! The derivation can stay on the legacy chunk-text path or migrate to typed
//! semantic sources, but the embedder identity is always pinned into the batch's
//! `EmbeddingModelContract`. This is the ingest counterpart to the query-time
//! model-identity gate: both sides read the same embedder identity so a corpus
//! vector and a query vector can never be silently produced by different models.
//!
//! Extracted from `ingest_dispatcher` so the routing dispatcher no longer owns
//! embedding/redistribution/digest mechanics — it just calls
//! [`derive_semantic_batch_from_search_corpus_batch`].

use std::collections::BTreeSet;

use quanta_index_contract::{
    CapabilityStatusV1, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
    EmbeddingNormalization, EmbeddingRecord, OwnerDocKind, SearchCorpusIngestBatch, SearchScopeKey,
    SearchScopeSurface, SemanticCorpusKindV1, SemanticIngestBatch, SemanticReplaceScope,
    SemanticSourceRecordV1, SemanticSourceScopeKeyV1, SemanticTombstoneScope, SourceRoleV1,
    lex::LanguageCode, lex::SymbolKindCode, validate_semantic_source_record_v1,
};
use quanta_index_core::{CoreError, TextEmbeddingProvider};
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
    SemanticDerivationModeV1::LegacyAllChunkText;

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
    Ok(EmbeddingModelContract {
        model_id: model_id.to_string().into_boxed_str(),
        model_version: embedder
            .model_version()
            .map(|version| version.to_string().into_boxed_str()),
        dimension,
        normalization: EmbeddingNormalization::L2Unit,
        distance_metric: EmbeddingDistanceMetric::Cosine,
        policy_digest: format!("{model_id}:{policy_digest}").into_boxed_str(),
        view_policy_digest: view_policy_digest.map(|value| value.to_string().into_boxed_str()),
    })
}

pub(crate) fn derive_semantic_batch_with_mode_v1(
    batch: &SearchCorpusIngestBatch,
    embedder: &dyn TextEmbeddingProvider,
    mode: SemanticDerivationModeV1,
) -> Result<SemanticIngestBatch, CoreError> {
    match mode {
        SemanticDerivationModeV1::LegacyAllChunkText => {
            derive_semantic_batch_from_search_corpus_batch(batch, embedder)
        }
        SemanticDerivationModeV1::SemanticSourcesWithLegacyFallback
        | SemanticDerivationModeV1::SemanticSourcesOnly => {
            derive_semantic_batch_from_semantic_sources_v1(batch, embedder, mode)
        }
    }
}

pub(crate) fn derive_semantic_batch_from_search_corpus_batch(
    batch: &SearchCorpusIngestBatch,
    embedder: &dyn TextEmbeddingProvider,
) -> Result<SemanticIngestBatch, CoreError> {
    batch
        .validate_surface_mutations_v1()
        .map_err(|err| CoreError::InvalidContract(format!("semantic derivation: {err}")))?;
    let dimension = embedder.dimension();
    if dimension == 0 {
        return Err(CoreError::InvalidContract(
            "semantic derivation: embedding dimension must be non-zero".to_string(),
        ));
    }
    let model_contract = embedding_model_contract_for(embedder, LEGACY_CHUNK_POLICY_DIGEST, None)?;
    // Embed the WHOLE batch in one call: gather every chunk text across ALL
    // scopes and hand them to the embedder together, so a real provider packs
    // them into the fewest token-budget-bounded requests (one network round trip
    // can carry many scopes/files). Embedding per scope instead forces at least
    // one request per scope — in practice one per file — which provider telemetry
    // confirmed dominates ingest cost. The deterministic hash embedder is
    // unaffected: its per-text vectors are identical regardless of batching.
    let all_texts: Vec<&str> = batch
        .replace_scopes
        .iter()
        .flat_map(|scope| scope.chunks.iter().map(|chunk| chunk.text.as_ref()))
        .collect();
    let all_vectors = embedder.embed_batch(&all_texts)?;
    if all_vectors.len() != all_texts.len() {
        return Err(CoreError::InvalidContract(format!(
            "semantic derivation: embedder returned {} vectors for {} batched chunk texts",
            all_vectors.len(),
            all_texts.len()
        )));
    }

    // Redistribute the flat vectors back to their scopes IN ORDER. A draining
    // iterator preserves chunk<->vector alignment without index arithmetic; an
    // underflow (fewer vectors than chunks) and a leftover (more than chunks)
    // both fail closed rather than silently misalign a vector with a chunk.
    let mut vectors = all_vectors.into_iter();
    let replace_scopes = batch
        .replace_scopes
        .iter()
        .map(|scope| {
            let embeddings = scope
                .chunks
                .iter()
                .map(|chunk| {
                    let vector = vectors.next().ok_or_else(|| {
                        CoreError::InvalidContract(
                            "semantic derivation: ran out of embedding vectors while \
                             redistributing the batched embed result across scopes"
                                .to_string(),
                        )
                    })?;
                    embedding_record_for(
                        chunk,
                        semantic_embedding_input_text(chunk),
                        vector,
                        &model_contract,
                    )
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(SemanticReplaceScope {
                scope: scope.scope.clone(),
                scope_digest: scope.scope_digest.clone(),
                embeddings,
                cluster_memberships: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    if vectors.next().is_some() {
        return Err(CoreError::InvalidContract(
            "semantic derivation: batched embed produced more vectors than the batch had chunks"
                .to_string(),
        ));
    }
    Ok(SemanticIngestBatch {
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        base_generation: batch.base_generation,
        manifest_digest: batch.manifest_digest.clone(),
        batch_digest: format!("{}:semantic-derive", batch.batch_digest),
        mode: batch.mode,
        model_contract,
        required_corpora: vec![SemanticCorpusKindV1::RawCodeFallback],
        corpus_policy_digest: None,
        clear_surfaces: batch.clear_surfaces.clone(),
        replace_scopes,
        tombstone_scopes: batch
            .tombstone_scopes
            .iter()
            .map(|scope| quanta_index_contract::SemanticTombstoneScope {
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
            .collect(),
        seal: batch.seal,
    })
}

pub(crate) fn derive_semantic_batch_from_semantic_sources_v1(
    batch: &SearchCorpusIngestBatch,
    embedder: &dyn TextEmbeddingProvider,
    mode: SemanticDerivationModeV1,
) -> Result<SemanticIngestBatch, CoreError> {
    batch
        .validate_surface_mutations_v1()
        .map_err(|err| CoreError::InvalidContract(format!("semantic derivation: {err}")))?;
    let dimension = embedder.dimension();
    if dimension == 0 {
        return Err(CoreError::InvalidContract(
            "semantic derivation: embedding dimension must be non-zero".to_string(),
        ));
    }
    let has_semantic_lifecycle_operation = !batch.semantic_tombstone_scopes.is_empty()
        || !batch.tombstone_scopes.is_empty()
        || !batch.clear_surfaces.is_empty()
        || batch.seal;
    if batch.semantic_replace_scopes.is_empty() && !has_semantic_lifecycle_operation {
        return match mode {
            SemanticDerivationModeV1::SemanticSourcesWithLegacyFallback => {
                let mut legacy = derive_semantic_batch_from_search_corpus_batch(batch, embedder)?;
                legacy.batch_digest = format!(
                    "{}:semantic-derive:legacy-fallback-empty-semantic-sources",
                    batch.batch_digest
                );
                legacy.model_contract = embedding_model_contract_for(
                    embedder,
                    SEMANTIC_SOURCE_POLICY_DIGEST,
                    Some(SEMANTIC_SOURCE_FALLBACK_VIEW_POLICY_DIGEST),
                )?;
                legacy.corpus_policy_digest =
                    Some(SEMANTIC_SOURCE_FALLBACK_VIEW_POLICY_DIGEST.to_string());
                Ok(legacy)
            }
            SemanticDerivationModeV1::SemanticSourcesOnly => Err(CoreError::InvalidContract(
                "semantic derivation: semantic sources required in semantic_only mode".to_string(),
            )),
            SemanticDerivationModeV1::LegacyAllChunkText => {
                derive_semantic_batch_from_search_corpus_batch(batch, embedder)
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
    let all_texts: Vec<&str> = validated_scopes
        .iter()
        .flat_map(|scope| scope.records.iter().map(|record| record.text.as_str()))
        .collect();
    let all_vectors = embedder.embed_batch(&all_texts)?;
    if all_vectors.len() != all_texts.len() {
        return Err(CoreError::InvalidContract(format!(
            "semantic derivation: embedder returned {} vectors for {} semantic source texts",
            all_vectors.len(),
            all_texts.len()
        )));
    }
    let mut vectors = all_vectors.into_iter();
    let replace_scopes = validated_scopes
        .into_iter()
        .map(|scope| {
            let embeddings = scope
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
                    embedding_record_for_semantic_source(record, vector, &model_contract)
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(SemanticReplaceScope {
                scope: scope.scope,
                scope_digest: scope.scope_digest,
                embeddings,
                cluster_memberships: scope.cluster_memberships,
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    if vectors.next().is_some() {
        return Err(CoreError::InvalidContract(
            "semantic derivation: semantic source embed produced more vectors than the batch had sources"
                .to_string(),
        ));
    }
    Ok(SemanticIngestBatch {
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        base_generation: batch.base_generation,
        manifest_digest: batch.manifest_digest.clone(),
        batch_digest: format!("{}:semantic-derive:semantic-source-v1", batch.batch_digest),
        mode: batch.mode,
        model_contract,
        required_corpora,
        corpus_policy_digest: Some(SEMANTIC_SOURCE_POLICY_DIGEST.to_string()),
        clear_surfaces: batch.clear_surfaces.clone(),
        replace_scopes,
        tombstone_scopes: semantic_tombstone_scopes_v1(batch),
        seal: batch.seal,
    })
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
            if !scope
                .cluster_memberships
                .windows(2)
                .all(|pair| pair[0].cluster_record_id < pair[1].cluster_record_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HashingQueryTextEmbedder, SEARCH_OWNED_SEMANTIC_DIMENSION};
    use quanta_index_contract::{
        BatchIngestMode, ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath,
        RevisionId, SearchCorpusReplaceScope, SearchScopeSurface, SemanticSourceReplaceScopeV1,
        SemanticSourceScopeKeyV1, lex::LanguageCode,
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
            repo_id: RepoId::new("repo-1"),
            revision_id: RevisionId::new("rev-1"),
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
        let batch = fixture_search_batch()?;
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
}
