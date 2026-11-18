//! Native rank query over the imported exact Legacy compiler projection.

use std::collections::BTreeSet;

use quanta_index_contract::{
    ChunkId, FileId, RepoMapDocType, RepoMapModuleId, RepoMapNodeRef, RepoRelativePath, SymbolId,
    native_protocol::{NativeRepoMapHit, NativeRepoMapQuery},
};
use quanta_index_core::index::{
    IndexError, IndexQueryPort, MemoryLease, MemoryLifetime, QueryOutcome, QueryReadContext,
    WorkBudget, effort::ResourceNeed, ports::PersistentMapPort,
};
use quanta_index_lq_text_normalizer::{CaseMode, case_chars};

use crate::{
    native::{NativeRepoMapRankPage, graph::NativeGraphReader},
    query::ranking::{RankCandidate, RankPlan, RankScanner, select, tokenize},
};

use super::{LoadedLegacyRepoMapRowV1, NativeLegacyRepoMapBuildV1, NativeLegacyRepoMapReadV1};

const TOKEN_HINT: u32 = 64;

pub struct NativeLegacyRepoMapRankReaderV1 {
    need: ResourceNeed,
}

impl NativeLegacyRepoMapRankReaderV1 {
    pub fn new(need: ResourceNeed) -> Result<Self, IndexError> {
        if need.cpu_units == 0
            || need.io_bytes == 0
            || need.memory_bytes == 0
            || need.provider_units != 0
        {
            return Err(IndexError::InvalidInput(
                "Legacy RepoMap rank reservation differs".into(),
            ));
        }
        Ok(Self { need })
    }
}

impl IndexQueryPort<NativeRepoMapQuery, NativeRepoMapRankPage> for NativeLegacyRepoMapRankReaderV1 {
    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        NativeLegacyRepoMapBuildV1::validate_profile(instance)
    }

    fn reservation(&self, request: &NativeRepoMapQuery) -> Result<ResourceNeed, IndexError> {
        if !request.valid_native_rank_query() {
            return Err(IndexError::InvalidInput(
                "Legacy RepoMap rank request differs".into(),
            ));
        }
        Ok(self.need)
    }

    fn query(
        &self,
        request: &NativeRepoMapQuery,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<NativeRepoMapRankPage>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        let member = NativeGraphReader::pinned_member(context)?;
        let read = NativeLegacyRepoMapReadV1::open(
            &member,
            context.ticket.view.source,
            context.objects,
            context.budget,
        )?;
        let page = query_rank(&read, request, context.maps, context.budget)?;
        QueryOutcome::core_scanned(context.ticket, context.instance, true, page)
    }
}

struct Candidate {
    row: LoadedLegacyRepoMapRowV1<quanta_index_contract::CompiledRepoMapProjectionEntryV1>,
    folded: String,
    score_millis: u32,
    ordinal: usize,
    _fold_memory: Option<MemoryLease>,
}

