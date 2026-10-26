//! Selected-view ANN search. Routing is bounded; every returned row is
//! authenticated against the pinned coverage, member, input and source roots.

use std::{cmp::Ordering, collections::BinaryHeap};

use quanta_index_contract::native_protocol::NativeSemanticTraversal;
use quanta_index_core::index::{
    IndexError, MapRoot, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey, StoredObject, WorkUsage,
    owner_from_map_key, owner_map_key,
};
use quanta_index_core::{
    DiagnosticAnnRegionObservationV1, DiagnosticAnnRegionPhaseV1, DiagnosticStageV1,
};

use super::{
    RankedHit, ReadContext, SearchQuery, SearchResult,
    ann_directory::{AnnDirectory, MAX_DELTA_OWNERS},
    cosine, finish_ranked, overlay, rank_member, search_exact, search_selected_exact_path,
    selected_owner::SelectedOwner,
    validate_search_query,
};
use crate::native::{
    ann_files, ann_lance, ann_session::AnnLanceSession, ann_store::AnnStoreLimits,
};

const MAX_ROUTED_REGIONS: usize = 8;
const TABLE_NAME: &str = "region";

struct RoutedRegions {
    entries: Vec<RoutedRegion>,
    _memory: Option<MemoryLease>,
}

struct RoutedRegion {
    score: f64,
    ordinal: u64,
    object: StoredObject,
}

#[derive(Clone, Copy)]
struct SeenOwner {
    owner: OwnerKey,
    stale: bool,
}

