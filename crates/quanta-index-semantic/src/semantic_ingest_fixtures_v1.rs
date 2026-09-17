//! Shared v4 semantic ingest fixtures for integration tests and owner-local tests.
//!
//! Centralizes `EmbeddingRecord` / `SemanticIngestBatch` construction so format
//! v4 field additions do not drift across test files.

use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, EmbeddingDistanceMetric, EmbeddingId,
    EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord, ManifestGeneration,
    OwnerDocKind, RepoId, RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface,
    SemanticCorpusKindV1, SemanticIngestBatch, SemanticReplaceScope, SemanticSourceScopeKeyV1,
    SemanticTombstoneScope, SourceRoleV1, lex::LanguageCode,
};
use quanta_index_core::{CoreError, SemanticPolicy, build_resident_semantic_batch_v1};

use crate::SemanticAdapter;

/// Build an already-resident fixture batch through `adapter`'s streamed
/// port under the adapter's own window policy, discarding the tally.
///
/// Fixtures hold their batches resident by construction; this is the one
/// place they enter the production build entry, so a fixture test proves
/// the same path a streamed batch takes.
pub fn build_resident_batch_v1(
    adapter: &SemanticAdapter,
    batch: &SemanticIngestBatch,
) -> Result<(), CoreError> {
    let _tally = build_resident_semantic_batch_v1(adapter, batch, adapter.window_policy())?;
    Ok(())
}

/// Build a legacy raw-chunk style embedding row with all v4 metadata fields populated.
pub fn legacy_chunk_embedding_record_v1(
    id: &str,
    path: &str,
    vector: Vec<f32>,
) -> Result<EmbeddingRecord, String> {
    embedding_record_v1(
        id,
        path,
        OwnerDocKind::Chunk,
        &format!("owner-{id}"),
        SemanticCorpusKindV1::RawCodeFallback,
        vector,
    )
}

/// Build an embedding row for an arbitrary owner/corpus pair.
///
/// The fixture contract is `L2Unit`, so the given direction is normalized
/// the way the runtime's provider wrapper normalizes real output (QI-BB-031);
/// a zero or non-finite direction is a fixture error.
pub fn embedding_record_v1(
    id: &str,
    path: &str,
    owner_kind: OwnerDocKind,
    owner_id: &str,
    corpus_kind: SemanticCorpusKindV1,
    mut vector: Vec<f32>,
) -> Result<EmbeddingRecord, String> {
    SemanticPolicy::normalize_l2_unit_v1(&mut vector)
        .map_err(|err| format!("fixture vector for {id} cannot be normalized: {err}"))?;
    let source_role = match corpus_kind {
        SemanticCorpusKindV1::RawCodeFallback => SourceRoleV1::RawFallbackText,
        SemanticCorpusKindV1::DocumentSummary => SourceRoleV1::SummaryText,
        SemanticCorpusKindV1::DocumentLeaf | SemanticCorpusKindV1::DocumentSection => {
            SourceRoleV1::DocumentText
        }
        SemanticCorpusKindV1::SymbolCard
        | SemanticCorpusKindV1::ModuleCard
        | SemanticCorpusKindV1::ClusterCard
        | SemanticCorpusKindV1::TestBehavior
        | SemanticCorpusKindV1::RepositorySummary => SourceRoleV1::CardText,
    };
    let capability_status = if corpus_kind == SemanticCorpusKindV1::RawCodeFallback {
        CapabilityStatusV1::Degraded
    } else {
        CapabilityStatusV1::Full
    };
    Ok(EmbeddingRecord {
        embedding_id: EmbeddingId::new(id),
        record_id: format!("record-{id}").into_boxed_str(),
        owner_kind,
        owner_id: owner_id.to_string().into_boxed_str(),
        corpus_kind,
        parent_owner_id: (corpus_kind == SemanticCorpusKindV1::RawCodeFallback)
            .then(|| owner_id.to_string().into_boxed_str()),
        source_doc_id: format!("doc-{id}").into_boxed_str(),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust").map_err(|err| format!("fixture language: {err}"))?,
        package: Some("crate".to_string().into_boxed_str()),
        symbol_kind: None,
        visibility: Some("pub".to_string().into_boxed_str()),
        source_role,
        generated: false,
        capability_status,
        authority_digest: format!("auth:{id}").into_boxed_str(),
        render_policy_digest: format!("render:{id}").into_boxed_str(),
        card_schema_version: u32::from(corpus_kind != SemanticCorpusKindV1::RawCodeFallback),
        start_byte: 0,
        end_byte: 12,
        start_line: 1,
        end_line: 3,
        snippet: format!("fn {id}() {{}}").into_boxed_str(),
        embedding_input_digest: format!("input:{id}").into_boxed_str(),
        vector_digest: format!("vector:{id}").into_boxed_str(),
        view_kind: "raw_chunk".to_string().into_boxed_str(),
        vector,
    })
}

