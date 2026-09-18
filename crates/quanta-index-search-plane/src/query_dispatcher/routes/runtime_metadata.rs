//! Runtime-metadata query route: catalog-seeded chunk matching over the
//! runtime and structural authorities, paged by keyset cursor.
//!
//! A page is cut from one consistent cut of two epoch-named snapshots
//! (QI-BB-020 W2): the runtime authority its predicates read and the
//! structural authority its chunk universe is joined from. Both are the
//! request's read view (`read_view.rs`), taken under one ledger read: a
//! fresh walk pins both current snapshots; a continuation pins the two
//! epochs its cursor names, and the response says which. A cursor whose
//! epoch is no longer retained is refused typed; it is never served from
//! a newer snapshot where a row could repeat, go missing or appear from
//! nowhere.
//!
//! The walk is a stream, not a materialization (QI-BB-025 W4): it drives
//! the narrowest authority set the query's filters name — or the chunk
//! universe — in candidate-id order, seeks past the cursor, evaluates the
//! complete predicate on each chunk, and keeps at most `top_k + 1` keys.
//! It stops at the first key past the page, so the window is a probe:
//! at least what it saw, exact only when the stream ran out.

use std::collections::BTreeSet;
use std::ops::Bound;

use imbl::OrdMap;
use quanta_index_contract::{
    AuxEpochV1, ChunkId, ChunkRecord, GenerationPin, LexicalCandidate, LqFilter, LqQuery,
    LqYesNoOnly, QueryResultWindowV1, RuntimeMetadataCursorV1, RuntimeMetadataQueryRequest,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneTrackKind,
};
use quanta_index_core::{CoreError, QueryRouteV1, RequestBudgetV1, validate_query_top_k};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::{
    ERR_RUNTIME_CATALOG_NOT_READY, runtime_catalog_head_missing, runtime_snapshot_unknown,
};
use crate::query_dispatcher::keyset_page::{KeysetPageCollector, StreamEnd};
use crate::query_dispatcher::read_view::{AuxEpochPinsV1, ReadViewRequestV1};
use crate::query_dispatcher::selection::resolve_optional_selection;
use crate::query_dispatcher::text_plane::{
    ExecutableTextPlanePolicy, expr_matches, leaf_matches_text, matches_text,
    validate_executable_text_query,
};
use crate::query_dispatcher::timeref::{
    parse_runtime_changed_scope_ms, parse_runtime_stale_scope_ms,
};
use crate::readiness::{
    AuxRead, ChangedDocState, DirtyDocState, DocFacetState, RuntimeMetadataState,
    StructuralAuthorityState,
};

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
        // Both snapshots are the read view's, taken under one read lock
        // and scanned outside it (QI-BB-020): a long scan never holds up
        // an ingest, an ingest never holds up a query.
        let view = self.acquire_read_view(
            &ReadViewRequestV1::declare(
                "runtime metadata",
                QueryRouteV1::RuntimeMetadata,
                Some(&lowered),
                &pin,
            )
            .with_epochs(AuxEpochPinsV1 {
                history: None,
                runtime: request.cursor.as_ref().map(|cursor| cursor.aux_epoch),
                structural: request.cursor.as_ref().map(|cursor| cursor.universe_epoch),
            }),
        )?;
        let read = RuntimeMetadataRead {
            runtime: view.runtime()?.clone(),
            universe: view.structural()?.clone(),
        };
        if runtime_query_requires_catalog(&lowered) {
            ensure_runtime_catalog_ready(&read.runtime.state)?;
            ensure_runtime_snapshot_names_known(&lowered, &read.runtime.state)?;
        }
        budget.checkpoint("runtime-metadata:execute")?;
        let epochs = read.epochs();
        let page = execute_runtime_metadata_query(
            &pin,
            &lowered,
            &read.runtime.state,
            &read.universe.state,
            epochs,
            request.text_query.top_k,
            request.cursor.as_ref(),
        )?;
        Ok(SearchPlaneRuntimeMetadataQueryResponse {
            generation: pin,
            results: page.results,
            window: page.window,
            read_epoch: epochs.runtime,
            universe_epoch: epochs.universe,
            examined: page.examined,
            next_cursor: page.next_cursor,
        })
    }
}

/// The two snapshots one runtime-metadata page reads: the runtime
/// authority for its predicates and the structural authority for the
/// chunk universe it joins them with.
#[derive(Debug)]
pub(crate) struct RuntimeMetadataRead {
    pub(crate) runtime: AuxRead<RuntimeMetadataState>,
    pub(crate) universe: AuxRead<StructuralAuthorityState>,
}

/// The epochs one runtime-metadata page is cut from; its continuation
/// names both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeMetadataEpochs {
    pub(crate) runtime: AuxEpochV1,
    pub(crate) universe: AuxEpochV1,
}

