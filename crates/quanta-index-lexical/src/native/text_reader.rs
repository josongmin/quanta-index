//! Ticket-bound query adapter for the native text member. The planner IR and
//! BM25 execution remain owned by their existing lexical modules.

use std::{borrow::Cow, sync::Arc};

use super::{Bm25Policy, NativeTextIndex, TextReadContext, TextRecipe};
use super::{
    text_overlay::{
        MAX_TEXT_OVERLAY_DOCUMENT_BYTES, MAX_TEXT_OVERLAY_DOCUMENTS, MAX_TEXT_OVERLAY_TOTAL_BYTES,
        TextOverlayInput,
    },
    text_query::{NativeTextHitSource, TextContentExecution, TextOverlaySource},
};
use crate::plan::LexicalPlan;
use quanta_index_core::index::{
    FiniteScore, IndexError, IndexQueryPort, MAX_SOURCE_PATH_BYTES, MemberBinding, MemoryLease,
    MemoryLifetime, ObjectRef, OwnerKey, QueryOutcome, QueryReadContext, SourceRecordOwner,
    WorkUsage, effort::ResourceNeed, member_map_key, owner_map_key, ports::IndexBuildPort,
};

use quanta_index_contract::native_protocol::MAX_NATIVE_TEXT_TOP_K;

/// A planned request retains the canonical lexical plan and a bounded result cap.
/// Lowering from the public text-query request belongs to the calling runtime.
#[derive(Debug)]
pub struct PlannedTextRead {
    pub plan: LexicalPlan,
    pub top_k: u32,
    constraints: quanta_index_contract::QueryConstraintSetV1,
    // Owned path backing drops before its retained grant. Deep Clone is unavailable.
    _constraint_memory: Option<MemoryLease>,
    plan_memory: [Option<MemoryLease>; 3],
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
            constraints: constraints.clone(),
            _constraint_memory: memory,
            plan_memory: [None, None, None],
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
}

impl PartialEq for TextReadHit {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.source == other.source
            && self.path == other.path
            && self.score == other.score
    }
}

impl TextReadHit {
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
        (
            self.owner,
            self.source,
            self.path,
            self.score,
            self.path_memory,
        )
    }
}

#[derive(Debug)]
pub struct TextReadResult {
    pub artifact: ObjectRef,
    pub matched: u64,
    pub hits: Vec<TextReadHit>,
    // The hit container dies before this retained capacity charge.
    hit_memory: Option<MemoryLease>,
}

impl PartialEq for TextReadResult {
    fn eq(&self, other: &Self) -> bool {
        self.artifact == other.artifact && self.matched == other.matched && self.hits == other.hits
    }
}

