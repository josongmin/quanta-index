//! QI-BB-021 — the ingest resource envelope measures a batch as the
//! derivation will embed it and the source files it will retain, and refuses
//! typed before anything is held.
//!
//! The oracle is arithmetic the test does itself: records and text bytes
//! counted from the typed semantic sources it built, vector bytes as `sources × dimension ×
//! 4`. The footprint must match exactly, and every ceiling must refuse
//! under `INGEST_RESOURCE_BUDGET_EXCEEDED` at one over and admit at the
//! bound.

#![forbid(unsafe_code)]

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, ChunkId, ChunkRecord, ManifestGeneration, OwnerDocKind,
    RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SemanticCorpusKindV1, SemanticSourceRecordV1, SemanticSourceReplaceScopeV1,
    SemanticSourceScopeKeyV1, SourceFileCoverage, SourceFileKey, SourceFileRevision,
    SourcePublicationEvent, SourceRoleV1, SymbolCoverage, source_event_payload_sha256,
    source_file_unit_set_sha256,
};
use quanta_index_core::{
    CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestResourcePolicy, MAX_EMBEDDING_DIMENSION,
};
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIMENSION: usize = 8;

fn chunk(
    index: usize,
    text: &str,
    start_byte: u32,
) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
    let line = u32::try_from(index)?
        .checked_add(1)
        .ok_or("line overflow")?;
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(format!("chunk-{index}")),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: LanguageCode::new("rust")
            .map_err(|err| -> Box<dyn std::error::Error> { format!("language: {err}").into() })?,
        start_byte,
        end_byte: start_byte
            .checked_add(u32::try_from(text.len())?)
            .ok_or("chunk end overflow")?,
        start_line: line,
        end_line: line,
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
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        owner_kind: OwnerDocKind::Symbol,
        owner_id: owner_id.clone(),
        source_doc_id: owner_id,
        parent_owner_id: None,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: Some("rust".to_string()),
        package: None,
        symbol_kind: None,
        visibility: None,
        source_role: SourceRoleV1::CardText,
        generated: false,
        capability_status: CapabilityStatusV1::Full,
        raw_fallback_reason: None,
        authority_digest: "test:authority".to_string(),
        render_policy_digest: "test:render".to_string(),
        card_schema_version: 0,
        text: text.to_string(),
    }
}

