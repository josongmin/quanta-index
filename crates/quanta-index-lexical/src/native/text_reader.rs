//! Ticket-bound query adapter for the native text member. The planner IR and
//! BM25 execution remain owned by their existing lexical modules.

use std::{borrow::Cow, sync::Arc};

use super::text::{BoundTextMember, authenticated_text_source, valid_path};
use super::text_memory::PreparedRows;
use super::{Bm25Policy, NativeTextIndex, TextReadContext, TextRecipe};
use super::{
    text_overlay::{
        MAX_TEXT_OVERLAY_DOCUMENT_BYTES, MAX_TEXT_OVERLAY_DOCUMENTS, MAX_TEXT_OVERLAY_TOTAL_BYTES,
        TextOverlayInput,
    },
    text_query::{
        NativeTextHitSource, PreviewSelection, PreviewSource, PreviewVisitor, TextContentExecution,
        TextOverlaySource,
    },
};
use crate::plan::LexicalPlan;
use crate::symbol::ResultSurface;
use quanta_index_contract::{
    PreviewUnavailableReason, RepoId,
    native_protocol::{MAX_NATIVE_TEXT_TOP_K, NativeTextPreview, NativeTextPreviewEvidence},
};
use quanta_index_core::index::{
    FiniteScore, IndexError, IndexQueryPort, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey,
    QueryOutcome, QueryReadContext, SourcePathOwners, SourceRecordOwner, WorkBudget, WorkUsage,
    effort::ResourceNeed, owner_map_key, ports::IndexBuildPort, visit_repository_path_sources,
};

/// A planned request retains the canonical lexical plan and a bounded result cap.
/// Lowering from the public text-query request belongs to the calling runtime.
#[derive(Debug)]
pub struct PlannedTextRead {
    pub plan: LexicalPlan,
    pub top_k: u32,
    rank_after: Option<(FiniteScore, OwnerKey)>,
    path_rank_after: Option<(FiniteScore, RepoId, String)>,
    // The owned path/repository anchor dies before its retained grant.
    _path_rank_after_memory: Option<MemoryLease>,
    constraints: quanta_index_contract::QueryConstraintSetV1,
    // Owned path backing drops before its retained grant. Deep Clone is unavailable.
    _constraint_memory: Option<MemoryLease>,
    plan_memory: [Option<MemoryLease>; 3],
    preview: bool,
    overlay: Vec<TextOverlayReadDocument>,
    // The caller funds the owned row container before constructing it.
    overlay_memory: Option<MemoryLease>,
}

/// One captured overlay row. `None` text masks the selected durable owner;
/// a present empty string is an indexed empty document.
#[derive(Debug)]
pub struct TextOverlayReadDocument {
    pub owner: OwnerKey,
    pub revision: u64,
    pub text: Option<Arc<str>>,
    pub path: Option<Arc<str>>,
}

impl PlannedTextRead {
    /// Attach selected-source preview evidence to the first ranked result.
    #[must_use]
    pub fn with_preview(mut self) -> Self {
        self.preview = true;
        self
    }

    /// Continue the selected ranking after an authenticated same-view anchor.
    /// The query owner validates score and query binding before execution.
    #[must_use]
    pub fn with_rank_after(mut self, rank_after: Option<(FiniteScore, OwnerKey)>) -> Self {
        self.rank_after = rank_after;
        self
    }

    /// Continue a Path projection after its authenticated repository/path key.
    #[must_use]
    pub fn with_path_rank_after_v1(
        mut self,
        rank_after: Option<(FiniteScore, RepoId, String)>,
        memory: Option<MemoryLease>,
    ) -> Self {
        let old_anchor = std::mem::replace(&mut self.path_rank_after, rank_after);
        let old_memory = std::mem::replace(&mut self._path_rank_after_memory, memory);
        drop(old_anchor);
        drop(old_memory);
        self
    }

    /// Keep the caller's before-planning reservation until the owned plan dies.
    #[must_use]
    pub fn with_plan_memory(mut self, memory: [Option<MemoryLease>; 3]) -> Self {
        self.plan_memory = memory;
        self
    }

    /// Adopt the caller's atomically captured rows and their preallocation
    /// grant. Snapshot and Arc backing remain owned by the request.
    #[must_use]
    pub fn with_overlay(
        mut self,
        documents: Vec<TextOverlayReadDocument>,
        memory: Option<MemoryLease>,
    ) -> Self {
        let old_documents = std::mem::replace(&mut self.overlay, documents);
        let old_memory = std::mem::replace(&mut self.overlay_memory, memory);
        drop(old_documents);
        drop(old_memory);
        self
    }

    pub fn result_limit(&self) -> Result<u32, IndexError> {
        match self.plan.options.count {
            None => Ok(self.top_k),
            Some(quanta_index_contract::LqCountBound::Bounded(count)) if count > 0 => {
                Ok(self.top_k.min(count))
            }
            Some(_) => Err(IndexError::InvalidInput(
                "native BM25 requires a bounded positive count".into(),
            )),
        }
    }

