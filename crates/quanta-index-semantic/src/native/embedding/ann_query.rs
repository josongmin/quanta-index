//! Selected-view ANN search. Routing is bounded; every returned row is
//! authenticated against the pinned coverage, member, input and source roots.

use std::cmp::Ordering;

use quanta_index_contract::native_protocol::NativeSemanticTraversal;
use quanta_index_core::index::{
    IndexError, MapRoot, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey, OwnerMemberBinding,
    StoredObject, WorkUsage, owner_from_map_key, owner_map_key,
};
use quanta_index_core::{
    DiagnosticAnnIndexKindV1, DiagnosticAnnRegionObservationV1, DiagnosticAnnRegionPhaseV1,
    DiagnosticCounterV1, DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1,
};

use super::native_graph;
use super::{
    ChunkMember, EmbeddingVector, LoadedRankArtifact, LoadedRankMember, RankArtifact, RankedHits,
    ReadContext, SearchQuery, SearchResult, SemanticSourceFile,
    ann_directory::{AnnBackend, AnnDirectory, LoadedRegionDescriptor, MAX_DELTA_OWNERS},
    cosine, finish_ranked, load_rank_member, rank_decoded_member, rank_member, search_exact,
    search_exact_with_delta, search_selected_exact_path,
    selected_owner::{SelectedOwner, SourceScope, scan_ordered_page},
    source_file, validate_search_query,
};
use crate::native::{
    ann_files, ann_lance,
    ann_session::{ANN_TABLE_NAME, AnnLanceSession},
    ann_store::AnnStoreLimits,
};

const MAX_ROUTED_GRAPH_REGIONS: usize = 8;
const MAX_CACHED_ARTIFACT_BYTES: usize = 512 * 1024;
const MAX_CACHED_MEMBERS: usize = 8;

fn finish_lance_query_stage<T>(
    span: Option<DiagnosticSpanGuardV1>,
    result: &Result<T, IndexError>,
) {
    let Some(span) = span else {
        return;
    };
    let outcome = match result {
        Ok(_) => DiagnosticTerminalV1::Ok,
        Err(IndexError::Cancelled) => DiagnosticTerminalV1::Cancelled { code: 1 },
        Err(_) => DiagnosticTerminalV1::Error { code: 1 },
    };
    let _finished = span.finish(outcome);
}

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

struct RankedLanceRegions {
    entries: Vec<RankedLanceRegion>,
    _memory: Option<MemoryLease>,
}

struct RankedLanceRegion {
    score: f64,
    ordinal: u64,
    projection: LanceRegionProjection,
}

#[derive(Clone, Copy)]
struct LanceRegionProjection {
    reference: ObjectRef,
    fileset: ObjectRef,
    members: MapRoot,
    rows: u32,
}

struct LanceRegionWindow {
    owner_slots: usize,
    candidate_cap: usize,
    minimum_prefix: Option<usize>,
}

struct DeferredLanceRegion {
    ordinal: u64,
    projection: LanceRegionProjection,
    candidate_cap: usize,
    minimum_prefix: usize,
    hydration_work: WorkUsage,
}

struct SeenOwner {
    owner: OwnerKey,
    stale: bool,
    member: Option<(ObjectRef, OwnerMemberBinding)>,
}

struct CachedMember {
    reference: ObjectRef,
    selected: SelectedOwner,
    artifact: RankArtifact,
    bytes: usize,
}

struct CachedLanceMember {
    reference: ObjectRef,
    selected: SelectedOwner,
    loaded: LoadedRankArtifact,
    bytes: usize,
}

/// A decoded member is optional query memory. WorkBudget discards this state
/// before refusing a mandatory allocation, including the next member decode.
struct LanceArtifactCache {
    members: [Option<CachedLanceMember>; MAX_CACHED_MEMBERS],
    next: usize,
    _state_memory: MemoryLease,
}

#[cfg(test)]
pub(super) fn lance_cache_state_bytes() -> usize {
    std::mem::size_of::<LanceArtifactCache>()
}

impl LanceArtifactCache {
    fn new(state_memory: MemoryLease) -> Self {
        Self {
            members: Default::default(),
            next: 0,
            _state_memory: state_memory,
        }
    }

    fn member_index(&self, owner: OwnerKey, reference: ObjectRef) -> Option<usize> {
        self.members.iter().position(|entry| {
            entry.as_ref().is_some_and(|member| {
                member.reference == reference && member.selected.coverage.owner == owner
            })
        })
    }

    fn selected(&self, owner: OwnerKey, reference: ObjectRef) -> Option<SelectedOwner> {
        self.members
            .get(self.member_index(owner, reference)?)?
            .as_ref()
            .map(|member| member.selected)
    }

    fn take(&mut self, owner: OwnerKey, reference: ObjectRef) -> Option<LoadedRankArtifact> {
        let index = self.member_index(owner, reference)?;
        self.members
            .get_mut(index)?
            .take()
            .map(|member| member.loaded)
    }