impl RuntimeMetadataRead {
    pub(crate) const fn epochs(&self) -> RuntimeMetadataEpochs {
        RuntimeMetadataEpochs {
            runtime: self.runtime.epoch,
            universe: self.universe.epoch,
        }
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

/// One page of runtime-metadata results in candidate-id order.
#[derive(Debug)]
pub(crate) struct RuntimeMetadataPage {
    pub(crate) results: Vec<LexicalCandidate>,
    pub(crate) window: QueryResultWindowV1,
    pub(crate) examined: u64,
    pub(crate) next_cursor: Option<RuntimeMetadataCursorV1>,
}

/// The candidate stream one runtime-metadata query walks (QI-BB-025 W4).
///
/// Each variant is one authority set keyed by chunk id, so it iterates
/// in candidate-id order — the route's total order — and seeks past a
/// cursor in logarithmic time. Which one drives the walk is a choice of
/// cost, never of meaning: every chunk it yields is still held to the
/// complete predicate ([`runtime_chunk_matches`]), which re-checks the
/// filter the driver was chosen for.
enum RuntimeDriver<'a> {
    /// The whole chunk universe of the pinned structural snapshot.
    Universe(&'a OrdMap<ChunkId, ChunkRecord>),
    /// The dirty overlay.
    DirtyDocs(&'a OrdMap<ChunkId, DirtyDocState>),
    /// The catalog's changed docs.
    ChangedDocs(&'a OrdMap<ChunkId, ChangedDocState>),
    /// The catalog's facet rows; the facet value is checked per chunk.
    DocFacets(&'a OrdMap<ChunkId, DocFacetState>),
    /// One named snapshot or edge-authority set.
    IdSet(&'a BTreeSet<ChunkId>),
    /// A filter no chunk can satisfy: `stale:` on a generation that is
    /// not stale, or an edge key the authority does not carry.
    Empty,
}

impl<'a> RuntimeDriver<'a> {
    /// How many chunk ids the stream holds in all: the cost of walking
    /// it from the start. For facet rows this counts every row, whatever
    /// its value.
    fn len(&self) -> usize {
        match self {
            Self::Universe(map) => map.len(),
            Self::DirtyDocs(map) => map.len(),
            Self::ChangedDocs(map) => map.len(),
            Self::DocFacets(map) => map.len(),
            Self::IdSet(set) => set.len(),
            Self::Empty => 0,
        }
    }

    /// The chunk ids strictly after `after` — all of them for `None` —
    /// in ascending order.
    fn ids_after(&self, after: Option<&'a ChunkId>) -> Box<dyn Iterator<Item = &'a ChunkId> + 'a> {
        let bounds: (Bound<&ChunkId>, Bound<&ChunkId>) = (
            after.map_or(Bound::Unbounded, Bound::Excluded),
            Bound::Unbounded,
        );
        match *self {
            Self::Universe(map) => Box::new(keys_after(map, bounds)),
            Self::DirtyDocs(map) => Box::new(keys_after(map, bounds)),
            Self::ChangedDocs(map) => Box::new(keys_after(map, bounds)),
            Self::DocFacets(map) => Box::new(keys_after(map, bounds)),
            Self::IdSet(set) => Box::new(set.range::<ChunkId, _>(bounds)),
            Self::Empty => Box::new(std::iter::empty()),
        }
    }
}

/// The keys of a chunk-keyed map within `bounds`, ascending.
fn keys_after<'a, V>(
    map: &'a OrdMap<ChunkId, V>,
    bounds: (Bound<&'a ChunkId>, Bound<&'a ChunkId>),
) -> impl Iterator<Item = &'a ChunkId> + 'a
where
    V: Clone,
{
    map.range::<_, ChunkId>(bounds)
        .map(|(chunk_id, _)| chunk_id)
}

/// Choose the narrowest stream the query's filters name; the chunk
/// universe when none narrows it.
///
/// `stale:` is a property of the generation, decided once here: a
/// generation that is not stale yields nothing. An edge key the authority
/// does not carry likewise names an empty set — that is its meaning, not
/// a failure. An unknown snapshot name is a typed refusal.
fn runtime_driver<'a>(
    query: &LqQuery,
    runtime_state: &'a RuntimeMetadataState,
    structural_state: &'a StructuralAuthorityState,
) -> Result<RuntimeDriver<'a>, CoreError> {
    let mut driver = RuntimeDriver::Universe(structural_state.chunks());
    for filter in &query.filters {
        let narrower = match filter {
            LqFilter::Dirty {
                mode: LqYesNoOnly::Yes | LqYesNoOnly::Only,
            } => Some(RuntimeDriver::DirtyDocs(runtime_state.dirty_docs())),
            LqFilter::Changed { .. } => {
                Some(RuntimeDriver::ChangedDocs(runtime_state.changed_docs()))
            }
            LqFilter::Stale { scope } => {
                let before_ms = parse_runtime_stale_scope_ms(scope)?;
                if runtime_generation_is_stale(runtime_state, before_ms)? {
                    None
                } else {
                    Some(RuntimeDriver::Empty)
                }
            }
            LqFilter::Snapshot { name } => Some(RuntimeDriver::IdSet(
                runtime_state
                    .snapshots()
                    .get(name.as_str())
                    .ok_or_else(|| runtime_snapshot_unknown(name))?,
            )),
            LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. } => {
                Some(RuntimeDriver::DocFacets(runtime_state.doc_facets()))
            }
            LqFilter::Affected { scope } => Some(
                runtime_state
                    .affected_docs()
                    .get(scope.as_str())
                    .map_or(RuntimeDriver::Empty, RuntimeDriver::IdSet),
            ),
            LqFilter::InvalidatedBy { source } => Some(
                runtime_state
                    .invalidated_by_docs()
                    .get(source.as_str())
                    .map_or(RuntimeDriver::Empty, RuntimeDriver::IdSet),
            ),
            // `dirty:no` is the complement of the overlay: only the
            // universe holds it. The rest are per-chunk predicates.
            LqFilter::Dirty {
                mode: LqYesNoOnly::No,
            }
            | LqFilter::File { .. }
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
        if let Some(narrower) = narrower
            && narrower.len() < driver.len()
        {
            driver = narrower;
        }
    }
    Ok(driver)
}

/// Walk the query's candidate stream after `cursor` and cut one page of
/// `top_k` rows in candidate-id order (QI-BB-025 W4).
///
/// `epochs` names the snapshots `runtime_state` and `structural_state`
/// are; the page's continuation carries them. The stream is in key
/// order, so the walk stops at the first key past the page: the window
/// is then a lower bound, and exact only when the stream ran out first.
/// Memory is bounded by the page; nothing but the selected rows is
/// materialized.
pub(crate) fn execute_runtime_metadata_query(
    pin: &GenerationPin,
    query: &LqQuery,
    runtime_state: &RuntimeMetadataState,
    structural_state: &StructuralAuthorityState,
    epochs: RuntimeMetadataEpochs,
    top_k: u32,
    cursor: Option<&RuntimeMetadataCursorV1>,
) -> Result<RuntimeMetadataPage, CoreError> {
    let driver = runtime_driver(query, runtime_state, structural_state)?;
    let after = cursor.map(|cursor| ChunkId::new(cursor.candidate_id.clone()));
    // The driver seeks past the cursor; the collector holds the boundary
    // regardless of what the stream yields.
    let mut collector = KeysetPageCollector::new(top_k, after.clone())?;
    let mut end = StreamEnd::Exhausted;
    for chunk_id in driver.ids_after(after.as_ref()) {
        collector.examined_one();
        let chunk = structural_state.chunks().get(chunk_id).ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "runtime metadata: seeded chunk `{}` is missing from lexical chunk authority",
                chunk_id.as_str()
            ))
        })?;
        if !runtime_chunk_matches(query, runtime_state, chunk_id, chunk)? {
            continue;
        }
        collector.offer(chunk_id);
        if collector.is_full() {
            end = StreamEnd::Stopped;
            break;
        }
    }
    let page = collector.finish(end)?;
    let results = page
        .keys
        .iter()
        .map(|chunk_id| {
            structural_state
                .chunks()
                .get(chunk_id)
                .map(|chunk| lexical_candidate_from_chunk(pin, chunk))
                .ok_or_else(|| {
                    CoreError::Storage(format!(
                        "runtime metadata: selected chunk `{}` vanished from the snapshot",
                        chunk_id.as_str()
                    ))
                })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    let next_cursor = page.next_key.map(|chunk_id| RuntimeMetadataCursorV1 {
        candidate_id: chunk_id.into_inner(),
        aux_epoch: epochs.runtime,
        universe_epoch: epochs.universe,
    });
    Ok(RuntimeMetadataPage {
        results,
        window: page.window,
        examined: page.examined,
        next_cursor,
    })
}

/// The complete predicate one chunk must satisfy: every filter of the
/// query, including the one its stream was chosen for, then the
/// expression over the chunk text.
fn runtime_chunk_matches(
    query: &LqQuery,
    runtime_state: &RuntimeMetadataState,
    chunk_id: &ChunkId,
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

fn runtime_doc_facet_matches(
    facet: Option<&DocFacetState>,
    field: impl Fn(&DocFacetState) -> Option<&str>,
    expected: &str,
) -> bool {
    facet.is_some_and(|facet| field(facet).is_some_and(|value| value == expected))
}

fn runtime_edge_matches(
    edges: &OrdMap<Box<str>, BTreeSet<ChunkId>>,
    expected: &str,
    chunk_id: &ChunkId,
) -> bool {
    edges
        .get(expected)
        .is_some_and(|doc_ids| doc_ids.contains(chunk_id))
}

/// Project one chunk of the pinned universe as a page row.
///
/// The route ranks nothing, so every row carries the constant score
/// `1.0`: the page's order is the candidate-id order the cursor names,
/// never a score order.
fn lexical_candidate_from_chunk(pin: &GenerationPin, chunk: &ChunkRecord) -> LexicalCandidate {
    LexicalCandidate {
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