fn insert_seen(
    seen: &mut Vec<SeenOwner>,
    owner: OwnerKey,
    stale: bool,
    limit: usize,
    capped: &mut bool,
) -> Result<bool, IndexError> {
    match seen.binary_search_by_key(&owner, |entry| entry.owner) {
        Ok(_index) => Ok(true),
        Err(_index) if seen.len() >= limit => {
            *capped = true;
            Ok(false)
        }
        Err(index) => {
            if seen.len() == seen.capacity() {
                return Err(IndexError::ResourceLimited);
            }
            seen.insert(index, SeenOwner { owner, stale });
            Ok(true)
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "selected-view authority and bounded ranking state are explicit at the ANN callback boundary"
)]
fn rank_selected(
    owner: OwnerKey,
    expected_member: ObjectRef,
    candidate: Option<&ann_lance::RegionCandidate>,
    query: &SearchQuery<'_>,
    query_norm: f64,
    top_k: usize,
    heap: &mut BinaryHeap<RankedHit>,
    seen: &mut Vec<SeenOwner>,
    durable_limit: usize,
    capped: &mut bool,
    context: &mut ReadContext<'_>,
) -> Result<bool, IndexError> {
    let key = owner_map_key(owner);
    let Some(coverage_ref) = context
        .maps
        .lookup(query.coverage.owners, &key, context.budget)?
    else {
        // Old immutable regions can retain a deleted owner.
        return Ok(false);
    };
    if seen
        .binary_search_by_key(&owner, |entry| entry.owner)
        .is_err()
        && seen.len() >= durable_limit
    {
        *capped = true;
        return Ok(false);
    }
    let selected = SelectedOwner::load(&key, coverage_ref, query, context)?;
    let stale = selected.coverage.covered != Some(selected.coverage.desired)
        || selected.coverage.searchable.is_none();
    if !insert_seen(seen, owner, stale, durable_limit, capped)? {
        return Ok(false);
    }
    if stale {
        return Ok(false);
    }
    if selected.coverage.searchable != Some(expected_member) {
        // This selected owner superseded the immutable region member.
        return Ok(false);
    }
    let member = selected.current_member(query, context)?;
    if let Some(candidate) = candidate
        && !ann_lance::selected_input_matches(
            candidate,
            member.owner,
            member.logical_commitment,
            &candidate.vector_id,
            member.input,
        )
    {
        return Err(IndexError::CorruptObject(
            "ANN candidate member/input stamp differs".into(),
        ));
    }
    if !selected.matches_exact_path(query, context)? {
        return Ok(false);
    }
    rank_member(
        &member,
        selected.contribution,
        candidate.map(|candidate| candidate.vector_id.as_slice()),
        query,
        query_norm,
        top_k,
        heap,
        context,
    )?;
    Ok(!query
        .overlay
        .as_ref()
        .is_some_and(|overlay| overlay::masks(overlay.documents, owner)))
}

fn route_regions(
    directory: &AnnDirectory,
    query: &SearchQuery<'_>,
    query_norm: f64,
    context: &mut ReadContext<'_>,
) -> Result<RoutedRegions, IndexError> {
    let route_bytes = MAX_ROUTED_REGIONS
        .checked_mul(std::mem::size_of::<RoutedRegion>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = context.budget.reserve_memory(
        u64::try_from(route_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut routes = Vec::new();
    routes
        .try_reserve_exact(MAX_ROUTED_REGIONS)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if routes.capacity() > MAX_ROUTED_REGIONS {
        return Err(IndexError::ResourceLimited);
    }
    let mut after = None;
    let mut count = 0u32;
    loop {
        let page = directory.region_page(after, 64, context)?;
        for (key, reference) in &page.entries {
            let reference = *reference.as_object_ref()?;
            let ordinal = u64::from_be_bytes(key.as_slice().try_into().map_err(|_error| {
                IndexError::CorruptObject("ANN region key width differs".into())
            })?);
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if count > directory.region_count {
                return Err(IndexError::CorruptObject(
                    "ANN region map count exceeded".into(),
                ));
            }
            context.budget.checkpoint()?;
            let descriptor = directory.region_from_page(ordinal, reference, context)?;
            if descriptor.descriptor.center.len() != query.vector.len() {
                return Err(IndexError::CorruptObject(
                    "ANN region center dimension differs from query recipe".into(),
                ));
            }
            let score = cosine(query.vector, query_norm, &descriptor.descriptor.center)?;
            let object = descriptor.into_stored_object();
            if routes.len() < MAX_ROUTED_REGIONS {
                routes.push(RoutedRegion {
                    score,
                    ordinal,
                    object,
                });
            } else if let Some(worst) = routes.last()
                && (score.total_cmp(&worst.score) == Ordering::Greater
                    || (score.total_cmp(&worst.score) == Ordering::Equal
                        && ordinal < worst.ordinal))
            {
                // Release displaced DATA custody before retaining the replacement.
                drop(routes.pop());
                routes.push(RoutedRegion {
                    score,
                    ordinal,
                    object,
                });
            }
            routes.sort_by(|left, right| {
                right
                    .score
                    .total_cmp(&left.score)
                    .then_with(|| left.ordinal.cmp(&right.ordinal))
            });
            after = Some(ordinal);
        }
        if !page.has_more {
            break;
        }
    }
    if count != directory.region_count {
        return Err(IndexError::CorruptObject(
            "ANN region map count differs".into(),
        ));
    }
    Ok(RoutedRegions {
        entries: routes,
        _memory: memory,
    })
}

pub(super) async fn search(
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    let query_norm = validate_search_query(query)?;
    let directory_ref = match query.coverage.searchable {
        Some(reference) => reference,
        None if query.coverage.owners == MapRoot(None)
            && query
                .overlay
                .as_ref()
                .is_some_and(|overlay| !overlay.documents.is_empty()) =>
        {
            return search_exact(query, context);
        }
        None => return Err(IndexError::SnapshotUnavailable),
    };
    let directory = AnnDirectory::load(directory_ref, query.recipe.fingerprint(), context)?;
    if directory.owners != query.coverage.owners {
        return Err(IndexError::CorruptObject(
            "ANN directory owners differ from selected coverage".into(),
        ));
    }
    if let Some(result) = super::lexical_scope::search(query, context)? {
        return Ok(result);
    }
    if let Some(result) = search_selected_exact_path(query, context)? {
        return Ok(result);
    }
    let small_exact = if directory.region_count == 0 {
        let selected = context.maps.scan(
            query.coverage.owners,
            None,
            MAX_DELTA_OWNERS,
            context.budget,
        )?;
        if selected.entries.len()
            > usize::try_from(MAX_DELTA_OWNERS).map_err(|_error| IndexError::ResourceLimited)?
            || (selected.entries.is_empty() && selected.has_more)
        {
            return Err(IndexError::BackendViolation(
                "ANN small-population scan exceeded its bounded page".into(),
            ));
        }
        !selected.has_more
    } else {
        false
    };
    let top_k = usize::try_from(query.top_k).map_err(|_error| IndexError::ResourceLimited)?;
    let overlay_rows = query
        .overlay
        .as_ref()
        .map_or(0, |overlay| overlay.documents.len());
    let max_owners =
        usize::try_from(query.max_owners).map_err(|_error| IndexError::ResourceLimited)?;
    let durable_limit = max_owners
        .checked_sub(overlay_rows)
        .ok_or(IndexError::ResourceLimited)?;
    let delta_owners =
        usize::try_from(directory.delta_owners).map_err(|_error| IndexError::ResourceLimited)?;
    let max_candidates = MAX_ROUTED_REGIONS
        .checked_mul(ann_lance::MAX_REGION_CANDIDATES)
        .and_then(|count| count.checked_add(delta_owners))
        .ok_or(IndexError::ResourceLimited)?;
    let seen_capacity = durable_limit.min(max_candidates);
    let seen_bytes = seen_capacity
        .checked_mul(std::mem::size_of::<SeenOwner>())
        .ok_or(IndexError::ResourceLimited)?;
    // Empty populations and overlay-only capacity allocate no owner table.
    // A live memory authority deliberately rejects zero-byte reservations.
    context.budget.checkpoint()?;
    let _seen_memory = if seen_bytes == 0 {
        None
    } else {
        context.budget.reserve_memory(
            u64::try_from(seen_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    let mut seen = Vec::new();
    seen.try_reserve_exact(seen_capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let heap_bytes = top_k
        .checked_mul(std::mem::size_of::<RankedHit>())
        .ok_or(IndexError::ResourceLimited)?;
    let _heap_memory = context.budget.reserve_memory(
        u64::try_from(heap_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut heap = BinaryHeap::new();
    heap.try_reserve_exact(top_k)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut capped = top_k > ann_lance::MAX_REGION_CANDIDATES
        || u64::from(directory.region_count)
            > u64::try_from(MAX_ROUTED_REGIONS).map_err(|_error| IndexError::ResourceLimited)?;

    // The bounded delta is exact over its current owner members.
    let mut after: Option<[u8; 38]> = None;
    let mut delta_entries = 0u32;
    loop {
        context.budget.checkpoint()?;
        let page = context.maps.scan(
            directory.delta,
            after.as_ref().map(<[u8; 38]>::as_slice),
            query.page_size.min(MAX_DELTA_OWNERS),
            context.budget,
        )?;
        if page.entries.is_empty() && page.has_more {
            return Err(IndexError::BackendViolation(
                "ANN delta map scan made no progress".into(),
            ));
        }
        for (key, member_ref) in &page.entries {
            let member_ref = member_ref.as_object_ref()?;
            context.budget.checkpoint()?;
            delta_entries = delta_entries
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if delta_entries > MAX_DELTA_OWNERS {
                return Err(IndexError::CorruptObject(
                    "ANN delta map exceeds owner bound".into(),
                ));
            }
            let owner = owner_from_map_key(key)?;
            let _binding = directory.delta_member_from_page(owner, *member_ref, context)?;
            if !small_exact {
                let _accepted = rank_selected(
                    owner,
                    *member_ref,
                    None,
                    query,
                    query_norm,
                    top_k,
                    &mut heap,
                    &mut seen,
                    durable_limit,
                    &mut capped,
                    context,
                )?;
            }
        }
        if !page.has_more {
            break;
        }
        let last = page.entries.last().ok_or_else(|| {
            IndexError::BackendViolation("ANN delta scan omitted continuation key".into())
        })?;
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(last.0.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        after = Some(last.0.as_slice().try_into().map_err(|_error| {
            IndexError::CorruptObject("ANN delta cursor width differs".into())
        })?);
    }
    if delta_entries != directory.delta_owners {
        return Err(IndexError::CorruptObject(
            "ANN delta map count differs from directory".into(),
        ));
    }
    if small_exact {
        // This small population has a bounded independent exact oracle.
        return search_exact(query, context);
    }

    let routes = route_regions(&directory, query, query_norm, context)?;
    for routed in routes.entries {
        let span = crate::build::begin_build_stage(
            context.budget.request().diagnostic_context(),
            DiagnosticStageV1::QueryCandidate,
        );
        let region_result = async {
            context.budget.checkpoint()?;
            let ordinal = routed.ordinal;
            let descriptor = directory.region_from_retained(ordinal, routed.object, context)?;
            let fileset = ann_files::load_fileset(
                context.objects,
                descriptor.descriptor.fileset,
                context.budget,
            )?;
            let seal_dimension = usize::try_from(fileset.seal.dimension)
                .map_err(|_error| IndexError::ResourceLimited)?;
            if fileset.seal.recipe != directory.recipe
                || fileset.seal.generation
                    != ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?
                || fileset.seal.rows != descriptor.descriptor.rows
                || seal_dimension != descriptor.descriptor.center.len()
            {
                return Err(IndexError::CorruptObject(
                    "ANN selected region seal differs from directory".into(),
                ));
            }
            let max_read_bytes = fileset
                .total_length
                .checked_mul(8)
                .and_then(|bytes| bytes.checked_add(1_048_576))
                .ok_or(IndexError::ResourceLimited)?
                .min(64 * 1024 * 1024);
            let limits = AnnStoreLimits {
                max_write_bytes: fileset.total_length.max(1),
                max_read_bytes,
                ..AnnStoreLimits::default()
            };
            let session = AnnLanceSession::hydrate(&fileset, limits, context.budget).await?;
            let connection = session.connect().await?;
            let table = connection
                .open_table(TABLE_NAME)
                .execute()
                .await
                .map_err(|error| IndexError::Storage(format!("ANN selected table: {error}")));
            let table = session.resolve(table)?;
            let opened = ann_lance::open_region(table, fileset.seal.clone(), context.budget).await;
            let region = session.resolve(opened)?;
            let region_top_k = top_k.min(ann_lance::MAX_REGION_CANDIDATES);
            let candidate_cap = ann_lance::MAX_REGION_CANDIDATES;
            let objects = context.objects;
            let maps = context.maps;
            let members = descriptor.descriptor.members;
            let page = ann_lance::query_region(
                &region,
                query.vector,
                region_top_k,
                region_top_k,
                candidate_cap,
                context.budget,
                |candidate, budget| {
                    let mut read = ReadContext {
                        objects,
                        maps,
                        budget,
                    };
                    let key = owner_map_key(candidate.owner);
                    let Some(member_ref) = read.maps.lookup_object(members, &key, read.budget)?
                    else {
                        return Err(IndexError::CorruptObject(
                            "ANN candidate is absent from sealed region members".into(),
                        ));
                    };
                    let accepted = rank_selected(
                        candidate.owner,
                        member_ref,
                        Some(candidate),
                        query,
                        query_norm,
                        top_k,
                        &mut heap,
                        &mut seen,
                        durable_limit,
                        &mut capped,
                        &mut read,
                    )?;
                    Ok(accepted)
                },
            )
            .await;
            let page = session.resolve(page)?;
            if let (Some(diagnostic), Some(span)) =
                (context.budget.request().diagnostic_context(), span.as_ref())
            {
                let files = u32::try_from(fileset.files.len())
                    .map_err(|_error| IndexError::ResourceLimited)?;
                let chunks = fileset.files.iter().try_fold(0u32, |count, file| {
                    count
                        .checked_add(
                            u32::try_from(file.chunks.len())
                                .map_err(|_error| IndexError::ResourceLimited)?,
                        )
                        .ok_or(IndexError::ResourceLimited)
                })?;
                diagnostic.ann_region_observation(
                    span,
                    DiagnosticAnnRegionObservationV1 {
                        phase: DiagnosticAnnRegionPhaseV1::SelectedQuery,
                        fileset_ref: descriptor.descriptor.fileset.0.0,
                        descriptor_ref: Some(descriptor.reference().0.0),
                        ordinal,
                        rows: fileset.seal.rows,
                        index_kind: ann_files::diagnostic_index_kind(&fileset.seal)?,
                        files,
                        chunks,
                    },
                );
            }
            Ok::<_, IndexError>(page.traversal)
        }
        .await;
        crate::build::finish_build_stage(span, region_result.is_ok());
        if !matches!(region_result?, NativeSemanticTraversal::ExactExhausted) {
            capped = true;
        }
    }
    let scanned_owners = u32::try_from(seen.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let skipped_stale = u32::try_from(seen.iter().filter(|entry| entry.stale).count())
        .map_err(|_error| IndexError::ResourceLimited)?;
    finish_ranked(
        query,
        query_norm,
        top_k,
        heap,
        scanned_owners,
        skipped_stale,
        NativeSemanticTraversal::Approximate { capped },
        context,
    )
}
