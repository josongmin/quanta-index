use std::sync::{Arc, Mutex, RwLock};

use quanta_index_core::{
    CoreError, IngestResourcePolicy, SearchCorpusIngestPort, SemanticIngestPort,
    SemanticStreamWindowPolicy,
};

use crate::ingest_dispatcher::auxiliary::AuxiliaryMutationCoordinator;
use crate::ingest_dispatcher::search_corpus::{
    DirectSearchCorpusMaterializer, SearchCorpusMaterializerParts,
};
use crate::ingest_dispatcher::semantic::DirectSemanticMaterializer;
use crate::ingest_dispatcher::tests::support::{
    CountingEmbedder, FakeSearchCorpusBuilder, FakeSemanticBuilder, FixedFakeEmbedder, TestRes,
    build_then_valid_generation, fixture_chunk_record, fixture_model_contract,
    fixture_search_corpus_batch, materializer_with_embedder, memory_aux_catalog, memory_catalog,
    multi_scope_corpus_batch, no_storage_sealed_reclaim, recording_search_corpus_authority,
    search_corpus_materializer, test_incomplete_generation_discard,
};
use crate::semantic_derive::{
    semantic_embedding_input_digest, semantic_embedding_input_text, semantic_vector_digest,
};
use crate::{Ledger, SEARCH_OWNED_SEMANTIC_DIMENSION, SnapshotRegistries};

// CASE-COVERS: corpus derivation fails closed when the embedder returns the
// wrong number of vectors — a misaligned batch must never reach the index.
#[test]
fn search_corpus_derivation_fails_closed_on_embedder_count_mismatch() -> TestRes {
    let materializer = materializer_with_embedder(Arc::new(FixedFakeEmbedder {
        dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
        vectors_per_call: 0,
        vector_len: SEARCH_OWNED_SEMANTIC_DIMENSION,
    }));
    let batch = fixture_search_corpus_batch()?;
    match materializer.publish_batch(&batch) {
        Err(CoreError::InvalidContract(message)) => {
            if !message.contains("vectors for") {
                return Err(format!("unexpected count-mismatch message: {message}").into());
            }
        }
        other => return Err(format!("count mismatch must fail closed, got {other:?}").into()),
    }
    Ok(())
}

// CASE-COVERS: corpus derivation fails closed when a returned vector has the
// wrong dimension — would corrupt the lancedb fixed-size-list schema.
#[test]
fn search_corpus_derivation_fails_closed_on_embedder_dim_mismatch() -> TestRes {
    let materializer = materializer_with_embedder(Arc::new(FixedFakeEmbedder {
        dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
        vectors_per_call: 1,
        vector_len: SEARCH_OWNED_SEMANTIC_DIMENSION + 1,
    }));
    let batch = fixture_search_corpus_batch()?;
    match materializer.publish_batch(&batch) {
        Err(CoreError::InvalidContract(message)) => {
            if !message.contains("returned dim") {
                return Err(format!("unexpected dim-mismatch message: {message}").into());
            }
        }
        other => return Err(format!("dim mismatch must fail closed, got {other:?}").into()),
    }
    Ok(())
}

// CASE-COVERS: a multi-scope ingest batch is embedded in ONE provider call
// (not one per scope/file), and the flat vectors are redistributed back to
// each scope's chunks IN ORDER. Reverting the derivation to a per-scope embed
// makes the call-count assertion fail; misaligning the redistribution makes
// the chunk-id-order assertion fail.
#[test]
fn corpus_derivation_batches_all_scopes_into_one_embed_call() -> TestRes {
    let embedder = Arc::new(CountingEmbedder {
        dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
        calls: Mutex::new(0),
    });
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(semantic_builder.clone()));
    let search_corpus_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
    let materializer = search_corpus_materializer!(
        search_corpus_builder,
        lexical_ledger,
        semantic_materializer,
        embedder.clone(),
        recording_search_corpus_authority(),
        build_then_valid_generation(),
        build_then_valid_generation(),
        test_incomplete_generation_discard(),
        test_incomplete_generation_discard(),
    );

    let batch = multi_scope_corpus_batch()?;
    let _receipt = materializer.publish_batch(&batch)?;

    // (1) The whole 3-scope / 5-chunk batch costs exactly ONE embed call.
    let calls = *embedder
        .calls
        .lock()
        .map_err(|err| format!("counting embedder lock poisoned: {err}"))?;
    if calls != 1 {
        return Err(format!(
            "expected ONE batched embed call for the whole batch, got {calls} (per-scope regression)"
        )
        .into());
    }

    // (2) Vectors redistributed back to scopes with chunk counts + order intact.
    let derived = semantic_builder.take()?;
    let derived_batch = derived
        .first()
        .ok_or_else(|| "expected one derived semantic batch".to_string())?;
    let per_scope_counts: Vec<usize> = derived_batch
        .replace_scopes
        .iter()
        .map(|scope| scope.embeddings.len())
        .collect();
    if per_scope_counts != vec![2, 1, 2] {
        return Err(format!(
            "scope->chunk redistribution wrong: {per_scope_counts:?}, expected [2, 1, 2]"
        )
        .into());
    }
    let ids: Vec<&str> = derived_batch
        .replace_scopes
        .iter()
        .flat_map(|scope| {
            scope
                .embeddings
                .iter()
                .map(|record| record.embedding_id.as_str())
        })
        .collect();
    if ids != vec!["a-1", "a-2", "b-1", "c-1", "c-2"] {
        return Err(format!("chunk<->vector alignment lost across scopes: {ids:?}").into());
    }
    Ok(())
}