impl TextReadResult {
    /// Data and custody move together; mapping must retain both until conversion.
    #[must_use]
    pub fn into_parts(self) -> (ObjectRef, u64, Vec<TextReadHit>, Option<MemoryLease>) {
        (self.artifact, self.matched, self.hits, self.hit_memory)
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

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_SOURCE_PATH_BYTES
        && !path.starts_with('/')
        && !path.contains('\0')
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
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

    fn pinned_member(
        &self,
        context: &mut QueryReadContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        let key = member_map_key(context.instance.instance);
        let reference = context
            .maps
            .lookup_object(context.ticket.view.members, &key, context.budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let stored = context.objects.get(reference, context.budget)?;
        let member =
            quanta_index_core::index::decode_member_binding(&stored.payload, context.budget)?;
        let children = [Some(member.artifact), member.input.root.0];
        if stored.registration.reference != reference
            || !stored
                .registration
                .children
                .iter()
                .copied()
                .eq(children.into_iter().flatten())
            || member.instance != context.instance.instance
            || member.format != context.instance.format
            || member.input.schema != context.instance.schema
            || member.input.policy != context.instance.policy
            || member.input.root != context.ticket.view.source.owners
        {
            return Err(IndexError::CorruptObject(
                "native text member differs from selected ticket or instance".into(),
            ));
        }
        Ok(member)
    }

    fn source_path(
        &self,
        owner: OwnerKey,
        source: Option<ObjectRef>,
        allow_excluded: bool,
        context: &mut QueryReadContext<'_>,
    ) -> Result<(String, Option<MemoryLease>), IndexError> {
        let pinned = context.maps.lookup_object(
            context.ticket.view.source.owners,
            &owner_map_key(owner),
            context.budget,
        )?;
        let source = match (pinned, source) {
            (Some(actual), Some(expected)) if actual == expected => actual,
            (Some(actual), None) => actual,
            (None, None) => {
                return Err(IndexError::InvalidInput(
                    "new overlay owner requires an explicit path".into(),
                ));
            }
            _ => {
                return Err(IndexError::CorruptObject(
                    "native text hit differs from pinned source owner".into(),
                ));
            }
        };
        let stored = context.objects.get(source, context.budget)?;
        let record = SourceRecordOwner::decode_json(
            &stored.payload,
            context.budget,
            MemoryLifetime::Temporary,
        )?;
        if stored.registration.reference != source
            || stored.registration.children.as_slice() != [record.bytes]
            || !valid_path(&record.path)
            || (!self.index.validate_selected_source(&record)? && !allow_excluded)
        {
            return Err(IndexError::CorruptObject(
                "native text hit source record or admission differs".into(),
            ));
        }
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(record.path.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let key_bytes = record
            .path
            .len()
            .checked_add(5)
            .ok_or(IndexError::ResourceLimited)?;
        let _key_memory = context.budget.reserve_memory(
            u64::try_from(key_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut path_key = Vec::new();
        path_key
            .try_reserve_exact(key_bytes)
            .map_err(|_error| IndexError::ResourceLimited)?;
        path_key.extend_from_slice(b"path\0");
        path_key.extend_from_slice(record.path.as_bytes());
        let marker_ref = context
            .maps
            .lookup(context.ticket.view.source.paths, &path_key, context.budget)?
            .ok_or_else(|| {
                IndexError::CorruptObject("native text source path lacks reverse owner".into())
            })?;
        let path_owner =
            quanta_index_core::index::owner_from_path_map_value(marker_ref, context.budget)?;
        if path_owner != owner {
            return Err(IndexError::CorruptObject(
                "native text source path owner differs from selected owner".into(),
            ));
        }
        context.budget.checkpoint()?;
        // The canonical owner transfers its path and retained grant together,
        // while the original decode custody is still live. No path is cloned.
        let (record, memory) = record.into_retained_parts(context.budget)?;
        Ok((record.path, memory))
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

    fn selected_path_owner(
        path: &str,
        context: &mut QueryReadContext<'_>,
    ) -> Result<Option<OwnerKey>, IndexError> {
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
            return Ok(None);
        };
        let owner =
            quanta_index_core::index::owner_from_path_map_value(marker_ref, context.budget)?;
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
        Ok(Some(owner))
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
        let member = self.pinned_member(context)?;
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
        Ok(self.need)
    }

    fn query(
        &self,
        request: &PlannedTextRead,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<TextReadResult>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        let member = self.pinned_member(context)?;
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
                if let Some(selected_owner) = Self::selected_path_owner(source.path, context)? {
                    let masked = request
                        .overlay
                        .binary_search_by_key(&selected_owner, |row| row.owner)
                        .is_ok();
                    if selected_owner != source.owner && !masked {
                        return Err(IndexError::Conflict);
                    }
                }
            }
            Some(self.index.prepare_overlay_corpus(
                &member,
                &overlay_inputs,
                &mut TextReadContext {
                    ticket: context.ticket,
                    objects: context.objects,
                    maps: context.maps,
                    budget: &mut *context.budget,
                },
            )?)
        };
        let mut ranked = self.index.search_content_with_overlay(
            &member,
            &request.plan,
            &request.constraints,
            TextContentExecution {
                top_k: request.top_k,
                policy: self.policy,
            },
            overlay_corpus
                .as_ref()
                .map(|corpus| (corpus, overlay_sources.as_slice())),
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
            hits: Vec::new(),
            hit_memory,
        };
        result
            .hits
            .try_reserve_exact(ranked.hits.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for hit in ranked.hits.drain(..) {
            let score = FiniteScore::new(hit.score).map_err(|_error| {
                IndexError::CorruptObject("native text ranking contains nonfinite score".into())
            })?;
            let (path, path_memory) = match hit.source {
                NativeTextHitSource::Durable(source) => {
                    self.source_path(hit.owner, Some(source), false, context)?
                }
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
                    Self::copy_overlay_path(source.path, context)?
                }
            };
            result.hits.push(TextReadHit {
                owner: hit.owner,
                source: hit.source,
                path,
                score,
                path_memory,
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

#[cfg(test)]
mod tests {
    use super::{NativeTextReader, PlannedTextRead, TextReadResult};
    use crate::{native::TextRecipe, planner::LexicalPlanner};
    use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan};
    use quanta_index_core::index::{
        Digest, GroupActivation, IndexError, IndexInstanceId, IndexKind, IndexQueryPort,
        InputDomain, InstanceSpec, PublicationGroupId, SchemaId, StorageFormatId,
        effort::ResourceNeed, ports::IndexBuildPort,
    };

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