    /// Copy the canonical constraints only after their retained backing is funded.
    /// Language selection remains unavailable without producer language metadata.
    pub fn new(
        plan: LexicalPlan,
        top_k: u32,
        constraints: &quanta_index_contract::QueryConstraintSetV1,
        reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
    ) -> Result<Self, IndexError> {
        if !constraints.language_any_of.is_empty() {
            return Err(IndexError::InvalidInput(
                "native text language metadata is unavailable".into(),
            ));
        }
        let bytes = constraints
            .repo_relative_path_exact
            .as_ref()
            .map_or(0, |path| path.as_str().len());
        let memory = if bytes == 0 {
            None
        } else {
            reserve(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?
        };
        Ok(Self {
            plan,
            top_k,
            rank_after: None,
            path_rank_after: None,
            _path_rank_after_memory: None,
            constraints: constraints.clone(),
            _constraint_memory: memory,
            plan_memory: [None, None, None],
            preview: false,
            overlay: Vec::new(),
            overlay_memory: None,
        })
    }
}

#[derive(Debug)]
pub struct TextReadHit {
    pub owner: OwnerKey,
    pub source: NativeTextHitSource,
    pub path: String,
    pub score: FiniteScore,
    // The path dies before its exact retained allocation charge.
    path_memory: Option<MemoryLease>,
    // A path projection transfers authenticated source-repository custody.
    path_group_repository: Option<(RepoId, Option<MemoryLease>)>,
}

impl PartialEq for TextReadHit {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.source == other.source
            && self.path == other.path
            && self.score == other.score
            && self.path_group_repository.as_ref().map(|(repo, _)| repo)
                == other.path_group_repository.as_ref().map(|(repo, _)| repo)
    }
}

impl TextReadHit {
    #[must_use]
    pub fn path_group_repository_v1(&self) -> Option<&RepoId> {
        self.path_group_repository.as_ref().map(|(repo, _)| repo)
    }

    /// Move repository identity and its retained allocation grant together.
    #[must_use]
    pub fn take_path_group_repository_v1(&mut self) -> Option<(RepoId, Option<MemoryLease>)> {
        self.path_group_repository.take()
    }

    /// Move the canonical hit and its path custody to the result mapper.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        OwnerKey,
        NativeTextHitSource,
        String,
        FiniteScore,
        Option<MemoryLease>,
    ) {
        let Self {
            owner,
            source,
            path,
            score,
            path_memory,
            path_group_repository,
        } = self;
        // A caller using the ordinary hit mapper must still retain the grant
        // that covers the path, even if it does not consume Path grouping.
        let path_memory =
            path_memory.or_else(|| path_group_repository.and_then(|(_, grant)| grant));
        (owner, source, path, score, path_memory)
    }
}

#[derive(Debug)]
pub struct TextReadResult {
    pub artifact: ObjectRef,
    pub matched: u64,
    /// Eligible matches strictly after the anchor, before the bounded page heap.
    pub remaining: u64,
    pub hits: Vec<TextReadHit>,
    previews: Option<OwnedTextPreviews>,
    // The hit container dies before this retained capacity charge.
    hit_memory: Option<MemoryLease>,
}

impl PartialEq for TextReadResult {
    fn eq(&self, other: &Self) -> bool {
        self.artifact == other.artifact
            && self.matched == other.matched
            && self.remaining == other.remaining
            && self.hits == other.hits
            && self.previews.as_ref().map(|preview| &preview.rows)
                == other.previews.as_ref().map(|preview| &preview.rows)
    }
}

impl TextReadResult {
    /// Transfer selected-source previews and their output allocation custody.
    #[must_use]
    pub fn take_previews(&mut self) -> Option<(Vec<NativeTextPreview>, Vec<MemoryLease>)> {
        self.previews
            .take()
            .map(|preview| (preview.rows, preview.memory))
    }

    /// Data and custody move together; mapping must retain both until conversion.
    #[must_use]
    pub fn into_parts(self) -> (ObjectRef, u64, Vec<TextReadHit>, Option<MemoryLease>) {
        (self.artifact, self.matched, self.hits, self.hit_memory)
    }
}

#[derive(Debug, Default)]
struct OwnedTextPreviews {
    rows: Vec<NativeTextPreview>,
    memory: Vec<MemoryLease>,
}

#[derive(Default)]
struct PreviewRead {
    hits: Vec<TextReadHit>,
    previews: OwnedTextPreviews,
    hit_memory: Option<MemoryLease>,
}

struct PreviewSourceCache {
    rows: PreparedRows<PreviewSource>,
    _box_memory: Option<MemoryLease>,
}

impl PreviewSourceCache {
    fn new(count: usize, budget: &WorkBudget) -> Result<Box<Self>, IndexError> {
        let bytes = u64::try_from(std::mem::size_of::<Self>())
            .map_err(|_width| IndexError::ResourceLimited)?;
        let box_memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        let rows = PreparedRows::with_capacity(count, budget)?;
        Ok(Box::new(Self {
            rows,
            _box_memory: box_memory,
        }))
    }

    fn join<T>(
        count: usize,
        budget: &mut WorkBudget,
        join: impl FnOnce(&mut WorkBudget) -> Result<T, IndexError>,
    ) -> Result<(T, Option<Box<Self>>), IndexError> {
        let sources = budget.try_optional_read(|budget| Self::new(count, budget))?;
        if let Some(sources) = sources {
            let (result, remaining) = budget.with_discardable_memory(sources, join);
            Ok((result?, remaining))
        } else {
            Ok((join(budget)?, None))
        }
    }
}

