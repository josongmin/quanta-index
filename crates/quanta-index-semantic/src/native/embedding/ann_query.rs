//! Selected-view ANN search. Routing is bounded; every returned row is
//! authenticated against the pinned coverage, member, input and source roots.

use std::{cmp::Ordering, collections::BinaryHeap};

use quanta_index_contract::native_protocol::NativeSemanticTraversal;
use quanta_index_core::index::{
    IndexError, MapRoot, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey, OwnerMemberBinding,
    StoredObject, WorkUsage, owner_from_map_key, owner_map_key,
};
use quanta_index_core::{
    DiagnosticAnnIndexKindV1, DiagnosticAnnRegionObservationV1, DiagnosticAnnRegionPhaseV1,
    DiagnosticStageV1,
};

use super::native_graph;
use super::{
    ChunkMember, EmbeddingVector, LoadedOwnerArtifact, OwnerArtifact, RankedHit, ReadContext,
    SearchQuery, SearchResult, SemanticSourceFile,
    ann_directory::{AnnDirectory, MAX_DELTA_OWNERS},
    cosine, finish_ranked, load_rank_member, rank_decoded_member, rank_member, search_exact,
    search_exact_with_delta, search_selected_exact_path,
    selected_owner::{SelectedOwner, scan_ordered_page},
    source_file, validate_rank_vector, validate_search_query,
};

const MAX_ROUTED_REGIONS: usize = 8;
const MAX_CACHED_ARTIFACT_BYTES: usize = 512 * 1024;
const MAX_CACHED_MEMBERS: usize = 8;

const fn region_was_capped(traversal: NativeSemanticTraversal) -> bool {
    matches!(
        traversal,
        NativeSemanticTraversal::ExactCapped
            | NativeSemanticTraversal::Approximate { capped: true }
    )
}

struct RoutedRegions {
    entries: Vec<RoutedRegion>,
    _memory: Option<MemoryLease>,
}

struct RoutedRegion {
    score: f64,
    ordinal: u64,
    object: StoredObject,
}

struct SeenOwner {
    owner: OwnerKey,
    stale: bool,
    member: Option<(ObjectRef, OwnerMemberBinding)>,
}

struct CachedMember {
    reference: ObjectRef,
    artifact: OwnerArtifact,
    bytes: usize,
}

struct RegionArtifactCache<'a> {
    members: [Option<CachedMember>; MAX_CACHED_MEMBERS],
    next: usize,
    candidates: &'a native_graph::Candidates,
}

impl<'a> RegionArtifactCache<'a> {
    fn new(candidates: &'a native_graph::Candidates) -> Self {
        Self {
            members: Default::default(),
            next: 0,
            candidates,
        }
    }

    fn capacity(&self) -> usize {
        self.candidates
            .reusable_scratch_bytes()
            .min(MAX_CACHED_ARTIFACT_BYTES)
    }

    fn get(&mut self, reference: ObjectRef) -> Option<&mut OwnerArtifact> {
        self.members
            .iter_mut()
            .flatten()
            .find(|entry| entry.reference == reference)
            .map(|entry| &mut entry.artifact)
    }

    fn remember(&mut self, reference: ObjectRef, loaded: LoadedOwnerArtifact) {
        let capacity = self.capacity();
        let bytes = decoded_backing(&loaded);
        if bytes > capacity {
            return;
        }
        let mut live_bytes = bytes;
        self.members[self.next] = None;
        for cached in &mut self.members {
            if let Some(member) = cached {
                live_bytes += member.bytes;
                if live_bytes > capacity {
                    live_bytes -= member.bytes;
                    *cached = None;
                }
            }
        }
        // Only the decoded artifact transfers to the candidates' scratch.
        // The source record, if still present, keeps its own decode grant.
        self.members[self.next] = Some(CachedMember {
            reference,
            artifact: loaded.artifact,
            bytes,
        });
        self.next = (self.next + 1) % MAX_CACHED_MEMBERS;
    }
}

