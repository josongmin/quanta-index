//! Runtime-metadata query route: catalog-seeded chunk matching over the
//! runtime and structural authorities.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, LqFilter, LqQuery, LqYesNoOnly,
    RuntimeMetadataQueryRequest, SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneTrackKind,
};
use quanta_index_core::{CoreError, RequestBudgetV1, validate_query_top_k};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::{
    ERR_RUNTIME_CATALOG_NOT_READY, runtime_catalog_head_missing, runtime_snapshot_unknown,
};
use crate::query_dispatcher::ranking::stabilize_ranked_candidates;
use crate::query_dispatcher::selection::resolve_optional_selection;
use crate::query_dispatcher::text_plane::{
    ExecutableTextPlanePolicy, expr_matches, leaf_matches_text, matches_text,
    validate_executable_text_query,
};
use crate::query_dispatcher::timeref::{
    parse_runtime_changed_scope_ms, parse_runtime_stale_scope_ms,
};
use crate::query_dispatcher::window::{finalize_probe_window_v1, probe_top_k_v1, top_k_limit};
use crate::readiness::{DocFacetState, RuntimeMetadataState, StructuralAuthorityState};

impl SearchPlaneDispatcher {
    pub(crate) fn runtime_metadata(
        &self,
        request: &RuntimeMetadataQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneRuntimeMetadataQueryResponse, CoreError> {
        budget.checkpoint("runtime-metadata:entry")?;
        let _accepted_top_k = validate_query_top_k(request.text_query.top_k)?;
        let lowered = lower_lexical_text_query(&request.text_query)?;
        validate_runtime_metadata_query(&lowered)?;
        let pin = resolve_optional_selection(
            self.activation_catalog.as_ref(),
            request.text_query.generation.clone(),
            request.text_query.generation_selector.as_ref(),
            SearchPlaneTrackKind::Lexical,
            "runtime metadata",
        )?
        .ok_or_else(|| {
            CoreError::InvalidContract("runtime metadata: generation selector required".to_string())
        })?;
        // Snapshots are cloned under the read lock and scanned outside it
        // (QI-BB-020): a long scan never holds up an ingest, and an ingest
        // never holds up a query.
        let (runtime_state, structural_state) = {
            let guard = self.ledger.read().map_err(|_poisoned| {
                CoreError::Storage("search-plane ledger poisoned".to_string())
            })?;
            let snapshots = (
                guard.runtime_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation),
                guard.structural_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation),
            );
            drop(guard);
            snapshots
        };
        let runtime_state = runtime_state.ok_or_else(|| {
            CoreError::NotReady(format!(
                "runtime metadata: generation {} is not materialized",
                pin.manifest_generation.get()
            ))
        })?;
        let structural_state = structural_state.ok_or_else(|| {
            CoreError::NotReady(format!(
                "runtime metadata: lexical chunk authority for generation {} is not materialized",
                pin.manifest_generation.get()
            ))
        })?;
        if runtime_query_requires_catalog(&lowered) {
            ensure_runtime_catalog_ready(&runtime_state)?;
            ensure_runtime_snapshot_names_known(&lowered, &runtime_state)?;
        }
        budget.checkpoint("runtime-metadata:execute")?;
        // One row past the page is the continuation probe (QI-BB-025): the
        // page keeps the first `top_k` matches in seed order, and the
        // window says whether a further match exists.
        let mut results = execute_runtime_metadata_query(
            &pin,
            &lowered,
            &runtime_state,
            &structural_state,
            probe_top_k_v1(request.text_query.top_k)?,
        )?;
        let window = finalize_probe_window_v1(&mut results, request.text_query.top_k)?;
        stabilize_ranked_candidates(&mut results);
        Ok(SearchPlaneRuntimeMetadataQueryResponse {
            generation: pin,
            results,
            window,
        })
    }
}

pub(crate) fn validate_runtime_metadata_query(query: &LqQuery) -> Result<(), CoreError> {
    validate_executable_text_query(query, ExecutableTextPlanePolicy::RuntimeMetadata)?;
    if runtime_query_requires_catalog(query) {
        // Scope parsing for changed/stale is validated in validate_filter; this
        // pass is reserved for future cross-filter catalog constraints.
    }
    Ok(())
}

fn runtime_query_requires_catalog(query: &LqQuery) -> bool {
    query.filters.iter().any(|filter| {
        matches!(
            filter,
            LqFilter::Changed { .. }
                | LqFilter::Stale { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. }
        )
    })
}

fn ensure_runtime_catalog_ready(state: &RuntimeMetadataState) -> Result<(), CoreError> {
    if state.catalog_materialized() {
        Ok(())
    } else {
        Err(CoreError::Typed {
            code: ERR_RUNTIME_CATALOG_NOT_READY.to_string(),
            message: "runtime metadata: catalog is not materialized for the pinned generation"
                .to_string(),
        })
    }
}