impl PreviewRead {
    fn new(capacity: usize, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let lease_capacity = capacity.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        let hit_bytes = capacity
            .checked_mul(std::mem::size_of::<TextReadHit>())
            .ok_or(IndexError::ResourceLimited)?;
        let lease_bytes = lease_capacity
            .checked_mul(std::mem::size_of::<MemoryLease>())
            .ok_or(IndexError::ResourceLimited)?;
        let preview_bytes = capacity
            .checked_mul(std::mem::size_of::<NativeTextPreview>())
            .and_then(|bytes| bytes.checked_add(lease_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        let hit_bytes = u64::try_from(hit_bytes).map_err(|_width| IndexError::ResourceLimited)?;
        let preview_bytes =
            u64::try_from(preview_bytes).map_err(|_width| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: hit_bytes
                .checked_add(preview_bytes)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let hit_memory = budget.reserve_memory(hit_bytes, MemoryLifetime::Retained)?;
        let preview_memory = budget.reserve_memory(preview_bytes, MemoryLifetime::Retained)?;
        let mut hits = Vec::new();
        let mut rows = Vec::new();
        let mut memory = Vec::new();
        hits.try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        rows.try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        memory
            .try_reserve_exact(lease_capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if hits.capacity() > capacity
            || rows.capacity() > capacity
            || memory.capacity() > lease_capacity
        {
            return Err(IndexError::ResourceLimited);
        }
        memory.extend(preview_memory);
        Ok(Self {
            hits,
            previews: OwnedTextPreviews { rows, memory },
            hit_memory,
        })
    }
}

/// The service owns ranking policy and a declared scheduler reservation.
/// Clients cannot switch models or request unbounded results through this port.
pub struct NativeTextReader {
    index: NativeTextIndex,
    policy: Bm25Policy,
    need: ResourceNeed,
}

struct OverlayPath<'a> {
    path: Cow<'a, str>,
    // Selected durable paths are decoded and retained for the query stage.
    _memory: Option<MemoryLease>,
}

fn retain_hit_source(
    record: SourceRecordOwner,
    path_projection: bool,
    budget: &WorkBudget,
) -> Result<
    (
        String,
        Option<MemoryLease>,
        Option<(RepoId, Option<MemoryLease>)>,
    ),
    IndexError,
> {
    if !path_projection {
        let (path, path_memory) = record.into_retained_path(budget)?;
        return Ok((path, path_memory, None));
    }
    if record.source_repo_id.is_none() {
        return Err(IndexError::CorruptObject(
            "native path hit lacks authenticated source repository".into(),
        ));
    }
    let (record, memory) = record.into_retained_parts(budget)?;
    let repository = record.source_repo_id.ok_or_else(|| {
        IndexError::BackendViolation("native path hit repository disappeared".into())
    })?;
    Ok((record.path, None, Some((repository, memory))))
}

fn reserve_rows<T>(
    count: usize,
    context: &mut QueryReadContext<'_>,
) -> Result<Option<MemoryLease>, IndexError> {
    let bytes = count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = u64::try_from(bytes).map_err(|_overflow| IndexError::ResourceLimited)?;
    if bytes == 0 {
        return Ok(None);
    }
    context.budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })?;
    context
        .budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)
}

impl NativeTextReader {
    pub fn new(
        recipe: TextRecipe,
        policy: Bm25Policy,
        need: ResourceNeed,
    ) -> Result<Self, IndexError> {
        policy.validate()?;
        if need.cpu_units == 0
            || need.io_bytes == 0
            || need.memory_bytes == 0
            || need.provider_units != 0
        {
            return Err(IndexError::InvalidInput(
                "native text read reservation is invalid".into(),
            ));
        }
        Ok(Self {
            index: NativeTextIndex::new(recipe),
            policy,
            need,
        })
    }

    fn source_path(
        &self,
        owner: OwnerKey,
        source: Option<ObjectRef>,
        allow_excluded: bool,
        context: &mut QueryReadContext<'_>,
    ) -> Result<(String, Option<MemoryLease>), IndexError> {
        self.selected_source(
            owner,
            source,
            allow_excluded,
            &mut TextReadContext {
                ticket: context.ticket,
                objects: context.objects,
                maps: context.maps,
                budget: &mut *context.budget,
            },
        )?
        .into_retained_path(context.budget)
    }

    fn source_hit(
        &self,
        owner: OwnerKey,
        source: ObjectRef,
        path_projection: bool,
        context: &mut QueryReadContext<'_>,
    ) -> Result<
        (
            String,
            Option<MemoryLease>,
            Option<(RepoId, Option<MemoryLease>)>,
        ),
        IndexError,
    > {
        let record = self.selected_source(
            owner,
            Some(source),
            path_projection,
            &mut TextReadContext {
                ticket: context.ticket,
                objects: context.objects,
                maps: context.maps,
                budget: &mut *context.budget,
            },
        )?;
        retain_hit_source(record, path_projection, context.budget)
    }

    fn selected_source(
        &self,
        owner: OwnerKey,
        source: Option<ObjectRef>,
        allow_excluded: bool,
        context: &mut TextReadContext<'_>,
    ) -> Result<SourceRecordOwner, IndexError> {
        authenticated_text_source(&self.index, owner, source, allow_excluded, context)
    }

