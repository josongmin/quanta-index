//! Same-view lexical eligibility, authenticated before semantic candidate work.

use std::collections::BinaryHeap;

use quanta_index_contract::{TextQuerySyntax, native_protocol::MAX_NATIVE_TEXT_TOP_K};
use quanta_index_core::index::{
    CoverageBinding, CoverageStatus, IndexError, MapRoot, MemoryLease, MemoryLifetime,
    NativeTextHitSource, ObjectRef, OwnerKey, ProjectionBinding, ViewRef, owner_from_map_key,
    owner_map_key,
};

use super::{
    EmbeddingRecipe, NativeSemanticTraversal, RankedHit, ReadContext, SearchQuery, SearchResult,
    SemanticReadRequest, ann_directory::AnnDirectory, finish_ranked,
    load_source_record_from_objects, rank_member, selected_owner::SelectedOwner,
    validate_search_query,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticLexicalEligibleSource {
    pub owner: OwnerKey,
    pub source: NativeTextHitSource,
}

/// A request-local copy of the validated lexical hit universe, not a query plan.
/// Data is destroyed before its original retained allocation grant.
#[derive(Debug)]
pub struct SemanticLexicalEligibility {
    view: ViewRef,
    entries: Vec<SemanticLexicalEligibleSource>,
    entries_memory: Option<MemoryLease>,
}

impl SemanticLexicalEligibility {
    pub fn new(
        view: ViewRef,
        entries: Vec<SemanticLexicalEligibleSource>,
        entries_memory: Option<MemoryLease>,
    ) -> Result<Self, IndexError> {
        // Form the owner before any error: function parameters otherwise drop
        // in reverse order and could refund the grant before the Vec backing.
        let mut value = Self {
            view,
            entries,
            entries_memory,
        };
        let maximum =
            usize::try_from(MAX_NATIVE_TEXT_TOP_K).map_err(|_error| IndexError::ResourceLimited)?;
        if value.entries.capacity() > maximum {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = value
            .entries
            .capacity()
            .checked_mul(std::mem::size_of::<SemanticLexicalEligibleSource>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        // None is the existing explicit standalone allocation contract.
        if value
            .entries_memory
            .as_ref()
            .is_some_and(|memory| memory.bytes() < bytes)
        {
            return Err(IndexError::ResourceLimited);
        }
        value.entries.sort_unstable_by_key(|entry| entry.owner);
        if value
            .entries
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.owner == right.owner))
        {
            return Err(IndexError::InvalidInput(
                "lexical eligibility repeats an owner".into(),
            ));
        }
        Ok(value)
    }

    pub(super) fn allows_overlay(&self, owner: OwnerKey, revision: u64) -> bool {
        self.entries
            .binary_search_by_key(&owner, |entry| entry.owner)
            .is_ok_and(|index| {
                self.entries
                    .get(index)
                    .is_some_and(|entry| entry.source == NativeTextHitSource::Overlay { revision })
            })
    }

    pub(super) fn overlay_count(&self) -> Result<u32, IndexError> {
        u32::try_from(
            self.entries
                .iter()
                .filter(|entry| matches!(entry.source, NativeTextHitSource::Overlay { .. }))
                .count(),
        )
        .map_err(|_error| IndexError::ResourceLimited)
    }
}

impl SemanticReadRequest {
    pub(super) fn validate_lexical_scope(&self) -> Result<(), IndexError> {
        let (scope, eligible) = match (&self.query.lexical_scope, &self.lexical_eligibility) {
            (None, None) => return Ok(()),
            (Some(scope), Some(eligible)) => (scope, eligible),
            _ => {
                return Err(IndexError::InvalidInput(
                    "lexical scope has no matching eligibility".into(),
                ));
            }
        };
        if scope.constraints != self.query.constraints
            || scope.syntax != TextQuerySyntax::Native
            || scope.query_text.trim().is_empty()
            || scope.generation.is_some()
            || scope.generation_selector.is_some()
            || scope.cursor.is_some()
            || !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&scope.top_k)
            || u64::try_from(eligible.entries.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                > u64::from(scope.top_k)
        {
            return Err(IndexError::InvalidInput(
                "lexical scope requires matching bounded Native eligibility".into(),
            ));
        }
        Ok(())
    }
}

pub(super) struct SelectedLexicalScope {
    pub(super) owners: Vec<SelectedOwner>,
    pub(super) overlay_owners: u32,
    limit: usize,
    backing_memory: Option<MemoryLease>,
}

impl SelectedLexicalScope {
    fn allocate(
        capacity: usize,
        context: &ReadContext<'_>,
    ) -> Result<(Vec<SelectedOwner>, Option<MemoryLease>), IndexError> {
        let bytes = capacity
            .checked_mul(std::mem::size_of::<SelectedOwner>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            None
        } else {
            context.budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
        };
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        // The allocator may grant more than requested. Never retain bytes
        // beyond the request-local lease, even on a constructor error.
        if owners.capacity() > capacity {
            return Err(IndexError::ResourceLimited);
        }
        Ok((owners, memory))
    }

    pub(super) fn reserve(limit: usize, context: &ReadContext<'_>) -> Result<Self, IndexError> {
        let (owners, memory) = Self::allocate(limit.min(64), context)?;
        Ok(Self {
            owners,
            overlay_owners: 0,
            limit,
            backing_memory: memory,
        })
    }

    fn push(&mut self, owner: SelectedOwner, context: &ReadContext<'_>) -> Result<(), IndexError> {
        if self.owners.len() >= self.limit {
            return Err(IndexError::ResourceLimited);
        }
        if self.owners.len() == self.owners.capacity() {
            let next = self
                .owners
                .capacity()
                .max(1)
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
                .min(self.limit);
            // Admit the whole replacement while the old backing is still
            // charged. A failed reservation leaves the original scope intact.
            let (mut replacement, memory) = Self::allocate(next, context)?;
            replacement.append(&mut self.owners);
            let old_owners = std::mem::replace(&mut self.owners, replacement);
            let old_memory = std::mem::replace(&mut self.backing_memory, memory);
            drop(old_owners);
            drop(old_memory);
        }
        self.owners.push(owner);
        Ok(())
    }
}

/// A complete selected projection with no desired or materialized owners is
/// an authenticated empty universe. No ANN directory can add authority here.
pub(super) fn selected_projected_empty(coverage: &CoverageBinding) -> bool {
    matches!(coverage.status, CoverageStatus::Complete)
        && coverage.covered == Some(coverage.desired)
        && coverage.desired.root == MapRoot(None)
        && coverage.owners == MapRoot(None)
        && coverage.searchable.is_none()
}

/// The selected directory is a search layout, never source authority.
pub(super) fn selected_projected_directory(
    coverage: &CoverageBinding,
    recipe: &EmbeddingRecipe,
    context: &mut ReadContext<'_>,
) -> Result<AnnDirectory, IndexError> {
    let reference = coverage.searchable.ok_or(IndexError::RebuildRequired)?;
    if coverage.searchable == coverage.owners.0 {
        return Err(IndexError::RebuildRequired);
    }
    let directory = AnnDirectory::load(reference, recipe.fingerprint(), context)?;
    if directory.owners != coverage.owners {
        return Err(IndexError::CorruptObject(
            "projected reverse directory owners differ from selected coverage".into(),
        ));
    }
    Ok(directory)
}

/// Append at most `limit` projected members for one authenticated source owner.
/// The last slot is the caller's cap sentinel; no whole-owner scan is used.
#[expect(
    clippy::too_many_arguments,
    reason = "selected source, directory, coverage and bounded result custody remain explicit"
)]
pub(super) fn select_projected_source(
    source_owner: OwnerKey,
    source: ObjectRef,
    expected_path: Option<&str>,
    coverage: &CoverageBinding,
    projection: &ProjectionBinding,
    source_owners: MapRoot,
    source_reverse: MapRoot,
    limit: usize,
    selected: &mut SelectedLexicalScope,
    context: &mut ReadContext<'_>,
) -> Result<(), IndexError> {
    authenticate_projected_source(source_owner, source, expected_path, source_owners, context)?;
    let prefix = owner_map_key(source_owner);
    if selected.limit != limit {
        return Err(IndexError::BackendViolation(
            "projected selection limit differs from reserved scope".into(),
        ));
    }
    if selected.owners.len() >= limit {
        return Ok(());
    }
    let mut cursor: Option<[u8; 76]> = None;
    loop {
        context.budget.checkpoint()?;
        let after: &[u8] = cursor
            .as_ref()
            .map_or(prefix.as_slice(), <[u8; 76]>::as_slice);
        let page = context
            .maps
            .scan(source_reverse, Some(after), 64, context.budget)?;
        if page.entries.len() > 64 || page.entries.is_empty() && page.has_more {
            return Err(IndexError::BackendViolation(
                "projected reverse map page violates bounded scan".into(),
            ));
        }
        let mut previous = after;
        let mut last = None;
        for (key, value) in &page.entries {
            context.budget.checkpoint()?;
            if key.as_slice() <= previous {
                return Err(IndexError::BackendViolation(
                    "projected reverse map page order differs".into(),
                ));
            }
            previous = key.as_slice();
            if !key.starts_with(&prefix) {
                return Ok(());
            }
            let pair: [u8; 76] = key.as_slice().try_into().map_err(|_error| {
                IndexError::CorruptObject("projected reverse key width differs".into())
            })?;
            let (_source_key, semantic_key_bytes) = pair.split_at(38);
            let semantic_owner = owner_from_map_key(semantic_key_bytes)?;
            let semantic_key = owner_map_key(semantic_owner);
            if context
                .maps
                .lookup(coverage.owners, &semantic_key, context.budget)?
                != Some(*value)
            {
                return Err(IndexError::CorruptObject(
                    "projected reverse value differs from selected coverage".into(),
                ));
            }
            let owner = SelectedOwner::load_selected(
                &semantic_key,
                *value,
                coverage,
                Some(projection),
                source_owners,
                context,
            )?;
            let contribution = owner.contribution.ok_or_else(|| {
                IndexError::CorruptObject("projected reverse owner lacks contribution".into())
            })?;
            if contribution.source_owner != Some(source_owner)
                || contribution.source != Some(source)
            {
                return Err(IndexError::CorruptObject(
                    "projected reverse source stamp differs".into(),
                ));
            }
            selected.push(owner, context)?;
            if selected.owners.len() == limit {
                return Ok(());
            }
            last = Some(pair);
        }
        if !page.has_more {
            return Ok(());
        }
        cursor = last;
    }
}