#[must_use]
pub fn search_scope_v1(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

#[must_use]
pub fn tombstone_scope_v1(path: &str) -> SemanticTombstoneScope {
    SemanticTombstoneScope {
        scope: Some(search_scope_v1(path)),
        semantic_scope: None,
    }
}

#[must_use]
pub fn tombstone_scope_with_semantic_owner_v1(
    path: &str,
    corpus_kind: SemanticCorpusKindV1,
    owner_kind: OwnerDocKind,
    owner_id: &str,
) -> SemanticTombstoneScope {
    SemanticTombstoneScope {
        scope: Some(search_scope_v1(path)),
        semantic_scope: Some(SemanticSourceScopeKeyV1 {
            corpus_kind,
            owner_kind,
            owner_id: owner_id.to_string(),
        }),
    }
}

#[must_use]
pub fn model_contract_v1(dimension: u32) -> EmbeddingModelContract {
    EmbeddingModelContract {
        model_id: "text-embed".to_string().into_boxed_str(),
        model_version: Some("1".to_string().into_boxed_str()),
        dimension,
        normalization: EmbeddingNormalization::L2Unit,
        distance_metric: EmbeddingDistanceMetric::Cosine,
        policy_digest: "policy:feed".to_string().into_boxed_str(),
        view_policy_digest: Some("view:feed".to_string().into_boxed_str()),
    }
}

/// Populate v4 batch metadata defaults on an in-progress batch builder.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "test fixtures deliberately expose every batch field positionally so a case can vary exactly one of them; grouping them into structs would make the varying field harder to see at each call site"
)]
pub fn ingest_batch_v1(
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    base_generation: Option<ManifestGeneration>,
    manifest_digest: String,
    batch_digest: String,
    mode: BatchIngestMode,
    model_contract: EmbeddingModelContract,
    replace_scopes: Vec<SemanticReplaceScope>,
    tombstone_scopes: Vec<SemanticTombstoneScope>,
    seal: bool,
) -> SemanticIngestBatch {
    SemanticIngestBatch {
        repo_id,
        revision_id,
        generation,
        base_generation,
        manifest_digest,
        batch_digest,
        mode,
        model_contract,
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes,
        seal,
    }
}

#[must_use]
pub fn sealed_replace_batch_v1(
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    path: &str,
    embeddings: Vec<EmbeddingRecord>,
    contract_dimension: u32,
) -> SemanticIngestBatch {
    ingest_batch_v1(
        repo_id,
        revision_id,
        generation,
        None,
        format!("manifest:{}", generation.get()),
        format!("batch:{}:{path}", generation.get()),
        BatchIngestMode::ReplaceGeneration,
        model_contract_v1(contract_dimension),
        vec![SemanticReplaceScope {
            scope: search_scope_v1(path),
            scope_digest: format!("scope:{path}"),
            embeddings,
            cluster_memberships: Vec::new(),
        }],
        Vec::new(),
        true,
    )
}