    fn remember(
        &mut self,
        reference: ObjectRef,
        selected: SelectedOwner,
        loaded: LoadedRankArtifact,
    ) {
        let bytes = loaded
            .decode_memory
            .as_ref()
            .and_then(|memory| usize::try_from(memory.bytes()).ok())
            .unwrap_or_else(|| decoded_backing(&loaded));
        if bytes > MAX_CACHED_ARTIFACT_BYTES {
            return;
        }
        self.members[self.next] = None;
        let mut live_bytes = bytes;
        for member in &mut self.members {
            if let Some(cached) = member {
                live_bytes += cached.bytes;
                if live_bytes > MAX_CACHED_ARTIFACT_BYTES {
                    live_bytes -= cached.bytes;
                    *member = None;
                }
            }
        }
        self.members[self.next] = Some(CachedLanceMember {
            reference,
            selected,
            loaded,
            bytes,
        });
        self.next = (self.next + 1) % MAX_CACHED_MEMBERS;
    }
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

    fn get(&mut self, owner: OwnerKey, reference: ObjectRef) -> Option<&mut CachedMember> {
        self.members
            .iter_mut()
            .flatten()
            .find(|entry| entry.reference == reference && entry.selected.coverage.owner == owner)
    }

    fn remember(
        &mut self,
        reference: ObjectRef,
        selected: SelectedOwner,
        loaded: LoadedRankArtifact,
    ) {
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
            selected,
            artifact: loaded.artifact,
            bytes,
        });
        self.next = (self.next + 1) % MAX_CACHED_MEMBERS;
    }
}

fn load_cached_file(
    member: &OwnerMemberBinding,
    selected: &SelectedOwner,
    heap: &RankedHits,
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
    source_file::load(member, selected, record, query, context)
}