fn authenticate_projected_source(
    source_owner: OwnerKey,
    source: ObjectRef,
    expected_path: Option<&str>,
    source_owners: MapRoot,
    context: &mut ReadContext<'_>,
) -> Result<(), IndexError> {
    context.budget.checkpoint()?;
    let prefix = owner_map_key(source_owner);
    if context
        .maps
        .lookup_object(source_owners, &prefix, context.budget)?
        != Some(source)
    {
        return Err(IndexError::CorruptObject(
            "projected lexical source differs from selected source owner".into(),
        ));
    }
    let record = load_source_record_from_objects(source, context.objects, context.budget)?;
    if expected_path.is_some_and(|path| path != record.path) {
        return Err(IndexError::BackendViolation(
            "projected lexical source violates selected path constraint".into(),
        ));
    }
    Ok(())
}

pub(super) fn prepare(
    request: &SemanticReadRequest,
    view: ViewRef,
    coverage: &CoverageBinding,
    projection: Option<&ProjectionBinding>,
    source_owners: MapRoot,
    recipe: &EmbeddingRecipe,
    max_owners: u32,
    context: &mut ReadContext<'_>,
) -> Result<Option<SelectedLexicalScope>, IndexError> {
    let Some(eligible) = &request.lexical_eligibility else {
        return Ok(None);
    };
    if eligible.view != view {
        return Err(IndexError::SnapshotUnavailable);
    }
    if matches!(coverage.status, CoverageStatus::Complete)
        && (coverage.covered != Some(coverage.desired)
            || coverage.owners == MapRoot(None) && coverage.desired.root != MapRoot(None))
    {
        return Err(IndexError::CorruptObject(
            "complete lexical scope coverage differs".into(),
        ));
    }
    let projected_empty = projection.is_some() && selected_projected_empty(coverage);
    let directory = projection
        .filter(|_binding| !projected_empty)
        .map(|_binding| selected_projected_directory(coverage, recipe, context))
        .transpose()?;
    let durable_count = eligible
        .entries
        .iter()
        .filter(|entry| matches!(entry.source, NativeTextHitSource::Durable(_)))
        .count();
    let durable_limit = max_owners
        .checked_sub(eligible.overlay_count()?)
        .ok_or(IndexError::ResourceLimited)?;
    let selected_capacity = if projected_empty {
        0
    } else if directory.is_some() && durable_count > 0 {
        usize::try_from(durable_limit)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?
    } else {
        durable_count
    };
    let mut selected = SelectedLexicalScope::reserve(selected_capacity, context)?;
    for entry in &eligible.entries {
        context.budget.checkpoint()?;
        if let NativeTextHitSource::Overlay { revision } = entry.source {
            let document = match request
                .overlay
                .binary_search_by_key(&entry.owner, |row| row.owner)
            {
                Ok(index) => request.overlay.get(index),
                Err(_missing) => None,
            }
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "lexical overlay owner is absent from captured semantic overlay".into(),
                )
            })?;
            if document.revision != revision || document.text.is_none() {
                return Err(IndexError::BackendViolation(
                    "lexical overlay revision or live body differs from captured overlay".into(),
                ));
            }
            if request
                .query
                .constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|path| document.path.as_deref() != Some(path.as_str()))
            {
                return Err(IndexError::BackendViolation(
                    "lexical overlay violates selected path constraint".into(),
                ));
            }
            selected.overlay_owners = selected
                .overlay_owners
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            continue;
        }
        let NativeTextHitSource::Durable(source) = entry.source else {
            return Err(IndexError::BackendViolation(
                "lexical hit has unknown source origin".into(),
            ));
        };
        if request
            .overlay
            .binary_search_by_key(&entry.owner, |row| row.owner)
            .is_ok()
        {
            return Err(IndexError::BackendViolation(
                "lexical durable owner is masked by captured overlay".into(),
            ));
        }
        if let (Some(binding), Some(directory)) = (projection, directory) {
            select_projected_source(
                entry.owner,
                source,
                request
                    .query
                    .constraints
                    .repo_relative_path_exact
                    .as_ref()
                    .map(quanta_index_contract::ExactRepoRelativePathV1::as_str),
                coverage,
                binding,
                source_owners,
                directory.source_reverse,
                selected_capacity,
                &mut selected,
                context,
            )?;
            continue;
        }
        if projected_empty {
            authenticate_projected_source(
                entry.owner,
                source,
                request
                    .query
                    .constraints
                    .repo_relative_path_exact
                    .as_ref()
                    .map(quanta_index_contract::ExactRepoRelativePathV1::as_str),
                source_owners,
                context,
            )?;
            continue;
        }
        let key = owner_map_key(entry.owner);
        let reference = context
            .maps
            .lookup(coverage.owners, &key, context.budget)?
            .ok_or_else(|| {
                IndexError::CorruptObject("lexical owner has no selected semantic coverage".into())
            })?;
        let owner =
            SelectedOwner::load_source_local(&key, reference, coverage, source_owners, context)?;
        if owner.coverage.desired.source != Some(source) {
            return Err(IndexError::CorruptObject(
                "lexical source differs from selected owner".into(),
            ));
        }
        // Authenticate the actual source record, including its physical edges,
        // before provider work; the Text result cannot substitute another path.
        let source = load_source_record_from_objects(source, context.objects, context.budget)?;
        if request
            .query
            .constraints
            .repo_relative_path_exact
            .as_ref()
            .is_some_and(|path| path.as_str() != source.path)
        {
            return Err(IndexError::BackendViolation(
                "lexical owner violates selected path constraint".into(),
            ));
        }
        selected.push(owner, context)?;
    }
    Ok(Some(selected))
}

