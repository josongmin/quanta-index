//! Evaluation over the Native producer's authenticated, retained chunk universe.

use quanta_index_contract::{
    AuxEpochV1, ChunkRecord, GenerationPin, QueryConstraintSetV1, QueryResultWindowV1,
    RuntimeMetadataCursorV1,
    native_protocol::{
        NativeDslFileV1, NativeDslQueryV1, NativeDslRuntimeCatalogV1, NativeDslRuntimeHitV1,
    },
};
use quanta_index_core::{CoreError, RequestBudgetV1};

use crate::{
    auxiliary_authority::runtime_catalog_transition,
    query_dispatcher::routes::runtime_metadata::{
        RuntimeMetadataEpochs, ensure_runtime_snapshot_names_known, execute_runtime_metadata_query,
        validate_runtime_metadata_query,
    },
    readiness::{
        DirtyDocState, RuntimeMetadataState, StructuralAuthorityState,
        validate_runtime_catalog_keys_with_checkpoint,
    },
};

pub use crate::query_dispatcher::routes::structural::execute_native_structural_v1;
pub use crate::readiness::{
    enforce_runtime_catalog_epoch_v1, runtime_catalog_chunk_ids_v1,
    validate_runtime_catalog_keys_v1,
};

/// The Native loader checks file custody and the source bytes before calling
/// this method. Continue to use the Structural owner's tree/chunk validator.
pub fn add_native_dsl_file_v1(
    state: &mut StructuralAuthorityState,
    file: NativeDslFileV1,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    budget.checkpoint("native-dsl:corpus-file")?;
    let mut authoritative_chunks = std::collections::BTreeMap::new();
    for chunk in &file.chunks {
        budget.checkpoint("native-dsl:corpus-chunk")?;
        let _previous = authoritative_chunks.insert(&chunk.chunk_id, &chunk.language);
    }
    for tree in &file.trees {
        budget.checkpoint("native-dsl:corpus-tree")?;
        if !authoritative_chunks
            .get(&tree.chunk_id)
            .is_some_and(|lang| *lang == &tree.record.lang)
        {
            return Err(CoreError::InvalidContract(
                "Native DSL parse tree belongs to another file or language".into(),
            ));
        }
    }
    drop(authoritative_chunks);
    for chunk in file.chunks {
        budget.checkpoint("native-dsl:corpus-chunk")?;
        let id = chunk.chunk_id.clone();
        if id.as_str().is_empty() || state.chunks().contains_key(&id) {
            return Err(CoreError::InvalidContract(
                "Native DSL chunk identity is empty or duplicated".into(),
            ));
        }
        state.restore_chunk(id, chunk);
    }
    for tree in file.trees {
        budget.checkpoint("native-dsl:corpus-tree")?;
        if state.parse_trees().contains_key(&tree.chunk_id) {
            return Err(CoreError::InvalidContract(
                "duplicate Native DSL parse tree".into(),
            ));
        }
        state.upsert_parse_tree(tree.chunk_id, tree.record)?;
    }
    Ok(())
}

pub fn restrict_native_structural_v1(
    state: &mut StructuralAuthorityState,
    constraints: &QueryConstraintSetV1,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    if constraints.is_unconstrained() {
        return Ok(());
    }
    budget.checkpoint("native-dsl:structural-constraints")?;
    let mut excluded = Vec::new();
    for (id, chunk) in state.chunks().iter() {
        budget.checkpoint("native-dsl:structural-constraints")?;
        if !admits_chunk(constraints, chunk) {
            excluded.push(id.clone());
        }
    }
    for id in excluded {
        budget.checkpoint("native-dsl:structural-constraints")?;
        state.remove_parse_tree(&id);
        state.remove_chunk(&id);
    }
    Ok(())
}

fn admits_chunk(constraints: &QueryConstraintSetV1, chunk: &ChunkRecord) -> bool {
    (constraints.language_any_of.is_empty()
        || constraints.language_any_of.contains(&chunk.language))
        && constraints
            .repo_relative_path_exact
            .as_ref()
            .is_none_or(|path| path.as_str() == chunk.repo_relative_path.as_str())
}

pub fn validate_native_runtime_catalog_pin_v1(
    input: &NativeDslRuntimeCatalogV1,
    pin: &GenerationPin,
) -> Result<(), CoreError> {
    if input.catalog.repo_id != pin.repo_id
        || input.catalog.revision_id != pin.revision_id
        || input.catalog.generation != pin.manifest_generation
    {
        return Err(CoreError::InvalidContract(
            "Native RuntimeMetadata catalog and chunk publication differ".into(),
        ));
    }
    Ok(())
}

