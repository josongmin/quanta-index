//! Bounded rank selection over one pinned, producer-certified graph member.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use quanta_index_contract::{
    RepoMapDocType, RepoMapNodeRef,
    native_protocol::{NativeRepoMapHit, NativeRepoMapQuery, native_repomap_rank_policy},
};
use quanta_index_core::index::{
    IndexError, IndexQueryPort, MapPageCursor, MapPageEntry, MapRoot, MemoryLease, MemoryLifetime,
    QueryOutcome, QueryReadContext, WorkBudget,
    effort::ResourceNeed,
    ports::{ImmutableObjectPort, PersistentMapPort},
};
use quanta_index_lq_text_normalizer::{CaseMode, case_chars};

use crate::query::ranking::{RankCandidate, RankPlan, RankScanner, select, tokenize};

use super::{
    NativeGraph,
    affected::{MAX_KEY, node_key},
    contribution::{
        GraphElement, LoadedGraphSupport, LoadedRankProjection, load_rank_projection, load_support,
    },
    query::NativeGraphReader,
    read_root_for_query,
};

const PAGE: u32 = 128;
const TOKEN_HINT: u32 = 64;
const NODE_KEY_PREFIX: &[u8] = b"node\0";

/// The result owns copied hits and their grants through wire conversion.
/// All data fields precede their retained allocation custody.
pub struct NativeRepoMapRankPage {
    hits: Vec<NativeRepoMapHit>,
    pub dropped_entries_count: u32,
    drop_reason_codes: Vec<String>,
    degraded_reason_codes: Vec<String>,
    _selection_memory: Option<MemoryLease>,
    _hits_memory: Option<MemoryLease>,
}

impl NativeRepoMapRankPage {
    /// Borrow results while their allocation custody remains in the page.
    #[must_use]
    pub fn hits(&self) -> &[NativeRepoMapHit] {
        &self.hits
    }

    #[must_use]
    pub fn drop_reason_codes(&self) -> &[String] {
        &self.drop_reason_codes
    }

    #[must_use]
    pub fn degraded_reason_codes(&self) -> &[String] {
        &self.degraded_reason_codes
    }
}

/// Query-only policy gate. Adjacency keeps its existing instance compatibility.
pub struct NativeRepoMapRankReader {
    need: ResourceNeed,
}

impl NativeRepoMapRankReader {
    pub fn new(need: ResourceNeed) -> Result<Self, IndexError> {
        if need.cpu_units == 0
            || need.io_bytes == 0
            || need.memory_bytes == 0
            || need.provider_units != 0
        {
            return Err(IndexError::InvalidInput(
                "native RepoMap rank reservation is invalid".into(),
            ));
        }
        Ok(Self { need })
    }
}

impl IndexQueryPort<NativeRepoMapQuery, NativeRepoMapRankPage> for NativeRepoMapRankReader {
    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        quanta_index_core::index::ports::IndexProjectionBuildPort::validate_instance(
            &NativeGraph,
            instance,
        )?;
        if instance.policy != native_repomap_rank_policy() {
            return Err(IndexError::InvalidInput(
                "native RepoMap rank policy differs".into(),
            ));
        }
        Ok(())
    }

    fn reservation(&self, request: &NativeRepoMapQuery) -> Result<ResourceNeed, IndexError> {
        validate_request(request)?;
        Ok(self.need)
    }

    fn query(
        &self,
        request: &NativeRepoMapQuery,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<NativeRepoMapRankPage>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        context.budget.checkpoint()?;
        let member = NativeGraphReader::pinned_member(context)?;
        let root = read_root_for_query(&member, context.objects, context.budget)?;
        if root.projection.producer_policy != native_repomap_rank_policy() {
            return Err(IndexError::InvalidInput(
                "native RepoMap producer rank policy differs".into(),
            ));
        }
        let page = query_rank(
            root.nodes,
            request,
            context.maps,
            context.objects,
            context.budget,
        )?;
        QueryOutcome::core_scanned(context.ticket, context.instance, true, page)
    }
}

fn validate_request(request: &NativeRepoMapQuery) -> Result<(), IndexError> {
    if !request.valid_native_rank_query() {
        return Err(IndexError::InvalidInput(
            "native RepoMap rank query outside service policy".into(),
        ));
    }
    Ok(())
}