pub(super) fn search(
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<Option<SearchResult>, IndexError> {
    let Some(owners) = query.lexical_owners else {
        return Ok(None);
    };
    let overlay_count = query
        .lexical_eligibility
        .ok_or(IndexError::SnapshotUnavailable)?
        .overlay_count()?;
    rank_selected(query, owners, overlay_count, context).map(Some)
}

/// Exact ranking over an already authenticated finite selected-owner set.
/// `overlay_count` is the caller's original cap charge for overlay rows.
pub(super) fn rank_selected(
    query: &SearchQuery<'_>,
    owners: &[SelectedOwner],
    overlay_count: u32,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    let norm = validate_search_query(query)?;
    let top_k = usize::try_from(query.top_k).map_err(|_error| IndexError::ResourceLimited)?;
    let bytes = top_k
        .checked_mul(std::mem::size_of::<RankedHit>())
        .ok_or(IndexError::ResourceLimited)?;
    let _heap_memory = context.budget.reserve_memory(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut heap = BinaryHeap::new();
    heap.try_reserve_exact(top_k)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if heap.capacity() > top_k {
        return Err(IndexError::ResourceLimited);
    }
    let mut scanned = 0_u32;
    let mut stale = 0_u32;
    let mut traversal = NativeSemanticTraversal::ExactExhausted;
    let durable_limit = query
        .max_owners
        .checked_sub(overlay_count)
        .ok_or(IndexError::ResourceLimited)?;
    for selected in owners {
        context.budget.checkpoint()?;
        if scanned == durable_limit {
            traversal = NativeSemanticTraversal::ExactCapped;
            break;
        }
        scanned = scanned.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        if selected.coverage.covered == Some(selected.coverage.desired)
            && selected.coverage.searchable.is_none()
            && selected
                .contribution
                .is_some_and(|value| value.output.is_none())
        {
            // A current projection with no output intentionally has no
            // member. It still consumes one selected-owner scan slot.
            continue;
        }
        if selected.coverage.covered != Some(selected.coverage.desired)
            || selected.coverage.searchable.is_none()
        {
            stale = stale.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            continue;
        }
        let member = selected.current_member(query, context)?;
        rank_member(
            &member,
            selected.contribution,
            None,
            query,
            norm,
            top_k,
            &mut heap,
            context,
        )?;
    }
    finish_ranked(query, norm, top_k, heap, scanned, stale, traversal, context)
}