impl RankCandidate for Candidate {
    fn identity(&self) -> &str {
        subject_identity(&self.row.value.subject)
    }
    fn doc_type(&self) -> RepoMapDocType {
        self.row.value.doc_type
    }
    fn owner_path(&self) -> &str {
        self.row.value.owner_path.as_str()
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

fn subject_identity(subject: &RepoMapNodeRef) -> &str {
    match subject {
        RepoMapNodeRef::File(id) => id.as_str(),
        RepoMapNodeRef::Module(id) => id.as_str(),
        RepoMapNodeRef::Symbol(id) => id.as_str(),
        RepoMapNodeRef::Chunk(id) => id.as_str(),
    }
}

struct Scanner<'a, 'f> {
    read: &'a NativeLegacyRepoMapReadV1,
    maps: &'a dyn PersistentMapPort,
    budget: &'a mut WorkBudget,
    focus: Option<(
        &'f BTreeSet<(&'f str, RepoMapDocType)>,
        &'f BTreeSet<&'f str>,
    )>,
}

impl Scanner<'_, '_> {
    fn scan_rows(
        &mut self,
        sink: &mut dyn FnMut(Candidate) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        let mut ordinal = 0_usize;
        self.read
            .visit_rank_rows(self.maps, self.budget, |row, budget| {
                if !row.value.valid_native_rank_row() {
                    return Err(IndexError::CorruptObject(
                        "Legacy RepoMap rank row differs".into(),
                    ));
                }
                let score_millis = u32::try_from(row.value.final_score_millis).map_err(|_| {
                    IndexError::CorruptObject("Legacy RepoMap rank score negative".into())
                })?;
                let folded_len = case_chars(&row.value.search_text, CaseMode::Folded)
                    .try_fold(0_usize, |length, scalar| {
                        length.checked_add(scalar.len_utf8())
                    })
                    .ok_or(IndexError::ResourceLimited)?;
                let fold_memory = budget.reserve_memory(
                    u64::try_from(folded_len).map_err(|_| IndexError::ResourceLimited)?,
                    MemoryLifetime::Retained,
                )?;
                let mut folded = String::new();
                folded
                    .try_reserve_exact(folded_len)
                    .map_err(|_| IndexError::ResourceLimited)?;
                if folded.capacity() > folded_len {
                    return Err(IndexError::ResourceLimited);
                }
                folded.extend(case_chars(&row.value.search_text, CaseMode::Folded));
                let candidate = Candidate {
                    row,
                    folded,
                    score_millis,
                    ordinal,
                    _fold_memory: fold_memory,
                };
                ordinal = ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                sink(candidate)
            })
    }
}

impl RankScanner for Scanner<'_, '_> {
    type Candidate = Candidate;
    type Error = IndexError;
    type Grant = Option<MemoryLease>;