/// A batch of lexical chunks and typed semantic sources.
#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn batch(
    chunk_texts: &[&str],
    source_texts: &[&str],
) -> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
    let mut raw_source = String::new();
    let mut chunks = Vec::with_capacity(chunk_texts.len());
    for (index, text) in chunk_texts.iter().enumerate() {
        if index != 0 {
            raw_source.push('\n');
        }
        let start_byte = u32::try_from(raw_source.len())?;
        raw_source.push_str(text);
        chunks.push(chunk(index, text, start_byte)?);
    }
    let repo_id = RepoId::new("repo").expect("static fixture ID satisfies canonical policy");
    let revision_id = RevisionId::new("rev").expect("static fixture ID satisfies canonical policy");
    let replace_scopes = if chunks.is_empty() {
        Vec::new()
    } else {
        vec![SearchCorpusReplaceScope {
            coverage: SourceFileCoverage {
                source: SourceFileRevision {
                    file: SourceFileKey {
                        source_repo_id: repo_id.clone(),
                        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                    },
                    revision_id: revision_id.clone(),
                    source_sha256: Sha256::digest(raw_source.as_bytes()).into(),
                },
                language: LanguageCode::new("rust").map_err(
                    |err| -> Box<dyn std::error::Error> { format!("language: {err}").into() },
                )?,
                producer_policy_sha256: Sha256::digest(b"ingest-resource-policy-fixture-v1").into(),
                symbol_name_source_policy: quanta_index_contract::SymbolNameSourcePolicyV1::Unspecified,
                unit_set_sha256: source_file_unit_set_sha256(&chunks, &[])?,
                text_admitted: true,
                symbols: SymbolCoverage::NotRequested,
            },
            source_bytes: raw_source.into_bytes(),
            chunks,
            symbols: Vec::new(),
        }]
    };
    let semantic_replace_scopes = source_texts
        .iter()
        .enumerate()
        .map(|(index, text)| SemanticSourceReplaceScopeV1 {
            scope: SemanticSourceScopeKeyV1 {
                corpus_kind: SemanticCorpusKindV1::SymbolCard,
                owner_kind: OwnerDocKind::Symbol,
                owner_id: format!("source-{index}"),
            },
            scope_digest: format!("scope:source-{index}"),
            sources: vec![source(index, text)],
            cluster_memberships: Vec::new(),
        })
        .collect();
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "ingest-resource-policy-fixture".to_string(),
            event_id: "ingest-resource-policy-g1".to_string(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id,
        revision_id,
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "manifest:1".to_string(),
        // The policy test is below IPC's batch-digest verification.
        batch_digest: "0".repeat(64),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes,
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
    Ok(batch)
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

/// A lexical-only replacement is an explicit semantic no-op.
#[test]
fn chunk_only_batches_do_not_request_embedding() -> TestResult {
    let batch = batch(&["alpha", "beta-beta", "γ"], &[])?;
    let footprint = IngestResourcePolicy::DEFAULT.admit_search_corpus_batch(&batch, DIMENSION)?;
    if footprint.carried_records != 3
        || footprint.embedded_records != 0
        || footprint.source_bytes != 18
        || footprint.text_bytes != 0
        || footprint.vector_bytes != 0
    {
        return Err(format!("chunk footprint drifted: {footprint:?}").into());
    }
    Ok(())
}

/// Only typed sources are embedded; chunks count as carried lexical rows.
#[test]
fn typed_sources_are_the_embedded_set_when_present() -> TestResult {
    let batch = batch(&["chunk text that is long"], &["s1", "s2"])?;
    let footprint = IngestResourcePolicy::DEFAULT.admit_search_corpus_batch(&batch, DIMENSION)?;
    if footprint.carried_records != 3
        || footprint.embedded_records != 2
        || footprint.source_bytes != 23
        || footprint.text_bytes != 4
        || footprint.vector_bytes != 2 * u64::try_from(DIMENSION)? * 4
    {
        return Err(format!("source footprint drifted: {footprint:?}").into());
    }
    Ok(())
}

/// File authority is budgeted even when the batch embeds no semantic text.
#[test]
fn source_file_bytes_have_a_typed_ceiling() -> TestResult {
    let batch = batch(&["abcdef"], &[])?;
    let at_source = IngestResourcePolicy::new(usize::MAX, 6, u64::MAX)?;
    let admitted = at_source.admit_search_corpus_batch(&batch, DIMENSION)?;
    if admitted.source_bytes != 6 || admitted.text_bytes != 0 {
        return Err(format!(
            "unexpected admitted bytes: source={}, text={}",
            admitted.source_bytes, admitted.text_bytes
        )
        .into());
    }
    let over_source = IngestResourcePolicy::new(usize::MAX, 5, u64::MAX)?;
    expect_refusal(
        over_source.admit_search_corpus_batch(&batch, DIMENSION),
        "source bytes",
    )
}

/// Each ceiling admits at the bound and refuses one past it, typed.
#[test]
fn every_ceiling_admits_at_the_bound_and_refuses_one_over() -> TestResult {
    let batch = batch(&[], &["ab", "cd", "ef"])?;
    let vector_bytes = 3 * u64::try_from(DIMENSION)? * 4;

    let at_records = IngestResourcePolicy::new(3, u64::MAX, u64::MAX)?;
    let _admitted = at_records.admit_search_corpus_batch(&batch, DIMENSION)?;
    let over_records = IngestResourcePolicy::new(2, u64::MAX, u64::MAX)?;
    expect_refusal(
        over_records.admit_search_corpus_batch(&batch, DIMENSION),
        "records",
    )?;

    let at_text = IngestResourcePolicy::new(usize::MAX, 6, u64::MAX)?;
    let _admitted = at_text.admit_search_corpus_batch(&batch, DIMENSION)?;
    let over_text = IngestResourcePolicy::new(usize::MAX, 5, u64::MAX)?;
    expect_refusal(
        over_text.admit_search_corpus_batch(&batch, DIMENSION),
        "text",
    )?;

    let at_vectors = IngestResourcePolicy::new(usize::MAX, u64::MAX, vector_bytes)?;
    let _admitted = at_vectors.admit_search_corpus_batch(&batch, DIMENSION)?;
    let over_vectors = IngestResourcePolicy::new(usize::MAX, u64::MAX, vector_bytes - 1)?;
    expect_refusal(
        over_vectors.admit_search_corpus_batch(&batch, DIMENSION),
        "vectors",
    )?;
    Ok(())
}

/// The vector ceiling scales with the dimension: the same batch that fits
/// at one dimension is refused at a wider one.
#[test]
fn a_wider_dimension_multiplies_the_vector_footprint() -> TestResult {
    let batch = batch(&[], &["ab", "cd"])?;
    let policy = IngestResourcePolicy::new(usize::MAX, u64::MAX, 2 * 8 * 4)?;
    let _admitted = policy.admit_search_corpus_batch(&batch, 8)?;
    expect_refusal(policy.admit_search_corpus_batch(&batch, 9), "dimension 9")
}

/// A dimension outside the plane's range is a composition defect, not a
/// batch refusal.
#[test]
fn an_out_of_range_dimension_is_an_invalid_contract() -> TestResult {
    let batch = batch(&[], &["ab"])?;
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
    if footprint.carried_records != 0
        || footprint.source_bytes != 0
        || footprint.text_bytes != 0
        || footprint.vector_bytes != 0
    {
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
