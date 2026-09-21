//! QI-BB-021 — the ingest resource envelope measures a batch as the
//! derivation will embed it, and refuses typed before anything is held.
//!
//! The oracle is arithmetic the test does itself: records and text bytes
//! counted from the batch it built, vector bytes as `records × dimension ×
//! 4`. The footprint must match exactly, and every ceiling must refuse
//! under `INGEST_RESOURCE_BUDGET_EXCEEDED` at one over and admit at the
//! bound.

#![forbid(unsafe_code)]

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, ChunkId, ChunkRecord, ManifestGeneration, OwnerDocKind,
    RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchScopeKey, SearchScopeSurface, SemanticCorpusKindV1, SemanticSourceRecordV1,
    SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, SourceRoleV1,
};
use quanta_index_core::{
    CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestResourcePolicy, MAX_EMBEDDING_DIMENSION,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIMENSION: usize = 8;

fn chunk(index: usize, text: &str) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(format!("chunk-{index}")),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: LanguageCode::new("rust")
            .map_err(|err| -> Box<dyn std::error::Error> { format!("language: {err}").into() })?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 1,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn source(index: usize, text: &str) -> SemanticSourceRecordV1 {
    let owner_id = format!("source-{index}");
    SemanticSourceRecordV1 {
        record_id: owner_id.clone(),
        corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
        owner_kind: OwnerDocKind::Chunk,
        owner_id: owner_id.clone(),
        source_doc_id: owner_id,
        parent_owner_id: None,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: Some("rust".to_string()),
        package: None,
        symbol_kind: None,
        visibility: None,
        source_role: SourceRoleV1::RawFallbackText,
        generated: false,
        capability_status: CapabilityStatusV1::Degraded,
        raw_fallback_reason: None,
        authority_digest: "test:authority".to_string(),
        render_policy_digest: "test:render".to_string(),
        card_schema_version: 0,
        text: text.to_string(),
    }
}

/// A batch of `chunk_texts` legacy chunks and `source_texts` typed sources.
#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn batch(
    chunk_texts: &[&str],
    source_texts: &[&str],
) -> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
    let chunks = chunk_texts
        .iter()
        .enumerate()
        .map(|(index, text)| chunk(index, text))
        .collect::<Result<Vec<_>, _>>()?;
    let replace_scopes = if chunks.is_empty() {
        Vec::new()
    } else {
        vec![SearchCorpusReplaceScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::File,
                repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            },
            scope_digest: "scope:src/lib.rs".to_string(),
            chunks,
            symbols: Vec::new(),
        }]
    };
    let semantic_replace_scopes = source_texts
        .iter()
        .enumerate()
        .map(|(index, text)| SemanticSourceReplaceScopeV1 {
            scope: SemanticSourceScopeKeyV1 {
                corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                owner_kind: OwnerDocKind::Chunk,
                owner_id: format!("source-{index}"),
            },
            scope_digest: format!("scope:source-{index}"),
            sources: vec![source(index, text)],
            cluster_memberships: Vec::new(),
        })
        .collect();
    Ok(SearchCorpusIngestBatch {
        repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "manifest:1".to_string(),
        batch_digest: "batch:1".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes,
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
}

fn is_envelope_refusal(err: &CoreError) -> bool {
    matches!(err, CoreError::Typed { code, .. } if *code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE)
}

fn expect_refusal(outcome: Result<impl std::fmt::Debug, CoreError>, what: &str) -> TestResult {
    match outcome {
        Err(err) if is_envelope_refusal(&err) => Ok(()),
        Err(other) => Err(format!("{what}: refused under the wrong error: {other}").into()),
        Ok(admitted) => Err(format!("{what}: admitted {admitted:?}").into()),
    }
}

/// Legacy chunks are what the derivation embeds when a batch carries no
/// typed sources; the footprint counts exactly them.
#[test]
fn chunk_only_batches_are_measured_by_their_chunks() -> TestResult {
    let batch = batch(&["alpha", "beta-beta", "γ"], &[])?;
    let footprint = IngestResourcePolicy::DEFAULT.admit_search_corpus_batch(&batch, DIMENSION)?;
    let text_bytes = u64::try_from("alpha".len() + "beta-beta".len() + "γ".len())?;
    if footprint.carried_records != 3
        || footprint.embedded_records != 3
        || footprint.text_bytes != text_bytes
        || footprint.vector_bytes != 3 * u64::try_from(DIMENSION)? * 4
    {
        return Err(format!("chunk footprint drifted: {footprint:?}").into());
    }
    Ok(())
}

/// Typed sources win: when a batch carries any, they are the embedded set
/// and the chunks only count as carried rows.
#[test]
fn typed_sources_are_the_embedded_set_when_present() -> TestResult {
    let batch = batch(&["chunk text that is long"], &["s1", "s2"])?;
    let footprint = IngestResourcePolicy::DEFAULT.admit_search_corpus_batch(&batch, DIMENSION)?;
    if footprint.carried_records != 3
        || footprint.embedded_records != 2
        || footprint.text_bytes != 4
        || footprint.vector_bytes != 2 * u64::try_from(DIMENSION)? * 4
    {
        return Err(format!("source footprint drifted: {footprint:?}").into());
    }
    Ok(())
}

/// Each ceiling admits at the bound and refuses one past it, typed.
#[test]
fn every_ceiling_admits_at_the_bound_and_refuses_one_over() -> TestResult {
    let batch = batch(&["ab", "cd", "ef"], &[])?;
    let vector_bytes = 3 * u64::try_from(DIMENSION)? * 4;

    let at_records = IngestResourcePolicy::new(3, u64::MAX, u64::MAX)?;
    let _admitted = at_records.admit_search_corpus_batch(&batch, DIMENSION)?;
    let over_records = IngestResourcePolicy::new(2, u64::MAX, u64::MAX)?;
    expect_refusal(over_records.admit_search_corpus_batch(&batch, DIMENSION), "records")?;

    let at_text = IngestResourcePolicy::new(usize::MAX, 6, u64::MAX)?;
    let _admitted = at_text.admit_search_corpus_batch(&batch, DIMENSION)?;
    let over_text = IngestResourcePolicy::new(usize::MAX, 5, u64::MAX)?;
    expect_refusal(over_text.admit_search_corpus_batch(&batch, DIMENSION), "text")?;

    let at_vectors = IngestResourcePolicy::new(usize::MAX, u64::MAX, vector_bytes)?;
    let _admitted = at_vectors.admit_search_corpus_batch(&batch, DIMENSION)?;
    let over_vectors = IngestResourcePolicy::new(usize::MAX, u64::MAX, vector_bytes - 1)?;
    expect_refusal(over_vectors.admit_search_corpus_batch(&batch, DIMENSION), "vectors")?;
    Ok(())
}

/// The vector ceiling scales with the dimension: the same batch that fits
/// at one dimension is refused at a wider one.
#[test]
fn a_wider_dimension_multiplies_the_vector_footprint() -> TestResult {
    let batch = batch(&["ab", "cd"], &[])?;
    let policy = IngestResourcePolicy::new(usize::MAX, u64::MAX, 2 * 8 * 4)?;
    let _admitted = policy.admit_search_corpus_batch(&batch, 8)?;
    expect_refusal(policy.admit_search_corpus_batch(&batch, 9), "dimension 9")
}

/// A dimension outside the plane's range is a composition defect, not a
/// batch refusal.
#[test]
fn an_out_of_range_dimension_is_an_invalid_contract() -> TestResult {
    let batch = batch(&["ab"], &[])?;
    let policy = IngestResourcePolicy::DEFAULT;
    let _admitted = policy.admit_search_corpus_batch(&batch, MAX_EMBEDDING_DIMENSION)?;
    for dimension in [0, MAX_EMBEDDING_DIMENSION + 1] {
        match policy.admit_search_corpus_batch(&batch, dimension) {
            Err(CoreError::InvalidContract(_)) => {}
            other => {
                return Err(format!("dimension {dimension} answered {other:?}").into());
            }
        }
    }
    Ok(())
}

/// An empty batch (a bare seal) has no footprint and always fits.
#[test]
fn an_empty_batch_fits_the_smallest_policy() -> TestResult {
    let batch = batch(&[], &[])?;
    let footprint =
        IngestResourcePolicy::new(1, 1, 1)?.admit_search_corpus_batch(&batch, DIMENSION)?;
    if footprint.carried_records != 0 || footprint.text_bytes != 0 || footprint.vector_bytes != 0 {
        return Err(format!("empty footprint drifted: {footprint:?}").into());
    }
    Ok(())
}

#[test]
fn zero_ceilings_are_refused_at_construction() {
    assert!(IngestResourcePolicy::new(0, 1, 1).is_err());
    assert!(IngestResourcePolicy::new(1, 0, 1).is_err());
    assert!(IngestResourcePolicy::new(1, 1, 0).is_err());
    assert!(IngestResourcePolicy::new(1, 1, 1).is_ok());
}