/// Validate catalog keys under the Native request's publication budget.
pub fn validate_native_runtime_catalog_keys_v1(
    batch: &quanta_index_contract::RuntimeCatalogIngestBatch,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    validate_runtime_catalog_keys_with_checkpoint(batch, || {
        budget.checkpoint("native-runtime-metadata:catalog")
    })
}

/// Resolve the catalog and dirty records against the authenticated chunk
/// generation. Publication and retained reads share these membership checks.
pub fn prepare_native_runtime_metadata_v1(
    state: &StructuralAuthorityState,
    input: &NativeDslRuntimeCatalogV1,
    pin: &GenerationPin,
    budget: &RequestBudgetV1,
) -> Result<RuntimeMetadataState, CoreError> {
    budget.checkpoint("native-runtime-metadata:prepare")?;
    validate_native_runtime_catalog_pin_v1(input, pin)?;
    // The Native read ticket owns the real cursor authority.
    let epoch = AuxEpochV1::GENESIS;
    let mut runtime = RuntimeMetadataState::default();
    for record in &input.dirty {
        budget.checkpoint("native-runtime-metadata:dirty")?;
        if record.wire_version != 1
            || !state.chunks().contains_key(&record.doc_id)
            || runtime.dirty_docs().contains_key(&record.doc_id)
        {
            return Err(CoreError::InvalidContract(
                "Native RuntimeMetadata dirty doc is foreign or duplicated".into(),
            ));
        }
        runtime.restore_dirty_doc(
            record.doc_id.clone(),
            DirtyDocState::new(record.applied_at_ms, record.payload_hash),
        );
    }
    let catalog = runtime_catalog_transition(Some(state), None, epoch, &input.catalog, || {
        budget.checkpoint("native-runtime-metadata:catalog")
    })?;
    runtime.apply_catalog_delta(&catalog);
    budget.checkpoint("native-runtime-metadata:catalog")?;
    Ok(runtime)
}

pub fn validate_native_runtime_dirty_v1(
    input: &NativeDslRuntimeCatalogV1,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    let mut seen = std::collections::BTreeSet::new();
    for record in &input.dirty {
        budget.checkpoint("native-runtime-metadata:dirty")?;
        if record.wire_version != 1 || !seen.insert(&record.doc_id) {
            return Err(CoreError::InvalidContract(
                "Native RuntimeMetadata dirty doc is foreign or duplicated".into(),
            ));
        }
    }
    Ok(())
}

pub fn execute_native_runtime_metadata_v1(
    state: &StructuralAuthorityState,
    runtime: &RuntimeMetadataState,
    pin: &GenerationPin,
    query: &NativeDslQueryV1,
    budget: &RequestBudgetV1,
) -> Result<(Vec<NativeDslRuntimeHitV1>, QueryResultWindowV1), CoreError> {
    budget.checkpoint("native-runtime-metadata:prepare")?;
    let lowered = crate::lower_lexical_text_query(&query.text)?;
    validate_runtime_metadata_query(&lowered)?;
    ensure_runtime_snapshot_names_known(&lowered, runtime)?;
    let epoch = AuxEpochV1::GENESIS;
    let after = query.after.as_ref().map(|cursor| RuntimeMetadataCursorV1 {
        candidate_id: cursor.candidate_id.clone(),
        aux_epoch: epoch,
        universe_epoch: epoch,
    });
    let page = execute_runtime_metadata_query(
        pin,
        &lowered,
        runtime,
        state,
        RuntimeMetadataEpochs {
            runtime: epoch,
            universe: epoch,
        },
        query.text.top_k,
        after.as_ref(),
        |chunk| {
            budget.checkpoint("native-runtime-metadata:chunk")?;
            Ok(admits_chunk(&query.text.constraints, chunk))
        },
    )?;
    let rows = page
        .results
        .into_iter()
        .map(|row| NativeDslRuntimeHitV1 {
            candidate_id: row.candidate_id,
            source_repo_id: row.source_repo_id,
            path: row.repo_relative_path,
            start_line: row.start_line,
            end_line: row.end_line,
            snippet: row.snippet,
        })
        .collect();
    Ok((rows, page.window))
}