fn load_cached_file(
    member: &OwnerMemberBinding,
    selected: &SelectedOwner,
    heap: &BinaryHeap<RankedHit>,
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<Option<SemanticSourceFile>, IndexError> {
    if !query.source_files {
        return Ok(None);
    }
    let file_owner = selected
        .contribution
        .map_or(Some(member.owner), |contribution| contribution.source_owner);
    let record = file_owner
        .zip(member.input.source)
        .and_then(|(owner, source)| source_file::retained_record(heap, owner, source));
    source_file::load(member, selected.contribution, record, query, context)
}

fn decoded_backing(artifact: &LoadedOwnerArtifact) -> usize {
    let vectors = &artifact.vectors;
    let mut bytes = vectors.capacity() * std::mem::size_of::<EmbeddingVector>()
        + artifact.chunks.capacity() * std::mem::size_of::<ChunkMember>();
    for vector in vectors {
        bytes += vector.id.capacity() + vector.values.capacity() * std::mem::size_of::<f32>();
    }
    bytes
}

enum Candidate<'a> {
    Delta(&'a OwnerMemberBinding),
    Region(&'a native_graph::Row),
}

/// Canonical bindings from a complete, ordered scan of the bounded delta.
pub(super) struct ExactDeltaMembers {
    entries: Vec<(ObjectRef, OwnerMemberBinding)>,
    _memory: Option<MemoryLease>,
}

impl ExactDeltaMembers {
    fn reserve(count: usize, context: &ReadContext<'_>) -> Result<Self, IndexError> {
        let bytes = count
            .checked_mul(std::mem::size_of::<(ObjectRef, OwnerMemberBinding)>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            None
        } else {
            context.budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
        };
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if entries.capacity() > count {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            entries,
            _memory: memory,
        })
    }

    pub(super) fn current(
        &self,
        owner: OwnerKey,
        reference: ObjectRef,
    ) -> Option<&OwnerMemberBinding> {
        let index = self
            .entries
            .binary_search_by_key(&owner, |(_, member)| member.owner)
            .ok()?;
        let (actual, member) = self.entries.get(index)?;
        (*actual == reference).then_some(member)
    }
}

fn insert_seen<'a>(
    seen: &'a mut Vec<SeenOwner>,
    owner: OwnerKey,
    stale: bool,
    limit: usize,
    capped: &mut bool,
) -> Result<Option<&'a mut SeenOwner>, IndexError> {
    match seen.binary_search_by_key(&owner, |entry| entry.owner) {
        Ok(index) => Ok(seen.get_mut(index)),
        Err(_index) if seen.len() >= limit => {
            *capped = true;
            Ok(None)
        }
        Err(index) => {
            if seen.len() == seen.capacity() {
                return Err(IndexError::ResourceLimited);
            }
            seen.insert(
                index,
                SeenOwner {
                    owner,
                    stale,
                    member: None,
                },
            );
            Ok(seen.get_mut(index))
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
    candidate: Candidate<'_>,
    query: &SearchQuery<'_>,
    query_norm: f64,
    top_k: usize,
    heap: &mut BinaryHeap<RankedHit>,
    seen: &mut Vec<SeenOwner>,
    durable_limit: usize,
    capped: &mut bool,
    mut cached: Option<&mut RegionArtifactCache<'_>>,
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
    let coverage = quanta_index_core::index::read_owner_coverage(
        coverage_ref,
        &key,
        context.objects,
        context.budget,
    )?;
    let current = coverage.covered == Some(coverage.desired);
    if current && coverage.searchable.is_some() && coverage.searchable != Some(expected_member) {
        // A superseded candidate cannot claim the current replacement's slot.
        return Ok(false);
    }
    let selected = SelectedOwner::authenticate(
        &key,
        coverage,
        query.coverage,
        query.projection.as_ref(),
        query.source_owners,
        context,
    )?;
    let cached_member = if matches!(candidate, Candidate::Region(_)) {
        cached.as_mut().and_then(|cache| cache.get(expected_member))
    } else {
        None
    };
    let selected_input = if cached_member.is_none() && query.source_files {
        // Projection members can share a file without sharing an artifact.
        let record = selected.contribution.and_then(|contribution| {
            source_file::retained_record(heap, contribution.source_owner?, contribution.source?)
        });
        let std::ops::ControlFlow::Continue(input) = selected.rank_input(query, record, context)?
        else {
            return Ok(false);
        };
        input
    } else {
        if cached_member.is_none() && selected.has_no_candidate(false) {
            return Ok(false);
        }
        None
    };
    let stale = !current || coverage.searchable.is_none();
    let Some(seen_owner) = insert_seen(seen, owner, stale, durable_limit, capped)? else {
        return Ok(false);
    };
    if stale {
        return Ok(false);
    }
    let (member, only) = match candidate {
        Candidate::Delta(member) => {
            selected.validate_member_identity(member, query)?;
            seen_owner.member = Some((expected_member, member.clone()));
            (member, None)
        }
        Candidate::Region(row) => {
            // The selected immutable coverage still points to expected_member.
            // Reuse its binding while checking each candidate's own stamp and ID.
            let member = match &seen_owner.member {
                Some((reference, member)) if *reference == expected_member => member,
                _ => {
                    &seen_owner
                        .member
                        .insert((expected_member, selected.current_member(query, context)?))
                        .1
                }
            };
            selected.validate_member_identity(member, query)?;
            if row.owner != member.owner || row.member != member.logical_commitment {
                return Err(IndexError::CorruptObject(
                    "native graph candidate member stamp differs".into(),
                ));
            }
            (member, Some(row.vector_id.as_slice()))
        }
    };
    let selected_input = if !query.source_files && cached_member.is_none() {
        let std::ops::ControlFlow::Continue(input) = selected.rank_input(query, None, context)?
        else {
            return Ok(false);
        };
        input
    } else {
        selected_input
    };
    if let (Some(id), Some(artifact)) = (only, cached_member) {
        // Graph rows are unique by owner and ID. A prior candidate may have
        // moved its ID into a result, but it cannot be this row's ID.
        validate_rank_vector(artifact, Some(id))?;
        let file = load_cached_file(member, &selected, heap, query, context)?;
        return rank_decoded_member(
            owner, artifact, file, only, query, query_norm, top_k, heap, context,
        );
    }
    if let (Some(cache), Some(id)) = (cached, only) {
        let (mut artifact, file) =
            load_rank_member(member, selected_input, Some(id), query, context)?;
        // File metadata belongs to the hit; the cache owns neither an Arc
        // clone nor any retained memory after the heap evicts that file.
        let reusable = artifact.vectors.len() > 1 && (!query.source_files || file.is_some());
        let ranked = rank_decoded_member(
            owner,
            &mut artifact.artifact,
            file,
            Some(id),
            query,
            query_norm,
            top_k,
            heap,
            context,
        )?;
        if reusable {
            cache.remember(expected_member, artifact);
        }
        return Ok(ranked);
    }
    rank_member(
        member,
        selected_input,
        only,
        query,
        query_norm,
        top_k,
        heap,
        context,
    )
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
        let page = directory.region_page(after, 64, after.is_some(), context)?;
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

#[cfg(test)]
pub(super) fn search(
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    super::ann_directory::validate_coverage_layout(query.coverage)?;
    let directory = AnnDirectory::load_for_coverage(query.coverage, query.recipe, context)?;
    search_loaded(query, directory.as_ref(), context)
}

pub(super) fn search_loaded(
    query: &SearchQuery<'_>,
    directory: Option<&AnnDirectory>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    let query_norm = validate_search_query(query)?;
    let directory = match directory {
        Some(directory) => directory,
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
    if let Some(result) = super::lexical_scope::search(query, context)? {
        return Ok(result);
    }
    if let Some(result) = search_selected_exact_path(query, Some(directory), context)? {
        return Ok(result);
    }
    let small_exact = if directory.region_count == 0 {
        let selected = scan_ordered_page(
            query.coverage.owners,
            None,
            MAX_DELTA_OWNERS,
            false,
            context,
        )?;
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
    let mut exact_delta = if small_exact {
        Some(ExactDeltaMembers::reserve(delta_owners, context)?)
    } else {
        None
    };
    let max_candidates = MAX_ROUTED_REGIONS
        .checked_mul(native_graph::MAX_REGION_CANDIDATES)
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
    let mut capped = top_k > native_graph::MAX_REGION_CANDIDATES
        || u64::from(directory.region_count)
            > u64::try_from(MAX_ROUTED_REGIONS).map_err(|_error| IndexError::ResourceLimited)?;

    // The bounded delta is exact over its current owner members.
    let mut after: Option<[u8; 38]> = None;
    let mut delta_entries = 0u32;
    loop {
        let page = scan_ordered_page(
            directory.delta,
            after.as_ref().map(<[u8; 38]>::as_slice),
            query.page_size.min(MAX_DELTA_OWNERS),
            after.is_some(),
            context,
        )?;
        for (key, member_ref) in &page.entries {
            let member_ref = member_ref.as_object_ref()?;
            context.budget.checkpoint()?;
            delta_entries = delta_entries
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if delta_entries > directory.delta_owners {
                return Err(IndexError::CorruptObject(
                    "ANN delta map exceeds declared owner count".into(),
                ));
            }
            let owner = owner_from_map_key(key)?;
            let binding = directory.delta_member_from_page(owner, *member_ref, context)?;
            if let Some(exact_delta) = exact_delta.as_mut() {
                exact_delta.entries.push((*member_ref, binding));
            } else {
                let _rescored = rank_selected(
                    owner,
                    *member_ref,
                    Candidate::Delta(&binding),
                    query,
                    query_norm,
                    top_k,
                    &mut heap,
                    &mut seen,
                    durable_limit,
                    &mut capped,
                    None,
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
    if let Some(exact_delta) = exact_delta {
        // This small population has a bounded independent exact oracle.
        drop(heap);
        drop(_heap_memory);
        drop(seen);
        drop(_seen_memory);
        return search_exact_with_delta(query, Some(&exact_delta), context);
    }

    let routes = route_regions(directory, query, query_norm, context)?;
    for routed in routes.entries {
        let span = crate::build_diagnostic::begin_build_stage(
            context.budget.request().diagnostic_context(),
            DiagnosticStageV1::QueryCandidate,
        );
        let region_result = (|| {
            context.budget.checkpoint()?;
            let ordinal = routed.ordinal;
            let descriptor = directory.region_from_retained(ordinal, routed.object, context)?;
            let graph = native_graph::load(
                context.objects,
                descriptor.descriptor.fileset,
                context.budget,
            )?;
            if graph.seal.recipe != directory.recipe
                || graph.seal.generation
                    != ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?
                || graph.seal.rows != descriptor.descriptor.rows
                || graph.seal.dimension as usize != descriptor.descriptor.center.len()
            {
                return Err(IndexError::CorruptObject(
                    "native graph selected seal differs from directory".into(),
                ));
            }
            let candidates = graph.candidates(
                query.vector,
                native_graph::MAX_REGION_ROWS,
                native_graph::MAX_REGION_CANDIDATES,
                context.budget,
            )?;
            // Preserve the original owner funding across all routed regions.
            // A raw graph frontier is not permission to re-open every owner.
            // File duplicates may refill this window, bounded by the existing
            // candidate/owner caps; truncation remains an explicit limitation.
            // Deleted, superseded and source-free rows do not spend this
            // original-vector window merely by occupying a graph position.
            let initial_rescores = top_k
                .checked_mul(4)
                .ok_or(IndexError::ResourceLimited)?
                .max(64)
                .min(native_graph::MAX_REGION_CANDIDATES);
            let mut rescore_capped = false;
            let mut rescored = 0usize;
            let mut cached = RegionArtifactCache::new(&candidates);
            for &index in &candidates.indices {
                if rescored >= initial_rescores
                    && !(query.source_files && heap.len() < top_k && seen.len() < durable_limit)
                {
                    rescore_capped = true;
                    break;
                }
                context.budget.checkpoint()?;
                let candidate = &graph.rows[index];
                let key = owner_map_key(candidate.owner);
                let Some(member_ref) = context.maps.lookup_object(
                    descriptor.descriptor.members,
                    &key,
                    context.budget,
                )?
                else {
                    return Err(IndexError::CorruptObject(
                        "native graph candidate absent from sealed members".into(),
                    ));
                };
                let did_rescore = rank_selected(
                    candidate.owner,
                    member_ref,
                    Candidate::Region(candidate),
                    query,
                    query_norm,
                    top_k,
                    &mut heap,
                    &mut seen,
                    durable_limit,
                    &mut capped,
                    Some(&mut cached),
                    context,
                )?;
                if did_rescore {
                    rescored = rescored.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
            }
            if let (Some(diagnostic), Some(span)) =
                (context.budget.request().diagnostic_context(), span.as_ref())
            {
                diagnostic.ann_region_observation(
                    span,
                    DiagnosticAnnRegionObservationV1 {
                        phase: DiagnosticAnnRegionPhaseV1::SelectedQuery,
                        fileset_ref: descriptor.descriptor.fileset.0.0,
                        descriptor_ref: Some(descriptor.reference().0.0),
                        ordinal,
                        rows: graph.seal.rows,
                        index_kind: DiagnosticAnnIndexKindV1::Hnsw,
                        files: 1,
                        chunks: 1,
                    },
                );
            }
            Ok::<_, IndexError>(NativeSemanticTraversal::Approximate {
                capped: candidates.capped || rescore_capped,
            })
        })();
        crate::build_diagnostic::finish_build_stage(span, region_result.is_ok());
        if region_was_capped(region_result?) {
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

#[cfg(test)]
mod tests {
    use super::{NativeSemanticTraversal, region_was_capped};

    #[test]
    fn region_approximation_does_not_imply_an_explicit_cap() {
        assert!(!region_was_capped(NativeSemanticTraversal::ExactExhausted));
        assert!(!region_was_capped(NativeSemanticTraversal::Approximate {
            capped: false,
        }));
        assert!(region_was_capped(NativeSemanticTraversal::ExactCapped));
        assert!(region_was_capped(NativeSemanticTraversal::Approximate {
            capped: true,
        }));
    }
}
