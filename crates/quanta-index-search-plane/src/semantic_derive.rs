//! Search-owned semantic batch derivation.
//!
//! Turns an accepted `SearchCorpusIngestBatch` into a `SemanticIngestBatch` by
//! embedding every chunk text once (batched across all scopes) and pinning the
//! embedder's model identity into the batch's `EmbeddingModelContract`. This is
//! the ingest counterpart to the query-time model-identity gate: both sides read
//! the same embedder identity so a corpus vector and a query vector can never be
//! silently produced by different models.
//!
//! Extracted from `ingest_dispatcher` so the routing dispatcher no longer owns
//! embedding/redistribution/digest mechanics — it just calls
//! [`derive_semantic_batch_from_search_corpus_batch`].

use quanta_index_contract::{
    EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract, EmbeddingNormalization,
    EmbeddingRecord, OwnerDocKind, SearchCorpusIngestBatch, SemanticIngestBatch,
    SemanticReplaceScope,
};
use quanta_index_core::{CoreError, TextEmbeddingProvider};
use sha2::{Digest, Sha256};

fn embedding_model_contract_for(
    embedder: &dyn TextEmbeddingProvider,
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
        policy_digest: format!("{model_id}:chunk.text").into_boxed_str(),
        view_policy_digest: None,
    })
}

pub(crate) fn derive_semantic_batch_from_search_corpus_batch(
    batch: &SearchCorpusIngestBatch,
    embedder: &dyn TextEmbeddingProvider,
) -> Result<SemanticIngestBatch, CoreError> {
    let dimension = embedder.dimension();
    if dimension == 0 {
        return Err(CoreError::InvalidContract(
            "semantic derivation: embedding dimension must be non-zero".to_string(),
        ));
    }
    let model_contract = embedding_model_contract_for(embedder)?;
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
        replace_scopes,
        tombstone_scopes: batch
            .tombstone_scopes
            .iter()
            .map(|scope| quanta_index_contract::SemanticTombstoneScope {
                scope: scope.scope.clone(),
            })
            .collect(),
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
        owner_kind: OwnerDocKind::Chunk,
        owner_id: chunk.chunk_id.as_str().to_string().into_boxed_str(),
        source_doc_id: chunk.chunk_id.as_str().to_string().into_boxed_str(),
        repo_relative_path: chunk.repo_relative_path.clone(),
        language: chunk.language.clone(),
        symbol_kind: None,
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
