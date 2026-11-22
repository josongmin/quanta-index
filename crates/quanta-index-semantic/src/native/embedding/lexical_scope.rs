//! Same-view lexical eligibility, authenticated before semantic candidate work.

use quanta_index_contract::{TextQuerySyntax, native_protocol::MAX_NATIVE_TEXT_TOP_K};
use quanta_index_core::index::{
    CoverageBinding, CoverageStatus, IndexError, MapRoot, MemoryLease, MemoryLifetime,
    NativeTextHitSource, ObjectRef, OwnerKey, ProjectionBinding, ViewRef, owner_from_map_key,
    owner_map_key,
};

use super::{
    NativeSemanticTraversal, RankedHits, ReadContext, SearchQuery, SearchResult,
    SelectedArtifactInput, SemanticReadRequest,
    ann_directory::AnnDirectory,
    finish_ranked, load_source_record_from_objects, rank_member,
    selected_owner::{AuthenticatedSource, SelectedOwner, SourceScope, scan_ordered_page},
    validate_search_query,
};

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
            || u64::try_from(eligible.entries().len())
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

    pub(super) const fn new(limit: usize, overlay_owners: u32) -> Self {
        Self {
            owners: Vec::new(),
            overlay_owners,
            limit,
            backing_memory: None,
        }
    }

    fn push(&mut self, owner: SelectedOwner, context: &ReadContext<'_>) -> Result<(), IndexError> {
        if self.owners.len() >= self.limit {
            return Err(IndexError::ResourceLimited);
        }
        if self.owners.len() == self.owners.capacity() {
            let next = self
                .owners
                .capacity()
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
                .max(1)
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

/// A source record authenticated against the selected view and path constraint.
pub(super) struct SelectedProjectedSource {
    authority: AuthenticatedSource,
    pub(super) file_record: Option<std::sync::Arc<super::source_file::SharedFileRecord>>,
}

impl SelectedProjectedSource {
    pub(super) fn from_path(
        owner: OwnerKey,
        path: &str,
        source_owners: MapRoot,
        source_files: bool,
        context: &mut ReadContext<'_>,
    ) -> Result<Self, IndexError> {
        let reference = context
            .maps
            .lookup_object(source_owners, &owner_map_key(owner), context.budget)?
            .ok_or_else(|| IndexError::CorruptObject("source path has no selected owner".into()))?;
        let record = super::load_source_record_with_lifetime(
            reference,
            context.objects,
            context.budget,
            if source_files {
                MemoryLifetime::Retained
            } else {
                MemoryLifetime::Temporary
            },
        )?;
        if record.path != path {
            return Err(IndexError::CorruptObject(
                "source path reverse record differs".into(),
            ));
        }
        Self::from_authenticated(
            owner,
            reference,
            source_owners,
            record,
            source_files,
            context,
        )
    }

    /// Construct from a source already checked against the path and selected cut.
    pub(super) fn from_authenticated(
        owner: OwnerKey,
        reference: ObjectRef,
        source_owners: MapRoot,
        record: super::SourceRecordOwner,
        source_files: bool,
        context: &mut ReadContext<'_>,
    ) -> Result<Self, IndexError> {
        let file_record = if source_files {
            Some(super::source_file::SharedFileRecord::retain(
                record,
                context.budget,
            )?)
        } else {
            None
        };
        Ok(Self {
            authority: AuthenticatedSource {
                root: source_owners,
                owner,
                reference,
            },
            file_record,
        })
    }
}

/// Append projected members up to the reserved cap sentinel by reverse lookup.
#[expect(
    clippy::too_many_arguments,
    reason = "the source proof is checked against the current source and coverage roots"
)]
pub(super) fn select_projected_source(
    source: &SelectedProjectedSource,
    coverage: &CoverageBinding,
    projection: &ProjectionBinding,
    source_owners: MapRoot,
    source_reverse: MapRoot,
    selected: &mut SelectedLexicalScope,
    source_files: bool,
    context: &mut ReadContext<'_>,
) -> Result<(), IndexError> {
    if source.authority.root != source_owners {
        return Err(IndexError::CorruptObject(
            "projected reverse source stamp differs".into(),
        ));
    }
    let source_owner = source.authority.owner;
    let prefix = owner_map_key(source_owner);
    let limit = selected.limit;
    if selected.owners.len() >= limit {
        return Ok(());
    }
    let mut cursor: Option<[u8; 76]> = None;
    loop {
        let after: &[u8] = cursor
            .as_ref()
            .map_or(prefix.as_slice(), <[u8; 76]>::as_slice);
        let page = scan_ordered_page(source_reverse, Some(after), 64, cursor.is_some(), context)?;
        let mut last = None;
        for (key, value) in &page.entries {
            context.budget.checkpoint()?;
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
                SourceScope::VerifiedProjection(&source.authority),
                context,
            )?;
            last = Some(pair);
            if source_files && owner.has_no_candidate(true) {
                continue;
            }
            selected.push(owner, context)?;
            if selected.owners.len() == limit {
                return Ok(());
            }
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
) -> Result<SelectedProjectedSource, IndexError> {
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
    SelectedProjectedSource::from_authenticated(
        source_owner,
        source,
        source_owners,
        record,
        false,
        context,
    )
}

pub(super) fn prepare(
    request: &SemanticReadRequest,
    view: ViewRef,
    coverage: &CoverageBinding,
    projection: Option<&ProjectionBinding>,
    source_owners: MapRoot,
    directory: Option<&AnnDirectory>,
    max_owners: u32,
    context: &mut ReadContext<'_>,
) -> Result<Option<SelectedLexicalScope>, IndexError> {
    let Some(eligible) = &request.lexical_eligibility else {
        return Ok(None);
    };
    if eligible.view() != view {
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
    if projection.is_some() && !projected_empty && directory.is_none() {
        return Err(IndexError::RebuildRequired);
    }
    let durable_count = eligible
        .entries()
        .iter()
        .filter(|entry| matches!(entry.source, NativeTextHitSource::Durable(_)))
        .count();
    let durable_limit = max_owners
        .checked_sub(eligible.overlay_count()?)
        .ok_or(IndexError::ResourceLimited)?;
    let selected_capacity = if projected_empty {
        0
    } else if projection.is_some() && durable_count > 0 {
        usize::try_from(durable_limit)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?
    } else {
        durable_count
    };
    let mut selected = SelectedLexicalScope::new(selected_capacity, 0);
    for entry in eligible.entries() {
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
        if projection.is_some() {
            let source = authenticate_projected_source(
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
            if let (Some(binding), Some(directory)) = (projection, directory) {
                select_projected_source(
                    &source,
                    coverage,
                    binding,
                    source_owners,
                    directory.source_reverse,
                    &mut selected,
                    false,
                    context,
                )?;
            }
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

/// Exact ranking over an already authenticated finite selected-owner set.
pub(super) fn rank_selected(
    query: &SearchQuery<'_>,
    selected: SelectedLexicalScope,
    file_record: Option<std::sync::Arc<super::source_file::SharedFileRecord>>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    let norm = validate_search_query(query)?;
    let mut heap = RankedHits::for_query(query, context.budget)?;
    let mut scanned = 0_u32;
    let mut stale = 0_u32;
    let mut traversal = NativeSemanticTraversal::ExactExhausted;
    let durable_limit = query
        .max_owners
        .checked_sub(selected.overlay_owners)
        .ok_or(IndexError::ResourceLimited)?;
    for owner in &selected.owners {
        context.budget.checkpoint()?;
        if scanned == durable_limit {
            traversal = NativeSemanticTraversal::ExactCapped;
            break;
        }
        scanned = scanned.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        if owner.coverage.covered == Some(owner.coverage.desired)
            && owner.coverage.searchable.is_none()
            && owner
                .contribution
                .is_some_and(|value| value.output.is_none())
        {
            // A current projection with no output intentionally has no
            // member. It still consumes one selected-owner scan slot.
            continue;
        }
        if owner.coverage.covered != Some(owner.coverage.desired)
            || owner.coverage.searchable.is_none()
        {
            stale = stale.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            continue;
        }
        let member = owner.current_member(query, context)?;
        let _rescored = rank_member(
            owner,
            &member,
            owner
                .contribution
                .map(|contribution| SelectedArtifactInput::Projection {
                    contribution,
                    file_record: file_record
                        .as_ref()
                        .cloned()
                        .map(super::source_file::FileRecord::Shared),
                }),
            None,
            query,
            norm,
            &mut heap,
            context,
        )?;
    }
    drop(selected);
    drop(file_record);
    finish_ranked(query, norm, heap, scanned, stale, traversal, context)
}