    fn project_previews(
        &self,
        selection: &PreviewSelection<'_>,
        path_projection: bool,
        context: &mut TextReadContext<'_>,
    ) -> Result<PreviewRead, IndexError> {
        let hits = selection.ranked();
        let count = hits.len();
        if count == 0 {
            return Ok(PreviewRead::default());
        }
        let mut selected = PreviewRead::new(count, context.budget)?;
        // Finish the same mandatory joins as ordinary Text before spending
        // any remaining Work on preview bodies.
        let ticket = context.ticket;
        let objects = context.objects;
        let maps = context.maps;
        let join = |budget: &mut WorkBudget| -> Result<(), IndexError> {
            let mut joined = TextReadContext {
                ticket,
                objects,
                maps,
                budget,
            };
            for (hit, prepared) in hits.zip(selection.prepared()) {
                let NativeTextHitSource::Durable(source) = hit.source else {
                    return Err(IndexError::BackendViolation(
                        "native preview selected an overlay source".into(),
                    ));
                };
                let score = hit_score(hit.score)?;
                let record =
                    self.selected_source(hit.owner, Some(source), path_projection, &mut joined)?;
                let locator = PreviewSource::from_record(&record);
                let (path, path_memory, path_group_repository) =
                    retain_hit_source(record, path_projection, joined.budget)?;
                if let Some(result) =
                    joined
                        .budget
                        .with_discardable_memory_mut(|cache: &mut PreviewSourceCache| {
                            cache.rows.push_preallocated(locator)
                        })
                {
                    result?;
                }
                selected.hits.push(TextReadHit {
                    owner: hit.owner,
                    source: hit.source,
                    path,
                    score,
                    path_memory,
                    path_group_repository,
                });
                selected.previews.rows.push(NativeTextPreview {
                    owner: hit.owner,
                    source,
                    evidence: if prepared.is_path_only() {
                        NativeTextPreviewEvidence::Path
                    } else {
                        NativeTextPreviewEvidence::Unavailable(PreviewUnavailableReason::WorkBudget)
                    },
                });
            }
            Ok(())
        };
        if selection.prepared().all(|prepared| prepared.is_path_only()) {
            join(context.budget)?;
            return Ok(selected);
        }
        let ((), sources) = PreviewSourceCache::join(count, context.budget, join)?;
        if let Some(sources) = sources {
            if sources.rows.len() != count {
                return Err(IndexError::BackendViolation(
                    "native preview source cache differs from selected hits".into(),
                ));
            }
            let objects = context.objects;
            for ((prepared, &source), row) in selection
                .prepared()
                .zip(sources.rows.as_slice())
                .zip(&mut selected.previews.rows)
            {
                if prepared.is_path_only() {
                    continue;
                }
                if let Some((evidence, memory)) = context
                    .budget
                    .try_optional_read(|budget| prepared.render(source, objects, budget))?
                {
                    row.evidence = evidence;
                    selected.previews.memory.extend(memory);
                }
            }
        }
        Ok(selected)
    }

    fn copy_overlay_path(
        path: &str,
        context: &mut QueryReadContext<'_>,
    ) -> Result<(String, Option<MemoryLease>), IndexError> {
        if !valid_path(path) {
            return Err(IndexError::InvalidInput(
                "native text overlay path is invalid".into(),
            ));
        }
        let bytes = u64::try_from(path.len()).map_err(|_error| IndexError::ResourceLimited)?;
        context.budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = context
            .budget
            .reserve_memory(bytes, MemoryLifetime::Retained)?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(path.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        owned.push_str(path);
        Ok((owned, memory))
    }

    fn validate_overlay_path_claim(
        path: &str,
        overlay_owner: OwnerKey,
        overlay: &[TextOverlayReadDocument],
        context: &mut QueryReadContext<'_>,
    ) -> Result<(), IndexError> {
        let check_owner = |owner| {
            if owner != overlay_owner
                && overlay
                    .binary_search_by_key(&owner, |row| row.owner)
                    .is_err()
            {
                return Err(IndexError::Conflict);
            }
            Ok(())
        };
        let key_bytes = path
            .len()
            .checked_add(5)
            .ok_or(IndexError::ResourceLimited)?;
        let key_bytes_u64 =
            u64::try_from(key_bytes).map_err(|_error| IndexError::ResourceLimited)?;
        context.budget.charge(WorkUsage {
            bytes: key_bytes_u64,
            ..WorkUsage::default()
        })?;
        let _key_memory = context
            .budget
            .reserve_memory(key_bytes_u64, MemoryLifetime::Temporary)?;
        let mut key = Vec::new();
        key.try_reserve_exact(key_bytes)
            .map_err(|_error| IndexError::ResourceLimited)?;
        key.extend_from_slice(b"path\0");
        key.extend_from_slice(path.as_bytes());
        let Some(marker_ref) =
            context
                .maps
                .lookup(context.ticket.view.source.paths, &key, context.budget)?
        else {
            return Ok(());
        };
        let owner = match SourcePathOwners::decode(marker_ref, context.budget)? {
            SourcePathOwners::LegacyOwner(owner) => owner,
            SourcePathOwners::Repositories(root) => {
                return visit_repository_path_sources(
                    root,
                    path,
                    context.ticket.view.source.owners,
                    context.maps,
                    context.objects,
                    context.budget,
                    &mut |owner, _source, _record, _budget| check_owner(owner),
                );
            }
        };
        let source = context
            .maps
            .lookup_object(
                context.ticket.view.source.owners,
                &owner_map_key(owner),
                context.budget,
            )?
            .ok_or_else(|| {
                IndexError::CorruptObject("native text overlay path marker lacks owner".into())
            })?;
        let stored = context.objects.get(source, context.budget)?;
        let record = SourceRecordOwner::decode_json(
            &stored.payload,
            context.budget,
            MemoryLifetime::Temporary,
        )?;
        if stored.registration.reference != source
            || stored.registration.children.as_slice() != [record.bytes]
            || !valid_path(&record.path)
            || record.path != path
        {
            return Err(IndexError::CorruptObject(
                "native text overlay reverse path differs from selected source".into(),
            ));
        }
        check_owner(owner)
    }
}

impl IndexQueryPort<PlannedTextRead, TextReadResult> for NativeTextReader {
    fn validate_searchable(
        &self,
        owner: OwnerKey,
        source: Option<ObjectRef>,
        context: &mut QueryReadContext<'_>,
    ) -> Result<(), IndexError> {
        self.validate_instance(context.instance)?;
        let member = context.source_member_binding()?;
        self.index.validate_searchable_owner(
            &member,
            owner,
            source,
            TextReadContext {
                ticket: context.ticket,
                objects: context.objects,
                maps: context.maps,
                budget: &mut *context.budget,
            },
        )?;
        context.budget.checkpoint()
    }

    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        self.index.validate_instance(instance)
    }