fn decoded_backing(artifact: &LoadedRankArtifact) -> usize {
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
    Lance(&'a ann_lance::RegionCandidate),
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

struct SeenOwners {
    entries: Vec<SeenOwner>,
    memory: Option<MemoryLease>,
    limit: usize,
}

impl SeenOwners {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn admit(
        &mut self,
        owner: OwnerKey,
        stale: bool,
        limit: usize,
        capped: &mut bool,
        budget: &quanta_index_core::index::WorkBudget,
    ) -> Result<Option<&mut SeenOwner>, IndexError> {
        match self
            .entries
            .binary_search_by_key(&owner, |entry| entry.owner)
        {
            Ok(index) => Ok(self.entries.get_mut(index)),
            Err(_index) if self.len() >= limit => {
                *capped = true;
                Ok(None)
            }
            Err(index) => {
                if self.len() == self.entries.capacity() {
                    self.grow(budget)?;
                }
                self.entries.insert(
                    index,
                    SeenOwner {
                        owner,
                        stale,
                        member: None,
                    },
                );
                Ok(self.entries.get_mut(index))
            }
        }
    }

    fn bytes(capacity: usize) -> Result<u64, IndexError> {
        capacity
            .checked_mul(std::mem::size_of::<SeenOwner>())
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(IndexError::ResourceLimited)
    }

    fn grow(&mut self, budget: &quanta_index_core::index::WorkBudget) -> Result<(), IndexError> {
        let needed = self
            .len()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if needed > self.limit {
            return Err(IndexError::ResourceLimited);
        }
        let preferred = needed
            .checked_next_power_of_two()
            .ok_or(IndexError::ResourceLimited)?
            .min(self.limit);
        if preferred > needed
            && let Some(memory) = budget
                .try_reserve_spare_memory(Self::bytes(preferred)?, MemoryLifetime::Temporary)?
        {
            return self.replace_allocation(preferred, Some(memory));
        }
        let memory = budget.reserve_memory(Self::bytes(needed)?, MemoryLifetime::Temporary)?;
        let capacity = if memory.is_some() { needed } else { preferred };
        self.replace_allocation(capacity, memory)
    }

    fn replace_allocation(
        &mut self,
        capacity: usize,
        memory: Option<MemoryLease>,
    ) -> Result<(), IndexError> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if entries.capacity() != capacity {
            return Err(IndexError::ResourceLimited);
        }
        // Both allocations are live until the old backing has been freed.
        entries.append(&mut self.entries);
        self.entries = entries;
        self.memory = memory;
        Ok(())
    }
}

fn load_candidate_owner(
    owner: OwnerKey,
    expected_member: ObjectRef,
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<Option<SelectedOwner>, IndexError> {
    let key = owner_map_key(owner);
    let Some(coverage_ref) = context
        .maps
        .lookup(query.coverage.owners, &key, context.budget)?
    else {
        // Old immutable regions can retain a deleted owner.
        return Ok(None);
    };
    let coverage = quanta_index_core::index::read_owner_coverage(
        coverage_ref,
        &key,
        context.objects,
        context.budget,
    )?;
    if coverage.covered == Some(coverage.desired)
        && coverage.searchable.is_some()
        && coverage.searchable != Some(expected_member)
    {
        // A superseded candidate cannot claim the current replacement's slot.
        return Ok(None);
    }
    SelectedOwner::authenticate(
        &key,
        coverage,
        query.coverage,
        query.projection.as_ref(),
        SourceScope::Lookup(query.source_owners),
        context,
    )
    .map(Some)
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
    heap: &mut RankedHits,
    seen: &mut SeenOwners,
    durable_limit: usize,
    capped: &mut bool,
    mut cached: Option<&mut RegionArtifactCache<'_>>,
    context: &mut ReadContext<'_>,
) -> Result<Option<OwnerKey>, IndexError> {
    let cached_member = if matches!(candidate, Candidate::Region(_)) {
        cached
            .as_mut()
            .and_then(|cache| cache.get(owner, expected_member))
    } else {
        None
    };
    let cached_selected = cached_member
        .as_ref()
        .map(|entry| entry.selected)
        .or_else(|| {
            if !matches!(candidate, Candidate::Lance(_)) {
                return None;
            }
            // Copy only the selected-view identity. Leave the decoded backing
            // discardable while the next candidate's required input is checked.
            context
                .budget
                .with_discardable_memory_mut::<LanceArtifactCache, _>(|cache| {
                    cache.selected(owner, expected_member)
                })
                .flatten()
        });
    let selected = match cached_selected {
        Some(selected) => selected,
        None => {
            let Some(selected) = load_candidate_owner(owner, expected_member, query, context)?
            else {
                return Ok(None);
            };
            selected
        }
    };
    let coverage = selected.coverage;
    let selected_input = if cached_member.is_none() && query.source_files {
        // Projection members can share a file without sharing an artifact.
        let record = selected.contribution.and_then(|contribution| {
            source_file::retained_record(heap, contribution.source_owner?, contribution.source?)
        });
        let std::ops::ControlFlow::Continue(input) = selected.rank_input(query, record, context)?
        else {
            return Ok(None);
        };
        input
    } else {
        if cached_member.is_none() && selected.has_no_candidate(false) {
            return Ok(None);
        }
        None
    };
    let stale = coverage.covered != Some(coverage.desired) || coverage.searchable.is_none();
    let Some(seen_owner) = seen.admit(owner, stale, durable_limit, capped, context.budget)? else {
        return Ok(None);
    };
    if stale {
        return Ok(None);
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
        Candidate::Lance(row) => {
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
            if !ann_lance::selected_input_matches(
                row,
                member.owner,
                member.logical_commitment,
                &row.vector_id,
                member.input,
            ) {
                return Err(IndexError::CorruptObject(
                    "ANN candidate member/input stamp differs".into(),
                ));
            }
            (member, Some(row.vector_id.as_slice()))
        }
    };
    let selected_input = if !query.source_files && cached_member.is_none() {
        let std::ops::ControlFlow::Continue(input) = selected.rank_input(query, None, context)?
        else {
            return Ok(None);
        };
        input
    } else {
        selected_input
    };
    // Refill progress belongs to the result identity, not to a scored vector.
    // Projection owners can share one source file.
    let result_owner = if query.source_files {
        selected
            .contribution
            .map_or(Some(member.owner), |contribution| contribution.source_owner)
    } else {
        Some(member.owner)
    };
    if let (Some(id), Some(cached_member)) = (only, cached_member) {
        let artifact = &mut cached_member.artifact;
        // Graph rows are unique by owner and ID. A prior candidate may have
        // moved its ID into a result, but it cannot be this row's ID.
        let vector_index = artifact.vector_index(Some(id))?;
        let file = load_cached_file(member, &selected, heap, query, context)?;
        let ranked = rank_decoded_member(
            owner,
            artifact,
            file,
            vector_index,
            query,
            query_norm,
            heap,
            context,
        )?;
        return Ok(ranked.then_some(result_owner).flatten());
    }
    if matches!(candidate, Candidate::Lance(_)) {
        let cached_artifact = context
            .budget
            .with_discardable_memory_mut::<LanceArtifactCache, _>(|cache| {
                cache.take(owner, expected_member)
            })
            .flatten();
        let LoadedRankMember {
            mut loaded,
            file,
            vector_index,
        } = if let Some(loaded) = cached_artifact {
            let vector_index = loaded.artifact.vector_index(only)?;
            // An exact-path input may own a source record. Drop it before
            // loading file metadata so it cannot inflate the live window.
            drop(selected_input);
            LoadedRankMember {
                loaded,
                file: load_cached_file(member, &selected, heap, query, context)?,
                vector_index,
            }
        } else {
            load_rank_member(
                member,
                Some(&selected),
                selected_input,
                only,
                query,
                context,
            )?
        };
        let reusable = loaded.vectors.len() > 1 && (!query.source_files || file.is_some());
        let ranked = rank_decoded_member(
            owner,
            &mut loaded.artifact,
            file,
            vector_index,
            query,
            query_norm,
            heap,
            context,
        )?;
        if reusable {
            let _cached = context
                .budget
                .with_discardable_memory_mut::<LanceArtifactCache, _>(|cache| {
                    cache.remember(expected_member, selected, loaded);
                });
        }
        return Ok(ranked.then_some(result_owner).flatten());
    }
    if let (Some(cache), Some(id)) = (cached, only) {
        let LoadedRankMember {
            mut loaded,
            file,
            vector_index,
        } = load_rank_member(
            member,
            Some(&selected),
            selected_input,
            Some(id),
            query,
            context,
        )?;
        // File metadata belongs to the hit; the cache owns neither an Arc
        // clone nor any retained memory after the heap evicts that file.
        let reusable = loaded.vectors.len() > 1 && (!query.source_files || file.is_some());
        let ranked = rank_decoded_member(
            owner,
            &mut loaded.artifact,
            file,
            vector_index,
            query,
            query_norm,
            heap,
            context,
        )?;
        if reusable {
            cache.remember(expected_member, selected, loaded);
        }
        return Ok(ranked.then_some(result_owner).flatten());
    }
    let ranked = rank_member(
        &selected,
        member,
        selected_input,
        only,
        query,
        query_norm,
        heap,
        context,
    )?;
    Ok(ranked.then_some(result_owner).flatten())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the selected region, original budget and ranking state remain explicit across the async Lance boundary"
)]
fn query_lance_region(
    directory: &AnnDirectory,
    descriptor: &LanceRegionProjection,
    ordinal: u64,
    query: &SearchQuery<'_>,
    query_norm: f64,
    top_k: usize,
    heap: &mut RankedHits,
    seen: &mut SeenOwners,
    window: LanceRegionWindow,
    capped: &mut bool,
    context: &mut ReadContext<'_>,
    runtime: &tokio::runtime::Runtime,
    span: Option<&DiagnosticSpanGuardV1>,
) -> Result<Option<(ann_lance::RegionPage, WorkUsage)>, IndexError> {
    let LanceRegionWindow {
        owner_slots,
        candidate_cap,
        minimum_prefix,
    } = window;
    let owner_limit = seen
        .len()
        .checked_add(owner_slots)
        .ok_or(IndexError::ResourceLimited)?;
    let rows = usize::try_from(descriptor.rows).map_err(|_error| IndexError::ResourceLimited)?;
    let vectors_per_owner =
        usize::try_from(query.recipe.max_vectors).map_err(|_error| IndexError::ResourceLimited)?;
    // An owner slot can cover several vector rows. These rows can still
    // improve the results after the region stops admitting new owners.
    let prefix_limit = owner_slots
        .saturating_mul(vectors_per_owner)
        .min(rows)
        .min(candidate_cap);
    let result_limit = if minimum_prefix.is_some() {
        // A replayed prefix also contains already-ranked files. Filling the
        // small residual owner window with those files is not progress.
        rows.min(candidate_cap)
    } else if query.source_files {
        owner_slots.min(prefix_limit)
    } else {
        prefix_limit
    };
    let region_top_k = top_k.min(result_limit);
    let stop_refill =
        (vectors_per_owner == 1).then_some(ann_lance::RefillProgress::OwnerLimitReached);
    let initial_candidates = top_k
        .checked_mul(4)
        .ok_or(IndexError::ResourceLimited)?
        .max(64)
        .min(prefix_limit)
        .max(minimum_prefix.unwrap_or_default())
        .max(region_top_k);
    let diagnostic = context
        .budget
        .request()
        .diagnostic_context()
        .zip(span)
        .map(|(context, parent)| context.child_context(parent));
    let fileset_span = crate::build_diagnostic::begin_build_stage(
        diagnostic.as_ref(),
        DiagnosticStageV1::QueryAnnFileSet,
    );
    let fileset_result = ann_files::load_fileset_if_admitted(
        context.objects,
        descriptor.fileset,
        context.budget,
        |seal, total_length, file_count, budget| {
            if seal.recipe != directory.recipe
                || seal.generation != ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?
                || seal.rows != descriptor.rows
                || usize::try_from(seal.dimension).map_err(|_error| IndexError::ResourceLimited)?
                    != query.vector.len()
            {
                return Err(IndexError::CorruptObject(
                    "ANN selected region seal differs from directory".into(),
                ));
            }
            let max_read_bytes = total_length
                .checked_mul(8)
                .and_then(|bytes| bytes.checked_add(1_048_576))
                .ok_or(IndexError::ResourceLimited)?
                .min(64 * 1024 * 1024);
            let limits = AnnStoreLimits {
                max_write_bytes: total_length.max(1),
                max_read_bytes,
                ..AnnStoreLimits::default()
            };
            let hydration_work = limits.hydration_work(file_count)?;
            let prefix_work = ann_lance::prefix_work(initial_candidates)?;
            let opening_work = WorkUsage {
                reads: hydration_work
                    .reads
                    .checked_add(ann_lance::OPEN_REGION_WORK.reads)
                    .and_then(|reads| reads.checked_add(prefix_work.reads))
                    .ok_or(IndexError::ResourceLimited)?,
                bytes: hydration_work
                    .bytes
                    .checked_add(prefix_work.bytes)
                    .ok_or(IndexError::ResourceLimited)?,
                ..hydration_work
            };
            let known_work = WorkUsage {
                // Verification charges file bytes independently of the store envelope.
                bytes: opening_work
                    .bytes
                    .checked_add(total_length)
                    .ok_or(IndexError::ResourceLimited)?,
                ..opening_work
            };
            budget.checkpoint()?;
            Ok(budget
                .can_charge(known_work)
                .then_some((limits, hydration_work, opening_work)))
        },
    );
    finish_lance_query_stage(fileset_span, &fileset_result);
    let Some((fileset, (limits, hydration_work, opening_work))) = fileset_result? else {
        return Ok(None);
    };
    // Object ports account for their own chunk reads. Recheck the known
    // cost against the current budget before paying for a private session.
    context.budget.checkpoint()?;
    if !context.budget.can_charge(opening_work) {
        return Ok(None);
    }
    let objects = context.objects;
    let maps = context.maps;
    let members = descriptor.members;
    let budget = &mut *context.budget;
    let cache_memory = budget.try_reserve_memory_for_optimization(
        u64::try_from(std::mem::size_of::<LanceArtifactCache>())
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut run = |budget: &mut quanta_index_core::index::WorkBudget| {
        crate::run_blocking(runtime, async {
            let hydrate_span = crate::build_diagnostic::begin_build_stage(
                diagnostic.as_ref(),
                DiagnosticStageV1::QueryAnnHydrate,
            );
            let session_result = AnnLanceSession::hydrate(&fileset, limits, budget).await;
            finish_lance_query_stage(hydrate_span, &session_result);
            let session = session_result?;
            let open_span = crate::build_diagnostic::begin_build_stage(
                diagnostic.as_ref(),
                DiagnosticStageV1::QueryAnnOpen,
            );
            let region_result = async {
                let connection = session.connect().await?;
                let table = session.resolve(
                    connection
                        .open_table(ANN_TABLE_NAME)
                        .execute()
                        .await
                        .map_err(|error| {
                            IndexError::Storage(format!("ANN selected table: {error}"))
                        }),
                )?;
                let opened = ann_lance::open_region(table, fileset.seal.clone(), budget).await;
                session.resolve(opened)
            }
            .await;
            finish_lance_query_stage(open_span, &region_result);
            let region = region_result?.with_read_store(session.read_store());
            let candidate_span = crate::build_diagnostic::begin_build_stage(
                diagnostic.as_ref(),
                DiagnosticStageV1::QueryAnnCandidate,
            );
            let page = ann_lance::query_region(
                &region,
                query.vector,
                region_top_k,
                initial_candidates,
                candidate_cap,
                budget,
                |candidate, budget| {
                    if seen.len() == owner_limit
                        && seen
                            .entries
                            .binary_search_by_key(&candidate.owner, |entry| entry.owner)
                            .is_err()
                    {
                        *capped = true;
                        return Ok(stop_refill);
                    }
                    let mut read = ReadContext {
                        objects,
                        maps,
                        budget,
                    };
                    let key = owner_map_key(candidate.owner);
                    let member_ref = read
                        .maps
                        .lookup_object(members, &key, read.budget)?
                        .ok_or_else(|| {
                            IndexError::CorruptObject(
                                "ANN candidate is absent from sealed region members".into(),
                            )
                        })?;
                    let scored = rank_selected(
                        candidate.owner,
                        member_ref,
                        Candidate::Lance(candidate),
                        query,
                        query_norm,
                        heap,
                        seen,
                        owner_limit,
                        capped,
                        None,
                        &mut read,
                    )?;
                    if seen.len() == owner_limit && stop_refill.is_some() {
                        return Ok(stop_refill);
                    }
                    Ok(scored.map(|owner| {
                        if query.source_files {
                            ann_lance::RefillProgress::SourceFile(owner)
                        } else {
                            ann_lance::RefillProgress::Vector
                        }
                    }))
                },
            )
            .await;
            finish_lance_query_stage(candidate_span, &page);
            page
        })
    };
    let page = if let Some(memory) = cache_memory {
        let (result, _cache) =
            budget.with_discardable_memory(Box::new(LanceArtifactCache::new(memory)), run);
        result
    } else {
        run(budget)
    }?;
    if let (Some(diagnostic), Some(span)) = (context.budget.request().diagnostic_context(), span) {
        let files =
            u32::try_from(fileset.files.len()).map_err(|_error| IndexError::ResourceLimited)?;
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
                fileset_ref: descriptor.fileset.0.0,
                descriptor_ref: Some(descriptor.reference.0.0),
                ordinal,
                rows: fileset.seal.rows,
                index_kind: ann_files::diagnostic_index_kind(&fileset.seal)?,
                files,
                chunks,
            },
        );
    }
    Ok(Some((page, hydration_work)))
}