    fn resource_limited() -> Self::Error {
        IndexError::ResourceLimited
    }
    fn reserve_prefix(&mut self, bytes: usize) -> Result<Self::Grant, Self::Error> {
        self.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )
    }
    fn scan(
        &mut self,
        sink: &mut dyn FnMut(Self::Candidate) -> Result<(), Self::Error>,
    ) -> Result<(), Self::Error> {
        let focus = self.focus;
        self.scan_rows(&mut |candidate| {
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

struct FundedPath {
    path: String,
    _memory: Option<MemoryLease>,
}

fn query_rank(
    read: &NativeLegacyRepoMapReadV1,
    request: &NativeRepoMapQuery,
    maps: &dyn PersistentMapPort,
    budget: &mut WorkBudget,
) -> Result<NativeRepoMapRankPage, IndexError> {
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
        u64::try_from(plan_bytes).map_err(|_| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let query_terms = tokenize(&request.query_text);
    let focus_keys: BTreeSet<(&str, RepoMapDocType)> = request
        .focus_subjects
        .iter()
        .map(|focus| (focus.subject_identity.as_str(), focus.subject_doc_type))
        .collect();
    let mut focus_paths = Vec::<FundedPath>::new();
    if !focus_keys.is_empty() {
        focus_paths
            .try_reserve_exact(focus_keys.len())
            .map_err(|_| IndexError::ResourceLimited)?;
        if focus_paths.capacity() > focus_keys.len() {
            return Err(IndexError::ResourceLimited);
        }
        let mut resolved = BTreeSet::<(&str, RepoMapDocType)>::new();
        read.visit_rank_rows(maps, budget, |row, budget| {
            let key = (subject_identity(&row.value.subject), row.value.doc_type);
            if let Some(matched) = focus_keys.iter().find(|candidate| **candidate == key) {
                let _newly_resolved = resolved.insert(*matched);
                let path = row.value.owner_path.as_str();
                let memory = budget.reserve_memory(
                    u64::try_from(path.len()).map_err(|_| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?;
                let mut copied = String::new();
                copied
                    .try_reserve_exact(path.len())
                    .map_err(|_| IndexError::ResourceLimited)?;
                if copied.capacity() > path.len() {
                    return Err(IndexError::ResourceLimited);
                }
                copied.push_str(path);
                focus_paths.push(FundedPath {
                    path: copied,
                    _memory: memory,
                });
            }
            Ok(())
        })?;
        if resolved.len() != focus_keys.len() {
            return Err(IndexError::FocusSubjectNotFound);
        }
    }
    let focus_owner_paths: BTreeSet<&str> =
        focus_paths.iter().map(|path| path.path.as_str()).collect();
    let top_k_limit = usize::try_from(request.top_k).map_err(|_| IndexError::ResourceLimited)?;
    let plan = RankPlan {
        focus_keys: &focus_keys,
        focus_owner_paths: &focus_owner_paths,
        query_terms: &query_terms,
        top_k_limit,
        initial_prefix: top_k_limit,
        token_budget: request.token_budget,
    };
    let mut scanner = Scanner {
        read,
        maps,
        budget,
        focus: (!focus_keys.is_empty()).then_some((&focus_keys, &focus_owner_paths)),
    };
    let mut selection = select(&mut scanner, &plan)?;
    let hit_bytes = selection.included.iter().try_fold(
        selection
            .included
            .len()
            .checked_mul(std::mem::size_of::<NativeRepoMapHit>())
            .ok_or(IndexError::ResourceLimited)?,
        |total, (candidate, _)| {
            total
                .checked_add(candidate.identity().len())
                .and_then(|bytes| bytes.checked_add(candidate.owner_path().len()))
                .ok_or(IndexError::ResourceLimited)
        },
    )?;
    let hits_memory = scanner.budget.reserve_memory(
        u64::try_from(hit_bytes).map_err(|_| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut hits = Vec::new();
    hits.try_reserve_exact(selection.included.len())
        .map_err(|_| IndexError::ResourceLimited)?;
    if hits.capacity() > selection.included.len() {
        return Err(IndexError::ResourceLimited);
    }
    for (candidate, rank) in &selection.included {
        hits.push(NativeRepoMapHit {
            subject: copy_subject(&candidate.row.value.subject)?,
            owner_path: RepoRelativePath::new(copy_string(candidate.owner_path())?),
            final_score_millis: candidate.final_score_millis(),
            rank: *rank,
        });
    }
    let page = NativeRepoMapRankPage {
        hits,
        dropped_entries_count: selection.dropped_entries_count,
        drop_reason_codes: std::mem::take(&mut selection.drop_reason_codes),
        degraded_reason_codes: std::mem::take(&mut selection.degraded_reason_codes),
        _selection_memory: selection.grant.take(),
        _hits_memory: hits_memory,
    };
    drop(selection);
    Ok(page)
}

fn copy_string(source: &str) -> Result<String, IndexError> {
    let mut value = String::new();
    value
        .try_reserve_exact(source.len())
        .map_err(|_| IndexError::ResourceLimited)?;
    if value.capacity() > source.len() {
        return Err(IndexError::ResourceLimited);
    }
    value.push_str(source);
    Ok(value)
}

fn copy_subject(subject: &RepoMapNodeRef) -> Result<RepoMapNodeRef, IndexError> {
    Ok(match subject {
        RepoMapNodeRef::File(id) => RepoMapNodeRef::File(FileId::new(copy_string(id.as_str())?)),
        RepoMapNodeRef::Module(id) => {
            RepoMapNodeRef::Module(RepoMapModuleId::new(copy_string(id.as_str())?))
        }
        RepoMapNodeRef::Symbol(id) => {
            RepoMapNodeRef::Symbol(SymbolId::new(copy_string(id.as_str())?))
        }
        RepoMapNodeRef::Chunk(id) => RepoMapNodeRef::Chunk(ChunkId::new(copy_string(id.as_str())?)),
    })
}