#[test]
fn semantic_digests_change_when_input_or_vector_changes() -> TestRes {
    let model = fixture_model_contract();
    let chunk_a = fixture_chunk_record()?;
    let mut chunk_b = fixture_chunk_record()?;
    chunk_b.text = "typed semantic parser with different body"
        .to_string()
        .into_boxed_str();
    let input_a = semantic_embedding_input_digest(
        &model,
        "chunk.text",
        semantic_embedding_input_text(&chunk_a),
    );
    let input_b = semantic_embedding_input_digest(
        &model,
        "chunk.text",
        semantic_embedding_input_text(&chunk_b),
    );
    if input_a == input_b {
        return Err("input digest must change when embedding input text changes".into());
    }
    let vec_a = semantic_vector_digest(&model, &[0.1, 0.2, 0.3]);
    let vec_b = semantic_vector_digest(&model, &[0.1, 0.2, 0.4]);
    if vec_a == vec_b {
        return Err("vector digest must change when vector contents change".into());
    }
    if !input_a.starts_with("search-owned-in:sha256:") {
        return Err(format!("unexpected input digest format: {input_a}").into());
    }
    if !vec_a.starts_with("search-owned-vec:sha256:") {
        return Err(format!("unexpected vector digest format: {vec_a}").into());
    }
    Ok(())
}

// CASE-COVERS (QI-BB-021 follow-up #2): under a narrow window the same
// 3-scope / 5-chunk batch costs one provider call PER WINDOW, the fake
// build consumes exactly that many windows, and what it adds up to is row
// for row what the production window derives: windowing changes residency,
// never content. The lexical builder is only reached after the semantic
// stream succeeded.
#[test]
fn corpus_derivation_embeds_one_window_per_provider_call_under_a_narrow_window() -> TestRes {
    fn materializer_under(
        policy: SemanticStreamWindowPolicy,
        embedder: Arc<CountingEmbedder>,
        semantic_builder: Arc<FakeSemanticBuilder>,
        lexical_builder: Arc<FakeSearchCorpusBuilder>,
    ) -> DirectSearchCorpusMaterializer {
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(semantic_builder));
        DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
            SearchCorpusMaterializerParts {
                builder: lexical_builder,
                ledger: Arc::new(RwLock::new(Ledger::new())),
                semantic_ingest: semantic_materializer,
                semantic_embedder: embedder,
                authority: recording_search_corpus_authority(),
                lexical_generation_validator: build_then_valid_generation(),
                semantic_generation_validator: build_then_valid_generation(),
                semantic_content_roots:
                    crate::content_roots_test_support::generation_keyed_content_roots(),
                lexical_incomplete_discard: test_incomplete_generation_discard(),
                semantic_incomplete_discard: test_incomplete_generation_discard(),
                lexical_reclaim: no_storage_sealed_reclaim(),
                semantic_reclaim: no_storage_sealed_reclaim(),
                snapshots: SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
                idempotency: memory_catalog(),
                resource_policy: IngestResourcePolicy::DEFAULT,
                semantic_stream_policy: policy,
                auxiliary_catalog: memory_aux_catalog(),
                auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
            },
        )
    }

    let batch = multi_scope_corpus_batch()?;
    let mut derived_under = Vec::new();
    for (policy, expected_calls, expected_windows) in [
        (SemanticStreamWindowPolicy::DEFAULT, 1_usize, 1_u64),
        (
            SemanticStreamWindowPolicy::new(
                2,
                SemanticStreamWindowPolicy::DEFAULT.max_vector_bytes(),
            )?,
            3,
            3,
        ),
    ] {
        let embedder = Arc::new(CountingEmbedder {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
            calls: Mutex::new(0),
        });
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let materializer = materializer_under(
            policy,
            embedder.clone(),
            semantic_builder.clone(),
            lexical_builder.clone(),
        );
        let _receipt = materializer.publish_batch(&batch)?;
        let calls = *embedder
            .calls
            .lock()
            .map_err(|err| format!("counting embedder lock poisoned: {err}"))?;
        if calls != expected_calls {
            return Err(format!(
                "expected {expected_calls} provider call(s) under {policy:?}, got {calls}"
            )
            .into());
        }
        if semantic_builder.windows_per_build()? != vec![expected_windows] {
            return Err(format!(
                "expected {expected_windows} window(s) under {policy:?}, got {:?}",
                semantic_builder.windows_per_build()?
            )
            .into());
        }
        if lexical_builder
            .batches
            .lock()
            .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
            .len()
            != 1
        {
            return Err("the lexical track builds once the semantic stream succeeded".into());
        }
        let mut derived = semantic_builder.take()?;
        derived_under.push(derived.pop().ok_or("one derived batch per publish")?);
    }
    let rows = |batch: &quanta_index_contract::SemanticIngestBatch| {
        batch
            .replace_scopes
            .iter()
            .flat_map(|scope| scope.embeddings.iter().cloned())
            .collect::<Vec<_>>()
    };
    let whole = derived_under.first().ok_or("default-window derivation")?;
    let windowed = derived_under.get(1).ok_or("narrow-window derivation")?;
    if rows(whole) != rows(windowed) || rows(whole).len() != 5 {
        return Err("windowing must not change the derived rows".into());
    }
    if windowed.replace_scopes.len() != 4
        || windowed
            .replace_scopes
            .iter()
            .map(|scope| scope.embeddings.len())
            .collect::<Vec<_>>()
            != vec![2, 1, 1, 1]
    {
        return Err(format!(
            "a path spanning windows is one fragment per window: {:?}",
            windowed
                .replace_scopes
                .iter()
                .map(|scope| scope.embeddings.len())
                .collect::<Vec<_>>()
        )
        .into());
    }
    Ok(())
}