fn visit_region_descriptors(
    directory: &AnnDirectory,
    dimension: usize,
    context: &mut ReadContext<'_>,
    mut visit: impl FnMut(
        u64,
        LoadedRegionDescriptor,
        usize,
        &mut ReadContext<'_>,
    ) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let mut after = None;
    let mut remaining =
        usize::try_from(directory.region_count).map_err(|_error| IndexError::ResourceLimited)?;
    loop {
        let page = directory.region_page(after, 64, after.is_some(), context)?;
        for (key, reference) in &page.entries {
            let reference = *reference.as_object_ref()?;
            let ordinal = u64::from_be_bytes(key.as_slice().try_into().map_err(|_error| {
                IndexError::CorruptObject("ANN region key width differs".into())
            })?);
            let remaining_after = remaining
                .checked_sub(1)
                .ok_or_else(|| IndexError::CorruptObject("ANN region map count exceeded".into()))?;
            context.budget.checkpoint()?;
            let descriptor = directory.region_from_page(ordinal, reference, context)?;
            if descriptor.descriptor.center.len() != dimension {
                return Err(IndexError::CorruptObject(
                    "ANN region center dimension differs from query recipe".into(),
                ));
            }
            visit(ordinal, descriptor, remaining, context)?;
            remaining = remaining_after;
            after = Some(ordinal);
        }
        if !page.has_more {
            break;
        }
    }
    if remaining != 0 {
        return Err(IndexError::CorruptObject(
            "ANN region map count differs".into(),
        ));
    }
    Ok(())
}