fn ensure_runtime_snapshot_names_known(
    query: &LqQuery,
    state: &RuntimeMetadataState,
) -> Result<(), CoreError> {
    for filter in &query.filters {
        if let LqFilter::Snapshot { name } = filter
            && !state.snapshots().contains_key(name.as_str())
        {
            return Err(runtime_snapshot_unknown(name));
        }
    }
    Ok(())
}

fn execute_runtime_metadata_query(
    pin: &GenerationPin,
    query: &LqQuery,
    runtime_state: &RuntimeMetadataState,
    structural_state: &StructuralAuthorityState,
    top_k: u32,
) -> Result<Vec<quanta_index_contract::LexicalCandidate>, CoreError> {
    let limit = top_k_limit(top_k);
    let mut out = Vec::new();
    let seed_ids = runtime_seed_ids(query, runtime_state, structural_state)?;
    for chunk_id in seed_ids {
        let chunk = structural_state.chunks().get(&chunk_id).ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "runtime metadata: seeded chunk `{}` is missing from lexical chunk authority",
                chunk_id.as_str()
            ))
        })?;
        if !runtime_chunk_matches(query, runtime_state, &chunk_id, chunk)? {
            continue;
        }
        out.push(lexical_candidate_from_chunk(pin, chunk));
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

fn runtime_chunk_matches(
    query: &LqQuery,
    runtime_state: &RuntimeMetadataState,
    chunk_id: &quanta_index_contract::ChunkId,
    chunk: &ChunkRecord,
) -> Result<bool, CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Dirty { mode } => {
                let in_dirty = runtime_state.dirty_docs().contains_key(chunk_id);
                match mode {
                    LqYesNoOnly::No => {
                        if in_dirty {
                            return Ok(false);
                        }
                    }
                    // `Yes` (is-dirty) and `Only` (dirty-only) share the same
                    // membership requirement for this predicate: the chunk must
                    // be in the dirty set, else it is filtered out.
                    LqYesNoOnly::Yes | LqYesNoOnly::Only => {
                        if !in_dirty {
                            return Ok(false);
                        }
                    }
                }
            }
            LqFilter::Changed { scope } => {
                let since_ms = parse_runtime_changed_scope_ms(scope)?;
                let Some(record) = runtime_state.changed_docs().get(chunk_id) else {
                    return Ok(false);
                };
                if record.applied_at_ms() < since_ms {
                    return Ok(false);
                }
            }
            LqFilter::Stale { scope } => {
                let before_ms = parse_runtime_stale_scope_ms(scope)?;
                if !runtime_generation_is_stale(runtime_state, before_ms)? {
                    return Ok(false);
                }
            }
            LqFilter::Snapshot { name } => {
                if let Some(docs) = runtime_state.snapshots().get(name.as_str()) {
                    if !docs.contains(chunk_id) {
                        return Ok(false);
                    }
                } else {
                    return Err(runtime_snapshot_unknown(name));
                }
            }
            LqFilter::MetaOwner { id } => {
                if !runtime_doc_facet_matches(
                    runtime_state.doc_facets().get(chunk_id),
                    DocFacetState::owner,
                    id,
                ) {
                    return Ok(false);
                }
            }
            LqFilter::MetaService { id } => {
                if !runtime_doc_facet_matches(
                    runtime_state.doc_facets().get(chunk_id),
                    DocFacetState::service,
                    id,
                ) {
                    return Ok(false);
                }
            }
            LqFilter::MetaLayer { id } => {
                if !runtime_doc_facet_matches(
                    runtime_state.doc_facets().get(chunk_id),
                    DocFacetState::layer,
                    id,
                ) {
                    return Ok(false);
                }
            }
            LqFilter::MetaSurface { id } => {
                if !runtime_doc_facet_matches(
                    runtime_state.doc_facets().get(chunk_id),
                    DocFacetState::surface,
                    id,
                ) {
                    return Ok(false);
                }
            }
            LqFilter::Affected { scope } => {
                if !runtime_edge_matches(runtime_state.affected_docs(), scope, chunk_id) {
                    return Ok(false);
                }
            }
            LqFilter::InvalidatedBy { source } => {
                if !runtime_edge_matches(runtime_state.invalidated_by_docs(), source, chunk_id) {
                    return Ok(false);
                }
            }
            LqFilter::File { pattern, .. } => {
                if !matches_text(pattern, chunk.repo_relative_path.as_str(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Lang { id } => {
                if !matches_text(id, chunk.language.as_str(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Content { leaf } => {
                if !leaf_matches_text(
                    "runtime metadata",
                    leaf,
                    chunk.text.as_ref(),
                    &query.options,
                )? {
                    return Ok(false);
                }
            }
            LqFilter::Repo { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => {}
        }
    }
    expr_matches(&query.expr, &mut |leaf| {
        leaf_matches_text(
            "runtime metadata",
            leaf,
            chunk.text.as_ref(),
            &query.options,
        )
    })
}

pub(crate) fn runtime_generation_is_stale(
    runtime_state: &RuntimeMetadataState,
    before_ms: u64,
) -> Result<bool, CoreError> {
    let Some(generation_materialized_at_ms) = runtime_state.generation_materialized_at_ms() else {
        return Err(runtime_catalog_head_missing(
            "generation_materialized_at_ms",
        ));
    };
    let Some(producer_head_applied_at_ms) = runtime_state.producer_head_applied_at_ms() else {
        return Err(runtime_catalog_head_missing("producer_head_applied_at_ms"));
    };
    Ok(producer_head_applied_at_ms > generation_materialized_at_ms
        && generation_materialized_at_ms < before_ms)
}

pub(crate) fn runtime_seed_ids(
    query: &LqQuery,
    runtime_state: &RuntimeMetadataState,
    structural_state: &StructuralAuthorityState,
) -> Result<BTreeSet<ChunkId>, CoreError> {
    let full_generation = structural_state
        .chunks()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut seed: Option<BTreeSet<ChunkId>> = None;
    for filter in &query.filters {
        let next = match filter {
            LqFilter::Dirty { mode } => match mode {
                // `Yes` (is-dirty) and `Only` (dirty-only) both seed from the
                // dirty-doc set; only `No` inverts against the full generation.
                LqYesNoOnly::Yes | LqYesNoOnly::Only => Some(
                    runtime_state
                        .dirty_docs()
                        .keys()
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                ),
                LqYesNoOnly::No => Some(
                    full_generation
                        .iter()
                        .filter(|chunk_id| !runtime_state.dirty_docs().contains_key(*chunk_id))
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                ),
            },
            LqFilter::Changed { .. } => Some(
                runtime_state
                    .changed_docs()
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
            ),
            LqFilter::Stale { scope } => {
                let before_ms = parse_runtime_stale_scope_ms(scope)?;
                Some(if runtime_generation_is_stale(runtime_state, before_ms)? {
                    full_generation.clone()
                } else {
                    BTreeSet::new()
                })
            }
            LqFilter::Snapshot { name } => Some(
                runtime_state
                    .snapshots()
                    .get(name.as_str())
                    .cloned()
                    .ok_or_else(|| runtime_snapshot_unknown(name))?,
            ),
            LqFilter::MetaOwner { id } => Some(runtime_matching_facet_doc_ids(
                runtime_state,
                DocFacetState::owner,
                id,
            )),
            LqFilter::MetaService { id } => Some(runtime_matching_facet_doc_ids(
                runtime_state,
                DocFacetState::service,
                id,
            )),
            LqFilter::MetaLayer { id } => Some(runtime_matching_facet_doc_ids(
                runtime_state,
                DocFacetState::layer,
                id,
            )),
            LqFilter::MetaSurface { id } => Some(runtime_matching_facet_doc_ids(
                runtime_state,
                DocFacetState::surface,
                id,
            )),
            LqFilter::Affected { scope } => Some(
                runtime_state
                    .affected_docs()
                    .get(scope.as_str())
                    .cloned()
                    .unwrap_or_default(),
            ),
            LqFilter::InvalidatedBy { source } => Some(
                runtime_state
                    .invalidated_by_docs()
                    .get(source.as_str())
                    .cloned()
                    .unwrap_or_default(),
            ),
            LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Content { .. }
            | LqFilter::Repo { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => None,
        };
        if let Some(next) = next {
            match &mut seed {
                Some(current) => current.retain(|chunk_id| next.contains(chunk_id)),
                None => seed = Some(next),
            }
        }
    }
    Ok(seed.unwrap_or(full_generation))
}

fn runtime_matching_facet_doc_ids(
    runtime_state: &RuntimeMetadataState,
    field: impl Fn(&DocFacetState) -> Option<&str>,
    expected: &str,
) -> BTreeSet<ChunkId> {
    runtime_state
        .doc_facets()
        .iter()
        .filter(|(_, facet)| field(facet) == Some(expected))
        .map(|(chunk_id, _)| chunk_id.clone())
        .collect()
}

fn runtime_doc_facet_matches(
    facet: Option<&DocFacetState>,
    field: impl Fn(&DocFacetState) -> Option<&str>,
    expected: &str,
) -> bool {
    facet.is_some_and(|facet| field(facet).is_some_and(|value| value == expected))
}

fn runtime_edge_matches(
    edges: &BTreeMap<Box<str>, BTreeSet<quanta_index_contract::ChunkId>>,
    expected: &str,
    chunk_id: &quanta_index_contract::ChunkId,
) -> bool {
    edges
        .get(expected)
        .is_some_and(|doc_ids| doc_ids.contains(chunk_id))
}

fn lexical_candidate_from_chunk(
    pin: &GenerationPin,
    chunk: &ChunkRecord,
) -> quanta_index_contract::LexicalCandidate {
    quanta_index_contract::LexicalCandidate {
        candidate_id: chunk.chunk_id.as_str().to_string(),
        repo_id: pin.repo_id.clone(),
        revision_id: pin.revision_id.clone(),
        manifest_generation: pin.manifest_generation,
        repo_relative_path: chunk.repo_relative_path.clone(),
        start_line: chunk.start_line,
        end_line: chunk.end_line,
        score: 1.0,
        snippet: chunk.derived_snippet().to_string(),
        // A projected chunk carries no single lexical hit anchor.
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}