    fn reservation(&self, request: &PlannedTextRead) -> Result<ResourceNeed, IndexError> {
        if request.top_k == 0 || request.top_k > MAX_NATIVE_TEXT_TOP_K {
            return Err(IndexError::InvalidInput(
                "native text top_k must be 1..=1024".into(),
            ));
        }
        let path_projection = request.plan.filters.surface == ResultSurface::Path;
        if path_projection && (request.rank_after.is_some() || !request.overlay.is_empty()) {
            return Err(IndexError::InvalidInput(
                "native path projection requires durable path ranking".into(),
            ));
        }
        if request.path_rank_after.is_some()
            && (!path_projection
                || request
                    .path_rank_after
                    .as_ref()
                    .is_some_and(|(_, _, path)| !valid_path(path)))
        {
            return Err(IndexError::InvalidInput(
                "native path ranking anchor is invalid".into(),
            ));
        }
        if request.preview
            && (request.rank_after.is_some()
                || request.path_rank_after.is_some()
                || !request.overlay.is_empty())
        {
            return Err(IndexError::InvalidInput(
                "native text preview requires a durable first-page query".into(),
            ));
        }
        Ok(self.need)
    }

    fn query(
        &self,
        request: &PlannedTextRead,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<TextReadResult>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        let bound = BoundTextMember::from_context(context)?;
        let member = bound.member();
        let mut overlay_input_memory = None;
        let mut overlay_path_memory = None;
        let mut overlay_source_memory = None;
        let mut overlay_inputs = Vec::new();
        let mut overlay_paths = Vec::new();
        let mut overlay_sources = Vec::new();
        let overlay_corpus = if request.overlay.is_empty() {
            None
        } else {
            if request.overlay.len() > MAX_TEXT_OVERLAY_DOCUMENTS {
                return Err(IndexError::ResourceLimited);
            }
            let mut previous = None;
            let mut total_bytes = 0usize;
            let mut present = 0usize;
            for document in &request.overlay {
                context.budget.checkpoint()?;
                if previous.is_some_and(|owner| owner >= document.owner) {
                    return Err(IndexError::InvalidInput(
                        "native text overlay owners must be sorted and unique".into(),
                    ));
                }
                previous = Some(document.owner);
                if let Some(path) = document.path.as_deref()
                    && !valid_path(path)
                {
                    return Err(IndexError::InvalidInput(
                        "native text overlay path is invalid".into(),
                    ));
                }
                if let Some(body) = document.text.as_deref() {
                    if body.len() > MAX_TEXT_OVERLAY_DOCUMENT_BYTES {
                        return Err(IndexError::ResourceLimited);
                    }
                    total_bytes = total_bytes
                        .checked_add(body.len())
                        .filter(|bytes| *bytes <= MAX_TEXT_OVERLAY_TOTAL_BYTES)
                        .ok_or(IndexError::ResourceLimited)?;
                    present = present.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
            }
            overlay_input_memory =
                reserve_rows::<TextOverlayInput<'_>>(request.overlay.len(), context)?;
            overlay_path_memory = reserve_rows::<OverlayPath<'_>>(present, context)?;
            overlay_source_memory = reserve_rows::<TextOverlaySource<'_>>(present, context)?;
            overlay_inputs
                .try_reserve_exact(request.overlay.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            overlay_paths
                .try_reserve_exact(present)
                .map_err(|_error| IndexError::ResourceLimited)?;
            overlay_sources
                .try_reserve_exact(present)
                .map_err(|_error| IndexError::ResourceLimited)?;
            for document in &request.overlay {
                overlay_inputs.push(TextOverlayInput {
                    owner: document.owner,
                    revision: document.revision,
                    text: document.text.as_deref(),
                });
                if document.text.is_some() {
                    let (path, memory) = if let Some(path) = document.path.as_deref() {
                        (Cow::Borrowed(path), None)
                    } else {
                        let (path, memory) =
                            self.source_path(document.owner, None, true, context)?;
                        (Cow::Owned(path), memory)
                    };
                    overlay_paths.push(OverlayPath {
                        path,
                        _memory: memory,
                    });
                }
            }
            for (document, path) in request
                .overlay
                .iter()
                .filter(|document| document.text.is_some())
                .zip(&overlay_paths)
            {
                let body = document.text.as_deref().ok_or_else(|| {
                    IndexError::BackendViolation("overlay text disappeared".into())
                })?;
                overlay_sources.push(TextOverlaySource {
                    owner: document.owner,
                    revision: document.revision,
                    path: path.path.as_ref(),
                    body,
                });
            }
            for (position, source) in overlay_sources.iter().enumerate() {
                for prior in overlay_sources.iter().take(position) {
                    let compared = u64::try_from(source.path.len().max(prior.path.len()))
                        .map_err(|_error| IndexError::ResourceLimited)?;
                    context.budget.charge(WorkUsage {
                        bytes: compared,
                        ..WorkUsage::default()
                    })?;
                    if source.path == prior.path {
                        return Err(IndexError::Conflict);
                    }
                }
                Self::validate_overlay_path_claim(
                    source.path,
                    source.owner,
                    &request.overlay,
                    context,
                )?;
            }
            Some(self.index.prepare_overlay_corpus(
                member,
                &overlay_inputs,
                &mut TextReadContext {
                    ticket: context.ticket,
                    objects: context.objects,
                    maps: context.maps,
                    budget: &mut *context.budget,
                },
            )?)
        };
        let mut preview_read = None;
        let mut visit_preview = |selection: &PreviewSelection<'_>,
                                 ctx: &mut TextReadContext<'_>| {
            preview_read = Some(self.project_previews(
                selection,
                request.plan.filters.surface == ResultSurface::Path,
                ctx,
            )?);
            Ok(())
        };
        let preview_visitor: Option<&mut PreviewVisitor<'_>> = if request.preview {
            Some(&mut visit_preview)
        } else {
            None
        };
        let mut ranked = self.index.search_content_with_bound_member(
            &bound,
            &request.plan,
            &request.constraints,
            TextContentExecution {
                top_k: request.top_k,
                policy: self.policy,
                rank_after: request.rank_after,
            },
            request
                .path_rank_after
                .as_ref()
                .map(|(score, repo, path)| (*score, repo, path.as_str())),
            overlay_corpus
                .as_ref()
                .map(|corpus| (corpus, overlay_sources.as_slice())),
            preview_visitor,
            TextReadContext {
                ticket: context.ticket,
                objects: context.objects,
                maps: context.maps,
                budget: &mut *context.budget,
            },
        )?;
        if ranked.view != context.ticket.view.identity || ranked.artifact != member.artifact {
            return Err(IndexError::BackendViolation(
                "native text ranking differs from selected member".into(),
            ));
        }
        if let Some(selected) = preview_read {
            if selected.hits.len() != ranked.hits.len()
                || selected.previews.rows.len() != ranked.hits.len()
            {
                return Err(IndexError::BackendViolation(
                    "native preview result differs from selected ranking".into(),
                ));
            }
            let result = TextReadResult {
                artifact: ranked.artifact,
                matched: ranked.matched,
                remaining: ranked.remaining,
                hits: selected.hits,
                previews: Some(selected.previews),
                hit_memory: selected.hit_memory,
            };
            drop(ranked);
            context.budget.checkpoint()?;
            return QueryOutcome::core_scanned(context.ticket, context.instance, true, result);
        }
        let hit_bytes = ranked
            .hits
            .len()
            .checked_mul(std::mem::size_of::<TextReadHit>())
            .ok_or(IndexError::ResourceLimited)?;
        let hit_memory = if hit_bytes == 0 {
            None
        } else {
            context.budget.reserve_memory(
                u64::try_from(hit_bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?
        };
        let mut result = TextReadResult {
            artifact: ranked.artifact,
            matched: ranked.matched,
            remaining: ranked.remaining,
            hits: Vec::new(),
            previews: None,
            hit_memory,
        };
        result
            .hits
            .try_reserve_exact(ranked.hits.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for hit in ranked.hits.drain(..) {
            let score = hit_score(hit.score)?;
            let (path, path_memory, path_group_repository) = match hit.source {
                NativeTextHitSource::Durable(source) => self.source_hit(
                    hit.owner,
                    source,
                    request.plan.filters.surface == ResultSurface::Path,
                    context,
                )?,
                NativeTextHitSource::Overlay { revision } => {
                    let index = overlay_sources
                        .binary_search_by_key(&hit.owner, |row| row.owner)
                        .map_err(|_error| {
                            IndexError::BackendViolation(
                                "ranked overlay owner lacks captured source".into(),
                            )
                        })?;
                    let source = overlay_sources
                        .get(index)
                        .ok_or(IndexError::ResourceLimited)?;
                    if source.revision != revision {
                        return Err(IndexError::Conflict);
                    }
                    let (path, path_memory) = Self::copy_overlay_path(source.path, context)?;
                    (path, path_memory, None)
                }
            };
            result.hits.push(TextReadHit {
                owner: hit.owner,
                source: hit.source,
                path,
                score,
                path_memory,
                path_group_repository,
            });
        }
        // The drained ranking owns its backing and grant even on mapping failure.
        drop(ranked);
        context.budget.checkpoint()?;
        let outcome = QueryOutcome::core_scanned(context.ticket, context.instance, true, result);
        drop(overlay_corpus);
        drop(overlay_sources);
        drop(overlay_paths);
        drop(overlay_inputs);
        drop(overlay_source_memory);
        drop(overlay_path_memory);
        drop(overlay_input_memory);
        outcome
    }
}

fn hit_score(score: f64) -> Result<FiniteScore, IndexError> {
    FiniteScore::new(score).map_err(|_error| {
        IndexError::CorruptObject("native text ranking contains nonfinite score".into())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        NativeTextReader, PlannedTextRead, PreviewSourceCache, TextReadHit, TextReadResult,
    };
    use crate::{native::TextRecipe, planner::LexicalPlanner};
    use quanta_index_contract::{LqExpr, LqFilter, LqLeaf, LqQuery, LqSelect, LqSpan, RepoId};
    use quanta_index_core::index::{
        Digest, FiniteScore, GroupActivation, IndexError, IndexInstanceId, IndexKind,
        IndexQueryPort, InputDomain, InstanceSpec, MemoryLease, MemoryLeaseOwner, MemoryLifetime,
        MemoryScope, MemoryScopePort, ObjectRef, OwnerKey, ProducerId, PublicationGroupId,
        SchemaId, StableOwnerId, StorageFormatId, WorkBudget, WorkLimits, effort::ResourceNeed,
        ports::IndexBuildPort,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    struct PreviewMemory {
        used: Arc<AtomicU64>,
        limit: u64,
    }

    struct PreviewRelease(Arc<AtomicU64>);

    impl MemoryLeaseOwner for PreviewRelease {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
        }
    }

    impl MemoryScopePort for PreviewMemory {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _previous = self
                .used
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                    used.checked_add(bytes).filter(|next| *next <= self.limit)
                })
                .map_err(|_used| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(
                bytes,
                Arc::new(PreviewRelease(Arc::clone(&self.used))),
            ))
        }
    }

    #[test]
    fn preview_source_cache_releases_funded_backing_for_mandatory_join() -> Result<(), IndexError> {
        let cache_bytes = u64::try_from(
            std::mem::size_of::<PreviewSourceCache>()
                .checked_add(std::mem::size_of::<super::PreviewSource>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_width| IndexError::ResourceLimited)?;
        let mandatory_bytes = 64_u64;
        let limit = cache_bytes.max(mandatory_bytes);
        let used = Arc::new(AtomicU64::new(0));
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1,
                writes: 0,
                bytes: 1,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(PreviewMemory {
            used: Arc::clone(&used),
            limit,
        }))));
        let (mandatory, cache) = PreviewSourceCache::join(1, &mut budget, |budget| {
            assert_eq!(used.load(Ordering::SeqCst), cache_bytes);
            budget.reserve_memory(mandatory_bytes, MemoryLifetime::Retained)
        })?;
        assert!(cache.is_none());
        let mandatory = mandatory.ok_or(IndexError::ResourceLimited)?;
        assert_eq!(used.load(Ordering::SeqCst), mandatory_bytes);
        assert_eq!(budget.first_denial(), None);
        drop(mandatory);
        assert_eq!(used.load(Ordering::SeqCst), 0);
        Ok(())
    }

    fn need() -> ResourceNeed {
        ResourceNeed {
            cpu_units: 1,
            io_bytes: 64 * 1024,
            memory_bytes: 4 * 1024 * 1024,
            provider_units: 0,
        }
    }

    fn recipe() -> TextRecipe {
        TextRecipe::new(Digest([31; 32]))
    }

    fn spec(recipe: TextRecipe) -> InstanceSpec {
        InstanceSpec {
            instance: IndexInstanceId([1; 16]),
            kind: IndexKind([2; 16]),
            group: PublicationGroupId([3; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::SourceLocal,
            schema: SchemaId([4; 16]),
            policy: recipe.output_policy(),
            format: StorageFormatId([5; 16]),
        }
    }

    fn planned(top_k: u32) -> Result<PlannedTextRead, Box<dyn std::error::Error>> {
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword("one".into()));
        Ok(PlannedTextRead::new(
            LexicalPlanner::plan(&query)?,
            top_k,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            |_bytes, _lifetime| Ok(None),
        )?)
    }

    fn planned_path() -> Result<PlannedTextRead, Box<dyn std::error::Error>> {
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword("one".into()));
        query.filters.push(LqFilter::Select {
            dim: LqSelect::Path,
        });
        Ok(PlannedTextRead::new(
            LexicalPlanner::plan(&query)?,
            10,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            |_bytes, _lifetime| Ok(None),
        )?)
    }

    #[test]
    fn path_rank_anchor_requires_path_projection_and_canonical_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = NativeTextReader::new(
            recipe(),
            super::super::Bm25Policy { k1: 1.2, b: 0.75 },
            need(),
        )?;
        let anchor = || {
            Some((
                FiniteScore::new(1.0).expect("finite static score"),
                RepoId::new("repo").expect("canonical static repository"),
                "src/lib.rs".to_owned(),
            ))
        };
        assert!(
            reader
                .reservation(&planned_path()?.with_path_rank_after_v1(anchor(), None))
                .is_ok()
        );
        assert!(
            reader
                .reservation(&planned(10)?.with_path_rank_after_v1(anchor(), None))
                .is_err()
        );
        assert!(
            reader
                .reservation(&planned_path()?.with_path_rank_after_v1(
                    Some((
                        FiniteScore::new(1.0)?,
                        RepoId::new("repo")?,
                        "../escape".to_owned(),
                    )),
                    None,
                ))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn path_hit_repository_and_path_grant_transfer_together() {
        let used = Arc::new(AtomicU64::new(17));
        let repository = RepoId::new("repo").expect("canonical static repository");
        let owner = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([2; 16]),
        };
        let hit = |repository: RepoId| TextReadHit {
            owner,
            source: super::NativeTextHitSource::Durable(ObjectRef(Digest([3; 32]))),
            path: "src/lib.rs".to_owned(),
            score: FiniteScore::new(1.0).expect("finite static score"),
            path_memory: None,
            path_group_repository: Some((
                repository,
                Some(MemoryLease::new(
                    17,
                    Arc::new(PreviewRelease(Arc::clone(&used))),
                )),
            )),
        };
        let mut first = hit(repository.clone());
        assert_eq!(first.path_group_repository_v1(), Some(&repository));
        let (moved, grant) = first
            .take_path_group_repository_v1()
            .expect("path repository");
        assert_eq!(moved, repository);
        assert!(first.path_group_repository_v1().is_none());
        assert!(first.into_parts().4.is_none());
        assert_eq!(used.load(Ordering::SeqCst), 17);
        drop(grant);
        assert_eq!(used.load(Ordering::SeqCst), 0);

        used.store(17, Ordering::SeqCst);
        let (_owner, _source, path, _score, path_grant) = hit(repository).into_parts();
        assert_eq!(path, "src/lib.rs");
        assert_eq!(used.load(Ordering::SeqCst), 17);
        drop(path);
        drop(path_grant);
        assert_eq!(used.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn server_registered_ranking_and_reservation_refuse_invalid_policy_or_capacity()
    -> Result<(), Box<dyn std::error::Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let ranking = super::super::Bm25Policy { k1: 1.2, b: 0.75 };
        let reader = NativeTextReader::new(recipe(), ranking, need())?;
        let port: &dyn IndexQueryPort<PlannedTextRead, TextReadResult> = &reader;
        check_eq!(port.reservation(&planned(1)?)?, need());
        check_eq!(port.reservation(&planned(1024)?)?, need());
        check!(port.reservation(&planned(0)?).is_err());
        check!(port.reservation(&planned(1025)?).is_err());
        check!(
            NativeTextReader::new(
                recipe(),
                super::super::Bm25Policy {
                    k1: f64::NAN,
                    b: 0.75
                },
                need(),
            )
            .is_err()
        );
        check!(
            NativeTextReader::new(
                recipe(),
                ranking,
                ResourceNeed {
                    provider_units: 1,
                    ..need()
                },
            )
            .is_err()
        );
        check!(
            NativeTextReader::new(
                recipe(),
                ranking,
                ResourceNeed {
                    memory_bytes: 0,
                    ..need()
                },
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn read_capability_admits_only_its_frozen_text_instance()
    -> Result<(), Box<dyn std::error::Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!($($message)+)).into()); }
            }};
        }

        let recipe = recipe();
        let reader = NativeTextReader::new(
            recipe,
            super::super::Bm25Policy { k1: 1.2, b: 0.75 },
            need(),
        )?;
        let port: &dyn IndexQueryPort<PlannedTextRead, TextReadResult> = &reader;
        let valid = spec(recipe);
        // Independently computed SHA-256 of the documented recipe bytes binds
        // the directory inventory, inline contribution, occurrence, Small v2
        // and full-key term dictionary layouts.
        check!(
            recipe.output_policy()
                == Digest([
                    125, 28, 0, 251, 234, 97, 215, 237, 2, 80, 243, 225, 226, 211, 160, 117, 109,
                    118, 37, 204, 15, 8, 18, 58, 3, 51, 82, 181, 106, 183, 21, 151
                ])
        );
        let prior_fixed_row_dictionary = InstanceSpec {
            policy: Digest([
                230, 168, 119, 63, 246, 62, 197, 108, 3, 56, 58, 227, 193, 52, 34, 50, 176, 61,
                213, 195, 212, 46, 184, 103, 196, 104, 112, 148, 181, 98, 108, 155,
            ]),
            ..valid
        };
        check!(matches!(
            port.validate_instance(&prior_fixed_row_dictionary),
            Err(IndexError::InvalidInput(_))
        ));
        let prior_term_map_layout = InstanceSpec {
            policy: Digest([
                11, 34, 232, 218, 194, 139, 191, 146, 78, 100, 117, 64, 140, 141, 169, 115, 202,
                243, 45, 220, 179, 43, 208, 223, 133, 230, 168, 17, 185, 192, 175, 197,
            ]),
            ..valid
        };
        check!(matches!(
            port.validate_instance(&prior_term_map_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            crate::native::NativeTextIndex::new(recipe).validate_instance(&prior_term_map_layout),
            Err(IndexError::InvalidInput(_))
        ));
        let prior_small_layout = InstanceSpec {
            policy: Digest([
                136, 124, 165, 228, 60, 19, 159, 52, 121, 219, 144, 171, 182, 45, 138, 119, 129,
                73, 194, 24, 244, 60, 76, 177, 237, 183, 112, 209, 38, 113, 45, 158,
            ]),
            ..valid
        };
        let prior_directory_layout = InstanceSpec {
            policy: Digest([
                106, 9, 87, 172, 113, 228, 220, 33, 99, 101, 112, 241, 85, 191, 130, 177, 167, 55,
                147, 210, 87, 35, 8, 11, 237, 108, 190, 209, 71, 42, 254, 218,
            ]),
            ..valid
        };
        let prior_posting_reference_layout = InstanceSpec {
            policy: Digest([
                20, 198, 213, 49, 209, 48, 78, 164, 215, 224, 199, 81, 144, 50, 36, 228, 164, 219,
                14, 248, 153, 29, 246, 190, 197, 45, 183, 66, 30, 76, 192, 168,
            ]),
            ..valid
        };
        let prior_inventory_layout = InstanceSpec {
            policy: Digest([
                178, 193, 223, 24, 180, 82, 72, 93, 105, 239, 78, 104, 123, 122, 53, 25, 240, 124,
                255, 247, 111, 226, 95, 10, 102, 93, 135, 6, 37, 141, 125, 149,
            ]),
            ..valid
        };
        let prior_block_layout = InstanceSpec {
            policy: Digest([
                238, 241, 88, 98, 162, 10, 241, 27, 249, 231, 23, 55, 71, 223, 46, 21, 88, 151,
                215, 89, 19, 71, 91, 197, 235, 74, 46, 232, 86, 189, 243, 62,
            ]),
            ..valid
        };
        let prior_leaf_layout = InstanceSpec {
            policy: Digest([
                85, 158, 4, 130, 19, 223, 227, 249, 120, 224, 253, 10, 163, 37, 82, 213, 222, 248,
                87, 48, 64, 55, 234, 104, 102, 169, 87, 75, 225, 178, 194, 198,
            ]),
            ..valid
        };
        check!(matches!(
            port.validate_instance(&prior_small_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            crate::native::NativeTextIndex::new(recipe).validate_instance(&prior_small_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            port.validate_instance(&prior_directory_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            crate::native::NativeTextIndex::new(recipe).validate_instance(&prior_directory_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            port.validate_instance(&prior_posting_reference_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            crate::native::NativeTextIndex::new(recipe)
                .validate_instance(&prior_posting_reference_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            port.validate_instance(&prior_inventory_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            crate::native::NativeTextIndex::new(recipe).validate_instance(&prior_inventory_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            port.validate_instance(&prior_block_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            crate::native::NativeTextIndex::new(recipe).validate_instance(&prior_block_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            port.validate_instance(&prior_leaf_layout),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            crate::native::NativeTextIndex::new(recipe).validate_instance(&prior_leaf_layout),
            Err(IndexError::InvalidInput(_))
        ));
        port.validate_instance(&valid)?;
        check!(
            crate::native::NativeTextIndex::new(recipe)
                .validate_instance(&valid)
                .is_ok()
        );
        check!(
            port.validate_instance(&InstanceSpec {
                policy: Digest([99; 32]),
                ..valid
            })
            .is_err()
        );
        check!(
            port.validate_instance(&InstanceSpec {
                activation: GroupActivation::AsyncCoverage,
                ..valid
            })
            .is_err()
        );
        check!(
            port.validate_instance(&InstanceSpec {
                input_domain: InputDomain::Projection {
                    projection: quanta_index_core::index::ProjectionId([7; 16]),
                    producer: quanta_index_core::index::ProducerId([8; 16]),
                },
                ..valid
            })
            .is_err()
        );
        Ok(())
    }
}