fn route_graph_regions(
    directory: &AnnDirectory,
    route_limit: usize,
    query: &SearchQuery<'_>,
    query_norm: f64,
    context: &mut ReadContext<'_>,
) -> Result<RoutedRegions, IndexError> {
    let route_bytes = route_limit
        .checked_mul(std::mem::size_of::<RoutedRegion>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = if route_bytes == 0 {
        None
    } else {
        context.budget.reserve_memory(
            u64::try_from(route_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    let mut routes = Vec::new();
    routes
        .try_reserve_exact(route_limit)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if routes.capacity() > route_limit {
        return Err(IndexError::ResourceLimited);
    }
    visit_region_descriptors(
        directory,
        query.vector.len(),
        context,
        |ordinal, descriptor, _, _| {
            let score = cosine(query.vector, query_norm, &descriptor.descriptor.center)?;
            let object = descriptor.into_stored_object();
            if routes.len() < route_limit {
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
            Ok(())
        },
    )?;
    Ok(RoutedRegions {
        entries: routes,
        _memory: memory,
    })
}

fn rank_lance_regions(
    directory: &AnnDirectory,
    query: &SearchQuery<'_>,
    query_norm: f64,
    context: &mut ReadContext<'_>,
) -> Result<RankedLanceRegions, IndexError> {
    let count =
        usize::try_from(directory.region_count).map_err(|_error| IndexError::ResourceLimited)?;
    let bytes = count
        .checked_mul(std::mem::size_of::<RankedLanceRegion>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = if bytes == 0 {
        None
    } else {
        context
            .budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)?
    };
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if entries.capacity() != count {
        return Err(IndexError::ResourceLimited);
    }
    visit_region_descriptors(
        directory,
        query.vector.len(),
        context,
        |ordinal, descriptor, _, _| {
            let score = cosine(query.vector, query_norm, &descriptor.descriptor.center)?;
            entries.push(RankedLanceRegion {
                score,
                ordinal,
                projection: LanceRegionProjection {
                    reference: descriptor.reference(),
                    fileset: descriptor.descriptor.fileset,
                    members: descriptor.descriptor.members,
                    rows: descriptor.descriptor.rows,
                },
            });
            Ok(())
        },
    )?;
    // Ordering changes which regions receive a bounded owner window. It does
    // not discard a region: every descriptor has already been authenticated.
    entries.sort_unstable_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.ordinal.cmp(&right.ordinal))
    });
    Ok(RankedLanceRegions {
        entries,
        _memory: memory,
    })
}

fn candidate_capacity(
    backend: AnnBackend,
    region_count: usize,
    candidate_cap: usize,
    delta_owners: usize,
) -> Result<(usize, usize), IndexError> {
    let route_limit = match backend {
        AnnBackend::Graph => region_count.min(MAX_ROUTED_GRAPH_REGIONS),
        AnnBackend::LanceDb => region_count,
    };
    let owners = route_limit
        .checked_mul(candidate_cap)
        .and_then(|count| count.checked_add(delta_owners))
        .ok_or(IndexError::ResourceLimited)?;
    Ok((route_limit, owners))
}

/// Authenticate every bounded delta entry before it is used by either ranking path.
fn visit_delta_members(
    directory: &AnnDirectory,
    page_size: u32,
    context: &mut ReadContext<'_>,
    mut visit: impl FnMut(
        OwnerKey,
        ObjectRef,
        OwnerMemberBinding,
        &mut ReadContext<'_>,
    ) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let mut after: Option<[u8; 38]> = None;
    let mut delta_entries = 0u32;
    loop {
        let page = scan_ordered_page(
            directory.delta,
            after.as_ref().map(<[u8; 38]>::as_slice),
            page_size.min(MAX_DELTA_OWNERS),
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
            visit(owner, *member_ref, binding, context)?;
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
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "share the request's ranking and budget across visited regions"
)]
fn search_lance_regions(
    directory: &AnnDirectory,
    query: &SearchQuery<'_>,
    query_norm: f64,
    top_k: usize,
    heap: &mut RankedHits,
    seen: &mut SeenOwners,
    durable_limit: usize,
    capped: &mut bool,
    context: &mut ReadContext<'_>,
    runtime: Option<&tokio::runtime::Runtime>,
) -> Result<(), IndexError> {
    let query_region = |ordinal,
                        descriptor: &LanceRegionProjection,
                        window,
                        heap: &mut RankedHits,
                        seen: &mut SeenOwners,
                        context: &mut ReadContext<'_>| {
        let runtime = runtime.ok_or_else(|| {
            IndexError::BackendViolation("ANN Lance runtime is unavailable".into())
        })?;
        let span = crate::build_diagnostic::begin_build_stage(
            context.budget.request().diagnostic_context(),
            DiagnosticStageV1::QueryCandidate,
        );
        let before = context.budget.usage();
        let mut owner_capped = false;
        let region_result = query_lance_region(
            directory,
            descriptor,
            ordinal,
            query,
            query_norm,
            top_k,
            heap,
            seen,
            window,
            &mut owner_capped,
            context,
            runtime,
            span.as_ref(),
        );
        if let Some(span) = span.as_ref() {
            context.budget.record_diagnostic_usage_delta(before, span);
        }
        if matches!(&region_result, Ok(None))
            && let (Some(diagnostic), Some(span)) =
                (context.budget.request().diagnostic_context(), span.as_ref())
        {
            diagnostic.counter(span, DiagnosticCounterV1::RejectedAtGuard, 1);
        }
        finish_lance_query_stage(span, &region_result);
        region_result.map(|region| region.map(|(page, work)| (page, work, owner_capped)))
    };

    let mut deferred = Vec::new();
    let mut deferred_memory = None;
    let ranked = rank_lance_regions(directory, query, query_norm, context)?;
    let region_count = ranked.entries.len();
    for (index, region) in ranked.entries.iter().enumerate() {
        let ordinal = region.ordinal;
        let remaining_regions = region_count
            .checked_sub(index)
            .ok_or(IndexError::ResourceLimited)?;
        let seen_before = seen.len();
        let slots = durable_limit
            .checked_sub(seen_before)
            .ok_or(IndexError::ResourceLimited)?;
        if slots == 0 {
            // Every descriptor was authenticated during ranking. Do not open
            // a fileset when no selected owner slot remains.
            *capped = true;
            let diagnostic = context.budget.request().diagnostic_context();
            let span = crate::build_diagnostic::begin_build_stage(
                diagnostic,
                DiagnosticStageV1::QueryCandidate,
            );
            if let (Some(diagnostic), Some(span)) = (diagnostic, span.as_ref()) {
                diagnostic.counter(span, DiagnosticCounterV1::RejectedAtGuard, 1);
            }
            crate::build_diagnostic::finish_build_stage(span, true);
            continue;
        }
        // Reserve part of the remaining owner window for subsequent
        // regions. Unused slots carry forward; only a new seen owner
        // consumes a slot, including stale owners.
        let owner_slots = slots
            .checked_div(remaining_regions)
            .ok_or(IndexError::ResourceLimited)?
            .max(1);
        let Some((page, hydration_work, owner_capped)) = query_region(
            ordinal,
            &region.projection,
            LanceRegionWindow {
                owner_slots,
                candidate_cap: ann_lance::MAX_REGION_CANDIDATES,
                minimum_prefix: None,
            },
            heap,
            seen,
            context,
        )?
        else {
            *capped = true;
            continue;
        };
        if !owner_capped && !region_was_capped(page.traversal) {
            continue;
        }
        let candidate_cap = ann_lance::MAX_REGION_CANDIDATES
            .checked_sub(page.requested)
            .ok_or(IndexError::ResourceLimited)?;
        let rows = usize::try_from(region.projection.rows)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let minimum_prefix = page
            .last_prefix
            .saturating_mul(2)
            .min(rows)
            .min(candidate_cap);
        // Retry only when the reserved owner window could have hidden a
        // result. Repeating the same prefix only helps when an owner
        // was refused inside it.
        if remaining_regions > 1
            && seen.len().checked_sub(seen_before) == Some(owner_slots)
            && (minimum_prefix > page.last_prefix
                || owner_capped && minimum_prefix == page.last_prefix)
        {
            if deferred.capacity() == 0 {
                let capacity = usize::try_from(directory.region_count)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .min(durable_limit);
                let bytes = capacity
                    .checked_mul(std::mem::size_of::<DeferredLanceRegion>())
                    .and_then(|bytes| u64::try_from(bytes).ok())
                    .ok_or(IndexError::ResourceLimited)?;
                deferred_memory = context
                    .budget
                    .reserve_memory(bytes, MemoryLifetime::Temporary)?;
                deferred
                    .try_reserve_exact(capacity)
                    .map_err(|_error| IndexError::ResourceLimited)?;
                if deferred.capacity() != capacity {
                    return Err(IndexError::ResourceLimited);
                }
            }
            if deferred.len() == deferred.capacity() {
                return Err(IndexError::ResourceLimited);
            }
            deferred.push(DeferredLanceRegion {
                ordinal,
                projection: region.projection,
                candidate_cap,
                minimum_prefix,
                hydration_work,
            });
        } else {
            *capped = true;
        }
    }
    drop(ranked);

    // Spend unused owner slots even when the heap is full: previously
    // deferred owners may outrank it. Earlier filesets are already released.
    for region in deferred {
        context.budget.checkpoint()?;
        let owner_slots = durable_limit
            .checked_sub(seen.len())
            .ok_or(IndexError::ResourceLimited)?;
        if owner_slots == 0 {
            *capped = true;
            break;
        }
        if !context.budget.can_charge(region.hydration_work) {
            *capped = true;
            continue;
        }
        let Some((page, _hydration_work, owner_capped)) = query_region(
            region.ordinal,
            &region.projection,
            LanceRegionWindow {
                owner_slots,
                candidate_cap: region.candidate_cap,
                minimum_prefix: Some(region.minimum_prefix),
            },
            heap,
            seen,
            context,
        )?
        else {
            *capped = true;
            continue;
        };
        *capped |= owner_capped || region_was_capped(page.traversal);
    }
    drop(deferred_memory);
    Ok(())
}

#[cfg(test)]
pub(super) fn search(
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    super::ann_directory::validate_coverage_layout(query.coverage)?;
    let directory = AnnDirectory::load_for_coverage(query.coverage, query.recipe, context)?;
    search_loaded(query, directory.as_ref(), None, context, None)
}

pub(super) fn search_loaded(
    query: &SearchQuery<'_>,
    directory: Option<&AnnDirectory>,
    lexical: Option<super::lexical_scope::SelectedLexicalScope>,
    context: &mut ReadContext<'_>,
    runtime: Option<&tokio::runtime::Runtime>,
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
    if let Some(selected) = lexical {
        return super::lexical_scope::rank_selected(query, selected, None, context);
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
    let backend = directory.backend()?;
    if small_exact {
        // Exact ranking owns its heap. Collect the authenticated delta before
        // admitting the separate ANN owner table and candidate heap.
        let mut exact_delta = ExactDeltaMembers::reserve(delta_owners, context)?;
        visit_delta_members(
            directory,
            query.page_size,
            context,
            |_, reference, binding, _| {
                exact_delta.entries.push((reference, binding));
                Ok(())
            },
        )?;
        return search_exact_with_delta(query, Some(exact_delta), context);
    }
    let candidate_cap = match backend {
        AnnBackend::Graph => native_graph::MAX_REGION_CANDIDATES,
        AnnBackend::LanceDb => ann_lance::MAX_REGION_CANDIDATES,
    };
    let region_count =
        usize::try_from(directory.region_count).map_err(|_error| IndexError::ResourceLimited)?;
    let (route_limit, max_candidates) =
        candidate_capacity(backend, region_count, candidate_cap, delta_owners)?;
    let seen_capacity = durable_limit.min(max_candidates);
    let mut seen = SeenOwners {
        entries: Vec::new(),
        memory: None,
        limit: seen_capacity,
    };
    context.budget.checkpoint()?;
    if matches!(backend, AnnBackend::Graph) && seen_capacity != 0 {
        let memory = context
            .budget
            .reserve_memory(SeenOwners::bytes(seen_capacity)?, MemoryLifetime::Temporary)?;
        seen.replace_allocation(seen_capacity, memory)?;
    }
    let mut heap = RankedHits::for_query(query, context.budget)?;
    let mut capped = top_k > candidate_cap || region_count > route_limit;

    visit_delta_members(
        directory,
        query.page_size,
        context,
        |owner, reference, binding, context| {
            let _rescored = rank_selected(
                owner,
                reference,
                Candidate::Delta(&binding),
                query,
                query_norm,
                &mut heap,
                &mut seen,
                durable_limit,
                &mut capped,
                None,
                context,
            )?;
            Ok(())
        },
    )?;

    match backend {
        AnnBackend::Graph => search_graph_regions(
            directory,
            route_limit,
            query,
            query_norm,
            top_k,
            &mut heap,
            &mut seen,
            durable_limit,
            &mut capped,
            context,
        )?,
        AnnBackend::LanceDb => search_lance_regions(
            directory,
            query,
            query_norm,
            top_k,
            &mut heap,
            &mut seen,
            durable_limit,
            &mut capped,
            context,
            runtime,
        )?,
    }
    let scanned_owners = u32::try_from(seen.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let skipped_stale = u32::try_from(seen.entries.iter().filter(|entry| entry.stale).count())
        .map_err(|_error| IndexError::ResourceLimited)?;
    // The owner table is complete at this boundary. Overlay embedding and
    // final result custody must be able to use its execution memory.
    drop(seen);
    finish_ranked(
        query,
        query_norm,
        heap,
        scanned_owners,
        skipped_stale,
        NativeSemanticTraversal::Approximate { capped },
        context,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "share the request's ranking and budget across visited regions"
)]
fn search_graph_regions(
    directory: &AnnDirectory,
    route_limit: usize,
    query: &SearchQuery<'_>,
    query_norm: f64,
    top_k: usize,
    heap: &mut RankedHits,
    seen: &mut SeenOwners,
    durable_limit: usize,
    capped: &mut bool,
    context: &mut ReadContext<'_>,
) -> Result<(), IndexError> {
    let RoutedRegions {
        entries,
        _memory: route_memory,
    } = route_graph_regions(directory, route_limit, query, query_norm, context)?;
    for routed in entries {
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
                    && !(query.source_files && !heap.is_full() && seen.len() < durable_limit)
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
                    heap,
                    seen,
                    durable_limit,
                    capped,
                    Some(&mut cached),
                    context,
                )?;
                if did_rescore.is_some() {
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
            *capped = true;
        }
    }
    drop(route_memory);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AnnBackend, NativeSemanticTraversal, candidate_capacity, region_was_capped};

    #[test]
    fn ann_candidate_capacity_tracks_only_routable_regions() -> Result<(), super::IndexError> {
        assert_eq!(candidate_capacity(AnnBackend::Graph, 0, 1024, 1)?, (0, 1));
        assert_eq!(
            candidate_capacity(AnnBackend::Graph, 1, 1024, 1)?,
            (1, 1025)
        );
        assert_eq!(
            candidate_capacity(AnnBackend::Graph, 9, 1024, 1)?,
            (8, 8193)
        );
        assert_eq!(
            candidate_capacity(AnnBackend::LanceDb, 9, 1024, 1)?,
            (9, 9217)
        );
        Ok(())
    }

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