fn query_rank(
    nodes: MapRoot,
    request: &NativeRepoMapQuery,
    maps: &dyn PersistentMapPort,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<NativeRepoMapRankPage, IndexError> {
    // Token and focus containers are admitted before the normalizer or tree
    // allocates. The text and focus count are fixed by the wire contract.
    let plan_bytes = request
        .query_text
        .len()
        .checked_mul(64)
        .and_then(|bytes| {
            request
                .focus_subjects
                .len()
                .checked_mul(256)
                .and_then(|focus| bytes.checked_add(focus))
        })
        .and_then(|bytes| bytes.checked_add(4096))
        .ok_or(IndexError::ResourceLimited)?;
    let _plan_memory = budget.reserve_memory(
        u64::try_from(plan_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let query_terms = tokenize(&request.query_text);
    let focus_keys: BTreeSet<(&str, RepoMapDocType)> = request
        .focus_subjects
        .iter()
        .map(|focus| (focus.subject_identity.as_str(), focus.subject_doc_type))
        .collect();

    // Resolve every focus from the same selected node root. A missing focus is
    // a refusal, never permission to broaden to the entire graph.
    let mut focus_paths = Vec::<FundedPath>::new();
    if !focus_keys.is_empty() {
        focus_paths
            .try_reserve_exact(focus_keys.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut focus_lookup = Vec::new();
        focus_lookup
            .try_reserve_exact(focus_keys.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        focus_lookup.extend(focus_keys.iter().copied());
        let mut resolved = BTreeSet::<usize>::new();
        let mut scanner = GraphRankScanner::new(nodes, maps, objects, budget, None);
        scanner.scan_rows(&mut |candidate, budget| {
            let position = focus_lookup.binary_search_by(|&(identity, doc_type)| {
                identity
                    .cmp(candidate.identity())
                    .then(doc_type.cmp(&candidate.doc_type()))
            });
            if let Ok(position) = position
                && resolved.insert(position)
            {
                focus_paths.push(FundedPath::copy(candidate.owner_path(), budget)?);
            }
            Ok(())
        })?;
        if resolved.len() != focus_keys.len() {
            return Err(IndexError::FocusSubjectNotFound);
        }
    }
    let focus_owner_paths: BTreeSet<&str> =
        focus_paths.iter().map(|path| path.path.as_str()).collect();
    let top_k_limit =
        usize::try_from(request.top_k).map_err(|_error| IndexError::ResourceLimited)?;
    let plan = RankPlan {
        focus_keys: &focus_keys,
        focus_owner_paths: &focus_owner_paths,
        query_terms: &query_terms,
        top_k_limit,
        initial_prefix: top_k_limit,
        token_budget: request.token_budget,
    };
    let mut scanner = GraphRankScanner::new(
        nodes,
        maps,
        objects,
        budget,
        (!focus_keys.is_empty()).then_some((&focus_keys, &focus_owner_paths)),
    );
    let mut selection = select(&mut scanner, &plan)?;
    let hit_bytes = selection.included.iter().try_fold(
        selection
            .included
            .len()
            .checked_mul(std::mem::size_of::<NativeRepoMapHit>())
            .ok_or(IndexError::ResourceLimited)?,
        |total, (candidate, _rank)| {
            total
                .checked_add(candidate.identity().len())
                .and_then(|bytes| bytes.checked_add(candidate.owner_path().len()))
                .ok_or(IndexError::ResourceLimited)
        },
    )?;
    let hits_memory = if hit_bytes == 0 {
        None
    } else {
        budget.reserve_memory(
            u64::try_from(hit_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?
    };
    let mut hits = Vec::new();
    hits.try_reserve_exact(selection.included.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    if hits.capacity() > selection.included.len() {
        return Err(IndexError::ResourceLimited);
    }
    for (candidate, rank) in &selection.included {
        budget.checkpoint()?;
        hits.push(NativeRepoMapHit {
            subject: copy_subject(&candidate.row.entry.subject)?,
            owner_path: quanta_index_contract::RepoRelativePath::new(copy_string(
                candidate.owner_path(),
            )?),
            final_score_millis: candidate.final_score_millis(),
            rank: *rank,
        });
    }
    let dropped_entries_count = selection.dropped_entries_count;
    let drop_reason_codes = std::mem::take(&mut selection.drop_reason_codes);
    let degraded_reason_codes = std::mem::take(&mut selection.degraded_reason_codes);
    let page = NativeRepoMapRankPage {
        hits,
        dropped_entries_count,
        drop_reason_codes,
        degraded_reason_codes,
        _selection_memory: selection.grant.take(),
        _hits_memory: hits_memory,
    };
    // Copied hits own separate grants. Release preview/folded source rows
    // before a slow wire consumer retains the page; only diagnostics still
    // depend on the selector's allocation grant.
    drop(selection);
    Ok(page)
}

struct FundedPath {
    path: String,
    _memory: Option<MemoryLease>,
}

impl FundedPath {
    fn copy(path: &str, budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(
            u64::try_from(path.len()).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        Ok(Self {
            path: copy_string(path)?,
            _memory: memory,
        })
    }
}

struct NativeCandidate {
    row: LoadedRankProjection,
    folded: String,
    score_millis: u32,
    ordinal: usize,
    _fold_memory: Option<MemoryLease>,
}

impl RankCandidate for NativeCandidate {
    fn identity(&self) -> &str {
        match &self.row.entry.subject {
            RepoMapNodeRef::File(id) => id.as_str(),
            RepoMapNodeRef::Module(id) => id.as_str(),
            RepoMapNodeRef::Symbol(id) => id.as_str(),
            RepoMapNodeRef::Chunk(id) => id.as_str(),
        }
    }

    fn doc_type(&self) -> RepoMapDocType {
        self.row.entry.doc_type
    }

    fn owner_path(&self) -> &str {
        self.row.entry.owner_path.as_str()
    }

    fn folded_search_text(&self) -> &str {
        &self.folded
    }

    fn final_score_millis(&self) -> u32 {
        self.score_millis
    }

    fn token_budget_hint(&self) -> u32 {
        TOKEN_HINT
    }

    fn source_ordinal(&self) -> usize {
        self.ordinal
    }
}

type FocusScope<'a> = (
    &'a BTreeSet<(&'a str, RepoMapDocType)>,
    &'a BTreeSet<&'a str>,
);

struct GraphRankScanner<'a, 'f> {
    root: MapRoot,
    maps: &'a dyn PersistentMapPort,
    objects: &'a dyn ImmutableObjectPort,
    budget: &'a mut WorkBudget,
    focus: Option<FocusScope<'f>>,
}

impl<'a, 'f> GraphRankScanner<'a, 'f> {
    fn new(
        root: MapRoot,
        maps: &'a dyn PersistentMapPort,
        objects: &'a dyn ImmutableObjectPort,
        budget: &'a mut WorkBudget,
        focus: Option<FocusScope<'f>>,
    ) -> Self {
        Self {
            root,
            maps,
            objects,
            budget,
            focus,
        }
    }

    fn scan_rows(
        &mut self,
        sink: &mut dyn FnMut(NativeCandidate, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        let key_memory = self.budget.reserve_memory(
            u64::try_from(MAX_KEY.checked_mul(2).ok_or(IndexError::ResourceLimited)?)
                .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let _key_memory = key_memory;
        let mut cursor = NodeCursor::new(self.root, self.maps);
        let mut pending: Option<PendingNode> = None;
        let mut ordinal = 0_usize;
        while let Some(entry) = cursor.next(self.budget)? {
            let (key, reference) = entry.object_parts()?;
            let loaded = load_support(*reference, self.objects, self.budget)?;
            let support = &loaded.support;
            let GraphElement::Node(node) = &support.element else {
                return Err(IndexError::CorruptObject(
                    "rank node index contains edge".into(),
                ));
            };
            if key.as_slice() != node_key(&node.identity, support.owner)? {
                return Err(IndexError::CorruptObject("rank node key differs".into()));
            }
            if pending
                .as_ref()
                .is_some_and(|prior| prior.identity() != &node.identity)
            {
                emit_pending(pending.take(), &mut ordinal, sink, self.budget)?;
            }
            if let Some(prior) = &pending {
                let GraphElement::Node(prior_node) = &prior.support.support.element else {
                    return Err(IndexError::CorruptObject("rank pending edge".into()));
                };
                if prior_node != node {
                    return Err(IndexError::CorruptObject(
                        "rank node supporters disagree".into(),
                    ));
                }
                if prior.supporters
                    >= quanta_index_contract::native_protocol::MAX_NATIVE_GRAPH_ITEMS
                {
                    return Err(IndexError::ResourceLimited);
                }
            }
            if pending.is_none()
                && super::affected::has_variant_conflict(
                    self.root,
                    &node.identity,
                    self.maps,
                    self.budget,
                )?
            {
                return Err(IndexError::CorruptObject(
                    "rank graph raw identity has conflicting typed variants".into(),
                ));
            }
            let candidate = match (&node.identity, support.projection) {
                (RepoMapNodeRef::File(_) | RepoMapNodeRef::Symbol(_), Some(row_ref)) => {
                    let row = load_rank_projection(row_ref, self.objects, self.budget)?;
                    if row.entry.subject != node.identity || !row.entry.valid_native_rank_row() {
                        return Err(IndexError::CorruptObject(
                            "rank projection differs from node".into(),
                        ));
                    }
                    if let Some(prior) = &pending {
                        if prior
                            .candidate
                            .as_ref()
                            .is_none_or(|prior| prior.row.entry != row.entry)
                        {
                            return Err(IndexError::CorruptObject(
                                "rank supporters disagree".into(),
                            ));
                        }
                        None
                    } else {
                        let folded_len = case_chars(&row.entry.search_text, CaseMode::Folded)
                            .try_fold(0_usize, |length, scalar| {
                                length.checked_add(scalar.len_utf8())
                            })
                            .ok_or(IndexError::ResourceLimited)?;
                        let fold_memory = if folded_len == 0 {
                            None
                        } else {
                            self.budget.reserve_memory(
                                u64::try_from(folded_len)
                                    .map_err(|_error| IndexError::ResourceLimited)?,
                                MemoryLifetime::Retained,
                            )?
                        };
                        let mut folded = String::new();
                        folded
                            .try_reserve_exact(folded_len)
                            .map_err(|_error| IndexError::ResourceLimited)?;
                        if folded.capacity() > folded_len {
                            return Err(IndexError::ResourceLimited);
                        }
                        folded.extend(case_chars(&row.entry.search_text, CaseMode::Folded));
                        if folded.len() != folded_len {
                            return Err(IndexError::CorruptObject(
                                "rank fold length changed".into(),
                            ));
                        }
                        let score_millis =
                            u32::try_from(row.entry.final_score_millis).map_err(|_error| {
                                IndexError::CorruptObject("negative rank score".into())
                            })?;
                        Some(NativeCandidate {
                            row,
                            folded,
                            score_millis,
                            ordinal,
                            _fold_memory: fold_memory,
                        })
                    }
                }
                (RepoMapNodeRef::File(_) | RepoMapNodeRef::Symbol(_), None) => {
                    return Err(IndexError::CorruptObject(
                        "rank row absent for searchable node".into(),
                    ));
                }
                (RepoMapNodeRef::Module(_) | RepoMapNodeRef::Chunk(_), None) => None,
                (RepoMapNodeRef::Module(_) | RepoMapNodeRef::Chunk(_), Some(_)) => {
                    return Err(IndexError::CorruptObject(
                        "rank row on non-searchable node".into(),
                    ));
                }
            };
            if pending.is_none() {
                pending = Some(PendingNode {
                    support: loaded,
                    candidate,
                    supporters: 1,
                });
            } else if let Some(prior) = &mut pending {
                prior.supporters = prior
                    .supporters
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        emit_pending(pending, &mut ordinal, sink, self.budget)
    }
}

impl RankScanner for GraphRankScanner<'_, '_> {
    type Candidate = NativeCandidate;
    type Error = IndexError;
    type Grant = Option<MemoryLease>;

    fn resource_limited() -> Self::Error {
        IndexError::ResourceLimited
    }

    fn reserve_prefix(&mut self, bytes: usize) -> Result<Self::Grant, Self::Error> {
        self.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )
    }

    fn scan(
        &mut self,
        sink: &mut dyn FnMut(Self::Candidate) -> Result<(), Self::Error>,
    ) -> Result<(), Self::Error> {
        let focus = self.focus;
        self.scan_rows(&mut |candidate, _budget| {
            if focus.is_none_or(|(keys, paths)| {
                keys.contains(&(candidate.identity(), candidate.doc_type()))
                    || paths.contains(candidate.owner_path())
            }) {
                sink(candidate)
            } else {
                Ok(())
            }
        })
    }
}

struct PendingNode {
    support: LoadedGraphSupport,
    candidate: Option<NativeCandidate>,
    supporters: usize,
}

impl PendingNode {
    fn identity(&self) -> &RepoMapNodeRef {
        match &self.support.support.element {
            GraphElement::Node(node) => &node.identity,
            GraphElement::Edge(edge) => &edge.source,
        }
    }
}

fn emit_pending(
    pending: Option<PendingNode>,
    ordinal: &mut usize,
    sink: &mut dyn FnMut(NativeCandidate, &mut WorkBudget) -> Result<(), IndexError>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if let Some(PendingNode {
        candidate: Some(candidate),
        ..
    }) = pending
    {
        sink(candidate, budget)?;
        *ordinal = ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    Ok(())
}

struct NodeCursor<'a> {
    root: MapRoot,
    maps: &'a dyn PersistentMapPort,
    page: MapPageCursor,
    after: Option<MapPageEntry>,
    more: bool,
}

impl<'a> NodeCursor<'a> {
    fn new(root: MapRoot, maps: &'a dyn PersistentMapPort) -> Self {
        Self {
            root,
            maps,
            page: MapPageCursor::default(),
            after: None,
            more: true,
        }
    }

    fn next(&mut self, budget: &mut WorkBudget) -> Result<Option<MapPageEntry>, IndexError> {
        loop {
            budget.checkpoint()?;
            if self.page.is_empty() {
                if !self.more {
                    return Ok(None);
                }
                let after = self
                    .after
                    .as_ref()
                    .map(MapPageEntry::key)
                    .transpose()?
                    .unwrap_or(NODE_KEY_PREFIX);
                let page = self.maps.scan(self.root, Some(after), PAGE, budget)?;
                if page.entries.is_empty() && page.has_more {
                    return Err(IndexError::BackendViolation(
                        "rank node page made no progress".into(),
                    ));
                }
                self.more = page.has_more;
                self.page = page.cursor();
            }
            let Some(entry) = self.page.next() else {
                continue;
            };
            let key = entry.key()?;
            if self
                .after
                .as_ref()
                .map(MapPageEntry::key)
                .transpose()?
                .is_some_and(|prior| prior >= key)
            {
                return Err(IndexError::CorruptObject("rank node page order".into()));
            }
            if !key.starts_with(NODE_KEY_PREFIX) {
                return Ok(None);
            }
            self.after = Some(entry.clone());
            return Ok(Some(entry));
        }
    }
}

fn copy_subject(subject: &RepoMapNodeRef) -> Result<RepoMapNodeRef, IndexError> {
    Ok(match subject {
        RepoMapNodeRef::File(id) => RepoMapNodeRef::File(quanta_index_contract::FileId::new(
            copy_string(id.as_str())?,
        )),
        RepoMapNodeRef::Module(id) => RepoMapNodeRef::Module(
            quanta_index_contract::RepoMapModuleId::new(copy_string(id.as_str())?),
        ),
        RepoMapNodeRef::Symbol(id) => RepoMapNodeRef::Symbol(quanta_index_contract::SymbolId::new(
            copy_string(id.as_str())?,
        )),
        RepoMapNodeRef::Chunk(id) => RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new(
            copy_string(id.as_str())?,
        )),
    })
}

fn copy_string(source: &str) -> Result<String, IndexError> {
    let mut copied = String::new();
    copied
        .try_reserve_exact(source.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    if copied.capacity() > source.len() {
        return Err(IndexError::ResourceLimited);
    }
    copied.push_str(source);
    Ok(copied)
}
